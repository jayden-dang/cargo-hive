#![cfg(target_os = "macos")]
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod common;

use cargo_hive_core::liveness::Liveness;
use cargo_hive_core::unit_path::{self, UnitLayout};
use cargo_hive_gc::checkout::{RequiredRoot, RootPresence, RootRole};
use common::cargo_fixture::{
    THIRTY_DAYS, WINDOW, backdate_all, decide, decide_at, layout_of, set_mtime,
};
use common::plan_fixture::tree;
use common::recording_fs::Recording;
use common::registry_fixture::{RegistryRepo, cargo_json, set_target_dir};
use common::unit_dirs::plant_unit;
use common::{Hive, Scratch, git, init_repo, interrupt, register, wait_within};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

#[test]
fn the_binaries_fixture_links_a_bin_a_test_an_example_a_build_script_and_a_proc_macro_offline() {
    let hive = Hive::ready();
    let repo = RegistryRepo::with_binaries(&hive);
    let _built = cargo_json(&hive, &repo.main, &["test", "--no-run"]);
    let target = repo.main.join("target");
    let debug = target.join("debug");
    let layout = layout_of(&target);

    assert!(debug.join("app").is_file(), "bin missing");
    assert!(debug.join("examples/ex").is_file(), "example missing");
    match layout {
        UnitLayout::Flat => {
            let deps = debug.join("deps");
            assert!(
                named_under(&deps, "it-").is_some_and(|p| is_linked_binary(&p)),
                "integration test binary missing under deps/"
            );
            assert!(
                build_script_under(&debug.join("build")).is_some(),
                "build-script-build missing under build/"
            );
            assert!(
                deps_dylib(&deps, "libappmacro-").is_some(),
                "proc-macro dylib missing under deps/"
            );
            let objects: Vec<_> = fs::read_dir(&deps)
                .unwrap()
                .map(|e| e.unwrap().path())
                .filter(|p| {
                    p.extension().is_some_and(|x| x == "o")
                        && p.to_string_lossy().contains(".rcgu.")
                })
                .collect();
            assert!(
                !objects.is_empty(),
                "expected loose deps/*.rcgu.o beside the linked binaries"
            );
        }
        UnitLayout::UnitDir => {
            let fs = cargo_hive_platform::current().unwrap().fs;
            let listing =
                cargo_hive_gc::checkout::list_profile(fs.as_ref(), &debug, 1_000_000, 100_000)
                    .unwrap();
            assert!(
                listing
                    .objects
                    .iter()
                    .any(|(p, _, _)| p.to_string_lossy().contains("/out/")
                        && p.to_string_lossy().contains(".rcgu.")),
                "expected out/*.rcgu.o objects: {:?}",
                listing.objects
            );
            assert!(
                listing.candidates.iter().any(|p| {
                    p.file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with("it-"))
                        && is_linked_binary(p)
                }),
                "integration test binary missing under out/: {:?}",
                listing.candidates
            );
            assert!(
                listing.candidates.iter().any(|p| {
                    p.file_name()
                        .is_some_and(|n| n.to_string_lossy().contains("appmacro"))
                        || p.file_name()
                            .is_some_and(|n| n.to_string_lossy().ends_with(".dylib"))
                }),
                "proc-macro dylib missing under out/: {:?}",
                listing.candidates
            );
            assert!(
                build_script_under_unit_dir(&debug.join("build")).is_some(),
                "build-script-build missing under build/<pkg>/<hex>/"
            );
        }
    }
}

#[test]
fn a_checkouts_dead_units_are_deleted_in_one_run_with_no_plan_stored() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let target = repo.main.join("target");
    let before = fingerprint_dirs(&target);
    assert!(before.len() > 1, "need dead and live fingerprint dirs");
    let (out, r) = checkout_gc(&hive, &repo.main);
    assert!(out.status.success(), "{r}");
    assert!(count(&r, "units_deleted") >= 1, "{r}");
    assert!(count(&r, "unit_files_deleted") >= 1, "{r}");
    let plans = hive.home.path.join("plans.json");
    assert!(
        !plans.exists() || fs::read_to_string(&plans).unwrap().contains("\"plans\":[]"),
        "checkout gc must not store a plan"
    );
    let after = fingerprint_dirs(&target);
    assert!(after.len() < before.len(), "dead fingerprint dirs remain");
}

#[test]
fn units_compiled_inside_the_window_are_kept_and_the_next_build_recompiles_none() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let (out, r) = checkout_gc(&hive, &repo.main);
    assert!(out.status.success(), "{r}");
    assert!(count(&r, "units_deleted") >= 1, "{r}");
    let again = cargo_json(&hive, &repo.main, &["test", "--no-run"]);
    assert!(
        again.workspace_recompiled.is_empty(),
        "workspace recompiled: {:?}",
        again.workspace_recompiled
    );
    assert!(
        again.registry_recompiled.is_empty(),
        "registry recompiled: {:?}",
        again.registry_recompiled
    );
}

#[test]
fn nothing_under_build_is_touched() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let target = repo.main.join("target");
    let build = target.join("debug/build");
    let before = tree(&build);
    let before_units = fingerprint_dirs(&target);
    let (out, r) = checkout_gc(&hive, &repo.main);
    assert!(out.status.success(), "{r}");
    match layout_of(&target) {
        UnitLayout::Flat => {
            assert_eq!(tree(&build), before, "build/ changed: {r}");
        }
        UnitLayout::UnitDir => {
            // Units live under build/ here, so dead unit dirs go (BUILDDIR-4.1) and nothing else
            // does: every unit dir that remains was there before, and the count says how many left.
            let after_units = fingerprint_dirs(&target);
            assert!(
                after_units.iter().all(|dir| before_units.contains(dir)),
                "a unit dir appeared under build/: {r}"
            );
            assert_eq!(
                count(&r, "units_deleted"),
                (before_units.len() - after_units.len()) as u64,
                "{r}"
            );
        }
    }
}

#[test]
fn an_undecided_target_deletes_no_unit_and_names_each_reason_with_what_clears_it() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let target = repo.main.join("target");
    let before = fingerprint_dirs(&target);
    let unit = newest_fingerprint_unit(&target);
    let dir = fingerprint_dir_for(&target, &unit);
    let hash = hash_file_in(&dir);
    fs::write(&hash, b"").unwrap();
    set_mtime(&hash, SystemTime::now());
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|ext| ext == "json") {
            fs::remove_file(&path).unwrap();
        }
    }
    let (out, r) = checkout_gc(&hive, &repo.main);
    assert_eq!(out.status.code(), Some(0), "{r}");
    assert_eq!(count(&r, "units_deleted"), 0, "{r}");
    let findings = r["findings"].as_array().unwrap();
    assert!(
        findings.iter().any(|f| {
            f["kind"] == "gc.undecided"
                && f["path"] == serde_json::json!(target)
                && f["detail"].as_str().unwrap().contains("clears when")
        }),
        "{r}"
    );
    assert_eq!(
        fingerprint_dirs(&target),
        before,
        "undecided run deleted fingerprint dirs: {r}"
    );
}

#[test]
fn an_out_of_range_window_is_refused_with_exit_1() {
    let hive = Hive::ready();
    let repo = RegistryRepo::with_binaries(&hive);
    cargo_json(&hive, &repo.main, &["test", "--no-run"]);
    hive.config(&format!(
        r#"{{"secondary_roots":["{}"], "gc_window_days": 0}}"#,
        repo.scratch.path.display()
    ));
    let (out, r) = checkout_gc(&hive, &repo.main);
    assert_eq!(out.status.code(), Some(1), "{r}");
    assert!(
        r["findings"].as_array().unwrap().iter().any(|f| {
            f["kind"] == "refusal" && f["detail"].as_str().unwrap().contains("gc_window_days")
        }),
        "{r}"
    );
}

#[test]
fn checkout_with_repos_plan_or_dry_run_is_an_argument_error_with_exit_2_and_no_receipt() {
    let hive = Hive::ready();
    let repo = RegistryRepo::with_binaries(&hive);
    let path = repo.main.to_str().unwrap();
    let cases = [
        vec!["gc", "--checkout", path, path],
        vec!["gc", "--checkout", path, "--plan"],
        vec!["gc", "--checkout", path, "--dry-run"],
    ];
    for args in cases {
        let out = hive.run(&args);
        assert_eq!(out.status.code(), Some(2), "{args:?} => {out:?}");
        assert!(
            !receipts_mention(&hive, "gc --checkout"),
            "wrote a receipt for {args:?}"
        );
    }
}

#[test]
fn a_missing_capability_refuses_with_exit_1() {
    let hive = Hive::ready();
    let repo = RegistryRepo::with_binaries(&hive);
    cargo_json(&hive, &repo.main, &["test", "--no-run"]);
    let path = hive.home.path.join("capabilities.json");
    let mut caps: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    caps["disabled"] = serde_json::json!({"fingerprint_reading": ["f"]});
    fs::write(&path, caps.to_string()).unwrap();
    let (out, r) = checkout_gc(&hive, &repo.main);
    assert_eq!(out.status.code(), Some(1), "{r}");
    assert!(
        r["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["kind"] == "refusal"),
        "{r}"
    );
}

#[test]
fn without_eviction_accounting_bytes_are_unmeasured_but_units_are_deleted() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let path = hive.home.path.join("capabilities.json");
    let mut caps: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    caps["disabled"] = serde_json::json!({"eviction_accounting": ["c"]});
    fs::write(&path, caps.to_string()).unwrap();
    let (out, r) = checkout_gc(&hive, &repo.main);
    assert!(out.status.success(), "{r}");
    assert!(count(&r, "units_deleted") >= 1, "{r}");
    assert!(r["counts"].get("freed_bytes").is_none(), "{r}");
    assert!(
        r["unmeasured"]
            .to_string()
            .contains("doctor check (c) did not pass"),
        "{r}"
    );
}

#[test]
fn the_receipt_is_schema_2_with_command_gc_checkout_and_only_that_checkout_in_scope() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let wt = repo.worktree("lane");
    cargo_json(&hive, &wt, &["test", "--no-run"]);
    backdate_all(&wt.join("target"), THIRTY_DAYS);
    bump_app_version(&wt);
    cargo_json(&hive, &wt, &["test", "--no-run"]);
    let (out, r) = checkout_gc(&hive, &wt);
    assert!(out.status.success(), "{r}");
    assert_eq!(r["schema"], 2, "{r}");
    assert_eq!(r["command"], "gc --checkout", "{r}");
    assert_eq!(r["scope"]["repos"], serde_json::json!([repo.main]), "{r}");
    assert_eq!(r["scope"]["checkouts"], serde_json::json!([wt]), "{r}");
}

#[test]
fn an_interrupted_checkout_gc_exits_130_with_a_receipt() {
    let hive = Hive::ready();
    let repo = RegistryRepo::with_binaries(&hive);
    cargo_json(&hive, &repo.main, &["test", "--no-run"]);
    let bin = repo.scratch.path.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let sentinel = repo.scratch.path.join("cargo.pid");
    fs::write(
        bin.join("cargo"),
        "#!/bin/sh\nprintf '%s\\n' \"$$\" > \"$HIVE_CARGO_SENTINEL\"\nsleep 300\n",
    )
    .unwrap();
    fs::set_permissions(bin.join("cargo"), fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let child = hive
        .command(&["gc", "--checkout", repo.main.to_str().unwrap(), "--json"])
        .env("PATH", &path)
        .env("HIVE_CARGO_SENTINEL", &sentinel)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let hive_pid = child.id();
    let started = Instant::now();
    loop {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "cargo metadata never wrote its pid"
        );
        if let Ok(text) = fs::read_to_string(&sentinel)
            && text.trim().parse::<u32>().is_ok()
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(pid_alive(hive_pid), "gc should be running before SIGINT");
    interrupt(hive_pid);
    let out = wait_within(child, Duration::from_secs(30));
    assert_eq!(out.status.code(), Some(130), "{out:?}");
    assert!(
        receipts_mention(&hive, "gc --checkout"),
        "a cancelled checkout gc appends a receipt"
    );
}

#[test]
fn a_cancel_right_after_discovery_deletes_nothing_and_exits_130() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let target = repo.main.join("target");
    let before = tree(&target);
    let bin = repo.scratch.path.join("bin-cancel");
    fs::create_dir_all(&bin).unwrap();
    let sentinel = repo.scratch.path.join("cargo-cancel.pid");
    fs::write(
        bin.join("cargo"),
        "#!/bin/sh\nprintf '%s\\n' \"$$\" > \"$HIVE_CARGO_SENTINEL\"\nsleep 300\n",
    )
    .unwrap();
    fs::set_permissions(bin.join("cargo"), fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let child = hive
        .command(&["gc", "--checkout", repo.main.to_str().unwrap(), "--json"])
        .env("PATH", &path)
        .env("HIVE_CARGO_SENTINEL", &sentinel)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let hive_pid = child.id();
    let started = Instant::now();
    loop {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "cargo metadata never wrote its pid"
        );
        if let Ok(text) = fs::read_to_string(&sentinel)
            && text.trim().parse::<u32>().is_ok()
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    interrupt(hive_pid);
    let out = wait_within(child, Duration::from_secs(30));
    assert_eq!(out.status.code(), Some(130), "{out:?}");
    assert_eq!(
        tree(&target),
        before,
        "a cancel after discovery deleted units"
    );
}

#[test]
fn a_failed_deletion_exits_1() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let debug = repo.main.join("target/debug");
    let plant = orphan_object_dir(&debug);
    let orphan = plant.join("orphan-faileddel0001.0.rcgu.o");
    fs::write(&orphan, b"orphan").unwrap();
    fs::set_permissions(&plant, fs::Permissions::from_mode(0o500)).unwrap();
    let (out, r) = checkout_gc(&hive, &repo.main);
    let _ = fs::set_permissions(&plant, fs::Permissions::from_mode(0o755));
    assert_eq!(out.status.code(), Some(1), "{r}");
    assert!(
        r["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["kind"] == "gc.delete_failed"),
        "{r}"
    );
}

#[test]
fn a_receipt_that_cannot_be_recorded_exits_1() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let receipts = hive.home.path.join("receipts");
    fs::create_dir_all(&receipts).unwrap();
    fs::set_permissions(&receipts, fs::Permissions::from_mode(0o500)).unwrap();
    let (out, r) = checkout_gc(&hive, &repo.main);
    let _ = fs::set_permissions(&receipts, fs::Permissions::from_mode(0o755));
    assert_eq!(out.status.code(), Some(1), "{r}");
}

