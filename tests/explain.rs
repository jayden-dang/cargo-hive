mod common;
use common::{init_repo, register, wait_within, Hive, Scratch};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const TAG: &str = "# This file is a cache directory tag created by cargo.";

fn explain(hive: &Hive, args: &[&str]) -> (std::process::Output, serde_json::Value) {
    let mut all = vec!["explain"];
    all.extend_from_slice(args);
    hive.json(&all)
}

fn findings<'a>(receipt: &'a serde_json::Value, kind: &str) -> Vec<&'a serde_json::Value> {
    receipt["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|finding| finding["kind"] == kind)
        .collect()
}

fn finding_at<'a>(
    receipt: &'a serde_json::Value,
    kind: &str,
    path: &Path,
) -> Option<&'a serde_json::Value> {
    receipt["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["kind"] == kind && finding["path"] == serde_json::json!(path))
}

fn measured_u64(finding: &serde_json::Value, key: &str) -> u64 {
    finding["measured"][key]
        .as_u64()
        .unwrap_or_else(|| panic!("no measured {key} in {finding}"))
}

fn tag_target(dir: &Path) {
    fs::create_dir_all(dir.join("debug/.fingerprint")).unwrap();
    fs::create_dir_all(dir.join("debug/deps")).unwrap();
    fs::write(
        dir.join("CACHEDIR.TAG"),
        format!("Signature: 8a477f597d28d172789f06886806bc55\n{TAG}\n"),
    )
    .unwrap();
}

fn write_unit(profile: &Path, dir_crate: &str, n: u64) {
    let hash = cargo_hive::fingerprint::le_hex(n);
    let dir = profile
        .join(".fingerprint")
        .join(format!("{dir_crate}-{hash}"));
    fs::create_dir_all(&dir).unwrap();
    let stem = format!("lib-{dir_crate}");
    fs::write(dir.join(&stem), &hash).unwrap();
    fs::write(
        dir.join(format!("{stem}.json")),
        "{\"rustc\":11722972603611519644,\"features\":\"[]\",\"declared_features\":\"[]\",\"target\":1,\"profile\":2,\"path\":3,\"deps\":[],\"local\":[],\"rustflags\":[],\"config\":4,\"compile_kind\":0}",
    )
    .unwrap();
}

fn disable(hive: &Hive, capability: &str, check: &str) {
    let path = hive.home.path.join("capabilities.json");
    let mut caps: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    caps["disabled"][capability] = serde_json::json!([check]);
    fs::write(&path, caps.to_string()).unwrap();
}

fn pid_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[test]
fn explain_lists_copies_of_a_crate_built_two_ways() {
    let hive = Hive::ready();
    let s = Scratch::new("explain-copies");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_crate(&repo, None);
    hive.check_crate(&repo);
    register(&hive, &repo);
    let (out, receipt) = explain(&hive, &[repo.to_str().unwrap()]);
    assert!(out.status.success(), "{receipt}");
    let lib: Vec<_> = findings(&receipt, "copies.group")
        .into_iter()
        .filter(|finding| {
            finding["measured"]["crate"] == "fixture" && finding["measured"]["target_kind"] == "lib"
        })
        .collect();
    assert_eq!(lib.len(), 1, "{receipt}");
    assert_eq!(measured_u64(lib[0], "mode_build"), 1);
    assert_eq!(measured_u64(lib[0], "mode_check"), 1);
}

#[test]
fn explain_names_a_failed_unit_with_its_code() {
    let hive = Hive::ready();
    let s = Scratch::new("explain-failed");
    let repo = init_repo(&s.path.join("app"), "");
    fs::write(
        repo.join("src/lib.rs"),
        "pub fn answer() -> u32 { \"no\" }\n",
    )
    .unwrap();
    hive.build_crate_failing(&repo);
    register(&hive, &repo);
    let (out, receipt) = explain(&hive, &[repo.to_str().unwrap()]);
    assert!(out.status.success(), "{receipt}");
    let failed = findings(&receipt, "unit.failed");
    assert_eq!(failed.len(), 1, "{receipt}");
    assert_eq!(failed[0]["measured"]["error_codes"], "E0308");
    let failed_units: u64 = findings(&receipt, "copies.totals")
        .into_iter()
        .map(|finding| measured_u64(finding, "failed_units"))
        .sum();
    assert_eq!(failed_units, 1, "{receipt}");
}

