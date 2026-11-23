//! Native Cargo causal controls for retained checkout inputs.

#![cfg(target_os = "macos")]

#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod common;

use cargo_hive_core::{fingerprint, unit_path};
use common::cargo_fixture::{layout_of, set_mtime};
use common::{Hive, Scratch, git, wait_within};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant, SystemTime};

const ANCHOR: &str = "consumer/src/main.rs";
const QUERY: &str = ".sqlx/query-fixture.json";
const ORIGINAL_QUERY: &str = r#"{"value":"original"}"#;
const CHANGED_QUERY: &str = r#"{"value":"modified"}"#;
const ORIGINAL: &str = "original:absent";
const CONSUMER: &str = "fn main() { println!(\"{}\", probe::observed!()); }\n";

const PROBE: &str = r#"
use proc_macro::TokenStream;

#[proc_macro]
pub fn observed(_: TokenStream) -> TokenStream {
    let manifest = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let workspace = manifest.parent().unwrap();
    let json = std::fs::read_to_string(workspace.join(".sqlx/query-fixture.json")).unwrap();
    let query = json.strip_prefix("{\"value\":\"").unwrap().strip_suffix("\"}").unwrap();
    let dotenv = match std::fs::read_to_string(workspace.join(".env")) {
        Ok(text) => text.strip_prefix("FIXTURE_VALUE=").unwrap().trim_end().to_owned(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => "absent".into(),
        Err(err) => panic!("could not read the fixture .env: {err}"),
    };
    format!("{:?}", format!("{query}:{dotenv}")).parse().unwrap()
}
"#;

const LOCK: &str = r#"version = 4

[[package]]
name = "consumer"
version = "0.0.0"
dependencies = ["probe"]

[[package]]
name = "independent"
version = "0.0.0"

[[package]]
name = "probe"
version = "0.0.0"
"#;

struct Fixture {
    _scratch: Scratch,
    hive: Hive,
    main: PathBuf,
    checkout: PathBuf,
    retained: PathBuf,
    clean: PathBuf,
}

impl Fixture {
    fn new(label: &str) -> Self {
        let scratch = Scratch::new(label);
        let hive = Hive::new();
        let main = scratch.path.join("main");
        fs::create_dir(&main).unwrap();
        git(&main, &["init", "-q"]);
        for (file, text) in [
            (
                "Cargo.toml",
                "[workspace]\nresolver = \"3\"\nmembers = [\"probe\", \"consumer\", \"independent\"]\n",
            ),
            ("Cargo.lock", LOCK),
            (".gitignore", "/target/\n/.env\n"),
            (QUERY, ORIGINAL_QUERY),
            (
                "probe/Cargo.toml",
                "[package]\nname = \"probe\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[lib]\nproc-macro = true\n",
            ),
            ("probe/src/lib.rs", PROBE),
            (
                "consumer/Cargo.toml",
                "[package]\nname = \"consumer\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[dependencies]\nprobe = { path = \"../probe\" }\n",
            ),
            (ANCHOR, CONSUMER),
            (
                "independent/Cargo.toml",
                "[package]\nname = \"independent\"\nversion = \"0.0.0\"\nedition = \"2024\"\n",
            ),
            (
                "independent/src/lib.rs",
                "pub fn independent() -> u8 { 7 }\n",
            ),
        ] {
            let path = main.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        git(&main, &["add", "-A"]);
        git(
            &main,
            &["commit", "-qm", "Establish reviewed fixture inputs"],
        );
        let checkout = scratch.path.join("checkout");
        git(
            &main,
            &[
                "worktree",
                "add",
                "--detach",
                checkout.to_str().unwrap(),
                "HEAD",
            ],
        );
        let checkout = checkout.canonicalize().unwrap();
        let retained = checkout.join("target");
        let clean = scratch.path.join("clean-target");
        Self {
            _scratch: scratch,
            hive,
            main,
            checkout,
            retained,
            clean,
        }
    }

    fn build(&self, target: &Path) -> Build {
        let child = self
            .hive
            .command_with(
                Path::new("cargo"),
                &[
                    "build",
                    "--workspace",
                    "--locked",
                    "--offline",
                    "--message-format=json",
                ],
            )
            .args(["--target-dir", target.to_str().unwrap()])
            .current_dir(&self.checkout)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let output = wait_within(child, Duration::from_secs(60));
        assert!(output.status.success(), "{output:?}");
        let messages = String::from_utf8(output.stdout).unwrap();
        self.observe_build(&self.checkout, target, &messages)
    }

    fn observe_build(&self, checkout: &Path, target: &Path, messages: &str) -> Build {
        let mut fresh = BTreeMap::new();
        let mut executable = None;
        for line in messages.lines().filter(|line| line.starts_with('{')) {
            let row: Value = serde_json::from_str(line).unwrap();
            if row["reason"] != "compiler-artifact" {
                continue;
            }
            let name = row["target"]["name"].as_str().unwrap();
            let source = match name {
                "consumer" => ANCHOR,
                "probe" => "probe/src/lib.rs",
                "independent" => "independent/src/lib.rs",
                _ => panic!("unexpected Cargo target {name}"),
            };
            assert_eq!(
                Path::new(row["target"]["src_path"].as_str().unwrap()),
                checkout.join(source)
            );
            assert!(
                fresh
                    .insert(name.to_owned(), row["fresh"].as_bool().unwrap())
                    .is_none()
            );
            if name == "consumer" {
                let path = PathBuf::from(row["executable"].as_str().unwrap());
                assert!(path.starts_with(target), "{path:?} escaped {target:?}");
                executable = Some(path);
            }
        }
        assert_eq!(fresh.len(), 3, "{fresh:?}");
        let executable = executable.expect("Cargo reported the fixture consumer executable");
        let result = self.hive.command_with(&executable, &[]).output().unwrap();
        assert!(result.status.success(), "{result:?}");
        let layout = layout_of(target);
        let toolchain = std::env::var("RUSTUP_TOOLCHAIN").unwrap_or_default();
        match toolchain.split('-').next() {
            Some("1.97.1") => assert_eq!(layout, unit_path::UnitLayout::Flat),
            Some("1.100.0") => assert_eq!(layout, unit_path::UnitLayout::UnitDir),
            _ => (),
        }
        Build {
            value: String::from_utf8(result.stdout)
                .unwrap()
                .trim_end()
                .to_owned(),
            fresh,
            layout,
        }
    }

    fn touch_consumer(checkout: &Path, target: &Path) {
        assert_eq!(
            git(checkout, &["ls-files", "--error-unmatch", "--", ANCHOR]),
            ANCHOR
        );
        let anchor = checkout.join(ANCHOR);
        let old_mtime = fs::metadata(&anchor).unwrap().modified().unwrap();
        let profile = target.join("debug");
        let report = fingerprint::read_profile(&profile);
        let units = report.graph.unwrap().units;
        let mut consumers = units.iter().filter(|unit| unit.loc.package() == "consumer");
        let consumer = consumers.next().unwrap();
        assert!(consumers.next().is_none());
        let stamp = unit_path::stamp(&profile, &consumer.loc, consumer.kind);
        let build_mtime = fs::metadata(stamp).unwrap().modified().unwrap();
        let before = old_mtime.max(build_mtime);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            set_mtime(&anchor, SystemTime::now());
            if fs::metadata(&anchor).unwrap().modified().unwrap() > before {
                break;
            }
            assert!(Instant::now() < deadline, "consumer mtime did not advance");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(fs::read_to_string(anchor).unwrap(), CONSUMER);
    }

    fn assert_clean(&self) {
        assert!(
            git(
                &self.checkout,
                &["status", "--porcelain", "--untracked-files=all"]
            )
            .is_empty()
        );
    }
}

#[derive(Debug)]
struct Build {
    value: String,
    fresh: BTreeMap<String, bool>,
    layout: unit_path::UnitLayout,
}

impl Build {
    fn check(&self, step: &str, expected: &str, consumer: bool, others: bool) {
        println!(
            "{step} {:?} value={:?} fresh={:?}",
            self.layout, self.value, self.fresh
        );
        assert_eq!(self.value, expected, "{step}: {self:?}");
        assert_eq!(self.fresh["consumer"], consumer, "{step}: {self:?}");
        for name in ["probe", "independent"] {
            assert_eq!(self.fresh[name], others, "{step}: {self:?}");
        }
    }
}

#[derive(Clone, Copy)]
enum ChangedInput {
    Sqlx,
    Dotenv,
}

fn causal_control(input: ChangedInput) {
    let (label, changed) = match input {
        ChangedInput::Sqlx => ("retained-sqlx", "modified:absent"),
        ChangedInput::Dotenv => ("retained-dotenv", "original:present"),
    };
    let fixture = Fixture::new(label);
    let query = fixture.checkout.join(QUERY);
    let dotenv = fixture.checkout.join(".env");
    let original_mtime = fs::metadata(&query).unwrap().modified().unwrap();
    assert!(!dotenv.exists());
    fixture
        .build(&fixture.retained)
        .check("baseline", ORIGINAL, false, false);

    match input {
        ChangedInput::Sqlx => fs::write(&query, CHANGED_QUERY).unwrap(),
        ChangedInput::Dotenv => fs::write(&dotenv, "FIXTURE_VALUE=present\n").unwrap(),
    }
    Fixture::touch_consumer(&fixture.checkout, &fixture.retained);
    fixture
        .build(&fixture.retained)
        .check("holder's changed input", changed, false, true);

    match input {
        ChangedInput::Sqlx => {
            fs::write(&query, ORIGINAL_QUERY).unwrap();
            set_mtime(&query, original_mtime);
        }
        ChangedInput::Dotenv => fs::remove_file(&dotenv).unwrap(),
    }
    assert_eq!(fs::read_to_string(query).unwrap(), ORIGINAL_QUERY);
    assert_eq!(
        fs::metadata(fixture.checkout.join(QUERY))
            .unwrap()
            .modified()
            .unwrap(),
        original_mtime
    );
    assert_eq!(
        fs::symlink_metadata(dotenv).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );
    fixture.assert_clean();

    let stale = fixture.build(&fixture.retained);
    stale.check("unprotected warm", changed, true, true);

    assert!(!fixture.clean.exists());
    let reference = fixture.build(&fixture.clean);
    reference.check("independent clean", ORIGINAL, false, false);
    assert_ne!(stale.value, reference.value);

    // This deliberately supplies the anchor touch without calling the slot guide.
    Fixture::touch_consumer(&fixture.checkout, &fixture.retained);
    fixture
        .build(&fixture.retained)
        .check("manual anchor control", &reference.value, false, true);
    fixture
        .build(&fixture.retained)
        .check("warm after the control", &reference.value, true, true);
    fixture.assert_clean();
}

#[test]
fn restored_sqlx_at_its_original_mtime_needs_the_consumer_anchor() {
    causal_control(ChangedInput::Sqlx);
}

#[test]
fn removing_a_previously_absent_dotenv_needs_the_consumer_anchor() {
    causal_control(ChangedInput::Dotenv);
}

#[test]
fn a_tracked_next_commit_reuses_an_independent_unit_at_the_same_checkout_path() {
    let fixture = Fixture::new("retained-next-commit");
    fixture
        .build(&fixture.retained)
        .check("prior commit", ORIGINAL, false, false);
    let current = git(&fixture.checkout, &["rev-parse", "HEAD"]);
    let before = fs::metadata(&fixture.retained).unwrap();
    fs::write(
        fixture.main.join(ANCHOR),
        "fn main() { println!(\"next:{}\", probe::observed!()); }\n",
    )
    .unwrap();
    git(
        &fixture.main,
        &["commit", "-qm", "Edit only the consumer", "--", ANCHOR],
    );
    let next = git(&fixture.main, &["rev-parse", "HEAD"]);
    assert_ne!(current, next);
    assert_eq!(
        git(&fixture.main, &["diff", "--name-only", &current, &next]),
        ANCHOR
    );
    git(&fixture.checkout, &["switch", "--detach", &next]);
    assert_eq!(git(&fixture.checkout, &["rev-parse", "HEAD"]), next);
    assert_eq!(
        Path::new(&git(&fixture.checkout, &["rev-parse", "--show-toplevel"])),
        fixture.checkout
    );
    fixture.assert_clean();
    fixture
        .build(&fixture.retained)
        .check("next commit warm", "next:original:absent", false, true);
    let after = fs::metadata(&fixture.retained).unwrap();
    assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
}

fn digest_file(path: &Path) -> String {
    let mut file = fs::File::open(path).unwrap();
    let size = file.metadata().unwrap().len();
    cargo_hive_platform::slot::sha256_reader(&mut file, size, || false)
        .unwrap()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn pinned_tool(fixture: &Fixture, args: &[&str]) -> PathBuf {
    let out = fixture
        .hive
        .command_with(Path::new("/usr/bin/xcrun"), args)
        .env_clear()
        .env("HOME", &fixture.hive.home.path)
        .env("TMPDIR", &fixture._scratch.path)
        .env("PATH", "/usr/bin:/bin")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    PathBuf::from(String::from_utf8(out.stdout).unwrap().trim())
        .canonicalize()
        .unwrap()
}

fn start_packaged(fixture: &Fixture, args: &[&str], input: &str) -> std::process::Child {
    use std::io::Write as _;
    let script = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../plugin/skills/hive/scripts/slot-handoff.sh");
    let child_args = [
        script.to_str().unwrap(),
        args[0],
        env!("CARGO_BIN_EXE_cargo-hive"),
    ];
    let mut command = fixture
        .hive
        .command_with(Path::new("/bin/bash"), &child_args);
    command
        .args(&args[1..])
        .env("SHELLOPTS", "allexport:xtrace")
        .env("credential", "exported-credential-decoy")
        .env("nonce", "exported-nonce-decoy")
        .env("producer_request", "exported-request-decoy")
        .env("CARGO_NET_OFFLINE", "true")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (name, _) in std::env::vars_os() {
        let text = name.to_string_lossy();
        if (text.starts_with("CARGO_") || text.starts_with("RUST"))
            && !matches!(
                text.as_ref(),
                "CARGO_HOME"
                    | "CARGO_NET_OFFLINE"
                    | "RUSTUP_HOME"
                    | "RUSTUP_TOOLCHAIN"
                    | "RUSTUP_AUTO_INSTALL"
            )
            || text.starts_with("DYLD_")
        {
            command.env_remove(name);
        }
    }
    let mut child = command.spawn().unwrap();
    writeln!(child.stdin.take().unwrap(), "{input}").unwrap();
    child
}

fn finish_packaged(child: std::process::Child, input: &str) -> std::process::Output {
    let out = wait_within(child, Duration::from_secs(60));
    for stream in [&out.stdout, &out.stderr] {
        assert!(
            !String::from_utf8_lossy(stream).contains(input),
            "guide disclosed private input"
        );
    }
    out
}

fn packaged(fixture: &Fixture, args: &[&str], input: &str) -> std::process::Output {
    finish_packaged(start_packaged(fixture, args, input), input)
}

fn live_writer(process: &Value) -> bool {
    let (Some(pid), Some(seconds), Some(micros)) = (
        process["pid"].as_u64(),
        process["start_secs"].as_u64(),
        process["start_micros"].as_u64(),
    ) else {
        return false;
    };
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let size = size_of::<libc::proc_bsdinfo>() as i32;
    // SAFETY: PROC_PIDTBSDINFO writes a proc_bsdinfo into the sized buffer.
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
    // SAFETY: the return count confirms the complete initialized structure.
    let info = unsafe { info.assume_init() };
    info.pbi_pid == pid as u32
        && info.pbi_pgid == pid as u32
        && (info.pbi_start_tvsec, info.pbi_start_tvusec) == (seconds, micros)
        && !matches!(info.pbi_status, libc::SSTOP | libc::SZOMB)
}

fn observe_writer_lock(
    fixture: &Fixture,
    pool: &str,
    target: &Path,
    child: &mut std::process::Child,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(45);
    let slots = fixture.hive.home.path.join("slots.json");
    let lock = fixture
        .hive
        .home
        .path
        .join("slot-locks")
        .join(pool)
        .join("slot-1.lock");
    let fs = cargo_hive_platform::host_fs();
    while Instant::now() < deadline {
        if target.join("debug").is_dir() {
            let record: Value = serde_json::from_slice(&fs::read(&slots).unwrap()).unwrap();
            let producer = &record["pools"][0]["slots"][0]["producer"];
            let process = &producer["foreground"];
            if producer["kind"] == "pending" && live_writer(process) {
                let held = matches!(
                    fs.try_lock_exclusive(&lock),
                    cargo_hive_platform::TryLock::Held
                );
                if live_writer(process) {
                    return held;
                }
            }
        }
        if child.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    false
}

fn fixture_seal(fixture: &Fixture, head: &str, false_driver: bool) -> PathBuf {
    use serde_json::json;
    let seal = fixture._scratch.path.join("reviewed-closure.json");
    let profile_path = fixture._scratch.path.join("reviewed-profile.json");
    let toolchain = std::env::var("RUSTUP_TOOLCHAIN").unwrap();
    let release = toolchain.split('-').next().unwrap();
    let layout = if release == "1.97.1" {
        "flat"
    } else {
        "unit_dir"
    };
    let native = format!("{}-apple-darwin", std::env::consts::ARCH);
    let bin = PathBuf::from(std::env::var_os("RUSTUP_HOME").unwrap())
        .join("toolchains")
        .join(format!("{release}-{native}"))
        .join("bin");
    let clang = pinned_tool(fixture, &["--no-cache", "--find", "clang"]);
    let ld = pinned_tool(fixture, &["--no-cache", "--find", "ld"]);
    let sdk = pinned_tool(fixture, &["--no-cache", "--show-sdk-path"]);
    let tool = |path: &Path| json!({"path": path, "sha256": digest_file(path)});
    let lib = bin.parent().unwrap().join("lib");
    let inventory = |dir: &Path, skip_rustlib: bool| {
        let mut files: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap())
            .filter(|entry| !(skip_rustlib && entry.file_name() == "rustlib"))
            .map(|entry| entry.path())
            .collect();
        files.sort();
        files
            .into_iter()
            .map(|path| {
                let mut material = tool(&path);
                if false_driver
                    && path
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with("librustc_driver-")
                {
                    material["sha256"] = json!("0".repeat(64));
                }
                material
            })
            .collect::<Vec<_>>()
    };
    let data_digest = |bytes: &str| {
        cargo_hive_platform::slot::sha256(bytes.as_bytes())
            .unwrap()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    let closure = json!({
        "schema": 1, "recipe": "native_build", "profile": {"id":"input-control","revision":1},
        "proof":"native-control", "invocation":"build-fixture", "layout":layout, "native_target":native,
        "endpoints":[{"commit":head,"tree":git(&fixture.main,&["rev-parse","HEAD^{tree}"])}],
        "tools":{
            "cargo":tool(&bin.join("cargo")), "rustc":tool(&bin.join("rustc")),
            "rust_runtime":inventory(&lib, true),
            "target_sysroot":inventory(&lib.join("rustlib").join(&native).join("lib"), false),
            "clang":tool(&clang), "ld":tool(&ld), "sdk":sdk,
            "sdk_settings":tool(&sdk.join("SDKSettings.json")),
            "system_library":tool(&sdk.join("usr/lib/libSystem.tbd").canonicalize().unwrap())
        },
        "packages":[
            {"name":"consumer","version":"0.0.0","manifest":"consumer/Cargo.toml","targets":[{"name":"consumer","kind":"bin","source":ANCHOR}],"dependencies":["probe"]},
            {"name":"independent","version":"0.0.0","manifest":"independent/Cargo.toml","targets":[{"name":"independent","kind":"lib","source":"independent/src/lib.rs"}],"dependencies":[]},
            {"name":"probe","version":"0.0.0","manifest":"probe/Cargo.toml","targets":[{"name":"probe","kind":"proc-macro","source":"probe/src/lib.rs"}],"dependencies":[]}
        ],
        "data":[
            {"id":"query","sha256":[data_digest(ORIGINAL_QUERY),data_digest(CHANGED_QUERY)],"absent":false},
            {"id":"dotenv","sha256":[data_digest("FIXTURE_VALUE=present\n")],"absent":true}
        ]
    });
    fs::write(&seal, serde_json::to_vec(&closure).unwrap()).unwrap();
    let mut graph: Vec<Value> = git(&fixture.main, &["ls-files"]).lines()
        .filter(|path| *path != QUERY)
        .map(|path| json!({
            "locator":{"kind":"checkout","path":path},
            "expectation":{"kind":"present","identity":{"kind":"git_object","oid":git(&fixture.main,&["rev-parse",&format!("HEAD:{path}")])}}
        })).collect();
    let seal_digest = digest_file(&seal);
    graph.push(json!({
        "locator":{"kind":"external","path":seal},
        "expectation":{"kind":"present","identity":{"kind":"reviewed_bytes","sha256":seal_digest}}
    }));
    let common = fixture.main.join(".git");
    let stat = fs::metadata(&common).unwrap();
    let anchor = git(&fixture.main, &["rev-parse", &format!("HEAD:{ANCHOR}")]);
    let profile = json!({
        "schema":1,"id":"input-control","revision":1,
        "common_git_id":{"path":common,"dev":stat.dev(),"ino":stat.ino()},
        "graph_inputs":graph, "invocations":[{
            "id":"build-fixture","cargo":format!("cargo {release}"),"rustc":format!("rustc {release}"),
            "layout":layout,"argv":["build","--workspace","--locked","--offline","--message-format=json"],
            "cwd":".","environment":[
                {"name":"CARGO_HOME","expectation":{"kind":"present","value":fixture.hive.cargo_home.path}},
                {"name":"CARGO_NET_OFFLINE","expectation":{"kind":"present","value":"true"}}
            ],"target_root":"target","build_root":"target"
        }], "mutable_inputs":[
            {"id":"query","locator":{"kind":"checkout","path":QUERY},"presence":"present",
                "invocations":["build-fixture"],"coverage":{"kind":"consumers","ids":["consumer-bin"]}},
            {"id":"dotenv","locator":{"kind":"checkout","path":".env"},"presence":"present_or_absent",
                "invocations":["build-fixture"],"coverage":{"kind":"consumers","ids":["consumer-bin"]}}
        ],"consumers":[{
            "id":"consumer-bin","package":"consumer","role":"bin","invocation":"build-fixture",
            "anchor":ANCHOR,"prior_object":anchor,"requested_object":anchor
        }],"proofs":[{"id":"native-control","invocation":"build-fixture","layout":layout,
            "evidence":format!("closure-v1:sha256={seal_digest}"),"unaffected_unit":"independent"}]
    });
    fs::write(&profile_path, serde_json::to_vec(&profile).unwrap()).unwrap();
    profile_path
}

#[test]
fn packaged_guide_admits_only_the_sealed_native_producer() {
    let fixture = Fixture::new("guided-native-seal");
    let head = git(&fixture.main, &["rev-parse", "HEAD"]);
    let profile = fixture_seal(&fixture, &head, false);
    fixture.hive.config(
        &serde_json::json!({"secondary_roots":[fixture.hive.secondary_root.path]}).to_string(),
    );
    fixture.hive.write_ready_capabilities();
    fs::create_dir(fixture.hive.home.path.join("no-cache")).unwrap();
    common::register(&fixture.hive, &fixture.main);
    let (created, initialized) = fixture.hive.json(&[
        "slot",
        "init",
        "--repo",
        fixture.main.to_str().unwrap(),
        "--root",
        fixture.hive.secondary_root.path.to_str().unwrap(),
        "--profile",
        profile.to_str().unwrap(),
    ]);
    assert!(created.status.success(), "{initialized}");
    let pool = initialized["pool_id"].as_str().unwrap();
    let checkout = PathBuf::from(initialized["slots"][0]["path"].as_str().unwrap());
    let key = "e".repeat(64);
    let first = packaged(
        &fixture,
        &[
            "take",
            fixture.main.to_str().unwrap(),
            pool,
            "1",
            "0",
            &head,
            &"a".repeat(32),
            &"b".repeat(32),
            &"c".repeat(32),
        ],
        &key,
    );
    assert!(first.status.success(), "{first:?}");
    let spawn_producer = |op: &str| {
        start_packaged(
            &fixture,
            &[
                "produce",
                fixture.main.to_str().unwrap(),
                pool,
                "1",
                "1",
                op,
                "build-fixture",
            ],
            &key,
        )
    };
    let produce = |op: &str| finish_packaged(spawn_producer(op), &key);
    let seal = fixture._scratch.path.join("reviewed-closure.json");
    let original = fs::read(&seal).unwrap();
    let mut tampered = original.clone();
    tampered.push(b' ');
    fs::write(&seal, tampered).unwrap();
    let rejected_seal = produce(&"8".repeat(32));
    assert!(!rejected_seal.status.success(), "{rejected_seal:?}");
    assert!(!checkout.join("target").exists());
    fs::write(seal, original).unwrap();
    let rejected = checkout.join(".env");
    fs::write(&rejected, "FIXTURE_VALUE=unreviewed\n").unwrap();
    let refused = produce(&"d".repeat(32));
    assert!(!refused.status.success(), "{refused:?}");
    assert!(!checkout.join("target").exists());
    fs::remove_file(rejected).unwrap();
    let retained = checkout.join("target");
    let run = |op: char| {
        let operation = op.to_string().repeat(32);
        let mut child = spawn_producer(&operation);
        let owns_lock =
            (op == 'f').then(|| observe_writer_lock(&fixture, pool, &retained, &mut child));
        let out = finish_packaged(child, &key);
        assert!(
            out.status.success(),
            "sealed producer did not complete: {out:?}"
        );
        assert!(
            owns_lock != Some(false),
            "the live Cargo writer did not retain its slot lock"
        );
        let receipt: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(receipt["kind"], "producer_complete");
        let record: Value =
            serde_json::from_slice(&fs::read(fixture.hive.home.path.join("slots.json")).unwrap())
                .unwrap();
        let producer = &record["pools"][0]["slots"][0]["producer"];
        assert_eq!(producer["kind"], "complete");
        let invocations = producer["invocations"].as_array().unwrap();
        let latest = invocations.last().unwrap();
        assert_eq!(latest["operation"], operation);
        assert!(
            latest["foreground"]["pid"]
                .as_u64()
                .is_some_and(|pid| pid > 1)
        );
        let evidence = &latest["evidence"];
        assert!(
            evidence["stable_digest"]
                .as_str()
                .is_some_and(|digest| digest.len() == 64)
        );
        let messages = String::from_utf8(out.stderr).unwrap();
        (
            fixture.observe_build(&checkout, &retained, &messages),
            evidence.clone(),
        )
    };
    let (baseline, original) = run('f');
    baseline.check("guided baseline", ORIGINAL, false, false);
    let query = checkout.join(QUERY);
    let original_mtime = fs::metadata(&query).unwrap().modified().unwrap();
    let dotenv = checkout.join(".env");
    assert!(!dotenv.exists());
    let reference = fixture.build(&fixture.clean);
    reference.check(
        "independent clean for guided controls",
        ORIGINAL,
        false,
        false,
    );

    for (input, changed, op) in [
        (ChangedInput::Sqlx, "modified:absent", ['1', '2', '3']),
        (ChangedInput::Dotenv, "original:present", ['4', '5', '6']),
    ] {
        match input {
            ChangedInput::Sqlx => fs::write(&query, CHANGED_QUERY).unwrap(),
            ChangedInput::Dotenv => fs::write(&dotenv, "FIXTURE_VALUE=present\n").unwrap(),
        }
        Fixture::touch_consumer(&checkout, &retained);
        let (build, changed_evidence) = run(op[0]);
        build.check("guided changed input", changed, false, true);
        assert_ne!(changed_evidence["graph_digest"], original["graph_digest"]);
        assert_eq!(changed_evidence["stable_digest"], original["stable_digest"]);
        match input {
            ChangedInput::Sqlx => {
                fs::write(&query, ORIGINAL_QUERY).unwrap();
                set_mtime(&query, original_mtime);
            }
            ChangedInput::Dotenv => fs::remove_file(&dotenv).unwrap(),
        }
        assert_eq!(
            fs::metadata(&query).unwrap().modified().unwrap(),
            original_mtime
        );
        assert!(!dotenv.exists());
        let (stale, restored_evidence) = run(op[1]);
        stale.check("guided unprotected restored input", changed, true, true);
        assert_eq!(restored_evidence, original);
        // The later retained-take test replaces this manual causal control with guide invalidation.
        Fixture::touch_consumer(&checkout, &retained);
        let (protected, final_evidence) = run(op[2]);
        protected.check(
            "guided build after manual anchor control",
            &reference.value,
            false,
            true,
        );
        assert_eq!(final_evidence, original);
    }
    assert!(git(&checkout, &["status", "--porcelain"]).is_empty());
    let op = "9".repeat(32);
    let mut child = fixture
        .hive
        .command(&[
            "slot",
            "return",
            "--repo",
            fixture.main.to_str().unwrap(),
            "--pool",
            pool,
            "--slot",
            "1",
            "--generation",
            "1",
            "--op",
            &op,
            "--json",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write as _;
        writeln!(child.stdin.take().unwrap(), "{key}").unwrap();
    }
    let out = wait_within(child, Duration::from_secs(30));
    assert!(out.status.success(), "{out:?}");
    let returned: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(returned["kind"], "returned");
}

#[test]
fn a_matching_seal_cannot_substitute_unproved_compiler_material() {
    let fixture = Fixture::new("guided-native-invalid-driver");
    let head = git(&fixture.main, &["rev-parse", "HEAD"]);
    let profile = fixture_seal(&fixture, &head, true);
    fixture.hive.config(
        &serde_json::json!({"secondary_roots":[fixture.hive.secondary_root.path]}).to_string(),
    );
    fixture.hive.write_ready_capabilities();
    fs::create_dir(fixture.hive.home.path.join("no-cache")).unwrap();
    common::register(&fixture.hive, &fixture.main);
    let (created, initialized) = fixture.hive.json(&[
        "slot",
        "init",
        "--repo",
        fixture.main.to_str().unwrap(),
        "--root",
        fixture.hive.secondary_root.path.to_str().unwrap(),
        "--profile",
        profile.to_str().unwrap(),
    ]);
    assert!(created.status.success(), "{initialized}");
    let pool = initialized["pool_id"].as_str().unwrap();
    let checkout = PathBuf::from(initialized["slots"][0]["path"].as_str().unwrap());
    let out = packaged(
        &fixture,
        &[
            "take",
            fixture.main.to_str().unwrap(),
            pool,
            "1",
            "0",
            &head,
            &"a".repeat(32),
            &"b".repeat(32),
            &"c".repeat(32),
        ],
        &"f".repeat(64),
    );
    assert!(!out.status.success(), "{out:?}");
    let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(
        reply["detail"]
            .as_str()
            .unwrap()
            .contains("compiler material"),
        "{reply}"
    );
    assert!(!checkout.join("target").exists());
}
