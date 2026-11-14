//! Child-process environment rules: one owner for every scrub and allowlist.

use cargo_hive_platform::proc::Spec;
use std::ffi::{OsStr, OsString};
use std::path::Path;

const TARGET_VARS: [&str; 3] = [
    "CARGO_TARGET_DIR",
    "CARGO_BUILD_TARGET_DIR",
    "CARGO_BUILD_BUILD_DIR",
];

/// How rustup/cargo children treat a caller-selected `RUSTUP_TOOLCHAIN`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolveEnv {
    /// Drop a directory-selected toolchain; keep one the caller named (`cli` / `env`).
    DropDirectoryToolchain,
    /// Always drop the caller's toolchain so the child directory can select its own.
    OwnDirectory,
}

/// Whether a toolchain source is one the caller asked for by name.
pub fn caller_toolchain_kept_from(source: Option<&OsStr>) -> bool {
    source.is_some_and(|s| s == "cli" || s == "env")
}

/// `RUSTUP_TOOLCHAIN_SOURCE` from the live process environment.
pub fn caller_toolchain_kept() -> bool {
    caller_toolchain_kept_from(std::env::var_os("RUSTUP_TOOLCHAIN_SOURCE").as_deref())
}

fn source_from_parent(parent: &[(OsString, OsString)]) -> Option<&OsStr> {
    parent
        .iter()
        .find(|(k, _)| k.as_os_str() == "RUSTUP_TOOLCHAIN_SOURCE")
        .map(|(_, v)| v.as_os_str())
}

fn drop_directory_toolchain(spec: &mut Spec, parent: &[(OsString, OsString)]) {
    if !caller_toolchain_kept_from(source_from_parent(parent)) {
        spec.env_remove.push("RUSTUP_TOOLCHAIN".into());
        spec.env_remove.push("RUSTUP_TOOLCHAIN_SOURCE".into());
    }
}

fn apply_resolve_env(spec: &mut Spec, parent: &[(OsString, OsString)], how: ResolveEnv) {
    match how {
        ResolveEnv::DropDirectoryToolchain => drop_directory_toolchain(spec, parent),
        ResolveEnv::OwnDirectory => {
            spec.env_remove.push("RUSTUP_TOOLCHAIN".into());
            spec.env_remove.push("RUSTUP_TOOLCHAIN_SOURCE".into());
        }
    }
}

/// Environment for `cargo metadata`: scrub target overrides and `GIT_*`, and drop a
/// directory-selected toolchain.
pub fn metadata(spec: &mut Spec, parent: &[(OsString, OsString)]) {
    spec.env_set
        .push(("RUSTUP_AUTO_INSTALL".into(), "0".into()));
    for var in TARGET_VARS {
        spec.env_remove.push(var.into());
    }
    git(spec, parent);
    drop_directory_toolchain(spec, parent);
}

/// Environment for `git`: every `GIT_*` variable removed.
pub fn git(spec: &mut Spec, parent: &[(OsString, OsString)]) {
    for (key, _) in parent {
        if key.to_string_lossy().starts_with("GIT_") {
            spec.env_remove.push(key.clone());
        }
    }
}

/// Environment for `rustup which` / resolve children: pin auto-install off and apply
/// the resolve policy.
pub fn rustup_which(spec: &mut Spec, parent: &[(OsString, OsString)], how: ResolveEnv) {
    spec.env_set
        .push(("RUSTUP_AUTO_INSTALL".into(), "0".into()));
    apply_resolve_env(spec, parent, how);
}

/// Allowlisted probe-build environment; `rustup_home` is set only when non-empty.
pub fn probe_build(
    spec: &mut Spec,
    parent: &[(OsString, OsString)],
    cargo_home: &Path,
    rustup_home: &Path,
    toolchain_name: &OsStr,
) {
    spec.env_clear = true;
    for key in ["PATH", "HOME", "TMPDIR"] {
        if let Some((_, value)) = parent.iter().find(|(k, _)| k.as_os_str() == key) {
            spec.env_set.push((OsString::from(key), value.clone()));
        }
    }
    if !rustup_home.as_os_str().is_empty() {
        spec.env_set
            .push(("RUSTUP_HOME".into(), rustup_home.as_os_str().into()));
    }
    if !toolchain_name.is_empty() {
        spec.env_set
            .push(("RUSTUP_TOOLCHAIN".into(), toolchain_name.into()));
    }
    spec.env_set
        .push(("RUSTUP_AUTO_INSTALL".into(), "0".into()));
    spec.env_set
        .push(("CARGO_HOME".into(), cargo_home.as_os_str().into()));
}

