mod common;
use common::{init_repo, Hive, Scratch};
use std::fs;

fn refusal(receipt: &serde_json::Value) -> String {
    receipt["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["kind"] == "refusal")
        .map(|f| f["detail"].as_str().unwrap().to_string())
        .unwrap_or_default()
}

fn registered(hive: &Hive, s: &Scratch) -> String {
    let repo = init_repo(&s.path.join("app"), "");
    assert!(hive
        .run(&["repo", "add", repo.to_str().unwrap()])
        .status
        .success());
    repo.display().to_string()
}

#[test]
fn status_without_doctor_results_is_refused_and_asks_for_doctor() {
    let hive = Hive::new();
    let s = Scratch::new("gate-none");
    let repo = registered(&hive, &s);
    let (out, receipt) = hive.json(&["status", &repo]);
    assert_eq!(out.status.code(), Some(1));
    assert!(refusal(&receipt).contains("cargo hive doctor"), "{receipt}");
    assert!(
        receipt["findings"]
            .as_array()
            .unwrap()
            .iter()
            .all(|f| !f["kind"].as_str().unwrap().starts_with("checkout.")),
        "nothing is scanned before the gate"
    );
}

#[test]
fn doctor_results_for_another_toolchain_are_refused() {
    let hive = Hive::new();
    let s = Scratch::new("gate-other");
    let repo = registered(&hive, &s);
    fs::write(
        hive.home.path.join("capabilities.json"),
        r#"{"toolchain_release":"0.0.0-elsewhere","disabled":{}}"#,
    )
    .unwrap();
    let (out, receipt) = hive.json(&["status", &repo]);
    assert_eq!(out.status.code(), Some(1));
    let why = refusal(&receipt);
    assert!(
        why.contains("cargo hive doctor") && why.contains("0.0.0-elsewhere"),
        "{why}"
    );
}

#[test]
fn a_disabled_capability_names_the_failing_doctor_check() {
    let hive = Hive::ready();
    let s = Scratch::new("gate-disabled");
    let repo = registered(&hive, &s);
    let path = hive.home.path.join("capabilities.json");
    let mut caps: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    caps["disabled"] = serde_json::json!({ "fingerprint_reading": ["f"] });
    fs::write(&path, caps.to_string()).unwrap();
    let (out, receipt) = hive.json(&["status", &repo]);
    assert_eq!(out.status.code(), Some(1));
    let why = refusal(&receipt);
    assert!(
        why.contains("(f)") && why.contains("fingerprint graph reading"),
        "{why}"
    );
}

#[test]
fn doctor_results_for_the_active_toolchain_let_status_run() {
    let hive = Hive::ready();
    let s = Scratch::new("gate-ok");
    let repo = registered(&hive, &s);
    let (out, receipt) = hive.json(&["status", &repo]);
    assert!(out.status.success(), "{receipt}");
    assert!(receipt["toolchain"].as_str().is_some());
}

/// Runs `status --json` with the variables the rustup proxy hands a cargo subcommand.
fn status_under_proxy(hive: &Hive, repo: &str, vars: &[(&str, &str)]) -> (bool, serde_json::Value) {
    let mut cmd = hive.command(&["status", "--no-sizes", repo, "--json"]);
    cmd.env_remove("RUSTUP_TOOLCHAIN")
        .env_remove("RUSTUP_TOOLCHAIN_SOURCE");
    for (key, value) in vars {
        cmd.env(key, value);
    }
    let out = cmd.output().unwrap();
    let receipt = serde_json::from_slice(&out.stdout).unwrap();
    (out.status.success(), receipt)
}

#[test]
fn a_toolchain_selected_by_the_callers_directory_does_not_reach_the_gate() {
    let hive = Hive::ready();
    let s = Scratch::new("gate-dir-pin");
    let repo = registered(&hive, &s);
    // `rust-toolchain.toml` and `rustup override set`, as the proxy reports them.
    for source in ["toolchain-file", "path-override"] {
        let (ok, receipt) = status_under_proxy(
            &hive,
            &repo,
            &[
                ("RUSTUP_TOOLCHAIN", "hive-test-not-installed"),
                ("RUSTUP_TOOLCHAIN_SOURCE", source),
            ],
        );
        assert!(ok, "{source}: {receipt}");
    }
}

#[test]
fn a_toolchain_of_unknown_origin_does_not_reach_the_gate() {
    let hive = Hive::ready();
    let s = Scratch::new("gate-no-source");
    let repo = registered(&hive, &s);
    // rustup before 1.29.0, and `rustup run`, set no source.
    let (ok, receipt) = status_under_proxy(
        &hive,
        &repo,
        &[("RUSTUP_TOOLCHAIN", "hive-test-not-installed")],
    );
    assert!(ok, "{receipt}");
}

#[test]
fn a_toolchain_the_caller_names_still_decides_the_gate() {
    let hive = Hive::ready();
    let s = Scratch::new("gate-named");
    let repo = registered(&hive, &s);
    // `cargo +<toolchain> hive …` and `RUSTUP_TOOLCHAIN` set in the shell.
    for source in ["cli", "env"] {
        let (ok, receipt) = status_under_proxy(
            &hive,
            &repo,
            &[
                ("RUSTUP_TOOLCHAIN", "hive-test-not-installed"),
                ("RUSTUP_TOOLCHAIN_SOURCE", source),
            ],
        );
        assert!(!ok, "{source}: the named toolchain was ignored: {receipt}");
        assert!(
            refusal(&receipt).contains("could not be determined"),
            "{source}: {receipt}"
        );
    }
}
