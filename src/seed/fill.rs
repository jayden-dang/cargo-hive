//! Clones a target's new registry units into the store while that target's Cargo
//! locks are held, then drops the locks before publishing anything.
//! A unit already in the store is not cloned again. A lock that is not taken fills
//! nothing from that target. An owned target also contributes one toolchain row
//! when `.rustc_info.json` names that checkout's toolchain and every unit shares
//! one toolchain value.

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::cargo_target::{Owner, TargetDir};
use crate::ctx::Ctx;
use crate::gc::{CRATE_DIR_CAP, DEPS_CAP};
use crate::locks::{self, Acquired, Locked};
use crate::platform::{EntryKind, Fs};
use crate::registry::Repo;
use crate::seed::source::{self, Skip, SourceUnit};
use crate::seed::store::{self, PublishFailed, Published, Row};
use crate::size::MAX_DEPTH;
use crate::state::{window_days, StateError};

use super::Error;

const HASH_CAP: u64 = 64;
const RUSTC_INFO_CAP: u64 = 1024 * 1024;

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
    Ready(Box<Cloned>),
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
    let listings = match profile_listings(pass.ctx.fs, target) {
        Ok(listings) => listings,
        Err(detail) => {
            pass.report.findings.push((
                "deps.listing".to_string(),
                Some(target.path.clone()),
                detail,
            ));
            return Ok(());
        }
    };
    let mut new_units = Vec::new();
    let mut held_rows = Vec::new();
    let mut refs = Vec::new();
    let mut newest_at = 0u64;
    for unit in units {
        let profile = target.path.join(&unit.rel_profile);
        let hash_path = profile
            .join(".fingerprint")
            .join(&unit.dir_name)
            .join(&unit.stem);
        newest_at = newest_at.max(mtime_nanos(&hash_path));
        let listing = listings.get(&profile).map(Vec::as_slice).unwrap_or(&[]);
        let files =
            match source::unit_files(pass.ctx.fs, &target.path, &unit, listing, CRATE_DIR_CAP) {
                Ok(files) => files,
                Err(refused) => {
                    pass.report.findings.push((
                        "unit.refused".to_string(),
                        Some(target.path.clone()),
                        format!("{}: {refused}", target.path.display()),
                    ));
                    continue;
                }
            };
        match holding_variant(pass.ctx, stored, &unit, &target.path, &files) {
            Ok(Holding::Held(variant)) => {
                match held_row(pass.ctx, stored, &unit, variant, pass.repo, pass.now) {
                    Ok(row) => {
                        refs.push(unit_ref(&row));
                        held_rows.push(row);
                    }
                    Err(detail) => pass.report.findings.push((
                        "entry.unreadable".to_string(),
                        Some(target.path.clone()),
                        detail,
                    )),
                }
            }
            Ok(Holding::New(variant)) => new_units.push(NewUnit {
                unit,
                variant,
                files,
            }),
            Err(detail) => pass.report.findings.push((
                "entry.unreadable".to_string(),
                Some(target.path.clone()),
                detail,
            )),
        }
    }
    let record = source_record(pass.repo, target, newest_at, refs);
    let tag_present = path_present(&store::tag_path(pass.ctx.state));
    if new_units.is_empty() && tag_present {
        pass.report.targets_read += 1;
        pass.report.entries_held += held_rows.len() as u64;
        let table = toolchain_table(target, pass.cancel, pass.report);
        return merge_rows(pass.ctx, held_rows, table, record, pass.cancel, pass.report);
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
            *pass
                .report
                .targets_skipped
                .entry("lock.failed")
                .or_insert(0) += 1;
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
    publish_batch(
        pass.ctx,
        target,
        batch,
        held_rows,
        record,
        pass.cancel,
        pass.report,
    )
}

/// A live unit the store does not hold with these bytes yet, and the variant it gets.
struct NewUnit {
    unit: SourceUnit,
    variant: u32,
    files: Vec<PathBuf>,
}

enum Holding {
    Held(u32),
    New(u32),
}

