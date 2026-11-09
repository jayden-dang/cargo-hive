//! Cargo target dirs: identification by tag and fingerprint layout, and
//! ownership through `cargo metadata` with the caller's target overrides scrubbed (Decision 4).
//! Child runners bound wall time, cancel, and captured stdout/stderr.

use crate::fingerprint::ProfileReport;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

pub const TAG_LINE: &str = "# This file is a cache directory tag created by cargo.";
pub const TARGET_VARS: [&str; 3] = [
    "CARGO_TARGET_DIR",
    "CARGO_BUILD_TARGET_DIR",
    "CARGO_BUILD_BUILD_DIR",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    Fingerprint,
    NewBuildDir,
}

fn is_dir(path: &Path) -> Result<bool, String> {
    match fs::metadata(path) {
        Ok(m) => Ok(m.is_dir()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn is_hex16(name: &str) -> bool {
    name.len() == 16
        && name
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn sorted_dirs(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut dirs = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => dirs.push(entry.path()),
            Ok(_) => {}
            Err(e) => return Err(format!("{}: {e}", entry.path().display())),
        }
    }
    dirs.sort();
    Ok(dirs)
}

type ListDirs<'a> = dyn Fn(&Path, usize) -> Result<Vec<PathBuf>, String> + 'a;
type IsDir<'a> = dyn Fn(&Path) -> Result<bool, String> + 'a;

/// `Some(layout)` when `dir` holds `.fingerprint/` or `build/<pkg>/<16 hex>/fingerprint/`.
fn profile_layout_with(
    dir: &Path,
    cap: usize,
    list: &ListDirs<'_>,
    is_dir: &IsDir<'_>,
) -> Result<Option<Layout>, String> {
    if is_dir(&dir.join(".fingerprint"))? {
        return Ok(Some(Layout::Fingerprint));
    }
    let build = dir.join("build");
    if !is_dir(&build)? {
        return Ok(None);
    }
    for pkg in list(&build, cap)? {
        for hash in list(&pkg, cap)? {
            let hex = hash
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(is_hex16);
            if hex && is_dir(&hash.join("fingerprint"))? {
                return Ok(Some(Layout::NewBuildDir));
            }
        }
    }
    Ok(None)
}

/// Profile dirs: direct children, or `<triple>/<profile>/` under `--target` builds.
fn profile_dirs(target: &Path) -> Result<Vec<(PathBuf, Layout)>, String> {
    profile_dirs_with(target, usize::MAX, &|path, _| sorted_dirs(path), &is_dir)
}

fn profile_dirs_with(
    target: &Path,
    cap: usize,
    list: &ListDirs<'_>,
    is_dir: &IsDir<'_>,
) -> Result<Vec<(PathBuf, Layout)>, String> {
    let mut out = Vec::new();
    for child in list(target, cap)? {
        if let Some(layout) = profile_layout_with(&child, cap, list, is_dir)? {
            out.push((child, layout));
            continue;
        }
        for grandchild in list(&child, cap)? {
            if let Some(layout) = profile_layout_with(&grandchild, cap, list, is_dir)? {
                out.push((grandchild, layout));
            }
        }
    }
    Ok(out)
}

/// `Some(Ok(profiles))` for a Cargo target dir; `Some(Err(reason))` for a dir
/// Cargo tagged whose profiles cannot be read; `None` for anything else.
pub fn identify(dir: &Path) -> Option<Result<Vec<(PathBuf, Layout)>, String>> {
    let tag = fs::read(dir.join("CACHEDIR.TAG")).ok()?;
    if !valid_tag(&tag) {
        return None;
    }
    finish_profiles(profile_dirs(dir))
}

/// Same as [`identify`], but every directory listing stops at `cap` entries.
pub fn identify_bounded(
    fs: &dyn crate::platform::Fs,
    dir: &Path,
    cap: usize,
) -> Option<Result<Vec<(PathBuf, Layout)>, String>> {
    let tag = fs.read(&dir.join("CACHEDIR.TAG")).ok()?;
    if !valid_tag(&tag) {
        return None;
    }
    finish_profiles(profile_dirs_with(
        dir,
        cap,
        &|path, cap| bounded_dirs(fs, path, cap),
        &|path| is_dir_bounded(fs, path),
    ))
}

fn valid_tag(tag: &[u8]) -> bool {
    String::from_utf8_lossy(tag)
        .lines()
        .any(|line| line.trim_end() == TAG_LINE)
}

fn finish_profiles(
    found: Result<Vec<(PathBuf, Layout)>, String>,
) -> Option<Result<Vec<(PathBuf, Layout)>, String>> {
    match found {
        Ok(profiles) if profiles.is_empty() => None,
        Ok(profiles) => Some(Ok(profiles)),
        Err(why) => Some(Err(why)),
    }
}

fn bounded_dirs(
    fs: &dyn crate::platform::Fs,
    dir: &Path,
    cap: usize,
) -> Result<Vec<PathBuf>, String> {
    let entries = fs
        .read_dir_bounded(dir, cap)
        .map_err(|err| format!("{}: {err}", dir.display()))?;
    let mut dirs = Vec::new();
    for (name, kind) in entries {
        if kind == crate::platform::EntryKind::Dir {
            dirs.push(dir.join(name));
        }
    }
    dirs.sort();
    Ok(dirs)
}

fn is_dir_bounded(fs: &dyn crate::platform::Fs, path: &Path) -> Result<bool, String> {
    match fs.entry_stat(path) {
        Ok(stat) => Ok(stat.kind == crate::platform::EntryKind::Dir),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(format!("{}: {err}", path.display())),
    }
}

/// The realpath of `p`, or of its nearest existing ancestor with the missing tail appended.
pub fn realpath_lossy(p: &Path) -> PathBuf {
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    let mut cur = p;
    loop {
        if let Ok(real) = cur.canonicalize() {
            return tail.iter().rev().fold(real, |acc, name| acc.join(name));
        }
        match (cur.parent(), cur.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name);
                cur = parent;
            }
            _ => return p.to_path_buf(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resolved {
    pub workspace_root: PathBuf,
    pub target_directory: PathBuf,
    pub build_directory: PathBuf,
}

pub const METADATA_DEADLINE: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub enum ChildOutputError {
    TimedOut,
    Cancelled,
    Failed(String),
}

impl fmt::Display for ChildOutputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChildOutputError::TimedOut => write!(f, "child process exceeded its deadline"),
            ChildOutputError::Cancelled => write!(f, "child process was cancelled"),
            ChildOutputError::Failed(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ChildOutputError {}

fn kill_pgid(pid: u32) {
    // SAFETY: the child leads its own process group; this signals only that group.
    unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
}

fn kill_group(child: &std::process::Child) {
    kill_pgid(child.id());
}

struct PipeReaders {
    stdout: Option<thread::JoinHandle<Vec<u8>>>,
    stderr: Option<thread::JoinHandle<Vec<u8>>>,
}

impl Drop for PipeReaders {
    fn drop(&mut self) {
        if let Some(handle) = self.stdout.take() {
            let _ = handle.join();
        }
        if let Some(handle) = self.stderr.take() {
            let _ = handle.join();
        }
    }
}

impl PipeReaders {
    fn finish(mut self) -> (Vec<u8>, Vec<u8>) {
        let stdout = self
            .stdout
            .take()
            .and_then(|h| h.join().ok())
            .unwrap_or_default();
        let stderr = self
            .stderr
            .take()
            .and_then(|h| h.join().ok())
            .unwrap_or_default();
        (stdout, stderr)
    }
}

/// Runs `cmd` with piped stdout and stderr. A deadline or a cancel kills the
/// process group. The pipe readers are joined on every return, including expiry.
pub fn output_with_deadline(
    cmd: &mut Command,
    deadline: Duration,
    mut cancel: impl FnMut() -> bool,
) -> Result<Output, ChildOutputError> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| ChildOutputError::Failed(format!("could not spawn: {e}")))?;
    let mut stdout_pipe = child.stdout.take();
    let mut stderr_pipe = child.stderr.take();
    let readers = PipeReaders {
        stdout: Some(thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(pipe) = stdout_pipe.as_mut() {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        })),
        stderr: Some(thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(pipe) = stderr_pipe.as_mut() {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        })),
    };
    let started = Instant::now();
    let status = loop {
        if cancel() {
            kill_group(&child);
            let _ = child.wait();
            return Err(ChildOutputError::Cancelled);
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(retry) if retry.kind() == io::ErrorKind::Interrupted => {
                    if cancel() {
                        kill_group(&child);
                        let _ = child.wait();
                        return Err(ChildOutputError::Cancelled);
                    }
                }
                Err(retry) => {
                    kill_group(&child);
                    let _ = child.wait();
                    return Err(ChildOutputError::Failed(retry.to_string()));
                }
            },
            Err(e) => {
                kill_group(&child);
                let _ = child.wait();
                return Err(ChildOutputError::Failed(e.to_string()));
            }
        }
        if started.elapsed() >= deadline {
            kill_group(&child);
            let _ = child.wait();
            return Err(ChildOutputError::TimedOut);
        }
        thread::sleep(Duration::from_millis(20));
    };
    let (stdout, stderr) = readers.finish();
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

pub struct Capture {
    pub stdout_cap: usize,
    pub stderr_cap: usize,
}

#[derive(Debug)]
pub struct Ran {
    pub status: ExitStatus,
    pub exited_after: Duration,
    pub stdout: Vec<u8>,
    pub stderr_tail: Vec<u8>,
}

/// Runs `cmd` to a deadline with capped stdout (prefix) and stderr (tail). A
/// cancel or the deadline kills the process group. `exited_after` is stamped
/// when `wait` returns, before reader joins.
pub fn run_bounded(
    cmd: &mut Command,
    deadline: Duration,
    capture: Capture,
    mut cancel: impl FnMut() -> bool,
) -> Result<Ran, ChildOutputError> {
    let stdout_stdio = if capture.stdout_cap == 0 {
        Stdio::null()
    } else {
        Stdio::piped()
    };
    let stderr_stdio = if capture.stderr_cap == 0 {
        Stdio::null()
    } else {
        Stdio::piped()
    };
    let start = Instant::now();
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(stdout_stdio)
        .stderr(stderr_stdio)
        .process_group(0)
        .spawn()
        .map_err(|e| ChildOutputError::Failed(format!("could not spawn: {e}")))?;
    let pid = child.id();
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    let stdout_rx = match stdout_pipe {
        Some(mut pipe) => {
            let cap = capture.stdout_cap;
            let (tx, rx) = mpsc::channel();
            thread::spawn(move || {
                let _ = tx.send(read_stdout_prefix(&mut pipe, cap));
            });
            Some(rx)
        }
        None => None,
    };
    let stderr_rx = match stderr_pipe {
        Some(mut pipe) => {
            let cap = capture.stderr_cap;
            let (tx, rx) = mpsc::channel();
            thread::spawn(move || {
                let _ = tx.send(read_stderr_tail(&mut pipe, cap));
            });
            Some(rx)
        }
        None => None,
    };

    let (wait_tx, wait_rx) = mpsc::channel();
    thread::spawn(move || {
        let status = child.wait();
        let exited_after = start.elapsed();
        let _ = wait_tx.send((status, exited_after));
    });

    let slice = Duration::from_millis(20);
    let (status, exited_after) = loop {
        match wait_rx.recv_timeout(slice) {
            Ok((Ok(status), exited_after)) => break (status, exited_after),
            Ok((Err(e), _)) => {
                finish_readers(stdout_rx, stderr_rx);
                return Err(ChildOutputError::Failed(e.to_string()));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if cancel() {
                    kill_pgid(pid);
                    let _ = wait_rx.recv();
                    finish_readers(stdout_rx, stderr_rx);
                    return Err(ChildOutputError::Cancelled);
                }
                if start.elapsed() >= deadline {
                    kill_pgid(pid);
                    let _ = wait_rx.recv();
                    finish_readers(stdout_rx, stderr_rx);
                    return Err(ChildOutputError::TimedOut);
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                finish_readers(stdout_rx, stderr_rx);
                return Err(ChildOutputError::Failed(
                    "waiter thread disconnected before exit".into(),
                ));
            }
        }
    };

    let (stdout, stderr_tail) = finish_readers(stdout_rx, stderr_rx);
    Ok(Ran {
        status,
        exited_after,
        stdout,
        stderr_tail,
    })
}

fn read_stdout_prefix(pipe: &mut impl Read, cap: usize) -> Vec<u8> {
    let mut kept = Vec::with_capacity(cap.min(8192));
    let mut buf = [0u8; 8192];
    loop {
        match pipe.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if kept.len() < cap {
                    let take = (cap - kept.len()).min(n);
                    kept.extend_from_slice(&buf[..take]);
                }
            }
            Err(_) => break,
        }
    }
    kept
}

