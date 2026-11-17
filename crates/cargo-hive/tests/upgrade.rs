//! Committed 0.1.x state files read and rewrite as this hive expects.

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod common;

use cargo_hive_cargo::discover::{SNAPSHOT_FILE, Snapshot};
use cargo_hive_cargo::registry::{self, Registry};
use cargo_hive_core::receipt::Receipt;
use cargo_hive_doctor::{CAPABILITIES_FILE, Capabilities};
use cargo_hive_gc::plan_tier::record::{MAX_PLANS_BYTES, PLANS_FILE, PlansFile};
use cargo_hive_platform::clock::utc_date;
use cargo_hive_seed::record::{MAX_SEEDS_BYTES, SEEDS_FILE, SeedsFile};
use cargo_hive_seed::store::{MAX_STORE_BYTES, STORE_FILE, StoreFile};
use cargo_hive_state::{CONFIG_FILE, Layout, State, StateError};
use common::{Hive, Scratch, init_repo, register};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

const STATE_FILES: &[&str] = &[
    registry::FILE,
    SNAPSHOT_FILE,
    CAPABILITIES_FILE,
    PLANS_FILE,
    STORE_FILE,
    SEEDS_FILE,
];

fn fixtures_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn fixture_homes() -> Vec<PathBuf> {
    let root = fixtures_root();
    let mut homes = Vec::new();
    for entry in fs::read_dir(&root).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with('v') {
            continue;
        }
        let home = entry.path().join("home");
        if home.is_dir() {
            homes.push(home);
        }
    }
    homes.sort();
    homes
}

fn insert_layout_line(fixture: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(fixture.len() + 16);
    let mut lines = fixture.split_inclusive(|b| *b == b'\n');
    let first = lines.next().expect("fixture has a first line");
    out.extend_from_slice(first);
    out.extend_from_slice(b"  \"layout\": 2,\n");
    for line in lines {
        out.extend_from_slice(line);
    }
    out
}

fn strip_layout(value: &mut Value) {
    if let Some(obj) = value.as_object_mut() {
        obj.remove("layout");
    }
}

fn json_contains_layout_key(value: &Value) -> bool {
    match value {
        Value::Object(map) => {
            if map.contains_key("layout") {
                return true;
            }
            map.values().any(json_contains_layout_key)
        }
        Value::Array(items) => items.iter().any(json_contains_layout_key),
        _ => false,
    }
}

fn read_state_file<T: DeserializeOwned + Default>(state: &State, name: &str) -> T {
    match name {
        STORE_FILE => state.read_bounded(name, MAX_STORE_BYTES).unwrap(),
        PLANS_FILE => state.read_bounded(name, MAX_PLANS_BYTES).unwrap(),
        SEEDS_FILE => state.read_bounded(name, MAX_SEEDS_BYTES).unwrap(),
        _ => state.read(name).unwrap(),
    }
}

fn write_state_file<T: Serialize>(state: &State, name: &str, value: &T) {
    state
        .locked(|tx| match name {
            STORE_FILE => tx.write_bounded(name, value, MAX_STORE_BYTES),
            PLANS_FILE => tx.write_bounded(name, value, MAX_PLANS_BYTES),
            SEEDS_FILE => tx.write_bounded(name, value, MAX_SEEDS_BYTES),
            _ => tx.write(name, value),
        })
        .unwrap();
}

fn for_each_state_file(home: &Path, mut body: impl FnMut(&str, &[u8])) {
    for name in STATE_FILES {
        let bytes = fs::read(home.join(name)).unwrap();
        body(name, &bytes);
    }
}

