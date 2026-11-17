#![cfg(target_os = "macos")]
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod common;

use common::cargo_fixture::{THIRTY_DAYS, backdate_all, decide, set_mtime};
use common::plan_fixture::tree;
use common::recording_fs::Recording;
use common::registry_fixture::{RegistryRepo, cargo_json, set_target_dir};
use common::{Hive, Scratch, git, interrupt, wait_within};
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
    let debug = repo.main.join("target/debug");
    let deps = debug.join("deps");

    assert!(debug.join("app").is_file(), "bin missing");
    assert!(
        named_under(&deps, "it-").is_some_and(|p| is_linked_binary(&p)),
        "integration test binary missing under deps/"
    );
    assert!(debug.join("examples/ex").is_file(), "example missing");
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
            p.extension().is_some_and(|x| x == "o") && p.to_string_lossy().contains(".rcgu.")
        })
        .collect();
    assert!(
        !objects.is_empty(),
        "expected loose deps/*.rcgu.o beside the linked binaries"
    );
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
    let build = repo.main.join("target/debug/build");
    let before = tree(&build);
    let (out, r) = checkout_gc(&hive, &repo.main);
    assert!(out.status.success(), "{r}");
    assert_eq!(tree(&build), before, "build/ changed: {r}");
}

#[test]
fn an_undecided_target_deletes_no_unit_and_names_each_reason_with_what_clears_it() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let target = repo.main.join("target");
    let before = fingerprint_dirs(&target);
    let unit = newest_fingerprint_unit(&target);
    let dir = target.join("debug/.fingerprint").join(&unit);
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
    let deps = repo.main.join("target/debug/deps");
    fs::set_permissions(&deps, fs::Permissions::from_mode(0o500)).unwrap();
    let (out, r) = checkout_gc(&hive, &repo.main);
    let _ = fs::set_permissions(&deps, fs::Permissions::from_mode(0o755));
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
            .find(|p| {
                p.parent()
                    .and_then(|parent| parent.file_name())
                    .is_some_and(|name| name == ".fingerprint")
            })
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
}

