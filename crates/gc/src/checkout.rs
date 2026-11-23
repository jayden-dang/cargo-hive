//! One checkout's dead-unit and unreferenced debug-object GC.

use crate::debug_map;
use crate::liveness::liveness;
use crate::plan_tier::guard::{self, Expect};
use cargo_hive_cargo::discover::{self, TargetScan};
use cargo_hive_core::caps::{CRATE_DIR_CAP, DEPS_CAP};
use cargo_hive_core::fingerprint::{self, Format, ProfileReport};
use cargo_hive_core::ledger::{self, Bytes, Ledger};
use cargo_hive_core::liveness::Liveness;
use cargo_hive_core::locks::{self, Acquired, Reason};
use cargo_hive_core::receipt::Finding;
use cargo_hive_core::target::{self, TargetDir};
use cargo_hive_core::unit_path;
use cargo_hive_platform::{EntryKind, Fs};
use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

pub struct CheckoutInput<'a> {
    pub fs: &'a dyn Fs,
    pub scan: &'a TargetScan,
    pub checkout: &'a Path,
    pub nested: &'a [PathBuf],
    pub now: SystemTime,
    pub window: Duration,
    pub account_bytes: bool,
}

#[derive(Debug)]
pub struct CheckoutReport {
    pub per_target: Vec<(PathBuf, Counts)>,
    pub totals: Counts,
    pub findings: Vec<Finding>,
    pub unmeasured: Vec<(PathBuf, String)>,
    pub unmeasured_more: u64,
    pub cancelled: bool,
    pub failed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RootRole {
    Target,
    Build,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootPresence {
    Present { dev: u64, ino: u64 },
    InitiallyAbsent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequiredRoot {
    pub path: PathBuf,
    pub role: RootRole,
    pub presence: RootPresence,
}

#[derive(Debug)]
pub struct EvidencedCheckoutReport {
    pub report: CheckoutReport,
    expected: Vec<RequiredRoot>,
    completed: Vec<RequiredRoot>,
}

impl EvidencedCheckoutReport {
    pub fn expected(&self) -> &[RequiredRoot] {
        &self.expected
    }

    pub fn completed(&self) -> &[RequiredRoot] {
        &self.completed
    }

    pub fn complete_roots(&self) -> Option<&[RequiredRoot; 2]> {
        if self.report.failed || self.report.cancelled {
            return None;
        }
        self.completed.as_slice().try_into().ok()
    }
}

#[derive(Debug)]
pub struct Counts {
    pub units_deleted: u64,
    pub unit_files_deleted: u64,
    pub objects_deleted: u64,
    pub objects_referenced: u64,
    pub binaries_read: u64,
    pub targets_shared: u64,
    pub targets_outside: u64,
    pub bytes: Option<Bytes>,
    pub skips: BTreeMap<&'static str, u64>,
}

pub struct Selection {
    pub own: Vec<PathBuf>,
    pub shared: Vec<PathBuf>,
    pub outside: Vec<PathBuf>,
    pub failed: Vec<(PathBuf, String)>,
}

impl Counts {
    fn empty() -> Counts {
        Counts {
            units_deleted: 0,
            unit_files_deleted: 0,
            objects_deleted: 0,
            objects_referenced: 0,
            binaries_read: 0,
            targets_shared: 0,
            targets_outside: 0,
            bytes: None,
            skips: BTreeMap::new(),
        }
    }

    fn add_target(&mut self, other: &Counts) {
        self.units_deleted += other.units_deleted;
        self.unit_files_deleted += other.unit_files_deleted;
        self.objects_deleted += other.objects_deleted;
        self.objects_referenced += other.objects_referenced;
        self.binaries_read += other.binaries_read;
        self.targets_shared += other.targets_shared;
        self.targets_outside += other.targets_outside;
        for (name, n) in &other.skips {
            *self.skips.entry(*name).or_insert(0) += *n;
        }
        if let Some(bytes) = other.bytes {
            let slot = self.bytes.get_or_insert(Bytes::default());
            slot.freed = slot.freed.saturating_add(bytes.freed);
            slot.still_shared = slot.still_shared.saturating_add(bytes.still_shared);
        }
    }
}

pub fn unit_order(files: &[PathBuf]) -> Vec<PathBuf> {
    if files
        .iter()
        .any(|path| unit_path::unit_dir_of(path).is_some())
    {
        return files.to_vec();
    }
    let mut deps = Vec::new();
    let mut others = Vec::new();
    let mut prints = Vec::new();
    for path in files {
        if is_dep_info(path) {
            deps.push(path.clone());
        } else if is_fingerprint_dir(path) {
            prints.push(path.clone());
        } else {
            others.push(path.clone());
        }
    }
    if deps.is_empty() && others.is_empty() {
        return Vec::new();
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
    group
}

pub fn select_targets(scan: &TargetScan, checkout: &Path, nested: &[PathBuf]) -> Selection {
    let failed = discover::failed_workspaces(scan);
    let mut own = Vec::new();
    let mut shared = Vec::new();
    let mut outside = Vec::new();
    let mut seen = BTreeSet::new();
    let mut targets: Vec<&Path> = scan.targets.iter().map(|t| t.path.as_path()).collect();
    targets.sort();
    for path in targets {
        if !seen.insert(path.to_path_buf()) {
            continue;
        }
        let resolvers = discover::resolvers_of(scan, path);
        if !resolvers.iter().any(|r| r == checkout) {
            continue;
        }
        let others: Vec<_> = resolvers.into_iter().filter(|r| r != checkout).collect();
        if !others.is_empty() {
            shared.push(path.to_path_buf());
            continue;
        }
        if !path.starts_with(checkout) || nested.iter().any(|n| path.starts_with(n)) {
            outside.push(path.to_path_buf());
            continue;
        }
        own.push(path.to_path_buf());
    }
    Selection {
        own,
        shared,
        outside,
        failed,
    }
}

pub fn run(input: &CheckoutInput, cancel: &dyn Fn() -> bool) -> CheckoutReport {
    let selected = select_targets(input.scan, input.checkout, input.nested);
    let mut report = selection_report(input, &selected);
    if report.failed {
        return report;
    }
    for path in &selected.own {
        if cancel() {
            report.cancelled = true;
            break;
        }
        let (counts, _) = clean_target(input, path, None, cancel, &mut report);
        report.totals.add_target(&counts);
        report.per_target.push((path.clone(), counts));
        if report.cancelled {
            break;
        }
    }
    report
}

/// Proves both Cargo output roles against fresh discovery. A shared path is
/// cleaned once and earns evidence for both roles only when their identities agree.
pub fn run_with_evidence(
    input: &CheckoutInput<'_>,
    expected: &[RequiredRoot],
    cancel: &dyn Fn() -> bool,
) -> EvidencedCheckoutReport {
    let selected = select_targets(input.scan, input.checkout, input.nested);
    let mut result = EvidencedCheckoutReport {
        report: selection_report(input, &selected),
        expected: expected.to_vec(),
        completed: Vec::new(),
    };
    let report = &mut result.report;
    if cancel() {
        report.cancelled = true;
        return result;
    }
    if report.failed {
        return result;
    }
    if input.scan.metadata_cache_hits > 0 {
        incomplete_root(
            report,
            None,
            "checkout GC evidence requires uncached Cargo metadata",
        );
        return result;
    }
    for fault in &input.scan.walk_faults {
        report.findings.push(Finding {
            kind: "walk.unreadable".into(),
            path: Some(fault.path.clone()),
            detail: fault.detail.clone(),
            measured: BTreeMap::new(),
        });
    }
    if !input.scan.walk_faults.is_empty() {
        report.failed = true;
        return result;
    }
    let Some(roots) = required_roots(input, &selected, expected, report) else {
        return result;
    };
    for same_path in roots.chunk_by(|a, b| a.path == b.path) {
        if cancel() {
            report.cancelled = true;
            break;
        }
        let root = same_path[0];
        if root.presence == RootPresence::InitiallyAbsent {
            continue;
        }
        let (counts, complete) = clean_target(input, &root.path, Some(root), cancel, report);
        report.totals.add_target(&counts);
        report.per_target.push((root.path.clone(), counts));
        if complete {
            result
                .completed
                .extend(same_path.iter().map(|&root| root.clone()));
        }
        if report.cancelled {
            break;
        }
    }
    if !report.cancelled {
        report.cancelled = cancel();
    }
    if !report.cancelled {
        for root in roots {
            if root.presence == RootPresence::InitiallyAbsent && check_root(input, root, report) {
                result.completed.push(root.clone());
            }
        }
    }
    result.completed.sort_by_key(|root| root.role);
    result
}

fn required_roots<'a>(
    input: &CheckoutInput<'_>,
    selection: &Selection,
    expected: &'a [RequiredRoot],
    report: &mut CheckoutReport,
) -> Option<[&'a RequiredRoot; 2]> {
    let [first, second] = expected else {
        incomplete_root(
            report,
            None,
            "expected exactly one target and one build root",
        );
        return None;
    };
    let mut roots = [first, second];
    roots.sort_by_key(|root| root.role);
    let [target, build] = roots;
    if target.role != RootRole::Target
        || build.role != RootRole::Build
        || (target.path == build.path && target.presence != build.presence)
        || (target.path != build.path
            && (target.path.starts_with(&build.path) || build.path.starts_with(&target.path)))
    {
        incomplete_root(
            report,
            None,
            "the required root roles or identities conflict",
        );
        return None;
    }
    let resolves_pair = input.scan.workspaces.iter().any(|workspace| {
        workspace.checkout == input.checkout
            && workspace.result.as_ref().is_ok_and(|resolved| {
                resolved.workspace_root.starts_with(input.checkout)
                    && resolved.target_directory == target.path
                    && resolved.build_directory == build.path
            })
    });
    if !resolves_pair {
        incomplete_root(
            report,
            None,
            "fresh workspace metadata did not resolve the required root roles",
        );
        return None;
    }
    for root in roots {
        let path = &root.path;
        if path == input.checkout
            || !path.starts_with(input.checkout)
            || input
                .nested
                .iter()
                .any(|nested| path.starts_with(nested) || nested.starts_with(path))
            || discover::resolvers_of(input.scan, path) != [input.checkout]
        {
            incomplete_root(
                report,
                Some(path),
                "the required root does not have exclusive checkout scope",
            );
            return None;
        }
        if matches!(root.presence, RootPresence::Present { .. })
            && (!selection.own.contains(path)
                || !input.scan.targets.iter().any(|row| {
                    row.path == *path
                        && row.unreadable.is_none()
                        && matches!(&row.owner, target::Owner::Owned { checkout } if checkout == input.checkout)
                }))
        {
            incomplete_root(report, Some(path), "the required root was not discovered as a supported owned Cargo root");
            return None;
        }
        if !check_root(input, root, report) {
            return None;
        }
    }
    roots.sort_by(|a, b| a.path.cmp(&b.path));
    Some(roots)
}

fn check_root(input: &CheckoutInput<'_>, root: &RequiredRoot, report: &mut CheckoutReport) -> bool {
    if matches_presence(input, root) {
        return true;
    }
    report.failed = true;
    report.findings.push(root_identity_finding(&root.path));
    false
}

const ROOT_IDENTITY_CHANGED: &str =
    "the required root identity or initial absence no longer matches";

fn root_identity_finding(path: &Path) -> Finding {
    incomplete_finding(Some(path), ROOT_IDENTITY_CHANGED)
}

fn matches_presence(input: &CheckoutInput<'_>, root: &RequiredRoot) -> bool {
    if input.checkout.canonicalize().as_deref().ok() != Some(input.checkout) {
        return false;
    }
    match root.presence {
        RootPresence::Present { dev, ino } => {
            root.path.canonicalize().as_ref().ok() == Some(&root.path)
                && input.fs.entry_stat(&root.path).is_ok_and(|stat| {
                    stat.kind == EntryKind::Dir && stat.dev == dev && stat.ino == ino
                })
        }
        RootPresence::InitiallyAbsent => {
            let Ok(relative) = root.path.strip_prefix(input.checkout) else {
                return false;
            };
            if !relative
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
            {
                return false;
            }
            let mut path = input.checkout.to_path_buf();
            for component in relative.components() {
                path.push(component);
                match input.fs.entry_stat(&path) {
                    Ok(stat) if stat.kind == EntryKind::Dir => (),
                    Err(err) if is_not_found(&err) => return true,
                    _ => return false,
                }
            }
            false
        }
    }
}

fn incomplete_root(report: &mut CheckoutReport, path: Option<&Path>, detail: &str) {
    report.failed = true;
    report.findings.push(incomplete_finding(path, detail));
}

fn incomplete_finding(path: Option<&Path>, detail: &str) -> Finding {
    Finding {
        kind: "gc.root_incomplete".into(),
        path: path.map(Path::to_path_buf),
        detail: detail.into(),
        measured: BTreeMap::new(),
    }
}

fn selection_report(input: &CheckoutInput<'_>, selected: &Selection) -> CheckoutReport {
    let mut report = CheckoutReport {
        per_target: Vec::new(),
        totals: Counts::empty(),
        findings: Vec::new(),
        unmeasured: Vec::new(),
        unmeasured_more: 0,
        cancelled: false,
        failed: false,
    };
    if !selected.failed.is_empty() {
        for (dir, err) in &selected.failed {
            report.findings.push(Finding {
                kind: "ownership.unresolved".into(),
                path: Some(dir.clone()),
                detail: format!(
                    "{err}; clears when that workspace's cargo metadata succeeds, or its worktree is removed"
                ),
                measured: BTreeMap::new(),
            });
        }
        report.failed = true;
        return report;
    }
    report.totals.targets_shared = selected.shared.len() as u64;
    report.totals.targets_outside = selected.outside.len() as u64;
    for path in &selected.shared {
        let resolvers = discover::resolvers_of(input.scan, path);
        let others: Vec<_> = resolvers
            .into_iter()
            .filter(|r| r != input.checkout)
            .map(|p| p.display().to_string())
            .collect();
        report.findings.push(Finding {
            kind: "gc.shared_target".into(),
            path: Some(path.clone()),
            detail: format!("also resolved by {}", others.join(", ")),
            measured: BTreeMap::new(),
        });
    }
    for path in &selected.outside {
        report.findings.push(Finding {
            kind: "gc.outside_checkout".into(),
            path: Some(path.clone()),
            detail: "target lies outside the checkout or inside a nested checkout".into(),
            measured: BTreeMap::new(),
        });
    }
    report
}

enum CheckoutLocks {
    Standalone(locks::Locked),
    Verified(locks::VerifiedLocks),
}

fn acquire_checkout_locks(
    fs: &dyn Fs,
    path: &Path,
    strict: bool,
    cancel: &dyn Fn() -> bool,
) -> Result<CheckoutLocks, locks::VerifyAcquireError> {
    use locks::VerifyAcquireError::{Failed, Skipped};

    if strict {
        return locks::acquire_verified(fs, path, cancel).map(CheckoutLocks::Verified);
    }
    match locks::acquire(fs, path, true) {
        Acquired::Locked(locked) => Ok(CheckoutLocks::Standalone(locked)),
        Acquired::Skipped(reason) => Err(Skipped(reason)),
        Acquired::Failed(why) => Err(Failed(why)),
    }
}

fn clean_target(
    input: &CheckoutInput,
    path: &Path,
    required: Option<&RequiredRoot>,
    cancel: &dyn Fn() -> bool,
    report: &mut CheckoutReport,
) -> (Counts, bool) {
    let mut counts = Counts::empty();
    let locked = match acquire_checkout_locks(input.fs, path, required.is_some(), cancel) {
        Ok(locked) => locked,
        Err(locks::VerifyAcquireError::Skipped(reason)) => {
            skip_target(&mut counts, report, path, reason);
            return (counts, false);
        }
        Err(locks::VerifyAcquireError::Failed(why)) => {
            report.failed = true;
            report.findings.push(Finding {
                kind: "gc.target_failed".into(),
                path: Some(path.to_path_buf()),
                detail: why,
                measured: BTreeMap::new(),
            });
            return (counts, false);
        }
    };
    if required.is_some_and(|root| !check_root(input, root, report)) {
        return (counts, false);
    }
    let mut complete = true;
    let found = match &locked {
        CheckoutLocks::Standalone(standalone) => {
            let found = match target::identify_bounded(input.fs, path, CRATE_DIR_CAP) {
                Some(Ok(found)) => found,
                Some(Err(why)) => {
                    report.failed = true;
                    report.findings.push(Finding {
                        kind: "gc.target_failed".into(),
                        path: Some(path.to_path_buf()),
                        detail: why,
                        measured: BTreeMap::new(),
                    });
                    return (counts, false);
                }
                None => {
                    skip_target(&mut counts, report, path, Reason::ProfileSet);
                    return (counts, false);
                }
            };
            let first: BTreeSet<_> = standalone.profiles.iter().cloned().collect();
            let again: BTreeSet<_> = found.iter().map(|(p, _)| p.clone()).collect();
            if first != again {
                skip_target(&mut counts, report, path, Reason::ProfileSet);
                return (counts, false);
            }
            found
        }
        CheckoutLocks::Verified(verified) => {
            match target::identify_strict(input.fs, path, CRATE_DIR_CAP, cancel).and_then(|found| {
                verified.verify_profiles(input.fs, &found)?;
                Ok(found)
            }) {
                Ok(found) => found,
                Err(why) => {
                    incomplete_root(report, Some(path), &why);
                    return (counts, false);
                }
            }
        }
    };
    let verify_scope = || -> Result<(), String> {
        if let (Some(root), CheckoutLocks::Verified(verified)) = (required, &locked) {
            if cancel() {
                return Err("checkout GC was cancelled".into());
            }
            if !matches_presence(input, root) {
                return Err(ROOT_IDENTITY_CHANGED.into());
            }
            verified.verify_current(input.fs)?;
        }
        Ok(())
    };
    if let Err(why) = verify_scope() {
        incomplete_root(report, Some(path), &why);
        return (counts, false);
    }
    let reports: Vec<ProfileReport> = if required.is_some() {
        let reader = fingerprint::StrictProfileReader::new(input.fs, cancel);
        found.iter().map(|(p, _)| reader.read_profile(p)).collect()
    } else {
        found
            .iter()
            .map(|(p, _)| fingerprint::read_profile(p))
            .collect()
    };
    let owner = input
        .scan
        .targets
        .iter()
        .find(|t| t.path == path)
        .map(|t| t.owner.clone())
        .unwrap_or_else(|| cargo_hive_core::target::Owner::Owned {
            checkout: input.checkout.to_path_buf(),
        });
    let fresh = TargetDir {
        path: path.to_path_buf(),
        owner,
        profiles: reports.clone(),
        size: None,
        unreadable: None,
    };
    let mut ledger = Ledger::new();
    match liveness(input.fs, &fresh, input.now, input.window) {
        Liveness::Undecided { reasons } => {
            complete = false;
            for reason in reasons {
                report.findings.push(Finding {
                    kind: "gc.undecided".into(),
                    path: Some(path.to_path_buf()),
                    detail: format!("{reason} — clears when {}", reason.clears_when()),
                    measured: BTreeMap::new(),
                });
            }
        }
        Liveness::Decided { dead, .. } => {
            'units: for unit in dead {
                let ordered = unit_order(&unit.files);
                if ordered.is_empty() {
                    continue;
                }
                if cancel() {
                    report.cancelled = true;
                    break;
                }
                let mut stats = Vec::new();
                for entry in &ordered {
                    match input.fs.entry_stat(entry) {
                        Ok(stat) if stat.kind != EntryKind::Other => stats.push((entry, stat)),
                        Ok(_) | Err(_) => {
                            complete = false;
                            report.failed = true;
                            report.findings.push(Finding {
                                kind: "gc.delete_failed".into(),
                                path: Some(entry.clone()),
                                detail: format!("could not stat {}", entry.display()),
                                measured: BTreeMap::new(),
                            });
                            break 'units;
                        }
                    }
                }
                let mut deleted_files = 0u64;
                let removed_all = 'remove: {
                    for (entry, stat) in &stats {
                        let expect = Expect::Entry {
                            kind: stat.kind,
                            dev: stat.dev,
                            ino: stat.ino,
                        };
                        if let Err(reason) = guard::check_entry(input.fs, path, entry, expect) {
                            report.failed = true;
                            report.findings.push(Finding {
                                kind: "gc.delete_failed".into(),
                                path: Some((*entry).clone()),
                                detail: format!("{} ({reason:?})", entry.display()),
                                measured: BTreeMap::new(),
                            });
                            break 'remove false;
                        }
                        if required.is_some_and(|root| !check_root(input, root, report)) {
                            break 'remove false;
                        }
                        let removed = if required.is_some() {
                            ledger::remove_counted_guarded(
                                input.fs,
                                &mut ledger,
                                path,
                                entry,
                                input.account_bytes,
                                &|| {
                                    removal_identity(
                                        input.fs,
                                        path,
                                        entry,
                                        Expect::Entry {
                                            kind: stat.kind,
                                            dev: stat.dev,
                                            ino: stat.ino,
                                        },
                                        &verify_scope,
                                    )
                                },
                            )
                        } else {
                            ledger::remove_counted(
                                input.fs,
                                &mut ledger,
                                path,
                                entry,
                                input.account_bytes,
                                false,
                            )
                        };
                        match removed {
                            Ok(removed) if removed.existed => {
                                if unit_path::unit_dir_of(entry).is_some() {
                                    deleted_files = deleted_files.saturating_add(removed.files);
                                } else {
                                    deleted_files = deleted_files.saturating_add(1);
                                }
                            }
                            Ok(_) => {}
                            Err(err) => {
                                report.failed = true;
                                report.findings.push(Finding {
                                    kind: "gc.delete_failed".into(),
                                    path: Some((*entry).clone()),
                                    detail: err.to_string(),
                                    measured: BTreeMap::new(),
                                });
                                break 'remove false;
                            }
                        }
                    }
                    true
                };
                if removed_all && deleted_files > 0 {
                    counts.units_deleted += 1;
                }
                if removed_all || required.is_some() {
                    counts.unit_files_deleted += deleted_files;
                }
                if !removed_all {
                    complete = false;
                    break;
                }
            }
        }
    }
    if !report.cancelled && (required.is_none() || complete) {
        for ((profile, _), profile_report) in found.iter().zip(&reports) {
            let outcome = clean_profile_objects(ObjectsClean {
                fs: input.fs,
                target: path,
                profile,
                profile_report,
                account_bytes: input.account_bytes,
                cancel,
                scope_valid: required.map(|_| &verify_scope as &dyn Fn() -> Result<(), String>),
                ledger: &mut ledger,
            });
            counts.binaries_read = counts.binaries_read.saturating_add(outcome.binaries_read);
            counts.objects_referenced = counts
                .objects_referenced
                .saturating_add(outcome.objects_referenced);
            counts.objects_deleted = counts
                .objects_deleted
                .saturating_add(outcome.objects_deleted);
            complete &= outcome.completed;
            for finding in outcome.findings {
                report.findings.push(finding);
            }
            if outcome.failed {
                report.failed = true;
            }
            if outcome.cancelled {
                report.cancelled = true;
                break;
            }
        }
    }
    let tally = ledger.finish();
    for (p, why) in tally.unmeasured {
        report.unmeasured.push((p, why));
    }
    report.unmeasured_more = report.unmeasured_more.saturating_add(tally.unmeasured_more);
    if input.account_bytes {
        counts.bytes = Some(tally.per_target.get(path).copied().unwrap_or(tally.total));
    }
    if let CheckoutLocks::Verified(verified) = &locked {
        report.cancelled |= cancel();
        if complete
            && !report.cancelled
            && let Err(why) = target::identify_strict(input.fs, path, CRATE_DIR_CAP, cancel)
                .and_then(|profiles| verified.verify_profiles(input.fs, &profiles))
                .and_then(|()| verify_scope())
        {
            incomplete_root(report, Some(path), &why);
            complete = false;
        }
        complete &= !report.cancelled;
    }
    drop(locked);
    (counts, complete)
}

struct ObjectsClean<'a> {
    fs: &'a dyn Fs,
    target: &'a Path,
    profile: &'a Path,
    profile_report: &'a ProfileReport,
    account_bytes: bool,
    cancel: &'a dyn Fn() -> bool,
    scope_valid: Option<&'a dyn Fn() -> Result<(), String>>,
    ledger: &'a mut Ledger,
}