#[test]
fn each_fixture_capabilities_without_unit_layout_reads_as_flat_when_check_f_passed() {
    use cargo_hive_core::unit_path::UnitLayout;
    for home in fixture_homes() {
        let state = State::open_at(home.clone()).unwrap();
        let caps: Capabilities = read_state_file(&state, CAPABILITIES_FILE);
        assert!(
            !caps
                .disabled
                .contains_key(&cargo_hive_doctor::Capability::FingerprintReading),
            "{}: check (f) did not pass in the fixture",
            home.display()
        );
        assert_eq!(
            caps.unit_layout_or_flat(),
            Some(UnitLayout::Flat),
            "{} capabilities unit layout",
            home.display()
        );
    }
}

#[test]
fn every_fixture_set_is_present_and_complete() {
    let homes = fixture_homes();
    assert!(!homes.is_empty(), "expected at least one fixtures/v*/home/");
    for home in homes {
        for name in STATE_FILES {
            assert!(
                home.join(name).is_file(),
                "{} missing {}",
                home.display(),
                name
            );
        }
        let receipts = home.join("receipts");
        assert!(receipts.is_dir(), "{} missing receipts/", home.display());
        let receipt_files: Vec<_> = fs::read_dir(&receipts)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
            .collect();
        assert!(
            !receipt_files.is_empty(),
            "{} receipts/ holds no day file",
            home.display()
        );

        let plans: PlansFile =
            serde_json::from_slice(&fs::read(home.join(PLANS_FILE)).unwrap()).unwrap();
        assert!(
            !plans.plans.is_empty(),
            "{} plans.json has no plan",
            home.display()
        );

        let seeds: SeedsFile =
            serde_json::from_slice(&fs::read(home.join(SEEDS_FILE)).unwrap()).unwrap();
        assert!(
            !seeds.targets.is_empty(),
            "{} seeds.json has no seed record",
            home.display()
        );
    }
}

#[test]
fn each_state_file_reads_the_values_its_release_wrote() {
    for home in fixture_homes() {
        let state = State::open_at(home.clone()).unwrap();
        for_each_state_file(&home, |name, fixture| {
            let mut fixture_value: Value = serde_json::from_slice(fixture).unwrap();
            // A release from 0.1.2 on writes the layout itself; compare the other fields.
            strip_layout(&mut fixture_value);
            let mut written = match name {
                registry::FILE => {
                    let v: Registry = read_state_file(&state, name);
                    serde_json::to_value(v).unwrap()
                }
                SNAPSHOT_FILE => {
                    let v: Snapshot = read_state_file(&state, name);
                    serde_json::to_value(v).unwrap()
                }
                CAPABILITIES_FILE => {
                    let v: Capabilities = read_state_file(&state, name);
                    serde_json::to_value(v).unwrap()
                }
                PLANS_FILE => {
                    let v: PlansFile = read_state_file(&state, name);
                    serde_json::to_value(v).unwrap()
                }
                STORE_FILE => {
                    let v: StoreFile = read_state_file(&state, name);
                    serde_json::to_value(v).unwrap()
                }
                SEEDS_FILE => {
                    let v: SeedsFile = read_state_file(&state, name);
                    serde_json::to_value(v).unwrap()
                }
                other => panic!("unexpected state file {other}"),
            };
            strip_layout(&mut written);
            assert_eq!(
                written,
                fixture_value,
                "{} / {name} field values",
                home.display()
            );
        });
    }
}

