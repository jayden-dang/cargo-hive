//! Store eviction for bare `cargo hive gc`: hold keys from live targets, then
//! rename out and remove unheld entries older than the age limit.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cargo_target::TargetDir;
use crate::discover::{Checkout, WalkFault};
use crate::doctor::Toolchain;
use crate::gc::{self, Ledger};
use crate::plan_tier::{faults_that_can_hide_an_owner, split_checkout_faults};
use crate::platform::Fs;
use crate::receipt::Finding;
use crate::seed::source::{self, Holds};
use crate::seed::store::{
    self, commit_eviction, evict_dir, inventory, rename_out, store_dir, CommitPlan, StoreKey,
};
use crate::state::State;

pub struct EvictCtx<'a> {
    pub state: &'a State,
    pub fs: &'a dyn Fs,
    pub targets: &'a [TargetDir],
    pub checkouts: &'a [Checkout],
    pub checkout_faults: &'a [WalkFault],
    pub scan_faults: &'a [WalkFault],
    pub cache_roots: &'a [PathBuf],
    pub window: std::time::Duration,
    pub account_bytes: bool,
    pub now: SystemTime,
    pub caller_toolchain: Option<Toolchain>,
    pub caller_toolchain_kept: bool,
    pub rustup: &'a Path,
}

pub struct Limits {
    pub cap_bytes: u64,
    pub max_age_secs: u64,
}

pub struct EvictOutcome {
    pub counts: Vec<(String, u64)>,
    pub findings: Vec<Finding>,
    pub notes: Vec<String>,
    pub unmeasured: Vec<(String, String)>,
    pub cancelled: bool,
}

pub fn run(
    ctx: &EvictCtx<'_>,
    limits: Limits,
    dry_run: bool,
    cancel: &dyn Fn() -> bool,
) -> EvictOutcome {
    let mut outcome = EvictOutcome {
        counts: Vec::new(),
        findings: Vec::new(),
        notes: Vec::new(),
        unmeasured: Vec::new(),
        cancelled: false,
    };

    let mut held = source::holds(ctx.fs, ctx.targets, ctx.now, ctx.window);
    absorb_walk_faults(
        &mut held,
        ctx.checkout_faults,
        ctx.scan_faults,
        ctx.cache_roots,
    );
    outcome.findings.extend(held.findings.iter().cloned());
    for finding in &held.whole_store {
        if !outcome.findings.iter().any(|existing| existing == finding) {
            outcome.findings.push(finding.clone());
        }
    }

    let inv = match inventory(ctx.fs, ctx.state) {
        Ok(inv) => inv,
        Err(err) => {
            outcome.findings.push(Finding {
                kind: "store.write_failed".into(),
                path: Some(ctx.state.root().join(store::STORE_FILE)),
                detail: err.to_string(),
                measured: BTreeMap::new(),
            });
            return outcome;
        }
    };
    push_count(&mut outcome, "store_tmp_bytes", inv.tmp_bytes);

    let now_secs = match ctx.now.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs(),
        Err(_) => 0,
    };

    if !held.whole_store.is_empty() {
        let kept = (inv.rows.len() + inv.rowless.len()) as u64;
        push_count(&mut outcome, "store_entries_held", kept);
        push_count(&mut outcome, "store_entries_held_whole_store", kept);
        return outcome;
    }

    let mut age_candidates = Vec::new();
    let mut rowless_candidates = Vec::new();
    let mut refreshed = Vec::new();
    let mut held_live = 0u64;

    for row in &inv.rows {
        let key = StoreKey {
            toolchain_value: row.toolchain_value,
            name_meta: row.name_meta.clone(),
            fingerprint_hash: row.fingerprint_hash.clone(),
        };
        if is_held(&held, &key) {
            refreshed.push(key);
            held_live += 1;
            continue;
        }
        if now_secs.saturating_sub(row.last_live_at) > limits.max_age_secs {
            age_candidates.push((key, Some(row.last_live_at)));
        }
    }
    for (key, _) in &inv.rowless {
        if is_held(&held, key) {
            held_live += 1;
            continue;
        }
        rowless_candidates.push((key.clone(), None));
    }

    push_count(&mut outcome, "store_entries_held", held_live);
    push_count(&mut outcome, "store_entries_held_live", held_live);

    let mut candidates = age_candidates;
    let age_n = candidates.len() as u64;
    let rowless_n = rowless_candidates.len() as u64;
    candidates.extend(rowless_candidates);

    if dry_run {
        push_count(&mut outcome, "store_entries_evicted_age", age_n);
        push_count(&mut outcome, "store_entries_evicted_rowless", rowless_n);
        return outcome;
    }

    if candidates.is_empty() && refreshed.is_empty() {
        return outcome;
    }

    if cancel() {
        outcome.cancelled = true;
        return outcome;
    }

    // Store lock stays held through commit and remove_counted (decision 3).
    let Some(store_lock) = (match ctx.state.try_store_exclusive() {
        Ok(lock) => lock,
        Err(err) => {
            outcome.findings.push(Finding {
                kind: "store.write_failed".into(),
                path: Some(ctx.state.root().join("store.lock")),
                detail: err.to_string(),
                measured: BTreeMap::new(),
            });
            return outcome;
        }
    }) else {
        return outcome;
    };

    if let Err(err) = remove_stale_evict_trees(ctx.state) {
        outcome.findings.push(Finding {
            kind: "store.rename_failed".into(),
            path: Some(store_dir(ctx.state).join(".tmp")),
            detail: err,
            measured: BTreeMap::new(),
        });
    }

    let plan = CommitPlan {
        candidates: candidates.clone(),
        refreshed,
        now: now_secs,
    };
    let mut seq = 0usize;
    let committed = match commit_eviction(ctx.state, &plan, |key| {
        if cancel() {
            return Ok(None);
        }
        let to = evict_dir(ctx.state, std::process::id(), seq);
        seq += 1;
        rename_out(ctx.state, key, &to)?;
        Ok(Some(to))
    }) {
        Ok(committed) => committed,
        Err(err) => {
            outcome.findings.push(Finding {
                kind: "store.write_failed".into(),
                path: Some(ctx.state.root().join(store::STORE_FILE)),
                detail: err.to_string(),
                measured: BTreeMap::new(),
            });
            return outcome;
        }
    };

    if cancel() {
        outcome.cancelled = true;
        return outcome;
    }

    remove_emptied_value_dirs(ctx.state, &committed.renamed);

    let mut age_evicted = 0u64;
    let mut rowless_evicted = 0u64;
    let planned: BTreeMap<_, _> = candidates.into_iter().collect();
    let mut ledger = Ledger::new();
    let store_root = store_dir(ctx.state);
    for (key, path) in &committed.renamed {
        match planned.get(key) {
            Some(Some(_)) => age_evicted += 1,
            Some(None) => rowless_evicted += 1,
            None => {}
        }
        if let Err(err) = gc::remove_counted(
            ctx.fs,
            &mut ledger,
            &store_root,
            path,
            ctx.account_bytes,
            false,
        ) {
            outcome.findings.push(Finding {
                kind: "store.rename_failed".into(),
                path: Some(path.clone()),
                detail: err.to_string(),
                measured: BTreeMap::new(),
            });
        }
    }

    push_count(&mut outcome, "store_entries_evicted_age", age_evicted);
    push_count(
        &mut outcome,
        "store_entries_evicted_rowless",
        rowless_evicted,
    );
    drop(store_lock);
    outcome
}

