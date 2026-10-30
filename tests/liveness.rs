mod common;
use cargo_hive::cargo_target::{self, TargetDir};
use cargo_hive::fingerprint;
use cargo_hive::liveness::{liveness, Liveness, UnitRef};
use cargo_hive::platform;
use clap::CommandFactory;
use common::prove::{self, WindowOutcome};
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
    prove::read_target(path).expect("a cargo-tagged target dir")
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

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|p| p.to_string()).collect()
}

fn at(secs: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
}

fn key(profile: &str, dir: &str) -> (PathBuf, String) {
    (PathBuf::from(profile), dir.to_string())
}

#[test]
fn commands_are_checked_and_json_is_inserted_after_the_subcommand() {
    for (cmd, index) in [
        (vec!["build"], 0),
        (vec!["+stable", "build", "--offline"], 1),
        (vec!["-q", "check"], 1),
        (vec!["test", "--", "--target-dir"], 0),
        (vec!["check", "--all-targets"], 0),
        (vec!["test", "--", "--nocapture"], 0),
    ] {
        assert_eq!(prove::check_command(&argv(&cmd)), Ok(index), "{cmd:?}");
    }
    for cmd in [
        vec![],
        vec!["--offline"],
        vec!["build", "--message-format", "short"],
        vec!["build", "--message-format=short"],
        vec!["build", "--target-dir", "x"],
        vec!["build", "--target-dir=x"],
        vec!["build", "--artifact-dir=x"],
        vec!["build", "--artifact-dir", "x"],
        vec!["--target-dir=x", "build"],
    ] {
        assert!(prove::check_command(&argv(&cmd)).is_err(), "{cmd:?}");
    }
    assert_eq!(
        prove::with_json(&argv(&["test", "--", "--nocapture"])),
        argv(&["test", "--message-format=json", "--", "--nocapture"])
    );
    assert_eq!(
        prove::with_json(&argv(&["check", "--all-targets"])),
        argv(&["check", "--message-format=json", "--all-targets"])
    );
    assert_eq!(
        prove::with_json(&argv(&["+stable", "build", "--offline"])),
        argv(&["+stable", "build", "--message-format=json", "--offline"])
    );
    assert_eq!(
        prove::with_json(&argv(&["-q", "check"])),
        argv(&["-q", "check", "--message-format=json"])
    );
}

#[test]
fn only_subcommands_that_compile_inside_the_checkout_are_allowed() {
    for sub in ["build", "check", "test", "doc", "bench", "run"] {
        assert_eq!(prove::check_command(&argv(&[sub])), Ok(0), "{sub}");
        assert_eq!(
            prove::check_command(&argv(&["+nightly", sub])),
            Ok(1),
            "{sub}"
        );
    }
    for sub in [
        "fix",
        "fmt",
        "rm",
        "update",
        "add",
        "remove",
        "install",
        "uninstall",
        "publish",
        "generate-lockfile",
        "clean",
        "new",
        "init",
        "login",
        "vendor",
        "my-alias",
    ] {
        for cmd in [
            vec![sub],
            vec!["+nightly", sub],
            vec!["-q", sub, "--allow-dirty"],
        ] {
            let err = prove::check_command(&argv(&cmd)).unwrap_err();
            assert!(err.contains(sub), "{cmd:?}: {err}");
        }
    }
    assert!(prove::check_command(&argv(&["+nightly", "fix", "--allow-dirty"])).is_err());
    assert_eq!(
        prove::replay_args(&argv(&["fix"])),
        argv(&["fix", "--message-format=json"]),
        "fix is no longer lockable"
    );
}

#[test]
fn a_flag_with_a_value_before_the_subcommand_is_parsed_or_refused() {
    assert_eq!(
        prove::check_command(&argv(&["--color", "always", "build"])),
        Ok(2)
    );
    assert_eq!(
        prove::check_command(&argv(&["--color=always", "build"])),
        Ok(1)
    );
    assert_eq!(
        prove::check_command(&argv(&["+stable", "--color", "never", "-q", "test"])),
        Ok(4)
    );
    assert_eq!(
        prove::replay_args(&argv(&["--color", "always", "build"])),
        argv(&[
            "--color",
            "always",
            "build",
            "--locked",
            "--message-format=json"
        ])
    );
    assert!(prove::check_command(&argv(&["--color", "always", "fmt"])).is_err());
    for cmd in [
        vec!["--color", "build"],
        vec!["--color"],
        vec!["-C", "dir", "build"],
        vec!["--manifest-path", "x/Cargo.toml", "build"],
        vec!["--features", "a", "build"],
        vec!["-p", "a", "build"],
        vec!["--frobnicate", "build"],
    ] {
        let err = prove::check_command(&argv(&cmd)).unwrap_err();
        assert!(
            err.contains("before the subcommand")
                || err.contains("without a value")
                || err.contains("no cargo subcommand"),
            "{cmd:?}: {err}"
        );
    }
}

#[test]
fn unstable_env_is_scrubbed_from_the_replay() {
    let names: Vec<std::ffi::OsString> = [
        "CARGO_UNSTABLE_UNSTABLE_OPTIONS",
        "CARGO_UNSTABLE_X",
        "CARGO_HOME",
        "PATH",
    ]
    .iter()
    .map(Into::into)
    .collect();
    let process = prove::replay_command_with_env(
        &argv(&["build"]),
        Path::new("/co"),
        Path::new("/cl"),
        &names,
    );
    let envs: BTreeMap<_, _> = process
        .get_envs()
        .map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_owned())))
        .collect();
    for gone in ["CARGO_UNSTABLE_UNSTABLE_OPTIONS", "CARGO_UNSTABLE_X"] {
        assert_eq!(envs.get(gone), Some(&None), "{gone}");
    }
    assert!(!envs.contains_key("CARGO_HOME"));
    assert!(!envs.contains_key("PATH"));
}