#[test]
fn each_state_files_first_write_adds_only_the_layout_line() {
    for home in fixture_homes() {
        for_each_state_file(&home, |name, fixture| {
            let scratch = Scratch::new(&format!("upgrade-first-{name}"));
            fs::write(scratch.path.join(name), fixture).unwrap();
            let state = State::open_at(scratch.path.clone()).unwrap();
            let has_layout = serde_json::from_slice::<Value>(fixture)
                .unwrap()
                .get("layout")
                .is_some();
            match name {
                registry::FILE => {
                    let mut v: Registry = read_state_file(&state, name);
                    if !has_layout {
                        v.layout = Layout::NEWEST;
                    }
                    write_state_file(&state, name, &v);
                }
                SNAPSHOT_FILE => {
                    let mut v: Snapshot = read_state_file(&state, name);
                    if !has_layout {
                        v.layout = Layout::NEWEST;
                    }
                    write_state_file(&state, name, &v);
                }
                CAPABILITIES_FILE => {
                    let mut v: Capabilities = read_state_file(&state, name);
                    if !has_layout {
                        v.layout = Layout::NEWEST;
                    }
                    write_state_file(&state, name, &v);
                }
                PLANS_FILE => {
                    let mut v: PlansFile = read_state_file(&state, name);
                    if !has_layout {
                        v.layout = Layout::NEWEST;
                    }
                    write_state_file(&state, name, &v);
                }
                STORE_FILE => {
                    let mut v: StoreFile = read_state_file(&state, name);
                    if !has_layout {
                        v.layout = Layout::NEWEST;
                    }
                    write_state_file(&state, name, &v);
                }
                SEEDS_FILE => {
                    let mut v: SeedsFile = read_state_file(&state, name);
                    if !has_layout {
                        v.layout = Layout::NEWEST;
                    }
                    write_state_file(&state, name, &v);
                }
                other => panic!("unexpected state file {other}"),
            }
            let got = fs::read(scratch.path.join(name)).unwrap();
            // A file that already records its layout comes back unchanged (UPGRADE-2.4).
            let want = if has_layout {
                fixture.to_vec()
            } else {
                insert_layout_line(fixture)
            };
            assert_eq!(got, want, "{} / {name} first write bytes", home.display());
        });
    }
}

#[test]
fn a_second_write_gives_identical_bytes() {
    for home in fixture_homes() {
        for_each_state_file(&home, |name, fixture| {
            let scratch = Scratch::new(&format!("upgrade-second-{name}"));
            fs::write(scratch.path.join(name), fixture).unwrap();
            let state = State::open_at(scratch.path.clone()).unwrap();
            let rewrite = |state: &State| match name {
                registry::FILE => {
                    let v: Registry = read_state_file(state, name);
                    write_state_file(state, name, &v);
                }
                SNAPSHOT_FILE => {
                    let v: Snapshot = read_state_file(state, name);
                    write_state_file(state, name, &v);
                }
                CAPABILITIES_FILE => {
                    let v: Capabilities = read_state_file(state, name);
                    write_state_file(state, name, &v);
                }
                PLANS_FILE => {
                    let v: PlansFile = read_state_file(state, name);
                    write_state_file(state, name, &v);
                }
                STORE_FILE => {
                    let v: StoreFile = read_state_file(state, name);
                    write_state_file(state, name, &v);
                }
                SEEDS_FILE => {
                    let v: SeedsFile = read_state_file(state, name);
                    write_state_file(state, name, &v);
                }
                other => panic!("unexpected state file {other}"),
            };
            rewrite(&state);
            let first = fs::read(scratch.path.join(name)).unwrap();
            rewrite(&state);
            let second = fs::read(scratch.path.join(name)).unwrap();
            assert_eq!(second, first, "{} / {name} second write", home.display());
        });
    }
}

#[test]
fn appending_a_receipt_keeps_every_earlier_line_and_each_parses() {
    for home in fixture_homes() {
        let receipts = home.join("receipts");
        let receipt_files: Vec<_> = fs::read_dir(&receipts)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jsonl"))
            .collect();
        assert!(
            !receipt_files.is_empty(),
            "{} has no receipt file",
            home.display()
        );

        for path in receipt_files {
            let earlier = fs::read(&path).unwrap();
            for line in String::from_utf8_lossy(&earlier).lines() {
                if line.is_empty() {
                    continue;
                }
                let receipt: Receipt = serde_json::from_str(line)
                    .unwrap_or_else(|e| panic!("{}: parse failed ({e}): {line}", path.display()));
                assert!(
                    receipt.schema == 2 || receipt.schema == 3,
                    "{} schema {}",
                    path.display(),
                    receipt.schema
                );
            }

            let scratch = Scratch::new("upgrade-receipt");
            let state = State::open_at(scratch.path.clone()).unwrap();
            let today = scratch
                .path
                .join("receipts")
                .join(format!("{}.jsonl", utc_date(SystemTime::now())));
            fs::write(&today, &earlier).unwrap();
            state
                .locked(|tx| {
                    tx.append_receipt_line(
                        r#"{"schema":2,"hive_version":"test","macos_version":null,"toolchain":null,"command":"probe","ts":"1970-01-01T00:00:00Z","scope":{"repos":[],"checkouts":[]},"counts":{},"checks":[],"findings":[],"unmeasured":[],"notes":[]}"#,
                    )
                })
                .unwrap();
            let after = fs::read(&today).unwrap();
            assert!(
                after.starts_with(&earlier),
                "{} earlier lines changed after append",
                path.display()
            );
        }
    }
}