struct ObjectsOutcome {
    completed: bool,
    binaries_read: u64,
    objects_referenced: u64,
    objects_deleted: u64,
    findings: Vec<Finding>,
    failed: bool,
    cancelled: bool,
}

fn clean_profile_objects(ctx: ObjectsClean<'_>) -> ObjectsOutcome {
    let mut out = ObjectsOutcome {
        completed: false,
        binaries_read: 0,
        objects_referenced: 0,
        objects_deleted: 0,
        findings: Vec::new(),
        failed: false,
        cancelled: false,
    };
    if let Format::Unsupported(why) = &ctx.profile_report.format {
        out.findings.push(objects_finding(
            ctx.profile,
            format!("fingerprint format unsupported: {why}"),
        ));
        return out;
    }
    let listing = match list_profile(ctx.fs, ctx.profile, DEPS_CAP, CRATE_DIR_CAP) {
        Ok(listing) => listing,
        Err(why) => {
            out.findings.push(objects_finding(ctx.profile, why));
            return out;
        }
    };
    if listing.objects.is_empty()
        && listing.candidates.is_empty()
        && !dir_present(ctx.fs, &ctx.profile.join("deps"))
        && !listing.saw_unit_dir
    {
        out.completed = true;
        return out;
    }
    let mut loose = BTreeSet::new();
    let mut oso_entries = 0u64;
    for path in &listing.candidates {
        match debug_map::read(ctx.fs, path) {
            Ok(Some(map)) => {
                out.binaries_read += 1;
                oso_entries = oso_entries
                    .saturating_add(map.loose.len() as u64)
                    .saturating_add(map.archive_entries);
                loose.extend(map.loose);
            }
            Ok(None) => {}
            Err(err) => {
                out.findings.push(objects_finding(path, err.to_string()));
                return out;
            }
        }
    }
    if out.binaries_read >= 1 && !listing.objects.is_empty() && oso_entries == 0 {
        out.findings.push(objects_finding(
            ctx.profile,
            "linked binaries and loose objects are present but no N_OSO debug-map entry was found"
                .into(),
        ));
        return out;
    }
    for (object, dev, ino) in &listing.objects {
        let name = object
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if loose.contains(name) {
            out.objects_referenced = out.objects_referenced.saturating_add(1);
            continue;
        }
        if (ctx.cancel)() {
            out.cancelled = true;
            return out;
        }
        if let Err(reason) = guard::check_entry(
            ctx.fs,
            ctx.target,
            object,
            Expect::Entry {
                kind: EntryKind::File,
                dev: *dev,
                ino: *ino,
            },
        ) {
            out.failed = true;
            out.findings.push(Finding {
                kind: "gc.delete_failed".into(),
                path: Some(object.clone()),
                detail: format!("{} ({reason:?})", object.display()),
                measured: BTreeMap::new(),
            });
            return out;
        }
        let removed = if let Some(scope_valid) = ctx.scope_valid {
            ledger::remove_counted_guarded(
                ctx.fs,
                ctx.ledger,
                ctx.target,
                object,
                ctx.account_bytes,
                &|| {
                    removal_identity(
                        ctx.fs,
                        ctx.target,
                        object,
                        Expect::Entry {
                            kind: EntryKind::File,
                            dev: *dev,
                            ino: *ino,
                        },
                        scope_valid,
                    )
                },
            )
        } else {
            ledger::remove_counted(
                ctx.fs,
                ctx.ledger,
                ctx.target,
                object,
                ctx.account_bytes,
                false,
            )
        };
        match removed {
            Ok(removed) if removed.existed => {
                out.objects_deleted = out.objects_deleted.saturating_add(1);
            }
            Ok(_) => {}
            Err(err) => {
                out.failed = true;
                out.findings.push(Finding {
                    kind: "gc.delete_failed".into(),
                    path: Some(object.clone()),
                    detail: err.to_string(),
                    measured: BTreeMap::new(),
                });
                return out;
            }
        }
    }
    out.completed = true;
    out
}

