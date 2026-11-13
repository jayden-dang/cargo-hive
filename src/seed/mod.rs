//! The seed command's library entry: the live registry units of a target, the
//! compressed store those units are published into, the fill that publishes them,
//! the choice of units for one repo, and the placement of those units into a
//! target that does not exist yet.
//!
//! `run` fills first, then clones, then records what it placed. A missing store
//! tag or toolchain row is one finding and clones nothing. `clone_units_chosen`
//! counts the units before the ones with an open edge are dropped, so it is
//! cloned plus existing plus failed plus dropped.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::ctx::Ctx;
use crate::discover::Checkout;
use crate::gc::CRATE_DIR_CAP;
use crate::registry::Repo;
use crate::state::window_days;
use crate::toolchain::Toolchain;

use choose::Choice;
use fill::FillReport;
use place::{PlaceReport, SeededUnit};
use store::tag_path;

pub mod choose;
#[doc(hidden)]
pub mod evict;
pub mod fill;
pub mod place;
pub mod record;
pub mod source;
pub mod store;

use std::fmt;

use crate::state::{StateError, StoreLock};

#[derive(Debug)]
pub struct FullDisk {
    pub detail: String,
    pub kept: Option<Outcome>,
}

impl FullDisk {
    pub fn new(detail: impl Into<String>) -> FullDisk {
        FullDisk {
            detail: detail.into(),
            kept: None,
        }
    }
}

#[derive(Debug)]
pub enum Error {
    State(StateError),
    DiskFull(Box<FullDisk>),
    Failed(String),
}

impl Error {
    pub(crate) fn disk_full(detail: impl Into<String>) -> Error {
        Error::DiskFull(Box::new(FullDisk::new(detail)))
    }

    pub(crate) fn disk_full_kept(detail: impl Into<String>, outcome: Outcome) -> Error {
        Error::DiskFull(Box::new(FullDisk {
            detail: detail.into(),
            kept: Some(outcome),
        }))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::State(err) => write!(f, "{err}"),
            Error::DiskFull(full) => f.write_str(&disk_full(&full.detail)),
            Error::Failed(why) => f.write_str(why),
        }
    }
}

pub(crate) fn disk_full(why: &str) -> String {
    format!("the disk is full ({why}); free space and run the command again")
}

pub(crate) fn directory_listing(dir: &Path, err: &io::Error, again: &str) -> String {
    let step = if listing_is_cap(err) {
        "shrink the directory below its cap"
    } else if err.kind() == io::ErrorKind::PermissionDenied {
        "make the directory readable"
    } else if err.kind() == io::ErrorKind::NotADirectory {
        "remove the path that is not a directory"
    } else {
        "fix that error"
    };
    format!("{}: {err}; {step} and {again}", dir.display())
}

fn listing_is_cap(err: &io::Error) -> bool {
    let text = err.to_string();
    text.contains("has more than") && text.contains("entries")
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::State(err) => Some(err),
            Error::DiskFull(_) | Error::Failed(_) => None,
        }
    }
}

#[derive(Debug)]
pub struct Outcome {
    pub counts: BTreeMap<String, u64>,
    pub findings: Vec<(String, Option<PathBuf>, String)>,
    pub unmeasured: Vec<(String, String)>,
    pub notes: Vec<String>,
    pub cancelled: bool,
    pub placed: Vec<SeededUnit>,
    pub target: Option<(PathBuf, (u64, u64))>,
}

pub fn exit_code(result: &Result<Outcome, Error>) -> i32 {
    match result {
        Ok(outcome) if outcome.cancelled => 130,
        Ok(_) => 0,
        Err(_) => 1,
    }
}

pub fn record_seed_result(
    receipt: &mut crate::receipt::ReceiptBuilder,
    result: &Result<Outcome, Error>,
) {
    match result {
        Ok(outcome) => copy_outcome(receipt, outcome),
        Err(Error::DiskFull(full)) => {
            if let Some(outcome) = &full.kept {
                copy_outcome(receipt, outcome);
                let path = outcome.target.as_ref().map(|(path, _)| path.clone());
                receipt.finding("error", path, result_text(result));
            } else {
                receipt.finding("error", None, result_text(result));
            }
        }
        Err(_) => {
            receipt.finding("error", None, result_text(result));
        }
    }
}