fn read_stderr_tail(pipe: &mut impl Read, cap: usize) -> Vec<u8> {
    let mut ring = Vec::with_capacity(cap.min(8192));
    let mut buf = [0u8; 8192];
    loop {
        match pipe.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => push_tail(&mut ring, cap, &buf[..n]),
            Err(_) => break,
        }
    }
    ring
}

fn push_tail(ring: &mut Vec<u8>, cap: usize, data: &[u8]) {
    if cap == 0 || data.is_empty() {
        return;
    }
    if data.len() >= cap {
        ring.clear();
        ring.extend_from_slice(&data[data.len() - cap..]);
        return;
    }
    let need = ring.len() + data.len();
    if need > cap {
        ring.drain(..need - cap);
    }
    ring.extend_from_slice(data);
}

fn finish_readers(
    stdout_rx: Option<mpsc::Receiver<Vec<u8>>>,
    stderr_rx: Option<mpsc::Receiver<Vec<u8>>>,
) -> (Vec<u8>, Vec<u8>) {
    let grace = Duration::from_secs(1);
    let stdout = match stdout_rx {
        Some(rx) => rx.recv_timeout(grace).unwrap_or_default(),
        None => Vec::new(),
    };
    let stderr = match stderr_rx {
        Some(rx) => rx.recv_timeout(grace).unwrap_or_default(),
        None => Vec::new(),
    };
    (stdout, stderr)
}

