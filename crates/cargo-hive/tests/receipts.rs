#![allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod common;
use common::{Hive, Scratch, init_repo};
use std::fs;
use std::os::unix::fs::PermissionsExt;

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
    assert_eq!(receipt["schema"], 2);
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

fn receipt_lines(hive: &Hive) -> Vec<serde_json::Value> {
    let dir = hive.home.path.join("receipts");
    let mut lines = Vec::new();
    let Ok(entries) = fs::read_dir(&dir) else {
        return lines;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            continue;
        }
        for line in fs::read_to_string(path).unwrap().lines() {
            lines.push(serde_json::from_str(line).unwrap());
        }
    }
    lines
}

#[test]
fn a_refusal_appends_a_receipt_line() {
    let hive = Hive::new();
    let s = Scratch::new("refuse-log");
    let (out, receipt) = hive.json(&["repo", "add", s.path.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(receipt["command"], "repo add");
    assert_eq!(receipt_lines(&hive), vec![receipt]);
}

#[test]
fn a_state_read_failure_still_prints_one_receipt() {
    let hive = Hive::new();
    fs::write(hive.home.path.join("registry.json"), b"{").unwrap();
    let out = hive.run(&["repo", "list", "--json"]);
    let receipt: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{e}\nstdout {}\nstderr {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(receipt["command"], "repo list");
    assert!(
        receipt["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["kind"] == "error"),
        "{receipt}"
    );
    assert_eq!(receipt_lines(&hive), vec![receipt]);
}

#[test]
fn json_is_still_printed_when_the_receipt_log_cannot_be_written() {
    let hive = Hive::new();
    assert!(hive.run(&["repo", "list"]).status.success());
    let receipts = hive.home.path.join("receipts");
    fs::set_permissions(&receipts, fs::Permissions::from_mode(0o555)).unwrap();
    let out = hive.run(&["repo", "list", "--json"]);
    fs::set_permissions(&receipts, fs::Permissions::from_mode(0o755)).unwrap();
    let receipt: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{e}\nstdout {}\nstderr {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    assert_eq!(receipt["command"], "repo list");
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("could not log"));
}

#[test]
fn status_still_prints_a_receipt_when_the_snapshot_cannot_be_written() {
    let hive = Hive::ready();
    let s = Scratch::new("snap-ro");
    let repo = init_repo(&s.path.join("app"), "");
    assert!(
        hive.run(&["repo", "add", repo.to_str().unwrap()])
            .status
            .success()
    );
    let home = hive.home.path.clone();
    fs::set_permissions(&home, fs::Permissions::from_mode(0o555)).unwrap();
    let out = hive.run(&["status", "--json"]);
    fs::set_permissions(&home, fs::Permissions::from_mode(0o755)).unwrap();
    let receipt: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{e}\nstdout {}\nstderr {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    assert_eq!(receipt["command"], "status");
    assert_ne!(out.status.code(), Some(0));
    assert!(
        receipt["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["kind"] == "error"),
        "{receipt}"
    );
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

#[test]
fn an_unparseable_capabilities_file_ends_status_with_a_refusal_naming_doctor() {
    let hive = Hive::new();
    let path = hive.home.path.join("capabilities.json");
    let damaged = b"{";
    fs::write(&path, damaged).unwrap();
    let (out, receipt) = hive.json(&["status"]);
    assert_eq!(out.status.code(), Some(1));
    let detail = receipt["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["kind"] == "error")
        .expect("error finding")["detail"]
        .as_str()
        .unwrap();
    assert!(
        detail.contains(path.to_str().unwrap()) && detail.contains("cargo hive doctor"),
        "{detail}"
    );
    assert_eq!(fs::read(&path).unwrap(), damaged);
}

#[cfg(target_os = "macos")]
#[test]
fn doctor_rewrites_a_damaged_capabilities_file() {
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    let hive = Hive::new();
    let path = hive.home.path.join("capabilities.json");
    let damaged = b"{";
    fs::write(&path, damaged).unwrap();
    let child = hive
        .command(&["doctor", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let started = Instant::now();
    let mut child = child;
    while child.try_wait().unwrap().is_none() {
        assert!(
            started.elapsed() <= Duration::from_secs(300),
            "doctor did not finish within 300s"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let bytes = fs::read(&path).unwrap();
    assert_ne!(bytes, damaged);
    let caps: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(caps["toolchain_release"].as_str().is_some(), "{caps}");
}
