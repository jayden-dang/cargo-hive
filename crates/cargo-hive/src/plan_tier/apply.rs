//! Applies one stored plan under the target's profile locks.
//! A refusal is a result, not an error. Locks stay held from the first acquisition
//! through the last delete, and a path is removed only after its entry still matches.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::caps::CRATE_DIR_CAP;
use crate::cargo_target::{self, Owner, TargetDir};
use crate::ctx::Ctx;
use crate::discover::{self, Snapshot};
use crate::fingerprint::{Format, ProfileReport, read_profile};
use crate::ledger::{self, Bytes, Ledger};
use crate::liveness::Liveness;
use crate::locks::{self, Acquired, Locked, Reason};
use crate::plan_tier::build::{TargetClass, class_label, class_of_target};
use crate::plan_tier::guard::{Deleted as EntryMismatch, Expect, check_entry};
use crate::plan_tier::orphan::{self, Counted, Removal};
use crate::plan_tier::record::{
    self, BytesRec, Lookup, Plan, PlanClass, PlannedEntry, PlannedTarget, Taken, kind_of,
};
use crate::receipt::{Outcome, Unmeasured};
use crate::state::StateError;
use cargo_hive_platform::Fs;

pub(crate) fn cross_check_notes(
    before: &std::collections::BTreeMap<String, (std::path::PathBuf, u64)>,
    after: &std::collections::BTreeMap<String, (std::path::PathBuf, u64)>,
    touched: &std::collections::BTreeSet<String>,
) -> Vec<String> {
    let mut notes = Vec::new();
    for container in touched {
        if let (Some((mount, was)), Some((_, now))) = (before.get(container), after.get(container))
        {
            let change = i128::from(*now) - i128::from(*was);
            notes.push(format!(
                "cross-check, not a count: available bytes in APFS container {container} (mounted at {}) changed by {change:+} during this run",
                mount.display()
            ));
        }
    }
    notes
}

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
    pub scope_repos: Vec<PathBuf>,
    pub scope_checkouts: Vec<PathBuf>,
}

fn plan_scope(plan: &Plan) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let mut checkouts = Vec::new();
    for target in &plan.targets {
        if let Some(owner) = &target.owner
            && !checkouts.contains(owner)
        {
            checkouts.push(owner.clone());
        }
    }
    (plan.scope_repos.clone(), checkouts)
}

pub fn apply(
    ctx: &Ctx,
    id: &str,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Applied, ApplyError> {
    let plan = match record::get(ctx.state, &|| cancel(), id).map_err(ApplyError::State)? {
        Lookup::Unknown => {
            return Ok(refused(
                id,
                PlanClass::Secondary,
                "unknown_plan",
                Vec::new(),
            ));
        }
        Lookup::Consumed => {
            return Ok(refused(
                id,
                PlanClass::Secondary,
                "already_consumed",
                Vec::new(),
            ));
        }
        Lookup::Live(plan) => plan,
    };
    if let Err(err) = crate::state::window_days(Some(plan.window_days)) {
        return Ok(window_refused(&plan, &err));
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
                ));
            }
            Taken::Unknown => {
                return Ok(refused(
                    id,
                    PlanClass::Secondary,
                    "unknown_plan",
                    Vec::new(),
                ));
            }
        };
    if cancel() {
        return Ok(cancelled_before_delete(&plan));
    }
    if plan.class == PlanClass::Orphan {
        return Ok(apply_orphan(ctx, &plan, cancel));
    }
    let mut noted = Vec::new();
    let resolved = match resolve(ctx, &plan, cancel, &mut noted) {
        Ok(resolved) => resolved,
        Err(ResolveStop::Cancelled) => return Ok(cancelled_before_delete(&plan)),
        Err(ResolveStop::Refusal(refusal)) => return Ok(refuse(&plan, refusal, BTreeMap::new())),
    };
    let mut done = execute(ctx, plan, &resolved, cancel);
    note_faults(&mut done.findings, noted);
    Ok(done)
}

