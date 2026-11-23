//! Clones chosen store entries into a checkout whose Cargo target does not exist yet.
//! An empty set creates nothing. A target path that already exists, a missing parent,
//! or a build directory that is not the target directory is one finding and clones nothing.
//! Each file is cloned on its own, `deps/` then `build/` then `.fingerprint/`. A clone that
//! still has the compressed-file flag is replaced in the same directory by a new regular file
//! of the same bytes, and that file gets the clone's mtime back, before sharing is read. The
//! store entry stays compressed. A clone counts as shared only when it reports `shares_all`
//! and the same clone id as its store file; anything else adds that file's allocated blocks.
//! `private_bytes` is not read. The store's size is its allocated blocks.

use std::collections::{BTreeMap, HashSet};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::store::{self, Row};
use cargo_hive_cargo::ctx::Ctx;
use cargo_hive_core::fingerprint;
use cargo_hive_core::liveness;
use cargo_hive_core::size::{MAX_DEPTH, allocated_bytes};
use cargo_hive_core::unit_path::{self, UnitLayout, UnitLoc};
use cargo_hive_platform::{EntryKind, Fs};

use super::Error;

pub use super::record::SeededUnit;

#[derive(Debug, Default)]
pub struct PlaceReport {
    pub placed: Vec<SeededUnit>,
    pub cloned: u64,
    pub existing: u64,
    pub failed: u64,
    pub outside_window: u64,
    pub bytes_not_shared: Option<u64>,
    pub store_bytes: Option<u64>,
    pub target_directory: Option<PathBuf>,
    pub target_identity: Option<(u64, u64)>,
    pub findings: Vec<(String, Option<PathBuf>, String)>,
    pub unmeasured: Vec<(String, String)>,
    pub cancelled: bool,
    /// Set when a later unit hits a full disk. Units already cloned stay in `placed`.
    pub disk_full: Option<String>,
}

enum Stop {
    Full(String),
    Failed(String),
}

#[derive(Default)]
struct Added {
    bytes: u64,
    failed: bool,
    gaps: Vec<(String, String)>,
    scoped_files: Option<BTreeMap<PathBuf, PlacedFile>>,
}

#[derive(PartialEq, Eq)]
struct PlacedFile {
    identity: cargo_hive_state::slots::FileIdentity,
    mode: u32,
    len: u64,
    changed: (i64, i64),
    modified: SystemTime,
}

impl PlacedFile {
    fn inspect(file: &std::fs::File) -> io::Result<Self> {
        let metadata = file.metadata()?;
        Ok(Self {
            identity: cargo_hive_state::slots::FileIdentity::of(&metadata),
            mode: metadata.mode(),
            len: metadata.len(),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
            modified: metadata.modified()?,
        })
    }
}

impl Added {
    fn accept(&mut self, fs: &dyn Fs, path: &Path) -> Result<(), Stop> {
        let Some(files) = &mut self.scoped_files else {
            return accept_placed(fs, path);
        };
        let unproved =
            || Stop::Failed("the scoped clone was not a proved regular uncompressed file".into());
        let observed = fs.entry_stat(path).map_err(|_| unproved())?;
        if observed.kind != EntryKind::File || observed.compressed {
            return Err(unproved());
        }
        let file =
            cargo_hive_platform::flock::open_owner_state_file(path).map_err(|_| unproved())?;
        let placed = PlacedFile::inspect(&file).map_err(|_| unproved())?;
        if (placed.identity.dev, placed.identity.ino) != (observed.dev, observed.ino)
            || files.insert(path.to_path_buf(), placed).is_some()
        {
            return Err(unproved());
        }
        Ok(())
    }

    fn gap(&mut self, path: &Path, err: &io::Error) {
        self.failed = true;
        self.gaps.push((
            "clone_bytes_not_shared".to_string(),
            format!(
                "{path}: {err}; the bytes this clone adds were not measured",
                path = path.display()
            ),
        ));
    }
}

const MAX_EXPAND_BYTES: u64 = 1024 * 1024 * 1024;

fn accept_placed(fs: &dyn Fs, dst: &Path) -> Result<(), Stop> {
    match expand_if_compressed(fs, dst) {
        Ok(()) => Ok(()),
        Err(err) => {
            let shown = format!("{}: {err}", dst.display());
            if is_full(&err) {
                Err(Stop::Full(shown))
            } else {
                Err(Stop::Failed(shown))
            }
        }
    }
}

fn expand_if_compressed(fs: &dyn Fs, dst: &Path) -> io::Result<()> {
    expand_capped(fs, dst, MAX_EXPAND_BYTES)
}

