// The snapshot helpers serve only the macOS guard below.
#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]
mod common;
use common::{git, init_repo, Hive, Scratch};
use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

type Entry = (&'static str, u64, SystemTime, u64);

/// Every entry under `root`: kind, length, mtime, and a hash of the content or link target.
fn tree(root: &Path) -> BTreeMap<PathBuf, Entry> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let meta = fs::symlink_metadata(&path).unwrap();
            let mut h = DefaultHasher::new();
            let kind = if meta.is_dir() {
                stack.push(path.clone());
                "dir"
            } else if meta.is_symlink() {
                fs::read_link(&path).unwrap().hash(&mut h);
                "symlink"
            } else if meta.is_file() {
                fs::read(&path).unwrap().hash(&mut h);
                "file"
            } else {
                "special"
            };
            out.insert(
                path,
                (kind, meta.len(), meta.modified().unwrap(), h.finish()),
            );
        }
    }
    out
}

fn changed(before: &BTreeMap<PathBuf, Entry>, after: &BTreeMap<PathBuf, Entry>) -> Vec<String> {
    let keys: BTreeSet<&PathBuf> = before.keys().chain(after.keys()).collect();
    keys.into_iter()
        .filter(|k| before.get(*k) != after.get(*k))
        .map(|k| k.display().to_string())
        .collect()
}

fn cargo_home() -> PathBuf {
    std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap()).join(".cargo"))
}

/// What hive could plausibly add to ~/.cargo: config, env files, or a wrapper in bin/.
/// Narrower than a full snapshot on purpose: other cargo processes on this machine may
/// legitimately touch the registry and git caches while the test runs.
fn cargo_home_marks() -> BTreeMap<String, Option<SystemTime>> {
    let home = cargo_home();
    let mut marks = BTreeMap::new();
    for name in ["config", "config.toml", "env", "env.fish"] {
        marks.insert(
            name.to_string(),
            fs::metadata(home.join(name))
                .and_then(|m| m.modified())
                .ok(),
        );
    }
    if let Ok(entries) = fs::read_dir(home.join("bin")) {
        for e in entries.flatten() {
            marks.insert(format!("bin/{}", e.file_name().to_string_lossy()), None);
        }
    }
    marks
}