/// The variant of the unit's key whose files have the unit's bytes, or the variant a new
/// copy gets (SEED-2.30). Variants are probed on disk, so one stored earlier in this fill
/// is seen too.
fn holding_variant(
    ctx: &Ctx<'_>,
    stored: &store::StoreFile,
    unit: &SourceUnit,
    target_root: &Path,
    files: &[PathBuf],
) -> Result<Holding, String> {
    let known = stored
        .units
        .iter()
        .filter(|row| {
            row.toolchain_value == unit.toolchain_value
                && row.name_meta == unit.dir_name
                && row.fingerprint_hash == unit.fingerprint_hash
        })
        .map(|row| row.variant)
        .max()
        .unwrap_or(0);
    let mut highest_present = None;
    let mut variant = 0u32;
    loop {
        let mut row = skeleton(unit);
        row.variant = variant;
        let dir = store::entry_dir(ctx.state, &row);
        if entry_present(&dir) {
            highest_present = Some(variant);
            match store::same_unit_bytes(ctx.fs, target_root, files, &dir) {
                Ok(true) => return Ok(Holding::Held(variant)),
                Ok(false) => {}
                Err(err) => return Err(format!("{}: {err}", dir.display())),
            }
        } else if variant >= known {
            break;
        }
        variant += 1;
    }
    Ok(Holding::New(highest_present.map_or(0, |v| v + 1)))
}

fn unit_ref(row: &Row) -> store::SourceUnitRef {
    store::SourceUnitRef {
        toolchain_value: row.toolchain_value,
        name_meta: row.name_meta.clone(),
        fingerprint_hash: row.fingerprint_hash.clone(),
        variant: row.variant,
    }
}

fn source_record(
    repo: &Repo,
    target: &TargetDir,
    newest_at: u64,
    units: Vec<store::SourceUnitRef>,
) -> Option<(PathBuf, store::SourceRecord)> {
    let Owner::Owned { checkout } = &target.owner else {
        return None;
    };
    Some((
        target.path.clone(),
        store::SourceRecord {
            repo: repo.main.clone(),
            checkout: checkout.clone(),
            newest_at,
            units,
        },
    ))
}

fn mtime_nanos(path: &Path) -> u64 {
    std::fs::symlink_metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
}

