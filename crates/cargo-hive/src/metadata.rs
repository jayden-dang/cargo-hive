//! `cargo metadata` resolve: target directories with caller overrides scrubbed.

use crate::cargo_target::realpath_lossy;
use crate::platform::proc::{self, ChildOutputError, Spec};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const TARGET_VARS: [&str; 3] = [
    "CARGO_TARGET_DIR",
    "CARGO_BUILD_TARGET_DIR",
    "CARGO_BUILD_BUILD_DIR",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resolved {
    pub workspace_root: PathBuf,
    pub target_directory: PathBuf,
    pub build_directory: PathBuf,
}

pub const METADATA_DEADLINE: Duration = Duration::from_secs(60);

/// Exactly `cargo metadata --no-deps --format-version 1 --locked --offline` in
/// `workspace_dir`, with the three target variables and every `GIT_*` removed (F9), and
/// without a toolchain the caller's directory selected.
pub fn resolve(workspace_dir: &Path, cancel: impl FnMut() -> bool) -> Result<Resolved, String> {
    let mut spec = Spec::new("cargo");
    spec.args = [
        "metadata",
        "--no-deps",
        "--format-version",
        "1",
        "--locked",
        "--offline",
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    spec.cwd = Some(workspace_dir.to_path_buf());
    spec.stdin_null = true;
    let parent: Vec<_> = std::env::vars_os().collect();
    crate::env::metadata(&mut spec, &parent);
    let out =
        proc::output_with_deadline(&spec, METADATA_DEADLINE, cancel).map_err(|e| match e {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_deadline_is_sixty_seconds() {
        assert_eq!(METADATA_DEADLINE, Duration::from_secs(60));
    }
}
