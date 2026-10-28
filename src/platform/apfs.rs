//! macOS `Fs`: APFS clones and sharing attributes through `libc`.

use super::{
    container_key, CompressOutcome, EntryKind, EntryStat, Fs, HeldLock, LockHolder, LockKind,
    LockState, Sharing, TempDir, TryLock, VolumeInfo, CHILD_DEADLINE,
};
use applesauce::compressor::Kind;
use applesauce::progress::{Progress, SkipReason, Task};
use applesauce::FileCompressor;
use std::ffi::{CStr, CString, OsString};
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::os::macos::fs::MetadataExt as _;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

/// `<sys/attr.h>` extended flags that `libc` does not define.
const EF_MAY_SHARE_BLOCKS: u64 = 0x1;
const EF_SHARES_ALL_BLOCKS: u64 = 0x40;

pub struct Apfs {
    holder: PathBuf,
}

impl Apfs {
    pub fn new(holder: PathBuf) -> Apfs {
        Apfs { holder }
    }

    /// The executable that implements the hidden `__hold-lock` command.
    pub fn holder(&self) -> &Path {
        &self.holder
    }
}

fn c_path(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains a NUL byte"))
}

impl Fs for Apfs {
    fn temp_dir(&self, parent: &Path, prefix: &str) -> io::Result<TempDir> {
        let mut template = c_path(&parent.join(format!("{prefix}XXXXXXXX")))?.into_bytes_with_nul();
        // SAFETY: `template` is a writable NUL-terminated buffer that mkdtemp fills in place.
        if unsafe { libc::mkdtemp(template.as_mut_ptr().cast()) }.is_null() {
            return Err(io::Error::last_os_error());
        }
        template.pop();
        Ok(TempDir::adopt(PathBuf::from(OsString::from_vec(template))))
    }

