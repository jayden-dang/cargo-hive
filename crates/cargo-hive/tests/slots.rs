//! Drives pool initialization and status through fresh processes and scratch state.
#![cfg(target_os = "macos")]

#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod common;

use common::{Hive, Scratch, git, init_repo, register};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
use std::path::{Path, PathBuf};

const CAP: usize = 1_048_576;

struct PoolFixture {
    hive: Hive,
    _repo_dir: Scratch,
    repo: PathBuf,
    profile: PathBuf,
}

impl PoolFixture {
    fn new() -> Self {
        let hive = Hive::new();
        hive.config(&json!({"secondary_roots": [hive.secondary_root.path]}).to_string());
        let repo_dir = Scratch::new("slot-registry");
        let repo = init_repo(&repo_dir.path.join("main"), "");
        register(&hive, &repo);
        let profile = repo_dir.path.join("reviewed.json");
        let common = repo.join(".git");
        let meta = fs::metadata(&common).unwrap();
        let oid = git(&repo, &["rev-parse", "HEAD:src/lib.rs"]);
        let manifest = git(&repo, &["rev-parse", "HEAD:Cargo.toml"]);
        fs::write(
            &profile,
            json!({
                "schema": 1,
                "id": "fixture",
                "revision": 1,
                "common_git_id": {"path": common, "dev": meta.dev(), "ino": meta.ino()},
                "graph_inputs": [{
                    "locator": {"kind": "checkout", "path": "Cargo.toml"},
                    "expectation": {"kind": "present", "identity": {"kind": "git_object", "oid": manifest}}
                }],
                "invocations": [{
                    "id": "check", "cargo": "cargo 1.97.1", "rustc": "rustc 1.97.1",
                    "layout": "flat", "argv": ["check", "--locked"],
                    "cwd": ".", "environment": [], "target_root": "target", "build_root": "target"
                }],
                "mutable_inputs": [{
                    "id": "source", "locator": {"kind": "checkout", "path": "src/lib.rs"},
                    "presence": "present", "invocations": ["check"],
                    "coverage": {"kind": "consumers", "ids": ["fixture-library"]}
                }],
                "consumers": [{
                    "id": "fixture-library", "package": "fixture", "role": "library",
                    "invocation": "check", "anchor": "src/lib.rs",
                    "prior_object": oid, "requested_object": oid
                }],
                "proofs": [{
                    "id": "check-proof", "invocation": "check", "layout": "flat",
                    "evidence": "fixture evidence 2026-11-23", "unaffected_unit": "independent"
                }]
            })
            .to_string(),
        )
        .unwrap();
        Self {
            hive,
            _repo_dir: repo_dir,
            repo,
            profile,
        }
    }

    fn file(&self) -> PathBuf {
        self.hive.home.path.join("slots.json")
    }

    fn init_at(&self, root: &Path) -> (std::process::Output, Value) {
        self.hive.json(&[
            "slot",
            "init",
            "--repo",
            self.repo.to_str().unwrap(),
            "--root",
            root.to_str().unwrap(),
            "--profile",
            self.profile.to_str().unwrap(),
        ])
    }

    fn init(&self) -> (std::process::Output, Value) {
        self.init_at(&self.hive.secondary_root.path)
    }

    fn status(&self) -> (std::process::Output, Value) {
        self.hive
            .json(&["slot", "status", "--repo", self.repo.to_str().unwrap()])
    }

    fn record(&self) -> Value {
        serde_json::from_slice(&fs::read(self.file()).unwrap()).unwrap()
    }
}

