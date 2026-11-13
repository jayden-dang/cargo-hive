//! Builds one plan per checkout class from ownership and a liveness verdict.
//! An orphan target is one entry for the directory itself, not the files inside it.
//! Planning records paths in hive's state and writes nothing in a checkout.
//! A listed path whose stat fails stores no plan. The finding names the path, and that target is not nothing to do.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::cargo_target::{Owner, TargetDir};
use crate::classify::Class;
use crate::ctx::Ctx;
use crate::discover::{self, Checkout, Scope, Snapshot};
use crate::gc;
use crate::liveness::{self, Liveness, Uncertain, UnitRef};
use crate::plan_tier::record::{
    kind_name, BytesRec, OrphanScope, Plan, PlanClass, PlanState, PlannedEntry, PlannedTarget,
};
use crate::receipt::{ReasonRecord, UndecidedRecord, Unmeasured};
use crate::state::window_days;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetClass {
    Class(PlanClass),
    Unresolved,
}

pub struct BuiltTarget {
    pub target: PlannedTarget,
    pub unmeasured: Vec<(PathBuf, String)>,
    pub unmeasured_more: u64,
}

pub enum TargetPlan {
    Planned(BuiltTarget),
    NothingToDo,
    Undecided(Vec<Uncertain>),
    Unreadable(Vec<(PathBuf, String)>),
}

pub struct Made {
    pub plans: Vec<Plan>,
    pub undecided: Vec<UndecidedRecord>,
    pub nothing_to_do: Vec<PathBuf>,
    pub unmeasured: Vec<Unmeasured>,
    pub findings: Vec<(String, Option<PathBuf>, String)>,
}

