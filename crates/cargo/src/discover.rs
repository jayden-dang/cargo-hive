//! Checkouts of a registered repo and its Cargo target dirs. Every walk is
//! depth-bounded and never enters `node_modules`, `target` or `.git`.

use crate::git::{Git, WorktreeRecord};
use crate::metadata::{self, Resolved};
use crate::object_store;
use crate::registry::Repo;
use cargo_hive_core::classify::{CheckoutFacts, CheckoutKind, Class, classify};
use cargo_hive_core::fingerprint::{self, Format};
use cargo_hive_core::target::{self, Layout as TargetLayout, Owner, TargetDir};
use cargo_hive_state::{Config, Layout};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// The reason a cache-root target stays `Unresolved` when only some repositories are named.
#[doc(hidden)]
pub const CACHE_ROOT_UNJUDGED: &str = "a cache-root target is judged against every registered repository; run `cargo hive status` without arguments";

pub const SKIP_DIRS: [&str; 3] = ["node_modules", "target", ".git"];

pub fn unit_layout_counts(report: &fingerprint::ProfileReport) -> (u64, u64) {
    use cargo_hive_core::unit_path::UnitLayout;
    let mut flat = 0u64;
    let mut unit_dir = 0u64;
    if let Some(graph) = &report.graph {
        for u in &graph.units {
            match u.loc.layout {
                UnitLayout::Flat => flat += 1,
                UnitLayout::UnitDir => unit_dir += 1,
            }
        }
    }
    (flat, unit_dir)
}