fn expand_capped(fs: &dyn Fs, dst: &Path, cap: u64) -> io::Result<()> {
    if !fs.entry_stat(dst)?.compressed {
        return Ok(());
    }
    let meta = std::fs::symlink_metadata(dst)?;
    let mtime = fs.mtime(dst)?;
    let len = meta.len();
    if len > cap {
        return Err(io::Error::other(format!(
            "{len} bytes is over the {cap} byte cap"
        )));
    }
    let bytes = fs.read(dst)?;
    if bytes.len() as u64 > cap {
        return Err(io::Error::other(format!(
            "{} bytes is over the {cap} byte cap",
            bytes.len()
        )));
    }
    let tmp = expand_sibling(dst)?;
    if let Err(err) = fs.write_file(&tmp, &bytes) {
        let _ = fs.remove_file(&tmp);
        return Err(err);
    }
    if let Err(err) = std::fs::set_permissions(&tmp, meta.permissions()) {
        let _ = fs.remove_file(&tmp);
        return Err(err);
    }
    if let Err(err) = std::fs::rename(&tmp, dst) {
        let _ = fs.remove_file(&tmp);
        return Err(err);
    }
    fs.set_mtime(dst, mtime)?;
    if fs.entry_stat(dst)?.compressed {
        return Err(io::Error::other("the file is still compressed"));
    }
    Ok(())
}

fn expand_sibling(dst: &Path) -> io::Result<PathBuf> {
    let name = dst.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "the placed file has no name")
    })?;
    let mut tmp = std::ffi::OsString::from(".hive-expand-");
    tmp.push(name);
    Ok(dst.with_file_name(tmp))
}

fn charge(fs: &dyn Fs, added: &mut Added, src: &Path, dst: &Path) {
    if added.failed {
        return;
    }
    let shared = match (fs.sharing(dst), fs.sharing(src)) {
        (Ok(clone), Ok(stored)) => clone.shares_all && clone.clone_id == stored.clone_id,
        (Err(err), _) => return added.gap(dst, &err),
        (_, Err(err)) => return added.gap(src, &err),
    };
    if shared {
        return;
    }
    match fs.entry_stat(dst) {
        Ok(stat) => added.bytes = added.bytes.saturating_add(stat.blocks.saturating_mul(512)),
        Err(err) => added.gap(dst, &err),
    }
}

fn record_bytes(ctx: &Ctx<'_>, report: &mut PlaceReport, added: &mut Added) {
    if !ctx.account_bytes {
        report.unmeasured.push((
            "clone_bytes_not_shared".to_string(),
            "doctor check (c) did not pass".to_string(),
        ));
    } else if added.failed {
        report.unmeasured.append(&mut added.gaps);
    } else {
        report.bytes_not_shared = Some(added.bytes);
    }
    match allocated_bytes(&store::store_dir(ctx.state), MAX_DEPTH) {
        Ok(bytes) => report.store_bytes = Some(bytes),
        Err(err) => report.unmeasured.push((
            "store_bytes".to_string(),
            format!("{err}; the store's size was not measured"),
        )),
    }
}

fn finish(ctx: &Ctx<'_>, mut report: PlaceReport, added: &mut Added) -> Result<PlaceReport, Error> {
    record_bytes(ctx, &mut report, added);
    Ok(report)
}

pub fn place(
    ctx: &Ctx<'_>,
    checkout: &Path,
    units: &[Row],
    window: Duration,
    dir_cap: usize,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<PlaceReport, Error> {
    place_selected(ctx, checkout, units, window, dir_cap, None, cancel)
}

/// An independently verified, initially absent slot output, using coincident Cargo roles.
pub struct SlotDestination<'a> {
    pub target: &'a Path,
    pub checkout_identity: cargo_hive_state::slots::FileIdentity,
    pub registry: &'a cargo_hive_cargo::registry::Registry,
    pub config: &'a cargo_hive_state::Config,
}

impl SlotDestination<'_> {
    fn scope_matches(&self, ctx: &Ctx<'_>) -> Result<bool, cargo_hive_state::StateError> {
        let current: cargo_hive_cargo::registry::Registry = ctx.state.read_regular_bounded(
            cargo_hive_cargo::registry::FILE,
            cargo_hive_state::slots::MAX_BYTES,
        )?;
        Ok(&current == self.registry && &ctx.state.slot_config()? == self.config)
    }

    fn create_root(
        &self,
        ctx: &Ctx<'_>,
        checkout: &Path,
        cancel: &(dyn Fn() -> bool + Sync),
    ) -> Result<io::Result<()>, Error> {
        ctx.state
            .slot_locked_or_cancelled(cancel, |tx| {
                let registry: cargo_hive_cargo::registry::Registry = tx.read_slot_registry()?;
                if cancel()
                    || &registry != self.registry
                    || &ctx.state.slot_config()? != self.config
                    || !self.checkout_matches(ctx.fs, checkout)
                    || !self
                        .target
                        .parent()
                        .is_some_and(|parent| parent.canonicalize().as_deref().ok() == Some(parent))
                    || !ctx
                        .fs
                        .entry_stat(self.target)
                        .is_err_and(|err| err.kind() == io::ErrorKind::NotFound)
                {
                    return Ok(Err(io::Error::other(
                        "the checked slot seed destination or owner scope changed",
                    )));
                }
                Ok(ctx.fs.create_dir(self.target))
            })
            .map_err(Error::State)
    }

    fn checkout_matches(&self, fs: &dyn Fs, checkout: &Path) -> bool {
        self.target != checkout
            && self.target.starts_with(checkout)
            && checkout.canonicalize().as_deref().ok() == Some(checkout)
            && fs.entry_stat(checkout).is_ok_and(|stat| {
                stat.kind == EntryKind::Dir
                    && (stat.dev, stat.ino)
                        == (self.checkout_identity.dev, self.checkout_identity.ino)
            })
    }
}