#[test]
fn walk_faults_become_walk_unreadable_findings() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let blocked = repo.scratch.path.join("blocked");
    fs::create_dir_all(&blocked).unwrap();
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();
    hive.config(&format!(
        r#"{{"secondary_roots":["{}"]}}"#,
        repo.scratch.path.display()
    ));
    let (out, r) = checkout_gc(&hive, &repo.main);
    let _ = fs::set_permissions(&blocked, fs::Permissions::from_mode(0o755));
    assert_eq!(out.status.code(), Some(0), "{r}");
    assert!(
        r["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["kind"] == "walk.unreadable"),
        "{r}"
    );
}

#[test]
fn a_target_two_worktrees_resolve_to_is_left_and_named() {
    let hive = Hive::ready();
    let repo = RegistryRepo::with_binaries(&hive);
    let wt = repo.worktree("shared-lane");
    let shared = repo.scratch.path.join("shared-target");
    set_target_dir(&repo.main, &shared);
    set_target_dir(&wt, &shared);
    cargo_json(&hive, &repo.main, &["test", "--no-run"]);
    backdate_all(&shared, THIRTY_DAYS);
    bump_app_version(&repo.main);
    cargo_json(&hive, &repo.main, &["test", "--no-run"]);
    let before = tree(&shared);
    let (out, r) = checkout_gc(&hive, &repo.main);
    assert!(out.status.success(), "{r}");
    assert_eq!(tree(&shared), before, "shared target was touched: {r}");
    assert!(
        r["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| { f["kind"] == "gc.shared_target" && f["path"] == serde_json::json!(shared) }),
        "{r}"
    );
    assert_eq!(count(&r, "targets_shared"), 1, "{r}");
}

#[test]
fn a_broken_manifest_in_any_worktree_blocks_every_deletion_and_exits_1() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let wt = repo.worktree("broken");
    fs::write(wt.join("Cargo.toml"), "this is not a manifest\n").unwrap();
    let target = repo.main.join("target");
    let before = tree(&target);
    let (out, r) = checkout_gc(&hive, &repo.main);
    assert_eq!(out.status.code(), Some(1), "{r}");
    assert_eq!(tree(&target), before, "broken manifest still deleted: {r}");
    assert!(
        r["findings"].as_array().unwrap().iter().any(|f| {
            f["kind"] == "ownership.unresolved"
                && f["detail"]
                    .as_str()
                    .unwrap()
                    .contains("cargo metadata succeeds")
        }),
        "{r}"
    );
}

#[test]
fn a_target_dir_outside_the_checkout_is_left_and_named() {
    let hive = Hive::ready();
    let repo = RegistryRepo::with_binaries(&hive);
    let outside = repo.scratch.path.join("outside-target");
    set_target_dir(&repo.main, &outside);
    cargo_json(&hive, &repo.main, &["test", "--no-run"]);
    backdate_all(&outside, THIRTY_DAYS);
    bump_app_version(&repo.main);
    cargo_json(&hive, &repo.main, &["test", "--no-run"]);
    let before = tree(&outside);
    let (out, r) = checkout_gc(&hive, &repo.main);
    assert!(out.status.success(), "{r}");
    assert_eq!(tree(&outside), before, "outside target was touched: {r}");
    assert!(
        r["findings"].as_array().unwrap().iter().any(|f| {
            f["kind"] == "gc.outside_checkout" && f["path"] == serde_json::json!(outside)
        }),
        "{r}"
    );
    assert_eq!(count(&r, "targets_outside"), 1, "{r}");
}

#[test]
fn gc_on_a_worktree_leaves_main_and_every_other_lane_byte_for_byte_unchanged() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let lane = repo.worktree("lane");
    let other = repo.worktree("other");
    for wt in [&lane, &other] {
        cargo_json(&hive, wt, &["test", "--no-run"]);
        backdate_all(&wt.join("target"), THIRTY_DAYS);
        bump_app_version(wt);
        cargo_json(&hive, wt, &["test", "--no-run"]);
    }
    let main_before = tree(&repo.main.join("target"));
    let other_before = tree(&other.join("target"));
    let (out, r) = checkout_gc(&hive, &lane);
    assert!(out.status.success(), "{r}");
    assert!(count(&r, "units_deleted") >= 1, "{r}");
    assert_eq!(
        tree(&repo.main.join("target")),
        main_before,
        "main target changed: {r}"
    );
    assert_eq!(
        tree(&other.join("target")),
        other_before,
        "other lane target changed: {r}"
    );
}

#[test]
fn gc_on_main_leaves_the_worktrees_nested_under_it() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let nested = repo.nested_worktree("nested");
    cargo_json(&hive, &nested, &["test", "--no-run"]);
    backdate_all(&nested.join("target"), THIRTY_DAYS);
    bump_app_version(&nested);
    cargo_json(&hive, &nested, &["test", "--no-run"]);
    let nested_before = tree(&nested.join("target"));
    let (out, r) = checkout_gc(&hive, &repo.main);
    assert!(out.status.success(), "{r}");
    assert!(count(&r, "units_deleted") >= 1, "{r}");
    assert_eq!(
        tree(&nested.join("target")),
        nested_before,
        "nested worktree target changed: {r}"
    );
}

#[test]
fn an_unregistered_path_is_refused_naming_repo_add() {
    let hive = Hive::ready();
    let _repo = RegistryRepo::with_binaries(&hive);
    let empty = Scratch::new("unregistered-empty");
    let git_dir = Scratch::new("unregistered-git");
    git(&git_dir.path, &["init", "-q"]);
    for path in [&empty.path, &git_dir.path] {
        let (out, r) = checkout_gc(&hive, path);
        assert_eq!(out.status.code(), Some(1), "{path:?} => {r}");
        assert!(
            r["findings"].as_array().unwrap().iter().any(|f| {
                f["kind"] == "refusal"
                    && f["detail"]
                        .as_str()
                        .unwrap()
                        .contains("cargo hive repo add")
            }),
            "{path:?} => {r}"
        );
    }
}

#[test]
fn a_directory_inside_a_checkout_is_refused_naming_its_root() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let lane = repo.worktree("inside-lane");
    cargo_json(&hive, &lane, &["test", "--no-run"]);
    backdate_all(&lane.join("target"), THIRTY_DAYS);
    bump_app_version(&lane);
    cargo_json(&hive, &lane, &["test", "--no-run"]);
    let main_target_before = tree(&repo.main.join("target"));
    let lane_target_before = tree(&lane.join("target"));
    let cases = [
        (repo.main.join("app"), &repo.main),
        (lane.join("app"), &repo.main),
    ];
    for (path, main) in &cases {
        let path = path.canonicalize().unwrap();
        let expected = format!(
            "{path} is not a checkout of {main}; name the checkout's root directory",
            path = path.display(),
            main = main.display()
        );
        let (out, r) = checkout_gc(&hive, &path);
        assert_eq!(out.status.code(), Some(1), "{path:?} => {r}");
        assert!(
            r["findings"].as_array().unwrap().iter().any(|f| {
                f["kind"] == "refusal" && f["detail"].as_str() == Some(expected.as_str())
            }),
            "{path:?} => {r}"
        );
    }
    assert_eq!(
        tree(&repo.main.join("target")),
        main_target_before,
        "refused path deleted under main"
    );
    assert_eq!(
        tree(&lane.join("target")),
        lane_target_before,
        "refused path deleted under lane"
    );
}

#[test]
fn only_the_receipt_line_and_snapshot_change_under_the_state_root() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let store = hive.home.path.join("store/keep");
    fs::create_dir_all(&store).unwrap();
    fs::write(store.join("blob"), b"store-bytes").unwrap();
    fs::write(
        hive.home.path.join("store.db"),
        br#"{"layout":1,"units":[],"toolchains":{}}"#,
    )
    .unwrap();
    let root = &hive.home.path;
    let before = tree(root);
    let receipts_before = receipt_file_bytes(root);
    let (out, r) = checkout_gc(&hive, &repo.main);
    assert!(out.status.success(), "{r}");
    assert!(count(&r, "units_deleted") >= 1, "{r}");
    let after = tree(root);
    let snapshot = root.join("snapshot.json");
    let lock = root.join(".lock");
    let receipts = root.join("receipts");
    for (path, meta) in &after {
        if path == &snapshot || path == &lock || path.starts_with(&receipts) {
            continue;
        }
        assert_eq!(
            before.get(path),
            Some(meta),
            "{} changed under the state root: {r}",
            path.display()
        );
    }
    for path in before.keys() {
        if path == &lock {
            continue;
        }
        assert!(
            after.contains_key(path),
            "{} removed under the state root: {r}",
            path.display()
        );
    }
    assert!(
        after.contains_key(&snapshot),
        "snapshot.json missing after checkout gc: {r}"
    );
    let receipts_after = receipt_file_bytes(root);
    assert_eq!(
        receipts_after.keys().cloned().collect::<Vec<_>>(),
        receipts_before.keys().cloned().collect::<Vec<_>>(),
        "receipt day files changed set: before={receipts_before:?} after={receipts_after:?}"
    );
    let mut appended = 0u64;
    for (path, after_bytes) in &receipts_after {
        let before_bytes = receipts_before.get(path).cloned().unwrap_or_default();
        assert!(
            after_bytes.starts_with(&before_bytes),
            "{} was rewritten rather than appended",
            path.display()
        );
        let added = after_bytes.len() - before_bytes.len();
        if added == 0 {
            continue;
        }
        let new_text = std::str::from_utf8(&after_bytes[before_bytes.len()..]).unwrap();
        let new_lines: Vec<_> = new_text.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(
            new_lines.len(),
            1,
            "{} gained more than one receipt line: {new_text}",
            path.display()
        );
        assert!(
            new_lines[0].contains("\"command\":\"gc --checkout\""),
            "new receipt line is not checkout gc: {}",
            new_lines[0]
        );
        appended += 1;
    }
    assert_eq!(appended, 1, "expected exactly one receipts file to grow");
}