#[test]
fn explain_without_check_g_counts_but_does_not_name() {
    let hive = Hive::ready();
    let s = Scratch::new("explain-no-g");
    let repo = init_repo(&s.path.join("app"), "");
    fs::write(
        repo.join("src/lib.rs"),
        "pub fn answer() -> u32 { \"no\" }\n",
    )
    .unwrap();
    hive.build_crate_failing(&repo);
    register(&hive, &repo);
    let path = hive.home.path.join("capabilities.json");
    let mut caps: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    caps.as_object_mut().unwrap().remove("failed_marker_check");
    fs::write(&path, caps.to_string()).unwrap();
    let (out, receipt) = explain(&hive, &[repo.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "{receipt}");
    assert!(findings(&receipt, "unit.failed").is_empty(), "{receipt}");
    assert!(
        receipt["unmeasured"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| {
                entry["field"] == "failed_units"
                    && entry["reason"]
                        .as_str()
                        .unwrap()
                        .contains("check (g) has not run")
            }),
        "{receipt}"
    );
    let unknown: u64 = findings(&receipt, "copies.totals")
        .into_iter()
        .map(|finding| measured_u64(finding, "compiling_or_unknown_units"))
        .sum();
    assert!(unknown >= 1, "{receipt}");
    assert!(findings(&receipt, "copies.totals")
        .iter()
        .all(|finding| finding["measured"].get("failed_units").is_none()));
}

#[test]
fn explain_scope_follows_the_arguments() {
    let hive = Hive::ready();
    let s = Scratch::new("explain-scope");
    let cache = s.path.join("cache");
    hive.config(&format!("{{\"cache_roots\": [\"{}\"]}}", cache.display()));
    let a = init_repo(&s.path.join("a"), "");
    let b = init_repo(&s.path.join("b"), "");
    hive.build_crate(&a, None);
    hive.build_crate(&b, None);
    hive.build_crate(&a, Some(&cache.join("old")));
    register(&hive, &a);
    register(&hive, &b);
    let (out, receipt) = explain(&hive, &[a.to_str().unwrap()]);
    assert!(out.status.success(), "{receipt}");
    let reports = |root: &Path| {
        receipt["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| {
                let kind = finding["kind"].as_str().unwrap();
                let about_a_target = kind.starts_with("copies.")
                    || kind.starts_with("target.")
                    || kind.starts_with("profile.")
                    || kind == "unit.failed";
                about_a_target
                    && finding["path"]
                        .as_str()
                        .is_some_and(|path| Path::new(path).starts_with(root))
            })
    };
    assert!(reports(&a.join("target")), "{receipt}");
    assert!(
        !reports(&b.join("target")),
        "explain <a> reported b: {receipt}"
    );
    assert!(
        !reports(&cache),
        "explain <a> reported a cache-root target: {receipt}"
    );
}

#[test]
fn explain_refuses_an_unregistered_argument_and_continues() {
    let hive = Hive::ready();
    let s = Scratch::new("explain-refuse-arg");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_crate(&repo, None);
    register(&hive, &repo);
    let nope = s.path.join("nope");
    fs::create_dir_all(&nope).unwrap();
    let (out, receipt) = explain(&hive, &[nope.to_str().unwrap(), repo.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1), "{receipt}");
    assert!(
        findings(&receipt, "refusal")
            .iter()
            .any(|finding| finding["path"] == serde_json::json!(nope)),
        "{receipt}"
    );
    assert!(
        finding_at(&receipt, "copies.totals", &repo.join("target")).is_some(),
        "{receipt}"
    );
}

#[test]
fn explain_refuses_without_fingerprint_reading() {
    let hive = Hive::ready();
    let s = Scratch::new("explain-no-read");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_crate(&repo, None);
    register(&hive, &repo);
    disable(&hive, "fingerprint_reading", "f");
    let (out, receipt) = explain(&hive, &[repo.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1), "{receipt}");
    let why = findings(&receipt, "refusal")
        .first()
        .map(|finding| finding["detail"].as_str().unwrap().to_string())
        .unwrap_or_default();
    assert!(
        why.contains("(f)") && why.contains("fingerprint graph reading"),
        "{why}"
    );
    assert!(findings(&receipt, "copies.group").is_empty(), "{receipt}");
    assert!(findings(&receipt, "copies.totals").is_empty(), "{receipt}");
    assert!(findings(&receipt, "unit.failed").is_empty(), "{receipt}");
}

