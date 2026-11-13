//! Compile-only cargo shapes for the timed bench build, always offline and locked.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;
use std::str::FromStr;

use crate::doctor::Toolchain;
use crate::platform::proc::Spec;
use std::ffi::OsStr;

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum BuildCmd {
    #[default]
    Build,
    CheckAll,
    ClippyAll,
    TestNoRun,
    CheckLib(String),
}

impl FromStr for BuildCmd {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err =
            || "expected build, check-all, clippy-all, test-no-run, or check-lib=<package>".into();
        match s {
            "build" => Ok(Self::Build),
            "check-all" => Ok(Self::CheckAll),
            "clippy-all" => Ok(Self::ClippyAll),
            "test-no-run" => Ok(Self::TestNoRun),
            other => {
                let Some(package) = other.strip_prefix("check-lib=") else {
                    return Err(err());
                };
                if package.is_empty()
                    || package.starts_with('-')
                    || !package
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                {
                    return Err(err());
                }
                Ok(Self::CheckLib(package.to_owned()))
            }
        }
    }
}

impl BuildCmd {
    pub fn args(&self) -> Vec<String> {
        let mut args = match self {
            Self::Build => vec!["build".into()],
            Self::CheckAll => vec!["check".into(), "--all-targets".into()],
            Self::ClippyAll => vec!["clippy".into(), "--all-targets".into()],
            Self::TestNoRun => vec!["test".into(), "--no-run".into()],
            Self::CheckLib(package) => {
                vec!["check".into(), "-p".into(), package.clone(), "--lib".into()]
            }
        };
        args.extend(["--offline".into(), "--locked".into()]);
        args
    }

    pub fn command_line(&self) -> String {
        format!("cargo {}", self.args().join(" "))
    }
}

pub struct EnvMap {
    vars: BTreeMap<OsString, OsString>,
    overridden: Vec<(&'static str, String, String)>,
}

pub fn env_map(caller: impl IntoIterator<Item = (OsString, OsString)>, tc: &Toolchain) -> EnvMap {
    const WRAPPERS: [&str; 4] = [
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
        "CARGO_BUILD_RUSTC_WRAPPER",
        "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
    ];

    let parent: Vec<(OsString, OsString)> = caller.into_iter().collect();
    let mut overridden = Vec::new();

    for (key, value) in &parent {
        let key_str = key.to_string_lossy();
        if WRAPPERS.iter().any(|w| *w == key_str.as_ref()) && !value.is_empty() {
            overridden.push((
                "env.wrapper.overridden",
                key_str.to_string(),
                value.to_string_lossy().into_owned(),
            ));
        }
        if key_str == "RUSTC" || key_str == "CARGO_BUILD_RUSTC" {
            overridden.push((
                "env.rustc.overridden",
                key_str.to_string(),
                value.to_string_lossy().into_owned(),
            ));
        }
    }

    let mut spec = Spec::new(&tc.cargo);
    let name = tc
        .name
        .as_deref()
        .map(OsStr::new)
        .unwrap_or_else(|| OsStr::new(""));
    crate::env::timed_build(&mut spec, &parent, &tc.cargo, &tc.rustc, name);
    let vars = spec.env_set.into_iter().collect();

    EnvMap { vars, overridden }
}

impl EnvMap {
    pub fn vars(&self) -> &BTreeMap<OsString, OsString> {
        &self.vars
    }

    pub fn command(&self, tc: &Toolchain, dir: &Path, build: &BuildCmd) -> Spec {
        let mut spec = Spec::new(&tc.cargo);
        spec.env_clear = true;
        spec.env_set = self
            .vars
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        spec.cwd = Some(dir.to_path_buf());
        spec.args = build.args().into_iter().map(Into::into).collect();
        spec
    }