fn removal_identity(
    fs: &dyn Fs,
    root: &Path,
    path: &Path,
    expect: Expect,
    scope_valid: &dyn Fn() -> Result<(), String>,
) -> Result<(), String> {
    let entry = guard::check_entry(fs, root, path, expect);
    scope_valid()?;
    entry
        .map(|_| ())
        .map_err(|reason| format!("{} ({reason:?})", path.display()))
}

fn objects_finding(path: &Path, detail: String) -> Finding {
    Finding {
        kind: "gc.objects_undecided".into(),
        path: Some(path.to_path_buf()),
        detail,
        measured: BTreeMap::new(),
    }
}

fn dir_present(fs: &dyn Fs, path: &Path) -> bool {
    matches!(fs.entry_stat(path), Ok(stat) if stat.kind == EntryKind::Dir)
}

fn is_not_found(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::NotFound
}

fn skip_target(counts: &mut Counts, report: &mut CheckoutReport, path: &Path, reason: Reason) {
    let name = reason.count_name();
    *counts.skips.entry(name).or_insert(0) += 1;
    report.findings.push(Finding {
        kind: "gc.skipped".into(),
        path: Some(path.to_path_buf()),
        detail: reason_text(reason),
        measured: BTreeMap::new(),
    });
}

fn reason_text(reason: Reason) -> String {
    match reason {
        Reason::Held => "a Cargo lock is held".into(),
        Reason::Unopenable => "a Cargo lock could not be opened".into(),
        Reason::Unlockable => "a Cargo lock could not be locked".into(),
        Reason::Missing => "a required Cargo lock is missing".into(),
        Reason::ProfileSet => "the profile set changed while locking".into(),
    }
}