#[test]
fn a_config_that_redirects_artifacts_is_refused() {
    let s = Scratch::new("prove-config");
    let work = s.path.join("a/b/checkout");
    fs::create_dir_all(work.join(".cargo")).unwrap();
    assert!(prove::check_config(&work).is_ok());
    for (file, body) in [
        (".cargo/config.toml", "[build]\nartifact-dir = \"/x\"\n"),
        (".cargo/config", "[build]\nout-dir = \"/x\"\n"),
    ] {
        fs::write(work.join(file), body).unwrap();
        let err = prove::check_config(&work).unwrap_err();
        assert!(
            err.contains(file) && (err.contains("artifact-dir") || err.contains("out-dir")),
            "{err}"
        );
        fs::remove_file(work.join(file)).unwrap();
    }
    fs::create_dir_all(s.path.join("a/.cargo")).unwrap();
    fs::write(
        s.path.join("a/.cargo/config.toml"),
        "[build]\nartifact-dir=\"/x\"\n",
    )
    .unwrap();
    assert!(prove::check_config(&work)
        .unwrap_err()
        .contains("artifact-dir"));
    fs::write(s.path.join("a/.cargo/config.toml"), "[build]\njobs = 2\n").unwrap();
    assert!(prove::check_config(&work).is_ok());
    fs::write(
        s.path.join("a/.cargo/config.toml"),
        vec![b'#'; (1 << 20) + 1],
    )
    .unwrap();
    assert!(prove::check_config(&work).unwrap_err().contains("1 MiB"));
}

#[test]
fn a_cargo_alias_env_var_is_scrubbed_from_the_replay() {
    let names: Vec<std::ffi::OsString> = ["CARGO_ALIAS_T", "CARGO_ALIAS_clippy", "PATH"]
        .iter()
        .map(Into::into)
        .collect();
    let process = prove::replay_command_with_env(
        &argv(&["build"]),
        Path::new("/co"),
        Path::new("/cl"),
        &names,
    );
    let envs: BTreeMap<_, _> = process
        .get_envs()
        .map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_owned())))
        .collect();
    for gone in ["CARGO_ALIAS_T", "CARGO_ALIAS_clippy"] {
        assert_eq!(envs.get(gone), Some(&None), "{gone}");
    }
    assert!(!envs.contains_key("PATH"));
}

#[test]
fn a_config_alias_cannot_turn_an_allowed_name_into_cargo_fix() {
    let s = Scratch::new("prove-alias");
    let app = s.path.join("app");
    let main = "fn main() { let x = 1; }\n";
    write(&app.join("Cargo.toml"), &manifest("app", ""));
    write(&app.join("src/main.rs"), main);
    write(
        &app.join(".cargo/config.toml"),
        "[alias]\nt = \"fix --allow-no-vcs --allow-dirty\"\n",
    );
    let status = Command::new("cargo")
        .args(["generate-lockfile", "--offline"])
        .current_dir(&app)
        .status()
        .unwrap();
    assert!(status.success());

    // Path 1: check_config refuses the checkout, naming the file.
    let err = prove::check_config(&app).unwrap_err();
    assert!(
        err.contains(".cargo/config.toml") && err.contains("`t`"),
        "{err}"
    );
    let inp = prove::ProveInput {
        target: s.path.join("target"),
        checkout: app.clone(),
        cmds: vec![argv(&["test"])],
        windows: vec![WINDOW],
        now: SystemTime::now(),
    };
    let fs_ = platform::current().unwrap().fs;
    let refused = prove::prove_with(fs_.as_ref(), &s.path, &inp).unwrap_err();
    assert!(refused.contains(".cargo/config.toml"), "{refused}");

    // Path 2: even past the config scan, the long name runs the real `test`.
    let args = prove::replay_args(&argv(&["test"]));
    assert_eq!(args[0], "test");
    let mut process = prove::replay_command(&args, &app, &s.path.join("clone"));
    process.arg("--offline");
    let out = process.output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(fs::read_to_string(app.join("src/main.rs")).unwrap(), main);
}

#[test]
fn only_aliases_that_shadow_an_allowed_name_or_include_files_are_refused() {
    let s = Scratch::new("prove-alias-scan");
    let work = s.path.join("a/checkout");
    fs::create_dir_all(work.join(".cargo")).unwrap();
    let cfg = work.join(".cargo/config.toml");
    for ok in [
        "[alias]\nxtask = \"run -p xtask --\"\n",
        "[build]\nbuild = 1\n",
        "[alias]\n# t = \"fix\"\nlint = \"clippy\"\n",
    ] {
        fs::write(&cfg, ok).unwrap();
        assert!(prove::check_config(&work).is_ok(), "{ok}");
    }
    for name in [
        "clippy", "build", "check", "test", "doc", "bench", "run", "rustc", "rustdoc", "b", "c",
        "d", "r", "t",
    ] {
        for body in [
            format!("[alias]\n{name} = \"fix\"\n"),
            format!("[alias]\n  \"{name}\"=[\"fix\"]\n"),
            format!("alias.{name} = \"fix\"\n"),
        ] {
            fs::write(&cfg, &body).unwrap();
            let err = prove::check_config(&work).unwrap_err();
            assert!(
                err.contains("config.toml") && err.contains(&format!("`{name}`")),
                "{body}: {err}"
            );
        }
    }
    fs::write(&cfg, "include = [\"more.toml\"]\n").unwrap();
    let err = prove::check_config(&work).unwrap_err();
    assert!(
        err.contains("config.toml") && err.contains("include"),
        "{err}"
    );
}

