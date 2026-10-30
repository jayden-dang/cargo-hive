//! Which units of a target dir are live, and which are dead. Read-only: it reads `.fingerprint/`
//! reports and `invoked.timestamp` mtimes through `Fs`, runs no process and writes nothing. A
//! unit is live when it was invoked inside the window or a live unit depends on it, across every
//! supported profile dir of the target. Anything it cannot decide is reported, never guessed.

use crate::cargo_target::TargetDir;
use crate::fingerprint::{self, le_hex, Format, Unit};
use crate::platform::Fs;
use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub const DEFAULT_WINDOW: Duration = Duration::from_secs(7 * 86_400);

const NEVER_COLLECTED: [&str; 5] = [
    "build-script",
    "run-build-script",
    "doc-lib",
    "doc-bin",
    "other",
];

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

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Uncertain {
    ProfileUnsupported { profile: PathBuf, why: String },
    NotCurrentInWindow { dir: PathBuf },
    Unresolved { unit: UnitRef, dependency: String },
    ListingFailed { dir: PathBuf, why: String },
    NoUnitHash { dir: PathBuf },
    DuplicateUnitHash { a: PathBuf, b: PathBuf },
    TargetUnreadable { target: PathBuf, why: String },
}

impl Uncertain {
    pub fn clears_when(&self) -> &'static str {
        match self {
            Uncertain::ProfileUnsupported { .. } => {
                "the profile dir uses a layout hive reads, or the profile dir is removed"
            }
            Uncertain::NotCurrentInWindow { .. } => {
                "Cargo finishes or abandons that rebuild and its timestamp ages out of the window"
            }
            Uncertain::Unresolved { .. } => {
                "Cargo rebuilds the unit or the dependency it names, or the unit is removed"
            }
            Uncertain::ListingFailed { .. } => "the directory is readable and within the entry cap",
            Uncertain::NoUnitHash { .. } => "the unit dir carries a 16-hex unit hash or is removed",
            Uncertain::DuplicateUnitHash { .. } => {
                "only one unit dir per profile carries that unit hash"
            }
            Uncertain::TargetUnreadable { .. } => "the target dir is readable",
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
                "{}: unit is mid-rebuild or failed, invoked inside the window",
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
        }
    }
}

pub fn unit_timestamp(profile: &Path, dir_name: &str, kind: &str) -> PathBuf {
    let base = if kind == "run-build-script" {
        "build"
    } else {
        ".fingerprint"
    };
    profile.join(base).join(dir_name).join("invoked.timestamp")
}

fn in_window(mtime: SystemTime, now: SystemTime, window: Duration) -> bool {
    now.checked_sub(window).is_none_or(|cutoff| mtime >= cutoff)
}

fn invoked_in_window(fs: &dyn Fs, timestamp: &Path, now: SystemTime, window: Duration) -> bool {
    fs.mtime(timestamp)
        .is_ok_and(|mtime| in_window(mtime, now, window))
}

