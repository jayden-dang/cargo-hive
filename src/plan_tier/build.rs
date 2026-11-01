//! Builds one plan per checkout class from ownership and a liveness verdict.
//! Planning records paths in hive's state and writes nothing in a checkout.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::cargo_target::{Owner, TargetDir};
use crate::classify::Class;
use crate::discover::{self, Checkout, Scope, Snapshot};
use crate::gc;
use crate::liveness::{self, Liveness, Uncertain, UnitRef};
use crate::plan_tier::record::{
    kind_name, BytesRec, Plan, PlanClass, PlanState, PlannedEntry, PlannedTarget,
};
use crate::plan_tier::Ctx;
use crate::receipt::{ReasonRecord, UndecidedRecord, Unmeasured};
use crate::state::window_days;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetClass {
    Class(PlanClass),
    Unresolved,
}

pub enum TargetPlan {
    Planned(PlannedTarget),
    NothingToDo,
    Undecided(Vec<Uncertain>),
}

pub struct Made {
    pub plans: Vec<Plan>,
    pub undecided: Vec<UndecidedRecord>,
    pub nothing_to_do: Vec<PathBuf>,
    pub unmeasured: Vec<Unmeasured>,
    pub findings: Vec<(String, Option<PathBuf>, String)>,
}

#[derive(Debug)]
pub enum MakeError {
    Window(crate::state::WindowError),
    Cancelled,
}

impl fmt::Display for MakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MakeError::Window(err) => write!(f, "{err}"),
            MakeError::Cancelled => write!(f, "gc --plan was cancelled"),
        }
    }
}

impl std::error::Error for MakeError {}

pub fn class_label(class: PlanClass) -> &'static str {
    match class {
        PlanClass::Secondary => "secondary",
        PlanClass::Protected => "protected",
        PlanClass::Orphan => "orphan",
    }
}

pub fn class_of_target(owner: &Owner, checkouts: &[Checkout]) -> TargetClass {
    match owner {
        Owner::Owned { checkout } => match checkouts.iter().find(|c| c.path == *checkout) {
            Some(checkout) if checkout.class == Class::Secondary => {
                TargetClass::Class(PlanClass::Secondary)
            }
            _ => TargetClass::Class(PlanClass::Protected),
        },
        Owner::OrphanCandidate { .. } => TargetClass::Class(PlanClass::Orphan),
        Owner::Unresolved { .. } => TargetClass::Unresolved,
    }
}

pub fn plan_for_target(ctx: &Ctx, target: &TargetDir, window: Duration) -> TargetPlan {
    let dead = match liveness::liveness(ctx.fs, target, ctx.now, window) {
        Liveness::Undecided { reasons } => return TargetPlan::Undecided(reasons),
        Liveness::Decided { dead, .. } => dead,
    };
    let mut seen = HashSet::new();
    let mut paths = Vec::new();
    let mut entries = Vec::new();
    for unit in dead {
        let label = unit_label(&target.path, &unit.unit);
        for path in unit.files {
            if !seen.insert(path.clone()) {
                continue;
            }
            let Ok(stat) = ctx.fs.entry_stat(&path) else {
                continue;
            };
            paths.push(path.clone());
            entries.push(PlannedEntry {
                path,
                unit: label.clone(),
                kind: kind_name(stat.kind).to_string(),
                dev: stat.dev,
                ino: stat.ino,
            });
        }
    }
    if entries.is_empty() {
        return TargetPlan::NothingToDo;
    }
    let owner = match &target.owner {
        Owner::Owned { checkout } => Some(checkout.clone()),
        Owner::OrphanCandidate { .. } | Owner::Unresolved { .. } => None,
    };
    let planned = ctx.account_bytes.then(|| {
        let foot = gc::measure(ctx.fs, &target.path, &paths, true);
        BytesRec {
            freed: foot.bytes.freed,
            still_shared: foot.bytes.still_shared,
        }
    });
    TargetPlan::Planned(PlannedTarget {
        path: target.path.clone(),
        owner,
        planned,
        entries,
        orphan: None,
    })
}

impl Plan {
    pub fn single(class: PlanClass, target: PlannedTarget, window_days: u64, now: u64) -> Plan {
        Plan {
            id: mint_id(),
            class,
            window_days,
            made_at: now,
            scope_repos: Vec::new(),
            planned: target.planned,
            targets: vec![target],
            state: PlanState::Live,
        }
    }
}

pub fn reissue(plan: &mut Plan) {
    plan.id = mint_id();
}

