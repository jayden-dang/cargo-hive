//! Checkouts of a registered repo and (Task 13) its Cargo target dirs. Every walk is
//! depth-bounded and never enters `node_modules`, `target` or `.git`.

use crate::cargo_target::{self, Layout, Owner, Resolved, TargetDir};
use crate::classify::{classify, CheckoutFacts, CheckoutKind, Class};
use crate::fingerprint::{self, Format};
use crate::git::Git;
use crate::registry::Repo;
use crate::state::Config;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub const SKIP_DIRS: [&str; 3] = ["node_modules", "target", ".git"];

#[derive(Debug, Clone, Serialize)]
pub struct Checkout {
    pub path: PathBuf,
    pub kind: CheckoutKind,
    pub class: Class,
    pub reason: String,
    pub missing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkFault {
    pub path: PathBuf,
    pub detail: String,
}

fn record_walk(faults: &mut Vec<WalkFault>, path: &Path, err: &std::io::Error) {
    faults.push(WalkFault {
        path: path.to_path_buf(),
        detail: err.to_string(),
    });
}

fn skipped(name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(|n| SKIP_DIRS.contains(&n))
}

/// Directories at most `depth` levels below `root` that are git checkouts (`.git` file
/// or directory). A found checkout is not descended into.
pub fn git_checkouts_within(
    root: &Path,
    depth: usize,
    faults: &mut Vec<WalkFault>,
) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if depth == 0 {
        return found;
    }
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) => {
            record_walk(faults, root, &e);
            return found;
        }
    };
    let mut dirs = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                record_walk(faults, root, &e);
                continue;
            }
        };
        if skipped(&entry.file_name()) {
            continue;
        }
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => dirs.push(entry.path()),
            Ok(_) => {}
            Err(e) => record_walk(faults, &entry.path(), &e),
        }
    }
    dirs.sort();
    for dir in dirs {
        match std::fs::symlink_metadata(dir.join(".git")) {
            Ok(_) => found.push(dir),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                found.extend(git_checkouts_within(&dir, depth - 1, faults));
            }
            Err(e) => record_walk(faults, &dir.join(".git"), &e),
        }
    }
    found
}

pub fn checkouts(
    repo: &Repo,
    registered_mains: &[PathBuf],
    cfg: &Config,
) -> (Vec<Checkout>, Vec<WalkFault>) {
    let mut out: Vec<Checkout> = Vec::new();
    let mut faults: Vec<WalkFault> = Vec::new();
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    let push =
        |out: &mut Vec<Checkout>, path: PathBuf, kind: CheckoutKind, git: Result<(), String>| {
            let realpath = path.canonicalize().map_err(|e| e.to_string());
            let facts = CheckoutFacts {
                realpath: realpath.clone(),
                kind: kind.clone(),
                git,
            };
            let (class, reason) = classify(&facts, registered_mains, &cfg.secondary_roots);
            out.push(Checkout {
                path: realpath.unwrap_or(path),
                kind,
                class,
                reason,
                missing: false,
            });
        };
    match Git::at(&repo.main).worktrees() {
        Ok(records) => {
            for rec in records.into_iter().filter(|r| !r.bare) {
                let is_main = rec.path.canonicalize().ok().as_ref() == Some(&repo.main)
                    || rec.path == repo.main;
                let kind = if is_main {
                    CheckoutKind::Main
                } else {
                    CheckoutKind::Linked {
                        main: repo.main.clone(),
                    }
                };
                match std::fs::symlink_metadata(&rec.path) {
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        out.push(Checkout {
                            path: rec.path,
                            kind,
                            class: Class::Protected,
                            reason: "missing: the worktree directory does not exist".into(),
                            missing: true,
                        });
                        continue;
                    }
                    Err(e) => {
                        push(&mut out, rec.path, kind, Err(e.to_string()));
                        continue;
                    }
                    Ok(_) => {}
                }
                seen.insert(rec.path.canonicalize().unwrap_or(rec.path.clone()));
                push(&mut out, rec.path, kind, Ok(()));
            }
        }
        Err(e) => {
            seen.insert(repo.main.clone());
            push(
                &mut out,
                repo.main.clone(),
                CheckoutKind::Main,
                Err(e.to_string()),
            );
        }
    }
    let mut clones: Vec<PathBuf> = Vec::new();
    for root in &cfg.secondary_roots {
        for dir in git_checkouts_within(root, 2, &mut faults) {
            let real = match dir.canonicalize() {
                Ok(path) => path,
                Err(e) => {
                    push(&mut out, dir, CheckoutKind::Clone, Err(e.to_string()));
                    continue;
                }
            };
            if seen.contains(&real) {
                continue;
            }
            let g = Git::at(&real);
            let (git_dir, common) = match (g.git_dir(), g.common_dir()) {
                (Ok(git_dir), Ok(common)) => (git_dir, common),
                (Err(e), _) | (_, Err(e)) => {
                    seen.insert(real.clone());
                    push(&mut out, real, CheckoutKind::Clone, Err(e.to_string()));
                    continue;
                }
            };
            if git_dir != common {
                continue;
            }
            match g.superproject() {
                Ok(None) => {}
                Ok(Some(_)) => continue,
                Err(e) => {
                    seen.insert(real.clone());
                    push(&mut out, real, CheckoutKind::Clone, Err(e.to_string()));
                    continue;
                }
            }
            match g.root_commits() {
                Ok(roots) if !roots.is_disjoint(&repo.root_commits) => {
                    seen.insert(real.clone());
                    clones.push(real);
                }
                Ok(_) => {}
                Err(e) => {
                    seen.insert(real.clone());
                    push(&mut out, real, CheckoutKind::Clone, Err(e.to_string()));
                }
            }
        }
    }
    clones.sort();
    for c in clones {
        push(&mut out, c, CheckoutKind::Clone, Ok(()));
    }
    (out, faults)
}

