//! Real macOS writer custody across shell exec, pause, and parent death.

#![cfg(target_os = "macos")]

use cargo_hive_platform::apfs::Apfs;
use cargo_hive_platform::flock::{
    InheritedLock, LockFileIdentity, duplicate_inherited_fd, open_private_file, try_exclusive,
    try_lock_inherited,
};
use cargo_hive_platform::slot::{effective_uid, parent_instance, random_slot_bytes};
use cargo_hive_platform::{Fs, TempDir};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixStream;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const DEADLINE: Duration = Duration::from_secs(10);
const ROLE: &str = "HIVE_SLOT_FENCE_TEST_ROLE";
const FD: &str = "HIVE_SLOT_FENCE_TEST_FD";
const LOCK: &str = "slot.lock";
const CONTROL: &str = "HIVE_SLOT_FENCE_TEST_CONTROL";
const OUTPUT: &str = "writer-output";

#[test]
fn inherited_description_fences_a_writer_after_its_paused_runner_is_killed() {
    exercise_writer(true);
}

#[test]
fn stripped_descriptor_leaves_custody_uncertain_after_the_runner_is_killed() {
    exercise_writer(false);
}

#[test]
fn private_file_reads_refuse_aliases_and_public_permissions() {
    let scratch = scratch();
    let path = scratch.path().join(LOCK);
    let original = create_lock(&path);
    assert_eq!(original.metadata().unwrap().uid(), effective_uid());
    assert!(open_private_file(&path).is_ok());

    let symlink = scratch.path().join("symlink");
    std::os::unix::fs::symlink(&path, &symlink).unwrap();
    assert!(open_private_file(&symlink).is_err());
    let hardlink = scratch.path().join("hardlink");
    fs::hard_link(&path, hardlink).unwrap();
    assert!(open_private_file(&path).is_err());
    fs::remove_file(scratch.path().join("hardlink")).unwrap();

    original
        .set_permissions(fs::Permissions::from_mode(0o644))
        .unwrap();
    assert!(open_private_file(&path).is_err());
    assert!(open_private_file(scratch.path()).is_err());
}

#[test]
fn different_descriptors_are_refused_before_locking() {
    let scratch = scratch();
    let path = scratch.path().join(LOCK);
    let lock = create_lock(&path);
    let identity = LockFileIdentity::from_path(&path).unwrap();
    let wrong = create_lock(&scratch.path().join("unrelated"));
    assert!(try_lock_inherited(wrong.as_fd(), identity).is_err());
    let contender = open_private_file(&path).unwrap();
    let guard = try_lock_inherited(lock.as_fd(), identity).unwrap();
    assert!(matches!(guard, InheritedLock::Acquired(_)));
    assert!(matches!(
        try_lock_inherited(contender.as_fd(), identity).unwrap(),
        InheritedLock::Held
    ));
}

#[test]
fn independent_helpers_see_the_same_parent_instance() {
    let scratch = scratch();
    for report in ["identity-a", "identity-b"] {
        assert!(
            Process::spawn(scratch.path(), report, None, None)
                .wait()
                .success()
        );
    }
    let read_report = |name: &str| {
        let mut record = String::new();
        File::open(scratch.path().join(name))
            .unwrap()
            .take(256)
            .read_to_string(&mut record)
            .unwrap();
        record
    };
    let first = read_report("identity-a");
    assert_eq!(first, read_report("identity-b"));

    let values: Vec<_> = first.split(' ').collect();
    let [pid, boot, seconds, micros] = values.as_slice() else {
        panic!("missing parent instance: {first}");
    };
    assert_eq!(pid.parse::<u32>().unwrap(), std::process::id());
    assert_eq!(boot.len(), 36, "boot session identity is missing");
    let mut spec = cargo_hive_platform::proc::Spec::new("/usr/sbin/sysctl");
    spec.args = ["-n", "kern.bootsessionuuid"].map(Into::into).into();
    let sysctl =
        cargo_hive_platform::proc::output_with_deadline(&spec, DEADLINE, || false).unwrap();
    assert!(sysctl.status.success());
    assert_eq!(*boot, std::str::from_utf8(&sysctl.stdout).unwrap().trim());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!((1..=now).contains(&seconds.parse::<u64>().unwrap()));
    assert!(micros.parse::<u64>().unwrap() < 1_000_000);
}