#[test]
fn config_scan_covers_home_relative_checkout_and_relative_cargo_home() {
    let s = Scratch::new("prove-scan-where");
    let root = s.path.clone();
    let bad = "[build]\nartifact-dir = \"/x\"\n";
    let work = root.join("w/checkout");
    fs::create_dir_all(&work).unwrap();
    let scan = |checkout: &Path, cwd: &Path, ch: Option<&str>, home: Option<&Path>| {
        prove::check_config_in(
            checkout,
            cwd,
            ch.map(Into::into),
            home.map(|h| h.as_os_str().to_owned()),
        )
    };
    // $HOME/.cargo is scanned when CARGO_HOME is unset, and not when it is set.
    let home = root.join("home");
    fs::create_dir_all(home.join(".cargo")).unwrap();
    fs::write(home.join(".cargo/config.toml"), bad).unwrap();
    assert!(scan(&work, &root, None, Some(&home))
        .unwrap_err()
        .contains("home/.cargo/config.toml"));
    assert!(scan(&work, &root, Some("ch"), Some(&home)).is_ok());
    assert!(scan(&work, &root, None, None).is_ok());
    // A relative checkout is resolved against the cwd, so its real ancestors are scanned.
    fs::create_dir_all(root.join("w/.cargo")).unwrap();
    fs::write(root.join("w/.cargo/config.toml"), bad).unwrap();
    assert!(scan(Path::new("checkout"), &root.join("w"), None, None)
        .unwrap_err()
        .contains("w/.cargo/config.toml"));
    fs::remove_file(root.join("w/.cargo/config.toml")).unwrap();
    // A relative CARGO_HOME is resolved against the checkout.
    fs::create_dir_all(work.join("ch")).unwrap();
    fs::write(work.join("ch/config.toml"), bad).unwrap();
    assert!(scan(&work, &root, Some("ch"), None)
        .unwrap_err()
        .contains("checkout/ch/config.toml"));
}

#[test]
fn short_names_and_alias_shadowable_subcommands_are_refused() {
    for (sub, hint) in [
        ("b", "build"),
        ("c", "check"),
        ("d", "doc"),
        ("r", "run"),
        ("t", "test"),
    ] {
        let err = prove::check_command(&argv(&["+stable", sub])).unwrap_err();
        assert!(
            err.contains(&format!("`cargo {sub}`")) && err.contains(&format!("use `{hint}`")),
            "{sub}: {err}"
        );
    }
    for sub in [
        "clippy", "rustc", "rustdoc", "fix", "fmt", "rm", "install", "v",
    ] {
        let err = prove::check_command(&argv(&[sub, "--", "-D", "warnings"])).unwrap_err();
        assert!(err.contains(&format!("`cargo {sub}`")), "{sub}: {err}");
    }
}

#[test]
fn args_after_the_dashes_are_refused_for_build_check_and_doc_only() {
    for sub in ["build", "check", "doc"] {
        for bad in ["-o", "--print=link-args=/x", "-Cincremental=/x", "anything"] {
            let err = prove::check_command(&argv(&[sub, "--", bad])).unwrap_err();
            assert!(
                err.contains(bad) && err.contains("after `--`"),
                "{sub} {bad}: {err}"
            );
        }
        assert_eq!(prove::check_command(&argv(&[sub, "--locked"])), Ok(0));
    }
    // test, bench and run hand their args to real project code, which runs anyway.
    for sub in ["test", "bench", "run"] {
        assert_eq!(
            prove::check_command(&argv(&[sub, "--", "-o", "x"])),
            Ok(0),
            "{sub}"
        );
    }
}

#[test]
fn prove_refuses_a_checkout_whose_config_redirects_artifacts() {
    let fx = proof_fixture("prove-artifactdir");
    fs::create_dir_all(fx.app.join(".cargo")).unwrap();
    fs::write(
        fx.app.join(".cargo/config.toml"),
        "[build]\nartifact-dir = \"/tmp/nope\"\n",
    )
    .unwrap();
    let scratch = Scratch::new("prove-scratch");
    let fs_ = platform::current().unwrap().fs;

    let err = prove::prove_with(fs_.as_ref(), &scratch.path, &input(&fx, &[WINDOW])).unwrap_err();

    assert!(err.contains("artifact-dir"), "{err}");
    no_clone_left(&scratch.path);
}

#[test]
fn a_clone_that_aliases_the_original_is_refused_and_nothing_is_deleted() {
    let fx = proof_fixture("prove-aliasclone");
    let scratch = Scratch::new("prove-scratch");
    // Empty checkout on purpose: if check_clone_root were dropped, the replay would fail at once (no Cargo.toml) instead of hanging on the build lock this process holds.
    let empty_checkout = Scratch::new("prove-empty-checkout");
    let before = tree_of(&fx.target);
    let fs_ = platform::current().unwrap().fs;
    let alias = |_: &Path, _: &str, target: &Path, dest: &Path| -> Result<(), String> {
        std::os::unix::fs::symlink(target, dest).map_err(|e| e.to_string())
    };

    let mut inp = input(&fx, &[WINDOW]);
    inp.checkout = empty_checkout.path.clone();
    let err = prove::prove_with_cloner(fs_.as_ref(), &scratch.path, &inp, &alias).unwrap_err();

    assert!(err.contains("not a real directory"), "{err}");
    assert_eq!(tree_of(&fx.target), before);
    no_clone_left(&scratch.path);
}