pub const SNAPSHOT_FILE: &str = "snapshot.json";
const TARGET_SKIP: [&str; 2] = ["node_modules", ".git"];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    /// `cargo metadata` results by workspace dir; valid while every stamped file keeps its mtime.
    pub metadata: BTreeMap<PathBuf, CachedMetadata>,
    /// Profile dirs last seen with an unsupported fingerprint format, and why.
    pub unsupported_profiles: BTreeMap<PathBuf, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CachedMetadata {
    pub stamp: BTreeMap<PathBuf, Option<u64>>,
    pub resolved: Resolved,
}

fn mtime_nanos(path: &Path) -> Option<u64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    u64::try_from(modified.duration_since(UNIX_EPOCH).ok()?.as_nanos()).ok()
}

/// Files whose change can change what `cargo metadata` resolves for `workspace_dir` (Decision 4).
fn stamp_files(workspace_dir: &Path, workspace_root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in [workspace_dir, workspace_root] {
        files.push(dir.join("Cargo.toml"));
        files.push(dir.join("Cargo.lock"));
    }
    for dir in workspace_dir.ancestors() {
        for name in [
            "rust-toolchain",
            "rust-toolchain.toml",
            ".cargo/config",
            ".cargo/config.toml",
        ] {
            files.push(dir.join(name));
        }
    }
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")));
    if let Some(home) = cargo_home {
        files.push(home.join("config"));
        files.push(home.join("config.toml"));
    }
    files.sort();
    files.dedup();
    files
}

fn cached(snapshot: &Snapshot, dir: &Path) -> Option<Resolved> {
    let entry = snapshot.metadata.get(dir)?;
    let stamp: BTreeMap<_, _> = stamp_files(dir, &entry.resolved.workspace_root)
        .into_iter()
        .map(|f| (f.clone(), mtime_nanos(&f)))
        .collect();
    (stamp == entry.stamp).then(|| entry.resolved.clone())
}

