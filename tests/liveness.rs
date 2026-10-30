mod common;
use cargo_hive::cargo_target::{self, Owner, TargetDir};
use cargo_hive::fingerprint;
use cargo_hive::liveness::{liveness, Liveness, UnitRef};
use cargo_hive::platform;
use clap::CommandFactory;
use common::{init_repo, register, wait_within, Hive, Scratch};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

const WINDOW: Duration = Duration::from_secs(6 * 3600);
const THIRTY_DAYS: Duration = Duration::from_secs(30 * 86_400);
const CARGO_DEADLINE: Duration = Duration::from_secs(300);

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn manifest(name: &str, extra: &str) -> String {
    format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n{extra}")
}

struct Fixture {
    _scratch: Scratch,
    app: PathBuf,
    target: PathBuf,
    logs: PathBuf,
}

fn fixture(label: &str) -> Fixture {
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

fn cargo(fx: &Fixture, args: &[&str]) {
    let cargo_home = fx.logs.join("cargo-home");
    fs::create_dir_all(&cargo_home).unwrap();
    let stdout = fs::File::create(fx.logs.join("stdout")).unwrap();
    let stderr = fs::File::create(fx.logs.join("stderr")).unwrap();
    let child = Command::new("cargo")
        .args(args)
        .arg("--offline")
        .current_dir(&fx.app)
        .env("CARGO_HOME", &cargo_home)
        .env("CARGO_TARGET_DIR", &fx.target)
        .env_remove("CARGO_BUILD_TARGET_DIR")
        .env_remove("CARGO_BUILD_BUILD_DIR")
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .unwrap();
    let out = wait_within(child, CARGO_DEADLINE);
    assert!(
        out.status.success(),
        "cargo {args:?} failed: {}",
        fs::read_to_string(fx.logs.join("stderr")).unwrap()
    );
}

fn host_triple() -> String {
    let out = Command::new("rustc").arg("-vV").output().unwrap();
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .find_map(|l| l.strip_prefix("host: "))
        .unwrap()
        .to_string()
}

fn profile_dirs(target: &Path) -> Vec<PathBuf> {
    cargo_target::identify(target)
        .expect("a cargo-tagged target dir")
        .expect("readable profiles")
        .into_iter()
        .map(|(path, _)| path)
        .collect()
}

fn set_mtime(path: &Path, time: SystemTime) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(time)
        .unwrap();
}

