#![cfg(target_os = "macos")]
mod common;

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use cargo_hive::bench::check::{check, Checked, Unfair};
use cargo_hive::bench::command::{env_map, BuildCmd};
use cargo_hive::bench::{run, End, Limits, Plan, Report, BUILD_DEADLINE};
use cargo_hive::plan_tier::Ctx;
use cargo_hive::receipt::Measured;
use cargo_hive::registry::Registry;
use cargo_hive::seed::{self, Fill};
use cargo_hive::state::{Config, State};
use common::plan_fixture::LibCtx;
use common::recording_fs::Recording;
use common::registry_fixture::{cargo_json, RegistryRepo};
use common::{env_lock, git, init_repo, register, wait_within, CargoHomeGuard, Hive, Scratch};

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

fn prime_store(hive: &Hive, plan: &Plan) {
    let _env = env_lock().lock().unwrap_or_else(|err| err.into_inner());
    let _home = CargoHomeGuard::set(&hive.cargo_home.path);
    let lib = LibCtx::open(hive, Recording::over_real().fs());
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
    match seed::run_fill(&ctx, &repos[0], &|| false).expect("prime fill") {
        Fill::Continue(_) => {}
        Fill::Stopped(_) => panic!("prime fill stopped"),
    }
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

#[test]
fn a_seeded_sample_copies_each_seed_finding_once() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let plan = bench_plan(&hive, &repo, 2);
    let blocked = repo.scratch.path.join("blocked");
    fs::create_dir_all(&blocked).unwrap();
    fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();
    let report = run_bench(
        &hive,
        &plan,
        Limits {
            build_deadline: BUILD_DEADLINE,
        },
        &|| false,
    );
    let _ = fs::set_permissions(&blocked, fs::Permissions::from_mode(0o755));
    assert!(matches!(report.end, End::Finished));
    let seeded: BTreeSet<_> = plan.seeded.iter().map(|c| c.path.clone()).collect();
    let rows: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.kind != "bench.sample" && f.path.as_ref().is_some_and(|p| seeded.contains(p)))
        .collect();
    assert!(
        rows.iter().any(|f| f.kind == "walk.unreadable"),
        "expected a fill finding on a seeded checkout, got {rows:?}"
    );
    let mut seen = BTreeSet::new();
    for f in rows {
        let key = (f.path.clone(), f.kind.clone(), f.detail.clone());
        assert!(
            seen.insert(key.clone()),
            "seed finding copied more than once: {key:?}"
        );
    }
}

#[test]
fn a_seeded_sample_carries_the_four_seed_counts_and_recompiles_none() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let report = run_bench(
        &hive,
        &bench_plan(&hive, &repo, 2),
        Limits {
            build_deadline: BUILD_DEADLINE,
        },
        &|| false,
    );
    for f in report.findings.iter().filter(|f| {
        f.kind == "bench.sample" && f.measured["arm"] == Measured::Text("seeded".into())
    }) {
        for key in [
            "seeded_fresh",
            "seeded_unneeded",
            "seeded_recompiled",
            "seeded_missing",
        ] {
            assert!(
                matches!(f.measured.get(key), Some(Measured::Int(_))),
                "{key}"
            );
        }
        assert_eq!(f.measured["seeded_recompiled"], Measured::Int(0));
    }
}

#[test]
fn a_seed_that_clones_nothing_still_builds_and_leaves_its_counts_unmeasured() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let mut plan = bench_plan(&hive, &repo, 2);
    plan.toolchain.version_text = "rustc 0.0.0 (no such toolchain)".into();
    let report = run_bench(
        &hive,
        &plan,
        Limits {
            build_deadline: BUILD_DEADLINE,
        },
        &|| false,
    );
    assert!(matches!(report.end, End::Finished));
    assert!(report
        .findings
        .iter()
        .any(|f| f.kind == "toolchain.missing"));
    assert!(report
        .unmeasured
        .iter()
        .any(|(field, _)| field.ends_with(".seed_counts")));
}