fn is_dep_info(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "d")
}

fn is_fingerprint_dir(path: &Path) -> bool {
    unit_path::is_fingerprint_dir(path)
}

#[derive(Debug)]
pub struct Listing {
    pub objects: Vec<(PathBuf, u64, u64)>,
    pub candidates: Vec<PathBuf>,
    pub saw_unit_dir: bool,
}

pub fn list_profile(
    fs: &dyn Fs,
    profile: &Path,
    deps_cap: usize,
    dir_cap: usize,
) -> Result<Listing, String> {
    let deps = profile.join("deps");
    let deps_entries = match fs.read_dir_bounded(&deps, deps_cap) {
        Ok(entries) => Some(entries),
        Err(err) if is_not_found(&err) => None,
        Err(err) => {
            return Err(format!("{}: {err}", deps.display()));
        }
    };
    let mut objects = Vec::new();
    let mut candidates = Vec::new();
    if let Some(deps_entries) = deps_entries {
        for (name, kind) in deps_entries {
            let path = deps.join(&name);
            if name.ends_with(".rcgu.o") && kind == EntryKind::File {
                let stat = fs
                    .entry_stat(&path)
                    .map_err(|err| format!("{}: {err}", path.display()))?;
                if stat.kind == EntryKind::File {
                    objects.push((path.clone(), stat.dev, stat.ino));
                }
            }
            if !name.ends_with(".o") && is_candidate_kind(kind) {
                candidates.push(path);
            }
        }
    } else if !profile_has_unit_dir(fs, profile, dir_cap)? {
        return Ok(Listing {
            objects: Vec::new(),
            candidates: Vec::new(),
            saw_unit_dir: false,
        });
    }
    push_dir_candidates(fs, profile, deps_cap, &mut candidates)?;
    let examples = profile.join("examples");
    match fs.read_dir_bounded(&examples, deps_cap) {
        Ok(entries) => {
            for (name, kind) in entries {
                if !name.ends_with(".o") && is_candidate_kind(kind) {
                    candidates.push(examples.join(name));
                }
            }
        }
        Err(err) if is_not_found(&err) => {}
        Err(err) => return Err(format!("{}: {err}", examples.display())),
    }
    let mut saw_unit_dir = false;
    let build = profile.join("build");
    match fs.read_dir_bounded(&build, dir_cap) {
        Ok(entries) => {
            for (name, kind) in entries {
                if kind != EntryKind::Dir {
                    if !name.ends_with(".o") && is_candidate_kind(kind) {
                        candidates.push(build.join(&name));
                    }
                    continue;
                }
                let child = build.join(&name);
                let child_entries = fs
                    .read_dir_bounded(&child, dir_cap)
                    .map_err(|err| format!("{}: {err}", child.display()))?;
                for (child_name, child_kind) in child_entries {
                    if child_kind == EntryKind::Dir
                        && fingerprint::UnitHash::parse(&child_name).is_some()
                    {
                        saw_unit_dir = true;
                        let loc = unit_path::UnitLoc {
                            layout: unit_path::UnitLayout::UnitDir,
                            name_meta: format!("{name}-{child_name}"),
                        };
                        let out_dir = unit_path::outputs(profile, &loc);
                        push_out_listing(fs, &out_dir, deps_cap, &mut objects, &mut candidates)?;
                        continue;
                    }
                    if !child_name.ends_with(".o") && is_candidate_kind(child_kind) {
                        candidates.push(child.join(child_name));
                    }
                }
            }
        }
        Err(err) if is_not_found(&err) => {}
        Err(err) => return Err(format!("{}: {err}", build.display())),
    }
    Ok(Listing {
        objects,
        candidates,
        saw_unit_dir,
    })
}