pub fn make(
    ctx: &Ctx,
    scope_repos: &[PathBuf],
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Made, MakeError> {
    let days = window_days(ctx.cfg.gc_window_days).map_err(MakeError::Window)?;
    if cancel() {
        return Err(MakeError::Cancelled);
    }
    let mut findings = Vec::new();
    let mut checkouts = Vec::new();
    let mut by_repo: Vec<(PathBuf, Vec<Checkout>)> = Vec::new();
    for repo in ctx.repos {
        if cancel() {
            return Err(MakeError::Cancelled);
        }
        let (rows, faults) = discover::checkouts(repo, ctx.mains, ctx.cfg);
        for fault in faults {
            findings.push((
                "walk.unreadable".to_string(),
                Some(fault.path),
                fault.detail,
            ));
        }
        by_repo.push((repo.main.clone(), rows.clone()));
        checkouts.extend(rows);
    }
    let mut snapshot = Snapshot::default();
    let scan = discover::targets(&checkouts, ctx.cfg, true, &mut snapshot, cancel);
    if cancel() {
        return Err(MakeError::Cancelled);
    }
    for fault in scan.walk_faults {
        findings.push((
            "walk.unreadable".to_string(),
            Some(fault.path),
            fault.detail,
        ));
    }
    let scope = scope_for(&checkouts, &by_repo, scope_repos, &mut findings);
    let recorded = repos_recorded(ctx, scope_repos, &checkouts);
    let window = Duration::from_secs(days * 86_400);
    let now = unix_secs(ctx.now);
    let mut buckets: [Option<Plan>; 3] = [None, None, None];
    let mut undecided = Vec::new();
    let mut nothing_to_do = Vec::new();
    for target in &scan.targets {
        if cancel() {
            return Err(MakeError::Cancelled);
        }
        if !scope.contains(target) {
            continue;
        }
        match class_of_target(&target.owner, &checkouts) {
            TargetClass::Unresolved => {
                let detail = match &target.owner {
                    Owner::Unresolved { reason } => reason.clone(),
                    Owner::Owned { .. } | Owner::OrphanCandidate { .. } => {
                        "ownership could not be resolved".to_string()
                    }
                };
                findings.push((
                    "ownership.unresolved".to_string(),
                    Some(target.path.clone()),
                    detail,
                ));
            }
            TargetClass::Class(PlanClass::Orphan) => {
                findings.push((
                    "plan.skipped".to_string(),
                    Some(target.path.clone()),
                    "an orphan target is not planned yet".to_string(),
                ));
            }
            TargetClass::Class(class) => match plan_for_target(ctx, target, window) {
                TargetPlan::Undecided(reasons) => undecided.push(UndecidedRecord {
                    target: target.path.clone(),
                    reasons: reasons
                        .iter()
                        .map(|reason| ReasonRecord {
                            what: reason.to_string(),
                            clears_when: reason.clears_when().to_string(),
                        })
                        .collect(),
                }),
                TargetPlan::NothingToDo => nothing_to_do.push(target.path.clone()),
                TargetPlan::Planned(planned) => {
                    let slot = class_index(class);
                    if let Some(plan) = buckets[slot].as_mut() {
                        add_target(plan, planned);
                    } else {
                        let mut plan = Plan::single(class, planned, days, now);
                        plan.scope_repos.clone_from(&recorded);
                        buckets[slot] = Some(plan);
                    }
                }
            },
        }
    }
    let plans: Vec<Plan> = buckets.into_iter().flatten().collect();
    let mut unmeasured = Vec::new();
    if !ctx.account_bytes && !plans.is_empty() {
        for field in ["freed_bytes", "deleted_bytes_still_shared"] {
            unmeasured.push(Unmeasured {
                field: field.to_string(),
                reason: "doctor check (c) did not pass".to_string(),
            });
        }
    }
    Ok(Made {
        plans,
        undecided,
        nothing_to_do,
        unmeasured,
        findings,
    })
}

fn class_index(class: PlanClass) -> usize {
    match class {
        PlanClass::Secondary => 0,
        PlanClass::Protected => 1,
        PlanClass::Orphan => 2,
    }
}

fn add_target(plan: &mut Plan, target: PlannedTarget) {
    match (&mut plan.planned, target.planned) {
        (Some(sum), Some(extra)) => {
            sum.freed = sum.freed.saturating_add(extra.freed);
            sum.still_shared = sum.still_shared.saturating_add(extra.still_shared);
        }
        (_, None) => plan.planned = None,
        (None, Some(_)) => {}
    }
    plan.targets.push(target);
}

fn unit_label(target: &Path, unit: &UnitRef) -> String {
    let rel = unit.profile.strip_prefix(target).unwrap_or(&unit.profile);
    format!("{}/{}", rel.display(), unit.dir_name)
}

fn mint_id() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    crate::plan_tier::record::new_id(
        nanos,
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
    )
}