#[test]
fn a_stop_after_each_position_in_a_unit_leaves_no_unit_fresh_with_a_file_missing() {
    let steps = {
        let hive = Hive::ready();
        let repo = aged_binaries(&hive);
        let ordered = first_fingerprint_first_unit(&repo.main.join("target"));
        assert!(
            ordered.len() >= 2,
            "need a multi-entry fingerprint-first unit"
        );
        ordered.len()
    };
    for stop_after in 0..steps {
        let hive = Hive::ready();
        let repo = aged_binaries(&hive);
        let target = repo.main.join("target");
        let ordered = first_fingerprint_first_unit(&target);
        assert!(
            ordered.len() >= 2,
            "need two ordered entries for stop {stop_after}, got {ordered:?}"
        );
        let fingerprint = ordered
            .iter()
            .find(|p| is_fingerprint_entry(p))
            .cloned()
            .expect("fingerprint dir in the ordered unit");
        let siblings: Vec<_> = ordered
            .iter()
            .filter(|p| *p != &fingerprint)
            .cloned()
            .collect();
        let rec = Recording::over_real().fail_removes_after(stop_after);
        let fs = rec.fs();
        let (scan, checkout, nested) = scan_only(&hive, &repo);
        let input = cargo_hive_gc::checkout::CheckoutInput {
            fs: fs.as_ref(),
            scan: &scan,
            checkout: &checkout,
            nested: &nested,
            now: SystemTime::now(),
            window: Duration::from_secs(7 * 86_400),
            account_bytes: true,
        };
        let report = cargo_hive_gc::checkout::run(&input, &|| false);
        assert!(report.failed, "stop {stop_after}: {report:?}");
        match layout_of(&target) {
            // Flat: the fingerprint dir goes first, so while it stands every sibling stands.
            UnitLayout::Flat => {
                if fingerprint.exists() {
                    for sibling in &siblings {
                        assert!(
                            sibling.exists(),
                            "fingerprint {} outlived deleted sibling {} (stop {stop_after})",
                            fingerprint.display(),
                            sibling.display()
                        );
                    }
                }
            }
            // Unit dir (BUILDDIR-3.3): out/ and run/ go before fingerprint/, so once the
            // fingerprint dir is gone, every sibling inside the unit dir is gone too.
            UnitLayout::UnitDir => {
                if !fingerprint.exists() {
                    for sibling in siblings.iter().filter(|p| !fingerprint.starts_with(p)) {
                        assert!(
                            !sibling.exists(),
                            "fingerprint {} is gone but sibling {} remains (stop {stop_after})",
                            fingerprint.display(),
                            sibling.display()
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn a_dsym_dir_of_a_dead_unit_is_deleted() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let target = repo.main.join("target");
    let (dead, _) = oldest_fingerprint_unit(&target);
    let hash = dead.rsplit('-').next().unwrap();
    let plant = orphan_object_dir(&target.join("debug"));
    let dsym = plant.join(format!("unit-{hash}.dSYM"));
    fs::create_dir_all(dsym.join("Contents")).unwrap();
    fs::write(dsym.join("Contents/Info.plist"), b"x").unwrap();
    let (fs_box, scan, checkout, nested) = library_scan(&hive, &repo);
    let input = cargo_hive_gc::checkout::CheckoutInput {
        fs: fs_box.as_ref(),
        scan: &scan,
        checkout: &checkout,
        nested: &nested,
        now: SystemTime::now(),
        window: Duration::from_secs(7 * 86_400),
        account_bytes: true,
    };
    let report = cargo_hive_gc::checkout::run(&input, &|| false);
    assert!(!report.failed, "{report:?}");
    assert!(!dsym.exists(), "dSYM of a dead unit remained: {report:?}");
}

#[test]
fn an_entry_swapped_after_its_stat_is_refused() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let target = repo.main.join("target");
    let (_, dead) = decide(&target);
    let entry = dead
        .iter()
        .flat_map(|unit| unit.files.iter())
        .find(|path| {
            let parent_name = path
                .parent()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned());
            matches!(parent_name.as_deref(), Some("deps") | Some("out")) && path.is_file()
        })
        .cloned()
        .expect("a dead unit deps file");
    let outside = repo.scratch.path.join("outside-file");
    fs::write(&outside, b"nope").unwrap();
    let rec = Recording::over_real().symlink_after_stat(&entry, &outside);
    let fs = rec.fs();
    let (scan, checkout, nested) = scan_only(&hive, &repo);
    let input = cargo_hive_gc::checkout::CheckoutInput {
        fs: fs.as_ref(),
        scan: &scan,
        checkout: &checkout,
        nested: &nested,
        now: SystemTime::now(),
        window: Duration::from_secs(7 * 86_400),
        account_bytes: true,
    };
    let report = cargo_hive_gc::checkout::run(&input, &|| false);
    assert!(report.failed, "{report:?}");
    assert!(
        report.findings.iter().any(|f| f.kind == "gc.delete_failed"),
        "{:?}",
        report.findings
    );
}

#[test]
fn an_object_swapped_after_its_stat_is_refused() {
    let hive = Hive::ready();
    let repo = RegistryRepo::with_binaries(&hive);
    cargo_json(&hive, &repo.main, &["test", "--no-run"]);
    let debug = repo.main.join("target/debug");
    let orphan = orphan_object_dir(&debug).join("orphan-swappedobject1.0.rcgu.o");
    fs::write(&orphan, b"orphan").unwrap();
    let outside = repo.scratch.path.join("outside-object");
    fs::write(&outside, b"nope").unwrap();
    let (_fs, scan, checkout, nested) = library_scan(&hive, &repo);
    let rec = Recording::over_real().symlink_after_stat(&orphan, &outside);
    let report = run_library(&rec, &scan, &checkout, &nested);
    assert!(
        orphan.symlink_metadata().is_ok(),
        "swapped object was deleted: {report:?}"
    );
    assert!(report.failed, "{report:?}");
    assert!(
        report
            .findings
            .iter()
            .any(|f| { f.kind == "gc.delete_failed" && f.path.as_ref() == Some(&orphan) }),
        "{:?}",
        report.findings
    );
}

#[test]
fn stamp_for_a_run_build_script_uses_the_kind_specific_path() {
    for layout in [UnitLayout::UnitDir, UnitLayout::Flat] {
        let scratch = Scratch::new("stamp-rbs");
        let target = scratch.path.join("target");
        let profile = target.join("debug");
        fs::create_dir_all(&profile).unwrap();
        let hex = "0123456789abcdef";
        let loc = common::unit_dirs::plant_unit(
            &profile,
            layout,
            "pkg",
            hex,
            "run-build-script",
            u64::from_str_radix(hex, 16).unwrap(),
            &[],
            &[],
        );
        let want = unit_path::stamp(
            &profile,
            &loc,
            cargo_hive_core::fingerprint::UnitKind::RunBuildScript,
        );
        assert!(want.is_file(), "{} ({layout:?})", want.display());
        assert_eq!(stamp_for(&target, &loc.name_meta), want, "{layout:?}");
    }
}

#[test]
fn completed_checkout_gc_proves_each_role_of_a_coincident_root() {
    let hive = hive_for_checkout_evidence();
    let repo = aged_binaries(&hive);
    let (fs, scan, checkout, nested) = library_scan(&hive, &repo);
    let target = checkout.join("target");
    let required = provided_roots(fs.as_ref(), &target, &target);
    let (report, completed) =
        checkout_with_evidence(fs.as_ref(), &scan, &checkout, &nested, &required, &|| false);

    assert_eq!(completed, required, "{report:?}");
    assert!(report.totals.units_deleted > 0, "{report:?}");
    assert_eq!(report.per_target.len(), 1, "{report:?}");
}

#[test]
fn checkout_gc_requires_a_distinct_role_and_existing_target_row() {
    let hive = hive_for_checkout_evidence();
    let repo = aged_binaries(&hive);
    let (fs, mut scan, checkout, nested) = library_scan(&hive, &repo);
    let target = checkout.join("target");
    let required = provided_roots(fs.as_ref(), &target, &target);
    let before = tree(&target);

    let (report, completed) = checkout_with_evidence(
        fs.as_ref(),
        &scan,
        &checkout,
        &nested,
        &required[..1],
        &|| false,
    );
    assert!(completed.is_empty(), "{report:?}");
    assert_eq!(tree(&target), before, "{report:?}");

    scan.targets.clear();
    let (report, completed) =
        checkout_with_evidence(fs.as_ref(), &scan, &checkout, &nested, &required, &|| false);
    assert!(completed.is_empty(), "{report:?}");
    assert!(!report.findings.is_empty(), "{report:?}");
    assert_eq!(tree(&target), before);
}

#[test]
fn checkout_gc_binds_split_roots_and_preserves_an_earlier_effect_on_a_later_skip() {
    let hive = hive_for_checkout_evidence();
    let repo = aged_binaries(&hive);
    let (fs, mut scan, checkout, nested) = library_scan(&hive, &repo);
    let target = checkout.join("target");
    let build = checkout.join("z-build");
    assert!(
        Command::new("cp")
            .args(["-c", "-R"])
            .arg(&target)
            .arg(&build)
            .status()
            .unwrap()
            .success()
    );
    let mut build_row = scan
        .targets
        .iter()
        .find(|row| row.path == target)
        .unwrap()
        .clone();
    build_row.path = build.clone();
    scan.targets.push(build_row);
    scan.workspaces
        .iter_mut()
        .find(|workspace| workspace.checkout == checkout)
        .unwrap()
        .result
        .as_mut()
        .unwrap()
        .build_directory = build.clone();
    let required = provided_roots(fs.as_ref(), &target, &build);
    let before = tree(&build);
    let mut holder = hive.hold_lock_child("flock", &build.join("debug/.cargo-build-lock"));

    let (report, completed) =
        checkout_with_evidence(fs.as_ref(), &scan, &checkout, &nested, &required, &|| false);
    let _ = holder.kill();
    let _ = holder.wait();

    assert_eq!(completed, required[..1], "{report:?}");
    assert_eq!(report.per_target.len(), 2, "{report:?}");
    assert!(report.totals.units_deleted > 0, "{report:?}");
    assert_eq!(
        report.totals.skips.get("targets_skipped_held"),
        Some(&1),
        "{report:?}"
    );
    assert_eq!(tree(&build), before);
    for root in [&target, &build] {
        for name in [".cargo-build-lock", ".cargo-lock", ".cargo-artifact-lock"] {
            let lock = root.join("debug").join(name);
            assert!(
                common::wait_until_lock_free(&lock, Duration::from_secs(5)),
                "the fixture lock did not become available: {}",
                lock.display()
            );
        }
    }

    let (report, completed) =
        checkout_with_evidence(fs.as_ref(), &scan, &checkout, &nested, &required, &|| false);
    assert_eq!(completed, required, "{report:?}");
    assert!(report.totals.units_deleted > 0, "{report:?}");
}

#[test]
fn checkout_gc_only_counts_an_absent_root_when_initial_absence_was_proved() {
    let hive = hive_for_checkout_evidence();
    let repo = RegistryRepo::new(&hive);
    let (fs, scan, checkout, nested) = library_scan(&hive, &repo);
    let target = checkout.join("target");
    let required = [RootRole::Target, RootRole::Build].map(|role| RequiredRoot {
        path: target.clone(),
        role,
        presence: RootPresence::InitiallyAbsent,
    });
    let (report, completed) =
        checkout_with_evidence(fs.as_ref(), &scan, &checkout, &nested, &required, &|| false);
    assert_eq!(completed, required, "{report:?}");
    assert!(report.per_target.is_empty(), "{report:?}");
    assert!(!target.exists());

    fs::create_dir(&target).unwrap();
    let (report, completed) =
        checkout_with_evidence(fs.as_ref(), &scan, &checkout, &nested, &required, &|| false);
    assert!(completed.is_empty(), "{report:?}");
    assert!(target.is_dir());
}

#[test]
fn checkout_gc_cannot_prove_a_cancelled_root() {
    let hive = hive_for_checkout_evidence();
    let repo = aged_binaries(&hive);
    let (fs, scan, checkout, nested) = library_scan(&hive, &repo);
    let target = checkout.join("target");
    let required = provided_roots(fs.as_ref(), &target, &target);
    let before = tree(&target);
    let (report, completed) =
        checkout_with_evidence(fs.as_ref(), &scan, &checkout, &nested, &required, &|| true);
    assert!(completed.is_empty(), "{report:?}");
    assert!(report.cancelled, "{report:?}");
    assert_eq!(tree(&target), before);
}

#[test]
fn checkout_gc_refuses_shared_outside_nested_and_unreadable_scope_without_effects() {
    use cargo_hive_cargo::discover::{WalkFault, WorkspaceResolution};

    let hive = hive_for_checkout_evidence();
    let repo = aged_binaries(&hive);
    let (fs, mut scan, checkout, nested) = library_scan(&hive, &repo);
    let target = checkout.join("target");
    let required = provided_roots(fs.as_ref(), &target, &target);
    let before = tree(&target);
    let no_cleanup = |scan: &_, nested: &[PathBuf], required: &[RequiredRoot]| {
        let (report, completed) =
            checkout_with_evidence(fs.as_ref(), scan, &checkout, nested, required, &|| false);
        assert!(completed.is_empty(), "{report:?}");
        assert!(!report.findings.is_empty(), "{report:?}");
    };

    no_cleanup(&scan, &[target.join("nested-checkout")], &required);
    scan.walk_faults.push(WalkFault {
        path: checkout.join("unreadable"),
        detail: "test discovery read error".into(),
        cannot_enter: false,
    });
    no_cleanup(&scan, &nested, &required);
    scan.walk_faults.clear();
    let owner = scan
        .workspaces
        .iter()
        .find(|resolution| resolution.checkout == checkout)
        .unwrap();
    scan.workspaces.push(WorkspaceResolution {
        dir: repo.scratch.path.join("other"),
        checkout: repo.scratch.path.join("other"),
        result: owner.result.clone(),
    });
    no_cleanup(&scan, &nested, &required);
    scan.workspaces.pop();
    assert_eq!(tree(&target), before);

    let outside = repo.scratch.path.join("outside");
    fs::rename(&target, &outside).unwrap();
    let before = tree(&outside);
    let owner = scan
        .workspaces
        .iter_mut()
        .find(|resolution| resolution.checkout == checkout)
        .unwrap();
    let resolved = owner.result.as_mut().unwrap();
    resolved.target_directory = outside.clone();
    resolved.build_directory = outside.clone();
    scan.targets
        .iter_mut()
        .find(|row| row.path == target)
        .unwrap()
        .path = outside.clone();
    let required = provided_roots(fs.as_ref(), &outside, &outside);
    no_cleanup(&scan, &nested, &required);
    assert_eq!(tree(&outside), before);
}

#[test]
fn checkout_gc_rechecks_the_root_inode_after_cargo_exclusion() {
    checkout_gc_refuses_root_exchange(RootExchange::BeforeLockedStat);
}

#[test]
fn checkout_gc_does_not_delete_after_a_root_exchange_following_the_first_locked_stat() {
    checkout_gc_refuses_root_exchange(RootExchange::AfterLockedStat);
}

#[test]
fn checkout_gc_rechecks_identity_after_accounting_and_before_removing() {
    checkout_gc_refuses_root_exchange(RootExchange::DuringAccounting);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RootExchange {
    BeforeLockedStat,
    AfterLockedStat,
    DuringAccounting,
}

fn checkout_gc_refuses_root_exchange(timing: RootExchange) {
    use cargo_hive_platform::testing::Hooked;
    use cargo_hive_platform::{Fs, TryLock};
    use std::sync::atomic::{AtomicBool, Ordering};

    let hive = hive_for_checkout_evidence();
    let repo = aged_binaries(&hive);
    let (fs, scan, checkout, nested) = library_scan(&hive, &repo);
    let target = checkout.join("target");
    let original = repo.scratch.path.join("original");
    let stash = original.clone();
    let original_before: std::collections::BTreeMap<_, _> = tree(&target)
        .into_iter()
        .map(|(path, evidence)| (original.join(path.strip_prefix(&target).unwrap()), evidence))
        .collect();
    let replacement = repo.scratch.path.join("replacement");
    assert!(
        Command::new("cp")
            .args(["-c", "-R"])
            .arg(&target)
            .arg(&replacement)
            .status()
            .unwrap()
            .success()
    );
    let before = tree(&replacement)
        .into_iter()
        .map(|(path, evidence)| {
            (
                target.join(path.strip_prefix(&replacement).unwrap()),
                evidence,
            )
        })
        .collect();
    let required = provided_roots(fs.as_ref(), &target, &target);
    let real = cargo_hive_platform::apfs::Apfs::new(std::env::current_exe().unwrap());
    let selected = target.clone();
    let swapped = Arc::new(AtomicBool::new(false));
    let did_swap = swapped.clone();
    let exchange = Arc::new(move |root: &Path| {
        if !did_swap.swap(true, Ordering::Relaxed) {
            fs::rename(root, &stash).unwrap();
            fs::rename(&replacement, root).unwrap();
        }
    });
    let exchange_during_accounting = exchange.clone();
    let accounting_root = target.clone();
    let sharing_fs = cargo_hive_platform::apfs::Apfs::new(std::env::current_exe().unwrap());
    let hooked = Hooked {
        inner: fs,
        compress: None,
        volume: None,
        sharing: Some(Box::new(move |path| {
            let result = sharing_fs.sharing(path);
            if timing == RootExchange::DuringAccounting && path.starts_with(&accounting_root) {
                exchange_during_accounting(&accounting_root);
            }
            result
        })),
        entry_stat: Some(Box::new(move |path| {
            if timing != RootExchange::DuringAccounting
                && path == selected
                && matches!(
                    real.try_lock_exclusive(&path.join("debug/.cargo-build-lock")),
                    TryLock::Held
                )
            {
                let stat_before = real.entry_stat(path);
                exchange(path);
                if timing == RootExchange::AfterLockedStat {
                    return stat_before;
                }
            }
            real.entry_stat(path)
        })),
    };
    let (report, completed) =
        checkout_with_evidence(&hooked, &scan, &checkout, &nested, &required, &|| false);
    assert!(
        swapped.load(Ordering::Relaxed),
        "swap must occur while the Cargo lock is held"
    );
    assert!(completed.is_empty(), "{report:?}");
    assert!(report.failed, "{report:?}");
    assert!(
        tree(&target) == before,
        "replacement changed during cleanup: {report:?}"
    );
    assert!(
        tree(&original) == original_before,
        "original changed during cleanup: {report:?}"
    );
}

#[test]
fn checkout_gc_cannot_clean_a_replacement_profile_under_displaced_locks() {
    use cargo_hive_platform::testing::Hooked;
    use cargo_hive_platform::{Fs, TryLock};
    use std::sync::atomic::{AtomicBool, Ordering};

    let hive = hive_for_checkout_evidence();
    let repo = aged_binaries(&hive);
    let (fs, scan, checkout, nested) = library_scan(&hive, &repo);
    let target = checkout.join("target");
    let debug = target.join("debug");
    let original = repo.scratch.path.join("original-profile");
    let replacement = repo.scratch.path.join("replacement-profile");
    assert!(
        Command::new("cp")
            .args(["-c", "-R"])
            .arg(&debug)
            .arg(&replacement)
            .status()
            .unwrap()
            .success()
    );
    let mut holder = hive.hold_lock_child("flock", &replacement.join(".cargo-build-lock"));
    let before: std::collections::BTreeMap<_, _> = tree(&replacement)
        .into_iter()
        .map(|(path, entry)| (debug.join(path.strip_prefix(&replacement).unwrap()), entry))
        .collect();
    let required = provided_roots(fs.as_ref(), &target, &target);
    let selected = target.clone();
    let did_swap = Arc::new(AtomicBool::new(false));
    let swapped = did_swap.clone();
    let real = cargo_hive_platform::apfs::Apfs::new(std::env::current_exe().unwrap());
    let hooked = Hooked {
        inner: fs,
        compress: None,
        volume: None,
        sharing: None,
        entry_stat: Some(Box::new(move |path| {
            if path == selected
                && !swapped.load(Ordering::Relaxed)
                && matches!(
                    real.try_lock_exclusive(&debug.join(".cargo-build-lock")),
                    TryLock::Held
                )
            {
                fs::rename(&debug, &original).unwrap();
                fs::rename(&replacement, &debug).unwrap();
                swapped.store(true, Ordering::Relaxed);
            }
            real.entry_stat(path)
        })),
    };
    let (report, completed) =
        checkout_with_evidence(&hooked, &scan, &checkout, &nested, &required, &|| false);
    let _ = holder.kill();
    let _ = holder.wait();
    assert!(
        did_swap.load(Ordering::Relaxed),
        "exchange must occur after the Cargo lock is acquired"
    );
    assert!(completed.is_empty(), "{report:?}");
    assert!(
        tree(&target.join("debug")) == before,
        "replacement changed: {report:?}"
    );
}

#[test]
fn checkout_gc_refuses_an_oversized_tag_before_removing_files() {
    let hive = hive_for_checkout_evidence();
    let repo = aged_binaries(&hive);
    let (fs, scan, checkout, nested) = library_scan(&hive, &repo);
    let target = checkout.join("target");
    let required = provided_roots(fs.as_ref(), &target, &target);
    let mut tag = fs::read(target.join("CACHEDIR.TAG")).unwrap();
    tag.resize(64 * 1024 + 1, b' ');
    fs::write(target.join("CACHEDIR.TAG"), tag).unwrap();
    let before = tree(&target);
    let (report, completed) =
        checkout_with_evidence(fs.as_ref(), &scan, &checkout, &nested, &required, &|| false);
    assert!(completed.is_empty(), "{report:?}");
    assert!(tree(&target) == before, "files changed: {report:?}");
}

#[test]
fn checkout_gc_refuses_symbolic_links_in_fingerprint_inputs() {
    for use_json in [false, true] {
        let hive = hive_for_checkout_evidence();
        let repo = aged_binaries(&hive);
        let (fs, scan, checkout, nested) = library_scan(&hive, &repo);
        let target = checkout.join("target");
        let required = provided_roots(fs.as_ref(), &target, &target);
        let dir = fingerprint_dir_for(&target, &newest_fingerprint_unit(&target));
        let json = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| path.extension().is_some_and(|ext| ext == "json"))
            .unwrap();
        let input = if use_json {
            json
        } else {
            dir.join(json.file_stem().unwrap())
        };
        let original = repo.scratch.path.join("original-fingerprint-input");
        fs::rename(&input, &original).unwrap();
        std::os::unix::fs::symlink(&original, &input).unwrap();
        let before = tree(&target);
        let (report, completed) =
            checkout_with_evidence(fs.as_ref(), &scan, &checkout, &nested, &required, &|| false);
        assert!(completed.is_empty(), "{report:?}");
        assert!(
            tree(&target) == before,
            "files changed for {}: {report:?}",
            input.display()
        );
    }
}

#[test]
fn checkout_gc_retains_measured_file_effects_when_a_later_removal_fails() {
    let hive = hive_for_checkout_evidence();
    let repo = aged_binaries(&hive);
    let (fs, scan, checkout, nested) = library_scan(&hive, &repo);
    let target = checkout.join("target");
    let required = provided_roots(fs.as_ref(), &target, &target);
    let before = tree(&target);
    let rec = Recording::over_real().fail_removes_after(1);
    let (report, completed) =
        checkout_with_evidence(&rec, &scan, &checkout, &nested, &required, &|| false);
    assert!(completed.is_empty(), "{report:?}");
    assert!(report.failed, "{report:?}");
    assert!(
        tree(&target).len() < before.len(),
        "fixture removed no files"
    );
    assert!(report.totals.unit_files_deleted > 0, "{report:?}");
}

#[test]
fn checkout_gc_cannot_prove_either_kind_of_undecided_traversal() {
    let hive = hive_for_checkout_evidence();
    let repo = RegistryRepo::with_binaries(&hive);
    cargo_json(&hive, &repo.main, &["test", "--no-run"]);
    let (fs, scan, checkout, nested) = library_scan(&hive, &repo);
    let target = checkout.join("target");
    let debug = target.join("debug");
    let required = provided_roots(fs.as_ref(), &target, &target);
    let bin = find_linked_named(&debug, "app-").expect("a built app binary");
    let rec = Recording::over_real().fail_read_at(bin);
    let before = tree(&target);
    let (report, completed) =
        checkout_with_evidence(&rec, &scan, &checkout, &nested, &required, &|| false);
    assert!(completed.is_empty(), "{report:?}");
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.kind == "gc.objects_undecided"),
        "{report:?}"
    );
    assert_eq!(tree(&target), before);

    let dir = fingerprint_dir_for(&target, &newest_fingerprint_unit(&target));
    fs::write(dir.join("extra-corrupt.json"), b"{}").unwrap();
    let before = tree(&target);
    let (report, completed) =
        checkout_with_evidence(fs.as_ref(), &scan, &checkout, &nested, &required, &|| false);
    assert!(completed.is_empty(), "{report:?}");
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.kind == "gc.undecided"),
        "{report:?}"
    );
    assert_eq!(tree(&target), before);
}

