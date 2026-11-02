#![cfg(target_os = "macos")]
mod common;

use std::collections::BTreeMap;
use std::fs;
use std::os::macos::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cargo_hive::plan_tier::Ctx;
use cargo_hive::seed::Error;
use common::plan_fixture::LibCtx;
use common::recording_fs::Recording;
use common::registry_fixture::{cargo_json, RegistryRepo};
use common::{init_repo, wait_within, Hive};

#[test]
fn the_fixture_builds_a_registry_crate_with_a_build_script_offline() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    let built = cargo_json(&hive, &repo.main, &["build"]);
    assert!(
        built.registry_recompiled.iter().any(|n| n == "depcrate"),
        "{:?}",
        built.registry_recompiled
    );
    assert!(built.registry_recompiled.iter().any(|n| n == "leafcrate"));
    let deps = repo.main.join("target/debug/deps");
    let dep_info = std::fs::read_dir(&deps)
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| {
            p.extension().is_some_and(|x| x == "d")
                && p.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("depcrate-")
        })
        .unwrap();
    let first = std::fs::read_to_string(dep_info).unwrap();
    let cargo_home = hive.cargo_home.path.join("registry/src");
    assert!(first.contains(&*cargo_home.to_string_lossy()), "{first}");
    let again = cargo_json(&hive, &repo.main, &["build"]);
    assert!(again.registry_recompiled.is_empty());
}

#[test]
fn the_first_build_of_a_seeded_worktree_recompiles_no_unit_it_cloned() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    cargo_json(&hive, &repo.main, &["check", "--all-targets"]);
    cargo_json(&hive, &repo.main, &["clippy", "--all-targets"]);
    cargo_json(&hive, &repo.main, &["test", "--no-run"]);
    for (name, args) in [
        ("b", vec!["build"]),
        ("c", vec!["check", "--all-targets"]),
        ("l", vec!["clippy", "--all-targets"]),
        ("t", vec!["test", "--no-run"]),
        ("p", vec!["check", "-p", "app", "--lib"]),
    ] {
        let lane = repo.worktree(name);
        let (out, receipt) = hive.json(&["seed", lane.to_str().unwrap()]);
        assert!(out.status.success(), "{receipt}");
        assert!(receipt["counts"]["clone_units_cloned"].as_u64().unwrap() > 0);
        let built = cargo_json(&hive, &lane, &args);
        assert!(
            built.registry_recompiled.is_empty(),
            "{name}: {:?}",
            built.registry_recompiled
        );
        assert!(!built.registry_fresh.is_empty(), "{name}");
    }
}

#[test]
fn a_second_lane_clones_without_filling_again() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let first = repo.worktree("first");
    let (out, receipt) = hive.json(&["seed", first.to_str().unwrap()]);
    assert!(out.status.success(), "{receipt}");
    assert!(receipt["counts"]["fill_entries_added"].as_u64().unwrap() > 0);
    let second = repo.worktree("second");
    let (out, receipt) = hive.json(&["seed", second.to_str().unwrap()]);
    assert!(out.status.success(), "{receipt}");
    assert_eq!(receipt["counts"]["fill_entries_added"], 0, "{receipt}");
    assert!(receipt["counts"]["fill_entries_held"].as_u64().unwrap() > 0);
    assert!(receipt["counts"]["clone_units_cloned"].as_u64().unwrap() > 0);
}

#[test]
fn seed_fills_even_when_the_path_exists() {
    let hive = Hive::ready();
    quiet_cache(&hive);
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let lane = repo.worktree("exists");
    cargo_json(&hive, &lane, &["build"]);
    let (out, receipt) = hive.json(&["seed", lane.to_str().unwrap()]);
    assert!(out.status.success(), "{receipt}");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(receipt["counts"]["clone_units_cloned"], 0, "{receipt}");
    assert_eq!(
        receipt["findings"].as_array().unwrap().len(),
        1,
        "{receipt}"
    );
    assert!(!store_units(&hive).is_empty(), "{receipt}");
}

