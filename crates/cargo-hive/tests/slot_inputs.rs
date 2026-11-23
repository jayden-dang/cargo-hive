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
        let mut fresh = BTreeMap::new();
        let mut executable = None;
        for line in String::from_utf8(output.stdout).unwrap().lines() {
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
                self.checkout.join(source)
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

    fn touch_consumer(&self) {
        assert_eq!(
            git(
                &self.checkout,
                &["ls-files", "--error-unmatch", "--", ANCHOR]
            ),
            ANCHOR
        );
        let anchor = self.checkout.join(ANCHOR);
        let old_mtime = fs::metadata(&anchor).unwrap().modified().unwrap();
        let profile = self.retained.join("debug");
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
    fixture.touch_consumer();
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
    fixture.touch_consumer();
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
