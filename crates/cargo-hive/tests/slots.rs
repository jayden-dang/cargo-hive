//! Drives pool initialization and status through fresh processes and scratch state.
#![cfg(target_os = "macos")]

#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod common;

use cargo_hive_platform::testing::pipe_read_status;
use common::{Hive, Scratch, git, init_repo, register};
use serde_json::{Value, json};
use std::ffi::OsStr;
use std::fs;
use std::io::{Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt, symlink};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Output, Stdio};
use std::time::{Duration, Instant};

const CAP: usize = 1_048_576;

struct PoolFixture {
    hive: Hive,
    _repo_dir: Scratch,
    repo: PathBuf,
    profile: PathBuf,
}

impl PoolFixture {
    fn new() -> Self {
        let repo_dir = Scratch::new("slot-registry");
        let repo = init_repo(&repo_dir.path.join("main"), "");
        Self::from_repo(repo_dir, repo)
    }

    fn from_repo(repo_dir: Scratch, repo: PathBuf) -> Self {
        let hive = Hive::new();
        hive.config(&json!({"secondary_roots": [hive.secondary_root.path]}).to_string());
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
        self.init_on(&self.hive, root)
    }

    fn init_on(&self, hive: &Hive, root: &Path) -> (std::process::Output, Value) {
        hive.json(&[
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

    fn reservation(
        &self,
        pool: &str,
        slot: u8,
        generation: u64,
        commit: &str,
        op: &str,
    ) -> Vec<String> {
        [
            "slot",
            "reserve",
            "--repo",
            self.repo.to_str().unwrap(),
            "--pool",
            pool,
            "--slot",
            &slot.to_string(),
            "--expect",
            &generation.to_string(),
            "--commit",
            commit,
            "--op",
            op,
            "--json",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    fn replay(
        &self,
        command: &str,
        pool: &str,
        slot: u8,
        generation: u64,
        op: &str,
    ) -> Vec<String> {
        [
            "slot",
            command,
            "--repo",
            self.repo.to_str().unwrap(),
            "--pool",
            pool,
            "--slot",
            &slot.to_string(),
            "--generation",
            &generation.to_string(),
            "--op",
            op,
            "--json",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }
}

fn start_private(hive: &Hive, args: Vec<String>, input: &str) -> Child {
    start_private_with(hive, args, input, |_| ())
}

#[allow(clippy::disallowed_types)]
fn start_private_with(
    hive: &Hive,
    args: Vec<String>,
    input: &str,
    configure: impl FnOnce(&mut std::process::Command),
) -> Child {
    assert!(args.iter().all(|arg| arg != input.trim_end()));
    let mut command = hive.command(&[]);
    configure(&mut command);
    let mut child = command
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child
}

fn finish_private(child: Child) -> (Output, Value) {
    let out = common::wait_within(child, Duration::from_secs(15));
    let result = serde_json::from_slice(&out.stdout).unwrap_or_else(|err| panic!("{err}: {out:?}"));
    (out, result)
}

struct LeasedSlot {
    fixture: PoolFixture,
    pool: String,
    path: PathBuf,
    admin: PathBuf,
    head: String,
    credential: String,
    operation: String,
}

impl LeasedSlot {
    fn new() -> Self {
        Self::from_fixture(PoolFixture::new())
    }

    fn from_fixture(fixture: PoolFixture) -> Self {
        let (out, initialized) = fixture.init();
        assert!(out.status.success(), "{initialized}");
        let pool = initialized["pool_id"].as_str().unwrap().to_owned();
        let path = PathBuf::from(initialized["slots"][0]["path"].as_str().unwrap());
        git(
            &fixture.repo,
            &[
                "worktree",
                "add",
                "--detach",
                path.to_str().unwrap(),
                "HEAD",
            ],
        );
        let head = git(&path, &["rev-parse", "HEAD"]);
        let admin = PathBuf::from(git(&path, &["rev-parse", "--absolute-git-dir"]));
        let directory = fs::metadata(&path).unwrap();
        let admin_metadata = fs::metadata(&admin).unwrap();
        let checkout = json!({
            "directory": {"dev": directory.dev(), "ino": directory.ino()},
            "admin": {"path": admin, "dev": admin_metadata.dev(), "ino": admin_metadata.ino()}
        });
        let credential = "d".repeat(64);
        let mut record = fixture.record();
        let slot = &mut record["pools"][0]["slots"][0];
        slot["generation"] = json!(1);
        slot["checkout_id"] = checkout.clone();
        slot["last_head"] = json!(head);
        slot["phase"] = json!({
            "kind": "leased", "ready_commit": head, "lease": {
                "requested_commit": head, "credential": credential, "reservation_op": "2".repeat(32)
            }
        });
        slot["producer"] = json!({
            "kind": "never_built", "evidence": {
                "epoch": {"pool_id": pool, "slot": 1, "checkout": checkout},
                "roots": {
                    "target": {"kind": "absent", "path": path.join("target")},
                    "build": {"kind": "absent", "path": path.join("target")}
                }
            }
        });
        fs::write(fixture.file(), serde_json::to_vec(&record).unwrap()).unwrap();
        Self {
            fixture,
            pool,
            path,
            admin,
            head,
            credential,
            operation: "6".repeat(32),
        }
    }

    fn start(&self, command: &str) -> Child {
        self.start_with(command, |_| ())
    }

    fn lock_path(&self) -> PathBuf {
        self.fixture
            .hive
            .home
            .path
            .join("slot-locks")
            .join(&self.pool)
            .join("slot-1.lock")
    }

    #[allow(clippy::disallowed_types)]
    fn start_with(
        &self,
        command: &str,
        configure: impl FnOnce(&mut std::process::Command),
    ) -> Child {
        start_private_with(
            &self.fixture.hive,
            self.fixture
                .replay(command, &self.pool, 1, 1, &self.operation),
            &self.credential,
            configure,
        )
    }

    fn assert_refused(&self, check: &str) {
        self.assert_refused_with(check, |_| ());
    }

    #[allow(clippy::disallowed_types)]
    fn assert_refused_with(&self, check: &str, configure: impl FnOnce(&mut std::process::Command)) {
        let checkout = [&*self.path, &*self.fixture.repo, &*self.fixture.file()];
        let before = snapshot_paths(&checkout);
        let (out, result) = finish_private(self.start_with("return", configure));
        assert_eq!(out.status.code(), Some(1), "{result}");
        let detail = result["detail"].as_str().unwrap();
        assert!(detail.contains(check), "{result}");
        assert_no_credential(&out, &self.credential);
        assert_eq!(snapshot_paths(&checkout), before);
    }

    fn retain(&self, path: &Path) {
        let mut record = self.fixture.record();
        let slot = &mut record["pools"][0]["slots"][0];
        let root_meta = fs::symlink_metadata(path).unwrap();
        let proof = json!({
            "path": path, "identity": {"dev": root_meta.dev(), "ino": root_meta.ino()}
        });
        slot["producer"] = json!({
            "kind": "complete",
            "epoch": slot["producer"]["evidence"]["epoch"].clone(),
            "generation": 1, "profile": {"id": "fixture", "revision": 1},
            "roots": {"target": proof, "build": proof},
            "invocations": [{
                "evidence": {
                    "invocation": "check", "graph_digest": "a".repeat(64),
                    "environment_digest": "b".repeat(64)
                },
                "operation": "1".repeat(32),
                "runner": {
                    "boot_session": "boot", "pid": 1, "start_secs": 1,
                    "start_micros": 0, "nonce": "c".repeat(64)
                },
                "foreground": {"pid": 2, "start_secs": 1, "start_micros": 0}
            }]
        });
        fs::write(self.fixture.file(), serde_json::to_vec(&record).unwrap()).unwrap();
    }
}

fn producer_slot() -> LeasedSlot {
    producer_slot_with_source(None)
}

fn producer_slot_with_source(source: Option<&str>) -> LeasedSlot {
    LeasedSlot::from_fixture(reviewed_pool_fixture(source))
}

fn reviewed_pool_fixture(source: Option<&str>) -> PoolFixture {
    let fixture = PoolFixture::new();
    fixture.hive.write_ready_capabilities();
    if let Some(source) = source {
        fs::write(fixture.repo.join("src/lib.rs"), source).unwrap();
    }
    fs::write(
        fixture.repo.join("Cargo.lock"),
        "version = 4\n\n[[package]]\nname = \"fixture\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"independent\"\nversion = \"0.1.0\"\n",
    ).unwrap();
    fs::OpenOptions::new()
        .append(true)
        .open(fixture.repo.join("Cargo.toml"))
        .unwrap()
        .write_all(b"\n[workspace]\nmembers = [\"independent\"]\n")
        .unwrap();
    fs::create_dir_all(fixture.repo.join("independent/src")).unwrap();
    fs::write(
        fixture.repo.join("independent/Cargo.toml"),
        "[package]\nname = \"independent\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        fixture.repo.join("independent/src/lib.rs"),
        "pub fn independent() {}\n",
    )
    .unwrap();
    git(&fixture.repo, &["add", "-A"]);
    git(&fixture.repo, &["commit", "-qm", "Lock fixture graph"]);
    let mut profile: Value = serde_json::from_slice(&fs::read(&fixture.profile).unwrap()).unwrap();
    let toolchain = std::env::var("RUSTUP_TOOLCHAIN").unwrap();
    let release = toolchain
        .split_once("-aarch64")
        .map_or(toolchain.as_str(), |pair| pair.0);
    profile["invocations"][0]["cargo"] = json!(format!("cargo {release}"));
    profile["invocations"][0]["rustc"] = json!(format!("rustc {release}"));
    profile["invocations"][0]["layout"] = json!(if release == "1.97.1" {
        "flat"
    } else {
        "unit_dir"
    });
    profile["invocations"][0]["environment"] = json!([
        {"name": "CARGO_HOME", "expectation": {"kind": "present", "value": fixture.hive.cargo_home.path}},
        {"name": "CARGO_NET_OFFLINE", "expectation": {"kind": "present", "value": "true"}}
    ]);
    profile["proofs"][0]["layout"] = profile["invocations"][0]["layout"].clone();
    profile["graph_inputs"] = json!(["Cargo.toml", "Cargo.lock", "independent/Cargo.toml"]
        .map(|path| json!({
            "locator": {"kind": "checkout", "path": path},
            "expectation": {"kind": "present", "identity": {
                "kind": "git_object", "oid": git(&fixture.repo, &["rev-parse", &format!("HEAD:{path}")])
            }}
        })));
    let source = git(&fixture.repo, &["rev-parse", "HEAD:src/lib.rs"]);
    profile["consumers"][0]["prior_object"] = json!(source);
    profile["consumers"][0]["requested_object"] = json!(source);
    fs::write(&fixture.profile, serde_json::to_vec(&profile).unwrap()).unwrap();
    fixture
}

fn producer_input(slot: &LeasedSlot) -> Value {
    json!({
        "action": "start",
        "credential": slot.credential,
        "nonce": "9".repeat(64),
        "shape": {
            "profile": {"id": "fixture", "revision": 1},
            "invocation": "check",
            "argv": ["check", "--locked"],
            "cwd": "."
        }
    })
}

const PRODUCER_SLOT_FD: libc::c_int = 198;

#[allow(clippy::disallowed_types)]
fn pass_producer_lock(command: &mut std::process::Command, lock: &fs::File) {
    let fd = lock.as_raw_fd();
    // SAFETY: dup2 is async-signal-safe, and lock lives through spawn.
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(fd, PRODUCER_SLOT_FD) != PRODUCER_SLOT_FD {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[allow(clippy::disallowed_types)]
fn start_producer(
    slot: &LeasedSlot,
    generation: u64,
    operation: &str,
    input: &Value,
    lock: Option<&fs::File>,
    configure: impl FnOnce(&mut std::process::Command),
) -> Child {
    let mut args = slot
        .fixture
        .replay("producer", &slot.pool, 1, generation, operation);
    args.extend(["--lock-fd".into(), PRODUCER_SLOT_FD.to_string()]);
    start_private_with(&slot.fixture.hive, args, &input.to_string(), |command| {
        producer_environment(command);
        configure(command);
        if let Some(lock) = lock {
            pass_producer_lock(command, lock);
        }
    })
}

#[allow(clippy::disallowed_types)]
fn producer_environment(command: &mut std::process::Command) {
    command.env("CARGO_NET_OFFLINE", "true").env_remove("CARGO");
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name.starts_with("DYLD_")
            || ((name.starts_with("CARGO_") || name.starts_with("RUST"))
                && !matches!(
                    name.as_ref(),
                    "CARGO_HOME"
                        | "CARGO_NET_OFFLINE"
                        | "RUSTUP_HOME"
                        | "RUSTUP_TOOLCHAIN"
                        | "RUSTUP_AUTO_INSTALL"
                ))
        {
            command.env_remove(key);
        }
    }
}

#[test]
fn a_guarded_producer_persists_pending_before_the_guide_can_launch_cargo() {
    let slot = producer_slot();
    let (lock, _) = cargo_hive_state::slots::open_slot_lock(&slot.lock_path()).unwrap();
    let _guard = cargo_hive_platform::flock::try_exclusive(&lock)
        .unwrap()
        .unwrap();
    let child = start_producer(
        &slot,
        1,
        &slot.operation,
        &producer_input(&slot),
        Some(&lock),
        |_| {},
    );
    let (out, result) = finish_private(child);
    assert!(out.status.success(), "{result} {out:?}");
    assert_eq!(result["kind"], "producer_started");
    assert_no_credential(&out, &slot.credential);
    let record = slot.fixture.record();
    let producer = &record["pools"][0]["slots"][0]["producer"];
    assert_eq!(producer["kind"], "pending");
    assert_eq!(producer["generation"], 1);
    assert_eq!(producer["profile"], json!({"id": "fixture", "revision": 1}));
    assert_eq!(producer["operation"], slot.operation);
    assert!(
        producer["invocation"]["graph_digest"]
            .as_str()
            .unwrap()
            .len()
            == 64
    );
    assert_eq!(
        producer["roots"]["target"],
        json!({
            "kind": "absent", "path": slot.path.join("target")
        })
    );
    assert_eq!(producer["roots"]["build"], producer["roots"]["target"]);
    assert!(!slot.path.join("target").exists());
}

fn producer_result_request(slot: &LeasedSlot, command: &str, generation: u64) -> (Output, Value) {
    finish_private(start_private(
        &slot.fixture.hive,
        slot.fixture
            .replay(command, &slot.pool, 1, generation, &slot.operation),
        &slot.credential,
    ))
}

#[test]
fn pending_producer_result_is_queryable_without_allowing_ack_or_a_writer() {
    let slot = producer_slot();
    let (lock, _) = cargo_hive_state::slots::open_slot_lock(&slot.lock_path()).unwrap();
    let _guard = cargo_hive_platform::flock::try_exclusive(&lock)
        .unwrap()
        .unwrap();
    let (started, result) = finish_private(start_producer(
        &slot,
        1,
        &slot.operation,
        &producer_input(&slot),
        Some(&lock),
        |_| (),
    ));
    assert!(started.status.success(), "{result}");
    let before = slot.fixture.record();
    let (read, result) = producer_result_request(&slot, "outcome", 1);
    assert!(read.status.success(), "{result}");
    assert_eq!(result["kind"], "producer_pending");
    assert_eq!(result["current"], false);
    let (ack, _) = producer_result_request(&slot, "ack", 1);
    assert_eq!(ack.status.code(), Some(1));
    assert_eq!(slot.fixture.record(), before);
    assert!(!slot.path.join("target").exists());
}

#[test]
fn mutable_rust_and_consumer_anchors_require_reviewed_compiler_inputs() {
    for case in ["unreviewed source edit", "unrelated tracked anchor"] {
        let slot = producer_slot();
        let (lock, _) = cargo_hive_state::slots::open_slot_lock(&slot.lock_path()).unwrap();
        let _guard = cargo_hive_platform::flock::try_exclusive(&lock)
            .unwrap()
            .unwrap();
        let mut saved = slot.fixture.record();
        if case == "unreviewed source edit" {
            fs::write(
                slot.path.join("src/lib.rs"),
                r#"pub const NEW: &str = include_str!("../Cargo.toml");"#,
            )
            .unwrap();
        } else {
            let consumer = &mut saved["pools"][0]["profile"]["consumers"][0];
            let path = "independent/src/lib.rs";
            let oid = git(&slot.path, &["rev-parse", &format!("HEAD:{path}")]);
            consumer["anchor"] = json!(path);
            consumer["prior_object"] = json!(oid);
            consumer["requested_object"] = json!(oid);
            saved["pools"][0]["profile"]["mutable_inputs"][0]["locator"]["path"] = json!(path);
            fs::write(slot.fixture.file(), serde_json::to_vec(&saved).unwrap()).unwrap();
        }
        let (out, result) = finish_private(start_producer(
            &slot,
            1,
            &slot.operation,
            &producer_input(&slot),
            Some(&lock),
            |_| (),
        ));
        assert_eq!(out.status.code(), Some(1), "{case}: {result}");
        assert!(
            result["detail"]
                .as_str()
                .unwrap()
                .contains("consumer anchor"),
            "{case}: {result}"
        );
        assert_eq!(slot.fixture.record(), saved);
        assert!(!slot.path.join("target").exists());
    }
}

#[test]
fn an_unchanged_rust_entry_cannot_admit_arbitrary_included_source() {
    for case in ["file", "environment"] {
        let source = if case == "file" {
            r#"include!("body.inc");"#
        } else {
            r#"include!(env!("HIVE_FIXTURE_RUST_BODY"));"#
        };
        let slot = producer_slot_with_source(Some(source));
        let external = slot.fixture._repo_dir.path.join("unreviewed-probe");
        fs::write(&external, "unreviewed content").unwrap();
        let include = format!(
            "pub const UNREVIEWED: &str = include_str!({:?});\n",
            external
        );
        let body = slot.path.join("src/body.inc");
        fs::write(&body, include.as_bytes()).unwrap();
        let mut saved = slot.fixture.record();
        let profile = &mut saved["pools"][0]["profile"];
        profile["mutable_inputs"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "id": "body-file", "locator": {"kind": "checkout", "path": "src/body.inc"},
                "presence": "present", "invocations": ["check"],
                "coverage": {"kind": "consumers", "ids": ["fixture-library"]}
            }));
        if case == "environment" {
            profile["invocations"][0]["environment"].as_array_mut().unwrap().push(json!({
                "name": "HIVE_FIXTURE_RUST_BODY", "expectation": {"kind": "mutable", "input": "body-env"}
            }));
            profile["mutable_inputs"].as_array_mut().unwrap().push(json!({
                "id": "body-env", "locator": {"kind":"environment","name":"HIVE_FIXTURE_RUST_BODY"},
                "presence": "present", "invocations": ["check"],
                "coverage": {"kind": "consumers", "ids": ["fixture-library"]}
            }));
        }
        fs::write(slot.fixture.file(), serde_json::to_vec(&saved).unwrap()).unwrap();
        let index = slot.admin.join("index");
        let state_file = slot.fixture.file();
        let files = [&*slot.path, &*index, &*state_file];
        let before = snapshot_paths(&files);
        let (lock, _) = cargo_hive_state::slots::open_slot_lock(&slot.lock_path()).unwrap();
        let _guard = cargo_hive_platform::flock::try_exclusive(&lock)
            .unwrap()
            .unwrap();
        let (out, result) = finish_private(start_producer(
            &slot,
            1,
            &slot.operation,
            &producer_input(&slot),
            Some(&lock),
            |command| {
                if case == "environment" {
                    command.env("HIVE_FIXTURE_RUST_BODY", &body);
                }
            },
        ));
        assert_eq!(out.status.code(), Some(1), "{case}: {result}");
        assert_eq!(snapshot_paths(&files), before, "{case}");
        assert!(!slot.path.join("target").exists());
    }
}

#[test]
fn only_the_native_builtin_target_is_admitted_without_a_target_spec_probe() {
    for case in ["native target", "unknown target", "target search path"] {
        let slot = producer_slot();
        let native = format!("{}-apple-darwin", std::env::consts::ARCH);
        let argv = json!([
            "check",
            "--locked",
            "--target",
            if case == "unknown target" {
                "review-fixture".to_owned()
            } else {
                native
            },
        ]);
        let mut saved = slot.fixture.record();
        let profile = &mut saved["pools"][0]["profile"]["invocations"][0];
        profile["argv"] = argv.clone();
        let search = slot.fixture._repo_dir.path.join("custom-targets");
        fs::create_dir(&search).unwrap();
        fs::write(search.join("review-fixture.json"), "{}").unwrap();
        if case == "target search path" {
            profile["environment"].as_array_mut().unwrap().push(json!({
                "name": "RUST_TARGET_PATH", "expectation": {"kind": "present", "value": search}
            }));
        }
        fs::write(slot.fixture.file(), serde_json::to_vec(&saved).unwrap()).unwrap();
        let mut input = producer_input(&slot);
        input["shape"]["argv"] = argv;
        let (lock, _) = cargo_hive_state::slots::open_slot_lock(&slot.lock_path()).unwrap();
        let _guard = cargo_hive_platform::flock::try_exclusive(&lock)
            .unwrap()
            .unwrap();
        let (out, result) = finish_private(start_producer(
            &slot,
            1,
            &slot.operation,
            &input,
            Some(&lock),
            |command| {
                if case == "target search path" {
                    command.env("RUST_TARGET_PATH", &search);
                }
            },
        ));
        if case == "native target" {
            assert!(out.status.success(), "{result}");
        } else {
            assert_eq!(out.status.code(), Some(1), "{case}: {result}");
            assert_eq!(slot.fixture.record(), saved);
        }
        assert!(!slot.path.join("target").exists());
    }
}

#[test]
fn an_unverified_native_linker_cannot_get_a_build_permit() {
    let slot = producer_slot();
    let shim_dir = slot.fixture._repo_dir.path.join("compiler-shims");
    fs::create_dir(&shim_dir).unwrap();
    let marker = shim_dir.join("ran");
    let shim = shim_dir.join("cc");
    fs::write(
        &shim,
        format!(
            "#!/bin/sh\nprintf invoked > '{}'\nexit 1\n",
            marker.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o700)).unwrap();
    let argv = json!(["build", "--locked"]);
    let mut saved = slot.fixture.record();
    saved["pools"][0]["profile"]["invocations"][0]["argv"] = argv.clone();
    fs::write(slot.fixture.file(), serde_json::to_vec(&saved).unwrap()).unwrap();
    let mut input = producer_input(&slot);
    input["shape"]["argv"] = argv;
    let (lock, _) = cargo_hive_state::slots::open_slot_lock(&slot.lock_path()).unwrap();
    let _guard = cargo_hive_platform::flock::try_exclusive(&lock)
        .unwrap()
        .unwrap();
    let (out, result) = finish_private(start_producer(
        &slot,
        1,
        &slot.operation,
        &input,
        Some(&lock),
        |command| {
            command.env(
                "PATH",
                format!("{}:{}", shim_dir.display(), std::env::var("PATH").unwrap()),
            );
        },
    ));
    assert_eq!(out.status.code(), Some(1), "{result}");
    assert!(!marker.exists());
    assert!(!slot.path.join("target").exists());
    assert_eq!(slot.fixture.record(), saved);
}

#[test]
fn a_return_without_a_writer_cannot_pass_old_output_to_a_second_reservation() {
    let slot = producer_slot();
    let output = slot.path.join("target");
    fs::create_dir(&output).unwrap();
    slot.retain(&output);
    let (returned, result) = finish_private(slot.start("return"));
    assert!(returned.status.success(), "{result}");
    let reserve = |expected, op: &str| {
        finish_private(start_private(
            &slot.fixture.hive,
            slot.fixture
                .reservation(&slot.pool, 1, expected, &slot.head, op),
            &slot.credential,
        ))
    };
    let (reserved, result) = reserve(1, &"a".repeat(32));
    assert!(reserved.status.success(), "{result}");
    assert_eq!(result["current"], true);
    let mut ready = slot.fixture.record();
    let state = &mut ready["pools"][0]["slots"][0];
    state["phase"]["kind"] = json!("leased");
    state["phase"]["ready_commit"] = json!(slot.head);
    fs::write(slot.fixture.file(), serde_json::to_vec(&ready).unwrap()).unwrap();
    let (returned, result) = finish_private(start_private(
        &slot.fixture.hive,
        slot.fixture
            .replay("return", &slot.pool, 1, 2, &"b".repeat(32)),
        &slot.credential,
    ));
    assert!(returned.status.success(), "{result}");
    let before = slot.fixture.record();
    let (reserve_again, result) = reserve(2, &"e".repeat(32));
    assert_eq!(reserve_again.status.code(), Some(1), "{result}");
    assert_eq!(slot.fixture.record(), before);
    let (out, status) = slot.fixture.status();
    assert!(out.status.success(), "{status}");
    assert_eq!(status["slots"][0]["phase"], "held");
}

#[test]
fn an_exact_reviewed_rust_variant_can_record_a_guarded_start() {
    let slot = producer_slot();
    let source = "pub fn answer() -> u32 { 43 }\n";
    fs::write(slot.path.join("src/lib.rs"), source).unwrap();
    let oid = git(
        &slot.path,
        &["hash-object", "-w", "--no-filters", "--", "src/lib.rs"],
    );
    let mut saved = slot.fixture.record();
    saved["pools"][0]["profile"]["consumers"][0]["requested_object"] = json!(oid);
    fs::write(slot.fixture.file(), serde_json::to_vec(&saved).unwrap()).unwrap();
    let (lock, _) = cargo_hive_state::slots::open_slot_lock(&slot.lock_path()).unwrap();
    let _guard = cargo_hive_platform::flock::try_exclusive(&lock)
        .unwrap()
        .unwrap();
    let (out, result) = finish_private(start_producer(
        &slot,
        1,
        &slot.operation,
        &producer_input(&slot),
        Some(&lock),
        |_| (),
    ));
    assert!(out.status.success(), "{result}");
    assert_eq!(result["kind"], "producer_started");
    assert!(!slot.path.join("target").exists());
}

#[test]
fn an_executable_dependency_without_foreground_proof_is_refused_before_start() {
    let slot = producer_slot();
    let relative = "independent/Cargo.toml";
    fs::OpenOptions::new()
        .append(true)
        .open(slot.path.join(relative))
        .unwrap()
        .write_all(b"\n[lib]\nproc-macro = true\n")
        .unwrap();
    git(&slot.path, &["add", relative]);
    git(
        &slot.path,
        &[
            "commit",
            "-qm",
            "Fixture dependency can execute during compilation",
        ],
    );
    let head = git(&slot.path, &["rev-parse", "HEAD"]);
    let oid = git(&slot.path, &["rev-parse", &format!("HEAD:{relative}")]);
    let mut saved = slot.fixture.record();
    let pool = &mut saved["pools"][0];
    pool["profile"]["graph_inputs"][2]["expectation"]["identity"]["oid"] = json!(oid);
    let state = &mut pool["slots"][0];
    state["last_head"] = json!(head);
    state["phase"]["ready_commit"] = json!(head);
    state["phase"]["lease"]["requested_commit"] = json!(head);
    fs::write(slot.fixture.file(), serde_json::to_vec(&saved).unwrap()).unwrap();
    let (lock, _) = cargo_hive_state::slots::open_slot_lock(&slot.lock_path()).unwrap();
    let _guard = cargo_hive_platform::flock::try_exclusive(&lock)
        .unwrap()
        .unwrap();
    let (out, result) = finish_private(start_producer(
        &slot,
        1,
        &slot.operation,
        &producer_input(&slot),
        Some(&lock),
        |_| (),
    ));
    assert_eq!(out.status.code(), Some(1), "{result}");
    assert!(
        result["detail"]
            .as_str()
            .unwrap()
            .contains("executable build target"),
        "{result}"
    );
    assert_eq!(slot.fixture.record(), saved);
    assert!(!slot.path.join("target").exists());
}

#[test]
fn a_reviewed_config_and_environment_must_exclude_compiler_overrides() {
    for case in [
        "benign config",
        "reviewed wrapper",
        "rustc path",
        "reviewed git env",
        "reviewed compiler env",
        "unreviewed toolchain",
    ] {
        let slot = producer_slot();
        let (lock, _) = cargo_hive_state::slots::open_slot_lock(&slot.lock_path()).unwrap();
        let _guard = cargo_hive_platform::flock::try_exclusive(&lock)
            .unwrap()
            .unwrap();
        let mut saved = slot.fixture.record();
        let profile = &mut saved["pools"][0]["profile"];
        let config = slot.fixture.hive.cargo_home.path.join("config.toml");
        let bin = slot.fixture._repo_dir.path.join("bin");
        fs::create_dir(&bin).unwrap();
        let shim = bin.join("rustc");
        let marker = slot.fixture._repo_dir.path.join("unexpected-compiler");
        fs::write(
            &shim,
            format!(
                "#!/bin/sh\nprintf controlled > '{}'\nexit 1\n",
                marker.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o700)).unwrap();
        if matches!(case, "reviewed wrapper" | "benign config") {
            let body = if case == "reviewed wrapper" {
                format!("[build]\nrustc-wrapper = '{}'\n", shim.display())
            } else {
                "[net]\noffline = true\n".to_owned()
            };
            fs::write(&config, &body).unwrap();
            let digest = cargo_hive_platform::slot::sha256(body.as_bytes())
                .unwrap()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            profile["graph_inputs"].as_array_mut().unwrap().push(json!({
                "locator": {"kind": "external", "path": config},
                "expectation": {"kind": "present", "identity": {"kind": "reviewed_bytes", "sha256": digest}}
            }));
        }
        if matches!(case, "reviewed git env" | "reviewed compiler env") {
            let name = if case == "reviewed git env" {
                "GIT_CONFIG_GLOBAL"
            } else {
                "CARGO_BUILD_RUSTC"
            };
            profile["invocations"][0]["environment"]
                .as_array_mut()
                .unwrap()
                .push(json!({
                    "name": name, "expectation": {"kind": "present", "value": shim}
                }));
        }
        fs::write(slot.fixture.file(), serde_json::to_vec(&saved).unwrap()).unwrap();
        let (out, result) = finish_private(start_producer(
            &slot,
            1,
            &slot.operation,
            &producer_input(&slot),
            Some(&lock),
            |command| match case {
                "rustc path" => {
                    command.env(
                        "PATH",
                        format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
                    );
                }
                "reviewed git env" => {
                    command.env("GIT_CONFIG_GLOBAL", &shim);
                }
                "reviewed compiler env" => {
                    command.env("CARGO_BUILD_RUSTC", &shim);
                }
                "unreviewed toolchain" => {
                    command.env("RUSTUP_TOOLCHAIN", "unreviewed-fixture-toolchain");
                }
                _ => (),
            },
        ));
        if case == "benign config" {
            assert!(out.status.success(), "{case}: {result}");
        } else {
            assert_eq!(out.status.code(), Some(1), "{case}: {result}");
            assert_eq!(slot.fixture.record(), saved, "{case}");
        }
        assert!(!marker.exists(), "{case}: {result}");
        assert!(!slot.path.join("target").exists(), "{case}: {result}");
    }
}

#[test]
fn a_legacy_completion_without_its_foreground_witness_is_held() {
    let slot = producer_slot();
    let target = slot.path.join("target");
    fs::create_dir(&target).unwrap();
    slot.retain(&target);
    let mut saved = slot.fixture.record();
    saved["pools"][0]["slots"][0]["producer"]["invocations"][0]
        .as_object_mut()
        .unwrap()
        .remove("foreground");
    fs::write(slot.fixture.file(), serde_json::to_vec(&saved).unwrap()).unwrap();
    let (out, status) = slot.fixture.status();
    assert!(out.status.success(), "{status}");
    assert_eq!(status["slots"][0]["phase"], "held");
    let (out, result) = finish_private(slot.start("return"));
    assert_eq!(out.status.code(), Some(1), "{result}");
    assert_eq!(slot.fixture.record(), saved);
    saved["pools"][0]["slots"][0]["phase"] = json!({"kind": "vacant"});
    fs::write(slot.fixture.file(), serde_json::to_vec(&saved).unwrap()).unwrap();
    let (out, status) = slot.fixture.status();
    assert!(out.status.success(), "{status}");
    assert_eq!(status["slots"][0]["phase"], "held");
    let (out, result) = finish_private(start_private(
        &slot.fixture.hive,
        slot.fixture
            .reservation(&slot.pool, 1, 1, &slot.head, &"e".repeat(32)),
        &slot.credential,
    ));
    assert_eq!(out.status.code(), Some(1), "{result}");
    assert_eq!(slot.fixture.record(), saved);
}

#[test]
fn acknowledging_completed_producers_releases_capacity_for_the_next_lease() {
    let slot = producer_slot();
    let target = slot.path.join("target");
    fs::create_dir(&target).unwrap();
    slot.retain(&target);
    let mut saved = slot.fixture.record();
    let state = &mut saved["pools"][0]["slots"][0];
    let template = state["producer"]["invocations"][0].clone();
    let operations: Vec<_> = (1..=cargo_hive_state::slots::MAX_OUTCOMES)
        .map(|n| format!("{n:032x}"))
        .collect();
    state["producer"]["invocations"] = json!(
        operations
            .iter()
            .map(|op| {
                let mut item = template.clone();
                item["operation"] = json!(op);
                item
            })
            .collect::<Vec<_>>()
    );
    state["outcomes"] = json!(operations.iter().map(|op| json!({
        "pool_id": slot.pool, "slot": 1, "generation": 1, "operation": op,
        "kind": "producer_complete", "attempt_id": null,
        "request": { "kind": "producer", "runner": template["runner"], "invocation": template["evidence"],
            "ready_commit": slot.head },
        "credential": slot.credential, "result": {"kind": "producer_complete"}
    })).collect::<Vec<_>>());
    fs::write(slot.fixture.file(), serde_json::to_vec(&saved).unwrap()).unwrap();

    let (blocked, result) = finish_private(slot.start("return"));
    assert_eq!(blocked.status.code(), Some(1), "{result}");
    assert!(
        result["detail"]
            .as_str()
            .unwrap()
            .contains("unacknowledged")
    );
    let result_for = |command: &str, operation: &str, generation: u64, credential: &str| {
        finish_private(start_private(
            &slot.fixture.hive,
            slot.fixture
                .replay(command, &slot.pool, 1, generation, operation),
            credential,
        ))
    };
    assert_eq!(
        result_for("ack", &operations[0], 1, &"f".repeat(64))
            .0
            .status
            .code(),
        Some(1)
    );
    for operation in &operations {
        let (out, result) = result_for("ack", operation, 1, &slot.credential);
        assert!(out.status.success(), "{result}");
        assert_eq!(result["kind"], "acknowledged");
        assert_eq!(result["current"], false);
    }
    assert_eq!(
        slot.fixture.record()["pools"][0]["slots"][0]["outcomes"],
        json!([])
    );
    assert_eq!(
        result_for("outcome", &operations[0], 1, &slot.credential)
            .0
            .status
            .code(),
        Some(1)
    );

    let (returned, result) = finish_private(slot.start("return"));
    assert!(returned.status.success(), "{result}");
    let (reserved, result) = finish_private(start_private(
        &slot.fixture.hive,
        slot.fixture
            .reservation(&slot.pool, 1, 1, &slot.head, &"e".repeat(32)),
        &"f".repeat(64),
    ));
    assert!(reserved.status.success(), "{result}");
    assert_eq!(result["generation"], 2);
    assert_eq!(
        result_for("outcome", &operations[0], 1, &slot.credential)
            .0
            .status
            .code(),
        Some(1)
    );
}

fn producer_gate_args(slot: &LeasedSlot) -> Vec<String> {
    let resolved = slot
        .fixture
        .hive
        .command_with(Path::new("rustup"), &["which", "cargo"])
        .output()
        .unwrap();
    assert!(resolved.status.success(), "{resolved:?}");
    let cargo = String::from_utf8(resolved.stdout).unwrap();
    let resolved = slot
        .fixture
        .hive
        .command_with(Path::new("rustup"), &["which", "rustc"])
        .output()
        .unwrap();
    assert!(resolved.status.success(), "{resolved:?}");
    let rustc = String::from_utf8(resolved.stdout).unwrap();
    let mut args = vec![
        format!("CARGO_HOME={}", slot.fixture.hive.cargo_home.path.display()),
        "CARGO_NET_OFFLINE=true".into(),
    ];
    args.extend(
        ["HOME", "PATH", "RUSTUP_HOME", "RUSTUP_TOOLCHAIN"]
            .into_iter()
            .filter_map(|name| {
                std::env::var(name)
                    .ok()
                    .map(|value| format!("{name}={value}"))
            }),
    );
    args.extend([
        format!("RUSTC={}", rustc.trim()),
        "RUSTUP_AUTO_INSTALL=0".into(),
    ]);
    args.sort();
    args.extend([
        cargo.trim().to_owned(),
        "check".to_owned(),
        "--locked".to_owned(),
    ]);
    args
}

fn stopped_stock_cargo(slot: &LeasedSlot, lock: Option<&fs::File>, bad_env: bool) -> Child {
    let mut args = producer_gate_args(slot);
    if bad_env {
        args.insert(1, "CARGO_TARGET_DIR=outside".into());
    }
    stopped_stock_cargo_with_args(slot, lock, args)
}

fn stopped_stock_cargo_with_args(
    slot: &LeasedSlot,
    lock: Option<&fs::File>,
    args: Vec<String>,
) -> Child {
    let mut command = slot.fixture.hive.command_with(Path::new("/bin/bash"), &[]);
    command
        .args([
            "--noprofile",
            "--norc",
            "-c",
            "builtin kill -STOP \"$$\"; builtin exec -c /usr/bin/env -i \"$@\"",
            "producer-gate",
        ])
        .args(args)
        .current_dir(&slot.path)
        .process_group(0)
        .env("CARGO_HOME", &slot.fixture.hive.cargo_home.path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    producer_environment(&mut command);
    if let Some(lock) = lock {
        pass_producer_lock(&mut command, lock);
    }
    let child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let mut status = 0;
        // SAFETY: this test owns the child and passes a writable wait status.
        let observed = unsafe {
            libc::waitpid(
                child.id() as libc::pid_t,
                &mut status,
                libc::WNOHANG | libc::WUNTRACED,
            )
        };
        if observed != 0 {
            assert_eq!(observed as u32, child.id());
            assert!(libc::WIFSTOPPED(status));
            break;
        }
        assert!(Instant::now() < deadline, "guide child did not stop");
        std::thread::sleep(Duration::from_millis(10));
    }
    child
}

#[test]
fn observed_stock_cargo_completion_and_a_reported_bypass_bind_the_same_epoch() {
    for (pass_writer_lock, bad_env) in [(true, false), (false, false), (true, true)] {
        let slot = producer_slot();
        let (lock, _) = cargo_hive_state::slots::open_slot_lock(&slot.lock_path()).unwrap();
        let _guard = cargo_hive_platform::flock::try_exclusive(&lock)
            .unwrap()
            .unwrap();
        let mut request = producer_input(&slot);
        let (started, result) = finish_private(start_producer(
            &slot,
            1,
            &slot.operation,
            &request,
            Some(&lock),
            |_| (),
        ));
        assert!(started.status.success(), "{result} {started:?}");
        let mut cargo = stopped_stock_cargo(&slot, pass_writer_lock.then_some(&lock), bad_env);
        request["action"] = json!("watch");
        request["child_pid"] = json!(cargo.id());
        let mut watcher = start_producer(&slot, 1, &slot.operation, &request, Some(&lock), |_| ());
        if pass_writer_lock && !bad_env {
            let until = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(status) = cargo.try_wait().unwrap() {
                    assert!(status.success(), "{status}");
                    break;
                }
                if Instant::now() >= until || watcher.try_wait().unwrap().is_some() {
                    cargo.kill().unwrap();
                    cargo.wait().unwrap();
                    let (out, result) = finish_private(watcher);
                    panic!("watch did not permit stock Cargo: {result} {out:?}");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let (out, result) = finish_private(watcher);
            assert!(out.status.success(), "{result} {out:?}");
            assert_eq!(result["kind"], "producer_complete");
            let record = slot.fixture.record();
            let producer = &record["pools"][0]["slots"][0]["producer"];
            let target = slot.path.join("target");
            let actual = fs::metadata(&target).unwrap();
            assert_eq!(
                producer["roots"]["target"],
                json!({
                    "path": target, "identity": {"dev": actual.dev(), "ino": actual.ino()}
                })
            );
            assert_eq!(producer["roots"]["build"], producer["roots"]["target"]);
            assert_eq!(producer["invocations"][0]["operation"], slot.operation);

            let (queried, history) = producer_result_request(&slot, "outcome", 1);
            assert!(queried.status.success(), "{history}");
            assert_eq!(history["kind"], "producer_complete");
            assert_eq!(history["current"], false);
            let (acknowledged, ack) = producer_result_request(&slot, "ack", 1);
            assert!(acknowledged.status.success(), "{ack}");
            assert_eq!(ack["kind"], "acknowledged");
            assert_eq!(
                producer_result_request(&slot, "outcome", 1).0.status.code(),
                Some(1)
            );
            let (repeated, _) = finish_private(start_producer(
                &slot,
                1,
                &slot.operation,
                &producer_input(&slot),
                Some(&lock),
                |_| (),
            ));
            assert_eq!(repeated.status.code(), Some(1));
            slot.fixture.hive.check_crate(&slot.path);
            let start = producer_input(&slot);
            let report = json!({
                "action": "report_bypass", "credential": start["credential"], "nonce": start["nonce"],
                "profile": start["shape"]["profile"]
            });
            let bypass_operation = "b".repeat(32);
            let run = |request: &Value, operation: &str| {
                finish_private(start_producer(
                    &slot,
                    1,
                    operation,
                    request,
                    Some(&lock),
                    |_| (),
                ))
            };
            let (reported, result) = run(&report, &bypass_operation);
            assert!(reported.status.success(), "{result}");
            assert_eq!(result["kind"], "producer_tainted");
            let bypass = slot.fixture.record();
            let tainted = &bypass["pools"][0]["slots"][0]["producer"];
            assert_eq!(tainted["kind"], "tainted");
            assert_eq!(tainted["epoch"], producer["epoch"]);
            assert_eq!(tainted["roots"], producer["roots"]);
            assert_eq!(tainted["reason"], "off_profile");
            let (replayed, result) = run(&report, &bypass_operation);
            assert!(replayed.status.success(), "{result}");
            assert_eq!(result["phase"], "held");
            assert_eq!(slot.fixture.record(), bypass);
            assert!(
                !run(&producer_input(&slot), &"a".repeat(32))
                    .0
                    .status
                    .success()
            );
            assert_eq!(slot.fixture.record(), bypass);
        } else {
            let (out, result) = finish_private(watcher);
            cargo.kill().unwrap();
            cargo.wait().unwrap();
            assert_eq!(out.status.code(), Some(1), "{result}");
            let reason = if bad_env {
                "environment allowlist"
            } else {
                "custody"
            };
            assert!(
                result["detail"].as_str().unwrap().contains(reason),
                "{result}"
            );
            assert!(!slot.path.join("target").exists());
            assert_eq!(
                slot.fixture.record()["pools"][0]["slots"][0]["producer"]["kind"],
                "pending"
            );
        }
    }
}

#[test]
fn losing_the_guide_during_a_live_writer_keeps_the_output_pending() {
    use std::fmt::Write as _;
    let mut source = String::new();
    for i in 0..70_000 {
        writeln!(
            source,
            "pub fn proof_{i}(n: u64) -> u64 {{ n.wrapping_add({i}) }}"
        )
        .unwrap();
    }
    let slot = producer_slot_with_source(Some(&source));
    let (lock, _) = cargo_hive_state::slots::open_slot_lock(&slot.lock_path()).unwrap();
    let guard = cargo_hive_platform::flock::try_exclusive(&lock)
        .unwrap()
        .unwrap();
    let scope = slot
        .fixture
        .replay("producer", &slot.pool, 1, 1, &slot.operation);
    let input_dir = &slot.fixture._repo_dir.path;
    let start_path = input_dir.join("producer-start");
    let watch_path = input_dir.join("producer-watch");
    let watch_prefix_path = input_dir.join("producer-watch-prefix");
    let started = input_dir.join("producer-start-result");
    let ended = input_dir.join("producer-watch-result");
    let mut payload = producer_input(&slot);
    let write_private = |path: &Path, contents: &str| {
        fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .unwrap()
            .write_all(contents.as_bytes())
            .unwrap();
    };
    write_private(&start_path, &payload.to_string());
    payload["action"] = json!("watch");
    write_private(&watch_prefix_path, &format!("{}\n", payload));
    let mut guide = slot.fixture.hive.command_with(Path::new("/bin/bash"), &[]);
    guide.args(["--noprofile", "--norc", "-c", r#"
set -m
bin=$1 start=$2 watch=$3 prefix=$4 started=$5 ended=$6
shift 6
args=()
while [[ $1 != --gate ]]; do args+=("$1"); shift; done
shift
"$bin" "${args[@]}" --lock-fd 198 < "$start" > "$started" || exit 41
/bin/bash --noprofile --norc -c 'builtin kill -STOP "$$"; builtin exec -c /usr/bin/env -i "$@"' producer-gate "$@" &
cargo_pid=$!
IFS= read -r payload < "$prefix"
(umask 077; printf '%s,"child_pid":%s}
' "${payload%?}" "$cargo_pid" > "$watch")
"$bin" "${args[@]}" --lock-fd 198 < "$watch" > "$ended" &
monitor=$!
wait "$monitor"
wait "$cargo_pid"
"#, "guide-fixture", env!("CARGO_BIN_EXE_cargo-hive")])
        .args([&start_path, &watch_path, &watch_prefix_path, &started, &ended])
        .args(scope)
        .arg("--gate")
        .args(producer_gate_args(&slot))
        .current_dir(&slot.path)
        .process_group(0)
        .env("CARGO_HOME", &slot.fixture.hive.cargo_home.path)
        .env("HIVE_HOME", &slot.fixture.hive.home.path)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    producer_environment(&mut guide);
    pass_producer_lock(&mut guide, &lock);
    let mut guide = guide.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let (cargo_pid, born) = loop {
        let record = slot.fixture.record();
        let pending = &record["pools"][0]["slots"][0]["producer"];
        let foreground = &pending["foreground"];
        if let (Some(pid), Some(secs), Some(micros)) = (
            foreground["pid"].as_u64(),
            foreground["start_secs"].as_u64(),
            foreground["start_micros"].as_u64(),
        ) && slot.path.join("target/debug").exists()
        {
            break (pid as i32, (secs, micros));
        }
        if guide.try_wait().unwrap().is_some() || Instant::now() >= deadline {
            let _ = guide.kill();
            let _ = guide.wait();
            if let Ok(data) = fs::read(&watch_path)
                && let Ok(value) = serde_json::from_slice::<Value>(&data)
                && let Some(pid) = value["child_pid"].as_u64()
            {
                // SAFETY: this is the guide fixture's dedicated writer process group.
                unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
            }
            panic!(
                "guide did not reach a live Cargo writer: {:?} {:?}",
                fs::read_to_string(started),
                fs::read_to_string(ended)
            );
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(running_fixture_writer(cargo_pid, born));
    guide.kill().unwrap();
    guide.wait().unwrap();
    assert!(
        running_fixture_writer(cargo_pid, born),
        "fixture writer exited before guide loss"
    );
    drop(guard);
    let deadline = Instant::now() + Duration::from_secs(30);
    while running_fixture_writer(cargo_pid, born) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    if running_fixture_writer(cargo_pid, born) {
        // SAFETY: this is the process group of the test's recorded Cargo writer.
        unsafe { libc::kill(-cargo_pid, libc::SIGKILL) };
        panic!("fixture writer failed to exit");
    }
    let record = slot.fixture.record();
    assert_eq!(
        record["pools"][0]["slots"][0]["producer"]["kind"],
        "pending"
    );
    let (_, status) = slot.fixture.status();
    assert_eq!(status["slots"][0]["phase"], "held");
}

fn running_fixture_writer(pid: i32, born: (u64, u64)) -> bool {
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = size_of::<libc::proc_bsdinfo>() as i32;
    // SAFETY: PROC_PIDTBSDINFO writes one proc_bsdinfo into the sized buffer.
    let read = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size,
        )
    };
    if read != size {
        return false;
    }
    // SAFETY: the return count confirmed the complete structure.
    let info = unsafe { info.assume_init() };
    info.pbi_pid == pid as u32
        && info.pbi_pgid == pid as u32
        && (info.pbi_start_tvsec, info.pbi_start_tvusec) == born
        && info.pbi_status != libc::SZOMB
}

#[test]
fn uncovered_producer_shapes_are_refused_before_output_is_created() {
    for case in [
        "profile revision",
        "argv",
        "cwd",
        "environment",
        "layout",
        "discovered root",
        "graph input",
        "consumer anchor",
        "mutable input",
        "changed manifest",
        "new config",
    ] {
        let slot = producer_slot();
        let (lock, _) = cargo_hive_state::slots::open_slot_lock(&slot.lock_path()).unwrap();
        let _guard = cargo_hive_platform::flock::try_exclusive(&lock)
            .unwrap()
            .unwrap();
        let mut request = producer_input(&slot);
        let mut record = slot.fixture.record();
        let pool = &mut record["pools"][0];
        let expected = match case {
            "profile revision" => {
                request["shape"]["profile"]["revision"] = json!(2);
                "revision"
            }
            "argv" => {
                request["shape"]["argv"] =
                    json!(["check", "--locked", "--target-dir", "/somewhere"]);
                "command"
            }
            "cwd" => {
                request["shape"]["cwd"] = json!("src");
                "working directory"
            }
            "environment" => "environment",
            "layout" => {
                let wrong = if pool["profile"]["invocations"][0]["layout"] == "flat" {
                    "unit_dir"
                } else {
                    "flat"
                };
                pool["profile"]["invocations"][0]["layout"] = json!(wrong);
                pool["profile"]["proofs"][0]["layout"] = json!(wrong);
                "layout"
            }
            "discovered root" => {
                pool["profile"]["invocations"][0]["target_root"] = json!("elsewhere");
                pool["profile"]["invocations"][0]["build_root"] = json!("elsewhere");
                let roots = &mut pool["slots"][0]["producer"]["evidence"]["roots"];
                roots["target"]["path"] = json!(slot.path.join("elsewhere"));
                roots["build"]["path"] = json!(slot.path.join("elsewhere"));
                "output roots"
            }
            "graph input" => {
                pool["profile"]["graph_inputs"]
                    .as_array_mut()
                    .unwrap()
                    .pop();
                "Cargo graph"
            }
            "consumer anchor" => {
                let consumer = &mut pool["profile"]["consumers"][0];
                consumer["prior_object"] = json!("f".repeat(40));
                consumer["requested_object"] = json!("e".repeat(40));
                "consumer anchor"
            }
            "mutable input" => {
                fs::write(slot.path.join(".env"), "PRODUCER_TEST=off-profile\n").unwrap();
                "outside verified generated output"
            }
            "changed manifest" => {
                let manifest = slot.path.join("Cargo.toml");
                let file = fs::OpenOptions::new().append(true).open(&manifest).unwrap();
                let mtime = file.metadata().unwrap().modified().unwrap();
                (&file).write_all(b"# changed fixed bytes\n").unwrap();
                file.set_times(fs::FileTimes::new().set_modified(mtime))
                    .unwrap();
                "tracked or index changes"
            }
            "new config" => {
                let ancestor = &slot.fixture.hive.secondary_root.path;
                fs::create_dir(ancestor.join(".cargo")).unwrap();
                fs::write(
                    ancestor.join(".cargo/config.toml"),
                    "[build]\n target-dir = 'elsewhere'\n",
                )
                .unwrap();
                "configuration path"
            }
            _ => unreachable!("test case"),
        };
        fs::write(slot.fixture.file(), serde_json::to_vec(&record).unwrap()).unwrap();
        let before = fs::read(slot.fixture.file()).unwrap();
        let (out, result) = finish_private(start_producer(
            &slot,
            1,
            &slot.operation,
            &request,
            Some(&lock),
            |command| {
                if case == "environment" {
                    command.env("RUSTC_WRAPPER", "/somewhere");
                }
            },
        ));
        assert_eq!(out.status.code(), Some(1), "{case}: {result}");
        assert!(
            result["detail"].as_str().unwrap().contains(expected),
            "{case}: {result}"
        );
        assert_no_credential(&out, &slot.credential);
        assert!(!slot.path.join("target").exists(), "{case}");
        assert!(!slot.path.join("elsewhere").exists(), "{case}");
        assert_eq!(fs::read(slot.fixture.file()).unwrap(), before, "{case}");
    }
}

#[test]
fn a_lost_start_and_stale_generation_cannot_be_given_a_new_writer() {
    let slot = producer_slot();
    let (lock, _) = cargo_hive_state::slots::open_slot_lock(&slot.lock_path()).unwrap();
    let guard = cargo_hive_platform::flock::try_exclusive(&lock)
        .unwrap()
        .unwrap();
    let request = producer_input(&slot);
    let run = |generation: u64, op: &str| {
        finish_private(start_producer(
            &slot,
            generation,
            op,
            &request,
            Some(&lock),
            |_| (),
        ))
    };
    let (stale, _) = run(0, &slot.operation);
    assert_eq!(stale.status.code(), Some(1));
    let (start, result) = run(1, &slot.operation);
    assert!(start.status.success(), "{result}");
    let pending = fs::read(slot.fixture.file()).unwrap();
    for op in [slot.operation.clone(), "8".repeat(32)] {
        let (again, _) = run(1, &op);
        assert_eq!(again.status.code(), Some(1));
        assert_eq!(fs::read(slot.fixture.file()).unwrap(), pending);
    }
    drop(guard);
    let (returned, result) = finish_private(slot.start("return"));
    assert_eq!(returned.status.code(), Some(1), "{result}");
    assert_eq!(fs::read(slot.fixture.file()).unwrap(), pending);
    let (_, status) = slot.fixture.status();
    assert_eq!(status["slots"][0]["phase"], "held");
}

#[test]
fn a_successful_off_protocol_build_cannot_restore_lost_provenance() {
    for reason in ["tainted", "unknown"] {
        let slot = producer_slot();
        let mut record = slot.fixture.record();
        let production = &mut record["pools"][0]["slots"][0]["producer"];
        *production = json!({
            "kind": reason, "epoch": production["evidence"]["epoch"], "generation": 1,
            "roots": {"target": null, "build": null},
            "reason": if reason == "tainted" { "off_profile" } else { "lost_writer" }
        });
        fs::write(slot.fixture.file(), serde_json::to_vec(&record).unwrap()).unwrap();
        slot.fixture.hive.check_crate(&slot.path);
        assert!(slot.path.join("target").is_dir());
        let (lock, _) = cargo_hive_state::slots::open_slot_lock(&slot.lock_path()).unwrap();
        let _guard = cargo_hive_platform::flock::try_exclusive(&lock)
            .unwrap()
            .unwrap();
        let (out, result) = finish_private(start_producer(
            &slot,
            1,
            &slot.operation,
            &producer_input(&slot),
            Some(&lock),
            |_| (),
        ));
        assert_eq!(out.status.code(), Some(1), "{result}");
        assert_eq!(
            slot.fixture.record()["pools"][0]["slots"][0]["producer"]["kind"],
            reason
        );
    }
}

#[test]
fn completed_history_limit_does_not_ask_to_acknowledge_absent_outcomes() {
    let slot = producer_slot();
    let target = slot.path.join("target");
    fs::create_dir(&target).unwrap();
    slot.retain(&target);
    let mut record = slot.fixture.record();
    let state = &mut record["pools"][0]["slots"][0];
    let template = state["producer"]["invocations"][0].clone();
    state["producer"]["invocations"] = json!(
        (0..32)
            .map(|number| {
                let mut entry = template.clone();
                entry["operation"] = json!(format!("{number:032x}"));
                entry["foreground"] = json!({"pid": 2, "start_secs": 1, "start_micros": 0});
                entry
            })
            .collect::<Vec<_>>()
    );
    assert_eq!(state["outcomes"], json!([]));
    fs::write(slot.fixture.file(), serde_json::to_vec(&record).unwrap()).unwrap();
    let (lock, _) = cargo_hive_state::slots::open_slot_lock(&slot.lock_path()).unwrap();
    let _guard = cargo_hive_platform::flock::try_exclusive(&lock)
        .unwrap()
        .unwrap();
    let (out, result) = finish_private(start_producer(
        &slot,
        1,
        &slot.operation,
        &producer_input(&slot),
        Some(&lock),
        |_| (),
    ));
    assert_eq!(out.status.code(), Some(1), "{result}");
    assert!(
        result["detail"]
            .as_str()
            .unwrap()
            .contains("completed Cargo invocations"),
        "{result}"
    );
    assert!(
        result["next_action"].as_str().unwrap().contains("return"),
        "{result}"
    );
    assert_eq!(slot.fixture.record(), record);
}

type PathSnapshot = (PathBuf, u64, u64, i64, i64, Option<Vec<u8>>);

fn snapshot_paths(paths: &[&Path]) -> Vec<PathSnapshot> {
    fn record(path: &Path, inventory: &mut Vec<PathSnapshot>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let bytes = if metadata.is_file() {
            Some(fs::read(path).unwrap())
        } else if metadata.is_symlink() {
            Some(fs::read_link(path).unwrap().as_os_str().as_bytes().to_vec())
        } else {
            None
        };
        inventory.push((
            path.to_owned(),
            metadata.dev(),
            metadata.ino(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            bytes,
        ));
        if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                record(&entry.unwrap().path(), inventory);
            }
        }
    }
    let mut inventory = Vec::new();
    for path in paths {
        record(path, &mut inventory);
    }
    inventory.sort();
    inventory
}

#[test]
fn tracked_edits_leave_the_checkout_index_and_lease_untouched_until_resolved() {
    let slot = LeasedSlot::new();
    fs::write(
        slot.path.join("src/lib.rs"),
        "pub fn answer() -> u32 { 43 }\n",
    )
    .unwrap();
    slot.assert_refused("tracked or index changes");
    git(&slot.path, &["add", "src/lib.rs"]);
    slot.assert_refused("tracked or index changes");
    git(&slot.path, &["commit", "-qm", "resolve the holder's edit"]);
    let resolved = git(&slot.path, &["rev-parse", "HEAD"]);
    assert_ne!(resolved, slot.head);
    let before = snapshot_paths(&[&slot.path, &slot.admin]);
    let (out, returned) = finish_private(slot.start("return"));
    assert!(out.status.success(), "{returned}");
    assert_eq!(returned["kind"], "returned");
    assert_eq!(returned["phase"], "vacant");
    assert_eq!(returned["commit"], resolved);
    assert_eq!(returned["current"], false);
    assert_eq!(
        slot.fixture.record()["pools"][0]["slots"][0]["last_head"],
        resolved
    );
    assert_eq!(snapshot_paths(&[&slot.path, &slot.admin]), before);
}

#[test]
fn return_requires_the_filesystem_type_recorded_in_the_git_index() {
    let slot = LeasedSlot::new();
    let tracked = slot.path.join("source-link");
    symlink("src/lib.rs", &tracked).unwrap();
    git(&slot.path, &["add", "source-link"]);
    git(&slot.path, &["commit", "-qm", "record the symlink"]);
    assert!(git(&slot.path, &["ls-files", "--stage", "source-link"]).starts_with("120000 "));
    git(&slot.path, &["config", "core.symlinks", "false"]);
    fs::remove_file(&tracked).unwrap();
    fs::write(&tracked, b"src/lib.rs").unwrap();
    let output = slot.path.join("target");
    fs::create_dir(&output).unwrap();
    fs::write(output.join("retained.rlib"), b"retained output").unwrap();
    slot.retain(&output);
    slot.assert_refused("tracked or index changes");

    fs::remove_file(&tracked).unwrap();
    symlink("src/lib.rs", &tracked).unwrap();
    let protected = [&*slot.path, &*slot.admin];
    let before = snapshot_paths(&protected);
    let (out, result) = finish_private(slot.start("return"));
    assert!(out.status.success(), "{result}");
    assert_eq!(snapshot_paths(&protected), before);
}

#[test]
fn an_unmerged_index_and_an_unfinished_git_operation_preserve_every_path() {
    let slot = LeasedSlot::new();
    git(&slot.path, &["switch", "-qc", "conflicting-lane"]);
    fs::write(
        slot.path.join("src/lib.rs"),
        "pub fn answer() -> u32 { 2 }\n",
    )
    .unwrap();
    git(&slot.path, &["commit", "-qam", "lane change"]);
    git(&slot.path, &["switch", "--detach", &slot.head]);
    fs::write(
        slot.path.join("src/lib.rs"),
        "pub fn answer() -> u32 { 3 }\n",
    )
    .unwrap();
    git(&slot.path, &["commit", "-qam", "conflicting change"]);
    let attempted = slot
        .fixture
        .hive
        .command_with(
            Path::new("/usr/bin/git"),
            &[
                "-c",
                "core.fsmonitor=false",
                "-C",
                slot.path.to_str().unwrap(),
                "merge",
                "--no-edit",
                "conflicting-lane",
            ],
        )
        .output()
        .unwrap();
    assert!(!attempted.status.success(), "{attempted:?}");
    assert!(git(&slot.path, &["ls-files", "--unmerged"]).contains("src/lib.rs"));
    slot.assert_refused("unfinished Git operation");
    fs::remove_file(slot.admin.join("MERGE_HEAD")).unwrap();
    let _ = fs::remove_file(slot.admin.join("AUTO_MERGE"));
    slot.assert_refused("tracked or index changes");
}

#[test]
fn ignored_dotenv_untracked_sqlx_and_unproved_targets_are_left_in_place() {
    let slot = LeasedSlot::new();
    fs::write(slot.fixture.repo.join(".git/info/exclude"), ".env\n").unwrap();
    fs::write(slot.path.join(".env"), b"LOCAL_SECRET=fixture-only\n").unwrap();
    assert_eq!(git(&slot.path, &["check-ignore", ".env"]), ".env");
    slot.assert_refused("untracked or ignored material");
    fs::remove_file(slot.path.join(".env")).unwrap();
    fs::create_dir(slot.path.join(".sqlx")).unwrap();
    fs::write(slot.path.join(".sqlx/query.json"), b"fixture query").unwrap();
    slot.assert_refused("untracked or ignored material");
    fs::remove_dir_all(slot.path.join(".sqlx")).unwrap();
    fs::create_dir(slot.path.join("target")).unwrap();
    fs::write(slot.path.join("target/borrowed"), b"not proved generated").unwrap();
    slot.assert_refused("retained output root identity");
}

#[test]
fn retained_output_identity_excludes_input_probes_symlinks_and_nested_checkouts() {
    let slot = LeasedSlot::new();
    let target = slot.path.join("target");
    fs::create_dir_all(target.join("debug")).unwrap();
    fs::write(target.join("debug/fixture.rmeta"), b"retained metadata").unwrap();
    slot.retain(&target);
    fs::write(target.join("debug/.ENV"), b"disguised ignored input").unwrap();
    slot.assert_refused("possible input");
    fs::remove_file(target.join("debug/.ENV")).unwrap();
    symlink(slot.path.join("Cargo.toml"), target.join("debug/alias")).unwrap();
    slot.assert_refused("symlink");
    fs::remove_file(target.join("debug/alias")).unwrap();
    fs::hard_link(
        slot.path.join("Cargo.toml"),
        target.join("debug/linked-input"),
    )
    .unwrap();
    slot.assert_refused("exclusive generated use");
    fs::remove_file(target.join("debug/linked-input")).unwrap();
    git(&target.join("debug"), &["init", "-q"]);
    slot.assert_refused("nested checkout");
    fs::remove_dir_all(target.join("debug/.git")).unwrap();

    let file = slot.fixture.file();
    let previous = fs::read(&file).unwrap();
    let mut record = slot.fixture.record();
    record["pools"][0]["profile"]["graph_inputs"]
        .as_array_mut()
        .unwrap()
        .push(json!({
            "locator": {"kind": "checkout", "path": "target/debug/negative-input"},
            "expectation": {"kind": "absent"}
        }));
    fs::write(&file, serde_json::to_vec(&record).unwrap()).unwrap();
    assert!(!target.join("debug/negative-input").exists());
    slot.assert_refused("possible input");
    fs::write(&file, &previous).unwrap();

    let mut overlapping = slot.fixture.record();
    let debug = target.join("debug");
    let metadata = fs::metadata(&debug).unwrap();
    overlapping["pools"][0]["profile"]["invocations"][0]["build_root"] = json!("target/debug");
    overlapping["pools"][0]["slots"][0]["producer"]["roots"]["build"] = json!({
        "path": debug, "identity": {"dev": metadata.dev(), "ino": metadata.ino()}
    });
    fs::write(&file, serde_json::to_vec(&overlapping).unwrap()).unwrap();
    slot.assert_refused("exclusive generated use");
    fs::write(&file, &previous).unwrap();

    git(&slot.path, &["add", "-f", "target/debug/fixture.rmeta"]);
    git(&slot.path, &["commit", "-qm", "disguised tracked input"]);
    slot.assert_refused("possible input");
    git(
        &slot.path,
        &["rm", "--cached", "target/debug/fixture.rmeta"],
    );
    git(&slot.path, &["commit", "-qm", "restore generated output"]);

    let moved = target.with_file_name("old-target");
    fs::rename(&target, &moved).unwrap();
    fs::create_dir(&target).unwrap();
    slot.assert_refused("retained output root identity");
    fs::remove_dir(&target).unwrap();
    fs::rename(moved, &target).unwrap();
    git(&slot.path, &["switch", "-qc", "kept-holder-branch"]);
    let main_branch = git(&slot.fixture.repo, &["symbolic-ref", "--short", "HEAD"]);
    git(&slot.path, &["branch", "--set-upstream-to", &main_branch]);
    git(&slot.path, &["config", "status.branch", "true"]);

    let expected = snapshot_paths(&[&slot.path, &slot.fixture.repo]);
    let (out, returned) = finish_private(slot.start("return"));
    assert!(out.status.success(), "{returned}");
    assert_eq!(returned["kind"], "returned");
    assert_eq!(snapshot_paths(&[&slot.path, &slot.fixture.repo]), expected);
    assert_eq!(
        git(&slot.path, &["symbolic-ref", "--short", "HEAD"]),
        "kept-holder-branch"
    );
}

#[test]
fn a_live_writer_and_lost_writer_custody_each_preserve_the_lease() {
    let slot = LeasedSlot::new();
    let lock_path = slot.lock_path();
    let mut locking_writer =
        common::KillOnDrop(slot.fixture.hive.hold_lock_child("flock", &lock_path));
    slot.assert_refused("writer may still hold the slot operation lock");
    locking_writer.0.kill().unwrap();
    locking_writer.0.wait().unwrap();
    let mut record = slot.fixture.record();
    let lease = &mut record["pools"][0]["slots"][0];
    let initial = &lease["producer"]["evidence"];
    let epoch = initial["epoch"].clone();
    lease["producer"] = json!({
        "kind": "pending", "epoch": epoch, "generation": 1,
        "profile": {"id": "fixture", "revision": 1},
        "invocation": {
            "invocation": "check", "graph_digest": "a".repeat(64),
            "environment_digest": "b".repeat(64)
        },
        "roots": initial["roots"].clone(),
        "operation": "1".repeat(32), "runner": {
            "boot_session": "boot", "pid": 1, "start_secs": 1,
            "start_micros": 0, "nonce": "c".repeat(64)
        },
        "prior": {"kind": "never_built", "evidence": initial}
    });
    fs::write(slot.fixture.file(), serde_json::to_vec(&record).unwrap()).unwrap();
    slot.assert_refused("prior writer may still change");

    record["pools"][0]["slots"][0]["producer"] = json!({
        "kind": "unknown", "epoch": epoch, "generation": 1,
        "roots": {"target": null, "build": null}, "reason": "lost_writer"
    });
    fs::write(slot.fixture.file(), serde_json::to_vec(&record).unwrap()).unwrap();
    slot.assert_refused("provenance is unproved");
}

#[test]
fn return_refuses_an_active_cargo_lock_in_a_verified_retained_root() {
    let slot = LeasedSlot::new();
    let root = slot.path.join("target");
    fs::create_dir_all(root.join("debug")).unwrap();
    let lock = root.join("debug/.cargo-lock");
    fs::write(&lock, b"").unwrap();
    slot.retain(&root);
    let mut writer = common::KillOnDrop(slot.fixture.hive.hold_lock_child("flock", &lock));
    slot.assert_refused("Cargo lock");
    writer.0.kill().unwrap();
    writer.0.wait().unwrap();
    let (out, returned) = finish_private(slot.start("return"));
    assert!(out.status.success(), "{returned}");
}

#[test]
fn return_refuses_canonical_cargo_locks_with_filesystem_aliases() {
    for alias in [".CARGO-BUILD-LOCK", ".cargo-build-locK"] {
        let slot = LeasedSlot::new();
        let root = slot.path.join("target");
        let output = root.join("debug");
        fs::create_dir_all(&output).unwrap();
        let stored = output.join(alias);
        let canonical = output.join(".cargo-build-lock");
        fs::write(&stored, b"").unwrap();
        match fs::metadata(&canonical) {
            Ok(aliased) => {
                let original = fs::metadata(&stored).unwrap();
                assert_eq!(
                    (aliased.dev(), aliased.ino()),
                    (original.dev(), original.ino())
                );
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                fs::rename(&stored, &canonical).unwrap();
            }
            Err(err) => panic!("could not inspect the canonical Cargo lock: {err}"),
        }
        slot.retain(&root);
        let mut writer = common::KillOnDrop(slot.fixture.hive.hold_lock_child("flock", &canonical));
        slot.assert_refused("Cargo lock");
        writer.0.kill().unwrap();
        writer.0.wait().unwrap();
        let (out, returned) = finish_private(slot.start("return"));
        assert!(out.status.success(), "{returned}");
    }
}

#[test]
fn return_refuses_checkout_substitution_symlink_and_overlapping_owner_paths() {
    let slot = LeasedSlot::new();
    let owner = slot.fixture._repo_dir.path.join("owner");
    git(
        &slot.fixture.repo,
        &[
            "worktree",
            "add",
            "--detach",
            owner.to_str().unwrap(),
            "HEAD",
        ],
    );
    let moved = slot.path.with_file_name("saved-slot");
    fs::rename(&slot.path, &moved).unwrap();
    fs::create_dir(&slot.path).unwrap();
    fs::copy(moved.join(".git"), slot.path.join(".git")).unwrap();
    slot.assert_refused("exact slot checkout");
    fs::remove_dir_all(&slot.path).unwrap();
    for alias in [&slot.fixture.repo, &owner, &moved] {
        symlink(alias, &slot.path).unwrap();
        slot.assert_refused("exact slot checkout");
        fs::remove_file(&slot.path).unwrap();
    }
    fs::rename(&moved, &slot.path).unwrap();
    let checkout = slot.path.join("owner-worktree");
    git(
        &slot.fixture.repo,
        &[
            "worktree",
            "add",
            "--detach",
            checkout.to_str().unwrap(),
            "HEAD",
        ],
    );
    slot.assert_refused("overlaps a registered main or owner worktree");
}

#[test]
fn a_lost_return_reply_can_be_replayed_after_the_next_reservation() {
    let slot = LeasedSlot::new();
    let mut child = slot.start("return");
    drop(child.stdout.take().unwrap());
    let lost = common::wait_within(child, Duration::from_secs(15));
    assert_eq!(lost.status.code(), Some(1), "{lost:?}");
    assert!(String::from_utf8_lossy(&lost.stderr).contains("could not deliver the slot result"));
    let saved = slot.fixture.record();
    assert_eq!(saved["pools"][0]["slots"][0]["phase"]["kind"], "vacant");
    let (out, replay) = finish_private(slot.start("return"));
    assert!(out.status.success(), "{replay}");
    assert_eq!(replay["kind"], "returned");
    assert_eq!(replay["current"], false);
    assert_eq!(replay["commit"], slot.head);
    assert_eq!(slot.fixture.record(), saved);
    let (out, next) = finish_private(start_private(
        &slot.fixture.hive,
        slot.fixture
            .reservation(&slot.pool, 1, 1, &slot.head, &"7".repeat(32)),
        &"3".repeat(64),
    ));
    assert!(out.status.success(), "{next}");
    assert_eq!(next["generation"], 2);
    let mut next_holder = common::KillOnDrop(
        slot.fixture
            .hive
            .hold_lock_child("flock", &slot.lock_path()),
    );
    let paths = [&*slot.path, &*slot.admin, &*slot.fixture.file()];
    let next_state = snapshot_paths(&paths);
    for command in ["return", "outcome"] {
        let (out, previous) = finish_private(slot.start(command));
        assert!(out.status.success(), "{previous}");
        assert_eq!(previous, replay);
        assert!(previous["path"].is_null(), "{previous}");
        assert_no_credential(&out, &slot.credential);
        assert_eq!(snapshot_paths(&paths), next_state);
    }
    let registry = slot.fixture.hive.home.path.join("registry.json");
    let original_registry = fs::read(&registry).unwrap();
    let mut unregistered: Value = serde_json::from_slice(&original_registry).unwrap();
    unregistered["repos"] = json!([]);
    fs::write(&registry, serde_json::to_vec(&unregistered).unwrap()).unwrap();
    slot.assert_refused("exact registered main checkout");
    fs::write(&registry, original_registry).unwrap();
    next_holder.0.kill().unwrap();
    next_holder.0.wait().unwrap();
    let (out, acknowledged) = finish_private(slot.start("ack"));
    assert!(out.status.success(), "{acknowledged}");
    assert_eq!(acknowledged["kind"], "acknowledged");
    let (out, stale) = finish_private(slot.start("return"));
    assert_eq!(out.status.code(), Some(1), "{stale}");
}

#[test]
fn a_historical_return_refuses_a_different_repo_registered_at_the_same_path() {
    let slot = LeasedSlot::new();
    let (out, returned) = finish_private(slot.start("return"));
    assert!(out.status.success(), "{returned}");
    let main = &slot.fixture.repo;
    let removed = slot
        .fixture
        .hive
        .run(&["repo", "remove", main.to_str().unwrap()]);
    assert!(removed.status.success(), "{removed:?}");
    let original = main.with_file_name("original-main");
    fs::rename(main, &original).unwrap();
    init_repo(main, "");
    git(
        main,
        &["commit", "--amend", "-qm", "replacement repository"],
    );
    register(&slot.fixture.hive, main);
    let original_git = fs::metadata(original.join(".git")).unwrap();
    let replacement_git = fs::metadata(main.join(".git")).unwrap();
    assert_ne!(
        (original_git.dev(), original_git.ino()),
        (replacement_git.dev(), replacement_git.ino())
    );

    let state = slot.fixture.file();
    let registry = slot.fixture.hive.home.path.join("registry.json");
    let paths = [&*slot.path, &**main, &*original, &*state, &*registry];
    let before = snapshot_paths(&paths);
    for command in ["return", "outcome"] {
        let (out, refused) = finish_private(slot.start(command));
        assert_eq!(out.status.code(), Some(1), "{refused}");
        assert_eq!(refused["kind"], "refused", "{refused}");
        assert!(
            refused["detail"]
                .as_str()
                .unwrap()
                .contains("pool's identity"),
            "{refused}"
        );
        assert!(refused["path"].is_null(), "{refused}");
        assert_no_credential(&out, &slot.credential);
        assert_eq!(snapshot_paths(&paths), before);
    }
}

#[test]
fn an_eof_confirmed_empty_tracked_tree_can_be_returned() {
    let slot = LeasedSlot::new();
    git(&slot.path, &["rm", "-rq", "."]);
    git(&slot.path, &["commit", "-qm", "remove the fixture tree"]);
    assert!(git(&slot.path, &["ls-files", "--stage", "-v", "-z"]).is_empty());
    let head = git(&slot.path, &["rev-parse", "HEAD"]);
    let before = snapshot_paths(&[&slot.path, &slot.admin]);
    let (out, returned) = finish_private(slot.start("return"));
    assert!(out.status.success(), "{returned}");
    assert_eq!(returned["commit"], head);
    assert_eq!(snapshot_paths(&[&slot.path, &slot.admin]), before);
}

#[allow(clippy::disallowed_types)]
fn with_status_git_shim(
    slot: &LeasedSlot,
    script: &str,
) -> impl FnOnce(&mut std::process::Command) + use<> {
    let fixture = &slot.fixture;
    let scratch = &fixture._repo_dir.path;
    let real_git = fixture
        .hive
        .command_with(Path::new("/bin/sh"), &["-c", "command -v git"])
        .output()
        .unwrap();
    assert!(real_git.status.success(), "{real_git:?}");
    let real_git = String::from_utf8(real_git.stdout).unwrap();
    let bin = scratch.join("bin");
    let path = common::counting_git(&bin, &scratch.join("git.log"));
    fs::write(
        bin.join("git"),
        format!(
            "#!/bin/sh\nfor arg do\n    if [ \"$arg\" = status ]; then\n{script}\n    fi\ndone\nexec \"$REAL_GIT\" \"$@\"\n"
        ),
    )
    .unwrap();
    move |command| {
        command.env("PATH", path).env("REAL_GIT", real_git.trim());
    }
}

#[test]
fn return_refuses_a_successful_git_status_with_an_open_stderr_pipe() {
    let slot = LeasedSlot::new();
    let shim = with_status_git_shim(&slot, "sleep 4 >/dev/null &\nexec \"$REAL_GIT\" \"$@\"");
    slot.assert_refused_with("complete checkout inventory", shim);
}

#[test]
fn return_refuses_a_malformed_optional_git_status_header() {
    let slot = LeasedSlot::new();
    let shim = with_status_git_shim(
        &slot,
        &format!(
            "printf '# branch.oid {}\\000# branch.head fixture\\000# branch.upstream main\\000# branch.ab +1 -\\000'\nexit 0",
            slot.head
        ),
    );
    slot.assert_refused_with("tracked or index changes", shim);
}

#[test]
fn return_refuses_before_a_configured_git_clean_filter_can_write() {
    fn configure_filter(checkout: &Path) -> &'static [u8] {
        let source = checkout.join("src/lib.rs");
        let edited = b"pub fn answer() -> u32 { 43 }\n";
        assert_eq!(fs::metadata(&source).unwrap().len(), edited.len() as u64);
        fs::write(
            checkout.join(".gitattributes"),
            "src/lib.rs filter=fixture\n",
        )
        .unwrap();
        git(checkout, &["add", ".gitattributes"]);
        git(
            checkout,
            &["commit", "-qm", "require a clean filter for source"],
        );
        git(
            checkout,
            &["config", "filter.fixture.clean", "tee clean-marker"],
        );
        fs::write(source, edited).unwrap();
        edited
    }

    let isolated = Scratch::new("slot-filter-positive");
    let control = init_repo(&isolated.path.join("control"), "");
    let edited = configure_filter(&control);
    assert!(!control.join("clean-marker").exists());
    let raw = git(
        &control,
        &[
            "--no-optional-locks",
            "--no-replace-objects",
            "-c",
            "core.filemode=true",
            "-c",
            "core.trustctime=true",
            "-c",
            "core.checkStat=default",
            "status",
            "--porcelain=v2",
            "-z",
            "--no-branch",
            "--no-show-stash",
            "--untracked-files=no",
            "--ignore-submodules=none",
        ],
    );
    assert!(raw.contains("src/lib.rs"), "{raw:?}");
    assert_eq!(fs::read(control.join("clean-marker")).unwrap(), edited);

    let slot = LeasedSlot::new();
    configure_filter(&slot.path);
    assert!(!slot.path.join("clean-marker").exists());
    // A linked checkout's admin directory is included through the main's common Git tree.
    slot.assert_refused("configured Git clean filter");
    assert!(!slot.path.join("clean-marker").exists());
}

#[test]
fn return_accepts_an_exact_tracked_index_inventory_and_refuses_one_extra_byte() {
    const LIMIT: usize = 4 * 1024 * 1024;
    const RECORD_BYTES: usize = 752;
    let slot = LeasedSlot::new();
    let tracked_len = || git(&slot.path, &["ls-files", "--stage", "-v", "-z"]).len();
    let remaining = LIMIT - tracked_len();
    let records = remaining.div_ceil(RECORD_BYTES);
    let overhead = format!("H 100644 {} 0\t\0", slot.head).len();
    let component = "dir".repeat(60);
    let directory = PathBuf::from(&component).join(&component).join(&component);
    fs::create_dir_all(slot.path.join(&directory)).unwrap();
    let mut last_entry = PathBuf::new();
    for number in 0..records {
        let bytes = remaining / records + usize::from(number < remaining % records);
        let stem = format!("{number:05}-");
        let padding = bytes - overhead - directory.as_os_str().len() - 1 - stem.len();
        last_entry = slot.path.join(&directory).join(stem + &"x".repeat(padding));
        fs::write(&last_entry, b"").unwrap();
    }
    let commit_all = || {
        git(&slot.path, &["add", "-A"]);
        git(
            &slot.path,
            &["commit", "-qm", "record the tracked inventory"],
        );
    };
    let mut extra_entry = last_entry.clone().into_os_string();
    extra_entry.push("x");
    fs::rename(&last_entry, &extra_entry).unwrap();
    commit_all();
    assert_eq!(tracked_len(), LIMIT + 1);
    slot.assert_refused("complete checkout inventory");
    fs::rename(&extra_entry, &last_entry).unwrap();
    commit_all();
    assert_eq!(tracked_len(), LIMIT);
    let (out, result) = finish_private(slot.start("return"));
    assert!(out.status.success(), "{result}");
    assert_eq!(result["kind"], "returned");
}

#[test]
fn return_accepts_the_exact_checkout_entry_limit_and_refuses_one_more() {
    const LIMIT: usize = 100_000;
    let slot = LeasedSlot::new();
    let target = slot.path.join("target");
    fs::create_dir(&target).unwrap();
    slot.retain(&target);
    let existing = snapshot_paths(&[&slot.path]).len() - 1;
    for number in 0..LIMIT - existing {
        fs::File::create(target.join(number.to_string())).unwrap();
    }
    assert_eq!(fs::read_dir(&target).unwrap().count() + existing, LIMIT);
    let extra = target.join("one-too-many");
    fs::File::create(&extra).unwrap();
    slot.assert_refused("complete checkout inventory");
    fs::remove_file(&extra).unwrap();
    let (out, result) = finish_private(slot.start("return"));
    assert!(out.status.success(), "{result}");
    assert_eq!(result["kind"], "returned");
}

#[test]
#[allow(clippy::disallowed_types)]
fn scoped_output_requires_complete_git_inventory_and_preserves_shipped_discovery() {
    const GIT_CAP: usize = 64 * 1024;
    let slot = LeasedSlot::new();
    let fixture = &slot.fixture;
    let linked = fixture.repo.join(".worktrees/lane");
    let owner = fixture._repo_dir.path.join("owner");
    for path in [&linked, &owner] {
        git(
            &fixture.repo,
            &[
                "worktree",
                "add",
                "--detach",
                path.to_str().unwrap(),
                "HEAD",
            ],
        );
    }
    let sandbox = &fixture._repo_dir.path;
    let real_git = fixture
        .hive
        .command_with(Path::new("/bin/sh"), &["-c", "command -v git"])
        .output()
        .unwrap();
    let real_git = std::str::from_utf8(&real_git.stdout).unwrap().trim();
    let path = common::counting_git(&sandbox.join("bin"), &sandbox.join("git.log"));
    fs::write(
        sandbox.join("bin/git"),
        r#"#!/bin/sh
if [ "$1" = --no-optional-locks ] && [ "${T4_REQUIRE_SCRUB:-}" = 1 ]; then
    [ "${GIT_NO_LAZY_FETCH:-}" = 1 ] && [ "${GIT_ALLOW_PROTOCOL-unset}" = "" ] &&
    [ "${CARGO_TARGET_DIR-unset}" = unset ] && [ "${CARGO_BUILD_TARGET_DIR-unset}" = unset ] &&
    [ "${CARGO_BUILD_BUILD_DIR-unset}" = unset ] || exit 101
fi
scope=missing
subcommand=missing
for arg do
    [ "$arg" != "$T4_LIST_SCOPE" ] || scope=matched
    [ "$arg" != worktree ] || subcommand=matched
done
if [ "$scope:$subcommand" = matched:matched ]; then
    exec /bin/cat "$T4_LIST"
fi
exec "$T4_REAL_GIT" "$@"
"#,
    )
    .unwrap();
    let list = sandbox.join("inventory");
    let actual = |repo: &Path| {
        let out = fixture
            .hive
            .command_with(
                Path::new(real_git),
                &[
                    "-C",
                    repo.to_str().unwrap(),
                    "worktree",
                    "list",
                    "--porcelain",
                    "-z",
                ],
            )
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        out.stdout
    };
    let scoped = |repo: &Path| {
        let scope = repo.to_owned();
        let path = &path;
        let list = &list;
        move |command: &mut std::process::Command| {
            command
                .env("PATH", path)
                .env("T4_REAL_GIT", real_git)
                .env("T4_LIST_SCOPE", scope)
                .env("T4_LIST", list)
                .env("T4_REQUIRE_SCRUB", "1")
                .env("GIT_NO_LAZY_FETCH", "0")
                .env("GIT_ALLOW_PROTOCOL", "file")
                .env("CARGO_TARGET_DIR", "unwanted-target")
                .env("CARGO_BUILD_TARGET_DIR", "unwanted-target")
                .env("CARGO_BUILD_BUILD_DIR", "unwanted-build");
        }
    };
    let mut valid = actual(&fixture.repo);
    assert!(valid.ends_with(b"\0\0"));
    assert_eq!(valid.pop(), Some(0));
    fs::write(&list, &valid).unwrap();
    slot.assert_refused_with("complete registered Git scope", scoped(&fixture.repo));

    fixture.hive.write_ready_capabilities();
    let (out, receipt) = fixture.hive.json_env(
        &["status", fixture.repo.to_str().unwrap()],
        &[
            ("PATH", &path),
            ("T4_REAL_GIT", real_git),
            ("T4_LIST_SCOPE", fixture.repo.to_str().unwrap()),
            ("T4_LIST", list.to_str().unwrap()),
        ],
    );
    assert!(out.status.success(), "{receipt}: {out:?}");
    for (checkout, class) in [
        (&fixture.repo, "checkout.protected"),
        (&linked, "checkout.secondary"),
        (&owner, "checkout.protected"),
        (&slot.path, "checkout.secondary"),
    ] {
        assert!(
            receipt["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|finding| { finding["path"] == json!(checkout) && finding["kind"] == class }),
            "{receipt}"
        );
    }

    let another = init_repo(&sandbox.join("other-registered"), "");
    register(&fixture.hive, &another);
    let incomplete = actual(&another);
    fs::write(&list, &incomplete[..incomplete.len() - 1]).unwrap();
    slot.assert_refused_with("complete registered Git scope", scoped(&another));

    valid.extend_from_slice(b"locked ");
    valid.resize(GIT_CAP - 2, b'x');
    valid.extend_from_slice(b"\0\0");
    assert_eq!(valid.len(), GIT_CAP);
    fs::write(&list, [&valid[..], b"w"].concat()).unwrap();
    slot.assert_refused_with("complete registered Git scope", scoped(&fixture.repo));
    fs::write(&list, valid).unwrap();
    let (out, returned) = finish_private(slot.start_with("return", scoped(&fixture.repo)));
    assert!(out.status.success(), "{returned}");
}

#[test]
fn competing_requests_can_reserve_the_last_vacant_slot_only_once() {
    let fixture = PoolFixture::new();
    let (out, initialized) = fixture.init();
    assert!(out.status.success(), "{initialized}");
    let pool = initialized["pool_id"].as_str().unwrap();
    let commit = git(&fixture.repo, &["rev-parse", "HEAD"]);
    let key = "a".repeat(64);
    let (out, occupied) = finish_private(start_private(
        &fixture.hive,
        fixture.reservation(pool, 1, 0, &commit, &"1".repeat(32)),
        &key,
    ));
    assert!(out.status.success(), "{occupied}");
    assert_eq!(occupied["kind"], "reserved");

    let mut lock = common::KillOnDrop(fixture.hive.hold_state_lock());
    let left = start_private(
        &fixture.hive,
        fixture.reservation(pool, 2, 0, &commit, &"2".repeat(32)),
        &"b".repeat(64),
    );
    let right = start_private(
        &fixture.hive,
        fixture.reservation(pool, 2, 0, &commit, &"3".repeat(32)),
        &"c".repeat(64),
    );
    lock.0.kill().unwrap();
    lock.0.wait().unwrap();

    let mut results = [finish_private(left), finish_private(right)];
    results.sort_by_key(|(out, _)| out.status.code());
    let [(winner, reserved), (loser, unavailable)] = results;
    assert!(winner.status.success(), "{reserved}");
    assert_eq!(reserved["kind"], "reserved");
    assert_eq!(reserved["slot"], 2);
    assert_eq!(reserved["generation"], 1);
    assert_eq!(reserved["current"], true);
    assert_eq!(loser.status.code(), Some(1), "{unavailable}");
    assert_eq!(unavailable["kind"], "unavailable");
    assert!(
        unavailable["next_action"]
            .as_str()
            .unwrap()
            .contains("separate fresh secondary checkout")
    );

    let record = fixture.record();
    assert_eq!(record["revision"], 3);
    for slot in record["pools"][0]["slots"].as_array().unwrap() {
        assert_eq!(slot["generation"], 1);
        assert_eq!(slot["outcomes"].as_array().unwrap().len(), 1);
    }
}

#[test]
fn exact_reservation_replay_confirms_only_the_original_request() {
    let fixture = PoolFixture::new();
    let (out, initialized) = fixture.init();
    assert!(out.status.success(), "{initialized}");
    let pool = initialized["pool_id"].as_str().unwrap();
    let first_commit = git(&fixture.repo, &["rev-parse", "HEAD"]);
    git(
        &fixture.repo,
        &["commit", "--allow-empty", "-m", "another commit"],
    );
    let second_commit = git(&fixture.repo, &["rev-parse", "HEAD"]);
    let operation = "1".repeat(32);
    let path = fixture._repo_dir.path.join("private-key");
    private_file(&path);
    assert!(
        key_command(&fixture, PRIVATE_KEY_COMMAND, &path, "3")
            .status
            .success()
    );
    let key = fs::read_to_string(path).unwrap();
    let other_key = "c".repeat(64);
    let exact = || fixture.reservation(pool, 1, 0, &first_commit, &operation);
    let (out, reserved) = finish_private(start_private(&fixture.hive, exact(), &key));
    assert!(out.status.success(), "{reserved}");
    assert!(reserved.get("path").is_none());
    assert_eq!(reserved["commit"], first_commit);
    assert_no_credential(&out, &key);
    let initial = fixture.record();

    let (out, again) = finish_private(start_private(&fixture.hive, exact(), &key));
    assert!(out.status.success(), "{again}");
    assert_eq!(again, reserved);
    assert_eq!(fixture.record()["revision"], initial["revision"]);
    let conflict = fixture.reservation(pool, 1, 0, &second_commit, &operation);
    let replay = || fixture.replay("outcome", pool, 1, 1, &operation);
    let ack = || fixture.replay("ack", pool, 1, 1, &operation);
    let mut with_attempt = replay();
    with_attempt.extend(["--attempt".into(), "1".into()]);
    for (command, input) in [
        (conflict, key.as_str()),
        (exact(), other_key.as_str()),
        (replay(), other_key.as_str()),
        (ack(), other_key.as_str()),
        (with_attempt, key.as_str()),
        (
            fixture.replay("outcome", pool, 1, 0, &operation),
            key.as_str(),
        ),
    ] {
        let (out, result) = finish_private(start_private(&fixture.hive, command, input));
        assert_eq!(out.status.code(), Some(1), "{result}");
        assert_no_credential(&out, &key);
        assert_no_credential(&out, &other_key);
        assert_eq!(fixture.record()["revision"], initial["revision"]);
    }

    let (out, found) = finish_private(start_private(&fixture.hive, replay(), &key));
    assert!(out.status.success(), "{found}");
    assert_eq!(found, reserved);
    assert_no_credential(&out, &key);
    let (out, acknowledged) = finish_private(start_private(&fixture.hive, ack(), &key));
    assert!(out.status.success(), "{acknowledged}");
    assert_eq!(acknowledged["kind"], "acknowledged");
    assert_no_credential(&out, &key);
    let compacted = fixture.record();
    assert_eq!(compacted["revision"], 3);
    assert_eq!(compacted["pools"][0]["slots"][0]["outcomes"], json!([]));
    assert_eq!(
        compacted["pools"][0]["slots"][0]["phase"]["kind"],
        "reserved"
    );

    for command in [exact(), replay(), ack()] {
        let (out, stale) = finish_private(start_private(&fixture.hive, command, &key));
        assert_eq!(out.status.code(), Some(1), "{stale}");
        assert_no_credential(&out, &key);
        assert_eq!(fixture.record()["revision"], compacted["revision"]);
    }
    let (out, status) = fixture.status();
    assert_eq!(status["slots"][0]["generation"], 1);
    assert_no_credential(&out, &key);
    for entry in fs::read_dir(fixture.hive.home.path.join("receipts")).unwrap() {
        let data = fs::read_to_string(entry.unwrap().path()).unwrap();
        assert!(!data.contains(key.trim_end()));
        assert!(!data.contains(&other_key));
        for line in data.lines() {
            let receipt: cargo_hive_core::receipt::Receipt = serde_json::from_str(line).unwrap();
            assert_eq!(receipt.schema, cargo_hive_core::receipt::SCHEMA);
        }
    }
}

fn assert_no_credential(out: &Output, key: &str) {
    assert!(!String::from_utf8_lossy(&out.stdout).contains(key.trim_end()));
    assert!(!String::from_utf8_lossy(&out.stderr).contains(key.trim_end()));
}

#[test]
fn the_thirty_third_unacknowledged_reservation_is_refused() {
    let fixture = PoolFixture::new();
    let (out, initialized) = fixture.init();
    assert!(out.status.success(), "{initialized}");
    let pool = initialized["pool_id"].as_str().unwrap();
    let commit = git(&fixture.repo, &["rev-parse", "HEAD"]);
    let credential = "a".repeat(64);
    let previous = |generation: u64| -> Value {
        json!({
            "pool_id": pool, "slot": 1, "generation": generation, "operation": format!("{generation:032x}"),
            "kind": "reserve", "attempt_id": null, "credential": credential,
            "request": {"kind": "reserve", "expected_generation": generation - 1, "commit": commit},
            "result": {"kind": "reserved"}
        })
    };
    let mut record = fixture.record();
    record["pools"][0]["slots"][0]["generation"] = json!(31);
    record["pools"][0]["slots"][0]["outcomes"] = json!((1..=31).map(previous).collect::<Vec<_>>());
    fs::write(fixture.file(), serde_json::to_vec(&record).unwrap()).unwrap();

    let op = format!("{:032x}", 32);
    let (out, reserved) = finish_private(start_private(
        &fixture.hive,
        fixture.reservation(pool, 1, 31, &commit, &op),
        &credential,
    ));
    assert!(out.status.success(), "{reserved}");
    assert_eq!(reserved["generation"], 32);
    let mut record = fixture.record();
    assert_eq!(
        record["pools"][0]["slots"][0]["outcomes"]
            .as_array()
            .unwrap()
            .len(),
        32
    );
    // Model the vacant slot left by a later return without implementing its writer.
    record["pools"][0]["slots"][0]["phase"] = json!({"kind": "vacant"});
    let bytes = serde_json::to_vec(&record).unwrap();
    fs::write(fixture.file(), &bytes).unwrap();

    let (out, status) = fixture.status();
    assert!(out.status.success(), "{status}");
    assert_eq!(status["slots"][0]["phase"], "held");
    assert!(
        status["slots"][0]["next_action"]
            .as_str()
            .unwrap()
            .contains("exact unresolved request")
    );
    let op = format!("{:032x}", 33);
    let next = || fixture.reservation(pool, 1, 32, &commit, &op);
    let (out, exhausted) = finish_private(start_private(&fixture.hive, next(), &credential));
    assert_eq!(out.status.code(), Some(1), "{exhausted}");
    assert!(
        exhausted["detail"]
            .as_str()
            .unwrap()
            .contains("32 unacknowledged")
    );
    assert_eq!(fs::read(fixture.file()).unwrap(), bytes);

    let op = format!("{:032x}", 1);
    let (out, historical) = finish_private(start_private(
        &fixture.hive,
        fixture.reservation(pool, 1, 0, &commit, &op),
        &credential,
    ));
    assert!(out.status.success(), "{historical}");
    assert_eq!(historical["generation"], 1);
    assert_eq!(historical["current"], false);
    assert!(historical.get("path").is_none());

    let (out, acknowledged) = finish_private(start_private(
        &fixture.hive,
        fixture.replay("ack", pool, 1, 1, &op),
        &credential,
    ));
    assert!(out.status.success(), "{acknowledged}");
    assert_eq!(acknowledged["current"], false);
    let (out, accepted) = finish_private(start_private(&fixture.hive, next(), &credential));
    assert!(out.status.success(), "{accepted}");
    assert_eq!(accepted["generation"], 33);
    assert_eq!(
        fixture.record()["pools"][0]["slots"][0]["outcomes"]
            .as_array()
            .unwrap()
            .len(),
        32
    );
}

#[test]
fn a_lost_reserve_response_replays_one_durable_allocation() {
    let fixture = PoolFixture::new();
    let (out, initialized) = fixture.init();
    assert!(out.status.success(), "{initialized}");
    let pool = initialized["pool_id"].as_str().unwrap();
    let commit = git(&fixture.repo, &["rev-parse", "HEAD"]);
    let op = "e".repeat(32);
    let credential = "5".repeat(64);
    let exact = || fixture.reservation(pool, 1, 0, &commit, &op);
    let mut lock = common::KillOnDrop(fixture.hive.hold_state_lock());
    let mut child = start_private(&fixture.hive, exact(), &credential);
    drop(child.stdout.take().unwrap());
    lock.0.kill().unwrap();
    lock.0.wait().unwrap();
    let lost = common::wait_within(child, Duration::from_secs(15));
    assert!(!lost.status.success());
    assert!(lost.stdout.is_empty());
    assert!(String::from_utf8_lossy(&lost.stderr).contains("could not deliver the slot result"));
    assert_no_credential(&lost, &credential);
    let (out, status) = fixture.status();
    assert!(out.status.success(), "{status}");
    assert_eq!(status["slots"][0]["generation"], 1);
    assert_eq!(status["slots"][0]["phase"], "reserved");
    let bytes = fs::read(fixture.file()).unwrap();
    let (out, replay) = finish_private(start_private(&fixture.hive, exact(), &credential));
    assert!(out.status.success(), "{replay}");
    assert_eq!(replay["generation"], 1);
    assert_eq!(replay["current"], true);
    assert_no_credential(&out, &credential);
    assert_eq!(fs::read(fixture.file()).unwrap(), bytes);
    let record = fixture.record();
    assert_eq!(record["revision"], 2);
    assert_eq!(
        record["pools"][0]["slots"][0]["outcomes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn a_reserve_from_an_old_namespace_cannot_claim_its_replacement() {
    let fixture = PoolFixture::new();
    let (out, initialized) = fixture.init();
    assert!(out.status.success(), "{initialized}");
    let old_pool = initialized["pool_id"].as_str().unwrap();
    let commit = git(&fixture.repo, &["rev-parse", "HEAD"]);
    let op = "7".repeat(32);
    let credential = "a".repeat(64);
    let original = || fixture.reservation(old_pool, 1, 0, &commit, &op);
    let (out, old) = finish_private(start_private(&fixture.hive, original(), &credential));
    assert!(out.status.success(), "{old}");

    let replacement = Hive::new();
    replacement.config(&json!({"secondary_roots": [replacement.secondary_root.path]}).to_string());
    register(&replacement, &fixture.repo);
    let (out, initialized) = fixture.init_on(&replacement, &replacement.secondary_root.path);
    assert!(out.status.success(), "{initialized}");
    let new_pool = initialized["pool_id"].as_str().unwrap();
    assert_ne!(old_pool, new_pool);
    assert_eq!(initialized["slots"][0]["number"], 1);
    assert_eq!(initialized["slots"][0]["generation"], 0);
    let file = replacement.home.path.join("slots.json");
    let bytes = fs::read(&file).unwrap();
    for command in [
        original(),
        fixture.replay("outcome", old_pool, 1, 1, &op),
        fixture.replay("ack", old_pool, 1, 1, &op),
    ] {
        let (out, stale) = finish_private(start_private(&replacement, command, &credential));
        assert_eq!(out.status.code(), Some(1), "{stale}");
        assert!(
            stale["detail"]
                .as_str()
                .unwrap()
                .contains("pool namespace is stale")
        );
        assert_no_credential(&out, &credential);
        assert_eq!(fs::read(&file).unwrap(), bytes);
    }

    let (out, reserved) = finish_private(start_private(
        &replacement,
        fixture.reservation(new_pool, 1, 0, &commit, &op),
        &credential,
    ));
    assert!(out.status.success(), "{reserved}");
    assert_eq!(reserved["pool_id"], new_pool);
    assert_eq!(reserved["generation"], 1);
}

#[test]
fn a_return_refuses_a_valid_credential_if_the_pipe_never_reaches_eof() {
    let slot = LeasedSlot::new();
    let state = slot.fixture.file();
    let paths = [&*state, &*slot.path, &*slot.admin];
    let before = snapshot_paths(&paths);
    let mut child = slot
        .fixture
        .hive
        .command(&[])
        .args(
            slot.fixture
                .replay("return", &slot.pool, 1, 1, &slot.operation),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    writeln!(input, "{}", slot.credential).unwrap();
    let out = common::wait_within(child, Duration::from_secs(5));
    let refused: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(1), "{refused}");
    assert_eq!(refused["kind"], "refused", "{refused}");
    assert!(
        refused["detail"]
            .as_str()
            .unwrap()
            .contains("private slot credential"),
        "{refused}"
    );
    assert_no_credential(&out, &slot.credential);
    assert_eq!(snapshot_paths(&paths), before);
    drop(input);

    let (out, returned) = finish_private(start_private(
        &slot.fixture.hive,
        slot.fixture
            .replay("return", &slot.pool, 1, 1, &slot.operation),
        &format!("{}\n", slot.credential),
    ));
    assert!(out.status.success(), "{returned}");
    assert_no_credential(&out, &slot.credential);
}

#[test]
fn cancelling_a_return_while_waiting_for_credential_eof_preserves_input_flags() {
    let slot = LeasedSlot::new();
    let state = slot.fixture.file();
    let paths = [&*state, &*slot.path, &*slot.admin];
    let before = snapshot_paths(&paths);
    let (reader, mut writer) = std::io::pipe().unwrap();
    let original_flags = pipe_read_status(&reader).unwrap().flags;
    writeln!(writer, "{}", slot.credential).unwrap();
    let child = slot
        .fixture
        .hive
        .command(&[])
        .args(
            slot.fixture
                .replay("return", &slot.pool, 1, 1, &slot.operation),
        )
        .stdin(Stdio::from(reader.try_clone().unwrap()))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut child = common::KillOnDrop(child);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let observed = pipe_read_status(&reader).unwrap();
        if observed.pending_bytes == 0 {
            assert_eq!(observed.flags, original_flags);
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the child did not read the credential"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    common::interrupt(child.0.id());
    let deadline = Instant::now() + Duration::from_secs(2);
    let status = loop {
        if let Some(status) = child.0.try_wait().unwrap() {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "the child did not handle SIGINT while waiting for EOF"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut out = Output {
        status,
        stdout: Vec::new(),
        stderr: Vec::new(),
    };
    child
        .0
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut out.stdout)
        .unwrap();
    child
        .0
        .stderr
        .take()
        .unwrap()
        .read_to_end(&mut out.stderr)
        .unwrap();
    assert_no_credential(&out, &slot.credential);
    let cancelled: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(130), "{cancelled}");
    assert_eq!(cancelled["kind"], "cancelled", "{cancelled}");
    assert_eq!(cancelled["detail"], "The slot operation was cancelled.");
    assert_eq!(pipe_read_status(&reader).unwrap().flags, original_flags);
    assert_eq!(snapshot_paths(&paths), before);
}

#[test]
fn reserve_refuses_inexact_commits_and_malformed_private_input() {
    let fixture = PoolFixture::new();
    let (out, initialized) = fixture.init();
    assert!(out.status.success(), "{initialized}");
    let saved = fs::read(fixture.file()).unwrap();
    let pool = initialized["pool_id"].as_str().unwrap();
    let commit = git(&fixture.repo, &["rev-parse", "HEAD"]);
    git(
        &fixture.repo,
        &["tag", "-a", "reviewed", "-m", "fixture tag"],
    );
    let tag = git(&fixture.repo, &["rev-parse", "refs/tags/reviewed"]);
    let blob = git(&fixture.repo, &["rev-parse", "HEAD:Cargo.toml"]);
    let op = "f".repeat(32);
    let key = "a".repeat(64);
    let missing_commit = "0".repeat(commit.len());
    for oid in [&commit[..12], &tag, &blob, &missing_commit] {
        let (out, invalid) = finish_private(start_private(
            &fixture.hive,
            fixture.reservation(pool, 1, 0, oid, &op),
            &key,
        ));
        assert_eq!(out.status.code(), Some(1), "{invalid}");
        assert!(
            invalid["detail"]
                .as_str()
                .unwrap()
                .contains("full object ID")
        );
        assert_eq!(fs::read(fixture.file()).unwrap(), saved);
    }

    for malformed in [
        "".into(),
        format!("{key}\n\n"),
        format!("{key}\0"),
        format!("{key}\ntrailing bytes"),
        key.to_uppercase(),
    ] {
        let (out, invalid) = finish_private(start_private(
            &fixture.hive,
            fixture.reservation(pool, 1, 0, &commit, &op),
            &malformed,
        ));
        assert_eq!(out.status.code(), Some(1), "{invalid}");
        assert!(
            invalid["detail"]
                .as_str()
                .unwrap()
                .contains("private slot credential")
        );
        assert_no_credential(&out, &key);
        assert_eq!(fs::read(fixture.file()).unwrap(), saved);
    }
}

#[test]
fn reserve_leaves_unfetched_commits_and_their_object_store_untouched() {
    let repo_dir = Scratch::new("slot-partial-clone");
    let origin = init_repo(&repo_dir.path.join("origin"), "");
    let local_commit = git(&origin, &["rev-parse", "HEAD"]);
    let missing_commit = git(
        &origin,
        &[
            "commit-tree",
            "HEAD^{tree}",
            "-p",
            "HEAD",
            "-m",
            "unfetched",
        ],
    );
    git(
        &origin,
        &["update-ref", "refs/heads/unfetched", &missing_commit],
    );
    git(&origin, &["config", "uploadpack.allowFilter", "true"]);
    git(
        &origin,
        &["config", "uploadpack.allowAnySHA1InWant", "true"],
    );
    git(
        &repo_dir.path,
        &[
            "clone",
            "--no-local",
            "--single-branch",
            "--filter=blob:none",
            origin.to_str().unwrap(),
            "main",
        ],
    );
    let repo = repo_dir.path.join("main").canonicalize().unwrap();
    git(&repo, &["config", "protocol.file.allow", "always"]);
    assert_eq!(git(&repo, &["config", "remote.origin.promisor"]), "true");

    let fixture = PoolFixture::from_repo(repo_dir, repo);
    let (out, initialized) = fixture.init();
    assert!(out.status.success(), "{initialized}");
    let pool = initialized["pool_id"].as_str().unwrap();
    let saved = fs::read(fixture.file()).unwrap();
    let local_objects = || {
        git(
            &fixture.repo,
            &[
                "cat-file",
                "--batch-all-objects",
                "--batch-check=%(objectname)",
            ],
        )
    };
    let inventory = local_objects();
    assert!(!inventory.lines().any(|oid| oid == missing_commit));
    assert!(inventory.lines().any(|oid| oid == local_commit));
    let key = "a".repeat(64);
    let op = "f".repeat(32);
    let stale_pool = "0".repeat(32);
    for (namespace, detail) in [
        (stale_pool.as_str(), "pool namespace is stale"),
        (pool, "full object ID"),
    ] {
        let (out, refused) = finish_private(start_private(
            &fixture.hive,
            fixture.reservation(namespace, 1, 0, &missing_commit, &op),
            &key,
        ));
        assert_eq!(out.status.code(), Some(1), "{refused}");
        assert!(
            refused["detail"].as_str().unwrap().contains(detail),
            "{refused}"
        );
        assert_eq!(
            local_objects(),
            inventory,
            "the slot probe changed the local objects"
        );
        assert_eq!(fs::read(fixture.file()).unwrap(), saved);
        assert!(out.stderr.is_empty(), "{out:?}");
    }

    let (out, reserved) = finish_private(start_private(
        &fixture.hive,
        fixture.reservation(pool, 1, 0, &local_commit, &op),
        &key,
    ));
    assert!(out.status.success(), "{reserved}");
    assert_eq!(reserved["generation"], 1);
    assert_eq!(local_objects(), inventory);
}

#[test]
fn an_audit_failure_neither_releases_nor_repeats_a_reservation() {
    let fixture = PoolFixture::new();
    let (out, initialized) = fixture.init();
    assert!(out.status.success(), "{initialized}");
    let pool = initialized["pool_id"].as_str().unwrap();
    let receipts = fixture.hive.home.path.join("receipts");
    let date = cargo_hive_platform::clock::utc_date(std::time::SystemTime::now());
    let audit = receipts.join(format!("{date}.jsonl"));
    let line = fs::read(&audit).unwrap();
    let preserved = line.repeat(CAP / line.len() + 1);
    fs::write(&audit, &preserved).unwrap();
    let commit = git(&fixture.repo, &["rev-parse", "HEAD"]);
    let credential = "b".repeat(64);
    let op = "3".repeat(32);
    let exact = || fixture.reservation(pool, 1, 0, &commit, &op);
    let (out, reserved) = finish_private(start_private(&fixture.hive, exact(), &credential));
    assert!(out.status.success(), "{reserved}");
    assert_eq!(reserved["kind"], "reserved");
    assert!(String::from_utf8_lossy(&out.stderr).contains("could not log the slot receipt"));
    assert_no_credential(&out, &credential);
    let bytes = fs::read(fixture.file()).unwrap();

    let (out, replay) = finish_private(start_private(&fixture.hive, exact(), &credential));
    assert!(out.status.success(), "{replay}");
    assert_eq!(reserved, replay);
    let competitor = fixture.reservation(pool, 1, 0, &commit, &"9".repeat(32));
    let (out, unavailable) =
        finish_private(start_private(&fixture.hive, competitor, &"c".repeat(64)));
    assert_eq!(out.status.code(), Some(1), "{unavailable}");
    assert_eq!(unavailable["kind"], "unavailable");
    assert_eq!(fs::read(fixture.file()).unwrap(), bytes);
    assert!(fs::read(audit).unwrap() == preserved);
    let record = fixture.record();
    assert_eq!(record["revision"], 2);
    assert_eq!(
        record["pools"][0]["slots"][0]["outcomes"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn initialization_records_two_private_unused_paths_across_processes() {
    let fixture = PoolFixture::new();
    let locked = fixture._repo_dir.path.join("locked-worktree");
    git(
        &fixture.repo,
        &[
            "worktree",
            "add",
            "--detach",
            "--lock",
            "--reason",
            "scratch\ncontrol",
            locked.to_str().unwrap(),
            "HEAD",
        ],
    );
    let missing = fixture._repo_dir.path.join("missing-worktree");
    git(
        &fixture.repo,
        &[
            "worktree",
            "add",
            "-b",
            "prunable",
            missing.to_str().unwrap(),
            "HEAD",
        ],
    );
    fs::rename(&missing, fixture._repo_dir.path.join("moved-worktree")).unwrap();
    let before = git(&fixture.repo, &["worktree", "list", "--porcelain"]);
    assert!(before.contains("\ndetached\nlocked "));
    assert!(before.contains("\nbranch refs/heads/prunable\nprunable "));
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
fn a_lost_record_leaves_nested_checkout_namespaces_held() {
    let fixture = PoolFixture::new();
    let nested = fixture
        .hive
        .secondary_root
        .path
        .join("durable")
        .join("nested");
    fs::create_dir_all(&nested).unwrap();
    let (out, fresh) = fixture.status();
    assert!(out.status.success(), "{fresh}");
    assert_eq!(fresh["kind"], "unconfigured");
    let (out, initialized) = fixture.init_at(&nested);
    assert!(out.status.success(), "{initialized}");
    let pool = initialized["pool_id"].as_str().unwrap();
    let old_path = nested.join("hive-slots").join(pool);
    let marker = old_path.join("preserve");
    fs::write(&marker, b"previous pool evidence").unwrap();
    let inode = fs::metadata(&old_path).unwrap().ino();
    fs::rename(fixture.file(), fixture._repo_dir.path.join("lost-record")).unwrap();
    fs::rename(
        fixture.hive.home.path.join("slot-locks"),
        fixture._repo_dir.path.join("lost-locks"),
    )
    .unwrap();

    for (out, result) in [fixture.status(), fixture.init_at(&nested)] {
        assert_eq!(out.status.code(), Some(1), "{result}");
        assert_eq!(result["kind"], "held");
        assert!(
            result.to_string().contains(nested.to_str().unwrap()),
            "{result}"
        );
        assert_eq!(fs::metadata(&old_path).unwrap().ino(), inode);
        assert_eq!(fs::read(&marker).unwrap(), b"previous pool evidence");
        assert!(!fixture.file().exists());
        assert!(!fixture.hive.home.path.join("slot-locks").exists());
    }
}

#[test]
fn checkout_namespace_inventory_checks_depth_at_the_limit_and_one_past_it() {
    let fixture = PoolFixture::new();
    let mut deep = fixture.hive.secondary_root.path.clone();
    for _ in 0..32 {
        deep.push("nested");
    }
    fs::create_dir_all(&deep).unwrap();
    let (out, result) = fixture.status();
    assert!(out.status.success(), "{result}");
    assert_eq!(result["kind"], "unconfigured");
    let extra = deep.join("one-too-deep");
    fs::create_dir(&extra).unwrap();
    let (out, result) = fixture.status();
    assert_eq!(out.status.code(), Some(1), "{result}");
    assert!(result.to_string().contains("inventory"), "{result}");
    assert!(extra.is_dir());
    assert!(!fixture.file().exists());
}

#[test]
fn checkout_namespace_inventory_checks_entries_at_the_limit_and_one_past_it() {
    let fixture = PoolFixture::new();
    let root = &fixture.hive.secondary_root.path;
    for number in 0..4096 {
        fs::File::create(root.join(number.to_string())).unwrap();
    }
    let (out, result) = fixture.status();
    assert!(out.status.success(), "{result}");
    assert_eq!(result["kind"], "unconfigured");
    let extra = root.join("one-too-many");
    fs::write(&extra, b"preserve").unwrap();
    let (out, result) = fixture.status();
    assert_eq!(out.status.code(), Some(1), "{result}");
    assert!(result.to_string().contains("inventory"), "{result}");
    assert_eq!(fs::read(&extra).unwrap(), b"preserve");
    assert!(!fixture.file().exists());
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

fn check_home_ancestor(change: impl FnOnce(&Path)) {
    let fixture = PoolFixture::new();
    let parent = Scratch::new("shared-slot-state");
    let home = parent.path.join("owner-home");
    fs::create_dir(&home).unwrap();
    for file in [".lock", "registry.json", "config.json"] {
        fs::copy(fixture.hive.home.path.join(file), home.join(file)).unwrap();
    }
    let lock = home.join(".lock");
    let identity = fs::metadata(&lock).unwrap().ino();
    let status = || {
        let out = fixture
            .hive
            .command(&[
                "slot",
                "status",
                "--repo",
                fixture.repo.to_str().unwrap(),
                "--json",
            ])
            .env("HIVE_HOME", &home)
            .output()
            .unwrap();
        let result: Value = serde_json::from_slice(&out.stdout).unwrap();
        (out.status.code(), result)
    };
    change(&parent.path);
    let (code, result) = status();
    assert_eq!(code, Some(1), "{result}");
    assert_eq!(result["kind"], "refused");
    assert_eq!(fs::metadata(&lock).unwrap().ino(), identity);
    assert!(!home.join("slots.json").exists());

    fs::set_permissions(&parent.path, fs::Permissions::from_mode(0o755)).unwrap();
    chmod_acl(&["-N"], &parent.path);
    chmod_acl(&["+a", "everyone deny delete"], &parent.path);
    let (code, result) = status();
    assert_eq!(code, Some(0), "{result}");
    assert_eq!(result["kind"], "unconfigured");
}

#[test]
fn slot_state_refuses_a_home_beneath_a_shared_writable_parent() {
    check_home_ancestor(|parent| {
        fs::set_permissions(parent, fs::Permissions::from_mode(0o777)).unwrap();
    });
}

#[test]
fn slot_state_refuses_a_home_beneath_a_parent_with_write_acl() {
    check_home_ancestor(|parent| {
        chmod_acl(&["+a", "everyone deny delete"], parent);
        chmod_acl(&["+a#", "1", "everyone allow delete_child"], parent);
    });
}

#[test]
fn slot_init_refuses_a_selected_root_beneath_a_shared_writable_parent() {
    let fixture = PoolFixture::new();
    let parent = &fixture.hive.secondary_root.path;
    let root = parent.join("selected");
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(parent, fs::Permissions::from_mode(0o777)).unwrap();
    let (out, result) = fixture.init_at(&root);
    assert_eq!(out.status.code(), Some(1), "{result}");
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
    assert!(!fixture.file().exists());
    assert!(!fixture.hive.home.path.join("slot-locks").exists());

    fs::set_permissions(parent, fs::Permissions::from_mode(0o755)).unwrap();
    let (out, result) = fixture.init_at(&root);
    assert!(out.status.success(), "{result}");
}

fn chmod_acl(args: &[&str], path: &Path) {
    let mut spec = cargo_hive_platform::proc::Spec::new("/bin/chmod");
    spec.args.extend(args.iter().map(|arg| (*arg).into()));
    spec.args.push(path.into());
    let out = cargo_hive_platform::proc::output_with_deadline(
        &spec,
        std::time::Duration::from_secs(5),
        || false,
    )
    .unwrap();
    assert!(out.status.success(), "{:?}", out.stderr);
}

#[test]
fn reviewed_profile_accepts_a_deny_only_parent_and_refuses_a_shared_writable_parent() {
    let mut fixture = PoolFixture::new();
    let home = Scratch::new("slot-profile-home");
    let parent = &home.path;
    let profile = parent.join("reviewed.json");
    fs::rename(&fixture.profile, &profile).unwrap();
    fixture.profile = profile;
    fs::set_permissions(&fixture.profile, fs::Permissions::from_mode(0o600)).unwrap();
    let bytes = fs::read(&fixture.profile).unwrap();

    fs::set_permissions(parent, fs::Permissions::from_mode(0o777)).unwrap();
    let (out, result) = fixture.init();
    fs::set_permissions(parent, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(out.status.code(), Some(1), "{result}");
    assert_eq!(result["kind"], "refused");
    assert!(
        result["detail"].as_str().unwrap().contains("reviewed.json"),
        "{result}"
    );
    assert!(!fixture.file().exists());
    assert!(!fixture.hive.home.path.join("slot-locks").exists());

    chmod_acl(&["+a", "everyone deny delete"], parent);
    let (out, result) = fixture.init();
    chmod_acl(&["-N"], parent);
    assert!(out.status.success(), "{result}");
    assert_eq!(result["kind"], "initialized");
    assert_eq!(fs::read(&fixture.profile).unwrap(), bytes);
}

#[test]
fn reviewed_profile_accepts_a_root_owned_sticky_parent() {
    struct RemoveProfileOnDrop<'a>(&'a Path);
    impl Drop for RemoveProfileOnDrop<'_> {
        fn drop(&mut self) {
            let _ = fs::remove_file(self.0);
        }
    }

    let mut fixture = PoolFixture::new();
    let profile = fixture._repo_dir.path.with_extension("reviewed.json");
    let parent = fs::metadata(profile.parent().unwrap()).unwrap();
    assert_eq!(parent.uid(), 0);
    assert_eq!(parent.mode() & 0o7777, 0o1777);

    let mut copy = fs::File::options()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&profile)
        .unwrap();
    let _cleanup = RemoveProfileOnDrop(&profile);
    std::io::copy(&mut fs::File::open(&fixture.profile).unwrap(), &mut copy).unwrap();
    drop(copy);
    fixture.profile.clone_from(&profile);

    let (out, result) = fixture.init();
    assert!(out.status.success(), "{result}");
    assert_eq!(result["kind"], "initialized");
}

#[test]
fn slot_init_refuses_a_reviewed_profile_with_a_write_acl() {
    let fixture = PoolFixture::new();
    fs::set_permissions(&fixture.profile, fs::Permissions::from_mode(0o600)).unwrap();
    let bytes = fs::read(&fixture.profile).unwrap();
    chmod_acl(&["+a", "everyone allow write,delete"], &fixture.profile);
    assert_eq!(
        fs::metadata(&fixture.profile).unwrap().mode() & 0o7777,
        0o600
    );
    let (out, result) = fixture.init();
    assert_eq!(out.status.code(), Some(1), "{result}");
    assert_eq!(result["kind"], "refused");
    assert!(result["detail"].as_str().unwrap().contains("reviewed.json"));
    assert!(!fixture.file().exists());
    assert!(!fixture.hive.home.path.join("slot-locks").exists());
    assert_eq!(
        fs::read_dir(&fixture.hive.secondary_root.path)
            .unwrap()
            .count(),
        0
    );
    assert_eq!(fs::read(&fixture.profile).unwrap(), bytes);

    chmod_acl(&["-N"], &fixture.profile);
    let (out, result) = fixture.init();
    assert!(out.status.success(), "{result}");
}

#[test]
fn slot_scope_refuses_write_acls_and_unsafe_legacy_lock_paths() {
    let fixture = PoolFixture::new();
    let root = &fixture.hive.secondary_root.path;
    fs::set_permissions(root, fs::Permissions::from_mode(0o700)).unwrap();
    chmod_acl(
        &[
            "+a",
            "everyone allow add_file,add_subdirectory,delete_child",
        ],
        root,
    );
    assert_eq!(fs::metadata(root).unwrap().mode() & 0o7777, 0o700);
    let (out, result) = fixture.init();
    assert_eq!(out.status.code(), Some(1), "{result}");
    assert_eq!(fs::read_dir(root).unwrap().count(), 0);
    assert!(!fixture.file().exists());
    chmod_acl(&["-N"], root);

    let lock = fixture.hive.home.path.join(".lock");
    let locked = fs::metadata(&lock).unwrap();
    chmod_acl(&["+a", "everyone allow write,delete"], &lock);
    let (out, result) = fixture.init();
    assert_eq!(out.status.code(), Some(1), "{result}");
    assert_eq!(fs::metadata(&lock).unwrap().ino(), locked.ino());
    assert!(!fixture.file().exists());
    chmod_acl(&["-N"], &lock);

    let original = lock.with_extension("original");
    fs::rename(&lock, &original).unwrap();
    let target = fixture._repo_dir.path.join("lock-target");
    fs::write(&target, b"lock target untouched").unwrap();
    symlink(&target, &lock).unwrap();
    let (out, result) = fixture.init();
    assert_eq!(out.status.code(), Some(1), "{result}");
    assert_eq!(fs::read_link(&lock).unwrap(), target);
    assert_eq!(fs::read(&target).unwrap(), b"lock target untouched");
    fs::remove_file(&lock).unwrap();
    fs::rename(&original, &lock).unwrap();

    let (out, result) = fixture.init();
    assert!(out.status.success(), "{result}");
    assert_eq!(fs::metadata(&lock).unwrap().ino(), locked.ino());
}

#[test]
fn an_oversize_audit_preserves_the_receipt_and_committed_pool() {
    let fixture = PoolFixture::new();
    let (out, initial) = fixture.status();
    assert!(out.status.success(), "{initial}");
    let receipts = fixture.hive.home.path.join("receipts");
    let date = cargo_hive_platform::clock::utc_date(std::time::SystemTime::now());
    let path = receipts.join(format!("{date}.jsonl"));
    let line = fs::read(&path).unwrap();
    let prior = line.repeat(CAP / line.len() + 1);
    assert!(prior.len() > CAP);
    fs::write(&path, &prior).unwrap();

    let (out, initialized) = fixture.init();
    assert!(out.status.success(), "{initialized}");
    assert_eq!(initialized["kind"], "initialized");
    assert_eq!(fs::read(&path).unwrap().len(), prior.len());
    assert!(fs::read(&path).unwrap() == prior);
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("could not log the slot receipt"),
        "{out:?}"
    );
    let (out, status) = fixture.status();
    assert!(out.status.success(), "{status}");
    assert_eq!(status["pool_id"], initialized["pool_id"]);
    assert_eq!(status["slots"], initialized["slots"]);
}

#[test]
fn an_unrepresentable_repository_has_one_sanitized_json_refusal() {
    let fixture = PoolFixture::new();
    let out = fixture
        .hive
        .command(&["slot", "status", "--json", "--repo"])
        .arg(OsStr::from_bytes(b"/not-unicode-\xff"))
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stdout = std::str::from_utf8(&out.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 1, "{out:?}");
    assert!(stdout.ends_with('\n'));
    let result: Value = serde_json::from_str(stdout).unwrap();
    assert_eq!(result["schema"], 1);
    assert_eq!(result["kind"], "refused");
    assert_eq!(result.get("repo"), Some(&Value::Null));
    assert!(result["next_action"].as_str().is_some());
    assert!(!stdout.contains("not-unicode"));
}

#[test]
fn a_native_guide_begin_issues_only_one_durable_permit() {
    let fixture = PoolFixture::new();
    let (out, initialized) = fixture.init();
    assert!(out.status.success());
    let pool = initialized["pool_id"].as_str().unwrap();
    let checkout = PathBuf::from(initialized["slots"][0]["path"].as_str().unwrap());
    let lock = fixture
        .hive
        .home
        .path
        .join("slot-locks")
        .join(pool)
        .join("slot-1.lock");
    let head = git(&fixture.repo, &["rev-parse", "HEAD"]);
    let proof = &fixture._repo_dir.path;
    let key_path = proof.join("begin-key");
    let nonce_path = proof.join("begin-nonce");
    for secret in [&key_path, &nonce_path] {
        private_file(secret);
        let out = key_command(&fixture, PRIVATE_KEY_COMMAND, secret, "3");
        assert!(out.status.success());
    }
    let key = fs::read_to_string(&key_path).unwrap();
    let nonce = fs::read_to_string(&nonce_path).unwrap();
    let reserve_op = "1".repeat(32);
    let begin_op = "2".repeat(32);
    let args = fixture.reservation(pool, 1, 0, &head, &reserve_op);
    let (out, reserved) = finish_private(start_private(&fixture.hive, args, &key));
    assert!(out.status.success(), "{reserved}");
    let reserved_revision = fixture.record()["revision"].as_u64().unwrap();
    for name in ["first-control", "retry-control", "wrong-open-control"] {
        private_file(&proof.join(name));
    }

    let helper = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugin/skills/hive/scripts/slot-handoff.sh")
        .canonicalize()
        .unwrap();
    // Use the packaged version check, then exercise the native checkpoint from its dedicated
    // Bash parent. No pipeline or command substitution may interpose on the CLI's parent.
    let script = r#"
set +x
bin=$1 helper=$2 lock=$3 proof=$4 repo=$5 pool=$6 operation=$7
/bin/bash "$helper" check-version "$bin" || exit 20
IFS= read -r credential < "$proof/begin-key" || exit 21
IFS= read -r nonce < "$proof/begin-nonce" || exit 22
printf -v request '{"credential":"%s","nonce":"%s"}' "$credential" "$nonce"
exec 9< "$lock" || exit 23
"$bin" slot begin --repo "$repo" --pool "$pool" --slot 1 --generation 1 \
    --op "$operation" --lock-fd 9 --control-fd 8 --json \
    <<< "$request" 8<> "$lock" > "$proof/alias-result"
printf '%s\n' "$?" > "$proof/alias-status" || exit 24
"$bin" slot begin --repo "$repo" --pool "$pool" --slot 1 --generation 1 \
    --op "$operation" --lock-fd 9 --control-fd 8 --json \
    <<< "$request" 9< "$lock" 8<> "$proof/wrong-open-control" > "$proof/wrong-open-result"
printf '%s\n' "$?" > "$proof/wrong-open-status" || exit 24
for call in first retry; do
    exec 8<> "$proof/$call-control" || exit 24
    "$bin" slot begin --repo "$repo" --pool "$pool" --slot 1 --generation 1 \
        --op "$operation" --lock-fd 9 --control-fd 8 --json \
        <<< "$request" > "$proof/$call-result"
    printf '%s\n' "$?" > "$proof/$call-status" || exit 25
    exec 8>&-
    : > "$proof/$call-exited"
    # Rust checks the lock independently while this parent retains FD 9.
    IFS= read -r _ || :
done
"#;
    let mut guide = fixture
        .hive
        .command_with(
            Path::new("/bin/bash"),
            &["--noprofile", "--norc", "-c", script, "guide"],
        )
        .arg(env!("CARGO_BIN_EXE_cargo-hive"))
        .arg(helper)
        .arg(&lock)
        .arg(proof)
        .arg(&fixture.repo)
        .arg(pool)
        .arg(&begin_op)
        .env_remove("BASH_ENV")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let independent = fs::File::open(&lock).unwrap();
    let mut held_after_call = Vec::new();
    let mut guide_input = guide.stdin.take().unwrap();
    for name in ["first-exited", "retry-exited"] {
        let deadline = Instant::now() + Duration::from_secs(15);
        let checkpoint = proof.join(name);
        while !checkpoint.is_file()
            && guide.try_wait().unwrap().is_none()
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        if !checkpoint.is_file() {
            break;
        }
        held_after_call.push(
            cargo_hive_platform::flock::try_exclusive(&independent)
                .unwrap()
                .is_none(),
        );
        guide_input.write_all(b"continue\n").unwrap();
    }
    drop(guide_input);
    let out = common::wait_within(guide, Duration::from_secs(5));
    assert_no_credential(&out, &key);
    assert_no_credential(&out, &nonce);
    assert!(out.status.success());
    assert_eq!(held_after_call.len(), 2);
    assert_eq!(
        fs::read_to_string(proof.join("alias-status")).unwrap(),
        "1\n"
    );
    assert_eq!(fs::metadata(&lock).unwrap().len(), 0);
    assert_eq!(
        fs::read_to_string(proof.join("wrong-open-status")).unwrap(),
        "1\n"
    );
    assert_eq!(
        fs::metadata(proof.join("wrong-open-control"))
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        fs::read_to_string(proof.join("first-status")).unwrap(),
        "0\n",
        "the native guide should receive its first preparation permit"
    );
    assert_eq!(
        fs::read_to_string(proof.join("retry-status")).unwrap(),
        "0\n"
    );
    for name in [
        "first-result",
        "retry-result",
        "first-control",
        "retry-control",
    ] {
        let result = fs::read(proof.join(name)).unwrap();
        let text = String::from_utf8_lossy(&result);
        assert!(!text.contains(key.trim_end()) && !text.contains(nonce.trim_end()));
    }
    let first_control = fs::read(proof.join("first-control")).unwrap();
    assert!(!first_control.is_empty() && first_control.ends_with(b"\0"));
    assert_eq!(fs::metadata(proof.join("retry-control")).unwrap().len(), 0);
    let retry: Value =
        serde_json::from_slice(&fs::read(proof.join("retry-result")).unwrap()).unwrap();
    assert_eq!(retry["current"], false);
    assert_eq!(
        held_after_call,
        [true, true],
        "checkpoint children must leave the guide's flock held"
    );
    assert!(
        cargo_hive_platform::flock::try_exclusive(&independent)
            .unwrap()
            .is_some()
    );

    let saved = fixture.record();
    let slot = &saved["pools"][0]["slots"][0];
    assert_eq!(saved["revision"], reserved_revision + 1);
    assert_eq!(slot["phase"]["kind"], "preparing");
    assert_eq!(slot["phase"]["attempt"]["stage"], "permit_issued");
    assert_eq!(slot["phase"]["attempt"]["begin_op"], begin_op);
    let outcomes = slot["outcomes"].as_array().unwrap();
    let begin_outcomes: Vec<_> = outcomes
        .iter()
        .filter(|outcome| outcome["kind"] == "begin")
        .collect();
    assert_eq!(begin_outcomes.len(), 1);
    assert_eq!(
        begin_outcomes[0]["attempt_id"],
        slot["phase"]["attempt"]["attempt_id"]
    );
    assert!(
        !checkout.exists(),
        "begin must not launch any checkout writer"
    );
}

#[test]
fn the_packaged_guide_returns_a_first_ready_slot_with_absent_roots() {
    let fixture = reviewed_pool_fixture(None);
    fixture.hive.write_ready_capabilities();
    let (out, initialized) = fixture.init();
    assert!(out.status.success(), "{initialized}");
    let pool = initialized["pool_id"].as_str().unwrap();
    let checkout = PathBuf::from(initialized["slots"][0]["path"].as_str().unwrap());
    let head = git(&fixture.repo, &["rev-parse", "HEAD"]);
    let key_path = fixture._repo_dir.path.join("take-key");
    private_file(&key_path);
    assert!(
        key_command(&fixture, PRIVATE_KEY_COMMAND, &key_path, "3")
            .status
            .success()
    );
    let credential = fs::read_to_string(&key_path).unwrap();
    let out = run_packaged_take(&fixture, pool, &head, &key_path, "take");
    assert_no_credential(&out, &credential);
    assert!(
        out.status.success(),
        "the packaged guide should return first ready: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let ready: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(ready["kind"], "ready");
    assert_eq!(ready["use_policy"], "guide_guarded_cargo_only");
    assert_eq!(ready["path"], json!(checkout));
    assert_eq!(ready["commit"], head);
    assert_eq!(ready["current"], true);
    assert_eq!(git(&checkout, &["rev-parse", "HEAD"]), head);
    assert!(git(&checkout, &["status", "--porcelain"]).is_empty());
    assert!(!checkout.join("target").exists());
    let saved = fixture.record();
    let slot = &saved["pools"][0]["slots"][0];
    assert_eq!(slot["phase"]["kind"], "leased");
    assert_eq!(
        slot["producer"]["evidence"]["roots"]["target"]["kind"],
        "absent"
    );
    assert_eq!(
        slot["producer"]["evidence"]["roots"]["build"]["kind"],
        "absent"
    );
    let attempt = ready["attempt"].as_u64().unwrap().to_string();
    let replay = |command| {
        let mut args = fixture.replay(command, pool, 1, 1, &"3".repeat(32));
        args.extend(["--attempt".to_owned(), attempt.clone()]);
        finish_private(start_private(&fixture.hive, args, &credential))
    };
    let (out, replayed) = replay("outcome");
    assert!(out.status.success(), "{replayed}");
    assert_eq!(replayed, ready);
    assert_eq!(fixture.record()["revision"], saved["revision"]);
    let returned = fixture.replay("return", pool, 1, 1, &"4".repeat(32));
    let (out, returned) = finish_private(start_private(&fixture.hive, returned, &credential));
    assert!(out.status.success(), "{returned}");
    let (out, historical) = replay("outcome");
    assert!(out.status.success(), "{historical}");
    assert_eq!(historical["current"], false);
    assert!(historical.get("path").is_none());
    assert!(historical.get("use_policy").is_none());
    let (out, ack) = replay("ack");
    assert!(out.status.success(), "{ack}");
    assert_eq!(ack["kind"], "acknowledged");
    let (out, _) = replay("outcome");
    assert_eq!(out.status.code(), Some(1));
}

fn run_packaged_take(
    fixture: &PoolFixture,
    pool: &str,
    head: &str,
    key_path: &Path,
    mode: &str,
) -> Output {
    common::wait_within(
        start_packaged_take(fixture, pool, head, key_path, mode),
        Duration::from_secs(30),
    )
}

fn start_packaged_take(
    fixture: &PoolFixture,
    pool: &str,
    head: &str,
    key_path: &Path,
    mode: &str,
) -> Child {
    start_packaged_take_at(fixture, pool, head, key_path, mode, "0")
}

fn start_packaged_take_at(
    fixture: &PoolFixture,
    pool: &str,
    head: &str,
    key_path: &Path,
    mode: &str,
    expect: &str,
) -> Child {
    packaged_take_command(fixture, pool, head, key_path, mode, expect)
        .spawn()
        .unwrap()
}

#[allow(clippy::disallowed_types)]
fn packaged_take_command(
    fixture: &PoolFixture,
    pool: &str,
    head: &str,
    key_path: &Path,
    mode: &str,
    expect: &str,
) -> std::process::Command {
    packaged_take_with_binary(
        fixture,
        pool,
        head,
        key_path,
        mode,
        expect,
        Path::new(env!("CARGO_BIN_EXE_cargo-hive")),
    )
}

#[allow(clippy::disallowed_types)]
fn packaged_take_with_binary(
    fixture: &PoolFixture,
    pool: &str,
    head: &str,
    key_path: &Path,
    mode: &str,
    expect: &str,
    binary: &Path,
) -> std::process::Command {
    let helper = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugin/skills/hive/scripts/slot-handoff.sh");
    let mut command = fixture
        .hive
        .command_with(Path::new("/bin/bash"), &["--noprofile", "--norc"]);
    producer_environment(&mut command);
    command
        .arg(helper)
        .arg(mode)
        .arg(binary)
        .arg(&fixture.repo)
        .args([
            pool,
            "1",
            expect,
            head,
            &"1".repeat(32),
            &"2".repeat(32),
            &"3".repeat(32),
        ])
        .env_remove("BASH_ENV")
        .stdin(fs::File::open(key_path).unwrap())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

fn guarded_workspace_check(slot: &LeasedSlot, generation: u64) {
    let (lock, _) = cargo_hive_state::slots::open_slot_lock(&slot.lock_path()).unwrap();
    let _guard = cargo_hive_platform::flock::try_exclusive(&lock)
        .unwrap()
        .unwrap();
    let mut request = producer_input(slot);
    request["shape"]["argv"] = json!(["check", "--locked", "--workspace"]);
    let (out, started) = finish_private(start_producer(
        slot,
        generation,
        &slot.operation,
        &request,
        Some(&lock),
        |_| (),
    ));
    assert!(out.status.success(), "{started}");
    let mut args = producer_gate_args(slot);
    args.push("--workspace".into());
    let mut cargo = stopped_stock_cargo_with_args(slot, Some(&lock), args);
    request["action"] = json!("watch");
    request["child_pid"] = json!(cargo.id());
    let mut watcher = start_producer(
        slot,
        generation,
        &slot.operation,
        &request,
        Some(&lock),
        |_| (),
    );
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = cargo.try_wait().unwrap() {
            assert!(status.success(), "{status}");
            break;
        }
        if Instant::now() >= until || watcher.try_wait().unwrap().is_some() {
            cargo.kill().unwrap();
            cargo.wait().unwrap();
            let (_, result) = finish_private(watcher);
            panic!("watch did not permit Cargo: {result}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let (out, completed) = finish_private(watcher);
    assert!(out.status.success(), "{completed}");
    assert_eq!(completed["kind"], "producer_complete");
}

fn workspace_unit_stamp(path: &Path, package: &str) -> std::time::SystemTime {
    fs::metadata(workspace_unit_stamp_path(path, package))
        .unwrap()
        .modified()
        .unwrap()
}

fn workspace_unit_stamp_path(path: &Path, package: &str) -> PathBuf {
    use cargo_hive_core::{fingerprint, unit_path};
    let profile = path.join("target/debug");
    let report = fingerprint::read_profile(&profile);
    let graph = report.graph.unwrap();
    let mut units = graph
        .units
        .iter()
        .filter(|unit| unit.loc.package() == package);
    let unit = units.next().unwrap();
    assert!(units.next().is_none());
    unit_path::stamp(&profile, &unit.loc, unit.kind)
}

const RETAINED_EDIT: &str = "pub fn answer() -> u32 { 43 }\n";

fn first_completed_workspace() -> (LeasedSlot, PathBuf, String) {
    let fixture = reviewed_pool_fixture(None);
    fixture.hive.write_ready_capabilities();
    let head = git(&fixture.repo, &["rev-parse", "HEAD"]);
    fs::write(fixture.repo.join("src/lib.rs"), RETAINED_EDIT).unwrap();
    git(
        &fixture.repo,
        &[
            "commit",
            "-qm",
            "Edit only the fixture crate",
            "--",
            "src/lib.rs",
        ],
    );
    let next = git(&fixture.repo, &["rev-parse", "HEAD"]);
    let mut profile: Value = serde_json::from_slice(&fs::read(&fixture.profile).unwrap()).unwrap();
    profile["consumers"][0]["requested_object"] =
        json!(git(&fixture.repo, &["rev-parse", "HEAD:src/lib.rs"]));
    profile["invocations"][0]["argv"] = json!(["check", "--locked", "--workspace"]);
    fs::write(&fixture.profile, serde_json::to_vec(&profile).unwrap()).unwrap();
    let (out, initialized) = fixture.init();
    assert!(out.status.success(), "{initialized}");
    let pool = initialized["pool_id"].as_str().unwrap().to_owned();
    let path = PathBuf::from(initialized["slots"][0]["path"].as_str().unwrap());
    let key = fixture._repo_dir.path.join("retained-key");
    private_file(&key);
    assert!(
        key_command(&fixture, PRIVATE_KEY_COMMAND, &key, "3")
            .status
            .success()
    );
    let credential = fs::read_to_string(&key).unwrap();
    let out = run_packaged_take(&fixture, &pool, &head, &key, "take");
    let first: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(out.status.success(), "{first}");
    assert_eq!(first["path"], json!(path));
    let admin = PathBuf::from(git(&path, &["rev-parse", "--absolute-git-dir"]));
    let slot = LeasedSlot {
        fixture,
        pool,
        path,
        admin,
        head,
        credential: credential.trim_end().to_owned(),
        operation: "5".repeat(32),
    };
    guarded_workspace_check(&slot, 1);
    (slot, key, next)
}

fn return_workspace(slot: &LeasedSlot, generation: u64) {
    let returned = slot
        .fixture
        .replay("return", &slot.pool, 1, generation, &"4".repeat(32));
    let (out, returned) = finish_private(start_private(
        &slot.fixture.hive,
        returned,
        &slot.credential,
    ));
    assert!(out.status.success(), "{returned}");
}

fn take_workspace(
    slot: &LeasedSlot,
    key: &Path,
    commit: &str,
    generation: &str,
) -> (Output, Value) {
    let child = start_packaged_take_at(&slot.fixture, &slot.pool, commit, key, "take", generation);
    let out = common::wait_within(child, Duration::from_secs(30));
    let result = serde_json::from_slice(&out.stdout).unwrap();
    (out, result)
}

#[test]
fn a_returned_slot_switches_to_the_next_commit_and_reuses_an_unrelated_unit() {
    let (slot, key, next) = first_completed_workspace();
    let before = fs::metadata(slot.path.join("target")).unwrap();
    let before_consumer = workspace_unit_stamp(&slot.path, "fixture");
    let before_independent = workspace_unit_stamp(&slot.path, "independent");
    return_workspace(&slot, 1);
    let (out, ready) = take_workspace(&slot, &key, &next, "1");
    assert!(out.status.success(), "{ready}");
    assert_eq!(ready["path"], json!(slot.path));
    assert_eq!(ready["generation"], 2);
    assert_eq!(ready["commit"], next);
    assert_eq!(
        fs::read_to_string(slot.path.join("src/lib.rs")).unwrap(),
        RETAINED_EDIT
    );
    let after = fs::metadata(slot.path.join("target")).unwrap();
    assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
    assert!(
        fs::metadata(slot.path.join("src/lib.rs"))
            .unwrap()
            .modified()
            .unwrap()
            > before_consumer
    );
    guarded_workspace_check(&slot, 2);
    assert!(workspace_unit_stamp(&slot.path, "fixture") > before_consumer);
    assert_eq!(
        workspace_unit_stamp(&slot.path, "independent"),
        before_independent
    );
}

#[test]
fn a_same_commit_handoff_invalidates_only_the_reviewed_consumer() {
    let (slot, key, _) = first_completed_workspace();
    let before = workspace_unit_stamp(&slot.path, "fixture");
    let independent = workspace_unit_stamp(&slot.path, "independent");
    let anchor = slot.path.join("src/lib.rs");
    let initial_anchor = fs::metadata(&anchor).unwrap();
    let content = fs::read(&anchor).unwrap();
    return_workspace(&slot, 1);
    let (out, ready) = take_workspace(&slot, &key, &slot.head, "1");
    assert!(out.status.success(), "{ready}");
    let touched = fs::metadata(&anchor).unwrap();
    assert_eq!(
        (touched.dev(), touched.ino()),
        (initial_anchor.dev(), initial_anchor.ino())
    );
    assert!(touched.modified().unwrap() > before);
    assert_eq!(fs::read(&anchor).unwrap(), content);
    guarded_workspace_check(&slot, 2);
    assert!(workspace_unit_stamp(&slot.path, "fixture") > before);
    assert_eq!(workspace_unit_stamp(&slot.path, "independent"), independent);
}

#[test]
fn retained_preparation_refuses_a_different_prior_head_or_unreviewed_commit() {
    for wrong_head in [true, false] {
        let (slot, key, next) = first_completed_workspace();
        return_workspace(&slot, 1);
        let commit = if wrong_head {
            git(&slot.path, &["switch", "--detach", &next]);
            slot.head.clone()
        } else {
            fs::write(
                slot.fixture.repo.join("independent/src/lib.rs"),
                "pub fn changed() {}\n",
            )
            .unwrap();
            git(
                &slot.fixture.repo,
                &[
                    "commit",
                    "-qm",
                    "outside the reviewed envelope",
                    "--",
                    "independent/src/lib.rs",
                ],
            );
            git(&slot.fixture.repo, &["rev-parse", "HEAD"])
        };
        let existing = git(&slot.path, &["rev-parse", "HEAD"]);
        let stamp = workspace_unit_stamp(&slot.path, "fixture");
        let (out, refused) = take_workspace(&slot, &key, &commit, "1");
        assert_eq!(out.status.code(), Some(1), "{refused}");
        assert!(refused.get("path").is_none());
        assert_eq!(git(&slot.path, &["rev-parse", "HEAD"]), existing);
        assert_eq!(workspace_unit_stamp(&slot.path, "fixture"), stamp);
        let saved = slot.fixture.record();
        assert_eq!(saved["pools"][0]["slots"][0]["last_head"], slot.head);
        assert_eq!(saved["pools"][0]["slots"][0]["phase"]["kind"], "reserved");
    }
}

#[test]
fn a_retained_attempt_refuses_changed_effective_fixed_tool_selection_before_permit() {
    let (slot, key, next) = first_completed_workspace();
    return_workspace(&slot, 1);
    let empty = slot.fixture._repo_dir.path.join("empty-path-entry");
    fs::create_dir(&empty).unwrap();
    let mut path = std::env::var_os("PATH").unwrap();
    path.push(":");
    path.push(&empty);
    let child = packaged_take_command(&slot.fixture, &slot.pool, &next, &key, "take", "1")
        .env("PATH", path)
        .spawn()
        .unwrap();
    let out = common::wait_within(child, Duration::from_secs(30));
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(1), "{result}");
    assert!(result.get("path").is_none());
    assert_eq!(git(&slot.path, &["rev-parse", "HEAD"]), slot.head);
    let saved = slot.fixture.record();
    assert_eq!(saved["pools"][0]["slots"][0]["phase"]["kind"], "reserved");
}

#[test]
fn a_replaced_unit_stamp_with_the_same_mtime_blocks_retained_ready() {
    let (slot, key, next) = first_completed_workspace();
    return_workspace(&slot, 1);
    let stamp = workspace_unit_stamp_path(&slot.path, "fixture");
    let before = fs::metadata(&stamp).unwrap();
    let wrapper = slot.fixture._repo_dir.path.join("replace-unit-stamp");
    let counter = slot.fixture._repo_dir.path.join("preparation-checkpoints");
    fs::write(
        &wrapper,
        r#"#!/bin/sh
if [ "$1" = slot ] && [ "$2" = step-check ]; then
    count=0
    if [ -f "$HIVE_TEST_COUNTER" ]; then IFS= read -r count < "$HIVE_TEST_COUNTER"; fi
    count=$((count + 1))
    printf '%s\n' "$count" > "$HIVE_TEST_COUNTER"
    if [ "$count" = 4 ]; then
        /bin/cp -p "$HIVE_TEST_UNIT_STAMP" "$HIVE_TEST_UNIT_STAMP.replacement" || exit 20
        /bin/mv -f "$HIVE_TEST_UNIT_STAMP.replacement" "$HIVE_TEST_UNIT_STAMP" || exit 21
    fi
fi
exec "$HIVE_TEST_BINARY" "$@"
"#,
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let child = packaged_take_with_binary(
        &slot.fixture,
        &slot.pool,
        &next,
        &key,
        "take",
        "1",
        &wrapper,
    )
    .env("HIVE_TEST_BINARY", env!("CARGO_BIN_EXE_cargo-hive"))
    .env("HIVE_TEST_COUNTER", &counter)
    .env("HIVE_TEST_UNIT_STAMP", &stamp)
    .spawn()
    .unwrap();
    let out = common::wait_within(child, Duration::from_secs(30));
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    let after = fs::metadata(&stamp).unwrap();
    assert_ne!(after.ino(), before.ino());
    assert_eq!(after.modified().unwrap(), before.modified().unwrap());
    assert_eq!(out.status.code(), Some(1), "{result}");
    assert!(result.get("path").is_none());
    let saved = slot.fixture.record();
    assert_eq!(
        saved["pools"][0]["slots"][0]["phase"]["attempt"]["stage"],
        "anchors_in_progress"
    );
}

#[test]
fn changing_dep_info_during_gc_does_not_discard_a_surviving_unit() {
    // Keep the probe's PATH in the producer evidence from the first build onward.
    // The subprocess isolates that PATH from the other integration cases.
    if std::env::var_os("HIVE_TEST_GC_DEP_INFO_CARGO").is_none() {
        use cargo_hive_platform::proc::{Spec, output_with_deadline};

        let bin = common::Scratch::new("gc-dep-info-cargo");
        let cargo = Path::new(env!("CARGO"));
        let shim = bin.path.join("cargo");
        fs::write(
            &shim,
            r#"#!/bin/sh
if [ "$1" = metadata ] && [ -n "$HIVE_TEST_GC_DEP_INFO" ] && [ ! -f "$HIVE_TEST_GC_CHANGED" ] &&
    /usr/bin/grep -q '"stage"[[:space:]]*:[[:space:]]*"gc_in_progress"' "$HIVE_HOME/slots.json"; then
    /bin/cp -p "$HIVE_TEST_GC_DEP_INFO" "$HIVE_TEST_GC_CHANGED" || exit 20
    printf '%s: independent/src/lib.rs\n' "$HIVE_TEST_GC_DEP_INFO" > "$HIVE_TEST_GC_DEP_INFO" || exit 21
    /usr/bin/touch -r "$HIVE_TEST_GC_CHANGED" "$HIVE_TEST_GC_DEP_INFO" || exit 22
fi
exec "$HIVE_TEST_GC_DEP_INFO_CARGO" "$@"
"#,
        )
        .unwrap();
        fs::set_permissions(&shim, fs::Permissions::from_mode(0o700)).unwrap();
        let mut path = bin.path.as_os_str().to_owned();
        path.push(":");
        path.push(std::env::var_os("PATH").unwrap());
        let mut child = Spec::new(std::env::current_exe().unwrap());
        child.args = vec![
            "--exact".into(),
            "changing_dep_info_during_gc_does_not_discard_a_surviving_unit".into(),
        ];
        child.env_set = vec![
            ("PATH".into(), path),
            ("HIVE_TEST_GC_DEP_INFO_CARGO".into(), cargo.into()),
        ];
        let out = output_with_deadline(&child, Duration::from_secs(45), || false).unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stdout)
        );
        return;
    }

    use cargo_hive_core::{fingerprint, unit_path};
    let (slot, key, next) = first_completed_workspace();
    return_workspace(&slot, 1);
    let profile = slot.path.join("target/debug");
    let graph = fingerprint::read_profile(&profile).graph.unwrap();
    let unit = graph
        .units
        .iter()
        .find(|unit| unit.loc.package() == "fixture")
        .unwrap();
    let namespace = unit_path::fingerprint_dir(&profile, &unit.loc);
    let stamp = unit_path::stamp(&profile, &unit.loc, unit.kind);
    let output = unit_path::outputs(&profile, &unit.loc);
    let dep_info = [
        output.join(format!("fixture-{}.d", unit.loc.hex())),
        output.join("fixture.d"),
    ]
    .into_iter()
    .find(|path| path.is_file())
    .unwrap();
    let changed = slot.fixture._repo_dir.path.join("original-dep-info");
    fs::File::open(&dep_info)
        .unwrap()
        .set_times(
            fs::FileTimes::new()
                .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_600_000_000)),
        )
        .unwrap();
    let before_dep_info = fs::metadata(&dep_info).unwrap().modified().unwrap();
    let before_stamp = fs::metadata(&stamp).unwrap();
    let child = packaged_take_command(&slot.fixture, &slot.pool, &next, &key, "take", "1")
        .env("HIVE_TEST_GC_DEP_INFO", &dep_info)
        .env("HIVE_TEST_GC_CHANGED", &changed)
        .spawn()
        .unwrap();
    let out = common::wait_within(child, Duration::from_secs(30));
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(namespace.is_dir());
    assert_ne!(fs::read(&dep_info).unwrap(), fs::read(&changed).unwrap());
    assert_eq!(
        fs::metadata(&dep_info).unwrap().modified().unwrap(),
        before_dep_info
    );
    let after_stamp = fs::metadata(&stamp).unwrap();
    assert_eq!(after_stamp.ino(), before_stamp.ino());
    assert_eq!(
        after_stamp.modified().unwrap(),
        before_stamp.modified().unwrap()
    );
    assert_eq!(out.status.code(), Some(1), "{result}");
    assert!(result.get("path").is_none());
    let saved = slot.fixture.record();
    assert_eq!(
        saved["pools"][0]["slots"][0]["phase"]["attempt"]["stage"],
        "gc_in_progress"
    );
}

#[test]
fn a_fully_removed_consumer_unit_allows_retained_ready() {
    let (slot, key, next) = first_completed_workspace();
    return_workspace(&slot, 1);
    let stamp = workspace_unit_stamp_path(&slot.path, "fixture");
    let independent = workspace_unit_stamp(&slot.path, "independent");
    let fingerprint = stamp.parent().unwrap();
    fs::File::open(&stamp)
        .unwrap()
        .set_times(
            fs::FileTimes::new()
                .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1_600_000_000)),
        )
        .unwrap();
    let (out, ready) = take_workspace(&slot, &key, &next, "1");
    assert!(out.status.success(), "{ready}");
    assert_eq!(ready["path"], json!(slot.path));
    assert!(!fingerprint.exists());
    assert_eq!(workspace_unit_stamp(&slot.path, "independent"), independent);
}

#[test]
fn a_swapped_anchor_parent_receives_no_mtime_effect() {
    let (slot, key, _) = first_completed_workspace();
    return_workspace(&slot, 1);
    let outside = slot.fixture._repo_dir.path.join("outside-anchor-parent");
    fs::create_dir(&outside).unwrap();
    let sentinel = outside.join("lib.rs");
    fs::write(&sentinel, b"outside content").unwrap();
    let before_outside = fs::metadata(&sentinel).unwrap().modified().unwrap();
    let source = slot.path.join("src");
    let before_anchor = fs::metadata(source.join("lib.rs"))
        .unwrap()
        .modified()
        .unwrap();
    let displaced = slot.path.join("src-displaced");
    let wrapper = slot.fixture._repo_dir.path.join("swap-anchor-parent");
    let counter = slot.fixture._repo_dir.path.join("anchor-fd-checkpoint");
    fs::write(
        &wrapper,
        r#"#!/bin/sh
if [ "$1" = slot ] && [ "$2" = step-check ]; then
    count=0
    if [ -f "$HIVE_TEST_COUNTER" ]; then IFS= read -r count < "$HIVE_TEST_COUNTER"; fi
    count=$((count + 1))
    printf '%s\n' "$count" > "$HIVE_TEST_COUNTER"
    if [ "$count" = 4 ]; then
        /bin/mv "$HIVE_TEST_SOURCE" "$HIVE_TEST_DISPLACED" || exit 20
        /bin/ln -s "$HIVE_TEST_OUTSIDE" "$HIVE_TEST_SOURCE" || exit 21
    fi
fi
exec "$HIVE_TEST_BINARY" "$@"
"#,
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let child = packaged_take_with_binary(
        &slot.fixture,
        &slot.pool,
        &slot.head,
        &key,
        "take",
        "1",
        &wrapper,
    )
    .env("HIVE_TEST_BINARY", env!("CARGO_BIN_EXE_cargo-hive"))
    .env("HIVE_TEST_COUNTER", &counter)
    .env("HIVE_TEST_SOURCE", &source)
    .env("HIVE_TEST_DISPLACED", &displaced)
    .env("HIVE_TEST_OUTSIDE", &outside)
    .spawn()
    .unwrap();
    let out = common::wait_within(child, Duration::from_secs(30));
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(1), "{result}");
    assert!(result.get("path").is_none());
    assert_eq!(
        fs::metadata(&sentinel).unwrap().modified().unwrap(),
        before_outside
    );
    assert_eq!(fs::read(&sentinel).unwrap(), b"outside content");
    assert_eq!(
        fs::metadata(displaced.join("lib.rs"))
            .unwrap()
            .modified()
            .unwrap(),
        before_anchor
    );
    let saved = slot.fixture.record();
    assert_eq!(
        saved["pools"][0]["slots"][0]["phase"]["attempt"]["stage"],
        "anchors_in_progress"
    );
}

#[test]
fn a_retained_cleanup_skip_never_publishes_ready() {
    let (slot, key, next) = first_completed_workspace();
    return_workspace(&slot, 1);
    let mut removed = 0;
    for name in [".cargo-build-lock", ".cargo-lock", ".cargo-artifact-lock"] {
        match fs::remove_file(slot.path.join("target/debug").join(name)) {
            Ok(()) => removed += 1,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => (),
            Err(err) => panic!("could not remove fixture lock: {err}"),
        }
    }
    assert!(removed > 0);
    let (out, refused) = take_workspace(&slot, &key, &next, "1");
    assert_eq!(out.status.code(), Some(1), "{refused}");
    assert!(refused.get("path").is_none());
    assert!(
        refused["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("Scoped cleanup")),
        "{refused}"
    );
    let saved = slot.fixture.record();
    let record = &saved["pools"][0]["slots"][0];
    assert_eq!(record["phase"]["attempt"]["stage"], "gc_in_progress");
    assert_eq!(record["producer"]["kind"], "complete");
}

#[test]
fn the_packaged_checkout_refuses_conditional_git_configuration_before_effects() {
    let fixture = reviewed_pool_fixture(None);
    fixture.hive.write_ready_capabilities();
    let included = fixture._repo_dir.path.join("slot-only.gitconfig");
    let marker = fixture._repo_dir.path.join("unreviewed-filter-ran");
    fs::write(
        &included,
        format!(
            "[filter \"review_probe\"]\nsmudge = touch {} && cat\n",
            marker.display()
        ),
    )
    .unwrap();
    fs::write(
        fixture.repo.join(".gitattributes"),
        "src/lib.rs filter=review_probe\n",
    )
    .unwrap();
    git(&fixture.repo, &["add", ".gitattributes"]);
    git(&fixture.repo, &["commit", "-qm", "Track filter attributes"]);
    let target_admin = fixture.repo.join(".git/worktrees/slot-1");
    git(
        &fixture.repo,
        &[
            "config",
            &format!("includeIf.gitdir:{}.path", target_admin.display()),
            included.to_str().unwrap(),
        ],
    );
    let (out, initialized) = fixture.init();
    assert!(out.status.success());
    let pool = initialized["pool_id"].as_str().unwrap();
    let slot = PathBuf::from(initialized["slots"][0]["path"].as_str().unwrap());
    let head = git(&fixture.repo, &["rev-parse", "HEAD"]);
    let key = fixture._repo_dir.path.join("conditional-config-key");
    private_file(&key);
    assert!(
        key_command(&fixture, PRIVATE_KEY_COMMAND, &key, "3")
            .status
            .success()
    );
    let out = run_packaged_take(&fixture, pool, &head, &key, "take");
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(1), "{result}");
    assert!(!marker.exists());
    assert!(!slot.exists());
}

#[test]
fn packaged_guide_secrets_never_inherit_caller_export_attributes() {
    let fixture = reviewed_pool_fixture(None);
    fixture.hive.write_ready_capabilities();
    let (out, initialized) = fixture.init();
    assert!(out.status.success());
    let pool = initialized["pool_id"].as_str().unwrap();
    let head = git(&fixture.repo, &["rev-parse", "HEAD"]);
    let key = fixture._repo_dir.path.join("export-sentinel-key");
    private_file(&key);
    assert!(
        key_command(&fixture, PRIVATE_KEY_COMMAND, &key, "3")
            .status
            .success()
    );
    let wrapped = fixture._repo_dir.path.join("observe-export-attributes");
    let log = fixture._repo_dir.path.join("unexpected-exports");
    fs::write(&wrapped, r#"#!/bin/sh
if [ "$1" = slot ] && { [ "${credential+x}" ] || [ "${nonce+x}" ] || [ "${request+x}" ] || [ "${step_request+x}" ] || [ "${extra+x}" ]; }; then
    printf 'unexpected export\n' >> "$HIVE_TEST_EXPORT_LOG"
fi
exec "$HIVE_TEST_BINARY" "$@"
"#).unwrap();
    fs::set_permissions(&wrapped, fs::Permissions::from_mode(0o700)).unwrap();
    let helper = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugin/skills/hive/scripts/slot-handoff.sh");
    let mut command = fixture
        .hive
        .command_with(Path::new("/bin/bash"), &["--noprofile", "--norc"]);
    producer_environment(&mut command);
    let child = command
        .arg(helper)
        .arg("take")
        .arg(&wrapped)
        .arg(&fixture.repo)
        .args([
            pool,
            "1",
            "0",
            &head,
            &"1".repeat(32),
            &"2".repeat(32),
            &"3".repeat(32),
        ])
        .env_remove("BASH_ENV")
        .env("SHELLOPTS", "allexport")
        .env("HIVE_TEST_BINARY", env!("CARGO_BIN_EXE_cargo-hive"))
        .env("HIVE_TEST_EXPORT_LOG", &log)
        .envs(
            ["credential", "nonce", "request", "step_request", "extra"]
                .map(|name| (name, "synthetic-sentinel")),
        )
        .stdin(fs::File::open(&key).unwrap())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out = common::wait_within(child, Duration::from_secs(30));
    assert_no_credential(&out, &fs::read_to_string(&key).unwrap());
    assert!(
        !log.exists(),
        "an internal credential variable was present in a child environment"
    );
    assert!(
        out.status.success(),
        "packaged take failed: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

fn external_fixture_seal(fixture: &PoolFixture, head: &str, recipe: &str, argv: &[&str]) {
    let mut profile: Value = serde_json::from_slice(&fs::read(&fixture.profile).unwrap()).unwrap();
    let root_package = profile["consumers"][0]["package"]
        .as_str()
        .unwrap()
        .to_owned();
    let effect = |path: &Path| {
        let mut file = fs::File::open(path).unwrap();
        let length = file.metadata().unwrap().len();
        cargo_hive_platform::slot::sha256_reader(&mut file, length, || false)
            .unwrap()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    let material = |path: &Path| json!({"path": path, "sha256": effect(path)});
    let find_tool = |args: &[&str]| {
        let out = fixture
            .hive
            .command_with(Path::new("/usr/bin/xcrun"), args)
            .env_clear()
            .env("HOME", &fixture.hive.cargo_home.path)
            .env("PATH", "/usr/bin:/bin")
            .output()
            .unwrap();
        assert!(out.status.success());
        PathBuf::from(String::from_utf8(out.stdout).unwrap().trim())
            .canonicalize()
            .unwrap()
    };
    let release = std::env::var("RUSTUP_TOOLCHAIN").unwrap();
    let release = release.split('-').next().unwrap();
    let native = format!("{}-apple-darwin", std::env::consts::ARCH);
    let layout = if release == "1.97.1" {
        "flat"
    } else {
        "unit_dir"
    };
    let toolchain = PathBuf::from(std::env::var_os("RUSTUP_HOME").unwrap())
        .join("toolchains")
        .join(format!("{release}-{native}"));
    let lib = toolchain.join("lib");
    let bin = toolchain.join("bin");
    let sysroot = lib.join("rustlib").join(&native).join("lib");
    let inventory = |path: &Path, skip_rustlib: bool| {
        let mut paths: Vec<_> = fs::read_dir(path)
            .unwrap()
            .map(|item| item.unwrap().path())
            .filter(|entry| !skip_rustlib || entry.file_name().unwrap() != "rustlib")
            .collect();
        paths.sort();
        paths
            .iter()
            .map(|entry| material(entry))
            .collect::<Vec<_>>()
    };
    let sdk = find_tool(&["--no-cache", "--show-sdk-path"]);
    let mut tools = json!({
        "cargo": material(&bin.join("cargo")), "rustc":material(&bin.join("rustc")),
        "otool":material(&find_tool(&["--no-cache","--find","otool"])),
        "rust_runtime":inventory(&lib,true), "target_sysroot":inventory(&sysroot,false),
        "clang":material(&find_tool(&["--no-cache","--find","clang"])),
        "ld":material(&find_tool(&["--no-cache","--find","ld"])), "sdk":sdk,
        "sdk_settings":material(&sdk.join("SDKSettings.json")),
        "system_library":material(&sdk.join("usr/lib/libSystem.tbd").canonicalize().unwrap())
    });
    if recipe == "clippy" {
        tools["cargo_clippy"] = material(&bin.join("cargo-clippy"));
        tools["clippy_driver"] = material(&bin.join("clippy-driver"));
    } else {
        let source = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|dir| dir.join("cargo-nextest"))
            .find(|path| path.is_file())
            .expect("Task 7 fixture requires the already installed pinned nextest");
        let cargo_bin = fixture.hive.cargo_home.path.join("bin");
        fs::create_dir(&cargo_bin).unwrap();
        let nextest = cargo_bin.join("cargo-nextest");
        fs::copy(source, &nextest).unwrap();
        tools["cargo_nextest"] = material(&nextest);
    }
    let seal = fixture._repo_dir.path.join("external-recipe.json");
    fs::write(&seal, serde_json::to_vec(&json!({
        "schema":1,"recipe":recipe,"profile":{"id":"fixture","revision":1},
        "proof":"check-proof","invocation":"check","layout":layout,"native_target":native,
        "endpoints":[{"commit":head,"tree":git(&fixture.repo,&["rev-parse","HEAD^{tree}"])}],
        "tools":tools, "packages":[
            {"name":root_package,"version":"0.1.0","manifest":"Cargo.toml","targets":[{"name":root_package,"kind":"lib","source":"src/lib.rs"}],"dependencies":[]},
            {"name":"independent","version":"0.1.0","manifest":"independent/Cargo.toml","targets":[{"name":"independent","kind":"lib","source":"independent/src/lib.rs"}],"dependencies":[]}
        ], "data":[{"id":"source","sha256":[effect(&fixture.repo.join("src/lib.rs"))],"absent":false}]
    })).unwrap()).unwrap();
    let graph = profile["graph_inputs"].as_array_mut().unwrap();
    graph.push(json!({"locator":{"kind":"checkout","path":"independent/src/lib.rs"},"expectation":{"kind":"present","identity":{"kind":"git_object","oid":git(&fixture.repo,&["rev-parse","HEAD:independent/src/lib.rs"])}}}));
    let hash = effect(&seal);
    graph.push(json!({"locator":{"kind":"external","path":seal},"expectation":{"kind":"present","identity":{"kind":"reviewed_bytes","sha256":hash}}}));
    profile["invocations"][0]["argv"] = json!(argv);
    profile["proofs"][0]["evidence"] = json!(format!("closure-v1:sha256={hash}"));
    fs::write(&fixture.profile, serde_json::to_vec(&profile).unwrap()).unwrap();
}

const EXTERNAL_LIST_WITNESS: &str = r#"
pub fn answer() -> u32 { 42 }

#[cfg(test)]
#[test]
fn fail_if_run_instead_of_listed() { panic!("nextest executed a test body"); }

#[cfg(test)]
extern "C" fn listing_witness() {
    use std::io::Seek;
    use std::os::unix::fs::MetadataExt;
    let args: Vec<_> = std::env::args().skip(1).collect();
    let values: Vec<_> = args.iter().map(String::as_str).collect();
    let leaf = match values.as_slice() {
        ["--list", "--format", "terse"] => "list-witness",
        ["--list", "--format", "terse", "--ignored"] => "ignored-list-witness",
        _ => return,
    };
    let mut lock = std::fs::File::open("/dev/fd/9").unwrap();
    let info = lock.metadata().unwrap();
    let scope = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
    std::fs::write(scope.join(leaf), format!("{} {} {}", info.dev(), info.ino(), lock.stream_position().unwrap())).unwrap();
}

#[cfg(test)]
#[used]
#[unsafe(link_section = "__DATA,__mod_init_func")]
static INIT_WITNESS: extern "C" fn() = listing_witness;
"#;

#[test]
fn the_packaged_guide_attests_the_exact_native_external_producers() {
    let recipes: [(&str, &[&str]); 2] = [
        (
            "clippy",
            &[
                "clippy",
                "--workspace",
                "--all-targets",
                "--all-features",
                "--locked",
                "--",
                "-D",
                "warnings",
            ],
        ),
        (
            "nextest_workspace",
            &[
                "nextest",
                "run",
                "--workspace",
                "--all-features",
                "--no-run",
                "--locked",
                "--user-config-file",
                "none",
            ],
        ),
    ];
    for (recipe, args) in recipes {
        exercise_external_recipe(recipe, args, false, None);
    }
}

fn exercise_external_recipe(
    recipe: &str,
    args: &[&str],
    gateways: bool,
    expected_refusal: Option<&str>,
) {
    let fixture = reviewed_pool_fixture(
        recipe
            .starts_with("nextest_")
            .then_some(EXTERNAL_LIST_WITNESS),
    );
    if gateways {
        for path in ["Cargo.toml", "Cargo.lock"] {
            let file = fixture.repo.join(path);
            let contents = fs::read_to_string(&file)
                .unwrap()
                .replace("name = \"fixture\"", "name = \"gateways\"");
            fs::write(file, contents).unwrap();
        }
        git(
            &fixture.repo,
            &[
                "commit",
                "-qm",
                "Name the selected workspace package",
                "--",
                "Cargo.toml",
                "Cargo.lock",
            ],
        );
        let mut profile: Value =
            serde_json::from_slice(&fs::read(&fixture.profile).unwrap()).unwrap();
        for item in profile["graph_inputs"].as_array_mut().unwrap() {
            let path = item["locator"]["path"].as_str().unwrap();
            item["expectation"]["identity"]["oid"] =
                json!(git(&fixture.repo, &["rev-parse", &format!("HEAD:{path}")]));
        }
        profile["consumers"][0]["package"] = json!("gateways");
        fs::write(&fixture.profile, serde_json::to_vec(&profile).unwrap()).unwrap();
    }
    fixture.hive.write_ready_capabilities();
    let head = git(&fixture.repo, &["rev-parse", "HEAD"]);
    external_fixture_seal(&fixture, &head, recipe, args);
    let (out, initialized) = fixture.init();
    assert!(out.status.success(), "{initialized}");
    let pool = initialized["pool_id"].as_str().unwrap();
    let key = fixture._repo_dir.path.join("external-recipe-key");
    private_file(&key);
    assert!(
        key_command(&fixture, PRIVATE_KEY_COMMAND, &key, "3")
            .status
            .success()
    );
    let out = run_packaged_take(&fixture, pool, &head, &key, "take");
    if let Some(detail) = expected_refusal {
        assert_eq!(out.status.code(), Some(1), "{recipe}: {out:?}");
        let checkout = Path::new(initialized["slots"][0]["path"].as_str().unwrap());
        assert!(!checkout.join("target").exists());
        let refused: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert!(
            refused["detail"]
                .as_str()
                .is_some_and(|observed| observed.contains(detail)),
            "{refused}"
        );
        return;
    }
    assert!(out.status.success(), "{recipe}: {out:?}");
    let helper = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugin/skills/hive/scripts/slot-handoff.sh");
    let mut command = fixture
        .hive
        .command_with(Path::new("/bin/bash"), &["--noprofile", "--norc"]);
    producer_environment(&mut command);
    let child = command
        .arg(helper)
        .args(["produce", env!("CARGO_BIN_EXE_cargo-hive")])
        .arg(&fixture.repo)
        .args([pool, "1", "1", &"5".repeat(32), "check"])
        .stdin(fs::File::open(&key).unwrap())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out = common::wait_within(child, Duration::from_secs(45));
    assert_no_credential(&out, &fs::read_to_string(&key).unwrap());
    assert!(out.status.success(), "{recipe}: {out:?}");
    let complete: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(complete["kind"], "producer_complete");
    if recipe.starts_with("nextest_") {
        let checkout = Path::new(initialized["slots"][0]["path"].as_str().unwrap());
        if gateways {
            let profile =
                cargo_hive_core::fingerprint::read_profile(&checkout.join("target/debug"));
            let graph = profile.graph.unwrap();
            assert!(
                graph
                    .units
                    .iter()
                    .any(|unit| unit.loc.package() == "gateways")
            );
            assert!(
                graph
                    .units
                    .iter()
                    .all(|unit| unit.loc.package() != "independent")
            );
        }
        let guard = fs::metadata(
            fixture
                .hive
                .home
                .path
                .join("slot-locks")
                .join(pool)
                .join("slot-1.lock"),
        )
        .unwrap();
        let mut offset = None;
        for leaf in ["list-witness", "ignored-list-witness"] {
            let evidence = fs::read_to_string(checkout.join("target").join(leaf)).unwrap();
            let facts: Vec<_> = evidence
                .split_ascii_whitespace()
                .map(|value| value.parse::<u64>().unwrap())
                .collect();
            assert_eq!(facts.len(), 3);
            assert_eq!((facts[0], facts[1]), (guard.dev(), guard.ino()));
            assert!(facts[2] >= 1 << 32);
            assert_eq!(*offset.get_or_insert(facts[2]), facts[2]);
        }
    }
}

fn fixture_sha(bytes: &[u8]) -> String {
    cargo_hive_platform::slot::sha256(bytes)
        .unwrap()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn fixture_inventory(root: &Path, exclude_git: bool) -> String {
    fn collect(
        root: &Path,
        directory: &Path,
        exclude_git: bool,
        rows: &mut Vec<(PathBuf, u64, Option<String>)>,
    ) {
        for item in fs::read_dir(directory).unwrap() {
            let item = item.unwrap();
            if exclude_git && directory == root && item.file_name() == ".git" {
                continue;
            }
            let path = item.path();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if item.file_type().unwrap().is_dir() {
                rows.push((relative, 0, None));
                collect(root, &path, exclude_git, rows);
            } else {
                assert!(item.file_type().unwrap().is_file());
                let bytes = fs::read(path).unwrap();
                rows.push((relative, bytes.len() as u64, Some(fixture_sha(&bytes))));
            }
        }
    }
    let mut rows = Vec::new();
    collect(root, root, exclude_git, &mut rows);
    rows.sort();
    fixture_sha(&serde_json::to_vec(&rows).unwrap())
}

fn fixture_resolve_digest(metadata: &Value, checkout: &Path) -> String {
    let ids: std::collections::BTreeMap<_, _> = metadata["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|pkg| {
            let id = pkg["id"].as_str().unwrap();
            let logical = if pkg["source"].is_null() {
                format!(
                    "workspace:{}",
                    Path::new(pkg["manifest_path"].as_str().unwrap())
                        .strip_prefix(checkout)
                        .unwrap()
                        .display()
                )
            } else {
                id.to_owned()
            };
            (id, logical)
        })
        .collect();
    let nodes: std::collections::BTreeMap<_, _> = metadata["resolve"]["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| {
            let mut features: Vec<_> = node["features"]
                .as_array()
                .unwrap()
                .iter()
                .map(|val| val.as_str().unwrap())
                .collect();
            features.sort();
            let mut deps: Vec<_> = node["deps"]
                .as_array()
                .unwrap()
                .iter()
                .map(|dep| {
                    let mut kinds: Vec<_> = dep["dep_kinds"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|kind| (kind["kind"].as_str(), kind["target"].as_str()))
                        .collect();
                    kinds.sort();
                    (
                        dep["name"].as_str().unwrap(),
                        &ids[dep["pkg"].as_str().unwrap()],
                        kinds,
                    )
                })
                .collect();
            deps.sort();
            (&ids[node["id"].as_str().unwrap()], (features, deps))
        })
        .collect();
    fixture_sha(&serde_json::to_vec(&nodes).unwrap())
}

#[test]
fn the_guarded_producer_binds_extracted_registry_bytes_before_writing() {
    exercise_materialized_dependency(DependencyFixture::Registry);
}

#[test]
fn the_guarded_producer_binds_local_git_revision_and_bytes_before_writing() {
    exercise_materialized_dependency(DependencyFixture::Git);
}

#[test]
fn the_guarded_producer_requires_exact_review_for_an_external_build_script() {
    exercise_materialized_dependency(DependencyFixture::ReviewedBuild);
}

#[test]
fn the_guarded_producer_refuses_an_incomplete_registry_extraction_before_metadata() {
    exercise_materialized_dependency(DependencyFixture::IncompleteRegistry);
}

#[derive(Clone, Copy)]
enum DependencyFixture {
    Registry,
    IncompleteRegistry,
    Git,
    ReviewedBuild,
}

fn exercise_materialized_dependency(scenario: DependencyFixture) {
    let git_dependency = matches!(
        scenario,
        DependencyFixture::Git | DependencyFixture::ReviewedBuild
    );
    let incomplete = matches!(scenario, DependencyFixture::IncompleteRegistry);
    let reviewed_build = matches!(scenario, DependencyFixture::ReviewedBuild);
    let fixture = reviewed_pool_fixture(Some(if git_dependency {
        "pub fn answer() -> u32 { git_leaf::value() }\n"
    } else {
        "use pin_project_lite::pin_project;\npin_project! { struct Sample { value: u32 } }\npub fn answer() -> u32 { Sample { value: 42 }.value }\n"
    }));
    fixture.hive.write_ready_capabilities();
    let home = &fixture.hive.cargo_home.path;
    let (dependency, requirement, git_revision) = if git_dependency {
        let donor = init_repo(&fixture._repo_dir.path.join("material-donor"), "");
        let manifest = fs::read_to_string(donor.join("Cargo.toml")).unwrap();
        fs::write(
            donor.join("Cargo.toml"),
            manifest.replace("name = \"fixture\"", "name = \"git-leaf\""),
        )
        .unwrap();
        fs::write(donor.join("src/lib.rs"), "pub fn value() -> u32 { 42 }\n").unwrap();
        if reviewed_build {
            fs::write(
                donor.join("build.rs"),
                "fn main() { println!(\"cargo:rustc-env=ROAD17_REVIEWED_BUILD=ready\"); }\n",
            )
            .unwrap();
            fs::write(
                donor.join("src/lib.rs"),
                "const _: &str = env!(\"ROAD17_REVIEWED_BUILD\");\npub fn value() -> u32 { 42 }\n",
            )
            .unwrap();
            git(&donor, &["add", "build.rs"]);
        }
        if matches!(scenario, DependencyFixture::Git) {
            let nested = donor.join("leaf");
            fs::create_dir(&nested).unwrap();
            fs::rename(donor.join("Cargo.toml"), nested.join("Cargo.toml")).unwrap();
            fs::rename(donor.join("src"), nested.join("src")).unwrap();
            fs::write(
                donor.join("Cargo.toml"),
                "[workspace]\nmembers = [\"leaf\"]\nresolver = \"2\"\n",
            )
            .unwrap();
            fs::write(donor.join("shared.txt"), "reviewed checkout-level data").unwrap();
            fs::write(
                nested.join("src/lib.rs"),
                "pub fn value() -> u32 { include_str!(\"../../shared.txt\").len() as u32 }\n",
            )
            .unwrap();
            git(&donor, &["add", "-A"]);
        }
        git(
            &donor,
            &["commit", "-qam", "Name the compiled Git dependency"],
        );
        let revision = git(&donor, &["rev-parse", "HEAD"]);
        let requirement = format!(
            "git-leaf = {{ git = \"file://{}\", rev = \"{}\" }}\n",
            donor.display(),
            revision
        );
        ("git-leaf", requirement, Some(revision))
    } else {
        let cache = PathBuf::from(std::env::var_os("CARGO_HOME").unwrap());
        let repository = "index.crates.io-1949cf8c6b5b557f";
        let archive = format!("registry/cache/{repository}/pin-project-lite-0.2.17.crate");
        for path in [
            format!("registry/index/{repository}/config.json"),
            format!("registry/index/{repository}/.cache/pi/n-/pin-project-lite"),
            archive,
        ] {
            let output = home.join(&path);
            fs::create_dir_all(output.parent().unwrap()).unwrap();
            fs::copy(cache.join(path), output).unwrap();
        }
        (
            "pin-project-lite",
            "pin-project-lite = \"=0.2.17\"\n".to_owned(),
            None,
        )
    };
    let manifest = fixture.repo.join("Cargo.toml");
    let updated = fs::read_to_string(&manifest).unwrap().replace(
        "[dependencies]\n",
        &format!("[dependencies]\n{requirement}"),
    );
    fs::write(manifest, updated).unwrap();
    let cargo = |args: &[&str]| {
        let out = fixture
            .hive
            .command_with(Path::new("cargo"), args)
            .current_dir(&fixture.repo)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        out.stdout
    };
    if git_dependency {
        let out = fixture
            .hive
            .command_with(Path::new("cargo"), &["generate-lockfile"])
            .current_dir(&fixture.repo)
            .env("CARGO_NET_OFFLINE", "false")
            .env("CARGO_NET_GIT_FETCH_WITH_CLI", "true")
            .env("GIT_ALLOW_PROTOCOL", "file")
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
    } else {
        cargo(&["generate-lockfile", "--offline"]);
    }
    git(
        &fixture.repo,
        &[
            "commit",
            "-qm",
            "Pin the isolated dependency",
            "--",
            "Cargo.toml",
            "Cargo.lock",
        ],
    );
    let metadata: Value = serde_json::from_slice(&cargo(&[
        "metadata",
        "--format-version",
        "1",
        "--all-features",
        "--locked",
        "--offline",
    ]))
    .unwrap();
    let package = metadata["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|pkg| pkg["name"] == dependency)
        .unwrap();
    let root = Path::new(package["manifest_path"].as_str().unwrap())
        .parent()
        .unwrap();
    let marker = root.join(".cargo-ok");
    if incomplete {
        fs::remove_file(&marker).unwrap();
    }
    let pin = root.join("src/lib.rs");
    let mut profile: Value = serde_json::from_slice(&fs::read(&fixture.profile).unwrap()).unwrap();
    for fact in profile["graph_inputs"].as_array_mut().unwrap() {
        let path = fact["locator"]["path"].as_str().unwrap();
        fact["expectation"]["identity"]["oid"] =
            json!(git(&fixture.repo, &["rev-parse", &format!("HEAD:{path}")]));
    }
    fs::write(&fixture.profile, serde_json::to_vec(&profile).unwrap()).unwrap();
    let head = git(&fixture.repo, &["rev-parse", "HEAD"]);
    let args = [
        "clippy",
        "--workspace",
        "--all-targets",
        "--all-features",
        "--locked",
        "--",
        "-D",
        "warnings",
    ];
    external_fixture_seal(&fixture, &head, "clippy", &args);
    let seal = fixture._repo_dir.path.join("external-recipe.json");
    let mut closure: Value = serde_json::from_slice(&fs::read(&seal).unwrap()).unwrap();
    let relative = root.strip_prefix(home).unwrap();
    let origin = if let Some(revision) = &git_revision {
        let git_checkout: PathBuf = relative.iter().take(4).collect();
        json!({"kind":"git", "revision":revision, "checkout":git_checkout, "tree_sha256":fixture_inventory(&home.join(&git_checkout), true)})
    } else {
        let index = relative.parent().unwrap().file_name().unwrap();
        let leaf = format!(
            "{dependency}-{}.crate",
            package["version"].as_str().unwrap()
        );
        let archive = home.join("registry/cache").join(index).join(leaf);
        json!({"kind":"registry", "checksum":fixture_sha(&fs::read(archive).unwrap())})
    };
    closure["packages"][0]["dependencies"] = json!([dependency]);
    closure["materialized"] = json!({"resolve_sha256":fixture_resolve_digest(&metadata, &fixture.repo),
        "packages":[{"id":package["id"],"name":dependency,"version":package["version"], "source":package["source"],
            "root":relative, "inventory_sha256":fixture_inventory(root,git_dependency), "origin":origin}]});
    let review = reviewed_build.then(|| {
        let review = fixture._repo_dir.path.join("reviewed-executable.json");
        let target = json!({"name":"build-script-build", "kind":"custom_build", "source":"build.rs"});
        let bytes = serde_json::to_vec(&json!({
            "schema":1,"profile":{"id":"fixture","revision":1},"invocation":"check","recipe":"clippy",
            "package_id":package["id"],"inventory_sha256":fixture_inventory(root,true),
            "source_sha256":fixture_sha(&fs::read(root.join("build.rs")).unwrap()),
            "target":target,"behavior":"metadata_only_build_script"
        })).unwrap();
        fs::write(&review, &bytes).unwrap();
        closure["materialized"]["packages"][0]["executables"] = json!([
            {"target":target,"review_sha256":fixture_sha(&bytes)}
        ]);
        (review, bytes)
    });
    let bytes = serde_json::to_vec(&closure).unwrap();
    fs::write(&seal, &bytes).unwrap();
    let mut profile: Value = serde_json::from_slice(&fs::read(&fixture.profile).unwrap()).unwrap();
    profile["graph_inputs"]
        .as_array_mut()
        .unwrap()
        .last_mut()
        .unwrap()["expectation"]["identity"]["sha256"] = json!(fixture_sha(&bytes));
    profile["proofs"][0]["evidence"] = json!(format!("closure-v1:sha256={}", fixture_sha(&bytes)));
    if let Some((path, bytes)) = &review {
        profile["graph_inputs"].as_array_mut().unwrap().push(json!({
            "locator":{"kind":"external","path":path},
            "expectation":{"kind":"present","identity":{"kind":"reviewed_bytes","sha256":fixture_sha(bytes)}}
        }));
    }
    fs::write(&fixture.profile, serde_json::to_vec(&profile).unwrap()).unwrap();
    let (out, initialized) = fixture.init();
    assert!(out.status.success(), "{initialized}");
    let pool = initialized["pool_id"].as_str().unwrap();
    let checkout = PathBuf::from(initialized["slots"][0]["path"].as_str().unwrap());
    let key = fixture._repo_dir.path.join("registry-material-key");
    private_file(&key);
    assert!(
        key_command(&fixture, PRIVATE_KEY_COMMAND, &key, "3")
            .status
            .success()
    );
    let out = run_packaged_take(&fixture, pool, &head, &key, "take");
    if incomplete {
        assert_eq!(out.status.code(), Some(1), "{out:?}");
        assert!(
            !marker.exists(),
            "Cargo metadata tried to re-extract a sealed package"
        );
        assert!(!checkout.join("target").exists());
        return;
    }
    assert!(out.status.success(), "{out:?}");
    let produce = |operation: &str| {
        let helper = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../plugin/skills/hive/scripts/slot-handoff.sh");
        let mut cmd = fixture
            .hive
            .command_with(Path::new("/bin/bash"), &["--noprofile", "--norc"]);
        producer_environment(&mut cmd);
        let child = cmd
            .arg(helper)
            .args(["produce", env!("CARGO_BIN_EXE_cargo-hive")])
            .arg(&fixture.repo)
            .args([pool, "1", "1", operation, "check"])
            .stdin(fs::File::open(&key).unwrap())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        common::wait_within(child, Duration::from_secs(50))
    };
    if let Some((path, bytes)) = &review {
        fs::remove_file(path).unwrap();
        assert_eq!(produce(&"9".repeat(32)).status.code(), Some(1));
        assert!(!checkout.join("target").exists());
        fs::write(path, bytes).unwrap();
    }
    if let Some(revision) = &git_revision {
        git(
            root,
            &[
                "commit",
                "--allow-empty",
                "-qm",
                "Same bytes at an unapproved commit",
            ],
        );
        assert_eq!(produce(&"8".repeat(32)).status.code(), Some(1));
        assert!(!checkout.join("target").exists());
        git(root, &["reset", "--soft", revision]);
    }
    if matches!(scenario, DependencyFixture::Git) {
        let shared = root.parent().unwrap().join("shared.txt");
        let original = fs::read(&shared).unwrap();
        let mtime = fs::metadata(&shared).unwrap().modified().unwrap();
        fs::write(&shared, b"unapproved sibling source").unwrap();
        fs::File::open(&shared)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
        let out = produce(&"a".repeat(32));
        assert_eq!(out.status.code(), Some(1), "{out:?}");
        assert!(!checkout.join("target").exists());
        fs::write(&shared, original).unwrap();
        fs::File::open(&shared)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
    }
    let original = fs::read(&pin).unwrap();
    let modified = fs::metadata(&pin).unwrap().modified().unwrap();
    fs::write(
        &pin,
        [original.as_slice(), b"\n// altered cached source\n"].concat(),
    )
    .unwrap();
    fs::File::open(&pin)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    let out = produce(&"5".repeat(32));
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(!checkout.join("target").exists());
    fs::write(&pin, original).unwrap();
    fs::File::open(&pin)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    let added = root.join("unreviewed-input");
    fs::write(&added, b"outside the approved package inventory").unwrap();
    assert_eq!(produce(&"6".repeat(32)).status.code(), Some(1));
    assert!(!checkout.join("target").exists());
    fs::remove_file(added).unwrap();
    let out = produce(&"7".repeat(32));
    assert!(out.status.success(), "{out:?}");
    let record: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(record["kind"], "producer_complete");
    let built = cargo_hive_core::fingerprint::read_profile(&checkout.join("target/debug"))
        .graph
        .unwrap();
    assert!(
        built
            .units
            .iter()
            .any(|unit| unit.loc.package() == dependency)
    );
    if reviewed_build {
        assert!(
            built
                .units
                .iter()
                .any(|unit| unit.loc.package() == dependency
                    && unit.kind == cargo_hive_core::fingerprint::UnitKind::RunBuildScript)
        );
    }
}

#[test]
fn the_gateways_nextest_recipe_lists_only_the_exact_selected_package() {
    let args = [
        "nextest",
        "run",
        "-p",
        "gateways",
        "--no-run",
        "--locked",
        "--user-config-file",
        "none",
    ];
    exercise_external_recipe("nextest_gateways", &args, true, None);
    let mut altered = args;
    altered[3] = "independent";
    exercise_external_recipe(
        "nextest_gateways",
        &altered,
        true,
        Some("native build closure"),
    );
    exercise_external_recipe(
        "nextest_gateways",
        &args,
        false,
        Some("native harness and package scope"),
    );
}

#[test]
fn a_lost_packaged_ready_reply_can_be_replayed_from_the_original_request() {
    let fixture = reviewed_pool_fixture(None);
    fixture.hive.write_ready_capabilities();
    let (out, initialized) = fixture.init();
    assert!(out.status.success());
    let pool = initialized["pool_id"].as_str().unwrap();
    let head = git(&fixture.repo, &["rev-parse", "HEAD"]);
    let key = fixture._repo_dir.path.join("lost-ready-key");
    private_file(&key);
    assert!(
        key_command(&fixture, PRIVATE_KEY_COMMAND, &key, "3")
            .status
            .success()
    );
    let credential = fs::read_to_string(&key).unwrap();
    let mut guide = start_packaged_take(&fixture, pool, &head, &key, "take");
    drop(guide.stdout.take());
    let out = common::wait_within(guide, Duration::from_secs(30));
    assert!(!out.status.success());
    assert_no_credential(&out, &credential);
    let saved = fixture.record();
    assert_eq!(saved["pools"][0]["slots"][0]["phase"]["kind"], "leased");

    let original = fixture.replay("outcome", pool, 1, 1, &"2".repeat(32));
    let (out, permit) = finish_private(start_private(&fixture.hive, original, &credential));
    assert!(out.status.success(), "{permit}");
    assert_eq!(permit["current"], false);
    assert!(permit.get("path").is_none());
    let mut request = fixture.replay("outcome", pool, 1, 1, &"3".repeat(32));
    request.extend([
        "--attempt".into(),
        permit["attempt"].as_u64().unwrap().to_string(),
    ]);
    let (out, ready) = finish_private(start_private(&fixture.hive, request, &credential));
    assert!(out.status.success(), "{ready}");
    assert_eq!(ready["kind"], "ready");
    assert_eq!(ready["current"], true);
    assert_eq!(ready["path"], initialized["slots"][0]["path"]);
    assert_eq!(fixture.record()["revision"], saved["revision"]);
}

#[test]
fn a_requested_initial_seed_without_registry_units_stays_unavailable() {
    let fixture = reviewed_pool_fixture(None);
    fixture.hive.write_ready_capabilities();
    let (out, initialized) = fixture.init();
    assert!(out.status.success());
    let pool = initialized["pool_id"].as_str().unwrap();
    let checkout = PathBuf::from(initialized["slots"][0]["path"].as_str().unwrap());
    let head = git(&fixture.repo, &["rev-parse", "HEAD"]);
    let key = fixture._repo_dir.path.join("seed-key");
    private_file(&key);
    assert!(
        key_command(&fixture, PRIVATE_KEY_COMMAND, &key, "3")
            .status
            .success()
    );
    let out = run_packaged_take(&fixture, pool, &head, &key, "take-seeded");
    assert_no_credential(&out, &fs::read_to_string(&key).unwrap());
    assert_eq!(out.status.code(), Some(1));
    let refused: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(refused.get("path").is_none());
    assert!(
        refused["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("registry-only"))
    );
    let saved = fixture.record();
    let slot = &saved["pools"][0]["slots"][0];
    assert_eq!(slot["phase"]["kind"], "preparing");
    assert_eq!(slot["phase"]["attempt"]["stage"], "seed_in_progress");
    assert_eq!(slot["producer"]["kind"], "never_built");
    assert!(!checkout.join("target").exists());
}

fn registry_seed_pool() -> (PoolFixture, common::registry_fixture::RegistryRepo, PathBuf) {
    use common::registry_fixture::{RegistryRepo, cargo_json};
    let fixture = reviewed_pool_fixture(None);
    fixture.hive.write_ready_capabilities();
    fs::create_dir(fixture.hive.home.path.join("no-cache")).unwrap();
    let registry_fixture = RegistryRepo::new(&fixture.hive);
    let manifest = fixture.repo.join("Cargo.toml");
    let donor = fs::read_to_string(&manifest)
        .unwrap()
        .replace("[dependencies]\n", "[dependencies]\nleafcrate = \"1\"\n");
    fs::write(manifest, donor).unwrap();
    let donor = cargo_json(&fixture.hive, &fixture.repo, &["build"]);
    assert!(
        donor
            .registry_recompiled
            .iter()
            .any(|unit| unit.starts_with("leafcrate"))
    );
    // The donor prepares the registry store, but the pinned first-ready source stays unchanged.
    git(
        &fixture.repo,
        &["restore", "--", "Cargo.toml", "Cargo.lock"],
    );
    fs::remove_file(fixture.hive.cargo_home.path.join("config.toml")).unwrap();
    fixture
        .hive
        .config(&json!({"secondary_roots": [fixture.hive.secondary_root.path]}).to_string());
    let prefill = fixture.hive.secondary_root.path.join("prefill");
    git(
        &fixture.repo,
        &[
            "worktree",
            "add",
            "--detach",
            prefill.to_str().unwrap(),
            "HEAD",
        ],
    );
    let (out, seeded) = fixture.hive.json(&["seed", prefill.to_str().unwrap()]);
    assert!(out.status.success(), "{seeded}");
    assert!(
        seeded["counts"]["fill_entries_added"].as_u64().unwrap() > 0,
        "{seeded}"
    );
    let pool_root = fixture.hive.secondary_root.path.join("pool-root");
    fs::create_dir(&pool_root).unwrap();
    (fixture, registry_fixture, pool_root)
}

#[test]
fn the_packaged_guide_can_attest_a_nonempty_registry_only_initial_seed() {
    let (fixture, _registry_fixture, pool_root) = registry_seed_pool();
    let (out, initialized) = fixture.init_at(&pool_root);
    assert!(out.status.success(), "{initialized}");
    let pool = initialized["pool_id"].as_str().unwrap();
    let checkout = PathBuf::from(initialized["slots"][0]["path"].as_str().unwrap());
    let head = git(&fixture.repo, &["rev-parse", "HEAD"]);
    let key = fixture._repo_dir.path.join("seed-key");
    private_file(&key);
    assert!(
        key_command(&fixture, PRIVATE_KEY_COMMAND, &key, "3")
            .status
            .success()
    );
    let out = run_packaged_take(&fixture, pool, &head, &key, "take-seeded");
    assert_no_credential(&out, &fs::read_to_string(&key).unwrap());
    let ready: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        out.status.success(),
        "{ready}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(ready["path"], json!(checkout));
    assert_eq!(ready["use_policy"], "guide_guarded_cargo_only");
    let saved = fixture.record();
    let slot = &saved["pools"][0]["slots"][0];
    assert_eq!(slot["phase"]["kind"], "leased");
    let producer = &slot["producer"];
    assert_eq!(producer["kind"], "seed_only");
    assert_eq!(producer["registry_units"], 1);
    assert_eq!(producer["evidence"]["roots"]["target"]["kind"], "absent");
    assert_eq!(producer["final_roots"]["target"]["kind"], "present");
    assert!(checkout.join("target/CACHEDIR.TAG").is_file());
}

#[test]
fn slot_seed_refuses_a_foreign_resolver_without_creating_the_target() {
    let (fixture, _registry_fixture, pool_root) = registry_seed_pool();
    let (out, initialized) = fixture.init_at(&pool_root);
    assert!(out.status.success());
    let pool = initialized["pool_id"].as_str().unwrap();
    let checkout = PathBuf::from(initialized["slots"][0]["path"].as_str().unwrap());
    let target = checkout.join("target");
    let foreign = init_repo(&fixture._repo_dir.path.join("foreign-owner"), "");
    register(&fixture.hive, &foreign);
    fs::create_dir(foreign.join(".cargo")).unwrap();
    fs::write(
        foreign.join(".cargo/config.toml"),
        format!("[build]\ntarget-dir = {:?}\n", target),
    )
    .unwrap();
    let head = git(&fixture.repo, &["rev-parse", "HEAD"]);
    let key = fixture._repo_dir.path.join("foreign-seed-key");
    private_file(&key);
    assert!(
        key_command(&fixture, PRIVATE_KEY_COMMAND, &key, "3")
            .status
            .success()
    );
    let out = run_packaged_take(&fixture, pool, &head, &key, "take-seeded");
    let refused: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(out.status.code(), Some(1), "{refused}");
    assert!(
        !target.exists(),
        "slot seed wrote to a target resolved by another workspace"
    );
    let saved = fixture.record();
    let slot = &saved["pools"][0]["slots"][0];
    assert_eq!(slot["producer"]["kind"], "never_built");
}

#[test]
fn guarded_seed_refuses_a_fifo_store_without_opening_it() {
    let (fixture, _registry, pool_root) = registry_seed_pool();
    let (out, initialized) = fixture.init_at(&pool_root);
    assert!(out.status.success());
    let pool = initialized["pool_id"].as_str().unwrap();
    let target = PathBuf::from(initialized["slots"][0]["path"].as_str().unwrap()).join("target");
    let store = fixture
        .hive
        .home
        .path
        .join(cargo_hive_seed::store::STORE_FILE);
    fs::remove_file(&store).unwrap();
    assert!(
        fixture
            .hive
            .command_with(Path::new("/usr/bin/mkfifo"), &["-m", "600"])
            .arg(&store)
            .status()
            .unwrap()
            .success()
    );
    let head = git(&fixture.repo, &["rev-parse", "HEAD"]);
    let key = fixture._repo_dir.path.join("fifo-seed-key");
    private_file(&key);
    assert!(
        key_command(&fixture, PRIVATE_KEY_COMMAND, &key, "3")
            .status
            .success()
    );
    let mut guide = packaged_take_command(&fixture, pool, &head, &key, "take-seeded", "0")
        .process_group(0)
        .spawn()
        .unwrap();
    let until = Instant::now() + Duration::from_secs(8);
    while guide.try_wait().unwrap().is_none() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(30));
    }
    let promptly_refused = guide.try_wait().unwrap().is_some();
    if !promptly_refused {
        // SAFETY: this fixture created the dedicated local process group.
        unsafe {
            libc::kill(-(guide.id() as i32), libc::SIGKILL);
        }
    }
    let out = common::wait_within(guide, Duration::from_secs(3));
    assert!(
        promptly_refused,
        "the seeded guide blocked opening a nonregular store"
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(!target.exists());
}

#[test]
fn guarded_seed_rechecks_the_registry_after_waiting_for_the_store_lock() {
    let (fixture, _registry, pool_root) = registry_seed_pool();
    let (out, initialized) = fixture.init_at(&pool_root);
    assert!(out.status.success());
    let pool = initialized["pool_id"].as_str().unwrap();
    let target = PathBuf::from(initialized["slots"][0]["path"].as_str().unwrap()).join("target");
    let foreign = init_repo(&fixture._repo_dir.path.join("late-registered-owner"), "");
    fs::create_dir(foreign.join(".cargo")).unwrap();
    fs::write(
        foreign.join(".cargo/config.toml"),
        format!("[build]\ntarget-dir = {:?}\n", target),
    )
    .unwrap();
    let head = git(&fixture.repo, &["rev-parse", "HEAD"]);
    let key = fixture._repo_dir.path.join("late-seed-key");
    private_file(&key);
    assert!(
        key_command(&fixture, PRIVATE_KEY_COMMAND, &key, "3")
            .status
            .success()
    );
    let wrapper = fixture._repo_dir.path.join("identify-seed-child");
    let pid_file = fixture._repo_dir.path.join("seed-child-pid");
    fs::write(
        &wrapper,
        r#"#!/bin/sh
if [ "$1" = slot ] && [ "$2" = step-check ]; then printf '%s\n' "$$" > "$HIVE_TEST_PID"; fi
exec "$HIVE_TEST_BINARY" "$@"
"#,
    )
    .unwrap();
    fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
    let state = cargo_hive_state::State::open_at(fixture.hive.home.path.clone()).unwrap();
    let guard = state.try_store_exclusive().unwrap().unwrap();
    let mut guide =
        packaged_take_with_binary(&fixture, pool, &head, &key, "take-seeded", "0", &wrapper)
            .env("HIVE_TEST_BINARY", env!("CARGO_BIN_EXE_cargo-hive"))
            .env("HIVE_TEST_PID", &pid_file)
            .process_group(0)
            .spawn()
            .unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    let mut waiting = false;
    while guide.try_wait().unwrap().is_none() && Instant::now() < until {
        if let Ok(pid) = fs::read_to_string(&pid_file) {
            let probe = fixture
                .hive
                .command_with(
                    Path::new("/usr/sbin/lsof"),
                    &["-a", "-p", pid.trim(), "-Fn", "--"],
                )
                .arg(fixture.hive.home.path.join("store.lock"))
                .output()
                .unwrap();
            if probe.status.success() {
                waiting = true;
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    if waiting {
        register(&fixture.hive, &foreign);
    }
    drop(guard);
    if !waiting {
        // SAFETY: this fixture created the dedicated local process group.
        unsafe {
            libc::kill(-(guide.id() as i32), libc::SIGKILL);
        }
    }
    let out = common::wait_within(guide, Duration::from_secs(10));
    assert!(
        waiting,
        "did not witness the seed child holding the store-lock descriptor"
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        !target.exists(),
        "the stale registry scope authorized a seed write"
    );
}

#[test]
fn the_packaged_guide_refuses_an_unfetched_tree_input_before_any_checkout() {
    let scratch = Scratch::new("slot-missing-blob");
    let origin = init_repo(&scratch.path.join("origin"), "");
    git(&origin, &["switch", "-c", "requested"]);
    fs::write(origin.join("new-input"), b"requires a missing blob").unwrap();
    git(&origin, &["add", "new-input"]);
    git(&origin, &["commit", "-qm", "new input"]);
    let commit = git(&origin, &["rev-parse", "HEAD"]);
    let blob = git(&origin, &["rev-parse", "HEAD:new-input"]);
    git(&origin, &["switch", "main"]);
    git(&origin, &["config", "uploadpack.allowFilter", "true"]);
    git(
        &origin,
        &["config", "uploadpack.allowAnySHA1InWant", "true"],
    );
    git(
        &scratch.path,
        &[
            "clone",
            "--no-local",
            "--single-branch",
            "--filter=blob:none",
            origin.to_str().unwrap(),
            "owner",
        ],
    );
    let repo = scratch.path.join("owner").canonicalize().unwrap();
    git(&repo, &["config", "protocol.file.allow", "always"]);
    git(
        &repo,
        &["fetch", "--filter=blob:none", "origin", "requested"],
    );
    let fixture = PoolFixture::from_repo(scratch, repo);
    let objects = || {
        git(
            &fixture.repo,
            &[
                "cat-file",
                "--batch-all-objects",
                "--batch-check=%(objectname)",
            ],
        )
    };
    let before = objects();
    assert!(before.lines().any(|oid| oid == commit));
    assert!(!before.lines().any(|oid| oid == blob));
    let (out, initialized) = fixture.init();
    assert!(out.status.success(), "{initialized}");
    let pool = initialized["pool_id"].as_str().unwrap();
    let checkout = PathBuf::from(initialized["slots"][0]["path"].as_str().unwrap());
    let key = fixture._repo_dir.path.join("missing-blob-key");
    private_file(&key);
    assert!(
        key_command(&fixture, PRIVATE_KEY_COMMAND, &key, "3")
            .status
            .success()
    );
    let out = run_packaged_take(&fixture, pool, &commit, &key, "take");
    assert_eq!(out.status.code(), Some(1));
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(result["kind"], "refused");
    assert_eq!(objects(), before);
    assert!(!checkout.exists());
    assert_eq!(
        fixture.record()["pools"][0]["slots"][0]["phase"]["attempt"]["stage"],
        "permit_issued"
    );
}

#[test]
fn unavailable_protocol_has_no_state_effects() {
    let fixture = PoolFixture::new();
    let absent = fixture._repo_dir.path.join("must-stay-absent");
    let out = fixture
        .hive
        .command(&[
            "slot",
            "recover",
            "--repo",
            fixture.repo.to_str().unwrap(),
            "--pool",
            &"1".repeat(32),
            "--slot",
            "1",
            "--generation",
            "1",
            "--op",
            &"2".repeat(32),
            "--json",
        ])
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

fn key_command(fixture: &PoolFixture, script: &str, path: &Path, fd: &str) -> Output {
    fixture
        .hive
        .command_with(
            Path::new("/bin/sh"),
            &["-c", script, "guide", env!("CARGO_BIN_EXE_cargo-hive")],
        )
        .arg(path)
        .arg(fd)
        .env("HIVE_HOME", &fixture.hive.home.path)
        .output()
        .unwrap()
}

const PRIVATE_KEY_COMMAND: &str = r#"exec 3<>"$2"; exec "$1" slot key --out-fd "$3" --json"#;

fn private_file(path: &Path) {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
}

#[test]
fn key_writes_only_to_a_fresh_private_descriptor() {
    let fixture = PoolFixture::new();
    let path = fixture._repo_dir.path.join("private-key");
    private_file(&path);
    let out = key_command(&fixture, PRIVATE_KEY_COMMAND, &path, "3");
    let result: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(out.status.success(), "{result}");
    assert_eq!(result["kind"], "key_generated");
    let key = fs::read_to_string(&path).unwrap();
    let raw = key.strip_suffix('\n').unwrap();
    assert_eq!(raw.len(), 64);
    assert!(
        raw.bytes()
            .all(|ch| ch.is_ascii_digit() || (b'a'..=b'f').contains(&ch))
    );
    assert!(!String::from_utf8_lossy(&out.stdout).contains(raw));
    assert!(out.stderr.is_empty());
    let other_path = fixture._repo_dir.path.join("other-private-key");
    private_file(&other_path);
    assert!(
        key_command(&fixture, PRIVATE_KEY_COMMAND, &other_path, "3")
            .status
            .success()
    );
    assert!(fs::read_to_string(other_path).unwrap() != key);
    assert!(!fixture.file().exists());

    let out = key_command(&fixture, PRIVATE_KEY_COMMAND, &path, "3");
    assert_eq!(out.status.code(), Some(1));
    assert!(fs::read_to_string(&path).unwrap() == key);
    assert!(!String::from_utf8_lossy(&out.stdout).contains(raw));
    assert!(!String::from_utf8_lossy(&out.stderr).contains(raw));
}

#[test]
fn key_refuses_stdio_aliases_and_unsafe_files() {
    let fixture = PoolFixture::new();
    let path = fixture._repo_dir.path.join("wrong-key-file");
    private_file(&path);
    for fd in ["0", "1", "2", "9"] {
        let out = key_command(&fixture, PRIVATE_KEY_COMMAND, &path, fd);
        assert_eq!(out.status.code(), Some(1));
        let result: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(result["kind"], "refused");
        assert_eq!(fs::metadata(&path).unwrap().len(), 0);
    }

    let out = key_command(
        &fixture,
        r#"exec 3<>"$2"; exec "$1" slot key --out-fd "$3" --json >"$2""#,
        &path,
        "3",
    );
    assert_eq!(out.status.code(), Some(1));
    let result: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(result["kind"], "refused");
    assert_eq!(result.as_object().unwrap().len(), 5);
    fs::write(&path, "").unwrap();

    let out = key_command(
        &fixture,
        r#"exec 3<>"$2"; exec "$1" slot key --out-fd "$3" --json 2>&3"#,
        &path,
        "3",
    );
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(fs::metadata(&path).unwrap().len(), 0);

    for mode in [0o644, 0o400] {
        fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
        let out = key_command(
            &fixture,
            r#"exec 3<"$2"; exec "$1" slot key --out-fd "$3" --json"#,
            &path,
            "3",
        );
        assert_eq!(out.status.code(), Some(1));
        assert_eq!(fs::metadata(&path).unwrap().len(), 0);
    }

    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    chmod_acl(&["+a", "everyone allow read"], &path);
    let out = key_command(&fixture, PRIVATE_KEY_COMMAND, &path, "3");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(fs::metadata(&path).unwrap().len(), 0);
    chmod_acl(&["-N"], &path);

    let link = fixture._repo_dir.path.join("linked-key-file");
    fs::hard_link(&path, link).unwrap();
    let out = key_command(&fixture, PRIVATE_KEY_COMMAND, &path, "3");
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(fs::metadata(path).unwrap().len(), 0);
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
fn cancelling_the_git_probe_in_existing_pool_status_exits_130() {
    let fixture = PoolFixture::new();
    let (out, initialized) = fixture.init();
    assert!(out.status.success(), "{initialized}");
    let saved = fs::read(fixture.file()).unwrap();
    let shim = fixture._repo_dir.path.join("git");
    let probe = fixture._repo_dir.path.join("cancelled-probe");
    fs::write(
        &shim,
        r#"#!/bin/sh
printf '%s\n' "$*" \
    "GIT_DIR=${GIT_DIR-unset}" \
    "GIT_NO_LAZY_FETCH=${GIT_NO_LAZY_FETCH-unset}" \
    "GIT_ALLOW_PROTOCOL=${GIT_ALLOW_PROTOCOL-unset}" \
    "CARGO_TARGET_DIR=${CARGO_TARGET_DIR-unset}" \
    "CARGO_BUILD_TARGET_DIR=${CARGO_BUILD_TARGET_DIR-unset}" \
    "CARGO_BUILD_BUILD_DIR=${CARGO_BUILD_BUILD_DIR-unset}" > "$SLOT_TEST_GIT_PROBE"
kill -INT "$PPID"
exec /bin/sleep 8
"#,
    )
    .unwrap();
    fs::set_permissions(shim, fs::Permissions::from_mode(0o700)).unwrap();
    let path = std::env::join_paths(
        std::iter::once(fixture._repo_dir.path.clone())
            .chain(std::env::split_paths(&std::env::var_os("PATH").unwrap())),
    )
    .unwrap();
    let child = fixture
        .hive
        .command(&[
            "slot",
            "status",
            "--repo",
            fixture.repo.to_str().unwrap(),
            "--json",
        ])
        .env("PATH", path)
        .env("SLOT_TEST_GIT_PROBE", &probe)
        .env("GIT_DIR", fixture.repo.join("caller-git"))
        .env("GIT_NO_LAZY_FETCH", "0")
        .env("GIT_ALLOW_PROTOCOL", "file")
        .env("CARGO_TARGET_DIR", fixture.repo.join("caller-target"))
        .env(
            "CARGO_BUILD_TARGET_DIR",
            fixture.repo.join("caller-build-target"),
        )
        .env("CARGO_BUILD_BUILD_DIR", fixture.repo.join("caller-build"))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let out = common::wait_within(child, std::time::Duration::from_secs(5));
    let recorded = fs::read_to_string(probe).unwrap();
    let mut lines = recorded.lines();
    assert!(lines.next().unwrap().contains("rev-parse"));
    assert_eq!(
        lines.collect::<Vec<_>>(),
        [
            "GIT_DIR=unset",
            "GIT_NO_LAZY_FETCH=1",
            "GIT_ALLOW_PROTOCOL=",
            "CARGO_TARGET_DIR=unset",
            "CARGO_BUILD_TARGET_DIR=unset",
            "CARGO_BUILD_BUILD_DIR=unset",
        ]
    );
    let result: Value =
        serde_json::from_slice(&out.stdout).unwrap_or_else(|err| panic!("{err}: {out:?}"));
    assert_eq!(out.status.code(), Some(130), "{result}");
    assert_eq!(result["kind"], "cancelled");
    assert_eq!(result["schema"], 1);
    assert_eq!(fs::read(fixture.file()).unwrap(), saved);

    fs::rename(fixture.repo.join(".git"), fixture.repo.join("old-git")).unwrap();
    let (out, status) = fixture.status();
    assert!(out.status.success(), "{status}");
    assert_eq!(status["slots"][0]["phase"], "held");
    assert_eq!(status["slots"][1]["phase"], "held");
    assert_eq!(fs::read(fixture.file()).unwrap(), saved);
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
fn a_pending_producer_requires_a_ready_lease() {
    let fixture = PoolFixture::new();
    let (out, initialized) = fixture.init();
    assert!(out.status.success(), "{initialized}");
    let path = PathBuf::from(initialized["slots"][0]["path"].as_str().unwrap());
    git(
        &fixture.repo,
        &[
            "worktree",
            "add",
            "--detach",
            path.to_str().unwrap(),
            "HEAD",
        ],
    );
    let head = git(&path, &["rev-parse", "HEAD"]);
    let admin = PathBuf::from(git(&path, &["rev-parse", "--absolute-git-dir"]));
    let directory = fs::metadata(&path).unwrap();
    let admin_metadata = fs::metadata(&admin).unwrap();
    let checkout = json!({
        "directory": {"dev": directory.dev(), "ino": directory.ino()},
        "admin": {"path": admin, "dev": admin_metadata.dev(), "ino": admin_metadata.ino()}
    });
    let roots = json!({
        "target": {"kind": "absent", "path": path.join("target")},
        "build": {"kind": "absent", "path": path.join("target")}
    });
    let epoch = json!({"pool_id": initialized["pool_id"], "slot": 1, "checkout": checkout});
    let mut record = fixture.record();
    let slot = &mut record["pools"][0]["slots"][0];
    slot["generation"] = json!(1);
    slot["checkout_id"] = checkout;
    slot["last_head"] = json!(head);
    slot["producer"] = json!({
        "kind": "pending", "epoch": epoch, "generation": 1,
        "profile": {"id": "fixture", "revision": 1},
        "invocation": {
            "invocation": "check", "graph_digest": "a".repeat(64),
            "environment_digest": "b".repeat(64)
        },
        "roots": roots, "operation": "1".repeat(32),
        "runner": {
            "boot_session": "boot", "pid": 1, "start_secs": 1,
            "start_micros": 0, "nonce": "c".repeat(64)
        },
        "prior": {"kind": "never_built", "evidence": {"epoch": epoch, "roots": roots}}
    });
    let lease = json!({
        "requested_commit": head, "credential": "d".repeat(64), "reservation_op": "2".repeat(32)
    });
    let ready = json!({"kind": "leased", "lease": lease, "ready_commit": head});
    for (phase, projected) in [
        (ready.clone(), "held"),
        (
            json!({"kind": "held", "reason": "interrupted", "prior": ready}),
            "held",
        ),
    ] {
        record["pools"][0]["slots"][0]["phase"] = phase;
        fs::write(fixture.file(), serde_json::to_vec(&record).unwrap()).unwrap();
        let (out, result) = fixture.status();
        assert!(out.status.success(), "{result}");
        assert_eq!(result["slots"][0]["phase"], projected);
    }

    for phase in [
        json!({"kind": "vacant"}),
        json!({"kind": "reserved", "lease": lease}),
        json!({"kind": "held", "reason": "interrupted", "prior": {"kind": "vacant"}}),
    ] {
        record["pools"][0]["slots"][0]["phase"] = phase;
        let bytes = serde_json::to_vec(&record).unwrap();
        fs::write(fixture.file(), &bytes).unwrap();
        let (out, result) = fixture.status();
        assert_eq!(out.status.code(), Some(1), "{result}");
        assert!(
            result
                .to_string()
                .contains("pending producer requires a ready lease"),
            "{result}"
        );
        assert_eq!(fs::read(fixture.file()).unwrap(), bytes);
    }
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