#[test]
fn command_json_output_carries_no_layout_key() {
    let home = fixture_homes()
        .into_iter()
        .next()
        .expect("at least one fixture home");
    let hive = Hive::new();
    for name in STATE_FILES {
        fs::copy(home.join(name), hive.home.path.join(name)).unwrap();
    }
    fs::copy(home.join(CONFIG_FILE), hive.home.path.join(CONFIG_FILE)).unwrap();
    let no_cache = hive.home.path.join("no-cache");
    fs::create_dir_all(&no_cache).unwrap();
    hive.config(&format!(
        r#"{{"cache_roots":["{}"],"secondary_roots":["{}"]}}"#,
        no_cache.display(),
        no_cache.display()
    ));
    // Fixture repos point at capture paths that are gone; clear the registry so
    // status does not wait on missing checkouts.
    fs::write(
        hive.home.path.join(registry::FILE),
        "{\n  \"repos\": []\n}\n",
    )
    .unwrap();
    let (_out, fixture_receipt) = hive.json(&["status"]);
    assert!(
        !json_contains_layout_key(&fixture_receipt),
        "fixture status --json carried a layout key: {fixture_receipt}"
    );

    let hive = Hive::ready();
    let scratch = Scratch::new("upgrade-status-repo");
    let repo = init_repo(&scratch.path.join("app"), "");
    hive.config(&format!(
        r#"{{"secondary_roots":["{}"]}}"#,
        scratch.path.display()
    ));
    register(&hive, &repo);
    let (out, receipt) = hive.json(&["status"]);
    assert!(
        out.status.success(),
        "status --json on a ready home failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !json_contains_layout_key(&receipt),
        "ready status --json carried a layout key: {receipt}"
    );
}

#[test]
fn config_json_is_read_without_a_layout_key_and_refused_with_one() {
    let home = fixture_homes()
        .into_iter()
        .next()
        .expect("at least one fixture home");
    let scratch = Scratch::new("upgrade-config");
    fs::copy(home.join(CONFIG_FILE), scratch.path.join(CONFIG_FILE)).unwrap();
    let state = State::open_at(scratch.path.clone()).unwrap();
    let config = state.config().expect("keyless config.json must read");
    assert_eq!(config.gc_window_days, None);

    fs::write(
        scratch.path.join(CONFIG_FILE),
        br#"{"layout":1,"gc_window_days":7}"#,
    )
    .unwrap();
    let err = state.config().expect_err("config with layout must refuse");
    assert!(
        matches!(err, StateError::Malformed { .. }),
        "expected Malformed, got {err}"
    );
}

#[derive(Clone, Copy)]
enum FileDamage {
    Unparseable,
    LayoutTwo,
}

fn damaged_state_bytes(damage: FileDamage, existing: Option<&[u8]>, empty_shape: &str) -> Vec<u8> {
    match damage {
        FileDamage::Unparseable => b"{not json".to_vec(),
        FileDamage::LayoutTwo => {
            let base = match existing {
                Some(bytes) => bytes.to_vec(),
                None => empty_shape.as_bytes().to_vec(),
            };
            let mut value: Value = serde_json::from_slice(&base)
                .unwrap_or_else(|_| serde_json::from_str(empty_shape).unwrap());
            value
                .as_object_mut()
                .expect("state file is an object")
                .insert("layout".into(), Value::from(3));
            serde_json::to_vec(&value).unwrap()
        }
    }
}

