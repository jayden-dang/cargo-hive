//! Removes one orphan target directory.
//! Locks are held across the relist, the fresh ownership check and the removal.
//! A symbolic link inside the directory is unlinked, not followed.

use std::path::{Path, PathBuf};

use crate::ctx::Ctx;
use crate::discover::{self, Snapshot, WalkFault};
use crate::plan_tier::guard::{Deleted, Expect, check_entry};
use crate::plan_tier::record::{BytesRec, PlannedTarget, kind_of};
use cargo_hive_core::caps::CRATE_DIR_CAP;
use cargo_hive_core::ledger::{self, Ledger, RemoveFailed};
use cargo_hive_core::locks::{self, Acquired};
use cargo_hive_core::target::{self, Owner};
use cargo_hive_platform::{EntryKind, Fs};

pub struct Counted {
    pub bytes: BytesRec,
    pub unmeasured: Vec<(PathBuf, String)>,
    pub unmeasured_more: u64,
}

pub struct FailedRemoval {
    pub detail: String,
    pub counted: Counted,
}

pub enum Removal {
    Removed(Counted),
    Skipped(String),
    Refused(String),
    Unreadable(Vec<WalkFault>),
    MeasureRefused(String),
    Failed(FailedRemoval),
}

/// Walk faults that did not stop the removal are added to `noted`.
pub fn remove(
    ctx: &Ctx,
    target: &PlannedTarget,
    cancel: &(dyn Fn() -> bool + Sync),
    noted: &mut Vec<WalkFault>,
) -> Removal {
    let Some(scope) = recorded(target) else {
        return Removal::Refused("entry_changed".to_string());
    };
    if let Err(reason) = same_entry(ctx.fs, &target.path, scope.kind, scope.dev, scope.ino) {
        return Removal::Refused(reason);
    }
    // Present lock files include `.cargo-artifact-lock`; unit plans pass that only for examples.
    let locked = match locks::acquire(ctx.fs, &target.path, true) {
        Acquired::Locked(locked) => locked,
        Acquired::Skipped(reason) => return Removal::Skipped(reason.count_name().to_string()),
        Acquired::Failed(why) => return Removal::Refused(why),
    };
    if let Err(reason) = same_entry(ctx.fs, &target.path, scope.kind, scope.dev, scope.ino) {
        return Removal::Refused(reason);
    }
    if let Err(removal) = profiles_match_plan(ctx.fs, &target.path, scope.dirs) {
        return removal;
    }
    if let Err(reason) = still_orphan(ctx, &target.path, cancel, noted) {
        return match reason {
            Stop::Cancelled => Removal::Skipped("cancelled".to_string()),
            Stop::Refused(reason) => Removal::Refused(reason),
            Stop::Unreadable(faults) => Removal::Unreadable(faults),
        };
    }
    let Some(parent) = target.path.parent() else {
        return Removal::Refused("entry_changed".to_string());
    };
    if let Err(reason) = check_entry(
        ctx.fs,
        parent,
        &target.path,
        Expect::Entry {
            kind: scope.kind,
            dev: scope.dev,
            ino: scope.ino,
        },
    ) {
        return Removal::Refused(guard_reason(reason));
    }
    if cancel() {
        return Removal::Skipped("cancelled".to_string());
    }
    let mut ledger = Ledger::new();
    let removal = match ledger::remove_counted(
        ctx.fs,
        &mut ledger,
        parent,
        &target.path,
        ctx.account_bytes,
        false,
    ) {
        Ok(removed) if removed.existed => Removal::Removed(into_counted(ledger)),
        Ok(_) => Removal::Refused("entry_changed".to_string()),
        Err(err) => removal_error(err, ledger),
    };
    drop(locked);
    removal
}

struct Recorded<'a> {
    kind: EntryKind,
    dev: u64,
    ino: u64,
    dirs: &'a [PathBuf],
}

fn recorded(target: &PlannedTarget) -> Option<Recorded<'_>> {
    let scope = target.orphan.as_ref()?;
    Some(Recorded {
        kind: kind_of(&scope.root_kind)?,
        dev: scope.root_dev,
        ino: scope.root_ino,
        dirs: &scope.dirs,
    })
}

