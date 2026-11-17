//! Decides which units of a target dir are live and which are dead.
//! Read-only: it reads fingerprint reports and invocation mtimes through `Fs`,
//! runs no process and writes nothing.

use cargo_hive_core::bitset::BitSet;
use cargo_hive_core::caps::DEPS_CAP;
use cargo_hive_core::fingerprint::{self, DepEdge, Format, Unit, UnitHash, UnitKind, le_hex};
use cargo_hive_core::liveness::{
    DeadUnit, Liveness, Uncertain, UnitRef, embedded_hashes, unit_timestamp,
};
use cargo_hive_core::target::TargetDir;
use cargo_hive_core::unit_path::{self, UnitLayout, UnitLoc};
use cargo_hive_platform::{EntryKind, Fs};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

fn in_window(mtime: SystemTime, now: SystemTime, window: Duration) -> bool {
    now.checked_sub(window).is_none_or(|cutoff| mtime >= cutoff)
}

/// The mtime of a file that dates a unit's invocation. A file that does not exist dates nothing;
/// any other read error leaves the question open, so the caller must not delete.
fn file_time(fs: &dyn Fs, path: &Path) -> Result<Option<SystemTime>, Uncertain> {
    match fs.mtime(path) {
        Ok(mtime) => Ok(Some(mtime)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(Uncertain::TimestampUnreadable {
            path: path.to_path_buf(),
            why: e.to_string(),
        }),
    }
}

/// Whether the unit was invoked inside the window.
fn invoked_in_window(
    fs: &dyn Fs,
    timestamp: &Path,
    now: SystemTime,
    window: Duration,
) -> Result<bool, Uncertain> {
    Ok(file_time(fs, timestamp)?.is_some_and(|mtime| in_window(mtime, now, window)))
}

/// A not-current unit with a fingerprint report: it keeps what it last wanted alive while a
/// command wanted it inside the window.
struct NotCurrentNode<'a> {
    profile: &'a Path,
    loc: &'a UnitLoc,
    stem: &'a str,
    deps: &'a [DepEdge],
}

impl NotCurrentNode<'_> {
    fn unit_ref(&self) -> UnitRef {
        make_unit_ref(self.profile, self.loc)
    }

    /// The later of the unit timestamp's and the emptied hash file's mtimes. Both files are read
    /// even when the first fails, so every unreadable one is reported.
    fn last_wanted(&self, fs: &dyn Fs, reasons: &mut Vec<Uncertain>) -> Option<SystemTime> {
        let kind = UnitKind::of(self.stem);
        let stamp = unit_timestamp(self.profile, self.loc, kind);
        let hash = unit_path::fingerprint_dir(self.profile, self.loc).join(self.stem);
        let mut latest = None;
        for path in [stamp, hash] {
            match file_time(fs, &path) {
                Ok(time) => latest = latest.max(time),
                Err(reason) => reasons.push(reason),
            }
        }
        latest
    }
}

/// Decides which units of `target` are live and which dead, or why it cannot decide.
pub fn liveness(fs: &dyn Fs, target: &TargetDir, now: SystemTime, window: Duration) -> Liveness {
    let mut reasons: Vec<Uncertain> = Vec::new();
    if let Some(why) = &target.unreadable {
        reasons.push(Uncertain::TargetUnreadable {
            target: target.path.clone(),
            why: why.clone(),
        });
    }

    let (units, nodes, not_current_hashes) = collect_units(fs, target, now, window, &mut reasons);
    let index = index_units(&units, &mut reasons);
    let marked = mark_live(fs, &units, &nodes, &index, now, window, &mut reasons);
    let live = marked.units;
    let live_units: BTreeSet<UnitRef> = units
        .iter()
        .enumerate()
        .filter(|(i, _)| live.contains(*i))
        .map(|(_, entry)| unit_ref(entry))
        .chain(marked.live_nodes)
        .collect();
    let dead_idx = dead_indices(&units, &live);
    let listings = list_dead_files(fs, &units, &dead_idx, &mut reasons);

    if !reasons.is_empty() {
        reasons.sort();
        reasons.dedup();
        return Liveness::Undecided { reasons };
    }

    assemble(&units, &not_current_hashes, live_units, &dead_idx, listings)
}

type ProfileUnit<'a> = (&'a Path, &'a Unit);

type Collected<'a> = (
    Vec<ProfileUnit<'a>>,
    Vec<NotCurrentNode<'a>>,
    Vec<(&'a Path, UnitHash)>,
);

