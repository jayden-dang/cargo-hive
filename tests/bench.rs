#![cfg(target_os = "macos")]
mod common;

use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::time::SystemTime;

use cargo_hive::bench::check::{check, Checked, Unfair};
use cargo_hive::bench::command::{env_map, BuildCmd};
use cargo_hive::bench::{run, End, Limits, Plan, Report, BUILD_DEADLINE};
use cargo_hive::plan_tier::Ctx;
use cargo_hive::receipt::Measured;
use cargo_hive::registry::Registry;
use cargo_hive::state::{Config, State};
use common::plan_fixture::LibCtx;
use common::recording_fs::Recording;
use common::registry_fixture::{cargo_json, RegistryRepo};
use common::{env_lock, git, init_repo, register, CargoHomeGuard, Hive, Scratch};

fn bench_plan(hive: &Hive, repo: &RegistryRepo, n: usize) -> Plan {
    let cold_paths: Vec<_> = (0..n).map(|i| repo.worktree(&format!("cold{i}"))).collect();
    let seeded_paths: Vec<_> = (0..n)
        .map(|i| repo.worktree(&format!("seeded{i}")))
        .collect();
    let checked = checked(hive, &cold_paths, &seeded_paths)
        .unwrap_or_else(|list| panic!("{}", reason(Err(list))));
    let mut vars: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    vars.retain(|(k, _)| k != "CARGO_HOME");
    vars.push((
        OsString::from("CARGO_HOME"),
        hive.cargo_home.path.as_os_str().to_owned(),
    ));
    let env = env_map(vars, &checked.toolchain);
    Plan {
        repo: checked.repo,
        cold: checked.cold,
        seeded: checked.seeded,
        toolchain: checked.toolchain,
        build: BuildCmd::default(),
        env,
    }
}

fn run_bench(
    hive: &Hive,
    plan: &Plan,
    limits: Limits,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Report {
    run_bench_cfg(hive, plan, limits, cancel, |_| {})
}

fn run_bench_cfg(
    hive: &Hive,
    plan: &Plan,
    limits: Limits,
    cancel: &(dyn Fn() -> bool + Sync),
    configure: impl FnOnce(&mut Config),
) -> Report {
    let _env = env_lock().lock().unwrap_or_else(|err| err.into_inner());
    let _home = CargoHomeGuard::set(&hive.cargo_home.path);
    let mut lib = LibCtx::open(hive, Recording::over_real().fs());
    configure(&mut lib.cfg);
    let repos = [plan.repo.clone()];
    let ctx = Ctx {
        state: &lib.state,
        fs: lib.fs.as_ref(),
        cfg: &lib.cfg,
        now: SystemTime::now(),
        account_bytes: true,
        repos: &repos,
        mains: &lib.mains,
    };
    run(&ctx, plan, limits, cancel)
}

fn checked(hive: &Hive, cold: &[PathBuf], seeded: &[PathBuf]) -> Result<Checked, Vec<Unfair>> {
    let _env = env_lock().lock().unwrap_or_else(|err| err.into_inner());
    let _home = CargoHomeGuard::set(&hive.cargo_home.path);
    let state = State::open_at(hive.home.path.clone()).unwrap();
    let reg: Registry = state.read(cargo_hive::registry::FILE).unwrap();
    let cfg = state.config().unwrap();
    let caps = state.read(cargo_hive::doctor::CAPABILITIES_FILE).unwrap();
    check(&reg, &cfg, &caps, cold, seeded, &|| false)
}

fn reason(r: Result<Checked, Vec<Unfair>>) -> String {
    match r {
        Err(list) => list
            .iter()
            .map(|x| x.reason.clone())
            .collect::<Vec<_>>()
            .join(" | "),
        Ok(_) => "accepted".into(),
    }
}

#[test]
fn an_unfair_set_of_checkouts_is_refused_with_a_reason_for_each() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    let [a, b, c, d] = ["a", "b", "c", "d"].map(|n| repo.worktree(n));
    assert!(reason(checked(
        &hive,
        std::slice::from_ref(&a),
        std::slice::from_ref(&b)
    ))
    .contains("at least 2"));
    assert!(reason(checked(
        &hive,
        &[a.clone(), b.clone()],
        std::slice::from_ref(&c)
    ))
    .contains("equal"));
    assert!(reason(checked(
        &hive,
        &[a.clone(), a.clone()],
        &[c.clone(), d.clone()]
    ))
    .contains("more than once"));
    assert!(reason(checked(
        &hive,
        &[repo.main.clone(), b.clone()],
        &[c.clone(), d.clone()]
    ))
    .contains("protected"));
    fs::write(c.join("src/lib.rs"), "pub fn changed() {}\n").unwrap();
    assert!(reason(checked(
        &hive,
        &[a.clone(), b.clone()],
        &[c.clone(), d.clone()]
    ))
    .contains("uncommitted"));
    git(&c, &["checkout", "--", "."]);
    git(
        &d,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "x",
        ],
    );
    assert!(reason(checked(
        &hive,
        &[a.clone(), b.clone()],
        &[c.clone(), d.clone()]
    ))
    .contains("at HEAD"));
    let e = repo.worktree("e");
    let f = repo.worktree("f");
    fs::create_dir(e.join("target")).unwrap();
    assert!(reason(checked(
        &hive,
        &[a.clone(), b.clone()],
        &[e.clone(), f.clone()]
    ))
    .contains("already exists"));
    let g = repo.worktree("g");
    assert!(checked(&hive, &[a, b], &[f, g]).is_ok());
}

