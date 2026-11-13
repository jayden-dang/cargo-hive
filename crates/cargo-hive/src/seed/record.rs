//! The seed record in `seeds.json`. One target keeps one record. A write drops
//! records whose target is gone, then the oldest, until the file fits its cap.
//! A file that cannot be read is replaced, and the note says so. `counts`
//! reports what became of that record's units in the target as it stands now.
//! A `deps/` listing that hits its bound or cannot be read is an error, not a count.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::{Deserialize, Serialize};

use cargo_hive_core::fingerprint::{self, DepEdge};
use cargo_hive_core::liveness;
use cargo_hive_core::target::TargetDir;
use cargo_hive_state::{State, StateError};

pub const SEEDS_FILE: &str = "seeds.json";
pub const MAX_SEEDS_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeededUnit {
    pub rel_profile: PathBuf,
    pub dir_name: String,
    pub stamp: Option<(PathBuf, u64)>,
    pub has_build: bool,
    pub deps_files: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeedRecord {
    pub identity: (u64, u64),
    pub written_at: u64,
    pub units: Vec<SeededUnit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SeedsFile {
    pub targets: BTreeMap<PathBuf, SeedRecord>,
}

pub fn write(
    state: &State,
    target: &Path,
    record: SeedRecord,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Option<String>, StateError> {
    write_capped(state, target, record, cancel, MAX_SEEDS_BYTES)
}

pub fn write_capped(
    state: &State,
    target: &Path,
    record: SeedRecord,
    cancel: &(dyn Fn() -> bool + Sync),
    cap: u64,
) -> Result<Option<String>, StateError> {
    state.locked_or_cancelled(cancel, |tx| {
        let (mut file, note) = match tx.read_bounded(SEEDS_FILE, cap) {
            Ok(file) => (file, None),
            Err(err) => (SeedsFile::default(), Some(replaced(&err))),
        };
        file.targets.insert(target.to_path_buf(), record);
        file.targets.retain(|path, _| target_present(path));
        drop_oldest_until(&mut file, cap, target);
        tx.write_bounded(SEEDS_FILE, &file, cap)?;
        Ok(note)
    })
}

pub fn read(state: &State) -> Result<SeedsFile, StateError> {
    state.read_bounded(SEEDS_FILE, MAX_SEEDS_BYTES)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeedCounts {
    pub fresh: u64,
    pub unneeded: u64,
    pub recompiled: u64,
    pub missing: u64,
}

pub fn counts(
    target: &TargetDir,
    record: &SeedRecord,
    deps_cap: usize,
) -> Result<Option<SeedCounts>, String> {
    let Some(identity) = dir_identity(&target.path) else {
        return Ok(None);
    };
    if identity != record.identity {
        return Ok(None);
    }
    let deps = dep_listings(target, record, deps_cap)?;
    let reached = reached_units(target, record);
    let mut fresh = 0u64;
    let mut unneeded = 0u64;
    let mut recompiled = 0u64;
    let mut missing = 0u64;
    for unit in &record.units {
        let names = names_for(&deps, &target.path, &unit.rel_profile);
        let key = (unit.rel_profile.clone(), unit.dir_name.clone());
        if unit_missing(&target.path, unit, names, deps_cap)? {
            missing += 1;
        } else if stamp_differs(&target.path, unit) {
            recompiled += 1;
        } else if reached.contains(&key) {
            fresh += 1;
        } else {
            unneeded += 1;
        }
    }
    Ok(Some(SeedCounts {
        fresh,
        unneeded,
        recompiled,
        missing,
    }))
}

fn dir_identity(path: &Path) -> Option<(u64, u64)> {
    let meta = fs::symlink_metadata(path).ok()?;
    Some((meta.dev(), meta.ino()))
}

fn dep_listings(
    target: &TargetDir,
    record: &SeedRecord,
    cap: usize,
) -> Result<BTreeMap<PathBuf, Vec<String>>, String> {
    let mut dirs = BTreeSet::new();
    for profile in &target.profiles {
        dirs.insert(profile.path.join("deps"));
    }
    for unit in &record.units {
        dirs.insert(target.path.join(&unit.rel_profile).join("deps"));
    }
    let mut listed = BTreeMap::new();
    for dir in dirs {
        let names = list_dir_names(&dir, cap)?;
        if let Some(profile) = dir.parent() {
            listed.insert(profile.to_path_buf(), names);
        }
    }
    Ok(listed)
}

fn list_dir_names(dir: &Path, cap: usize) -> Result<Vec<String>, String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(readable(dir, &err)),
    };
    let mut names = Vec::new();
    for (seen, entry) in entries.enumerate() {
        if seen >= cap {
            return Err(format!(
                "{} has more than {cap} entries; shrink the directory below its cap and build the target again",
                dir.display()
            ));
        }
        let entry = entry.map_err(|err| readable(dir, &err))?;
        let name = entry.file_name().into_string().map_err(|_| {
            format!(
                "{} has a name that is not valid UTF-8; rename that entry to valid UTF-8 and build the target again",
                dir.display()
            )
        })?;
        names.push(name);
    }
    Ok(names)
}

fn readable(path: &Path, err: &io::Error) -> String {
    format!(
        "{}: {err}; make it readable and build the target again",
        path.display()
    )
}

fn names_for<'a>(
    listings: &'a BTreeMap<PathBuf, Vec<String>>,
    target: &Path,
    rel: &Path,
) -> &'a [String] {
    listings
        .get(&target.join(rel))
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn unit_missing(
    target: &Path,
    unit: &SeededUnit,
    deps: &[String],
    cap: usize,
) -> Result<bool, String> {
    let profile = target.join(&unit.rel_profile);
    let present = counted_deps(&profile.join("deps"), deps, &unit.dir_name, cap)?;
    let unit_dir_gone = !is_dir(&profile.join(".fingerprint").join(&unit.dir_name));
    let build_gone = unit.has_build && !is_dir(&profile.join("build").join(&unit.dir_name));
    Ok(unit_dir_gone || build_gone || present < unit.deps_files)
}

fn is_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir())
}