/// Walks every profile: reports unsupported profiles and never-compiled units, and gathers the
/// units of the supported ones, the not-current units with a report, and the hashes of all
/// not-current unit dirs.
fn collect_units<'a>(
    fs: &dyn Fs,
    target: &'a TargetDir,
    now: SystemTime,
    window: Duration,
    reasons: &mut Vec<Uncertain>,
) -> Collected<'a> {
    let mut units: Vec<ProfileUnit> = Vec::new();
    let mut nodes: Vec<NotCurrentNode> = Vec::new();
    let mut not_current_hashes: Vec<(&Path, UnitHash)> = Vec::new();
    for profile in &target.profiles {
        if let Format::Unsupported(why) = &profile.format {
            reasons.push(Uncertain::ProfileUnsupported {
                profile: profile.path.clone(),
                why: why.clone(),
            });
            continue;
        }
        for nc in &profile.not_current_units {
            if nc.loc.name_meta.is_empty() {
                reasons.push(Uncertain::NoUnitHash {
                    dir: nc.dir.clone(),
                });
                continue;
            }
            if let Some(hash) = fingerprint::unit_hash(&nc.loc.name_meta) {
                not_current_hashes.push((profile.path.as_path(), hash));
            }
            if let Some(stem) = nc.stem.as_deref() {
                nodes.push(NotCurrentNode {
                    profile: profile.path.as_path(),
                    loc: &nc.loc,
                    stem,
                    deps: &nc.deps,
                });
                continue;
            }
            let stamp = unit_timestamp(&profile.path, &nc.loc, UnitKind::Other);
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
    (units, nodes, not_current_hashes)
}

/// Indexes units by their report hash, and reports dirs without a unit hash or sharing one.
fn index_units(
    units: &[ProfileUnit<'_>],
    reasons: &mut Vec<Uncertain>,
) -> HashMap<UnitHash, Vec<usize>> {
    let mut index: HashMap<UnitHash, Vec<usize>> = HashMap::new();
    let mut by_unit_hash: HashMap<(&Path, UnitHash), usize> = HashMap::new();
    for (i, (profile, unit)) in units.iter().enumerate() {
        index.entry(unit.hash).or_default().push(i);
        let dir = unit_path::fingerprint_dir(profile, &unit.loc);
        match fingerprint::unit_hash(&unit.loc.name_meta) {
            None => reasons.push(Uncertain::NoUnitHash { dir }),
            Some(hash) => {
                if let Some(&first) = by_unit_hash.get(&(*profile, hash)) {
                    let (_, other) = units[first];
                    reasons.push(Uncertain::DuplicateUnitHash {
                        a: unit_path::fingerprint_dir(profile, &other.loc),
                        b: dir,
                    });
                } else {
                    by_unit_hash.insert((*profile, hash), i);
                }
            }
        }
    }
    index
}

/// Which units are live (by index), and which not-current nodes are.
struct Marked {
    units: BitSet,
    live_nodes: Vec<UnitRef>,
}

/// A unit or not-current node queued for traversal.
#[derive(Clone, Copy)]
enum Walk {
    Unit(usize),
    Node(usize),
}

/// The package name a unit dir stands for: the part before the last `-`, with `-` read as `_`.
fn package_name(dir_name: &str) -> Option<String> {
    dir_name
        .rsplit_once('-')
        .map(|(name, _)| name.replace('-', "_"))
}

/// Package id to the names of the current units its edges matched.
fn learn_packages(
    units: &[ProfileUnit],
    nodes: &[NotCurrentNode],
    index: &HashMap<UnitHash, Vec<usize>>,
) -> BTreeMap<u64, BTreeSet<String>> {
    let mut learned: BTreeMap<u64, BTreeSet<String>> = BTreeMap::new();
    let edges = units
        .iter()
        .flat_map(|(_, unit)| unit.deps.iter())
        .chain(nodes.iter().flat_map(|node| node.deps.iter()));
    for edge in edges {
        let key = UnitHash::from_fingerprint(edge.fingerprint);
        for &hit in index.get(&key).into_iter().flatten() {
            if let Some(name) = package_name(&units[hit].1.loc.name_meta) {
                learned.entry(edge.package).or_default().insert(name);
            }
        }
    }
    learned
}

/// Marks bit `i` and queues `walk`, unless `i` was already marked.
fn enqueue(flags: &mut BitSet, queue: &mut Vec<Walk>, i: usize, walk: Walk) {
    if flags.insert(i) {
        queue.push(walk);
    }
}

/// Marks units invoked inside the window and not-current units wanted inside it live, then
/// everything they depend on. An edge no current unit matches keeps the not-current units of its
/// package, when the package is known.
fn mark_live(
    fs: &dyn Fs,
    units: &[ProfileUnit],
    nodes: &[NotCurrentNode],
    index: &HashMap<UnitHash, Vec<usize>>,
    now: SystemTime,
    window: Duration,
    reasons: &mut Vec<Uncertain>,
) -> Marked {
    let mut live = BitSet::with_len(units.len());
    let mut node_live = BitSet::with_len(nodes.len());
    let mut queue: Vec<Walk> = Vec::new();
    for (i, (profile, unit)) in units.iter().enumerate() {
        let stamp = unit_timestamp(profile, &unit.loc, unit.kind);
        match invoked_in_window(fs, &stamp, now, window) {
            Ok(true) => enqueue(&mut live, &mut queue, i, Walk::Unit(i)),
            Ok(false) => {}
            Err(reason) => reasons.push(reason),
        }
    }
    for (i, node) in nodes.iter().enumerate() {
        if node
            .last_wanted(fs, reasons)
            .is_some_and(|time| in_window(time, now, window))
        {
            enqueue(&mut node_live, &mut queue, i, Walk::Node(i));
        }
    }
    // Only a not-current node can be kept by package, so without nodes neither map is read.
    let learned = if nodes.is_empty() {
        BTreeMap::new()
    } else {
        learn_packages(units, nodes, index)
    };
    let mut nodes_of: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, node) in nodes.iter().enumerate() {
        if let Some(name) = package_name(&node.loc.name_meta) {
            nodes_of.entry(name).or_default().push(i);
        }
    }
    let mut cursor = 0;
    while cursor < queue.len() {
        let walk = queue[cursor];
        cursor += 1;
        let (profile, loc, deps, stem) = match walk {
            Walk::Unit(i) => (
                units[i].0,
                &units[i].1.loc,
                units[i].1.deps.as_slice(),
                units[i].1.stem.as_str(),
            ),
            Walk::Node(i) => (nodes[i].profile, nodes[i].loc, nodes[i].deps, nodes[i].stem),
        };
        for &dep in deps {
            let key = UnitHash::from_fingerprint(dep.fingerprint);
            if let Some(hits) = index.get(&key) {
                for &hit in hits {
                    enqueue(&mut live, &mut queue, hit, Walk::Unit(hit));
                }
                continue;
            }
            let kept = learned.get(&dep.package).and_then(|names| {
                let mut names = names.iter();
                match (names.next(), names.next()) {
                    (Some(name), None) => nodes_of.get(name),
                    _ => None,
                }
            });
            match kept {
                Some(hits) => {
                    for &hit in hits {
                        enqueue(&mut node_live, &mut queue, hit, Walk::Node(hit));
                    }
                }
                None => {
                    let fp_dir = unit_path::fingerprint_dir(profile, loc);
                    reasons.push(Uncertain::Unresolved {
                        dependency: fingerprint::dep_name(&fp_dir, stem, dep.fingerprint)
                            .unwrap_or_else(|| le_hex(dep.fingerprint)),
                        unit: make_unit_ref(profile, loc),
                    });
                }
            }
        }
    }
    let live_nodes = nodes
        .iter()
        .enumerate()
        .filter(|(i, _)| node_live.contains(*i))
        .map(|(_, node)| node.unit_ref())
        .collect();
    Marked {
        units: live,
        live_nodes,
    }
}

/// Indices of units that are not live, are collectable and carry a unit hash.
fn dead_indices(units: &[ProfileUnit], live: &BitSet) -> Vec<usize> {
    (0..units.len())
        .filter(|&i| {
            !live.contains(i)
                && !matches!(
                    units[i].1.kind,
                    UnitKind::BuildScript
                        | UnitKind::RunBuildScript
                        | UnitKind::DocLib
                        | UnitKind::DocBin
                        | UnitKind::Other
                )
                && fingerprint::unit_hash(&units[i].1.loc.name_meta).is_some()
        })
        .collect()
}

/// Flat profile listings plus per-index unit-dir deletion lists.
struct DeadListings<'a> {
    flat: BTreeMap<&'a Path, Vec<(String, PathBuf)>>,
    unit_dir: HashMap<usize, Vec<PathBuf>>,
}