#[test]
fn a_build_that_fails_stops_the_run_and_reports_no_summary() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let mut plan = bench_plan(&hive, &repo, 2);
    plan.build = "check-lib=no_such_package".parse().unwrap();
    let report = run_bench(
        &hive,
        &plan,
        Limits {
            build_deadline: BUILD_DEADLINE,
        },
        &|| false,
    );
    assert!(matches!(report.end, End::Failed));
    assert_eq!(report.exit_code(), 1);
    let failed = report
        .findings
        .iter()
        .find(|f| f.kind == "bench.failed")
        .unwrap();
    assert!(failed.detail.contains("exit"), "{}", failed.detail);
    assert!(!report.counts.keys().any(|k| k.ends_with("_median")));
    assert_eq!(
        report
            .unmeasured
            .iter()
            .filter(|(f, _)| f.starts_with("sample."))
            .count(),
        3
    );
}

#[test]
fn a_build_past_its_deadline_is_killed_and_ends_the_run() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let plan = bench_plan(&hive, &repo, 2);
    let report = run_bench(
        &hive,
        &plan,
        Limits {
            build_deadline: Duration::from_millis(1),
        },
        &|| false,
    );
    assert!(matches!(report.end, End::Failed));
    let failed = report
        .findings
        .iter()
        .find(|f| f.kind == "bench.failed")
        .unwrap();
    assert!(failed.detail.contains("deadline"), "{}", failed.detail);
    for c in plan.cold.iter().chain(&plan.seeded) {
        assert!(c.path.exists());
    }
}

#[test]
fn a_seed_that_errors_stops_the_run() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let plan = bench_plan(&hive, &repo, 2);
    // Fill the store under a valid window first. A bad window makes fill store
    // nothing (src/seed/fill.rs), which would trip nothing_to_seed before clone;
    // run_clone is what returns Err on the bad window (src/seed/mod.rs:242-244).
    prime_store(&hive, &plan);
    let report = run_bench_cfg(
        &hive,
        &plan,
        Limits {
            build_deadline: BUILD_DEADLINE,
        },
        &|| false,
        |cfg| cfg.gc_window_days = Some(0),
    );
    assert!(matches!(report.end, End::Failed));
    let failed = report
        .findings
        .iter()
        .find(|f| f.kind == "bench.failed")
        .unwrap();
    assert!(failed.detail.contains("seed"), "{}", failed.detail);
    assert_eq!(
        report
            .findings
            .iter()
            .filter(|f| f.kind == "bench.sample")
            .count(),
        1
    ); // cold_1 only
}

#[test]
fn an_empty_store_for_the_repo_after_the_prime_stops_before_any_sample() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive); // main never built: the prime fills nothing
    let plan = bench_plan(&hive, &repo, 2);
    let report = run_bench(
        &hive,
        &plan,
        Limits {
            build_deadline: BUILD_DEADLINE,
        },
        &|| false,
    );
    assert!(matches!(report.end, End::Failed));
    assert_eq!(report.exit_code(), 1);
    assert!(report.counts.contains_key("prime_ms"));
    assert!(report
        .findings
        .iter()
        .any(|f| f.kind == "bench.nothing_to_seed"));
    assert_eq!(
        report
            .findings
            .iter()
            .filter(|f| f.kind == "bench.sample")
            .count(),
        0
    );
    for c in plan.cold.iter().chain(&plan.seeded) {
        assert!(!c.path.join("target").exists());
    }
}

#[test]
fn a_cancel_after_the_first_sample_keeps_it_and_lists_the_rest() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let plan = bench_plan(&hive, &repo, 2);
    // cold_1 builds first and finishes; seeded_1's seed then clones into its target/debug, which trips the
    // cancel during seeded_1's build.
    let seeded_debug = plan.seeded[0].path.join("target/debug");
    let cancel = move || seeded_debug.exists();
    let report = run_bench(
        &hive,
        &plan,
        Limits {
            build_deadline: BUILD_DEADLINE,
        },
        &cancel,
    );
    assert!(matches!(report.end, End::Cancelled));
    assert_eq!(report.exit_code(), 130);
    let samples: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.kind == "bench.sample")
        .collect();
    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].measured["arm"], Measured::Text("cold".into()));
    assert!(!report.counts.keys().any(|k| k.ends_with("_median")));
    assert!(report
        .unmeasured
        .iter()
        .any(|(f, r)| f.starts_with("sample.") && r == "cancelled"));
    for c in plan.cold.iter().chain(&plan.seeded) {
        assert!(c.path.exists());
    }
}

