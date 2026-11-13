#![cfg(target_os = "macos")]
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cargo_hive::seed::choose::{self, Choice};
use cargo_hive::seed::evict::{self, EvictCtx, Limits};
use cargo_hive::seed::store::{self, MAX_STORE_BYTES, Row, STORE_FILE, StoreFile, store_dir};
use cargo_hive_cargo::discover::Checkout;
use cargo_hive_core::classify::{CheckoutKind, Class};
use cargo_hive_core::fingerprint;
use cargo_hive_core::ledger;
use cargo_hive_core::receipt::Measured;
use cargo_hive_core::target::{Owner, TAG_LINE, TargetDir};
use cargo_hive_platform::Fs;
use cargo_hive_platform::apfs::Apfs;
use cargo_hive_state::State;
use common::store_fixture::{
    count, drop_row, entry_dirs, has_finding, plant_rowless, rows, seeded_store, set_last_live_at,
    synth_entry, synth_entry_at, tree_files,
};
use common::{Hive, crate_dirs, plant_session, wait_within};

const DAY: u64 = 86_400;
const CAP_BYTES: u64 = 40_000_000_000;
const MAX_AGE_SECS: u64 = 30 * DAY;

#[test]
fn an_entry_no_target_holds_for_31_days_is_evicted_with_its_row() {
    let hive = Hive::ready();
    let repo = seeded_store(&hive);
    let before = rows(&hive).len();
    assert!(before > 0);
    set_last_live_at(&hive, "", 31 * DAY);
    remove_all_targets(&repo);
    let (out, receipt) = hive.json(&["gc"]);
    assert!(out.status.success(), "{receipt}");
    let evicted = count(&receipt, "store_entries_evicted_age");
    assert!(evicted > 0, "{receipt}");
    assert_eq!(evicted, before as u64, "{receipt}");
    assert!(rows(&hive).is_empty(), "{receipt}");
    assert!(
        entry_dirs(&hive).is_empty(),
        "entry dirs remain: {:?}",
        entry_dirs(&hive)
    );
}

#[test]
fn an_entry_held_by_a_live_target_stays_and_its_last_live_at_is_refreshed() {
    let hive = Hive::ready();
    let _repo = seeded_store(&hive);
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    set_last_live_at(&hive, "", 31 * DAY);
    let (out, receipt) = hive.json(&["gc"]);
    assert!(out.status.success(), "{receipt}");
    assert_eq!(count(&receipt, "store_entries_evicted_age"), 0, "{receipt}");
    let after = rows(&hive);
    assert!(!after.is_empty(), "{receipt}");
    for row in &after {
        let last = row["last_live_at"].as_u64().unwrap();
        assert!(
            last >= started,
            "last_live_at {last} < started {started}: {row}"
        );
    }
}

#[test]
fn an_entry_29_days_old_stays() {
    let hive = Hive::ready();
    let repo = seeded_store(&hive);
    set_last_live_at(&hive, "", 29 * DAY);
    remove_all_targets(&repo);
    let before = rows(&hive).len();
    let (out, receipt) = hive.json(&["gc"]);
    assert!(out.status.success(), "{receipt}");
    assert_eq!(count(&receipt, "store_entries_evicted_age"), 0, "{receipt}");
    assert_eq!(rows(&hive).len(), before, "{receipt}");
}

#[test]
fn a_rowless_entry_dir_no_target_holds_is_evicted() {
    let hive = Hive::ready();
    let repo = seeded_store(&hive);
    let value = rows(&hive)[0]["toolchain_value"].as_u64().unwrap();
    plant_rowless(
        &hive,
        value,
        "gonecrate-aaaaaaaaaaaaaaaa",
        "bbbbbbbbbbbbbbbb",
    );
    remove_all_targets(&repo);
    set_last_live_at(&hive, "", 31 * DAY);
    let (out, receipt) = hive.json(&["gc"]);
    assert!(out.status.success(), "{receipt}");
    assert!(
        count(&receipt, "store_entries_evicted_rowless") >= 1,
        "{receipt}"
    );
    assert!(
        entry_dirs(&hive)
            .iter()
            .all(|p| !p.to_string_lossy().contains("gonecrate-aaaaaaaaaaaaaaaa")),
        "{:?}",
        entry_dirs(&hive)
    );
}

#[test]
fn a_rowless_entry_dir_a_target_holds_is_left_for_fill() {
    let hive = Hive::ready();
    let _repo = seeded_store(&hive);
    let sample = rows(&hive)[0].clone();
    let name_meta = sample["name_meta"].as_str().unwrap().to_string();
    let dir_before = entry_dirs(&hive)
        .into_iter()
        .find(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&name_meta)
        })
        .expect("entry dir for held row");
    drop_row(&hive, &name_meta);
    assert!(dir_before.is_dir(), "{}", dir_before.display());
    let (out, receipt) = hive.json(&["gc"]);
    assert!(out.status.success(), "{receipt}");
    assert_eq!(
        count(&receipt, "store_entries_evicted_rowless"),
        0,
        "{receipt}"
    );
    assert!(
        dir_before.is_dir(),
        "held rowless dir was removed: {}",
        dir_before.display()
    );
}

#[test]
fn tmp_is_left_alone_and_its_bytes_are_counted() {
    let hive = Hive::ready();
    let repo = seeded_store(&hive);
    let tmp = hive.home.path.join("store/.tmp");
    fs::create_dir_all(&tmp).unwrap();
    let marker = tmp.join("keep-me");
    fs::write(&marker, vec![7u8; 16_384]).unwrap();
    set_last_live_at(&hive, "", 31 * DAY);
    remove_all_targets(&repo);
    let (out, receipt) = hive.json(&["gc"]);
    assert!(out.status.success(), "{receipt}");
    assert!(count(&receipt, "store_tmp_bytes") > 0, "{receipt}");
    assert_eq!(fs::read(&marker).unwrap(), vec![7u8; 16_384]);
}

#[test]
fn gc_window_days_out_of_range_evicts_nothing_and_still_deletes_sessions() {
    let hive = Hive::ready();
    let repo = seeded_store(&hive);
    set_last_live_at(&hive, "", 31 * DAY);
    let target = repo.main.join("target");
    let krate = crate_dirs(&target)
        .into_iter()
        .next()
        .expect("incremental crate dir");
    let (session, _) = plant_session(&krate, 3600, "svh0", false);
    assert!(krate.join(&session).is_dir());
    let before_rows = rows(&hive);
    let before_dirs = entry_dirs(&hive);
    hive.config(&format!(
        r#"{{"secondary_roots": ["{}"], "gc_window_days": 0}}"#,
        repo.scratch.path.display()
    ));
    let (out, receipt) = hive.json(&["gc"]);
    assert!(out.status.success(), "{receipt}");
    assert!(has_finding(&receipt, "store.window_invalid"), "{receipt}");
    assert_eq!(count(&receipt, "store_entries_evicted_age"), 0, "{receipt}");
    assert!(
        count(&receipt, "sessions_deleted") >= 1,
        "sessions not deleted: {receipt}"
    );
    assert!(!krate.join(&session).exists(), "{receipt}");
    assert_eq!(rows(&hive), before_rows);
    assert_eq!(entry_dirs(&hive), before_dirs);
}

