#![allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod common;
use common::{Hive, Scratch, git, init_repo, register};
use std::fs;
use std::path::{Path, PathBuf};

const CARGO_TAG: &str = "Signature: 8a477f597d28d172789f06886806bc55\n# This file is a cache directory tag created by cargo.\n# For information about cache directory tags see https://bford.info/cachedir/\n";

/// A checkout whose target dir something other than Cargo created: Cargo's layout, no tag.
fn untagged_checkout(s: &Scratch, name: &str) -> (PathBuf, PathBuf) {
    let repo = init_repo(&s.path.join(name), "");
    let target = repo.join("target");
    fs::create_dir_all(target.join("debug/.fingerprint/fixture-0123456789abcdef")).unwrap();
    fs::create_dir_all(target.join("nextest")).unwrap();
    (repo, target)
}

fn tree(root: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                out.push((path.clone(), None));
                stack.push(path);
            } else {
                out.push((path.clone(), Some(fs::read(&path).unwrap())));
            }
        }
    }
    out.sort();
    out
}

fn findings<'a>(receipt: &'a serde_json::Value, kind: &str) -> Vec<&'a serde_json::Value> {
    receipt["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["kind"] == kind)
        .collect()
}

fn assert_names_the_remedy(finding: &serde_json::Value, target: &Path) {
    assert_eq!(finding["path"], serde_json::json!(target), "{finding}");
    let detail = finding["detail"].as_str().unwrap();
    assert!(detail.contains("CACHEDIR.TAG"), "{detail}");
    assert!(
        detail.contains("8a477f597d28d172789f06886806bc55"),
        "{detail}"
    );
    assert!(detail.contains("https://bford.info/cachedir/"), "{detail}");
}

#[test]
fn status_reports_a_cargo_shaped_target_that_has_no_tag_and_leaves_it_alone() {
    let hive = Hive::ready();
    let s = Scratch::new("untagged-status");
    let (repo, target) = untagged_checkout(&s, "app");
    register(&hive, &repo);
    let before = tree(&target);
    let (out, receipt) = hive.json(&["status", repo.to_str().unwrap()]);
    assert!(out.status.success(), "{receipt}");
    let found = findings(&receipt, "target.untagged");
    assert_eq!(found.len(), 1, "{receipt}");
    assert_names_the_remedy(found[0], &target);
    assert_eq!(receipt["counts"]["targets"], 0, "{receipt}");
    assert_eq!(tree(&target), before);
}

#[test]
fn a_target_with_only_cargos_rustc_info_file_is_reported_as_untagged_too() {
    let hive = Hive::ready();
    let s = Scratch::new("untagged-rustc-info");
    let repo = init_repo(&s.path.join("app"), "");
    let target = repo.join("target");
    fs::create_dir_all(&target).unwrap();
    fs::write(target.join(".rustc_info.json"), "{}").unwrap();
    register(&hive, &repo);
    let (out, receipt) = hive.json(&["status", repo.to_str().unwrap()]);
    assert!(out.status.success(), "{receipt}");
    assert_eq!(findings(&receipt, "target.untagged").len(), 1, "{receipt}");
}

#[test]
fn a_directory_that_is_not_cargo_shaped_or_is_tagged_gets_no_untagged_finding() {
    let hive = Hive::ready();
    let s = Scratch::new("untagged-negative");
    let plain = init_repo(&s.path.join("plain"), "");
    fs::create_dir_all(plain.join("target/nextest")).unwrap();
    let (tagged, tagged_target) = untagged_checkout(&s, "tagged");
    fs::write(tagged_target.join("CACHEDIR.TAG"), CARGO_TAG).unwrap();
    let only_info = init_repo(&s.path.join("info"), "");
    fs::create_dir_all(only_info.join("target")).unwrap();
    fs::write(only_info.join("target/.rustc_info.json"), "{}").unwrap();
    fs::write(only_info.join("target/CACHEDIR.TAG"), CARGO_TAG).unwrap();
    for repo in [&plain, &tagged, &only_info] {
        register(&hive, repo);
    }
    let (out, receipt) = hive.json(&["status"]);
    assert!(out.status.success(), "{receipt}");
    assert_eq!(findings(&receipt, "target.untagged").len(), 0, "{receipt}");
    assert_eq!(findings(&receipt, "target.owned").len(), 1, "{receipt}");
}