fn backdate_all(target: &Path, age: Duration) {
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

fn read_target(path: &Path) -> TargetDir {
    let mut target = TargetDir {
        path: path.to_path_buf(),
        owner: Owner::Unresolved {
            reason: "test".into(),
        },
        profiles: Vec::new(),
        size: None,
        unreadable: None,
    };
    match cargo_target::identify(path).expect("a cargo-tagged target dir") {
        Err(why) => target.unreadable = Some(why),
        Ok(profiles) => {
            target.profiles = profiles
                .iter()
                .map(|(dir, _)| fingerprint::read_profile(dir))
                .collect();
        }
    }
    target
}

fn decide(target: &Path) -> (BTreeSet<UnitRef>, Vec<cargo_hive::liveness::DeadUnit>) {
    let fs = platform::current().unwrap().fs;
    match liveness(fs.as_ref(), &read_target(target), SystemTime::now(), WINDOW) {
        Liveness::Decided { live, dead } => (live.into_iter().collect(), dead),
        Liveness::Undecided { reasons } => panic!("liveness was undecided: {reasons:#?}"),
    }
}

fn kinds(target: &Path) -> BTreeMap<UnitRef, (String, String)> {
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

fn find(
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

fn only(units: Vec<UnitRef>) -> UnitRef {
    assert_eq!(units.len(), 1, "{units:?}");
    units.into_iter().next().unwrap()
}

fn timestamp_of(unit: &UnitRef, kind: &str) -> PathBuf {
    cargo_hive::liveness::unit_timestamp(&unit.profile, &unit.dir_name, kind)
}

#[test]
fn a_proc_macro_in_the_host_profile_stays_live_for_a_target_triple_build() {
    let fx = fixture("liveness-triple");
    let triple = host_triple();
    cargo(&fx, &["build", "--target", &triple]);
    backdate_all(&fx.target, THIRTY_DAYS);
    let all = kinds(&fx.target);
    let bin = only(find(&all, "bin", "app", Some(&triple)));
    set_mtime(&timestamp_of(&bin, "bin"), SystemTime::now());

    let (live, dead) = decide(&fx.target);

    let run = only(find(
        &all,
        "run-build-script",
        "build-script-build",
        Some(&triple),
    ));
    let pm = only(find(&all, "lib", "pm", None));
    let script = only(find(&all, "build-script", "build-script-build", None));
    assert!(!pm.profile.to_string_lossy().contains(&triple));
    for unit in [&bin, &run, &pm, &script] {
        assert!(live.contains(unit), "{unit:?} should be live in {live:#?}");
    }
    assert_eq!(dead, vec![]);
}

#[test]
fn a_backdated_check_variant_is_dead_with_its_rmeta_and_dep_info() {
    let fx = fixture("liveness-check");
    cargo(&fx, &["check"]);
    backdate_all(&fx.target, THIRTY_DAYS);
    cargo(&fx, &["build", "--bins", "--examples"]);

    let (live, dead) = decide(&fx.target);

    let all = kinds(&fx.target);
    let dead_units: BTreeSet<_> = dead.iter().map(|d| d.unit.clone()).collect();
    let dep_units = find(&all, "lib", "dep", None);
    assert_eq!(dep_units.len(), 2, "one check and one build unit of dep");
    let check: Vec<_> = dep_units
        .iter()
        .filter(|u| dead_units.contains(*u))
        .collect();
    assert_eq!(check.len(), 1, "{dead:#?}");
    let check = check[0];
    let hash = fingerprint::unit_hash(&check.dir_name).unwrap();
    let deps_dir = check.profile.join("deps");
    let mut expected: Vec<PathBuf> = vec![check.profile.join(".fingerprint").join(&check.dir_name)];
    let mut owned: Vec<PathBuf> = fs::read_dir(&deps_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            name.contains(&format!("-{hash}"))
        })
        .collect();
    owned.sort();
    assert!(
        owned.iter().all(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            (name.starts_with("lib") && name.ends_with(&format!("-{hash}.rmeta")))
                || name.ends_with(&format!("-{hash}.d"))
        }),
        "a check unit owns only rmeta and dep-info: {owned:?}"
    );
    assert_eq!(owned.len(), 2, "{owned:?}");
    expected.extend(owned);
    let files = &dead.iter().find(|d| &d.unit == check).unwrap().files;
    assert_eq!(files, &expected);
    assert!(!live.contains(check), "the check variant must not be live");
    let build_unit: Vec<_> = dep_units.iter().filter(|u| live.contains(*u)).collect();
    assert_eq!(build_unit.len(), 1);
    for kind in ["build-script", "run-build-script"] {
        for unit in find(&all, kind, "build-script-build", None) {
            assert!(live.contains(&unit), "{kind} stays live: {unit:?}");
            assert!(!dead_units.contains(&unit));
        }
    }
    let pm = only(find(&all, "lib", "pm", None));
    assert!(
        live.contains(&pm),
        "the proc-macro is live through traversal"
    );
}

#[test]
fn a_dead_example_lists_its_examples_entries() {
    let fx = fixture("liveness-example");
    cargo(&fx, &["build", "--examples"]);
    backdate_all(&fx.target, THIRTY_DAYS);
    cargo(&fx, &["build", "--examples", "--features", "f"]);

    let (live, dead) = decide(&fx.target);

    let all = kinds(&fx.target);
    let old_example = only(
        find(&all, "example", "ex", None)
            .into_iter()
            .filter(|u| !live.contains(u))
            .collect(),
    );
    let hash = fingerprint::unit_hash(&old_example.dir_name).unwrap();
    let examples_dir = old_example.profile.join("examples");
    let mut owned: Vec<PathBuf> = fs::read_dir(&examples_dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(&format!("ex-{hash}"))
        })
        .collect();
    owned.sort();
    assert!(!owned.is_empty());
    let mut expected = vec![old_example
        .profile
        .join(".fingerprint")
        .join(&old_example.dir_name)];
    expected.extend(owned);
    let files = &dead
        .iter()
        .find(|d| d.unit == old_example)
        .unwrap_or_else(|| panic!("{old_example:?} should be dead in {dead:#?}"))
        .files;
    assert_eq!(files, &expected);
}