#[test]
fn rebuilt_units_are_new_dirs_or_changed_timestamps() {
    let before: prove::Stamps = BTreeMap::from([
        (key("debug", "a-1"), Some(at(1))),
        (key("debug", "b-2"), Some(at(1))),
        (key("debug", "c-3"), None),
    ]);
    let after: prove::Stamps = BTreeMap::from([
        (key("debug", "a-1"), Some(at(1))),
        (key("debug", "b-2"), Some(at(2))),
        (key("debug", "c-3"), None),
        (key("debug", "d-4"), Some(at(2))),
    ]);
    assert_eq!(
        prove::rebuilt(&before, &after),
        vec![key("debug", "b-2"), key("debug", "d-4")]
    );
    assert_eq!(prove::rebuilt(&after, &after), vec![]);
}

#[test]
fn a_rebuilt_live_unit_fails_the_window() {
    let rebuilt = vec![key("debug", "b-2"), key("x86/debug", "d-4")];
    let live = BTreeSet::from([key("debug", "b-2"), key("debug", "d-4")]);
    let hit = prove::rebuilt_live(&rebuilt, &live);
    assert_eq!(hit, vec!["debug/b-2".to_string()]);
    assert_eq!(
        prove::outcome(&hit, None),
        WindowOutcome::RebuiltLive(hit.clone())
    );
    let none = prove::rebuilt_live(&rebuilt, &BTreeSet::new());
    assert_eq!(none, Vec::<String>::new());
    assert_eq!(prove::outcome(&none, None), WindowOutcome::Passed);
    assert_eq!(
        prove::outcome(&none, Some("why".into())),
        WindowOutcome::Vacuous("why".into())
    );
}

#[test]
fn a_window_is_vacuous_when_undecided_empty_failed_or_silent() {
    let unit = UnitRef {
        profile: PathBuf::from("/t/debug"),
        dir_name: "a-0123456789abcdef".into(),
    };
    let dead = |files: Vec<PathBuf>| cargo_hive::liveness::DeadUnit {
        unit: unit.clone(),
        files,
    };
    let undecided = Liveness::Undecided {
        reasons: vec![cargo_hive::liveness::Uncertain::TargetUnreadable {
            target: PathBuf::from("/t"),
            why: "denied".into(),
        }],
    };
    let empty = Liveness::Decided {
        live: vec![],
        dead: vec![],
    };
    let listed = Liveness::Decided {
        live: vec![],
        dead: vec![dead(vec![PathBuf::from("/t/debug/deps/x")])],
    };
    let unlisted = Liveness::Decided {
        live: vec![],
        dead: vec![dead(vec![])],
    };
    assert!(prove::vacuous_reason(&undecided, &[0], &[1])
        .unwrap()
        .contains("undecided"));
    assert!(prove::vacuous_reason(&empty, &[0], &[1])
        .unwrap()
        .contains("no dead unit"));
    assert!(prove::vacuous_reason(&unlisted, &[0], &[1])
        .unwrap()
        .contains("no dead unit"));
    assert!(prove::vacuous_reason(&listed, &[0, 101], &[1, 1])
        .unwrap()
        .contains("101"));
    assert!(prove::vacuous_reason(&listed, &[0, 0], &[1, 0])
        .unwrap()
        .contains("compiler-artifact"));
    assert_eq!(prove::vacuous_reason(&listed, &[0, 0], &[3, 1]), None);
    assert_eq!(prove::vacuous_reason(&listed, &[], &[]), None);
}

#[test]
fn clone_timestamps_and_original_trees_are_compared_exactly() {
    let original: prove::Stamps = BTreeMap::from([
        (key("debug", "a-1"), Some(at(1))),
        (key("debug", "b-2"), Some(at(1))),
    ]);
    let mut clone = original.clone();
    assert_eq!(prove::mtime_mismatches(&original, &clone), vec![]);
    clone.insert(key("debug", "b-2"), Some(at(2)));
    assert_eq!(
        prove::mtime_mismatches(&original, &clone),
        vec![key("debug", "b-2")]
    );
    clone.remove(&key("debug", "a-1"));
    assert_eq!(
        prove::mtime_mismatches(&original, &clone),
        vec![key("debug", "a-1"), key("debug", "b-2")]
    );

    let before: prove::Tree = BTreeMap::from([
        (PathBuf::from("/t/a"), (at(1), 10)),
        (PathBuf::from("/t/b"), (at(1), 10)),
        (PathBuf::from("/t/c"), (at(1), 10)),
        (PathBuf::from("/t/d"), (at(1), 10)),
    ]);
    let mut after = before.clone();
    assert_eq!(prove::tree_changes(&before, &after), Vec::<PathBuf>::new());
    after.insert(PathBuf::from("/t/a"), (at(2), 10));
    after.insert(PathBuf::from("/t/b"), (at(1), 11));
    after.remove(&PathBuf::from("/t/c"));
    after.insert(PathBuf::from("/t/e"), (at(1), 0));
    assert_eq!(
        prove::tree_changes(&before, &after),
        ["/t/a", "/t/b", "/t/c", "/t/e"].map(PathBuf::from).to_vec()
    );
}

#[test]
fn the_walk_records_metadata_and_stops_at_its_cap() {
    let s = Scratch::new("prove-walk");
    write(&s.path.join("a/b.txt"), "hello");
    write(&s.path.join("c.txt"), "x");
    let tree = prove::walk(&s.path, 100).unwrap();
    assert_eq!(tree[&s.path.join("a/b.txt")].1, 5);
    assert!(tree.contains_key(&s.path.join("a")));
    assert!(prove::walk(&s.path, 2).is_err());
}