pub(crate) fn place_for_slot(
    ctx: &Ctx<'_>,
    checkout: &Path,
    units: &[Row],
    window: Duration,
    dir_cap: usize,
    destination: &SlotDestination<'_>,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<PlaceReport, Error> {
    place_selected(
        ctx,
        checkout,
        units,
        window,
        dir_cap,
        Some(destination),
        cancel,
    )
}

#[derive(Clone, Copy)]
struct PlacementRoot<'a> {
    path: &'a Path,
    guard: Option<&'a dyn Fn(&Path) -> bool>,
}

impl PlacementRoot<'_> {
    fn permits(self, directory: &Path) -> bool {
        self.guard.is_none_or(|check| check(directory))
    }
}

fn place_selected(
    ctx: &Ctx<'_>,
    checkout: &Path,
    units: &[Row],
    window: Duration,
    dir_cap: usize,
    destination: Option<&SlotDestination<'_>>,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<PlaceReport, Error> {
    if units.is_empty() {
        return finish(ctx, PlaceReport::default(), &mut Added::default());
    }
    let mut report = PlaceReport::default();
    let target = if let Some(destination) = destination {
        let ready = !cancel()
            && destination.checkout_matches(ctx.fs, checkout)
            && destination
                .target
                .parent()
                .is_some_and(|parent| parent.canonicalize().as_deref().ok() == Some(parent))
            && ctx
                .fs
                .entry_stat(destination.target)
                .is_err_and(|err| err.kind() == io::ErrorKind::NotFound);
        if !ready {
            return Err(Error::Failed(
                "the verified slot seed destination changed".into(),
            ));
        }
        destination.target.to_path_buf()
    } else {
        let resolved = match cargo_hive_cargo::metadata::resolve(checkout, cancel) {
            Ok(resolved) => resolved,
            Err(reason) => {
                report.findings.push((
                    "target.unresolved".to_string(),
                    Some(checkout.to_path_buf()),
                    format!(
                        "{checkout}: {reason}; nothing was cloned",
                        checkout = checkout.display()
                    ),
                ));
                return finish(ctx, report, &mut Added::default());
            }
        };
        if resolved.build_directory != resolved.target_directory {
            report.findings.push((
            "target.split".to_string(),
            Some(resolved.target_directory.clone()),
            format!(
                "the build directory {} differs from the target directory {}; nothing was cloned",
                resolved.build_directory.display(),
                resolved.target_directory.display()
            ),
        ));
            return finish(ctx, report, &mut Added::default());
        }
        resolved.target_directory
    };
    report.target_directory = Some(target.clone());
    let created = match destination {
        Some(scope) => scope.create_root(ctx, checkout, cancel)?,
        None => ctx.fs.create_dir(&target),
    };
    match created {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
            report.findings.push((
                "target.exists".to_string(),
                Some(target.clone()),
                format!(
                    "{path} already exists; seed leaves an existing target untouched",
                    path = target.display()
                ),
            ));
            return finish(ctx, report, &mut Added::default());
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            report.findings.push((
                "target.missing".to_string(),
                Some(target.clone()),
                format!(
                    "{path}: {err}; a parent of the target does not exist, so nothing was created",
                    path = target.display()
                ),
            ));
            return finish(ctx, report, &mut Added::default());
        }
        Err(err) => return Err(io_error(&target, &err)),
    }
    let stat = ctx.fs.entry_stat(&target).map_err(|err| {
        Error::Failed(format!(
            "{path}: {err}; the new target could not be identified, so run the command again",
            path = target.display()
        ))
    })?;
    report.target_identity = Some((stat.dev, stat.ino));
    let check = |directory: &Path| {
        destination.is_some_and(|destination| {
            !cancel()
                && destination.checkout_matches(ctx.fs, checkout)
                && directory.starts_with(&target)
                && directory.canonicalize().as_deref().ok() == Some(directory)
                && ctx.fs.entry_stat(&target).is_ok_and(|actual| {
                    actual.kind == EntryKind::Dir
                        && (actual.dev, actual.ino) == (stat.dev, stat.ino)
                })
        })
    };
    let root = PlacementRoot {
        path: &target,
        guard: destination.map(|_| &check as &dyn Fn(&Path) -> bool),
    };
    if !root.permits(&target) {
        return Err(Error::Failed("the created slot seed target changed".into()));
    }
    if let Some(scope) = destination
        && !scope.scope_matches(ctx).map_err(Error::State)?
    {
        return Err(Error::Failed(
            "the scoped seed registry or configuration changed before cloning".into(),
        ));
    }
    let mut added = Added {
        scoped_files: destination.map(|_| BTreeMap::new()),
        ..Added::default()
    };
    if let Err(stop) = clone_tag(ctx, &mut added, &target) {
        return match stop.into_result(&mut report, "tag.clone", &target) {
            Ok(report) => finish(ctx, report, &mut added),
            Err(err) => Err(err),
        };
    }
    let mut made = if destination.is_some() {
        let identity = directory_identity(&target).ok_or_else(|| {
            Error::Failed("the new slot seed target identity is unavailable".into())
        })?;
        if (identity.dev, identity.ino) != (stat.dev, stat.ino) {
            return Err(Error::Failed(
                "the new slot seed target identity changed".into(),
            ));
        }
        PlacementDirs::Scoped(BTreeMap::from([(target.clone(), identity)]))
    } else {
        PlacementDirs::Standalone(HashSet::from([target.clone()]))
    };
    for row in units {
        if cancel() {
            report.cancelled = true;
            break;
        }
        if let Some(scope) = destination
            && !scope.scope_matches(ctx).map_err(Error::State)?
        {
            return Err(Error::Failed(
                "the scoped seed registry or configuration changed".into(),
            ));
        }
        match place_unit(ctx, root, row, window, dir_cap, &mut made, &mut added) {
            Ok(UnitOutcome::Cloned(unit, outside)) => {
                report.cloned += 1;
                if outside {
                    report.outside_window += 1;
                }
                report.placed.push(unit);
            }
            Ok(UnitOutcome::Existing) => report.existing += 1,
            Ok(UnitOutcome::Failed(path, detail)) => {
                report.failed += 1;
                report
                    .findings
                    .push(("unit.refused".to_string(), Some(path), detail));
            }
            Err(why) => {
                let why = name_removal(&target, why);
                if report.placed.is_empty() {
                    return Err(Error::disk_full(why));
                }
                report.disk_full = Some(why);
                if destination.is_none() {
                    align_mtimes(ctx, &target, &mut report);
                }
                return finish(ctx, report, &mut added);
            }
        }
    }
    if !root.permits(&target) {
        return Err(Error::Failed(
            "the slot seed target changed before completion".into(),
        ));
    }
    match destination {
        Some(scope) if !report.cancelled && report.failed == 0 && report.existing == 0 => {
            align_scoped(ctx, scope, root, &made, &added, &mut report)?;
        }
        Some(_) => (),
        None => align_mtimes(ctx, &target, &mut report),
    }
    finish(ctx, report, &mut added)
}