/// Timed bench-build environment: clear, filter the parent, pin rustc and wrappers.
pub fn timed_build(
    spec: &mut Spec,
    parent: &[(OsString, OsString)],
    cargo: &Path,
    rustc: &Path,
    toolchain_name: &OsStr,
) {
    const DROP_EXACT: [&str; 8] = [
        "CARGO_MANIFEST_DIR",
        "OUT_DIR",
        "CARGO_BUILD_RUSTC_WRAPPER",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
        "RUSTC",
        "CARGO_BUILD_RUSTC",
        "RUSTUP_TOOLCHAIN",
        "RUSTUP_TOOLCHAIN_SOURCE",
    ];

    spec.program = cargo.as_os_str().into();
    spec.env_clear = true;
    for (key, value) in parent {
        let key_str = key.to_string_lossy();
        let drop = TARGET_VARS.iter().any(|v| *v == key_str.as_ref())
            || key_str.starts_with("GIT_")
            || key_str.starts_with("CARGO_PKG_")
            || DROP_EXACT.iter().any(|v| *v == key_str.as_ref());
        if !drop {
            spec.env_set.push((key.clone(), value.clone()));
        }
    }
    spec.env_set
        .push(("RUSTC_WRAPPER".into(), OsString::from("")));
    spec.env_set
        .push(("RUSTC_WORKSPACE_WRAPPER".into(), OsString::from("")));
    spec.env_set
        .push(("RUSTUP_AUTO_INSTALL".into(), OsString::from("0")));
    spec.env_set
        .push(("RUSTC".into(), rustc.as_os_str().into()));
    if !toolchain_name.is_empty() {
        spec.env_set
            .push(("RUSTUP_TOOLCHAIN".into(), toolchain_name.into()));
    }
}

/// Apply `env_clear` / `env_remove` / `env_set` to `parent`. Never reads the live process env.
#[cfg(test)]
fn child_env(
    spec: &Spec,
    parent: &[(OsString, OsString)],
) -> std::collections::BTreeMap<OsString, OsString> {
    let mut map = if spec.env_clear {
        std::collections::BTreeMap::new()
    } else {
        parent.iter().cloned().collect()
    };
    for (key, value) in &spec.env_set {
        map.insert(key.clone(), value.clone());
    }
    for key in &spec.env_remove {
        map.remove(key);
    }
    map
}

#[cfg(test)]
mod tests {
    use super::{ResolveEnv, child_env, git, metadata, probe_build, rustup_which, timed_build};
    use cargo_hive_platform::proc::Spec;
    use std::collections::BTreeMap;
    use std::ffi::{OsStr, OsString};
    use std::path::Path;

    fn parent(source: &str) -> Vec<(OsString, OsString)> {
        [
            ("CARGO_TARGET_DIR", "/td"),
            ("CARGO_BUILD_TARGET_DIR", "/cbtd"),
            ("CARGO_BUILD_BUILD_DIR", "/cbbd"),
            ("GIT_DIR", "/gd"),
            ("GIT_WORK_TREE", "/gwt"),
            ("RUSTUP_TOOLCHAIN", "x"),
            ("RUSTUP_TOOLCHAIN_SOURCE", source),
            ("HOME", "/home"),
            ("PATH", "/bin"),
        ]
        .into_iter()
        .map(|(k, v)| (OsString::from(k), OsString::from(v)))
        .collect()
    }

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<OsString, OsString> {
        pairs
            .iter()
            .map(|(k, v)| (OsString::from(*k), OsString::from(*v)))
            .collect()
    }

    fn assert_rule(
        rule: &str,
        source: &str,
        apply: impl FnOnce(&mut Spec, &[(OsString, OsString)]),
        expected: BTreeMap<OsString, OsString>,
    ) {
        let parent_env = parent(source);
        let mut spec = Spec::new("tool");
        apply(&mut spec, &parent_env);
        let got = child_env(&spec, &parent_env);
        assert_eq!(got, expected, "rule={rule} source={source}");
    }