#[test]
fn all_vacuous_and_render_name_each_window_command_and_exit() {
    let window = |secs: u64, outcome: WindowOutcome, exit: i32| prove::WindowReport {
        window: Duration::from_secs(secs),
        outcome,
        dead_units: 3,
        dead_bytes: Default::default(),
        unmapped: Default::default(),
        rebuilt: 2,
        rebuilt_live: 0,
        reasons: vec!["a reason".into()],
        commands: vec![(argv(&["build", "--message-format=json", "--offline"]), exit)],
    };
    let mut report = prove::ProveReport {
        baseline_rebuilt: vec![],
        windows: vec![
            window(6 * 3600, WindowOutcome::Vacuous("none".into()), 0),
            window(7 * 86_400, WindowOutcome::Vacuous("none".into()), 101),
        ],
    };
    assert!(report.all_vacuous());
    let text = report.render();
    assert!(
        text.contains("build --message-format=json --offline"),
        "{text}"
    );
    assert!(text.contains("exit 101"), "{text}");
    assert!(text.contains("exit 0"), "{text}");
    assert!(text.contains("6h") && text.contains("7d"), "{text}");
    assert!(text.contains("a reason"), "{text}");
    report.windows[0].outcome = WindowOutcome::Passed;
    assert!(!report.all_vacuous());
    assert!(report.render().contains("only the replayed commands"));
}

#[test]
fn a_command_printing_more_than_a_pipe_holds_is_counted_from_its_log() {
    let s = Scratch::new("prove-log");
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(
        "i=0; while [ $i -lt 3000 ]; do echo '{\"reason\":\"compiler-artifact\",\"pad\":\"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\"}'; echo noise; i=$((i+1)); done; exit 7",
    );
    let ran = prove::run_logged(&s.path, "big", cmd, Duration::from_secs(60), 1 << 30).unwrap();
    assert_eq!(ran.exit, 7);
    assert_eq!(ran.artifact_lines, 3000);
    assert!(fs::metadata(&ran.stdout).unwrap().len() > 64 * 1024);
    assert!(prove::count_artifacts(&ran.stdout, 1024).is_err());
}

struct Volumes {
    inner: Box<dyn platform::Fs>,
    scratch: PathBuf,
    fs_type: &'static str,
    scratch_mount: String,
    touch_when_measured: Option<PathBuf>,
    fail_remove_file: bool,
}

impl platform::Fs for Volumes {
    fn temp_dir(&self, p: &Path, prefix: &str) -> std::io::Result<platform::TempDir> {
        self.inner.temp_dir(p, prefix)
    }
    fn write_file(&self, p: &Path, b: &[u8]) -> std::io::Result<()> {
        self.inner.write_file(p, b)
    }
    fn read(&self, p: &Path) -> std::io::Result<Vec<u8>> {
        self.inner.read(p)
    }
    fn hardlink(&self, s: &Path, d: &Path) -> std::io::Result<()> {
        self.inner.hardlink(s, d)
    }
    fn set_mtime(&self, p: &Path, t: SystemTime) -> std::io::Result<()> {
        self.inner.set_mtime(p, t)
    }
    fn mtime(&self, p: &Path) -> std::io::Result<SystemTime> {
        self.inner.mtime(p)
    }
    fn inode(&self, p: &Path) -> std::io::Result<u64> {
        self.inner.inode(p)
    }
    fn clone_file(&self, s: &Path, d: &Path) -> std::io::Result<()> {
        self.inner.clone_file(s, d)
    }
    fn sharing(&self, p: &Path) -> std::io::Result<platform::Sharing> {
        if let Some(path) = &self.touch_when_measured {
            set_mtime(path, SystemTime::now() + Duration::from_secs(5));
        }
        self.inner.sharing(p)
    }
    fn compress(&self, p: &Path) -> platform::CompressOutcome {
        self.inner.compress(p)
    }
    fn hold_lock(&self, k: platform::LockKind, p: &Path) -> std::io::Result<platform::LockHolder> {
        self.inner.hold_lock(k, p)
    }
    fn try_lock_exclusive(&self, p: &Path) -> platform::TryLock {
        self.inner.try_lock_exclusive(p)
    }
    fn try_lock_elsewhere(
        &self,
        k: platform::LockKind,
        p: &Path,
    ) -> std::io::Result<platform::LockState> {
        self.inner.try_lock_elsewhere(k, p)
    }
    fn entry_stat(&self, p: &Path) -> std::io::Result<platform::EntryStat> {
        self.inner.entry_stat(p)
    }
    fn read_dir_bounded(
        &self,
        d: &Path,
        cap: usize,
    ) -> std::io::Result<Vec<(String, platform::EntryKind)>> {
        self.inner.read_dir_bounded(d, cap)
    }
    fn remove_tree(&self, p: &Path) -> std::io::Result<()> {
        self.inner.remove_tree(p)
    }
    fn remove_file(&self, p: &Path) -> std::io::Result<()> {
        if self.fail_remove_file {
            return Err(std::io::Error::other("injected remove_file failure"));
        }
        self.inner.remove_file(p)
    }
    fn volume(&self, p: &Path) -> std::io::Result<platform::VolumeInfo> {
        let mut info = self.inner.volume(p)?;
        info.fs_type = self.fs_type.to_string();
        if p.starts_with(&self.scratch) {
            info.mount = PathBuf::from(&self.scratch_mount);
        }
        Ok(info)
    }
}

fn proof_fixture(label: &str) -> Fixture {
    let fx = fixture(label);
    cargo(&fx, &["check"]);
    backdate_all(&fx.target, THIRTY_DAYS);
    cargo(&fx, &["build", "--bins", "--examples"]);
    fx
}

fn input(fx: &Fixture, windows: &[Duration]) -> prove::ProveInput {
    prove::ProveInput {
        target: fx.target.clone(),
        checkout: fx.app.clone(),
        cmds: vec![argv(&["build", "--offline"])],
        windows: windows.to_vec(),
        now: SystemTime::now(),
    }
}

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn no_clone_left(scratch: &Path) {
    let left: Vec<_> = entries(scratch)
        .into_iter()
        .filter(|n| n.starts_with("clone"))
        .collect();
    assert_eq!(left, Vec::<String>::new());
}