/// Gives every placed file the newest mtime among them. A unit held from an
/// earlier build keeps that build's mtimes while one new in a later build is newer, and
/// Cargo calls a unit stale when a dependency's output is newer than its own. With one
/// mtime, no dependency is newer. The seed record's stamps take that mtime too.
fn align_mtimes(ctx: &Ctx<'_>, target: &Path, report: &mut PlaceReport) {
    let files = match list_files(ctx.fs, target, cargo_hive_core::caps::DEPS_CAP) {
        Ok(files) => files,
        Err(detail) => {
            report.findings.push((
                "mtime.align".to_string(),
                Some(target.to_path_buf()),
                format!("{}: {detail}", target.display()),
            ));
            return;
        }
    };
    let mut newest = None;
    for rel in &files {
        if let Ok(mtime) = ctx.fs.mtime(&target.join(rel)) {
            newest = newest.max(Some(mtime));
        }
    }
    let Some(newest) = newest else {
        return;
    };
    for rel in &files {
        let path = target.join(rel);
        if let Err(err) = ctx.fs.set_mtime(&path, newest) {
            report.findings.push((
                "mtime.align".to_string(),
                Some(path.clone()),
                format!(
                    "{}: {err}; Cargo may rebuild units that depend on it",
                    path.display()
                ),
            ));
        }
    }
    update_stamps(report, newest);
}

fn update_stamps(report: &mut PlaceReport, newest: SystemTime) {
    let Some(ns) = newest
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_nanos()).ok())
    else {
        return;
    };
    for unit in &mut report.placed {
        if let Some((_, stamp)) = unit.stamp.as_mut() {
            *stamp = ns;
        }
    }
}

