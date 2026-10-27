mod common;
use common::{init_repo, Hive, Scratch};
use std::fs;

const RECEIPT_KEYS: [&str; 12] = [
    "schema",
    "hive_version",
    "macos_version",
    "toolchain",
    "command",
    "ts",
    "scope",
    "counts",
    "checks",
    "findings",
    "unmeasured",
    "notes",
];

#[test]
fn json_mode_prints_exactly_one_receipt_and_nothing_else() {
    let hive = Hive::new();
    let (out, receipt) = hive.json(&["repo", "list"]);
    assert!(out.status.success());
    let obj = receipt.as_object().unwrap();
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort();
    let mut expected = RECEIPT_KEYS.to_vec();
    expected.sort();
    assert_eq!(keys, expected);
    assert_eq!(receipt["schema"], 1);
    assert_eq!(receipt["command"], "repo list");
    assert!(!receipt["hive_version"].as_str().unwrap().is_empty());
    assert!(
        receipt["macos_version"].as_str().is_some(),
        "sw_vers is available on the proof machine"
    );
    assert_eq!(receipt["counts"]["repos"], 0);
}

#[test]
fn every_command_appends_its_receipt_to_the_log() {
    let hive = Hive::new();
    let (_, first) = hive.json(&["repo", "list"]);
    let (_, second) = hive.json(&["repo", "list"]);
    let dir = hive.home.path.join("receipts");
    let files: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1);
    let lines: Vec<serde_json::Value> = fs::read_to_string(&files[0])
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines, vec![first, second]);
}

#[test]
fn a_fresh_machine_needs_no_configuration() {
    let hive = Hive::new();
    fs::remove_dir_all(&hive.home.path).unwrap();
    let out = hive.run(&["repo", "list"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(hive.home.path.join("receipts").is_dir());
    assert!(!hive.home.path.join("config.json").exists());
}

#[test]
fn scope_comes_from_arguments_never_from_the_working_directory() {
    let hive = Hive::new();
    let repos = Scratch::new("cwd");
    let repo = init_repo(&repos.path.join("r"), "");
    let out = hive
        .command(&["repo", "list", "--json"])
        .current_dir(&repo)
        .output()
        .unwrap();
    let receipt: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(receipt["scope"]["repos"], serde_json::json!([]));
    assert_eq!(receipt["scope"]["checkouts"], serde_json::json!([]));
}

#[test]
fn human_mode_prints_text_not_json() {
    let hive = Hive::new();
    let out = hive.run(&["repo", "list"]);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.contains("no repositories registered"), "{text}");
    assert!(serde_json::from_str::<serde_json::Value>(&text).is_err());
}