fn same_entry(fs: &dyn Fs, path: &Path, kind: EntryKind, dev: u64, ino: u64) -> Result<(), String> {
    match fs.entry_stat(path) {
        Ok(stat) if stat.kind == kind && stat.dev == dev && stat.ino == ino => Ok(()),
        Ok(_) | Err(_) => Err("entry_changed".to_string()),
    }
}

fn profiles_match_plan(fs: &dyn Fs, target: &Path, planned: &[PathBuf]) -> Result<(), Removal> {
    let found = match target::identify_bounded(fs, target, CRATE_DIR_CAP) {
        Some(Ok(found)) => found,
        Some(Err(why)) => return Err(Removal::Refused(why)),
        None => return Err(Removal::Refused("unsupported_format".to_string())),
    };
    // Profile directories are what the plan records, including `triple/profile`.
    // The triple directory itself is only listed so its profiles stay inside the cap.
    let listing = fs
        .read_dir_bounded(target, CRATE_DIR_CAP)
        .map_err(|err| Removal::MeasureRefused(format!("could not measure: {err}")))?;
    for (name, kind) in &listing {
        if *kind != EntryKind::Dir {
            continue;
        }
        let child = target.join(name);
        if found.iter().any(|(dir, _)| dir == &child) {
            continue;
        }
        fs.read_dir_bounded(&child, CRATE_DIR_CAP)
            .map_err(|err| Removal::MeasureRefused(format!("could not measure: {err}")))?;
    }
    if found
        .iter()
        .any(|(dir, _)| !planned.iter().any(|planned| planned == dir))
    {
        return Err(Removal::Refused("profile_set".to_string()));
    }
    Ok(())
}

enum Stop {
    Cancelled,
    Refused(String),
    Unreadable(Vec<WalkFault>),
}

fn still_orphan(
    ctx: &Ctx,
    target: &Path,
    cancel: &(dyn Fn() -> bool + Sync),
    noted: &mut Vec<WalkFault>,
) -> Result<(), Stop> {
    if cancel() {
        return Err(Stop::Cancelled);
    }
    let (checkouts, faults) = crate::ctx::checkout_walk(ctx);
    if cancel() {
        return Err(Stop::Cancelled);
    }
    let (faults, cannot_enter) = crate::ctx::split_checkout_faults(faults);
    noted.extend(cannot_enter);
    if !faults.is_empty() {
        return Err(Stop::Unreadable(faults));
    }
    let mut snapshot = Snapshot::default();
    let scan = discover::targets(&checkouts, ctx.cfg, true, &mut snapshot, cancel);
    if cancel() {
        return Err(Stop::Cancelled);
    }
    let faults = crate::ctx::faults_that_can_hide_an_owner(scan.walk_faults, &ctx.cfg.cache_roots);
    if !faults.is_empty() {
        return Err(Stop::Unreadable(faults));
    }
    let stat = ctx.fs.entry_stat(target).ok();
    let found = scan.targets.iter().find(|cand| {
        cand.path == target
            || stat.is_some_and(|stat| {
                ctx.fs
                    .entry_stat(&cand.path)
                    .is_ok_and(|other| other.dev == stat.dev && other.ino == stat.ino)
            })
    });
    match found.map(|cand| &cand.owner) {
        Some(Owner::OrphanCandidate { .. }) => Ok(()),
        Some(Owner::Unresolved { .. }) | None => {
            Err(Stop::Refused("ownership_unresolved".to_string()))
        }
        Some(_) => Err(Stop::Refused("class_changed".to_string())),
    }
}

fn guard_reason(reason: Deleted) -> String {
    match reason {
        Deleted::Missing | Deleted::Changed | Deleted::Outside | Deleted::Removed => {
            "entry_changed".to_string()
        }
    }
}

fn into_counted(ledger: Ledger) -> Counted {
    let tally = ledger.finish();
    Counted {
        bytes: BytesRec {
            freed: tally.total.freed,
            still_shared: tally.total.still_shared,
        },
        unmeasured: tally.unmeasured,
        unmeasured_more: tally.unmeasured_more,
    }
}

fn removal_error(err: RemoveFailed, ledger: Ledger) -> Removal {
    match err {
        RemoveFailed::Scan(why) => Removal::MeasureRefused(format!("could not measure: {why}")),
        RemoveFailed::Remove(error) => Removal::Failed(FailedRemoval {
            detail: format!("could not remove it: {error}"),
            counted: into_counted(ledger),
        }),
    }
}