#[test]
fn initialization_records_two_private_unused_paths_across_processes() {
    let fixture = PoolFixture::new();
    let before = git(&fixture.repo, &["worktree", "list", "--porcelain"]);
    let (out, result) = fixture.init();
    assert!(out.status.success(), "{result}");
    assert_eq!(result["kind"], "initialized");
    assert_eq!(result["schema"], 1);
    let pool_id = result["pool_id"].as_str().unwrap();
    assert_eq!(pool_id.len(), 32);
    assert!(
        pool_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );
    assert_eq!(result["slots"].as_array().unwrap().len(), 2);

    let root = fixture
        .hive
        .secondary_root
        .path
        .join("hive-slots")
        .join(pool_id);
    let locks = fixture.hive.home.path.join("slot-locks").join(pool_id);
    assert_eq!(fs::metadata(fixture.file()).unwrap().mode() & 0o7777, 0o600);
    for directory in [
        &root,
        &locks,
        root.parent().unwrap(),
        locks.parent().unwrap(),
    ] {
        assert_eq!(fs::metadata(directory).unwrap().mode() & 0o7777, 0o700);
    }
    for number in 1..=2 {
        let path = root.join(format!("slot-{number}"));
        let lock = locks.join(format!("slot-{number}.lock"));
        let metadata = fs::metadata(lock).unwrap();
        assert_eq!(metadata.mode() & 0o7777, 0o600);
        assert_eq!(metadata.nlink(), 1);
        assert_eq!(metadata.len(), 0);
        assert!(fs::symlink_metadata(&path).is_err());
        assert_eq!(
            result["slots"][number - 1],
            json!({
                "number": number, "path": path, "phase": "vacant", "generation": 0
            })
        );
    }

    let stored = fs::read(fixture.file()).unwrap();
    assert!(stored.starts_with(b"{\n  \"layout\": 2,"));
    assert_eq!(fixture.record()["revision"], 1);
    let (out, status) = fixture.status();
    assert!(out.status.success(), "{status}");
    assert_eq!(status["pool_id"], pool_id);
    assert_eq!(status["slots"], result["slots"]);
    let (out, repeated) = fixture.init();
    assert_eq!(out.status.code(), Some(1), "{repeated}");
    assert_eq!(fs::read(fixture.file()).unwrap(), stored);
    assert_eq!(
        git(&fixture.repo, &["worktree", "list", "--porcelain"]),
        before
    );
}

#[test]
fn locks_or_checkout_evidence_without_a_record_remain_held() {
    let fixture = PoolFixture::new();
    let (out, fresh) = fixture.status();
    assert!(out.status.success(), "{fresh}");
    assert_eq!(fresh["kind"], "unconfigured");
    for orphan_under_root in [true, false] {
        let fixture = PoolFixture::new();
        let orphan = if orphan_under_root {
            fixture
                .hive
                .secondary_root
                .path
                .join("hive-slots/prior/slot-1")
        } else {
            fixture.hive.home.path.join("slot-locks/prior")
        };
        fs::create_dir_all(&orphan).unwrap();
        let bytes = orphan.join("preserve");
        fs::write(&bytes, b"prior work").unwrap();
        for (out, result) in [fixture.status(), fixture.init()] {
            assert_eq!(out.status.code(), Some(1), "{result}");
            assert_eq!(result["kind"], "held");
            assert!(result.to_string().contains("slots.json"), "{result}");
            assert!(result.to_string().contains("quarantine"), "{result}");
            assert!(result.to_string().contains("cargo hive seed"), "{result}");
        }
        assert!(!fixture.file().exists());
        assert_eq!(fs::read(bytes).unwrap(), b"prior work");
    }
}

#[test]
fn bad_slot_metadata_names_recovery_and_is_left_untouched() {
    let fixture = PoolFixture::new();
    let (out, result) = fixture.init();
    assert!(out.status.success(), "{result}");
    let good = fs::read(fixture.file()).unwrap();
    let value = fixture.record();
    let corrupt = |change: fn(&mut Value)| {
        let mut changed = value.clone();
        change(&mut changed);
        serde_json::to_vec(&changed).unwrap()
    };
    let cases = [
        ("invalid JSON", b"not json".to_vec()),
        ("old layout", corrupt(|v| v["layout"] = json!(1))),
        ("newer layout", corrupt(|v| v["layout"] = json!(3))),
        (
            "wrong profile schema",
            corrupt(|v| v["pools"][0]["profile"]["schema"] = json!(2)),
        ),
        (
            "duplicate pool",
            corrupt(|v| {
                let pool = v["pools"][0].clone();
                v["pools"].as_array_mut().unwrap().push(pool);
            }),
        ),
        (
            "duplicate lock identity",
            corrupt(|v| {
                v["pools"][0]["slots"][1]["lock_id"] = v["pools"][0]["slots"][0]["lock_id"].clone();
            }),
        ),
        (
            "duplicate slot number",
            corrupt(|v| v["pools"][0]["slots"][1]["number"] = json!(1)),
        ),
        (
            "unsafe path spelling",
            corrupt(|v| v["pools"][0]["slots"][1]["path"] = json!("elsewhere/../slot-2")),
        ),
        (
            "unknown nested field",
            corrupt(|v| {
                v["pools"][0]["slots"][1]["phase"]["credential"] = json!("secret sentinel")
            }),
        ),
        (
            "omitted nullable field",
            corrupt(|v| {
                v["pools"][0]["slots"][0]
                    .as_object_mut()
                    .unwrap()
                    .remove("last_head");
            }),
        ),
        ("zero revision", corrupt(|v| v["revision"] = json!(0))),
    ];
    for (case, bytes) in cases {
        fs::write(fixture.file(), &bytes).unwrap();
        let (out, status) = fixture.status();
        assert_eq!(out.status.code(), Some(1), "{case}: {status}");
        let result = status.to_string();
        assert!(result.contains("slots.json"), "{case}: {result}");
        assert!(
            result.contains("inspect") || result.contains("newer cargo-hive"),
            "{case}: {result}"
        );
        assert!(!result.contains("secret sentinel"), "{case}: {result}");
        assert_eq!(fs::read(fixture.file()).unwrap(), bytes, "{case}");
    }
    let mut at_cap = good;
    at_cap.resize(CAP, b' ');
    fs::write(fixture.file(), &at_cap).unwrap();
    let (out, status) = fixture.status();
    assert!(out.status.success(), "{status}");
    at_cap.push(b' ');
    fs::write(fixture.file(), &at_cap).unwrap();
    let (out, status) = fixture.status();
    assert_eq!(out.status.code(), Some(1), "{status}");
    assert!(status.to_string().contains("1048576"), "{status}");
    assert_eq!(fs::read(fixture.file()).unwrap(), at_cap);
}