fn counted_deps(
    deps_dir: &Path,
    names: &[String],
    dir_name: &str,
    cap: usize,
) -> Result<u32, String> {
    let Some(meta) = fingerprint::unit_hash(dir_name) else {
        return Ok(0);
    };
    let mut count = 0u32;
    for name in names {
        if name.ends_with(".rcgu.o") || !liveness::embedded_hashes(name).any(|hash| hash == meta) {
            continue;
        }
        count = count.saturating_add(count_files(&deps_dir.join(name), cap, 0)?);
    }
    Ok(count)
}

/// Regular files at `path`. A directory is walked down to `depth`; `.rcgu.o` is skipped.
fn count_files(path: &Path, cap: usize, depth: usize) -> Result<u32, String> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(readable(path, &err)),
    };
    if meta.file_type().is_symlink() {
        return Ok(0);
    }
    if meta.is_file() {
        return Ok(1);
    }
    if !meta.is_dir() {
        return Ok(0);
    }
    if depth > cargo_hive_core::size::MAX_DEPTH {
        return Err(format!(
            "{} is deeper than {} levels; remove directories past that depth and build the target again",
            path.display(),
            cargo_hive_core::size::MAX_DEPTH
        ));
    }
    let mut count = 0u32;
    for name in list_dir_names(path, cap)? {
        if name.ends_with(".rcgu.o") {
            continue;
        }
        count = count.saturating_add(count_files(&path.join(name), cap, depth + 1)?);
    }
    Ok(count)
}

fn stamp_differs(target: &Path, unit: &SeededUnit) -> bool {
    match &unit.stamp {
        Some((rel, recorded)) => file_mtime_ns(&target.join(rel)) != Some(*recorded),
        None => stamp_present(target, unit),
    }
}

fn stamp_present(target: &Path, unit: &SeededUnit) -> bool {
    let profile = target.join(&unit.rel_profile);
    [".fingerprint", "build"].into_iter().any(|base| {
        fs::metadata(
            profile
                .join(base)
                .join(&unit.dir_name)
                .join(fingerprint::INVOKED_TIMESTAMP),
        )
        .is_ok()
    })
}