/// Exactly `cargo metadata --no-deps --format-version 1 --locked --offline` in
/// `workspace_dir`, with the three target variables and every `GIT_*` removed (F9), and
/// without a toolchain the caller's directory selected.
pub fn resolve(workspace_dir: &Path, cancel: impl FnMut() -> bool) -> Result<Resolved, String> {
    let mut cmd = Command::new("cargo");
    cmd.args([
        "metadata",
        "--no-deps",
        "--format-version",
        "1",
        "--locked",
        "--offline",
    ])
    .current_dir(workspace_dir)
    .env("RUSTUP_AUTO_INSTALL", "0")
    .stdin(Stdio::null());
    for var in TARGET_VARS {
        cmd.env_remove(var);
    }
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            cmd.env_remove(&key);
        }
    }
    crate::doctor::drop_directory_toolchain(&mut cmd);
    let out = output_with_deadline(&mut cmd, METADATA_DEADLINE, cancel).map_err(|e| match e {
        ChildOutputError::TimedOut => "cargo metadata exceeded its deadline".to_string(),
        ChildOutputError::Cancelled => "cargo metadata was cancelled".to_string(),
        ChildOutputError::Failed(message) => format!("could not run cargo metadata: {message}"),
    })?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "cargo metadata failed: {}",
            stderr
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("no output")
                .trim()
        ));
    }
    let value: serde_json::Value = serde_json::from_slice(&out.stdout)
        .map_err(|e| format!("cargo metadata printed invalid JSON: {e}"))?;
    let path = |key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .map(PathBuf::from)
    };
    let target_directory =
        path("target_directory").ok_or("cargo metadata printed no target_directory")?;
    let build_directory = path("build_directory").unwrap_or_else(|| target_directory.clone());
    let workspace_root =
        path("workspace_root").ok_or("cargo metadata printed no workspace_root")?;
    Ok(Resolved {
        workspace_root: realpath_lossy(&workspace_root),
        target_directory: realpath_lossy(&target_directory),
        build_directory: realpath_lossy(&build_directory),
    })
}