#[test]
fn a_live_unit_in_an_orphan_candidate_target_holds_its_key() {
    let hive = Hive::ready();
    let repo = seeded_store(&hive);
    let source_target = repo.main.join("target");
    let orphan_parent = hive.home.path.join("orphan-cache");
    let orphan = orphan_parent.join("orphan-target");
    fs::create_dir_all(&orphan_parent).unwrap();
    let status = std::process::Command::new("cp")
        .args([
            "-R",
            source_target.to_str().unwrap(),
            orphan.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let orphan = orphan.canonicalize().unwrap();
    remove_all_targets(&repo);
    set_last_live_at(&hive, "", 31 * DAY);
    hive.config(&format!(
        r#"{{"secondary_roots": ["{}"], "cache_roots": ["{}"]}}"#,
        repo.scratch.path.display(),
        orphan_parent.display()
    ));
    let before = rows(&hive).len();
    let (out, receipt) = hive.json(&["gc"]);
    assert!(out.status.success(), "{receipt}");
    assert_eq!(count(&receipt, "store_entries_evicted_age"), 0, "{receipt}");
    assert_eq!(rows(&hive).len(), before, "{receipt}");
    assert!(orphan.is_dir());
}

#[test]
fn store_db_keeps_its_fields_after_eviction() {
    let hive = Hive::ready();
    let _repo = seeded_store(&hive);
    let before = rows(&hive);
    let field_sets: Vec<Vec<String>> = before
        .iter()
        .map(|row| {
            let mut keys: Vec<String> = row.as_object().unwrap().keys().cloned().collect();
            keys.sort();
            keys
        })
        .collect();
    let sample = before[0].clone();
    synth_entry(
        &hive,
        (
            sample["toolchain_value"].as_u64().unwrap(),
            "oldcrate-cccccccccccccccc",
            "dddddddddddddddd",
        ),
        &[],
        &[("f", vec![1u8; 64])],
        31 * DAY,
    );
    let (out, receipt) = hive.json(&["gc"]);
    assert!(out.status.success(), "{receipt}");
    assert!(
        count(&receipt, "store_entries_evicted_age") >= 1,
        "{receipt}"
    );
    let after = rows(&hive);
    assert!(!after.is_empty(), "{receipt}");
    for row in &after {
        let mut keys: Vec<String> = row.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert!(
            field_sets.iter().any(|expected| expected == &keys),
            "row fields changed: {keys:?} not in {field_sets:?}"
        );
    }
}

#[test]
fn eviction_touches_nothing_outside_the_store() {
    let hive = Hive::ready();
    let repo = seeded_store(&hive);
    let lane = repo.scratch.path.join("seed-lane");
    let sample = rows(&hive)[0].clone();
    synth_entry(
        &hive,
        (
            sample["toolchain_value"].as_u64().unwrap(),
            "outside-eeeeeeeeeeeeeeee",
            "ffffffffffffffff",
        ),
        &[],
        &[("f", vec![9u8; 32])],
        31 * DAY,
    );
    let snap_repo = tree_files(&repo.main);
    let snap_target = tree_files(&repo.main.join("target"));
    let snap_lane = tree_files(&lane);
    let snap_cargo = tree_files(&hive.cargo_home.path);
    let snap_seeds = read_optional(&hive.home.path.join("seeds.json"));
    let snap_plans = read_optional(&hive.home.path.join("plans.json"));
    let snap_config = read_optional(&hive.home.path.join("config.json"));
    let (out, receipt) = hive.json(&["gc"]);
    assert!(out.status.success(), "{receipt}");
    assert!(
        count(&receipt, "store_entries_evicted_age") >= 1,
        "{receipt}"
    );
    assert_eq!(tree_files(&repo.main), snap_repo);
    assert_eq!(tree_files(&repo.main.join("target")), snap_target);
    assert_eq!(tree_files(&lane), snap_lane);
    assert_eq!(tree_files(&hive.cargo_home.path), snap_cargo);
    assert_eq!(
        read_optional(&hive.home.path.join("seeds.json")),
        snap_seeds
    );
    assert_eq!(
        read_optional(&hive.home.path.join("plans.json")),
        snap_plans
    );
    assert_eq!(
        read_optional(&hive.home.path.join("config.json")),
        snap_config
    );
}

#[test]
fn an_undecided_lane_keeps_its_entries_past_30_days() {
    let hive = Hive::ready();
    let repo = seeded_store(&hive);
    let before = rows(&hive).len();
    assert!(before > 0);
    let profile = repo.main.join("target/debug");
    let nc = profile.join(".fingerprint/stuck-1111111111111111");
    fs::create_dir_all(&nc).unwrap();
    fs::write(nc.join("output-lib-stuck"), "").unwrap();
    let stamp = nc.join("invoked.timestamp");
    fs::write(&stamp, "").unwrap();
    let now = SystemTime::now();
    fs::File::options()
        .write(true)
        .open(&stamp)
        .unwrap()
        .set_modified(now)
        .unwrap();
    set_last_live_at(&hive, "", 31 * DAY);
    let (out, receipt) = hive.json(&["gc"]);
    assert!(out.status.success(), "{receipt}");
    assert_eq!(count(&receipt, "store_entries_evicted_age"), 0, "{receipt}");
    assert_eq!(rows(&hive).len(), before, "{receipt}");
    assert!(has_finding(&receipt, "store.target_undecided"), "{receipt}");
    let detail = receipt["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["kind"] == "store.target_undecided")
        .unwrap()["detail"]
        .as_str()
        .unwrap();
    assert!(
        detail.contains("never compiled")
            || detail.contains("ages out of the window")
            || detail.contains("abandons it"),
        "{detail}"
    );
}

#[test]
fn a_live_unit_of_a_target_still_in_the_scan_holds_its_key_past_age() {
    let hive = Hive::ready();
    let repo = seeded_store(&hive);
    let target = common::cargo_fixture::read_target(&repo.main.join("target"));
    set_last_live_at(&hive, "", 31 * DAY);
    let before = rows(&hive).len();
    assert!(before > 0);
    let fs = apfs();
    let outcome = run_evict(&hive, &fs, &[target], limits(), true);
    assert_eq!(
        outcome_count(&outcome, "store_entries_evicted_age"),
        0,
        "{:?}",
        outcome.counts
    );
    assert!(
        outcome_count(&outcome, "store_entries_held_live") >= 1
            || outcome_count(&outcome, "store_entries_held") >= 1,
        "{:?}",
        outcome.counts
    );
    assert_eq!(rows(&hive).len(), before, "{:?}", rows(&hive));
}

#[test]
fn a_live_unit_in_a_target_gc_skips_for_an_unsupported_profile_still_holds_its_key() {
    let hive = Hive::ready();
    let repo = seeded_store(&hive);
    let before = rows(&hive).len();
    assert!(before > 0);
    let fp = repo.main.join("target/debug/.fingerprint");
    let unit_dir = fs::read_dir(&fp)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.is_dir())
        .expect("fingerprint unit dir");
    let json = fs::read_dir(&unit_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|ext| ext == "json"))
        .expect("fingerprint json");
    let text = fs::read_to_string(&json).unwrap();
    fs::write(&json, text.replace(",\"compile_kind\":0", "")).unwrap();
    set_last_live_at(&hive, "", 31 * DAY);
    let (out, receipt) = hive.json(&["gc"]);
    assert!(out.status.success(), "{receipt}");
    assert_eq!(count(&receipt, "store_entries_evicted_age"), 0, "{receipt}");
    assert_eq!(rows(&hive).len(), before, "{receipt}");
}

#[test]
fn an_unreadable_target_holds_the_whole_store_and_gc_still_deletes_sessions_with_its_usual_exit_code()
 {
    use std::os::unix::fs::PermissionsExt;

    let hive = Hive::ready();
    let repo = seeded_store(&hive);
    let main_target = repo.main.join("target");
    let krate = crate_dirs(&main_target)
        .into_iter()
        .next()
        .expect("incremental crate dir");
    let (session, _) = plant_session(&krate, 3600, "svh0", false);
    assert!(krate.join(&session).is_dir());
    let orphan_parent = hive.home.path.join("orphan-cache");
    let orphan = orphan_parent.join("orphan-target");
    fs::create_dir_all(&orphan_parent).unwrap();
    let status = std::process::Command::new("cp")
        .args([
            "-R",
            main_target.to_str().unwrap(),
            orphan.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let orphan = orphan.canonicalize().unwrap();
    set_last_live_at(&hive, "", 31 * DAY);
    let before_rows = rows(&hive).len();
    hive.config(&format!(
        r#"{{"secondary_roots": ["{}"], "cache_roots": ["{}"]}}"#,
        repo.scratch.path.display(),
        orphan_parent.display()
    ));
    let blocked = orphan.join("debug");
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();
    let (out, receipt) = hive.json(&["gc"]);
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(out.status.success(), "{receipt}");
    assert!(
        has_finding(&receipt, "store.target_unreadable"),
        "{receipt}"
    );
    assert_eq!(count(&receipt, "store_entries_evicted_age"), 0, "{receipt}");
    assert!(
        count(&receipt, "store_entries_held_whole_store") >= 1,
        "{receipt}"
    );
    assert_eq!(rows(&hive).len(), before_rows, "{receipt}");
    assert!(
        count(&receipt, "sessions_deleted") >= 1,
        "sessions not deleted: {receipt}"
    );
    assert!(!krate.join(&session).exists(), "{receipt}");
}

#[test]
fn a_checkout_walk_fault_apply_would_refuse_on_holds_the_whole_store() {
    use std::os::unix::fs::PermissionsExt;

    let hive = Hive::ready();
    let repo = seeded_store(&hive);
    set_last_live_at(&hive, "", 31 * DAY);
    let before_rows = rows(&hive).len();
    let blocked = repo.scratch.path.join("blocked-checkouts");
    fs::create_dir_all(&blocked).unwrap();
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o100)).unwrap();
    let (out, receipt) = hive.json(&["gc"]);
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(out.status.success(), "{receipt}");
    assert!(has_finding(&receipt, "store.walk_fault"), "{receipt}");
    assert_eq!(count(&receipt, "store_entries_evicted_age"), 0, "{receipt}");
    assert!(
        count(&receipt, "store_entries_held_whole_store") >= 1,
        "{receipt}"
    );
    assert_eq!(rows(&hive).len(), before_rows, "{receipt}");
}

#[test]
fn liveness_writes_nothing_during_eviction() {
    let hive = Hive::ready();
    let repo = seeded_store(&hive);
    let target = repo.main.join("target");
    let before = tree_mtimes(&target);
    set_last_live_at(&hive, "", 31 * DAY);
    let (out, receipt) = hive.json(&["gc"]);
    assert!(out.status.success(), "{receipt}");
    assert_eq!(tree_mtimes(&target), before);
}

fn tree_mtimes(root: &Path) -> std::collections::BTreeMap<PathBuf, SystemTime> {
    let mut out = std::collections::BTreeMap::new();
    if !root.exists() {
        return out;
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            let meta = fs::symlink_metadata(&path).unwrap();
            out.insert(path.clone(), meta.modified().unwrap());
            if meta.is_dir() {
                stack.push(path);
            }
        }
    }
    out
}

fn read_optional(path: &Path) -> Option<Vec<u8>> {
    fs::read(path).ok()
}

fn remove_all_targets(repo: &common::registry_fixture::RegistryRepo) {
    for entry in fs::read_dir(&repo.scratch.path).unwrap() {
        let path = entry.unwrap().path();
        let target = path.join("target");
        if target.is_dir() {
            fs::remove_dir_all(&target).unwrap();
        }
    }
}

fn toolchains(hive: &Hive) -> serde_json::Map<String, serde_json::Value> {
    let path = hive.home.path.join("store.db");
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["toolchains"].as_object().cloned().unwrap_or_default()
}