#[test]
fn slot_entropy_returns_independent_32_byte_values() {
    let first: [u8; 32] = random_slot_bytes().unwrap();
    let second = random_slot_bytes().unwrap();
    assert_ne!(first, second);
}

fn parent_record() -> String {
    let parent = parent_instance().unwrap();
    format!(
        "{} {} {} {}",
        parent.pid, parent.boot_session, parent.start_secs, parent.start_micros,
    )
}

fn exercise_writer(inherit: bool) {
    let scratch = scratch();
    let lock = scratch.path().join(LOCK);
    let contender = create_lock(&lock);
    let (mut writer, writer_endpoint) = UnixStream::pair().unwrap();

    let mut runner = Process::spawn(
        scratch.path(),
        if inherit { "runner" } else { "runner-strips" },
        None,
        Some(&writer_endpoint),
    );
    drop(writer_endpoint);
    writer.set_read_timeout(Some(DEADLINE)).unwrap();
    writer.set_write_timeout(Some(DEADLINE)).unwrap();
    let mut reply = [0];
    writer.read_exact(&mut reply).unwrap();
    assert_eq!(reply, if inherit { *b"I" } else { *b"?" });

    runner.stop();
    assert!(try_exclusive(&contender).unwrap().is_none());
    assert!(!scratch.path().join(OUTPUT).exists());

    runner.0.kill().unwrap();
    assert_eq!(runner.wait().signal(), Some(libc::SIGKILL));

    let contender_guard = try_exclusive(&contender).unwrap();
    assert_eq!(contender_guard.is_none(), inherit);
    writer.write_all(b"W").unwrap();
    writer.read_exact(&mut reply).unwrap();
    assert_eq!(reply, *b"W");
    let mut output = [0; 5];
    File::open(scratch.path().join(OUTPUT))
        .unwrap()
        .read_exact(&mut output)
        .unwrap();
    assert_eq!(&output, b"wrote");

    if inherit {
        assert!(try_exclusive(&contender).unwrap().is_none());
    }
    writer.write_all(b"X").unwrap();
    assert_eq!(writer.read(&mut reply).unwrap(), 0);
    drop(contender_guard);
    poll("released lock", || try_exclusive(&contender).unwrap());
}

fn create_lock(path: &Path) -> File {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap()
}

fn scratch() -> TempDir {
    Apfs::new(PathBuf::new())
        .temp_dir(Path::new("/private/tmp"), "hive-it-slot-fence-")
        .unwrap()
}

