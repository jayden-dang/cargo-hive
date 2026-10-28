#![cfg(target_os = "macos")]
mod common;
use common::{crate_dirs, init_repo, plant_session, sessions_in, wait_within, Hive, Scratch};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

fn register(hive: &Hive, repo: &Path) {
    let out = hive.run(&["repo", "add", repo.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn gc(hive: &Hive, args: &[&str]) -> (std::process::Output, serde_json::Value) {
    let mut all = vec!["gc"];
    all.extend_from_slice(args);
    hive.json(&all)
}

fn count(r: &serde_json::Value, key: &str) -> u64 {
    r["counts"][key]
        .as_u64()
        .unwrap_or_else(|| panic!("no count {key}: {r}"))
}

fn session_counts(target: &Path) -> BTreeMap<PathBuf, usize> {
    crate_dirs(target)
        .into_iter()
        .map(|d| {
            let n = sessions_in(&d).len();
            (d, n)
        })
        .collect()
}

/// Every path under `root` with its length and mtime.
fn tree(root: &Path) -> BTreeMap<PathBuf, (u64, std::time::SystemTime)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            let m = fs::symlink_metadata(&p).unwrap();
            if m.is_dir() {
                stack.push(p.clone());
            }
            out.insert(p, (m.len(), m.modified().unwrap()));
        }
    }
    out
}

fn fresh_build_compiles_nothing(hive: &Hive, repo: &Path) -> bool {
    let child = Command::new("cargo")
        .args(["build", "--offline"])
        .current_dir(repo)
        .env("CARGO_HOME", &hive.cargo_home.path)
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR")
        .env_remove("CARGO_BUILD_BUILD_DIR")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out = wait_within(child, Duration::from_secs(300));
    out.status.success() && !String::from_utf8_lossy(&out.stderr).contains("Compiling")
}