fn apply_orphan(ctx: &Ctx, plan: &Plan, cancel: &(dyn Fn() -> bool + Sync)) -> Applied {
    let mut order: Vec<&PlannedTarget> = plan.targets.iter().collect();
    order.sort_by(|left, right| left.path.cmp(&right.path));
    let paths: Vec<PathBuf> = order.iter().map(|target| target.path.clone()).collect();
    let before = volumes(ctx.fs, &paths);
    let mut skips = skip_counts();
    let mut freed = BytesRec {
        freed: 0,
        still_shared: 0,
    };
    let mut paths_deleted = 0u64;
    let mut applied_targets = 0u64;
    let mut deleted_by_target: BTreeMap<PathBuf, u64> = BTreeMap::new();
    let mut per_target: BTreeMap<PathBuf, Bytes> = BTreeMap::new();
    let mut unmeasured = Vec::new();
    let mut unmeasured_more = 0u64;
    let mut findings = Vec::new();
    let mut touched = BTreeSet::new();
    let mut cancelled = false;
    let mut refusal: Option<String> = None;
    let mut delete_failed = false;
    for target in order {
        if cancel() {
            cancelled = true;
            break;
        }
        let mut noted = Vec::new();
        let removal = orphan::remove(ctx, target, cancel, &mut noted);
        note_faults(&mut findings, noted);
        match removal {
            Removal::Removed(counted) => {
                paths_deleted += 1;
                applied_targets += 1;
                deleted_by_target.insert(target.path.clone(), 1);
                note_counted(
                    counted,
                    &target.path,
                    &mut freed,
                    &mut per_target,
                    &mut unmeasured,
                    &mut unmeasured_more,
                );
                if let Ok(volume) = ctx.fs.volume(&target.path) {
                    touched.insert(volume.container);
                }
            }
            Removal::Skipped(reason) if reason == "cancelled" => {
                cancelled = true;
                break;
            }
            Removal::Skipped(reason) => {
                if let Some(count) = skips.get_mut(reason.as_str()) {
                    *count += 1;
                }
            }
            Removal::Refused(reason) => {
                findings.push((reason.clone(), Some(target.path.clone()), reason.clone()));
                refusal = Some(reason);
                break;
            }
            Removal::Unreadable(faults) => {
                for fault in faults {
                    findings.push((
                        "walk.unreadable".to_string(),
                        Some(fault.path),
                        fault.detail,
                    ));
                }
                refusal = Some("walk.unreadable".to_string());
                break;
            }
            Removal::MeasureRefused(reason) => {
                unmeasured.push(Unmeasured {
                    field: target.path.display().to_string(),
                    reason: reason.clone(),
                });
                findings.push((reason.clone(), Some(target.path.clone()), reason.clone()));
                refusal = Some(reason);
                break;
            }
            Removal::Failed(failed) => {
                note_counted(
                    failed.counted,
                    &target.path,
                    &mut freed,
                    &mut per_target,
                    &mut unmeasured,
                    &mut unmeasured_more,
                );
                findings.push((
                    "plan.delete_failed".to_string(),
                    Some(target.path.clone()),
                    failed.detail,
                ));
                delete_failed = true;
            }
        }
    }
    let after = volumes(ctx.fs, &paths);
    let notes = cross_check_notes(&before, &after, &touched);
    let total = plan.targets.len() as u64;
    let (outcome, reason) = if cancelled {
        (Outcome::Partial, Some("cancelled".to_string()))
    } else if delete_failed {
        (Outcome::Partial, Some("delete failed".to_string()))
    } else if let Some(reason) = refusal {
        if paths_deleted > 0 {
            (Outcome::Partial, Some(reason))
        } else {
            (Outcome::Refused, Some(reason))
        }
    } else if applied_targets != total {
        let why = if applied_targets == 0 {
            "all_targets_skipped"
        } else {
            "skipped"
        };
        (Outcome::Partial, Some(why.to_string()))
    } else {
        (Outcome::Applied, None)
    };
    let measured = freed.freed > 0 || freed.still_shared > 0;
    let publish_bytes =
        ctx.account_bytes && (outcome == Outcome::Applied || paths_deleted > 0 || measured);
    let bytes = publish_bytes.then_some(freed);
    if !ctx.account_bytes && outcome != Outcome::Refused {
        let gaps = std::mem::take(&mut unmeasured);
        unmeasured = unmeasured_accounting();
        unmeasured.extend(gaps);
    }
    if unmeasured_more > 0 {
        unmeasured.push(Unmeasured {
            field: "unmeasured_paths".to_string(),
            reason: format!(
                "{unmeasured_more} more paths could not be measured and are not listed"
            ),
        });
    }
    Applied {
        outcome,
        reason,
        class: plan.class,
        id: plan.id.clone(),
        targets: target_paths(plan),
        counts: {
            let mut counts = count_map(paths_deleted, applied_targets, &skips, bytes);
            record_target_counts(
                &mut counts,
                &plan.targets,
                &deleted_by_target,
                &per_target,
                publish_bytes,
            );
            counts
        },
        bytes,
        notes,
        unmeasured,
        findings,
        scope_repos: plan.scope_repos.clone(),
        scope_checkouts: plan_scope(plan).1,
    }
}

