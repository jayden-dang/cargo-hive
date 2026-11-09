#![cfg(target_os = "macos")]
mod common;

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use common::store_fixture::{
    count, drop_row, entry_dirs, has_finding, plant_rowless, rows, seeded_store, set_last_live_at,
    synth_entry, tree_files,
};
use common::{crate_dirs, plant_session, Hive};

const DAY: u64 = 86_400;

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