#[test]
fn the_proof_passes_a_window_whose_dead_units_cargo_does_not_need_and_calls_an_empty_window_vacuous(
) {
    let fx = proof_fixture("prove-pass");
    let scratch = Scratch::new("prove-scratch");
    let before = prove::walk(&fx.target, 5_000_000).unwrap();
    let lock_before = fs::metadata(fx.app.join("Cargo.lock"))
        .unwrap()
        .modified()
        .unwrap();
    let fs_ = platform::current().unwrap().fs;
    let window = Duration::from_secs(6 * 3600);
    let long = Duration::from_secs(3650 * 86_400);

    let report =
        prove::prove_with(fs_.as_ref(), &scratch.path, &input(&fx, &[window, long])).unwrap();

    println!("{}", report.render());
    assert_eq!(report.baseline_rebuilt, Vec::<String>::new());
    let first = &report.windows[0];
    assert_eq!(first.outcome, WindowOutcome::Passed, "{}", report.render());
    assert!(first.dead_units > 0);
    assert!(first.dead_bytes.bytes.freed > 0, "{first:?}");
    assert!(first.unmapped.bytes.freed > 0, "{first:?}");
    assert_eq!(first.rebuilt_live, 0);
    assert_eq!(
        first.commands,
        vec![(
            argv(&["build", "--locked", "--message-format=json", "--offline"]),
            0
        )]
    );
    assert!(matches!(
        report.windows[1].outcome,
        WindowOutcome::Vacuous(_)
    ));
    assert!(!report.all_vacuous());
    assert_eq!(
        prove::tree_changes(&before, &prove::walk(&fx.target, 5_000_000).unwrap()),
        Vec::<PathBuf>::new()
    );
    assert_eq!(
        fs::metadata(fx.app.join("Cargo.lock"))
            .unwrap()
            .modified()
            .unwrap(),
        lock_before
    );
    no_clone_left(&scratch.path);
}

#[test]
fn a_held_build_lock_fails_the_proof_and_leaves_no_clone() {
    let fx = proof_fixture("prove-lock");
    let scratch = Scratch::new("prove-scratch");
    let fs_ = platform::current().unwrap().fs;
    let profile = fx.target.join("debug");
    let lock = profile.join(".cargo-build-lock");
    let held = match fs_.try_lock_exclusive(&lock) {
        platform::TryLock::Acquired(guard) => guard,
        other => panic!("expected the fixture lock to be free: {other:?}"),
    };

    let err = prove::prove_with(fs_.as_ref(), &scratch.path, &input(&fx, &[WINDOW])).unwrap_err();

    assert!(err.contains(&profile.display().to_string()), "{err}");
    assert_eq!(entries(&scratch.path), Vec::<String>::new());
    drop(held);
    assert!(prove::prove_with(fs_.as_ref(), &scratch.path, &input(&fx, &[WINDOW])).is_ok());
}

#[test]
fn a_volume_that_cannot_clone_fails_before_copying() {
    let fx = proof_fixture("prove-volume");
    let scratch = Scratch::new("prove-scratch");
    for (fs_type, scratch_mount, wanted) in [
        ("hfs", "/Volumes/other", "apfs"),
        ("apfs", "/Volumes/other", "same volume"),
    ] {
        let fs_ = Volumes {
            inner: platform::current().unwrap().fs,
            scratch: scratch.path.clone(),
            fs_type,
            scratch_mount: scratch_mount.to_string(),
            touch_when_measured: None,
            fail_remove_file: false,
        };
        let err = prove::prove_with(&fs_, &scratch.path, &input(&fx, &[WINDOW])).unwrap_err();
        assert!(err.contains(wanted), "{err}");
        assert_eq!(entries(&scratch.path), Vec::<String>::new());
    }
}

#[test]
fn a_baseline_rebuild_makes_every_window_vacuous() {
    let fx = proof_fixture("prove-baseline");
    let scratch = Scratch::new("prove-scratch");
    let source = fx.app.join("src/main.rs");
    fs::write(
        &source,
        "pm::m!();\nfn main() { println!(\"{}\", dep::answer()); }\n",
    )
    .unwrap();
    set_mtime(&source, SystemTime::now() + Duration::from_secs(120));
    let fs_ = platform::current().unwrap().fs;

    let report = prove::prove_with(
        fs_.as_ref(),
        &scratch.path,
        &input(&fx, &[WINDOW, THIRTY_DAYS]),
    )
    .unwrap();

    assert!(
        report.baseline_rebuilt.iter().any(|u| u.contains("app-")),
        "{report:?}"
    );
    assert!(report.all_vacuous());
    for w in &report.windows {
        match &w.outcome {
            WindowOutcome::Vacuous(why) => {
                assert!(why.contains("baseline") && why.contains("app-"), "{why}")
            }
            other => panic!("expected Vacuous, got {other:?}"),
        }
    }
    no_clone_left(&scratch.path);
}

#[test]
fn a_failing_command_makes_every_window_vacuous_and_leaves_no_clone() {
    let fx = proof_fixture("prove-failing");
    let scratch = Scratch::new("prove-scratch");
    let mut request = input(&fx, &[WINDOW]);
    request.cmds = vec![argv(&["build", "--offline", "--bogus-flag"])];
    let fs_ = platform::current().unwrap().fs;

    let report = prove::prove_with(fs_.as_ref(), &scratch.path, &request).unwrap();

    match &report.windows[0].outcome {
        WindowOutcome::Vacuous(why) => assert!(why.contains("exit"), "{why}"),
        other => panic!("expected Vacuous, got {other:?}"),
    }
    assert!(report.all_vacuous());
    no_clone_left(&scratch.path);
}

