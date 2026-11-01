//! Applies one stored plan under the target's profile locks.
//! A refusal is a result, not an error. Locks stay held from the first acquisition
//! through the last delete, and a path is removed only after its entry still matches.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::cargo_target::{self, Owner, TargetDir};
use crate::cli::cross_check_notes;
use crate::discover::{self, Snapshot};
use crate::fingerprint::read_profile;
use crate::gc::{self, Ledger};
use crate::liveness::{self, Liveness};
use crate::plan_tier::build::{class_of_target, TargetClass};
use crate::plan_tier::guard::{check_entry, Expect};
use crate::plan_tier::locks::{self, Acquired, Locked, Reason};
use crate::plan_tier::record::{
    self, kind_of, BytesRec, Lookup, Plan, PlanClass, PlannedEntry, PlannedTarget, Taken,
};
use crate::plan_tier::Ctx;
use crate::platform::Fs;
use crate::receipt::{Outcome, Unmeasured};
use crate::state::StateError;

#[derive(Debug)]
pub enum ApplyError {
    State(StateError),
    Platform(String),
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApplyError::State(err) => write!(f, "{err}"),
            ApplyError::Platform(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for ApplyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            ApplyError::State(err) => Some(err),
            ApplyError::Platform(_) => None,
        }
    }
}

#[derive(Debug)]
pub enum Authorization {
    Secondary,
}

#[derive(Debug)]
pub struct ResolvedTarget {
    pub path: PathBuf,
    pub owner: Option<PathBuf>,
    pub class: PlanClass,
}

#[derive(Debug)]
pub struct Applied {
    pub outcome: Outcome,
    pub reason: Option<String>,
    pub class: PlanClass,
    pub id: String,
    pub targets: Vec<PathBuf>,
    pub counts: BTreeMap<String, u64>,
    pub bytes: Option<BytesRec>,
    pub notes: Vec<String>,
    pub unmeasured: Vec<Unmeasured>,
    pub findings: Vec<(String, Option<PathBuf>, String)>,
}

