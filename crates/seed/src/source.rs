//! Which live units of a target dir are registry units, and which files belong to
//! one unit. A dep-info's first input decides a unit that has one. A unit with no
//! dep-info is a registry unit only when it has an edge and every edge resolves to
//! a registry unit. An undecided target, an unsupported profile, or a hit bound
//! fills nothing from that target.

use crate::store::StoreKey;
use cargo_hive_cargo::ctx::{Ctx, checkout_walk};
use cargo_hive_cargo::discover::{self, Snapshot};
use cargo_hive_core::caps::{CRATE_DIR_CAP, DEPS_CAP};
use cargo_hive_core::fingerprint::{self, Format, Unit, le_hex};
use cargo_hive_core::liveness::{self, Liveness, LivenessFn, Uncertain};
use cargo_hive_core::receipt::Finding;
use cargo_hive_core::size::MAX_DEPTH;
use cargo_hive_core::target::{Owner, TargetDir};
use cargo_hive_core::unit_path::{self, UnitLayout, UnitLoc};
use cargo_hive_platform::{EntryKind, Fs};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceUnit {
    pub rel_profile: PathBuf,
    pub dir_name: String,
    pub stem: String,
    pub kind: String,
    pub fingerprint_hash: String,
    pub toolchain_value: u64,
    pub registry_deps: Vec<String>,
    pub layout: UnitLayout,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
    Undecided(String),
    Unsupported(String),
    Bound(String),
}

impl fmt::Display for Skip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Skip::Undecided(why) => write!(f, "liveness is undecided: {why}"),
            Skip::Unsupported(why) => write!(f, "profile dir unsupported: {why}"),
            Skip::Bound(why) => write!(f, "a bound was hit: {why}"),
        }
    }
}

impl std::error::Error for Skip {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused(pub PathBuf, pub String);

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.0.display(), self.1)
    }
}

impl std::error::Error for Refused {}

const DEP_INFO_CAP: u64 = 1024 * 1024;

pub fn cargo_home() -> PathBuf {
    match std::env::var_os("CARGO_HOME") {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cargo"),
    }
}

pub fn is_registry_dep_info(first_line: &str, cargo_home: &Path) -> bool {
    let Some((_, rest)) = first_line.split_once(": ") else {
        return false;
    };
    let unescaped = unescape_spaces(first_dep_input(rest));
    let prefix = format!("{}/", cargo_home.join("registry").join("src").display());
    unescaped.starts_with(&prefix)
}

/// A finding kind, the path it names when it names one, and its detail.
pub type SourceFinding = (String, Option<PathBuf>, String);

pub fn owned_targets(
    ctx: &Ctx<'_>,
    cancel: &(dyn Fn() -> bool + Sync),
) -> (Vec<TargetDir>, Vec<SourceFinding>) {
    let (checkouts, faults) = checkout_walk(ctx);
    let mut snapshot = Snapshot::default();
    let scan = discover::targets(&checkouts, ctx.cfg, false, &mut snapshot, cancel);
    let mut findings = Vec::new();
    for fault in faults.into_iter().chain(scan.walk_faults) {
        findings.push((
            "walk.unreadable".to_string(),
            None,
            format!("{}: {}", fault.path.display(), fault.detail),
        ));
    }
    let mut owned = Vec::new();
    for target in scan.targets {
        match &target.owner {
            Owner::Owned { .. } => owned.push(target),
            Owner::Unresolved { reason } => {
                // Seed names only this repo, so `status` advice would not make it a source.
                let why = if reason == discover::CACHE_ROOT_UNJUDGED {
                    "a cache-root target no checkout of this repository owns, so seed does not read it"
                } else {
                    reason.as_str()
                };
                findings.push((
                    "source.unresolved".to_string(),
                    Some(target.path.clone()),
                    format!("{}: {why}", target.path.display()),
                ));
            }
            Owner::OrphanCandidate { .. } => {}
        }
    }
    (owned, findings)
}