fn note_counted(
    counted: Counted,
    target: &Path,
    freed: &mut BytesRec,
    per_target: &mut BTreeMap<PathBuf, Bytes>,
    unmeasured: &mut Vec<Unmeasured>,
    unmeasured_more: &mut u64,
) {
    freed.freed = freed.freed.saturating_add(counted.bytes.freed);
    freed.still_shared = freed
        .still_shared
        .saturating_add(counted.bytes.still_shared);
    per_target.insert(
        target.to_path_buf(),
        Bytes {
            freed: counted.bytes.freed,
            still_shared: counted.bytes.still_shared,
        },
    );
    for (path, why) in counted.unmeasured {
        unmeasured.push(Unmeasured {
            field: path.display().to_string(),
            reason: why,
        });
    }
    *unmeasured_more = unmeasured_more.saturating_add(counted.unmeasured_more);
}

pub(crate) fn exit_code(applied: &Applied) -> i32 {
    if applied.reason.as_deref() == Some("cancelled") {
        return 130;
    }
    if applied.outcome == Outcome::Refused
        || applied
            .findings
            .iter()
            .any(|(kind, _, _)| kind == "plan.delete_failed")
    {
        return 1;
    }
    0
}

#[doc(hidden)]
pub fn execute(
    ctx: &Ctx,
    plan: Plan,
    resolved: &[ResolvedTarget],
    cancel: &(dyn Fn() -> bool + Sync),
) -> Applied {
    if let Err(err) = crate::state::window_days(Some(plan.window_days)) {
        return window_refused(&plan, &err);
    }
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
            Acquired::Failed(why) => {
                return refuse(
                    &plan,
                    Refusal {
                        reason: "unsupported_format",
                        target: target.path.clone(),
                        path: Some(target.path.clone()),
                        details: vec![why],
                    },
                    count_map(0, 0, &skips, None),
                );
            }
        }
    }
    let skipped_profiles = match recheck(ctx, &plan, resolved, &locked) {
        Ok(skipped) => skipped,
        Err(refusal) => return refuse(&plan, refusal, count_map(0, 0, &skips, None)),
    };
    if let Some(count) = skips.get_mut(Reason::ProfileSet.count_name()) {
        *count += skipped_profiles.len() as u64;
    }
    locked.retain(|(target, _)| !skipped_profiles.contains(&target.path));
    let locked_paths: Vec<PathBuf> = locked
        .iter()
        .map(|(target, _)| target.path.clone())
        .collect();
    let before = volumes(ctx.fs, &locked_paths);
    let mut ledger = Ledger::new();
    let mut paths_deleted = 0u64;
    let mut deleted_by_target: BTreeMap<PathBuf, u64> = BTreeMap::new();
    let mut applied_targets = 0u64;
    let mut failed = false;
    let mut cancelled = false;
    let mut findings = Vec::new();
    let mut touched = BTreeSet::new();
    for (target, _) in &locked {
        let mut deleted_here = 0u64;
        let mut target_failed = false;
        for unit in removal_order(&target.entries) {
            if cancel() {
                cancelled = true;
                break;
            }
            let mut unit_failed = false;
            for entry in unit {
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
                        unit_failed = true;
                        target_failed = true;
                        failed = true;
                    }
                }
            }
            if unit_failed {
                break;
            }
        }
        deleted_by_target.insert(target.path.clone(), deleted_here);
        if !target_failed && !cancelled {
            applied_targets += 1;
        }
        if deleted_here > 0
            && let Ok(volume) = ctx.fs.volume(&target.path)
        {
            touched.insert(volume.container);
        }
        if cancelled {
            break;
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
    let (outcome, reason) = if cancelled {
        (Outcome::Partial, Some("cancelled".to_string()))
    } else if failed || applied_targets != total {
        let reason = if failed {
            Some("delete failed".to_string())
        } else if applied_targets == 0 {
            Some("all_targets_skipped".to_string())
        } else {
            Some("skipped".to_string())
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
        counts: {
            let mut counts = count_map(paths_deleted, applied_targets, &skips, bytes);
            record_target_counts(
                &mut counts,
                &plan.targets,
                &deleted_by_target,
                &tally.per_target,
                ctx.account_bytes,
            );
            counts
        },
        bytes,
        notes,
        unmeasured,
        findings,
        scope_repos: plan.scope_repos.clone(),
        scope_checkouts: plan_scope(&plan).1,
    }
}

struct Refusal {
    reason: &'static str,
    target: PathBuf,
    path: Option<PathBuf>,
    details: Vec<String>,
}

enum ResolveStop {
    Refusal(Refusal),
    Cancelled,
}

/// Reports walk faults that did not stop the plan, each path once.
fn note_faults(
    findings: &mut Vec<(String, Option<PathBuf>, String)>,
    faults: Vec<crate::discover::WalkFault>,
) {
    for fault in faults {
        let seen = findings
            .iter()
            .any(|(kind, path, _)| kind == "walk.unreadable" && path.as_ref() == Some(&fault.path));
        if !seen {
            findings.push((
                "walk.unreadable".to_string(),
                Some(fault.path),
                fault.detail,
            ));
        }
    }
}

fn resolve(
    ctx: &Ctx,
    plan: &Plan,
    cancel: &(dyn Fn() -> bool + Sync),
    noted: &mut Vec<crate::discover::WalkFault>,
) -> Result<Vec<ResolvedTarget>, ResolveStop> {
    let (checkouts, faults) = crate::ctx::checkout_walk(ctx);
    if cancel() {
        return Err(ResolveStop::Cancelled);
    }
    let (faults, cannot_enter) = crate::ctx::split_checkout_faults(faults);
    noted.extend(cannot_enter);
    if !faults.is_empty() {
        return Err(walk_refusal(plan, faults));
    }
    let mut snapshot = Snapshot::default();
    let scan = discover::targets(&checkouts, ctx.cfg, true, &mut snapshot, cancel);
    if cancel() {
        return Err(ResolveStop::Cancelled);
    }
    let faults = crate::ctx::faults_that_can_hide_an_owner(scan.walk_faults, &ctx.cfg.cache_roots);
    if !faults.is_empty() {
        return Err(walk_refusal(plan, faults));
    }
    let mut planned: Vec<&PlannedTarget> = plan.targets.iter().collect();
    planned.sort_by(|left, right| left.path.cmp(&right.path));
    let mut located = Vec::new();
    for target in &planned {
        let Some(found) = scan.targets.iter().find(|cand| cand.path == target.path) else {
            return Err(ResolveStop::Refusal(ownership_unresolved(
                target,
                "the target was not resolved",
            )));
        };
        if let Owner::Unresolved { reason } = &found.owner {
            return Err(ResolveStop::Refusal(ownership_unresolved(target, reason)));
        }
        if let TargetClass::Unresolved = class_of_target(&found.owner, &checkouts) {
            return Err(ResolveStop::Refusal(ownership_unresolved(
                target,
                "ownership could not be resolved",
            )));
        }
        located.push((*target, found));
    }
    for (target, found) in &located {
        let owner = owned_checkout(&found.owner);
        if owner != target.owner {
            let recorded = path_or_none(target.owner.as_deref());
            let now = path_or_none(owner.as_deref());
            return Err(ResolveStop::Refusal(Refusal {
                reason: "owner_changed",
                target: target.path.clone(),
                path: None,
                details: vec![format!("owner is {now} and the plan recorded {recorded}")],
            }));
        }
    }
    let mut resolved = Vec::new();
    for (target, found) in located {
        let TargetClass::Class(class) = class_of_target(&found.owner, &checkouts) else {
            return Err(ResolveStop::Refusal(ownership_unresolved(
                target,
                "ownership could not be resolved",
            )));
        };
        if class != plan.class {
            return Err(ResolveStop::Refusal(Refusal {
                reason: "class_changed",
                target: target.path.clone(),
                path: None,
                details: vec![format!(
                    "class is {} and the plan recorded {}",
                    class_label(class),
                    class_label(plan.class)
                )],
            }));
        }
        resolved.push(ResolvedTarget {
            path: found.path.clone(),
            owner: owned_checkout(&found.owner),
            class,
        });
    }
    Ok(resolved)
}

fn walk_refusal(plan: &Plan, faults: Vec<crate::discover::WalkFault>) -> ResolveStop {
    let target = plan
        .targets
        .first()
        .map(|target| target.path.clone())
        .unwrap_or_else(|| faults[0].path.clone());
    let path = faults[0].path.clone();
    let details = faults
        .iter()
        .map(|fault| format!("{}: {}", fault.path.display(), fault.detail))
        .collect();
    ResolveStop::Refusal(Refusal {
        reason: "walk.unreadable",
        target,
        path: Some(path),
        details,
    })
}

fn owned_checkout(owner: &Owner) -> Option<PathBuf> {
    match owner {
        Owner::Owned { checkout } => Some(checkout.clone()),
        Owner::Unresolved { .. } | Owner::OrphanCandidate { .. } => None,
    }
}

fn path_or_none(path: Option<&Path>) -> String {
    path.map(|path| path.display().to_string())
        .unwrap_or_else(|| "none".to_string())
}

fn ownership_unresolved(target: &PlannedTarget, detail: &str) -> Refusal {
    Refusal {
        reason: "ownership_unresolved",
        target: target.path.clone(),
        path: None,
        details: vec![detail.to_string()],
    }
}

struct Loaded<'a> {
    target: &'a PlannedTarget,
    reports: Vec<ProfileReport>,
}