fn unix_secs(now: SystemTime) -> u64 {
    now.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn real(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn scope_for(
    checkouts: &[Checkout],
    by_repo: &[(PathBuf, Vec<Checkout>)],
    scope_repos: &[PathBuf],
    findings: &mut Vec<(String, Option<PathBuf>, String)>,
) -> Scope {
    if scope_repos.is_empty() {
        return Scope::All;
    }
    let mut paths = Vec::new();
    for arg in scope_repos {
        let path = real(arg);
        if let Some((_, rows)) = by_repo.iter().find(|(main, _)| *main == path) {
            for checkout in rows {
                if !checkout.missing && !paths.contains(&checkout.path) {
                    paths.push(checkout.path.clone());
                }
            }
            continue;
        }
        if let Some(checkout) = checkouts.iter().find(|c| !c.missing && c.path == path) {
            if !paths.contains(&checkout.path) {
                paths.push(checkout.path.clone());
            }
            continue;
        }
        findings.push((
            "refusal".to_string(),
            Some(arg.clone()),
            "not a registered repository; run `cargo hive repo add` first".to_string(),
        ));
    }
    Scope::from_paths(false, paths.into_iter())
}

fn repos_recorded(ctx: &Ctx, scope_repos: &[PathBuf], checkouts: &[Checkout]) -> Vec<PathBuf> {
    if scope_repos.is_empty() {
        return ctx.repos.iter().map(|repo| repo.main.clone()).collect();
    }
    let mut mains = Vec::new();
    for arg in scope_repos {
        let path = real(arg);
        if let Some(repo) = ctx.repos.iter().find(|repo| repo.main == path) {
            push_unique(&mut mains, repo.main.clone());
            continue;
        }
        let Some(checkout) = checkouts.iter().find(|c| c.path == path) else {
            continue;
        };
        let main = match &checkout.kind {
            crate::classify::CheckoutKind::Linked { main } => Some(main.clone()),
            crate::classify::CheckoutKind::Main | crate::classify::CheckoutKind::Clone => ctx
                .repos
                .iter()
                .find(|repo| repo.main == checkout.path)
                .map(|repo| repo.main.clone()),
        };
        if let Some(main) = main {
            push_unique(&mut mains, main);
        }
    }
    mains
}

fn push_unique(paths: &mut Vec<PathBuf>, path: PathBuf) {
    if !paths.contains(&path) {
        paths.push(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cargo_target::Owner;
    use crate::classify::{CheckoutKind, Class};
    use crate::discover::Checkout;

    fn checkout(path: &str, class: Class) -> Checkout {
        Checkout {
            path: PathBuf::from(path),
            kind: CheckoutKind::Main,
            class,
            reason: String::new(),
            missing: false,
        }
    }

    #[test]
    fn an_owned_secondary_checkout_is_secondary() {
        let checkouts = [checkout("/wt", Class::Secondary)];
        let owner = Owner::Owned {
            checkout: PathBuf::from("/wt"),
        };
        assert_eq!(
            class_of_target(&owner, &checkouts),
            TargetClass::Class(PlanClass::Secondary)
        );
    }

    #[test]
    fn an_owned_protected_checkout_is_protected() {
        let checkouts = [checkout("/main", Class::Protected)];
        let owner = Owner::Owned {
            checkout: PathBuf::from("/main"),
        };
        assert_eq!(
            class_of_target(&owner, &checkouts),
            TargetClass::Class(PlanClass::Protected)
        );
    }

    #[test]
    fn an_owner_missing_from_the_checkout_list_is_protected() {
        let checkouts = [checkout("/main", Class::Secondary)];
        let owner = Owner::Owned {
            checkout: PathBuf::from("/gone"),
        };
        assert_eq!(
            class_of_target(&owner, &checkouts),
            TargetClass::Class(PlanClass::Protected)
        );
    }

    #[test]
    fn an_orphan_candidate_is_its_own_class() {
        let owner = Owner::OrphanCandidate {
            owned_under_caller_env: None,
        };
        assert_eq!(
            class_of_target(&owner, &[]),
            TargetClass::Class(PlanClass::Orphan)
        );
    }

    #[test]
    fn an_unresolved_owner_is_not_a_class() {
        let owner = Owner::Unresolved {
            reason: "cargo metadata failed".to_string(),
        };
        assert_eq!(class_of_target(&owner, &[]), TargetClass::Unresolved);
    }
}
