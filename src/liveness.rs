//! Which units of a target dir are live, and which are dead. Read-only: it reads `.fingerprint/`
//! reports and `invoked.timestamp` mtimes through `Fs`, runs no process and writes nothing. A
//! unit is live when it was invoked inside the window or a live unit depends on it, across every
//! supported profile dir of the target. Anything it cannot decide is reported, never guessed.

use crate::cargo_target::TargetDir;
use crate::fingerprint::{self, le_hex, Format, Unit};
use crate::gc::DEPS_CAP;
use crate::platform::Fs;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How far back an invocation still counts as recent: 7 days.
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
    TimestampUnreadable { path: PathBuf, why: String },
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
            Uncertain::TimestampUnreadable { .. } => {
                "the timestamp file is readable, or the unit is removed"
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
            Uncertain::TimestampUnreadable { path, why } => {
                write!(
                    f,
                    "{}: invocation timestamp unreadable: {why}",
                    path.display()
                )
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
    profile.join(base).join(dir_name).join("invoked.timestamp")
}

fn in_window(mtime: SystemTime, now: SystemTime, window: Duration) -> bool {
    now.checked_sub(window).is_none_or(|cutoff| mtime >= cutoff)
}

/// Whether the unit was invoked inside the window. A timestamp that does not exist means it was
/// never invoked; any other read error leaves the question open, so the caller must not delete.
fn invoked_in_window(
    fs: &dyn Fs,
    timestamp: &Path,
    now: SystemTime,
    window: Duration,
) -> Result<bool, Uncertain> {
    match fs.mtime(timestamp) {
        Ok(mtime) => Ok(in_window(mtime, now, window)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(Uncertain::TimestampUnreadable {
            path: timestamp.to_path_buf(),
            why: e.to_string(),
        }),
    }
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
    let mut not_current_hashes: Vec<(&Path, &str)> = Vec::new();
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
                reasons.push(Uncertain::NoUnitHash {
                    dir: nc.dir.clone(),
                });
                continue;
            };
            let kind = nc
                .stem
                .as_deref()
                .map_or_else(|| "other".to_string(), fingerprint::kind_of);
            if let Some(hash) = fingerprint::unit_hash(dir_name) {
                not_current_hashes.push((profile.path.as_path(), hash));
            }
            let stamp = unit_timestamp(&profile.path, dir_name, &kind);
            match invoked_in_window(fs, &stamp, now, window) {
                Ok(true) => reasons.push(Uncertain::NotCurrentInWindow {
                    dir: nc.dir.clone(),
                }),
                Ok(false) => {}
                Err(reason) => reasons.push(reason),
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
        match invoked_in_window(fs, &stamp, now, window) {
            Ok(true) => {
                live[i] = true;
                queue.push(i);
            }
            Ok(false) => {}
            Err(reason) => reasons.push(reason),
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

    let dead_idx: Vec<usize> = (0..units.len())
        .filter(|&i| {
            !live[i]
                && !NEVER_COLLECTED.contains(&units[i].1.kind.as_str())
                && fingerprint::unit_hash(&units[i].1.dir_name).is_some()
        })
        .collect();

    let mut listings: BTreeMap<&Path, Vec<(String, PathBuf)>> = BTreeMap::new();
    for &i in &dead_idx {
        listings.entry(units[i].0).or_default();
    }
    for (profile, entries) in &mut listings {
        for sub in ["deps", "examples"] {
            let dir = profile.join(sub);
            match fs.read_dir_bounded(&dir, DEPS_CAP) {
                Ok(listed) => {
                    entries.extend(listed.into_iter().map(|(name, _)| {
                        let path = dir.join(&name);
                        (name, path)
                    }));
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => reasons.push(Uncertain::ListingFailed {
                    dir,
                    why: e.to_string(),
                }),
            }
        }
        entries.sort();
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
    let dead_set: HashSet<usize> = dead_idx.iter().copied().collect();
    let mut kept_hashes: HashMap<&Path, Vec<&str>> = HashMap::new();
    for (i, (profile, unit)) in units.iter().enumerate() {
        if !dead_set.contains(&i) {
            if let Some(hash) = fingerprint::unit_hash(&unit.dir_name) {
                kept_hashes.entry(profile).or_default().push(hash);
            }
        }
    }
    for (profile, hash) in &not_current_hashes {
        kept_hashes.entry(profile).or_default().push(hash);
    }

    let mut live_units = BTreeSet::new();
    for (i, entry) in units.iter().enumerate() {
        if live[i] {
            live_units.insert(unit_ref(entry));
        }
    }
    let mut dead_units: BTreeMap<UnitRef, Vec<PathBuf>> = BTreeMap::new();
    for &i in &dead_idx {
        let (profile, unit) = units[i];
        let Some(hash) = fingerprint::unit_hash(&unit.dir_name) else {
            continue;
        };
        let kept = kept_hashes.get(profile).map_or(&[][..], Vec::as_slice);
        let matching: Vec<&PathBuf> = listings[profile]
            .iter()
            .filter(|(name, _)| carries(name, hash) && !kept.iter().any(|h| carries(name, h)))
            .map(|(_, path)| path)
            .collect();
        let mut files = Vec::new();
        if !matching.is_empty() {
            files.push(profile.join(".fingerprint").join(&unit.dir_name));
            files.extend(matching.into_iter().cloned());
        }
        dead_units.insert(unit_ref(&units[i]), files);
    }
    Liveness::Decided {
        live: live_units.into_iter().collect(),
        dead: dead_units
            .into_iter()
            .map(|(unit, files)| DeadUnit { unit, files })
            .collect(),
    }
}

fn carries(name: &str, unit_hash: &str) -> bool {
    let needle = format!("-{unit_hash}");
    name.match_indices(&needle).any(|(at, _)| {
        let rest = &name[at + needle.len()..];
        rest.is_empty() || rest.starts_with('.')
    })
}

#[cfg(test)]
#[cfg(target_os = "macos")]
mod tests {
    use super::*;
    use crate::cargo_target::Owner;
    use crate::fingerprint::read_profile;
    use std::fs;
    use std::io;
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
        let mut got = names(&live);
        got.sort();
        assert_eq!(got, [A, B, C]);
    }

    #[test]
    fn an_edge_matching_two_units_in_one_profile_reaches_both() {
        let s = Scratch::new("twice-same");
        let p = s.0.join("debug");
        write_unit(&p, A, "bin-a", 101, &[("x", 102)], Some(HOUR));
        write_unit(&p, B, "lib-x", 102, &[], Some(30 * DAY));
        write_unit(&p, C, "lib-x", 102, &[], Some(30 * DAY));
        write_unit(&p, D, "lib-y", 104, &[], Some(30 * DAY));
        let (live, dead) = decided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
        assert_eq!(names(&live), [A, B, C]);
        assert_eq!(dead.len(), 1);
        assert_eq!(dead[0].unit.dir_name, D);
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
        write_unit(&p, B, "lib-b", 102, &[], Some(HOUR));
        let huge = Duration::from_secs(u64::MAX);
        let (live, dead) = decided(decide(&target_of(&s.0, &[&p]), huge));
        assert_eq!(names(&live), [A, B]);
        assert!(dead.is_empty());
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
    fn the_default_window_keeps_a_unit_invoked_just_under_seven_days_ago() {
        let s = Scratch::new("default-window");
        let p = s.0.join("debug");
        write_unit(&p, A, "lib-a", 101, &[], Some(6 * DAY + 23 * HOUR));
        write_unit(&p, B, "lib-b", 102, &[], Some(7 * DAY + HOUR));
        let (live, dead) = decided(decide(&target_of(&s.0, &[&p]), DEFAULT_WINDOW));
        assert_eq!(names(&live), [A]);
        assert_eq!(dead.len(), 1);
        assert_eq!(dead[0].unit.dir_name, B);
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

    fn undecided(l: Liveness) -> Vec<Uncertain> {
        match l {
            Liveness::Undecided { reasons } => reasons,
            other => panic!("expected Undecided, got {other:?}"),
        }
    }

    fn never_compiled(profile: &Path, name: &str, age: Duration) {
        let dir = profile.join(".fingerprint").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("output-lib-x"), "").unwrap();
        touch(&dir.join("invoked.timestamp"), age);
    }

    fn emptied_hash(profile: &Path, name: &str, stem: &str, age: Option<Duration>) {
        write_unit(profile, name, stem, 1, &[], age);
        fs::write(profile.join(".fingerprint").join(name).join(stem), "").unwrap();
    }

    #[test]
    fn an_unsupported_profile_makes_the_whole_target_undecided() {
        let s = Scratch::new("unsupported");
        let good = s.0.join("debug");
        let bare = s.0.join("release");
        fs::create_dir_all(&bare).unwrap();
        write_unit(&good, A, "lib-a", 101, &[], Some(HOUR));
        let reasons = undecided(decide(&target_of(&s.0, &[&good, &bare]), 6 * HOUR));
        assert!(matches!(
            &reasons[..],
            [Uncertain::ProfileUnsupported { profile, .. }] if *profile == bare
        ));
    }

    #[test]
    fn a_not_current_unit_inside_the_window_makes_the_target_undecided() {
        type Make = fn(&Path, &str);
        let cases: [(&str, Make); 3] = [
            ("never compiled", |p, n| never_compiled(p, n, HOUR)),
            ("emptied hash", |p, n| {
                emptied_hash(p, n, "lib-x", Some(HOUR))
            }),
            ("run build script", |p, n| {
                emptied_hash(p, n, "run-build-script-build-script-build", Some(HOUR))
            }),
        ];
        for (label, make) in cases {
            let s = Scratch::new("nc-in");
            let p = s.0.join("debug");
            write_unit(&p, B, "lib-b", 102, &[], Some(30 * DAY));
            make(&p, A);
            let reasons = undecided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
            let dir = p.join(".fingerprint").join(A);
            assert!(
                matches!(&reasons[..], [Uncertain::NotCurrentInWindow { dir: d }] if *d == dir),
                "{label}: {reasons:?}"
            );
        }
    }

    #[test]
    fn a_not_current_unit_outside_the_window_or_without_a_timestamp_is_ignored() {
        let s = Scratch::new("nc-out");
        let p = s.0.join("debug");
        write_unit(&p, D, "lib-d", 104, &[], Some(30 * DAY));
        never_compiled(&p, A, 30 * DAY);
        emptied_hash(&p, B, "lib-b", None);
        emptied_hash(&p, C, "run-build-script-build-script-build", Some(30 * DAY));
        let (live, dead) = decided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
        assert!(live.is_empty());
        let dead: Vec<&str> = dead.iter().map(|d| d.unit.dir_name.as_str()).collect();
        assert_eq!(dead, [D]);
    }

    #[test]
    fn an_unresolved_edge_from_a_live_unit_names_the_unit_and_the_dependency() {
        let s = Scratch::new("unres");
        let p = s.0.join("debug");
        write_unit(&p, A, "lib-a", 101, &[("ghost", 999)], Some(HOUR));
        let unit = UnitRef {
            profile: p.clone(),
            dir_name: A.to_string(),
        };
        let target = target_of(&s.0, &[&p]);
        let reasons = undecided(decide(&target, 6 * HOUR));
        assert_eq!(
            reasons,
            [Uncertain::Unresolved {
                unit: unit.clone(),
                dependency: "ghost".into()
            }]
        );

        fs::remove_file(p.join(".fingerprint").join(A).join("lib-a.json")).unwrap();
        let reasons = undecided(decide(&target, 6 * HOUR));
        assert_eq!(
            reasons,
            [Uncertain::Unresolved {
                unit,
                dependency: le_hex(999)
            }]
        );
    }

    #[test]
    fn an_unresolved_edge_from_a_dead_unit_leaves_the_target_decided() {
        let s = Scratch::new("unres-dead");
        let p = s.0.join("debug");
        write_unit(&p, A, "lib-a", 101, &[("ghost", 999)], Some(30 * DAY));
        write_unit(&p, B, "lib-b", 102, &[], Some(HOUR));
        let (live, dead) = decided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
        assert_eq!(names(&live), [B]);
        assert_eq!(dead[0].unit.dir_name, A);
    }

    #[test]
    fn a_dir_without_a_unit_hash_makes_the_target_undecided() {
        let s = Scratch::new("nohash");
        let p = s.0.join("debug");
        write_unit(&p, "plain", "lib-a", 101, &[], Some(30 * DAY));
        let reasons = undecided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
        assert_eq!(
            reasons,
            [Uncertain::NoUnitHash {
                dir: p.join(".fingerprint").join("plain")
            }]
        );
    }

    #[test]
    fn two_units_sharing_one_unit_hash_make_the_target_undecided() {
        let s = Scratch::new("dup");
        let p = s.0.join("debug");
        let other = "z-1111111111111111";
        write_unit(&p, A, "lib-a", 101, &[], Some(30 * DAY));
        write_unit(&p, other, "lib-z", 102, &[], Some(30 * DAY));
        let reasons = undecided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
        assert_eq!(
            reasons,
            [Uncertain::DuplicateUnitHash {
                a: p.join(".fingerprint").join(A),
                b: p.join(".fingerprint").join(other),
            }]
        );
    }

    #[test]
    fn the_same_unit_hash_in_two_profile_dirs_is_not_a_duplicate() {
        let s = Scratch::new("dup-cross");
        let host = s.0.join("debug");
        let cross = s.0.join("aarch64-apple-darwin").join("debug");
        write_unit(&host, A, "lib-a", 101, &[], Some(HOUR));
        write_unit(&cross, A, "lib-a", 101, &[], Some(HOUR));
        let (live, _) = decided(decide(&target_of(&s.0, &[&host, &cross]), 6 * HOUR));
        assert_eq!(live.len(), 2);
    }

    #[test]
    fn an_unreadable_target_is_undecided() {
        let s = Scratch::new("unreadable");
        let mut target = target_of(&s.0, &[]);
        target.unreadable = Some("permission denied".into());
        let reasons = undecided(decide(&target, 6 * HOUR));
        assert_eq!(
            reasons,
            [Uncertain::TargetUnreadable {
                target: s.0.clone(),
                why: "permission denied".into()
            }]
        );
    }

    #[test]
    fn every_reason_is_reported_with_what_clears_it() {
        let s = Scratch::new("all");
        let bare = s.0.join("release");
        fs::create_dir_all(&bare).unwrap();
        let p = s.0.join("debug");
        write_unit(&p, A, "lib-a", 101, &[("ghost", 999)], Some(HOUR));
        never_compiled(&p, B, HOUR);
        let reasons = undecided(decide(&target_of(&s.0, &[&p, &bare]), 6 * HOUR));
        assert!(matches!(
            &reasons[..],
            [
                Uncertain::ProfileUnsupported { .. },
                Uncertain::NotCurrentInWindow { .. },
                Uncertain::Unresolved { .. }
            ]
        ));
        for reason in &reasons {
            assert!(!reason.clears_when().is_empty());
            assert!(!reason.to_string().is_empty());
        }
    }

    #[test]
    fn every_kind_of_reason_says_what_clears_it() {
        let path = PathBuf::from("/t/debug");
        let unit = UnitRef {
            profile: path.clone(),
            dir_name: A.into(),
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

    const HASH_A: &str = "1111111111111111";
    const HASH_D: &str = "4444444444444444";

    fn plant(profile: &Path, sub: &str, name: &str) {
        let path = profile.join(sub).join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }

    fn dead_of(l: Liveness, dir_name: &str) -> DeadUnit {
        decided(l)
            .1
            .into_iter()
            .find(|d| d.unit.dir_name == dir_name)
            .unwrap_or_else(|| panic!("{dir_name} is not dead"))
    }

    enum Listing {
        Reversed,
        FailOn(&'static str),
        MtimeFails(io::ErrorKind),
    }

    struct FakeFs {
        real: Box<dyn Fs>,
        listing: Listing,
        listed: std::cell::RefCell<Vec<PathBuf>>,
    }

    impl FakeFs {
        fn new(listing: Listing) -> FakeFs {
            FakeFs {
                real: crate::platform::current().unwrap().fs,
                listing,
                listed: Default::default(),
            }
        }
    }

    impl Fs for FakeFs {
        fn temp_dir(&self, parent: &Path, prefix: &str) -> io::Result<crate::platform::TempDir> {
            self.real.temp_dir(parent, prefix)
        }
        fn write_file(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
            self.real.write_file(path, bytes)
        }
        fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
            self.real.read(path)
        }
        fn hardlink(&self, src: &Path, dst: &Path) -> io::Result<()> {
            self.real.hardlink(src, dst)
        }
        fn set_mtime(&self, path: &Path, t: SystemTime) -> io::Result<()> {
            self.real.set_mtime(path, t)
        }
        fn mtime(&self, path: &Path) -> io::Result<SystemTime> {
            match self.listing {
                Listing::MtimeFails(kind) if path.ends_with("invoked.timestamp") => {
                    Err(io::Error::new(kind, "injected"))
                }
                _ => self.real.mtime(path),
            }
        }
        fn inode(&self, path: &Path) -> io::Result<u64> {
            self.real.inode(path)
        }
        fn clone_file(&self, src: &Path, dst: &Path) -> io::Result<()> {
            self.real.clone_file(src, dst)
        }
        fn sharing(&self, path: &Path) -> io::Result<crate::platform::Sharing> {
            self.real.sharing(path)
        }
        fn compress(&self, path: &Path) -> crate::platform::CompressOutcome {
            self.real.compress(path)
        }
        fn hold_lock(
            &self,
            kind: crate::platform::LockKind,
            path: &Path,
        ) -> io::Result<crate::platform::LockHolder> {
            self.real.hold_lock(kind, path)
        }
        fn try_lock_exclusive(&self, path: &Path) -> crate::platform::TryLock {
            self.real.try_lock_exclusive(path)
        }
        fn try_lock_elsewhere(
            &self,
            kind: crate::platform::LockKind,
            path: &Path,
        ) -> io::Result<crate::platform::LockState> {
            self.real.try_lock_elsewhere(kind, path)
        }
        fn entry_stat(&self, path: &Path) -> io::Result<crate::platform::EntryStat> {
            self.real.entry_stat(path)
        }
        fn read_dir_bounded(
            &self,
            dir: &Path,
            cap: usize,
        ) -> io::Result<Vec<(String, crate::platform::EntryKind)>> {
            self.listed.borrow_mut().push(dir.to_path_buf());
            match self.listing {
                Listing::FailOn(leaf) if dir.ends_with(leaf) => {
                    Err(io::Error::other("entry cap exceeded"))
                }
                Listing::Reversed => {
                    let mut entries = self.real.read_dir_bounded(dir, cap)?;
                    entries.reverse();
                    Ok(entries)
                }
                _ => self.real.read_dir_bounded(dir, cap),
            }
        }
        fn remove_tree(&self, path: &Path) -> io::Result<()> {
            self.real.remove_tree(path)
        }
        fn remove_file(&self, path: &Path) -> io::Result<()> {
            self.real.remove_file(path)
        }
        fn volume(&self, path: &Path) -> io::Result<crate::platform::VolumeInfo> {
            self.real.volume(path)
        }
    }

    #[test]
    fn a_dead_unit_lists_its_fingerprint_dir_and_every_entry_carrying_its_hash() {
        let s = Scratch::new("files");
        let p = s.0.join("debug");
        let h = HASH_D;
        write_unit(&p, D, "lib-d", 104, &[], Some(30 * DAY));
        let carried = [
            ("deps", format!("libdep-{h}.rlib")),
            ("deps", format!("libdep-{h}.rmeta")),
            ("deps", format!("dep-{h}.d")),
            ("deps", format!("dep-{h}")),
            ("deps", format!("dep-{h}.abc.rcgu.o")),
            (
                "deps",
                format!("dep-{h}.dep.0123456789abcdef-cgu.02.rcgu.o"),
            ),
            ("examples", format!("ex-{h}")),
            ("examples", format!("ex-{h}.d")),
        ];
        let others = [
            ("deps", format!("dep-{h}x")),
            ("deps", h.to_string()),
            ("deps", format!("dep{h}.o")),
            ("examples", "ex".to_string()),
            ("examples", "ex.d".to_string()),
        ];
        for (sub, name) in carried.iter().chain(&others) {
            plant(&p, sub, name);
        }
        let dead = dead_of(decide(&target_of(&s.0, &[&p]), 6 * HOUR), D);
        let mut want: Vec<(String, PathBuf)> = carried
            .iter()
            .map(|(sub, name)| (name.clone(), p.join(sub).join(name)))
            .collect();
        want.sort();
        let mut expected = vec![p.join(".fingerprint").join(D)];
        expected.extend(want.into_iter().map(|(_, path)| path));
        assert_eq!(dead.files, expected);
    }

    #[test]
    fn nothing_outside_the_units_own_deps_and_examples_is_listed() {
        let s = Scratch::new("scope");
        let host = s.0.join("debug");
        let cross = s.0.join("aarch64-apple-darwin").join("debug");
        let h = HASH_D;
        write_unit(&host, D, "lib-d", 104, &[], Some(30 * DAY));
        write_unit(&cross, A, "lib-a", 101, &[], Some(HOUR));
        for (profile, sub) in [
            (&host, "build"),
            (&host, "incremental"),
            (&host, ""),
            (&host, ".fingerprint"),
            (&cross, "deps"),
            (&cross, "examples"),
        ] {
            plant(profile, sub, &format!("dep-{h}.o"));
        }
        let target = target_of(&s.0, &[&host, &cross]);
        let dead = dead_of(decide(&target, 6 * HOUR), D);
        assert!(dead.files.is_empty(), "{:?}", dead.files);

        let s = Scratch::new("scope-both");
        let host = s.0.join("debug");
        let cross = s.0.join("aarch64-apple-darwin").join("debug");
        write_unit(&host, D, "lib-d", 104, &[], Some(30 * DAY));
        write_unit(&cross, D, "lib-d", 104, &[], Some(30 * DAY));
        plant(&host, "deps", &format!("dep-{h}.o"));
        plant(&cross, "deps", &format!("dep-{h}.o"));
        let target = target_of(&s.0, &[&host, &cross]);
        let (_, dead) = decided(decide(&target, 6 * HOUR));
        assert_eq!(dead.len(), 2);
        for d in dead {
            let profile = &d.unit.profile;
            assert_eq!(
                d.files,
                [
                    profile.join(".fingerprint").join(D),
                    profile.join("deps").join(format!("dep-{h}.o"))
                ]
            );
        }
    }

    #[test]
    fn an_entry_carrying_the_hash_of_a_unit_that_is_not_dead_is_not_listed() {
        let live = (A, HASH_A);
        let never = ("s-5555555555555555", "5555555555555555");
        let stale = ("n-6666666666666666", "6666666666666666");
        let kinds: [(&str, (&str, &str)); 3] = [
            ("live", live),
            ("never collected", never),
            ("not current", stale),
        ];
        for (label, (dir, other)) in kinds {
            let s = Scratch::new("shared");
            let p = s.0.join("debug");
            let h = HASH_D;
            write_unit(&p, D, "lib-d", 104, &[], Some(30 * DAY));
            match label {
                "live" => write_unit(&p, dir, "lib-a", 101, &[], Some(HOUR)),
                "never collected" => {
                    write_unit(&p, dir, "build-script-build", 105, &[], Some(30 * DAY))
                }
                _ => never_compiled(&p, dir, 30 * DAY),
            }
            plant(&p, "deps", &format!("dep-{other}.cgu-{h}.o"));
            plant(&p, "deps", &format!("dep-{h}.cgu-{other}.o"));
            plant(&p, "examples", &format!("ex-{h}.d"));
            plant(&p, "deps", &format!("dep-{other}.o"));
            let dead = dead_of(decide(&target_of(&s.0, &[&p]), 6 * HOUR), D);
            assert_eq!(
                dead.files,
                [
                    p.join(".fingerprint").join(D),
                    p.join("examples").join(format!("ex-{h}.d"))
                ],
                "{label}"
            );
        }
    }

    #[test]
    fn a_dead_unit_with_no_entries_lists_no_file() {
        let s = Scratch::new("empty");
        let p = s.0.join("debug");
        write_unit(&p, D, "lib-d", 104, &[], Some(30 * DAY));
        plant(&p, "deps", "dep-9999999999999999.o");
        let dead = dead_of(decide(&target_of(&s.0, &[&p]), 6 * HOUR), D);
        assert!(dead.files.is_empty());
    }

    #[test]
    fn a_listing_that_fails_makes_the_target_undecided() {
        let s = Scratch::new("listfail");
        let p = s.0.join("debug");
        write_unit(&p, D, "lib-d", 104, &[], Some(30 * DAY));
        fs::write(p.join("deps"), "").unwrap();
        let reasons = undecided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
        assert!(
            matches!(&reasons[..], [Uncertain::ListingFailed { dir, .. }] if *dir == p.join("deps")),
            "{reasons:?}"
        );

        fs::remove_file(p.join("deps")).unwrap();
        let (_, dead) = decided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
        assert!(dead[0].files.is_empty());
    }

    #[test]
    fn a_listing_over_the_cap_joins_the_other_reasons() {
        let s = Scratch::new("overcap");
        let p = s.0.join("debug");
        write_unit(&p, D, "lib-d", 104, &[], Some(30 * DAY));
        write_unit(&p, A, "lib-a", 101, &[("ghost", 999)], Some(HOUR));
        let target = target_of(&s.0, &[&p]);
        let fake = FakeFs::new(Listing::FailOn("examples"));
        let reasons = undecided(liveness(&fake, &target, now(), 6 * HOUR));
        assert!(
            matches!(
                &reasons[..],
                [Uncertain::Unresolved { .. }, Uncertain::ListingFailed { dir, .. }] if *dir == p.join("examples")
            ),
            "{reasons:?}"
        );
    }

    #[test]
    fn only_profiles_holding_a_dead_unit_are_listed() {
        let s = Scratch::new("lazy");
        let host = s.0.join("debug");
        let cross = s.0.join("aarch64-apple-darwin").join("debug");
        write_unit(&host, D, "lib-d", 104, &[], Some(30 * DAY));
        write_unit(&cross, A, "lib-a", 101, &[], Some(HOUR));
        let fake = FakeFs::new(Listing::Reversed);
        let target = target_of(&s.0, &[&host, &cross]);
        decided(liveness(&fake, &target, now(), 6 * HOUR));
        let mut listed = fake.listed.borrow().clone();
        listed.sort();
        assert_eq!(listed, [host.join("deps"), host.join("examples")]);
    }

    #[test]
    fn file_lists_do_not_depend_on_listing_order() {
        let s = Scratch::new("order");
        let p = s.0.join("debug");
        write_unit(&p, D, "lib-d", 104, &[], Some(30 * DAY));
        write_unit(&p, B, "lib-b", 102, &[], Some(30 * DAY));
        for name in ["z", "a", "m", "b"] {
            plant(&p, "deps", &format!("{name}-{HASH_D}.o"));
            plant(&p, "examples", &format!("{name}-2222222222222222.o"));
        }
        let target = target_of(&s.0, &[&p]);
        let real = decide(&target, 6 * HOUR);
        let shuffled = liveness(&FakeFs::new(Listing::Reversed), &target, now(), 6 * HOUR);
        assert_eq!(real, shuffled);
        let dead = dead_of(real, D);
        assert_eq!(dead.files.len(), 5);
    }

    fn mtime_fails_with(kind: io::ErrorKind) -> FakeFs {
        FakeFs::new(Listing::MtimeFails(kind))
    }

    #[test]
    fn a_missing_timestamp_of_a_current_unit_is_not_a_root() {
        let s = Scratch::new("ts-missing");
        let p = s.0.join("debug");
        write_unit(&p, A, "lib-a", 101, &[], Some(HOUR));
        let target = target_of(&s.0, &[&p]);
        let fake = mtime_fails_with(io::ErrorKind::NotFound);
        let (live, dead) = decided(liveness(&fake, &target, now(), 6 * HOUR));
        assert!(live.is_empty());
        assert_eq!(dead[0].unit.dir_name, A);
    }

    #[test]
    fn an_unreadable_timestamp_of_a_current_unit_makes_the_target_undecided() {
        let s = Scratch::new("ts-denied");
        let p = s.0.join("debug");
        write_unit(&p, A, "lib-a", 101, &[], Some(30 * DAY));
        let target = target_of(&s.0, &[&p]);
        for kind in [io::ErrorKind::PermissionDenied, io::ErrorKind::Other] {
            let reasons = undecided(liveness(&mtime_fails_with(kind), &target, now(), 6 * HOUR));
            assert!(
                matches!(&reasons[..], [Uncertain::TimestampUnreadable { path, .. }]
                    if *path == unit_timestamp(&p, A, "lib")),
                "{kind:?}: {reasons:?}"
            );
        }
    }

    #[test]
    fn a_missing_timestamp_of_a_not_current_unit_is_ignored() {
        let s = Scratch::new("nc-ts-missing");
        let p = s.0.join("debug");
        never_compiled(&p, B, HOUR);
        let target = target_of(&s.0, &[&p]);
        let fake = mtime_fails_with(io::ErrorKind::NotFound);
        decided(liveness(&fake, &target, now(), 6 * HOUR));
    }

    #[test]
    fn an_unreadable_timestamp_of_a_not_current_unit_makes_the_target_undecided() {
        let s = Scratch::new("nc-ts-denied");
        let p = s.0.join("debug");
        never_compiled(&p, B, 30 * DAY);
        let target = target_of(&s.0, &[&p]);
        let fake = mtime_fails_with(io::ErrorKind::PermissionDenied);
        let reasons = undecided(liveness(&fake, &target, now(), 6 * HOUR));
        assert!(
            matches!(&reasons[..], [Uncertain::TimestampUnreadable { path, .. }]
                if *path == p.join(".fingerprint").join(B).join("invoked.timestamp")),
            "{reasons:?}"
        );
    }

    #[test]
    fn a_not_current_unit_dir_whose_name_is_not_utf8_has_no_unit_hash() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let s = Scratch::new("nc-nonutf8");
        let p = s.0.join("debug");
        let odd = p
            .join(".fingerprint")
            .join(OsStr::from_bytes(b"caf\xff-1111111111111111"));
        fs::create_dir_all(p.join(".fingerprint")).unwrap();
        let mut target = target_of(&s.0, &[&p]);
        target.profiles[0]
            .not_current_units
            .push(crate::fingerprint::NotCurrentUnit {
                dir: odd.clone(),
                stem: None,
                features: None,
                outputs: Vec::new(),
            });
        let reasons = undecided(decide(&target, 6 * HOUR));
        assert_eq!(reasons, [Uncertain::NoUnitHash { dir: odd }]);
    }
}