/// Dirs at depth 0–2 of a checkout that hold a `Cargo.toml`, with their depth.
pub fn workspace_dirs(checkout: &Path, faults: &mut Vec<WalkFault>) -> Vec<(usize, PathBuf)> {
    let mut out = Vec::new();
    let mut level = vec![checkout.to_path_buf()];
    for depth in 0..=2 {
        let mut next = Vec::new();
        for dir in &level {
            if dir.join("Cargo.toml").is_file() {
                out.push((depth, dir.clone()));
            }
            if depth == 2 {
                continue;
            }
            let entries = match std::fs::read_dir(dir) {
                Ok(entries) => entries,
                Err(e) => {
                    record_walk(faults, dir, &e);
                    continue;
                }
            };
            let mut kids = Vec::new();
            for entry in entries {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(e) => {
                        record_walk(faults, dir, &e);
                        continue;
                    }
                };
                if skipped(&entry.file_name()) {
                    continue;
                }
                match entry.file_type() {
                    Ok(kind) if kind.is_dir() => kids.push(entry.path()),
                    Ok(_) => {}
                    Err(e) => record_walk(faults, &entry.path(), &e),
                }
            }
            kids.sort();
            next.extend(kids);
        }
        level = next;
    }
    out
}

type Found = (PathBuf, Result<Vec<(PathBuf, Layout)>, String>);

/// Cargo target dirs at most `depth` levels below `root`. A found target is not entered,
/// and neither is `node_modules` or `.git`.
pub fn find_targets(root: &Path, depth: usize, out: &mut Vec<Found>, faults: &mut Vec<WalkFault>) {
    if depth == 0 {
        return;
    }
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) => {
            record_walk(faults, root, &e);
            return;
        }
    };
    let mut dirs = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                record_walk(faults, root, &e);
                continue;
            }
        };
        if entry
            .file_name()
            .to_str()
            .is_some_and(|n| TARGET_SKIP.contains(&n))
        {
            continue;
        }
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => dirs.push(entry.path()),
            Ok(_) => {}
            Err(e) => record_walk(faults, &entry.path(), &e),
        }
    }
    dirs.sort();
    for dir in dirs {
        match cargo_target::identify(&dir) {
            Some(found) => out.push((dir, found)),
            None => find_targets(&dir, depth - 1, out, faults),
        }
    }
}

struct Resolution {
    dir: PathBuf,
    result: Result<Resolved, String>,
    cached: bool,
}