fn is_held(held: &Holds, key: &StoreKey) -> bool {
    held.keys.contains(key) || held.name_metas.contains(&key.name_meta)
}

fn push_count(outcome: &mut EvictOutcome, name: &str, n: u64) {
    outcome.counts.push((name.to_string(), n));
}

fn absorb_walk_faults(
    held: &mut Holds,
    checkout_faults: &[WalkFault],
    scan_faults: &[WalkFault],
    cache_roots: &[PathBuf],
) {
    let (kept, _) = split_checkout_faults(checkout_faults.to_vec());
    let scan_kept = faults_that_can_hide_an_owner(scan_faults.to_vec(), cache_roots);
    for fault in kept.into_iter().chain(scan_kept) {
        let finding = Finding {
            kind: "store.walk_fault".into(),
            path: Some(fault.path),
            detail: fault.detail,
            measured: BTreeMap::new(),
        };
        held.whole_store.push(finding.clone());
        held.findings.push(finding);
        *held.by_reason.entry("whole_store").or_insert(0) += 1;
    }
}

fn remove_stale_evict_trees(state: &State) -> Result<(), String> {
    let dir = store_dir(state).join(".tmp");
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("{}: {err}", dir.display())),
    };
    for entry in entries {
        let entry = entry.map_err(|err| format!("{}: {err}", dir.display()))?;
        let name = entry.file_name();
        let Some(text) = name.to_str() else {
            continue;
        };
        if !text.starts_with("evict-") {
            continue;
        }
        let path = entry.path();
        let meta =
            fs::symlink_metadata(&path).map_err(|err| format!("{}: {err}", path.display()))?;
        if meta.is_dir() {
            fs::remove_dir_all(&path).map_err(|err| format!("{}: {err}", path.display()))?;
        } else {
            fs::remove_file(&path).map_err(|err| format!("{}: {err}", path.display()))?;
        }
    }
    Ok(())
}

fn remove_emptied_value_dirs(state: &State, renamed: &[(StoreKey, PathBuf)]) {
    let mut values = BTreeMap::new();
    for (key, _) in renamed {
        values.insert(key.toolchain_value, ());
    }
    for value in values.into_keys() {
        let dir = store_dir(state).join(value.to_string());
        let Ok(mut entries) = fs::read_dir(&dir) else {
            continue;
        };
        if entries.next().is_none() {
            let _ = fs::remove_dir(&dir);
        }
    }
}
