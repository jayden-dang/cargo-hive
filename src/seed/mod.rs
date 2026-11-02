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
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::discover::Checkout;
use crate::doctor::Toolchain;
use crate::gc::CRATE_DIR_CAP;
use crate::plan_tier::Ctx;
use crate::registry::Repo;
use crate::state::window_days;

use choose::Choice;
use fill::FillReport;
use place::{PlaceReport, SeededUnit};
use store::tag_path;

pub mod choose;
pub mod fill;
pub mod place;
pub mod record;
pub mod source;
pub mod store;

use std::fmt;

use crate::state::StateError;

#[derive(Debug)]
pub enum Error {
    State(StateError),
    DiskFull(String),
    Failed(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::State(err) => write!(f, "{err}"),
            Error::DiskFull(why) => f.write_str(&disk_full(why)),
            Error::Failed(why) => f.write_str(why),
        }
    }
}

pub(crate) fn disk_full(why: &str) -> String {
    format!("the disk is full ({why}); free space and run the command again")
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

pub fn run(
    ctx: &Ctx<'_>,
    repo: &Repo,
    checkout: &Checkout,
    toolchain: &Toolchain,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<Outcome, Error> {
    let (targets, faults) = source::owned_targets(ctx, cancel);
    let mut findings = Vec::new();
    for fault in faults {
        findings.push(("walk.unreadable".to_string(), None, fault));
    }
    if cancel() {
        return Ok(from_fill(&empty_fill(), findings, true, 0));
    }
    let mut filled = fill::fill(ctx, repo, &targets, cancel)?;
    let cancelled = filled.cancelled;
    findings.append(&mut filled.findings);
    if cancelled || cancel() {
        return Ok(from_fill(&filled, findings, true, 0));
    }
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
        return Ok(from_fill(&filled, findings, false, 0));
    }
    let stored = store::read(ctx.state).map_err(Error::State)?;
    let (rows, dropped) = match choose::choose(&stored, &repo.main, &toolchain.version_text) {
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
            return Ok(from_fill(&filled, findings, false, 0));
        }
        Choice::Units { rows, dropped } if rows.is_empty() => {
            findings.push((
                "units.empty".to_string(),
                None,
                "the store has no unit to clone for this checkout, so nothing was created"
                    .to_string(),
            ));
            return Ok(from_fill(&filled, findings, false, dropped));
        }
        Choice::Units { rows, dropped } => (rows, dropped),
    };
    let window = Duration::from_secs(days.saturating_mul(86_400));
    let placed = place::place(ctx, &checkout.path, &rows, window, CRATE_DIR_CAP, cancel)?;
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
    Err(Error::DiskFull(why))
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
    filled: &FillReport,
    findings: Vec<(String, Option<PathBuf>, String)>,
    cancelled: bool,
    dropped: u64,
) -> Outcome {
    Outcome {
        counts: clone_counts(filled, 0, 0, 0, dropped, 0),
        findings,
        unmeasured: unmeasured_bytes(),
        notes: Vec::new(),
        cancelled,
        placed: Vec::new(),
        target: None,
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

fn unmeasured_bytes() -> Vec<(String, String)> {
    vec![
        (
            "clone_bytes_not_shared".to_string(),
            "the bytes this clone adds were not measured".to_string(),
        ),
        (
            "store_bytes".to_string(),
            "the store's size was not measured".to_string(),
        ),
    ]
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