    fn write_file(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        fs::write(path, bytes)
    }

    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        fs::read(path)
    }

    fn hardlink(&self, src: &Path, dst: &Path) -> io::Result<()> {
        fs::hard_link(src, dst)
    }

    fn set_mtime(&self, path: &Path, t: SystemTime) -> io::Result<()> {
        fs::File::options().write(true).open(path)?.set_modified(t)
    }

    fn mtime(&self, path: &Path) -> io::Result<SystemTime> {
        fs::metadata(path)?.modified()
    }

    fn inode(&self, path: &Path) -> io::Result<u64> {
        Ok(fs::metadata(path)?.ino())
    }

    fn clone_file(&self, src: &Path, dst: &Path) -> io::Result<()> {
        let (s, d) = (c_path(src)?, c_path(dst)?);
        // SAFETY: both are valid NUL-terminated paths for the duration of the call.
        if unsafe { libc::clonefile(s.as_ptr(), d.as_ptr(), 0) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn sharing(&self, path: &Path) -> io::Result<Sharing> {
        let p = c_path(path)?;
        let mut list = libc::attrlist {
            bitmapcount: libc::ATTR_BIT_MAP_COUNT,
            reserved: 0,
            commonattr: 0,
            volattr: 0,
            dirattr: 0,
            fileattr: 0,
            // With FSOPT_ATTR_CMN_EXTENDED this group carries the common-extended attributes.
            forkattr: libc::ATTR_CMNEXT_PRIVATESIZE
                | libc::ATTR_CMNEXT_CLONEID
                | libc::ATTR_CMNEXT_EXT_FLAGS,
        };
        let mut buf = [0u8; 64];
        // SAFETY: `list` and `buf` outlive the call; the kernel writes at most `buf.len()` bytes.
        let rc = unsafe {
            libc::getattrlist(
                p.as_ptr(),
                (&mut list as *mut libc::attrlist).cast(),
                buf.as_mut_ptr().cast(),
                buf.len(),
                libc::FSOPT_ATTR_CMN_EXTENDED | libc::FSOPT_NOFOLLOW,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        // A u32 total length, then the attributes in bit order: PRIVATESIZE (off_t),
        // CLONEID (u64), EXT_FLAGS (u64).
        let word =
            |range: std::ops::Range<usize>| -> [u8; 8] { buf[range].try_into().expect("8 bytes") };
        let len = u32::from_ne_bytes(buf[0..4].try_into().expect("4 bytes"));
        if len < 28 {
            return Err(io::Error::other(format!(
                "getattrlist returned {len} bytes; expected 28"
            )));
        }
        let private = i64::from_ne_bytes(word(4..12));
        let flags = u64::from_ne_bytes(word(20..28));
        Ok(Sharing {
            private_bytes: private_byte_count(private)?,
            clone_id: u64::from_ne_bytes(word(12..20)),
            may_share: flags & EF_MAY_SHARE_BLOCKS != 0,
            shares_all: flags & EF_SHARES_ALL_BLOCKS != 0,
        })
    }

    fn compress(&self, path: &Path) -> CompressOutcome {
        let watch = Watch::default();
        let mut compressor = FileCompressor::new();
        let _stats = compressor.recursive_compress([path], Kind::Lzfse, 0.95, 5, &watch, true);
        if let Some(why) = watch
            .failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            return CompressOutcome::Failed(why);
        }
        if watch.hard_link.load(Ordering::SeqCst) {
            return CompressOutcome::SkippedHardLink;
        }
        match fs::symlink_metadata(path) {
            Ok(m) if m.st_flags() & libc::UF_COMPRESSED != 0 => CompressOutcome::Compressed,
            Ok(_) => {
                CompressOutcome::Failed("the file is not marked UF_COMPRESSED afterwards".into())
            }
            Err(e) => CompressOutcome::Failed(e.to_string()),
        }
    }

    fn hold_lock(&self, kind: LockKind, path: &Path) -> io::Result<LockHolder> {
        let mut child = Command::new(&self.holder)
            .arg("__hold-lock")
            .arg(kind.as_arg())
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let line = first_line_within(&mut child, CHILD_DEADLINE);
        if !matches!(&line, Ok(l) if l.trim() == "held") {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other(format!(
                "the lock holder did not take the lock ({line:?})"
            )));
        }
        Ok(LockHolder::new(move || {
            let _ = child.kill();
            let _ = child.wait();
        }))
    }

    fn try_lock_exclusive(&self, path: &Path) -> TryLock {
        let p = match c_path(path) {
            Ok(p) => p,
            Err(e) => return TryLock::Unopenable(e),
        };
        // SAFETY: `p` is a valid NUL-terminated path for the duration of the call.
        let fd = unsafe {
            libc::open(
                p.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            let err = io::Error::last_os_error();
            return if err.kind() == io::ErrorKind::NotFound {
                TryLock::Gone
            } else {
                TryLock::Unopenable(err)
            };
        }
        // SAFETY: `fd` was just opened by this process and nothing else owns it.
        let file = unsafe { fs::File::from_raw_fd(fd) };
        match file.metadata() {
            Ok(m) if m.file_type().is_file() => {}
            Ok(_) => {
                return TryLock::Unopenable(io::Error::other("the lock path is not a regular file"))
            }
            Err(e) => return TryLock::Unopenable(e),
        }
        // SAFETY: `file` owns a valid descriptor for the call.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let err = io::Error::last_os_error();
            return if err.raw_os_error() == Some(libc::EWOULDBLOCK) {
                TryLock::Held
            } else {
                TryLock::Unlockable(err)
            };
        }
        TryLock::Acquired(HeldLock::new(move || drop(file)))
    }

    fn try_lock_elsewhere(&self, kind: LockKind, path: &Path) -> io::Result<LockState> {
        let mut child = Command::new(&self.holder)
            .arg("__try-lock")
            .arg(kind.as_arg())
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let line = first_line_within(&mut child, CHILD_DEADLINE);
        let _ = child.kill();
        let _ = child.wait();
        match line?.trim() {
            "acquired" => Ok(LockState::Free),
            "refused" => Ok(LockState::Held),
            other => Err(io::Error::other(format!(
                "the lock child answered {other:?}"
            ))),
        }
    }

    fn entry_stat(&self, path: &Path) -> io::Result<EntryStat> {
        let m = fs::symlink_metadata(path)?;
        let t = m.file_type();
        let kind = if t.is_symlink() {
            EntryKind::Symlink
        } else if t.is_dir() {
            EntryKind::Dir
        } else if t.is_file() {
            EntryKind::File
        } else {
            EntryKind::Other
        };
        Ok(EntryStat {
            dev: m.dev(),
            ino: m.ino(),
            nlink: m.nlink(),
            blocks: m.blocks(),
            kind,
            compressed: m.st_flags() & libc::UF_COMPRESSED != 0,
        })
    }

    fn read_dir_bounded(&self, dir: &Path, cap: usize) -> io::Result<Vec<(String, EntryKind)>> {
        let mut out = Vec::new();
        for (seen, entry) in fs::read_dir(dir)?.enumerate() {
            if seen >= cap {
                return Err(io::Error::other(format!(
                    "{} has more than {cap} entries",
                    dir.display()
                )));
            }
            let entry = entry?;
            let t = entry.file_type()?;
            let kind = if t.is_symlink() {
                EntryKind::Symlink
            } else if t.is_dir() {
                EntryKind::Dir
            } else if t.is_file() {
                EntryKind::File
            } else {
                EntryKind::Other
            };
            if let Ok(name) = entry.file_name().into_string() {
                out.push((name, kind));
            }
        }
        Ok(out)
    }

    fn remove_tree(&self, path: &Path) -> io::Result<()> {
        // std's remove_dir_all opens every level with O_NOFOLLOW and unlinks a symlink root.
        fs::remove_dir_all(path)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }

    fn volume(&self, path: &Path) -> io::Result<VolumeInfo> {
        let p = c_path(path)?;
        // SAFETY: an all-zero `statfs` struct is valid output space.
        let mut s: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: `p` is NUL-terminated and `s` is writable; both outlive the call.
        if unsafe { libc::statfs(p.as_ptr(), &mut s) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the kernel NUL-terminates these fixed-size name fields.
        let text = |field: &[libc::c_char]| {
            unsafe { CStr::from_ptr(field.as_ptr()) }
                .to_string_lossy()
                .into_owned()
        };
        let (fs_type, from, on) = (
            text(&s.f_fstypename),
            text(&s.f_mntfromname),
            text(&s.f_mntonname),
        );
        Ok(VolumeInfo {
            container: container_key(&from).unwrap_or_else(|| format!("mount:{on}")),
            fs_type,
            mount: PathBuf::from(on),
            available_bytes: s.f_bavail.saturating_mul(u64::from(s.f_bsize)),
        })
    }
}

fn record(slot: &Mutex<Option<String>>, why: String) {
    slot.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_or_insert(why);
}

/// Turns Applesauce's progress callbacks into one outcome for one file.
#[derive(Default)]
struct Watch {
    hard_link: AtomicBool,
    failure: Arc<Mutex<Option<String>>>,
}

struct WatchTask {
    failure: Arc<Mutex<Option<String>>>,
}

impl Progress for Watch {
    type Task = WatchTask;

    fn error(&self, path: &Path, message: &str) {
        record(&self.failure, format!("{}: {message}", path.display()));
    }

    fn file_skipped(&self, _path: &Path, why: SkipReason) {
        match why {
            SkipReason::HardLink => self.hard_link.store(true, Ordering::SeqCst),
            other => record(&self.failure, format!("skipped: {other}")),
        }
    }

    fn file_task(&self, _path: &Path, _size: u64) -> WatchTask {
        WatchTask {
            failure: Arc::clone(&self.failure),
        }
    }
}

impl Task for WatchTask {
    fn increment(&self, _amt: u64) {}

    fn error(&self, message: &str) {
        record(&self.failure, message.to_string());
    }

    fn not_compressible_enough(&self, _path: &Path) {
        record(&self.failure, "not compressible enough".to_string());
    }
}

/// A negative private-byte count from getattrlist is not a measurement of zero.
pub fn private_byte_count(private: i64) -> io::Result<u64> {
    u64::try_from(private)
        .map_err(|_| io::Error::other(format!("private byte count is negative: {private}")))
}

/// The first stdout line of `child`, or an error once `limit` passes. The caller kills the child.
fn first_line_within(child: &mut Child, limit: Duration) -> io::Result<String> {
    let out = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("the lock child has no stdout"))?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = tx.send(BufReader::new(out).read_line(&mut line).map(|_| line));
    });
    match rx.recv_timeout(limit) {
        Ok(read) => read,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("the lock child did not answer within {limit:?}"),
        )),
    }
}