fn hive_for_checkout_evidence() -> Hive {
    let hive = Hive::ready();
    fs::create_dir(hive.home.path.join("no-cache")).unwrap();
    hive
}

#[test]
fn checkout_gc_requires_an_uncached_scan_before_any_deletion() {
    let hive = hive_for_checkout_evidence();
    let repo = aged_binaries(&hive);
    let (fs, mut scan, checkout, nested) = library_scan(&hive, &repo);
    let target = checkout.join("target");
    let required = provided_roots(fs.as_ref(), &target, &target);
    let before = tree(&target);
    scan.metadata_cache_hits = 1;
    let (report, completed) =
        checkout_with_evidence(fs.as_ref(), &scan, &checkout, &nested, &required, &|| false);
    assert!(completed.is_empty(), "{report:?}");
    assert_eq!(tree(&target), before);
}

fn provided_roots(
    fs: &dyn cargo_hive_platform::Fs,
    target: &Path,
    build: &Path,
) -> [RequiredRoot; 2] {
    [(target, RootRole::Target), (build, RootRole::Build)].map(|(path, role)| {
        let stat = fs.entry_stat(path).unwrap();
        RequiredRoot {
            path: path.to_path_buf(),
            role,
            presence: RootPresence::Present {
                dev: stat.dev,
                ino: stat.ino,
            },
        }
    })
}

fn checkout_with_evidence(
    fs: &dyn cargo_hive_platform::Fs,
    scan: &cargo_hive_cargo::discover::TargetScan,
    checkout: &Path,
    nested: &[PathBuf],
    required: &[RequiredRoot],
    cancel: &dyn Fn() -> bool,
) -> (cargo_hive_gc::checkout::CheckoutReport, Vec<RequiredRoot>) {
    let input = cargo_hive_gc::checkout::CheckoutInput {
        fs,
        scan,
        checkout,
        nested,
        now: SystemTime::now(),
        window: WINDOW,
        account_bytes: true,
    };
    let evidence = cargo_hive_gc::checkout::run_with_evidence(&input, required, cancel);
    assert_eq!(evidence.expected(), required);
    let completed = evidence.completed().to_vec();
    (evidence.report, completed)
}

#[test]
fn a_unit_changed_between_scan_and_lock_is_judged_on_the_fresh_read() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let target = repo.main.join("target");
    let (dead, stamp) = oldest_fingerprint_unit(&target);
    let (fs, scan, checkout, nested) = library_scan(&hive, &repo);
    let before_dead = decide(&target).1.len();
    assert!(before_dead >= 1, "need a dead unit before the revive");
    set_mtime(&stamp, SystemTime::now());
    let input = cargo_hive_gc::checkout::CheckoutInput {
        fs: fs.as_ref(),
        scan: &scan,
        checkout: &checkout,
        nested: &nested,
        now: SystemTime::now(),
        window: Duration::from_secs(7 * 86_400),
        account_bytes: true,
    };
    let report = cargo_hive_gc::checkout::run(&input, &|| false);
    let dir = fingerprint_dir_for(&target, &dead);
    assert!(
        dir.exists(),
        "a unit revived before the fresh read was deleted: {report:?}"
    );
    assert!(
        report.totals.units_deleted >= 1,
        "other dead units must still be collected on the fresh read: {report:?}"
    );
}

#[test]
fn a_held_lock_skips_the_target_names_it_and_deletes_nothing() {
    for name in [".cargo-build-lock", ".cargo-lock", ".cargo-artifact-lock"] {
        let hive = Hive::ready();
        let repo = aged_binaries(&hive);
        let target = repo.main.join("target").canonicalize().unwrap();
        let lock = target.join("debug").join(name);
        assert!(lock.is_file(), "{name} missing in the fixture");
        let before = tree(&target);
        let mut holder = hive.hold_lock_child("flock", &lock);
        let (out, r) = checkout_gc(&hive, &repo.main);
        let _ = holder.kill();
        let _ = holder.wait();
        assert_eq!(out.status.code(), Some(0), "{name}: {r}");
        assert_eq!(count(&r, "targets_skipped_held"), 1, "{name}: {r}");
        assert!(
            r["findings"].as_array().unwrap().iter().any(|f| {
                f["kind"] == "gc.skipped"
                    && f["path"] == serde_json::json!(target)
                    && f["detail"] == "a Cargo lock is held"
            }),
            "{name}: {r}"
        );
        assert_eq!(
            tree(&target),
            before,
            "{name} deleted under the held lock: {r}"
        );
    }
}

#[test]
fn a_profile_with_fingerprints_and_no_lock_file_is_skipped_as_missing() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let target = repo.main.join("target").canonicalize().unwrap();
    let before = fingerprint_dirs(&target);
    for name in [".cargo-build-lock", ".cargo-lock", ".cargo-artifact-lock"] {
        let _ = fs::remove_file(target.join("debug").join(name));
    }
    let (out, r) = checkout_gc(&hive, &repo.main);
    assert_eq!(out.status.code(), Some(0), "{r}");
    assert_eq!(count(&r, "targets_skipped_lock_missing"), 1, "{r}");
    assert_eq!(count(&r, "units_deleted"), 0, "{r}");
    assert!(
        r["findings"].as_array().unwrap().iter().any(|f| {
            f["kind"] == "gc.skipped"
                && f["path"] == serde_json::json!(target)
                && f["detail"] == "a required Cargo lock is missing"
        }),
        "{r}"
    );
    assert_eq!(
        fingerprint_dirs(&target),
        before,
        "missing-lock skip deleted fingerprint dirs: {r}"
    );
}