pub fn status_layout_suffix(report: &fingerprint::ProfileReport) -> String {
    use cargo_hive_core::unit_path::UnitLayout;
    let layout = match (
        report.layouts.contains(&UnitLayout::Flat),
        report.layouts.contains(&UnitLayout::UnitDir),
    ) {
        (true, true) => " (flat + unit-dir)",
        (true, false) => " (flat)",
        (false, true) => " (unit-dir)",
        (false, false) => "",
    };
    if report.partial_unit_dirs == 0 {
        layout.to_string()
    } else {
        format!("{layout}, {} partial", report.partial_unit_dirs)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Checkout {
    pub path: PathBuf,
    pub kind: CheckoutKind,
    pub class: Class,
    pub reason: String,
    pub missing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Every target the scan found.
    All,
    /// Targets owned by, or lying inside, these checkouts (realpaths).
    Checkouts(Vec<PathBuf>),
}

impl Scope {
    pub fn from_paths(args_empty: bool, checkouts: impl Iterator<Item = PathBuf>) -> Scope {
        if args_empty {
            Scope::All
        } else {
            Scope::Checkouts(checkouts.collect())
        }
    }

    pub fn contains(&self, t: &TargetDir) -> bool {
        match self {
            Scope::All => true,
            Scope::Checkouts(checkouts) => {
                matches!(&t.owner, Owner::Owned { checkout } if checkouts.contains(checkout))
                    || checkouts.iter().any(|c| t.path.starts_with(c))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkFault {
    pub path: PathBuf,
    pub detail: String,
    /// The directory holding `path` cannot be entered by this user, so no build this
    /// user runs can work inside it.
    pub cannot_enter: bool,
}

fn record_walk(faults: &mut Vec<WalkFault>, path: &Path, err: &std::io::Error) {
    faults.push(WalkFault {
        path: path.to_path_buf(),
        detail: err.to_string(),
        cannot_enter: false,
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
    git_checkouts_within_budget(root, depth, faults, None)
}

struct WalkBudget<'a> {
    remaining: usize,
    exhausted: bool,
    cancel: &'a dyn Fn() -> bool,
}

impl<'a> WalkBudget<'a> {
    fn new(cancel: &'a dyn Fn() -> bool) -> Self {
        Self {
            remaining: cargo_hive_core::caps::CRATE_DIR_CAP,
            exhausted: false,
            cancel,
        }
    }

    fn active(&self) -> bool {
        !self.exhausted && !(self.cancel)()
    }

    fn charge(&mut self, dir: &Path, faults: &mut Vec<WalkFault>) -> bool {
        if !self.active() {
            return false;
        }
        if let Some(left) = self.remaining.checked_sub(1) {
            self.remaining = left;
            true
        } else {
            self.exhausted = true;
            faults.push(WalkFault {
                path: dir.to_path_buf(),
                detail: "strict checkout discovery entry limit exceeded".into(),
                cannot_enter: false,
            });
            false
        }
    }
}

fn directory_children(
    root: &Path,
    faults: &mut Vec<WalkFault>,
    mut budget: Option<&mut WalkBudget<'_>>,
) -> Vec<PathBuf> {
    if budget.as_ref().is_some_and(|budget| !budget.active()) {
        return Vec::new();
    }
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) => {
            record_walk(faults, root, &e);
            return Vec::new();
        }
    };
    let mut dirs = Vec::new();
    for entry in entries {
        if let Some(budget) = &mut budget
            && !budget.charge(root, faults)
        {
            break;
        }
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
    dirs
}

fn git_checkouts_within_budget(
    root: &Path,
    depth: usize,
    faults: &mut Vec<WalkFault>,
    mut budget: Option<&mut WalkBudget<'_>>,
) -> Vec<PathBuf> {
    let mut found = Vec::new();
    if depth == 0 {
        return found;
    }
    let dirs = directory_children(root, faults, budget.as_deref_mut());
    for dir in dirs {
        if budget.as_ref().is_some_and(|budget| !budget.active()) {
            break;
        }
        match std::fs::symlink_metadata(dir.join(".git")) {
            Ok(_) => found.push(dir),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                found.extend(git_checkouts_within_budget(
                    &dir,
                    depth - 1,
                    faults,
                    budget.as_deref_mut(),
                ));
            }
            // `dir` was listed, so a denied stat of a name inside it means `dir` cannot
            // be entered.
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                faults.push(WalkFault {
                    path: dir.join(".git"),
                    detail: e.to_string(),
                    cannot_enter: true,
                });
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
    let records = Git::at(&repo.main)
        .worktrees()
        .map_err(|err| err.to_string());
    checkouts_from_inventory(repo, registered_mains, cfg, records, None)
}

const GIT_DISCOVERY_BYTES: usize = 64 * 1024;

/// Uses one bounded, complete Git inventory as the source of all linked checkout rows.
pub fn checkouts_complete(
    repo: &Repo,
    registered_mains: &[PathBuf],
    cfg: &Config,
    cancel: &dyn Fn() -> bool,
) -> (Vec<Checkout>, Vec<WalkFault>) {
    let records = Git::at(&repo.main)
        .worktrees_complete_until(GIT_DISCOVERY_BYTES, cancel)
        .map_err(|err| err.to_string())
        .and_then(|records| {
            if records.first().map(|record| &record.path) == Some(&repo.main) {
                Ok(records)
            } else {
                Err("worktree inventory did not name the registered main".into())
            }
        });
    match records {
        Ok(records) => {
            let (rows, mut faults) =
                checkouts_from_inventory(repo, registered_mains, cfg, Ok(records), Some(cancel));
            faults.extend(
                rows.iter()
                    .filter(|row| row.reason.starts_with("unclassifiable:"))
                    .map(|row| WalkFault {
                        path: row.path.clone(),
                        detail: row.reason.clone(),
                        cannot_enter: false,
                    }),
            );
            (rows, faults)
        }
        Err(detail) => (
            Vec::new(),
            vec![WalkFault {
                path: repo.main.clone(),
                detail,
                cannot_enter: false,
            }],
        ),
    }
}

fn checkouts_from_inventory(
    repo: &Repo,
    registered_mains: &[PathBuf],
    cfg: &Config,
    records: Result<Vec<WorktreeRecord>, String>,
    strict_cancel: Option<&dyn Fn() -> bool>,
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
    match records {
        Ok(records) => {
            for rec in records.into_iter().filter(|r| !r.bare) {
                if strict_cancel.is_some_and(|cancel| cancel()) {
                    return (out, faults);
                }
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
                    Ok(meta) if strict_cancel.is_some() && !meta.is_dir() => {
                        push(
                            &mut out,
                            rec.path,
                            kind,
                            Err("worktree path is not a directory".into()),
                        );
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
    let mut budget = strict_cancel.map(WalkBudget::new);
    for root in &cfg.secondary_roots {
        if budget.as_ref().is_some_and(|budget| !budget.active()) {
            break;
        }
        for dir in git_checkouts_within_budget(root, 2, &mut faults, budget.as_mut()) {
            if budget.as_ref().is_some_and(|budget| !budget.active()) {
                break;
            }
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
            if strict_cancel.is_none()
                && object_store::holds_none(&real.join(".git"), &repo.root_commits)
            {
                continue;
            }
            let g = Git::at(&real);
            let related = if let Some(cancel) = strict_cancel {
                related_clone_complete(&g, repo, cancel)
            } else {
                related_clone(&g, repo)
            };
            match related {
                Ok(true) => {
                    seen.insert(real.clone());
                    clones.push(real);
                }
                Ok(false) => {}
                Err(err) => {
                    seen.insert(real.clone());
                    push(&mut out, real, CheckoutKind::Clone, Err(err));
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

fn related_clone(git: &Git, repo: &Repo) -> Result<bool, String> {
    let (dir, common) = match (git.git_dir(), git.common_dir()) {
        (Ok(dir), Ok(common)) => (dir, common),
        (Err(err), _) | (_, Err(err)) => return Err(err.to_string()),
    };
    if dir != common || git.superproject().map_err(|err| err.to_string())?.is_some() {
        return Ok(false);
    }
    git.root_commits()
        .map(|roots| !roots.is_disjoint(&repo.root_commits))
        .map_err(|err| err.to_string())
}

fn related_clone_complete(
    git: &Git,
    repo: &Repo,
    cancel: &dyn Fn() -> bool,
) -> Result<bool, String> {
    let read = |args: &[&str]| -> Result<String, String> {
        let bytes = git
            .slot_read_only_until(args, GIT_DISCOVERY_BYTES, cancel)
            .map_err(|err| err.to_string())?;
        String::from_utf8(bytes).map_err(|err| err.to_string())
    };
    let paths = read(&[
        "rev-parse",
        "--path-format=absolute",
        "--git-dir",
        "--git-common-dir",
        "--show-superproject-working-tree",
    ])?;
    let fields: Vec<_> = paths
        .strip_suffix('\n')
        .ok_or("unterminated Git path inventory")?
        .split('\n')
        .collect();
    let [dir, common, remaining @ ..] = fields.as_slice() else {
        return Err("incomplete Git path inventory".into());
    };
    if fields.iter().any(|path| !Path::new(path).is_absolute()) || remaining.len() > 1 {
        return Err("invalid Git path inventory".into());
    }
    if dir != common || !remaining.is_empty() {
        return Ok(false);
    }
    let roots = read(&["rev-list", "--max-parents=0", "HEAD"])?;
    let roots = roots
        .strip_suffix('\n')
        .ok_or("unterminated Git root inventory")?;
    let mut related = false;
    for root in roots.split('\n') {
        if !cargo_hive_state::slots::object_id(root) {
            return Err("invalid Git root identity".into());
        }
        related |= repo.root_commits.contains(root);
    }
    Ok(related)
}

pub const SNAPSHOT_FILE: &str = "snapshot.json";
const TARGET_SKIP: [&str; 2] = ["node_modules", ".git"];

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    #[serde(default = "Layout::absent")]
    pub layout: Layout,
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
    workspace_dirs_with_budget(checkout, faults, None)
}

fn workspace_dirs_with_budget(
    checkout: &Path,
    faults: &mut Vec<WalkFault>,
    mut budget: Option<&mut WalkBudget<'_>>,
) -> Vec<(usize, PathBuf)> {
    let mut out = Vec::new();
    let mut level = vec![checkout.to_path_buf()];
    for depth in 0..=2 {
        let mut next = Vec::new();
        for dir in &level {
            if budget.as_ref().is_some_and(|budget| !budget.active()) {
                return out;
            }
            let manifest = dir.join("Cargo.toml");
            let is_manifest = if budget.is_some() {
                match manifest.symlink_metadata() {
                    Ok(meta) if meta.is_file() => true,
                    Ok(_) => {
                        record_walk(
                            faults,
                            &manifest,
                            &std::io::Error::other("Cargo manifest is not a regular file"),
                        );
                        false
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
                    Err(err) => {
                        record_walk(faults, &manifest, &err);
                        false
                    }
                }
            } else {
                manifest.is_file()
            };
            if is_manifest {
                out.push((depth, dir.clone()));
            }
            if depth < 2 {
                next.extend(directory_children(dir, faults, budget.as_deref_mut()));
            }
        }
        level = next;
    }
    out
}

type Found = (PathBuf, Result<Vec<(PathBuf, TargetLayout)>, String>);

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
        match target::identify(&dir) {
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
    cancel: &(impl Fn() -> bool + Sync),
    group_workspaces: bool,
) -> Vec<Resolution> {
    let mut by_depth: BTreeMap<usize, BTreeSet<PathBuf>> = BTreeMap::new();
    let mut budget = (!group_workspaces).then(|| WalkBudget::new(cancel));
    for checkout in checkouts {
        if budget.as_ref().is_some_and(|budget| !budget.active()) {
            break;
        }
        for (depth, dir) in workspace_dirs_with_budget(checkout, faults, budget.as_mut()) {
            by_depth.entry(depth).or_default().insert(dir);
        }
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for dirs in by_depth.into_values() {
        let mut uncached = Vec::new();
        // Roots found at this depth skip only deeper dirs, whether they came from the
        // cache or from cargo metadata, so a cache hit cannot hide a nested checkout.
        let mut found_here = Vec::new();
        for dir in dirs {
            let already_discovered = if group_workspaces {
                roots.iter().any(|root| dir.starts_with(root))
            } else {
                !seen.insert(dir.clone())
            };
            if already_discovered {
                continue;
            }
            match group_workspaces.then(|| cached(snapshot, &dir)).flatten() {
                Some(r) => {
                    found_here.push(r.workspace_root.clone());
                    out.push(Resolution {
                        dir,
                        result: Ok(r),
                        cached: true,
                    });
                }
                None => uncached.push(dir),
            }
        }
        roots.extend(found_here);
        for chunk in uncached.chunks(4) {
            if cancel() {
                break;
            }
            let results: Vec<Result<Resolved, String>> = std::thread::scope(|s| {
                let handles: Vec<_> = chunk
                    .iter()
                    .map(|d| s.spawn(move || metadata::resolve(d, cancel)))
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
                if group_workspaces {
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

pub struct WorkspaceResolution {
    pub dir: PathBuf,
    pub checkout: PathBuf,
    pub result: Result<Resolved, String>,
}

pub struct TargetScan {
    pub targets: Vec<TargetDir>,
    pub metadata_calls: u64,
    pub metadata_cache_hits: u64,
    pub ignored_vars: Vec<String>,
    pub walk_faults: Vec<WalkFault>,
    /// Profile paths this run found supported; merge must drop them even if disk still lists them.
    pub cleared_profiles: BTreeSet<PathBuf>,
    pub workspaces: Vec<WorkspaceResolution>,
}

pub fn resolvers_of(scan: &TargetScan, target: &Path) -> Vec<PathBuf> {
    let real = target
        .canonicalize()
        .unwrap_or_else(|_| target.to_path_buf());
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for workspace in &scan.workspaces {
        let Ok(resolved) = &workspace.result else {
            continue;
        };
        if (resolved.target_directory == real || resolved.build_directory == real)
            && seen.insert(workspace.checkout.clone())
        {
            out.push(workspace.checkout.clone());
        }
    }
    out
}

/// Checks both configured output roles of foreign workspaces, even when no target row
/// was requested for their destination.
pub fn foreign_output_overlaps(scan: &TargetScan, checkout: &Path, root: &Path) -> bool {
    let Some(root) = OutputPath::resolve(root) else {
        return true;
    };
    scan.workspaces.iter().any(|workspace| {
        workspace.checkout != checkout
            && workspace.result.as_ref().is_ok_and(|resolved| {
                [&resolved.target_directory, &resolved.build_directory]
                    .into_iter()
                    .any(|output| {
                        OutputPath::resolve(output).is_none_or(|path| root.overlaps(&path))
                    })
            })
    })
}

struct OutputPath {
    path: PathBuf,
    existing_components: usize,
}

impl OutputPath {
    fn resolve(path: &Path) -> Option<Self> {
        use std::io::ErrorKind;
        use std::path::Component;

        if !path.is_absolute()
            || path
                .components()
                .any(|part| !matches!(part, Component::RootDir | Component::Normal(_)))
        {
            return None;
        }
        let mut missing = Vec::new();
        for ancestor in path.ancestors() {
            match ancestor.canonicalize() {
                Ok(mut canonical) => {
                    let existing_components = canonical.components().count();
                    canonical.extend(missing.into_iter().rev());
                    return Some(Self {
                        path: canonical,
                        existing_components,
                    });
                }
                Err(err) if err.kind() == ErrorKind::NotFound => {
                    // A dangling symlink is not proof of a missing path component.
                    if !std::fs::symlink_metadata(ancestor)
                        .is_err_and(|err| err.kind() == ErrorKind::NotFound)
                    {
                        return None;
                    }
                    missing.push(ancestor.file_name()?);
                }
                Err(_) => return None,
            }
        }
        None
    }

    fn overlaps(&self, other: &Self) -> bool {
        let known = self.existing_components.min(other.existing_components);
        self.path
            .components()
            .zip(other.path.components())
            .enumerate()
            .all(|(i, (left, right))| {
                if left == right {
                    return true;
                }
                if i < known {
                    return false;
                }
                let left = left.as_os_str().as_encoded_bytes();
                let right = right.as_os_str().as_encoded_bytes();
                // Without an inode, non-ASCII equivalence under this volume is unproved.
                !left.is_ascii() || !right.is_ascii() || left.eq_ignore_ascii_case(right)
            })
    }
}

pub fn failed_workspaces(scan: &TargetScan) -> Vec<(PathBuf, String)> {
    scan.workspaces
        .iter()
        .filter_map(|workspace| {
            workspace
                .result
                .as_ref()
                .err()
                .map(|err| (workspace.dir.clone(), err.clone()))
        })
        .collect()
}

/// Every Cargo target dir of the live `checkouts` (3 levels) and the cache roots (2 levels),
/// plus every resolved build directory wherever it lies, with owner and fingerprint reports.
pub fn targets(
    checkouts: &[Checkout],
    cfg: &Config,
    all_repos: bool,
    snapshot: &mut Snapshot,
    cancel: impl Fn() -> bool + Sync,
) -> TargetScan {
    let live: Vec<PathBuf> = checkouts
        .iter()
        .filter(|c| !c.missing)
        .map(|c| c.path.clone())
        .collect();
    let mut walk_faults = Vec::new();
    let resolutions = resolve_workspaces(&live, snapshot, &mut walk_faults, &cancel, true);
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
    let overrides = metadata::caller_overrides();

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
        if let Some(identified) = target::identify(&r.build_directory) {
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
    let mut cleared_profiles = BTreeSet::new();
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
                (Some(why), _) => Owner::Unresolved {
                    reason: why.clone(),
                },
                (None, true) => Owner::Unresolved {
                    reason: CACHE_ROOT_UNJUDGED.into(),
                },
                (None, false) => Owner::OrphanCandidate {
                    owned_under_caller_env: overrides
                        .iter()
                        .find(|(_, p)| *p == real)
                        .map(|(name, _)| name.clone()),
                },
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
                    cleared_profiles.insert(p.path.clone());
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
    let workspaces = resolutions
        .iter()
        .map(|r| WorkspaceResolution {
            dir: r.dir.clone(),
            checkout: owner_checkout(&r.dir, &live),
            result: r.result.clone(),
        })
        .collect();
    TargetScan {
        targets,
        metadata_calls: resolutions.iter().filter(|r| !r.cached).count() as u64,
        metadata_cache_hits: resolutions.iter().filter(|r| r.cached).count() as u64,
        ignored_vars: overrides.into_iter().map(|(name, _)| name).collect(),
        walk_faults,
        cleared_profiles,
        workspaces,
    }
}

/// Resolves every admitted manifest independently and reports ownership only for requested paths.
pub fn targets_for_checkout_gc(
    checkouts: &[Checkout],
    required_paths: &[PathBuf],
    cancel: impl Fn() -> bool + Sync,
) -> TargetScan {
    let live: Vec<_> = checkouts
        .iter()
        .filter(|row| !row.missing)
        .map(|row| row.path.clone())
        .collect();
    let mut walk_faults = Vec::new();
    let resolutions = resolve_workspaces(
        &live,
        &mut Snapshot::default(),
        &mut walk_faults,
        &cancel,
        false,
    );
    let targets = required_paths
        .iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|path| {
            let matching: Vec<_> = resolutions
                .iter()
                .filter_map(|resolution| resolution.result.as_ref().ok())
                .filter(|resolved| {
                    resolved.target_directory == *path || resolved.build_directory == *path
                })
                .collect();
            (!matching.is_empty()).then(|| TargetDir {
                path: path.clone(),
                owner: Owner::Owned {
                    checkout: protected_owner(&matching, &live, checkouts),
                },
                profiles: Vec::new(),
                size: None,
                unreadable: None,
            })
        })
        .collect();
    let metadata_calls = resolutions.len() as u64;
    let workspaces = resolutions
        .into_iter()
        .map(|resolution| WorkspaceResolution {
            checkout: owner_checkout(&resolution.dir, &live),
            dir: resolution.dir,
            result: resolution.result,
        })
        .collect();
    TargetScan {
        targets,
        metadata_calls,
        metadata_cache_hits: 0,
        ignored_vars: Vec::new(),
        walk_faults,
        cleared_profiles: BTreeSet::new(),
        workspaces,
    }
}

/// When several workspaces resolve to the same target, a protected checkout wins.
/// A single match still uses the deepest containing checkout, even if that path is not registered.
fn protected_owner(matches: &[&Resolved], live: &[PathBuf], checkouts: &[Checkout]) -> PathBuf {
    if matches.len() > 1
        && let Some(path) = matches.iter().find_map(|resolved| {
            let path = owner_checkout(&resolved.workspace_root, live);
            checkouts
                .iter()
                .find(|c| !c.missing && c.class == Class::Protected && c.path == path)
                .map(|c| c.path.clone())
        })
    {
        return path;
    }
    owner_checkout(&matches[0].workspace_root, live)
}

fn stamp_path_cmp(left: Option<&Option<u64>>, right: Option<&Option<u64>>) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (left, right) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Less,
        (Some(_), None) => Ordering::Greater,
        (Some(None), Some(None)) => Ordering::Equal,
        (Some(None), Some(Some(_))) => Ordering::Less,
        (Some(Some(_)), Some(None)) => Ordering::Greater,
        (Some(Some(a)), Some(Some(b))) => a.cmp(b),
    }
}

/// True when `left` is not equal to `right`, every path compares left ≥ right, and at least one path is left > right.
fn stamp_dominates(
    left: &BTreeMap<PathBuf, Option<u64>>,
    right: &BTreeMap<PathBuf, Option<u64>>,
) -> bool {
    use std::cmp::Ordering;
    let mut any_greater = false;
    let mut keys = BTreeSet::new();
    keys.extend(left.keys().cloned());
    keys.extend(right.keys().cloned());
    for key in keys {
        match stamp_path_cmp(left.get(&key), right.get(&key)) {
            Ordering::Less => return false,
            Ordering::Equal => {}
            Ordering::Greater => any_greater = true,
        }
    }
    any_greater
}

/// Merges metadata by stamp dominance and unsupported profiles by incoming-over-disk,
/// then drops every profile path this run found supported.
pub fn merge_snapshot(
    incoming: Snapshot,
    disk: Snapshot,
    cleared_profiles: &BTreeSet<PathBuf>,
) -> Snapshot {
    let mut metadata = BTreeMap::new();
    let mut dirs = BTreeSet::new();
    dirs.extend(incoming.metadata.keys().cloned());
    dirs.extend(disk.metadata.keys().cloned());
    for dir in dirs {
        match (incoming.metadata.get(&dir), disk.metadata.get(&dir)) {
            (Some(inc), Some(on_disk)) => {
                if stamp_dominates(&inc.stamp, &on_disk.stamp) {
                    metadata.insert(dir, inc.clone());
                } else if stamp_dominates(&on_disk.stamp, &inc.stamp) || inc.stamp == on_disk.stamp
                {
                    metadata.insert(dir, on_disk.clone());
                }
            }
            (Some(inc), None) => {
                metadata.insert(dir, inc.clone());
            }
            (None, Some(on_disk)) => {
                metadata.insert(dir, on_disk.clone());
            }
            (None, None) => {}
        }
    }
    let mut unsupported_profiles = disk.unsupported_profiles;
    unsupported_profiles.extend(incoming.unsupported_profiles);
    for path in cleared_profiles {
        unsupported_profiles.remove(path);
    }
    Snapshot {
        layout: Layout::NEWEST,
        metadata,
        unsupported_profiles,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::Resolved;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    fn cached_with(root: &str, stamp: BTreeMap<PathBuf, Option<u64>>) -> CachedMetadata {
        CachedMetadata {
            stamp,
            resolved: Resolved {
                workspace_root: PathBuf::from(root),
                target_directory: PathBuf::from(root).join("target"),
                build_directory: PathBuf::from(root).join("target"),
            },
        }
    }

    fn cached(root: &str, mtime: Option<u64>) -> CachedMetadata {
        cached_with(
            root,
            BTreeMap::from([(PathBuf::from(root).join("Cargo.lock"), mtime)]),
        )
    }

    #[test]
    fn a_dominating_disk_stamp_survives_and_a_cleared_profile_stays_gone() {
        let incoming = Snapshot {
            layout: Layout::NEWEST,
            metadata: BTreeMap::from([
                (PathBuf::from("/w/shared"), cached("/w/shared", Some(1))),
                (PathBuf::from("/w/incoming"), cached("/w/incoming", Some(4))),
            ]),
            unsupported_profiles: BTreeMap::from([(PathBuf::from("/p/shared"), "incoming".into())]),
        };
        let disk = Snapshot {
            layout: Layout::NEWEST,
            metadata: BTreeMap::from([
                (PathBuf::from("/w/shared"), cached("/w/shared", Some(9))),
                (PathBuf::from("/w/disk"), cached("/w/disk", Some(2))),
            ]),
            unsupported_profiles: BTreeMap::from([
                (PathBuf::from("/p/shared"), "disk".into()),
                (PathBuf::from("/p/disk"), "disk".into()),
                (PathBuf::from("/p/cleared"), "stale".into()),
            ]),
        };
        let cleared = BTreeSet::from([PathBuf::from("/p/cleared")]);
        let merged = merge_snapshot(incoming, disk, &cleared);
        let shared = merged.metadata.get(Path::new("/w/shared")).unwrap();
        assert_eq!(
            shared
                .stamp
                .get(&PathBuf::from("/w/shared/Cargo.lock"))
                .copied(),
            Some(Some(9))
        );
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
        assert!(
            !merged
                .unsupported_profiles
                .contains_key(Path::new("/p/cleared"))
        );
    }

    #[test]
    fn an_incoming_stamp_that_dominates_disk_is_stored() {
        let lock = PathBuf::from("/w/a/Cargo.lock");
        let toml = PathBuf::from("/w/a/Cargo.toml");
        let incoming = Snapshot {
            layout: Layout::NEWEST,
            metadata: BTreeMap::from([(
                PathBuf::from("/w/a"),
                cached_with(
                    "/w/a",
                    BTreeMap::from([(lock.clone(), Some(5)), (toml.clone(), Some(3))]),
                ),
            )]),
            unsupported_profiles: BTreeMap::new(),
        };
        let disk = Snapshot {
            layout: Layout::NEWEST,
            metadata: BTreeMap::from([(
                PathBuf::from("/w/a"),
                cached_with(
                    "/w/a",
                    BTreeMap::from([(lock.clone(), Some(4)), (toml.clone(), Some(3))]),
                ),
            )]),
            unsupported_profiles: BTreeMap::new(),
        };
        let merged = merge_snapshot(incoming, disk, &BTreeSet::new());
        let entry = merged.metadata.get(Path::new("/w/a")).unwrap();
        assert_eq!(entry.stamp.get(&lock).copied(), Some(Some(5)));
    }

    #[test]
    fn equal_stamps_keep_the_disk_entry() {
        let incoming = Snapshot {
            layout: Layout::NEWEST,
            metadata: BTreeMap::from([(PathBuf::from("/w/a"), cached("/w/a", Some(3)))]),
            unsupported_profiles: BTreeMap::new(),
        };
        let disk = Snapshot {
            layout: Layout::NEWEST,
            metadata: BTreeMap::from([(
                PathBuf::from("/w/a"),
                CachedMetadata {
                    stamp: BTreeMap::from([(PathBuf::from("/w/a/Cargo.lock"), Some(3))]),
                    resolved: Resolved {
                        workspace_root: PathBuf::from("/w/a-disk"),
                        target_directory: PathBuf::from("/w/a-disk/target"),
                        build_directory: PathBuf::from("/w/a-disk/target"),
                    },
                },
            )]),
            unsupported_profiles: BTreeMap::new(),
        };
        let merged = merge_snapshot(incoming, disk, &BTreeSet::new());
        assert_eq!(
            merged
                .metadata
                .get(Path::new("/w/a"))
                .unwrap()
                .resolved
                .workspace_root,
            PathBuf::from("/w/a-disk")
        );
    }

    #[test]
    fn incomparable_stamps_drop_the_workspace_key() {
        let lock = PathBuf::from("/w/a/Cargo.lock");
        let toml = PathBuf::from("/w/a/Cargo.toml");
        let incoming = Snapshot {
            layout: Layout::NEWEST,
            metadata: BTreeMap::from([(
                PathBuf::from("/w/a"),
                cached_with(
                    "/w/a",
                    BTreeMap::from([(lock.clone(), Some(9)), (toml.clone(), Some(1))]),
                ),
            )]),
            unsupported_profiles: BTreeMap::new(),
        };
        let disk = Snapshot {
            layout: Layout::NEWEST,
            metadata: BTreeMap::from([(
                PathBuf::from("/w/a"),
                cached_with(
                    "/w/a",
                    BTreeMap::from([(lock.clone(), Some(2)), (toml.clone(), Some(8))]),
                ),
            )]),
            unsupported_profiles: BTreeMap::new(),
        };
        let merged = merge_snapshot(incoming, disk, &BTreeSet::new());
        assert!(!merged.metadata.contains_key(Path::new("/w/a")));
    }

    fn target_at(path: &str, owner: Owner) -> TargetDir {
        TargetDir {
            path: PathBuf::from(path),
            owner,
            profiles: Vec::new(),
            size: None,
            unreadable: None,
        }
    }

    #[test]
    fn a_scope_without_arguments_contains_every_target() {
        let scope = Scope::from_paths(true, [PathBuf::from("/repo/unused")].into_iter());
        let owned = target_at(
            "/cache/owned",
            Owner::Owned {
                checkout: PathBuf::from("/repo/other"),
            },
        );
        let orphan = target_at(
            "/cache/orphan",
            Owner::OrphanCandidate {
                owned_under_caller_env: None,
            },
        );
        let unresolved = target_at(
            "/somewhere/unresolved",
            Owner::Unresolved {
                reason: "no checkout".into(),
            },
        );
        assert!(scope.contains(&owned));
        assert!(scope.contains(&orphan));
        assert!(scope.contains(&unresolved));
    }

    #[test]
    fn a_checkout_scope_contains_targets_owned_by_or_inside_its_checkouts() {
        let listed = PathBuf::from("/repo/listed");
        let scope = Scope::from_paths(false, [listed.clone()].into_iter());
        let owned_by_listed = target_at(
            "/cache/listed-target",
            Owner::Owned {
                checkout: listed.clone(),
            },
        );
        let inside_but_owned_elsewhere = target_at(
            "/repo/listed/target",
            Owner::Owned {
                checkout: PathBuf::from("/repo/other"),
            },
        );
        let orphan_under_cache = target_at(
            "/cache/root/target",
            Owner::OrphanCandidate {
                owned_under_caller_env: Some("CARGO_TARGET_DIR".into()),
            },
        );
        assert!(scope.contains(&owned_by_listed));
        assert!(scope.contains(&inside_but_owned_elsewhere));
        assert!(!scope.contains(&orphan_under_cache));
    }

    #[test]
    fn writes_layout_as_the_first_key() {
        let cases = [
            Snapshot::default(),
            Snapshot {
                layout: Layout::NEWEST,
                metadata: BTreeMap::from([(PathBuf::from("/w/a"), cached("/w/a", Some(1)))]),
                unsupported_profiles: BTreeMap::new(),
            },
        ];
        for value in cases {
            let pretty = serde_json::to_string_pretty(&value).unwrap();
            assert!(pretty.starts_with("{\n  \"layout\": 2,"), "{pretty}");
        }
    }

    #[test]
    fn a_keyless_0_1_0_file_reads_and_its_first_write_only_adds_the_layout_line() {
        // Hand-copied 0.1.0 snapshot.json shape (empty maps).
        let body = "{\n  \"metadata\": {},\n  \"unsupported_profiles\": {}\n}";
        let parsed: Snapshot = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.layout, Layout::absent());
        assert!(parsed.metadata.is_empty());
        assert!(parsed.unsupported_profiles.is_empty());
        let written = serde_json::to_string_pretty(&parsed).unwrap();
        assert_eq!(
            written,
            "{\n  \"layout\": 1,\n  \"metadata\": {},\n  \"unsupported_profiles\": {}\n}"
        );
    }

    #[test]
    fn an_unknown_key_other_than_layout_is_still_refused() {
        let cases = [
            r#"{"mystery":true,"metadata":{},"unsupported_profiles":{}}"#,
            r#"{"layout":1,"mystery":true,"metadata":{},"unsupported_profiles":{}}"#,
        ];
        for body in cases {
            assert!(serde_json::from_str::<Snapshot>(body).is_err(), "{body}");
        }
        assert!(
            serde_json::from_str::<Snapshot>(
                r#"{"layout":1,"metadata":{},"unsupported_profiles":{}}"#
            )
            .is_ok()
        );
    }

    fn resolved(root: &str, target: &str, build: &str) -> Resolved {
        Resolved {
            workspace_root: PathBuf::from(root),
            target_directory: PathBuf::from(target),
            build_directory: PathBuf::from(build),
        }
    }

    fn scan_with(workspaces: Vec<WorkspaceResolution>, targets: Vec<TargetDir>) -> TargetScan {
        TargetScan {
            targets,
            metadata_calls: 0,
            metadata_cache_hits: 0,
            ignored_vars: Vec::new(),
            walk_faults: Vec::new(),
            cleared_profiles: BTreeSet::new(),
            workspaces,
        }
    }

    #[test]
    fn resolvers_of_names_every_checkout_that_resolves_to_a_target() {
        let shared = PathBuf::from("/shared/target");
        let main = PathBuf::from("/repo/main");
        let worktree = PathBuf::from("/repo/worktree");
        let other = PathBuf::from("/repo/other");
        let scan = scan_with(
            vec![
                WorkspaceResolution {
                    dir: main.clone(),
                    checkout: main.clone(),
                    result: Ok(resolved("/repo/main", "/shared/target", "/shared/target")),
                },
                WorkspaceResolution {
                    dir: worktree.clone(),
                    checkout: worktree.clone(),
                    result: Ok(resolved(
                        "/repo/worktree",
                        "/shared/target",
                        "/shared/build",
                    )),
                },
                WorkspaceResolution {
                    dir: other.clone(),
                    checkout: other.clone(),
                    result: Ok(resolved(
                        "/repo/other",
                        "/repo/other/target",
                        "/repo/other/target",
                    )),
                },
                WorkspaceResolution {
                    dir: main.join("crate"),
                    checkout: main.clone(),
                    result: Ok(resolved("/repo/main", "/shared/target", "/shared/target")),
                },
            ],
            Vec::new(),
        );
        assert_eq!(resolvers_of(&scan, &shared), vec![main, worktree]);
        assert_eq!(
            resolvers_of(&scan, Path::new("/shared/build")),
            vec![PathBuf::from("/repo/worktree")]
        );
        assert!(resolvers_of(&scan, Path::new("/nobody")).is_empty());
    }

    #[test]
    fn foreign_output_overlap_checks_both_roles_without_discovered_target_rows() {
        let lane = Path::new("/repo/lane");
        let root = lane.join("target");
        let own = || WorkspaceResolution {
            dir: lane.to_path_buf(),
            checkout: lane.to_path_buf(),
            result: Ok(resolved(
                "/repo/lane",
                "/repo/lane/target",
                "/repo/lane/target",
            )),
        };
        for (target, build, overlaps) in [
            ("/other/target", "/repo/lane/target/debug/foreign", true),
            ("/repo/lane", "/other/build", true),
            ("/repo/lane/target-other", "/other/build", false),
        ] {
            let foreign = WorkspaceResolution {
                dir: "/other".into(),
                checkout: "/other".into(),
                result: Ok(resolved("/other", target, build)),
            };
            let scan = scan_with(vec![own(), foreign], Vec::new());
            assert_eq!(resolvers_of(&scan, &root), [lane]);
            assert_eq!(super::foreign_output_overlaps(&scan, lane, &root), overlaps);
        }
    }

    #[test]
    fn resolvers_of_matches_through_a_symlinked_target_path() {
        let root = PathBuf::from(format!(
            "/private/tmp/hive-it-resolvers-symlink-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("real")).unwrap();
        let real_target = root.join("real").join("target");
        std::fs::create_dir_all(&real_target).unwrap();
        let link = root.join("link-target");
        std::os::unix::fs::symlink(&real_target, &link).unwrap();
        let canonical = real_target.canonicalize().unwrap();
        let checkout = PathBuf::from("/repo/main");
        let scan = scan_with(
            vec![WorkspaceResolution {
                dir: checkout.clone(),
                checkout: checkout.clone(),
                result: Ok(Resolved {
                    workspace_root: checkout.clone(),
                    target_directory: canonical.clone(),
                    build_directory: canonical.clone(),
                }),
            }],
            Vec::new(),
        );
        assert_eq!(resolvers_of(&scan, &link), vec![checkout]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn failed_workspaces_lists_every_failure_not_only_the_first() {
        let scan = scan_with(
            vec![
                WorkspaceResolution {
                    dir: PathBuf::from("/repo/ok"),
                    checkout: PathBuf::from("/repo/ok"),
                    result: Ok(resolved("/repo/ok", "/repo/ok/target", "/repo/ok/target")),
                },
                WorkspaceResolution {
                    dir: PathBuf::from("/repo/a"),
                    checkout: PathBuf::from("/repo/a"),
                    result: Err("first boom".into()),
                },
                WorkspaceResolution {
                    dir: PathBuf::from("/repo/b"),
                    checkout: PathBuf::from("/repo/b"),
                    result: Err("second boom".into()),
                },
            ],
            Vec::new(),
        );
        assert_eq!(
            failed_workspaces(&scan),
            vec![
                (PathBuf::from("/repo/a"), "first boom".into()),
                (PathBuf::from("/repo/b"), "second boom".into()),
            ]
        );
    }

    #[test]
    fn existing_owner_and_failure_are_unchanged() {
        let checkout = PathBuf::from("/repo/main");
        let first_failure =
            "ownership unknown: cargo metadata failed in /repo/broken: boom".to_string();
        let owned = target_at(
            "/repo/main/target",
            Owner::Owned {
                checkout: checkout.clone(),
            },
        );
        let unresolved = target_at(
            "/cache/elsewhere",
            Owner::Unresolved {
                reason: first_failure.clone(),
            },
        );
        let scan = scan_with(
            vec![
                WorkspaceResolution {
                    dir: checkout.clone(),
                    checkout: checkout.clone(),
                    result: Ok(resolved(
                        "/repo/main",
                        "/repo/main/target",
                        "/repo/main/target",
                    )),
                },
                WorkspaceResolution {
                    dir: PathBuf::from("/repo/broken"),
                    checkout: PathBuf::from("/repo/broken"),
                    result: Err("boom".into()),
                },
                WorkspaceResolution {
                    dir: PathBuf::from("/repo/also-broken"),
                    checkout: PathBuf::from("/repo/also-broken"),
                    result: Err("also boom".into()),
                },
            ],
            vec![owned.clone(), unresolved.clone()],
        );
        assert_eq!(scan.targets[0].owner, owned.owner);
        assert_eq!(scan.targets[1].owner, unresolved.owner);
        match &scan.targets[1].owner {
            Owner::Unresolved { reason } => assert_eq!(reason, &first_failure),
            other => panic!("expected Unresolved, got {other:?}"),
        }
        let scope = Scope::Checkouts(vec![checkout]);
        assert!(scope.contains(&scan.targets[0]));
        assert!(!scope.contains(&scan.targets[1]));
        assert_eq!(failed_workspaces(&scan).len(), 2);
    }
}
