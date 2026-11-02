//! The seed command's library entry: the live registry units of a target, the
//! compressed store those units are published into, the fill that publishes them,
//! the choice of units for one repo, and the placement of those units into a
//! target that does not exist yet.
//!
//! `run` fills first, then clones. A missing store tag or toolchain row is one
//! finding and clones nothing. `clone_units_chosen` counts the units before the
//! ones with an open edge are dropped, so it is cloned plus existing plus failed
//! plus dropped.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

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
        return Ok(from_fill(&empty_fill(), findings, true));
    }
    let mut filled = fill::fill(ctx, repo, &targets, cancel)?;
    let cancelled = filled.cancelled;
    findings.append(&mut filled.findings);
    if cancelled || cancel() {
        return Ok(from_fill(&filled, findings, true));
    }
    let Ok(days) = window_days(ctx.cfg.gc_window_days) else {
        return Ok(from_fill(&filled, findings, false));
    };
    let tag = tag_path(ctx.state);
    if std::fs::symlink_metadata(&tag).is_err() {
        findings.push((
            "store.tag".to_string(),
            Some(tag.clone()),
            format!(
                "{path} is missing, so nothing was cloned; fill a source target and run the command again",
                path = tag.display()
            ),
        ));
        return Ok(from_fill(&filled, findings, false));
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
            return Ok(from_fill(&filled, findings, false));
        }
        Choice::Units { rows, dropped } => (rows, dropped),
    };
    let window = Duration::from_secs(days.saturating_mul(86_400));
    let placed = place::place(ctx, &checkout.path, &rows, window, CRATE_DIR_CAP, cancel)?;
    Ok(assemble(&filled, placed, dropped, findings, &checkout.path))
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
) -> Outcome {
    Outcome {
        counts: clone_counts(filled, 0, 0, 0, 0, 0),
        findings,
        unmeasured: Vec::new(),
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
    checkout: &Path,
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
        None => {
            if !unmeasured
                .iter()
                .any(|(field, _)| field == "clone_bytes_not_shared")
            {
                unmeasured.push((
                    "clone_bytes_not_shared".to_string(),
                    "the bytes this clone adds were not measured".to_string(),
                ));
            }
        }
    }
    match placed.store_bytes {
        Some(bytes) => {
            counts.insert("store_bytes".to_string(), bytes);
        }
        None => {
            if !unmeasured.iter().any(|(field, _)| field == "store_bytes") {
                unmeasured.push((
                    "store_bytes".to_string(),
                    "the store's size was not measured".to_string(),
                ));
            }
        }
    }
    let target = placed
        .target_identity
        .map(|identity| (checkout.join("target"), identity));
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
