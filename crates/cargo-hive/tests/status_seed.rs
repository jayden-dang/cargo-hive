#![cfg(target_os = "macos")]
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod common;

use std::collections::BTreeMap;
use std::fs;
use std::fs::File;
use std::io::Write;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use common::Hive;
use common::registry_fixture::{RegistryRepo, cargo_json};

#[test]
fn after_the_first_build_status_reports_reused_units_and_no_recompiled_ones() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    cargo_json(&hive, &repo.main, &["check"]);
    let lane = repo.worktree("lane");
    hive.json(&["seed", lane.to_str().unwrap()]);
    cargo_json(&hive, &lane, &["build"]);
    let (_, status) = hive.json(&["status", repo.main.to_str().unwrap()]);
    let target = lane.join("target").canonicalize().unwrap();
    let key = |name: &str| format!("{name}:{}", target.display());
    assert!(status["counts"][key("reused_fresh")].as_u64().unwrap() > 0);
    assert_eq!(status["counts"][key("seeded_recompiled")], 0);
    assert_eq!(status["counts"][key("seeded_missing")], 0);
    assert!(
        status["counts"][key("seeded_unneeded")].as_u64().unwrap() > 0,
        "check-mode units"
    );
}

#[test]
fn status_for_a_target_with_no_record_is_unchanged() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let (_, before) = hive.json(&["status", repo.main.to_str().unwrap()]);
    let lane = repo.worktree("lane");
    let (out, seeded) = hive.json(&["seed", lane.to_str().unwrap()]);
    assert!(out.status.success(), "{seeded}");
    assert!(seeded["counts"]["clone_units_cloned"].as_u64().unwrap() > 0);
    let (_, after) = hive.json(&["status", repo.main.to_str().unwrap()]);
    let main_target = repo.main.join("target").canonicalize().unwrap();
    let shown = |status: &serde_json::Value| {
        let mut map = BTreeMap::new();
        let path = main_target.display().to_string();
        for (key, value) in status["counts"].as_object().unwrap() {
            if key.contains(&path) {
                map.insert(key.clone(), value.clone());
            }
        }
        map
    };
    assert_eq!(shown(&before), shown(&after));
    assert!(shown(&before).keys().all(|key| {
        !key.starts_with("reused_fresh:")
            && !key.starts_with("seeded_unneeded:")
            && !key.starts_with("seeded_recompiled:")
            && !key.starts_with("seeded_missing:")
    }));
}

#[test]
fn a_rebuilt_target_reports_no_seed_counts() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let lane = repo.worktree("lane");
    let (out, seeded) = hive.json(&["seed", lane.to_str().unwrap()]);
    assert!(out.status.success(), "{seeded}");
    cargo_json(&hive, &lane, &["build"]);
    let target = lane.join("target");
    let before = fs::symlink_metadata(&target).unwrap();
    fs::remove_dir_all(&target).unwrap();
    cargo_json(&hive, &lane, &["build"]);
    let after = fs::symlink_metadata(&target).unwrap();
    assert_ne!(
        (before.dev(), before.ino()),
        (after.dev(), after.ino()),
        "the rebuilt target is a different directory"
    );
    let (_, status) = hive.json(&["status", repo.main.to_str().unwrap()]);
    let canon = target.canonicalize().unwrap();
    let counts = status["counts"].as_object().unwrap();
    for name in [
        "reused_fresh",
        "seeded_unneeded",
        "seeded_recompiled",
        "seeded_missing",
    ] {
        let key = format!("{name}:{}", canon.display());
        assert!(counts.get(&key).is_none(), "{key} in {status}");
    }
}

#[test]
fn an_unreadable_record_reports_no_counts_and_one_note() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let lane = repo.worktree("lane");
    let (out, seeded) = hive.json(&["seed", lane.to_str().unwrap()]);
    assert!(out.status.success(), "{seeded}");
    let path = hive.home.path.join("seeds.json");
    fs::write(&path, b"{not json").unwrap();
    let mut perms = fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o000);
    fs::set_permissions(&path, perms).unwrap();
    let (out, status) = hive.json(&["status", repo.main.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "{status} {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let notes: Vec<_> = status["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|note| note.as_str().unwrap().to_string())
        .collect();
    let about: Vec<_> = notes
        .iter()
        .filter(|note| note.contains("seeds.json"))
        .collect();
    assert_eq!(about.len(), 1, "{notes:?}");
    let counts = status["counts"].as_object().unwrap();
    assert!(
        counts.keys().all(|key| {
            !key.starts_with("reused_fresh:")
                && !key.starts_with("seeded_unneeded:")
                && !key.starts_with("seeded_recompiled:")
                && !key.starts_with("seeded_missing:")
        }),
        "{counts:?}"
    );
}

#[test]
fn the_counts_are_as_of_each_status() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    cargo_json(&hive, &repo.main, &["check"]);
    let lane = repo.worktree("lane");
    hive.json(&["seed", lane.to_str().unwrap()]);
    cargo_json(&hive, &lane, &["build"]);
    let target = lane.join("target").canonicalize().unwrap();
    let key = |name: &str| format!("{name}:{}", target.display());
    let count =
        |status: &serde_json::Value, name: &str| status["counts"][&key(name)].as_u64().unwrap();
    let (_, first) = hive.json(&["status", repo.main.to_str().unwrap()]);
    let fresh = count(&first, "reused_fresh");
    let recompiled = count(&first, "seeded_recompiled");
    assert!(fresh > 0, "{first}");
    assert_eq!(recompiled, 0);
    cargo_json(&hive, &lane, &["build"]);
    bump_one_stamp(&hive, &target);
    let (_, second) = hive.json(&["status", repo.main.to_str().unwrap()]);
    assert!(
        count(&second, "seeded_recompiled") > recompiled,
        "first {first} second {second}"
    );
    assert_ne!(count(&second, "reused_fresh"), fresh, "{second}");
}

fn bump_one_stamp(hive: &Hive, target: &Path) {
    let path = hive.home.path.join("seeds.json");
    let file: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let targets = file["targets"].as_object().unwrap();
    let key = targets
        .keys()
        .find(|key| Path::new(key.as_str()) == target)
        .unwrap_or_else(|| panic!("no record for {}", target.display()));
    for unit in file["targets"][key]["units"].as_array().unwrap() {
        let Some(stamp) = unit["stamp"].as_array() else {
            continue;
        };
        let rel = stamp[0].as_str().unwrap();
        let recorded = stamp[1].as_u64().unwrap();
        let stamp_path = target.join(rel);
        let mut opened = File::options().write(true).open(&stamp_path).unwrap();
        opened.write_all(b"touched").unwrap();
        let bumped = SystemTime::UNIX_EPOCH + Duration::from_secs(86_400);
        opened.set_modified(bumped).unwrap();
        let got = fs::metadata(&stamp_path)
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        assert_ne!(
            u64::try_from(got).unwrap(),
            recorded,
            "{}",
            stamp_path.display()
        );
        return;
    }
    panic!("the record has no stamp to touch");
}