#[test]
fn gc_deletes_superseded_sessions_and_the_next_build_compiles_nothing() {
    let hive = Hive::ready();
    let s = Scratch::new("gc-basic");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_with_sessions(&repo, None);
    register(&hive, &repo);
    let target = repo.join("target");
    assert!(
        session_counts(&target).values().any(|n| *n > 1),
        "the fixture has no superseded session"
    );
    let (out, r) = gc(&hive, &[repo.to_str().unwrap()]);
    assert!(out.status.success(), "{r}");
    assert_eq!(r["command"], "gc");
    let deleted = count(&r, "sessions_deleted");
    assert!(deleted >= 1, "{r}");
    assert_eq!(
        count(&r, &format!("sessions_deleted:{}", target.display())),
        deleted
    );
    assert_eq!(count(&r, "session_lock_files_deleted"), deleted);
    assert!(count(&r, "freed_bytes") > 0, "{r}");
    assert!(
        r["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n.as_str().unwrap().starts_with("cross-check, not a count")),
        "{r}"
    );
    assert!(
        session_counts(&target).values().all(|n| *n == 1),
        "{:?}",
        session_counts(&target)
    );
    assert!(fresh_build_compiles_nothing(&hive, &repo));
}

#[test]
fn a_dry_run_reports_what_a_real_run_then_deletes_and_changes_nothing() {
    let hive = Hive::ready();
    let s = Scratch::new("gc-dry");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_with_sessions(&repo, None);
    fs::create_dir_all(repo.join("target/debug/deps/rmetaDry1")).unwrap();
    register(&hive, &repo);
    let before = tree(&repo.join("target"));
    let (_, dry) = gc(&hive, &["--dry-run", repo.to_str().unwrap()]);
    assert_eq!(dry["command"], "gc --dry-run");
    assert_eq!(
        tree(&repo.join("target")),
        before,
        "the dry run changed the target"
    );
    let (_, real) = gc(&hive, &[repo.to_str().unwrap()]);
    for key in [
        "sessions_deleted",
        "session_lock_files_deleted",
        "temp_entries_deleted",
        "freed_bytes",
        "deleted_bytes_still_shared",
    ] {
        assert_eq!(count(&dry, key), count(&real, key), "{key}");
    }
}

#[test]
fn held_sessions_and_a_held_build_lock_are_left_until_released() {
    let hive = Hive::ready();
    let s = Scratch::new("gc-held");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_with_sessions(&repo, None);
    register(&hive, &repo);
    let target = repo.join("target");
    let krate = crate_dirs(&target).remove(0);
    let (working, lock) = plant_session(&krate, 3600, "working", false);
    let temp = target.join("debug/deps/rmetaHeld1");
    fs::create_dir_all(&temp).unwrap();
    let mut session_holder = hive.hold_lock_child("fcntl", &krate.join(&lock));
    let mut build_holder = hive.hold_lock_child("flock", &target.join("debug/.cargo-build-lock"));
    let (_, r) = gc(&hive, &[repo.to_str().unwrap()]);
    assert!(krate.join(&working).exists() && temp.exists(), "{r}");
    assert!(count(&r, "skipped_held") >= 2, "{r}");
    for child in [&mut session_holder, &mut build_holder] {
        child.kill().unwrap();
        child.wait().unwrap();
    }
    gc(&hive, &[repo.to_str().unwrap()]);
    assert!(!krate.join(&working).exists() && !temp.exists());
}

#[test]
fn temp_entries_go_and_look_alikes_stay() {
    let hive = Hive::ready();
    let s = Scratch::new("gc-temps");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_crate(&repo, None);
    register(&hive, &repo);
    let deps = repo.join("target/debug/deps");
    fs::create_dir_all(deps.join("rmetaAb12")).unwrap();
    fs::write(deps.join("rmetaAb12/lib.rmeta"), vec![1u8; 4096]).unwrap();
    fs::create_dir_all(deps.join("rustcQw3")).unwrap();
    fs::write(deps.join("rustc_hash-0123456789abcdef.d"), b"x").unwrap();
    let (_, r) = gc(&hive, &[repo.to_str().unwrap()]);
    assert_eq!(count(&r, "temp_entries_deleted"), 2, "{r}");
    assert!(!deps.join("rmetaAb12").exists() && !deps.join("rustcQw3").exists());
    assert!(deps.join("rustc_hash-0123456789abcdef.d").exists());
}

#[test]
fn scope_follows_the_arguments() {
    let hive = Hive::ready();
    let s = Scratch::new("gc-scope");
    let cache = s.path.join("cache");
    hive.config(&format!("{{\"cache_roots\": [\"{}\"]}}", cache.display()));
    let a = init_repo(&s.path.join("a"), "");
    let b = init_repo(&s.path.join("b"), "");
    hive.build_with_sessions(&a, None);
    hive.build_with_sessions(&b, None);
    hive.build_with_sessions(&a, Some(&cache.join("old")));
    register(&hive, &a);
    register(&hive, &b);
    let (b0, c0) = (
        session_counts(&b.join("target")),
        session_counts(&cache.join("old")),
    );
    let (out, _) = gc(&hive, &[a.to_str().unwrap()]);
    assert!(out.status.success());
    assert!(session_counts(&a.join("target")).values().all(|n| *n == 1));
    assert_eq!(session_counts(&b.join("target")), b0, "gc <a> touched b");
    assert_eq!(
        session_counts(&cache.join("old")),
        c0,
        "gc <a> touched a cache-root target"
    );
    fs::create_dir_all(s.path.join("nope")).unwrap();
    let (out, r) = gc(
        &hive,
        &[s.path.join("nope").to_str().unwrap(), b.to_str().unwrap()],
    );
    assert_eq!(out.status.code(), Some(1));
    assert!(
        r["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["kind"] == "refusal"),
        "{r}"
    );
    assert!(
        session_counts(&b.join("target")).values().all(|n| *n == 1),
        "the valid argument was not processed"
    );
    gc(&hive, &[]);
    assert!(
        session_counts(&cache.join("old")).values().all(|n| *n == 1),
        "bare gc left the cache-root target"
    );
}

#[test]
fn gc_is_refused_without_doctor_results_from_the_current_lock_check() {
    let hive = Hive::ready();
    let s = Scratch::new("gc-gate");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_with_sessions(&repo, None);
    register(&hive, &repo);
    let path = hive.home.path.join("capabilities.json");
    let mut caps: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    caps.as_object_mut().unwrap().remove("lock_check");
    fs::write(&path, caps.to_string()).unwrap();
    let before = session_counts(&repo.join("target"));
    let (out, r) = gc(&hive, &[repo.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let refusal = r["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["kind"] == "refusal")
        .unwrap()["detail"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(refusal.contains("cargo hive doctor"), "{refusal}");
    assert_eq!(session_counts(&repo.join("target")), before);
}

#[test]
fn gc_reads_no_input_and_prints_exactly_one_receipt() {
    let hive = Hive::ready();
    let s = Scratch::new("gc-noinput");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_crate(&repo, None);
    register(&hive, &repo);
    let child = hive
        .command(&["gc", "--json", repo.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out = wait_within(child, Duration::from_secs(120));
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let receipt: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("exactly one JSON receipt on stdout");
    for key in ["schema", "hive_version", "macos_version", "scope"] {
        assert!(!receipt[key].is_null(), "{key}: {receipt}");
    }
    let logged: Vec<String> = fs::read_dir(hive.home.path.join("receipts"))
        .unwrap()
        .flat_map(|e| {
            fs::read_to_string(e.unwrap().path())
                .unwrap()
                .lines()
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(logged.iter().any(|l| l.contains("\"command\":\"gc\"")));
}

#[test]
fn an_unsupported_profile_is_reported_and_left_alone() {
    let hive = Hive::ready();
    let s = Scratch::new("gc-unsupported");
    let repo = init_repo(&s.path.join("app"), "");
    register(&hive, &repo);
    let weird = repo.join("weird");
    fs::create_dir_all(weird.join("debug/build/fixture/0123456789abcdef/fingerprint")).unwrap();
    fs::write(
        weird.join("CACHEDIR.TAG"),
        "# This file is a cache directory tag created by cargo.\n",
    )
    .unwrap();
    let krate = weird.join("debug/incremental/fixture-0abc");
    fs::create_dir_all(&krate).unwrap();
    let (old, _) = plant_session(&krate, 3600, "svh1", false);
    plant_session(&krate, 60, "svh2", false);
    let (_, r) = gc(&hive, &[]);
    assert!(
        krate.join(&old).exists(),
        "an unsupported profile was cleaned"
    );
    assert!(
        r["findings"].as_array().unwrap().iter().any(|f| {
            f["kind"] == "gc.skipped" && f["path"] == serde_json::json!(weird.join("debug"))
        }),
        "{r}"
    );
}

#[test]
fn builds_running_beside_a_looping_gc_never_fail() {
    let hive = Hive::ready();
    let s = Scratch::new("gc-concurrent");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_with_sessions(&repo, None);
    register(&hive, &repo);
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let looping = {
        let (stop, cmd_home, cargo_home, repo) = (
            stop.clone(),
            hive.home.path.clone(),
            hive.cargo_home.path.clone(),
            repo.clone(),
        );
        std::thread::spawn(move || {
            let mut runs = 0;
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                let out = Command::new(env!("CARGO_BIN_EXE_cargo-hive"))
                    .args(["gc", repo.to_str().unwrap()])
                    .env("HIVE_HOME", &cmd_home)
                    .env("CARGO_HOME", &cargo_home)
                    .output()
                    .unwrap();
                assert!(
                    out.status.success(),
                    "{}",
                    String::from_utf8_lossy(&out.stderr)
                );
                runs += 1;
            }
            runs
        })
    };
    for i in 0..12 {
        fs::write(
            repo.join("src/lib.rs"),
            format!("pub fn answer() -> u32 {{ {} }}\n", 100 + i),
        )
        .unwrap();
        let out = Command::new("cargo")
            .args(["build", "--offline"])
            .current_dir(&repo)
            .env("CARGO_HOME", &hive.cargo_home.path)
            .env_remove("CARGO_TARGET_DIR")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "build {i} failed: {stderr}");
        assert!(
            !stderr.contains("incremental"),
            "build {i} warned about incremental state: {stderr}"
        );
    }
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(looping.join().unwrap() > 0, "gc never ran");
    assert!(fresh_build_compiles_nothing(&hive, &repo));
}

#[test]
fn killing_gc_at_any_point_never_costs_a_rebuild() {
    let hive = Hive::ready();
    let s = Scratch::new("gc-kill");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_with_sessions(&repo, None);
    register(&hive, &repo);
    let target = repo.join("target");
    // The kept session per crate dir: the newest by name timestamp (same-length base 36).
    let kept: BTreeMap<PathBuf, String> = crate_dirs(&target)
        .into_iter()
        .map(|d| {
            let newest = sessions_in(&d)
                .into_iter()
                .max_by_key(|n| n.split('-').nth(1).unwrap().to_string())
                .unwrap();
            (d, newest)
        })
        .collect();
    let snapshot = |k: &BTreeMap<PathBuf, String>| -> BTreeMap<PathBuf, u64> {
        let mut out = BTreeMap::new();
        for (d, n) in k {
            for e in fs::read_dir(d.join(n)).unwrap() {
                let p = e.unwrap().path();
                out.insert(
                    p.clone(),
                    std::os::unix::fs::MetadataExt::ino(&fs::symlink_metadata(&p).unwrap()),
                );
            }
        }
        out
    };
    let kept_files = snapshot(&kept);
    for d in kept.keys() {
        for i in 0..40 {
            plant_session(d, 7200 + i, "svh0", false);
        }
    }
    for round in 0..8u64 {
        let mut child = hive
            .command(&["gc", repo.to_str().unwrap()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(round * 7));
        let _ = Command::new("kill")
            .args(["-9", &child.id().to_string()])
            .status();
        let _ = child.wait();
        assert_eq!(
            snapshot(&kept),
            kept_files,
            "round {round}: the kept session changed"
        );
    }
    let (out, _) = gc(&hive, &[repo.to_str().unwrap()]);
    assert!(out.status.success());
    for (d, n) in &kept {
        assert_eq!(
            sessions_in(d),
            vec![n.clone()],
            "gc did not converge in {}",
            d.display()
        );
    }
    assert!(fresh_build_compiles_nothing(&hive, &repo));
    fs::write(repo.join("src/lib.rs"), "pub fn answer() -> u32 { 7 }\n").unwrap();
    hive.build_crate(&repo, None);
}

#[test]
fn without_check_c_gc_still_deletes_and_records_the_byte_counts_as_unmeasured() {
    let hive = Hive::ready();
    let s = Scratch::new("gc-nocheckc");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_with_sessions(&repo, None);
    register(&hive, &repo);
    let path = hive.home.path.join("capabilities.json");
    let mut caps: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    caps["disabled"] = serde_json::json!({"eviction_accounting": ["c"]});
    fs::write(&path, caps.to_string()).unwrap();
    let (out, r) = gc(&hive, &[repo.to_str().unwrap()]);
    assert!(out.status.success(), "{r}");
    assert!(count(&r, "sessions_deleted") >= 1, "{r}");
    assert!(r["counts"].get("freed_bytes").is_none(), "{r}");
    assert!(
        r["unmeasured"]
            .to_string()
            .contains("doctor check (c) did not pass"),
        "{r}"
    );
}

#[test]
fn lockless_sessions_and_orphan_locks_are_cleaned_through_the_binary() {
    let hive = Hive::ready();
    let s = Scratch::new("gc-lockless");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_with_sessions(&repo, None);
    register(&hive, &repo);
    let krate = crate_dirs(&repo.join("target")).remove(0);
    let (lockless, _) = plant_session(&krate, 3600, "svh0", true);
    let (gone, orphan_lock) = plant_session(&krate, 3600, "svh0", false);
    fs::remove_dir_all(krate.join(&gone)).unwrap();
    let (_, r) = gc(&hive, &[repo.to_str().unwrap()]);
    assert!(!krate.join(&lockless).exists(), "{r}");
    assert!(!krate.join(&orphan_lock).exists(), "{r}");
}

#[test]
fn an_unreadable_target_is_reported_and_left_alone() {
    use std::os::unix::fs::PermissionsExt;
    let hive = Hive::ready();
    let s = Scratch::new("gc-unreadable");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_with_sessions(&repo, None);
    register(&hive, &repo);
    let debug = repo.join("target/debug");
    let before = session_counts(&repo.join("target"));
    fs::set_permissions(&debug, fs::Permissions::from_mode(0o000)).unwrap();
    let (_, r) = gc(&hive, &[repo.to_str().unwrap()]);
    fs::set_permissions(&debug, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(session_counts(&repo.join("target")), before);
    assert!(
        r["findings"].as_array().unwrap().iter().any(|f| {
            f["kind"] == "gc.skipped" && f["path"] == serde_json::json!(repo.join("target"))
        }),
        "{r}"
    );
}

#[test]
fn a_target_two_repositories_share_is_cleaned_by_either() {
    let hive = Hive::ready();
    let s = Scratch::new("gc-shared");
    let shared = s.path.join("shared-target");
    let a = init_repo(&s.path.join("a"), "");
    let b = init_repo(&s.path.join("b"), "");
    for repo in [&a, &b] {
        fs::create_dir_all(repo.join(".cargo")).unwrap();
        fs::write(
            repo.join(".cargo/config.toml"),
            format!("[build]\ntarget-dir = \"{}\"\n", shared.display()),
        )
        .unwrap();
    }
    hive.build_with_sessions(&a, None);
    register(&hive, &a);
    register(&hive, &b);
    assert!(session_counts(&shared).values().any(|n| *n > 1));
    let (out, r) = gc(&hive, &[b.to_str().unwrap()]);
    assert!(out.status.success(), "{r}");
    assert!(
        session_counts(&shared).values().all(|n| *n == 1),
        "gc <b> left the shared target: {r}"
    );
}

#[test]
fn gc_changes_nothing_outside_its_scope_and_status_reports_the_same() {
    let hive = Hive::ready();
    let s = Scratch::new("gc-outside");
    let a = init_repo(&s.path.join("a"), "");
    let b = init_repo(&s.path.join("b"), "");
    hive.build_with_sessions(&a, None);
    hive.build_with_sessions(&b, None);
    register(&hive, &a);
    register(&hive, &b);
    let (b_tree, cargo_home) = (tree(&b), tree(&hive.cargo_home.path));
    let status_b = |hive: &Hive| {
        let (_, r) = hive.json(&["status", "--no-sizes", b.to_str().unwrap()]);
        let mut keys: Vec<String> = r["counts"].as_object().unwrap().keys().cloned().collect();
        keys.sort();
        let findings: Vec<String> = r["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| format!("{} {}", f["kind"], f["path"]))
            .collect();
        (keys, findings)
    };
    let before = status_b(&hive);
    gc(&hive, &[a.to_str().unwrap()]);
    assert_eq!(tree(&b), b_tree, "gc <a> changed repository b");
    assert_eq!(
        tree(&hive.cargo_home.path),
        cargo_home,
        "gc changed CARGO_HOME"
    );
    assert_eq!(status_b(&hive), before);
}