#[test]
fn a_dsym_dir_of_a_dead_unit_is_deleted() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let target = repo.main.join("target");
    let dead = oldest_fingerprint_unit(&target);
    let hash = dead.rsplit('-').next().unwrap();
    let dsym = target.join("debug/deps").join(format!("unit-{hash}.dSYM"));
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
            path.parent()
                .and_then(|p| p.file_name())
                .is_some_and(|n| n == "deps")
                && path.is_file()
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
    let deps = repo.main.join("target/debug/deps");
    let orphan = deps.join("orphan-swappedobject1.0.rcgu.o");
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
fn a_unit_changed_between_scan_and_lock_is_judged_on_the_fresh_read() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let target = repo.main.join("target");
    let dead = oldest_fingerprint_unit(&target);
    let stamp = target
        .join("debug/.fingerprint")
        .join(&dead)
        .join("invoked.timestamp");
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
    let dir = target.join("debug/.fingerprint").join(&dead);
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
    let deps = debug.join("deps");
    let fs = cargo_hive_platform::current().unwrap().fs;
    let (_live, dead) = decide(&target);
    let doomed: std::collections::BTreeSet<_> = dead
        .iter()
        .flat_map(|unit| unit.files.iter().cloned())
        .collect();
    let listing =
        cargo_hive_gc::checkout::list_profile(fs.as_ref(), &debug, 1_000_000, 100_000).unwrap();
    let mut kept_names = std::collections::BTreeSet::new();
    for path in &listing.candidates {
        if doomed.contains(path) {
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
    let before_named: Vec<_> = kept_names
        .iter()
        .map(|name| deps.join(name))
        .filter(|p| p.is_file())
        .collect();
    assert!(!before_named.is_empty(), "named objects must exist on disk");
    let orphan = deps.join("orphan-deadbeefdeadbeef.0.rcgu.o");
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
    let deps = debug.join("deps");
    let it = named_under(&deps, "it-").expect("integration test binary");
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
    let orphan = deps.join("orphan-cafecafecafecafe.0.rcgu.o");
    fs::write(&orphan, b"orphan").unwrap();
    let (out, r) = checkout_gc(&hive, &repo.main);
    assert!(out.status.success(), "{r}");
    assert!(!orphan.exists(), "orphan remained: {r}");
    for name in &it_loose {
        assert!(
            deps.join(name).is_file(),
            "unrelinked test binary object {name} was deleted: {r}"
        );
    }
}

#[test]
fn objects_named_only_by_a_dead_units_binary_go_in_the_same_run() {
    let hive = Hive::ready();
    let repo = aged_binaries(&hive);
    let target = repo.main.join("target");
    let debug = target.join("debug");
    let deps = debug.join("deps");
    let (_live, dead) = decide(&target);
    let dead_hash = dead
        .iter()
        .find_map(|unit| {
            unit.unit.loc.name_meta.rsplit('-').next().and_then(|h| {
                (h.len() == 16 && h.chars().all(|c| c.is_ascii_hexdigit())).then_some(h.to_string())
            })
        })
        .expect("a dead unit with a 16-hex hash");
    let object_name = "only-dead-unit-object.0.rcgu.o";
    let object = deps.join(object_name);
    fs::write(&object, b"only-dead").unwrap();
    let binary = deps.join(format!("staleextra-{dead_hash}"));
    fs::write(&binary, thin_execute_bytes(&[object_name])).unwrap();
    assert!(
        decide(&target)
            .1
            .iter()
            .any(|unit| unit.files.iter().any(|p| p == &binary)),
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
    let deps = debug.join("deps");
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
    let named_files: Vec<_> = referenced
        .iter()
        .map(|name| deps.join(name))
        .filter(|p| p.is_file())
        .collect();
    assert!(!named_files.is_empty(), "no named objects on disk");
    let orphan = deps.join("orphan-clonedcloned00.0.rcgu.o");
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
    let deps = debug.join("deps");
    let unit = newest_fingerprint_unit(&target);
    let dir = debug.join(".fingerprint").join(&unit);
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
    let orphan = deps.join("orphan-undecided0001.0.rcgu.o");
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
    let deps = debug.join("deps");
    let unit = newest_fingerprint_unit(&repo.main.join("target"));
    let dir = debug.join(".fingerprint").join(&unit);
    fs::write(dir.join("extra-corrupt.json"), b"{}").unwrap();
    let orphan = deps.join("orphan-unsupported01.0.rcgu.o");
    fs::write(&orphan, b"orphan").unwrap();
    let objects_before = deps_rcgu_o(&deps);
    assert!(!objects_before.is_empty());
    let (out, r) = checkout_gc(&hive, &repo.main);
    assert_eq!(out.status.code(), Some(0), "{r}");
    assert!(orphan.is_file(), "unsupported profile lost an object: {r}");
    assert_eq!(
        deps_rcgu_o(&deps).len(),
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
    let deps = repo.main.join("target/debug/deps");
    fs::write(deps.join("orphan-countcount0001.0.rcgu.o"), b"orphan").unwrap();
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
    let deps = debug.join("deps");
    let bin = debug.join("app");
    assert!(bin.is_file(), "app binary missing");
    let orphan = deps.join("orphan-failread00001.0.rcgu.o");
    fs::write(&orphan, b"orphan").unwrap();
    let before = deps_rcgu_o(&deps);
    let (_fs, scan, checkout, nested) = library_scan(&hive, &repo);
    let rec = Recording::over_real().fail_read_at(&bin);
    let report = run_library(&rec, &scan, &checkout, &nested);
    assert!(orphan.is_file(), "read failure still deleted objects");
    assert_eq!(deps_rcgu_o(&deps).len(), before.len());
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
            unit.files.iter().any(|p| {
                p.parent()
                    .and_then(|parent| parent.file_name())
                    .is_some_and(|name| name == ".fingerprint")
            }) && unit.files.iter().any(|p| {
                p.parent()
                    .and_then(|parent| parent.file_name())
                    .is_some_and(|name| name != ".fingerprint")
            })
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
            .find(|p| {
                p.parent()
                    .and_then(|parent| parent.file_name())
                    .is_some_and(|name| name == ".fingerprint")
            })
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
    let fp = target.join("debug/.fingerprint");
    let mut out = Vec::new();
    for entry in fs::read_dir(&fp).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        if name.contains('-') {
            out.push(name);
        }
    }
    out.sort();
    out
}

fn oldest_fingerprint_unit(target: &Path) -> String {
    let fp = target.join("debug/.fingerprint");
    let mut best: Option<(SystemTime, String)> = None;
    for entry in fs::read_dir(&fp).unwrap() {
        let dir = entry.unwrap().path();
        let stamp = dir.join("invoked.timestamp");
        if !stamp.exists() {
            continue;
        }
        let t = fs::metadata(&stamp).unwrap().modified().unwrap();
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        if best.as_ref().is_none_or(|(b, _)| t < *b) {
            best = Some((t, name));
        }
    }
    best.expect("a fingerprint unit").1
}

fn newest_fingerprint_unit(target: &Path) -> String {
    let fp = target.join("debug/.fingerprint");
    let mut best: Option<(SystemTime, String)> = None;
    for entry in fs::read_dir(&fp).unwrap() {
        let dir = entry.unwrap().path();
        let stamp = dir.join("invoked.timestamp");
        if !stamp.exists() {
            continue;
        }
        let t = fs::metadata(&stamp).unwrap().modified().unwrap();
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        if best.as_ref().is_none_or(|(b, _)| t > *b) {
            best = Some((t, name));
        }
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
            if child.is_dir() {
                push_dir(&child);
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
