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

fn skipped(name: &std::ffi::OsStr) -> bool {
    name.to_str().is_some_and(|n| SKIP_DIRS.contains(&n))
}

/// Directories at most `depth` levels below `root` that are git checkouts (`.git` file
/// or directory). A found checkout is not descended into.
pub fn git_checkouts_within(root: &Path, depth: usize) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if depth == 0 {
        return found;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return found;
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| !skipped(&e.file_name()) && e.file_type().is_ok_and(|t| t.is_dir()))
        .map(|e| e.path())
        .collect();
    dirs.sort();
    for dir in dirs {
        if dir.join(".git").exists() {
            found.push(dir);
        } else {
            found.extend(git_checkouts_within(&dir, depth - 1));
        }
    }
    found
}

pub fn checkouts(repo: &Repo, registered_mains: &[PathBuf], cfg: &Config) -> Vec<Checkout> {
    let mut out: Vec<Checkout> = Vec::new();
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
                if !rec.path.exists() {
                    out.push(Checkout {
                        path: rec.path,
                        kind,
                        class: Class::Protected,
                        reason: "missing: the worktree directory does not exist".into(),
                        missing: true,
                    });
                    continue;
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
        for dir in git_checkouts_within(root, 2) {
            let real = dir.canonicalize().unwrap_or(dir.clone());
            if seen.contains(&real) {
                continue;
            }
            let g = Git::at(&real);
            let primary = matches!((g.git_dir(), g.common_dir()), (Ok(a), Ok(b)) if a == b);
            let not_sub = matches!(g.superproject(), Ok(None));
            let shares_root = g
                .root_commits()
                .is_ok_and(|roots| !roots.is_disjoint(&repo.root_commits));
            if primary && not_sub && shares_root {
                seen.insert(real.clone());
                clones.push(real);
            }
        }
    }
    clones.sort();
    for c in clones {
        push(&mut out, c, CheckoutKind::Clone, Ok(()));
    }
    out
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
pub fn workspace_dirs(checkout: &Path) -> Vec<(usize, PathBuf)> {
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
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            let mut kids: Vec<PathBuf> = entries
                .flatten()
                .filter(|e| !skipped(&e.file_name()) && e.file_type().is_ok_and(|t| t.is_dir()))
                .map(|e| e.path())
                .collect();
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
pub fn find_targets(root: &Path, depth: usize, out: &mut Vec<Found>) {
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_none_or(|n| !TARGET_SKIP.contains(&n))
                && e.file_type().is_ok_and(|t| t.is_dir())
        })
        .map(|e| e.path())
        .collect();
    dirs.sort();
    for dir in dirs {
        match cargo_target::identify(&dir) {
            Some(found) => out.push((dir, found)),
            None => find_targets(&dir, depth - 1, out),
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
fn resolve_workspaces(checkouts: &[PathBuf], snapshot: &mut Snapshot) -> Vec<Resolution> {
    let mut by_depth: BTreeMap<usize, BTreeSet<PathBuf>> = BTreeMap::new();
    for checkout in checkouts {
        for (depth, dir) in workspace_dirs(checkout) {
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
    let resolutions = resolve_workspaces(&live, snapshot);
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
        find_targets(checkout, 3, &mut v);
        found.extend(v.into_iter().map(|f| (f, false)));
    }
    for root in &cfg.cache_roots {
        let mut v = Vec::new();
        find_targets(root, 2, &mut v);
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
        let owner = match resolved.iter().find(|r| r.target_directory == real || r.build_directory == real) {
            Some(r) => Owner::Owned { checkout: owner_checkout(&r.workspace_root, &live) },
            None => match (&failure, from_cache_root && !all_repos) {
                (Some(why), _) => Owner::Unresolved { reason: why.clone() },
                (None, true) => Owner::Unresolved { reason: "a cache-root target is judged against every registered repository; run `cargo hive status` without arguments".into() },
                (None, false) => Owner::OrphanCandidate { owned_under_caller_env: overrides.iter().find(|(_, p)| *p == real).map(|(name, _)| name.clone()) },
            },
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
    }
}
