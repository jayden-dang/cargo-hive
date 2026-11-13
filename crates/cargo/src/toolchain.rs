//! Toolchain resolution: rustup which, rustc -vV, and the caller's keep rules.

use crate::env::ResolveEnv;
use cargo_hive_platform::proc::{self, ChildOutputError, Spec};
use serde::Serialize;
use std::path::{Path, PathBuf};

pub use crate::env::{caller_toolchain_kept, caller_toolchain_kept_from};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Toolchain {
    pub name: Option<String>,
    pub cargo: PathBuf,
    pub rustc: PathBuf,
    pub release: String,
    #[serde(skip)]
    pub version_text: String,
}

/// `NotFound` is `(os error 2)` with the closing parenthesis. `(os error 20)` contains
/// the shorter text and is a different failure.
fn rustup_is_missing(msg: &str) -> bool {
    msg.contains("(os error 2)")
}

fn tool_error(tool: &str, err: ChildOutputError) -> String {
    match err {
        ChildOutputError::TimedOut => {
            format!("{tool} exceeded its deadline")
        }
        ChildOutputError::Cancelled => format!("{tool} was cancelled"),
        ChildOutputError::Failed(msg) => {
            format!("could not run {tool}: {msg}")
        }
    }
}

fn rustup_which(
    rustup: &Path,
    tool: &str,
    cwd: &Path,
    env: ResolveEnv,
    cancel: impl Fn() -> bool + Copy,
) -> Result<Option<PathBuf>, String> {
    let mut spec = Spec::new(rustup);
    spec.args = vec!["which".into(), tool.into()];
    spec.cwd = Some(cwd.to_path_buf());
    spec.stdin_null = true;
    let parent: Vec<_> = std::env::vars_os().collect();
    crate::env::rustup_which(&mut spec, &parent, env);
    match child_output(&spec, cancel) {
        Err(ChildOutputError::Failed(msg)) if rustup_is_missing(&msg) => Ok(None),
        Err(err) => Err(tool_error("rustup", err)),
        Ok(out) if out.status.success() => Ok(Some(PathBuf::from(
            String::from_utf8_lossy(&out.stdout).trim(),
        ))),
        Ok(out) => Err(format!(
            "rustup which {tool} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
    }
}

fn child_output(
    spec: &Spec,
    cancel: impl FnMut() -> bool,
) -> Result<std::process::Output, ChildOutputError> {
    proc::output_with_deadline(spec, crate::metadata::METADATA_DEADLINE, cancel)
}

/// The toolchain a build started in `cwd` would use: rustup's real binaries, or the
/// `cargo` and `rustc` on PATH when rustup is absent.
pub fn resolve_toolchain(cwd: &Path) -> Result<Toolchain, String> {
    resolve_toolchain_with(
        cwd,
        Path::new("rustup"),
        ResolveEnv::DropDirectoryToolchain,
        cargo_hive_platform::signals::cancelled,
    )
}

/// `resolve_toolchain`, stopping its `rustup` and `rustc -vV` runs when `cancel` returns true.
pub fn resolve_toolchain_until(
    cwd: &Path,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Toolchain, String> {
    resolve_toolchain_with(
        cwd,
        Path::new("rustup"),
        ResolveEnv::DropDirectoryToolchain,
        cancel,
    )
}

/// The toolchain `cwd` selects in its own directory, with any caller `RUSTUP_TOOLCHAIN` removed.
pub fn resolve_toolchain_own_dir_until(
    cwd: &Path,
    rustup: &Path,
    cancel: &dyn Fn() -> bool,
) -> Result<Toolchain, String> {
    resolve_toolchain_with(cwd, rustup, ResolveEnv::OwnDirectory, cancel)
}

fn resolve_toolchain_with(
    cwd: &Path,
    rustup: &Path,
    env: ResolveEnv,
    cancel: impl Fn() -> bool + Copy,
) -> Result<Toolchain, String> {
    let (cargo, rustc) = match (
        rustup_which(rustup, "cargo", cwd, env, cancel)?,
        rustup_which(rustup, "rustc", cwd, env, cancel)?,
    ) {
        (Some(c), Some(r)) => (c, r),
        _ => (PathBuf::from("cargo"), PathBuf::from("rustc")),
    };
    // `…/toolchains/<name>/bin/rustc`
    let name = rustc
        .parent()
        .and_then(Path::parent)
        .filter(|dir| dir.parent().is_some_and(|p| p.ends_with("toolchains")))
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().to_string());
    let mut spec = Spec::new(&rustc);
    spec.args.push("-vV".into());
    spec.cwd = Some(cwd.to_path_buf());
    spec.stdin_null = true;
    let parent: Vec<_> = std::env::vars_os().collect();
    if matches!(env, ResolveEnv::OwnDirectory) {
        crate::env::rustup_which(&mut spec, &parent, env);
    } else {
        spec.env_set
            .push(("RUSTUP_AUTO_INSTALL".into(), "0".into()));
    }
    let out = child_output(&spec, cancel).map_err(|err| tool_error("rustc", err))?;
    if !out.status.success() {
        return Err(format!(
            "{} -vV failed: {}",
            rustc.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let version_text = text.to_string();
    let release = text
        .lines()
        .find_map(|l| l.strip_prefix("release: "))
        .map(str::trim)
        .ok_or("rustc -vV printed no release line")?
        .to_string();
    Ok(Toolchain {
        name,
        cargo,
        rustc,
        release,
        version_text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!(
            "hive-toolchain-unit-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn the_active_toolchain_release_comes_from_rustc() {
        let t = resolve_toolchain(&std::env::temp_dir()).unwrap();
        assert!(t.release.starts_with(|c: char| c.is_ascii_digit()), "{t:?}");
    }

    #[test]
    fn an_os_error_20_is_not_a_missing_rustup() {
        let missing = "could not spawn: No such file or directory (os error 2)";
        assert!(rustup_is_missing(missing), "{missing}");
        for code in 20..=29 {
            let msg = format!("could not spawn: Not a directory (os error {code})");
            assert!(!rustup_is_missing(&msg), "{msg}");
        }
    }

    #[test]
    fn a_cancel_that_is_already_true_returns_cancelled_without_the_deadline() {
        let mut spec = Spec::new("sleep");
        spec.args.push("90".into());
        let started = std::time::Instant::now();
        let err = child_output(&spec, || true).unwrap_err();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "still waiting after {:?}",
            started.elapsed()
        );
        assert!(matches!(err, ChildOutputError::Cancelled), "{err:?}");
    }

    #[test]
    fn a_stuck_rustup_returns_an_error_that_names_rustup() {
        let dir = scratch("stuck-rustup");
        let rustup = dir.join("rustup");
        std::fs::write(&rustup, "#!/bin/sh\nsleep 90\nexit 0\n").unwrap();
        std::fs::set_permissions(&rustup, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let started = std::time::Instant::now();
        let err =
            resolve_toolchain_with(&dir, &rustup, ResolveEnv::DropDirectoryToolchain, || false)
                .unwrap_err();
        let elapsed = started.elapsed();
        assert!(
            err.contains("rustup") && err.contains("deadline"),
            "{err} after {elapsed:?}"
        );
        assert!(
            elapsed < std::time::Duration::from_secs(75),
            "rustup was still running after {elapsed:?}: {err}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_resolved_toolchain_keeps_the_whole_version_text() {
        let dir = scratch("version-text");
        let t = resolve_toolchain(&dir).unwrap();
        assert!(t.version_text.starts_with("rustc "), "{:?}", t.version_text);
        assert!(t.version_text.contains("host: "), "{:?}", t.version_text);
        assert!(
            t.version_text.contains(&format!("release: {}", t.release)),
            "release {} missing from {:?}",
            t.release,
            t.version_text
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_named_caller_toolchain_is_kept_and_a_directory_or_default_one_is_not() {
        let cases: &[(&str, bool)] = &[
            ("cli", true),
            ("env", true),
            ("default", false),
            ("toolchain-file", false),
            ("path-override", false),
            ("other", false),
        ];
        for &(source, kept) in cases {
            assert_eq!(
                caller_toolchain_kept_from(Some(std::ffi::OsStr::new(source))),
                kept,
                "{source}"
            );
        }
        assert!(!caller_toolchain_kept_from(None));
    }

    #[test]
    fn own_dir_mode_passes_no_rustup_toolchain_to_its_children() {
        let dir = scratch("own-dir-env");
        let log = dir.join("toolchain-env");
        let rustc = dir.join("fake-rustc");
        let rustup = dir.join("fake-rustup");
        std::fs::write(
            &rustc,
            "#!/bin/sh\nprintf 'rustc 1.99.0\\nrelease: 1.99.0\\nhost: aarch64-apple-darwin\\n'\n",
        )
        .unwrap();
        std::fs::write(
            &rustup,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"${{RUSTUP_TOOLCHAIN-unset}}\" > '{}'\nprintf '%s\\n' '{}'\n",
                log.display(),
                rustc.display()
            ),
        )
        .unwrap();
        for path in [&rustup, &rustc] {
            std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o755))
                .unwrap();
        }
        let parent_had = std::env::var_os("RUSTUP_TOOLCHAIN");
        let got = resolve_toolchain_with(&dir, &rustup, ResolveEnv::OwnDirectory, || false)
            .expect("own-directory resolve against the fake rustup");
        assert_eq!(got.release, "1.99.0");
        let logged = std::fs::read_to_string(&log).unwrap();
        assert_eq!(
            logged.trim(),
            "unset",
            "child saw RUSTUP_TOOLCHAIN={logged:?}; parent had {parent_had:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