#[test]
fn unsafe_existing_inodes_and_repository_paths_are_never_adopted() {
    let fixture = PoolFixture::new();
    let profile = fs::read(&fixture.profile).unwrap();
    let mut bad_profile: Value = serde_json::from_slice(&profile).unwrap();
    bad_profile["schema"] = json!(2);
    fs::write(&fixture.profile, serde_json::to_vec(&bad_profile).unwrap()).unwrap();
    let (out, result) = fixture.init();
    assert_eq!(out.status.code(), Some(1), "{result}");
    assert!(!fixture.hive.home.path.join("slot-locks").exists());
    fs::write(&fixture.profile, profile).unwrap();

    let alias = fixture._repo_dir.path.join("alias");
    symlink(&fixture.hive.secondary_root.path, &alias).unwrap();
    for root in [&alias, &fixture.repo] {
        let (out, result) = fixture.init_at(root);
        assert_eq!(out.status.code(), Some(1), "{result}");
        assert!(!fixture.file().exists());
    }

    let target = fixture._repo_dir.path.join("foreign");
    fs::write(&target, b"foreign bytes").unwrap();
    symlink(&target, fixture.file()).unwrap();
    let (out, result) = fixture.init();
    assert_eq!(out.status.code(), Some(1), "{result}");
    assert!(result.to_string().contains("slots.json"), "{result}");
    assert_eq!(fs::read(&target).unwrap(), b"foreign bytes");
    fs::remove_file(fixture.file()).unwrap();

    let (out, result) = fixture.init();
    assert!(out.status.success(), "{result}");
    let bytes = fs::read(fixture.file()).unwrap();
    for mode in [0o400, 0o640, 0o666] {
        fs::set_permissions(fixture.file(), fs::Permissions::from_mode(mode)).unwrap();
        let (out, status) = fixture.status();
        assert_eq!(out.status.code(), Some(1), "{status}");
        assert!(status.to_string().contains("slots.json"), "{status}");
        assert_eq!(fs::read(fixture.file()).unwrap(), bytes);
    }
    fs::set_permissions(fixture.file(), fs::Permissions::from_mode(0o600)).unwrap();
    let linked = fixture.hive.home.path.join("other");
    fs::hard_link(fixture.file(), linked).unwrap();
    let (out, status) = fixture.status();
    assert_eq!(out.status.code(), Some(1), "{status}");
}

#[test]
fn unavailable_protocol_has_no_state_effects() {
    let fixture = PoolFixture::new();
    let absent = fixture._repo_dir.path.join("must-stay-absent");
    let out = fixture
        .hive
        .command(&["slot", "key", "--out-fd", "1", "--json"])
        .env("HIVE_HOME", &absent)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        serde_json::from_slice::<Value>(&out.stdout).unwrap()["kind"],
        "unavailable"
    );
    assert!(!absent.exists());
}

#[test]
fn an_independent_owner_checkout_below_the_root_blocks_initialization() {
    let fixture = PoolFixture::new();
    let owner = init_repo(&fixture.hive.secondary_root.path.join("existing-lane"), "");
    let original = fs::read(owner.join("src/lib.rs")).unwrap();
    let (out, result) = fixture.init();
    assert_eq!(out.status.code(), Some(1), "{result}");
    assert!(
        result.to_string().contains("existing Git checkout"),
        "{result}"
    );
    assert!(!fixture.file().exists());
    assert!(!fixture.hive.home.path.join("slot-locks").exists());
    assert_eq!(fs::read(owner.join("src/lib.rs")).unwrap(), original);
}