fn align_scoped(
    ctx: &Ctx<'_>,
    scope: &SlotDestination<'_>,
    root: PlacementRoot<'_>,
    made: &PlacementDirs,
    added: &Added,
    report: &mut PlaceReport,
) -> Result<(), Error> {
    let unproved =
        || Error::Failed("the verified scoped seed files changed before alignment".into());
    let files = added.scoped_files.as_ref().ok_or_else(unproved)?;
    let newest = files
        .values()
        .map(|file| file.modified)
        .max()
        .ok_or_else(unproved)?;
    if newest > SystemTime::now() {
        return Err(unproved());
    }
    for (path, original) in files {
        let parent = path.parent().ok_or_else(unproved)?;
        if !root.permits(parent)
            || !made.matches(root.path, parent)
            || !scope.scope_matches(ctx).map_err(Error::State)?
        {
            return Err(unproved());
        }
        let file =
            cargo_hive_platform::flock::open_owner_state_file(path).map_err(|_| unproved())?;
        if PlacedFile::inspect(&file).map_err(|_| unproved())? != *original
            || !root.permits(parent)
            || !made.matches(root.path, parent)
        {
            return Err(unproved());
        }
        file.set_modified(newest).map_err(|_| unproved())?;
        let observed = PlacedFile::inspect(&file).map_err(|_| unproved())?;
        if observed.identity != original.identity
            || observed.mode != original.mode
            || observed.len != original.len
            || observed.modified != newest
        {
            return Err(unproved());
        }
    }
    update_stamps(report, newest);
    Ok(())
}

enum UnitOutcome {
    Cloned(SeededUnit, bool),
    Existing,
    Failed(PathBuf, String),
}

fn place_unit(
    ctx: &Ctx<'_>,
    root: PlacementRoot<'_>,
    row: &Row,
    window: Duration,
    dir_cap: usize,
    made: &mut PlacementDirs,
    added: &mut Added,
) -> Result<UnitOutcome, String> {
    let target = root.path;
    if !normal_only(&row.rel_profile) {
        return Ok(UnitOutcome::Failed(
            target.to_path_buf(),
            format!(
                "{} is not a path inside the target; this unit was not cloned",
                row.rel_profile.display()
            ),
        ));
    }
    let entry = store::entry_dir(ctx.state, row);
    let mut files = match list_files(ctx.fs, &entry, dir_cap) {
        Ok(files) => files,
        Err(detail) => return Ok(UnitOutcome::Failed(entry, detail)),
    };
    if let Some(rel) = files.iter().find(|rel| !normal_only(rel)) {
        return Ok(UnitOutcome::Failed(
            target.join(rel),
            format!(
                "{} is not a path inside the target; this unit was not cloned",
                rel.display()
            ),
        ));
    }
    files.sort_by(|left, right| {
        area(left, row.unit_layout)
            .cmp(&area(right, row.unit_layout))
            .then(left.cmp(right))
    });
    let mut deps_files = 0u32;
    let mut has_build = false;
    for rel in &files {
        let dst = target.join(rel);
        if !root.permits(target) {
            return Ok(UnitOutcome::Failed(
                dst,
                "the slot seed target changed before directory creation".into(),
            ));
        }
        if let Some(parent) = dst.parent()
            && let Err(stop) = made.ensure(ctx.fs, root.path, parent)
        {
            return match stop {
                Stop::Full(why) => Err(why),
                Stop::Failed(detail) => Ok(UnitOutcome::Failed(dst, detail)),
            };
        }
        if !dst
            .parent()
            .is_some_and(|parent| root.permits(parent) && made.matches(root.path, parent))
        {
            return Ok(UnitOutcome::Failed(
                dst,
                "the slot seed destination changed before cloning".into(),
            ));
        }
        let src = entry.join(rel);
        match ctx.fs.clone_file(&src, &dst) {
            Ok(()) => {
                if let Err(stop) = added.accept(ctx.fs, &dst) {
                    return match stop {
                        Stop::Full(why) => Err(why),
                        Stop::Failed(detail) => Ok(UnitOutcome::Failed(
                            dst,
                            format!("{detail}; this unit was not cloned"),
                        )),
                    };
                }
                if ctx.account_bytes {
                    charge(ctx.fs, added, &src, &dst);
                }
                match (row.unit_layout, area(rel, row.unit_layout)) {
                    (UnitLayout::Flat, 0) => deps_files = deps_files.saturating_add(1),
                    (UnitLayout::Flat, 1) => has_build = true,
                    (UnitLayout::UnitDir, 0) => deps_files = deps_files.saturating_add(1),
                    (UnitLayout::UnitDir, _) => {}
                    _ => {}
                }
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                return Ok(UnitOutcome::Existing);
            }
            Err(err) if is_full(&err) => return Err(format!("{}: {err}", dst.display())),
            Err(err) => {
                let detail = format!(
                    "{shown}: {err}; this unit was not cloned",
                    shown = dst.display()
                );
                return Ok(UnitOutcome::Failed(dst, detail));
            }
        }
    }
    if row.unit_layout == UnitLayout::UnitDir {
        has_build = false;
    }
    let stamp = read_stamp(ctx.fs, target, row);
    let outside = stamp_outside(stamp.as_ref(), ctx.now, window);
    Ok(UnitOutcome::Cloned(
        SeededUnit {
            rel_profile: row.rel_profile.clone(),
            dir_name: row.name_meta.clone(),
            stamp,
            has_build,
            deps_files,
            unit_layout: row.unit_layout,
        },
        outside,
    ))
}