/// Shallowest dirs first; a dir inside an already resolved workspace root is not queried.
/// Uncached calls run up to 4 at a time.
fn resolve_workspaces(
    checkouts: &[PathBuf],
    snapshot: &mut Snapshot,
    faults: &mut Vec<WalkFault>,
) -> Vec<Resolution> {
    let mut by_depth: BTreeMap<usize, BTreeSet<PathBuf>> = BTreeMap::new();
    for checkout in checkouts {
        for (depth, dir) in workspace_dirs(checkout, faults) {
            by_depth.entry(depth).or_default().insert(dir);
        }
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut out = Vec::new();
    for dirs in by_depth.into_values() {
        let mut uncached = Vec::new();
        for dir in dirs {
            if roots.iter().any(|r| dir.starts_with(r)) {
                continue;
            }
            match cached(snapshot, &dir) {
                Some(r) => {
                    roots.push(r.workspace_root.clone());
                    out.push(Resolution {
                        dir,
                        result: Ok(r),
                        cached: true,
                    });
                }
                None => uncached.push(dir),
            }
        }
        for chunk in uncached.chunks(4) {
            let results: Vec<Result<Resolved, String>> = std::thread::scope(|s| {
                let handles: Vec<_> = chunk
                    .iter()
                    .map(|d| s.spawn(move || cargo_target::resolve(d)))
                    .collect();
                handles
                    .into_iter()
                    .map(|h| {
                        h.join()
                            .unwrap_or_else(|_| Err("the cargo metadata resolver panicked".into()))
                    })
                    .collect()
            });
            for (dir, result) in chunk.iter().cloned().zip(results) {
                match &result {
                    Ok(r) => {
                        roots.push(r.workspace_root.clone());
                        let stamp = stamp_files(&dir, &r.workspace_root)
                            .into_iter()
                            .map(|f| (f.clone(), mtime_nanos(&f)))
                            .collect();
                        snapshot.metadata.insert(
                            dir.clone(),
                            CachedMetadata {
                                stamp,
                                resolved: r.clone(),
                            },
                        );
                    }
                    Err(_) => {
                        snapshot.metadata.remove(&dir);
                    }
                }
                out.push(Resolution {
                    dir,
                    result,
                    cached: false,
                });
            }
        }
    }
    out
}

/// The known checkout that most deeply contains `workspace_root`.
fn owner_checkout(workspace_root: &Path, checkouts: &[PathBuf]) -> PathBuf {
    checkouts
        .iter()
        .filter(|c| workspace_root.starts_with(c))
        .max_by_key(|c| c.components().count())
        .cloned()
        .unwrap_or_else(|| workspace_root.to_path_buf())
}

pub struct TargetScan {
    pub targets: Vec<TargetDir>,
    pub metadata_calls: u64,
    pub metadata_cache_hits: u64,
    pub ignored_vars: Vec<String>,
    pub walk_faults: Vec<WalkFault>,
}

/// Every Cargo target dir of the live `checkouts` (3 levels) and the cache roots (2 levels),
/// plus every resolved build directory wherever it lies, with owner and fingerprint reports.
pub fn targets(
    checkouts: &[Checkout],
    cfg: &Config,
    all_repos: bool,
    snapshot: &mut Snapshot,
) -> TargetScan {
    let live: Vec<PathBuf> = checkouts
        .iter()
        .filter(|c| !c.missing)
        .map(|c| c.path.clone())
        .collect();
    let mut walk_faults = Vec::new();
    let resolutions = resolve_workspaces(&live, snapshot, &mut walk_faults);
    let resolved: Vec<&Resolved> = resolutions
        .iter()
        .filter_map(|r| r.result.as_ref().ok())
        .collect();
    let failure = resolutions.iter().find_map(|r| {
        r.result.as_ref().err().map(|e| {
            format!(
                "ownership unknown: cargo metadata failed in {}: {e}",
                r.dir.display()
            )
        })
    });
    let overrides = cargo_target::caller_overrides();

    let mut found: Vec<(Found, bool)> = Vec::new();
    for checkout in &live {
        let mut v = Vec::new();
        find_targets(checkout, 3, &mut v, &mut walk_faults);
        found.extend(v.into_iter().map(|f| (f, false)));
    }
    for root in &cfg.cache_roots {
        let mut v = Vec::new();
        find_targets(root, 2, &mut v, &mut walk_faults);
        found.extend(v.into_iter().map(|f| (f, true)));
    }
    for r in &resolved {
        if let Some(identified) = cargo_target::identify(&r.build_directory) {
            found.push(((r.build_directory.clone(), identified), false));
        }
    }

    let mut seen = BTreeSet::new();
    let mut entries = Vec::new();
    for ((path, identified), from_cache_root) in found {
        let real = path.canonicalize().unwrap_or(path);
        if seen.insert(real.clone()) {
            entries.push((real, identified, from_cache_root));
        }
    }
    // Profiles are read one scoped thread per target: klynt's checkouts hold thousands of
    // units, and the 2 s status budget includes reading them.
    let reports: Vec<Vec<fingerprint::ProfileReport>> = std::thread::scope(|s| {
        let handles: Vec<_> = entries
            .iter()
            .map(|(_, identified, _)| {
                s.spawn(move || match identified {
                    Ok(dirs) => dirs
                        .iter()
                        .map(|(p, _)| fingerprint::read_profile(p))
                        .collect(),
                    Err(_) => Vec::new(),
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_default())
            .collect()
    });
    let mut targets = Vec::new();
    for ((real, identified, from_cache_root), profiles) in entries.into_iter().zip(reports) {
        let matches: Vec<&Resolved> = resolved
            .iter()
            .copied()
            .filter(|r| r.target_directory == real || r.build_directory == real)
            .collect();
        let owner = if !matches.is_empty() {
            Owner::Owned {
                checkout: protected_owner(&matches, &live, checkouts),
            }
        } else {
            match (&failure, from_cache_root && !all_repos) {
                (Some(why), _) => Owner::Unresolved { reason: why.clone() },
                (None, true) => Owner::Unresolved { reason: "a cache-root target is judged against every registered repository; run `cargo hive status` without arguments".into() },
                (None, false) => Owner::OrphanCandidate { owned_under_caller_env: overrides.iter().find(|(_, p)| *p == real).map(|(name, _)| name.clone()) },
            }
        };
        for p in &profiles {
            match &p.format {
                Format::Unsupported(why) => {
                    snapshot
                        .unsupported_profiles
                        .insert(p.path.clone(), why.clone());
                }
                Format::Supported => {
                    snapshot.unsupported_profiles.remove(&p.path);
                }
            }
        }
        targets.push(TargetDir {
            path: real,
            owner,
            profiles,
            size: None,
            unreadable: identified.err(),
        });
    }
    TargetScan {
        targets,
        metadata_calls: resolutions.iter().filter(|r| !r.cached).count() as u64,
        metadata_cache_hits: resolutions.iter().filter(|r| r.cached).count() as u64,
        ignored_vars: overrides.into_iter().map(|(name, _)| name).collect(),
        walk_faults,
    }
}

/// When several workspaces resolve to the same target, a protected checkout wins.
/// A single match still uses the deepest containing checkout, even if that path is not registered.
fn protected_owner(matches: &[&Resolved], live: &[PathBuf], checkouts: &[Checkout]) -> PathBuf {
    if matches.len() > 1 {
        if let Some(path) = matches.iter().find_map(|resolved| {
            let path = owner_checkout(&resolved.workspace_root, live);
            checkouts
                .iter()
                .find(|c| !c.missing && c.class == Class::Protected && c.path == path)
                .map(|c| c.path.clone())
        }) {
            return path;
        }
    }
    owner_checkout(&matches[0].workspace_root, live)
}

fn stamp_generation(stamp: &BTreeMap<PathBuf, Option<u64>>) -> Option<u64> {
    stamp.values().copied().flatten().max()
}

/// Unions metadata and unsupported profiles. A disk metadata entry whose stamp is
/// strictly newer than the incoming one is kept.
pub fn merge_snapshot(incoming: Snapshot, disk: Snapshot) -> Snapshot {
    let mut metadata = disk.metadata;
    for (dir, incoming_entry) in incoming.metadata {
        let keep_disk = metadata.get(&dir).is_some_and(|on_disk| {
            stamp_generation(&on_disk.stamp) > stamp_generation(&incoming_entry.stamp)
        });
        if !keep_disk {
            metadata.insert(dir, incoming_entry);
        }
    }
    let mut unsupported_profiles = disk.unsupported_profiles;
    unsupported_profiles.extend(incoming.unsupported_profiles);
    Snapshot {
        metadata,
        unsupported_profiles,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cargo_target::Resolved;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    fn cached(root: &str, mtime: Option<u64>) -> CachedMetadata {
        CachedMetadata {
            stamp: BTreeMap::from([(PathBuf::from(root).join("Cargo.lock"), mtime)]),
            resolved: Resolved {
                workspace_root: PathBuf::from(root),
                target_directory: PathBuf::from(root).join("target"),
                build_directory: PathBuf::from(root).join("target"),
            },
        }
    }

    #[test]
    fn a_newer_disk_stamp_survives_and_profiles_are_unioned() {
        let incoming = Snapshot {
            metadata: BTreeMap::from([
                (PathBuf::from("/w/shared"), cached("/w/shared", Some(1))),
                (PathBuf::from("/w/incoming"), cached("/w/incoming", Some(4))),
            ]),
            unsupported_profiles: BTreeMap::from([(PathBuf::from("/p/shared"), "incoming".into())]),
        };
        let disk = Snapshot {
            metadata: BTreeMap::from([
                (PathBuf::from("/w/shared"), cached("/w/shared", Some(9))),
                (PathBuf::from("/w/disk"), cached("/w/disk", Some(2))),
            ]),
            unsupported_profiles: BTreeMap::from([
                (PathBuf::from("/p/shared"), "disk".into()),
                (PathBuf::from("/p/disk"), "disk".into()),
            ]),
        };
        let merged = merge_snapshot(incoming, disk);
        let shared = merged.metadata.get(Path::new("/w/shared")).unwrap();
        assert_eq!(super::stamp_generation(&shared.stamp), Some(9));
        assert!(merged.metadata.contains_key(Path::new("/w/disk")));
        assert!(merged.metadata.contains_key(Path::new("/w/incoming")));
        assert_eq!(
            merged
                .unsupported_profiles
                .get(Path::new("/p/shared"))
                .map(String::as_str),
            Some("incoming")
        );
        assert_eq!(
            merged
                .unsupported_profiles
                .get(Path::new("/p/disk"))
                .map(String::as_str),
            Some("disk")
        );
    }
}
