//! Compile-only cargo shapes for the timed bench build, always offline and locked.

use std::str::FromStr;

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