#[doc(hidden)]
pub fn unit_key(unit: &Unit) -> StoreKey {
    StoreKey {
        variant: 0,
        toolchain_value: unit.toolchain_value,
        unit_layout: unit.loc.layout,
        name_meta: unit.loc.name_meta.clone(),
        fingerprint_hash: unit.hash.to_string(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TargetHolds {
    pub keys: BTreeSet<StoreKey>,
    pub name_metas: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Holds {
    pub keys: BTreeSet<StoreKey>,
    pub name_metas: BTreeSet<String>,
    pub by_target: BTreeMap<PathBuf, TargetHolds>,
    pub whole_store: Vec<Finding>,
    pub findings: Vec<Finding>,
    pub by_reason: BTreeMap<&'static str, u64>,
    key_reason: BTreeMap<StoreKey, &'static str>,
}

impl Holds {
    pub(crate) fn hold_reason(&self, key: &StoreKey) -> Option<&'static str> {
        if let Some(reason) = self.key_reason.get(key) {
            return Some(*reason);
        }
        if self.name_metas.contains(&key.name_meta) {
            Some("name_meta")
        } else {
            None
        }
    }
}

#[doc(hidden)]
pub fn holds(
    fs: &dyn Fs,
    targets: &[TargetDir],
    now: SystemTime,
    window: Duration,
    liveness: LivenessFn,
) -> Holds {
    let mut out = Holds::default();
    for target in targets {
        if let Some(why) = &target.unreadable {
            let finding = Finding {
                kind: "store.target_unreadable".into(),
                path: Some(target.path.clone()),
                detail: why.clone(),
                measured: BTreeMap::new(),
            };
            out.whole_store.push(finding.clone());
            out.findings.push(finding);
            *out.by_reason.entry("whole_store").or_insert(0) += 1;
            continue;
        }
        match liveness(fs, target, now, window) {
            Liveness::Undecided { reasons } => {
                hold_undecided(fs, target, &reasons, &mut out);
            }
            Liveness::Decided { live, .. } => {
                let mut local = TargetHolds::default();
                for uref in live {
                    let key = target
                        .profiles
                        .iter()
                        .find(|profile| profile.path == uref.profile)
                        .and_then(|profile| profile.graph.as_ref())
                        .and_then(|graph| {
                            graph
                                .units
                                .iter()
                                .find(|unit| unit.loc.name_meta == uref.loc.name_meta)
                                .map(unit_key)
                        });
                    if let Some(key) = key {
                        record_key(&mut out, &mut local, key, "live");
                    } else {
                        record_name_meta(&mut out, &mut local, uref.loc.name_meta.clone());
                    }
                }
                if !local.keys.is_empty() || !local.name_metas.is_empty() {
                    out.by_target.insert(target.path.clone(), local);
                }
            }
        }
    }
    out
}

fn hold_undecided(fs: &dyn Fs, target: &TargetDir, reasons: &[Uncertain], out: &mut Holds) {
    let detail = reasons
        .iter()
        .map(|reason| format!("{reason}\n{}", reason.clears_when()))
        .collect::<Vec<_>>()
        .join("\n");
    let finding = Finding {
        kind: "store.target_undecided".into(),
        path: Some(target.path.clone()),
        detail,
        measured: BTreeMap::new(),
    };
    out.findings.push(finding.clone());

    let listing_blocks = reasons.iter().any(|reason| {
        matches!(
            reason,
            Uncertain::ListingFailed { .. } | Uncertain::TargetUnreadable { .. }
        )
    });
    if listing_blocks {
        out.whole_store.push(finding);
        *out.by_reason.entry("whole_store").or_insert(0) += 1;
        return;
    }

    let mut local = TargetHolds::default();
    for profile in &target.profiles {
        match &profile.format {
            Format::Unsupported(_) => {
                if !hold_unsupported_fingerprint(fs, &profile.path, out, &mut local) {
                    out.whole_store.push(finding);
                    *out.by_reason.entry("whole_store").or_insert(0) += 1;
                    return;
                }
            }
            Format::Supported => {
                if let Some(graph) = &profile.graph {
                    for unit in &graph.units {
                        record_key(out, &mut local, unit_key(unit), "undecided");
                    }
                }
                for nc in &profile.not_current_units {
                    if !nc.loc.name_meta.is_empty() {
                        record_name_meta(out, &mut local, nc.loc.name_meta.clone());
                    }
                }
            }
        }
    }
    if !local.keys.is_empty() || !local.name_metas.is_empty() {
        out.by_target.insert(target.path.clone(), local);
    }
}

fn hold_unsupported_fingerprint(
    fs: &dyn Fs,
    profile: &Path,
    out: &mut Holds,
    local: &mut TargetHolds,
) -> bool {
    let fp = profile.join(".fingerprint");
    match fs.read_dir_bounded(&fp, CRATE_DIR_CAP) {
        Ok(entries) => {
            for (name, kind) in entries {
                if kind == EntryKind::Dir {
                    record_name_meta(out, local, name);
                }
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(_) => return false,
    }
    hold_unsupported_unit_dirs(fs, profile, out, local)
}

fn hold_unsupported_unit_dirs(
    fs: &dyn Fs,
    profile: &Path,
    out: &mut Holds,
    local: &mut TargetHolds,
) -> bool {
    let build = profile.join("build");
    let packages = match fs.read_dir_bounded(&build, CRATE_DIR_CAP) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return true,
        Err(_) => return false,
    };
    for (pkg_name, kind) in packages {
        if kind != EntryKind::Dir || fingerprint::unit_hash(&pkg_name).is_some() {
            continue;
        }
        let pkg_dir = build.join(&pkg_name);
        let children = match fs.read_dir_bounded(&pkg_dir, CRATE_DIR_CAP) {
            Ok(entries) => entries,
            Err(err) if err.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => return false,
        };
        for (hex, kind) in children {
            if kind == EntryKind::Dir && fingerprint::UnitHash::parse(&hex).is_some() {
                record_name_meta(out, local, format!("{pkg_name}-{hex}"));
            }
        }
    }
    true
}

fn record_key(out: &mut Holds, local: &mut TargetHolds, key: StoreKey, reason: &'static str) {
    local.keys.insert(key.clone());
    out.keys.insert(key.clone());
    if reason == "live" {
        out.key_reason.insert(key, "live");
    } else {
        out.key_reason.entry(key).or_insert(reason);
    }
    *out.by_reason.entry(reason).or_insert(0) += 1;
}

fn record_name_meta(out: &mut Holds, local: &mut TargetHolds, name_meta: String) {
    local.name_metas.insert(name_meta.clone());
    out.name_metas.insert(name_meta);
    *out.by_reason.entry("name_meta").or_insert(0) += 1;
}

#[doc(hidden)]
pub fn live_registry_units(
    ctx: &Ctx<'_>,
    target: &TargetDir,
    cargo_home: &Path,
    window: Duration,
) -> Result<(Vec<SourceUnit>, Vec<String>), Skip> {
    let unsupported: Vec<String> = target
        .profiles
        .iter()
        .filter_map(|profile| match &profile.format {
            Format::Unsupported(why) => Some(format!("{}: {why}", profile.path.display())),
            Format::Supported => None,
        })
        .collect();
    if !unsupported.is_empty() {
        return Err(Skip::Unsupported(unsupported.join("\n")));
    }
    let live = match (ctx.liveness)(ctx.fs, target, ctx.now, window) {
        Liveness::Undecided { reasons } => {
            let why = reasons
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            return Err(Skip::Undecided(why));
        }
        Liveness::Decided { live, .. } => live,
    };
    let live_keys: HashSet<(PathBuf, String)> = live
        .into_iter()
        .map(|unit| (unit.profile, unit.loc.name_meta))
        .collect();

    let mut deps_of: HashMap<PathBuf, Vec<(String, EntryKind)>> = HashMap::new();
    for profile in &target.profiles {
        deps_of.insert(profile.path.clone(), list_deps(ctx.fs, &profile.path)?);
    }

    let mut cands = Vec::new();
    for profile in &target.profiles {
        let Some(graph) = &profile.graph else {
            continue;
        };
        let listing = deps_of.get(&profile.path).map(Vec::as_slice).unwrap_or(&[]);
        let rel = relative_to(&target.path, &profile.path);
        for unit in &graph.units {
            let key = unit_key(unit);
            let class = dep_class(
                ctx.fs,
                &profile.path,
                &unit.loc,
                &unit.stem,
                listing,
                cargo_home,
            )?;
            let (known, registry) = match class {
                DepClass::Registry => (true, true),
                DepClass::Workspace => (true, false),
                DepClass::Absent => (false, false),
            };
            cands.push(Cand {
                rel_profile: rel.clone(),
                dir_name: key.name_meta.clone(),
                stem: unit.stem.clone(),
                kind: unit.kind.as_str().to_string(),
                fingerprint_hash: key.fingerprint_hash,
                toolchain_value: key.toolchain_value,
                layout: unit.loc.layout,
                deps: unit.deps.iter().map(|dep| dep.fingerprint).collect(),
                live: live_keys.contains(&(profile.path.clone(), key.name_meta)),
                known,
                registry,
                left_out: false,
            });
        }
    }

    let mut by_hash: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, cand) in cands.iter().enumerate() {
        by_hash
            .entry(cand.fingerprint_hash.clone())
            .or_default()
            .push(i);
    }
    settle_units_without_dep_info(&mut cands, &by_hash);
    mark_unresolved_edges(&mut cands, &by_hash);

    let mut units = Vec::new();
    let mut left_out = Vec::new();
    for cand in &cands {
        if cand.live && cand.left_out {
            left_out.push(cand.dir_name.clone());
        }
        if !(cand.live && cand.registry && !cand.left_out) {
            continue;
        }
        units.push(SourceUnit {
            rel_profile: cand.rel_profile.clone(),
            dir_name: cand.dir_name.clone(),
            stem: cand.stem.clone(),
            kind: cand.kind.clone(),
            fingerprint_hash: cand.fingerprint_hash.clone(),
            toolchain_value: cand.toolchain_value,
            registry_deps: registry_dep_hashes(cand, &cands, &by_hash),
            layout: cand.layout,
        });
    }
    units.sort_by(|a, b| (&a.rel_profile, &a.dir_name).cmp(&(&b.rel_profile, &b.dir_name)));
    left_out.sort();
    left_out.dedup();
    Ok((units, left_out))
}

pub fn unit_files(
    fs: &dyn Fs,
    target: &Path,
    unit: &SourceUnit,
    deps_listing: &[(String, EntryKind)],
    dir_cap: usize,
) -> Result<Vec<PathBuf>, Refused> {
    let profile = target.join(&unit.rel_profile);
    let mut files = Vec::new();
    match unit.layout {
        UnitLayout::Flat => {
            let loc = UnitLoc {
                layout: UnitLayout::Flat,
                name_meta: unit.dir_name.clone(),
            };
            walk(
                fs,
                target,
                &unit_path::fingerprint_dir(&profile, &loc),
                dir_cap,
                0,
                false,
                &mut files,
            )?;
            let run_stamp = unit_path::stamp(&profile, &loc, fingerprint::UnitKind::RunBuildScript);
            let build_dir = run_stamp.parent().expect("stamp path has a parent");
            walk(fs, target, build_dir, dir_cap, 0, false, &mut files)?;
            let meta = fingerprint::unit_hash(&unit.dir_name)
                .map(|h| h.to_string())
                .unwrap_or_default();
            let mut names: Vec<&str> = deps_listing
                .iter()
                .filter(|(name, _)| {
                    !name.ends_with(".rcgu.o")
                        && liveness::embedded_hashes(name).any(|hash| hash == meta)
                })
                .map(|(name, _)| name.as_str())
                .collect();
            names.sort_unstable();
            let deps = unit_path::flat_deps_dir(&profile);
            for name in names {
                take_entry(fs, target, &deps.join(name), dir_cap, 0, false, &mut files)?;
            }
        }
        UnitLayout::UnitDir => {
            let loc = UnitLoc {
                layout: UnitLayout::UnitDir,
                name_meta: unit.dir_name.clone(),
            };
            let Some(root) = unit_path::unit_dir(&profile, &loc) else {
                return Ok(files);
            };
            walk(fs, target, &root, dir_cap, 0, true, &mut files)?;
        }
    }
    files.sort();
    Ok(files)
}

struct Cand {
    rel_profile: PathBuf,
    dir_name: String,
    stem: String,
    kind: String,
    fingerprint_hash: String,
    toolchain_value: u64,
    layout: UnitLayout,
    deps: Vec<u64>,
    live: bool,
    known: bool,
    registry: bool,
    left_out: bool,
}

enum DepClass {
    Registry,
    Workspace,
    Absent,
}

fn list_deps(fs: &dyn Fs, profile: &Path) -> Result<Vec<(String, EntryKind)>, Skip> {
    let dir = unit_path::flat_deps_dir(profile);
    match fs.read_dir_bounded(&dir, DEPS_CAP) {
        Ok(entries) => Ok(entries),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(Skip::Bound(super::directory_listing(
            &dir,
            &err,
            "build the target again",
        ))),
    }
}

/// Flat: `deps/<crate>-<metadata>.d` or a hash-named `.d` under `build/<dir>/`.
/// Unit-dir: every `.d` in the unit's `out/`. `<crate>` is the fingerprint stem
/// with its kind prefix removed (`lib-depcrate` → `depcrate`).
fn dep_class(
    fs: &dyn Fs,
    profile: &Path,
    loc: &UnitLoc,
    stem: &str,
    deps_listing: &[(String, EntryKind)],
    cargo_home: &Path,
) -> Result<DepClass, Skip> {
    match loc.layout {
        UnitLayout::Flat => {
            flat_dep_class(fs, profile, &loc.name_meta, stem, deps_listing, cargo_home)
        }
        UnitLayout::UnitDir => unit_dir_dep_class(fs, profile, loc, cargo_home),
    }
}

fn flat_dep_class(
    fs: &dyn Fs,
    profile: &Path,
    dir_name: &str,
    stem: &str,
    deps_listing: &[(String, EntryKind)],
    cargo_home: &Path,
) -> Result<DepClass, Skip> {
    let Some(meta) = fingerprint::unit_hash(dir_name) else {
        return Ok(DepClass::Absent);
    };
    let meta_text = meta.to_string();
    let dep_name = format!("{}-{meta_text}.d", dep_info_crate(stem));
    let path = if deps_listing
        .iter()
        .any(|(name, kind)| name == &dep_name && *kind == EntryKind::File)
    {
        Some(unit_path::flat_deps_dir(profile).join(dep_name))
    } else {
        build_dep_info(fs, profile, dir_name, &meta_text)?
    };
    let Some(path) = path else {
        return Ok(DepClass::Absent);
    };
    let Some(line) = read_dep_info(&path)? else {
        return Ok(DepClass::Absent);
    };
    Ok(if is_registry_dep_info(&line, cargo_home) {
        DepClass::Registry
    } else {
        DepClass::Workspace
    })
}

fn unit_dir_dep_class(
    fs: &dyn Fs,
    profile: &Path,
    loc: &UnitLoc,
    cargo_home: &Path,
) -> Result<DepClass, Skip> {
    let out = unit_path::outputs(profile, loc);
    let entries = match fs.read_dir_bounded(&out, CRATE_DIR_CAP) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(DepClass::Absent),
        Err(err) => {
            return Err(Skip::Bound(super::directory_listing(
                &out,
                &err,
                "build the target again",
            )));
        }
    };
    let mut names: Vec<String> = entries
        .into_iter()
        .filter(|(name, kind)| *kind == EntryKind::File && name.ends_with(".d"))
        .map(|(name, _)| name)
        .collect();
    names.sort();
    if names.is_empty() {
        return Ok(DepClass::Absent);
    }
    classify_dep_lines(
        names.into_iter().map(|name| read_dep_info(&out.join(name))),
        cargo_home,
    )
}

fn classify_dep_lines(
    lines: impl IntoIterator<Item = Result<Option<String>, Skip>>,
    cargo_home: &Path,
) -> Result<DepClass, Skip> {
    let mut saw_registry = false;
    for line in lines {
        let Some(line) = line? else {
            continue;
        };
        if is_registry_dep_info(&line, cargo_home) {
            saw_registry = true;
        } else {
            return Ok(DepClass::Workspace);
        }
    }
    Ok(if saw_registry {
        DepClass::Registry
    } else {
        DepClass::Absent
    })
}

fn build_dep_info(
    fs: &dyn Fs,
    profile: &Path,
    dir_name: &str,
    meta: &str,
) -> Result<Option<PathBuf>, Skip> {
    let loc = UnitLoc {
        layout: UnitLayout::Flat,
        name_meta: dir_name.to_string(),
    };
    let run_stamp = unit_path::stamp(profile, &loc, fingerprint::UnitKind::RunBuildScript);
    let dir = run_stamp
        .parent()
        .expect("stamp path has a parent")
        .to_path_buf();
    let entries = match fs.read_dir_bounded(&dir, CRATE_DIR_CAP) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(Skip::Bound(super::directory_listing(
                &dir,
                &err,
                "build the target again",
            )));
        }
    };
    let mut names: Vec<String> = entries
        .into_iter()
        .filter(|(name, kind)| {
            *kind == EntryKind::File
                && name.ends_with(".d")
                && liveness::embedded_hashes(name).any(|hash| hash == meta)
        })
        .map(|(name, _)| name)
        .collect();
    names.sort();
    Ok(names.first().map(|name| dir.join(name)))
}

