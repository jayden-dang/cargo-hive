//! Clones a target's new registry units into the store while that target's Cargo
//! locks are held, then drops the locks before compressing or publishing anything.
//! A unit already in the store is not cloned again. A lock that is not taken fills
//! nothing from that target.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::cargo_target::TargetDir;
use crate::gc::{CRATE_DIR_CAP, DEPS_CAP};
use crate::plan_tier::locks::{self, Acquired, Locked};
use crate::plan_tier::Ctx;
use crate::platform::{EntryKind, Fs};
use crate::registry::Repo;
use crate::seed::source::{self, Skip, SourceUnit};
use crate::seed::store::{self, PublishFailed, Published, Row};
use crate::size::MAX_DEPTH;
use crate::state::{window_days, StateError};

use super::Error;

const HASH_CAP: u64 = 64;

#[derive(Debug)]
pub struct FillReport {
    pub entries_added: u64,
    pub entries_held: u64,
    pub targets_read: u64,
    pub targets_skipped: BTreeMap<&'static str, u64>,
    pub findings: Vec<(String, Option<PathBuf>, String)>,
    pub cancelled: bool,
}

struct Cloned {
    row: Row,
    tmp: PathBuf,
}

enum ClonedUnit {
    Ready(Cloned),
    Skip,
    StopTarget,
}

enum Stop {
    Full(String),
    Failed(String),
}

struct Batch {
    cloned: Vec<Cloned>,
    disk_full: Option<String>,
}

struct Pass<'a> {
    ctx: &'a Ctx<'a>,
    repo: &'a Repo,
    now: u64,
    cancel: &'a (dyn Fn() -> bool + Sync),
    report: &'a mut FillReport,
}