#[cfg(target_os = "macos")]
#[test]
fn repo_add_status_and_doctor_leave_checkouts_targets_and_cargo_home_unchanged() {
    let hive = Hive::ready();
    let s = Scratch::new("guard");
    let built = init_repo(&s.path.join("built"), "");
    hive.build_crate(&built, None);
    git(
        &built,
        &[
            "worktree",
            "add",
            "-q",
            built.join(".worktrees/lane").to_str().unwrap(),
        ],
    );
    let never_built = init_repo(&s.path.join("never-built"), "");
    fs::write(
        built.join("src/lib.rs"),
        "pub fn answer() -> u32 { \"no\" }\n",
    )
    .unwrap();
    hive.build_crate_failing(&built);
    let real_cargo_home = cargo_home();
    let (before, marks) = (tree(&s.path), cargo_home_marks());
    let runs: Vec<Vec<&str>> = vec![
        vec!["repo", "add", built.to_str().unwrap()],
        vec!["repo", "add", never_built.to_str().unwrap()],
        vec!["status"],
        vec!["status", "--no-sizes"],
        vec!["doctor"],
        vec!["explain"],
    ];
    for args in runs {
        let out = hive
            .command(&args)
            .env("CARGO_HOME", &real_cargo_home)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert_eq!(
        changed(&before, &tree(&s.path)),
        Vec::<String>::new(),
        "hive changed files in checkouts or targets"
    );
    assert_eq!(cargo_home_marks(), marks, "hive changed ~/.cargo");
}

#[test]
fn repo_add_status_and_doctor_leave_a_scratch_cargo_home_unchanged() {
    let hive = Hive::ready();
    let s = Scratch::new("cargo-guard");
    let repo = init_repo(&s.path.join("app"), "");
    let cargo_home = Scratch::new("cargo-tree");
    fs::write(cargo_home.path.join("marker"), b"keep").unwrap();
    let before = tree(&cargo_home.path);
    let runs: Vec<Vec<&str>> = vec![
        vec!["repo", "add", repo.to_str().unwrap()],
        vec!["status", "--no-sizes"],
        vec!["doctor"],
        vec!["explain"],
    ];
    for args in runs {
        let out = hive
            .command(&args)
            .env("CARGO_HOME", &cargo_home.path)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert_eq!(
        changed(&before, &tree(&cargo_home.path)),
        Vec::<String>::new(),
        "hive changed the scratch CARGO_HOME"
    );
}

#[test]
fn explain_installs_nothing_in_the_build_path() {
    let hive = Hive::ready();
    let s = Scratch::new("explain-path");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_crate(&repo, None);
    assert!(hive
        .run(&["repo", "add", repo.to_str().unwrap()])
        .status
        .success());
    let dot_cargo = repo.join(".cargo");
    assert!(
        !dot_cargo.exists(),
        "the fixture checkout already had .cargo"
    );
    let real_cargo_home = cargo_home();
    let marks = cargo_home_marks();
    let out = hive
        .command(&["explain"])
        .env("CARGO_HOME", &real_cargo_home)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !dot_cargo.exists(),
        "explain created .cargo in the checkout"
    );
    assert_eq!(cargo_home_marks(), marks, "explain changed ~/.cargo");
}

#[test]
fn cargo_builds_a_registered_checkout_exactly_as_without_hive() {
    let hive = Hive::ready();
    let s = Scratch::new("as-before");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_crate(&repo, None);
    assert!(hive
        .run(&["repo", "add", repo.to_str().unwrap()])
        .status
        .success());
    assert!(hive.run(&["status"]).status.success());
    let out = Command::new("cargo")
        .args(["build", "--offline"])
        .current_dir(&repo)
        .env("CARGO_HOME", &hive.cargo_home.path)
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR")
        .env_remove("CARGO_BUILD_BUILD_DIR")
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success() && !stderr.contains("Compiling"),
        "the build was not fresh after hive ran:\n{stderr}"
    );
    assert!(!repo.join(".cargo").exists());
    let allowed: BTreeSet<&str> = [
        ".lock",
        "capabilities.json",
        "config.json",
        "receipts",
        "registry.json",
        "snapshot.json",
    ]
    .into();
    for entry in fs::read_dir(&hive.home.path).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().to_string();
        assert!(
            allowed.contains(name.as_str()),
            "unexpected file in hive's state: {name}"
        );
    }
}

