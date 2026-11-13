//! Liveness result types and shared fingerprint helpers. The algorithm that
//! decides live vs dead units lives in `crate::gc::liveness`; callers pass it
//! into seed as a `LivenessFn`.

use crate::cargo_target::TargetDir;
use crate::fingerprint;
use crate::platform::Fs;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How far back an invocation still counts as recent: 7 days.
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(7 * 86_400);

/// The verdict for a target dir: every unit sorted live or dead, or the reasons it cannot be decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Liveness {
    Decided {
        live: Vec<UnitRef>,
        dead: Vec<DeadUnit>,
    },
    Undecided {
        reasons: Vec<Uncertain>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UnitRef {
    pub profile: PathBuf,
    pub dir_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DeadUnit {
    pub unit: UnitRef,
    pub files: Vec<PathBuf>,
}

/// Something the liveness check could not decide, which blocks any deletion.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Uncertain {
    ProfileUnsupported { profile: PathBuf, why: String },
    NotCurrentInWindow { dir: PathBuf },
    Unresolved { unit: UnitRef, dependency: String },
    ListingFailed { dir: PathBuf, why: String },
    NoUnitHash { dir: PathBuf },
    DuplicateUnitHash { a: PathBuf, b: PathBuf },
    TargetUnreadable { target: PathBuf, why: String },
    TimestampUnreadable { path: PathBuf, why: String },
}

impl Uncertain {
    pub fn clears_when(&self) -> &'static str {
        match self {
            Uncertain::ProfileUnsupported { .. } => {
                "the profile dir uses a layout hive reads, or the profile dir is removed"
            }
            Uncertain::NotCurrentInWindow { .. } => {
                "Cargo builds the never compiled unit or abandons it, or its timestamp ages out of the window"
            }
            Uncertain::Unresolved { .. } => {
                "Cargo builds the dependency it names, or the depending unit is no longer live"
            }
            Uncertain::ListingFailed { .. } => "the directory is readable and within the entry cap",
            Uncertain::NoUnitHash { .. } => "the unit dir carries a 16-hex unit hash or is removed",
            Uncertain::DuplicateUnitHash { .. } => {
                "only one unit dir per profile carries that unit hash"
            }
            Uncertain::TargetUnreadable { .. } => "the target dir is readable",
            Uncertain::TimestampUnreadable { .. } => {
                "the file is readable, or the unit is removed"
            }
        }
    }
}

impl fmt::Display for Uncertain {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Uncertain::ProfileUnsupported { profile, why } => {
                write!(f, "{}: profile dir unsupported: {why}", profile.display())
            }
            Uncertain::NotCurrentInWindow { dir } => write!(
                f,
                "{}: unit never compiled, invoked inside the window",
                dir.display()
            ),
            Uncertain::Unresolved { unit, dependency } => write!(
                f,
                "{}: dependency `{dependency}` matches no unit of the target",
                unit.profile
                    .join(".fingerprint")
                    .join(&unit.dir_name)
                    .display()
            ),
            Uncertain::ListingFailed { dir, why } => {
                write!(f, "{}: cannot list: {why}", dir.display())
            }
            Uncertain::NoUnitHash { dir } => {
                write!(
                    f,
                    "{}: directory name has no 16-hex unit hash",
                    dir.display()
                )
            }
            Uncertain::DuplicateUnitHash { a, b } => write!(
                f,
                "{} and {}: two unit dirs share one unit hash",
                a.display(),
                b.display()
            ),
            Uncertain::TargetUnreadable { target, why } => {
                write!(f, "{}: target dir unreadable: {why}", target.display())
            }
            Uncertain::TimestampUnreadable { path, why } => {
                let what = if path.file_name() == Some(fingerprint::INVOKED_TIMESTAMP.as_ref()) {
                    "invocation timestamp"
                } else {
                    "file that dates the unit's last invocation"
                };
                write!(f, "{}: {what} unreadable: {why}", path.display())
            }
        }
    }
}

/// Where a unit's `invoked.timestamp` lives: `build/<dir>/` for run-build-script, `.fingerprint/<dir>/` for every other kind.
pub fn unit_timestamp(profile: &Path, dir_name: &str, kind: &str) -> PathBuf {
    let base = if kind == "run-build-script" {
        "build"
    } else {
        ".fingerprint"
    };
    profile
        .join(base)
        .join(dir_name)
        .join(fingerprint::INVOKED_TIMESTAMP)
}

/// The `<hash>` of every `-<hash>` in `name` where `<hash>` is 16 lowercase hex digits and is
/// followed by `.` or the end of the name.
pub(crate) fn embedded_hashes(name: &str) -> impl Iterator<Item = &str> {
    name.match_indices('-').filter_map(move |(at, _)| {
        let tail = &name[at + 1..];
        let hash = tail.get(..16)?;
        let rest = &tail[16..];
        let boundary = rest.is_empty() || rest.starts_with('.');
        (boundary
            && hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        .then_some(hash)
    })
}

/// Function signature seed receives for the liveness verdict.
pub type LivenessFn = fn(&dyn Fs, &TargetDir, SystemTime, Duration) -> Liveness;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn a_run_build_script_unit_is_timed_by_its_build_dir() {
        assert_eq!(
            unit_timestamp(Path::new("/t/debug"), "x-1", "run-build-script"),
            Path::new("/t/debug/build/x-1/invoked.timestamp")
        );
        assert_eq!(
            unit_timestamp(Path::new("/t/debug"), "x-1", "lib"),
            Path::new("/t/debug/.fingerprint/x-1/invoked.timestamp")
        );
    }

    #[test]
    fn an_unresolved_reason_clears_when_cargo_builds_the_dependency_or_the_unit_stops_being_live() {
        let reason = Uncertain::Unresolved {
            unit: UnitRef {
                profile: PathBuf::from("/t/debug"),
                dir_name: "a-1111111111111111".to_string(),
            },
            dependency: "liba".into(),
        };
        let text = reason.clears_when();
        assert!(text.contains("builds the dependency"), "{text}");
        assert!(text.contains("no longer live"), "{text}");
    }

    #[test]
    fn every_kind_of_reason_says_what_clears_it() {
        let path = PathBuf::from("/t/debug");
        let unit = UnitRef {
            profile: path.clone(),
            dir_name: "a-1111111111111111".into(),
        };
        let all = [
            Uncertain::ProfileUnsupported {
                profile: path.clone(),
                why: "w".into(),
            },
            Uncertain::NotCurrentInWindow { dir: path.clone() },
            Uncertain::Unresolved {
                unit,
                dependency: "d".into(),
            },
            Uncertain::ListingFailed {
                dir: path.clone(),
                why: "w".into(),
            },
            Uncertain::NoUnitHash { dir: path.clone() },
            Uncertain::DuplicateUnitHash {
                a: path.clone(),
                b: path.clone(),
            },
            Uncertain::TargetUnreadable {
                target: path.clone(),
                why: "w".into(),
            },
            Uncertain::TimestampUnreadable {
                path,
                why: "w".into(),
            },
        ];
        for reason in all {
            assert!(!reason.clears_when().is_empty(), "{reason:?}");
            assert!(reason.to_string().contains("/t/debug"), "{reason:?}");
        }
    }
}