fn recheck(
    ctx: &Ctx,
    plan: &Plan,
    resolved: &[ResolvedTarget],
    locked: &[(&PlannedTarget, Locked)],
) -> Result<Vec<PathBuf>, Refusal> {
    let window = Duration::from_secs(plan.window_days.saturating_mul(86_400));
    let now = SystemTime::now();
    let mut loaded = Vec::new();
    let mut skipped = Vec::new();
    for (target, held) in locked {
        let profiles = listed_profiles(ctx.fs, target)?;
        let again: BTreeSet<_> = profiles.iter().cloned().collect();
        let first: BTreeSet<_> = held.profiles.iter().cloned().collect();
        if again != first {
            skipped.push(target.path.clone());
            continue;
        }
        loaded.push(Loaded {
            target,
            reports: profile_reports(target, &profiles)?,
        });
    }
    for item in &loaded {
        for entry in &item.target.entries {
            if let Some(refusal) = entry_refusal(ctx, item.target, entry) {
                return Err(refusal);
            }
        }
    }
    let mut decided = Vec::new();
    for item in loaded {
        let dead = dead_paths(ctx, resolved, item.target, item.reports, now, window)?;
        decided.push((item.target, dead));
    }
    for (target, dead_paths) in decided {
        for entry in &target.entries {
            if !dead_paths.contains(entry.path.as_path()) {
                return Err(Refusal {
                    reason: "stale_item",
                    target: target.path.clone(),
                    path: Some(entry.path.clone()),
                    details: vec!["the planned path is no longer dead".to_string()],
                });
            }
        }
    }
    Ok(skipped)
}