#[test]
fn killing_hive_mid_write_never_leaves_a_partial_state_file() {
    let hive = Hive::ready();
    let s = Scratch::new("kill");
    let repos: Vec<String> = (0..6)
        .map(|i| {
            init_repo(&s.path.join(format!("r{i}")), "")
                .display()
                .to_string()
        })
        .collect();
    for i in 0..40u64 {
        let repo = &repos[(i % 6) as usize];
        let args: Vec<&str> = if i % 3 == 0 {
            vec!["status", "--no-sizes", "--json"]
        } else {
            vec!["repo", "add", repo.as_str(), "--json"]
        };
        let mut child = hive
            .command(&args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_micros((i * 997) % 25_000));
        let _ = child.kill();
        let _ = child.wait();
    }
    for name in [
        "registry.json",
        "snapshot.json",
        "capabilities.json",
        "config.json",
    ] {
        let path = hive.home.path.join(name);
        if path.exists() {
            serde_json::from_slice::<serde_json::Value>(&fs::read(&path).unwrap())
                .unwrap_or_else(|e| panic!("{name} is not complete JSON: {e}"));
        }
    }
    for entry in fs::read_dir(hive.home.path.join("receipts")).unwrap() {
        for line in fs::read_to_string(entry.unwrap().path()).unwrap().lines() {
            serde_json::from_str::<serde_json::Value>(line)
                .unwrap_or_else(|e| panic!("a partial receipt line: {e}"));
        }
    }
    let out = hive.run(&["repo", "list"]);
    assert!(
        out.status.success(),
        "hive can no longer read its state: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn concurrent_registrations_are_all_kept() {
    let hive = Hive::ready();
    let s = Scratch::new("concurrent");
    let repos: Vec<PathBuf> = (0..8)
        .map(|i| init_repo(&s.path.join(format!("r{i}")), ""))
        .collect();
    let children: Vec<_> = repos
        .iter()
        .map(|r| {
            hive.command(&["repo", "add", r.to_str().unwrap()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();
    for mut child in children {
        assert!(child.wait().unwrap().success());
    }
    let reg: serde_json::Value =
        serde_json::from_slice(&fs::read(hive.home.path.join("registry.json")).unwrap()).unwrap();
    assert_eq!(reg["repos"].as_array().unwrap().len(), 8);
}

#[test]
#[ignore = "manual performance check on the proof machine; needs ~/Developer/klynt/klynt"]
fn status_on_klynt_finishes_within_two_seconds_cold_and_warm() {
    let klynt = PathBuf::from(std::env::var_os("HOME").unwrap()).join("Developer/klynt/klynt");
    assert!(
        klynt.join("Cargo.toml").is_file(),
        "{} is not a checkout",
        klynt.display()
    );
    let klynt = klynt.to_str().unwrap();
    let hive = Hive::ready();
    fs::remove_file(hive.home.path.join("config.json")).unwrap(); // the real defaults, as a user has them
    let real = cargo_home();
    assert!(hive
        .command(&["repo", "add", klynt])
        .env("CARGO_HOME", &real)
        .status()
        .unwrap()
        .success());
    let run = || {
        let started = Instant::now();
        let out = hive
            .command(&["status", klynt, "--no-sizes", "--json"])
            .env("CARGO_HOME", &real)
            .output()
            .unwrap();
        let took = started.elapsed();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        took
    };
    let _ = fs::remove_file(hive.home.path.join("snapshot.json"));
    let cold = run();
    let mut warm: Vec<Duration> = (0..5).map(|_| run()).collect();
    warm.sort();
    eprintln!("cold {cold:?}; warm {warm:?}");
    assert!(cold < Duration::from_secs(2), "cold run took {cold:?}");
    assert!(
        warm[2] < Duration::from_secs(2),
        "warm median took {:?}",
        warm[2]
    );
}

#[test]
#[ignore = "manual performance check on the proof machine; needs ~/Developer/klynt/klynt"]
fn explain_on_klynt_stays_within_two_seconds_of_status() {
    let klynt = PathBuf::from(std::env::var_os("HOME").unwrap()).join("Developer/klynt/klynt");
    assert!(
        klynt.join("Cargo.toml").is_file(),
        "{} is not a checkout",
        klynt.display()
    );
    let klynt = klynt.to_str().unwrap();
    let hive = Hive::ready();
    fs::remove_file(hive.home.path.join("config.json")).unwrap(); // the real defaults, as a user has them
    let real = cargo_home();
    assert!(hive
        .command(&["repo", "add", klynt])
        .env("CARGO_HOME", &real)
        .status()
        .unwrap()
        .success());
    let run = |args: &[&str]| {
        let started = Instant::now();
        let out = hive
            .command(args)
            .env("CARGO_HOME", &real)
            .output()
            .unwrap();
        let took = started.elapsed();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        took
    };
    let snapshot = hive.home.path.join("snapshot.json");
    let _ = fs::remove_file(&snapshot);
    let status_cold = run(&["status", klynt, "--no-sizes", "--json"]);
    let _ = fs::remove_file(&snapshot);
    let explain_cold = run(&["explain", klynt, "--json"]);
    let mut status_warm: Vec<Duration> = (0..5)
        .map(|_| run(&["status", klynt, "--no-sizes", "--json"]))
        .collect();
    status_warm.sort();
    let mut explain_warm: Vec<Duration> =
        (0..5).map(|_| run(&["explain", klynt, "--json"])).collect();
    explain_warm.sort();
    eprintln!(
        "cold status {status_cold:?} explain {explain_cold:?}; warm status {status_warm:?} explain {explain_warm:?}"
    );
    assert!(
        explain_cold.saturating_sub(status_cold) <= Duration::from_secs(2),
        "cold explain {explain_cold:?} minus status {status_cold:?}"
    );
    assert!(
        explain_warm[2].saturating_sub(status_warm[2]) <= Duration::from_secs(2),
        "warm median explain {:?} minus status {:?}",
        explain_warm[2],
        status_warm[2]
    );
}
