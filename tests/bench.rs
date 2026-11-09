#![cfg(target_os = "macos")]
mod common;

use std::fs;
use std::path::PathBuf;

use cargo_hive::bench::check::{check, Checked, Unfair};
use cargo_hive::registry::Registry;
use cargo_hive::state::State;
use common::registry_fixture::RegistryRepo;
use common::{env_lock, git, init_repo, register, CargoHomeGuard, Hive, Scratch};

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
