//! Store eviction for bare `cargo hive gc`: hold keys from live targets, then
//! rename out and remove unheld entries older than the age limit or belonging to
//! a toolchain generation no checkout selects.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cargo_target::TargetDir;
use crate::discover::{Checkout, WalkFault};
use crate::doctor::{self, Toolchain};
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
    let mut held_undecided = 0u64;
    let mut held_name_meta = 0u64;

    for row in &inv.rows {
        let key = StoreKey {
            toolchain_value: row.toolchain_value,
            name_meta: row.name_meta.clone(),
            fingerprint_hash: row.fingerprint_hash.clone(),
        };
        match held.hold_reason(&key) {
            Some("live") => {
                refreshed.push(key);
                held_live += 1;
                continue;
            }
            Some("undecided") => {
                refreshed.push(key);
                held_undecided += 1;
                continue;
            }
            Some("name_meta") => {
                refreshed.push(key);
                held_name_meta += 1;
                continue;
            }
            Some(_) | None => {}
        }
        if now_secs.saturating_sub(row.last_live_at) > limits.max_age_secs {
            age_candidates.push((key, Some(row.last_live_at)));
        }
    }
    for (key, _) in &inv.rowless {
        match held.hold_reason(key) {
            Some("live") => {
                held_live += 1;
                continue;
            }
            Some("undecided") => {
                held_undecided += 1;
                continue;
            }
            Some("name_meta") => {
                held_name_meta += 1;
                continue;
            }
            Some(_) | None => {}
        }
        rowless_candidates.push((key.clone(), None));
    }

    let held_total = held_live + held_undecided + held_name_meta;
    push_count(&mut outcome, "store_entries_held", held_total);
    if held_live > 0 {
        push_count(&mut outcome, "store_entries_held_live", held_live);
    }
    if held_undecided > 0 {
        push_count(&mut outcome, "store_entries_held_undecided", held_undecided);
    }
    if held_name_meta > 0 {
        push_count(&mut outcome, "store_entries_held_name_meta", held_name_meta);
    }

    let age_keys: BTreeSet<_> = age_candidates.iter().map(|(k, _)| k.clone()).collect();
    let rowless_keys: BTreeSet<_> = rowless_candidates.iter().map(|(k, _)| k.clone()).collect();
    let mut candidates = age_candidates;
    let age_n = candidates.len() as u64;
    let rowless_n = rowless_candidates.len() as u64;
    candidates.extend(rowless_candidates);

    let mut toolchain_keys = BTreeSet::new();
    let inventory_nonempty = !inv.rows.is_empty() || !inv.rowless.is_empty();
    if inventory_nonempty {
        match toolchains_in_use(ctx, cancel) {
            ToolchainsInUse::Cancelled => {
                outcome.cancelled = true;
                return outcome;
            }
            ToolchainsInUse::Blocked { findings, note } => {
                outcome.findings.extend(findings);
                if let Some(note) = note {
                    outcome.notes.push(note);
                }
            }
            ToolchainsInUse::Resolved(texts) => {
                let used_values = values_for_texts(ctx.state, &texts);
                let already: BTreeSet<_> = candidates.iter().map(|(k, _)| k.clone()).collect();
                for row in &inv.rows {
                    let key = StoreKey {
                        toolchain_value: row.toolchain_value,
                        name_meta: row.name_meta.clone(),
                        fingerprint_hash: row.fingerprint_hash.clone(),
                    };
                    if used_values.contains(&row.toolchain_value) {
                        continue;
                    }
                    if held.hold_reason(&key).is_some() {
                        continue;
                    }
                    if already.contains(&key) {
                        continue;
                    }
                    toolchain_keys.insert(key.clone());
                    candidates.push((key, Some(row.last_live_at)));
                }
                for (key, _) in &inv.rowless {
                    if used_values.contains(&key.toolchain_value) {
                        continue;
                    }
                    if held.hold_reason(key).is_some() {
                        continue;
                    }
                    if already.contains(key) {
                        continue;
                    }
                    toolchain_keys.insert(key.clone());
                    candidates.push((key.clone(), None));
                }
            }
        }
    }
    let toolchain_n = toolchain_keys.len() as u64;

    if dry_run {
        push_count(&mut outcome, "store_entries_evicted_age", age_n);
        push_count(&mut outcome, "store_entries_evicted_rowless", rowless_n);
        push_count(&mut outcome, "store_entries_evicted_toolchain", toolchain_n);
        return outcome;
    }

    if candidates.is_empty() && refreshed.is_empty() {
        return outcome;
    }

    if cancel() {
        outcome.cancelled = true;
        return outcome;
    }

    // Store lock stays held through commit and remove_counted.
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
    let mut toolchain_evicted = 0u64;
    let mut ledger = Ledger::new();
    let store_root = store_dir(ctx.state);
    for (key, path) in &committed.renamed {
        if age_keys.contains(key) {
            age_evicted += 1;
        } else if rowless_keys.contains(key) {
            rowless_evicted += 1;
        } else if toolchain_keys.contains(key) {
            toolchain_evicted += 1;
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
    push_count(
        &mut outcome,
        "store_entries_evicted_toolchain",
        toolchain_evicted,
    );
    push_count(
        &mut outcome,
        "store_toolchains_dropped",
        committed.dropped_values.len() as u64,
    );
    drop(store_lock);
    outcome
}

enum ToolchainsInUse {
    Cancelled,
    Blocked {
        findings: Vec<Finding>,
        note: Option<String>,
    },
    Resolved(Vec<String>),
}

fn toolchains_in_use(ctx: &EvictCtx<'_>, cancel: &dyn Fn() -> bool) -> ToolchainsInUse {
    let mut texts = Vec::new();
    let mut findings = Vec::new();
    let mut saw_checkout = false;

    for checkout in ctx.checkouts.iter().filter(|c| !c.missing) {
        saw_checkout = true;
        if cancel() {
            return ToolchainsInUse::Cancelled;
        }
        match doctor::resolve_toolchain_own_dir_until(&checkout.path, ctx.rustup, cancel) {
            Ok(toolchain) => texts.push(toolchain.version_text),
            Err(_) if cancel() => return ToolchainsInUse::Cancelled,
            Err(err) => {
                findings.push(Finding {
                    kind: "store.toolchain_unresolved".into(),
                    path: Some(checkout.path.clone()),
                    detail: err,
                    measured: BTreeMap::new(),
                });
            }
        }
    }

    if !saw_checkout {
        return ToolchainsInUse::Blocked {
            findings,
            note: Some("store.no_checkout_resolved".into()),
        };
    }

    if ctx.caller_toolchain_kept {
        match &ctx.caller_toolchain {
            Some(toolchain) => texts.push(toolchain.version_text.clone()),
            None => {
                findings.push(Finding {
                    kind: "store.toolchain_unresolved".into(),
                    path: Some(ctx.state.root().to_path_buf()),
                    detail: "caller toolchain was kept but not resolved".into(),
                    measured: BTreeMap::new(),
                });
            }
        }
    }

    if !findings.is_empty() {
        return ToolchainsInUse::Blocked {
            findings,
            note: None,
        };
    }
    ToolchainsInUse::Resolved(texts)
}

fn values_for_texts(state: &State, texts: &[String]) -> BTreeSet<u64> {
    let Ok(file) = store::read(state) else {
        return BTreeSet::new();
    };
    let mut values = BTreeSet::new();
    for text in texts {
        if let Some(&value) = file.toolchains.get(text) {
            values.insert(value);
        }
    }
    values
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
    remove_stale_evict_trees_bounded(state, crate::gc::CRATE_DIR_CAP)
}

fn remove_stale_evict_trees_bounded(state: &State, cap: usize) -> Result<(), String> {
    let dir = store_dir(state).join(".tmp");
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("{}: {err}", dir.display())),
    };
    for (seen, entry) in entries.enumerate() {
        if seen >= cap {
            return Err(format!(
                "{} has more than {cap} entries; remove temp entries until fewer remain and run the command again",
                dir.display()
            ));
        }
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
    remove_emptied_value_dirs_bounded(state, renamed, crate::gc::CRATE_DIR_CAP)
}