pub fn fill(
    ctx: &Ctx<'_>,
    repo: &Repo,
    targets: &[TargetDir],
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<FillReport, Error> {
    let mut report = FillReport {
        entries_added: 0,
        entries_held: 0,
        targets_read: 0,
        targets_skipped: BTreeMap::new(),
        findings: Vec::new(),
        cancelled: false,
    };
    if let Err(err) = store::sweep_tmp(ctx.state, ctx.now) {
        report.findings.push((
            "store.tmp".to_string(),
            Some(store::store_dir(ctx.state).join(".tmp")),
            err,
        ));
        return Ok(report);
    }
    let stored = store::read(ctx.state).map_err(Error::State)?;
    let days = match window_days(ctx.cfg.gc_window_days) {
        Ok(days) => days,
        Err(err) => {
            report
                .findings
                .push(("config.window".to_string(), None, err.to_string()));
            return Ok(report);
        }
    };
    let window = Duration::from_secs(days.saturating_mul(86_400));
    let now = epoch_secs(ctx.now);
    let home = source::cargo_home();
    for target in targets {
        if cancel() {
            report.cancelled = true;
            break;
        }
        fill_target(
            &mut Pass {
                ctx,
                repo,
                now,
                cancel,
                report: &mut report,
            },
            target,
            &stored,
            &home,
            window,
        )?;
        if report.cancelled {
            break;
        }
    }
    Ok(report)
}

fn fill_target(
    pass: &mut Pass<'_>,
    target: &TargetDir,
    stored: &store::StoreFile,
    home: &Path,
    window: Duration,
) -> Result<(), Error> {
    let units = match source::live_registry_units(pass.ctx, target, home, window) {
        Ok((units, left_out)) => {
            if !left_out.is_empty() {
                pass.report.findings.push((
                    "unit.left_out".to_string(),
                    Some(target.path.clone()),
                    format!(
                        "{}: {} left out; a dependency edge resolved to no unit, so these units were not stored",
                        target.path.display(),
                        left_out.join(", ")
                    ),
                ));
            }
            units
        }
        Err(skip) => {
            note_skip(pass.report, target, &skip);
            return Ok(());
        }
    };
    let mut new_units = Vec::new();
    let mut held_rows = Vec::new();
    for unit in units {
        if entry_present(&store::entry_dir(pass.ctx.state, &skeleton(&unit))) {
            match held_row(pass.ctx, stored, &unit, pass.repo, pass.now) {
                Ok(row) => held_rows.push(row),
                Err(detail) => pass.report.findings.push((
                    "entry.unreadable".to_string(),
                    Some(target.path.clone()),
                    detail,
                )),
            }
        } else {
            new_units.push(unit);
        }
    }
    let tag_present = path_present(&store::tag_path(pass.ctx.state));
    if new_units.is_empty() && tag_present {
        pass.report.targets_read += 1;
        pass.report.entries_held += held_rows.len() as u64;
        return merge_rows(pass.ctx, held_rows, pass.cancel, pass.report);
    }
    let locked = match locks::acquire(pass.ctx.fs, &target.path, false) {
        Acquired::Locked(locked) => locked,
        Acquired::Skipped(reason) => {
            *pass
                .report
                .targets_skipped
                .entry(reason.count_name())
                .or_insert(0) += 1;
            return Ok(());
        }
        Acquired::Failed(why) => {
            pass.report.findings.push((
                "lock.failed".to_string(),
                Some(target.path.clone()),
                format!("{}: {why}", target.path.display()),
            ));
            return Ok(());
        }
    };
    let batch = clone_under_lock(pass, target, &locked, &new_units, tag_present);
    drop(locked);
    publish_batch(pass.ctx, target, batch, held_rows, pass.cancel, pass.report)
}

fn clone_under_lock(
    pass: &mut Pass<'_>,
    target: &TargetDir,
    _locked: &Locked,
    new_units: &[SourceUnit],
    tag_present: bool,
) -> Batch {
    let listings = match profile_listings(pass.ctx.fs, target) {
        Ok(listings) => listings,
        Err(detail) => {
            pass.report.findings.push((
                "deps.listing".to_string(),
                Some(target.path.clone()),
                detail,
            ));
            return Batch {
                cloned: Vec::new(),
                disk_full: None,
            };
        }
    };
    let mut batch = Batch {
        cloned: Vec::new(),
        disk_full: None,
    };
    for unit in new_units {
        if (pass.cancel)() {
            pass.report.cancelled = true;
            break;
        }
        match clone_unit(
            pass.ctx,
            target,
            unit,
            &listings,
            pass.repo,
            pass.now,
            pass.report,
        ) {
            Ok(ClonedUnit::Ready(cloned)) => batch.cloned.push(cloned),
            Ok(ClonedUnit::Skip) => {}
            Ok(ClonedUnit::StopTarget) => return batch,
            Err(why) => {
                batch.disk_full = Some(why);
                return batch;
            }
        }
    }
    if !tag_present {
        match clone_tag(pass.ctx, target) {
            Ok(()) => {}
            Err(Stop::Full(why)) => {
                batch.disk_full = Some(why);
                return batch;
            }
            Err(Stop::Failed(detail)) => {
                pass.report.findings.push((
                    "tag.clone".to_string(),
                    Some(target.path.clone()),
                    format!("{}: {detail}", target.path.display()),
                ));
            }
        }
    }
    batch
}

fn clone_unit(
    ctx: &Ctx<'_>,
    target: &TargetDir,
    unit: &SourceUnit,
    listings: &HashMap<PathBuf, Vec<(String, EntryKind)>>,
    repo: &Repo,
    now: u64,
    report: &mut FillReport,
) -> Result<ClonedUnit, String> {
    let profile = target.path.join(&unit.rel_profile);
    let hash_path = profile
        .join(".fingerprint")
        .join(&unit.dir_name)
        .join(&unit.stem);
    match hash_unchanged(&hash_path, &unit.fingerprint_hash) {
        Ok(true) => {}
        Ok(false) => return Ok(ClonedUnit::Skip),
        Err(detail) => {
            report.findings.push((
                "hash.unread".to_string(),
                Some(target.path.clone()),
                format!("{}: {detail}", target.path.display()),
            ));
            return Ok(ClonedUnit::Skip);
        }
    }
    let listing = listings.get(&profile).map(Vec::as_slice).unwrap_or(&[]);
    let files = match source::unit_files(ctx.fs, &target.path, unit, listing, CRATE_DIR_CAP) {
        Ok(files) => files,
        Err(refused) => {
            report.findings.push((
                "unit.refused".to_string(),
                Some(target.path.clone()),
                format!("{}: {refused}", target.path.display()),
            ));
            return Ok(ClonedUnit::Skip);
        }
    };
    let key = format!("{}-{}", unit.dir_name, unit.fingerprint_hash);
    let tmp = store::tmp_dir(ctx.state, &key);
    if let Err(stop) = reset_tmp(&tmp) {
        return match stop {
            Stop::Full(why) => Err(why),
            Stop::Failed(detail) => {
                report.findings.push((
                    "store.tmp".to_string(),
                    Some(target.path.clone()),
                    format!("{}: {detail}", target.path.display()),
                ));
                Ok(ClonedUnit::Skip)
            }
        };
    }
    let mut size = 0u64;
    for rel in &files {
        let src = target.path.join(rel);
        let dst = tmp.join(rel);
        if let Some(parent) = dst.parent() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                abandon(&tmp);
                let detail = format!(
                    "{}: {}: {err}; remove the temp directory and run the command again",
                    target.path.display(),
                    parent.display()
                );
                return match stop_io(&err, &detail) {
                    Stop::Full(why) => Err(why),
                    Stop::Failed(detail) => {
                        report.findings.push((
                            "store.tmp".to_string(),
                            Some(target.path.clone()),
                            detail,
                        ));
                        Ok(ClonedUnit::Skip)
                    }
                };
            }
        }
        let stat = match ctx.fs.entry_stat(&src) {
            Ok(stat) => stat,
            Err(err) => {
                abandon(&tmp);
                report.findings.push((
                    "clone.failed".to_string(),
                    Some(target.path.clone()),
                    format!(
                        "{}: {}: {err}; the entry was left out of the store, so run the command again",
                        target.path.display(),
                        src.display()
                    ),
                ));
                return Ok(ClonedUnit::Skip);
            }
        };
        if stat.kind != EntryKind::File {
            abandon(&tmp);
            report.findings.push((
                "unit.refused".to_string(),
                Some(target.path.clone()),
                format!(
                    "{}: {} is not a regular file; this unit was not stored",
                    target.path.display(),
                    src.display()
                ),
            ));
            return Ok(ClonedUnit::Skip);
        }
        if let Err(err) = ctx.fs.clone_file(&src, &dst) {
            abandon(&tmp);
            let detail = format!(
                "{}: {}: {err}; the entry was left out of the store, so run the command again",
                target.path.display(),
                src.display()
            );
            report.findings.push((
                "clone.failed".to_string(),
                Some(target.path.clone()),
                detail,
            ));
            if err.raw_os_error() == Some(libc::EXDEV) {
                return Ok(ClonedUnit::StopTarget);
            }
            if err.raw_os_error() == Some(libc::ENOSPC) {
                return Err(format!("{}: {err}", src.display()));
            }
            return Ok(ClonedUnit::Skip);
        }
        size = size.saturating_add(stat.blocks.saturating_mul(512));
    }
    let mut row = skeleton(unit);
    row.size = size;
    row.filled_at = now;
    row.last_live_at = now;
    row.repos = vec![repo.main.clone()];
    Ok(ClonedUnit::Ready(Cloned { row, tmp }))
}