#[test]
fn seed_reports_an_untagged_target_of_the_repository_and_does_not_touch_it() {
    let hive = Hive::ready();
    let s = Scratch::new("untagged-seed");
    let main = init_repo(&s.path.join("app"), "");
    register(&hive, &main);
    let lane = hive.secondary_root.path.join("lane");
    git(&main, &["worktree", "add", "-q", lane.to_str().unwrap()]);
    let lane = lane.canonicalize().unwrap();
    let target = lane.join("target");
    fs::create_dir_all(target.join("debug/.fingerprint/fixture-0123456789abcdef")).unwrap();
    let before = tree(&target);
    let (out, receipt) = hive.json(&["seed", lane.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "{receipt}");
    let found = findings(&receipt, "target.untagged");
    assert_eq!(found.len(), 1, "{receipt}");
    assert_names_the_remedy(found[0], &target);
    assert_eq!(tree(&target), before);
}

#[test]
fn checkout_gc_reports_an_untagged_target_and_deletes_nothing_in_it() {
    let hive = Hive::ready();
    let s = Scratch::new("untagged-gc");
    let (repo, target) = untagged_checkout(&s, "app");
    register(&hive, &repo);
    let before = tree(&target);
    let (_, receipt) = hive.json(&["gc", "--checkout", repo.to_str().unwrap()]);
    let found = findings(&receipt, "target.untagged");
    assert_eq!(found.len(), 1, "{receipt}");
    assert_names_the_remedy(found[0], &target);
    assert_eq!(tree(&target), before);
}

#[test]
fn a_target_that_seed_creates_carries_cargos_exact_tag_and_no_untagged_finding() {
    use common::registry_fixture::{RegistryRepo, cargo_json};
    let hive = Hive::ready();
    fs::create_dir_all(hive.home.path.join("no-cache")).unwrap();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let lane = repo.worktree("seeded");
    let (out, receipt) = hive.json(&["seed", lane.to_str().unwrap()]);
    assert!(out.status.success(), "{receipt}");
    assert_eq!(
        fs::read_to_string(lane.join("target/CACHEDIR.TAG")).unwrap(),
        CARGO_TAG
    );
    assert_eq!(findings(&receipt, "target.untagged").len(), 0, "{receipt}");
}

#[test]
fn seed_tags_the_target_it_creates_even_when_the_store_has_no_unit_to_place() {
    use common::registry_fixture::{RegistryRepo, cargo_json};
    let hive = Hive::ready();
    fs::create_dir_all(hive.home.path.join("no-cache")).unwrap();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let warm = repo.worktree("warm");
    let (out, receipt) = hive.json(&["seed", warm.to_str().unwrap()]);
    assert!(out.status.success(), "{receipt}");
    fs::remove_dir_all(warm.join("target")).unwrap();
    fs::remove_dir_all(repo.main.join("target")).unwrap();
    for entry in fs::read_dir(hive.home.path.join("store")).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().is_some_and(|name| name == "CACHEDIR.TAG") {
            continue;
        }
        if path.is_dir() {
            fs::remove_dir_all(&path).unwrap();
        } else {
            fs::remove_file(&path).unwrap();
        }
    }
    let lane = repo.worktree("empty-choice");
    let (out, receipt) = hive.json(&["seed", lane.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "{receipt}");
    assert_eq!(findings(&receipt, "units.empty").len(), 1, "{receipt}");
    assert_eq!(
        fs::read_to_string(lane.join("target/CACHEDIR.TAG")).unwrap(),
        CARGO_TAG
    );
    assert!(!lane.join("target/debug").exists());
}