#[test]
fn deleting_dead_check_units_makes_a_check_replay_rebuild_them_without_failing() {
    let fx = proof_fixture("prove-check");
    let scratch = Scratch::new("prove-scratch");
    let mut request = input(&fx, &[WINDOW]);
    request.cmds = vec![argv(&["check", "--offline"])];
    let fs_ = platform::current().unwrap().fs;

    let report = prove::prove_with(fs_.as_ref(), &scratch.path, &request).unwrap();

    let w = &report.windows[0];
    assert_eq!(w.outcome, WindowOutcome::Passed, "{}", report.render());
    assert!(w.rebuilt > 0, "{}", report.render());
    assert_eq!(w.rebuilt_live, 0);
    no_clone_left(&scratch.path);
}

#[test]
fn a_change_to_the_original_during_the_proof_fails_it_naming_the_entry() {
    let fx = proof_fixture("prove-changed");
    let scratch = Scratch::new("prove-scratch");
    let touched = fx.app.join("Cargo.lock");
    let inner = platform::current().unwrap().fs;
    let mount = inner.volume(&scratch.path).unwrap().mount;
    let fs_ = Volumes {
        inner,
        scratch: scratch.path.clone(),
        fs_type: "apfs",
        scratch_mount: mount.to_string_lossy().into_owned(),
        touch_when_measured: Some(touched),
        fail_remove_file: false,
    };

    let err = prove::prove_with(&fs_, &scratch.path, &input(&fx, &[WINDOW])).unwrap_err();

    assert!(err.contains("Cargo.lock"), "{err}");
    no_clone_left(&scratch.path);
}

fn tree_of(dir: &Path) -> Vec<(PathBuf, bool)> {
    let mut out = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(p) = pending.pop() {
        let meta = fs::symlink_metadata(&p).unwrap();
        out.push((p.clone(), meta.is_dir()));
        if meta.is_dir() {
            for e in fs::read_dir(&p).unwrap() {
                pending.push(e.unwrap().path());
            }
        }
    }
    out.sort();
    out
}

#[test]
fn a_symlinked_target_root_is_refused_and_nothing_is_touched() {
    let fx = proof_fixture("prove-linkroot");
    let scratch = Scratch::new("prove-scratch");
    let link = fx._scratch.path.join("target-link");
    std::os::unix::fs::symlink(&fx.target, &link).unwrap();
    let before = tree_of(&fx.target);
    let walked = prove::walk(&fx.target, 5_000_000).unwrap();
    let fs_ = platform::current().unwrap().fs;
    let mut request = input(&fx, &[WINDOW]);
    request.target = link.clone();

    let err = prove::prove_with(fs_.as_ref(), &scratch.path, &request).unwrap_err();

    assert!(err.contains(&link.display().to_string()), "{err}");
    assert!(err.contains("symbolic link"), "{err}");
    assert_eq!(tree_of(&fx.target), before);
    assert_eq!(
        prove::tree_changes(&walked, &prove::walk(&fx.target, 5_000_000).unwrap()),
        Vec::<PathBuf>::new()
    );
    assert_eq!(entries(&scratch.path), Vec::<String>::new());
    assert!(prove::walk(&link, 5_000_000).is_err());
}

#[test]
fn a_symlinked_directory_inside_the_target_is_refused_and_the_outside_survives() {
    let fx = proof_fixture("prove-linkdir");
    let scratch = Scratch::new("prove-scratch");
    let outside = fx._scratch.path.join("outside-deps");
    let deps = fx.target.join("debug/deps");
    fs::rename(&deps, &outside).unwrap();
    std::os::unix::fs::symlink(&outside, &deps).unwrap();
    let before = tree_of(&outside);
    let fs_ = platform::current().unwrap().fs;

    let err = prove::prove_with(fs_.as_ref(), &scratch.path, &input(&fx, &[WINDOW])).unwrap_err();

    assert!(err.contains(&deps.display().to_string()), "{err}");
    assert_eq!(tree_of(&outside), before);
    no_clone_left(&scratch.path);
}

#[test]
fn a_delete_cannot_be_redirected_through_a_symlink_inside_the_clone() {
    let s = Scratch::new("prove-redirect");
    let target = s.path.join("target");
    let clone = s.path.join("clone");
    let outside = s.path.join("outside");
    write(&outside.join("keep.rlib"), "outside");
    write(&target.join("debug/deps/keep.rlib"), "original");
    fs::create_dir_all(clone.join("debug")).unwrap();
    std::os::unix::fs::symlink(&outside, clone.join("debug/deps")).unwrap();
    let fs_ = platform::current().unwrap().fs;

    let err = prove::delete_in_clone(
        fs_.as_ref(),
        &[target.join("debug/deps/keep.rlib")],
        &target,
        &clone,
    )
    .unwrap_err();

    assert!(err.contains("debug/deps"), "{err}");
    assert!(outside.join("keep.rlib").exists());
}

#[test]
fn a_clone_root_must_be_a_real_directory_inside_the_scratch() {
    let s = Scratch::new("prove-cloneroot");
    let elsewhere = Scratch::new("prove-elsewhere");
    let scratch = s.path.join("scratch");
    fs::create_dir_all(scratch.join("good")).unwrap();
    fs::create_dir_all(elsewhere.path.join("real")).unwrap();
    std::os::unix::fs::symlink(elsewhere.path.join("real"), scratch.join("link")).unwrap();
    let target = s.path.join("target");
    fs::create_dir_all(&target).unwrap();

    assert!(prove::check_clone_root(&scratch.join("good"), &scratch, &target).is_ok());
    let linked = prove::check_clone_root(&scratch.join("link"), &scratch, &target).unwrap_err();
    assert!(linked.contains("link"), "{linked}");
    assert!(prove::check_clone_root(&elsewhere.path.join("real"), &scratch, &target).is_err());
    assert!(prove::check_clone_root(&scratch.join("missing"), &scratch, &target).is_err());
}