fn read_dep_info(path: &Path) -> Result<Option<String>, Skip> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(Skip::Bound(format!(
                "{}: {err}; make the dep-info readable and build the target again",
                path.display()
            )));
        }
    };
    read_dep_info_line(&mut file, path).map(Some)
}

fn read_dep_info_line(mut reader: impl Read, path: &Path) -> Result<String, Skip> {
    let mut buf = Vec::new();
    reader
        .by_ref()
        .take(DEP_INFO_CAP + 1)
        .read_to_end(&mut buf)
        .map_err(|err| {
            Skip::Bound(format!(
                "{}: {err}; make the dep-info readable and build the target again",
                path.display()
            ))
        })?;
    if buf.len() as u64 > DEP_INFO_CAP {
        return Err(Skip::Bound(format!(
            "{} is larger than 1 MiB; shorten the dep-info below 1 MiB and build the target again",
            path.display()
        )));
    }
    let text = String::from_utf8(buf).map_err(|_| {
        Skip::Bound(format!(
            "{} is not valid UTF-8; rewrite the dep-info as UTF-8 and build the target again",
            path.display()
        ))
    })?;
    Ok(text.split('\n').next().unwrap_or("").to_string())
}

fn settle_units_without_dep_info(cands: &mut [Cand], by_hash: &HashMap<String, Vec<usize>>) {
    loop {
        let mut changed = false;
        for i in 0..cands.len() {
            if cands[i].known {
                continue;
            }
            let deps = cands[i].deps.clone();
            if deps.is_empty() {
                cands[i].known = true;
                changed = true;
                continue;
            }
            let mut pending = false;
            let mut bad = false;
            let mut missing = false;
            for dep in deps {
                match by_hash.get(&le_hex(dep)) {
                    None => missing = true,
                    Some(hits) => {
                        if hits
                            .iter()
                            .any(|&hit| cands[hit].known && !cands[hit].registry)
                        {
                            bad = true;
                        }
                        if hits.iter().any(|&hit| !cands[hit].known) {
                            pending = true;
                        }
                    }
                }
            }
            if bad {
                cands[i].known = true;
                changed = true;
            } else if !pending && missing {
                cands[i].known = true;
                cands[i].registry = true;
                cands[i].left_out = true;
                changed = true;
            } else if !pending {
                cands[i].known = true;
                cands[i].registry = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

fn mark_unresolved_edges(cands: &mut [Cand], by_hash: &HashMap<String, Vec<usize>>) {
    for cand in cands.iter_mut() {
        if !cand.registry || cand.left_out {
            continue;
        }
        if cand
            .deps
            .iter()
            .any(|dep| !by_hash.contains_key(&le_hex(*dep)))
        {
            cand.left_out = true;
        }
    }
}

fn registry_dep_hashes(
    cand: &Cand,
    cands: &[Cand],
    by_hash: &HashMap<String, Vec<usize>>,
) -> Vec<String> {
    let mut hashes = Vec::new();
    for dep in &cand.deps {
        let hash = le_hex(*dep);
        let Some(hits) = by_hash.get(&hash) else {
            continue;
        };
        if hits.iter().all(|&hit| cands[hit].registry) && !hashes.contains(&hash) {
            hashes.push(hash);
        }
    }
    hashes
}

fn walk(
    fs: &dyn Fs,
    target: &Path,
    dir: &Path,
    dir_cap: usize,
    depth: usize,
    skip_rcgu_under_out: bool,
    files: &mut Vec<PathBuf>,
) -> Result<(), Refused> {
    if depth > MAX_DEPTH {
        return Err(Refused(
            dir.to_path_buf(),
            format!(
                "{} is deeper than {MAX_DEPTH} levels; remove directories past that depth and build the unit again",
                dir.display()
            ),
        ));
    }
    let entries = match fs.read_dir_bounded(dir, dir_cap) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(Refused(dir.to_path_buf(), err.to_string())),
    };
    let skip_rcgu = skip_rcgu_under_out && dir.file_name().is_some_and(|name| name == "out");
    for (name, _) in entries {
        if skip_rcgu && name.ends_with(".rcgu.o") {
            continue;
        }
        take_entry(
            fs,
            target,
            &dir.join(name),
            dir_cap,
            depth + 1,
            skip_rcgu_under_out,
            files,
        )?;
    }
    Ok(())
}

fn take_entry(
    fs: &dyn Fs,
    target: &Path,
    path: &Path,
    dir_cap: usize,
    depth: usize,
    skip_rcgu_under_out: bool,
    files: &mut Vec<PathBuf>,
) -> Result<(), Refused> {
    let stat = fs
        .entry_stat(path)
        .map_err(|err| Refused(path.to_path_buf(), err.to_string()))?;
    match stat.kind {
        EntryKind::File => {
            files.push(relative_to(target, path));
            Ok(())
        }
        EntryKind::Dir => walk(fs, target, path, dir_cap, depth, skip_rcgu_under_out, files),
        EntryKind::Symlink => Err(Refused(
            path.to_path_buf(),
            "symbolic link; remove it and build the unit again".to_string(),
        )),
        EntryKind::Other => Err(Refused(
            path.to_path_buf(),
            "not a regular file or directory; remove it and build the unit again".to_string(),
        )),
    }
}

fn dep_info_crate(stem: &str) -> &str {
    let kind = fingerprint::UnitKind::of(stem);
    match stem.strip_prefix(&format!("{}-", kind.as_str())) {
        Some(name) if !name.is_empty() => name,
        _ => stem,
    }
}

fn relative_to(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| path.to_path_buf())
}

fn first_dep_input(rest: &str) -> &str {
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b' ' && (i == 0 || bytes[i - 1] != b'\\') {
            return &rest[..i];
        }
        i += 1;
    }
    rest
}

fn unescape_spaces(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' && chars.peek() == Some(&' ') {
            chars.next();
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(any(test, fuzzing))]
#[doc(hidden)]
pub mod fuzzing {
    use super::*;
    pub fn dep_info(data: &[u8]) {
        let home = Path::new("cargo-home");
        if let Ok(line) = read_dep_info_line(data, Path::new("probe.d")) {
            let _ = is_registry_dep_info(&line, home);
        }
        let suffix = match data.first().copied().unwrap_or(0) % 3 {
            0 => "name.rs",
            1 => "file\\ name.rs",
            _ => "unicode-λ.rs",
        };
        let registry =
            format!("object: cargo-home/registry/src/package/{suffix} other.rs\nignored: line\n");
        let workspace = "object: project/src/lib.rs\n".to_string();
        let line = read_dep_info_line(registry.as_bytes(), Path::new("probe.d")).unwrap();
        assert!(is_registry_dep_info(&line, home));
        assert!(!is_registry_dep_info(&workspace, home));
        assert!(!is_registry_dep_info(
            "object: cargo-home/registry/src-other/pkg.rs",
            home
        ));
        assert!(is_registry_dep_info(
            "object: cargo-home/registry/src/",
            home
        ));
        assert!(matches!(
            classify_dep_lines([Ok(Some(line.clone())), Ok(Some(workspace))], home).unwrap(),
            DepClass::Workspace
        ));
        assert!(matches!(
            classify_dep_lines([Ok(Some(line)), Ok(None)], home).unwrap(),
            DepClass::Registry
        ));
        assert!(matches!(
            classify_dep_lines([Ok(None)], home).unwrap(),
            DepClass::Absent
        ));
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;

    struct Chunked<'a> {
        bytes: &'a [u8],
        interrupted: bool,
        fail_at_end: bool,
    }

    impl Read for Chunked<'_> {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            if self.interrupted {
                self.interrupted = false;
                return Err(io::ErrorKind::Interrupted.into());
            }
            if self.bytes.is_empty() && self.fail_at_end {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            let n = out.len().min(2).min(self.bytes.len());
            out[..n].copy_from_slice(&self.bytes[..n]);
            self.bytes = &self.bytes[n..];
            Ok(n)
        }
    }

    #[test]
    fn dep_info_prefixes_preserve_the_first_prerequisite_rule() {
        let prefix = "object: cargo-home/registry/src/";
        let text = format!("{prefix}package/lib.rs other.rs\nignored: line\n");
        let home = Path::new("cargo-home");
        for end in 0..=text.len() {
            let line = read_dep_info_line(&text.as_bytes()[..end], Path::new("probe.d")).unwrap();
            assert_eq!(
                is_registry_dep_info(&line, home),
                end >= prefix.len(),
                "cut {end}"
            );
        }
    }

    #[test]
    fn dep_info_streams_retry_interruptions_and_refuse_errors_after_valid_prefixes() {
        let text = b"object: cargo-home/registry/src/package/lib.rs\n";
        for fail_at_end in [false, true] {
            let stream = Chunked {
                bytes: text,
                interrupted: true,
                fail_at_end,
            };
            let result = read_dep_info_line(stream, Path::new("probe.d"));
            if fail_at_end {
                assert!(
                    matches!(result, Err(Skip::Bound(why)) if why.contains("probe.d") && why.contains("permission denied"))
                );
            } else {
                assert_eq!(
                    result.unwrap(),
                    "object: cargo-home/registry/src/package/lib.rs"
                );
            }
        }
    }

    #[test]
    fn dependency_reader_models_keep_prefix_rules_and_workspace_veto() {
        for input in [&b""[..], &b"\xff\n"[..], &b"2"[..], &b"target: x\n"[..]] {
            fuzzing::dep_info(input);
        }
        assert!(read_dep_info_line(&b"\xff"[..], Path::new("probe.d")).is_err());
        let exact = std::io::repeat(b'x').take(DEP_INFO_CAP);
        assert_eq!(
            read_dep_info_line(exact, Path::new("probe.d"))
                .unwrap()
                .len() as u64,
            DEP_INFO_CAP
        );
        assert!(
            read_dep_info_line(
                std::io::repeat(b'x').take(DEP_INFO_CAP + 1),
                Path::new("probe.d")
            )
            .is_err()
        );
        assert!(read_dep_info_line(FailingRead, Path::new("probe.d")).is_err());
    }

    struct FailingRead;
    impl Read for FailingRead {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::ErrorKind::PermissionDenied.into())
        }
    }

    use cargo_hive_cargo::ctx::Ctx;
    use cargo_hive_cargo::registry::Repo;
    use cargo_hive_core::fingerprint::le_hex;
    use cargo_hive_core::size::MAX_DEPTH;
    use cargo_hive_core::target::{Owner, TAG_LINE};
    use cargo_hive_platform::EntryKind;
    use cargo_hive_state::{Config, State};
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Scratch {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = PathBuf::from(format!(
                "/private/tmp/hive-it-seed-{label}-{}-{nanos}",
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

    struct CargoHomeGuard {
        prev: Option<std::ffi::OsString>,
    }

    impl CargoHomeGuard {
        fn set(path: &Path) -> CargoHomeGuard {
            let prev = std::env::var_os("CARGO_HOME");
            // `cargo metadata` inherits this process's environment. The previous
            // value is restored when the guard drops, including when an assertion panics.
            cargo_hive_platform::testing::set_env("CARGO_HOME", path);
            CargoHomeGuard { prev }
        }
    }

    impl Drop for CargoHomeGuard {
        fn drop(&mut self) {
            // Paired with `CargoHomeGuard::set`. Restores the environment this test replaced.
            match self.prev.take() {
                Some(value) => cargo_hive_platform::testing::set_env("CARGO_HOME", value),
                None => cargo_hive_platform::testing::remove_env("CARGO_HOME"),
            }
        }
    }

    struct Env {
        state: State,
        cfg: Config,
        platform: cargo_hive_platform::Platform,
        now: SystemTime,
    }

    impl Env {
        fn new(scratch: &Path) -> Env {
            Env {
                state: State::open_at(scratch.join("hive")).unwrap(),
                cfg: Config {
                    secondary_roots: Vec::new(),
                    cache_roots: Vec::new(),
                    gc_window_days: None,
                },
                platform: cargo_hive_platform::current().unwrap(),
                now: SystemTime::now(),
            }
        }

        fn ctx(&self) -> Ctx<'_> {
            Ctx {
                state: &self.state,
                fs: self.platform.fs.as_ref(),
                cfg: &self.cfg,
                now: self.now,
                account_bytes: false,
                repos: &[],
                mains: &[],
                liveness: crate::no_liveness,
            }
        }
    }

    fn source_unit(rel: &str, dir_name: &str) -> SourceUnit {
        SourceUnit {
            rel_profile: PathBuf::from(rel),
            dir_name: dir_name.to_string(),
            stem: "lib-pkg".into(),
            kind: "lib".into(),
            fingerprint_hash: "0a00000000000000".into(),
            toolchain_value: 1,
            registry_deps: Vec::new(),
            layout: UnitLayout::Flat,
        }
    }

    fn plant(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"x").unwrap();
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args([
                "-c",
                "user.name=hive",
                "-c",
                "user.email=hive@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "init.defaultBranch=main",
                "-c",
                "core.fsmonitor=false",
            ])
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    fn tag_target(dir: &Path) {
        fs::create_dir_all(dir.join("debug/.fingerprint")).unwrap();
        fs::write(dir.join("CACHEDIR.TAG"), format!("{TAG_LINE}\n")).unwrap();
    }

    #[test]
    fn the_first_input_decides_whether_a_unit_is_a_registry_unit() {
        let home = Path::new("/Users/x/.cargo");
        let cases = [
            (
                "/t/deps/a-1.d: /Users/x/.cargo/registry/src/index.crates.io-1/a-1.0.0/src/lib.rs",
                true,
            ),
            ("/t/deps/b-2.d: src/lib.rs src/x.rs", false),
            (
                "/t/deps/c-3.d: src/lib.rs /Users/x/.cargo/registry/src/index.crates.io-1/a-1.0.0/data.txt",
                false,
            ),
            (
                "/t/deps/d-4.d: /Users/x/.cargo/git/checkouts/d/src/lib.rs",
                false,
            ),
            ("/t/deps/e-5.d:", false),
            (
                "/t/deps/f-6.d: /Users/x\\ y/.cargo/registry/src/i/f/src/lib.rs",
                false,
            ),
        ];
        for (line, want) in cases {
            assert_eq!(is_registry_dep_info(line, home), want, "{line}");
        }
        let spaced = Path::new("/Users/x y/.cargo");
        assert!(is_registry_dep_info(
            "/t/deps/f-6.d: /Users/x\\ y/.cargo/registry/src/i/f/src/lib.rs",
            spaced
        ));
    }

    #[test]
    fn unit_files_takes_the_whole_unit_dirs_and_hash_named_deps_but_no_rcgu_objects() {
        let scratch = Scratch::new("files");
        let env = Env::new(&scratch.0);
        let dir = "pkg-0123456789abcdef";
        let fp = scratch.0.join("debug/.fingerprint").join(dir);
        let build = scratch.0.join("debug/build").join(dir);
        plant(&fp.join("lib-pkg"));
        plant(&fp.join("lib-pkg.json"));
        plant(&fp.join("invoked.timestamp"));
        plant(&fp.join("extra/note"));
        plant(&build.join("output"));
        plant(&build.join("invoked.timestamp"));
        let hash = "0123456789abcdef";
        let deps = [
            (format!("libpkg-{hash}.rlib"), EntryKind::File),
            (format!("libpkg-{hash}.rmeta"), EntryKind::File),
            (format!("pkg-{hash}.d"), EntryKind::File),
            (format!("libpkg-{hash}.rcgu.o"), EntryKind::File),
            ("other-fedcba9876543210.rlib".to_string(), EntryKind::File),
        ];
        for (name, _) in &deps {
            plant(&scratch.0.join("debug/deps").join(name));
        }
        let listing: Vec<(String, EntryKind)> = deps.to_vec();
        let got = unit_files(
            env.platform.fs.as_ref(),
            &scratch.0,
            &source_unit("debug", dir),
            &listing,
            10,
        )
        .unwrap();
        let rel = |tail: &str| PathBuf::from(format!("debug/{tail}"));
        assert_eq!(
            got,
            vec![
                rel(".fingerprint/pkg-0123456789abcdef/extra/note"),
                rel(".fingerprint/pkg-0123456789abcdef/invoked.timestamp"),
                rel(".fingerprint/pkg-0123456789abcdef/lib-pkg"),
                rel(".fingerprint/pkg-0123456789abcdef/lib-pkg.json"),
                rel("build/pkg-0123456789abcdef/invoked.timestamp"),
                rel("build/pkg-0123456789abcdef/output"),
                rel(&format!("deps/libpkg-{hash}.rlib")),
                rel(&format!("deps/libpkg-{hash}.rmeta")),
                rel(&format!("deps/pkg-{hash}.d")),
            ]
        );
    }

    #[test]
    fn unit_files_walks_a_dsym_directory_within_its_bound() {
        let scratch = Scratch::new("dsym");
        let env = Env::new(&scratch.0);
        let dir = "pkg-0123456789abcdef";
        let hash = "0123456789abcdef";
        let bundle = format!("pkg-{hash}.dSYM");
        plant(
            &scratch
                .0
                .join("debug/deps")
                .join(&bundle)
                .join("Contents/Info.plist"),
        );
        plant(
            &scratch
                .0
                .join("debug/deps")
                .join(&bundle)
                .join("Contents/Resources/DWARF/pkg"),
        );
        let listing = [(bundle.clone(), EntryKind::Dir)];
        let got = unit_files(
            env.platform.fs.as_ref(),
            &scratch.0,
            &source_unit("debug", dir),
            &listing,
            2,
        )
        .unwrap();
        assert_eq!(
            got,
            vec![
                PathBuf::from(format!("debug/deps/{bundle}/Contents/Info.plist")),
                PathBuf::from(format!("debug/deps/{bundle}/Contents/Resources/DWARF/pkg")),
            ]
        );
    }

    #[test]
    fn a_symlink_in_a_unit_refuses_the_whole_unit() {
        let scratch = Scratch::new("symlink");
        let env = Env::new(&scratch.0);
        let dir = "pkg-0123456789abcdef";
        let fp = scratch.0.join("debug/.fingerprint").join(dir);
        plant(&fp.join("lib-pkg"));
        let link = fp.join("link");
        std::os::unix::fs::symlink(fp.join("lib-pkg"), &link).unwrap();
        let err = unit_files(
            env.platform.fs.as_ref(),
            &scratch.0,
            &source_unit("debug", dir),
            &[],
            10,
        )
        .unwrap_err();
        assert_eq!(err.0, link);
        assert!(err.1.contains("symbolic link"), "{err:?}");
    }

    #[test]
    fn a_unit_dir_at_its_cap_lists_and_one_past_it_refuses() {
        let dir = "pkg-0123456789abcdef";
        for cap in [2usize, 3] {
            let ok = Scratch::new("dir-cap-ok");
            let env = Env::new(&ok.0);
            let fp = ok.0.join("debug/.fingerprint").join(dir);
            for i in 0..cap {
                plant(&fp.join(format!("f{i}")));
            }
            let listed = unit_files(
                env.platform.fs.as_ref(),
                &ok.0,
                &source_unit("debug", dir),
                &[],
                cap,
            )
            .unwrap();
            assert_eq!(listed.len(), cap, "cap {cap}");

            let over = Scratch::new("dir-cap-over");
            let env = Env::new(&over.0);
            let fp = over.0.join("debug/.fingerprint").join(dir);
            for i in 0..=cap {
                plant(&fp.join(format!("f{i}")));
            }
            let err = unit_files(
                env.platform.fs.as_ref(),
                &over.0,
                &source_unit("debug", dir),
                &[],
                cap,
            )
            .unwrap_err();
            assert_eq!(err.0, fp);
            assert!(err.1.contains("more than"), "{err:?}");
        }
    }

    #[test]
    fn a_unit_walk_at_the_depth_bound_lists_and_one_past_it_refuses() {
        let dir_name = "pkg-0123456789abcdef";
        let nest = |root: &Path, levels: usize| {
            let mut dir = root.to_path_buf();
            for i in 1..=levels {
                dir.push(format!("n{i}"));
            }
            fs::create_dir_all(&dir).unwrap();
            dir
        };

        let ok = Scratch::new("depth-ok");
        let env = Env::new(&ok.0);
        let fp = ok.0.join("debug/.fingerprint").join(dir_name);
        let leaf = nest(&fp, MAX_DEPTH);
        plant(&leaf.join("kept"));
        let got = unit_files(
            env.platform.fs.as_ref(),
            &ok.0,
            &source_unit("debug", dir_name),
            &[],
            4,
        )
        .unwrap();
        assert_eq!(got.len(), 1);
        assert!(got[0].ends_with("kept"), "{got:?}");

        let over = Scratch::new("depth-over");
        let env = Env::new(&over.0);
        let fp = over.0.join("debug/.fingerprint").join(dir_name);
        let leaf = nest(&fp, MAX_DEPTH + 1);
        plant(&leaf.join("too-deep"));
        let err = unit_files(
            env.platform.fs.as_ref(),
            &over.0,
            &source_unit("debug", dir_name),
            &[],
            4,
        )
        .unwrap_err();
        assert!(
            err.1.contains("deeper") && err.1.contains("build the unit again"),
            "{err:?}"
        );
    }

    #[test]
    fn owned_targets_keeps_a_target_this_repo_owns_and_reports_walk_faults_and_unresolved_targets()
    {
        let scratch = Scratch::new("owned");
        let repo = scratch.0.join("repo");
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(
            repo.join("Cargo.toml"),
            "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        fs::write(repo.join("src/lib.rs"), "pub fn answer() -> u32 { 42 }\n").unwrap();
        fs::write(
            repo.join("Cargo.lock"),
            "# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 4\n\n[[package]]\nname = \"fixture\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let target = repo.join("target");
        tag_target(&target);
        git(&repo, &["init", "-q"]);
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-q", "-m", "init"]);
        let root = git(&repo, &["rev-list", "--max-parents=0", "HEAD"]);
        let repo = repo.canonicalize().unwrap();
        let orphan_root = scratch.0.join("cache");
        let orphan = orphan_root.join("orphan-target");
        tag_target(&orphan);
        let sealed = scratch.0.join("not-a-dir");
        fs::write(&sealed, b"x").unwrap();

        let state = State::open_at(scratch.0.join("hive")).unwrap();
        let cfg = Config {
            secondary_roots: vec![sealed.clone()],
            cache_roots: vec![orphan_root],
            gc_window_days: None,
        };
        let repos = [Repo {
            main: repo.clone(),
            root_commits: BTreeSet::from([root]),
        }];
        let mains = [repo.clone()];
        let platform = cargo_hive_platform::current().unwrap();
        let ctx = Ctx {
            state: &state,
            fs: platform.fs.as_ref(),
            cfg: &cfg,
            now: SystemTime::now(),
            account_bytes: false,
            repos: &repos,
            mains: &mains,
            liveness: crate::no_liveness,
        };
        let cargo_home = scratch.0.join("cargo-home");
        fs::create_dir_all(&cargo_home).unwrap();
        let _cargo_home = CargoHomeGuard::set(&cargo_home);
        let (targets, findings) = owned_targets(&ctx, &|| false);
        assert_eq!(targets.len(), 1, "{targets:?} {findings:?}");
        assert_eq!(targets[0].path, target.canonicalize().unwrap());
        assert!(
            matches!(&targets[0].owner, Owner::Owned { checkout } if *checkout == repo),
            "{:?}",
            targets[0].owner
        );
        let orphan = orphan.canonicalize().unwrap();
        assert!(
            findings
                .iter()
                .any(|(kind, path, detail)| kind == "source.unresolved"
                    && path.as_deref() == Some(orphan.as_path())
                    && detail.contains("cache-root")),
            "{findings:?}"
        );
        assert!(
            findings
                .iter()
                .any(|(kind, _, detail)| kind == "walk.unreadable"
                    && detail.contains(&sealed.display().to_string())),
            "{findings:?}"
        );
    }

    #[test]
    fn a_deps_listing_that_is_not_the_cap_does_not_say_shrink() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new("deps-perm");
        let env = Env::new(&scratch.0);
        let profile = scratch.0.join("debug");
        let deps = profile.join("deps");
        fs::create_dir_all(&deps).unwrap();
        fs::set_permissions(&deps, fs::Permissions::from_mode(0o000)).unwrap();
        let err = list_deps(env.ctx().fs, &profile).unwrap_err();
        fs::set_permissions(&deps, fs::Permissions::from_mode(0o755)).unwrap();
        let Skip::Bound(text) = err else {
            panic!("expected the listing error, got {err}");
        };
        assert!(!text.contains("shrink"), "{text}");
        assert!(text.contains(&deps.display().to_string()), "{text}");
        assert!(
            text.contains("Permission denied") || text.contains("os error 13"),
            "{text}"
        );
        assert!(text.contains("make the directory readable"), "{text}");
        assert!(text.contains("build the target again"), "{text}");
    }

    #[test]
    fn a_unit_key_carries_the_toolchain_value_directory_name_and_fingerprint() {
        use cargo_hive_core::fingerprint::{Coverage, Unit, UnitHash};
        use cargo_hive_core::unit_path::{UnitLayout, UnitLoc};
        use std::sync::Arc;

        let fingerprint = 0x0102_0304_0506_0708u64;
        let unit = Unit {
            loc: UnitLoc {
                layout: UnitLayout::Flat,
                name_meta: format!("serde-{}", le_hex(fingerprint)),
            },
            kind: fingerprint::UnitKind::Lib,
            hash: UnitHash::parse(&le_hex(fingerprint)).unwrap(),
            deps: Vec::new(),
            stem: "lib-serde".into(),
            features: Arc::from(""),
            coverage: Coverage::NotRecorded,
            profile: 0,
            target: 0,
            path: 0,
            toolchain_value: 42,
        };
        let key = unit_key(&unit);
        assert_eq!(key.toolchain_value, 42);
        assert_eq!(key.name_meta, unit.loc.name_meta);
        assert_eq!(key.fingerprint_hash, le_hex(fingerprint));
    }
}
