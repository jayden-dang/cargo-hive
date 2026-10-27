#![cfg(target_os = "macos")]
mod common;
use common::Hive;
use std::process::{Child, Output, Stdio};
use std::time::{Duration, Instant};

/// Waits for `child` up to `limit`; a hang fails the test instead of blocking the suite.
pub fn wait_within(mut child: Child, limit: Duration) -> Output {
    let started = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > limit {
            let _ = child.kill();
            panic!("doctor did not finish within {limit:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child.wait_with_output().unwrap()
}

fn doctor(hive: &Hive) -> (Output, serde_json::Value) {
    let child = hive
        .command(&["doctor", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out = wait_within(child, Duration::from_secs(300));
    let receipt = serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&out.stderr)));
    (out, receipt)
}

const EXPECTED_IDS: [&str; 6] = ["a", "b", "c", "d", "e", "f"];

#[test]
fn doctor_reports_every_check_with_its_measured_value_and_records_capabilities() {
    let hive = Hive::new();
    let (out, receipt) = doctor(&hive);
    let checks = receipt["checks"].as_array().unwrap();
    let ids: Vec<&str> = checks.iter().map(|c| c["id"].as_str().unwrap()).collect();
    assert_eq!(ids, EXPECTED_IDS);
    for c in checks {
        assert!(
            ["pass", "fail", "unavailable"].contains(&c["verdict"].as_str().unwrap()),
            "{c}"
        );
        assert!(!c["measured"].as_object().unwrap().is_empty(), "{c}");
        if c["id"] != "e" {
            assert_eq!(
                c["verdict"], "pass",
                "on the proof machine every filesystem check passes: {c}"
            );
        }
    }
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let caps: serde_json::Value =
        serde_json::from_slice(&std::fs::read(hive.home.path.join("capabilities.json")).unwrap())
            .unwrap();
    assert_eq!(caps["toolchain_release"], receipt["toolchain"]);
    assert!(receipt["toolchain"].as_str().is_some());
}

#[test]
fn doctor_finishes_with_no_terminal_attached() {
    // `doctor()` spawns with stdin from /dev/null and no TTY; `wait_within` fails on a hang.
    let hive = Hive::new();
    let (_, receipt) = doctor(&hive);
    let f = receipt["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "f")
        .unwrap()
        .clone();
    assert_eq!(f["verdict"], "pass", "{f}");
    assert!(f["measured"]["units"].as_u64().unwrap() >= 4, "{f}");
    assert_eq!(f["measured"]["edges_unresolved"], 0, "{f}");
    assert_eq!(f["measured"]["edges_ambiguous"], 0, "{f}");
}