pub fn liveness(fs: &dyn Fs, target: &TargetDir, now: SystemTime, window: Duration) -> Liveness {
    let mut reasons: Vec<Uncertain> = Vec::new();
    if let Some(why) = &target.unreadable {
        reasons.push(Uncertain::TargetUnreadable {
            target: target.path.clone(),
            why: why.clone(),
        });
    }

    let mut units: Vec<(&Path, &Unit)> = Vec::new();
    for profile in &target.profiles {
        if let Format::Unsupported(why) = &profile.format {
            reasons.push(Uncertain::ProfileUnsupported {
                profile: profile.path.clone(),
                why: why.clone(),
            });
            continue;
        }
        for nc in &profile.not_current_units {
            let Some(dir_name) = nc.dir.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let kind = nc
                .stem
                .as_deref()
                .map_or_else(|| "other".to_string(), fingerprint::kind_of);
            let stamp = unit_timestamp(&profile.path, dir_name, &kind);
            if invoked_in_window(fs, &stamp, now, window) {
                reasons.push(Uncertain::NotCurrentInWindow {
                    dir: nc.dir.clone(),
                });
            }
        }
        if let Some(graph) = &profile.graph {
            units.extend(graph.units.iter().map(|u| (profile.path.as_path(), u)));
        }
    }

    let mut index: HashMap<&str, Vec<usize>> = HashMap::new();
    let mut by_unit_hash: HashMap<(&Path, &str), usize> = HashMap::new();
    for (i, (profile, unit)) in units.iter().enumerate() {
        index.entry(unit.hash_hex.as_str()).or_default().push(i);
        let dir = profile.join(".fingerprint").join(&unit.dir_name);
        match fingerprint::unit_hash(&unit.dir_name) {
            None => reasons.push(Uncertain::NoUnitHash { dir }),
            Some(hash) => {
                if let Some(&first) = by_unit_hash.get(&(*profile, hash)) {
                    let (_, other) = units[first];
                    reasons.push(Uncertain::DuplicateUnitHash {
                        a: profile.join(".fingerprint").join(&other.dir_name),
                        b: dir,
                    });
                } else {
                    by_unit_hash.insert((*profile, hash), i);
                }
            }
        }
    }

    let mut live = vec![false; units.len()];
    let mut queue: Vec<usize> = Vec::new();
    for (i, (profile, unit)) in units.iter().enumerate() {
        let stamp = unit_timestamp(profile, &unit.dir_name, &unit.kind);
        if invoked_in_window(fs, &stamp, now, window) {
            live[i] = true;
            queue.push(i);
        }
    }
    let mut cursor = 0;
    while cursor < queue.len() {
        let (profile, unit) = units[queue[cursor]];
        cursor += 1;
        for &dep in &unit.deps {
            match index.get(le_hex(dep).as_str()) {
                Some(hits) => {
                    for &hit in hits {
                        if !live[hit] {
                            live[hit] = true;
                            queue.push(hit);
                        }
                    }
                }
                None => {
                    let fp_dir = profile.join(".fingerprint").join(&unit.dir_name);
                    reasons.push(Uncertain::Unresolved {
                        unit: UnitRef {
                            profile: profile.to_path_buf(),
                            dir_name: unit.dir_name.clone(),
                        },
                        dependency: fingerprint::dep_name(&fp_dir, &unit.stem, dep)
                            .unwrap_or_else(|| le_hex(dep)),
                    });
                }
            }
        }
    }

    if !reasons.is_empty() {
        reasons.sort();
        reasons.dedup();
        return Liveness::Undecided { reasons };
    }

    let unit_ref = |(profile, unit): &(&Path, &Unit)| UnitRef {
        profile: profile.to_path_buf(),
        dir_name: unit.dir_name.clone(),
    };
    let mut live_units = BTreeSet::new();
    let mut dead_units = BTreeSet::new();
    for (i, entry) in units.iter().enumerate() {
        if live[i] {
            live_units.insert(unit_ref(entry));
        } else if !NEVER_COLLECTED.contains(&entry.1.kind.as_str()) {
            dead_units.insert(unit_ref(entry));
        }
    }
    Liveness::Decided {
        live: live_units.into_iter().collect(),
        dead: dead_units
            .into_iter()
            .map(|unit| DeadUnit {
                unit,
                files: Vec::new(),
            })
            .collect(),
    }
}

#[cfg(test)]
#[cfg(target_os = "macos")]
mod tests {
    use super::*;
    use crate::cargo_target::Owner;
    use crate::fingerprint::read_profile;
    use std::fs;
    use std::time::UNIX_EPOCH;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Scratch {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = PathBuf::from(format!(
                "/private/tmp/hive-it-liveness-{label}-{}-{nanos}",
                std::process::id()
            ));
            fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    const HOUR: Duration = Duration::from_secs(3600);
    const DAY: Duration = Duration::from_secs(86_400);

    fn now() -> SystemTime {
        SystemTime::now()
    }

    fn touch(path: &Path, age: Duration) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = fs::File::create(path).unwrap();
        file.set_modified(now() - age).unwrap();
    }

