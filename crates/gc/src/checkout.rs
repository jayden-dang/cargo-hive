//! One checkout's dead-unit GC: select its own targets, lock each, re-read, delete.

use crate::liveness::liveness;
use crate::plan_tier::guard::{self, Expect};
use cargo_hive_cargo::discover::{self, TargetScan};
use cargo_hive_core::caps::CRATE_DIR_CAP;
use cargo_hive_core::fingerprint::read_profile;
use cargo_hive_core::ledger::{self, Bytes, Ledger};
use cargo_hive_core::liveness::Liveness;
use cargo_hive_core::locks::{self, Acquired, Reason};
use cargo_hive_core::receipt::Finding;
use cargo_hive_core::target::{self, TargetDir};
use cargo_hive_platform::{EntryKind, Fs};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
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
    let mut report = CheckoutReport {
        per_target: Vec::new(),
        totals: Counts::empty(),
        findings: Vec::new(),
        unmeasured: Vec::new(),
        unmeasured_more: 0,
        cancelled: false,
        failed: false,
    };
    let selected = select_targets(input.scan, input.checkout, input.nested);
    if !selected.failed.is_empty() {
        for (dir, err) in selected.failed {
            report.findings.push(Finding {
                kind: "ownership.unresolved".into(),
                path: Some(dir),
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
    for path in &selected.own {
        if cancel() {
            report.cancelled = true;
            break;
        }
        let counts = clean_target(input, path, cancel, &mut report);
        report.totals.add_target(&counts);
        report.per_target.push((path.clone(), counts));
        if report.cancelled {
            break;
        }
    }
    report
}

fn clean_target(
    input: &CheckoutInput,
    path: &Path,
    cancel: &dyn Fn() -> bool,
    report: &mut CheckoutReport,
) -> Counts {
    let mut counts = Counts::empty();
    let locked = match locks::acquire(input.fs, path, true) {
        Acquired::Locked(locked) => locked,
        Acquired::Skipped(reason) => {
            *counts.skips.entry(reason.count_name()).or_insert(0) += 1;
            report.findings.push(Finding {
                kind: "gc.skipped".into(),
                path: Some(path.to_path_buf()),
                detail: reason_text(reason),
                measured: BTreeMap::new(),
            });
            return counts;
        }
        Acquired::Failed(why) => {
            report.failed = true;
            report.findings.push(Finding {
                kind: "gc.target_failed".into(),
                path: Some(path.to_path_buf()),
                detail: why,
                measured: BTreeMap::new(),
            });
            return counts;
        }
    };
    let profiles = match target::identify_bounded(input.fs, path, CRATE_DIR_CAP) {
        Some(Ok(found)) => found.into_iter().map(|(p, _)| p).collect::<Vec<_>>(),
        Some(Err(why)) => {
            report.failed = true;
            report.findings.push(Finding {
                kind: "gc.target_failed".into(),
                path: Some(path.to_path_buf()),
                detail: why,
                measured: BTreeMap::new(),
            });
            return counts;
        }
        None => {
            skip_profile_set(&mut counts, report, path);
            return counts;
        }
    };
    let first: BTreeSet<_> = locked.profiles.iter().cloned().collect();
    let again: BTreeSet<_> = profiles.iter().cloned().collect();
    if first != again {
        skip_profile_set(&mut counts, report, path);
        return counts;
    }
    let reports: Vec<_> = profiles.iter().map(|p| read_profile(p)).collect();
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
        profiles: reports,
        size: None,
        unreadable: None,
    };
    match liveness(input.fs, &fresh, input.now, input.window) {
        Liveness::Undecided { reasons } => {
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
            let mut ledger = Ledger::new();
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
                for (entry, stat) in &stats {
                    if let Err(reason) = guard::check_entry(
                        input.fs,
                        path,
                        entry,
                        Expect::Entry {
                            kind: stat.kind,
                            dev: stat.dev,
                            ino: stat.ino,
                        },
                    ) {
                        report.failed = true;
                        report.findings.push(Finding {
                            kind: "gc.delete_failed".into(),
                            path: Some((*entry).clone()),
                            detail: format!("{} ({reason:?})", entry.display()),
                            measured: BTreeMap::new(),
                        });
                        break 'units;
                    }
                    match ledger::remove_counted(
                        input.fs,
                        &mut ledger,
                        path,
                        entry,
                        input.account_bytes,
                        false,
                    ) {
                        Ok(removed) if removed.existed => deleted_files += 1,
                        Ok(_) => {}
                        Err(err) => {
                            report.failed = true;
                            report.findings.push(Finding {
                                kind: "gc.delete_failed".into(),
                                path: Some((*entry).clone()),
                                detail: err.to_string(),
                                measured: BTreeMap::new(),
                            });
                            break 'units;
                        }
                    }
                }
                if deleted_files > 0 {
                    counts.units_deleted += 1;
                    counts.unit_files_deleted += deleted_files;
                }
            }
            let tally = ledger.finish();
            for (p, why) in tally.unmeasured {
                report.unmeasured.push((p, why));
            }
            report.unmeasured_more = report.unmeasured_more.saturating_add(tally.unmeasured_more);
            if input.account_bytes {
                if let Some(bytes) = tally.per_target.get(path).copied() {
                    counts.bytes = Some(bytes);
                } else if tally.total.freed > 0 || tally.total.still_shared > 0 {
                    counts.bytes = Some(tally.total);
                } else {
                    counts.bytes = Some(Bytes::default());
                }
            }
        }
    }
    drop(locked);
    counts
}

fn skip_profile_set(counts: &mut Counts, report: &mut CheckoutReport, path: &Path) {
    let name = Reason::ProfileSet.count_name();
    *counts.skips.entry(name).or_insert(0) += 1;
    report.findings.push(Finding {
        kind: "gc.skipped".into(),
        path: Some(path.to_path_buf()),
        detail: reason_text(Reason::ProfileSet),
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
    path.parent()
        .and_then(|parent| parent.file_name())
        .is_some_and(|name| name == ".fingerprint")
}

#[cfg(test)]
mod tests {
    use super::{select_targets, unit_order};
    use cargo_hive_cargo::discover::{TargetScan, WorkspaceResolution};
    use cargo_hive_core::target::{Owner, TargetDir};
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

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
}