fn error_detail(receipt: &Value) -> &str {
    receipt["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["kind"] == "error")
        .expect("error finding")["detail"]
        .as_str()
        .unwrap()
}

#[test]
fn status_stops_on_each_damaged_state_file_and_leaves_it_untouched() {
    let cases = [
        (
            registry::FILE,
            r#"{"repos":[]}"#,
            "remove it, then run `cargo hive repo add <main>` again for each repository",
        ),
        (
            CAPABILITIES_FILE,
            r#"{"toolchain_release":"1.0.0","disabled":{},"lock_check":1,"failed_marker_check":1}"#,
            "run `cargo hive doctor`, which rewrites it",
        ),
        (
            SNAPSHOT_FILE,
            r#"{"metadata":{},"unsupported_profiles":[]}"#,
            "remove it; the next `cargo hive status` rebuilds it (it is a metadata cache)",
        ),
    ];
    for (name, empty_shape, recovery) in cases {
        for damage in [FileDamage::Unparseable, FileDamage::LayoutTwo] {
            let hive = Hive::ready();
            let scratch = Scratch::new(&format!("upgrade-status-{name}"));
            let repo = init_repo(&scratch.path.join("app"), "");
            hive.config(&format!(
                r#"{{"secondary_roots":["{}"]}}"#,
                scratch.path.display()
            ));
            register(&hive, &repo);

            let path = hive.home.path.join(name);
            let existing = fs::read(&path).ok();
            let damaged = damaged_state_bytes(damage, existing.as_deref(), empty_shape);
            fs::write(&path, &damaged).unwrap();

            let (out, receipt) = hive.json(&["status"]);
            assert_eq!(out.status.code(), Some(1), "{name} {receipt}");
            let detail = error_detail(&receipt);
            assert!(
                detail.contains(path.to_str().unwrap()),
                "{name}: detail missing path: {detail}"
            );
            match damage {
                FileDamage::Unparseable => assert!(
                    detail.contains(recovery),
                    "{name}: detail missing recovery: {detail}"
                ),
                FileDamage::LayoutTwo => assert!(
                    detail.contains("records layout 3")
                        && detail.contains("cargo install cargo-hive"),
                    "{name}: detail missing newer-layout text: {detail}"
                ),
            }
            assert_eq!(fs::read(&path).unwrap(), damaged, "{name} bytes changed");
        }
    }
}

#[cfg(target_os = "macos")]
#[test]
fn gc_plan_stops_on_a_damaged_plans_file_and_leaves_it_untouched() {
    let recovery =
        "remove it; unapplied plans are lost, so make new ones with `cargo hive gc --plan`";
    for damage in [FileDamage::Unparseable, FileDamage::LayoutTwo] {
        let hive = Hive::ready();
        let path = hive.home.path.join(PLANS_FILE);
        let damaged = damaged_state_bytes(damage, None, r#"{"plans":[]}"#);
        fs::write(&path, &damaged).unwrap();

        let (out, receipt) = hive.json(&["gc", "--plan"]);
        assert_eq!(out.status.code(), Some(1), "{receipt}");
        let detail = error_detail(&receipt);
        assert!(
            detail.contains(path.to_str().unwrap()),
            "detail missing path: {detail}"
        );
        match damage {
            FileDamage::Unparseable => assert!(
                detail.ends_with(recovery),
                "detail missing plans recovery: {detail}"
            ),
            FileDamage::LayoutTwo => assert!(
                detail.contains("records layout 3") && detail.contains("cargo install cargo-hive"),
                "detail missing newer-layout text: {detail}"
            ),
        }
        assert_eq!(fs::read(&path).unwrap(), damaged);
    }
}