    /// A unit dir named `name` whose fingerprint is `fp`, depending on `deps` as (name, fingerprint).
    fn write_unit(
        profile: &Path,
        name: &str,
        stem: &str,
        fp: u64,
        deps: &[(&str, u64)],
        age: Option<Duration>,
    ) {
        let dir = profile.join(".fingerprint").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(stem), le_hex(fp)).unwrap();
        let deps: Vec<String> = deps
            .iter()
            .map(|(n, h)| format!("[1,\"{n}\",false,{h}]"))
            .collect();
        fs::write(
            dir.join(format!("{stem}.json")),
            format!(
                "{{\"rustc\":1,\"features\":\"[]\",\"declared_features\":\"[]\",\"target\":2,\"profile\":3,\"path\":4,\"deps\":[{}],\"local\":[],\"rustflags\":[],\"config\":5,\"compile_kind\":0}}",
                deps.join(",")
            ),
        )
        .unwrap();
        if let Some(age) = age {
            let stamp = unit_timestamp(profile, name, &fingerprint::kind_of(stem));
            touch(&stamp, age);
        }
    }

    fn target_of(root: &Path, profiles: &[&Path]) -> TargetDir {
        TargetDir {
            path: root.to_path_buf(),
            owner: Owner::Unresolved {
                reason: "test".into(),
            },
            profiles: profiles.iter().map(|p| read_profile(p)).collect(),
            size: None,
            unreadable: None,
        }
    }

    fn decide(target: &TargetDir, window: Duration) -> Liveness {
        let platform = crate::platform::current().unwrap();
        liveness(&*platform.fs, target, now(), window)
    }

    fn names(units: &[UnitRef]) -> Vec<&str> {
        units.iter().map(|u| u.dir_name.as_str()).collect()
    }

    fn decided(l: Liveness) -> (Vec<UnitRef>, Vec<DeadUnit>) {
        match l {
            Liveness::Decided { live, dead } => (live, dead),
            other => panic!("expected Decided, got {other:?}"),
        }
    }

    const A: &str = "a-1111111111111111";
    const B: &str = "b-2222222222222222";
    const C: &str = "c-3333333333333333";
    const D: &str = "d-4444444444444444";

    #[test]
    fn a_unit_inside_the_window_and_everything_it_reaches_are_live() {
        let s = Scratch::new("reach");
        let p = s.0.join("debug");
        write_unit(&p, A, "lib-a", 101, &[("b", 102)], Some(HOUR));
        write_unit(&p, B, "lib-b", 102, &[("c", 103)], Some(30 * DAY));
        write_unit(&p, C, "lib-c", 103, &[], Some(30 * DAY));
        write_unit(&p, D, "lib-d", 104, &[], Some(30 * DAY));
        let (live, dead) = decided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
        assert_eq!(names(&live), [A, B, C]);
        assert_eq!(dead.len(), 1);
        assert_eq!(dead[0].unit.dir_name, D);
        assert!(dead[0].files.is_empty());
    }

    #[test]
    fn a_dependency_in_another_profile_dir_of_the_same_target_is_reached() {
        let s = Scratch::new("cross");
        let host = s.0.join("debug");
        let cross = s.0.join("aarch64-apple-darwin").join("debug");
        write_unit(&cross, A, "bin-a", 101, &[("m", 102)], Some(HOUR));
        write_unit(&host, B, "lib-m", 102, &[], Some(30 * DAY));
        let (live, dead) = decided(decide(&target_of(&s.0, &[&host, &cross]), 6 * HOUR));
        assert_eq!(names(&live), [A, B]);
        assert!(dead.is_empty());
    }

    #[test]
    fn an_edge_matching_two_units_reaches_both() {
        let s = Scratch::new("twice");
        let host = s.0.join("debug");
        let cross = s.0.join("aarch64-apple-darwin").join("debug");
        write_unit(&host, A, "bin-a", 101, &[("x", 102)], Some(HOUR));
        write_unit(&host, B, "lib-x", 102, &[], Some(30 * DAY));
        write_unit(&cross, C, "lib-x", 102, &[], Some(30 * DAY));
        let (live, _) = decided(decide(&target_of(&s.0, &[&host, &cross]), 6 * HOUR));
        assert_eq!(live.len(), 3);
    }

    #[test]
    fn a_unit_without_a_readable_timestamp_is_not_a_root() {
        let s = Scratch::new("nostamp");
        let p = s.0.join("debug");
        write_unit(&p, A, "lib-a", 101, &[], None);
        let (live, dead) = decided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
        assert!(live.is_empty());
        assert_eq!(dead[0].unit.dir_name, A);
    }

    #[test]
    fn a_timestamp_later_than_now_makes_a_root() {
        let s = Scratch::new("future");
        let p = s.0.join("debug");
        write_unit(&p, A, "lib-a", 101, &[], None);
        let stamp = unit_timestamp(&p, A, "lib");
        fs::create_dir_all(stamp.parent().unwrap()).unwrap();
        fs::File::create(&stamp)
            .unwrap()
            .set_modified(now() + 10 * DAY)
            .unwrap();
        let (live, _) = decided(decide(&target_of(&s.0, &[&p]), HOUR));
        assert_eq!(names(&live), [A]);
    }

    #[test]
    fn a_window_larger_than_the_time_since_the_epoch_does_not_panic() {
        let s = Scratch::new("huge");
        let p = s.0.join("debug");
        write_unit(&p, A, "lib-a", 101, &[], Some(30 * DAY));
        let huge = Duration::from_secs(u64::MAX / 2);
        let (live, _) = decided(decide(&target_of(&s.0, &[&p]), huge));
        assert_eq!(names(&live), [A]);
    }

    #[test]
    fn never_collected_kinds_are_roots_and_carry_edges_but_are_never_dead() {
        for (stem, kind) in [
            ("build-script-build", "build-script"),
            ("run-build-script-build-script-build", "run-build-script"),
            ("doc-lib-a", "doc-lib"),
            ("doc-bin-a", "doc-bin"),
            ("weird", "other"),
        ] {
            let s = Scratch::new("never");
            let p = s.0.join("debug");
            write_unit(&p, A, stem, 101, &[("b", 102)], Some(HOUR));
            write_unit(&p, B, "lib-b", 102, &[], Some(30 * DAY));
            let stamp = unit_timestamp(&p, A, kind);
            assert!(stamp.exists(), "{kind}: timestamp at {}", stamp.display());
            let (live, dead) = decided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
            assert_eq!(names(&live), [A, B], "{kind}");
            assert!(dead.is_empty(), "{kind}");

            let aged = Scratch::new("never-old");
            let p = aged.0.join("debug");
            write_unit(&p, A, stem, 101, &[], Some(30 * DAY));
            let (live, dead) = decided(decide(&target_of(&aged.0, &[&p]), 6 * HOUR));
            assert!(live.is_empty(), "{kind}");
            assert!(dead.is_empty(), "{kind}");
        }
    }

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
    fn no_window_means_seven_days() {
        assert_eq!(DEFAULT_WINDOW, Duration::from_secs(7 * 86_400));
    }

    #[test]
    fn two_calls_on_an_unchanged_target_are_equal_and_write_nothing() {
        fn tree(root: &Path) -> Vec<(PathBuf, Vec<u8>, SystemTime)> {
            let mut out = Vec::new();
            let mut stack = vec![root.to_path_buf()];
            while let Some(dir) = stack.pop() {
                for entry in fs::read_dir(&dir).unwrap() {
                    let path = entry.unwrap().path();
                    let meta = fs::metadata(&path).unwrap();
                    if meta.is_dir() {
                        stack.push(path.clone());
                        out.push((path, Vec::new(), meta.modified().unwrap()));
                    } else {
                        out.push((
                            path.clone(),
                            fs::read(&path).unwrap(),
                            meta.modified().unwrap(),
                        ));
                    }
                }
            }
            out.sort();
            out
        }
        let s = Scratch::new("stable");
        let p = s.0.join("debug");
        write_unit(&p, A, "lib-a", 101, &[("b", 102)], Some(HOUR));
        write_unit(&p, B, "lib-b", 102, &[], Some(30 * DAY));
        write_unit(&p, D, "lib-d", 104, &[], Some(30 * DAY));
        let target = target_of(&s.0, &[&p]);
        let before = tree(&s.0);
        let first = decide(&target, 6 * HOUR);
        let second = decide(&target, 6 * HOUR);
        assert_eq!(first, second);
        assert_eq!(before, tree(&s.0));
    }

    #[test]
    fn a_dependency_no_unit_provides_leaves_the_target_undecided_and_names_it() {
        let s = Scratch::new("unresolved");
        let p = s.0.join("debug");
        write_unit(&p, A, "lib-a", 101, &[("ghost", 999)], Some(HOUR));
        match decide(&target_of(&s.0, &[&p]), 6 * HOUR) {
            Liveness::Undecided { reasons } => assert!(matches!(
                &reasons[..],
                [Uncertain::Unresolved { dependency, .. }] if dependency == "ghost"
            )),
            other => panic!("expected Undecided, got {other:?}"),
        }
    }
}