#[test]
fn an_unopenable_or_unlockable_lock_is_skipped_and_named() {
    {
        let hive = Hive::ready();
        let repo = aged_binaries(&hive);
        let target = repo.main.join("target").canonicalize().unwrap();
        let lock = target.join("debug/.cargo-build-lock");
        let before = tree(&target);
        let rec = Recording::over_real().unlockable(&lock);
        let fs = rec.fs();
        let (scan, checkout, nested) = scan_only(&hive, &repo);
        let report = run_library(fs.as_ref(), &scan, &checkout, &nested);
        assert!(
            !report.failed,
            "unlockable should skip, not fail: {report:?}"
        );
        assert_eq!(
            report
                .totals
                .skips
                .get("targets_skipped_lock_unsupported")
                .copied(),
            Some(1),
            "{report:?}"
        );
        assert!(
            report.findings.iter().any(|f| {
                f.kind == "gc.skipped"
                    && f.path.as_deref() == Some(target.as_path())
                    && f.detail == "a Cargo lock could not be locked"
            }),
            "{:?}",
            report.findings
        );
        assert_eq!(tree(&target), before);
    }
    {
        let hive = Hive::ready();
        let repo = aged_binaries(&hive);
        let target = repo.main.join("target").canonicalize().unwrap();
        let lock = target.join("debug/.cargo-build-lock");
        fs::remove_file(&lock).unwrap();
        std::os::unix::fs::symlink("/etc/hosts", &lock).unwrap();
        let before = tree(&target);
        let rec = Recording::over_real();
        let fs = rec.fs();
        let (scan, checkout, nested) = scan_only(&hive, &repo);
        let report = run_library(fs.as_ref(), &scan, &checkout, &nested);
        assert!(
            !report.failed,
            "unopenable should skip, not fail: {report:?}"
        );
        assert_eq!(
            report
                .totals
                .skips
                .get("targets_skipped_lock_unopenable")
                .copied(),
            Some(1),
            "{report:?}"
        );
        assert!(
            report.findings.iter().any(|f| {
                f.kind == "gc.skipped"
                    && f.path.as_deref() == Some(target.as_path())
                    && f.detail == "a Cargo lock could not be opened"
            }),
            "{:?}",
            report.findings
        );
        assert_eq!(tree(&target), before);
    }
}

#[test]
fn a_profile_appearing_while_locking_is_skipped_as_profile_set() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let target = repo.main.join("target").canonicalize().unwrap();
    let before = tree(&target);
    let planted = target.join("release/.fingerprint");
    let rec = Recording::over_real().create_dir_after_lock(&planted);
    let fs = rec.fs();
    let (scan, checkout, nested) = scan_only(&hive, &repo);
    let report = run_library(fs.as_ref(), &scan, &checkout, &nested);
    assert_eq!(
        report
            .totals
            .skips
            .get("targets_skipped_profile_set")
            .copied(),
        Some(1),
        "{report:?}"
    );
    assert!(
        report.findings.iter().any(|f| {
            f.kind == "gc.skipped"
                && f.path.as_deref() == Some(target.as_path())
                && f.detail == "the profile set changed while locking"
        }),
        "{:?}",
        report.findings
    );
    let mut after = tree(&target);
    after.retain(|path, _| !path.starts_with(target.join("release")));
    assert_eq!(after, before, "profile-set skip deleted under the target");
}

#[test]
fn a_target_whose_profile_listing_fails_while_locking_exits_1() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let target = repo.main.join("target").canonicalize().unwrap();
    let profile = target.join("debug");
    let before = tree(&target);
    fs::set_permissions(&profile, fs::Permissions::from_mode(0o000)).unwrap();
    let (out, r) = checkout_gc(&hive, &repo.main);
    let _ = fs::set_permissions(&profile, fs::Permissions::from_mode(0o755));
    assert_eq!(out.status.code(), Some(1), "{r}");
    assert!(
        r["findings"].as_array().unwrap().iter().any(|f| {
            f["kind"] == "gc.target_failed"
                && f["path"] == serde_json::json!(target)
                && f["detail"].as_str().is_some_and(|d| !d.is_empty())
        }),
        "{r}"
    );
    assert_eq!(count(&r, "units_deleted"), 0, "{r}");
    assert_eq!(
        tree(&target),
        before,
        "failed listing mutated the target: {r}"
    );
}

#[test]
fn every_lock_is_taken_before_the_first_read_of_the_target() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let target = repo.main.join("target").canonicalize().unwrap();
    let rec = Recording::over_real();
    let fs = rec.fs();
    let (scan, checkout, nested) = scan_only(&hive, &repo);
    let report = run_library(fs.as_ref(), &scan, &checkout, &nested);
    assert!(!report.failed, "{report:?}");
    assert!(report.totals.units_deleted >= 1, "{report:?}");
    let log = rec.log();
    let last_lock = log
        .iter()
        .rposition(|line| line.starts_with("lock "))
        .expect("lock lines");
    let first_deps_or_read_at = log.iter().position(|line| {
        line.starts_with("read_at ") || (line.starts_with("read ") && line.contains("/deps"))
    });
    let Some(first) = first_deps_or_read_at else {
        panic!("expected a deps listing or read_at in the log: {log:?}");
    };
    assert!(
        last_lock < first,
        "a deps/read_at happened before the last lock: last_lock={last_lock} first={first} log={log:?}"
    );
    let _ = target;
}

#[test]
fn the_locks_stay_held_until_the_last_deletion() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let target = repo.main.join("target").canonicalize().unwrap();
    let lock = target.join("debug/.cargo-build-lock");
    let last_file = last_deleted_file(&target);
    let seen = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&seen);
    let lock_for_hook = lock.clone();
    let rec = Recording::over_real().on_remove(last_file.clone(), move || {
        let out = Command::new(env!("CARGO_BIN_EXE_cargo-hive"))
            .args(["__try-lock", "flock", lock_for_hook.to_str().unwrap()])
            .output()
            .expect("spawn __try-lock");
        *slot.lock().unwrap() = Some(String::from_utf8_lossy(&out.stdout).trim().to_string());
    });
    let fs = rec.fs();
    let (scan, checkout, nested) = scan_only(&hive, &repo);
    let report = run_library(fs.as_ref(), &scan, &checkout, &nested);
    assert!(!report.failed, "{report:?}");
    assert!(report.totals.units_deleted >= 1, "{report:?}");
    assert!(
        !last_file.exists(),
        "hooked path was not deleted: {}",
        last_file.display()
    );
    let answer = seen.lock().unwrap().clone();
    assert_eq!(
        answer.as_deref(),
        Some("refused"),
        "lock was free at the last file deletion: {answer:?}"
    );
}

#[test]
fn unreferenced_objects_are_deleted_and_every_object_a_binary_names_is_kept() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let target = repo.main.join("target");
    let debug = target.join("debug");
    let fs = cargo_hive_platform::current().unwrap().fs;
    let doomed: std::collections::BTreeSet<_> = match decide_at(&target, SystemTime::now(), WINDOW)
    {
        Liveness::Decided { dead, .. } => dead
            .iter()
            .flat_map(|unit| unit.files.iter().cloned())
            .collect(),
        Liveness::Undecided { reasons } => panic!("liveness was undecided: {reasons:#?}"),
    };
    // A dead unit-dir unit goes whole, so every object inside it goes with it.
    let doomed_path = |path: &Path| doomed.iter().any(|gone| path.starts_with(gone));
    let listing =
        cargo_hive_gc::checkout::list_profile(fs.as_ref(), &debug, 1_000_000, 100_000).unwrap();
    let mut kept_names = std::collections::BTreeSet::new();
    for path in &listing.candidates {
        if doomed_path(path) {
            continue;
        }
        if let Ok(Some(map)) = cargo_hive_gc::debug_map::read(fs.as_ref(), path) {
            kept_names.extend(map.loose);
        }
    }
    assert!(
        !kept_names.is_empty(),
        "expected live binaries to name loose objects"
    );
    let before_named: Vec<_> = object_paths_named(&debug, &kept_names)
        .into_iter()
        .filter(|path| !doomed_path(path))
        .collect();
    assert!(!before_named.is_empty(), "named objects must exist on disk");
    let orphan = orphan_object_dir(&debug).join("orphan-deadbeefdeadbeef.0.rcgu.o");
    fs::write(&orphan, b"orphan").unwrap();
    let (out, r) = checkout_gc(&hive, &repo.main);
    assert!(out.status.success(), "{r}");
    assert!(!orphan.exists(), "unreferenced object remained: {r}");
    for path in &before_named {
        assert!(
            path.is_file(),
            "binary-named object was deleted: {}: {r}",
            path.display()
        );
    }
    assert!(count(&r, "objects_deleted") >= 1, "{r}");
    assert!(count(&r, "objects_referenced") >= 1, "{r}");
}

#[test]
fn a_test_binary_that_was_not_relinked_still_keeps_its_objects() {
    let hive = Hive::ready();
    let repo = RegistryRepo::with_binaries(&hive);
    cargo_json(&hive, &repo.main, &["test", "--no-run"]);
    let debug = repo.main.join("target/debug");
    let it = find_linked_named(&debug, "it-").expect("integration test binary");
    let fs = cargo_hive_platform::current().unwrap().fs;
    let it_loose = match cargo_hive_gc::debug_map::read(fs.as_ref(), &it) {
        Ok(Some(map)) => map.loose,
        other => panic!("expected a debug map for {}: {other:?}", it.display()),
    };
    assert!(!it_loose.is_empty(), "test binary named no loose objects");
    fs::write(
        repo.main.join("app/src/main.rs"),
        "fn main() { println!(\"relinked-only-bin\"); }\n",
    )
    .unwrap();
    cargo_json(&hive, &repo.main, &["test", "--no-run"]);
    assert!(it.is_file(), "unrelinked test binary disappeared");
    let orphan = orphan_object_dir(&debug).join("orphan-cafecafecafecafe.0.rcgu.o");
    fs::write(&orphan, b"orphan").unwrap();
    let (out, r) = checkout_gc(&hive, &repo.main);
    assert!(out.status.success(), "{r}");
    assert!(!orphan.exists(), "orphan remained: {r}");
    let named = object_paths_named(&debug, &it_loose);
    assert_eq!(
        named.len(),
        it_loose.len(),
        "unrelinked test binary objects missing: {r}"
    );
    for path in &named {
        assert!(
            path.is_file(),
            "unrelinked test binary object {} was deleted: {r}",
            path.display()
        );
    }
}

#[test]
fn objects_named_only_by_a_dead_units_binary_go_in_the_same_run() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let target = repo.main.join("target");
    let debug = target.join("debug");
    let (_live, dead) = decide(&target);
    let dead_unit = dead
        .iter()
        .find(|unit| {
            unit.unit.loc.hex().len() == 16
                && (unit.unit.loc.layout == UnitLayout::Flat
                    || unit_path::outputs(&unit.unit.profile, &unit.unit.loc).is_dir())
        })
        .expect("a dead unit with a 16-hex hash");
    let dead_hash = dead_unit.unit.loc.hex().to_string();
    // A flat unit's outputs share deps/ with every other unit; a unit-dir unit owns its out/.
    let plant = match dead_unit.unit.loc.layout {
        UnitLayout::Flat => orphan_object_dir(&debug),
        UnitLayout::UnitDir => unit_path::outputs(&dead_unit.unit.profile, &dead_unit.unit.loc),
    };
    let object_name = "only-dead-unit-object.0.rcgu.o";
    let object = plant.join(object_name);
    fs::write(&object, b"only-dead").unwrap();
    let binary = plant.join(format!("staleextra-{dead_hash}"));
    fs::write(&binary, thin_execute_bytes(&[object_name])).unwrap();
    assert!(
        decide(&target)
            .1
            .iter()
            .any(|unit| unit.files.iter().any(|p| binary.starts_with(p))),
        "planted binary must belong to a dead unit"
    );
    let (out, r) = checkout_gc(&hive, &repo.main);
    assert!(out.status.success(), "{r}");
    assert!(!binary.exists(), "dead unit binary remained: {r}");
    assert!(
        !object.exists(),
        "object named only by a dead unit's binary survived: {r}"
    );
}

#[test]
fn a_cloned_target_keeps_the_objects_its_binaries_name_under_the_old_path() {
    let hive = Hive::ready();
    let repo = RegistryRepo::with_binaries(&hive);
    let src = repo.worktree("clone-src");
    let dst = repo.worktree("clone-dst");
    cargo_json(&hive, &src, &["test", "--no-run"]);
    let src_target = src.join("target");
    let dst_target = dst.join("target");
    let _ = fs::remove_dir_all(&dst_target);
    let status = Command::new("cp")
        .args(["-c", "-R"])
        .arg(&src_target)
        .arg(&dst_target)
        .status()
        .expect("spawn cp");
    assert!(
        status.success(),
        "cp -c -R failed; APFS clone is required for this case: {status}"
    );
    let debug = dst_target.join("debug");
    let fs = cargo_hive_platform::current().unwrap().fs;
    let listing =
        cargo_hive_gc::checkout::list_profile(fs.as_ref(), &debug, 1_000_000, 100_000).unwrap();
    let mut referenced = std::collections::BTreeSet::new();
    for path in &listing.candidates {
        if let Ok(Some(map)) = cargo_hive_gc::debug_map::read(fs.as_ref(), path) {
            referenced.extend(map.loose);
        }
    }
    assert!(!referenced.is_empty(), "cloned binaries named no objects");
    let named_files = object_paths_named(&debug, &referenced);
    assert!(!named_files.is_empty(), "no named objects on disk");
    let orphan = orphan_object_dir(&debug).join("orphan-clonedcloned00.0.rcgu.o");
    fs::write(&orphan, b"orphan").unwrap();
    let (out, r) = checkout_gc(&hive, &dst);
    assert!(out.status.success(), "{r}");
    assert!(!orphan.exists(), "orphan remained in cloned target: {r}");
    for path in &named_files {
        assert!(
            path.is_file(),
            "object {} named under the old path was deleted: {r}",
            path.file_name().unwrap().to_string_lossy()
        );
    }
}