/// Body of `__try-lock`: one non-blocking attempt, released when this returns.
pub fn try_lock_once(kind: LockKind, path: &Path) -> io::Result<LockState> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)?;
    let fd = file.as_raw_fd();
    let rc = match kind {
        // SAFETY: `fd` is valid while `file` lives.
        LockKind::Flock => unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) },
        LockKind::Fcntl | LockKind::FcntlRead => {
            // SAFETY: an all-zero `flock` struct is valid; the fields that matter are set next.
            let mut lock: libc::flock = unsafe { std::mem::zeroed() };
            lock.l_type = (if kind == LockKind::FcntlRead {
                libc::F_RDLCK
            } else {
                libc::F_WRLCK
            }) as libc::c_short;
            lock.l_whence = libc::SEEK_SET as libc::c_short;
            // SAFETY: `lock` outlives the call and `fd` is valid.
            unsafe { libc::fcntl(fd, libc::F_SETLK, &lock) }
        }
    };
    if rc == 0 {
        return Ok(LockState::Free);
    }
    let err = io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::EWOULDBLOCK) | Some(libc::EACCES) => Ok(LockState::Held),
        _ => Err(err),
    }
}

/// Body of `__hold-lock`: take the lock (blocking), report it, then wait to be killed.
/// The file must already exist; this never creates one.
pub fn hold_lock_forever(kind: LockKind, path: &Path) -> io::Result<std::convert::Infallible> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)?;
    let fd = file.as_raw_fd();
    let rc = match kind {
        // SAFETY: `fd` is valid while `file` lives, which is forever below.
        LockKind::Flock => unsafe { libc::flock(fd, libc::LOCK_EX) },
        LockKind::Fcntl | LockKind::FcntlRead => {
            // SAFETY: an all-zero `flock` struct is valid; the fields that matter are set next.
            let mut lock: libc::flock = unsafe { std::mem::zeroed() };
            lock.l_type = (if kind == LockKind::FcntlRead {
                libc::F_RDLCK
            } else {
                libc::F_WRLCK
            }) as libc::c_short;
            lock.l_whence = libc::SEEK_SET as libc::c_short;
            // SAFETY: `lock` outlives the call and `fd` is valid.
            unsafe { libc::fcntl(fd, libc::F_SETLKW, &lock) }
        }
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    let mut out = io::stdout().lock();
    writeln!(out, "held")?;
    out.flush()?;
    drop(out);
    loop {
        std::thread::park();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::CompressOutcome;
    use std::fs;
    use std::time::{Duration, UNIX_EPOCH};

    fn apfs() -> Apfs {
        Apfs::new(PathBuf::from("/nonexistent/cargo-hive"))
    }

    fn tmp(label: &str) -> TempDir {
        apfs()
            .temp_dir(Path::new("/private/tmp"), &format!("hive-apfs-{label}-"))
            .unwrap()
    }

    #[test]
    fn a_temp_dir_is_created_with_its_prefix_and_removed_on_drop() {
        let t = tmp("drop");
        let path = t.path().to_path_buf();
        assert!(path.is_dir());
        assert!(path
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("hive-apfs-drop-"));
        apfs().write_file(&path.join("f"), b"x").unwrap();
        drop(t);
        assert!(!path.exists());
    }

    #[test]
    fn a_clone_shares_every_block_until_it_is_rewritten() {
        let t = tmp("clone");
        let (src, dst) = (t.path().join("src"), t.path().join("dst"));
        apfs().write_file(&src, &vec![7u8; 1 << 20]).unwrap();
        let alone = apfs().sharing(&src).unwrap();
        assert!(!alone.shares_all && !alone.may_share);
        apfs().clone_file(&src, &dst).unwrap();
        let (s, d) = (apfs().sharing(&src).unwrap(), apfs().sharing(&dst).unwrap());
        assert_eq!((s.private_bytes, d.private_bytes), (0, 0));
        assert!(d.shares_all && d.may_share);
        assert_eq!(s.clone_id, d.clone_id);
        apfs().write_file(&dst, &vec![9u8; 1 << 20]).unwrap();
        let rewritten = apfs().sharing(&dst).unwrap();
        assert_ne!(rewritten.clone_id, s.clone_id);
        assert!(!rewritten.shares_all);
        assert_eq!(apfs().read(&dst).unwrap()[0], 9);
    }

    #[test]
    fn a_negative_private_byte_count_is_an_error() {
        let err = private_byte_count(-1).unwrap_err();
        assert!(err.to_string().contains("-1"), "{err}");
    }

    #[test]
    fn holding_a_lock_does_not_create_the_file() {
        let t = tmp("hold-missing");
        let path = t.path().join("missing.lock");
        let (tx, rx) = std::sync::mpsc::channel();
        let probe = path.clone();
        std::thread::spawn(move || {
            let result = hold_lock_forever(LockKind::Flock, &probe);
            let _ = tx.send(result.err().map(|e| e.kind()));
        });
        let got = rx.recv_timeout(Duration::from_millis(500));
        let created = path.exists();
        assert!(!created, "hold-lock created a missing file");
        assert_eq!(got.ok().flatten(), Some(io::ErrorKind::NotFound));
    }

    #[test]
    fn hardlinks_share_an_inode_and_mtimes_can_be_set() {
        let t = tmp("links");
        let (a, b) = (t.path().join("a"), t.path().join("b"));
        apfs().write_file(&a, b"same").unwrap();
        apfs().hardlink(&a, &b).unwrap();
        assert_eq!(apfs().inode(&a).unwrap(), apfs().inode(&b).unwrap());
        let fixed = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        apfs().set_mtime(&a, fixed).unwrap();
        assert_eq!(apfs().mtime(&b).unwrap(), fixed);
    }

    #[test]
    fn compression_keeps_content_and_mtime_and_skips_hardlinked_files() {
        let t = tmp("compress");
        let content: Vec<u8> = (0..4 << 20).map(|i| (i % 251) as u8).collect();
        let fixed = UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        let single = t.path().join("single");
        apfs().write_file(&single, &content).unwrap();
        apfs().set_mtime(&single, fixed).unwrap();
        assert_eq!(apfs().compress(&single), CompressOutcome::Compressed);
        assert_eq!(apfs().read(&single).unwrap(), content);
        assert_eq!(apfs().mtime(&single).unwrap(), fixed);

        let (a, b) = (t.path().join("a"), t.path().join("b"));
        apfs().write_file(&a, &content[..1 << 20]).unwrap();
        apfs().hardlink(&a, &b).unwrap();
        apfs().set_mtime(&a, fixed).unwrap();
        let inode = apfs().inode(&a).unwrap();
        assert_eq!(apfs().compress(&a), CompressOutcome::SkippedHardLink);
        assert_eq!(apfs().inode(&a).unwrap(), inode);
        assert_eq!(apfs().read(&a).unwrap(), &content[..1 << 20]);
        assert_eq!(apfs().mtime(&a).unwrap(), fixed);
        assert_eq!(
            fs::symlink_metadata(&a).unwrap().st_flags() & libc::UF_COMPRESSED,
            0
        );
    }

    use crate::platform::EntryKind;

    #[test]
    fn entry_stat_never_follows_a_link_and_sees_links_and_compression() {
        let t = tmp("stat");
        let (a, b, link) = (
            t.path().join("a"),
            t.path().join("b"),
            t.path().join("link"),
        );
        apfs().write_file(&a, &vec![3u8; 8192]).unwrap();
        let one = apfs().entry_stat(&a).unwrap();
        assert_eq!(
            (one.kind, one.nlink, one.compressed),
            (EntryKind::File, 1, false)
        );
        apfs().hardlink(&a, &b).unwrap();
        let two = apfs().entry_stat(&b).unwrap();
        assert_eq!((two.ino, two.nlink), (one.ino, 2));
        std::os::unix::fs::symlink(&a, &link).unwrap();
        assert_eq!(apfs().entry_stat(&link).unwrap().kind, EntryKind::Symlink);
        assert_eq!(apfs().entry_stat(t.path()).unwrap().kind, EntryKind::Dir);
        let packed = t.path().join("packed");
        let content: Vec<u8> = (0..4 << 20).map(|i| (i % 251) as u8).collect();
        apfs().write_file(&packed, &content).unwrap();
        assert_eq!(apfs().compress(&packed), CompressOutcome::Compressed);
        assert!(apfs().entry_stat(&packed).unwrap().compressed);
    }

    #[test]
    fn a_bounded_listing_fails_one_entry_past_its_cap() {
        let t = tmp("list");
        for name in ["x", "y"] {
            apfs().write_file(&t.path().join(name), b"").unwrap();
        }
        std::os::unix::fs::symlink("x", t.path().join("z")).unwrap();
        let mut listed = apfs().read_dir_bounded(t.path(), 3).unwrap();
        listed.sort();
        assert_eq!(
            listed,
            vec![
                ("x".into(), EntryKind::File),
                ("y".into(), EntryKind::File),
                ("z".into(), EntryKind::Symlink)
            ]
        );
        assert!(apfs().read_dir_bounded(t.path(), 2).is_err());
    }

    #[test]
    fn removing_a_tree_never_follows_a_link_out_of_it() {
        let t = tmp("remove");
        let outside = t.path().join("outside");
        apfs().write_file(&outside, b"keep").unwrap();
        let tree = t.path().join("tree");
        std::fs::create_dir_all(tree.join("sub")).unwrap();
        std::os::unix::fs::symlink(&outside, tree.join("sub/link")).unwrap();
        std::os::unix::fs::symlink(t.path(), tree.join("up")).unwrap();
        apfs().remove_tree(&tree).unwrap();
        assert!(!tree.exists());
        assert_eq!(apfs().read(&outside).unwrap(), b"keep");
        let root_link = t.path().join("root-link");
        std::os::unix::fs::symlink(&outside, &root_link).unwrap();
        apfs().remove_tree(&root_link).unwrap();
        assert!(outside.exists() && std::fs::symlink_metadata(&root_link).is_err());
    }

    #[test]
    fn the_scratch_volume_is_apfs_with_a_disk_container() {
        let v = apfs().volume(Path::new("/private/tmp")).unwrap();
        assert_eq!(v.fs_type, "apfs");
        assert!(v.container.starts_with("disk"), "{v:?}");
        assert!(v.available_bytes > 0);
    }

    #[test]
    fn sharing_does_not_follow_a_final_symlink() {
        let t = tmp("share-link");
        let a = t.path().join("a");
        apfs().write_file(&a, &vec![1u8; 1 << 20]).unwrap();
        let link = t.path().join("link");
        std::os::unix::fs::symlink(&a, &link).unwrap();
        // With FSOPT_NOFOLLOW the call describes the link itself (or refuses it), never the 1 MiB target.
        if let Ok(s) = apfs().sharing(&link) {
            assert_ne!(s.private_bytes, 1 << 20);
        }
    }
}