#[test]
fn replayed_cargo_is_locked_and_cannot_be_redirected() {
    for (cmd, want) in [
        (
            vec!["build", "--offline"],
            vec!["build", "--locked", "--message-format=json", "--offline"],
        ),
        (
            vec!["+stable", "test", "--", "--nocapture"],
            vec![
                "+stable",
                "test",
                "--locked",
                "--message-format=json",
                "--",
                "--nocapture",
            ],
        ),
        (
            vec!["test", "--", "--locked"],
            vec![
                "test",
                "--locked",
                "--message-format=json",
                "--",
                "--locked",
            ],
        ),
        (
            vec!["check", "--frozen"],
            vec!["check", "--message-format=json", "--frozen"],
        ),
        (
            vec!["check", "--locked"],
            vec!["check", "--message-format=json", "--locked"],
        ),
    ] {
        assert_eq!(prove::replay_args(&argv(&cmd)), argv(&want), "{cmd:?}");
    }

    let process = prove::replay_command(&argv(&["build"]), Path::new("/co"), Path::new("/cl"));
    let envs: BTreeMap<_, _> = process
        .get_envs()
        .map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_owned())))
        .collect();
    for gone in [
        "CARGO_BUILD_ARTIFACT_DIR",
        "RUSTC_WRAPPER",
        "RUSTC_WORKSPACE_WRAPPER",
    ] {
        assert_eq!(envs.get(gone), Some(&None), "{gone}");
    }
    for set in ["CARGO_TARGET_DIR", "CARGO_BUILD_BUILD_DIR"] {
        assert_eq!(
            envs.get(set).unwrap().as_deref(),
            Some(std::ffi::OsStr::new("/cl")),
            "{set}"
        );
    }
    assert_eq!(
        envs.get("CARGO_BUILD_TARGET_DIR"),
        Some(&None),
        "CARGO_BUILD_TARGET_DIR"
    );
}

#[test]
fn config_out_dir_and_unstable_flags_are_refused() {
    for cmd in [
        vec!["build", "--config", "build.target-dir=\"/x\""],
        vec!["build", "--config=build.target-dir=\"/x\""],
        vec!["build", "--config", "build.build-dir=\"/x\""],
        vec!["--config", "x=y", "build"],
        vec!["build", "--out-dir", "x"],
        vec!["build", "--out-dir=x"],
        vec!["build", "-Z", "unstable-options"],
        vec!["build", "-Zunstable-options"],
        vec!["-Zunstable-options", "build"],
    ] {
        assert!(prove::check_command(&argv(&cmd)).is_err(), "{cmd:?}");
    }
    assert_eq!(
        prove::check_command(&argv(&["test", "--", "--config", "-Z"])),
        Ok(0)
    );
}

#[test]
fn an_error_after_a_clone_exists_removes_the_clone_before_the_scratch_does() {
    let fx = proof_fixture("prove-errclone");
    let scratch = Scratch::new("prove-scratch");
    let inner = platform::current().unwrap().fs;
    let mount = inner.volume(&scratch.path).unwrap().mount;
    let fs_ = Volumes {
        inner,
        scratch: scratch.path.clone(),
        fs_type: "apfs",
        scratch_mount: mount.to_string_lossy().into_owned(),
        touch_when_measured: None,
        fail_remove_file: true,
    };

    let err = prove::prove_with(&fs_, &scratch.path, &input(&fx, &[WINDOW])).unwrap_err();

    assert!(err.contains("cannot delete"), "{err}");
    assert!(scratch.path.exists());
    no_clone_left(&scratch.path);
}

#[test]
fn a_held_second_profile_lock_releases_the_first_profiles_lock() {
    let fx = proof_fixture("prove-locks");
    cargo(&fx, &["build", "--release"]);
    let scratch = Scratch::new("prove-scratch");
    let fs_ = platform::current().unwrap().fs;
    let profiles = profile_dirs(&fx.target);
    assert!(profiles.len() >= 2, "{profiles:?}");
    let last = profiles.last().unwrap();
    let held = match fs_.try_lock_exclusive(&last.join(".cargo-build-lock")) {
        platform::TryLock::Acquired(guard) => guard,
        other => panic!("expected the fixture lock to be free: {other:?}"),
    };

    let err = prove::prove_with(fs_.as_ref(), &scratch.path, &input(&fx, &[WINDOW])).unwrap_err();

    assert!(err.contains(&last.display().to_string()), "{err}");
    for profile in &profiles[..profiles.len() - 1] {
        match fs_.try_lock_exclusive(&profile.join(".cargo-build-lock")) {
            platform::TryLock::Acquired(_) => {}
            other => panic!("{} stayed locked: {other:?}", profile.display()),
        }
    }
    drop(held);
}

#[test]
fn a_command_writing_past_the_cap_is_killed_and_refused() {
    let s = Scratch::new("prove-cap");
    let started = std::time::Instant::now();

    let err = prove::run_logged(
        &s.path,
        "flood",
        Command::new("yes"),
        Duration::from_secs(20),
        1 << 20,
    )
    .err()
    .expect("a flood over the cap is an error");

    assert!(err.contains("cap"), "{err}");
    assert!(started.elapsed() < Duration::from_secs(15));
}
