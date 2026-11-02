//! Removes one approved orphan target directory.
//! Locks are held across the relist, the fresh ownership check and the removal.
//! A symbolic link inside the directory is unlinked, not followed.

use std::path::{Path, PathBuf};

use crate::cargo_target::{self, Owner};
use crate::discover::{self, Snapshot};
use crate::gc::{self, Ledger, CRATE_DIR_CAP};
use crate::plan_tier::apply::Approval;
use crate::plan_tier::guard::{check_entry, Deleted, Expect};
use crate::plan_tier::locks::{self, Acquired};
use crate::plan_tier::record::{kind_of, BytesRec, PlannedTarget};
use crate::plan_tier::Ctx;
use crate::platform::{EntryKind, Fs};

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
    MeasureRefused(String),
    Failed(FailedRemoval),
}

pub fn remove(
    ctx: &Ctx,
    target: &PlannedTarget,
    approval: &Approval,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Removal {
    if approval.plan_id().is_empty() {
        return Removal::Refused("approval_mismatch".to_string());
    }
    let Some(scope) = recorded(target) else {
        return Removal::Refused("entry_changed".to_string());
    };
    if let Err(reason) = same_entry(ctx.fs, &target.path, scope.kind, scope.dev, scope.ino) {
        return Removal::Refused(reason);
    }
    let locked = match locks::acquire(ctx.fs, &target.path, false) {
        Acquired::Locked(locked) => locked,
        Acquired::Skipped(reason) => return Removal::Skipped(reason.count_name().to_string()),
    };
    if let Err(reason) = same_entry(ctx.fs, &target.path, scope.kind, scope.dev, scope.ino) {
        return Removal::Refused(reason);
    }
    if let Err(removal) = profiles_match_plan(ctx.fs, &target.path, scope.dirs) {
        return removal;
    }
    if let Err(reason) = still_orphan(ctx, &target.path, cancel) {
        return match reason {
            Stop::Cancelled => Removal::Skipped("cancelled".to_string()),
            Stop::Refused(reason) => Removal::Refused(reason),
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
    let removal = match gc::remove_counted(
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
    let found = match cargo_target::identify(target) {
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
}

fn still_orphan(ctx: &Ctx, target: &Path, cancel: &(dyn Fn() -> bool + Sync)) -> Result<(), Stop> {
    if cancel() {
        return Err(Stop::Cancelled);
    }
    let mut checkouts = Vec::new();
    for repo in ctx.repos {
        let (rows, _) = discover::checkouts(repo, ctx.mains, ctx.cfg);
        checkouts.extend(rows);
    }
    if cancel() {
        return Err(Stop::Cancelled);
    }
    let mut snapshot = Snapshot::default();
    let scan = discover::targets(&checkouts, ctx.cfg, true, &mut snapshot, cancel);
    if cancel() {
        return Err(Stop::Cancelled);
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
        Some(_) => Err(Stop::Refused("class_changed".to_string())),
        None => Err(Stop::Refused("ownership_unresolved".to_string())),
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

fn removal_error(err: crate::gc::account::RemoveFailed, ledger: Ledger) -> Removal {
    use crate::gc::account::RemoveFailed;
    match err {
        RemoveFailed::Scan(why) => Removal::MeasureRefused(format!("could not measure: {why}")),
        RemoveFailed::Remove(error) => Removal::Failed(FailedRemoval {
            detail: format!("could not remove it: {error}"),
            counted: into_counted(ledger),
        }),
    }
}