    #[test]
    fn each_child_rule_matches_the_pinned_environment() {
        let cargo_home = Path::new("/cargo-home");
        let rustup_home = Path::new("/rustup-home");
        let cargo = Path::new("/tc/bin/cargo");
        let rustc = Path::new("/tc/bin/rustc");
        let toolchain = OsStr::new("stable-aarch64-apple-darwin");

        assert_rule(
            "metadata",
            "directory",
            metadata,
            map(&[
                ("HOME", "/home"),
                ("PATH", "/bin"),
                ("RUSTUP_AUTO_INSTALL", "0"),
            ]),
        );
        assert_rule(
            "metadata",
            "cli",
            metadata,
            map(&[
                ("HOME", "/home"),
                ("PATH", "/bin"),
                ("RUSTUP_AUTO_INSTALL", "0"),
                ("RUSTUP_TOOLCHAIN", "x"),
                ("RUSTUP_TOOLCHAIN_SOURCE", "cli"),
            ]),
        );
        assert_rule(
            "git",
            "directory",
            git,
            map(&[
                ("CARGO_TARGET_DIR", "/td"),
                ("CARGO_BUILD_TARGET_DIR", "/cbtd"),
                ("CARGO_BUILD_BUILD_DIR", "/cbbd"),
                ("HOME", "/home"),
                ("PATH", "/bin"),
                ("RUSTUP_TOOLCHAIN", "x"),
                ("RUSTUP_TOOLCHAIN_SOURCE", "directory"),
            ]),
        );
        assert_rule(
            "git",
            "cli",
            git,
            map(&[
                ("CARGO_TARGET_DIR", "/td"),
                ("CARGO_BUILD_TARGET_DIR", "/cbtd"),
                ("CARGO_BUILD_BUILD_DIR", "/cbbd"),
                ("HOME", "/home"),
                ("PATH", "/bin"),
                ("RUSTUP_TOOLCHAIN", "x"),
                ("RUSTUP_TOOLCHAIN_SOURCE", "cli"),
            ]),
        );
        assert_rule(
            "rustup_which",
            "directory",
            |spec, parent| rustup_which(spec, parent, ResolveEnv::DropDirectoryToolchain),
            map(&[
                ("CARGO_TARGET_DIR", "/td"),
                ("CARGO_BUILD_TARGET_DIR", "/cbtd"),
                ("CARGO_BUILD_BUILD_DIR", "/cbbd"),
                ("GIT_DIR", "/gd"),
                ("GIT_WORK_TREE", "/gwt"),
                ("HOME", "/home"),
                ("PATH", "/bin"),
                ("RUSTUP_AUTO_INSTALL", "0"),
            ]),
        );
        assert_rule(
            "rustup_which",
            "cli",
            |spec, parent| rustup_which(spec, parent, ResolveEnv::DropDirectoryToolchain),
            map(&[
                ("CARGO_TARGET_DIR", "/td"),
                ("CARGO_BUILD_TARGET_DIR", "/cbtd"),
                ("CARGO_BUILD_BUILD_DIR", "/cbbd"),
                ("GIT_DIR", "/gd"),
                ("GIT_WORK_TREE", "/gwt"),
                ("HOME", "/home"),
                ("PATH", "/bin"),
                ("RUSTUP_AUTO_INSTALL", "0"),
                ("RUSTUP_TOOLCHAIN", "x"),
                ("RUSTUP_TOOLCHAIN_SOURCE", "cli"),
            ]),
        );

        for source in ["directory", "cli"] {
            let parent_env = parent(source);
            let mut spec = Spec::new("cargo");
            probe_build(&mut spec, &parent_env, cargo_home, rustup_home, toolchain);
            let got = child_env(&spec, &parent_env);
            let expected = map(&[
                ("PATH", "/bin"),
                ("HOME", "/home"),
                ("RUSTUP_HOME", "/rustup-home"),
                ("RUSTUP_TOOLCHAIN", "stable-aarch64-apple-darwin"),
                ("RUSTUP_AUTO_INSTALL", "0"),
                ("CARGO_HOME", "/cargo-home"),
            ]);
            assert_eq!(got, expected, "rule=probe_build source={source}");
        }

        for source in ["directory", "cli"] {
            let parent_env = parent(source);
            let mut spec = Spec::new("cargo");
            timed_build(&mut spec, &parent_env, cargo, rustc, toolchain);
            let got = child_env(&spec, &parent_env);
            let expected = map(&[
                ("HOME", "/home"),
                ("PATH", "/bin"),
                ("RUSTC_WRAPPER", ""),
                ("RUSTC_WORKSPACE_WRAPPER", ""),
                ("RUSTUP_AUTO_INSTALL", "0"),
                ("RUSTC", "/tc/bin/rustc"),
                ("RUSTUP_TOOLCHAIN", "stable-aarch64-apple-darwin"),
            ]);
            assert_eq!(got, expected, "rule=timed_build source={source}");
        }

        let parent_env = parent("cli");
        let mut spec = Spec::new("rustup");
        rustup_which(&mut spec, &parent_env, ResolveEnv::OwnDirectory);
        let got = child_env(&spec, &parent_env);
        assert!(
            !got.contains_key(OsStr::new("RUSTUP_TOOLCHAIN")),
            "OwnDirectory must drop RUSTUP_TOOLCHAIN"
        );
        assert!(
            !got.contains_key(OsStr::new("RUSTUP_TOOLCHAIN_SOURCE")),
            "OwnDirectory must drop RUSTUP_TOOLCHAIN_SOURCE"
        );
        assert_eq!(
            got.get(OsStr::new("RUSTUP_AUTO_INSTALL"))
                .map(|v| v.as_os_str()),
            Some(OsStr::new("0"))
        );
    }
}