/// Exit status of `gc --plan` after planning finishes.
pub fn command_code(made: &Made) -> i32 {
    i32::from(
        made.findings
            .iter()
            .any(|(kind, _, _)| kind == "refusal" || kind == "plan.unreadable"),
    )
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
    let mut unread = Vec::new();
    for unit in dead {
        let label = unit_label(&target.path, &unit.unit);
        for path in unit.files {
            if !seen.insert(path.clone()) {
                continue;
            }
            let stat = match ctx.fs.entry_stat(&path) {
                Ok(stat) => stat,
                Err(err) => {
                    unread.push((path, err.to_string()));
                    continue;
                }
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
    if !unread.is_empty() {
        return TargetPlan::Unreadable(unread);
    }
    if entries.is_empty() {
        return TargetPlan::NothingToDo;
    }
    let owner = match &target.owner {
        Owner::Owned { checkout } => Some(checkout.clone()),
        Owner::OrphanCandidate { .. } | Owner::Unresolved { .. } => None,
    };
    let (planned, unmeasured, unmeasured_more) = if ctx.account_bytes {
        let foot = gc::measure(ctx.fs, &target.path, &paths, true);
        (
            Some(BytesRec {
                freed: foot.bytes.freed,
                still_shared: foot.bytes.still_shared,
            }),
            foot.unmeasured,
            foot.unmeasured_more,
        )
    } else {
        (None, Vec::new(), 0)
    };
    TargetPlan::Planned(BuiltTarget {
        target: PlannedTarget {
            path: target.path.clone(),
            owner,
            planned,
            entries,
            orphan: None,
        },
        unmeasured,
        unmeasured_more,
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
    let mut measure_gaps = Vec::new();
    let mut blocked = false;
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
                if let Some(planned) = plan_orphan(ctx, target, &mut findings, &mut measure_gaps) {
                    store_target(
                        &mut buckets,
                        PlanClass::Orphan,
                        planned,
                        &recorded,
                        days,
                        now,
                    );
                }
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
                TargetPlan::Unreadable(failed) => {
                    record_stat_failures(&mut findings, failed);
                    blocked = true;
                }
                TargetPlan::Planned(built) => {
                    push_gaps(&mut measure_gaps, built.unmeasured, built.unmeasured_more);
                    store_target(&mut buckets, class, built.target, &recorded, days, now);
                }
            },
        }
    }
    let plans: Vec<Plan> = if blocked {
        Vec::new()
    } else {
        buckets.into_iter().flatten().collect()
    };
    let mut unmeasured = measure_gaps;
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

fn plan_orphan(
    ctx: &Ctx,
    target: &TargetDir,
    findings: &mut Vec<(String, Option<PathBuf>, String)>,
    unmeasured: &mut Vec<Unmeasured>,
) -> Option<PlannedTarget> {
    if let Owner::OrphanCandidate {
        owned_under_caller_env: Some(var),
    } = &target.owner
    {
        findings.push((
            "plan.caller_env".to_string(),
            Some(target.path.clone()),
            var.clone(),
        ));
    }
    let stat = match ctx.fs.entry_stat(&target.path) {
        Ok(stat) => stat,
        Err(err) => {
            findings.push((
                "plan.unreadable".to_string(),
                Some(target.path.clone()),
                err.to_string(),
            ));
            return None;
        }
    };
    let kind = kind_name(stat.kind).to_string();
    let dirs =
        match crate::cargo_target::identify_bounded(ctx.fs, &target.path, crate::gc::CRATE_DIR_CAP)
        {
            Some(Ok(found)) => found.into_iter().map(|(dir, _)| dir).collect(),
            Some(Err(why)) => {
                findings.push((
                    "plan.unreadable".to_string(),
                    Some(target.path.clone()),
                    why,
                ));
                return None;
            }
            None => {
                findings.push((
                    "plan.unreadable".to_string(),
                    Some(target.path.clone()),
                    format!("{} is no longer a Cargo target dir", target.path.display()),
                ));
                return None;
            }
        };
    Some(PlannedTarget {
        path: target.path.clone(),
        owner: None,
        planned: measured_orphan(ctx, target, unmeasured),
        entries: vec![PlannedEntry {
            path: target.path.clone(),
            unit: target.path.display().to_string(),
            kind: kind.clone(),
            dev: stat.dev,
            ino: stat.ino,
        }],
        orphan: Some(OrphanScope {
            root_kind: kind,
            root_dev: stat.dev,
            root_ino: stat.ino,
            dirs,
        }),
    })
}

fn measured_orphan(
    ctx: &Ctx,
    target: &TargetDir,
    unmeasured: &mut Vec<Unmeasured>,
) -> Option<BytesRec> {
    if !ctx.account_bytes {
        return None;
    }
    let paths = [target.path.clone()];
    let foot = gc::measure(ctx.fs, &target.path, &paths, true);
    push_gaps(unmeasured, foot.unmeasured, foot.unmeasured_more);
    Some(BytesRec {
        freed: foot.bytes.freed,
        still_shared: foot.bytes.still_shared,
    })
}

fn push_gaps(unmeasured: &mut Vec<Unmeasured>, gaps: Vec<(PathBuf, String)>, more: u64) {
    for (path, reason) in gaps {
        unmeasured.push(Unmeasured {
            field: path.display().to_string(),
            reason,
        });
    }
    if more > 0 {
        unmeasured.push(Unmeasured {
            field: "unmeasured_paths".to_string(),
            reason: format!("{more} more paths could not be measured and are not listed"),
        });
    }
}

fn store_target(
    buckets: &mut [Option<Plan>; 3],
    class: PlanClass,
    planned: PlannedTarget,
    recorded: &[PathBuf],
    days: u64,
    now: u64,
) {
    let slot = class_index(class);
    if let Some(plan) = buckets[slot].as_mut() {
        add_target(plan, planned);
    } else {
        let mut plan = Plan::single(class, planned, days, now);
        plan.scope_repos = recorded.to_vec();
        buckets[slot] = Some(plan);
    }
}

fn record_stat_failures(
    findings: &mut Vec<(String, Option<PathBuf>, String)>,
    failed: Vec<(PathBuf, String)>,
) {
    for (path, detail) in failed {
        findings.push(("plan.unreadable".to_string(), Some(path), detail));
    }
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

    #[cfg(target_os = "macos")]
    struct FailStat {
        inner: Box<dyn crate::platform::Fs>,
        deny: fn(&Path) -> bool,
    }

    #[cfg(target_os = "macos")]
    impl crate::platform::Fs for FailStat {
        fn temp_dir(
            &self,
            parent: &Path,
            prefix: &str,
        ) -> std::io::Result<crate::platform::TempDir> {
            self.inner.temp_dir(parent, prefix)
        }
        fn write_file(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
            self.inner.write_file(path, bytes)
        }
        fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
            self.inner.read(path)
        }
        fn hardlink(&self, src: &Path, dst: &Path) -> std::io::Result<()> {
            self.inner.hardlink(src, dst)
        }
        fn set_mtime(&self, path: &Path, t: SystemTime) -> std::io::Result<()> {
            self.inner.set_mtime(path, t)
        }
        fn mtime(&self, path: &Path) -> std::io::Result<SystemTime> {
            self.inner.mtime(path)
        }
        fn inode(&self, path: &Path) -> std::io::Result<u64> {
            self.inner.inode(path)
        }
        fn clone_file(&self, src: &Path, dst: &Path) -> std::io::Result<()> {
            self.inner.clone_file(src, dst)
        }
        fn create_dir(&self, path: &Path) -> std::io::Result<()> {
            self.inner.create_dir(path)
        }
        fn sharing(&self, path: &Path) -> std::io::Result<crate::platform::Sharing> {
            self.inner.sharing(path)
        }
        fn compress(&self, path: &Path) -> crate::platform::CompressOutcome {
            self.inner.compress(path)
        }
        fn hold_lock(
            &self,
            kind: crate::platform::LockKind,
            path: &Path,
        ) -> std::io::Result<crate::platform::LockHolder> {
            self.inner.hold_lock(kind, path)
        }
        fn try_lock_exclusive(&self, path: &Path) -> crate::platform::TryLock {
            self.inner.try_lock_exclusive(path)
        }
        fn try_lock_elsewhere(
            &self,
            kind: crate::platform::LockKind,
            path: &Path,
        ) -> std::io::Result<crate::platform::LockState> {
            self.inner.try_lock_elsewhere(kind, path)
        }
        fn entry_stat(&self, path: &Path) -> std::io::Result<crate::platform::EntryStat> {
            if (self.deny)(path) {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("injected stat failure for {}", path.display()),
                ))
            } else {
                self.inner.entry_stat(path)
            }
        }
        fn read_dir_bounded(
            &self,
            dir: &Path,
            cap: usize,
        ) -> std::io::Result<Vec<(String, crate::platform::EntryKind)>> {
            self.inner.read_dir_bounded(dir, cap)
        }
        fn remove_tree(&self, path: &Path) -> std::io::Result<()> {
            self.inner.remove_tree(path)
        }
        fn remove_file(&self, path: &Path) -> std::io::Result<()> {
            self.inner.remove_file(path)
        }
        fn volume(&self, path: &Path) -> std::io::Result<crate::platform::VolumeInfo> {
            self.inner.volume(path)
        }
    }

    #[cfg(target_os = "macos")]
    struct ScratchDir(PathBuf);

    #[cfg(target_os = "macos")]
    impl Drop for ScratchDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(target_os = "macos")]
    fn planted_dead_unit(label: &str) -> (ScratchDir, TargetDir, PathBuf) {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = PathBuf::from(format!(
            "/private/tmp/hive-it-statfail-{label}-{}-{nanos}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let profile = root.join("debug");
        let unit = "fixture-4444444444444444";
        let unit_dir = profile.join(".fingerprint").join(unit);
        std::fs::create_dir_all(&unit_dir).unwrap();
        std::fs::write(unit_dir.join("lib-fixture"), "0000000000000068").unwrap();
        std::fs::write(
            unit_dir.join("lib-fixture.json"),
            r#"{"rustc":1,"features":"[]","declared_features":"[]","target":2,"profile":3,"path":4,"deps":[],"local":[],"rustflags":[],"config":5,"compile_kind":0}"#,
        )
        .unwrap();
        let stamp = unit_dir.join("invoked.timestamp");
        std::fs::write(&stamp, b"").unwrap();
        std::fs::File::open(&stamp)
            .unwrap()
            .set_modified(SystemTime::now() - Duration::from_secs(30 * 86_400))
            .unwrap();
        let artifact = profile
            .join("deps")
            .join("libfixture-4444444444444444.rlib");
        std::fs::create_dir_all(artifact.parent().unwrap()).unwrap();
        std::fs::write(&artifact, b"artifact").unwrap();
        let target = TargetDir {
            path: root.clone(),
            owner: Owner::Owned {
                checkout: root.clone(),
            },
            profiles: vec![crate::fingerprint::read_profile(&profile)],
            size: None,
            unreadable: None,
        };
        (ScratchDir(root), target, artifact)
    }

    #[cfg(target_os = "macos")]
    fn plan_with(target: &TargetDir, deny: fn(&Path) -> bool) -> TargetPlan {
        let state = crate::state::State::open_at(target.path.join("home")).unwrap();
        let cfg = crate::state::Config {
            secondary_roots: Vec::new(),
            cache_roots: Vec::new(),
            gc_window_days: None,
        };
        let fs = FailStat {
            inner: crate::platform::current().unwrap().fs,
            deny,
        };
        let ctx = Ctx {
            state: &state,
            fs: &fs,
            cfg: &cfg,
            now: SystemTime::now(),
            account_bytes: false,
            repos: &[],
            mains: &[],
        };
        plan_for_target(&ctx, target, Duration::from_secs(7 * 86_400))
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_listed_path_whose_stat_fails_is_not_nothing_to_do() {
        let (_scratch, target, artifact) = planted_dead_unit("all");
        let plan = plan_with(&target, |_| true);
        let TargetPlan::Unreadable(failed) = plan else {
            panic!("a listed path whose stat failed was reported as nothing to do");
        };
        assert!(
            failed.iter().any(|(path, _)| path == &artifact),
            "the artifact whose stat failed was not recorded: {failed:?}"
        );
        assert!(
            failed
                .iter()
                .any(|(path, _)| path.ends_with(".fingerprint/fixture-4444444444444444")),
            "the fingerprint dir whose stat failed was not recorded: {failed:?}"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_listed_path_whose_stat_fails_is_recorded_beside_the_entries_that_stat() {
        let (_scratch, target, artifact) = planted_dead_unit("one");
        let plan = plan_with(&target, |path| {
            path.extension().is_some_and(|ext| ext == "rlib")
        });
        let TargetPlan::Unreadable(failed) = plan else {
            panic!("a path that could not be stat'd was still planned");
        };
        assert!(
            failed.iter().any(|(path, _)| path == &artifact),
            "the artifact whose stat failed was not recorded: {failed:?}"
        );
    }
}
