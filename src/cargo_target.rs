//! Cargo target dirs: identification by tag and fingerprint layout, and
//! ownership through `cargo metadata` with the caller's target overrides scrubbed (Decision 4).

use crate::fingerprint::ProfileReport;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
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

/// `Some(layout)` when `dir` holds `.fingerprint/` or `build/<pkg>/<16 hex>/fingerprint/`.
fn profile_layout(dir: &Path) -> Result<Option<Layout>, String> {
    if is_dir(&dir.join(".fingerprint"))? {
        return Ok(Some(Layout::Fingerprint));
    }
    let build = dir.join("build");
    if !is_dir(&build)? {
        return Ok(None);
    }
    for pkg in sorted_dirs(&build)? {
        for hash in sorted_dirs(&pkg)? {
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
    let mut out = Vec::new();
    for child in sorted_dirs(target)? {
        if let Some(layout) = profile_layout(&child)? {
            out.push((child, layout));
            continue;
        }
        for grandchild in sorted_dirs(&child)? {
            if let Some(layout) = profile_layout(&grandchild)? {
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
    if !String::from_utf8_lossy(&tag)
        .lines()
        .any(|l| l.trim_end() == TAG_LINE)
    {
        return None;
    }
    match profile_dirs(dir) {
        Ok(profiles) if profiles.is_empty() => None,
        Ok(profiles) => Some(Ok(profiles)),
        Err(why) => Some(Err(why)),
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

fn kill_group(child: &std::process::Child) {
    // SAFETY: the child leads its own process group; this signals only that group.
    unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
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
}