fn clone_under_lock(
    pass: &mut Pass<'_>,
    target: &TargetDir,
    _locked: &Locked,
    new_units: &[NewUnit],
    tag_present: bool,
) -> Batch {
    let mut batch = Batch {
        cloned: Vec::new(),
        disk_full: None,
    };
    for new in new_units {
        if (pass.cancel)() {
            pass.report.cancelled = true;
            break;
        }
        match clone_unit(pass.ctx, target, new, pass.repo, pass.now, pass.report) {
            Ok(ClonedUnit::Ready(cloned)) => batch.cloned.push(*cloned),
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
    new: &NewUnit,
    repo: &Repo,
    now: u64,
    report: &mut FillReport,
) -> Result<ClonedUnit, String> {
    let unit = &new.unit;
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
    let files = &new.files;
    let key = store::entry_name(&unit.dir_name, &unit.fingerprint_hash, new.variant);
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
    for rel in files {
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
            return match clone_io_branch(&err) {
                CloneIoBranch::StopTarget => Ok(ClonedUnit::StopTarget),
                CloneIoBranch::DiskFull => Err(format!("{}: {err}", src.display())),
                CloneIoBranch::Skip => Ok(ClonedUnit::Skip),
            };
        }
        size = size.saturating_add(stat.blocks.saturating_mul(512));
    }
    let mut row = skeleton(unit);
    row.variant = new.variant;
    row.size = size;
    row.filled_at = now;
    row.last_live_at = now;
    row.repos = vec![repo.main.clone()];
    Ok(ClonedUnit::Ready(Box::new(Cloned { row, tmp })))
}

fn publish_batch(
    ctx: &Ctx<'_>,
    target: &TargetDir,
    batch: Batch,
    held_rows: Vec<Row>,
    mut record: Option<(PathBuf, store::SourceRecord)>,
    cancel: &(dyn Fn() -> bool + Sync),
    report: &mut FillReport,
) -> Result<(), Error> {
    report.targets_read += 1;
    report.entries_held += held_rows.len() as u64;
    let table = toolchain_table(target, cancel, report);
    let mut rows = held_rows;
    for mut item in batch.cloned {
        if cancel() {
            report.cancelled = true;
            break;
        }
        match store::publish_entry(ctx.fs, ctx.state, &item.tmp, &mut item.row, cancel) {
            Ok(Published::Added) => {
                report.entries_added += 1;
                if let Some((_, record)) = record.as_mut() {
                    record.units.push(unit_ref(&item.row));
                }
                rows.push(item.row);
            }
            Ok(Published::Held) => {
                report.entries_held += 1;
                if let Some((_, record)) = record.as_mut() {
                    record.units.push(unit_ref(&item.row));
                }
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
                merge_rows(ctx, rows, table, None, cancel, report)?;
                return Err(Error::disk_full(why));
            }
            Err(PublishFailed::Failed(why)) => {
                let failed = PublishFailed::Failed(why);
                report.findings.push((
                    "publish.failed".to_string(),
                    Some(target.path.clone()),
                    format!("{}: {failed}", target.path.display()),
                ));
            }
        }
    }
    if report.cancelled || batch.disk_full.is_some() {
        record = None;
    }
    merge_rows(ctx, rows, table, record, cancel, report)?;
    if let Some(why) = batch.disk_full {
        return Err(Error::disk_full(why));
    }
    Ok(())
}

fn merge_rows(
    ctx: &Ctx<'_>,
    rows: Vec<Row>,
    table: BTreeMap<String, u64>,
    record: Option<(PathBuf, store::SourceRecord)>,
    cancel: &(dyn Fn() -> bool + Sync),
    report: &mut FillReport,
) -> Result<(), Error> {
    if rows.is_empty() && table.is_empty() && record.is_none() {
        return Ok(());
    }
    match store::merge_with_source(ctx.state, rows, table, record, cancel) {
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
    variant: u32,
    repo: &Repo,
    now: u64,
) -> Result<Row, String> {
    let mut row = skeleton(unit);
    row.variant = variant;
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
        variant: 0,
    }
}

fn same_key(left: &Row, right: &Row) -> bool {
    left.toolchain_value == right.toolchain_value
        && left.name_meta == right.name_meta
        && left.fingerprint_hash == right.fingerprint_hash
        && left.variant == right.variant
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
                return Err(super::directory_listing(
                    &dir,
                    &err,
                    "build the target again",
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
    if err.kind() == io::ErrorKind::StorageFull {
        Stop::Full(detail.to_string())
    } else {
        Stop::Failed(detail.to_string())
    }
}

#[derive(Debug, PartialEq, Eq)]
enum CloneIoBranch {
    StopTarget,
    DiskFull,
    Skip,
}

fn clone_io_branch(err: &io::Error) -> CloneIoBranch {
    match err.kind() {
        io::ErrorKind::CrossesDevices => CloneIoBranch::StopTarget,
        io::ErrorKind::StorageFull => CloneIoBranch::DiskFull,
        _ => CloneIoBranch::Skip,
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
        let entries = fs
            .read_dir_bounded(&dir, CRATE_DIR_CAP)
            .map_err(|err| super::directory_listing(&dir, &err, "run the command again"))?;
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

pub fn rustc_info_version(text: &str) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|_| "not JSON; rebuild the target and run the command again".to_string())?;
    let Some(outputs) = value.get("outputs").and_then(serde_json::Value::as_object) else {
        return Err("has no outputs; rebuild the target and run the command again".to_string());
    };
    let mut found = None;
    for entry in outputs.values() {
        let Some(stdout) = entry.get("stdout").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if !stdout.lines().any(|line| line.contains("release:")) {
            continue;
        }
        if found.is_some() {
            return Err(
                "has more than one release line; rebuild the target and run the command again"
                    .to_string(),
            );
        }
        found = Some(stdout.to_string());
    }
    found.ok_or_else(|| {
        "has no release line; rebuild the target and run the command again".to_string()
    })
}

pub fn toolchain_row(
    target: &TargetDir,
    source_checkout: &Path,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<(String, u64), String> {
    let path = target.path.join(".rustc_info.json");
    let text = read_capped(&path, RUSTC_INFO_CAP)?;
    let version = rustc_info_version(&text).map_err(|err| format!("{}: {err}", path.display()))?;
    let resolved =
        crate::toolchain::resolve_toolchain_until(source_checkout, cancel).map_err(|err| {
            if cancel() {
                format!("{}: {err}", source_checkout.display())
            } else {
                format!(
                    "{}: {err}; install the toolchain that checkout selects and run the command again",
                    source_checkout.display()
                )
            }
        })?;
    if version != resolved.version_text {
        return Err(format!(
            "{}: toolchain text does not match {}; build the target with that checkout's toolchain and run the command again",
            target.path.display(),
            source_checkout.display()
        ));
    }
    Ok((version, one_toolchain_value(target)?))
}

fn toolchain_table(
    target: &TargetDir,
    cancel: &(dyn Fn() -> bool + Sync),
    report: &mut FillReport,
) -> BTreeMap<String, u64> {
    let Owner::Owned { checkout } = &target.owner else {
        return BTreeMap::new();
    };
    match toolchain_row(target, checkout, cancel) {
        Ok((text, value)) => BTreeMap::from([(text, value)]),
        Err(_) if cancel() => {
            report.cancelled = true;
            BTreeMap::new()
        }
        Err(detail) => {
            report.findings.push((
                "toolchain.row".to_string(),
                Some(target.path.clone()),
                detail,
            ));
            BTreeMap::new()
        }
    }
}

fn one_toolchain_value(target: &TargetDir) -> Result<u64, String> {
    let mut found = None;
    for profile in &target.profiles {
        let Some(graph) = &profile.graph else {
            continue;
        };
        for unit in &graph.units {
            match found {
                None => found = Some(unit.toolchain_value),
                Some(value) if value == unit.toolchain_value => {}
                Some(_) => {
                    return Err(format!(
                        "{}: units hold more than one toolchain value; collect the older units and run the command again",
                        target.path.display()
                    ));
                }
            }
        }
    }
    found.ok_or_else(|| {
        format!(
            "{}: no unit recorded a toolchain value; build the target and run the command again",
            target.path.display()
        )
    })
}

fn read_capped(path: &Path, cap: u64) -> Result<String, String> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Err(format!(
                "{} is missing; Cargo leaves it out while a rustc wrapper is set to the empty string (RUSTC_WRAPPER= or RUSTC_WORKSPACE_WRAPPER=) or CARGO_CACHE_RUSTC_INFO=0, so build the target once without them and run the command again",
                path.display()
            ));
        }
        Err(err) => {
            return Err(format!(
                "{}: {err}; make the file readable and run the command again",
                path.display()
            ));
        }
    };
    let mut buf = Vec::new();
    file.by_ref()
        .take(cap + 1)
        .read_to_end(&mut buf)
        .map_err(|err| {
            format!(
                "{}: {err}; make the file readable and run the command again",
                path.display()
            )
        })?;
    if buf.len() as u64 > cap {
        return Err(format!(
            "{} is larger than 1 MiB; shorten it below 1 MiB and build the target again",
            path.display()
        ));
    }
    String::from_utf8(buf).map_err(|_| {
        format!(
            "{} is not valid UTF-8; rebuild the target and run the command again",
            path.display()
        )
    })
}

fn epoch_secs(now: SystemTime) -> u64 {
    now.duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::rustc_info_version;
    use crate::cargo_target::{Owner, TargetDir};
    use crate::fingerprint::{Coverage, Format, ProfileReport, Unit, UnitGraph};
    use crate::toolchain::resolve_toolchain;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn info(entries: &[(&str, &str)]) -> String {
        let mut outputs = serde_json::Map::new();
        for (key, stdout) in entries {
            outputs.insert((*key).to_string(), serde_json::json!({ "stdout": stdout }));
        }
        serde_json::json!({ "outputs": outputs }).to_string()
    }

    #[test]
    fn rustc_info_version_keeps_the_one_stdout_with_a_release_line() {
        let release = "rustc 1.97.1\nbinary: rustc\nrelease: 1.97.1\n";
        let cases = [
            (
                "one release stdout",
                info(&[("probe", "lib___.rlib\n"), ("version", release)]),
                Some(release),
            ),
            ("no release line", info(&[("probe", "lib___.rlib\n")]), None),
            (
                "two release lines",
                info(&[
                    ("older", "release: 1.0.0\n"),
                    ("newer", "rustc 1.1.0\nrelease: 1.1.0\n"),
                ]),
                None,
            ),
            ("not json", "not json".to_string(), None),
            ("no outputs", r#"{"successes":{}}"#.to_string(), None),
        ];
        for (name, text, want) in cases {
            let got = rustc_info_version(&text);
            match want {
                Some(stdout) => assert_eq!(got.unwrap(), stdout, "{name}"),
                None => assert!(got.is_err(), "{name}: {got:?}"),
            }
        }
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Scratch {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = PathBuf::from(format!(
                "/private/tmp/hive-it-fill-{label}-{}-{nanos}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Scratch(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn sample_unit(value: u64) -> Unit {
        Unit {
            dir_name: "dep-0000000000000001".to_string(),
            kind: "lib".to_string(),
            hash_hex: "0123456789abcdef".to_string(),
            deps: Vec::new(),
            stem: "lib-dep".to_string(),
            features: Arc::<str>::from(""),
            coverage: Coverage::NotRecorded,
            profile: 1,
            target: 1,
            path: 1,
            toolchain_value: value,
        }
    }

    fn owned_with(root: &Path, value: u64) -> TargetDir {
        let path = root.join("target");
        std::fs::create_dir_all(&path).unwrap();
        TargetDir {
            path,
            owner: Owner::Owned {
                checkout: root.to_path_buf(),
            },
            profiles: vec![ProfileReport {
                path: root.join("target/debug"),
                format: Format::Supported,
                units: 1,
                not_current: 0,
                not_current_units: Vec::new(),
                edges: None,
                graph: Some(UnitGraph {
                    units: vec![sample_unit(value)],
                    edges: Vec::new(),
                }),
            }],
            size: None,
            unreadable: None,
        }
    }

    fn rustc_info_text(stdout: &str, extra: usize) -> String {
        serde_json::to_string(&serde_json::json!({
            "pad": "a".repeat(extra),
            "outputs": {
                "probe": {"stdout": "lib___.rlib\n"},
                "version": {"stdout": stdout}
            }
        }))
        .unwrap()
    }

    #[test]
    fn a_rustc_info_of_one_mibibyte_yields_the_checkout_text() {
        let scratch = Scratch::new("bound");
        let version = resolve_toolchain(&scratch.0).unwrap().version_text;
        let bare = rustc_info_text(&version, 0);
        let room = (1024 * 1024usize)
            .checked_sub(bare.len())
            .expect("the unpadded file is under 1 MiB");
        let exact = rustc_info_text(&version, room);
        assert_eq!(exact.len(), 1024 * 1024);
        let target = owned_with(&scratch.0, 9);
        std::fs::write(target.path.join(".rustc_info.json"), &exact).unwrap();
        assert_eq!(
            super::toolchain_row(&target, &scratch.0, &|| false).unwrap(),
            (version, 9)
        );
    }

    #[test]
    fn a_cancelled_toolchain_lookup_marks_the_fill_cancelled_and_says_nothing_about_installing() {
        let scratch = Scratch::new("cancel");
        let version = resolve_toolchain(&scratch.0).unwrap().version_text;
        let target = owned_with(&scratch.0, 9);
        std::fs::write(
            target.path.join(".rustc_info.json"),
            rustc_info_text(&version, 0),
        )
        .unwrap();
        let mut report = super::FillReport {
            entries_added: 0,
            entries_held: 0,
            targets_read: 0,
            targets_skipped: std::collections::BTreeMap::new(),
            findings: Vec::new(),
            cancelled: false,
        };
        let table = super::toolchain_table(&target, &|| true, &mut report);
        assert!(table.is_empty(), "{table:?}");
        assert!(report.cancelled, "{report:?}");
        assert!(
            report
                .findings
                .iter()
                .all(|(_, _, d)| !d.contains("install")),
            "{:?}",
            report.findings
        );
        let err = super::toolchain_row(&target, &scratch.0, &|| true).unwrap_err();
        assert!(err.contains("cancelled"), "{err}");
        assert!(!err.contains("install"), "{err}");
    }

    #[test]
    fn a_rustc_info_one_byte_past_one_mibibyte_is_an_error() {
        let scratch = Scratch::new("over");
        let version = resolve_toolchain(&scratch.0).unwrap().version_text;
        let bare = rustc_info_text(&version, 0);
        let exact = rustc_info_text(&version, (1024 * 1024usize) - bare.len());
        let over = rustc_info_text(&version, (1024 * 1024usize) - bare.len() + 1);
        assert_eq!(exact.len(), 1024 * 1024);
        assert_eq!(over.len(), 1024 * 1024 + 1);
        let target = owned_with(&scratch.0, 9);
        std::fs::write(target.path.join(".rustc_info.json"), &over).unwrap();
        let err = super::toolchain_row(&target, &scratch.0, &|| false).unwrap_err();
        assert!(err.contains("1 MiB"), "{err}");
        assert!(
            err.contains(&target.path.join(".rustc_info.json").display().to_string()),
            "{err}"
        );
    }

    #[test]
    fn a_listing_that_is_not_the_cap_does_not_say_shrink() {
        let scratch = Scratch::new("not-dir");
        let path = scratch.0.join("store");
        std::fs::write(&path, b"x").unwrap();
        let fs = crate::platform::current().unwrap().fs;
        let err = super::dir_size(fs.as_ref(), &path).unwrap_err();
        assert!(!err.contains("shrink"), "{err}");
        assert!(err.contains(&path.display().to_string()), "{err}");
        assert!(
            err.contains("remove the path that is not a directory"),
            "{err}"
        );
        assert!(err.contains("run the command again"), "{err}");
    }

    #[test]
    fn a_lock_that_cannot_be_read_is_skipped_and_fills_nothing() {
        let scratch = Scratch::new("lock-failed");
        let target_path = scratch.0.join("target");
        std::fs::create_dir_all(&target_path).unwrap();
        std::fs::write(
            target_path.join("CACHEDIR.TAG"),
            "# This file is a cache directory tag created by cargo.\n",
        )
        .unwrap();
        let mode = std::os::unix::fs::PermissionsExt::from_mode(0o111);
        std::fs::set_permissions(&target_path, mode).unwrap();
        struct Restore(PathBuf);
        impl Drop for Restore {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(
                    &self.0,
                    std::os::unix::fs::PermissionsExt::from_mode(0o755),
                );
            }
        }
        let _restore = Restore(target_path.clone());
        let home = scratch.0.join("home");
        let state = crate::state::State::open_at(home).unwrap();
        let platform = crate::platform::current().unwrap();
        let cfg = crate::state::Config {
            secondary_roots: Vec::new(),
            cache_roots: Vec::new(),
            gc_window_days: None,
        };
        let repo = crate::registry::Repo {
            main: scratch.0.clone(),
            root_commits: std::collections::BTreeSet::new(),
        };
        let repos = [repo.clone()];
        let mains = [scratch.0.clone()];
        let ctx = crate::ctx::Ctx {
            state: &state,
            fs: platform.fs.as_ref(),
            cfg: &cfg,
            now: SystemTime::UNIX_EPOCH,
            account_bytes: true,
            repos: &repos,
            mains: &mains,
        };
        let target = TargetDir {
            path: target_path.clone(),
            owner: Owner::Owned {
                checkout: scratch.0.clone(),
            },
            profiles: Vec::new(),
            size: None,
            unreadable: None,
        };
        let report = super::fill(&ctx, &repo, &[target], &|| false).unwrap();
        assert_eq!(report.targets_skipped.get("lock.failed").copied(), Some(1));
        assert_eq!(report.entries_added, 0);
        assert_eq!(report.entries_held, 0);
        assert!(report
            .findings
            .iter()
            .any(|(kind, _, _)| kind == "lock.failed"));
        assert!(crate::seed::store::read(&state).unwrap().units.is_empty());
    }

    #[test]
    fn a_permission_denied_clone_takes_the_non_cross_device_branch() {
        let err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        assert_eq!(super::clone_io_branch(&err), super::CloneIoBranch::Skip);
    }
}