#[test]
fn bench_writes_one_schema_two_receipt_and_exits_zero() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let lanes = ["c1", "s1", "s2", "c2"].map(|n| repo.worktree(n));
    let tracked = git(&repo.main, &["ls-files", "-s"]);
    // each linked worktree's index lives where `git rev-parse --git-path index` says
    let index_of =
        |lane: &PathBuf| lane.join(git(lane, &["rev-parse", "--git-path", "index"]).trim());
    let indexes: Vec<Vec<u8>> = lanes
        .iter()
        .map(|l| fs::read(index_of(l)).unwrap())
        .collect();
    let s = |p: &PathBuf| p.to_str().unwrap().to_string();
    let (out, receipt) = hive.json(&[
        "bench",
        "--cold",
        &s(&lanes[0]),
        &s(&lanes[3]),
        "--seeded",
        &s(&lanes[1]),
        &s(&lanes[2]),
    ]);
    assert!(out.status.success(), "{receipt}");
    assert_eq!(receipt["command"], "bench");
    assert_eq!(receipt["schema"], 2);
    let keys: BTreeSet<_> = receipt.as_object().unwrap().keys().cloned().collect();
    let allowed: BTreeSet<_> = [
        "schema",
        "hive_version",
        "macos_version",
        "toolchain",
        "command",
        "ts",
        "scope",
        "counts",
        "checks",
        "findings",
        "unmeasured",
        "notes",
    ]
    .map(String::from)
    .into();
    assert!(keys.is_subset(&allowed), "{keys:?}");
    assert!(receipt["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["kind"] == "bench.command"));
    assert_eq!(git(&repo.main, &["ls-files", "-s"]), tracked);
    for (lane, before) in lanes.iter().zip(&indexes) {
        assert!(
            lane.join("target").exists(),
            "{}: bench removed nothing, so its target stays",
            lane.display()
        );
        assert_eq!(
            git(
                lane,
                &[
                    "--no-optional-locks",
                    "status",
                    "--porcelain",
                    "--untracked-files=no"
                ]
            ),
            "",
            "{}: a tracked file changed",
            lane.display()
        );
        assert_eq!(
            &fs::read(index_of(lane)).unwrap(),
            before,
            "{}",
            lane.display()
        );
    }
    let allowed: BTreeSet<&str> = [
        ".lock",
        "capabilities.json",
        "config.json",
        "receipts",
        "registry.json",
        "snapshot.json",
        "seeds.json",
        "store",
        "store.db",
    ]
    .into();
    let _ = fs::remove_file(hive.home.path.join("cargo-json.out"));
    let _ = fs::remove_file(hive.home.path.join("cargo-json.err"));
    for entry in fs::read_dir(&hive.home.path).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        assert!(
            allowed.contains(name.as_str()),
            "unexpected state file {name}"
        );
    }
}

#[test]
fn bench_refuses_with_exit_one_and_rejects_an_unknown_shape_with_exit_two() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    let a = repo.worktree("a");
    let b = repo.worktree("b");
    let s = |p: &PathBuf| p.to_str().unwrap().to_string();
    let (out, receipt) = hive.json(&["bench", "--cold", &s(&a), "--seeded", &s(&b)]);
    assert_eq!(out.status.code(), Some(1), "{receipt}");
    assert!(receipt["findings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["kind"] == "refusal"));
    let out = hive.run(&[
        "bench",
        "--cold",
        &s(&a),
        "--seeded",
        &s(&b),
        "--build-cmd",
        "run",
    ]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn ctrl_c_stops_bench_with_exit_130() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let lanes = ["c1", "s1", "s2", "c2"].map(|n| repo.worktree(n));
    let s = |p: &PathBuf| p.to_str().unwrap().to_string();
    let child = hive
        .command(&[
            "bench",
            "--cold",
            &s(&lanes[0]),
            &s(&lanes[3]),
            "--seeded",
            &s(&lanes[1]),
            &s(&lanes[2]),
            "--json",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let pid = child.id();
    let started = Instant::now();
    while !lanes[0].join("target/debug").exists() {
        assert!(
            started.elapsed() < Duration::from_secs(60),
            "the first build never started"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(Command::new("kill")
        .args(["-INT", &pid.to_string()])
        .status()
        .unwrap()
        .success());
    let out = wait_within(child, Duration::from_secs(30));
    assert_eq!(out.status.code(), Some(130), "{out:?}");
    for lane in &lanes {
        assert!(lane.exists());
    }
}