fn has_note(receipt: &serde_json::Value, text: &str) -> bool {
    receipt["notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|note| note.as_str() == Some(text))
}

fn apfs() -> Apfs {
    Apfs::new(env!("CARGO_BIN_EXE_cargo-hive").into())
}

fn limits() -> Limits {
    Limits {
        cap_bytes: CAP_BYTES,
        max_age_secs: MAX_AGE_SECS,
    }
}

fn checkout_at(path: &Path) -> Checkout {
    Checkout {
        path: path.to_path_buf(),
        kind: CheckoutKind::Main,
        class: Class::Protected,
        reason: "test checkout".into(),
        missing: false,
    }
}

fn fake_rustup_logger(dir: &Path) -> (PathBuf, PathBuf) {
    let log = dir.join("rustup-calls");
    let rustup = dir.join("fake-rustup");
    fs::write(
        &rustup,
        format!(
            "#!/bin/sh\nprintf 'call\\n' >> '{}'\necho 'rustup which should not run' >&2\nexit 1\n",
            log.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&rustup, fs::Permissions::from_mode(0o755)).unwrap();
    (rustup, log)
}

#[test]
fn entries_of_a_value_no_checkout_maps_to_are_dropped_with_their_table_text() {
    let hive = Hive::ready();
    let repo = seeded_store(&hive);
    let real_before = rows(&hive).len();
    assert!(real_before > 0);
    let unused = 99_001u64;
    let text = format!("synth-{unused}");
    synth_entry(
        &hive,
        (unused, "goneunit-aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb"),
        &[],
        &[("blob", vec![1u8; 32])],
        0,
    );
    assert!(toolchains(&hive).contains_key(&text));
    let (out, receipt) = hive.json(&["gc"]);
    assert!(out.status.success(), "{receipt}");
    assert!(
        count(&receipt, "store_entries_evicted_toolchain") >= 1,
        "{receipt}"
    );
    assert!(
        count(&receipt, "store_toolchains_dropped") >= 1,
        "{receipt}"
    );
    assert!(
        rows(&hive)
            .iter()
            .all(|row| row["toolchain_value"].as_u64() != Some(unused)),
        "{:?}",
        rows(&hive)
    );
    assert!(
        !toolchains(&hive).contains_key(&text),
        "{:?}",
        toolchains(&hive)
    );
    assert_eq!(rows(&hive).len(), real_before, "{receipt}");
    let state = State::open_at(hive.home.path.clone()).unwrap();
    let file = store::read(&state).unwrap();
    assert!(matches!(
        choose::choose(&file, &repo.main, &text, &[]),
        Choice::NoRow { .. }
    ));
}

#[test]
fn a_value_still_used_by_one_checkout_is_kept() {
    let hive = Hive::ready();
    let _repo = seeded_store(&hive);
    let before_rows = rows(&hive);
    let before_n = before_rows.len();
    assert!(before_n > 0);
    let value = before_rows[0]["toolchain_value"].as_u64().unwrap();
    let texts_before: Vec<_> = toolchains(&hive)
        .iter()
        .filter(|(_, v)| v.as_u64() == Some(value))
        .map(|(k, _)| k.clone())
        .collect();
    assert!(!texts_before.is_empty());
    let (out, receipt) = hive.json(&["gc"]);
    assert!(out.status.success(), "{receipt}");
    assert_eq!(rows(&hive).len(), before_n, "{receipt}");
    for text in &texts_before {
        assert!(
            toolchains(&hive).contains_key(text),
            "missing {text}: {:?}",
            toolchains(&hive)
        );
    }
}

#[test]
fn held_entries_of_a_dropped_value_keep_its_text_until_none_remain() {
    let hive = Hive::ready();
    let repo = seeded_store(&hive);
    let unused = 99_002u64;
    let text = format!("synth-{unused}");
    let held_meta = "stuck-1111111111111111";
    let profile = repo.main.join("target/debug");
    let nc = profile.join(format!(".fingerprint/{held_meta}"));
    fs::create_dir_all(&nc).unwrap();
    fs::write(nc.join("output-lib-stuck"), "").unwrap();
    let stamp = nc.join("invoked.timestamp");
    fs::write(&stamp, "").unwrap();
    fs::File::options()
        .write(true)
        .open(&stamp)
        .unwrap()
        .set_modified(SystemTime::now())
        .unwrap();
    synth_entry(
        &hive,
        (unused, held_meta, "cccccccccccccccc"),
        &[],
        &[("blob", vec![2u8; 16])],
        0,
    );
    synth_entry(
        &hive,
        (unused, "freeunit-dddddddddddddddd", "eeeeeeeeeeeeeeee"),
        &[],
        &[("blob", vec![3u8; 16])],
        0,
    );
    assert!(toolchains(&hive).contains_key(&text));
    let (out, receipt) = hive.json(&["gc"]);
    assert!(out.status.success(), "{receipt}");
    assert!(
        count(&receipt, "store_entries_evicted_toolchain") >= 1,
        "{receipt}"
    );
    let after = rows(&hive);
    assert!(
        after.iter().any(|row| {
            row["toolchain_value"].as_u64() == Some(unused)
                && row["name_meta"].as_str() == Some(held_meta)
        }),
        "{after:?}"
    );
    assert!(
        after.iter().all(|row| {
            !(row["toolchain_value"].as_u64() == Some(unused)
                && row["name_meta"].as_str() == Some("freeunit-dddddddddddddddd"))
        }),
        "{after:?}"
    );
    assert!(
        toolchains(&hive).contains_key(&text),
        "text dropped while a held entry remains: {:?}",
        toolchains(&hive)
    );
}

#[test]
fn a_checkout_whose_toolchain_cannot_resolve_blocks_every_drop_but_age_still_runs() {
    let hive = Hive::ready();
    let repo = seeded_store(&hive);
    let unused = 99_003u64;
    let text = format!("synth-{unused}");
    synth_entry(
        &hive,
        (unused, "blockunit-aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb"),
        &[],
        &[("blob", vec![4u8; 16])],
        0,
    );
    let sample = rows(&hive)
        .into_iter()
        .find(|row| row["toolchain_value"].as_u64() != Some(unused))
        .expect("seeded row");
    synth_entry(
        &hive,
        (
            sample["toolchain_value"].as_u64().unwrap(),
            "agedunit-ffffffffffffffff",
            "0123456789abcdef",
        ),
        &[],
        &[("blob", vec![5u8; 16])],
        31 * DAY,
    );
    fs::write(
        repo.main.join("rust-toolchain.toml"),
        "[toolchain]\nchannel = \"1.0.0-hive-missing\"\n",
    )
    .unwrap();
    let (out, receipt) = hive.json(&["gc"]);
    assert!(out.status.success(), "{receipt}");
    assert!(
        has_finding(&receipt, "store.toolchain_unresolved"),
        "{receipt}"
    );
    assert_eq!(
        count(&receipt, "store_entries_evicted_toolchain"),
        0,
        "{receipt}"
    );
    assert!(
        toolchains(&hive).contains_key(&text),
        "{:?}",
        toolchains(&hive)
    );
    assert!(
        rows(&hive)
            .iter()
            .any(|row| row["toolchain_value"].as_u64() == Some(unused)),
        "{:?}",
        rows(&hive)
    );
    assert!(
        count(&receipt, "store_entries_evicted_age") >= 1,
        "{receipt}"
    );
    assert!(
        rows(&hive)
            .iter()
            .all(|row| row["name_meta"].as_str() != Some("agedunit-ffffffffffffffff")),
        "{:?}",
        rows(&hive)
    );
}

#[test]
fn no_registered_repo_drops_no_value() {
    let hive = Hive::ready();
    let _repo = seeded_store(&hive);
    let unused = 99_004u64;
    let text = format!("synth-{unused}");
    synth_entry(
        &hive,
        (unused, "orphanunit-aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb"),
        &[],
        &[("blob", vec![6u8; 16])],
        0,
    );
    fs::write(hive.home.path.join("registry.json"), r#"{"repos":[]}"#).unwrap();
    let (out, receipt) = hive.json(&["gc"]);
    assert!(out.status.success(), "{receipt}");
    assert!(
        has_note(&receipt, "store.no_checkout_resolved"),
        "{receipt}"
    );
    assert_eq!(
        count(&receipt, "store_entries_evicted_toolchain"),
        0,
        "{receipt}"
    );
    assert!(
        toolchains(&hive).contains_key(&text),
        "{:?}",
        toolchains(&hive)
    );
    assert!(
        rows(&hive)
            .iter()
            .any(|row| row["toolchain_value"].as_u64() == Some(unused)),
        "{:?}",
        rows(&hive)
    );
}

#[test]
fn an_empty_store_runs_no_own_dir_lookup() {
    let hive = Hive::ready();
    let state = State::open_at(hive.home.path.clone()).unwrap();
    let fs = apfs();
    let scratch = hive.home.path.join("empty-lookup");
    fs::create_dir_all(&scratch).unwrap();
    let (rustup, log) = fake_rustup_logger(&scratch);
    let checkout = checkout_at(&scratch);
    let outcome = evict::run(
        &EvictCtx {
            state: &state,
            fs: &fs,
            targets: &[],
            checkouts: &[checkout],
            checkout_faults: &[],
            scan_faults: &[],
            cache_roots: &[],
            window: Duration::from_secs(7 * DAY),
            account_bytes: true,
            now: SystemTime::now(),
            caller_toolchain: None,
            caller_toolchain_kept: false,
            rustup: &rustup,
            liveness: cargo_hive::gc::liveness::liveness,
        },
        limits(),
        false,
        &|| false,
    );
    assert!(
        !outcome.cancelled,
        "empty store should not cancel: {:?}",
        outcome.findings
    );
    assert!(
        !log.exists() || fs::read_to_string(&log).unwrap().is_empty(),
        "own-dir lookup ran on an empty store: {}",
        fs::read_to_string(&log).unwrap_or_default()
    );
}

#[test]
fn no_caller_toolchain_while_it_is_kept_blocks_every_drop() {
    let hive = Hive::ready();
    let unused = 99_005u64;
    let text = format!("synth-{unused}");
    synth_entry(
        &hive,
        (unused, "keptblock-aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb"),
        &[],
        &[("blob", vec![7u8; 16])],
        0,
    );
    synth_entry(
        &hive,
        (unused + 1, "agedblock-cccccccccccccccc", "dddddddddddddddd"),
        &[],
        &[("blob", vec![8u8; 16])],
        31 * DAY,
    );
    let state = State::open_at(hive.home.path.clone()).unwrap();
    let fs = apfs();
    let cwd = hive.home.path.join("caller-kept");
    fs::create_dir_all(&cwd).unwrap();
    let checkout = checkout_at(&cwd);
    let rustup = PathBuf::from("rustup");
    let outcome = evict::run(
        &EvictCtx {
            state: &state,
            fs: &fs,
            targets: &[],
            checkouts: &[checkout],
            checkout_faults: &[],
            scan_faults: &[],
            cache_roots: &[],
            window: Duration::from_secs(7 * DAY),
            account_bytes: true,
            now: SystemTime::now(),
            caller_toolchain: None,
            caller_toolchain_kept: true,
            rustup: &rustup,
            liveness: cargo_hive::gc::liveness::liveness,
        },
        limits(),
        false,
        &|| false,
    );
    assert!(
        outcome
            .findings
            .iter()
            .any(|f| f.kind == "store.toolchain_unresolved"),
        "{:?}",
        outcome.findings
    );
    let toolchain_evicted = outcome
        .counts
        .iter()
        .find(|(n, _)| n == "store_entries_evicted_toolchain")
        .map(|(_, n)| *n)
        .unwrap_or(0);
    assert_eq!(toolchain_evicted, 0, "{:?}", outcome.counts);
    assert!(
        toolchains(&hive).contains_key(&text),
        "{:?}",
        toolchains(&hive)
    );
    let age_evicted = outcome
        .counts
        .iter()
        .find(|(n, _)| n == "store_entries_evicted_age")
        .map(|(_, n)| *n)
        .unwrap_or(0);
    assert!(age_evicted >= 1, "{:?}", outcome.counts);
    assert!(
        rows(&hive)
            .iter()
            .any(|row| row["toolchain_value"].as_u64() == Some(unused)),
        "{:?}",
        rows(&hive)
    );
    assert!(
        rows(&hive)
            .iter()
            .all(|row| row["name_meta"].as_str() != Some("agedblock-cccccccccccccccc")),
        "{:?}",
        rows(&hive)
    );
}

fn kib256(seed: u64) -> Vec<u8> {
    let mut bytes = vec![0u8; 256 * 1024];
    let mut state = seed;
    for b in &mut bytes {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        *b = (state >> 33) as u8;
    }
    bytes
}

fn outcome_count(outcome: &evict::EvictOutcome, name: &str) -> u64 {
    outcome
        .counts
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, n)| *n)
        .unwrap_or(0)
}

fn entry_path(
    hive: &Hive,
    toolchain_value: u64,
    name_meta: &str,
    fingerprint_hash: &str,
) -> PathBuf {
    store_dir(&State::open_at(hive.home.path.clone()).unwrap())
        .join(toolchain_value.to_string())
        .join(format!("{name_meta}-{fingerprint_hash}"))
}

fn freeable_of(fs: &Apfs, dir: &Path) -> u64 {
    ledger::measure(fs, dir, &[dir.to_path_buf()], true)
        .bytes
        .freed
}

fn run_evict(
    hive: &Hive,
    fs: &dyn Fs,
    targets: &[TargetDir],
    limits: Limits,
    account_bytes: bool,
) -> evict::EvictOutcome {
    let state = State::open_at(hive.home.path.clone()).unwrap();
    let rustup = PathBuf::from("rustup");
    evict::run(
        &EvictCtx {
            state: &state,
            fs,
            targets,
            checkouts: &[],
            checkout_faults: &[],
            scan_faults: &[],
            cache_roots: &[],
            window: Duration::from_secs(7 * DAY),
            account_bytes,
            now: SystemTime::now(),
            caller_toolchain: None,
            caller_toolchain_kept: false,
            rustup: &rustup,
            liveness: cargo_hive::gc::liveness::liveness,
        },
        limits,
        false,
        &|| false,
    )
}

fn plant_undecided_name_meta_target(root: &Path, name_meta: &str) -> TargetDir {
    let profile = root.join("debug");
    let unit = profile.join(".fingerprint").join(name_meta);
    fs::create_dir_all(&unit).unwrap();
    fs::write(
        root.join("CACHEDIR.TAG"),
        format!("Signature: 8a477f597d28d172789f06886806bc55\n{TAG_LINE}\n"),
    )
    .unwrap();
    fs::write(unit.join("output-lib-meta"), "").unwrap();
    let stamp = unit.join("invoked.timestamp");
    fs::write(&stamp, "").unwrap();
    fs::File::options()
        .write(true)
        .open(&stamp)
        .unwrap()
        .set_modified(SystemTime::now())
        .unwrap();
    TargetDir {
        path: root.to_path_buf(),
        owner: Owner::Unresolved {
            reason: "cap test".into(),
        },
        profiles: vec![fingerprint::read_profile(&profile)],
        size: None,
        unreadable: None,
    }
}

#[test]
fn the_cap_evicts_unheld_entries_oldest_first_until_under() {
    let hive = Hive::ready();
    let fs = apfs();
    let value = 77_001u64;
    let keys = [
        (value, "old-aaaaaaaaaaaaaaaa", "1111111111111111", 3 * DAY),
        (value, "mid-bbbbbbbbbbbbbbbb", "2222222222222222", 2 * DAY),
        (value, "new-cccccccccccccccc", "3333333333333333", DAY),
    ];
    for &(tv, meta, fp, age) in &keys {
        synth_entry(&hive, (tv, meta, fp), &[], &[("blob", kib256(age))], age);
    }
    let free: Vec<u64> = keys
        .iter()
        .map(|&(tv, meta, fp, _)| freeable_of(&fs, &entry_path(&hive, tv, meta, fp)))
        .collect();
    assert!(free.iter().all(|&n| n > 0), "{free:?}");
    let total: u64 = free.iter().sum();
    let cap = free[0] + free[1] + free[2] / 2;
    assert!(
        cap < total && cap >= free[1] + free[2],
        "cap={cap} free={free:?}"
    );

    let outcome = run_evict(
        &hive,
        &fs,
        &[],
        Limits {
            cap_bytes: cap,
            max_age_secs: MAX_AGE_SECS,
        },
        true,
    );
    assert_eq!(
        outcome_count(&outcome, "store_entries_evicted_cap"),
        1,
        "{:?}",
        outcome.counts
    );
    assert!(
        outcome_count(&outcome, "store_freeable_bytes_remaining") <= cap,
        "{:?}",
        outcome.counts
    );
    assert!(
        rows(&hive)
            .iter()
            .all(|row| row["name_meta"].as_str() != Some("old-aaaaaaaaaaaaaaaa")),
        "{:?}",
        rows(&hive)
    );
    assert!(
        rows(&hive)
            .iter()
            .any(|row| row["name_meta"].as_str() == Some("mid-bbbbbbbbbbbbbbbb")),
        "{:?}",
        rows(&hive)
    );
    assert!(
        rows(&hive)
            .iter()
            .any(|row| row["name_meta"].as_str() == Some("new-cccccccccccccccc")),
        "{:?}",
        rows(&hive)
    );
}

#[test]
fn within_one_last_live_at_an_entry_goes_before_the_entries_its_registry_deps_name() {
    let hive = Hive::ready();
    let fs = apfs();
    let value = 77_002u64;
    let dep_fp = "aaaaaaaaaaaaaaaa";
    let user_fp = "zzzzzzzzzzzzzzzz";
    // One second for both, or the test is about age, not about dependency order.
    let now = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    synth_entry_at(
        &hive,
        (value, "aaa-dep", dep_fp),
        &[],
        &[("blob", kib256(1))],
        now,
    );
    synth_entry_at(
        &hive,
        (value, "zzz-user", user_fp),
        &[dep_fp],
        &[("blob", kib256(2))],
        now,
    );
    let dep_free = freeable_of(&fs, &entry_path(&hive, value, "aaa-dep", dep_fp));
    let user_free = freeable_of(&fs, &entry_path(&hive, value, "zzz-user", user_fp));
    assert!(dep_free > 0 && user_free > 0, "{dep_free} {user_free}");
    let cap = dep_free.max(user_free);

    let outcome = run_evict(
        &hive,
        &fs,
        &[],
        Limits {
            cap_bytes: cap,
            max_age_secs: MAX_AGE_SECS,
        },
        true,
    );
    assert_eq!(
        outcome_count(&outcome, "store_entries_evicted_cap"),
        1,
        "{:?}",
        outcome.counts
    );
    assert!(
        rows(&hive)
            .iter()
            .all(|row| row["name_meta"].as_str() != Some("zzz-user")),
        "dependent should go first: {:?}",
        rows(&hive)
    );
    assert!(
        rows(&hive)
            .iter()
            .any(|row| row["name_meta"].as_str() == Some("aaa-dep")),
        "dependency should stay: {:?}",
        rows(&hive)
    );
}

#[test]
fn held_entries_over_the_cap_are_kept_and_reported() {
    let hive = Hive::ready();
    let repo = seeded_store(&hive);
    let fs = apfs();
    let sample = rows(&hive)[0].clone();
    let value = sample["toolchain_value"].as_u64().unwrap();
    let key_meta = sample["name_meta"].as_str().unwrap().to_string();
    let key_fp = sample["fingerprint_hash"].as_str().unwrap().to_string();
    let key_dir = entry_path(&hive, value, &key_meta, &key_fp);
    fs::write(key_dir.join("cap-pad"), kib256(9)).unwrap();

    let shared_meta = "sharemeta-aaaaaaaaaaaaaaaa";
    let shared_fp = "bbbbbbbbbbbbbbbb";
    synth_entry(
        &hive,
        (value, shared_meta, shared_fp),
        &[],
        &[("blob", kib256(10))],
        0,
    );
    let only_meta = "onlymeta-cccccccccccccccc";
    let only_fp = "dddddddddddddddd";
    synth_entry(
        &hive,
        (value, only_meta, only_fp),
        &[],
        &[("blob", kib256(11))],
        0,
    );

    let key_target = common::cargo_fixture::read_target(&repo.main.join("target"));
    let undecided_root = hive.home.path.join("undecided-cap-target");
    let _ = plant_undecided_name_meta_target(&undecided_root, shared_meta);
    for meta in [only_meta, key_meta.as_str()] {
        let unit = undecided_root.join("debug/.fingerprint").join(meta);
        fs::create_dir_all(&unit).unwrap();
        fs::write(unit.join("output-lib-meta"), "").unwrap();
        let stamp = unit.join("invoked.timestamp");
        fs::write(&stamp, "").unwrap();
        fs::File::options()
            .write(true)
            .open(&stamp)
            .unwrap()
            .set_modified(SystemTime::now())
            .unwrap();
    }
    let undecided = TargetDir {
        path: undecided_root.clone(),
        owner: Owner::Unresolved {
            reason: "cap test".into(),
        },
        profiles: vec![fingerprint::read_profile(&undecided_root.join("debug"))],
        size: None,
        unreadable: None,
    };

    let key_free = freeable_of(&fs, &key_dir);
    let shared_free = freeable_of(&fs, &entry_path(&hive, value, shared_meta, shared_fp));
    let only_free = freeable_of(&fs, &entry_path(&hive, value, only_meta, only_fp));
    assert!(key_free > 0 && shared_free > 0 && only_free > 0);

    // Decided target holds every seeded live key (including the padded entry). Undecided
    // holds shared_meta, only_meta, and key_meta by name-meta; the padded entry counts for both.
    let mut decided_bytes = 0u64;
    for row in rows(&hive) {
        let meta = row["name_meta"].as_str().unwrap();
        if meta == shared_meta || meta == only_meta {
            continue;
        }
        let tv = row["toolchain_value"].as_u64().unwrap();
        let fp = row["fingerprint_hash"].as_str().unwrap();
        decided_bytes += freeable_of(&fs, &entry_path(&hive, tv, meta, fp));
    }
    let undecided_bytes = key_free + shared_free + only_free;
    let held_total = decided_bytes + shared_free + only_free;
    assert!(held_total > decided_bytes, "{held_total} {decided_bytes}");
    let mut expected = [
        (key_target.path.clone(), decided_bytes),
        (undecided.path.clone(), undecided_bytes),
    ];
    expected.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let cap = held_total.saturating_sub(1);
    let targets = [key_target, undecided];
    let before = rows(&hive).len();

    let outcome = run_evict(
        &hive,
        &fs,
        &targets,
        Limits {
            cap_bytes: cap,
            max_age_secs: MAX_AGE_SECS,
        },
        true,
    );
    assert_eq!(
        outcome_count(&outcome, "store_entries_evicted_cap"),
        0,
        "{:?}",
        outcome.counts
    );
    assert_eq!(rows(&hive).len(), before, "{:?}", rows(&hive));
    let finding = outcome
        .findings
        .iter()
        .find(|f| f.kind == "store.held_over_cap")
        .expect("store.held_over_cap");
    assert_eq!(
        finding.measured.get("total"),
        Some(&Measured::Int(held_total)),
        "{finding:?}"
    );
    assert_eq!(
        finding.measured.get("cap"),
        Some(&Measured::Int(cap)),
        "{finding:?}"
    );
    for (path, bytes) in &expected {
        assert_eq!(
            finding.measured.get(&path.display().to_string()),
            Some(&Measured::Int(*bytes)),
            "path {} bytes {bytes}: {finding:?}",
            path.display()
        );
    }
    let larger = &expected[0].0;
    let smaller = &expected[1].0;
    let detail = &finding.detail;
    assert!(
        detail.find(&larger.display().to_string()) < detail.find(&smaller.display().to_string()),
        "larger first in detail: {detail}"
    );
}

#[test]
fn an_entry_that_frees_0_bytes_is_not_evicted_for_the_cap() {
    let hive = Hive::ready();
    let fs = apfs();
    let value = 77_003u64;
    synth_entry(
        &hive,
        (value, "shared-aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb"),
        &[],
        &[("blob", kib256(3))],
        0,
    );
    synth_entry(
        &hive,
        (value, "alone-cccccccccccccccc", "dddddddddddddddd"),
        &[],
        &[("blob", kib256(4))],
        0,
    );
    let shared_dir = entry_path(&hive, value, "shared-aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb");
    let keep = hive.home.path.join("clone-keep");
    fs::create_dir_all(&keep).unwrap();
    fs.clone_file(&shared_dir.join("blob"), &keep.join("blob"))
        .unwrap();
    assert_eq!(freeable_of(&fs, &shared_dir), 0);
    let alone_free = freeable_of(
        &fs,
        &entry_path(&hive, value, "alone-cccccccccccccccc", "dddddddddddddddd"),
    );
    assert!(alone_free > 0);

    let outcome = run_evict(
        &hive,
        &fs,
        &[],
        Limits {
            cap_bytes: alone_free / 2,
            max_age_secs: MAX_AGE_SECS,
        },
        true,
    );
    assert!(
        rows(&hive)
            .iter()
            .any(|row| row["name_meta"].as_str() == Some("shared-aaaaaaaaaaaaaaaa")),
        "zero-freeable entry must stay: {:?}",
        rows(&hive)
    );
    assert!(
        rows(&hive)
            .iter()
            .all(|row| row["name_meta"].as_str() != Some("alone-cccccccccccccccc")),
        "{:?}",
        rows(&hive)
    );
    assert_eq!(
        outcome_count(&outcome, "store_entries_evicted_cap"),
        1,
        "{:?}",
        outcome.counts
    );
}

#[test]
fn with_accounting_off_the_cap_evicts_nothing_and_says_so() {
    let hive = Hive::ready();
    let fs = apfs();
    let value = 77_004u64;
    synth_entry(
        &hive,
        (value, "capcand-aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb"),
        &[],
        &[("blob", kib256(5))],
        0,
    );
    synth_entry(
        &hive,
        (value, "agedcap-cccccccccccccccc", "dddddddddddddddd"),
        &[],
        &[("blob", kib256(6))],
        31 * DAY,
    );
    let young_free = freeable_of(
        &fs,
        &entry_path(&hive, value, "capcand-aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb"),
    );
    assert!(young_free > 0);

    let outcome = run_evict(
        &hive,
        &fs,
        &[],
        Limits {
            cap_bytes: young_free / 2,
            max_age_secs: MAX_AGE_SECS,
        },
        false,
    );
    assert!(
        outcome_count(&outcome, "store_entries_evicted_age") >= 1,
        "{:?}",
        outcome.counts
    );
    assert_eq!(
        outcome_count(&outcome, "store_entries_evicted_cap"),
        0,
        "{:?}",
        outcome.counts
    );
    assert!(
        rows(&hive)
            .iter()
            .any(|row| row["name_meta"].as_str() == Some("capcand-aaaaaaaaaaaaaaaa")),
        "{:?}",
        rows(&hive)
    );
    assert!(
        outcome.unmeasured.iter().any(|(field, reason)| {
            reason == "doctor check (c) did not pass"
                && (field == "store_freeable_bytes_remaining" || field.contains("cap"))
        }),
        "{:?}",
        outcome.unmeasured
    );
}

fn store_counts(receipt: &serde_json::Value) -> std::collections::BTreeMap<String, u64> {
    receipt["counts"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(name, _)| name.starts_with("store_"))
        .map(|(name, value)| (name.clone(), value.as_u64().unwrap_or(0)))
        .collect()
}

#[test]
fn a_dry_run_reports_the_counts_the_next_real_run_reports() {
    let hive = Hive::ready();
    let repo = seeded_store(&hive);
    let value = rows(&hive)[0]["toolchain_value"].as_u64().unwrap();
    plant_rowless(
        &hive,
        value,
        "dryrowless-aaaaaaaaaaaaaaaa",
        "bbbbbbbbbbbbbbbb",
    );
    remove_all_targets(&repo);
    set_last_live_at(&hive, "", 31 * DAY);
    synth_entry(
        &hive,
        (99_008, "drytool-cccccccccccccccc", "dddddddddddddddd"),
        &[],
        &[("blob", vec![4u8; 32])],
        0,
    );
    let (dry_out, dry) = hive.json(&["gc", "--dry-run"]);
    assert!(dry_out.status.success(), "{dry}");
    assert!(count(&dry, "store_entries_evicted_age") >= 1, "{dry}");
    assert!(count(&dry, "store_entries_evicted_rowless") >= 1, "{dry}");
    assert!(count(&dry, "store_entries_evicted_toolchain") >= 1, "{dry}");
    let (real_out, real) = hive.json(&["gc"]);
    assert!(real_out.status.success(), "{real}");
    assert_eq!(
        store_counts(&dry),
        store_counts(&real),
        "dry={dry}\nreal={real}"
    );
}

#[test]
fn a_dry_run_changes_nothing_under_the_store_or_in_store_db() {
    let hive = Hive::ready();
    let repo = seeded_store(&hive);
    let value = rows(&hive)[0]["toolchain_value"].as_u64().unwrap();
    plant_rowless(&hive, value, "drykeep-eeeeeeeeeeeeeeee", "ffffffffffffffff");
    synth_entry(
        &hive,
        (99_009, "drykeep-gggggggggggggggg", "hhhhhhhhhhhhhhhh"),
        &[],
        &[("blob", vec![5u8; 16])],
        0,
    );
    remove_all_targets(&repo);
    set_last_live_at(&hive, "", 31 * DAY);
    let store = hive.home.path.join("store");
    let db = hive.home.path.join("store.db");
    let snap_store = tree_files(&store);
    let snap_db = fs::read(&db).unwrap();
    let (out, receipt) = hive.json(&["gc", "--dry-run"]);
    assert!(out.status.success(), "{receipt}");
    assert!(
        count(&receipt, "store_entries_evicted_age")
            + count(&receipt, "store_entries_evicted_rowless")
            + count(&receipt, "store_entries_evicted_toolchain")
            >= 1,
        "{receipt}"
    );
    assert_eq!(tree_files(&store), snap_store, "{receipt}");
    assert_eq!(fs::read(&db).unwrap(), snap_db, "{receipt}");
}

#[test]
fn a_scoped_gc_reads_and_writes_no_store_state_and_says_why() {
    let hive = Hive::ready();
    let repo = seeded_store(&hive);
    let value = rows(&hive)[0]["toolchain_value"].as_u64().unwrap();
    plant_rowless(&hive, value, "scoped-iiiiiiiiiiiiiiii", "jjjjjjjjjjjjjjjj");
    synth_entry(
        &hive,
        (99_010, "scoped-kkkkkkkkkkkkkkkk", "llllllllllllllll"),
        &[],
        &[("blob", vec![6u8; 16])],
        31 * DAY,
    );
    set_last_live_at(&hive, "", 31 * DAY);
    let store = hive.home.path.join("store");
    let db = hive.home.path.join("store.db");
    let snap_store = tree_files(&store);
    let snap_db = fs::read(&db).unwrap();
    let snap_mtimes = tree_mtimes(&store);
    let db_mtime = fs::symlink_metadata(&db).unwrap().modified().unwrap();
    let main = repo.main.to_str().unwrap();
    let (out, receipt) = hive.json(&["gc", main]);
    assert!(out.status.success(), "{receipt}");
    assert!(has_note(&receipt, "store.skipped_scoped"), "{receipt}");
    assert!(store_counts(&receipt).is_empty(), "{receipt}");
    assert_eq!(tree_files(&store), snap_store, "{receipt}");
    assert_eq!(fs::read(&db).unwrap(), snap_db, "{receipt}");
    assert_eq!(tree_mtimes(&store), snap_mtimes, "{receipt}");
    assert_eq!(
        fs::symlink_metadata(&db).unwrap().modified().unwrap(),
        db_mtime,
        "{receipt}"
    );
}

#[test]
fn while_the_store_is_busy_freeable_remaining_is_the_untouched_store() {
    let hive = Hive::ready();
    let fs = apfs();
    let value = 88_011u64;
    synth_entry(
        &hive,
        (value, "busyfree-aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb"),
        &[],
        &[("blob", kib256(1))],
        31 * DAY,
    );
    synth_entry(
        &hive,
        (value, "busyfree-cccccccccccccccc", "dddddddddddddddd"),
        &[],
        &[("blob", kib256(2))],
        31 * DAY,
    );
    let first = entry_path(
        &hive,
        value,
        "busyfree-aaaaaaaaaaaaaaaa",
        "bbbbbbbbbbbbbbbb",
    );
    let second = entry_path(
        &hive,
        value,
        "busyfree-cccccccccccccccc",
        "dddddddddddddddd",
    );
    let untouched = freeable_of(&fs, &first) + freeable_of(&fs, &second);
    assert!(untouched > 0, "{untouched}");
    let cap = freeable_of(&fs, &first).saturating_sub(1).max(1);

    let state = State::open_at(hive.home.path.clone()).unwrap();
    let held = state.store_shared_or_cancelled(|| false).unwrap();
    let outcome = run_evict(
        &hive,
        &fs,
        &[],
        Limits {
            cap_bytes: cap,
            max_age_secs: MAX_AGE_SECS,
        },
        true,
    );
    drop(held);

    assert!(
        outcome.findings.iter().any(|f| f.kind == "store.busy"),
        "{:?}",
        outcome.findings
    );
    assert_eq!(
        outcome_count(&outcome, "store_entries_evicted_age"),
        0,
        "{:?}",
        outcome.counts
    );
    assert_eq!(
        outcome_count(&outcome, "store_freeable_bytes_remaining"),
        untouched,
        "busy must report the untouched store, not the post-plan preview: {:?}",
        outcome.counts
    );
}

#[test]
fn a_permission_denied_rename_is_named_and_keeps_the_entry() {
    use std::os::unix::fs::PermissionsExt;

    let hive = Hive::ready();
    let fs = apfs();
    let value = 88_010u64;
    let meta = "renamefail-aaaaaaaaaaaaaaaa";
    let fp = "bbbbbbbbbbbbbbbb";
    synth_entry(
        &hive,
        (value, meta, fp),
        &[],
        &[("blob", vec![9u8; 32])],
        31 * DAY,
    );
    let value_dir =
        store_dir(&State::open_at(hive.home.path.clone()).unwrap()).join(value.to_string());
    fs::set_permissions(&value_dir, fs::Permissions::from_mode(0o555)).unwrap();
    let before_rows = rows(&hive);
    let entry = entry_path(&hive, value, meta, fp);
    assert!(entry.is_dir());

    let outcome = run_evict(&hive, &fs, &[], limits(), true);
    let _ = fs::set_permissions(&value_dir, fs::Permissions::from_mode(0o755));

    let finding = outcome
        .findings
        .iter()
        .find(|f| f.kind == "store.rename_failed")
        .expect("store.rename_failed");
    assert_eq!(finding.path.as_ref(), Some(&entry), "{finding:?}");
    assert!(
        finding.detail.contains("Permission denied")
            || finding.detail.contains("os error 13")
            || finding.detail.contains("Read-only"),
        "{finding:?}"
    );
    assert_eq!(
        outcome_count(&outcome, "store_entries_evicted_age"),
        0,
        "{:?}",
        outcome.counts
    );
    assert_eq!(rows(&hive), before_rows, "{:?}", rows(&hive));
    assert!(entry.is_dir(), "{}", entry.display());
}

#[test]
fn while_a_seed_holds_the_store_gc_evicts_nothing_and_keeps_its_exit_code() {
    let hive = Hive::ready();
    let repo = seeded_store(&hive);
    let main_target = repo.main.join("target");
    let session_parent = hive.home.path.join("session-cache");
    let session_target = session_parent.join("session-target");
    fs::create_dir_all(&session_parent).unwrap();
    let status = std::process::Command::new("cp")
        .args([
            "-R",
            main_target.to_str().unwrap(),
            session_target.to_str().unwrap(),
        ])
        .status()
        .unwrap();
    assert!(status.success());
    let session_target = session_target.canonicalize().unwrap();
    let krate = crate_dirs(&session_target)
        .into_iter()
        .next()
        .expect("incremental crate dir");
    let (session, _) = plant_session(&krate, 3600, "svh0", false);
    assert!(krate.join(&session).is_dir());
    remove_all_targets(&repo);
    set_last_live_at(&hive, "", 31 * DAY);
    synth_entry(
        &hive,
        (99_020, "busyage-aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb"),
        &[],
        &[("blob", vec![7u8; 32])],
        31 * DAY,
    );
    hive.config(&format!(
        r#"{{"secondary_roots": ["{}"], "cache_roots": ["{}"]}}"#,
        repo.scratch.path.display(),
        session_parent.display()
    ));
    let before_rows = rows(&hive);
    let before_dirs = entry_dirs(&hive);
    assert!(
        before_rows
            .iter()
            .any(|row| row["name_meta"].as_str() == Some("busyage-aaaaaaaaaaaaaaaa")),
        "{before_rows:?}"
    );
    assert!(!before_dirs.is_empty());

    let state = State::open_at(hive.home.path.clone()).unwrap();
    let held = state.store_shared_or_cancelled(|| false).unwrap();
    let child = hive
        .command(&["gc", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let busy_out = wait_within(child, Duration::from_secs(180));
    drop(held);
    let busy: serde_json::Value = serde_json::from_slice(&busy_out.stdout).unwrap_or_else(|err| {
        panic!(
            "busy gc did not print a receipt ({err}): stdout {} stderr {}",
            String::from_utf8_lossy(&busy_out.stdout),
            String::from_utf8_lossy(&busy_out.stderr)
        )
    });
    assert!(
        has_finding(&busy, "store.busy"),
        "expected store.busy: {busy}"
    );
    let detail = busy["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["kind"] == "store.busy")
        .unwrap()["detail"]
        .as_str()
        .unwrap();
    assert!(
        detail.contains("run `gc` again when no seed runs"),
        "{detail}"
    );
    assert!(count(&busy, "store_entries_held_busy") >= 1, "{busy}");
    assert_eq!(count(&busy, "store_entries_evicted_age"), 0, "{busy}");
    assert_eq!(rows(&hive), before_rows, "{busy}");
    assert_eq!(entry_dirs(&hive), before_dirs, "{busy}");
    assert!(
        count(&busy, "sessions_deleted") >= 1,
        "sessions not deleted: {busy}"
    );
    assert!(!krate.join(&session).exists(), "{busy}");

    let (session2, _) = plant_session(&krate, 3600, "svh1", false);
    assert!(krate.join(&session2).is_dir());
    let (free_out, free) = hive.json(&["gc"]);
    assert_eq!(
        busy_out.status.code(),
        free_out.status.code(),
        "busy={busy}\nfree={free}"
    );
    assert!(free_out.status.success(), "{free}");
}

#[test]
fn seeds_and_gcs_in_a_loop_leave_no_partial_unit() {
    let hive = Hive::ready();
    let repo = seeded_store(&hive);
    for cycle in 0..12 {
        set_last_live_at(&hive, "", 31 * DAY);
        let lane = repo.worktree(&format!("loop-{cycle}"));
        let lane_s = lane.to_str().unwrap().to_string();
        let (seed_out, seed_receipt, gc_out, _gc_receipt) = std::thread::scope(|scope| {
            let seed = scope.spawn(|| hive.json(&["seed", lane_s.as_str()]));
            let gc = scope.spawn(|| hive.json(&["gc"]));
            let (seed_out, seed_receipt) = seed.join().unwrap();
            let (gc_out, gc_receipt) = gc.join().unwrap();
            (seed_out, seed_receipt, gc_out, gc_receipt)
        });
        assert!(
            seed_out.status.success(),
            "cycle {cycle} seed: {seed_receipt}"
        );
        assert!(
            !seed_receipt["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|finding| finding["kind"] == "unit.refused"),
            "cycle {cycle} unit.refused: {seed_receipt}"
        );
        assert!(gc_out.status.success(), "cycle {cycle} gc failed");
        assert_seeded_units_whole_or_absent(&hive, &lane);
    }
}

#[test]
fn a_stale_evict_tree_leaves_seed_store_bytes_working_and_the_next_gc_removes_it() {
    let hive = Hive::ready();
    let repo = seeded_store(&hive);
    let stale = hive.home.path.join("store/.tmp/evict-1-0");
    fs::create_dir_all(&stale).unwrap();
    fs::write(stale.join("leftover"), b"stale-evict").unwrap();
    assert!(stale.is_dir());
    let lane = repo.worktree("stale-evict");
    let (seed_out, seed) = hive.json(&["seed", lane.to_str().unwrap()]);
    assert!(seed_out.status.success(), "{seed}");
    assert!(
        seed["counts"]["store_bytes"].as_u64().is_some(),
        "store_bytes missing: {seed}"
    );
    assert!(
        seed["unmeasured"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry["field"] != "store_bytes"),
        "{seed}"
    );
    assert!(
        stale.is_dir(),
        "fill must keep a current-mtime evict tree for the next gc"
    );
    let (gc_out, gc) = hive.json(&["gc"]);
    assert!(gc_out.status.success(), "{gc}");
    assert!(
        !stale.exists(),
        "bare gc must remove store/.tmp/evict-*: {gc}"
    );
}

#[test]
fn freed_and_still_shared_bytes_split_by_sharing() {
    let hive = Hive::ready();
    let fs = apfs();
    let value = 88_001u64;
    let meta = "bytesplit-aaaaaaaaaaaaaaaa";
    let fp = "bbbbbbbbbbbbbbbb";
    let text = mib2_text();
    let random = mib2_random(7);
    synth_entry(
        &hive,
        (value, meta, fp),
        &[],
        &[("packed", text), ("noise", random)],
        31 * DAY,
    );
    let dir = entry_path(&hive, value, meta, fp);
    let packed = dir.join("packed");
    let noise = dir.join("noise");
    assert!(matches!(
        fs.compress(&packed),
        cargo_hive_platform::CompressOutcome::Compressed
    ));
    let keep = hive.home.path.join("clone-keep");
    fs::create_dir_all(&keep).unwrap();
    fs.clone_file(&noise, &keep.join("noise")).unwrap();
    let packed_blocks = fs.entry_stat(&packed).unwrap().blocks.saturating_mul(512);
    let noise_blocks = fs.entry_stat(&noise).unwrap().blocks.saturating_mul(512);
    assert!(
        packed_blocks > 0 && packed_blocks < 2 << 20,
        "{packed_blocks}"
    );
    assert_eq!(noise_blocks, 2 << 20);

    let outcome = run_evict(&hive, &fs, &[], limits(), true);
    assert!(
        outcome_count(&outcome, "store_entries_evicted_age") >= 1,
        "{:?}",
        outcome.counts
    );
    assert_eq!(
        outcome_count(&outcome, "store_freed_bytes"),
        packed_blocks,
        "{:?}",
        outcome.counts
    );
    assert_eq!(
        outcome_count(&outcome, "store_bytes_still_shared"),
        noise_blocks,
        "{:?}",
        outcome.counts
    );
}

#[test]
fn with_check_c_failed_the_store_bytes_are_unmeasured() {
    let hive = Hive::ready();
    let fs = apfs();
    let value = 88_002u64;
    synth_entry(
        &hive,
        (value, "nocheckc-aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb"),
        &[],
        &[("blob", mib2_text())],
        31 * DAY,
    );
    let outcome = run_evict(&hive, &fs, &[], limits(), false);
    assert!(
        outcome_count(&outcome, "store_entries_evicted_age") >= 1,
        "{:?}",
        outcome.counts
    );
    assert_eq!(
        outcome_count(&outcome, "store_freed_bytes"),
        0,
        "{:?}",
        outcome.counts
    );
    assert_eq!(
        outcome_count(&outcome, "store_bytes_still_shared"),
        0,
        "{:?}",
        outcome.counts
    );
    assert_eq!(
        outcome_count(&outcome, "store_freeable_bytes_remaining"),
        0,
        "{:?}",
        outcome.counts
    );
    let reason = "doctor check (c) did not pass";
    for field in [
        "store_freed_bytes",
        "store_bytes_still_shared",
        "store_freeable_bytes_remaining",
    ] {
        assert!(
            outcome
                .unmeasured
                .iter()
                .any(|(f, r)| f == field && r == reason),
            "missing {field}: {:?}",
            outcome.unmeasured
        );
    }
}

#[test]
fn an_unreadable_file_in_an_evicted_entry_is_unmeasured_not_counted() {
    let hive = Hive::ready();
    let value = 88_003u64;
    let meta = "unreadable-aaaaaaaaaaaaaaaa";
    let fp = "bbbbbbbbbbbbbbbb";
    synth_entry(
        &hive,
        (value, meta, fp),
        &[],
        &[("blob", mib2_text())],
        31 * DAY,
    );
    let renamed_blob = hive
        .home
        .path
        .join("store/.tmp")
        .join(format!("evict-{}-0", std::process::id()))
        .join("blob");
    let recording = common::recording_fs::Recording::over_real().fail_sharing(&renamed_blob);
    let fs = recording.fs();
    let outcome = run_evict(&hive, fs.as_ref(), &[], limits(), true);
    assert!(
        outcome_count(&outcome, "store_entries_evicted_age") >= 1,
        "{:?}",
        outcome.counts
    );
    assert!(
        outcome
            .unmeasured
            .iter()
            .any(|(field, _)| field.contains("blob") && field.contains("evict-")),
        "{:?}",
        outcome.unmeasured
    );
    assert_eq!(
        outcome_count(&outcome, "store_freed_bytes"),
        0,
        "{:?}",
        outcome.counts
    );
    assert_eq!(
        outcome_count(&outcome, "store_bytes_still_shared"),
        0,
        "{:?}",
        outcome.counts
    );
}

#[test]
fn a_cancel_during_commit_records_what_was_renamed_and_releases_both_locks() {
    let hive = Hive::ready();
    let value = 91_001u64;
    let files = [
        ("a", b"one".to_vec()),
        ("b", b"two".to_vec()),
        ("c", b"three".to_vec()),
    ];
    let keys: Vec<(u64, String, String)> = (0..5)
        .map(|i| {
            (
                value,
                format!("cancel{i:02}-aaaaaaaaaaaaaaaa"),
                format!("{i:016x}"),
            )
        })
        .collect();
    for (tv, meta, fp) in &keys {
        synth_entry(
            &hive,
            (*tv, meta.as_str(), fp.as_str()),
            &[],
            &files,
            31 * DAY,
        );
    }
    let expected: std::collections::BTreeSet<_> =
        files.iter().map(|(name, _)| PathBuf::from(*name)).collect();
    let fs = apfs();
    let rustup = PathBuf::from("rustup");
    let mut saw_mid_commit = false;
    for cancel_after in 2..=6 {
        for (tv, meta, fp) in &keys {
            let dir = entry_path(&hive, *tv, meta, fp);
            if !dir.exists() {
                synth_entry(
                    &hive,
                    (*tv, meta.as_str(), fp.as_str()),
                    &[],
                    &files,
                    31 * DAY,
                );
            } else {
                set_last_live_at(&hive, meta, 31 * DAY);
            }
        }
        let calls = AtomicUsize::new(0);
        let state = State::open_at(hive.home.path.clone()).unwrap();
        let outcome = evict::run(
            &EvictCtx {
                state: &state,
                fs: &fs,
                targets: &[],
                checkouts: &[],
                checkout_faults: &[],
                scan_faults: &[],
                cache_roots: &[],
                window: Duration::from_secs(7 * DAY),
                account_bytes: true,
                now: SystemTime::now(),
                caller_toolchain: None,
                caller_toolchain_kept: false,
                rustup: &rustup,
                liveness: cargo_hive::gc::liveness::liveness,
            },
            limits(),
            false,
            &|| calls.fetch_add(1, Ordering::SeqCst) + 1 >= cancel_after,
        );
        assert!(
            outcome.cancelled,
            "cancel_after {cancel_after}: {:?}",
            outcome.counts
        );
        drop(state);
        let listed: std::collections::BTreeSet<_> = rows(&hive)
            .into_iter()
            .filter_map(|row| {
                Some((
                    row["toolchain_value"].as_u64()?,
                    row["name_meta"].as_str()?.to_string(),
                    row["fingerprint_hash"].as_str()?.to_string(),
                ))
            })
            .collect();
        let mut absent = 0usize;
        let mut complete = 0usize;
        for (tv, meta, fp) in &keys {
            let dir = entry_path(&hive, *tv, meta, fp);
            let key = (*tv, meta.clone(), fp.clone());
            if listed.contains(&key) {
                let present: std::collections::BTreeSet<_> =
                    entry_relative_files(&dir).into_iter().collect();
                assert_eq!(
                    present,
                    expected,
                    "cancel_after {cancel_after}: incomplete {}",
                    dir.display()
                );
                complete += 1;
            } else {
                assert!(
                    !dir.exists(),
                    "cancel_after {cancel_after}: raw store.db has no row but dir remains {}",
                    dir.display()
                );
                absent += 1;
            }
        }
        if absent > 0 && complete > 0 {
            saw_mid_commit = true;
        }
        let state = State::open_at(hive.home.path.clone()).unwrap();
        assert!(
            common::wait_until_lock_free(
                &hive.home.path.join("store.lock"),
                Duration::from_secs(5)
            ),
            "cancel_after {cancel_after}: store lock still held"
        );
        state
            .locked(|_| Ok::<(), cargo_hive_state::StateError>(()))
            .unwrap();
    }
    assert!(
        saw_mid_commit,
        "expected at least one cancel_after with both renamed-absent raw rows and remaining complete dirs"
    );
}

#[test]
fn killing_gc_during_store_eviction_never_leaves_a_partial_entry_listed_or_adoptable() {
    let hive = Hive::ready();
    hive.config(&format!(
        r#"{{"secondary_roots":["{}"]}}"#,
        hive.home.path.join("no-secondary").display()
    ));
    let repo = seeded_store(&hive);
    remove_all_targets(&repo);
    let expected_files = ["a", "b", "c"];
    plant_many_aged(
        &hive,
        92_001,
        2_000,
        &expected_files,
        &[b"one", b"two", b"three"],
        31 * DAY,
    );
    let delays_ms = [0u64, 5, 15, 40, 80, 150, 300, 600];
    let lane = repo.worktree("after-kill");
    let last = delays_ms.len() - 1;
    for (round, delay) in delays_ms.into_iter().enumerate() {
        let mut child = hive
            .command(&["gc", "--json"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        if delay > 0 {
            std::thread::sleep(Duration::from_millis(delay));
        }
        let _ = Command::new("kill")
            .args(["-9", &child.id().to_string()])
            .status();
        let _ = child.wait();
        assert_store_entries_whole_or_absent(&hive, &expected_files, round, delay);
        let (seed_out, seed) = hive.json(&["seed", lane.to_str().unwrap()]);
        assert!(
            seed_out.status.success(),
            "round {round} delay {delay}: {seed}"
        );
        assert!(
            seed["findings"]
                .as_array()
                .unwrap()
                .iter()
                .all(|finding| finding["kind"] != "unit.refused"),
            "round {round} delay {delay}: unit.refused on post-kill store: {seed}"
        );
        if round != last {
            plant_many_aged(
                &hive,
                92_001,
                2_000,
                &expected_files,
                &[b"one", b"two", b"three"],
                31 * DAY,
            );
        }
    }
}

#[test]
fn gc_exits_130_when_store_eviction_is_cancelled() {
    let hive = Hive::ready();
    hive.config(&format!(
        r#"{{"secondary_roots":["{}"]}}"#,
        hive.home.path.join("no-secondary").display()
    ));
    for i in 0..8 {
        synth_entry(
            &hive,
            (
                93_001,
                &format!("sigint{i:02}-aaaaaaaaaaaaaaaa"),
                &format!("{i:016x}"),
            ),
            &[],
            &[("a", b"1".to_vec()), ("b", b"2".to_vec())],
            31 * DAY,
        );
    }
    let mut holder = hive.hold_state_lock();
    let child = hive
        .command(&["gc", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let hive_pid = child.id();
    wait_until_open(hive_pid, &hive.home.path.join(".lock"));
    let started = Instant::now();
    while started.elapsed() < Duration::from_millis(250) {
        assert!(
            pid_alive(hive_pid),
            "gc should still be blocked on the state lock"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        Command::new("kill")
            .args(["-INT", &hive_pid.to_string()])
            .status()
            .unwrap()
            .success()
    );
    let _ = holder.kill();
    let _ = holder.wait();
    let out = wait_within(child, Duration::from_secs(60));
    assert_eq!(out.status.code(), Some(130), "{out:?}");
}

fn plant_many_aged(
    hive: &Hive,
    toolchain_value: u64,
    count: usize,
    names: &[&str],
    contents: &[&[u8]],
    last_live_secs_ago: u64,
) {
    assert_eq!(names.len(), contents.len());
    let state = State::open_at(hive.home.path.clone()).unwrap();
    let store = store_dir(&state);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let last_live_at = now.saturating_sub(last_live_secs_ago);
    let mut rows = Vec::with_capacity(count);
    for i in 0..count {
        let name_meta = format!("kill{i:04}-aaaaaaaaaaaaaaaa");
        let fingerprint_hash = format!("{i:016x}");
        let dir = store
            .join(toolchain_value.to_string())
            .join(format!("{name_meta}-{fingerprint_hash}"));
        fs::create_dir_all(&dir).unwrap();
        let mut size = 0u64;
        for (name, bytes) in names.iter().zip(contents.iter()) {
            fs::write(dir.join(name), bytes).unwrap();
            size += bytes.len() as u64;
        }
        rows.push(Row {
            toolchain_value,
            name_meta,
            fingerprint_hash,
            kind: "lib".into(),
            rel_profile: PathBuf::from("debug"),
            size,
            filled_at: last_live_at,
            last_live_at,
            repos: Vec::new(),
            registry_deps: Vec::new(),
            variant: 0,
        });
    }
    state
        .locked(|tx| {
            let mut file: StoreFile = tx
                .read_bounded(STORE_FILE, MAX_STORE_BYTES)
                .unwrap_or_default();
            file.toolchains
                .entry(format!("synth-{toolchain_value}"))
                .or_insert(toolchain_value);
            file.units
                .retain(|row| row.toolchain_value != toolchain_value);
            file.units.extend(rows);
            tx.write_bounded(STORE_FILE, &file, MAX_STORE_BYTES)
        })
        .unwrap();
}

fn assert_store_entries_whole_or_absent(
    hive: &Hive,
    expected_files: &[&str],
    round: usize,
    delay: u64,
) {
    let state = State::open_at(hive.home.path.clone()).unwrap();
    let file = store::read(&state).unwrap();
    let expected: std::collections::BTreeSet<_> =
        expected_files.iter().map(|n| PathBuf::from(*n)).collect();
    for row in &file.units {
        let dir = store::entry_dir(&state, row);
        assert!(
            dir.is_dir(),
            "round {round} delay {delay}: listed row missing dir {}",
            dir.display()
        );
        if row.name_meta.contains("kill") && row.name_meta.contains("-aaaaaaaaaaaaaaaa") {
            let present: std::collections::BTreeSet<_> =
                entry_relative_files(&dir).into_iter().collect();
            assert_eq!(
                present,
                expected,
                "round {round} delay {delay}: partial entry {}",
                dir.display()
            );
        }
    }
    let store = store_dir(&state);
    let Ok(values) = fs::read_dir(&store) else {
        return;
    };
    for value in values {
        let value = value.unwrap().path();
        let name = value.file_name().unwrap().to_string_lossy();
        if name == ".tmp" || !value.is_dir() || name.parse::<u64>().is_err() {
            continue;
        }
        for entry in fs::read_dir(&value).unwrap() {
            let entry = entry.unwrap().path();
            if !entry.is_dir() {
                continue;
            }
            let entry_name = entry.file_name().unwrap().to_string_lossy();
            if !entry_name.contains("kill") {
                continue;
            }
            let present: std::collections::BTreeSet<_> =
                entry_relative_files(&entry).into_iter().collect();
            assert_eq!(
                present,
                expected,
                "round {round} delay {delay}: partial dir without row check {}",
                entry.display()
            );
        }
    }
}

fn wait_until_open(pid: u32, path: &Path) {
    let started = Instant::now();
    loop {
        let open = Command::new("lsof")
            .args(["-a", "-p", &pid.to_string()])
            .arg(path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success();
        if open {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "{} never opened {}",
            pid,
            path.display()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn pid_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn mib2_text() -> Vec<u8> {
    vec![b'a'; 2 << 20]
}

fn mib2_random(seed: u64) -> Vec<u8> {
    let mut bytes = vec![0u8; 2 << 20];
    let mut state = seed;
    for b in &mut bytes {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
        *b = (state >> 33) as u8;
    }
    bytes
}

fn assert_seeded_units_whole_or_absent(hive: &Hive, lane: &Path) {
    let target = lane.join("target");
    if !target.is_dir() {
        return;
    }
    let state = State::open_at(hive.home.path.clone()).unwrap();
    let Ok(file) = store::read(&state) else {
        return;
    };
    for row in &file.units {
        let fp = target
            .join(&row.rel_profile)
            .join(".fingerprint")
            .join(&row.name_meta);
        if !fp.exists() {
            continue;
        }
        let entry = store::entry_dir(&state, row);
        for rel in entry_relative_files(&entry) {
            let placed = target.join(&rel);
            assert!(
                placed.exists(),
                "partial unit {} missing {} under {}",
                row.name_meta,
                rel.display(),
                target.display()
            );
        }
    }
}

fn entry_relative_files(entry: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![entry.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for item in entries {
            let path = item.unwrap().path();
            let meta = fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                out.push(path.strip_prefix(entry).unwrap().to_path_buf());
            }
        }
    }
    out
}