fn file_mtime_ns(path: &Path) -> Option<u64> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    u64::try_from(modified.duration_since(UNIX_EPOCH).ok()?.as_nanos()).ok()
}

struct GraphNode {
    rel: PathBuf,
    dir_name: String,
    deps: Vec<DepEdge>,
}

fn reached_units(target: &TargetDir, record: &SeedRecord) -> BTreeSet<(PathBuf, String)> {
    let mut nodes = Vec::new();
    let mut by_hash: HashMap<String, Vec<usize>> = HashMap::new();
    for profile in &target.profiles {
        let Some(graph) = &profile.graph else {
            continue;
        };
        let rel = profile
            .path
            .strip_prefix(&target.path)
            .unwrap_or(profile.path.as_path())
            .to_path_buf();
        for unit in &graph.units {
            let idx = nodes.len();
            by_hash.entry(unit.hash_hex.clone()).or_default().push(idx);
            nodes.push(GraphNode {
                rel: rel.clone(),
                dir_name: unit.dir_name.clone(),
                deps: unit.deps.clone(),
            });
        }
    }
    let mut queue = VecDeque::new();
    let mut seen = HashSet::new();
    for (idx, node) in nodes.iter().enumerate() {
        let recorded = record
            .units
            .iter()
            .find(|unit| unit.rel_profile == node.rel && unit.dir_name == node.dir_name);
        let start = match recorded {
            None => true,
            Some(unit) => stamp_differs(&target.path, unit),
        };
        if start && seen.insert(idx) {
            queue.push_back(idx);
        }
    }
    let mut reached = BTreeSet::new();
    while let Some(idx) = queue.pop_front() {
        let edges: Vec<u64> = nodes[idx].deps.iter().map(|dep| dep.fingerprint).collect();
        for edge in edges {
            let Some(nexts) = by_hash.get(&fingerprint::le_hex(edge)) else {
                continue;
            };
            for &next in nexts {
                if seen.insert(next) {
                    queue.push_back(next);
                    let node = &nodes[next];
                    reached.insert((node.rel.clone(), node.dir_name.clone()));
                }
            }
        }
    }
    reached
}

fn replaced(err: &StateError) -> String {
    format!("{err}; this seed replaced the record file")
}

fn target_present(path: &Path) -> bool {
    match fs::symlink_metadata(path) {
        Ok(_) => true,
        Err(err) if err.kind() == io::ErrorKind::NotFound => false,
        Err(_) => true,
    }
}

fn drop_oldest_until(file: &mut SeedsFile, cap: u64, inserted: &Path) {
    while encoded_len(file).is_some_and(|len| len > cap) {
        let Some(oldest) = oldest_other(file, inserted) else {
            break;
        };
        file.targets.remove(&oldest);
    }
}

fn encoded_len(file: &SeedsFile) -> Option<u64> {
    let mut bytes = serde_json::to_vec_pretty(file).ok()?;
    bytes.push(b'\n');
    Some(bytes.len() as u64)
}