fn remove_emptied_value_dirs_bounded(state: &State, renamed: &[(StoreKey, PathBuf)], cap: usize) {
    let mut values = BTreeMap::new();
    for (key, _) in renamed {
        values.insert(key.toolchain_value, ());
    }
    for value in values.into_keys() {
        let dir = store_dir(state).join(value.to_string());
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        if entries.take(cap).next().is_none() {
            let _ = fs::remove_dir(&dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn scratch_state(label: &str) -> State {
        let dir = PathBuf::from(format!(
            "/private/tmp/hive-it-evict-{label}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        State::open_at(dir).unwrap()
    }

    fn finish(state: &State) {
        let _ = std::fs::remove_dir_all(state.root());
    }

    #[test]
    fn remove_stale_evict_trees_reports_a_listing_past_its_cap_and_leaves_the_rest() {
        let state = scratch_state("stale-cap");
        let tmp = store_dir(&state).join(".tmp");
        std::fs::create_dir_all(&tmp).unwrap();
        let mut planted = Vec::new();
        for name in ["evict-1", "evict-2", "evict-3", "evict-4"] {
            let path = tmp.join(name);
            std::fs::create_dir(&path).unwrap();
            planted.push(path);
        }
        let err = remove_stale_evict_trees_bounded(&state, 3)
            .expect_err("a listing past the cap is not a finished stale cleanup");
        assert!(
            err.contains(&format!("{} has more than 3 entries", tmp.display())),
            "{err}"
        );
        assert!(
            err.contains("remove temp entries until fewer remain and run the command again"),
            "{err}"
        );
        assert_eq!(
            planted.iter().filter(|path| path.exists()).count(),
            1,
            "the entry past the cap stays"
        );
        finish(&state);
    }
}
