//! Store eviction for bare `cargo hive gc`: hold keys from live targets, then
//! rename out and remove unheld entries older than the age limit or belonging to
//! a toolchain generation no checkout selects.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cargo_target::TargetDir;
use crate::discover::{Checkout, WalkFault};
use crate::gc::{self, Ledger};
use crate::plan_tier::{faults_that_can_hide_an_owner, split_checkout_faults};
use crate::platform::Fs;
use crate::receipt::{Finding, Measured};
use crate::seed::source::{self, Holds};
use crate::seed::store::{
    self, commit_eviction, entry_dir, evict_dir, inventory, key_dir, rename_out, row_key,
    row_matches, store_dir, CommitPlan, Inventory, Row, StoreKey,
};
use crate::state::{State, StateError};
use crate::toolchain::{self, Toolchain};

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
        let key = row_key(row);
        if bump_hold(
            held.hold_reason(&key),
            &mut held_live,
            &mut held_undecided,
            &mut held_name_meta,
        ) {
            refreshed.push(key);
            continue;
        }
        if now_secs.saturating_sub(row.last_live_at) > limits.max_age_secs {
            age_candidates.push((key, Some(row.last_live_at)));
        }
    }
    for (key, _) in &inv.rowless {
        if bump_hold(
            held.hold_reason(key),
            &mut held_live,
            &mut held_undecided,
            &mut held_name_meta,
        ) {
            continue;
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
            ToolchainsInUse::Resolved(texts) => match values_for_texts(ctx.state, &texts) {
                Ok(used_values) => {
                    let already: BTreeSet<_> = candidates.iter().map(|(k, _)| k.clone()).collect();
                    for row in &inv.rows {
                        let key = row_key(row);
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
                Err(err) => {
                    outcome.findings.push(Finding {
                        kind: "store.write_failed".into(),
                        path: Some(ctx.state.root().join(store::STORE_FILE)),
                        detail: err.to_string(),
                        measured: BTreeMap::new(),
                    });
                }
            },
        }
    }
    let mut cap_keys = BTreeSet::new();
    let mut post_plan_remaining = None;
    if !ctx.account_bytes {
        let reason = "doctor check (c) did not pass";
        for field in [
            "store_freeable_bytes_remaining",
            "store_freed_bytes",
            "store_bytes_still_shared",
        ] {
            outcome.unmeasured.push((field.into(), reason.into()));
        }
    } else {
        let planned: BTreeSet<_> = candidates.iter().map(|(k, _)| k.clone()).collect();
        let CapPlan {
            evict,
            freeable_remaining,
            held_over_cap,
        } = plan_cap(
            ctx.fs,
            ctx.state,
            &inv.rows,
            &inv.rowless,
            &planned,
            &held,
            limits.cap_bytes,
        );
        for (key, at) in evict {
            cap_keys.insert(key.clone());
            candidates.push((key, at));
        }
        post_plan_remaining = Some(freeable_remaining);
        if let Some(finding) = held_over_cap {
            outcome.findings.push(finding);
        }
    }

    if candidates.is_empty() && refreshed.is_empty() {
        if let Some(n) = post_plan_remaining {
            push_count(&mut outcome, "store_freeable_bytes_remaining", n);
        }
        push_count(&mut outcome, "store_entries_evicted_cap", 0);
        return outcome;
    }

    if dry_run {
        if let Some(n) = post_plan_remaining {
            push_count(&mut outcome, "store_freeable_bytes_remaining", n);
        }
        let renamed = preview_renames(&inv, &candidates);
        let mut age_evicted = 0u64;
        let mut rowless_evicted = 0u64;
        let mut toolchain_evicted = 0u64;
        let mut cap_evicted = 0u64;
        for key in &renamed {
            if age_keys.contains(key) {
                age_evicted += 1;
            } else if rowless_keys.contains(key) {
                rowless_evicted += 1;
            } else if toolchain_keys.contains(key) {
                toolchain_evicted += 1;
            } else if cap_keys.contains(key) {
                cap_evicted += 1;
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
        push_count(&mut outcome, "store_entries_evicted_cap", cap_evicted);
        match preview_dropped_values(ctx.state, &inv, &renamed) {
            Ok(n) => push_count(&mut outcome, "store_toolchains_dropped", n),
            Err(err) => {
                outcome.findings.push(Finding {
                    kind: "store.write_failed".into(),
                    path: Some(ctx.state.root().join(store::STORE_FILE)),
                    detail: err.to_string(),
                    measured: BTreeMap::new(),
                });
                push_count(&mut outcome, "store_toolchains_dropped", 0);
            }
        }
        let mut ledger = Ledger::new();
        let store_root = store_dir(ctx.state);
        for key in &renamed {
            if let Some(path) = path_for_key(ctx.state, &inv, key) {
                let _ = gc::remove_counted(
                    ctx.fs,
                    &mut ledger,
                    &store_root,
                    &path,
                    ctx.account_bytes,
                    true,
                );
            }
        }
        apply_removal_ledger(&mut outcome, ledger, ctx.account_bytes);
        return outcome;
    }

    if cancel() {
        push_untouched_freeable(
            &mut outcome,
            post_plan_remaining,
            ctx.fs,
            ctx.state,
            &inv.rows,
            &inv.rowless,
            &held,
        );
        outcome.cancelled = true;
        return outcome;
    }

    // Store lock stays held through commit and remove_counted.
    let Some(store_lock) = (match ctx.state.try_store_exclusive() {
        Ok(lock) => lock,
        Err(err) => {
            push_untouched_freeable(
                &mut outcome,
                post_plan_remaining,
                ctx.fs,
                ctx.state,
                &inv.rows,
                &inv.rowless,
                &held,
            );
            outcome.findings.push(Finding {
                kind: "store.write_failed".into(),
                path: Some(ctx.state.root().join("store.lock")),
                detail: err.to_string(),
                measured: BTreeMap::new(),
            });
            return outcome;
        }
    }) else {
        push_untouched_freeable(
            &mut outcome,
            post_plan_remaining,
            ctx.fs,
            ctx.state,
            &inv.rows,
            &inv.rowless,
            &held,
        );
        let busy = candidates.len() as u64;
        push_count(&mut outcome, "store_entries_held_busy", busy);
        push_count(
            &mut outcome,
            "store_entries_held",
            held_total.saturating_add(busy),
        );
        outcome.findings.push(Finding {
            kind: "store.busy".into(),
            path: Some(ctx.state.root().join("store.lock")),
            detail: "run `gc` again when no seed runs".into(),
            measured: BTreeMap::new(),
        });
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
    let committed = match commit_eviction(ctx.state, &plan, &|| cancel(), |key| {
        if cancel() {
            return Ok(None);
        }
        let to = evict_dir(ctx.state, std::process::id(), seq);
        seq += 1;
        rename_out(ctx.state, key, &to)?;
        Ok(Some(to))
    }) {
        Ok(committed) => committed,
        Err(StateError::Cancelled) => {
            push_untouched_freeable(
                &mut outcome,
                post_plan_remaining,
                ctx.fs,
                ctx.state,
                &inv.rows,
                &inv.rowless,
                &held,
            );
            outcome.cancelled = true;
            return outcome;
        }
        Err(err) => {
            push_untouched_freeable(
                &mut outcome,
                post_plan_remaining,
                ctx.fs,
                ctx.state,
                &inv.rows,
                &inv.rowless,
                &held,
            );
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
        if let Some(n) = post_plan_remaining {
            push_count(&mut outcome, "store_freeable_bytes_remaining", n);
        }
        outcome.cancelled = true;
        return outcome;
    }

    for (key, detail) in &committed.rename_failures {
        let path = key_dir(ctx.state, key);
        outcome.findings.push(Finding {
            kind: "store.rename_failed".into(),
            path: Some(path),
            detail: detail.clone(),
            measured: BTreeMap::new(),
        });
    }

    if let Err(err) = remove_emptied_value_dirs(ctx.state, &committed.renamed) {
        outcome.findings.push(Finding {
            kind: "store.rename_failed".into(),
            path: Some(store_dir(ctx.state)),
            detail: err,
            measured: BTreeMap::new(),
        });
    }

    let mut age_evicted = 0u64;
    let mut rowless_evicted = 0u64;
    let mut toolchain_evicted = 0u64;
    let mut cap_evicted = 0u64;
    let mut ledger = Ledger::new();
    let store_root = store_dir(ctx.state);
    for (key, path) in &committed.renamed {
        if age_keys.contains(key) {
            age_evicted += 1;
        } else if rowless_keys.contains(key) {
            rowless_evicted += 1;
        } else if toolchain_keys.contains(key) {
            toolchain_evicted += 1;
        } else if cap_keys.contains(key) {
            cap_evicted += 1;
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
    push_count(&mut outcome, "store_entries_evicted_cap", cap_evicted);
    push_count(
        &mut outcome,
        "store_toolchains_dropped",
        committed.dropped_values.len() as u64,
    );
    if let Some(n) = post_plan_remaining {
        push_count(&mut outcome, "store_freeable_bytes_remaining", n);
    }
    apply_removal_ledger(&mut outcome, ledger, ctx.account_bytes);
    drop(store_lock);
    outcome
}

fn untouched_freeable(
    fs: &dyn Fs,
    state: &State,
    rows: &[Row],
    rowless: &[(StoreKey, PathBuf)],
    held: &Holds,
) -> u64 {
    plan_cap(fs, state, rows, rowless, &BTreeSet::new(), held, u64::MAX).freeable_remaining
}

fn apply_removal_ledger(outcome: &mut EvictOutcome, ledger: Ledger, account_bytes: bool) {
    let tally = ledger.finish();
    if account_bytes {
        push_count(outcome, "store_freed_bytes", tally.total.freed);
        push_count(
            outcome,
            "store_bytes_still_shared",
            tally.total.still_shared,
        );
    }
    for (path, why) in tally.unmeasured {
        outcome.unmeasured.push((path.display().to_string(), why));
    }
    if tally.unmeasured_more > 0 {
        outcome.unmeasured.push((
            "unmeasured_paths".into(),
            format!(
                "{} more paths could not be measured and are not listed",
                tally.unmeasured_more
            ),
        ));
    }
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
        match toolchain::resolve_toolchain_own_dir_until(&checkout.path, ctx.rustup, cancel) {
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

fn values_for_texts(
    state: &State,
    texts: &[String],
) -> Result<BTreeSet<u64>, crate::state::StateError> {
    let file = store::read(state)?;
    let mut values = BTreeSet::new();
    for text in texts {
        if let Some(&value) = file.toolchains.get(text) {
            values.insert(value);
        }
    }
    Ok(values)
}

fn push_count(outcome: &mut EvictOutcome, name: &str, n: u64) {
    outcome.counts.push((name.to_string(), n));
}

fn bump_hold(
    reason: Option<&str>,
    live: &mut u64,
    undecided: &mut u64,
    name_meta: &mut u64,
) -> bool {
    match reason {
        Some("live") => *live += 1,
        Some("undecided") => *undecided += 1,
        Some("name_meta") => *name_meta += 1,
        _ => return false,
    }
    true
}

fn push_untouched_freeable(
    outcome: &mut EvictOutcome,
    post_plan_remaining: Option<u64>,
    fs: &dyn Fs,
    state: &State,
    rows: &[Row],
    rowless: &[(StoreKey, PathBuf)],
    held: &Holds,
) {
    if post_plan_remaining.is_some() {
        push_count(
            outcome,
            "store_freeable_bytes_remaining",
            untouched_freeable(fs, state, rows, rowless, held),
        );
    }
}

fn path_for_key(state: &State, inv: &Inventory, key: &StoreKey) -> Option<PathBuf> {
    if inv.rows.iter().any(|row| row_matches(row, key)) {
        return Some(key_dir(state, key));
    }
    inv.rowless
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, path)| path.clone())
}

fn preview_renames(inv: &Inventory, candidates: &[(StoreKey, Option<u64>)]) -> BTreeSet<StoreKey> {
    let mut survivors = BTreeSet::new();
    for (key, planned) in candidates {
        match planned {
            Some(planned_at) => match inv.rows.iter().find(|row| row_matches(row, key)) {
                Some(row) if row.last_live_at > *planned_at => {}
                Some(_) => {
                    survivors.insert(key.clone());
                }
                None => {}
            },
            None => {
                if inv.rows.iter().any(|row| row_matches(row, key)) {
                    continue;
                }
                survivors.insert(key.clone());
            }
        }
    }
    survivors
}

fn preview_dropped_values(
    state: &State,
    inv: &Inventory,
    renamed: &BTreeSet<StoreKey>,
) -> Result<u64, crate::state::StateError> {
    let file = store::read(state)?;
    let mut values = BTreeSet::new();
    for value in file.toolchains.values().copied() {
        values.insert(value);
    }
    let mut dropped = 0u64;
    for value in values {
        let remains = inv
            .rows
            .iter()
            .any(|row| row.toolchain_value == value && !renamed.contains(&row_key(row)))
            || inv
                .rowless
                .iter()
                .any(|(key, _)| key.toolchain_value == value && !renamed.contains(key));
        if !remains {
            dropped += 1;
        }
    }
    Ok(dropped)
}

struct CapPlan {
    evict: Vec<(StoreKey, Option<u64>)>,
    freeable_remaining: u64,
    held_over_cap: Option<Finding>,
}

#[derive(Clone)]
struct CapEntry {
    key: StoreKey,
    last_live_at: Option<u64>,
    registry_deps: Vec<String>,
    freeable: u64,
}

fn plan_cap(
    fs: &dyn Fs,
    state: &State,
    rows: &[Row],
    rowless: &[(StoreKey, PathBuf)],
    planned: &BTreeSet<StoreKey>,
    held: &Holds,
    cap_bytes: u64,
) -> CapPlan {
    let mut unheld = Vec::new();
    let mut held_freeable = BTreeMap::new();
    let mut sum = 0u64;
    for row in rows {
        let key = row_key(row);
        if planned.contains(&key) {
            continue;
        }
        let dir = entry_dir(state, row);
        let freeable = gc::measure(fs, &dir, std::slice::from_ref(&dir), true)
            .bytes
            .freed;
        sum += freeable;
        if held.hold_reason(&key).is_some() {
            held_freeable.insert(key, freeable);
        } else {
            unheld.push(CapEntry {
                key,
                last_live_at: Some(row.last_live_at),
                registry_deps: row.registry_deps.clone(),
                freeable,
            });
        }
    }
    for (key, dir) in rowless {
        if planned.contains(key) {
            continue;
        }
        let freeable = gc::measure(fs, dir, std::slice::from_ref(dir), true)
            .bytes
            .freed;
        sum += freeable;
        if held.hold_reason(key).is_some() {
            held_freeable.insert(key.clone(), freeable);
        } else {
            unheld.push(CapEntry {
                key: key.clone(),
                last_live_at: None,
                registry_deps: Vec::new(),
                freeable,
            });
        }
    }

    sort_cap_candidates(&mut unheld);

    let mut evict = Vec::new();
    for entry in unheld {
        if sum <= cap_bytes {
            break;
        }
        if entry.freeable == 0 {
            continue;
        }
        sum = sum.saturating_sub(entry.freeable);
        evict.push((entry.key, entry.last_live_at));
    }

    let held_over_cap = if sum > cap_bytes {
        Some(held_over_cap_finding(held, &held_freeable, sum, cap_bytes))
    } else {
        None
    };

    CapPlan {
        evict,
        freeable_remaining: sum,
        held_over_cap,
    }
}

fn sort_cap_candidates(entries: &mut [CapEntry]) {
    entries.sort_by(|a, b| match (a.last_live_at, b.last_live_at) {
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (None, None) => a.key.cmp(&b.key),
        (Some(a_at), Some(b_at)) => a_at.cmp(&b_at).then_with(|| a.key.cmp(&b.key)),
    });

    let mut i = 0;
    while i < entries.len() {
        let at = entries[i].last_live_at;
        let mut j = i + 1;
        while j < entries.len() && entries[j].last_live_at == at {
            j += 1;
        }
        reverse_topo_cohort(&mut entries[i..j]);
        i = j;
    }
}

fn reverse_topo_cohort(cohort: &mut [CapEntry]) {
    if cohort.len() <= 1 {
        return;
    }
    let n = cohort.len();
    let index: BTreeMap<&str, usize> = cohort
        .iter()
        .enumerate()
        .map(|(i, e)| (e.key.fingerprint_hash.as_str(), i))
        .collect();
    let mut indegree = vec![0usize; n];
    let mut succ: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, entry) in cohort.iter().enumerate() {
        for dep in &entry.registry_deps {
            if let Some(&j) = index.get(dep.as_str()) {
                succ[i].push(j);
                indegree[j] += 1;
            }
        }
    }
    let mut ready: BTreeSet<(StoreKey, usize)> = BTreeSet::new();
    for (i, &deg) in indegree.iter().enumerate() {
        if deg == 0 {
            ready.insert((cohort[i].key.clone(), i));
        }
    }
    let mut order = Vec::with_capacity(n);
    while let Some((_, i)) = ready.pop_first() {
        order.push(i);
        for &j in &succ[i] {
            indegree[j] -= 1;
            if indegree[j] == 0 {
                ready.insert((cohort[j].key.clone(), j));
            }
        }
    }
    if order.len() < n {
        let mut rest: Vec<usize> = (0..n).filter(|i| !order.contains(i)).collect();
        rest.sort_by(|&a, &b| cohort[a].key.cmp(&cohort[b].key));
        order.extend(rest);
    }
    let sorted: Vec<CapEntry> = order
        .into_iter()
        .map(|i| CapEntry {
            key: cohort[i].key.clone(),
            last_live_at: cohort[i].last_live_at,
            registry_deps: cohort[i].registry_deps.clone(),
            freeable: cohort[i].freeable,
        })
        .collect();
    cohort.clone_from_slice(&sorted);
}

fn held_over_cap_finding(
    held: &Holds,
    held_freeable: &BTreeMap<StoreKey, u64>,
    total: u64,
    cap_bytes: u64,
) -> Finding {
    let mut per_target: Vec<(PathBuf, u64)> = Vec::new();
    for (path, local) in &held.by_target {
        let mut bytes = 0u64;
        for (key, freeable) in held_freeable {
            if local.keys.contains(key) || local.name_metas.contains(&key.name_meta) {
                bytes += freeable;
            }
        }
        if bytes > 0 {
            per_target.push((path.clone(), bytes));
        }
    }
    per_target.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    per_target.truncate(10);

    let mut measured = BTreeMap::new();
    measured.insert("total".into(), Measured::Int(total));
    measured.insert("cap".into(), Measured::Int(cap_bytes));
    let mut detail = format!("{total} freeable bytes held over the {cap_bytes} byte cap");
    for (path, bytes) in &per_target {
        measured.insert(path.display().to_string(), Measured::Int(*bytes));
        detail.push_str(&format!("\n{}: {bytes}", path.display()));
    }

    Finding {
        kind: "store.held_over_cap".into(),
        path: per_target.first().map(|(p, _)| p.clone()),
        detail,
        measured,
    }
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

fn remove_emptied_value_dirs(state: &State, renamed: &[(StoreKey, PathBuf)]) -> Result<(), String> {
    remove_emptied_value_dirs_bounded(state, renamed, crate::gc::CRATE_DIR_CAP)
}

fn remove_emptied_value_dirs_bounded(
    state: &State,
    renamed: &[(StoreKey, PathBuf)],
    cap: usize,
) -> Result<(), String> {
    let mut values = BTreeMap::new();
    for (key, _) in renamed {
        values.insert(key.toolchain_value, ());
    }
    for value in values.into_keys() {
        let dir = store_dir(state).join(value.to_string());
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(format!("{}: {err}", dir.display())),
        };
        let mut seen = 0usize;
        for entry in entries {
            if seen >= cap {
                return Err(format!(
                    "{} has more than {cap} entries; remove until fewer remain and run the command again",
                    dir.display()
                ));
            }
            entry.map_err(|err| format!("{}: {err}", dir.display()))?;
            seen += 1;
        }
        if seen == 0 {
            fs::remove_dir(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;
        }
    }
    Ok(())
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

    #[test]
    fn remove_emptied_value_dirs_reports_a_listing_past_its_cap() {
        let state = scratch_state("empty-value-cap");
        let value = 42u64;
        let value_dir = store_dir(&state).join(value.to_string());
        std::fs::create_dir_all(&value_dir).unwrap();
        for name in ["a", "b", "c", "d"] {
            std::fs::create_dir(value_dir.join(name)).unwrap();
        }
        let key = StoreKey {
            variant: 0,
            toolchain_value: value,
            name_meta: "cap".into(),
            fingerprint_hash: "aaaaaaaaaaaaaaaa".into(),
        };
        let err = remove_emptied_value_dirs_bounded(&state, &[(key, value_dir.join("moved"))], 3)
            .expect_err("a listing past the cap is not a finished empty-dir cleanup");
        assert!(
            err.contains(&format!("{} has more than 3 entries", value_dir.display())),
            "{err}"
        );
        assert!(
            err.contains("remove until fewer remain and run the command again"),
            "{err}"
        );
        assert!(value_dir.is_dir(), "over-cap value dir must stay");
        assert_eq!(
            std::fs::read_dir(&value_dir).unwrap().count(),
            4,
            "nothing deleted when the listing exceeds the cap"
        );
        finish(&state);
    }

    #[test]
    fn values_for_texts_reports_an_unreadable_store_db() {
        use std::os::unix::fs::PermissionsExt;

        let state = scratch_state("values-unreadable-db");
        let path = state.root().join(store::STORE_FILE);
        std::fs::write(&path, b"{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let err = values_for_texts(&state, &[String::from("synth-1")])
            .expect_err("an unreadable store.db must not look like no toolchains in use");
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644));
        assert!(err.to_string().contains(store::STORE_FILE), "{err}");
        finish(&state);
    }

    #[test]
    fn preview_dropped_values_reports_an_unreadable_store_db() {
        use std::os::unix::fs::PermissionsExt;

        let state = scratch_state("preview-unreadable-db");
        let path = state.root().join(store::STORE_FILE);
        std::fs::write(&path, b"{}").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let inv = Inventory {
            rows: Vec::new(),
            rowless: Vec::new(),
            tmp_bytes: 0,
        };
        let err = preview_dropped_values(&state, &inv, &BTreeSet::new())
            .expect_err("an unreadable store.db must not look like zero toolchain drops");
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644));
        assert!(err.to_string().contains(store::STORE_FILE), "{err}");
        finish(&state);
    }
}