fn publish_batch(
    ctx: &Ctx<'_>,
    target: &TargetDir,
    batch: Batch,
    held_rows: Vec<Row>,
    cancel: &(dyn Fn() -> bool + Sync),
    report: &mut FillReport,
) -> Result<(), Error> {
    report.targets_read += 1;
    report.entries_held += held_rows.len() as u64;
    let mut rows = held_rows;
    for item in batch.cloned {
        if cancel() {
            report.cancelled = true;
            break;
        }
        match store::compress_and_publish(ctx.fs, ctx.state, &item.tmp, &item.row, cancel) {
            Ok(Published::Added) => {
                report.entries_added += 1;
                rows.push(item.row);
            }
            Ok(Published::Held) => {
                report.entries_held += 1;
                rows.push(item.row);
            }
            Err(PublishFailed::Stopped(path)) => {
                report.cancelled = true;
                let failed = PublishFailed::Stopped(path);
                report.findings.push((
                    "publish.stopped".to_string(),
                    Some(target.path.clone()),
                    failed.to_string(),
                ));
                break;
            }
            Err(PublishFailed::DiskFull(why)) => {
                merge_rows(ctx, rows, cancel, report)?;
                return Err(Error::DiskFull(why));
            }
            Err(PublishFailed::Compress(why)) => {
                let failed = PublishFailed::Compress(why);
                report.findings.push((
                    "publish.failed".to_string(),
                    Some(target.path.clone()),
                    format!("{}: {failed}", target.path.display()),
                ));
            }
        }
    }
    merge_rows(ctx, rows, cancel, report)?;
    if let Some(why) = batch.disk_full {
        return Err(Error::DiskFull(why));
    }
    Ok(())
}