fn result_text(result: &Result<Outcome, Error>) -> String {
    match result {
        Ok(_) => String::new(),
        Err(err) => err.to_string(),
    }
}

fn copy_outcome(receipt: &mut crate::receipt::ReceiptBuilder, outcome: &Outcome) {
    for (name, n) in &outcome.counts {
        receipt.count(name, *n);
    }
    for (field, reason) in &outcome.unmeasured {
        receipt.unmeasured(field, reason.clone());
    }
    for note in &outcome.notes {
        receipt.note(note.clone());
    }
    for (kind, path, detail) in &outcome.findings {
        receipt.finding(kind, path.clone(), detail.clone());
    }
}

pub enum Fill {
    Continue(Filled),
    Stopped(Outcome),
}

pub struct Filled {
    report: FillReport,
    findings: Vec<(String, Option<PathBuf>, String)>,
    // Held until clone finishes so eviction cannot run mid-seed.
    store_lock: StoreLock,
}

impl Filled {
    pub fn report(&self) -> &fill::FillReport {
        &self.report
    }

    pub fn findings(&self) -> &[(String, Option<PathBuf>, String)] {
        &self.findings
    }
}

pub fn run_fill(
    ctx: &Ctx<'_>,
    repo: &Repo,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Fill, Error> {
    let (targets, mut findings) = source::owned_targets(ctx, cancel);
    if cancel() {
        return Ok(Fill::Stopped(from_fill(
            ctx,
            &empty_fill(),
            findings,
            true,
            0,
        )));
    }
    let store_lock = match ctx.state.store_shared_or_cancelled(cancel) {
        Ok(lock) => lock,
        Err(StateError::Cancelled) => {
            return Ok(Fill::Stopped(from_fill(
                ctx,
                &empty_fill(),
                findings,
                true,
                0,
            )));
        }
        Err(err) => return Err(Error::State(err)),
    };
    let mut filled = fill::fill(ctx, repo, &targets, cancel)?;
    let cancelled = filled.cancelled;
    findings.append(&mut filled.findings);
    if cancelled || cancel() {
        return Ok(Fill::Stopped(from_fill(ctx, &filled, findings, true, 0)));
    }
    Ok(Fill::Continue(Filled {
        report: filled,
        findings,
        store_lock,
    }))
}

pub fn run_clone(
    ctx: &Ctx<'_>,
    repo: &Repo,
    checkout: &Checkout,
    toolchain: &Toolchain,
    filled: Filled,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Outcome, Error> {
    let Filled {
        report: filled,
        mut findings,
        store_lock,
    } = filled;
    let days = match window_days(ctx.cfg.gc_window_days) {
        Ok(days) => days,
        Err(err) => return Err(Error::Failed(err.to_string())),
    };
    let tag = tag_path(ctx.state);
    if let Err(err) = std::fs::symlink_metadata(&tag) {
        let detail = if err.kind() == std::io::ErrorKind::NotFound {
            format!(
                "{path} is missing, so nothing was cloned; fill a source target and run the command again",
                path = tag.display()
            )
        } else {
            format!(
                "{path}: {err}; the store tag could not be read, so nothing was cloned",
                path = tag.display()
            )
        };
        findings.push(("store.tag".to_string(), Some(tag.clone()), detail));
        return Ok(from_fill(ctx, &filled, findings, false, 0));
    }
    let stored = store::read(ctx.state).map_err(Error::State)?;
    let lane_lock = read_lock_file(&checkout.path);
    let order = choose::source_order(&stored, &repo.main, &|record| {
        lane_lock.is_some() && read_lock_file(&record.checkout) == lane_lock
    });
    let (rows, dropped) = match choose::choose(&stored, &repo.main, &toolchain.version_text, &order)
    {
        Choice::NoSource => {
            findings.push((
                "source.none".to_string(),
                None,
                "no source target of this repository has a record for this toolchain, so nothing was cloned; build a checkout of the repository and run the command again".to_string(),
            ));
            return Ok(from_fill(ctx, &filled, findings, false, 0));
        }
        Choice::NoRow { release } => {
            let detail = if release.is_empty() {
                "the store has no toolchain row for this version text, so nothing was cloned"
                    .to_string()
            } else {
                format!(
                    "the store has no toolchain row for release {release}, so nothing was cloned"
                )
            };
            findings.push(("toolchain.missing".to_string(), None, detail));
            return Ok(from_fill(ctx, &filled, findings, false, 0));
        }
        Choice::Units { rows, dropped } if rows.is_empty() => {
            findings.push((
                "units.empty".to_string(),
                None,
                "the store has no unit to clone for this checkout, so nothing was created"
                    .to_string(),
            ));
            return Ok(from_fill(ctx, &filled, findings, false, dropped));
        }
        Choice::Units { rows, dropped } => (rows, dropped),
    };
    let window = Duration::from_secs(days.saturating_mul(86_400));
    let placed = place::place(ctx, &checkout.path, &rows, window, CRATE_DIR_CAP, cancel)?;
    drop(store_lock);
    let disk_full = placed.disk_full.clone();
    let outcome = record_placed(ctx, assemble(&filled, placed, dropped, findings), cancel);
    let Some(why) = disk_full else {
        return Ok(outcome);
    };
    let why = match outcome
        .findings
        .iter()
        .find(|(kind, _, _)| kind == "record.failed")
    {
        Some((_, _, detail)) => format!("{why}; {detail}"),
        None => why,
    };
    Err(Error::disk_full_kept(why, outcome))
}

pub fn run(
    ctx: &Ctx<'_>,
    repo: &Repo,
    checkout: &Checkout,
    toolchain: &Toolchain,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Outcome, Error> {
    match run_fill(ctx, repo, cancel)? {
        Fill::Stopped(outcome) => Ok(outcome),
        Fill::Continue(filled) => run_clone(ctx, repo, checkout, toolchain, filled, cancel),
    }
}

fn empty_fill() -> FillReport {
    FillReport {
        entries_added: 0,
        entries_held: 0,
        targets_read: 0,
        targets_skipped: BTreeMap::new(),
        findings: Vec::new(),
        cancelled: false,
    }
}

fn from_fill(
    ctx: &Ctx<'_>,
    filled: &FillReport,
    findings: Vec<(String, Option<PathBuf>, String)>,
    cancelled: bool,
    dropped: u64,
) -> Outcome {
    let mut counts = clone_counts(filled, 0, 0, 0, dropped, 0);
    let mut unmeasured = Vec::new();
    if ctx.account_bytes {
        counts.insert("clone_bytes_not_shared".to_string(), 0);
    } else {
        unmeasured.push((
            "clone_bytes_not_shared".to_string(),
            "doctor check (c) did not pass".to_string(),
        ));
    }
    match measured_store(ctx) {
        Ok(bytes) => {
            counts.insert("store_bytes".to_string(), bytes);
        }
        Err(err) => unmeasured.push((
            "store_bytes".to_string(),
            format!("{err}; the store's size was not measured"),
        )),
    }
    Outcome {
        counts,
        findings,
        unmeasured,
        notes: Vec::new(),
        cancelled,
        placed: Vec::new(),
        target: None,
    }
}

fn measured_store(ctx: &Ctx<'_>) -> Result<u64, crate::size::SizeError> {
    let root = store::store_dir(ctx.state);
    match crate::size::allocated_bytes(&root, crate::size::MAX_DEPTH) {
        Err(crate::size::SizeError::Io(path, err))
            if path == root && err.kind() == io::ErrorKind::NotFound =>
        {
            Ok(0)
        }
        other => other,
    }
}

fn assemble(
    filled: &FillReport,
    placed: PlaceReport,
    dropped: u64,
    mut findings: Vec<(String, Option<PathBuf>, String)>,
) -> Outcome {
    findings.extend(placed.findings);
    let mut counts = clone_counts(
        filled,
        placed.cloned,
        placed.existing,
        placed.failed,
        dropped,
        placed.outside_window,
    );
    let mut unmeasured = placed.unmeasured;
    match placed.bytes_not_shared {
        Some(bytes) => {
            counts.insert("clone_bytes_not_shared".to_string(), bytes);
        }
        None => push_unmeasured(
            &mut unmeasured,
            "clone_bytes_not_shared",
            "the bytes this clone adds were not measured",
        ),
    }
    match placed.store_bytes {
        Some(bytes) => {
            counts.insert("store_bytes".to_string(), bytes);
        }
        None => push_unmeasured(
            &mut unmeasured,
            "store_bytes",
            "the store's size was not measured",
        ),
    }
    let target = placed.target_directory.zip(placed.target_identity);
    Outcome {
        counts,
        findings,
        unmeasured,
        notes: Vec::new(),
        cancelled: placed.cancelled,
        placed: placed.placed,
        target,
    }
}

fn record_placed(
    ctx: &Ctx<'_>,
    mut outcome: Outcome,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Outcome {
    if outcome.placed.is_empty() {
        return outcome;
    }
    let Some((target, identity)) = outcome.target.clone() else {
        return outcome;
    };
    let stored = record::SeedRecord {
        identity,
        written_at: epoch_secs(ctx.now),
        units: outcome.placed.clone(),
    };
    match record::write(ctx.state, &target, stored, cancel) {
        Ok(Some(note)) => outcome.notes.push(note),
        Ok(None) => {}
        Err(err) => outcome.findings.push((
            "record.failed".to_string(),
            Some(target),
            format!("{err}; this seed was not recorded, so status reports nothing for the target"),
        )),
    }
    outcome
}

fn epoch_secs(now: SystemTime) -> u64 {
    now.duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn push_unmeasured(unmeasured: &mut Vec<(String, String)>, field: &str, reason: &str) {
    if unmeasured.iter().any(|(have, _)| have == field) {
        return;
    }
    unmeasured.push((field.to_string(), reason.to_string()));
}

fn clone_counts(
    filled: &FillReport,
    cloned: u64,
    existing: u64,
    failed: u64,
    dropped: u64,
    outside_window: u64,
) -> BTreeMap<String, u64> {
    let mut counts = BTreeMap::new();
    counts.insert("fill_entries_added".to_string(), filled.entries_added);
    counts.insert("fill_entries_held".to_string(), filled.entries_held);
    counts.insert("fill_targets_read".to_string(), filled.targets_read);
    let mut skipped = 0u64;
    for (name, n) in &filled.targets_skipped {
        skipped = skipped.saturating_add(*n);
        counts.insert((*name).to_string(), *n);
    }
    counts.insert("fill_targets_skipped".to_string(), skipped);
    counts.insert(
        "clone_units_chosen".to_string(),
        cloned
            .saturating_add(existing)
            .saturating_add(failed)
            .saturating_add(dropped),
    );
    counts.insert("clone_units_cloned".to_string(), cloned);
    counts.insert("clone_units_dropped".to_string(), dropped);
    counts.insert("clone_units_existing".to_string(), existing);
    counts.insert("clone_units_failed".to_string(), failed);
    counts.insert("clone_units_outside_window".to_string(), outside_window);
    counts
}

/// A checkout's `Cargo.lock`, when it is a regular file of at most 16 MiB.
fn read_lock_file(checkout: &std::path::Path) -> Option<Vec<u8>> {
    let path = checkout.join("Cargo.lock");
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > 16 * 1024 * 1024 {
        return None;
    }
    std::fs::read(&path).ok()
}

#[cfg(test)]
mod tests {
    use super::{empty_fill, from_fill, record_seed_result, Error, Outcome};
    use crate::ctx::Ctx;
    use crate::state::{Config, State};
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = PathBuf::from(format!(
            "/private/tmp/hive-it-seed-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn ctx_at(root: &std::path::Path) -> (State, crate::platform::Platform, Config) {
        let state = State::open_at(root.join("home")).unwrap();
        let platform = crate::platform::current().unwrap();
        let cfg = Config {
            secondary_roots: Vec::new(),
            cache_roots: Vec::new(),
            gc_window_days: None,
        };
        (state, platform, cfg)
    }

    fn outcome_of(
        state: &State,
        platform: &crate::platform::Platform,
        cfg: &Config,
        account_bytes: bool,
    ) -> Outcome {
        let repos = [];
        let mains = [];
        let ctx = Ctx {
            state,
            fs: platform.fs.as_ref(),
            cfg,
            now: UNIX_EPOCH,
            account_bytes,
            repos: &repos,
            mains: &mains,
        };
        from_fill(&ctx, &empty_fill(), Vec::new(), false, 0)
    }

    #[test]
    fn a_finish_before_place_counts_no_clone_bytes_and_the_store() {
        let root = scratch("early");
        let (state, platform, cfg) = ctx_at(&root);
        let on = outcome_of(&state, &platform, &cfg, true);
        assert_eq!(on.counts.get("clone_bytes_not_shared").copied(), Some(0));
        assert_eq!(on.counts.get("store_bytes").copied(), Some(0));
        assert!(on.unmeasured.is_empty(), "{:?}", on.unmeasured);
        let off = outcome_of(&state, &platform, &cfg, false);
        assert!(!off.counts.contains_key("clone_bytes_not_shared"));
        assert_eq!(off.counts.get("store_bytes").copied(), Some(0));
        assert!(off.unmeasured.iter().any(|(field, reason)| {
            field == "clone_bytes_not_shared" && reason == "doctor check (c) did not pass"
        }));
        let store = crate::seed::store::store_dir(&state);
        std::fs::write(&store, b"not a directory").unwrap();
        let blocked = outcome_of(&state, &platform, &cfg, true);
        assert_eq!(
            blocked.counts.get("clone_bytes_not_shared").copied(),
            Some(0)
        );
        assert!(!blocked.counts.contains_key("store_bytes"));
        assert!(blocked.unmeasured.iter().any(|(field, reason)| {
            field == "store_bytes" && reason.contains("the store's size was not measured")
        }));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_full_disk_keeps_the_counts_and_names_the_directory() {
        let target = PathBuf::from("/private/tmp/hive-it-kept-target");
        let mut counts = BTreeMap::new();
        counts.insert("clone_units_cloned".to_string(), 2);
        counts.insert("fill_entries_added".to_string(), 3);
        let outcome = Outcome {
            counts,
            findings: vec![("unit.refused".to_string(), None, "left behind".to_string())],
            unmeasured: vec![(
                "clone_bytes_not_shared".to_string(),
                "not measured".to_string(),
            )],
            notes: vec!["kept".to_string()],
            cancelled: false,
            placed: Vec::new(),
            target: Some((target.clone(), (1, 2))),
        };
        let err = Error::disk_full_kept(
            format!(
                "no space left; remove {} before running seed again",
                target.display()
            ),
            outcome,
        );
        let mut receipt = crate::receipt::Receipt::new("seed", crate::receipt::Scope::default());
        record_seed_result(&mut receipt, &Err(err));
        let receipt = receipt.finish();
        assert_eq!(receipt.counts.get("clone_units_cloned").copied(), Some(2));
        assert_eq!(receipt.counts.get("fill_entries_added").copied(), Some(3));
        assert!(receipt
            .unmeasured
            .iter()
            .any(|row| row.field == "clone_bytes_not_shared"));
        assert_eq!(receipt.notes, vec!["kept".to_string()]);
        assert!(receipt
            .findings
            .iter()
            .any(|finding| finding.kind == "unit.refused"));
        let error = receipt
            .findings
            .iter()
            .find(|finding| finding.kind == "error")
            .expect("error finding");
        assert_eq!(error.path.as_deref(), Some(target.as_path()));
        let detail = &error.detail;
        assert!(detail.contains("free space"), "{detail}");
        assert!(detail.contains("run the command again"), "{detail}");
        assert!(detail.contains("remove"), "{detail}");
        assert!(detail.contains(&target.display().to_string()), "{detail}");
        assert!(detail.contains("before running seed again"), "{detail}");
    }
}