/// The target variables set in hive's own environment, with their realpaths.
pub fn caller_overrides() -> Vec<(String, PathBuf)> {
    TARGET_VARS
        .iter()
        .filter_map(|var| {
            std::env::var_os(var)
                .filter(|v| !v.is_empty())
                .map(|v| (var.to_string(), realpath_lossy(Path::new(&v))))
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Owner {
    Owned {
        checkout: PathBuf,
    },
    Unresolved {
        reason: String,
    },
    OrphanCandidate {
        owned_under_caller_env: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct TargetDir {
    pub path: PathBuf,
    pub owner: Owner,
    pub profiles: Vec<ProfileReport>,
    pub size: Option<u64>,
    pub unreadable: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!(
            "hive-target-{label}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn identify_bounded_matches_identify_at_the_cap_and_errors_one_past() {
        let root = scratch("bounded");
        tag(&root, TAG_LINE);
        fs::create_dir_all(root.join("debug/.fingerprint")).unwrap();
        fs::write(root.join("extra"), b"x").unwrap();
        let n = fs::read_dir(&root).unwrap().count();
        let open = identify(&root).unwrap().unwrap();
        let fs = crate::platform::current().unwrap().fs;
        let capped = identify_bounded(fs.as_ref(), &root, n).unwrap().unwrap();
        assert_eq!(open, capped);
        let err = identify_bounded(fs.as_ref(), &root, n - 1)
            .unwrap()
            .unwrap_err();
        assert!(err.contains("more than"), "{err}");
        assert!(identify(&root).is_some_and(|found| found.is_ok()));
        let _ = fs::remove_dir_all(root);
    }

    fn tag(dir: &Path, line: &str) {
        fs::create_dir_all(dir).unwrap();
        fs::write(
            dir.join("CACHEDIR.TAG"),
            format!("Signature: 8a477f597d28d172789f06886806bc55\n{line}\n"),
        )
        .unwrap();
    }

    fn mkdir(p: PathBuf) {
        fs::create_dir_all(p).unwrap();
    }

    #[test]
    fn a_target_needs_cargos_tag_line_and_a_profile_with_fingerprints() {
        let root = scratch("identify");
        let plain = root.join("plain");
        mkdir(plain.join("debug/.fingerprint"));
        assert_eq!(identify(&plain), None, "no tag");
        let other = root.join("other");
        tag(
            &other,
            "# This file is a cache directory tag created by someone else.",
        );
        mkdir(other.join("debug/.fingerprint"));
        assert_eq!(identify(&other), None, "someone else's tag");
        let artifacts = root.join("artifacts");
        tag(&artifacts, TAG_LINE);
        mkdir(artifacts.join("debug/deps"));
        assert_eq!(
            identify(&artifacts),
            None,
            "uplifted artifacts only (a target dir whose build dir is elsewhere)"
        );
        let old = root.join("old");
        tag(&old, TAG_LINE);
        for p in [
            "debug/.fingerprint",
            "release/.fingerprint",
            "aarch64-apple-darwin/debug/.fingerprint",
            "doc/fixture",
        ] {
            mkdir(old.join(p));
        }
        assert_eq!(
            identify(&old),
            Some(Ok(vec![
                (old.join("aarch64-apple-darwin/debug"), Layout::Fingerprint),
                (old.join("debug"), Layout::Fingerprint),
                (old.join("release"), Layout::Fingerprint)
            ]))
        );
        let new = root.join("new");
        tag(&new, TAG_LINE);
        mkdir(new.join("debug/build/serde/0123456789abcdef/fingerprint"));
        assert_eq!(
            identify(&new),
            Some(Ok(vec![(new.join("debug"), Layout::NewBuildDir)]))
        );
        let not_hash = root.join("not-hash");
        tag(&not_hash, TAG_LINE);
        mkdir(not_hash.join("debug/build/serde-0123/out"));
        assert_eq!(identify(&not_hash), None);
        let locked = root.join("locked");
        tag(&locked, TAG_LINE);
        mkdir(locked.join("debug/.fingerprint"));
        fs::set_permissions(locked.join("debug"), fs::Permissions::from_mode(0o000)).unwrap();
        let got = identify(&locked);
        fs::set_permissions(locked.join("debug"), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            matches!(&got, Some(Err(why)) if why.contains("debug")),
            "{got:?}"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_unreadable_package_dir_is_not_treated_as_not_a_target() {
        let root = scratch("unreadable-pkg");
        let dir = root.join("target");
        tag(&dir, TAG_LINE);
        let pkg = dir.join("debug/build/serde");
        mkdir(pkg.join("0123456789abcdef/fingerprint"));
        fs::set_permissions(&pkg, fs::Permissions::from_mode(0o000)).unwrap();
        let got = identify(&dir);
        fs::set_permissions(&pkg, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(&got, Some(Err(why)) if !why.is_empty()), "{got:?}");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_missing_tail_is_appended_to_the_realpath_of_its_nearest_existing_ancestor() {
        let root = scratch("lossy");
        assert_eq!(
            realpath_lossy(&root.join("a/b")),
            root.canonicalize().unwrap().join("a/b")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_short_deadline_kills_a_sleeping_child() {
        assert_eq!(METADATA_DEADLINE, Duration::from_secs(60));
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let started = Instant::now();
        let err = output_with_deadline(&mut cmd, Duration::from_millis(200), || false)
            .expect_err("a 30s sleep must not finish inside the deadline");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "sleep was still running after {:?}",
            started.elapsed()
        );
        assert!(matches!(err, ChildOutputError::TimedOut), "{err:?}");
    }

    #[test]
    fn a_cancel_kills_the_child_instead_of_waiting_out_the_deadline() {
        let mut cmd = Command::new("sleep");
        cmd.arg("30");
        let started = Instant::now();
        let err = output_with_deadline(&mut cmd, Duration::from_secs(60), || true)
            .expect_err("cancel stops the child");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(matches!(err, ChildOutputError::Cancelled), "{err:?}");
    }

    fn sh(script: &str) -> Command {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", script]);
        cmd
    }

    #[test]
    fn a_child_that_exits_reports_its_status_and_when_it_exited() {
        let ran = run_bounded(
            &mut sh("sleep 0.2; exit 3"),
            Duration::from_secs(10),
            Capture {
                stdout_cap: 0,
                stderr_cap: 1024,
            },
            || false,
        )
        .unwrap();
        assert_eq!(ran.status.code(), Some(3));
        assert!(
            ran.exited_after >= Duration::from_millis(200),
            "{:?}",
            ran.exited_after
        );
        assert!(ran.exited_after < Duration::from_secs(5));
    }

    #[test]
    fn output_past_its_cap_keeps_only_the_cap() {
        let ran = run_bounded(
            &mut sh("head -c 100000 /dev/zero; head -c 100000 /dev/zero >&2"),
            Duration::from_secs(10),
            Capture {
                stdout_cap: 4096,
                stderr_cap: 4096,
            },
            || false,
        )
        .unwrap();
        assert_eq!(ran.stdout.len(), 4096);
        assert_eq!(ran.stderr_tail.len(), 4096);
    }

    #[test]
    fn output_at_its_cap_is_kept_whole() {
        let ran = run_bounded(
            &mut sh("head -c 4096 /dev/zero >&2"),
            Duration::from_secs(10),
            Capture {
                stdout_cap: 0,
                stderr_cap: 4096,
            },
            || false,
        )
        .unwrap();
        assert_eq!(ran.stderr_tail.len(), 4096);
    }

    #[test]
    fn a_child_past_its_deadline_is_killed_with_its_group() {
        let start = Instant::now();
        let err = run_bounded(
            &mut sh("sleep 30 & sleep 30"),
            Duration::from_millis(300),
            Capture {
                stdout_cap: 0,
                stderr_cap: 0,
            },
            || false,
        )
        .unwrap_err();
        assert!(matches!(err, ChildOutputError::TimedOut));
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_cancel_kills_the_child() {
        let start = Instant::now();
        let err = run_bounded(
            &mut sh("sleep 30"),
            Duration::from_secs(60),
            Capture {
                stdout_cap: 0,
                stderr_cap: 0,
            },
            || start.elapsed() > Duration::from_millis(200),
        )
        .unwrap_err();
        assert!(matches!(err, ChildOutputError::Cancelled));
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_grandchild_holding_stderr_open_does_not_delay_the_result() {
        let start = Instant::now();
        let ran = run_bounded(
            &mut sh("(sleep 20 >&2 &) ; exit 0"),
            Duration::from_secs(60),
            Capture {
                stdout_cap: 0,
                stderr_cap: 1024,
            },
            || false,
        )
        .unwrap();
        assert_eq!(ran.status.code(), Some(0));
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
    }

    fn assert_error<E: std::error::Error>(_: &E) {}

    #[test]
    fn child_output_errors_display_what_went_wrong() {
        let cancelled = ChildOutputError::Cancelled;
        let timed_out = ChildOutputError::TimedOut;
        let failed = ChildOutputError::Failed("could not spawn: boom".into());
        assert_error(&cancelled);
        assert_error(&timed_out);
        assert_error(&failed);
        assert_eq!(cancelled.to_string(), "child process was cancelled");
        assert_eq!(timed_out.to_string(), "child process exceeded its deadline");
        assert_eq!(failed.to_string(), "could not spawn: boom");
        assert!(std::error::Error::source(&cancelled).is_none());
    }
}