fn oldest_other(file: &SeedsFile, inserted: &Path) -> Option<PathBuf> {
    let max_at = file
        .targets
        .values()
        .map(|record| record.written_at)
        .max()?;
    file.targets
        .iter()
        .filter(|(path, record)| {
            record.written_at < max_at
                || (record.written_at == max_at && path.as_path() != inserted)
        })
        .min_by(|(left_path, left), (right_path, right)| {
            left.written_at
                .cmp(&right.written_at)
                .then_with(|| left_path.cmp(right_path))
        })
        .map(|(path, _)| path.clone())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};
    use std::time::UNIX_EPOCH;

    use super::*;

    use cargo_hive_core::fingerprint::{Coverage, DepEdge, Format, ProfileReport, Unit, UnitGraph};
    use cargo_hive_core::target::{Owner, TargetDir};
    use cargo_hive_state::State;

    /// A scratch root under /private/tmp, removed when the test drops it.
    struct Root(PathBuf);

    impl std::ops::Deref for Root {
        type Target = PathBuf;
        fn deref(&self) -> &PathBuf {
            &self.0
        }
    }

    impl Drop for Root {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn scratch(label: &str) -> (State, Root) {
        let root = PathBuf::from(format!(
            "/private/tmp/hive-it-seedrec-{label}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let state = State::open_at(root.join("state")).unwrap();
        (state, Root(root))
    }

    fn target(root: &Path, name: &str) -> PathBuf {
        let path = root.join(name);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn record(identity: (u64, u64), written_at: u64, pad: usize) -> SeedRecord {
        SeedRecord {
            identity,
            written_at,
            units: vec![SeededUnit {
                rel_profile: PathBuf::from("debug"),
                dir_name: "n".repeat(pad),
                stamp: Some((
                    PathBuf::from(format!("debug/build/{}/invoked.timestamp", "n".repeat(pad))),
                    5,
                )),
                has_build: true,
                deps_files: 1,
            }],
        }
    }

    fn file_of(entries: &[(&Path, SeedRecord)]) -> SeedsFile {
        let mut targets = BTreeMap::new();
        for (path, stored) in entries {
            targets.insert((*path).to_path_buf(), stored.clone());
        }
        SeedsFile { targets }
    }

    fn encoded(file: &SeedsFile) -> Vec<u8> {
        let mut bytes = serde_json::to_vec_pretty(file).unwrap();
        bytes.push(b'\n');
        bytes
    }

    fn fitted_cap(old: &SeedsFile, newest: &SeedsFile, both: &SeedsFile) -> u64 {
        let cap = encoded(newest).len() as u64;
        assert!(encoded(old).len() as u64 <= cap);
        assert!(encoded(both).len() as u64 > cap);
        assert!(encoded(newest).len() as u64 <= cap);
        cap
    }

    #[test]
    fn a_write_drops_records_whose_target_is_gone() {
        let (state, root) = scratch("gone");
        let gone = target(&root, "gone");
        let kept = target(&root, "kept");
        assert!(
            super::write(&state, &gone, record((1, 9), 100, 4), &|| false)
                .unwrap()
                .is_none()
        );
        fs::remove_dir_all(&gone).unwrap();
        assert!(
            super::write(&state, &kept, record((2, 8), 1, 4), &|| false)
                .unwrap()
                .is_none()
        );
        let file = read(&state).unwrap();
        assert!(!file.targets.contains_key(&gone), "{file:?}");
        let kept_record = file.targets.get(&kept).expect("the live target stays");
        assert_eq!(kept_record.identity, (2, 8));
        assert_eq!(kept_record.written_at, 1);
        assert_eq!(kept_record.units.len(), 1);
    }

    #[test]
    fn a_write_over_the_cap_drops_the_oldest_records_until_it_fits() {
        let (state, root) = scratch("cap");
        let older = target(&root, "older");
        let middle = target(&root, "middle");
        let newest = target(&root, "newest");
        let first = record((1, 1), 1, 8);
        let second = record((2, 2), 2, 8);
        let mut pad = 8usize;
        let (third, cap) = loop {
            assert!(pad < 20_000, "no pad made the newest the one that fits");
            let third = record((3, 3), 3, pad);
            let old = file_of(&[(&older, first.clone()), (&middle, second.clone())]);
            let alone = file_of(&[(&newest, third.clone())]);
            let with_middle = file_of(&[(&middle, second.clone()), (&newest, third.clone())]);
            if encoded(&old).len() as u64 <= encoded(&alone).len() as u64
                && encoded(&with_middle).len() as u64 > encoded(&alone).len() as u64
            {
                break (third, fitted_cap(&old, &alone, &with_middle));
            }
            pad += 32;
        };
        assert!(
            write_capped(&state, &older, first, &|| false, cap)
                .unwrap()
                .is_none()
        );
        assert!(
            write_capped(&state, &middle, second, &|| false, cap)
                .unwrap()
                .is_none()
        );
        let before = read(&state).unwrap();
        assert!(before.targets.contains_key(&older), "{before:?}");
        assert!(before.targets.contains_key(&middle), "{before:?}");
        assert!(
            write_capped(&state, &newest, third.clone(), &|| false, cap)
                .unwrap()
                .is_none()
        );
        let file = read(&state).unwrap();
        assert!(!file.targets.contains_key(&older), "{file:?}");
        assert!(!file.targets.contains_key(&middle), "{file:?}");
        assert_eq!(
            file.targets.get(&newest).map(|row| &row.units),
            Some(&third.units)
        );
        let on_disk = fs::metadata(state.root().join(SEEDS_FILE)).unwrap().len();
        assert!(on_disk <= cap, "on disk {on_disk} cap {cap}");
    }

    #[test]
    fn the_newest_record_survives_the_cap() {
        let (state, root) = scratch("newest");
        let older = target(&root, "older");
        let middle = target(&root, "middle");
        let newest = target(&root, "newest");
        let first = record((4, 1), 10, 4);
        let second = record((4, 2), 20, 4);
        let mut pad = 4usize;
        let (third, cap) = loop {
            assert!(pad < 20_000, "no pad left the newest under the cap");
            let third = record((4, 3), 30, pad);
            let old = file_of(&[(&older, first.clone()), (&middle, second.clone())]);
            let alone = file_of(&[(&newest, third.clone())]);
            let all = file_of(&[
                (&older, first.clone()),
                (&middle, second.clone()),
                (&newest, third.clone()),
            ]);
            if encoded(&old).len() <= encoded(&alone).len()
                && encoded(&all).len() > encoded(&alone).len()
            {
                break (third, encoded(&alone).len() as u64);
            }
            pad += 32;
        };
        write_capped(&state, &older, first, &|| false, cap).unwrap();
        write_capped(&state, &middle, second, &|| false, cap).unwrap();
        write_capped(&state, &newest, third.clone(), &|| false, cap).unwrap();
        let file = read(&state).unwrap();
        let kept = file
            .targets
            .get(&newest)
            .expect("the newest record is not the one dropped");
        assert_eq!(kept.written_at, 30);
        assert_eq!(kept.units, third.units);
        assert!(file.targets.len() < 3, "{file:?}");
        let on_disk = fs::metadata(state.root().join(SEEDS_FILE)).unwrap().len();
        assert!(on_disk <= cap, "on disk {on_disk} cap {cap}");
    }

    #[test]
    fn an_unreadable_seeds_file_is_replaced_and_the_note_names_it() {
        let (state, root) = scratch("unreadable");
        let kept = target(&root, "kept");
        fs::write(state.root().join(SEEDS_FILE), b"{not json").unwrap();
        let note = super::write(&state, &kept, record((7, 7), 7, 3), &|| false)
            .unwrap()
            .expect("an unreadable file is a note");
        assert!(note.contains("seeds.json"), "{note}");
        let file = read(&state).unwrap();
        assert_eq!(file.targets.len(), 1, "{file:?}");
        assert!(file.targets.contains_key(&kept), "{file:?}");
    }

    #[derive(Clone, Copy)]
    enum Stamp {
        Same,
        Differ,
        Appeared,
    }

    #[derive(Clone)]
    struct Spec {
        id: u64,
        recorded: bool,
        in_graph: bool,
        deps: Vec<u64>,
        stamp: Stamp,
        fingerprint: bool,
        has_build: bool,
        build_dir: bool,
        files: u32,
        recorded_files: u32,
        rcgu: bool,
        dir_files: u32,
    }

    fn seeded(id: u64) -> Spec {
        Spec {
            id,
            recorded: true,
            in_graph: true,
            deps: Vec::new(),
            stamp: Stamp::Same,
            fingerprint: true,
            has_build: false,
            build_dir: false,
            files: 1,
            recorded_files: 1,
            rcgu: false,
            dir_files: 0,
        }
    }

    fn outside(id: u64, deps: &[u64]) -> Spec {
        let mut spec = seeded(id);
        spec.recorded = false;
        spec.deps = deps.to_vec();
        spec.files = 0;
        spec.recorded_files = 0;
        spec
    }

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn mtime_ns(path: &Path) -> Option<u64> {
        let modified = fs::metadata(path).ok()?.modified().ok()?;
        u64::try_from(modified.duration_since(UNIX_EPOCH).ok()?.as_nanos()).ok()
    }

    fn build(label: &str, specs: &[Spec]) -> (Scratch, TargetDir, SeedRecord) {
        let root = PathBuf::from(format!(
            "/private/tmp/hive-it-seedcnt-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = fs::remove_dir_all(&root);
        let target = root.join("target");
        let profile = target.join("debug");
        fs::create_dir_all(profile.join("deps")).unwrap();
        let mut graph_units = Vec::new();
        let mut recorded = Vec::new();
        for spec in specs {
            let hex = cargo_hive_core::fingerprint::le_hex(spec.id);
            let dir_name = format!("unit-{hex}");
            if spec.fingerprint {
                let dir = profile.join(".fingerprint").join(&dir_name);
                fs::create_dir_all(&dir).unwrap();
                if matches!(spec.stamp, Stamp::Same | Stamp::Differ | Stamp::Appeared) {
                    fs::write(dir.join("invoked.timestamp"), b"t").unwrap();
                }
            }
            if spec.build_dir {
                fs::create_dir_all(profile.join("build").join(&dir_name)).unwrap();
            }
            for n in 0..spec.files {
                fs::write(profile.join("deps").join(format!("f{n}-{hex}.rlib")), b"d").unwrap();
            }
            if spec.dir_files > 0 {
                let bundle = profile.join("deps").join(format!("bundle-{hex}"));
                fs::create_dir_all(&bundle).unwrap();
                for n in 0..spec.dir_files {
                    fs::write(bundle.join(format!("part{n}")), b"d").unwrap();
                }
                fs::write(bundle.join("noise.rcgu.o"), b"o").unwrap();
            }
            if spec.rcgu {
                fs::write(
                    profile.join("deps").join(format!("extra-{hex}.rcgu.o")),
                    b"o",
                )
                .unwrap();
            }
            if spec.in_graph {
                graph_units.push(Unit {
                    dir_name: dir_name.clone(),
                    kind: "lib".to_string(),
                    hash_hex: hex.clone(),
                    deps: spec
                        .deps
                        .iter()
                        .map(|fingerprint| DepEdge {
                            package: 0,
                            fingerprint: *fingerprint,
                        })
                        .collect(),
                    stem: "unit".to_string(),
                    features: std::sync::Arc::from(""),
                    coverage: Coverage::NotRecorded,
                    profile: 0,
                    target: 0,
                    path: 0,
                    toolchain_value: 0,
                });
            }
            if spec.recorded {
                let rel = PathBuf::from(format!("debug/.fingerprint/{dir_name}/invoked.timestamp"));
                let stamp = match spec.stamp {
                    Stamp::Appeared => None,
                    Stamp::Same | Stamp::Differ => {
                        let ns = mtime_ns(&target.join(&rel)).unwrap_or(1);
                        let recorded_ns = match spec.stamp {
                            Stamp::Differ => ns.wrapping_add(1),
                            _ => ns,
                        };
                        Some((rel, recorded_ns))
                    }
                };
                recorded.push(SeededUnit {
                    rel_profile: PathBuf::from("debug"),
                    dir_name,
                    stamp,
                    has_build: spec.has_build,
                    deps_files: spec.recorded_files,
                });
            }
        }
        let meta = fs::symlink_metadata(&target).unwrap();
        let target_dir = TargetDir {
            path: target,
            owner: Owner::Unresolved {
                reason: "fixture".to_string(),
            },
            profiles: vec![ProfileReport {
                path: profile,
                format: Format::Supported,
                units: graph_units.len(),
                not_current: 0,
                not_current_units: Vec::new(),
                edges: None,
                graph: Some(UnitGraph {
                    units: graph_units,
                    edges: Vec::new(),
                }),
            }],
            size: None,
            unreadable: None,
        };
        let record = SeedRecord {
            identity: (meta.dev(), meta.ino()),
            written_at: 1,
            units: recorded,
        };
        (Scratch(root), target_dir, record)
    }

    fn assert_buckets(label: &str, specs: &[Spec], expect: SeedCounts) {
        let (_scratch, target, record) = build(label, specs);
        let got = counts(&target, &record, 1_000)
            .unwrap_or_else(|err| panic!("{label}: {err}"))
            .unwrap_or_else(|| panic!("{label}: no counts"));
        let recorded = record.units.len() as u64;
        assert_eq!(
            got.fresh + got.unneeded + got.recompiled + got.missing,
            recorded,
            "{label}: every seeded unit is in one bucket, got {got:?}"
        );
        assert_eq!(got, expect, "{label}");
    }

    #[test]
    fn each_seeded_unit_lands_in_one_bucket() {
        let mut direct = seeded(2);
        direct.has_build = true;
        direct.build_dir = true;
        let mut through = seeded(2);
        through.deps = vec![3];
        let mut restamped = seeded(5);
        restamped.stamp = Stamp::Differ;
        restamped.deps = vec![6];
        let mut born = seeded(7);
        born.stamp = Stamp::Appeared;
        born.deps = vec![8];
        let mut no_fingerprint = seeded(9);
        no_fingerprint.fingerprint = false;
        let mut no_build = seeded(10);
        no_build.has_build = true;
        no_build.build_dir = false;
        let mut short = seeded(11);
        short.files = 1;
        short.recorded_files = 2;
        short.rcgu = true;
        let mut only_rcgu = seeded(12);
        only_rcgu.files = 0;
        only_rcgu.recorded_files = 1;
        only_rcgu.rcgu = true;
        let mut aside = seeded(13);
        aside.in_graph = false;
        let mut bundled = seeded(14);
        bundled.files = 0;
        bundled.dir_files = 2;
        bundled.recorded_files = 2;
        let rows: Vec<(&str, Vec<Spec>, SeedCounts)> = vec![
            (
                "reached from a unit outside the record",
                vec![outside(1, &[2]), direct],
                SeedCounts {
                    fresh: 1,
                    unneeded: 0,
                    recompiled: 0,
                    missing: 0,
                },
            ),
            (
                "reached only through another seeded unit",
                vec![outside(1, &[2]), through, seeded(3)],
                SeedCounts {
                    fresh: 2,
                    unneeded: 0,
                    recompiled: 0,
                    missing: 0,
                },
            ),
            (
                "not reached",
                vec![seeded(4)],
                SeedCounts {
                    fresh: 0,
                    unneeded: 1,
                    recompiled: 0,
                    missing: 0,
                },
            ),
            (
                "stamp differs and starts the walk",
                vec![restamped, seeded(6)],
                SeedCounts {
                    fresh: 1,
                    unneeded: 0,
                    recompiled: 1,
                    missing: 0,
                },
            ),
            (
                "no stamp at seed and a stamp now",
                vec![born, seeded(8)],
                SeedCounts {
                    fresh: 1,
                    unneeded: 0,
                    recompiled: 1,
                    missing: 0,
                },
            ),
            (
                "fingerprint directory gone",
                vec![no_fingerprint],
                SeedCounts {
                    fresh: 0,
                    unneeded: 0,
                    recompiled: 0,
                    missing: 1,
                },
            ),
            (
                "build directory gone",
                vec![no_build],
                SeedCounts {
                    fresh: 0,
                    unneeded: 0,
                    recompiled: 0,
                    missing: 1,
                },
            ),
            (
                "fewer dep files than recorded",
                vec![short],
                SeedCounts {
                    fresh: 0,
                    unneeded: 0,
                    recompiled: 0,
                    missing: 1,
                },
            ),
            (
                "a new rcgu object does not hide a missing file",
                vec![only_rcgu],
                SeedCounts {
                    fresh: 0,
                    unneeded: 0,
                    recompiled: 0,
                    missing: 1,
                },
            ),
            (
                "absent from the graph",
                vec![aside],
                SeedCounts {
                    fresh: 0,
                    unneeded: 1,
                    recompiled: 0,
                    missing: 0,
                },
            ),
            (
                "a dep directory counts the files inside it",
                vec![bundled],
                SeedCounts {
                    fresh: 0,
                    unneeded: 1,
                    recompiled: 0,
                    missing: 0,
                },
            ),
        ];
        for (label, specs, expect) in &rows {
            assert_buckets(label, specs, *expect);
        }

        let mut through = seeded(2);
        through.deps = vec![3];
        let through_again = through.clone();
        let mut restamped = seeded(5);
        restamped.stamp = Stamp::Differ;
        restamped.deps = vec![6];
        let mut born = seeded(7);
        born.stamp = Stamp::Appeared;
        born.deps = vec![8];
        let mut no_fingerprint = seeded(9);
        no_fingerprint.fingerprint = false;
        let mut no_build = seeded(10);
        no_build.has_build = true;
        no_build.build_dir = false;
        let mut short = seeded(11);
        short.files = 1;
        short.recorded_files = 2;
        short.rcgu = true;
        let mut only_rcgu = seeded(12);
        only_rcgu.files = 0;
        only_rcgu.recorded_files = 1;
        only_rcgu.rcgu = true;
        let mut aside = seeded(13);
        aside.in_graph = false;
        let mut bundled = seeded(14);
        bundled.files = 0;
        bundled.dir_files = 2;
        bundled.recorded_files = 2;
        assert_buckets(
            "together",
            &[
                outside(1, &[2]),
                through_again,
                seeded(3),
                seeded(4),
                restamped,
                seeded(6),
                born,
                seeded(8),
                no_fingerprint,
                no_build,
                short,
                only_rcgu,
                aside,
                bundled,
            ],
            SeedCounts {
                fresh: 4,
                unneeded: 3,
                recompiled: 2,
                missing: 4,
            },
        );
    }

    #[test]
    fn counts_are_absent_when_the_target_identity_differs() {
        let (_scratch, target, mut record) = build("identity", &[seeded(1)]);
        record.identity = (
            record.identity.0.wrapping_add(1),
            record.identity.1.wrapping_add(1),
        );
        assert_eq!(counts(&target, &record, 1_000), Ok(None));
    }

    #[test]
    fn a_deps_listing_over_its_cap_gives_no_counts() {
        let (scratch, mut target, record) = build("cap", &[seeded(1)]);
        let deps = scratch.0.join("target/debug/deps");
        fs::write(deps.join("extra-a"), b"a").unwrap();
        fs::write(deps.join("extra-b"), b"b").unwrap();
        fs::write(deps.join("extra-c"), b"c").unwrap();
        target.profiles[0].path = scratch.0.join("target/debug");
        let err = counts(&target, &record, 2).expect_err("a listing past its cap");
        let shown = deps.display().to_string();
        assert!(
            err.contains(&shown) && err.contains("shrink the directory"),
            "{err}"
        );
    }

    #[test]
    fn deleting_one_file_inside_a_dep_directory_makes_the_unit_missing() {
        let mut bundled = seeded(14);
        bundled.files = 0;
        bundled.dir_files = 2;
        bundled.recorded_files = 2;
        let (scratch, target, record) = build("depdir", &[bundled]);
        let kept = counts(&target, &record, 1_000)
            .unwrap()
            .expect("two files inside the directory");
        assert_eq!(kept.missing, 0, "{kept:?}");
        assert_eq!(kept.unneeded, 1, "{kept:?}");
        let hex = cargo_hive_core::fingerprint::le_hex(14);
        let part = scratch
            .0
            .join("target/debug/deps")
            .join(format!("bundle-{hex}"))
            .join("part0");
        fs::remove_file(&part).unwrap();
        let gone = counts(&target, &record, 1_000)
            .unwrap()
            .expect("one file left inside the directory");
        assert_eq!(
            gone,
            SeedCounts {
                fresh: 0,
                unneeded: 0,
                recompiled: 0,
                missing: 1,
            }
        );
    }
}