#[test]
fn an_undecided_target_still_loses_its_unreferenced_objects() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let target = repo.main.join("target");
    let debug = target.join("debug");
    let unit = newest_fingerprint_unit(&target);
    let dir = fingerprint_dir_for(&target, &unit);
    let json = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|ext| ext == "json"))
        .expect("fingerprint json");
    let text = fs::read_to_string(&json).unwrap();
    let ghost = "[1,\"ghostcrate\",false,9999999999999999]";
    let patched = if text.contains("\"deps\":[]") {
        text.replacen("\"deps\":[]", &format!("\"deps\":[{ghost}]"), 1)
    } else {
        text.replacen("\"deps\":[", &format!("\"deps\":[{ghost},"), 1)
    };
    assert_ne!(patched, text, "failed to inject a ghost dependency");
    fs::write(&json, patched).unwrap();
    let orphan = orphan_object_dir(&debug).join("orphan-undecided0001.0.rcgu.o");
    fs::write(&orphan, b"orphan").unwrap();
    let before = fingerprint_dirs(&target);
    let (out, r) = checkout_gc(&hive, &repo.main);
    assert_eq!(out.status.code(), Some(0), "{r}");
    assert_eq!(count(&r, "units_deleted"), 0, "{r}");
    assert_eq!(fingerprint_dirs(&target), before, "{r}");
    assert!(
        r["findings"].as_array().unwrap().iter().any(|f| {
            f["kind"] == "gc.undecided"
                && f["detail"].as_str().unwrap().contains("clears when")
                && !f["detail"]
                    .as_str()
                    .unwrap()
                    .contains("profile dir unsupported")
        }),
        "{r}"
    );
    assert!(
        !orphan.exists(),
        "undecided target kept an unreferenced object: {r}"
    );
    assert!(count(&r, "objects_deleted") >= 1, "{r}");
}

#[test]
fn a_profile_with_an_unsupported_fingerprint_format_keeps_every_object() {
    let hive = Hive::ready();
    let repo = RegistryRepo::with_binaries(&hive);
    cargo_json(&hive, &repo.main, &["test", "--no-run"]);
    let debug = repo.main.join("target/debug");
    let plant = orphan_object_dir(&debug);
    let unit = newest_fingerprint_unit(&repo.main.join("target"));
    let dir = fingerprint_dir_for(&repo.main.join("target"), &unit);
    fs::write(dir.join("extra-corrupt.json"), b"{}").unwrap();
    let orphan = plant.join("orphan-unsupported01.0.rcgu.o");
    fs::write(&orphan, b"orphan").unwrap();
    let objects_before = deps_rcgu_o(&plant);
    assert!(!objects_before.is_empty());
    let (out, r) = checkout_gc(&hive, &repo.main);
    assert_eq!(out.status.code(), Some(0), "{r}");
    assert!(orphan.is_file(), "unsupported profile lost an object: {r}");
    assert_eq!(
        deps_rcgu_o(&plant).len(),
        objects_before.len(),
        "unsupported profile deleted objects: {r}"
    );
    assert!(
        r["findings"].as_array().unwrap().iter().any(|f| {
            f["kind"] == "gc.objects_undecided" && f["path"] == serde_json::json!(debug)
        }),
        "{r}"
    );
    assert_eq!(count(&r, "objects_deleted"), 0, "{r}");
}

#[test]
fn the_receipt_counts_objects_deleted_kept_and_binaries_read() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let plant = orphan_object_dir(&repo.main.join("target/debug"));
    fs::write(plant.join("orphan-countcount0001.0.rcgu.o"), b"orphan").unwrap();
    let (out, r) = checkout_gc(&hive, &repo.main);
    assert!(out.status.success(), "{r}");
    assert!(count(&r, "objects_deleted") >= 1, "{r}");
    assert!(count(&r, "objects_referenced") >= 1, "{r}");
    assert!(count(&r, "binaries_read") >= 1, "{r}");
}

#[test]
fn objects_are_read_with_ranged_reads_and_no_child_process() {
    let hive = Hive::ready();
    let repo = RegistryRepo::with_binaries(&hive);
    cargo_json(&hive, &repo.main, &["test", "--no-run"]);
    let debug = repo.main.join("target/debug");
    let binaries = candidate_binaries(&debug);
    assert!(
        !binaries.is_empty(),
        "no linked binaries found under debug/"
    );
    let marker = repo.scratch.path.join("nm-was-run");
    let nm_dir = repo.scratch.path.join("fake-nm-bin");
    fs::create_dir_all(&nm_dir).unwrap();
    let nm = nm_dir.join("nm");
    fs::write(
        &nm,
        format!("#!/bin/sh\necho ran > '{}'\nexit 1\n", marker.display()),
    )
    .unwrap();
    let mut perms = fs::metadata(&nm).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&nm, perms).unwrap();
    let path = format!(
        "{}:{}",
        nm_dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let (_fs_box, scan, checkout, nested) = library_scan(&hive, &repo);
    let rec = Recording::over_real();
    let old_path = std::env::var_os("PATH");
    // SAFETY: single-threaded test process; PATH restored before the test returns.
    unsafe { std::env::set_var("PATH", &path) };
    let report = run_library(&rec, &scan, &checkout, &nested);
    match old_path {
        Some(v) => unsafe { std::env::set_var("PATH", v) },
        None => unsafe { std::env::remove_var("PATH") },
    }
    assert!(!report.failed, "{:?}", report.findings);
    assert!(report.totals.binaries_read >= 1, "{report:?}");
    let log = rec.log();
    for bin in &binaries {
        assert!(
            log.iter().any(|line| {
                line.starts_with("read_at ") && line.contains(&bin.display().to_string())
            }),
            "missing read_at for {}: {log:?}",
            bin.display()
        );
    }
    assert!(
        !marker.exists(),
        "nm on PATH was invoked; marker at {}",
        marker.display()
    );
}

#[test]
fn a_binary_whose_read_fails_keeps_every_object_in_its_profile() {
    let hive = Hive::ready();
    let repo = RegistryRepo::with_binaries(&hive);
    cargo_json(&hive, &repo.main, &["test", "--no-run"]);
    let debug = repo.main.join("target/debug");
    let plant = orphan_object_dir(&debug);
    let bin = debug.join("app");
    assert!(bin.is_file(), "app binary missing");
    let orphan = plant.join("orphan-failread00001.0.rcgu.o");
    fs::write(&orphan, b"orphan").unwrap();
    let before = deps_rcgu_o(&plant);
    let (_fs, scan, checkout, nested) = library_scan(&hive, &repo);
    let rec = Recording::over_real().fail_read_at(&bin);
    let report = run_library(&rec, &scan, &checkout, &nested);
    assert!(orphan.is_file(), "read failure still deleted objects");
    assert_eq!(deps_rcgu_o(&plant).len(), before.len());
    assert_eq!(report.totals.objects_deleted, 0, "{report:?}");
    assert!(
        report
            .findings
            .iter()
            .any(|f| { f.kind == "gc.objects_undecided" && f.path.as_ref() == Some(&bin) }),
        "{:?}",
        report.findings
    );
}

#[test]
fn a_dead_unit_dir_is_deleted_in_order_and_its_files_are_counted() {
    let hive = Hive::ready();
    let s = Scratch::new("checkout-udir-dead");
    let repo = init_repo(&s.path.join("app"), "");
    register(&hive, &repo);
    hive.build_crate(&repo, None);
    let target = repo.join("target");
    let profile = target.join("debug");
    let loc = plant_unit(
        &profile,
        UnitLayout::UnitDir,
        "probe",
        "0123456789abcdef",
        "lib-probe",
        42,
        &[],
        &["libprobe.rmeta", "z.d", "a.d", "m.d", "libprobe.rlib"],
    );
    let unit = unit_path::unit_dir(&profile, &loc).unwrap();
    fs::create_dir_all(unit.join("run")).unwrap();
    let stamp = unit_path::stamp(
        &profile,
        &loc,
        cargo_hive_core::fingerprint::UnitKind::of("lib-probe"),
    );
    set_mtime(&stamp, SystemTime::now() - THIRTY_DAYS);
    let expected_order = [
        unit.join("out").join("a.d"),
        unit.join("out").join("m.d"),
        unit.join("out").join("z.d"),
        unit.join("out"),
        unit.join("run"),
        unit.join("fingerprint"),
        unit.clone(),
    ];
    let files_before = count_regular_files_under(&unit);
    assert!(files_before > 0);

    let rec = Recording::over_real();
    let state = cargo_hive_state::State::open_at(hive.home.path.clone()).unwrap();
    let cfg = state.config().unwrap();
    let reg: cargo_hive_cargo::registry::Registry =
        state.read(cargo_hive_cargo::registry::FILE).unwrap();
    let registered: Vec<_> = reg.repos.iter().map(|r| r.main.clone()).collect();
    let (rows, _) = cargo_hive_cargo::discover::checkouts(
        reg.repos.iter().find(|r| r.main == repo).unwrap(),
        &registered,
        &cfg,
    );
    let live: Vec<_> = rows.into_iter().filter(|c| !c.missing).collect();
    let nested: Vec<PathBuf> = Vec::new();
    let mut snapshot = cargo_hive_cargo::discover::Snapshot::default();
    let scan = cargo_hive_cargo::discover::targets(&live, &cfg, false, &mut snapshot, || false);
    let report = run_library(rec.fs().as_ref(), &scan, &repo, &nested);

    assert!(
        !unit.exists(),
        "dead unit dir remained: {:?}; findings={:?}",
        report.totals,
        report.findings
    );
    assert!(
        profile.join("build/probe").is_dir(),
        "package dir must stay"
    );
    let removes: Vec<_> = rec
        .log()
        .into_iter()
        .filter_map(|line| line.strip_prefix("remove ").map(PathBuf::from))
        .filter(|p| p.starts_with(&unit) || p == &unit)
        .collect();
    assert_eq!(
        removes,
        expected_order,
        "deletion order; log={:?}",
        rec.log()
    );
    assert_eq!(report.totals.units_deleted, 1, "{report:?}");
    assert_eq!(
        report.totals.unit_files_deleted, files_before,
        "unit_files_deleted must count every regular file under the unit dir: {report:?}"
    );
}

fn count_regular_files_under(root: &Path) -> u64 {
    let mut n = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let ft = entry.file_type().unwrap();
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn an_out_cap_hit_or_unreadable_package_dir_names_the_dir_and_deletes_no_object() {
    let real = cargo_hive_platform::current().unwrap().fs;

    let s_cap = Scratch::new("checkout-udir-out-cap");
    let profile_cap = s_cap.path.join("debug");
    fs::create_dir_all(&profile_cap).unwrap();
    let loc = plant_unit(
        &profile_cap,
        UnitLayout::UnitDir,
        "probe",
        "0123456789abcdef",
        "lib-probe",
        1,
        &[],
        &["a.0.rcgu.o", "b.0.rcgu.o", "c.0.rcgu.o"],
    );
    let out = unit_path::outputs(&profile_cap, &loc);
    let err =
        cargo_hive_gc::checkout::list_profile(real.as_ref(), &profile_cap, 2, 100).unwrap_err();
    assert!(
        err.contains(out.display().to_string().as_str()),
        "cap error must name the out dir: {err}"
    );

    let hive = Hive::ready();
    let s = Scratch::new("checkout-udir-pkg-unreadable");
    let repo = init_repo(&s.path.join("app"), "");
    register(&hive, &repo);
    hive.build_crate(&repo, None);
    let profile = repo.join("target/debug");
    let loc = plant_unit(
        &profile,
        UnitLayout::UnitDir,
        "probe",
        "1123456789abcdef",
        "lib-probe",
        2,
        &[],
        &["keep.0.rcgu.o", "orphan.0.rcgu.o"],
    );
    let out_dir = unit_path::outputs(&profile, &loc);
    fs::write(out_dir.join("bin"), thin_execute_bytes(&["keep.0.rcgu.o"])).unwrap();
    let orphan = out_dir.join("orphan.0.rcgu.o");
    let keep = out_dir.join("keep.0.rcgu.o");
    let pkg = profile.join("build/probe");
    let objects_before = count_regular_files_under(&out_dir);
    let fail = FailListFs {
        real,
        blocked: pkg.clone(),
    };
    let state = cargo_hive_state::State::open_at(hive.home.path.clone()).unwrap();
    let cfg = state.config().unwrap();
    let reg: cargo_hive_cargo::registry::Registry =
        state.read(cargo_hive_cargo::registry::FILE).unwrap();
    let registered: Vec<_> = reg.repos.iter().map(|r| r.main.clone()).collect();
    let (rows, _) = cargo_hive_cargo::discover::checkouts(
        reg.repos.iter().find(|r| r.main == repo).unwrap(),
        &registered,
        &cfg,
    );
    let live: Vec<_> = rows.into_iter().filter(|c| !c.missing).collect();
    let mut snapshot = cargo_hive_cargo::discover::Snapshot::default();
    let scan = cargo_hive_cargo::discover::targets(&live, &cfg, false, &mut snapshot, || false);
    let report = run_library(&fail, &scan, &repo, &[]);
    assert!(
        orphan.is_file() && keep.is_file(),
        "unreadable package dir must delete no object: {report:?}"
    );
    assert_eq!(
        count_regular_files_under(&out_dir),
        objects_before,
        "{report:?}"
    );
    assert_eq!(report.totals.objects_deleted, 0, "{report:?}");
    assert!(
        report.findings.iter().any(|f| {
            f.kind == "gc.objects_undecided"
                && f.detail.contains(pkg.display().to_string().as_str())
        }),
        "{:?}",
        report.findings
    );
}

struct FailListFs {
    real: Box<dyn cargo_hive_platform::Fs>,
    blocked: PathBuf,
}

impl cargo_hive_platform::Fs for FailListFs {
    fn temp_dir(
        &self,
        parent: &Path,
        prefix: &str,
    ) -> std::io::Result<cargo_hive_platform::TempDir> {
        self.real.temp_dir(parent, prefix)
    }
    fn write_file(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        self.real.write_file(path, bytes)
    }
    fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        self.real.read(path)
    }
    fn read_at(&self, path: &Path, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
        self.real.read_at(path, offset, len)
    }
    fn hardlink(&self, src: &Path, dst: &Path) -> std::io::Result<()> {
        self.real.hardlink(src, dst)
    }
    fn set_mtime(&self, path: &Path, t: SystemTime) -> std::io::Result<()> {
        self.real.set_mtime(path, t)
    }
    fn mtime(&self, path: &Path) -> std::io::Result<SystemTime> {
        self.real.mtime(path)
    }
    fn inode(&self, path: &Path) -> std::io::Result<u64> {
        self.real.inode(path)
    }
    fn clone_file(&self, src: &Path, dst: &Path) -> std::io::Result<()> {
        self.real.clone_file(src, dst)
    }
    fn create_dir(&self, path: &Path) -> std::io::Result<()> {
        self.real.create_dir(path)
    }
    fn sharing(&self, path: &Path) -> std::io::Result<cargo_hive_platform::Sharing> {
        self.real.sharing(path)
    }
    fn compress(&self, path: &Path) -> cargo_hive_platform::CompressOutcome {
        self.real.compress(path)
    }
    fn hold_lock(
        &self,
        kind: cargo_hive_platform::LockKind,
        path: &Path,
    ) -> std::io::Result<cargo_hive_platform::LockHolder> {
        self.real.hold_lock(kind, path)
    }
    fn try_lock_exclusive(&self, path: &Path) -> cargo_hive_platform::TryLock {
        self.real.try_lock_exclusive(path)
    }
    fn try_lock_elsewhere(
        &self,
        kind: cargo_hive_platform::LockKind,
        path: &Path,
    ) -> std::io::Result<cargo_hive_platform::LockState> {
        self.real.try_lock_elsewhere(kind, path)
    }
    fn entry_stat(&self, path: &Path) -> std::io::Result<cargo_hive_platform::EntryStat> {
        self.real.entry_stat(path)
    }
    fn read_dir_bounded(
        &self,
        dir: &Path,
        cap: usize,
    ) -> std::io::Result<Vec<(String, cargo_hive_platform::EntryKind)>> {
        if dir == self.blocked {
            return Err(std::io::Error::other(format!(
                "{}: permission denied",
                dir.display()
            )));
        }
        self.real.read_dir_bounded(dir, cap)
    }
    fn remove_tree(&self, path: &Path) -> std::io::Result<()> {
        self.real.remove_tree(path)
    }
    fn remove_file(&self, path: &Path) -> std::io::Result<()> {
        self.real.remove_file(path)
    }
    fn volume(&self, path: &Path) -> std::io::Result<cargo_hive_platform::VolumeInfo> {
        self.real.volume(path)
    }
}