fn listed_profiles(fs: &dyn Fs, target: &PlannedTarget) -> Result<Vec<PathBuf>, Refusal> {
    match cargo_target::identify_bounded(fs, &target.path, CRATE_DIR_CAP) {
        Some(Ok(profiles)) => Ok(profiles.into_iter().map(|(path, _)| path).collect()),
        Some(Err(why)) => Err(Refusal {
            reason: "unsupported_format",
            target: target.path.clone(),
            path: Some(target.path.clone()),
            details: vec![why],
        }),
        None => Err(Refusal {
            reason: "unsupported_format",
            target: target.path.clone(),
            path: Some(target.path.clone()),
            details: vec!["the target has no readable profile set".to_string()],
        }),
    }
}

fn profile_reports(
    target: &PlannedTarget,
    profiles: &[PathBuf],
) -> Result<Vec<ProfileReport>, Refusal> {
    let reports: Vec<_> = profiles.iter().map(|path| read_profile(path)).collect();
    for report in &reports {
        if let Format::Unsupported(why) = &report.format {
            return Err(Refusal {
                reason: "unsupported_format",
                target: target.path.clone(),
                path: Some(report.path.clone()),
                details: vec![why.clone()],
            });
        }
    }
    Ok(reports)
}

fn dead_paths(
    ctx: &Ctx,
    resolved: &[ResolvedTarget],
    target: &PlannedTarget,
    reports: Vec<ProfileReport>,
    now: SystemTime,
    window: Duration,
) -> Result<HashSet<PathBuf>, Refusal> {
    let owner = match resolved
        .iter()
        .find(|row| row.path == target.path)
        .and_then(|row| row.owner.clone())
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
    match crate::gc::liveness::liveness(ctx.fs, &dir, now, window) {
        Liveness::Decided { dead, .. } => Ok(dead
            .iter()
            .flat_map(|unit| unit.files.iter().cloned())
            .collect()),
        Liveness::Undecided { reasons } => {
            let details = if reasons.is_empty() {
                vec!["the target could not be decided".to_string()]
            } else {
                reasons.iter().map(ToString::to_string).collect()
            };
            Err(Refusal {
                reason: "undecided",
                target: target.path.clone(),
                path: None,
                details,
            })
        }
    }
}

