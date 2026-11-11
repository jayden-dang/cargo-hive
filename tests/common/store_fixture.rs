//! Scratch-store helpers for store GC tests. Every `store.db` write goes through
//! `State::open_at` and `State::locked` with `Tx::write_bounded`.

use super::registry_fixture::{cargo_json, RegistryRepo};
use super::Hive;
use cargo_hive::seed::store::{store_dir, Row, StoreFile, MAX_STORE_BYTES, STORE_FILE};
use cargo_hive::state::State;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn seeded_store(hive: &Hive) -> RegistryRepo {
    fs::create_dir_all(hive.home.path.join("no-cache")).unwrap();
    let repo = RegistryRepo::new(hive);
    cargo_json(hive, &repo.main, &["build"]);
    let lane = repo.worktree("seed-lane");
    let (out, receipt) = hive.json(&["seed", lane.to_str().unwrap()]);
    assert!(out.status.success(), "{receipt}");
    assert!(
        receipt["counts"]["fill_entries_added"].as_u64().unwrap() > 0,
        "{receipt}"
    );
    repo
}

pub fn rows(hive: &Hive) -> Vec<serde_json::Value> {
    let path = hive.home.path.join("store.db");
    let Ok(bytes) = fs::read(&path) else {
        return Vec::new();
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["units"].as_array().cloned().unwrap_or_default()
}

pub fn set_last_live_at(hive: &Hive, name_meta_prefix: &str, secs_ago: u64) {
    let state = State::open_at(hive.home.path.clone()).unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let at = now.saturating_sub(secs_ago);
    state
        .locked(|tx| {
            let mut file: StoreFile = tx.read_bounded(STORE_FILE, MAX_STORE_BYTES)?;
            for row in &mut file.units {
                if row.name_meta.starts_with(name_meta_prefix) {
                    row.last_live_at = at;
                }
            }
            tx.write_bounded(STORE_FILE, &file, MAX_STORE_BYTES)
        })
        .unwrap();
}

pub fn synth_entry(
    hive: &Hive,
    key: (u64, &str, &str),
    registry_deps: &[&str],
    files: &[(&str, Vec<u8>)],
    last_live_secs_ago: u64,
) {
    let (toolchain_value, name_meta, fingerprint_hash) = key;
    let dir = store_dir_path(hive)
        .join(toolchain_value.to_string())
        .join(format!("{name_meta}-{fingerprint_hash}"));
    fs::create_dir_all(&dir).unwrap();
    let mut size = 0u64;
    for (rel, bytes) in files {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&path, bytes).unwrap();
        size += bytes.len() as u64;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let last_live_at = now.saturating_sub(last_live_secs_ago);
    let row = Row {
        toolchain_value,
        name_meta: name_meta.into(),
        fingerprint_hash: fingerprint_hash.into(),
        kind: "lib".into(),
        rel_profile: PathBuf::from("debug"),
        size,
        filled_at: last_live_at,
        last_live_at,
        repos: Vec::new(),
        registry_deps: registry_deps.iter().map(|s| (*s).to_string()).collect(),
        variant: 0,
    };
    let state = State::open_at(hive.home.path.clone()).unwrap();
    state
        .locked(|tx| {
            let mut file: StoreFile = tx
                .read_bounded(STORE_FILE, MAX_STORE_BYTES)
                .unwrap_or_default();
            file.toolchains
                .entry(format!("synth-{toolchain_value}"))
                .or_insert(toolchain_value);
            file.units.retain(|existing| {
                !(existing.toolchain_value == row.toolchain_value
                    && existing.name_meta == row.name_meta
                    && existing.fingerprint_hash == row.fingerprint_hash)
            });
            file.units.push(row);
            tx.write_bounded(STORE_FILE, &file, MAX_STORE_BYTES)
        })
        .unwrap();
}

pub fn entry_dirs(hive: &Hive) -> Vec<PathBuf> {
    let store = store_dir_path(hive);
    let mut out = Vec::new();
    let Ok(top) = fs::read_dir(&store) else {
        return out;
    };
    for entry in top {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy();
        if name == ".tmp" || !path.is_dir() {
            continue;
        }
        if name.parse::<u64>().is_err() {
            continue;
        }
        for child in fs::read_dir(&path).unwrap() {
            let child = child.unwrap().path();
            if child.is_dir() {
                out.push(child);
            }
        }
    }
    out.sort();
    out
}

fn store_dir_path(hive: &Hive) -> PathBuf {
    let state = State::open_at(hive.home.path.clone()).unwrap();
    store_dir(&state)
}

pub fn drop_row(hive: &Hive, name_meta_prefix: &str) {
    let state = State::open_at(hive.home.path.clone()).unwrap();
    state
        .locked(|tx| {
            let mut file: StoreFile = tx.read_bounded(STORE_FILE, MAX_STORE_BYTES)?;
            file.units
                .retain(|row| !row.name_meta.starts_with(name_meta_prefix));
            tx.write_bounded(STORE_FILE, &file, MAX_STORE_BYTES)
        })
        .unwrap();
}

pub fn plant_rowless(hive: &Hive, toolchain_value: u64, name_meta: &str, fingerprint_hash: &str) {
    let dir = store_dir_path(hive)
        .join(toolchain_value.to_string())
        .join(format!("{name_meta}-{fingerprint_hash}"));
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("blob"), b"rowless").unwrap();
}

pub fn count(receipt: &serde_json::Value, name: &str) -> u64 {
    receipt["counts"][name].as_u64().unwrap_or(0)
}

pub fn has_finding(receipt: &serde_json::Value, kind: &str) -> bool {
    receipt["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|finding| finding["kind"] == kind)
}

pub fn tree_files(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
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
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file() {
                let bytes = fs::read(&path).unwrap();
                out.insert(path, bytes);
            }
        }
    }
    out
}