fn profile_has_unit_dir(fs: &dyn Fs, profile: &Path, dir_cap: usize) -> Result<bool, String> {
    let build = profile.join("build");
    let packages = match fs.read_dir_bounded(&build, dir_cap) {
        Ok(entries) => entries,
        Err(err) if is_not_found(&err) => return Ok(false),
        Err(err) => return Err(format!("{}: {err}", build.display())),
    };
    for (name, kind) in packages {
        if kind != EntryKind::Dir {
            continue;
        }
        let pkg = build.join(name);
        let children = fs
            .read_dir_bounded(&pkg, dir_cap)
            .map_err(|err| format!("{}: {err}", pkg.display()))?;
        if children.iter().any(|(child, child_kind)| {
            *child_kind == EntryKind::Dir && fingerprint::UnitHash::parse(child).is_some()
        }) {
            return Ok(true);
        }
    }
    Ok(false)
}

fn push_out_listing(
    fs: &dyn Fs,
    out_dir: &Path,
    deps_cap: usize,
    objects: &mut Vec<(PathBuf, u64, u64)>,
    candidates: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let entries = match fs.read_dir_bounded(out_dir, deps_cap) {
        Ok(entries) => entries,
        Err(err) if is_not_found(&err) => return Ok(()),
        Err(err) => return Err(format!("{}: {err}", out_dir.display())),
    };
    for (name, kind) in entries {
        let path = out_dir.join(&name);
        if name.ends_with(".rcgu.o") && kind == EntryKind::File {
            let stat = fs
                .entry_stat(&path)
                .map_err(|err| format!("{}: {err}", path.display()))?;
            if stat.kind == EntryKind::File {
                objects.push((path.clone(), stat.dev, stat.ino));
            }
            continue;
        }
        if !name.ends_with(".o") && is_candidate_kind(kind) {
            candidates.push(path);
        }
    }
    Ok(())
}