#[test]
fn a_bad_window_is_refused_and_nothing_filled() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    hive.config(&format!(
        r#"{{"secondary_roots": ["{}"], "gc_window_days": 0}}"#,
        repo.scratch.path.display()
    ));
    let (out, receipt) = hive.json(&["seed", repo.main.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1), "{receipt}");
    assert!(
        receipt["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding["kind"] == "refusal"
                && finding["detail"]
                    .as_str()
                    .unwrap()
                    .contains("gc_window_days")),
        "{receipt}"
    );
    assert!(nothing_filled(&hive), "{receipt}");
}

#[test]
fn a_bad_window_inside_run_is_a_refusal() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    hive.config(&format!(
        r#"{{"secondary_roots": ["{}"], "gc_window_days": 0}}"#,
        repo.scratch.path.display()
    ));
    let lane = repo.worktree("bad-window");
    let result = run_seed(&hive, Recording::over_real(), &lane);
    assert!(
        result.is_err(),
        "a bad window is a refusal, not a clone of nothing: {result:?}"
    );
    let err = result.as_ref().unwrap_err();
    assert!(err.to_string().contains("gc_window_days"), "{err}");
    assert_eq!(cargo_hive::seed::exit_code(&result), 1);
    assert!(!lane.join("target").exists());
}

#[test]
fn a_protected_checkout_is_refused_and_nothing_filled() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    let (out, receipt) = hive.json(&["seed", repo.main.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1), "{receipt}");
    assert!(nothing_filled(&hive), "{receipt}");
}

#[test]
fn a_checkout_of_no_registered_repo_is_refused() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    let stranger = init_repo(&repo.scratch.path.join("stranger"), "");
    let (out, receipt) = hive.json(&["seed", stranger.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1), "{receipt}");
    assert!(nothing_filled(&hive), "{receipt}");
    let missing = repo.scratch.path.join("missing-checkout");
    let (out, receipt) = hive.json(&["seed", missing.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1), "{receipt}");
    assert!(nothing_filled(&hive), "{receipt}");
}

#[test]
fn a_disabled_capability_is_refused_and_nothing_filled() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    for (name, disabled) in [
        ("a", serde_json::json!({"seeding": ["a"]})),
        ("b", serde_json::json!({"store_compression": ["b"]})),
        ("d", serde_json::json!({"deletion": ["d"]})),
    ] {
        hive.write_capabilities(disabled);
        let lane = repo.worktree(name);
        let (out, receipt) = hive.json(&["seed", lane.to_str().unwrap()]);
        assert_eq!(out.status.code(), Some(1), "{name}: {receipt}");
        assert!(
            receipt["findings"]
                .as_array()
                .unwrap()
                .iter()
                .any(|finding| finding["kind"] == "refusal"),
            "{name}: {receipt}"
        );
        assert!(nothing_filled(&hive), "{name}: {receipt}");
    }
}