pub fn apply(
    ctx: &Ctx,
    id: &str,
    auth: Authorization,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Applied, ApplyError> {
    match auth {
        Authorization::Secondary => {}
    }
    let plan = match record::get(ctx.state, id).map_err(ApplyError::State)? {
        Lookup::Unknown => {
            return Ok(refused(
                id,
                PlanClass::Secondary,
                "unknown_plan",
                Vec::new(),
            ))
        }
        Lookup::Consumed => {
            return Ok(refused(
                id,
                PlanClass::Secondary,
                "already_consumed",
                Vec::new(),
            ))
        }
        Lookup::Live(plan) => plan,
    };
    if plan.class != PlanClass::Secondary {
        return Ok(refused(
            &plan.id,
            plan.class,
            "unsupported_class",
            target_paths(&plan),
        ));
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let plan =
        match record::consume(ctx.state, &|| cancel(), &plan.id, now).map_err(ApplyError::State)? {
            Taken::Plan(plan) => plan,
            Taken::AlreadyConsumed => {
                return Ok(refused(
                    id,
                    PlanClass::Secondary,
                    "already_consumed",
                    Vec::new(),
                ))
            }
            Taken::Unknown => {
                return Ok(refused(
                    id,
                    PlanClass::Secondary,
                    "unknown_plan",
                    Vec::new(),
                ))
            }
        };
    let resolved = match resolve(ctx, &plan, cancel) {
        Ok(resolved) => resolved,
        Err(reason) => return Ok(refused(&plan.id, plan.class, reason, target_paths(&plan))),
    };
    Ok(execute(ctx, plan, &resolved, cancel))
}

#[doc(hidden)]
pub fn execute(
    ctx: &Ctx,
    plan: Plan,
    resolved: &[ResolvedTarget],
    cancel: &(dyn Fn() -> bool + Sync),
) -> Applied {
    let _ = cancel;
    let mut order: Vec<&PlannedTarget> = plan.targets.iter().collect();
    order.sort_by(|left, right| left.path.cmp(&right.path));
    let mut skips = skip_counts();
    let mut locked: Vec<(&PlannedTarget, Locked)> = Vec::new();
    for target in &order {
        match locks::acquire(ctx.fs, &target.path, lists_examples(target)) {
            Acquired::Locked(held) => locked.push((*target, held)),
            Acquired::Skipped(reason) => {
                if let Some(count) = skips.get_mut(reason.count_name()) {
                    *count += 1;
                }
            }
        }
    }
    if !recheck(ctx, &plan, resolved, &locked) {
        return Applied {
            outcome: Outcome::Refused,
            reason: Some("stale_item".to_string()),
            class: plan.class,
            id: plan.id.clone(),
            targets: target_paths(&plan),
            counts: count_map(0, 0, &skips, None),
            bytes: None,
            notes: Vec::new(),
            unmeasured: Vec::new(),
            findings: Vec::new(),
        };
    }
    let locked_paths: Vec<PathBuf> = locked
        .iter()
        .map(|(target, _)| target.path.clone())
        .collect();
    let before = volumes(ctx.fs, &locked_paths);
    let mut ledger = Ledger::new();
    let mut paths_deleted = 0u64;
    let mut applied_targets = 0u64;
    let mut failed = false;
    let mut findings = Vec::new();
    let mut touched = BTreeSet::new();
    for (target, _) in &locked {
        let mut deleted_here = 0u64;
        let mut target_failed = false;
        for entry in removal_order(&target.entries) {
            match delete_entry(ctx, &mut ledger, &target.path, entry) {
                Deleted::Removed => {
                    paths_deleted += 1;
                    deleted_here += 1;
                }
                Deleted::Absent => {}
                Deleted::Failed(why) => {
                    findings.push((
                        "plan.delete_failed".to_string(),
                        Some(entry.path.clone()),
                        why,
                    ));
                    target_failed = true;
                    failed = true;
                    break;
                }
            }
        }
        if !target_failed {
            applied_targets += 1;
        }
        if deleted_here > 0 {
            if let Ok(volume) = ctx.fs.volume(&target.path) {
                touched.insert(volume.container);
            }
        }
    }
    let after = volumes(ctx.fs, &locked_paths);
    let notes = cross_check_notes(&before, &after, &touched);
    let tally = ledger.finish();
    let bytes = ctx.account_bytes.then_some(BytesRec {
        freed: tally.total.freed,
        still_shared: tally.total.still_shared,
    });
    let mut unmeasured = if ctx.account_bytes {
        Vec::new()
    } else {
        unmeasured_accounting()
    };
    for (path, why) in tally.unmeasured {
        unmeasured.push(Unmeasured {
            field: path.display().to_string(),
            reason: why,
        });
    }
    if tally.unmeasured_more > 0 {
        unmeasured.push(Unmeasured {
            field: "unmeasured_paths".to_string(),
            reason: format!(
                "{} more paths could not be measured and are not listed",
                tally.unmeasured_more
            ),
        });
    }
    let total = plan.targets.len() as u64;
    let (outcome, reason) = if failed || applied_targets != total {
        let reason = if !failed && applied_targets == 0 {
            Some("all_targets_skipped".to_string())
        } else if !failed {
            Some("skipped".to_string())
        } else {
            None
        };
        (Outcome::Partial, reason)
    } else {
        (Outcome::Applied, None)
    };
    Applied {
        outcome,
        reason,
        class: plan.class,
        id: plan.id.clone(),
        targets: target_paths(&plan),
        counts: count_map(paths_deleted, applied_targets, &skips, bytes),
        bytes,
        notes,
        unmeasured,
        findings,
    }
}

fn resolve(
    ctx: &Ctx,
    plan: &Plan,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Vec<ResolvedTarget>, &'static str> {
    let mut checkouts = Vec::new();
    for repo in ctx.repos {
        let (rows, _) = discover::checkouts(repo, ctx.mains, ctx.cfg);
        checkouts.extend(rows);
    }
    let mut snapshot = Snapshot::default();
    let scan = discover::targets(&checkouts, ctx.cfg, true, &mut snapshot, cancel);
    let mut resolved = Vec::new();
    for target in &plan.targets {
        let Some(found) = scan.targets.iter().find(|cand| cand.path == target.path) else {
            return Err("owner_changed");
        };
        let class = match class_of_target(&found.owner, &checkouts) {
            TargetClass::Class(class) => class,
            TargetClass::Unresolved => return Err("owner_changed"),
        };
        if class != plan.class {
            return Err("owner_changed");
        }
        let owner = match &found.owner {
            Owner::Owned { checkout } => Some(checkout.clone()),
            Owner::Unresolved { .. } | Owner::OrphanCandidate { .. } => None,
        };
        if owner != target.owner {
            return Err("owner_changed");
        }
        resolved.push(ResolvedTarget {
            path: found.path.clone(),
            owner,
            class,
        });
    }
    Ok(resolved)
}

fn recheck(
    ctx: &Ctx,
    plan: &Plan,
    resolved: &[ResolvedTarget],
    locked: &[(&PlannedTarget, Locked)],
) -> bool {
    let window = Duration::from_secs(plan.window_days.saturating_mul(86_400));
    let now = SystemTime::now();
    for (target, _) in locked {
        let Some(Ok(profiles)) = cargo_target::identify(&target.path) else {
            return false;
        };
        let reports: Vec<_> = profiles
            .iter()
            .map(|(path, _)| read_profile(path))
            .collect();
        let owner = match resolved
            .iter()
            .find(|item| item.path == target.path)
            .and_then(|item| item.owner.clone())
            .or_else(|| target.owner.clone())
        {
            Some(checkout) => Owner::Owned { checkout },
            None => Owner::OrphanCandidate {
                owned_under_caller_env: None,
            },
        };
        let dir = TargetDir {
            path: target.path.clone(),
            owner,
            profiles: reports,
            size: None,
            unreadable: None,
        };
        let dead = match liveness::liveness(ctx.fs, &dir, now, window) {
            Liveness::Decided { dead, .. } => dead,
            Liveness::Undecided { .. } => return false,
        };
        let dead_paths: HashSet<&Path> = dead
            .iter()
            .flat_map(|unit| unit.files.iter().map(PathBuf::as_path))
            .collect();
        for entry in &target.entries {
            if !dead_paths.contains(entry.path.as_path()) {
                return false;
            }
            let Some(kind) = kind_of(&entry.kind) else {
                return false;
            };
            if check_entry(
                ctx.fs,
                &target.path,
                &entry.path,
                Expect::Entry {
                    kind,
                    dev: entry.dev,
                    ino: entry.ino,
                },
            )
            .is_err()
            {
                return false;
            }
        }
    }
    true
}

enum Deleted {
    Removed,
    Absent,
    Failed(String),
}

fn delete_entry(ctx: &Ctx, ledger: &mut Ledger, target: &Path, entry: &PlannedEntry) -> Deleted {
    let Some(kind) = kind_of(&entry.kind) else {
        return Deleted::Failed(format!(
            "{} has an unknown kind in the plan",
            entry.path.display()
        ));
    };
    if let Err(reason) = check_entry(
        ctx.fs,
        target,
        &entry.path,
        Expect::Entry {
            kind,
            dev: entry.dev,
            ino: entry.ino,
        },
    ) {
        return Deleted::Failed(format!(
            "{} was not the planned entry ({reason:?})",
            entry.path.display()
        ));
    }
    match gc::remove_counted(
        ctx.fs,
        ledger,
        target,
        &entry.path,
        ctx.account_bytes,
        false,
    ) {
        Ok(removed) if removed.existed => Deleted::Removed,
        Ok(_) => Deleted::Absent,
        Err(err) => Deleted::Failed(err.to_string()),
    }
}

fn removal_order(entries: &[PlannedEntry]) -> Vec<&PlannedEntry> {
    let mut units = Vec::new();
    for entry in entries {
        if !units.iter().any(|unit: &String| unit == &entry.unit) {
            units.push(entry.unit.clone());
        }
    }
    let mut ordered = Vec::new();
    for unit in &units {
        let mut deps = Vec::new();
        let mut others = Vec::new();
        let mut prints = Vec::new();
        for entry in entries.iter().filter(|entry| &entry.unit == unit) {
            if is_dep_info(entry) {
                deps.push(entry);
            } else if is_fingerprint_dir(entry) {
                prints.push(entry);
            } else {
                others.push(entry);
            }
        }
        if deps.is_empty() && others.is_empty() {
            continue;
        }
        if deps.is_empty() {
            ordered.extend(prints);
            ordered.extend(others);
        } else {
            ordered.extend(deps);
            ordered.extend(others);
            ordered.extend(prints);
        }
    }
    ordered
}

fn is_dep_info(entry: &PlannedEntry) -> bool {
    entry.path.extension().is_some_and(|ext| ext == "d")
}

fn is_fingerprint_dir(entry: &PlannedEntry) -> bool {
    entry
        .path
        .parent()
        .and_then(|parent| parent.file_name())
        .is_some_and(|name| name == ".fingerprint")
}

fn lists_examples(target: &PlannedTarget) -> bool {
    target.entries.iter().any(|entry| {
        entry
            .path
            .parent()
            .and_then(|parent| parent.file_name())
            .is_some_and(|name| name == "examples")
    })
}

fn skip_counts() -> BTreeMap<&'static str, u64> {
    let mut counts = BTreeMap::new();
    for reason in [
        Reason::Held,
        Reason::Unopenable,
        Reason::Unlockable,
        Reason::Missing,
        Reason::ProfileSet,
    ] {
        counts.insert(reason.count_name(), 0);
    }
    counts
}

fn unmeasured_accounting() -> Vec<Unmeasured> {
    ["freed_bytes", "deleted_bytes_still_shared"]
        .into_iter()
        .map(|field| Unmeasured {
            field: field.to_string(),
            reason: "doctor check (c) did not pass".to_string(),
        })
        .collect()
}

fn volumes(fs: &dyn Fs, paths: &[PathBuf]) -> BTreeMap<String, (PathBuf, u64)> {
    paths
        .iter()
        .filter_map(|path| fs.volume(path).ok())
        .map(|volume| (volume.container, (volume.mount, volume.available_bytes)))
        .collect()
}

fn target_paths(plan: &Plan) -> Vec<PathBuf> {
    plan.targets
        .iter()
        .map(|target| target.path.clone())
        .collect()
}

fn refused(id: &str, class: PlanClass, reason: &str, targets: Vec<PathBuf>) -> Applied {
    Applied {
        outcome: Outcome::Refused,
        reason: Some(reason.to_string()),
        class,
        id: id.to_string(),
        targets,
        counts: BTreeMap::new(),
        bytes: None,
        notes: Vec::new(),
        unmeasured: Vec::new(),
        findings: Vec::new(),
    }
}

fn count_map(
    paths_deleted: u64,
    applied_targets: u64,
    skips: &BTreeMap<&'static str, u64>,
    bytes: Option<BytesRec>,
) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();
    counts.insert("paths_deleted".to_string(), paths_deleted);
    counts.insert("targets_applied".to_string(), applied_targets);
    for (name, count) in skips {
        counts.insert((*name).to_string(), *count);
    }
    if let Some(bytes) = bytes {
        counts.insert("freed_bytes".to_string(), bytes.freed);
        counts.insert("deleted_bytes_still_shared".to_string(), bytes.still_shared);
    }
    counts
}