fn clone_tag(ctx: &Ctx<'_>, added: &mut Added, target: &Path) -> Result<(), Stop> {
    let src = store::tag_path(ctx.state);
    let dst = target.join("CACHEDIR.TAG");
    match ctx.fs.clone_file(&src, &dst) {
        Ok(()) => {
            if let Err(stop) = added.accept(ctx.fs, &dst) {
                return Err(match stop {
                    Stop::Full(why) => Stop::Full(why),
                    Stop::Failed(detail) => Stop::Failed(format!(
                        "{detail}; the target tag was not cloned, so no unit was cloned"
                    )),
                });
            }
            if ctx.account_bytes {
                charge(ctx.fs, added, &src, &dst);
            }
            Ok(())
        }
        Err(err) if is_full(&err) => Err(Stop::Full(format!("{}: {err}", dst.display()))),
        Err(err) => Err(Stop::Failed(format!(
            "{}: {err}; the target tag was not cloned, so no unit was cloned",
            dst.display()
        ))),
    }
}

fn list_files(fs: &dyn Fs, root: &Path, dir_cap: usize) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    let mut stack = vec![(root.to_path_buf(), PathBuf::new(), 0usize)];
    while let Some((dir, rel, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            return Err(format!(
                "{} is deeper than {MAX_DEPTH} levels; this unit was not cloned",
                dir.display()
            ));
        }
        let mut entries = fs
            .read_dir_bounded(&dir, dir_cap)
            .map_err(|err| format!("{err}; this unit was not cloned"))?;
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        for (name, kind) in entries {
            let child_rel = rel.join(&name);
            if !normal_only(&child_rel) {
                return Err(format!(
                    "{} is not a path inside the target; this unit was not cloned",
                    child_rel.display()
                ));
            }
            let child = dir.join(&name);
            match kind {
                EntryKind::Dir => stack.push((child, child_rel, depth + 1)),
                EntryKind::File => files.push(child_rel),
                EntryKind::Symlink | EntryKind::Other => {
                    return Err(format!(
                        "{} is not a regular file; this unit was not cloned",
                        child.display()
                    ));
                }
            }
        }
    }
    Ok(files)
}

enum PlacementDirs {
    Standalone(HashSet<PathBuf>),
    Scoped(BTreeMap<PathBuf, cargo_hive_state::slots::FileIdentity>),
}

fn directory_identity(path: &Path) -> Option<cargo_hive_state::slots::FileIdentity> {
    if path.canonicalize().as_deref().ok() != Some(path) {
        return None;
    }
    cargo_hive_state::slots::owner_directory(path)
        .ok()
        .map(|metadata| cargo_hive_state::slots::FileIdentity::of(&metadata))
}

impl PlacementDirs {
    fn matches(&self, root: &Path, directory: &Path) -> bool {
        let Self::Scoped(made) = self else {
            return true;
        };
        scoped_chain_matches(root, directory, made)
    }

    fn ensure(&mut self, fs: &dyn Fs, root: &Path, directory: &Path) -> Result<(), Stop> {
        let made = match self {
            Self::Scoped(made) => made,
            Self::Standalone(made) => return ensure_dirs(fs, directory, root, made),
        };
        let unproved = || Stop::Failed("a checked slot seed ancestor changed".into());
        let relative = directory.strip_prefix(root).map_err(|_| unproved())?;
        if relative.components().count() > MAX_DEPTH || !normal_only(relative) {
            return Err(unproved());
        }
        let mut parent = root.to_path_buf();
        for part in relative.components() {
            if !scoped_chain_matches(root, &parent, made) {
                return Err(unproved());
            }
            let path = parent.join(part.as_os_str());
            if let Some(expected) = made.get(&path) {
                if directory_identity(&path).as_ref() != Some(expected) {
                    return Err(unproved());
                }
            } else {
                match fs.create_dir(&path) {
                    Ok(()) => (),
                    Err(err) if is_full(&err) => return Err(Stop::Full(err.to_string())),
                    Err(_) => return Err(unproved()),
                }
                let identity = directory_identity(&path).ok_or_else(unproved)?;
                let target = made.get(root).ok_or_else(unproved)?;
                if identity.dev != target.dev {
                    return Err(unproved());
                }
                made.insert(path.clone(), identity);
            }
            parent = path;
        }
        if scoped_chain_matches(root, directory, made) {
            Ok(())
        } else {
            Err(unproved())
        }
    }
}

fn scoped_chain_matches(
    root: &Path,
    directory: &Path,
    made: &BTreeMap<PathBuf, cargo_hive_state::slots::FileIdentity>,
) -> bool {
    directory.starts_with(root)
        && directory
            .ancestors()
            .take_while(|path| path.starts_with(root))
            .all(|path| {
                made.get(path)
                    .is_some_and(|id| directory_identity(path).as_ref() == Some(id))
            })
}

