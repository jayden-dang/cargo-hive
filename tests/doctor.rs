#![cfg(target_os = "macos")]
mod common;
use common::Hive;
use common::Scratch;
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

#[test]
fn a_doctor_error_still_prints_one_receipt() {
    let hive = Hive::new();
    let not_dir = hive.home.path.join("not-a-tmpdir");
    std::fs::write(&not_dir, b"x").unwrap();
    let out = hive
        .command(&["doctor", "--json"])
        .env("TMPDIR", &not_dir)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let receipt: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        panic!(
            "{e}\nstdout {}\nstderr {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(receipt["command"], "doctor");
    assert!(
        receipt["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["kind"] == "error"),
        "{receipt}"
    );
    let log = hive.home.path.join("receipts");
    let lines: Vec<String> = std::fs::read_dir(&log)
        .unwrap()
        .flat_map(|e| {
            std::fs::read_to_string(e.unwrap().path())
                .unwrap()
                .lines()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(lines.len(), 1, "{lines:?}");
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
fn check_d_reports_the_lock_matrix_and_passes_on_this_mac() {
    let hive = Hive::new();
    let (_, receipt) = doctor(&hive);
    let d = receipt["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == "d")
        .unwrap()
        .clone();
    assert_eq!(d["measured"].as_object().unwrap().len(), 8, "{d}");
    for (key, value) in d["measured"].as_object().unwrap() {
        assert_eq!(value, &serde_json::json!(true), "{key}: {d}");
    }
    assert_eq!(d["verdict"], "pass", "{d}");
    let caps: serde_json::Value =
        serde_json::from_slice(&std::fs::read(hive.home.path.join("capabilities.json")).unwrap())
            .unwrap();
    assert_eq!(caps["lock_check"], 1);
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

fn doctor_dirs(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("hive-doctor-")
        })
        .collect()
}

#[test]
fn doctor_leaves_no_temp_dir_behind_and_sweeps_dead_runs() {
    let hive = Hive::new();
    let tmp = Scratch::new("doctor-tmpdir");
    let mut dead = std::process::Command::new("true").spawn().unwrap();
    let dead_pid = dead.id();
    dead.wait().unwrap();
    std::fs::create_dir_all(tmp.path.join(format!("hive-doctor-{dead_pid}-leftover1"))).unwrap();
    let child = hive
        .command(&["doctor", "--json"])
        .env("TMPDIR", &tmp.path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out = wait_within(child, Duration::from_secs(300));
    let receipt: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(receipt["counts"]["stale_temp_dirs_removed"], 1);
    assert_eq!(doctor_dirs(&tmp.path), Vec::<std::path::PathBuf>::new());
}

#[test]
fn an_interrupted_doctor_exits_130_and_removes_its_temp_dir() {
    for signal in ["-TERM", "-INT"] {
        let hive = Hive::new();
        let tmp = Scratch::new("doctor-signal");
        let child = hive
            .command(&["doctor"])
            .env("TMPDIR", &tmp.path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id().to_string();
        // Wait until the probe crate exists, so the signal lands inside the build of check f.
        let started = Instant::now();
        while !doctor_dirs(&tmp.path)
            .iter()
            .any(|d| d.join("f-crate/Cargo.toml").exists())
        {
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "doctor never reached check f"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(std::process::Command::new("kill")
            .args([signal, &pid])
            .status()
            .unwrap()
            .success());
        let out = wait_within(child, Duration::from_secs(60));
        assert_eq!(out.status.code(), Some(130), "{signal}");
        assert_eq!(
            doctor_dirs(&tmp.path),
            Vec::<std::path::PathBuf>::new(),
            "{signal}"
        );
        assert!(
            !hive.home.path.join("capabilities.json").exists(),
            "{signal}: a cancelled doctor must not record capabilities"
        );
        let logged = std::fs::read_dir(hive.home.path.join("receipts"))
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| {
                std::fs::read_to_string(e.path())
                    .unwrap_or_default()
                    .contains("\"command\":\"doctor\"")
            });
        assert!(logged, "{signal}: a cancelled doctor appends a receipt");
    }
}
