//! `gc --plan` stores one plan per checkout class and deletes nothing in the checkout.
//! `apply` deletes the dead files of a secondary plan. These tests drive the hive binary
//! against a scratch home and read the receipt.

#![cfg(target_os = "macos")]
mod common;
use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;

use common::plan_fixture::{tree, Planned};
use common::{init_repo, interrupt, register, wait_until_open, wait_within, Hive, Scratch};

fn write_caps(p: &Planned, disabled: serde_json::Value) {
    let release = cargo_hive::doctor::resolve_toolchain(&p.hive.home.path)
        .unwrap()
        .release;
    let caps = serde_json::json!({"toolchain_release": release, "disabled": disabled, "lock_check": 1, "failed_marker_check": 1});
    std::fs::write(p.hive.home.path.join("capabilities.json"), caps.to_string()).unwrap();
}

#[test]
fn planning_does_not_read_a_missing_or_huge_snapshot() {
    let p = Planned::new("plan-no-snapshot");
    let snap = p.hive.home.path.join("snapshot.json");
    let _ = std::fs::remove_file(&snap);
    let missing = p.plan();
    assert!(missing["plans"].is_array(), "{missing}");
    let huge = vec![b'x'; 8 * 1024 * 1024];
    std::fs::write(&snap, &huge).unwrap();
    let planned = p.plan();
    assert!(planned["plans"].is_array(), "{planned}");
    assert_eq!(std::fs::read(&snap).unwrap(), huge);
}

#[test]
fn gc_plan_stores_one_plan_per_class_and_deletes_nothing() {
    let p = Planned::new("plan-per-class");
    let before = (tree(&p.main_target()), tree(&p.worktree_target()));
    let snapshot = std::fs::read(p.hive.home.path.join("snapshot.json")).ok();
    let r = p.plan();
    assert_eq!(r["schema"], 3);
    let plans = r["plans"].as_array().unwrap();
    let mut classes: Vec<&str> = plans.iter().map(|x| x["class"].as_str().unwrap()).collect();
    classes.sort();
    assert_eq!(classes, ["protected", "secondary"]);
    for plan in plans {
        assert_eq!(plan["id"].as_str().unwrap().len(), 16);
        assert!(plan["outcome"].is_null());
        assert!(plan["paths"].as_u64().unwrap() > 0, "{plan}");
        assert!(
            plan["counts"]["freed_bytes"].as_u64().is_some(),
            "planned bytes are in the receipt: {plan}"
        );
        assert_eq!(plan["targets"].as_array().unwrap().len(), 1);
    }
    assert_eq!(
        (tree(&p.main_target()), tree(&p.worktree_target())),
        before,
        "gc --plan touched a file"
    );
    assert_eq!(
        std::fs::read(p.hive.home.path.join("snapshot.json")).ok(),
        snapshot,
        "plan tier must not touch the snapshot cache"
    );
}