fn ensure_dirs(
    fs: &dyn Fs,
    dir: &Path,
    target: &Path,
    made: &mut HashSet<PathBuf>,
) -> Result<(), Stop> {
    if made.contains(dir) {
        return Ok(());
    }
    let mut chain = Vec::new();
    let mut cur = dir.to_path_buf();
    loop {
        if cur == target || made.contains(&cur) {
            break;
        }
        if chain.len() > MAX_DEPTH {
            return Err(Stop::Failed(format!(
                "{} is deeper than {MAX_DEPTH} levels; this unit was not cloned",
                dir.display()
            )));
        }
        let Some(parent) = cur.parent().map(Path::to_path_buf) else {
            return Err(Stop::Failed(format!(
                "{} is not inside {}; this unit was not cloned",
                dir.display(),
                target.display()
            )));
        };
        chain.push(cur);
        cur = parent;
    }
    for path in chain.iter().rev() {
        match fs.create_dir(path) {
            Ok(()) => {
                made.insert(path.clone());
            }
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                made.insert(path.clone());
            }
            Err(err) if is_full(&err) => {
                return Err(Stop::Full(format!("{}: {err}", path.display())));
            }
            Err(err) => {
                return Err(Stop::Failed(format!(
                    "{}: {err}; this unit was not cloned",
                    path.display()
                )));
            }
        }
    }
    Ok(())
}

fn read_stamp(fs: &dyn Fs, target: &Path, row: &Row) -> Option<(PathBuf, u64)> {
    let loc = UnitLoc {
        layout: row.unit_layout,
        name_meta: row.name_meta.clone(),
    };
    let absolute = liveness::unit_timestamp(
        &target.join(&row.rel_profile),
        &loc,
        fingerprint::UnitKind::of(&row.kind),
    );
    let mtime = fs.mtime(&absolute).ok()?;
    let ns = mtime.duration_since(UNIX_EPOCH).ok()?.as_nanos();
    let ns = u64::try_from(ns).ok()?;
    let rel = absolute
        .strip_prefix(target)
        .map(Path::to_path_buf)
        .unwrap_or(absolute);
    Some((rel, ns))
}

fn stamp_outside(stamp: Option<&(PathBuf, u64)>, now: SystemTime, window: Duration) -> bool {
    let Some((_, ns)) = stamp else {
        return true;
    };
    let Some(mtime) = UNIX_EPOCH.checked_add(Duration::from_nanos(*ns)) else {
        return true;
    };
    match now.checked_sub(window) {
        None => false,
        Some(cutoff) => mtime < cutoff,
    }
}

fn normal_only(path: &Path) -> bool {
    path.components()
        .all(|component| matches!(component, Component::Normal(_)))
}

fn area(rel: &Path, layout: UnitLayout) -> u8 {
    match layout {
        UnitLayout::Flat => {
            for component in rel.components() {
                match component.as_os_str().to_str() {
                    Some("deps") => return 0,
                    Some("build") => return 1,
                    Some(".fingerprint") => return 2,
                    _ => {}
                }
            }
            3
        }
        UnitLayout::UnitDir => {
            let Some(root) = unit_path::unit_dir_of(rel) else {
                return 3;
            };
            let Ok(rest) = rel.strip_prefix(&root) else {
                return 3;
            };
            match rest
                .components()
                .next()
                .and_then(|c| c.as_os_str().to_str())
            {
                Some("out") => 0,
                Some("run") => 1,
                Some("fingerprint") => 2,
                _ => 3,
            }
        }
    }
}

fn is_full(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::StorageFull
}

fn name_removal(target: &Path, detail: impl Into<String>) -> String {
    format!(
        "{}; remove {} before running seed again",
        detail.into(),
        target.display()
    )
}

fn io_error(path: &Path, err: &io::Error) -> Error {
    let detail = format!(
        "{path}: {err}; the target was not created, so run the command again",
        path = path.display()
    );
    if is_full(err) {
        Error::disk_full(detail)
    } else {
        Error::Failed(detail)
    }
}