#[test]
fn explain_does_not_need_deletion() {
    let hive = Hive::ready();
    let s = Scratch::new("explain-no-delete");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_crate(&repo, None);
    register(&hive, &repo);
    disable(&hive, "deletion", "d");
    let (out, receipt) = explain(&hive, &[repo.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "{receipt}");
    assert!(
        finding_at(&receipt, "copies.totals", &repo.join("target")).is_some(),
        "{receipt}"
    );
}

#[test]
fn an_unsupported_profile_is_named_and_skipped() {
    let hive = Hive::ready();
    let s = Scratch::new("explain-unsupported");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_crate(&repo, None);
    let profile = repo.join("target/debug");
    let unit = fs::read_dir(profile.join(".fingerprint"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("fixture-")
        })
        .unwrap();
    let json = fs::read_dir(&unit)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|ext| ext == "json"))
        .unwrap();
    let text = fs::read_to_string(&json)
        .unwrap()
        .replacen('{', "{\"version\":1,", 1);
    fs::write(&json, text).unwrap();
    register(&hive, &repo);
    let (out, receipt) = explain(&hive, &[repo.to_str().unwrap()]);
    assert!(out.status.success(), "{receipt}");
    let named = finding_at(&receipt, "profile.unsupported", &profile).unwrap_or_else(|| {
        panic!("profile was not named: {receipt}");
    });
    assert!(!named["detail"].as_str().unwrap().is_empty(), "{receipt}");
    assert!(
        findings(&receipt, "copies.group")
            .iter()
            .all(|finding| finding["path"] != serde_json::json!(profile)),
        "{receipt}"
    );
}

#[test]
fn an_unreadable_target_is_named_and_skipped() {
    let hive = Hive::ready();
    let s = Scratch::new("explain-unreadable");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_crate(&repo, None);
    let extra = repo.join("extra");
    tag_target(&extra);
    write_unit(&extra.join("debug"), "widget", 1);
    write_unit(&extra.join("debug"), "widget", 2);
    fs::set_permissions(repo.join("target/debug"), fs::Permissions::from_mode(0o000)).unwrap();
    register(&hive, &repo);
    let (out, receipt) = explain(&hive, &[repo.to_str().unwrap()]);
    fs::set_permissions(repo.join("target/debug"), fs::Permissions::from_mode(0o755)).unwrap();
    assert!(out.status.success(), "{receipt}");
    let named = finding_at(&receipt, "target.unreadable", &repo.join("target"))
        .unwrap_or_else(|| panic!("target was not named: {receipt}"));
    assert!(
        named["detail"]
            .as_str()
            .unwrap()
            .contains("ermission denied"),
        "{receipt}"
    );
    assert!(
        findings(&receipt, "copies.group")
            .iter()
            .all(|finding| finding["path"] != serde_json::json!(repo.join("target/debug"))),
        "{receipt}"
    );
    assert!(
        finding_at(&receipt, "copies.totals", &extra).is_some()
            || findings(&receipt, "copies.group")
                .iter()
                .any(|finding| { finding["path"] == serde_json::json!(extra.join("debug")) }),
        "the other target was not explained: {receipt}"
    );
}

