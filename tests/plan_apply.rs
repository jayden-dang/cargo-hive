//! `gc --plan` stores one plan per checkout class and deletes nothing in the checkout.
//! These tests drive the hive binary against a scratch home and read the receipt.
//! A plan is stored here and is not applied.

#![cfg(target_os = "macos")]
mod common;
use common::plan_fixture::{tree, Planned};
use common::{init_repo, register, Hive, Scratch};

fn write_caps(p: &Planned, disabled: serde_json::Value) {
    let release = cargo_hive::doctor::resolve_toolchain(&p.hive.home.path)
        .unwrap()
        .release;
    let caps = serde_json::json!({"toolchain_release": release, "disabled": disabled, "lock_check": 1, "failed_marker_check": 1});
    std::fs::write(p.hive.home.path.join("capabilities.json"), caps.to_string()).unwrap();
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
    let (out, _) = p.hive.json(&["gc", "--plan"]);
    assert_eq!(out.status.code(), Some(1));
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
    assert_eq!(p.hive.json(&["gc", "--plan"]).0.status.code(), Some(1));
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