/// Lists flat `deps/`/`examples/` and each dead unit-dir's ordered entries.
fn list_dead_files<'a>(
    fs: &dyn Fs,
    units: &[ProfileUnit<'a>],
    dead_idx: &[usize],
    reasons: &mut Vec<Uncertain>,
) -> DeadListings<'a> {
    let mut flat: BTreeMap<&Path, Vec<(String, PathBuf)>> = BTreeMap::new();
    for &i in dead_idx {
        if units[i].1.loc.layout == UnitLayout::Flat {
            flat.entry(units[i].0).or_default();
        }
    }
    for (profile, entries) in &mut flat {
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

    let mut unit_dir = HashMap::new();
    for &i in dead_idx {
        let (profile, unit) = units[i];
        if unit.loc.layout != UnitLayout::UnitDir {
            continue;
        }
        unit_dir.insert(i, unit_dir_dead_files(fs, profile, &unit.loc, reasons));
    }
    DeadListings { flat, unit_dir }
}

/// `[out/*.d sorted…, out/?, run/?, fingerprint/, <unit dir>]` for one dead unit-dir unit.
fn unit_dir_dead_files(
    fs: &dyn Fs,
    profile: &Path,
    loc: &UnitLoc,
    reasons: &mut Vec<Uncertain>,
) -> Vec<PathBuf> {
    let out_dir = unit_path::outputs(profile, loc);
    let mut files = Vec::new();
    let mut out_present = false;
    match fs.read_dir_bounded(&out_dir, DEPS_CAP) {
        Ok(listed) => {
            out_present = true;
            let mut d_files: Vec<PathBuf> = listed
                .into_iter()
                .filter(|(name, _)| name.ends_with(".d"))
                .map(|(name, _)| out_dir.join(name))
                .collect();
            d_files.sort();
            files.extend(d_files);
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => reasons.push(Uncertain::ListingFailed {
            dir: out_dir.clone(),
            why: e.to_string(),
        }),
    }
    if out_present {
        files.push(out_dir);
    }

    let unit = unit_path::unit_dir(profile, loc).expect("unit-dir layout has a unit dir");
    let run_dir = unit_path::run_dir(profile, loc).expect("unit-dir layout has a run dir");
    match fs.entry_stat(&run_dir) {
        Ok(stat) if stat.kind == EntryKind::Dir => files.push(run_dir),
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => reasons.push(Uncertain::ListingFailed {
            dir: run_dir,
            why: e.to_string(),
        }),
    }

    files.push(unit_path::fingerprint_dir(profile, loc));
    files.push(unit);
    files
}

/// Builds the decided result: the live units, and per dead unit the files only it owns.
fn assemble(
    units: &[ProfileUnit],
    not_current_hashes: &[(&Path, UnitHash)],
    live_units: BTreeSet<UnitRef>,
    dead_idx: &[usize],
    listings: DeadListings<'_>,
) -> Liveness {
    let dead_set: HashSet<usize> = dead_idx.iter().copied().collect();
    let mut kept_hashes: HashMap<&Path, HashSet<UnitHash>> = HashMap::new();
    for (i, (profile, unit)) in units.iter().enumerate() {
        if !dead_set.contains(&i)
            && let Some(hash) = fingerprint::unit_hash(&unit.loc.name_meta)
        {
            kept_hashes.entry(profile).or_default().insert(hash);
        }
    }
    for (profile, hash) in not_current_hashes {
        kept_hashes.entry(profile).or_default().insert(*hash);
    }

    let mut dead_by_hash: HashMap<&Path, HashMap<UnitHash, Vec<usize>>> = HashMap::new();
    for &i in dead_idx {
        let (profile, unit) = units[i];
        if unit.loc.layout != UnitLayout::Flat {
            continue;
        }
        if let Some(hash) = fingerprint::unit_hash(&unit.loc.name_meta) {
            dead_by_hash
                .entry(profile)
                .or_default()
                .entry(hash)
                .or_default()
                .push(i);
        }
    }
    let no_hashes = HashSet::new();
    let mut claimed: HashMap<usize, Vec<&PathBuf>> = HashMap::new();
    for (profile, entries) in &listings.flat {
        let Some(dead) = dead_by_hash.get(profile) else {
            continue;
        };
        let kept = kept_hashes.get(profile).unwrap_or(&no_hashes);
        for (name, path) in entries {
            for i in claimants(name, kept, dead) {
                claimed.entry(i).or_default().push(path);
            }
        }
    }

    let mut dead_units: BTreeMap<UnitRef, Vec<PathBuf>> = BTreeMap::new();
    for &i in dead_idx {
        let (profile, unit) = units[i];
        let files = match unit.loc.layout {
            UnitLayout::UnitDir => listings.unit_dir.get(&i).cloned().unwrap_or_default(),
            UnitLayout::Flat => {
                let mut files = Vec::new();
                if let Some(matching) = claimed.remove(&i) {
                    files.push(unit_path::fingerprint_dir(profile, &unit.loc));
                    files.extend(matching.into_iter().cloned());
                }
                files
            }
        };
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

/// The dead units a file name belongs to: none when the name carries a kept unit's hash, else
/// each dead unit whose hash it carries, once and in ascending index order.
fn claimants(
    name: &str,
    kept: &HashSet<UnitHash>,
    dead: &HashMap<UnitHash, Vec<usize>>,
) -> Vec<usize> {
    let mut owners: Vec<usize> = Vec::new();
    for hash in embedded_hashes(name) {
        let Some(parsed) = UnitHash::parse(hash) else {
            continue;
        };
        if kept.contains(&parsed) {
            return Vec::new();
        }
        if let Some(units) = dead.get(&parsed) {
            owners.extend(units);
        }
    }
    owners.sort_unstable();
    owners.dedup();
    owners
}

fn unit_ref((profile, unit): &ProfileUnit) -> UnitRef {
    make_unit_ref(profile, &unit.loc)
}

fn make_unit_ref(profile: &Path, loc: &UnitLoc) -> UnitRef {
    UnitRef {
        profile: profile.to_path_buf(),
        loc: loc.clone(),
    }
}

#[cfg(test)]
#[cfg(target_os = "macos")]
mod tests {
    use super::*;
    use cargo_hive_core::fingerprint::{UnitId, le_hex, read_profile};
    use cargo_hive_core::liveness::DEFAULT_WINDOW;
    use cargo_hive_core::target::Owner;
    use cargo_hive_core::unit_path::UnitLayout;
    use std::fs;
    use std::io;
    use std::time::UNIX_EPOCH;

    fn flat_loc(name_meta: &str) -> UnitLoc {
        UnitLoc {
            layout: UnitLayout::Flat,
            name_meta: name_meta.to_string(),
        }
    }

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
        let edges: Vec<(&str, u64, u64)> = deps.iter().map(|&(n, h)| (n, 1, h)).collect();
        write_unit_edges(profile, name, stem, fp, &edges, age);
    }

    /// Like `write_unit`, with each dependency as (name, package id, fingerprint).
    fn write_unit_edges(
        profile: &Path,
        name: &str,
        stem: &str,
        fp: u64,
        deps: &[(&str, u64, u64)],
        age: Option<Duration>,
    ) {
        let dir = profile.join(".fingerprint").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(stem), le_hex(fp)).unwrap();
        let deps: Vec<String> = deps
            .iter()
            .map(|(n, p, h)| format!("[{p},\"{n}\",false,{h}]"))
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
            let stamp = unit_timestamp(profile, &flat_loc(name), UnitKind::of(stem));
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

    fn expected_successors(units: &[Unit]) -> Vec<Vec<UnitId>> {
        let mut index: HashMap<UnitHash, Vec<UnitId>> = HashMap::new();
        for (i, u) in units.iter().enumerate() {
            let id =
                UnitId(u32::try_from(i).expect("fixture profiles stay within u32 unit counts"));
            index.entry(u.hash).or_default().push(id);
        }
        let mut adj = vec![Vec::new(); units.len()];
        for (i, u) in units.iter().enumerate() {
            for dep in &u.deps {
                let key =
                    UnitHash::parse(&le_hex(dep.fingerprint)).expect("le_hex is 16 lowercase hex");
                if let Some([j]) = index.get(&key).map(Vec::as_slice) {
                    adj[i].push(*j);
                }
            }
        }
        adj
    }

    #[test]
    fn each_profiles_successors_match_the_resolved_pair_list() {
        let s = Scratch::new("csr");
        let host = s.0.join("debug");
        let cross = s.0.join("aarch64-apple-darwin").join("debug");
        write_unit(
            &host,
            A,
            "bin-a",
            101,
            &[("b", 102), ("missing", 999), ("dup", 200)],
            Some(HOUR),
        );
        write_unit(&host, B, "lib-b", 102, &[], Some(30 * DAY));
        write_unit(&host, C, "lib-c", 103, &[], Some(30 * DAY));
        write_unit(
            &host,
            "dup-a-2000000000000200",
            "lib-dup-a",
            200,
            &[],
            Some(30 * DAY),
        );
        write_unit(
            &host,
            "dup-b-2000000000000200",
            "lib-dup-b",
            200,
            &[],
            Some(30 * DAY),
        );
        write_unit(&cross, D, "lib-d", 104, &[("a", 101)], Some(HOUR));
        write_unit(&cross, A, "lib-a", 101, &[], Some(30 * DAY));
        let target = target_of(&s.0, &[&host, &cross]);
        for profile in &target.profiles {
            let graph = profile
                .graph
                .as_ref()
                .expect("fixture profiles are supported");
            let want = expected_successors(&graph.units);
            assert_eq!(graph.edge_count(), want.iter().map(Vec::len).sum::<usize>());
            for (i, expected) in want.iter().enumerate() {
                let id =
                    UnitId(u32::try_from(i).expect("fixture profiles stay within u32 unit counts"));
                assert_eq!(graph.successors(id), expected.as_slice(), "unit {i}");
            }
        }
    }

    fn decide(target: &TargetDir, window: Duration) -> Liveness {
        let platform = cargo_hive_platform::current().unwrap();
        liveness(&*platform.fs, target, now(), window)
    }

    fn names(units: &[UnitRef]) -> Vec<&str> {
        units.iter().map(|u| u.loc.name_meta.as_str()).collect()
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
        assert_eq!(dead[0].unit.loc.name_meta, D);
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
        assert_eq!(dead[0].unit.loc.name_meta, D);
    }

    #[test]
    fn a_unit_without_a_readable_timestamp_is_not_a_root() {
        let s = Scratch::new("nostamp");
        let p = s.0.join("debug");
        write_unit(&p, A, "lib-a", 101, &[], None);
        let (live, dead) = decided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
        assert!(live.is_empty());
        assert_eq!(dead[0].unit.loc.name_meta, A);
    }

    #[test]
    fn a_timestamp_later_than_now_makes_a_root() {
        let s = Scratch::new("future");
        let p = s.0.join("debug");
        write_unit(&p, A, "lib-a", 101, &[], None);
        let stamp = unit_timestamp(&p, &flat_loc(A), UnitKind::Lib);
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
            ("build-script-build", UnitKind::BuildScript),
            (
                "run-build-script-build-script-build",
                UnitKind::RunBuildScript,
            ),
            ("doc-lib-a", UnitKind::DocLib),
            ("doc-bin-a", UnitKind::DocBin),
            ("weird", UnitKind::Other),
        ] {
            let s = Scratch::new("never");
            let p = s.0.join("debug");
            write_unit(&p, A, stem, 101, &[("b", 102)], Some(HOUR));
            write_unit(&p, B, "lib-b", 102, &[], Some(30 * DAY));
            let stamp = unit_timestamp(&p, &flat_loc(A), kind);
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
    fn the_default_window_keeps_a_unit_invoked_just_under_seven_days_ago() {
        let s = Scratch::new("default-window");
        let p = s.0.join("debug");
        write_unit(&p, A, "lib-a", 101, &[], Some(6 * DAY + 23 * HOUR));
        write_unit(&p, B, "lib-b", 102, &[], Some(7 * DAY + HOUR));
        let (live, dead) = decided(decide(&target_of(&s.0, &[&p]), DEFAULT_WINDOW));
        assert_eq!(names(&live), [A]);
        assert_eq!(dead.len(), 1);
        assert_eq!(dead[0].unit.loc.name_meta, B);
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

    /// A unit Cargo is rebuilding: its hash file is emptied and last written `hash_age` ago.
    fn emptied_hash(
        profile: &Path,
        name: &str,
        stem: &str,
        age: Option<Duration>,
        hash_age: Duration,
    ) {
        emptied_hash_wanting(profile, name, stem, 1, &[], age, hash_age);
    }

    fn emptied_hash_wanting(
        profile: &Path,
        name: &str,
        stem: &str,
        fp: u64,
        deps: &[(&str, u64)],
        age: Option<Duration>,
        hash_age: Duration,
    ) {
        write_unit(profile, name, stem, fp, deps, age);
        empty_hash(profile, name, stem, hash_age);
    }

    /// A not-current unit that depends on `deps` as (name, package id, fingerprint).
    fn emptied_hash_edges(
        profile: &Path,
        name: &str,
        stem: &str,
        deps: &[(&str, u64, u64)],
        age: Option<Duration>,
        hash_age: Duration,
    ) {
        write_unit_edges(profile, name, stem, 1, deps, age);
        empty_hash(profile, name, stem, hash_age);
    }

    fn empty_hash(profile: &Path, name: &str, stem: &str, hash_age: Duration) {
        let hash = profile.join(".fingerprint").join(name).join(stem);
        fs::write(&hash, "").unwrap();
        fs::File::options()
            .write(true)
            .open(&hash)
            .unwrap()
            .set_modified(now() - hash_age)
            .unwrap();
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
    fn a_never_compiled_unit_invoked_inside_the_window_makes_the_target_undecided() {
        let s = Scratch::new("nc-in");
        let p = s.0.join("debug");
        write_unit(&p, B, "lib-b", 102, &[], Some(30 * DAY));
        never_compiled(&p, A, HOUR);
        let reasons = undecided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
        let dir = p.join(".fingerprint").join(A);
        assert_eq!(reasons, [Uncertain::NotCurrentInWindow { dir }]);
        assert!(reasons[0].to_string().contains("never compiled"));
        assert!(reasons[0].clears_when().contains("never compiled"));
    }

    #[test]
    fn a_never_compiled_unit_invoked_outside_the_window_is_ignored() {
        let s = Scratch::new("nc-out-never");
        let p = s.0.join("debug");
        write_unit(&p, D, "lib-d", 104, &[], Some(30 * DAY));
        never_compiled(&p, A, 30 * DAY);
        let (live, dead) = decided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
        assert!(live.is_empty());
        assert_eq!(dead.len(), 1);
    }

    #[test]
    fn a_not_current_unit_is_ignored_when_both_of_its_files_are_older_than_the_window() {
        let s = Scratch::new("nc-out");
        let p = s.0.join("debug");
        write_unit(&p, D, "lib-d", 104, &[], Some(30 * DAY));
        emptied_hash(&p, B, "lib-b", Some(2 * DAY), 2 * DAY);
        emptied_hash(
            &p,
            C,
            "run-build-script-build-script-build",
            Some(30 * DAY),
            30 * DAY,
        );
        let (live, dead) = decided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
        assert!(live.is_empty());
        let dead: Vec<&str> = dead.iter().map(|d| d.unit.loc.name_meta.as_str()).collect();
        assert_eq!(dead, [D]);
    }

    #[test]
    fn a_not_current_unit_with_an_old_stamp_and_a_fresh_hash_file_is_a_root_that_keeps_its_dependency()
     {
        let s = Scratch::new("nc-root-hash");
        let p = s.0.join("debug");
        write_unit(&p, C, "lib-c", 103, &[], Some(30 * DAY));
        write_unit(&p, D, "lib-d", 104, &[], Some(30 * DAY));
        emptied_hash_wanting(&p, A, "lib-a", 101, &[("c", 103)], Some(2 * DAY), HOUR);
        let (live, dead) = decided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
        assert_eq!(names(&live), [A, C]);
        let dead: Vec<&str> = dead.iter().map(|d| d.unit.loc.name_meta.as_str()).collect();
        assert_eq!(dead, [D]);
    }

    #[test]
    fn a_not_current_unit_with_a_fresh_stamp_and_an_old_hash_file_is_a_root() {
        let s = Scratch::new("nc-root-stamp");
        let p = s.0.join("debug");
        write_unit(&p, C, "lib-c", 103, &[], Some(30 * DAY));
        emptied_hash_wanting(&p, A, "lib-a", 101, &[("c", 103)], Some(HOUR), 2 * DAY);
        let (live, dead) = decided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
        assert_eq!(names(&live), [A, C]);
        assert!(dead.is_empty());
    }

    #[test]
    fn a_run_build_script_not_current_unit_is_timed_by_its_build_dir_and_its_hash_file() {
        let s = Scratch::new("nc-root-rbs");
        let p = s.0.join("debug");
        let stem = "run-build-script-build-script-build";
        emptied_hash(&p, A, stem, Some(2 * DAY), HOUR);
        let (live, dead) = decided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
        assert_eq!(names(&live), [A]);
        assert!(dead.is_empty());
    }

    #[test]
    fn a_not_current_unit_outside_the_window_still_keeps_entries_out_of_dead_units_files() {
        let s = Scratch::new("nc-keeps");
        let p = s.0.join("debug");
        write_unit(&p, D, "lib-d", 104, &[], Some(30 * DAY));
        emptied_hash(&p, B, "lib-b", Some(2 * DAY), 2 * DAY);
        plant(&p, "deps", &format!("dep-{HASH_D}.o"));
        plant(&p, "deps", &format!("dep-2222222222222222.cgu-{HASH_D}.o"));
        let dead = dead_of(decide(&target_of(&s.0, &[&p]), 6 * HOUR), D);
        assert_eq!(
            dead.files,
            [
                p.join(".fingerprint").join(D),
                p.join("deps").join(format!("dep-{HASH_D}.o"))
            ]
        );
    }

    #[test]
    fn a_not_current_unit_with_no_readable_file_time_is_not_a_root_and_gives_no_reason() {
        let s = Scratch::new("nc-both-missing");
        let p = s.0.join("debug");
        emptied_hash(&p, A, "lib-a", Some(HOUR), HOUR);
        let target = target_of(&s.0, &[&p]);
        fs::remove_file(p.join(".fingerprint").join(A).join("lib-a")).unwrap();
        let fake = mtime_fails_with(io::ErrorKind::NotFound);
        let (live, dead) = decided(liveness(&fake, &target, now(), 6 * HOUR));
        assert!(live.is_empty());
        assert!(dead.is_empty());
    }

    #[test]
    fn an_unreadable_hash_file_time_of_a_not_current_unit_makes_the_target_undecided() {
        let s = Scratch::new("nc-hash-denied");
        let p = s.0.join("debug");
        emptied_hash(&p, A, "lib-a", Some(30 * DAY), 30 * DAY);
        let hash = p.join(".fingerprint").join(A).join("lib-a");
        let target = target_of(&s.0, &[&p]);
        let fake = FakeFs::new(Listing::MtimeFailsAt(
            io::ErrorKind::PermissionDenied,
            hash.clone(),
        ));
        let reasons = undecided(liveness(&fake, &target, now(), 6 * HOUR));
        assert!(
            matches!(&reasons[..], [Uncertain::TimestampUnreadable { path, .. }] if *path == hash),
            "{reasons:?}"
        );
        assert!(reasons[0].to_string().contains("file"));
        assert!(reasons[0].clears_when().contains("file"));
    }

    #[test]
    fn a_not_current_unit_is_never_dead() {
        let s = Scratch::new("nc-never-dead");
        let p = s.0.join("debug");
        emptied_hash(&p, A, "lib-a", Some(30 * DAY), 30 * DAY);
        emptied_hash(&p, B, "lib-b", Some(HOUR), HOUR);
        never_compiled(&p, C, 30 * DAY);
        let (live, dead) = decided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
        assert_eq!(names(&live), [B]);
        assert!(dead.is_empty());
    }

    #[test]
    fn an_edge_of_a_not_current_root_that_no_unit_provides_is_unresolved() {
        let s = Scratch::new("nc-unres");
        let p = s.0.join("debug");
        emptied_hash_wanting(&p, A, "lib-a", 101, &[("ghost", 999)], Some(HOUR), HOUR);
        let reasons = undecided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
        assert!(matches!(
            &reasons[..],
            [Uncertain::Unresolved { dependency, .. }] if dependency == "ghost"
        ));
    }

    const LIBA_1: &str = "liba-aaaaaaaaaaaaaaa1";

    const LIBA_2: &str = "liba-aaaaaaaaaaaaaaa2";

    const LIBA_3: &str = "liba-aaaaaaaaaaaaaaa3";

    const LIBB_1: &str = "libb-bbbbbbbbbbbbbbb1";

    const PACKAGE: u64 = 7;

    /// A live unit `A` whose edge (package 7, fingerprint 999) matches nothing, a live unit `D`
    /// whose edge (package 7) matches the current `liba`, and that current `liba`.
    fn unmatched_edge_of_a_learned_package(p: &Path) {
        write_unit_edges(p, A, "bin-a", 101, &[("liba", PACKAGE, 999)], Some(HOUR));
        write_unit_edges(p, D, "bin-d", 104, &[("liba", PACKAGE, 201)], Some(HOUR));
        write_unit(p, LIBA_1, "lib-liba", 201, &[], Some(30 * DAY));
    }

    #[test]
    fn an_unmatched_edge_keeps_the_not_current_unit_of_its_package_and_what_it_wanted() {
        let s = Scratch::new("pkg-keep");
        let p = s.0.join("debug");
        unmatched_edge_of_a_learned_package(&p);
        write_unit(&p, C, "lib-c", 103, &[], Some(30 * DAY));
        write_unit(&p, B, "lib-b", 102, &[], Some(30 * DAY));
        emptied_hash_edges(
            &p,
            LIBA_2,
            "lib-liba",
            &[("c", 1, 103)],
            Some(30 * DAY),
            30 * DAY,
        );
        let (live, dead) = decided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
        let mut got = names(&live);
        got.sort();
        assert_eq!(got, [A, C, D, LIBA_1, LIBA_2]);
        let dead: Vec<&str> = dead.iter().map(|d| d.unit.loc.name_meta.as_str()).collect();
        assert_eq!(dead, [B]);
    }

    #[test]
    fn the_not_current_unit_of_the_package_may_sit_in_another_profile_dir() {
        let s = Scratch::new("pkg-cross");
        let host = s.0.join("debug");
        let cross = s.0.join("aarch64-apple-darwin").join("debug");
        unmatched_edge_of_a_learned_package(&cross);
        emptied_hash(&host, LIBA_2, "lib-liba", Some(30 * DAY), 30 * DAY);
        let (live, _) = decided(decide(&target_of(&s.0, &[&host, &cross]), 6 * HOUR));
        assert!(
            live.iter()
                .any(|u| u.loc.name_meta == LIBA_2 && u.profile == host)
        );
    }

    #[test]
    fn package_names_compare_with_dashes_and_underscores_equal() {
        let s = Scratch::new("pkg-dash");
        let p = s.0.join("debug");
        write_unit_edges(&p, A, "bin-a", 101, &[("m", PACKAGE, 999)], Some(HOUR));
        write_unit_edges(&p, D, "bin-d", 104, &[("m", PACKAGE, 201)], Some(HOUR));
        write_unit(
            &p,
            "my-lib-aaaaaaaaaaaaaaa1",
            "lib-my-lib",
            201,
            &[],
            Some(30 * DAY),
        );
        emptied_hash(
            &p,
            "my_lib-aaaaaaaaaaaaaaa2",
            "lib-my_lib",
            Some(30 * DAY),
            30 * DAY,
        );
        let (live, _) = decided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
        assert!(names(&live).contains(&"my_lib-aaaaaaaaaaaaaaa2"));
    }

    #[test]
    fn a_package_no_edge_ever_matched_is_unresolved_whatever_the_crate_name() {
        let s = Scratch::new("pkg-unlearned");
        let p = s.0.join("debug");
        write_unit_edges(&p, A, "bin-a", 101, &[("liba", PACKAGE, 999)], Some(HOUR));
        emptied_hash(&p, LIBA_2, "lib-liba", Some(30 * DAY), 30 * DAY);
        let reasons = undecided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
        assert!(matches!(
            &reasons[..],
            [Uncertain::Unresolved { dependency, .. }] if dependency == "liba"
        ));
    }

    #[test]
    fn a_package_learned_under_two_names_is_unresolved() {
        let s = Scratch::new("pkg-two-names");
        let p = s.0.join("debug");
        unmatched_edge_of_a_learned_package(&p);
        write_unit_edges(&p, C, "bin-c", 103, &[("libb", PACKAGE, 202)], Some(HOUR));
        write_unit(&p, LIBB_1, "lib-libb", 202, &[], Some(30 * DAY));
        emptied_hash(&p, LIBA_2, "lib-liba", Some(30 * DAY), 30 * DAY);
        let reasons = undecided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
        assert!(matches!(&reasons[..], [Uncertain::Unresolved { .. }]));
    }

    #[test]
    fn a_learned_package_with_no_not_current_unit_is_unresolved_as_before() {
        let s = Scratch::new("pkg-none");
        let p = s.0.join("debug");
        unmatched_edge_of_a_learned_package(&p);
        let reasons = undecided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
        let fp_dir = p.join(".fingerprint").join(A);
        assert_eq!(
            reasons[0].to_string(),
            format!(
                "{}: dependency `liba` matches no unit of the target",
                fp_dir.display()
            )
        );
        assert_eq!(reasons.len(), 1);
    }

    #[test]
    fn a_not_current_dir_without_a_dash_is_neither_a_candidate_nor_a_reason() {
        let s = Scratch::new("pkg-nodash");
        let p = s.0.join("debug");
        unmatched_edge_of_a_learned_package(&p);
        emptied_hash(&p, "liba", "lib-liba", Some(30 * DAY), 30 * DAY);
        let reasons = undecided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
        assert!(matches!(&reasons[..], [Uncertain::Unresolved { .. }]));
    }

    #[test]
    fn marking_a_package_s_not_current_units_does_not_depend_on_directory_order() {
        let s = Scratch::new("pkg-order");
        let p = s.0.join("debug");
        unmatched_edge_of_a_learned_package(&p);
        emptied_hash(&p, LIBA_2, "lib-liba", Some(30 * DAY), 30 * DAY);
        emptied_hash(&p, LIBA_3, "lib-liba", Some(30 * DAY), 30 * DAY);
        let target = target_of(&s.0, &[&p]);
        let plain = decide(&target, 6 * HOUR);
        // `units` come from the sorted `.fingerprint` listing, so only this order can vary.
        let mut reversed = target.clone();
        for profile in &mut reversed.profiles {
            assert_eq!(profile.not_current_units.len(), 2);
            profile.not_current_units.reverse();
        }
        assert_ne!(
            target.profiles[0].not_current_units, reversed.profiles[0].not_current_units,
            "the two orders must differ for the comparison to mean anything"
        );
        let shuffled = decide(&reversed, 6 * HOUR);
        assert_eq!(plain, shuffled);
        let (live, _) = decided(plain);
        assert_eq!(names(&live), [A, D, LIBA_1, LIBA_2, LIBA_3]);
    }

    #[test]
    fn an_unresolved_edge_from_a_live_unit_names_the_unit_and_the_dependency() {
        let s = Scratch::new("unres");
        let p = s.0.join("debug");
        write_unit(&p, A, "lib-a", 101, &[("ghost", 999)], Some(HOUR));
        let unit = UnitRef {
            profile: p.clone(),
            loc: flat_loc(A),
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
        assert_eq!(dead[0].unit.loc.name_meta, A);
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
            .find(|d| d.unit.loc.name_meta == dir_name)
            .unwrap_or_else(|| panic!("{dir_name} is not dead"))
    }

    enum Listing {
        Reversed,
        FailOn(&'static str),
        MtimeFails(io::ErrorKind),
        MtimeFailsAt(io::ErrorKind, PathBuf),
    }

    struct FakeFs {
        real: Box<dyn Fs>,
        listing: Listing,
        listed: std::cell::RefCell<Vec<PathBuf>>,
    }

    impl FakeFs {
        fn new(listing: Listing) -> FakeFs {
            FakeFs {
                real: cargo_hive_platform::current().unwrap().fs,
                listing,
                listed: Default::default(),
            }
        }
    }

    impl Fs for FakeFs {
        fn temp_dir(
            &self,
            parent: &Path,
            prefix: &str,
        ) -> io::Result<cargo_hive_platform::TempDir> {
            self.real.temp_dir(parent, prefix)
        }
        fn write_file(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
            self.real.write_file(path, bytes)
        }
        fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
            self.real.read(path)
        }
        fn read_at(&self, path: &Path, offset: u64, len: usize) -> io::Result<Vec<u8>> {
            self.real.read_at(path, offset, len)
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
                Listing::MtimeFailsAt(kind, ref only) if path == only => {
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
        fn create_dir(&self, path: &Path) -> io::Result<()> {
            self.real.create_dir(path)
        }
        fn sharing(&self, path: &Path) -> io::Result<cargo_hive_platform::Sharing> {
            self.real.sharing(path)
        }
        fn compress(&self, path: &Path) -> cargo_hive_platform::CompressOutcome {
            self.real.compress(path)
        }
        fn hold_lock(
            &self,
            kind: cargo_hive_platform::LockKind,
            path: &Path,
        ) -> io::Result<cargo_hive_platform::LockHolder> {
            self.real.hold_lock(kind, path)
        }
        fn try_lock_exclusive(&self, path: &Path) -> cargo_hive_platform::TryLock {
            self.real.try_lock_exclusive(path)
        }
        fn try_lock_elsewhere(
            &self,
            kind: cargo_hive_platform::LockKind,
            path: &Path,
        ) -> io::Result<cargo_hive_platform::LockState> {
            self.real.try_lock_elsewhere(kind, path)
        }
        fn entry_stat(&self, path: &Path) -> io::Result<cargo_hive_platform::EntryStat> {
            self.real.entry_stat(path)
        }
        fn read_dir_bounded(
            &self,
            dir: &Path,
            cap: usize,
        ) -> io::Result<Vec<(String, cargo_hive_platform::EntryKind)>> {
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
        fn volume(&self, path: &Path) -> io::Result<cargo_hive_platform::VolumeInfo> {
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
            (&host, "build/other-0000000000000001"),
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
        assert_eq!(dead[0].unit.loc.name_meta, A);
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
                    if *path == unit_timestamp(&p, &flat_loc(A), UnitKind::Lib)),
                "{kind:?}: {reasons:?}"
            );
        }
        let reasons = undecided(liveness(
            &mtime_fails_with(io::ErrorKind::PermissionDenied),
            &target,
            now(),
            6 * HOUR,
        ));
        let stamp = unit_timestamp(&p, &flat_loc(A), UnitKind::Lib);
        assert!(
            reasons[0].to_string().starts_with(&format!(
                "{}: invocation timestamp unreadable: ",
                stamp.display()
            )),
            "{}",
            reasons[0]
        );
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
            .push(cargo_hive_core::fingerprint::NotCurrentUnit {
                dir: odd.clone(),
                loc: flat_loc(""),
                stem: None,
                features: None,
                outputs: Vec::new(),
                deps: Vec::new(),
            });
        let reasons = undecided(decide(&target, 6 * HOUR));
        assert_eq!(reasons, [Uncertain::NoUnitHash { dir: odd }]);
    }

    fn unit_dir_loc(package: &str, hex: &str) -> UnitLoc {
        UnitLoc {
            layout: UnitLayout::UnitDir,
            name_meta: format!("{package}-{hex}"),
        }
    }

    /// A unit-dir unit under `build/<package>/<hex>/` with optional `out/` names and age.
    fn write_unit_dir(
        profile: &Path,
        package: &str,
        hex: &str,
        stem: &str,
        fp: u64,
        outs: &[&str],
        age: Option<Duration>,
    ) -> UnitLoc {
        let loc = unit_dir_loc(package, hex);
        let fp_dir = unit_path::fingerprint_dir(profile, &loc);
        fs::create_dir_all(&fp_dir).unwrap();
        fs::write(unit_path::hash_file(profile, &loc, stem), le_hex(fp)).unwrap();
        fs::write(
            fp_dir.join(format!("{stem}.json")),
            "{\"rustc\":1,\"features\":\"[]\",\"declared_features\":\"[]\",\"target\":2,\"profile\":3,\"path\":4,\"deps\":[],\"local\":[],\"rustflags\":[],\"config\":5,\"compile_kind\":0}",
        )
        .unwrap();
        if !outs.is_empty() {
            let out = unit_path::outputs(profile, &loc);
            fs::create_dir_all(&out).unwrap();
            for name in outs {
                fs::write(out.join(name), "").unwrap();
            }
        }
        if let Some(age) = age {
            touch(&unit_timestamp(profile, &loc, UnitKind::of(stem)), age);
        }
        loc
    }

    #[test]
    fn a_dead_unit_dir_lib_lists_out_d_files_then_out_run_fingerprint_and_the_unit_dir() {
        let s = Scratch::new("udir-order");
        let p = s.0.join("debug");
        let hex = "4444444444444444";
        let loc = write_unit_dir(
            &p,
            "d",
            hex,
            "lib-d",
            104,
            &["libd.rmeta", "z.d", "a.d", "m.d", "libd.rlib"],
            Some(30 * DAY),
        );
        let unit = unit_path::unit_dir(&p, &loc).unwrap();
        fs::create_dir_all(unit.join("run")).unwrap();
        let dead = dead_of(decide(&target_of(&s.0, &[&p]), 6 * HOUR), &loc.name_meta);
        assert_eq!(
            dead.files,
            [
                unit.join("out").join("a.d"),
                unit.join("out").join("m.d"),
                unit.join("out").join("z.d"),
                unit.join("out"),
                unit.join("run"),
                unit.join("fingerprint"),
                unit.clone(),
            ]
        );
    }

    #[test]
    fn a_dead_unit_dir_with_no_out_lists_fingerprint_and_the_unit_dir() {
        let s = Scratch::new("udir-no-out");
        let p = s.0.join("debug");
        let loc = write_unit_dir(
            &p,
            "d",
            "4444444444444444",
            "lib-d",
            104,
            &[],
            Some(30 * DAY),
        );
        let unit = unit_path::unit_dir(&p, &loc).unwrap();
        let dead = dead_of(decide(&target_of(&s.0, &[&p]), 6 * HOUR), &loc.name_meta);
        assert_eq!(dead.files, [unit.join("fingerprint"), unit.clone()]);
    }

    #[test]
    fn an_out_listing_error_makes_the_target_undecided_with_listing_failed() {
        let s = Scratch::new("udir-out-fail");
        let p = s.0.join("debug");
        let loc = write_unit_dir(
            &p,
            "d",
            "4444444444444444",
            "lib-d",
            104,
            &[],
            Some(30 * DAY),
        );
        let out = unit_path::outputs(&p, &loc);
        fs::write(&out, "").unwrap();
        let reasons = undecided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
        assert!(
            matches!(&reasons[..], [Uncertain::ListingFailed { dir, .. }] if *dir == out),
            "{reasons:?}"
        );
    }

    #[test]
    fn never_collected_unit_dir_kinds_not_current_and_partials_are_never_dead() {
        for (stem, pkg, hex) in [
            ("build-script-build", "bs", "aaaaaaaaaaaaaaaa"),
            (
                "run-build-script-build-script-build",
                "rbs",
                "bbbbbbbbbbbbbbbb",
            ),
            ("doc-lib-a", "doc", "cccccccccccccccc"),
            ("doc-bin-a", "docb", "dddddddddddddddd"),
            ("weird", "oth", "eeeeeeeeeeeeeeee"),
        ] {
            let s = Scratch::new("udir-never");
            let p = s.0.join("debug");
            let loc = write_unit_dir(&p, pkg, hex, stem, 1, &[], Some(30 * DAY));
            let (live, dead) = decided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
            assert!(live.is_empty(), "{stem}");
            assert!(
                dead.iter().all(|d| d.unit.loc.name_meta != loc.name_meta),
                "{stem}: {dead:?}"
            );
        }

        let s = Scratch::new("udir-nc-partial");
        let p = s.0.join("debug");
        let dead_loc = write_unit_dir(
            &p,
            "lib",
            "4444444444444444",
            "lib-d",
            104,
            &["a.d"],
            Some(30 * DAY),
        );
        let nc_hex = "ffffffffffffffff";
        let nc_fp = p.join("build").join("nc").join(nc_hex).join("fingerprint");
        fs::create_dir_all(&nc_fp).unwrap();
        fs::write(nc_fp.join("invoked.timestamp"), "").unwrap();
        fs::write(nc_fp.join("output-lib-nc"), "").unwrap();
        touch(&nc_fp.join("invoked.timestamp"), 30 * DAY);
        let partial = p.join("build").join("part").join("0123456789abcdef");
        fs::create_dir_all(&partial).unwrap();
        let (_, dead) = decided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
        assert_eq!(dead.len(), 1);
        assert_eq!(dead[0].unit.loc.name_meta, dead_loc.name_meta);
        let listed = &dead[0].files;
        assert!(
            !listed.iter().any(|f| f.starts_with(&partial)),
            "{listed:?}"
        );
        assert!(
            !listed
                .iter()
                .any(|f| f.components().any(|c| c.as_os_str() == "nc")),
            "{listed:?}"
        );
    }

    #[test]
    fn nothing_under_build_outside_the_dead_unit_dir_is_listed() {
        let s = Scratch::new("udir-scope");
        let p = s.0.join("debug");
        let loc = write_unit_dir(
            &p,
            "d",
            "4444444444444444",
            "lib-d",
            104,
            &["a.d"],
            Some(30 * DAY),
        );
        let unit = unit_path::unit_dir(&p, &loc).unwrap();
        let flat_bs = p.join("build").join("other-0000000000000001");
        fs::create_dir_all(&flat_bs).unwrap();
        fs::write(flat_bs.join("invoked.timestamp"), "").unwrap();
        let other_pkg = p.join("build").join("otherpkg");
        fs::create_dir_all(other_pkg.join("0123456789abcdef")).unwrap();
        let dead = dead_of(decide(&target_of(&s.0, &[&p]), 6 * HOUR), &loc.name_meta);
        assert_eq!(
            dead.files,
            [
                unit.join("out").join("a.d"),
                unit.join("out"),
                unit.join("fingerprint"),
                unit.clone(),
            ]
        );
        assert!(flat_bs.exists());
        assert!(other_pkg.exists());
    }

    #[test]
    fn a_mixed_target_lists_flat_dead_units_with_their_hash_keyed_files() {
        let s = Scratch::new("udir-mixed");
        let p = s.0.join("debug");
        write_unit(&p, D, "lib-d", 104, &[], Some(30 * DAY));
        let ud = write_unit_dir(
            &p,
            "u",
            "5555555555555555",
            "lib-u",
            105,
            &["u.d"],
            Some(30 * DAY),
        );
        plant(&p, "deps", &format!("dep-{HASH_D}.o"));
        plant(&p, "examples", &format!("ex-{HASH_D}.d"));
        plant(&p, "deps", "dep-9999999999999999.o");
        let (_, dead) = decided(decide(&target_of(&s.0, &[&p]), 6 * HOUR));
        assert_eq!(dead.len(), 2);
        let flat = dead.iter().find(|d| d.unit.loc.name_meta == D).unwrap();
        assert_eq!(
            flat.files,
            [
                p.join(".fingerprint").join(D),
                p.join("deps").join(format!("dep-{HASH_D}.o")),
                p.join("examples").join(format!("ex-{HASH_D}.d")),
            ]
        );
        let unit = unit_path::unit_dir(&p, &ud).unwrap();
        let ud_dead = dead
            .iter()
            .find(|d| d.unit.loc.name_meta == ud.name_meta)
            .unwrap();
        assert_eq!(
            ud_dead.files,
            [
                unit.join("out").join("u.d"),
                unit.join("out"),
                unit.join("fingerprint"),
                unit.clone(),
            ]
        );
    }
}