fn merge_rows(
    ctx: &Ctx<'_>,
    rows: Vec<Row>,
    cancel: &(dyn Fn() -> bool + Sync),
    report: &mut FillReport,
) -> Result<(), Error> {
    if rows.is_empty() {
        return Ok(());
    }
    match store::merge(ctx.state, rows, BTreeMap::new(), cancel) {
        Ok(()) => Ok(()),
        Err(StateError::Cancelled) => {
            report.cancelled = true;
            report.findings.push((
                "store.merge".to_string(),
                Some(ctx.state.root().join(store::STORE_FILE)),
                "stopped while merging store rows; run the command again".to_string(),
            ));
            Ok(())
        }
        Err(err) => Err(Error::State(err)),
    }
}

fn note_skip(report: &mut FillReport, target: &TargetDir, skip: &Skip) {
    let name = match skip {
        Skip::Undecided(_) => "targets_skipped_undecided",
        Skip::Unsupported(_) => "targets_skipped_unsupported",
        Skip::Bound(_) => "targets_skipped_bound",
    };
    *report.targets_skipped.entry(name).or_insert(0) += 1;
    report.findings.push((
        "source.skipped".to_string(),
        Some(target.path.clone()),
        format!(
            "{}: {skip}; this target was not filled",
            target.path.display()
        ),
    ));
}

fn held_row(
    ctx: &Ctx<'_>,
    stored: &store::StoreFile,
    unit: &SourceUnit,
    repo: &Repo,
    now: u64,
) -> Result<Row, String> {
    let mut row = skeleton(unit);
    row.filled_at = now;
    row.last_live_at = now;
    row.repos = vec![repo.main.clone()];
    if let Some(existing) = stored.units.iter().find(|kept| same_key(kept, &row)) {
        row.size = existing.size;
        row.filled_at = existing.filled_at;
        return Ok(row);
    }
    let dir = store::entry_dir(ctx.state, &row);
    row.size = dir_size(ctx.fs, &dir)?;
    Ok(row)
}

fn skeleton(unit: &SourceUnit) -> Row {
    Row {
        toolchain_value: unit.toolchain_value,
        name_meta: unit.dir_name.clone(),
        fingerprint_hash: unit.fingerprint_hash.clone(),
        kind: unit.kind.clone(),
        rel_profile: unit.rel_profile.clone(),
        size: 0,
        filled_at: 0,
        last_live_at: 0,
        repos: Vec::new(),
        registry_deps: unit.registry_deps.clone(),
    }
}

fn same_key(left: &Row, right: &Row) -> bool {
    left.toolchain_value == right.toolchain_value
        && left.name_meta == right.name_meta
        && left.fingerprint_hash == right.fingerprint_hash
}

fn profile_listings(
    fs: &dyn Fs,
    target: &TargetDir,
) -> Result<HashMap<PathBuf, Vec<(String, EntryKind)>>, String> {
    let mut listings = HashMap::new();
    for profile in &target.profiles {
        let dir = profile.path.join("deps");
        let listing = match fs.read_dir_bounded(&dir, DEPS_CAP) {
            Ok(listing) => listing,
            Err(err) if err.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(err) => {
                return Err(format!(
                    "{}: {err}; shrink the directory below its cap and build the target again",
                    dir.display()
                ));
            }
        };
        listings.insert(profile.path.clone(), listing);
    }
    Ok(listings)
}

