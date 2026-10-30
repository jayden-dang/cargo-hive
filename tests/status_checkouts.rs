mod common;
use common::{git, init_repo, register, Hive, Scratch};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn class_of(receipt: &serde_json::Value, path: &Path) -> String {
    receipt["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            f["path"] == serde_json::json!(path)
                && f["kind"].as_str().unwrap().starts_with("checkout.")
        })
        .map(|f| {
            f["kind"]
                .as_str()
                .unwrap()
                .trim_start_matches("checkout.")
                .to_string()
        })
        .unwrap_or_else(|| "absent".into())
}

#[test]
fn main_worktrees_and_clones_are_listed_with_their_class() {
    let hive = Hive::ready();
    let s = Scratch::new("classes");
    let sec = s.path.join("sec");
    std::fs::create_dir_all(&sec).unwrap();
    hive.config(&format!("{{\"secondary_roots\": [\"{}\"]}}", sec.display()));
    let main = init_repo(&s.path.join("main"), "");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            main.join(".worktrees/lane1").to_str().unwrap(),
        ],
    );
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            s.path.join("sibling").to_str().unwrap(),
        ],
    );
    git(&sec, &["clone", "-q", main.to_str().unwrap(), "run1/app"]);
    register(&hive, &main);
    let (out, receipt) = hive.json(&["status", main.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(class_of(&receipt, &main), "protected");
    assert_eq!(
        class_of(&receipt, &main.join(".worktrees/lane1")),
        "secondary"
    );
    assert_eq!(class_of(&receipt, &s.path.join("sibling")), "protected");
    assert_eq!(class_of(&receipt, &sec.join("run1/app")), "secondary");
    assert_eq!(receipt["counts"]["checkouts"], 4);
}

#[test]
fn a_missing_worktree_is_listed_as_missing_even_when_locked() {
    let hive = Hive::ready();
    let s = Scratch::new("missing");
    let main = init_repo(&s.path.join("main"), "");
    let gone = main.join(".worktrees/gone");
    git(&main, &["worktree", "add", "-q", gone.to_str().unwrap()]);
    git(&main, &["worktree", "lock", gone.to_str().unwrap()]);
    std::fs::remove_dir_all(&gone).unwrap();
    register(&hive, &main);
    let (_, receipt) = hive.json(&["status", main.to_str().unwrap()]);
    assert_eq!(class_of(&receipt, &gone), "missing");
    assert_eq!(receipt["counts"]["missing"], 1);
    let at_gone: Vec<&str> = receipt["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["path"] == serde_json::json!(gone))
        .map(|f| f["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        at_gone,
        vec!["checkout.missing"],
        "a missing worktree appears in no other result"
    );
    let counts = &receipt["counts"];
    assert_eq!(
        counts["protected"].as_u64().unwrap() + counts["secondary"].as_u64().unwrap(),
        counts["checkouts"].as_u64().unwrap() - 1
    );
}

#[test]
fn secondary_roots_are_compared_as_realpaths() {
    let hive = Hive::ready();
    let s = Scratch::new("realpath");
    let real_root = s.path.join("real-sec");
    std::fs::create_dir_all(&real_root).unwrap();
    let alias = s.path.join("alias-sec");
    std::os::unix::fs::symlink(&real_root, &alias).unwrap();
    hive.config(&format!(
        "{{\"secondary_roots\": [\"{}\"]}}",
        alias.display()
    ));
    let main = init_repo(&s.path.join("main"), "");
    git(&real_root, &["clone", "-q", main.to_str().unwrap(), "app"]);
    register(&hive, &main);
    let (_, receipt) = hive.json(&["status", main.to_str().unwrap()]);
    assert_eq!(class_of(&receipt, &real_root.join("app")), "secondary");
}

#[test]
fn without_configuration_private_tmp_is_a_secondary_root() {
    let hive = Hive::ready();
    let s = Scratch::new("defaults"); // lives under /private/tmp
    let main = init_repo(&s.path.join("main"), "");
    git(&s.path, &["clone", "-q", main.to_str().unwrap(), "clone"]);
    register(&hive, &main);
    let (_, receipt) = hive.json(&["status", main.to_str().unwrap()]);
    // s.path is level 1 below /private/tmp, so its `clone` is level 2: inside the walk.
    assert_eq!(class_of(&receipt, &s.path.join("clone")), "secondary");
    assert!(receipt["notes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n.as_str().unwrap().contains("/private/tmp")));
}

#[test]
fn configured_secondary_roots_replace_the_defaults() {
    let hive = Hive::ready();
    let s = Scratch::new("replace");
    let other = s.path.join("other");
    std::fs::create_dir_all(&other).unwrap();
    hive.config(&format!(
        "{{\"secondary_roots\": [\"{}\"]}}",
        other.display()
    ));
    let main = init_repo(&s.path.join("main"), "");
    let lane = s.path.join("lane");
    git(&main, &["worktree", "add", "-q", lane.to_str().unwrap()]);
    register(&hive, &main);
    let (_, receipt) = hive.json(&["status", main.to_str().unwrap()]);
    assert_eq!(
        class_of(&receipt, &lane),
        "protected",
        "a sibling worktree under /private/tmp is protected once the defaults are replaced"
    );
}

#[test]
fn status_without_arguments_reports_every_registered_repo_from_any_cwd() {
    let hive = Hive::ready();
    let s = Scratch::new("all");
    let a = init_repo(&s.path.join("a"), "");
    let b = init_repo(&s.path.join("b"), "");
    register(&hive, &a);
    register(&hive, &b);
    let out = hive
        .command(&["status", "--json"])
        .current_dir(&a)
        .output()
        .unwrap();
    let receipt: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let mut repos: Vec<String> = receipt["scope"]["repos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().unwrap().to_string())
        .collect();
    repos.sort();
    assert_eq!(
        repos,
        vec![a.display().to_string(), b.display().to_string()]
    );
}

#[test]
fn discovery_stops_at_two_levels_and_never_enters_node_modules() {
    let hive = Hive::ready();
    let s = Scratch::new("bounded");
    let sec = s.path.join("sec");
    std::fs::create_dir_all(sec.join("a/b")).unwrap();
    std::fs::create_dir_all(sec.join("node_modules")).unwrap();
    hive.config(&format!("{{\"secondary_roots\": [\"{}\"]}}", sec.display()));
    let main = init_repo(&s.path.join("main"), "");
    git(
        &sec.join("a/b"),
        &["clone", "-q", main.to_str().unwrap(), "deep"],
    );
    git(
        &sec.join("node_modules"),
        &["clone", "-q", main.to_str().unwrap(), "pkg"],
    );
    git(&sec, &["clone", "-q", main.to_str().unwrap(), "near"]);
    register(&hive, &main);
    let (_, receipt) = hive.json(&["status", main.to_str().unwrap()]);
    assert_eq!(class_of(&receipt, &sec.join("near")), "secondary");
    assert_eq!(class_of(&receipt, &sec.join("a/b/deep")), "absent");
    assert_eq!(class_of(&receipt, &sec.join("node_modules/pkg")), "absent");
}

#[test]
fn an_unreadable_worktree_is_protected_and_not_missing() {
    let hive = Hive::ready();
    let s = Scratch::new("unreadable-wt");
    let main = init_repo(&s.path.join("main"), "");
    let lane = main.join(".worktrees/lane");
    git(&main, &["worktree", "add", "-q", lane.to_str().unwrap()]);
    register(&hive, &main);
    let parent = main.join(".worktrees");
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o000)).unwrap();
    let (out, receipt) = hive.json(&["status", main.to_str().unwrap()]);
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(class_of(&receipt, &lane), "protected");
    assert_eq!(receipt["counts"]["missing"], 0);
    let detail = receipt["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == serde_json::json!(lane))
        .unwrap()["detail"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        detail.to_lowercase().contains("permission") || detail.contains("denied"),
        "{detail}"
    );
    assert!(!detail.contains("does not exist"), "{detail}");
}

#[test]
fn a_secondary_checkout_whose_git_metadata_fails_is_protected() {
    let hive = Hive::ready();
    let s = Scratch::new("sec-git");
    let sec = s.path.join("sec");
    std::fs::create_dir_all(&sec).unwrap();
    hive.config(&format!("{{\"secondary_roots\": [\"{}\"]}}", sec.display()));
    let main = init_repo(&s.path.join("main"), "");
    git(&sec, &["clone", "-q", main.to_str().unwrap(), "app"]);
    let clone = sec.join("app");
    std::fs::set_permissions(clone.join(".git"), std::fs::Permissions::from_mode(0o000)).unwrap();
    register(&hive, &main);
    let (_, receipt) = hive.json(&["status", main.to_str().unwrap()]);
    std::fs::set_permissions(clone.join(".git"), std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(class_of(&receipt, &clone), "protected");
    let detail = receipt["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["kind"] == "checkout.protected" && f["path"] == serde_json::json!(clone))
        .unwrap()["detail"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(
        detail.to_lowercase().contains("permission")
            || detail.contains("denied")
            || detail.contains("git"),
        "{detail}"
    );
    assert_eq!(receipt["counts"]["missing"], 0);
}

#[test]
fn an_unreadable_directory_is_reported_and_the_walk_continues() {
    let hive = Hive::ready();
    let s = Scratch::new("walk");
    let sec = s.path.join("sec");
    let hidden = sec.join("hidden");
    std::fs::create_dir_all(&hidden).unwrap();
    hive.config(&format!("{{\"secondary_roots\": [\"{}\"]}}", sec.display()));
    let main = init_repo(&s.path.join("main"), "");
    let blocked = main.join("blocked");
    std::fs::create_dir_all(&blocked).unwrap();
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();
    std::fs::set_permissions(&hidden, std::fs::Permissions::from_mode(0o000)).unwrap();
    register(&hive, &main);
    let (_, receipt) = hive.json(&["status", "--no-sizes", main.to_str().unwrap()]);
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::set_permissions(&hidden, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(class_of(&receipt, &main), "protected");
    let details: Vec<String> = receipt["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["kind"] == "walk.unreadable")
        .map(|f| f["detail"].as_str().unwrap().to_string())
        .collect();
    assert!(
        details
            .iter()
            .any(|d| d.to_lowercase().contains("permission") || d.contains("denied")),
        "{details:?} {receipt}"
    );
}

#[test]
fn an_unregistered_repo_argument_is_refused() {
    let hive = Hive::ready();
    let s = Scratch::new("unknown");
    let (out, receipt) = hive.json(&["status", s.path.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(receipt["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["kind"] == "refusal"));
}