fn poll<T>(operation: &str, mut attempt: impl FnMut() -> Option<T>) -> T {
    let until = Instant::now() + DEADLINE;
    loop {
        if let Some(result) = attempt() {
            return result;
        }
        assert!(Instant::now() < until, "timed out waiting for {operation}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct Process(Child);

impl Process {
    fn spawn(
        path: &Path,
        role: &str,
        transfer: Option<(&File, bool)>,
        control: Option<&UnixStream>,
    ) -> Self {
        let mut cmd = Command::new("/bin/bash");
        cmd.args([
            "-c",
            "exec \"$1\" --exact fixture_subprocess --ignored --nocapture",
            "bash",
        ])
        .arg(std::env::current_exe().unwrap())
        .current_dir(path)
        .env_clear()
        .env(ROLE, role)
        .stdin(Stdio::null())
        .stdout(Stdio::null());
        let transfer = transfer.map(|(file, inherit)| {
            let fd = file.as_raw_fd();
            cmd.env(FD, fd.to_string());
            (fd, if inherit { 0 } else { libc::FD_CLOEXEC })
        });
        let control = control.map(|stream| {
            let fd = stream.as_raw_fd();
            cmd.env(CONTROL, fd.to_string());
            (fd, 0)
        });
        // SAFETY: Both descriptors remain open through spawn. The hook only calls fcntl.
        unsafe {
            cmd.pre_exec(move || {
                for (fd, flags) in transfer.into_iter().chain(control) {
                    if libc::fcntl(fd, libc::F_SETFD, flags) == -1 {
                        return Err(io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
        Self(cmd.spawn().unwrap())
    }

    fn wait(&mut self) -> ExitStatus {
        poll("fixture process exit", || self.0.try_wait().unwrap())
    }

    fn stop(&mut self) {
        let pid = self.0.id() as libc::pid_t;
        // SAFETY: The child is still unreaped, so its PID cannot be reused.
        assert_eq!(unsafe { libc::kill(pid, libc::SIGSTOP) }, 0);
        poll("stopped runner", || {
            let mut status = 0;
            // SAFETY: The status is writable and this waits only on the known child.
            let observed =
                unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG | libc::WUNTRACED) };
            assert_ne!(observed, -1, "waitpid: {}", io::Error::last_os_error());
            if observed == 0 {
                return None;
            }
            assert!(libc::WIFSTOPPED(status));
            Some(())
        });
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let until = Instant::now() + DEADLINE;
            while matches!(self.0.try_wait(), Ok(None)) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

#[test]
#[ignore = "launched inside a private fixture by the custody tests"]
fn fixture_subprocess() {
    let root = std::env::current_dir().unwrap();
    assert_eq!(root.parent(), Some(Path::new("/private/tmp")));
    assert!(
        root.file_name()
            .unwrap()
            .as_encoded_bytes()
            .starts_with(b"hive-it-slot-fence-")
    );
    match std::env::var(ROLE).unwrap().as_str() {
        role @ ("runner" | "runner-strips") => {
            let lock = File::open(LOCK).unwrap();
            for _ in 0..2 {
                assert!(
                    Process::spawn(&root, "claim", Some((&lock, true)), None)
                        .wait()
                        .success()
                );
            }
            let control = writer_endpoint();
            let mut writer = Process::spawn(
                &root,
                "writer",
                Some((&lock, role == "runner")),
                Some(&control),
            );
            drop(control);
            assert!(writer.wait().success());
        }
        "claim" => {
            let lock = inherited_file().unwrap();
            let identity = LockFileIdentity::from_path(Path::new(LOCK)).unwrap();
            let InheritedLock::Acquired(_guard) =
                try_lock_inherited(lock.as_fd(), identity).unwrap()
            else {
                panic!("the runner's inherited description can claim the lock");
            };
        }
        "writer" => writer(),
        report @ ("identity-a" | "identity-b") => fs::write(report, parent_record()).unwrap(),
        role => panic!("unknown fixture role {role}"),
    }
}

fn inherited_file() -> Option<File> {
    let fd = std::env::var(FD).unwrap().parse::<RawFd>().unwrap();
    let file = match duplicate_inherited_fd(fd) {
        Ok(owned) => File::from(owned),
        Err(err) if err.raw_os_error() == Some(libc::EBADF) => return None,
        Err(err) => panic!("duplicate fixture lock descriptor: {err}"),
    };
    let inode = file.metadata().unwrap();
    let lock = fs::symlink_metadata(LOCK).unwrap();
    (inode.dev() == lock.dev() && inode.ino() == lock.ino()).then_some(file)
}

fn writer_endpoint() -> UnixStream {
    let fd = std::env::var(CONTROL).unwrap().parse::<RawFd>().unwrap();
    // SAFETY: Process::spawn transfers this socket endpoint exclusively to the new process.
    unsafe { UnixStream::from_raw_fd(fd) }
}

fn writer() {
    let lock = inherited_file();
    let mut control = writer_endpoint();
    control.set_read_timeout(Some(DEADLINE)).unwrap();
    control.set_write_timeout(Some(DEADLINE)).unwrap();
    control
        .write_all(if lock.is_some() { b"I" } else { b"?" })
        .unwrap();
    let mut command = [0];
    if control.read_exact(&mut command).is_err() {
        return;
    }
    assert_eq!(command, *b"W");
    fs::write(OUTPUT, b"wrote").unwrap();
    control.write_all(b"W").unwrap();
    if control.read_exact(&mut command).is_ok() {
        assert_eq!(command, *b"X");
    }
}