#[test]
fn a_real_cargo_unit_dir_checkout_deletes_unreferenced_out_objects() {
    if !rustc_release_is("1.100.0") {
        eprintln!("skip: active rustc is not 1.100.0");
        return;
    }
    let hive = Hive::ready();
    let repo = RegistryRepo::with_binaries(&hive);
    cargo_json(&hive, &repo.main, &["test", "--no-run"]);
    let target = repo.main.join("target");
    let layout = layout_of(&target);
    assert_eq!(
        layout,
        UnitLayout::UnitDir,
        "1.100.0 fixture must land in the unit-dir layout"
    );
    let debug = target.join("debug");
    let fs = cargo_hive_platform::current().unwrap().fs;
    let listing =
        cargo_hive_gc::checkout::list_profile(fs.as_ref(), &debug, 1_000_000, 100_000).unwrap();
    assert!(
        listing
            .objects
            .iter()
            .any(|(p, _, _)| p.to_string_lossy().contains("/out/")),
        "expected out/*.rcgu.o objects: {:?}",
        listing.objects
    );
    let mut kept_names = std::collections::BTreeSet::new();
    for path in &listing.candidates {
        if let Ok(Some(map)) = cargo_hive_gc::debug_map::read(fs.as_ref(), path) {
            kept_names.extend(map.loose);
        }
    }
    assert!(
        !kept_names.is_empty(),
        "test or example binaries in out/ must name loose objects"
    );
    let lib_out = listing
        .objects
        .iter()
        .map(|(p, _, _)| p.parent().unwrap().to_path_buf())
        .find(|out| {
            out.join("libapp.rlib").is_file()
                || fs::read_dir(out).unwrap().any(|e| {
                    let n = e.unwrap().file_name();
                    let n = n.to_string_lossy();
                    n.starts_with("lib") && n.ends_with(".rlib")
                })
        })
        .expect("a lib unit out/ dir");
    let before_named: Vec<_> = kept_names
        .iter()
        .flat_map(|name| {
            listing
                .objects
                .iter()
                .map(|(p, _, _)| p)
                .filter(|p| p.file_name().is_some_and(|n| n == name.as_str()))
                .cloned()
        })
        .collect();
    assert!(!before_named.is_empty(), "named out objects must exist");
    let orphan = lib_out.join("orphan-unitdirdead01.0.rcgu.o");
    fs::write(&orphan, b"orphan").unwrap();
    let (out, r) = checkout_gc(&hive, &repo.main);
    assert!(out.status.success(), "{r}");
    assert!(
        !orphan.exists(),
        "unreferenced lib out object remained: {r}"
    );
    for path in &before_named {
        assert!(
            path.is_file(),
            "binary-named out object was deleted: {}: {r}",
            path.display()
        );
    }
    assert!(count(&r, "objects_deleted") >= 1, "{r}");
    assert!(count(&r, "binaries_read") > 0, "{r}");
}

fn rustc_release_is(want: &str) -> bool {
    let out = Command::new("rustc").arg("-V").output().unwrap();
    String::from_utf8_lossy(&out.stdout).contains(want)
}

#[test]
fn a_profile_with_unit_dirs_and_no_deps_still_cleans_out_objects() {
    let hive = Hive::ready();
    let s = Scratch::new("checkout-udir-no-deps");
    let repo = init_repo(&s.path.join("app"), "");
    register(&hive, &repo);
    hive.build_crate(&repo, None);
    let profile = repo.join("target/debug");
    let _ = fs::remove_dir_all(profile.join("deps"));
    assert!(!profile.join("deps").exists());
    let loc = plant_unit(
        &profile,
        UnitLayout::UnitDir,
        "probe",
        "2123456789abcdef",
        "lib-probe",
        3,
        &[],
        &["keep.0.rcgu.o", "orphan.0.rcgu.o", "libprobe.rlib"],
    );
    let out = unit_path::outputs(&profile, &loc);
    fs::write(out.join("bin"), thin_execute_bytes(&["keep.0.rcgu.o"])).unwrap();
    let keep = out.join("keep.0.rcgu.o");
    let orphan = out.join("orphan.0.rcgu.o");
    let (cmd, r) = checkout_gc(&hive, &repo);
    assert!(cmd.status.success(), "{r}");
    assert!(!orphan.exists(), "unreferenced out object remained: {r}");
    assert!(keep.is_file(), "referenced out object was deleted: {r}");
    assert!(count(&r, "objects_deleted") >= 1, "{r}");
    assert!(count(&r, "binaries_read") >= 1, "{r}");
}

fn run_library(
    fs: &dyn cargo_hive_platform::Fs,
    scan: &cargo_hive_cargo::discover::TargetScan,
    checkout: &Path,
    nested: &[PathBuf],
) -> cargo_hive_gc::checkout::CheckoutReport {
    let input = cargo_hive_gc::checkout::CheckoutInput {
        fs,
        scan,
        checkout,
        nested,
        now: SystemTime::now(),
        window: Duration::from_secs(7 * 86_400),
        account_bytes: true,
    };
    cargo_hive_gc::checkout::run(&input, &|| false)
}

fn last_deleted_file(target: &Path) -> PathBuf {
    let (_, dead) = decide(target);
    let mut last = None;
    for unit in &dead {
        let ordered = cargo_hive_gc::checkout::unit_order(&unit.files);
        for path in ordered {
            if path.is_file() {
                last = Some(path);
            }
        }
    }
    last.expect("a dead unit file that remove_file will delete")
}

/// Revive every dead unit but one, drop that unit's dep-info, and return its fingerprint-first order.
fn first_fingerprint_first_unit(target: &Path) -> Vec<PathBuf> {
    let (_, dead) = decide(target);
    let (index, unit) = dead
        .iter()
        .enumerate()
        .find(|(_, unit)| {
            unit.files.iter().any(|p| is_fingerprint_entry(p))
                && unit.files.iter().any(|p| !is_fingerprint_entry(p))
        })
        .expect("a dead unit with a fingerprint dir and at least one other file");
    let now = SystemTime::now();
    for (i, other) in dead.iter().enumerate() {
        if i == index {
            continue;
        }
        let stamp = other
            .files
            .iter()
            .find(|p| is_fingerprint_entry(p))
            .map(|dir| dir.join("invoked.timestamp"));
        if let Some(stamp) = stamp
            && stamp.exists()
        {
            set_mtime(&stamp, now);
        }
    }
    for path in &unit.files {
        if path.extension().is_some_and(|ext| ext == "d") {
            let _ = fs::remove_file(path);
        }
    }
    let files: Vec<_> = unit
        .files
        .iter()
        .filter(|p| p.extension().is_none_or(|ext| ext != "d"))
        .filter(|p| p.exists())
        .cloned()
        .collect();
    cargo_hive_gc::checkout::unit_order(&files)
}

/// A unit's fingerprint dir: `.fingerprint/<name-meta>` (flat) or `<unit dir>/fingerprint`.
fn is_fingerprint_entry(p: &Path) -> bool {
    p.file_name().is_some_and(|name| name == "fingerprint")
        || p.parent()
            .and_then(|parent| parent.file_name())
            .is_some_and(|name| name == ".fingerprint")
}

fn aged_binaries(hive: &Hive) -> RegistryRepo {
    let repo = RegistryRepo::with_binaries(hive);
    cargo_json(hive, &repo.main, &["test", "--no-run"]);
    backdate_all(&repo.main.join("target"), THIRTY_DAYS);
    bump_app_version(&repo.main);
    cargo_json(hive, &repo.main, &["test", "--no-run"]);
    repo
}

fn bump_app_version(checkout: &Path) {
    for pkg in ["app", "appmacro"] {
        let manifest = checkout.join(pkg).join("Cargo.toml");
        if !manifest.exists() {
            continue;
        }
        let text = fs::read_to_string(&manifest).unwrap();
        let next = if text.contains("version = \"0.1.0\"") {
            text.replace("version = \"0.1.0\"", "version = \"0.1.1\"")
        } else {
            text.replace("version = \"0.1.1\"", "version = \"0.1.2\"")
        };
        fs::write(&manifest, next).unwrap();
    }
}

fn checkout_gc(hive: &Hive, path: &Path) -> (Output, serde_json::Value) {
    hive.json(&["gc", "--checkout", path.to_str().unwrap()])
}

fn receipt_file_bytes(root: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    let dir = root.join("receipts");
    let mut out = std::collections::BTreeMap::new();
    let Ok(entries) = fs::read_dir(&dir) else {
        return out;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            out.insert(path.clone(), fs::read(&path).unwrap());
        }
    }
    out
}

fn count(r: &serde_json::Value, name: &str) -> u64 {
    r["counts"][name].as_u64().unwrap_or(0)
}

fn receipts_mention(hive: &Hive, command: &str) -> bool {
    let dir = hive.home.path.join("receipts");
    let Ok(entries) = fs::read_dir(&dir) else {
        return false;
    };
    entries.filter_map(|e| e.ok()).any(|e| {
        fs::read_to_string(e.path())
            .unwrap_or_default()
            .contains(&format!("\"command\":\"{command}\""))
    })
}

fn fingerprint_dirs(target: &Path) -> Vec<String> {
    let profile = target.join("debug");
    let mut out = std::collections::BTreeSet::new();
    let build = profile.join("build");
    let mut saw_unit_dir = false;
    if let Ok(pkgs) = fs::read_dir(&build) {
        for pkg in pkgs {
            let pkg = pkg.unwrap().path();
            if !pkg.is_dir() {
                continue;
            }
            let pkg_name = pkg.file_name().unwrap().to_string_lossy();
            if let Ok(hexes) = fs::read_dir(&pkg) {
                for hex in hexes {
                    let hex = hex.unwrap().path();
                    if !hex.is_dir() {
                        continue;
                    }
                    let hex_name = hex.file_name().unwrap().to_string_lossy();
                    if hex_name.len() == 16 && hex_name.chars().all(|c| c.is_ascii_hexdigit()) {
                        saw_unit_dir = true;
                        out.insert(format!("{pkg_name}-{hex_name}"));
                    }
                }
            }
        }
    }
    if !saw_unit_dir {
        let fp = profile.join(".fingerprint");
        if let Ok(entries) = fs::read_dir(&fp) {
            for entry in entries {
                let name = entry.unwrap().file_name().to_string_lossy().into_owned();
                if name.contains('-') {
                    out.insert(name);
                }
            }
        }
    }
    out.into_iter().collect()
}

fn unit_loc_for(target: &Path, name_meta: &str) -> unit_path::UnitLoc {
    unit_path::UnitLoc {
        layout: layout_of(target),
        name_meta: name_meta.to_string(),
    }
}