fn entry_refusal(ctx: &Ctx, target: &PlannedTarget, entry: &PlannedEntry) -> Option<Refusal> {
    let Some(kind) = kind_of(&entry.kind) else {
        return Some(Refusal {
            reason: "entry_changed",
            target: target.path.clone(),
            path: Some(entry.path.clone()),
            details: vec!["the plan records an unknown entry kind".to_string()],
        });
    };
    let mismatch = check_entry(
        ctx.fs,
        &target.path,
        &entry.path,
        Expect::Entry {
            kind,
            dev: entry.dev,
            ino: entry.ino,
        },
    )
    .err()?;
    Some(Refusal {
        reason: "entry_changed",
        target: target.path.clone(),
        path: Some(entry.path.clone()),
        details: vec![match mismatch {
            EntryMismatch::Missing => "the planned entry is missing".to_string(),
            EntryMismatch::Changed => "the planned entry changed".to_string(),
            EntryMismatch::Outside => "the planned entry is outside the target".to_string(),
            EntryMismatch::Removed => "the planned entry was removed".to_string(),
        }],
    })
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
    match ledger::remove_counted(
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

fn removal_order(entries: &[PlannedEntry]) -> Vec<Vec<&PlannedEntry>> {
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
        let mut group = Vec::new();
        if deps.is_empty() {
            group.extend(prints);
            group.extend(others);
        } else {
            group.extend(deps);
            group.extend(others);
            group.extend(prints);
        }
        ordered.push(group);
    }
    ordered
}

fn cancelled_before_delete(plan: &Plan) -> Applied {
    Applied {
        outcome: Outcome::Partial,
        reason: Some("cancelled".to_string()),
        class: plan.class,
        id: plan.id.clone(),
        targets: target_paths(plan),
        counts: count_map(0, 0, &skip_counts(), None),
        bytes: None,
        notes: Vec::new(),
        unmeasured: Vec::new(),
        findings: Vec::new(),
        scope_repos: plan.scope_repos.clone(),
        scope_checkouts: plan_scope(plan).1,
    }
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

fn refuse(plan: &Plan, refusal: Refusal, counts: BTreeMap<String, u64>) -> Applied {
    let findings = refusal_findings(&refusal);
    Applied {
        outcome: Outcome::Refused,
        reason: Some(refusal.reason.to_string()),
        class: plan.class,
        id: plan.id.clone(),
        targets: target_paths(plan),
        counts,
        bytes: None,
        notes: Vec::new(),
        unmeasured: Vec::new(),
        findings,
        scope_repos: plan.scope_repos.clone(),
        scope_checkouts: plan_scope(plan).1,
    }
}

fn refusal_findings(refusal: &Refusal) -> Vec<(String, Option<PathBuf>, String)> {
    let kind = refusal.reason.to_string();
    if refusal.reason == "undecided" {
        return refusal
            .details
            .iter()
            .map(|detail| (kind.clone(), Some(refusal.target.clone()), detail.clone()))
            .collect();
    }
    let path = refusal
        .path
        .clone()
        .unwrap_or_else(|| refusal.target.clone());
    let detail = if refusal.path.is_none() {
        refusal.details.join("; ")
    } else {
        format!(
            "{}: {}",
            refusal.target.display(),
            refusal.details.join("; ")
        )
    };
    vec![(kind, Some(path), detail)]
}

fn window_refused(plan: &Plan, err: &crate::state::WindowError) -> Applied {
    let mut applied = refused(&plan.id, plan.class, "window", target_paths(plan));
    applied
        .findings
        .push(("window".to_string(), None, err.to_string()));
    let (repos, checkouts) = plan_scope(plan);
    applied.scope_repos = repos;
    applied.scope_checkouts = checkouts;
    applied
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
        scope_repos: Vec::new(),
        scope_checkouts: Vec::new(),
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

fn record_target_counts(
    counts: &mut BTreeMap<String, u64>,
    targets: &[PlannedTarget],
    deleted_by_target: &BTreeMap<PathBuf, u64>,
    per_target: &BTreeMap<PathBuf, Bytes>,
    account_bytes: bool,
) {
    for target in targets {
        let label = target.path.display().to_string();
        counts.insert(
            format!("paths_deleted:{label}"),
            deleted_by_target.get(&target.path).copied().unwrap_or(0),
        );
        if account_bytes {
            let bytes = per_target.get(&target.path).copied().unwrap_or_default();
            counts.insert(format!("freed_bytes:{label}"), bytes.freed);
            counts.insert(
                format!("deleted_bytes_still_shared:{label}"),
                bytes.still_shared,
            );
        }
    }
}

#[cfg(test)]
mod exit_code_tests {
    use super::*;

    fn sample(outcome: Outcome, reason: Option<&str>, finding: Option<&str>) -> Applied {
        Applied {
            outcome,
            reason: reason.map(str::to_string),
            class: PlanClass::Secondary,
            id: "plan".to_string(),
            targets: Vec::new(),
            counts: BTreeMap::new(),
            bytes: None,
            notes: Vec::new(),
            unmeasured: Vec::new(),
            findings: finding
                .map(|kind| vec![(kind.to_string(), None, "stopped".to_string())])
                .unwrap_or_default(),
            scope_repos: Vec::new(),
            scope_checkouts: Vec::new(),
        }
    }

    #[test]
    fn each_outcome_keeps_its_exit_and_a_failed_delete_is_not_a_skip() {
        assert_eq!(exit_code(&sample(Outcome::Applied, None, None)), 0);
        assert_eq!(
            exit_code(&sample(Outcome::Refused, Some("unknown_plan"), None)),
            1
        );
        assert_eq!(
            exit_code(&sample(Outcome::Partial, Some("cancelled"), None)),
            130
        );
        assert_eq!(
            exit_code(&sample(
                Outcome::Partial,
                Some("delete failed"),
                Some("plan.delete_failed")
            )),
            1
        );
        assert_eq!(
            exit_code(&sample(Outcome::Partial, Some("skipped"), None)),
            0
        );
        assert_eq!(
            exit_code(&sample(Outcome::Partial, Some("all_targets_skipped"), None)),
            0
        );
        assert_eq!(
            exit_code(&sample(
                Outcome::Partial,
                Some("cancelled"),
                Some("plan.delete_failed")
            )),
            130
        );
    }
}