#[test]
fn a_build_script_unit_is_never_dead_and_its_run_timestamp_in_build_is_a_root() {
    let fx = fixture("liveness-script");
    cargo(&fx, &["build"]);
    backdate_all(&fx.target, THIRTY_DAYS);
    let all = kinds(&fx.target);
    let script = only(find(&all, "build-script", "build-script-build", None));
    let run = only(find(&all, "run-build-script", "build-script-build", None));

    let (live, dead) = decide(&fx.target);
    assert!(live.is_empty(), "{live:#?}");
    assert!(!dead.is_empty());
    assert!(dead.iter().all(|d| d.unit != script && d.unit != run));

    set_mtime(&timestamp_of(&run, "run-build-script"), SystemTime::now());
    assert!(run
        .profile
        .join("build")
        .join(&run.dir_name)
        .join("invoked.timestamp")
        .exists());

    let (live, dead) = decide(&fx.target);
    assert_eq!(live, BTreeSet::from([run.clone(), script.clone()]));
    assert!(dead.iter().all(|d| d.unit != script && d.unit != run));
    assert!(dead.iter().any(|d| all[&d.unit.clone()].0 == "bin"));
}

fn walk(command: &clap::Command, path: &str, out: &mut Vec<String>) {
    for arg in command.get_arguments() {
        out.push(format!("{path} --{}", arg.get_id()));
    }
    for sub in command.get_subcommands() {
        let name = format!("{path} {}", sub.get_name());
        out.push(name.clone());
        walk(sub, &name, out);
    }
}

#[test]
fn the_cli_has_no_new_subcommand_or_flag() {
    let mut surface = Vec::new();
    walk(&cargo_hive::cli::Cli::command(), "cargo-hive", &mut surface);
    surface.sort();
    let expected = [
        "cargo-hive __hold-lock",
        "cargo-hive __hold-lock --kind",
        "cargo-hive __hold-lock --path",
        "cargo-hive __try-lock",
        "cargo-hive __try-lock --kind",
        "cargo-hive __try-lock --path",
        "cargo-hive doctor",
        "cargo-hive doctor --json",
        "cargo-hive explain",
        "cargo-hive explain --json",
        "cargo-hive explain --repos",
        "cargo-hive explain --top",
        "cargo-hive gc",
        "cargo-hive gc --dry_run",
        "cargo-hive gc --json",
        "cargo-hive gc --repos",
        "cargo-hive repo",
        "cargo-hive repo add",
        "cargo-hive repo add --json",
        "cargo-hive repo add --path",
        "cargo-hive repo list",
        "cargo-hive repo list --json",
        "cargo-hive status",
        "cargo-hive status --json",
        "cargo-hive status --no_sizes",
        "cargo-hive status --repos",
    ];
    assert_eq!(surface, expected);
}

#[test]
fn status_writes_a_snapshot_with_only_its_two_fields() {
    let hive = Hive::ready();
    let scratch = Scratch::new("liveness-snapshot");
    let repo = init_repo(&scratch.path.join("app"), "");
    register(&hive, &repo);
    let out = hive.run(&["status", repo.to_str().unwrap(), "--json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let snapshot: serde_json::Value =
        serde_json::from_slice(&fs::read(hive.home.path.join("snapshot.json")).unwrap()).unwrap();
    let keys: BTreeSet<_> = snapshot.as_object().unwrap().keys().cloned().collect();
    assert_eq!(
        keys,
        BTreeSet::from(["metadata".to_string(), "unsupported_profiles".to_string()])
    );
}

#[test]
fn the_package_ships_no_test_file() {
    let scratch = Scratch::new("liveness-package");
    let stdout = fs::File::create(scratch.path.join("stdout")).unwrap();
    let stderr = fs::File::create(scratch.path.join("stderr")).unwrap();
    let child = Command::new("cargo")
        .args(["package", "--list", "--allow-dirty", "--offline"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .env("CARGO_TARGET_DIR", scratch.path.join("target"))
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .unwrap();
    let out = wait_within(child, CARGO_DEADLINE);
    let listing = fs::read_to_string(scratch.path.join("stdout")).unwrap();
    assert!(
        out.status.success(),
        "{}",
        fs::read_to_string(scratch.path.join("stderr")).unwrap()
    );
    assert!(listing.lines().any(|l| l == "Cargo.toml"));
    assert!(
        !listing.lines().any(|l| l.starts_with("tests/")),
        "{listing}"
    );
}