fn hash_unchanged(path: &Path, expected: &str) -> Result<bool, String> {
    let mut file = File::open(path).map_err(|err| format!("{}: {err}", path.display()))?;
    let mut buf = Vec::new();
    file.by_ref()
        .take(HASH_CAP + 1)
        .read_to_end(&mut buf)
        .map_err(|err| format!("{}: {err}", path.display()))?;
    if buf.len() as u64 > HASH_CAP {
        return Err(format!(
            "{} is longer than {HASH_CAP} bytes; shorten the hash file and build the unit again",
            path.display()
        ));
    }
    Ok(buf == expected.as_bytes())
}

fn clone_tag(ctx: &Ctx<'_>, target: &TargetDir) -> Result<(), Stop> {
    let dst = store::tag_path(ctx.state);
    if path_present(&dst) {
        return Ok(());
    }
    let src = target.path.join("CACHEDIR.TAG");
    if let Some(parent) = dst.parent() {
        if let Err(err) = std::fs::create_dir_all(parent) {
            return Err(stop_io(
                &err,
                &format!(
                    "{}: {err}; remove the store directory and run the command again",
                    parent.display()
                ),
            ));
        }
    }
    match ctx.fs.clone_file(&src, &dst) {
        Ok(()) => Ok(()),
        Err(err) => Err(stop_io(
            &err,
            &format!(
                "{}: {err}; the store tag was not written, so run the command again",
                src.display()
            ),
        )),
    }
}

fn stop_io(err: &io::Error, detail: &str) -> Stop {
    if err.raw_os_error() == Some(libc::ENOSPC) || err.kind() == io::ErrorKind::StorageFull {
        Stop::Full(detail.to_string())
    } else {
        Stop::Failed(detail.to_string())
    }
}

fn reset_tmp(tmp: &Path) -> Result<(), Stop> {
    let removed = match std::fs::symlink_metadata(tmp) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(tmp),
        Ok(_) => std::fs::remove_file(tmp),
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(tmp_stop(tmp, err)),
    };
    if let Err(err) = removed {
        return Err(tmp_stop(tmp, err));
    }
    std::fs::create_dir_all(tmp).map_err(|err| tmp_stop(tmp, err))
}

fn tmp_stop(tmp: &Path, err: io::Error) -> Stop {
    stop_io(
        &err,
        &format!(
            "{}: {err}; remove the temp directory and run the command again",
            tmp.display()
        ),
    )
}

fn abandon(tmp: &Path) {
    let _ = std::fs::remove_dir_all(tmp);
}

fn dir_size(fs: &dyn Fs, root: &Path) -> Result<u64, String> {
    let mut total = 0u64;
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            return Err(format!(
                "{} is deeper than {MAX_DEPTH} levels; remove directories past that depth and run the command again",
                dir.display()
            ));
        }
        let entries = fs.read_dir_bounded(&dir, CRATE_DIR_CAP).map_err(|err| {
            format!(
                "{}: {err}; shrink the directory below its cap and run the command again",
                dir.display()
            )
        })?;
        for (name, kind) in entries {
            let path = dir.join(name);
            match kind {
                EntryKind::Dir => stack.push((path, depth + 1)),
                EntryKind::File => {
                    let stat = fs.entry_stat(&path).map_err(|err| {
                        format!(
                            "{}: {err}; make the entry readable and run the command again",
                            path.display()
                        )
                    })?;
                    total = total.saturating_add(stat.blocks.saturating_mul(512));
                }
                EntryKind::Symlink | EntryKind::Other => {
                    return Err(format!(
                        "{} is not a regular file; remove it and run the command again",
                        path.display()
                    ));
                }
            }
        }
    }
    Ok(total)
}

fn entry_present(path: &Path) -> bool {
    std::fs::symlink_metadata(path)
        .map(|meta| meta.is_dir())
        .unwrap_or(false)
}

fn path_present(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

fn epoch_secs(now: SystemTime) -> u64 {
    now.duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}