#[test]
fn explain_cancelled_on_sigint_exits_130_and_leaves_the_snapshot() {
    let hive = Hive::ready();
    let s = Scratch::new("explain-signal");
    let repo = init_repo(&s.path.join("app"), "");
    register(&hive, &repo);
    let primed = hive.run(&["explain", repo.to_str().unwrap()]);
    assert!(
        primed.status.success(),
        "{}",
        String::from_utf8_lossy(&primed.stderr)
    );
    let snapshot_path = hive.home.path.join("snapshot.json");
    let before = fs::read(&snapshot_path).unwrap();
    let manifest = repo.join("Cargo.toml");
    let later = fs::metadata(&manifest).unwrap().modified().unwrap() + Duration::from_secs(5);
    fs::File::options()
        .write(true)
        .open(&manifest)
        .unwrap()
        .set_modified(later)
        .unwrap();
    let bin = s.path.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let sentinel = s.path.join("cargo.pid");
    fs::write(
        bin.join("cargo"),
        "#!/bin/sh\nprintf '%s\\n' \"$$\" > \"$HIVE_CARGO_SENTINEL\"\nsleep 300\n",
    )
    .unwrap();
    fs::set_permissions(bin.join("cargo"), fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let child = hive
        .command(&["explain", repo.to_str().unwrap(), "--json"])
        .env("PATH", &path)
        .env("HIVE_CARGO_SENTINEL", &sentinel)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let hive_pid = child.id();
    let started = Instant::now();
    let cargo_pid = loop {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "cargo metadata never wrote its pid"
        );
        if let Ok(text) = fs::read_to_string(&sentinel) {
            if let Ok(pid) = text.trim().parse::<u32>() {
                break pid;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(
        pid_alive(cargo_pid),
        "sleeper should be running before SIGINT"
    );
    assert!(Command::new("kill")
        .args(["-INT", &hive_pid.to_string()])
        .status()
        .unwrap()
        .success());
    let out = wait_within(child, Duration::from_secs(30));
    assert_eq!(out.status.code(), Some(130), "{out:?}");
    assert!(!pid_alive(cargo_pid), "cargo metadata sleeper must be dead");
    assert_eq!(
        fs::read(&snapshot_path).unwrap(),
        before,
        "a cancelled explain must leave snapshot.json unchanged"
    );
}

#[test]
fn explain_merges_the_snapshot() {
    let hive = Hive::ready();
    let s = Scratch::new("explain-merge");
    let repo = init_repo(&s.path.join("app"), "");
    tag_target(&repo.join("target"));
    write_unit(&repo.join("target/debug"), "widget", 1);
    register(&hive, &repo);
    let (out, receipt) = explain(&hive, &[repo.to_str().unwrap()]);
    assert!(out.status.success(), "{receipt}");
    let snapshot_path = hive.home.path.join("snapshot.json");
    let mut snap: serde_json::Value =
        serde_json::from_slice(&fs::read(&snapshot_path).unwrap()).unwrap();
    let repo_key = repo.display().to_string();
    assert!(
        snap["metadata"].get(&repo_key).is_some(),
        "explain did not record metadata: {snap}"
    );
    let (status_out, status_receipt) = hive.json(&["status", "--no-sizes", repo.to_str().unwrap()]);
    assert!(status_out.status.success(), "{status_receipt}");
    assert_eq!(
        status_receipt["counts"]["metadata_calls"], 0,
        "{status_receipt}"
    );
    assert_eq!(
        status_receipt["counts"]["metadata_cache_hits"], 1,
        "{status_receipt}"
    );
    snap = serde_json::from_slice(&fs::read(&snapshot_path).unwrap()).unwrap();
    let mut preserved = snap["metadata"][&repo_key].clone();
    preserved["resolved"]["workspace_root"] = serde_json::json!("/w/preserved");
    preserved["resolved"]["target_directory"] = serde_json::json!("/w/preserved/target");
    preserved["resolved"]["build_directory"] = serde_json::json!("/w/preserved/target");
    preserved["stamp"] = serde_json::json!({"/w/preserved/Cargo.lock": 1});
    snap["metadata"]["/w/preserved"] = preserved;
    let profile = repo.join("target/debug");
    snap["unsupported_profiles"][profile.to_str().unwrap()] = serde_json::json!("stale");
    snap["unsupported_profiles"]["/not/scanned"] = serde_json::json!("keep");
    fs::write(&snapshot_path, serde_json::to_vec_pretty(&snap).unwrap()).unwrap();
    let (again_out, again) = explain(&hive, &[repo.to_str().unwrap()]);
    assert!(again_out.status.success(), "{again}");
    let merged: serde_json::Value =
        serde_json::from_slice(&fs::read(&snapshot_path).unwrap()).unwrap();
    assert!(
        merged["metadata"].get("/w/preserved").is_some(),
        "merge dropped a disk-only metadata entry: {merged}"
    );
    assert!(
        merged["metadata"].get(&repo_key).is_some(),
        "merge dropped the workspace: {merged}"
    );
    assert!(
        merged["unsupported_profiles"]
            .get(profile.to_str().unwrap())
            .is_none(),
        "a supported profile stayed unsupported: {merged}"
    );
    assert_eq!(
        merged["unsupported_profiles"]["/not/scanned"], "keep",
        "{merged}"
    );
}

#[test]
fn the_json_receipt_is_the_logged_receipt() {
    let hive = Hive::ready();
    let s = Scratch::new("explain-json");
    let repo = init_repo(&s.path.join("app"), "");
    register(&hive, &repo);
    let out = hive
        .command(&["explain", "-n", "0", "--json", repo.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8(out.stdout).unwrap();
    let dir = hive.home.path.join("receipts");
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    files.sort();
    let log = fs::read_to_string(files.last().unwrap()).unwrap();
    assert_eq!(stdout.trim_end(), log.lines().last().unwrap());
}

#[test]
fn top_zero_shows_every_group() {
    let hive = Hive::ready();
    let s = Scratch::new("explain-top");
    let repo = init_repo(&s.path.join("app"), "");
    let profile = repo.join("target/debug");
    tag_target(&repo.join("target"));
    for i in 0..12 {
        let name = format!("c{i:02}");
        write_unit(&profile, &name, i * 2 + 1);
        write_unit(&profile, &name, i * 2 + 2);
    }
    register(&hive, &repo);
    let (all_out, all) = explain(&hive, &["-n", "0", repo.to_str().unwrap()]);
    assert!(all_out.status.success(), "{all}");
    assert_eq!(findings(&all, "copies.group").len(), 12, "{all}");
    let (default_out, defaulted) = explain(&hive, &[repo.to_str().unwrap()]);
    assert!(default_out.status.success(), "{defaulted}");
    assert_eq!(
        findings(&defaulted, "copies.group").len(),
        10,
        "{defaulted}"
    );
}