    pub fn findings(&self) -> Vec<(&'static str, String)> {
        self.overridden
            .iter()
            .map(|(kind, var, value)| {
                let detail = if *kind == "env.rustc.overridden" {
                    format!("{var}={value}; the build used the resolved toolchain")
                } else {
                    format!("{var}={value}")
                };
                (*kind, detail)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::Toolchain;
    use std::ffi::OsString;
    use std::path::Path;

    fn tc(name: Option<&str>) -> Toolchain {
        Toolchain {
            name: name.map(String::from),
            cargo: "/tc/bin/cargo".into(),
            rustc: "/tc/bin/rustc".into(),
            release: "1.97.1".into(),
            version_text: "rustc 1.97.1".into(),
        }
    }

    #[test]
    fn each_shape_is_its_cargo_command_offline_and_locked() {
        for (word, args) in [
            ("build", vec!["build"]),
            ("check-all", vec!["check", "--all-targets"]),
            ("clippy-all", vec!["clippy", "--all-targets"]),
            ("test-no-run", vec!["test", "--no-run"]),
            ("check-lib=app", vec!["check", "-p", "app", "--lib"]),
        ] {
            let cmd: BuildCmd = word.parse().unwrap();
            let mut want: Vec<String> = args.into_iter().map(String::from).collect();
            want.extend(["--offline".into(), "--locked".into()]);
            assert_eq!(cmd.args(), want, "{word}");
            assert_eq!(cmd.command_line(), format!("cargo {}", want.join(" ")));
        }
        assert_eq!(BuildCmd::default(), BuildCmd::Build);
        for bad in [
            "run",
            "check-lib=",
            "check-lib=--features",
            "check-lib=a b",
            "bench",
        ] {
            assert!(bad.parse::<BuildCmd>().is_err(), "{bad}");
        }
    }

    #[test]
    fn the_build_environment_drops_wrappers_and_pins_the_resolved_toolchain() {
        let caller = [
            ("PATH", "/bin"),
            ("CARGO_HOME", "/h"),
            ("RUSTFLAGS", "-Dwarnings"),
            ("CARGO_TARGET_DIR", "/t"),
            ("CARGO_BUILD_TARGET_DIR", "/t"),
            ("CARGO_BUILD_BUILD_DIR", "/b"),
            ("GIT_DIR", "/g"),
            ("CARGO_PKG_NAME", "x"),
            ("CARGO_MANIFEST_DIR", "/m"),
            ("OUT_DIR", "/o"),
            ("RUSTC_WRAPPER", "sccache"),
            ("RUSTC_WORKSPACE_WRAPPER", ""),
            ("CARGO_BUILD_RUSTC_WRAPPER", "sccache"),
            ("CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER", "w"),
            ("RUSTC", "/other/rustc"),
            ("CARGO_BUILD_RUSTC", "/r"),
            ("RUSTUP_TOOLCHAIN", "nightly"),
            ("RUSTUP_TOOLCHAIN_SOURCE", "env"),
        ]
        .map(|(k, v)| (OsString::from(k), OsString::from(v)));
        let env = env_map(caller, &tc(Some("stable-aarch64-apple-darwin")));
        let get = |k: &str| {
            env.vars()
                .get(&OsString::from(k))
                .map(|v| v.to_string_lossy().into_owned())
        };
        for kept in ["PATH", "CARGO_HOME", "RUSTFLAGS"] {
            assert!(get(kept).is_some(), "{kept}");
        }
        for gone in [
            "CARGO_TARGET_DIR",
            "CARGO_BUILD_TARGET_DIR",
            "CARGO_BUILD_BUILD_DIR",
            "GIT_DIR",
            "CARGO_PKG_NAME",
            "CARGO_MANIFEST_DIR",
            "OUT_DIR",
            "CARGO_BUILD_RUSTC_WRAPPER",
            "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
            "CARGO_BUILD_RUSTC",
            "RUSTUP_TOOLCHAIN_SOURCE",
        ] {
            assert_eq!(get(gone), None, "{gone}");
        }
        assert_eq!(get("RUSTC_WRAPPER").as_deref(), Some(""));
        assert_eq!(get("RUSTC_WORKSPACE_WRAPPER").as_deref(), Some(""));
        assert_eq!(get("RUSTC").as_deref(), Some("/tc/bin/rustc"));
        assert_eq!(
            get("RUSTUP_TOOLCHAIN").as_deref(),
            Some("stable-aarch64-apple-darwin")
        );
        assert_eq!(get("RUSTUP_AUTO_INSTALL").as_deref(), Some("0"));
        let kinds: Vec<_> = env.findings().into_iter().map(|(k, _)| k).collect();
        assert_eq!(
            kinds
                .iter()
                .filter(|k| **k == "env.wrapper.overridden")
                .count(),
            3
        );
        assert_eq!(
            kinds
                .iter()
                .filter(|k| **k == "env.rustc.overridden")
                .count(),
            2
        );
    }

    #[test]
    fn without_rustup_the_pin_is_the_resolved_rustc_alone() {
        let env = env_map([], &tc(None));
        assert!(env
            .vars()
            .get(&OsString::from("RUSTUP_TOOLCHAIN"))
            .is_none());
        assert_eq!(
            env.vars()[&OsString::from("RUSTC")],
            OsString::from("/tc/bin/rustc")
        );
        assert!(env.findings().is_empty());
    }

    #[test]
    fn the_build_command_runs_the_resolved_cargo_in_the_checkout_with_exactly_the_map() {
        let env = env_map(
            [(OsString::from("PATH"), OsString::from("/bin"))],
            &tc(None),
        );
        let cmd = env.command(&tc(None), Path::new("/lane"), &BuildCmd::Build);
        assert_eq!(cmd.program, OsString::from("/tc/bin/cargo"));
        assert_eq!(cmd.cwd.as_deref(), Some(Path::new("/lane")));
        assert_eq!(
            cmd.args,
            ["build", "--offline", "--locked"].map(OsString::from)
        );
        assert!(cmd.env_clear);
    }
}