#[test]
fn checkouts_of_two_registered_repos_are_refused() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    let other_scratch = Scratch::new("other-repo");
    let other_main = init_repo(&other_scratch.path.join("main"), "");
    hive.config(&format!(
        r#"{{"secondary_roots": ["{}", "{}"]}}"#,
        repo.scratch.path.display(),
        other_scratch.path.display()
    ));
    register(&hive, &other_main);
    let other_lane = other_scratch.path.join("lane");
    git(
        &other_main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "lane",
            other_lane.to_str().unwrap(),
        ],
    );
    let [a, b, c] = ["a", "b", "c"].map(|n| repo.worktree(n));
    let r = reason(checked(&hive, &[a, b], &[c, other_lane]));
    assert!(r.contains("belongs to"), "{r}");
}

#[test]
fn a_disabled_seeding_capability_is_refused() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    hive.write_capabilities(serde_json::json!({"seeding": ["f"]}));
    let [a, b, c, d] = ["a", "b", "c", "d"].map(|n| repo.worktree(n));
    let r = reason(checked(&hive, &[a, b], &[c, d]));
    assert!(
        r.contains("seeding") || r.contains("disabled") || r.contains("doctor"),
        "{r}"
    );
}

#[test]
fn two_checkouts_that_share_a_target_directory_are_refused() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    let shared = repo.scratch.path.join("shared-target");
    fs::create_dir_all(repo.main.join(".cargo")).unwrap();
    fs::write(
        repo.main.join(".cargo/config.toml"),
        format!("[build]\ntarget-dir = \"{}\"\n", shared.display()),
    )
    .unwrap();
    git(&repo.main, &["add", ".cargo"]);
    git(&repo.main, &["commit", "-q", "-m", "share the target"]);
    let [a, b, c, d] = ["a", "b", "c", "d"].map(|n| repo.worktree(n));
    let r = reason(checked(&hive, &[a, b], &[c, d]));
    assert!(r.contains("shares the target"), "{r}");
}

#[test]
fn a_split_build_directory_is_refused() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    let build_out = repo.scratch.path.join("build-out");
    fs::create_dir_all(repo.main.join(".cargo")).unwrap();
    fs::write(
        repo.main.join(".cargo/config.toml"),
        format!("[build]\nbuild-dir = \"{}\"\n", build_out.display()),
    )
    .unwrap();
    git(&repo.main, &["add", ".cargo"]);
    git(&repo.main, &["commit", "-q", "-m", "split build dir"]);
    let [a, b, c, d] = ["a", "b", "c", "d"].map(|n| repo.worktree(n));
    let r = reason(checked(&hive, &[a, b], &[c, d]));
    assert!(r.contains("differs from the target directory"), "{r}");
}

#[test]
fn a_repo_without_a_cargo_lock_is_refused() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    git(&repo.main, &["rm", "-q", "Cargo.lock"]);
    git(&repo.main, &["commit", "-q", "-m", "drop lock"]);
    let [a, b, c, d] = ["a", "b", "c", "d"].map(|n| repo.worktree(n));
    let r = reason(checked(&hive, &[a, b], &[c, d]));
    assert!(r.contains("has no Cargo.lock"), "{r}");
}

#[test]
fn a_bench_primes_once_then_reports_every_sample_in_abba_order() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let plan = bench_plan(&hive, &repo, 2);
    let report = run_bench(
        &hive,
        &plan,
        Limits {
            build_deadline: BUILD_DEADLINE,
        },
        &|| false,
    );
    assert!(matches!(report.end, End::Finished));
    assert_eq!(report.exit_code(), 0);
    for key in ["prime_ms", "prime_entries_added", "prime_targets_read"] {
        assert!(report.counts.contains_key(key), "{key}");
    }
    let samples: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.kind == "bench.sample")
        .collect();
    let arms: Vec<_> = samples.iter().map(|f| f.measured["arm"].clone()).collect();
    assert_eq!(
        arms,
        ["cold", "seeded", "seeded", "cold"].map(|a| Measured::Text(a.into()))
    );
    for (i, f) in samples.iter().enumerate() {
        assert_eq!(f.measured["position"], Measured::Int(i as u64 + 1));
        for key in ["build_ms", "load_before_x100", "load_after_x100"] {
            assert!(
                matches!(f.measured.get(key), Some(Measured::Int(_))),
                "{key}"
            );
        }
    }
    for f in samples
        .iter()
        .filter(|f| f.measured["arm"] == Measured::Text("seeded".into()))
    {
        let ms = |k: &str| match f.measured[k] {
            Measured::Int(v) => v,
            _ => panic!("{k}"),
        };
        assert_eq!(ms("seed_ms"), ms("fill_ms") + ms("clone_ms"));
        assert!(f.measured.contains_key("fill_targets_read"));
    }
    for series in [
        "cold_build_ms",
        "seeded_build_ms",
        "seeded_seed_ms",
        "seeded_total_ms",
    ] {
        for stat in ["median", "p90", "min", "max", "spread"] {
            assert!(
                report.counts.contains_key(&format!("{series}_{stat}")),
                "{series}_{stat}"
            );
        }
    }
    for f in &report.findings {
        for key in f.measured.keys() {
            for banned in ["seconds", "duration", "elapsed", "saved", "estimate"] {
                assert!(!key.contains(banned));
            }
        }
    }
}