#[test]
fn a_failing_toolchain_lookup_is_refused() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    let lane = repo.worktree("absent-toolchain");
    fs::write(
        lane.join("rust-toolchain.toml"),
        "[toolchain]\nchannel = \"1.0.0\"\n",
    )
    .unwrap();
    let out = hive
        .command(&["seed", lane.to_str().unwrap(), "--json"])
        .env("RUSTUP_AUTO_INSTALL", "0")
        .env_remove("RUSTUP_TOOLCHAIN")
        .env_remove("RUSTUP_TOOLCHAIN_SOURCE")
        .output()
        .unwrap();
    let receipt: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|err| {
        panic!(
            "not one receipt ({err}): stdout {} stderr {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    assert_eq!(out.status.code(), Some(1), "{receipt}");
    assert!(nothing_filled(&hive), "{receipt}");
}

#[test]
fn a_store_with_no_tag_clones_nothing_with_a_finding() {
    let hive = Hive::ready();
    quiet_cache(&hive);
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let warmed = repo.worktree("warmed");
    let (out, receipt) = hive.json(&["seed", warmed.to_str().unwrap()]);
    assert!(out.status.success(), "{receipt}");
    let tag = hive.home.path.join("store").join("CACHEDIR.TAG");
    assert!(tag.is_file(), "{receipt}");
    fs::remove_file(&tag).unwrap();
    let _held = hold_build_locks(&hive, &repo.scratch.path);
    let lane = repo.worktree("untagged");
    let (out, receipt) = hive.json(&["seed", lane.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "{receipt}");
    assert_eq!(receipt["counts"]["clone_units_cloned"], 0, "{receipt}");
    assert!(
        receipt["counts"]["targets_skipped_held"].as_u64().unwrap() > 0,
        "{receipt}"
    );
    assert_eq!(
        receipt["findings"].as_array().unwrap().len(),
        1,
        "{receipt}"
    );
    assert_byte_names_unmeasured(&receipt);
    assert!(!tag.exists());
    assert!(!lane.join("target").exists());
}

#[test]
fn a_store_tag_that_cannot_be_stated_names_the_error() {
    let hive = Hive::ready();
    quiet_cache(&hive);
    let repo = RegistryRepo::new(&hive);
    let lane = repo.worktree("tag-error");
    let store = hive.home.path.join("store");
    fs::write(&store, b"not a directory").unwrap();
    let tag = store.join("CACHEDIR.TAG");
    let err = fs::symlink_metadata(&tag).expect_err("the tag cannot be stated");
    assert_ne!(err.kind(), std::io::ErrorKind::NotFound);
    let outcome = run_seed(&hive, Recording::over_real(), &lane).unwrap();
    assert_eq!(
        outcome
            .counts
            .get("clone_units_cloned")
            .copied()
            .unwrap_or(0),
        0
    );
    assert!(!lane.join("target").exists());
    let named = outcome.findings.iter().any(|(_, path, detail)| {
        path.as_deref() == Some(tag.as_path())
            && detail.contains(&err.to_string())
            && !detail.contains("is missing")
    });
    assert!(named, "{err}; {:?}", outcome.findings);
}

#[test]
fn an_empty_choice_clones_nothing_and_reports_why() {
    let hive = Hive::ready();
    quiet_cache(&hive);
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let warm = repo.worktree("warm-empty");
    let (out, receipt) = hive.json(&["seed", warm.to_str().unwrap()]);
    assert!(out.status.success(), "{receipt}");
    fs::remove_dir_all(warm.join("target")).unwrap();
    let store = hive.home.path.join("store");
    let mut removed = 0usize;
    for entry in fs::read_dir(&store).unwrap() {
        let path = entry.unwrap().path();
        if path.file_name().is_some_and(|name| name == "CACHEDIR.TAG") {
            continue;
        }
        if path.is_dir() {
            fs::remove_dir_all(&path).unwrap();
        } else {
            fs::remove_file(&path).unwrap();
        }
        removed += 1;
    }
    assert!(removed > 0, "the store had entries to drop");
    assert!(store.join("CACHEDIR.TAG").is_file());
    let _held = hold_build_locks(&hive, &repo.scratch.path);
    let lane = repo.worktree("empty-choice");
    let (out, receipt) = hive.json(&["seed", lane.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "{receipt}");
    assert_eq!(receipt["counts"]["clone_units_cloned"], 0, "{receipt}");
    let findings = receipt["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1, "{receipt}");
    assert_eq!(findings[0]["kind"], "units.empty", "{receipt}");
    assert!(!lane.join("target").exists());
}

#[test]
fn no_toolchain_row_clones_nothing_with_a_finding_and_exit_zero() {
    let hive = Hive::ready();
    quiet_cache(&hive);
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    fs::remove_file(repo.main.join("target").join(".rustc_info.json")).unwrap();
    let lane = repo.worktree("no-row");
    let (out, receipt) = hive.json(&["seed", lane.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "{receipt}");
    assert_eq!(receipt["counts"]["clone_units_cloned"], 0, "{receipt}");
    assert!(
        receipt["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|finding| finding["kind"] == "toolchain.missing"),
        "{receipt}"
    );
    assert_byte_names_unmeasured(&receipt);
    let stored: serde_json::Value =
        serde_json::from_slice(&fs::read(hive.home.path.join("store.db")).unwrap()).unwrap();
    assert!(
        stored["toolchains"].as_object().unwrap().is_empty(),
        "{stored}"
    );
    assert!(!lane.join("target").exists());
}

#[test]
fn the_receipt_separates_the_fill_from_the_clone_and_holds_no_time() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let lane = repo.worktree("receipt");
    let (out, receipt) = hive.json(&["seed", lane.to_str().unwrap()]);
    assert!(out.status.success(), "{receipt}");
    for name in [
        "fill_entries_added",
        "fill_entries_held",
        "fill_targets_read",
        "fill_targets_skipped",
        "clone_units_chosen",
        "clone_units_cloned",
        "clone_units_dropped",
        "clone_units_existing",
        "clone_units_failed",
        "clone_units_outside_window",
        "clone_bytes_not_shared",
        "store_bytes",
    ] {
        assert!(
            receipt["counts"]
                .get(name)
                .and_then(|n| n.as_u64())
                .is_some(),
            "{name} missing from {receipt}"
        );
    }
    let counts = &receipt["counts"];
    assert_eq!(
        counts["clone_units_chosen"].as_u64().unwrap(),
        counts["clone_units_cloned"].as_u64().unwrap()
            + counts["clone_units_existing"].as_u64().unwrap()
            + counts["clone_units_failed"].as_u64().unwrap()
            + counts["clone_units_dropped"].as_u64().unwrap(),
        "{receipt}"
    );
    let mut keys = Vec::new();
    collect_keys(&receipt, &mut keys, 0);
    for key in &keys {
        for banned in ["seconds", "duration", "elapsed"] {
            assert!(!key.contains(banned), "{key}");
        }
    }
    assert!(!receipt.to_string().contains("reused_fresh"), "{receipt}");
    let real = lane.canonicalize().unwrap();
    assert!(
        receipt["scope"]["checkouts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path.as_str() == Some(real.to_str().unwrap())),
        "{receipt}"
    );
    let release = cargo_hive::doctor::resolve_toolchain(&lane)
        .unwrap()
        .release;
    assert_eq!(receipt["toolchain"], release, "{receipt}");
}

#[test]
fn there_is_no_fill_command() {
    let hive = Hive::ready();
    let out = hive.run(&["fill"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
}

#[test]
fn compression_finishes_while_another_process_holds_the_state_lock() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let lane = repo.worktree("compress");
    let mut holder = hive.hold_state_lock();
    let child = hive
        .command(&["seed", lane.to_str().unwrap(), "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let store = hive.home.path.join("store");
    let started = Instant::now();
    loop {
        assert!(
            started.elapsed() < Duration::from_secs(180),
            "no compressed rlib appeared under {}",
            store.display()
        );
        if compressed_rlib(&store) {
            assert!(
                pid_alive(child.id()),
                "seed exited before the state lock was released"
            );
            break;
        }
        if !pid_alive(child.id()) {
            let out = wait_within(child, Duration::from_secs(5));
            panic!("seed exited before a compressed rlib existed: {out:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = holder.kill();
    let _ = holder.wait();
    let out = wait_within(child, Duration::from_secs(180));
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let stored: serde_json::Value =
        serde_json::from_slice(&fs::read(hive.home.path.join("store.db")).unwrap()).unwrap();
    assert!(!stored["units"].as_array().unwrap().is_empty(), "{stored}");
}

#[test]
fn a_configured_target_dir_is_the_directory_seed_returns() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let lane = repo.worktree("custom-dir");
    fs::create_dir_all(lane.join(".cargo")).unwrap();
    fs::write(
        lane.join(".cargo/config.toml"),
        "[build]\ntarget-dir = \"custom-target\"\n",
    )
    .unwrap();
    let outcome = run_seed(&hive, Recording::over_real(), &lane).unwrap();
    let (path, (dev, ino)) = outcome
        .target
        .expect("seed returns the directory it created");
    let created = lane.join("custom-target");
    assert_eq!(path, created.canonicalize().unwrap());
    assert_ne!(path, lane.join("target"));
    assert!(!lane.join("target").exists());
    let meta = fs::symlink_metadata(&created).unwrap();
    assert_eq!((meta.st_dev(), meta.st_ino()), (dev, ino));
    assert!(
        outcome
            .counts
            .get("clone_units_cloned")
            .copied()
            .unwrap_or(0)
            > 0
    );
}

#[test]
fn a_signal_exits_130() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let lane = repo.worktree("signal");
    let mut holder = hive.hold_state_lock();
    let child = hive
        .command(&["seed", lane.to_str().unwrap(), "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let store = hive.home.path.join("store");
    let started = Instant::now();
    loop {
        assert!(
            started.elapsed() < Duration::from_secs(180),
            "store never appeared at {}",
            store.display()
        );
        if store.is_dir() {
            break;
        }
        if !pid_alive(child.id()) {
            let out = wait_within(child, Duration::from_secs(5));
            panic!("seed exited before the store existed: {out:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap()
        .success());
    let out = wait_within(child, Duration::from_secs(60));
    assert_eq!(out.status.code(), Some(130), "{out:?}");
    let receipt: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|err| {
        panic!(
            "seed did not print a receipt ({err}): stdout {} stderr {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    assert_eq!(receipt["command"], "seed", "{receipt}");
    assert_eq!(receipt["schema"], 2, "{receipt}");
    assert_byte_names_unmeasured(&receipt);
    let _ = holder.kill();
    let _ = holder.wait();
}

#[test]
fn the_exit_code_follows_the_result() {
    let ok = Ok(blank_outcome(false));
    assert_eq!(cargo_hive::seed::exit_code(&ok), 0);
    let cancelled = Ok(blank_outcome(true));
    assert_eq!(cargo_hive::seed::exit_code(&cancelled), 130);
    let full = Err(cargo_hive::seed::Error::DiskFull("no space".into()));
    assert_eq!(cargo_hive::seed::exit_code(&full), 1);
    let state = Err(cargo_hive::seed::Error::State(
        cargo_hive::state::StateError::Cancelled,
    ));
    assert_eq!(cargo_hive::seed::exit_code(&state), 1);
}

#[test]
fn a_full_disk_reaches_the_caller() {
    let hive = Hive::ready();
    quiet_cache(&hive);
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let lane = repo.worktree("full");
    let tag = hive.home.path.join("store").join("CACHEDIR.TAG");
    let recording = Recording::over_real().fail_clone(&tag, libc::ENOSPC);
    let err = run_seed(&hive, recording, &lane).expect_err("a full disk is not success");
    assert!(matches!(err, Error::DiskFull(_)), "{err}");

    let hive = Hive::ready();
    quiet_cache(&hive);
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let _held = hold_build_locks(&hive, &repo.scratch.path);
    let lane = repo.worktree("no-tag");
    let outcome = run_seed(&hive, Recording::over_real(), &lane).unwrap();
    assert_eq!(outcome.findings.len(), 1, "{:?}", outcome.findings);
    assert_eq!(
        outcome
            .counts
            .get("clone_units_cloned")
            .copied()
            .unwrap_or(0),
        0
    );
    assert_eq!(
        outcome
            .counts
            .get("clone_units_chosen")
            .copied()
            .unwrap_or(0),
        0
    );
    assert!(outcome.placed.is_empty());
    assert!(outcome.target.is_none());
    for name in ["clone_bytes_not_shared", "store_bytes"] {
        assert!(
            outcome.unmeasured.iter().any(|(field, _)| field == name),
            "{name} omitted from {outcome:?}"
        );
        assert!(!outcome.counts.contains_key(name), "{outcome:?}");
    }
}

#[test]
fn seed_runs_no_cargo_subcommand_but_metadata() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let lane = repo.worktree("metadata");
    let bin = repo.scratch.path.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let log = repo.scratch.path.join("cargo.log");
    fs::write(
        bin.join("cargo"),
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$HIVE_CARGO_LOG\"\nrest=$PATH\nrest=${rest#*:}\nPATH=$rest\nexport PATH\nexec cargo \"$@\"\n",
    )
    .unwrap();
    fs::set_permissions(bin.join("cargo"), fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let child = hive
        .command(&["seed", lane.to_str().unwrap(), "--json"])
        .env("PATH", &path)
        .env("HIVE_CARGO_LOG", &log)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out = wait_within(child, Duration::from_secs(180));
    assert!(out.status.success(), "{out:?}");
    let text = fs::read_to_string(&log).unwrap();
    let mut lines = 0usize;
    for line in text.lines() {
        lines += 1;
        assert!(lines < 100_000, "walk bound hit");
        assert_eq!(
            line,
            "metadata --no-deps --format-version 1 --locked --offline"
        );
    }
    assert!(lines > 0, "seed ran no cargo at all");
}

#[test]
fn seed_writes_a_record_holding_each_cloned_unit_and_its_stamp() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let lane = repo.worktree("recorded");
    let (out, receipt) = hive.json(&["seed", lane.to_str().unwrap()]);
    assert!(out.status.success(), "{receipt}");
    let cloned = receipt["counts"]["clone_units_cloned"].as_u64().unwrap();
    assert!(cloned > 0, "{receipt}");
    let target = lane.join("target").canonicalize().unwrap();
    let state = cargo_hive::state::State::open_at(hive.home.path.clone()).unwrap();
    let path = state.root().join(cargo_hive::seed::record::SEEDS_FILE);
    assert!(path.is_file(), "{}", path.display());
    let file = cargo_hive::seed::record::read(&state).unwrap();
    let record = file
        .targets
        .get(&target)
        .unwrap_or_else(|| panic!("record keys {:?}", file.targets.keys().collect::<Vec<_>>()));
    let meta = fs::symlink_metadata(&target).unwrap();
    assert_eq!(record.identity, (meta.st_dev(), meta.st_ino()));
    assert_eq!(record.units.len() as u64, cloned, "{record:?}");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(
        record.written_at <= now && now.saturating_sub(record.written_at) < 86_400,
        "written_at {} is not epoch seconds near {now}",
        record.written_at
    );
    let script = record
        .units
        .iter()
        .find(|unit| stamp_under_build(unit))
        .expect("a run-build-script unit records a stamp under build/<dir>/");
    let (rel, ns) = script.stamp.clone().unwrap();
    let stamp = target.join(&rel);
    assert!(stamp.is_file(), "{}", stamp.display());
    let modified = fs::symlink_metadata(&stamp).unwrap().modified().unwrap();
    let modified_ns = u64::try_from(modified.duration_since(UNIX_EPOCH).unwrap().as_nanos())
        .expect("stamp mtime fits in u64");
    assert_eq!(ns, modified_ns, "{}", rel.display());
    assert!(
        record.units.iter().all(|unit| !unit.dir_name.is_empty()),
        "{record:?}"
    );
}

#[test]
fn seeds_receipt_holds_none_of_the_four_counts() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let lane = repo.worktree("receipt-counts");
    let (out, receipt) = hive.json(&["seed", lane.to_str().unwrap()]);
    assert!(out.status.success(), "{receipt}");
    assert!(receipt["counts"]["clone_units_cloned"].as_u64().unwrap() > 0);
    let mut keys = Vec::new();
    collect_keys(&receipt, &mut keys, 0);
    let banned: Vec<_> = keys
        .iter()
        .filter(|key| is_seed_count(key))
        .cloned()
        .collect();
    assert!(banned.is_empty(), "{banned:?} in {receipt}");
}

#[test]
fn a_cancelled_seed_records_what_it_cloned() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let lane = repo.worktree("cancel-record");
    let target = lane.join("target");
    let cancel = || target.join("debug").is_dir();
    let outcome = run_seed_until(&hive, Recording::over_real(), &lane, &cancel).unwrap();
    assert!(outcome.cancelled, "{outcome:?}");
    assert!(!outcome.placed.is_empty(), "{outcome:?}");
    let cloned = outcome
        .counts
        .get("clone_units_cloned")
        .copied()
        .unwrap_or(0);
    assert_eq!(cloned, outcome.placed.len() as u64, "{outcome:?}");
    assert!(
        outcome.placed.len() < store_units(&hive).len(),
        "cancel should leave a stored unit uncloned: {:?}",
        outcome.placed
    );
    assert_eq!(cargo_hive::seed::exit_code(&Ok(ref_outcome(&outcome))), 130);
    let (path, identity) = outcome.target.clone().expect("the target was created");
    let state = cargo_hive::state::State::open_at(hive.home.path.clone()).unwrap();
    let file = cargo_hive::seed::record::read(&state).unwrap();
    let record = file
        .targets
        .get(&path)
        .expect("the cancelled seed was recorded");
    assert_eq!(record.identity, identity);
    assert_eq!(record.units, outcome.placed);
}

#[test]
fn a_seed_that_cloned_nothing_writes_no_record() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let lane = repo.worktree("nothing-cloned");
    cargo_json(&hive, &lane, &["build"]);
    let (out, receipt) = hive.json(&["seed", lane.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "{receipt}");
    assert_eq!(receipt["counts"]["clone_units_cloned"], 0, "{receipt}");
    let path = hive.home.path.join(cargo_hive::seed::record::SEEDS_FILE);
    assert!(!path.exists(), "{}", path.display());
}

fn stamp_under_build(unit: &cargo_hive::seed::record::SeededUnit) -> bool {
    let Some((rel, _)) = &unit.stamp else {
        return false;
    };
    let text = rel.to_string_lossy();
    text.contains(&format!("/build/{}/", unit.dir_name))
        || text.starts_with(&format!("build/{}/", unit.dir_name))
}

fn is_seed_count(key: &str) -> bool {
    [
        "reused_fresh",
        "seeded_unneeded",
        "seeded_recompiled",
        "seeded_missing",
    ]
    .iter()
    .any(|name| key == *name || key.starts_with(&format!("{name}:")))
}

fn ref_outcome(outcome: &cargo_hive::seed::Outcome) -> cargo_hive::seed::Outcome {
    cargo_hive::seed::Outcome {
        counts: outcome.counts.clone(),
        findings: outcome.findings.clone(),
        unmeasured: outcome.unmeasured.clone(),
        notes: outcome.notes.clone(),
        cancelled: outcome.cancelled,
        placed: outcome.placed.clone(),
        target: outcome.target.clone(),
    }
}

fn assert_byte_names_unmeasured(receipt: &serde_json::Value) {
    let fields: Vec<_> = receipt["unmeasured"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|row| row["field"].as_str())
        .collect();
    for name in ["clone_bytes_not_shared", "store_bytes"] {
        assert!(fields.contains(&name), "{name} omitted from {receipt}");
        assert!(receipt["counts"].get(name).is_none(), "{receipt}");
    }
}

fn quiet_cache(hive: &Hive) {
    fs::create_dir_all(hive.home.path.join("no-cache")).unwrap();
}

fn nothing_filled(hive: &Hive) -> bool {
    !hive.home.path.join("store").exists() && !hive.home.path.join("store.db").exists()
}

fn store_units(hive: &Hive) -> Vec<serde_json::Value> {
    let path = hive.home.path.join("store.db");
    let Ok(bytes) = fs::read(&path) else {
        return Vec::new();
    };
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    value["units"].as_array().cloned().unwrap_or_default()
}

fn collect_keys(value: &serde_json::Value, out: &mut Vec<String>, depth: usize) {
    assert!(depth < 16, "walk bound hit");
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                out.push(key.clone());
                collect_keys(child, out, depth + 1);
            }
        }
        serde_json::Value::Array(items) => {
            assert!(items.len() < 100_000, "walk bound hit");
            for child in items {
                collect_keys(child, out, depth + 1);
            }
        }
        _ => {}
    }
}

fn hold_build_locks(hive: &Hive, root: &Path) -> Vec<Child> {
    let mut locks = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        assert!(depth < 10, "walk bound hit");
        let mut seen = 0usize;
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            seen += 1;
            assert!(seen < 100_000, "walk bound hit");
            let entry = entry.unwrap();
            let path = entry.path();
            let name = entry.file_name();
            if name == ".cargo-build-lock" {
                locks.push(hive.hold_lock_child("flock", &path));
                continue;
            }
            if name == ".git" || name == "reg" {
                continue;
            }
            if path.is_dir() {
                stack.push((path, depth + 1));
            }
        }
    }
    assert!(!locks.is_empty(), "no build lock under {}", root.display());
    locks
}

fn compressed_rlib(store: &Path) -> bool {
    let Ok(tops) = fs::read_dir(store) else {
        return false;
    };
    let mut seen = 0usize;
    for top in tops {
        seen += 1;
        assert!(seen < 100_000, "walk bound hit");
        let top = top.unwrap().path();
        let name = top.file_name().unwrap().to_string_lossy();
        if !name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if rlib_is_compressed(&top, 0) {
            return true;
        }
    }
    false
}

fn rlib_is_compressed(dir: &Path, depth: usize) -> bool {
    if depth > 8 {
        return false;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    let mut seen = 0usize;
    for entry in entries {
        seen += 1;
        assert!(seen < 100_000, "walk bound hit");
        let path = entry.unwrap().path();
        let meta = fs::symlink_metadata(&path).unwrap();
        if meta.is_dir() && rlib_is_compressed(&path, depth + 1) {
            return true;
        }
        if meta.is_file()
            && path.extension().is_some_and(|ext| ext == "rlib")
            && meta.st_flags() & libc::UF_COMPRESSED != 0
        {
            return true;
        }
    }
    false
}

fn pid_alive(pid: u32) -> bool {
    let child = Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_within(child, Duration::from_secs(10)).status.success()
}

fn blank_outcome(cancelled: bool) -> cargo_hive::seed::Outcome {
    cargo_hive::seed::Outcome {
        counts: BTreeMap::new(),
        findings: Vec::new(),
        unmeasured: Vec::new(),
        notes: Vec::new(),
        cancelled,
        placed: Vec::new(),
        target: None,
    }
}

fn run_seed(
    hive: &Hive,
    recording: Recording,
    lane: &Path,
) -> Result<cargo_hive::seed::Outcome, Error> {
    run_seed_until(hive, recording, lane, &|| false)
}

fn run_seed_until(
    hive: &Hive,
    recording: Recording,
    lane: &Path,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<cargo_hive::seed::Outcome, Error> {
    let _env = env_lock().lock().unwrap_or_else(|err| err.into_inner());
    let _home = CargoHomeGuard::set(&hive.cargo_home.path);
    let lib = LibCtx::open(hive, recording.fs());
    let lane = lane.canonicalize().unwrap();
    let mut matched = None;
    for candidate in &lib.repos {
        let (rows, _) = cargo_hive::discover::checkouts(candidate, &lib.mains, &lib.cfg);
        if let Some(row) = rows
            .into_iter()
            .find(|row| row.path == lane && !row.missing)
        {
            matched = Some((candidate.clone(), row));
            break;
        }
    }
    let (repo, checkout) = matched.expect("the lane is a discovered checkout");
    let toolchain = cargo_hive::doctor::resolve_toolchain(&lane).unwrap();
    let repos = [repo];
    let ctx = Ctx {
        state: &lib.state,
        fs: lib.fs.as_ref(),
        cfg: &lib.cfg,
        now: SystemTime::now(),
        account_bytes: true,
        repos: &repos,
        mains: &lib.mains,
    };
    cargo_hive::seed::run(&ctx, &repos[0], &checkout, &toolchain, cancel)
}

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

struct CargoHomeGuard {
    prev: Option<std::ffi::OsString>,
}

impl CargoHomeGuard {
    fn set(path: &Path) -> CargoHomeGuard {
        let prev = std::env::var_os("CARGO_HOME");
        // SAFETY: registry classification reads this process's `CARGO_HOME`. The previous
        // value is restored when the guard drops, including when an assertion panics.
        unsafe { std::env::set_var("CARGO_HOME", path) };
        CargoHomeGuard { prev }
    }
}

impl Drop for CargoHomeGuard {
    fn drop(&mut self) {
        // SAFETY: paired with `CargoHomeGuard::set`. Restores the environment this test replaced.
        unsafe {
            match self.prev.take() {
                Some(value) => std::env::set_var("CARGO_HOME", value),
                None => std::env::remove_var("CARGO_HOME"),
            }
        }
    }
}