impl Stop {
    fn into_result(
        self,
        report: &mut PlaceReport,
        code: &str,
        path: &Path,
    ) -> Result<PlaceReport, Error> {
        match self {
            Stop::Full(why) => Err(Error::disk_full(name_removal(path, why))),
            Stop::Failed(detail) => {
                report
                    .findings
                    .push((code.to_string(), Some(path.to_path_buf()), detail));
                Ok(std::mem::take(report))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::expand_capped;
    use cargo_hive_platform::testing::Hooked;
    use std::fs;
    use std::os::unix::fs::MetadataExt;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn scratch(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = PathBuf::from(format!(
            "/private/tmp/hive-it-expand-{label}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn scoped_seed_does_not_create_through_a_replaced_ancestor() {
        let proof = cargo_hive_platform::TempDir::adopt(scratch("seed-ancestor"));
        let root = proof.path().join("target");
        let profile = root.join("debug");
        let outside = proof.path().join("outside");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&profile).unwrap();
        fs::create_dir(&outside).unwrap();
        let mut made = super::PlacementDirs::Scoped(std::collections::BTreeMap::from([
            (root.clone(), super::directory_identity(&root).unwrap()),
            (
                profile.clone(),
                super::directory_identity(&profile).unwrap(),
            ),
        ]));
        fs::rename(&profile, root.join("retired")).unwrap();
        std::os::unix::fs::symlink(&outside, &profile).unwrap();
        let fs = cargo_hive_platform::host_fs();
        let result = made.ensure(fs.as_ref(), &root, &profile.join("deps"));
        assert!(
            !outside.join("deps").exists(),
            "scoped placement created through an alias"
        );
        assert!(result.is_err());
    }

    #[test]
    fn scoped_alignment_refuses_a_replaced_file_without_touching_its_destination() {
        let scratch = cargo_hive_platform::TempDir::adopt(scratch("seed-alignment"));
        let target = scratch.path().join("target");
        fs::create_dir(&target).unwrap();
        let output = target.join("confirmed-clone");
        let sentinel = scratch.path().join("outside");
        fs::write(&output, b"confirmed clone").unwrap();
        fs::write(&sentinel, b"preserve me").unwrap();
        let before = fs::metadata(&sentinel).unwrap().modified().unwrap();
        let state = cargo_hive_state::State::open_at(scratch.path().join("hive")).unwrap();
        let config = state.slot_config().unwrap();
        let registry = cargo_hive_cargo::registry::Registry::default();
        let native = cargo_hive_platform::host_fs();
        let ctx = cargo_hive_cargo::ctx::Ctx {
            state: &state,
            fs: native.as_ref(),
            cfg: &config,
            now: SystemTime::now(),
            account_bytes: false,
            repos: &[],
            mains: &[],
            liveness: crate::no_liveness,
        };
        let identity = super::directory_identity(&target).unwrap();
        let scope = super::SlotDestination {
            target: &target,
            checkout_identity: super::directory_identity(scratch.path()).unwrap(),
            registry: &registry,
            config: &config,
        };
        let dirs = super::PlacementDirs::Scoped(std::collections::BTreeMap::from([(
            target.clone(),
            identity,
        )]));
        let root = super::PlacementRoot {
            path: &target,
            guard: Some(&|parent| parent == target),
        };
        let mut added = super::Added {
            scoped_files: Some(std::collections::BTreeMap::new()),
            ..super::Added::default()
        };
        assert!(added.accept(native.as_ref(), &output).is_ok());
        fs::rename(&output, target.join("original-inode")).unwrap();
        std::os::unix::fs::symlink(&sentinel, &output).unwrap();
        let mut report = super::PlaceReport::default();
        assert!(super::align_scoped(&ctx, &scope, root, &dirs, &added, &mut report).is_err());
        assert_eq!(fs::metadata(&sentinel).unwrap().modified().unwrap(), before);
        assert_eq!(fs::read(&sentinel).unwrap(), b"preserve me");
    }

    fn pretend_compressed(inner: Box<dyn cargo_hive_platform::Fs>) -> Hooked {
        let once = Mutex::new(std::collections::HashSet::<PathBuf>::new());
        Hooked {
            inner,
            compress: None,
            volume: None,
            sharing: None,
            entry_stat: Some(Box::new(move |path| {
                let real = cargo_hive_platform::current().unwrap().fs;
                let mut stat = real.entry_stat(path)?;
                if once.lock().unwrap().insert(path.to_path_buf()) {
                    stat.compressed = true;
                }
                Ok(stat)
            })),
        }
    }

    fn plant(dir: &Path, name: &str, len: usize) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, vec![7u8; len]).unwrap();
        let when = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(when)
            .unwrap();
        path
    }

    #[test]
    fn a_small_cap_accepts_that_many_bytes_and_rejects_one_more() {
        let root = scratch("cap");
        let cap = 4u64;
        let exact = plant(&root, "exact", cap as usize);
        let over = plant(&root, "over", (cap + 1) as usize);
        let over_ino = fs::symlink_metadata(&over).unwrap().ino();
        let over_bytes = fs::read(&over).unwrap();
        let fs = pretend_compressed(cargo_hive_platform::current().unwrap().fs);
        expand_capped(&fs, &exact, cap).unwrap();
        assert_eq!(fs::read(&exact).unwrap(), vec![7u8; cap as usize]);
        assert_eq!(
            fs::symlink_metadata(&exact).unwrap().modified().unwrap(),
            UNIX_EPOCH + Duration::from_secs(1_700_000_000)
        );
        let err = expand_capped(&fs, &over, cap).unwrap_err();
        let text = err.to_string();
        assert!(text.contains(&cap.to_string()), "{text}");
        assert!(!text.contains("1073741824"), "{text}");
        assert_eq!(fs::read(&over).unwrap(), over_bytes);
        assert_eq!(fs::symlink_metadata(&over).unwrap().ino(), over_ino);
        assert!(!root.join(".hive-expand-over").exists());
        let _ = fs::remove_dir_all(&root);
    }
}