#[test]
fn a_named_worktree_is_in_plan_scope_and_apply_scope_comes_from_the_plan() {
    let p = Planned::new("scope-worktree");
    let worktree = p.worktree.canonicalize().unwrap();
    let main = p.main.canonicalize().unwrap();
    let r = p.plan_for(&[worktree.to_str().unwrap()]);
    let planned = r["scope"]["checkouts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert!(
        planned
            .iter()
            .any(|path| path == worktree.to_str().unwrap()),
        "the named worktree was left out of scope: {r}"
    );
    let id = Planned::id_of(&r, "secondary");
    let (out, applied) = p.hive.json(&["apply", &id]);
    assert!(out.status.success(), "{applied}");
    let repos = applied["scope"]["repos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    let checkouts = applied["scope"]["checkouts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert!(
        repos.iter().any(|path| path == main.to_str().unwrap()),
        "apply scope dropped the plan's repo: {applied}"
    );
    assert!(
        checkouts
            .iter()
            .any(|path| path == worktree.to_str().unwrap()),
        "apply scope dropped the owner checkout: {applied}"
    );
}

#[test]
fn a_stored_plan_records_what_apply_must_compare_against() {
    let p = Planned::new("plan-recorded");
    let r = p.plan();
    let id = Planned::id_of(&r, "secondary");
    let stored = p.plans_json();
    let plan = stored["plans"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == id.as_str())
        .unwrap();
    assert_eq!(plan["window_days"], 7);
    assert_eq!(plan["class"], "secondary");
    assert!(plan["scope_repos"].is_array());
    let target = &plan["targets"][0];
    assert_eq!(
        target["path"].as_str().unwrap(),
        p.worktree_target()
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
    );
    assert!(target["owner"].as_str().is_some());
    let entry = &target["entries"][0];
    for key in ["path", "unit", "kind", "dev", "ino"] {
        assert!(!entry[key].is_null(), "entry lacks {key}: {entry}");
    }
    assert!(plan["planned"]["freed"].as_u64().is_some());
}

#[test]
fn a_target_with_no_dead_unit_is_reported_as_nothing_to_do() {
    let hive = Hive::ready();
    let s = Scratch::new("plan-nothing");
    let repo = init_repo(&s.path.join("main"), "");
    register(&hive, &repo);
    hive.build_crate(&repo, None);
    let (out, r) = hive.json(&["gc", "--plan"]);
    assert!(out.status.success());
    assert_eq!(r["plans"].as_array().unwrap().len(), 0);
    let nothing: Vec<&str> = r["nothing_to_do"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(nothing.len(), 1, "{r}");
    assert!(nothing[0].ends_with("/target"));
}

#[test]
fn a_target_the_graph_cannot_decide_is_reported_undecided_with_what_clears_it() {
    let hive = Hive::ready();
    let s = Scratch::new("plan-undecided");
    let repo = init_repo(&s.path.join("main"), "");
    register(&hive, &repo);
    hive.build_crate(&repo, None);
    let unit = repo.join("target/debug/.fingerprint/zz-0123456789abcdef");
    std::fs::create_dir_all(&unit).unwrap();
    std::fs::write(unit.join("invoked.timestamp"), b"").unwrap();
    std::fs::write(unit.join("output-lib-zz"), b"").unwrap();
    let (_, r) = hive.json(&["gc", "--plan"]);
    assert_eq!(r["plans"].as_array().unwrap().len(), 0, "{r}");
    let u = &r["undecided"][0];
    assert!(u["target"].as_str().unwrap().ends_with("/target"));
    assert!(
        !u["reasons"][0]["clears_when"].as_str().unwrap().is_empty(),
        "{r}"
    );
}

#[test]
fn plan_refuses_a_window_out_of_range_and_has_no_window_option() {
    let p = Planned::new("plan-window");
    p.hive.config(r#"{"gc_window_days": 0}"#);
    let (out, r) = p.hive.json(&["gc", "--plan"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(r["schema"], 3, "{r}");
    assert!(r.get("plans").is_some(), "{r}");
    assert!(
        !p.hive
            .run(&["gc", "--plan", "--window", "3"])
            .status
            .success(),
        "an option set the window"
    );
}

#[test]
fn a_target_shared_with_a_protected_checkout_goes_in_the_protected_plan() {
    let p = Planned::new("plan-shared");
    std::fs::create_dir_all(p.worktree.join(".cargo")).unwrap();
    std::fs::write(
        p.worktree.join(".cargo/config.toml"),
        format!("[build]\ntarget-dir = \"{}\"\n", p.main_target().display()),
    )
    .unwrap();
    p.hive.build_crate(&p.worktree, None);
    let r = p.plan();
    let main_target = p.main_target().canonicalize().unwrap();
    let in_plan = |class: &str| {
        r["plans"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|x| x["class"] == class)
            .any(|x| {
                x["targets"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|t| t.as_str().unwrap() == main_target.to_str().unwrap())
            })
    };
    assert!(in_plan("protected"), "{r}");
    assert!(!in_plan("secondary"), "{r}");
}

#[test]
fn early_plan_and_apply_refusals_are_schema_3_and_other_commands_stay_2() {
    let p = Planned::new("schema-early");
    std::fs::write(p.hive.home.path.join("config.json"), b"{").unwrap();
    let (out, r) = p.hive.json(&["gc", "--plan"]);
    assert_eq!(out.status.code(), Some(1), "{r}");
    assert_eq!(r["schema"], 3, "{r}");
    assert!(
        r.get("plans").is_some() && r.get("nothing_to_do").is_some(),
        "{r}"
    );
    let (out, r) = p.hive.json(&["apply", "0123456789abcdef"]);
    assert_eq!(out.status.code(), Some(1), "{r}");
    assert_eq!(r["schema"], 3, "{r}");
    let (_, r) = p.hive.json(&["status"]);
    assert_eq!(r["schema"], 2, "{r}");
    p.hive.config(r#"{"cache_roots":[]}"#);
    write_caps(&p, serde_json::json!({"deletion": ["d"]}));
    let (out, r) = p.hive.json(&["apply", "0123456789abcdef"]);
    assert_eq!(out.status.code(), Some(1), "{r}");
    assert_eq!(r["schema"], 3, "{r}");
    let (_, gc) = p.hive.json(&["gc", "--dry-run"]);
    assert_eq!(gc["schema"], 2, "{gc}");
}

#[test]
fn gc_plan_conflicts_with_dry_run_and_gc_without_plan_is_unchanged() {
    let p = Planned::new("plan-conflict");
    assert!(!p.hive.run(&["gc", "--plan", "--dry-run"]).status.success());
    let (_, r) = p.hive.json(&["gc", "--dry-run"]);
    assert_eq!(r["schema"], 2);
    assert!(r.get("plans").is_none());
}

#[test]
fn gc_plan_needs_only_the_fingerprint_capability() {
    let p = Planned::new("plan-capability");
    write_caps(&p, serde_json::json!({"fingerprint_reading": ["f"]}));
    let (out, r) = p.hive.json(&["gc", "--plan"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(r["schema"], 3, "{r}");
    write_caps(&p, serde_json::json!({"deletion": ["d"]}));
    assert_eq!(p.hive.json(&["gc", "--plan"]).0.status.code(), Some(0));
}

#[test]
fn without_the_accounting_capability_plans_are_made_and_the_bytes_are_unmeasured() {
    let p = Planned::new("plan-unmeasured");
    write_caps(&p, serde_json::json!({"eviction_accounting": ["c"]}));
    let (out, r) = p.hive.json(&["gc", "--plan"]);
    assert_eq!(out.status.code(), Some(0));
    let plans = r["plans"].as_array().unwrap();
    assert!(!plans.is_empty());
    for plan in plans {
        assert!(plan["counts"].get("freed_bytes").is_none(), "{plan}");
    }
    assert!(!r["unmeasured"].as_array().unwrap().is_empty(), "{r}");
}

#[test]
fn a_dead_unit_that_owns_no_artifact_is_never_planned_for_its_lone_fingerprint_dir() {
    let p = Planned::new("plan-no-artifact");
    let hash = p.dead_unit_hash(&p.worktree_target());
    let mut seen = 0usize;
    for e in std::fs::read_dir(p.worktree_target().join("debug/deps")).unwrap() {
        seen += 1;
        assert!(seen < 100_000, "walk bound hit");
        let path = e.unwrap().path();
        if path.to_str().unwrap().contains(&hash) {
            std::fs::remove_file(path).unwrap();
        }
    }
    let r = p.plan_for(&[p.worktree.to_str().unwrap()]);
    assert!(r["plans"].as_array().unwrap().is_empty(), "{r}");
    assert_eq!(r["nothing_to_do"].as_array().unwrap().len(), 1, "{r}");
}

fn removed_count(
    before: &std::collections::BTreeMap<std::path::PathBuf, (u64, std::time::SystemTime)>,
    after: &std::collections::BTreeMap<std::path::PathBuf, (u64, std::time::SystemTime)>,
) -> usize {
    before.keys().filter(|k| !after.contains_key(*k)).count()
}

#[test]
fn applying_a_secondary_plan_removes_its_dead_files_and_the_next_build_rebuilds_nothing_live() {
    let p = Planned::new("apply-secondary");
    let planned = p.plan();
    let id = Planned::id_of(&planned, "secondary");
    let listed = planned["plans"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["class"] == "secondary")
        .unwrap()["paths"]
        .as_u64()
        .unwrap() as usize;
    let main_before = tree(&p.main_target());
    let wt_before = tree(&p.worktree_target());
    let (out, r) = p.hive.json(&["apply", &id]);
    assert_eq!(out.status.code(), Some(0), "{r}");
    let plan = &r["plans"][0];
    assert_eq!(plan["outcome"], "applied");
    assert_eq!(
        plan["counts"]["paths_deleted"].as_u64().unwrap() as usize,
        listed
    );
    assert!(plan["counts"]["freed_bytes"].as_u64().is_some());
    assert_eq!(
        tree(&p.main_target()),
        main_before,
        "the protected target must not change"
    );
    let wt_after = tree(&p.worktree_target());
    assert!(removed_count(&wt_before, &wt_after) >= listed);
    p.hive.build_crate(&p.worktree, None);
    assert_eq!(
        tree(&p.worktree_target()).len(),
        wt_after.len(),
        "the next build rebuilt something"
    );
}

#[test]
fn only_the_dead_units_files_are_removed_and_incremental_and_live_files_stay() {
    let p = Planned::new("apply-only-dead");
    let hash = p.dead_unit_hash(&p.worktree_target());
    let id = Planned::id_of(&p.plan(), "secondary");
    let before = tree(&p.worktree_target());
    assert_eq!(p.hive.json(&["apply", &id]).0.status.code(), Some(0));
    let after = tree(&p.worktree_target());
    for gone in before.keys().filter(|k| !after.contains_key(*k)) {
        let s = gone.to_str().unwrap();
        assert!(
            s.contains(&hash),
            "{s} is not a file of the dead unit {hash}"
        );
        assert!(!s.contains("/incremental/"), "{s}");
    }
    for (path, meta) in &after {
        if std::fs::symlink_metadata(path).unwrap().is_dir() {
            continue;
        }
        assert_eq!(before.get(path), Some(meta), "{} changed", path.display());
    }
}

#[test]
fn an_applied_plan_cannot_be_applied_again() {
    let p = Planned::new("apply-once");
    let id = Planned::id_of(&p.plan(), "secondary");
    assert_eq!(p.hive.json(&["apply", &id]).0.status.code(), Some(0));
    let (out, r) = p.hive.json(&["apply", &id]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(r["plans"][0]["outcome"], "refused");
    assert_eq!(r["plans"][0]["reason"], "already_consumed");
}

#[test]
fn a_target_whose_build_lock_is_held_is_skipped_and_nothing_in_it_is_deleted() {
    let p = Planned::new("apply-held");
    let id = Planned::id_of(&p.plan(), "secondary");
    let lock = p.worktree_target().join("debug/.cargo-build-lock");
    let mut holder = p.hive.hold_lock_child("flock", &lock);
    let before = tree(&p.worktree_target());
    let (out, r) = p.hive.json(&["apply", &id]);
    let _ = holder.kill();
    let _ = holder.wait();
    assert_eq!(out.status.code(), Some(0), "{r}");
    let plan = &r["plans"][0];
    assert_eq!(plan["outcome"], "partial");
    assert_eq!(plan["reason"], "all_targets_skipped");
    assert_eq!(plan["counts"]["targets_skipped_held"], 1);
    assert_eq!(tree(&p.worktree_target()), before);
}

#[test]
fn apply_takes_exactly_one_id_and_refuses_an_unknown_one() {
    let p = Planned::new("apply-ids");
    assert!(!p.hive.run(&["apply"]).status.success());
    assert!(!p.hive.run(&["apply", "a", "b"]).status.success());
    let (out, r) = p.hive.json(&["apply", "0123456789abcdef"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(r["plans"][0]["reason"], "unknown_plan");
}

#[test]
fn apply_refuses_without_the_deletion_capability_and_leaves_the_plan_unconsumed() {
    let p = Planned::new("apply-capability");
    let id = Planned::id_of(&p.plan(), "secondary");
    write_caps(&p, serde_json::json!({"deletion": ["d"]}));
    assert_eq!(p.hive.json(&["apply", &id]).0.status.code(), Some(1));
    p.hive.write_ready_capabilities();
    assert_eq!(p.hive.json(&["apply", &id]).0.status.code(), Some(0));
}

#[test]
fn an_agent_applies_a_protected_plan_itself_and_the_plan_is_consumed() {
    let p = Planned::new("protected-apply");
    let id = Planned::id_of(&p.plan(), "protected");
    let before = tree(&p.main_target());
    let (out, r) = p.hive.json(&["apply", &id]);
    assert_eq!(out.status.code(), Some(0), "{r}");
    let plan = &r["plans"][0];
    assert_eq!(plan["outcome"], "applied");
    assert_eq!(plan["class"], "protected");
    assert_eq!(plan["id"], id.as_str());
    assert!(
        plan["counts"]["paths_deleted"].as_u64().unwrap() > 0,
        "{plan}"
    );
    assert!(tree(&p.main_target()).len() < before.len());
    let (again, r) = p.hive.json(&["apply", &id]);
    assert_eq!(again.status.code(), Some(1), "{r}");
    assert_eq!(r["plans"][0]["reason"], "already_consumed");
}

#[test]
fn apply_never_blocks_on_an_owner_authentication_prompt() {
    let p = Planned::new("no-dialog");
    let id = Planned::id_of(&p.plan(), "protected");
    let child = p
        .hive
        .command(&["apply", &id, "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out = common::wait_within(child, std::time::Duration::from_secs(60));
    assert_eq!(out.status.code(), Some(0));
}

#[test]
fn applying_two_targets_records_each_targets_counts_and_the_plan_totals() {
    let p = Planned::with_worktrees("apply-per-target", 2);
    let planned = p.plan();
    let secondary = planned["plans"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["class"] == "secondary")
        .unwrap();
    assert_eq!(secondary["targets"].as_array().unwrap().len(), 2);
    let id = Planned::id_of(&planned, "secondary");
    let (out, r) = p.hive.json(&["apply", &id]);
    assert_eq!(out.status.code(), Some(0), "{r}");
    let plan = &r["plans"][0];
    let counts = &plan["counts"];
    let mut paths = 0u64;
    let mut freed = 0u64;
    let mut shared = 0u64;
    for target in plan["targets"].as_array().unwrap() {
        let path = target.as_str().unwrap();
        let paths_key = format!("paths_deleted:{path}");
        let freed_key = format!("freed_bytes:{path}");
        let shared_key = format!("deleted_bytes_still_shared:{path}");
        let deleted = counts[paths_key.as_str()]
            .as_u64()
            .unwrap_or_else(|| panic!("missing {paths_key} in {counts}"));
        let freed_here = counts[freed_key.as_str()]
            .as_u64()
            .unwrap_or_else(|| panic!("missing {freed_key} in {counts}"));
        let shared_here = counts[shared_key.as_str()]
            .as_u64()
            .unwrap_or_else(|| panic!("missing {shared_key} in {counts}"));
        assert!(deleted > 0, "{path} deleted nothing");
        paths += deleted;
        freed += freed_here;
        shared += shared_here;
    }
    assert_eq!(counts["paths_deleted"].as_u64().unwrap(), paths);
    assert_eq!(counts["freed_bytes"].as_u64().unwrap(), freed);
    assert_eq!(
        counts["deleted_bytes_still_shared"].as_u64().unwrap(),
        shared
    );
}

fn refused(p: &Planned, id: &str, reason: &str) {
    let before = (tree(&p.main_target()), tree(&p.worktree_target()));
    let (out, r) = p.hive.json(&["apply", id]);
    assert_eq!(out.status.code(), Some(1), "{r}");
    assert_eq!(r["plans"][0]["outcome"], "refused");
    assert_eq!(r["plans"][0]["reason"], reason, "{r}");
    assert_eq!(
        (tree(&p.main_target()), tree(&p.worktree_target())),
        before,
        "a refused plan deleted something"
    );
    let (_, again) = p.hive.json(&["apply", id]);
    assert_eq!(
        again["plans"][0]["reason"], "already_consumed",
        "a refused plan stays consumed"
    );
}

#[test]
fn a_planned_unit_that_a_command_made_live_again_refuses_the_whole_plan() {
    let p = Planned::new("stale-live");
    let hash = p.dead_unit_hash(&p.worktree_target());
    let id = Planned::id_of(&p.plan(), "secondary");
    p.revive(&p.worktree_target(), &hash);
    refused(&p, &id, "stale_item");
}

#[test]
fn a_planned_path_that_vanished_refuses_the_plan() {
    let p = Planned::new("stale-entry");
    let hash = p.dead_unit_hash(&p.worktree_target());
    let id = Planned::id_of(&p.plan(), "secondary");
    let deps = p.worktree_target().join("debug/deps");
    let victim = std::fs::read_dir(&deps)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|f| f.to_str().unwrap().contains(&hash) && f.extension().is_some_and(|e| e == "rlib"))
        .unwrap();
    std::fs::remove_file(&victim).unwrap();
    refused(&p, &id, "entry_changed");
}

#[test]
fn a_unit_that_became_dead_after_the_plan_is_left_alone() {
    let p = Planned::new("stale-newly-dead");
    let id = Planned::id_of(&p.plan(), "secondary");
    common::cargo_fixture::backdate_all(
        &p.worktree_target(),
        std::time::Duration::from_secs(10 * 86_400),
    );
    let manifest = p.worktree.join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest)
        .unwrap()
        .replace("0.1.1", "0.1.2");
    std::fs::write(&manifest, text).unwrap();
    p.hive.build_crate(&p.worktree, None);
    let (out, r) = p.hive.json(&["apply", &id]);
    assert_eq!(out.status.code(), Some(0), "{r}");
    assert_eq!(r["plans"][0]["outcome"], "applied");
}

#[test]
fn a_directory_that_cannot_be_entered_under_a_secondary_root_does_not_refuse_apply() {
    let p = Planned::new("apply-cannot-enter");
    let id = Planned::id_of(&p.plan(), "secondary");
    let before = tree(&p.worktree_target());
    // The mode a semaphore directory under /private/tmp was found with.
    let sealed = p.scratch.path.join("sealed");
    std::fs::create_dir_all(&sealed).unwrap();
    std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o222)).unwrap();
    let (out, r) = p.hive.json(&["apply", &id]);
    std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(out.status.code(), Some(0), "{r}");
    assert_eq!(r["plans"][0]["outcome"], "applied", "{r}");
    assert_ne!(tree(&p.worktree_target()), before, "nothing was deleted");
    assert!(
        r["findings"].as_array().unwrap().iter().any(|f| {
            f["kind"] == "walk.unreadable" && f["path"].as_str().unwrap_or("").contains("sealed")
        }),
        "{r}"
    );
}

#[test]
fn a_checkout_walk_fault_refuses_apply_before_any_delete() {
    let p = Planned::new("apply-walk-fault");
    let id = Planned::id_of(&p.plan(), "secondary");
    let before = tree(&p.worktree_target());
    // Enterable but not listable: a build can still run in a checkout inside it.
    let blocked = p.scratch.path.join("blocked-checkouts");
    std::fs::create_dir_all(&blocked).unwrap();
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o111)).unwrap();
    let (out, r) = p.hive.json(&["apply", &id]);
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(out.status.code(), Some(1), "{r}");
    assert_eq!(r["plans"][0]["outcome"], "refused", "{r}");
    assert_eq!(tree(&p.worktree_target()), before);
    assert!(
        r["findings"].as_array().unwrap().iter().any(|f| {
            f["path"]
                .as_str()
                .unwrap_or("")
                .contains("blocked-checkouts")
                || f["detail"]
                    .as_str()
                    .unwrap_or("")
                    .contains("blocked-checkouts")
        }),
        "{r}"
    );
}

#[test]
fn a_target_scan_fault_refuses_apply_before_any_delete() {
    let p = Planned::new("apply-scan-fault");
    let id = Planned::id_of(&p.plan(), "secondary");
    let before = tree(&p.worktree_target());
    let blocked = p.worktree.join("unreadable-child");
    std::fs::create_dir_all(&blocked).unwrap();
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let (out, r) = p.hive.json(&["apply", &id]);
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(out.status.code(), Some(1), "{r}");
    assert_eq!(r["plans"][0]["outcome"], "refused", "{r}");
    assert_eq!(r["plans"][0]["reason"], "walk.unreadable", "{r}");
    assert_eq!(tree(&p.worktree_target()), before);
    assert!(
        r["findings"].as_array().unwrap().iter().any(|f| {
            f["path"]
                .as_str()
                .unwrap_or("")
                .contains("unreadable-child")
                || f["detail"]
                    .as_str()
                    .unwrap_or("")
                    .contains("unreadable-child")
        }),
        "{r}"
    );
}

#[test]
fn a_changed_class_refuses_the_plan() {
    let p = Planned::new("stale-class");
    let id = Planned::id_of(&p.plan(), "secondary");
    // with no secondary roots the worktree is no longer a secondary checkout
    p.hive.config(r#"{"secondary_roots": []}"#);
    refused(&p, &id, "class_changed");
}

#[test]
fn a_changed_owner_refuses_the_plan() {
    let p = Planned::new("stale-owner");
    let id = Planned::id_of(&p.plan(), "secondary");
    // the main checkout now builds into the worktree's target dir, and a protected owner wins
    std::fs::create_dir_all(p.main.join(".cargo")).unwrap();
    std::fs::write(
        p.main.join(".cargo/config.toml"),
        format!(
            "[build]\ntarget-dir = \"{}\"\n",
            p.worktree_target().display()
        ),
    )
    .unwrap();
    refused(&p, &id, "owner_changed");
}

#[test]
fn a_profile_with_an_unknown_fingerprint_format_refuses_the_plan() {
    let p = Planned::new("stale-format");
    let hash = p.dead_unit_hash(&p.worktree_target());
    let id = Planned::id_of(&p.plan(), "secondary");
    let unit = p
        .worktree_target()
        .join(format!("debug/.fingerprint/fixture-{hash}"));
    for entry in std::fs::read_dir(&unit).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "json") {
            std::fs::write(&path, r#"{"unexpected": true}"#).unwrap();
        }
    }
    refused(&p, &id, "unsupported_format");
}

#[test]
fn a_target_that_became_undecided_after_the_plan_refuses_it_with_undecided() {
    let p = Planned::new("stale-undecided");
    let id = Planned::id_of(&p.plan(), "secondary");
    let unit = p
        .worktree_target()
        .join("debug/.fingerprint/zz-0123456789abcdef");
    std::fs::create_dir_all(&unit).unwrap();
    std::fs::write(unit.join("invoked.timestamp"), b"").unwrap();
    std::fs::write(unit.join("output-lib-zz"), b"").unwrap();
    refused(&p, &id, "undecided");
}

#[test]
fn a_workspace_that_no_longer_resolves_refuses_the_plan_as_ownership_unresolved() {
    let p = Planned::new("stale-unresolved");
    let id = Planned::id_of(&p.plan(), "secondary");
    std::fs::write(p.worktree.join("Cargo.toml"), "this is not a manifest").unwrap();
    refused(&p, &id, "ownership_unresolved");
}

#[test]
fn a_stored_window_outside_1_to_365_is_refused_before_any_delete() {
    let p = Planned::new("apply-window");
    let id = Planned::id_of(&p.plan(), "secondary");
    let before = tree(&p.worktree_target());
    let mut stored = p.plans_json();
    for plan in stored["plans"].as_array_mut().unwrap() {
        if plan["id"] == id.as_str() {
            plan["window_days"] = serde_json::json!(0);
        }
    }
    std::fs::write(p.hive.home.path.join("plans.json"), stored.to_string()).unwrap();
    let (out, r) = p.hive.json(&["apply", &id]);
    assert_eq!(out.status.code(), Some(1), "{r}");
    assert_eq!(r["plans"][0]["outcome"], "refused");
    assert_eq!(r["plans"][0]["reason"], "window");
    assert_eq!(tree(&p.worktree_target()), before);
    let (again, second) = p.hive.json(&["apply", &id]);
    assert_eq!(again.status.code(), Some(1), "{second}");
    assert_eq!(
        second["plans"][0]["reason"], "window",
        "a refused window consumed the plan"
    );
}

#[test]
fn a_plan_edited_to_name_a_file_outside_its_target_is_refused_and_the_file_survives() {
    let p = Planned::new("stale-outside");
    let id = Planned::id_of(&p.plan(), "secondary");
    let outside = p.scratch.path.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let keep = outside.join("keep");
    std::fs::write(&keep, b"mine").unwrap();
    let md = std::fs::metadata(&keep).unwrap();
    use std::os::unix::fs::MetadataExt;
    let mut stored = p.plans_json();
    for plan in stored["plans"].as_array_mut().unwrap() {
        if plan["id"] == id.as_str() {
            let e = &mut plan["targets"][0]["entries"][0];
            e["path"] = serde_json::json!(keep.to_str().unwrap());
            e["kind"] = serde_json::json!("file");
            e["dev"] = serde_json::json!(md.dev());
            e["ino"] = serde_json::json!(md.ino());
        }
    }
    std::fs::write(p.hive.home.path.join("plans.json"), stored.to_string()).unwrap();
    let (out, r) = p.hive.json(&["apply", &id]);
    assert_eq!(out.status.code(), Some(1), "{r}");
    assert_eq!(r["plans"][0]["outcome"], "refused");
    assert!(keep.exists(), "a path outside the target was deleted");
}

#[test]
fn a_plan_whose_parent_is_a_symlink_is_refused_and_the_target_stays() {
    let p = Planned::new("stale-symlink");
    let id = Planned::id_of(&p.plan(), "secondary");
    let outside = p.scratch.path.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let keep = outside.join("keep");
    std::fs::write(&keep, b"mine").unwrap();
    let escape = p.worktree_target().join("debug/deps/escape");
    std::os::unix::fs::symlink(&outside, &escape).unwrap();
    let linked = escape.join("keep");
    let md = std::fs::metadata(&keep).unwrap();
    use std::os::unix::fs::MetadataExt;
    let mut stored = p.plans_json();
    for plan in stored["plans"].as_array_mut().unwrap() {
        if plan["id"] == id.as_str() {
            let e = &mut plan["targets"][0]["entries"][0];
            e["path"] = serde_json::json!(linked.to_str().unwrap());
            e["kind"] = serde_json::json!("file");
            e["dev"] = serde_json::json!(md.dev());
            e["ino"] = serde_json::json!(md.ino());
        }
    }
    std::fs::write(p.hive.home.path.join("plans.json"), stored.to_string()).unwrap();
    let before = tree(&p.worktree_target());
    let (out, r) = p.hive.json(&["apply", &id]);
    assert_eq!(out.status.code(), Some(1), "{r}");
    assert_eq!(r["plans"][0]["outcome"], "refused", "{r}");
    assert_eq!(
        tree(&p.worktree_target()),
        before,
        "apply changed a target whose plan named a symlinked parent"
    );
    assert_eq!(std::fs::read(&keep).unwrap(), b"mine");
}

fn pid_alive(pid: u32) -> bool {
    let child = std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    common::wait_within(child, std::time::Duration::from_secs(30))
        .status
        .success()
}

#[test]
fn a_sigint_during_apply_exits_130_logs_a_receipt_and_leaves_the_plan_consumed() {
    let p = Planned::new("apply-sigint");
    let id = Planned::id_of(&p.plan(), "secondary");
    let bin = p.scratch.path.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let sentinel = p.scratch.path.join("cargo.pid");
    std::fs::write(
        bin.join("cargo"),
        "#!/bin/sh\nprintf '%s\\n' \"$$\" > \"$HIVE_CARGO_SENTINEL\"\nsleep 300\n",
    )
    .unwrap();
    std::fs::set_permissions(bin.join("cargo"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let child = p
        .hive
        .command(&["apply", &id, "--json"])
        .env("PATH", &path)
        .env("HIVE_CARGO_SENTINEL", &sentinel)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let hive_pid = child.id();
    let started = std::time::Instant::now();
    loop {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(30),
            "cargo metadata never wrote its pid"
        );
        if std::fs::read_to_string(&sentinel).is_ok_and(|t| t.trim().parse::<u32>().is_ok()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(pid_alive(hive_pid), "apply should be running before SIGINT");
    let killer = std::process::Command::new("kill")
        .args(["-INT", &hive_pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    assert!(
        common::wait_within(killer, std::time::Duration::from_secs(30))
            .status
            .success()
    );
    let out = common::wait_within(child, std::time::Duration::from_secs(30));
    assert_eq!(out.status.code(), Some(130), "{out:?}");
    let mut seen = 0usize;
    let mut logged = false;
    for entry in std::fs::read_dir(p.hive.home.path.join("receipts")).unwrap() {
        seen += 1;
        assert!(seen < 100_000, "walk bound hit");
        let path = entry.unwrap().path();
        if std::fs::read_to_string(&path)
            .unwrap()
            .contains("\"command\":\"apply\"")
        {
            logged = true;
        }
    }
    assert!(logged, "a cancelled apply appends a receipt");
    let (again, r) = p.hive.json(&["apply", &id]);
    assert_eq!(again.status.code(), Some(1));
    assert_eq!(r["plans"][0]["reason"], "already_consumed");
}

#[test]
fn every_target_is_checked_before_any_is_deleted() {
    let p = Planned::with_worktrees("stale-two-targets", 2);
    let id = Planned::id_of(&p.plan(), "secondary");
    let second = p.more_worktrees[0].join("target");
    let hash = p.dead_unit_hash(&second);
    p.revive(&second, &hash);
    let first_before = tree(&p.worktree_target());
    let (out, r) = p.hive.json(&["apply", &id]);
    assert_eq!(out.status.code(), Some(1), "{r}");
    assert_eq!(
        tree(&p.worktree_target()),
        first_before,
        "the first target was deleted although the second was stale"
    );
}

#[test]
fn an_unowned_target_dir_becomes_one_whole_directory_entry_in_an_orphan_plan() {
    let (p, orphan) = Planned::with_orphan("orphan-plan");
    let before = tree(&orphan);
    let id = p.orphan_plan_id(&orphan);
    let stored = p.plans_json();
    let plan = stored["plans"]
        .as_array()
        .unwrap()
        .iter()
        .find(|x| x["id"] == id.as_str())
        .unwrap();
    assert_eq!(plan["class"], "orphan");
    let target = &plan["targets"][0];
    assert_eq!(
        target["entries"].as_array().unwrap().len(),
        1,
        "the target dir itself and nothing inside it"
    );
    assert_eq!(
        target["entries"][0]["path"].as_str().unwrap(),
        orphan.canonicalize().unwrap().to_str().unwrap()
    );
    let dirs = target["orphan"]["dirs"].as_array().unwrap();
    assert!(
        dirs.iter().any(|d| d.as_str().unwrap().ends_with("/debug")),
        "{target}"
    );
    for key in ["root_kind", "root_dev", "root_ino"] {
        assert!(!target["orphan"][key].is_null(), "{key}");
    }
    assert_eq!(tree(&orphan), before);
}

#[test]
fn no_other_class_plans_the_removal_of_a_whole_target_dir() {
    let (p, orphan) = Planned::with_orphan("orphan-only");
    p.orphan_plan_id(&orphan);
    let r = p.plan();
    for other in r["plans"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|x| x["class"] != "orphan")
    {
        assert!(
            !other["targets"].to_string().contains("gone-repo-target"),
            "{other}"
        );
    }
}

#[test]
fn an_agent_removes_an_orphan_target_itself() {
    let (p, orphan) = Planned::with_orphan("orphan-apply");
    let id = p.orphan_plan_id(&orphan);
    let (out, r) = p.hive.json(&["apply", &id]);
    assert_eq!(out.status.code(), Some(0), "{r}");
    assert_eq!(r["plans"][0]["outcome"], "applied");
    assert_eq!(r["plans"][0]["class"], "orphan");
    assert!(!orphan.exists());
}

#[test]
fn doctor_replaces_capabilities_recorded_with_the_retired_approval_check() {
    let p = Planned::new("retired-approval");
    let id = Planned::id_of(&p.plan(), "secondary");
    write_caps(&p, serde_json::json!({"approval": ["e"]}));
    assert_eq!(p.hive.json(&["apply", &id]).0.status.code(), Some(1));
    assert!(p.hive.run(&["doctor"]).status.success());
    let (out, r) = p.hive.json(&["apply", &id]);
    assert_eq!(out.status.code(), Some(0), "{r}");
}

#[test]
fn a_target_owned_by_a_repo_outside_the_named_scope_is_not_an_orphan() {
    let p = Planned::new("orphan-scope");
    // repo B (a second registered repo) is configured to build into a directory inside A's tree
    let b_dir = p.scratch.path.join("b");
    let b = init_repo(&b_dir, "");
    register(&p.hive, &b);
    let shared = p.main.join("shared-target");
    std::fs::create_dir_all(b.join(".cargo")).unwrap();
    std::fs::write(
        b.join(".cargo/config.toml"),
        format!("[build]\ntarget-dir = \"{}\"\n", shared.display()),
    )
    .unwrap();
    p.hive.build_crate(&b, None);
    let r = p.plan_for(&[p.main.to_str().unwrap()]); // only A is named
    assert!(
        r["plans"]
            .as_array()
            .unwrap()
            .iter()
            .all(|x| x["class"] != "orphan"),
        "{r}"
    );
    let shared_c = shared.canonicalize().unwrap();
    let listed = r["plans"].as_array().unwrap().iter().any(|x| {
        x["targets"]
            .to_string()
            .contains(shared_c.to_str().unwrap())
    });
    assert!(
        listed
            || r["nothing_to_do"]
                .to_string()
                .contains(shared_c.to_str().unwrap()),
        "the shared target is owned by B and planned or nothing-to-do, not orphan: {r}"
    );
}

#[test]
fn an_orphan_is_not_planned_while_a_registered_workspace_does_not_resolve() {
    let (p, orphan) = Planned::with_orphan("orphan-unresolved");
    std::fs::write(p.worktree.join("Cargo.toml"), "this is not a manifest").unwrap();
    p.hive.config(&format!(
        r#"{{"cache_roots": ["{}"]}}"#,
        orphan.parent().unwrap().display()
    ));
    let r = p.plan();
    assert!(
        r["plans"]
            .as_array()
            .unwrap()
            .iter()
            .all(|x| x["class"] != "orphan"),
        "an unresolved workspace means ownership is unknown: {r}"
    );
}

#[test]
fn an_orphan_owned_under_the_callers_environment_is_planned_with_a_note() {
    let (p, orphan) = Planned::with_orphan("orphan-caller-env");
    p.hive.config(&format!(
        r#"{{"cache_roots": ["{}"]}}"#,
        orphan.parent().unwrap().display()
    ));
    let (out, r) = p.hive.json_env(
        &["gc", "--plan"],
        &[("CARGO_TARGET_DIR", orphan.to_str().unwrap())],
    );
    assert!(out.status.success());
    assert!(
        r["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["kind"] == "plan.caller_env"),
        "{r}"
    );
    assert!(
        r["plans"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["class"] == "orphan"),
        "{r}"
    );
}

#[test]
fn an_interrupted_apply_waiting_for_the_state_lock_exits_130_without_waiting_for_the_holder() {
    let hive = Hive::ready();
    std::fs::write(hive.home.path.join(".lock"), b"").unwrap();
    let _holder = common::KillOnDrop(hive.hold_state_lock());
    let child = hive
        .command(&["apply", "p-0000000000000000", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // Apply's first state lock is the plan lookup.
    wait_until_open(child.id(), &hive.home.path.join(".lock"));
    interrupt(child.id());
    let out = wait_within(child, std::time::Duration::from_secs(10));
    assert_eq!(out.status.code(), Some(130), "{out:?}");
}