fn fingerprint_dir_for(target: &Path, name_meta: &str) -> PathBuf {
    unit_path::fingerprint_dir(&target.join("debug"), &unit_loc_for(target, name_meta))
}

fn stamp_for(target: &Path, name_meta: &str) -> PathBuf {
    let profile = target.join("debug");
    let loc = unit_loc_for(target, name_meta);
    unit_path::stamp(&profile, &loc, kind_for_unit(target, name_meta))
}

fn kind_for_unit(target: &Path, name_meta: &str) -> cargo_hive_core::fingerprint::UnitKind {
    let report = cargo_hive_core::fingerprint::read_profile(&target.join("debug"));
    if let Some(graph) = &report.graph
        && let Some(unit) = graph.units.iter().find(|u| u.loc.name_meta == name_meta)
    {
        return unit.kind;
    }
    report
        .not_current_units
        .iter()
        .find(|nc| nc.loc.name_meta == name_meta)
        .map(|nc| {
            nc.stem.as_deref().map_or(
                cargo_hive_core::fingerprint::UnitKind::Other,
                cargo_hive_core::fingerprint::UnitKind::of,
            )
        })
        .unwrap_or(cargo_hive_core::fingerprint::UnitKind::Lib)
}

/// Directory where a planted unreferenced `.rcgu.o` is visible to checkout GC.
fn orphan_object_dir(debug: &Path) -> PathBuf {
    let target = debug.parent().expect("profile under target");
    match layout_of(target) {
        UnitLayout::Flat => {
            let deps = debug.join("deps");
            let _ = fs::create_dir_all(&deps);
            deps
        }
        UnitLayout::UnitDir => {
            let fs = cargo_hive_platform::current().unwrap().fs;
            let listing =
                cargo_hive_gc::checkout::list_profile(fs.as_ref(), debug, 1_000_000, 100_000)
                    .unwrap();
            if let Some(out) = listing
                .objects
                .iter()
                .map(|(p, _, _)| p.parent().unwrap().to_path_buf())
                .next()
            {
                return out;
            }
            for pkg in fs::read_dir(debug.join("build")).unwrap() {
                let pkg = pkg.unwrap().path();
                if !pkg.is_dir() {
                    continue;
                }
                for hex in fs::read_dir(&pkg).unwrap() {
                    let hex = hex.unwrap().path();
                    let out = hex.join("out");
                    if out.is_dir() {
                        return out;
                    }
                }
            }
            panic!("no out/ dir to plant orphans under {}", debug.display());
        }
    }
}

fn object_paths_named(debug: &Path, names: &std::collections::BTreeSet<String>) -> Vec<PathBuf> {
    let fs = cargo_hive_platform::current().unwrap().fs;
    let listing =
        cargo_hive_gc::checkout::list_profile(fs.as_ref(), debug, 1_000_000, 100_000).unwrap();
    names
        .iter()
        .flat_map(|name| {
            listing
                .objects
                .iter()
                .map(|(p, _, _)| p)
                .filter(|p| p.file_name().is_some_and(|n| n == name.as_str()))
                .cloned()
        })
        .collect()
}

fn find_linked_named(debug: &Path, prefix: &str) -> Option<PathBuf> {
    let target = debug.parent()?;
    match layout_of(target) {
        UnitLayout::Flat => {
            named_under(&debug.join("deps"), prefix).filter(|p| is_linked_binary(p))
        }
        UnitLayout::UnitDir => {
            let fs = cargo_hive_platform::current().unwrap().fs;
            let listing =
                cargo_hive_gc::checkout::list_profile(fs.as_ref(), debug, 1_000_000, 100_000)
                    .ok()?;
            listing.candidates.into_iter().find(|p| {
                p.file_name().is_some_and(|n| {
                    let name = n.to_string_lossy();
                    name.starts_with(prefix) && !name.contains('.')
                }) && is_linked_binary(p)
            })
        }
    }
}

fn oldest_fingerprint_unit(target: &Path) -> (String, PathBuf) {
    let profile = target.join("debug");
    let report = cargo_hive_core::fingerprint::read_profile(&profile);
    let mut best: Option<(SystemTime, String, PathBuf)> = None;
    let mut consider = |loc: &unit_path::UnitLoc, kind: cargo_hive_core::fingerprint::UnitKind| {
        let stamp = unit_path::stamp(&profile, loc, kind);
        let Ok(meta) = fs::metadata(&stamp) else {
            return;
        };
        let t = meta.modified().unwrap();
        if best.as_ref().is_none_or(|(b, _, _)| t < *b) {
            best = Some((t, loc.name_meta.clone(), stamp));
        }
    };
    if let Some(graph) = &report.graph {
        for unit in &graph.units {
            consider(&unit.loc, unit.kind);
        }
    }
    for nc in &report.not_current_units {
        let kind = nc.stem.as_deref().map_or(
            cargo_hive_core::fingerprint::UnitKind::Other,
            cargo_hive_core::fingerprint::UnitKind::of,
        );
        consider(&nc.loc, kind);
    }
    let (_, name, stamp) = best.expect("a fingerprint unit");
    (name, stamp)
}

fn newest_fingerprint_unit(target: &Path) -> String {
    let profile = target.join("debug");
    let report = cargo_hive_core::fingerprint::read_profile(&profile);
    let mut best: Option<(SystemTime, String)> = None;
    let mut consider = |loc: &unit_path::UnitLoc, kind: cargo_hive_core::fingerprint::UnitKind| {
        let stamp = unit_path::stamp(&profile, loc, kind);
        let Ok(meta) = fs::metadata(&stamp) else {
            return;
        };
        let t = meta.modified().unwrap();
        if best.as_ref().is_none_or(|(b, _)| t > *b) {
            best = Some((t, loc.name_meta.clone()));
        }
    };
    if let Some(graph) = &report.graph {
        for unit in &graph.units {
            consider(&unit.loc, unit.kind);
        }
    }
    for nc in &report.not_current_units {
        let kind = nc.stem.as_deref().map_or(
            cargo_hive_core::fingerprint::UnitKind::Other,
            cargo_hive_core::fingerprint::UnitKind::of,
        );
        consider(&nc.loc, kind);
    }
    best.expect("a fingerprint unit").1
}

fn hash_file_in(dir: &Path) -> PathBuf {
    fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            let name = p.file_name().unwrap().to_string_lossy();
            p.extension().is_none()
                && name != "invoked.timestamp"
                && !name.starts_with("dep-")
                && !name.starts_with("output-")
        })
        .expect("a hash file")
}

fn library_scan(
    hive: &Hive,
    repo: &RegistryRepo,
) -> (
    Box<dyn cargo_hive_platform::Fs>,
    cargo_hive_cargo::discover::TargetScan,
    PathBuf,
    Vec<PathBuf>,
) {
    let fs = cargo_hive_platform::current().unwrap().fs;
    let (scan, checkout, nested) = scan_only(hive, repo);
    (fs, scan, checkout, nested)
}

fn scan_only(
    hive: &Hive,
    repo: &RegistryRepo,
) -> (
    cargo_hive_cargo::discover::TargetScan,
    PathBuf,
    Vec<PathBuf>,
) {
    let state = cargo_hive_state::State::open_at(hive.home.path.clone()).unwrap();
    let cfg = state.config().unwrap();
    let reg: cargo_hive_cargo::registry::Registry =
        state.read(cargo_hive_cargo::registry::FILE).unwrap();
    let registered: Vec<_> = reg.repos.iter().map(|r| r.main.clone()).collect();
    let (rows, _) = cargo_hive_cargo::discover::checkouts(
        reg.repos.iter().find(|r| r.main == repo.main).unwrap(),
        &registered,
        &cfg,
    );
    let live: Vec<_> = rows.into_iter().filter(|c| !c.missing).collect();
    let checkout = repo.main.clone();
    let nested: Vec<_> = live
        .iter()
        .map(|c| c.path.clone())
        .filter(|p| p != &checkout && p.starts_with(&checkout))
        .collect();
    let mut snapshot = cargo_hive_cargo::discover::Snapshot::default();
    let scan = cargo_hive_cargo::discover::targets(&live, &cfg, false, &mut snapshot, || false);
    (scan, checkout, nested)
}

fn thin_execute_bytes(oso: &[&str]) -> Vec<u8> {
    const MH_MAGIC_64: u32 = 0xfeed_facf;
    const MH_EXECUTE: u32 = 0x2;
    const LC_SYMTAB: u32 = 0x2;
    const N_OSO: u8 = 0x66;
    const HEADER64: u32 = 32;
    const SYMTAB_CMD: u32 = 24;
    const NLIST64: u32 = 16;
    let mut strtab = vec![0u8];
    let mut str_offs = Vec::new();
    for s in oso {
        str_offs.push(strtab.len() as u32);
        strtab.extend_from_slice(s.as_bytes());
        strtab.push(0);
    }
    let nsyms = oso.len() as u32;
    let symoff = HEADER64 + SYMTAB_CMD;
    let stroff = symoff + nsyms * NLIST64;
    let strsize = strtab.len() as u32;
    let mut buf = Vec::new();
    let put = |buf: &mut Vec<u8>, v: u32| buf.extend_from_slice(&v.to_le_bytes());
    put(&mut buf, MH_MAGIC_64);
    put(&mut buf, 0x0100_000c);
    put(&mut buf, 0);
    put(&mut buf, MH_EXECUTE);
    put(&mut buf, 1);
    put(&mut buf, SYMTAB_CMD);
    put(&mut buf, 0);
    put(&mut buf, 0);
    put(&mut buf, LC_SYMTAB);
    put(&mut buf, SYMTAB_CMD);
    put(&mut buf, symoff);
    put(&mut buf, nsyms);
    put(&mut buf, stroff);
    put(&mut buf, strsize);
    for off in str_offs {
        put(&mut buf, off);
        buf.push(N_OSO);
        buf.push(0);
        buf.extend_from_slice(&0u16.to_le_bytes());
        buf.extend_from_slice(&0u64.to_le_bytes());
    }
    buf.extend_from_slice(&strtab);
    buf
}

fn deps_rcgu_o(deps: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(deps) else {
        return out;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy();
        if path.is_file() && name.ends_with(".rcgu.o") {
            out.push(path);
        }
    }
    out.sort();
    out
}

fn candidate_binaries(debug: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut push_dir = |dir: &Path| {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy();
            if name.ends_with(".o") {
                continue;
            }
            if path.is_file() || path_is_symlink_to_file(&path) {
                out.push(path);
            }
        }
    };
    push_dir(debug);
    push_dir(&debug.join("deps"));
    push_dir(&debug.join("examples"));
    if let Ok(entries) = fs::read_dir(debug.join("build")) {
        for entry in entries {
            let child = entry.unwrap().path();
            if !child.is_dir() {
                continue;
            }
            push_dir(&child);
            if let Ok(hexes) = fs::read_dir(&child) {
                for hex in hexes {
                    let hex = hex.unwrap().path();
                    if hex.is_dir() {
                        push_dir(&hex.join("out"));
                    }
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out.into_iter()
        .filter(|path| {
            let fs = cargo_hive_platform::current().unwrap().fs;
            matches!(
                cargo_hive_gc::debug_map::read(fs.as_ref(), path),
                Ok(Some(_))
            )
        })
        .collect()
}

fn path_is_symlink_to_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .ok()
        .is_some_and(|m| m.file_type().is_symlink())
        && path.is_file()
}

fn named_under(dir: &Path, prefix: &str) -> Option<PathBuf> {
    fs::read_dir(dir).ok()?.find_map(|e| {
        let p = e.ok()?.path();
        let name = p.file_name()?.to_str()?;
        (name.starts_with(prefix) && !name.contains('.')).then_some(p)
    })
}

fn deps_dylib(dir: &Path, prefix: &str) -> Option<PathBuf> {
    fs::read_dir(dir).ok()?.find_map(|e| {
        let p = e.ok()?.path();
        let name = p.file_name()?.to_str()?;
        (name.starts_with(prefix) && name.ends_with(".dylib")).then_some(p)
    })
}

fn build_script_under(build: &Path) -> Option<PathBuf> {
    for entry in fs::read_dir(build).ok()? {
        let dir = entry.ok()?.path();
        if !dir.is_dir() {
            continue;
        }
        let name = dir.file_name()?.to_str()?;
        if !name.starts_with("app-") {
            continue;
        }
        let script = dir.join("build-script-build");
        if script.is_file() {
            return Some(script);
        }
    }
    None
}

fn build_script_under_unit_dir(build: &Path) -> Option<PathBuf> {
    for pkg in fs::read_dir(build).ok()? {
        let pkg = pkg.ok()?.path();
        if !pkg.is_dir() {
            continue;
        }
        for hex in fs::read_dir(&pkg).ok()? {
            let hex = hex.ok()?.path();
            if !hex.is_dir() {
                continue;
            }
            let script = hex.join("out").join("build_script_build");
            if script.is_file() {
                return Some(script);
            }
        }
    }
    None
}

fn is_linked_binary(path: &Path) -> bool {
    path.is_file()
        && fs::metadata(path)
            .ok()
            .is_some_and(|m| m.len() > 0 && !m.file_type().is_symlink())
}

fn pid_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn checkout_gc_lists_the_repositorys_checkouts_once() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    let log = repo.scratch.path.join("git-calls.log");
    let path = common::counting_git(&repo.scratch.path.join("shim"), &log);
    let out = hive
        .command(&["gc", "--checkout", repo.main.to_str().unwrap(), "--json"])
        .env("PATH", &path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let calls = fs::read_to_string(&log).unwrap_or_default();
    let listings = calls
        .lines()
        .filter(|l| l.contains("worktree list"))
        .count();
    assert_eq!(listings, 1, "git calls:\n{calls}");
}
