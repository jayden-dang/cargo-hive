//! Receipt `hive_version` matches `cargo-hive --version`.

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod common;

use common::Hive;
use std::process::Command;

#[test]
fn hive_version_matches_the_binary_version() {
    let bin = env!("CARGO_BIN_EXE_cargo-hive");
    let out = Command::new(bin)
        .arg("--version")
        .output()
        .expect("cargo-hive --version");
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    let version = stdout
        .split_whitespace()
        .nth(1)
        .unwrap_or_else(|| panic!("no version word in {stdout:?}"));

    let hive = Hive::new();
    let out = hive
        .command(&["doctor", "--json"])
        .output()
        .expect("cargo-hive doctor --json");
    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let receipt: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("doctor --json receipt");
    assert_eq!(
        receipt["hive_version"].as_str(),
        Some(version),
        "receipt hive_version must equal cargo-hive --version"
    );
}
