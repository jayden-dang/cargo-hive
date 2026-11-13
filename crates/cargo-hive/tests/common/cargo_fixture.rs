//! Fixtures that run real `cargo` offline in a scratch workspace: builds into a scratch target dir
//! with a scratch `CARGO_HOME`, backdating of what a build wrote, and reading a target dir to
//! decide liveness. Shared by the liveness tests; `cargo_in` kills a cargo that outlives its deadline.

use super::{Scratch, prove, wait_within};
use cargo_hive::cargo_target::TargetDir;
use cargo_hive::gc::liveness::liveness;
use cargo_hive::liveness::{DeadUnit, Liveness, UnitRef};
use cargo_hive::platform;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

pub const WINDOW: Duration = Duration::from_secs(6 * 3600);
pub const THIRTY_DAYS: Duration = Duration::from_secs(30 * 86_400);
pub const CARGO_DEADLINE: Duration = Duration::from_secs(300);

pub fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

pub fn manifest(name: &str, extra: &str) -> String {
    format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n{extra}")
}

pub struct Fixture {
    pub _scratch: Scratch,
    pub app: PathBuf,
    pub target: PathBuf,
    pub logs: PathBuf,
}

pub fn fixture(label: &str) -> Fixture {
    let scratch = Scratch::new(label);
    let root = scratch.path.clone();
    write(
        &root.join("app/Cargo.toml"),
        &manifest(
            "app",
            "[features]\nf = [\"dep/f\"]\n\n[dependencies]\ndep = { path = \"../dep\" }\npm = { path = \"../pm\" }\n",
        ),
    );
    write(
        &root.join("app/build.rs"),
        "fn main() { println!(\"cargo:rerun-if-changed=build.rs\"); }\n",
    );
    write(
        &root.join("app/src/main.rs"),
        "pm::m!();\nfn main() { println!(\"{}\", dep::answer()); }\n",
    );
    write(
        &root.join("app/examples/ex.rs"),
        "fn main() { println!(\"{}\", dep::answer()); }\n",
    );
    write(
        &root.join("dep/Cargo.toml"),
        &manifest("dep", "[features]\nf = []\n"),
    );
    write(
        &root.join("dep/src/lib.rs"),
        "pub fn answer() -> u32 { 42 }\n",
    );
    write(
        &root.join("pm/Cargo.toml"),
        &manifest("pm", "[lib]\nproc-macro = true\n"),
    );
    write(
        &root.join("pm/src/lib.rs"),
        "extern crate proc_macro;\nuse proc_macro::TokenStream;\n#[proc_macro]\npub fn m(_: TokenStream) -> TokenStream { TokenStream::new() }\n",
    );
    let logs = root.join("logs");
    fs::create_dir_all(&logs).unwrap();
    Fixture {
        app: root.join("app"),
        target: root.join("target"),
        logs,
        _scratch: scratch,
    }
}

pub fn cargo(fx: &Fixture, args: &[&str]) {
    assert!(
        cargo_in(&fx.app, &fx.target, &fx.logs, args),
        "cargo {args:?} failed: {}",
        fs::read_to_string(fx.logs.join("stderr")).unwrap()
    );
}

/// `cargo <args> --offline` in `dir` with a scratch CARGO_HOME under `logs`, building into
/// `target`; true when cargo succeeds. Its stdout and stderr land in `logs`.
pub fn cargo_in(dir: &Path, target: &Path, logs: &Path, args: &[&str]) -> bool {
    let cargo_home = logs.join("cargo-home");
    fs::create_dir_all(&cargo_home).unwrap();
    let stdout = fs::File::create(logs.join("stdout")).unwrap();
    let stderr = fs::File::create(logs.join("stderr")).unwrap();
    let child = Command::new("cargo")
        .args(args)
        .arg("--offline")
        .current_dir(dir)
        .env("CARGO_HOME", &cargo_home)
        .env("CARGO_TARGET_DIR", target)
        .env_remove("CARGO_BUILD_TARGET_DIR")
        .env_remove("CARGO_BUILD_BUILD_DIR")
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .unwrap();
    wait_within(child, CARGO_DEADLINE).status.success()
}

pub fn host_triple() -> String {
    let out = Command::new("rustc").arg("-vV").output().unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .find_map(|l| l.strip_prefix("host: "))
        .unwrap()
        .to_string()
}

pub fn profile_dirs(target: &Path) -> Vec<PathBuf> {
    prove::profile_dirs_of(target).expect("a cargo-tagged target dir with readable profiles")
}

pub fn set_mtime(path: &Path, time: SystemTime) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(time)
        .unwrap();
}

pub fn backdate_all(target: &Path, age: Duration) {
    let old = SystemTime::now() - age;
    for profile in profile_dirs(target) {
        for sub in [".fingerprint", "build"] {
            let Ok(entries) = fs::read_dir(profile.join(sub)) else {
                continue;
            };
            for entry in entries {
                let stamp = entry.unwrap().path().join("invoked.timestamp");
                if stamp.exists() {
                    set_mtime(&stamp, old);
                }
            }
        }
    }
}

pub fn read_target(path: &Path) -> TargetDir {
    prove::read_target(path).expect("a cargo-tagged target dir")
}

pub fn decide(target: &Path) -> (BTreeSet<UnitRef>, Vec<DeadUnit>) {
    match decide_at(target, SystemTime::now(), WINDOW) {
        Liveness::Decided { live, dead } => (live.into_iter().collect(), dead),
        Liveness::Undecided { reasons } => panic!("liveness was undecided: {reasons:#?}"),
    }
}

pub fn decide_at(target: &Path, now: SystemTime, window: Duration) -> Liveness {
    let fs = platform::current().unwrap().fs;
    liveness(fs.as_ref(), &read_target(target), now, window)
}

pub fn kinds(target: &Path) -> BTreeMap<UnitRef, (String, String)> {
    let mut out = BTreeMap::new();
    for profile in read_target(target).profiles {
        for unit in profile.graph.expect("a supported profile").units {
            out.insert(
                UnitRef {
                    profile: profile.path.clone(),
                    dir_name: unit.dir_name.clone(),
                },
                (unit.kind.clone(), unit.stem.clone()),
            );
        }
    }
    out
}

pub fn find(
    kinds: &BTreeMap<UnitRef, (String, String)>,
    kind: &str,
    stem: &str,
    triple: Option<&str>,
) -> Vec<UnitRef> {
    kinds
        .iter()
        .filter(|(unit, (k, s))| {
            k == kind
                && s.ends_with(stem)
                && triple.is_none_or(|t| unit.profile.to_string_lossy().contains(t))
        })
        .map(|(unit, _)| unit.clone())
        .collect()
}

pub fn only(units: Vec<UnitRef>) -> UnitRef {
    assert_eq!(units.len(), 1, "{units:?}");
    units.into_iter().next().unwrap()
}

pub fn timestamp_of(unit: &UnitRef, kind: &str) -> PathBuf {
    cargo_hive::liveness::unit_timestamp(&unit.profile, &unit.dir_name, kind)
}