fn is_candidate_kind(kind: EntryKind) -> bool {
    matches!(kind, EntryKind::File | EntryKind::Symlink)
}

fn push_dir_candidates(
    fs: &dyn Fs,
    dir: &Path,
    cap: usize,
    candidates: &mut Vec<PathBuf>,
) -> Result<(), String> {
    let entries = fs
        .read_dir_bounded(dir, cap)
        .map_err(|err| format!("{}: {err}", dir.display()))?;
    for (name, kind) in entries {
        if !name.ends_with(".o") && is_candidate_kind(kind) {
            candidates.push(dir.join(name));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{CheckoutInput, list_profile, select_targets, unit_order};
    use cargo_hive_cargo::discover::{TargetScan, WorkspaceResolution};
    use cargo_hive_core::target::{Owner, TAG_LINE, TargetDir};
    use cargo_hive_platform::{self as platform, EntryKind, EntryStat, Fs};
    use std::cell::Cell;
    use std::collections::BTreeSet;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn scan(
        targets: Vec<(&str, Owner)>,
        workspaces: Vec<(&str, &str, Result<&str, &str>)>,
    ) -> TargetScan {
        TargetScan {
            targets: targets
                .into_iter()
                .map(|(path, owner)| TargetDir {
                    path: PathBuf::from(path),
                    owner,
                    profiles: Vec::new(),
                    size: None,
                    unreadable: None,
                })
                .collect(),
            metadata_calls: 0,
            metadata_cache_hits: 0,
            ignored_vars: Vec::new(),
            walk_faults: Vec::new(),
            cleared_profiles: BTreeSet::new(),
            workspaces: workspaces
                .into_iter()
                .map(|(dir, checkout, result)| WorkspaceResolution {
                    dir: PathBuf::from(dir),
                    checkout: PathBuf::from(checkout),
                    result: match result {
                        Ok(target) => Ok(cargo_hive_cargo::metadata::Resolved {
                            workspace_root: PathBuf::from(dir),
                            target_directory: PathBuf::from(target),
                            build_directory: PathBuf::from(target),
                        }),
                        Err(err) => Err(err.to_string()),
                    },
                })
                .collect(),
        }
    }

    #[test]
    fn unit_entries_are_ordered_dep_info_first_and_fingerprint_dir_last() {
        let files = [
            PathBuf::from("/t/debug/.fingerprint/u-aaaaaaaaaaaaaaaa"),
            PathBuf::from("/t/debug/deps/libu-aaaaaaaaaaaaaaaa.rlib"),
            PathBuf::from("/t/debug/deps/u-aaaaaaaaaaaaaaaa.d"),
            PathBuf::from("/t/debug/deps/u-aaaaaaaaaaaaaaaa.dSYM"),
        ];
        let ordered = unit_order(&files);
        assert_eq!(
            ordered,
            vec![
                PathBuf::from("/t/debug/deps/u-aaaaaaaaaaaaaaaa.d"),
                PathBuf::from("/t/debug/deps/libu-aaaaaaaaaaaaaaaa.rlib"),
                PathBuf::from("/t/debug/deps/u-aaaaaaaaaaaaaaaa.dSYM"),
                PathBuf::from("/t/debug/.fingerprint/u-aaaaaaaaaaaaaaaa"),
            ]
        );
    }

    #[test]
    fn a_unit_without_dep_info_orders_its_fingerprint_dir_first() {
        let files = [
            PathBuf::from("/t/debug/deps/libu-aaaaaaaaaaaaaaaa.rlib"),
            PathBuf::from("/t/debug/.fingerprint/u-aaaaaaaaaaaaaaaa"),
        ];
        let ordered = unit_order(&files);
        assert_eq!(
            ordered,
            vec![
                PathBuf::from("/t/debug/.fingerprint/u-aaaaaaaaaaaaaaaa"),
                PathBuf::from("/t/debug/deps/libu-aaaaaaaaaaaaaaaa.rlib"),
            ]
        );
    }

    #[test]
    fn a_unit_with_only_a_fingerprint_dir_yields_nothing() {
        let files = [PathBuf::from("/t/debug/.fingerprint/u-aaaaaaaaaaaaaaaa")];
        assert!(unit_order(&files).is_empty());
    }

    #[test]
    fn select_targets_splits_own_shared_outside_and_failed() {
        let checkout = Path::new("/repo/main");
        let nested = [PathBuf::from("/repo/main/.worktrees/lane")];
        let scan = scan(
            vec![
                (
                    "/repo/main/target",
                    Owner::Owned {
                        checkout: checkout.to_path_buf(),
                    },
                ),
                (
                    "/shared/target",
                    Owner::Owned {
                        checkout: checkout.to_path_buf(),
                    },
                ),
                (
                    "/outside/target",
                    Owner::Owned {
                        checkout: checkout.to_path_buf(),
                    },
                ),
                (
                    "/repo/main/.worktrees/lane/extra",
                    Owner::Owned {
                        checkout: checkout.to_path_buf(),
                    },
                ),
            ],
            vec![
                ("/repo/main", "/repo/main", Ok("/repo/main/target")),
                ("/repo/main", "/repo/main", Ok("/shared/target")),
                ("/repo/wt", "/repo/wt", Ok("/shared/target")),
                ("/repo/main", "/repo/main", Ok("/outside/target")),
                (
                    "/repo/main",
                    "/repo/main",
                    Ok("/repo/main/.worktrees/lane/extra"),
                ),
                ("/repo/broken", "/repo/broken", Err("manifest broken")),
            ],
        );
        let got = select_targets(&scan, checkout, &nested);
        assert_eq!(got.own, vec![PathBuf::from("/repo/main/target")]);
        assert_eq!(got.shared, vec![PathBuf::from("/shared/target")]);
        assert_eq!(
            got.outside,
            vec![
                PathBuf::from("/outside/target"),
                PathBuf::from("/repo/main/.worktrees/lane/extra"),
            ]
        );
        assert_eq!(
            got.failed,
            vec![(PathBuf::from("/repo/broken"), "manifest broken".to_string())]
        );
    }

    #[cfg(target_os = "macos")]
    struct Scratch(PathBuf);

    #[cfg(target_os = "macos")]
    impl Scratch {
        fn new(label: &str) -> Scratch {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let dir = PathBuf::from(format!(
                "/private/tmp/hive-it-checkout-{label}-{}-{nanos}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }
    }

    #[cfg(target_os = "macos")]
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(target_os = "macos")]
    fn thin_execute(strings: &[&str]) -> Vec<u8> {
        const MH_MAGIC_64: u32 = 0xfeed_facf;
        const MH_EXECUTE: u32 = 0x2;
        const LC_SYMTAB: u32 = 0x2;
        const N_OSO: u8 = 0x66;
        const HEADER64: u32 = 32;
        const SYMTAB_CMD: u32 = 24;
        const NLIST64: u32 = 16;
        let mut strtab = vec![0u8];
        let mut str_offs = Vec::new();
        for s in strings {
            str_offs.push(strtab.len() as u32);
            strtab.extend_from_slice(s.as_bytes());
            strtab.push(0);
        }
        let nsyms = strings.len() as u32;
        let symoff = HEADER64 + SYMTAB_CMD;
        let stroff = symoff + nsyms * NLIST64;
        let strsize = strtab.len() as u32;
        let mut buf = Vec::new();
        let put = |buf: &mut Vec<u8>, v: u32| buf.extend_from_slice(&v.to_le_bytes());
        put(&mut buf, MH_MAGIC_64);
        put(&mut buf, 0x0100_000c);
        put(&mut buf, 0);
        put(&mut buf, MH_EXECUTE);
        put(&mut buf, 1);
        put(&mut buf, SYMTAB_CMD);
        put(&mut buf, 0);
        put(&mut buf, 0);
        put(&mut buf, LC_SYMTAB);
        put(&mut buf, SYMTAB_CMD);
        put(&mut buf, symoff);
        put(&mut buf, nsyms);
        put(&mut buf, stroff);
        put(&mut buf, strsize);
        for off in str_offs {
            put(&mut buf, off);
            buf.push(N_OSO);
            buf.push(0);
            buf.extend_from_slice(&0u16.to_le_bytes());
            buf.extend_from_slice(&0u64.to_le_bytes());
        }
        buf.extend_from_slice(&strtab);
        buf
    }

    #[cfg(target_os = "macos")]
    fn thin_execute_no_oso() -> Vec<u8> {
        thin_execute(&[])
    }

    #[cfg(target_os = "macos")]
    fn plant_locks(profile: &Path) {
        for name in [".cargo-build-lock", ".cargo-lock", ".cargo-artifact-lock"] {
            std::fs::write(profile.join(name), b"").unwrap();
        }
    }

    #[cfg(target_os = "macos")]
    fn plant_fingerprint(profile: &Path) {
        let unit = profile.join(".fingerprint/lib-aaaaaaaaaaaaaaaa");
        std::fs::create_dir_all(&unit).unwrap();
        std::fs::write(unit.join("lib-lib"), "0000000000000068").unwrap();
        std::fs::write(
            unit.join("lib-lib.json"),
            r#"{"rustc":1,"features":"[]","declared_features":"[]","target":2,"profile":3,"path":4,"deps":[],"local":[],"rustflags":[],"config":5,"compile_kind":0}"#,
        )
        .unwrap();
        let stamp = unit.join("invoked.timestamp");
        std::fs::write(&stamp, b"").unwrap();
        let aged = SystemTime::now() - Duration::from_secs(30 * 86_400);
        std::fs::File::open(&stamp)
            .unwrap()
            .set_modified(aged)
            .unwrap();
    }

    #[cfg(target_os = "macos")]
    fn plant_target(root: &Path) -> PathBuf {
        let target = root.join("target");
        let profile = target.join("debug");
        std::fs::create_dir_all(profile.join("deps")).unwrap();
        std::fs::write(
            target.join("CACHEDIR.TAG"),
            format!("Signature: 8a477f597d28d172789f06886806bc55\n{TAG_LINE}\n"),
        )
        .unwrap();
        plant_fingerprint(&profile);
        plant_locks(&profile);
        target
    }

    #[cfg(target_os = "macos")]
    fn scan_one(checkout: &Path, target: &Path) -> TargetScan {
        scan(
            vec![(
                target.to_str().unwrap(),
                Owner::Owned {
                    checkout: checkout.to_path_buf(),
                },
            )],
            vec![(
                checkout.to_str().unwrap(),
                checkout.to_str().unwrap(),
                Ok(target.to_str().unwrap()),
            )],
        )
    }

    #[cfg(target_os = "macos")]
    fn run_one(fs: &dyn Fs, checkout: &Path, target: &Path) -> super::CheckoutReport {
        let scan = scan_one(checkout, target);
        let input = CheckoutInput {
            fs,
            scan: &scan,
            checkout,
            nested: &[],
            now: SystemTime::now(),
            window: Duration::from_secs(7 * 86_400),
            account_bytes: true,
        };
        super::run(&input, &|| false)
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_missing_examples_or_build_dir_reads_as_empty() {
        let s = Scratch::new("missing-dirs");
        let profile = s.0.join("debug");
        std::fs::create_dir_all(profile.join("deps")).unwrap();
        std::fs::write(profile.join("deps/keep-aaaaaaaaaaaaaaaa.0.rcgu.o"), b"o").unwrap();
        std::fs::write(profile.join("app"), b"bin").unwrap();
        let fs = platform::current().unwrap().fs;
        let listing = list_profile(fs.as_ref(), &profile, 100, 100).unwrap();
        assert!(
            listing
                .candidates
                .iter()
                .any(|p| p.file_name().is_some_and(|n| n == "app")),
            "profile file must be a candidate: {:?}",
            listing.candidates
        );
        assert!(
            !listing
                .candidates
                .iter()
                .any(|p| p.to_string_lossy().contains("/examples/")
                    || p.to_string_lossy().contains("/build/")),
            "missing examples/ and build/ must read as empty: {:?}",
            listing.candidates
        );
        assert_eq!(listing.objects.len(), 1, "{:?}", listing.objects);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn list_profile_at_its_cap_lists_and_one_past_fails() {
        let s = Scratch::new("list-cap");
        let profile = s.0.join("debug");
        let deps = profile.join("deps");
        std::fs::create_dir_all(&deps).unwrap();
        for i in 0..3 {
            std::fs::write(deps.join(format!("f{i}.rcgu.o")), b"o").unwrap();
        }
        let fs = platform::current().unwrap().fs;
        let ok = list_profile(fs.as_ref(), &profile, 3, 100).unwrap();
        assert_eq!(ok.objects.len(), 3, "{:?}", ok.objects);
        let err = list_profile(fs.as_ref(), &profile, 2, 100).unwrap_err();
        assert!(!err.is_empty(), "{err}");

        let build = profile.join("build");
        std::fs::create_dir_all(build.join("pkg")).unwrap();
        for i in 0..3 {
            std::fs::write(build.join("pkg").join(format!("c{i}")), b"x").unwrap();
        }
        let ok = list_profile(fs.as_ref(), &profile, 100, 3).unwrap();
        assert!(
            ok.candidates
                .iter()
                .any(|p| p.ends_with("build/pkg/c0") || p.file_name().is_some_and(|n| n == "c0")),
            "{:?}",
            ok.candidates
        );
        let err = list_profile(fs.as_ref(), &profile, 100, 2).unwrap_err();
        assert!(!err.is_empty(), "{err}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn only_deps_rcgu_o_files_are_candidates_for_deletion() {
        let s = Scratch::new("only-rcgu");
        let profile = s.0.join("debug");
        let deps = profile.join("deps");
        std::fs::create_dir_all(profile.join("examples")).unwrap();
        std::fs::create_dir_all(deps).unwrap();
        std::fs::write(profile.join("deps/keep-aaaaaaaaaaaaaaaa.0.rcgu.o"), b"o").unwrap();
        std::fs::write(profile.join("deps/other.o"), b"o").unwrap();
        std::fs::write(profile.join("deps/libx.rlib"), b"r").unwrap();
        std::fs::write(profile.join("examples/ex.rcgu.o"), b"o").unwrap();
        let fs = platform::current().unwrap().fs;
        let listing = list_profile(fs.as_ref(), &profile, 100, 100).unwrap();
        let names: Vec<_> = listing
            .objects
            .iter()
            .map(|(p, _, _)| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["keep-aaaaaaaaaaaaaaaa.0.rcgu.o".to_string()]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn names_ending_in_dot_o_are_not_read() {
        let s = Scratch::new("skip-dot-o");
        let profile = s.0.join("debug");
        std::fs::create_dir_all(profile.join("deps")).unwrap();
        std::fs::write(profile.join("deps/foo.rcgu.o"), b"o").unwrap();
        std::fs::write(profile.join("deps/bar.o"), b"o").unwrap();
        std::fs::write(profile.join("deps/libx.rlib"), b"r").unwrap();
        std::fs::write(profile.join("app"), b"bin").unwrap();
        let fs = platform::current().unwrap().fs;
        let listing = list_profile(fs.as_ref(), &profile, 100, 100).unwrap();
        assert!(
            listing.candidates.iter().all(|p| !p
                .file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with(".o")),
            "{:?}",
            listing.candidates
        );
        let names: BTreeSet<_> = listing
            .candidates
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains("libx.rlib"), "{names:?}");
        assert!(names.contains("app"), "{names:?}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_profile_with_binaries_and_objects_but_no_debug_map_keeps_every_object() {
        let s = Scratch::new("no-debug-map");
        let checkout = s.0.join("repo");
        std::fs::create_dir_all(&checkout).unwrap();
        let target = plant_target(&checkout);
        let profile = target.join("debug");
        let object = profile.join("deps/orphan-bbbbbbbbbbbbbbbb.0.rcgu.o");
        std::fs::write(&object, b"obj").unwrap();
        std::fs::write(profile.join("app"), thin_execute_no_oso()).unwrap();
        let fs = platform::current().unwrap().fs;
        let report = run_one(fs.as_ref(), &checkout, &target);
        assert!(object.is_file(), "object was deleted without a debug map");
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.kind == "gc.objects_undecided" && f.path.as_ref() == Some(&profile)),
            "{:?}",
            report.findings
        );
        assert_eq!(report.totals.objects_deleted, 0, "{report:?}");
    }

    #[cfg(target_os = "macos")]
    struct SwapAfterStat {
        real: Box<dyn Fs>,
        path: PathBuf,
        swapped: Cell<bool>,
    }

    #[cfg(target_os = "macos")]
    impl Fs for SwapAfterStat {
        fn temp_dir(&self, parent: &Path, prefix: &str) -> io::Result<platform::TempDir> {
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
            self.real.mtime(path)
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
        fn sharing(&self, path: &Path) -> io::Result<platform::Sharing> {
            self.real.sharing(path)
        }
        fn compress(&self, path: &Path) -> platform::CompressOutcome {
            self.real.compress(path)
        }
        fn hold_lock(
            &self,
            kind: platform::LockKind,
            path: &Path,
        ) -> io::Result<platform::LockHolder> {
            self.real.hold_lock(kind, path)
        }
        fn try_lock_exclusive(&self, path: &Path) -> platform::TryLock {
            self.real.try_lock_exclusive(path)
        }
        fn try_lock_elsewhere(
            &self,
            kind: platform::LockKind,
            path: &Path,
        ) -> io::Result<platform::LockState> {
            self.real.try_lock_elsewhere(kind, path)
        }
        fn entry_stat(&self, path: &Path) -> io::Result<EntryStat> {
            let stat = self.real.entry_stat(path)?;
            if path == self.path && !self.swapped.get() && stat.kind == EntryKind::File {
                self.swapped.set(true);
                std::fs::remove_file(path).unwrap();
                std::fs::create_dir(path).unwrap();
            }
            Ok(stat)
        }
        fn read_dir_bounded(&self, dir: &Path, cap: usize) -> io::Result<Vec<(String, EntryKind)>> {
            self.real.read_dir_bounded(dir, cap)
        }
        fn remove_tree(&self, path: &Path) -> io::Result<()> {
            self.real.remove_tree(path)
        }
        fn remove_file(&self, path: &Path) -> io::Result<()> {
            self.real.remove_file(path)
        }
        fn volume(&self, path: &Path) -> io::Result<platform::VolumeInfo> {
            self.real.volume(path)
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn an_object_swapped_for_a_directory_is_refused() {
        let s = Scratch::new("swap-object");
        let checkout = s.0.join("repo");
        std::fs::create_dir_all(&checkout).unwrap();
        let target = plant_target(&checkout);
        let profile = target.join("debug");
        let kept = "keep-cccccccccccccccc.0.rcgu.o";
        let swap = "swap-dddddddddddddddd.0.rcgu.o";
        std::fs::write(profile.join("deps").join(kept), b"k").unwrap();
        let swap_path = profile.join("deps").join(swap);
        std::fs::write(&swap_path, b"s").unwrap();
        std::fs::write(
            profile.join("app"),
            thin_execute(&[&format!("/old/deps/{kept}")]),
        )
        .unwrap();
        let fs = SwapAfterStat {
            real: platform::current().unwrap().fs,
            path: swap_path.clone(),
            swapped: Cell::new(false),
        };
        let report = run_one(&fs, &checkout, &target);
        assert!(
            swap_path.is_dir(),
            "swapped path must be refused, not removed"
        );
        assert!(
            fs.swapped.get(),
            "listing must have statted the object before the check: {report:?}"
        );
        assert!(
            profile.join("deps").join(kept).is_file(),
            "referenced object must remain"
        );
    }
}