#[test]
fn status_holds_an_unrecorded_checkout_and_a_replaced_lock() {
    let fixture = PoolFixture::new();
    let (out, initialized) = fixture.init();
    assert!(out.status.success(), "{initialized}");
    let pool = initialized["pool_id"].as_str().unwrap();
    let path = Path::new(initialized["slots"][0]["path"].as_str().unwrap());
    fs::create_dir(path).unwrap();
    fs::write(path.join("unfinished"), b"preserve this").unwrap();
    let lock = fixture
        .hive
        .home
        .path
        .join("slot-locks")
        .join(pool)
        .join("slot-2.lock");
    fs::rename(&lock, lock.with_extension("previous")).unwrap();
    fs::write(&lock, b"").unwrap();
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o600)).unwrap();
    let saved = fs::read(fixture.file()).unwrap();
    let (out, status) = fixture.status();
    assert!(out.status.success(), "{status}");
    assert_eq!(status["slots"][0]["phase"], "held");
    assert_eq!(status["slots"][1]["phase"], "held");
    assert!(
        status["next_action"]
            .as_str()
            .unwrap()
            .contains("cargo hive seed")
    );
    assert_eq!(fs::read(fixture.file()).unwrap(), saved);
    assert_eq!(fs::read(path.join("unfinished")).unwrap(), b"preserve this");
}

#[test]
fn a_replaced_registered_main_cannot_become_a_new_pool() {
    let fixture = PoolFixture::new();
    let old_main = fixture.repo.with_file_name("old-main");
    fs::rename(&fixture.repo, &old_main).unwrap();
    init_repo(&fixture.repo, "");
    git(
        &fixture.repo,
        &["commit", "--amend", "-q", "-m", "different root"],
    );
    let profile_bytes = fs::read(&fixture.profile).unwrap();
    let mut profile: Value = serde_json::from_slice(&profile_bytes).unwrap();
    let new_git = fs::metadata(fixture.repo.join(".git")).unwrap();
    profile["common_git_id"]["dev"] = json!(new_git.dev());
    profile["common_git_id"]["ino"] = json!(new_git.ino());
    fs::write(&fixture.profile, serde_json::to_vec(&profile).unwrap()).unwrap();
    let (out, result) = fixture.init();
    assert_eq!(out.status.code(), Some(1), "{result}");
    assert!(
        result.to_string().contains("registered Git scope"),
        "{result}"
    );
    assert!(!fixture.hive.home.path.join("slot-locks").exists());
    assert!(!fixture.file().exists());
}

#[test]
fn an_outcome_cannot_rebind_the_current_lease_or_repeat_a_replay_key() {
    let fixture = PoolFixture::new();
    let (out, result) = fixture.init();
    assert!(out.status.success(), "{result}");
    let mut record = fixture.record();
    let pool = record["pools"][0]["pool_id"].clone();
    let commit = git(&fixture.repo, &["rev-parse", "HEAD"]);
    let operation = "1".repeat(32);
    let credential = "a".repeat(64);
    let slot = &mut record["pools"][0]["slots"][0];
    slot["generation"] = json!(1);
    slot["phase"] = json!({
        "kind": "reserved", "lease": {
            "requested_commit": commit, "credential": credential, "reservation_op": operation
        }
    });
    let outcome = json!({
        "pool_id": pool, "slot": 1, "generation": 1, "operation": operation,
        "kind": "reserve", "attempt_id": null,
        "request": {"kind": "reserve", "expected_generation": 0, "commit": commit},
        "credential": credential, "result": {"kind": "reserved"}
    });
    slot["outcomes"] = json!([outcome]);
    fs::write(fixture.file(), serde_json::to_vec(&record).unwrap()).unwrap();
    let (out, status) = fixture.status();
    assert!(out.status.success(), "{status}");
    assert_eq!(status["slots"][0]["phase"], "reserved");
    assert!(!status.to_string().contains(&credential));

    for outcomes in [
        json!([outcome, outcome]),
        json!([outcome, { "unrecognized": "sensitive value" }]),
        {
            let mut wrong = outcome.clone();
            wrong["credential"] = json!("b".repeat(64));
            json!([wrong])
        },
    ] {
        record["pools"][0]["slots"][0]["outcomes"] = outcomes;
        fs::write(fixture.file(), serde_json::to_vec(&record).unwrap()).unwrap();
        let (out, status) = fixture.status();
        assert_eq!(out.status.code(), Some(1), "{status}");
        assert!(!status.to_string().contains(&credential));
        assert!(!status.to_string().contains(&"b".repeat(64)));
    }
}
