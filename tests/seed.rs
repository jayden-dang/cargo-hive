#![cfg(target_os = "macos")]
mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Seek};
use std::os::macos::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cargo_hive::discover::Checkout;
use cargo_hive::plan_tier::Ctx;
use cargo_hive::registry::Repo;
use cargo_hive::seed::Error;
use common::cargo_fixture::backdate_all;
use common::plan_fixture::LibCtx;
use common::recording_fs::Recording;
use common::registry_fixture::{cargo_json, RegistryRepo};
use common::{env_lock, init_repo, register, wait_within, CargoHomeGuard, Hive};

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
fn a_lane_seeded_after_two_builds_of_a_path_dependent_crate_still_builds() {
    let hive = Hive::ready();
    let repo = RegistryRepo::with_top(&hive);
    // One lane's last build stopped at depcrate, whose metadata embeds that lane's
    // OUT_DIR path. Seeding another lane fills the store from it first.
    let partial = repo.worktree("partial");
    cargo_json(&hive, &partial, &["build", "-p", "depcrate"]);
    let first = repo.worktree("first");
    let (out, receipt) = hive.json(&["seed", first.to_str().unwrap()]);
    assert!(out.status.success(), "{receipt}");
    // The main checkout then builds everything, topcrate against its own depcrate.
    cargo_json(&hive, &repo.main, &["build"]);
    let lane = repo.worktree("lane");
    let (out, receipt) = hive.json(&["seed", lane.to_str().unwrap()]);
    assert!(out.status.success(), "{receipt}");
    assert!(
        receipt["counts"]["clone_units_cloned"].as_u64().unwrap() > 0,
        "{receipt}"
    );
    // depcrate embeds its OUT_DIR path, so the two builds are two variants; leafcrate's
    // bytes are the same in both, so it is stored once (SEED-2.30, SEED-2.32).
    let stored: serde_json::Value =
        serde_json::from_slice(&fs::read(hive.home.path.join("store.db")).unwrap()).unwrap();
    let variants = |name: &str| {
        stored["units"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| {
                row["name_meta"].as_str().unwrap().starts_with(name)
                    && row["kind"].as_str() != Some("build-run")
            })
            .map(|row| row["variant"].as_u64().unwrap())
            .collect::<BTreeSet<_>>()
    };
    assert_eq!(variants("leafcrate-"), BTreeSet::from([0]), "{stored}");
    assert!(variants("depcrate-").contains(&1), "{stored}");
    let built = cargo_json(&hive, &lane, &["build"]);
    assert!(
        built.registry_recompiled.is_empty(),
        "{:?}",
        built.registry_recompiled
    );
}

#[test]
fn every_placed_file_carries_one_mtime_so_no_dependency_looks_newer() {
    let hive = Hive::ready();
    let repo = RegistryRepo::with_top(&hive);
    // Units held from an earlier build keep that build's older mtimes; a unit new in a
    // later build is newer. Cargo calls a dependent stale when a dependency is newer.
    let partial = repo.worktree("partial-mtime");
    cargo_json(&hive, &partial, &["build", "-p", "leafcrate"]);
    let first = repo.worktree("first-mtime");
    let (out, receipt) = hive.json(&["seed", first.to_str().unwrap()]);
    assert!(out.status.success(), "{receipt}");
    std::thread::sleep(Duration::from_millis(1100));
    cargo_json(&hive, &repo.main, &["build"]);
    let lane = repo.worktree("lane-mtime");
    let (out, receipt) = hive.json(&["seed", lane.to_str().unwrap()]);
    assert!(out.status.success(), "{receipt}");
    let mut mtimes = BTreeSet::new();
    let mut stack = vec![lane.join("target")];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let entry = entry.unwrap();
            let meta = entry.metadata().unwrap();
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                mtimes.insert(meta.modified().unwrap());
            }
        }
    }
    assert_eq!(mtimes.len(), 1, "{mtimes:?}");
    let built = cargo_json(&hive, &lane, &["build"]);
    assert!(
        built.registry_recompiled.is_empty(),
        "{:?}",
        built.registry_recompiled
    );
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
fn a_disabled_store_compression_does_not_refuse_seed() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    hive.write_capabilities(serde_json::json!({"store_compression": ["b"]}));
    let lane = repo.worktree("no-compress");
    let (out, receipt) = hive.json(&["seed", lane.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "{receipt}");
    assert!(
        receipt["counts"]["clone_units_cloned"].as_u64().unwrap() > 0,
        "{receipt}"
    );
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
fn the_rustup_default_does_not_beat_the_checkouts_own_toolchain_file() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    let lane = repo.worktree("default-vs-file");
    // The lane pins an absent channel, so a lookup that honors the file is refused.
    fs::write(
        lane.join("rust-toolchain.toml"),
        "[toolchain]\nchannel = \"1.0.0\"\n",
    )
    .unwrap();
    // What the rustup proxy hands `cargo hive` when the caller's directory has no
    // override: the default toolchain, with source `default`.
    let out = hive
        .command(&["seed", lane.to_str().unwrap(), "--json"])
        .env("RUSTUP_AUTO_INSTALL", "0")
        .env("RUSTUP_TOOLCHAIN", "stable")
        .env("RUSTUP_TOOLCHAIN_SOURCE", "default")
        .output()
        .unwrap();
    let receipt: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|err| {
        panic!(
            "not one receipt ({err}): stdout {} stderr {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    assert_eq!(
        out.status.code(),
        Some(1),
        "the default beat the lane's rust-toolchain.toml: {receipt}"
    );
    let refusal = receipt["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|finding| finding["kind"] == "refusal")
        .unwrap_or_else(|| panic!("no refusal: {receipt}"));
    let detail = refusal["detail"].as_str().unwrap();
    assert!(
        detail.contains(lane.to_str().unwrap())
            && detail.contains("install the toolchain that checkout selects"),
        "refused for another reason: {receipt}"
    );
    assert!(nothing_filled(&hive), "{receipt}");
}

#[test]
fn a_cache_root_target_this_repo_does_not_own_is_named_as_an_unresolved_source() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    // A built target no registered checkout owns, in the default cache root.
    let gone = init_repo(&repo.scratch.path.join("gone-repo"), "");
    let foreign = hive.home.path.join("no-cache/gone-repo-target");
    fs::create_dir_all(foreign.parent().unwrap()).unwrap();
    hive.build_crate(&gone, Some(&foreign));
    let foreign = foreign.canonicalize().unwrap();
    let lane = repo.worktree("cache-root");
    let (out, receipt) = hive.json(&["seed", lane.to_str().unwrap()]);
    assert!(out.status.success(), "{receipt}");
    let findings = receipt["findings"].as_array().unwrap();
    let foreign_text = foreign.display().to_string();
    let named: Vec<_> = findings
        .iter()
        .filter(|f| f["detail"].as_str().unwrap_or("").contains(&foreign_text))
        .collect();
    assert_eq!(named.len(), 1, "{receipt}");
    let finding = named[0];
    assert_eq!(finding["kind"], "source.unresolved", "{receipt}");
    assert_eq!(
        finding["path"].as_str(),
        Some(foreign_text.as_str()),
        "{receipt}"
    );
    // `status` advice does not fit seed, which reads only this repo's targets.
    assert!(
        !finding["detail"]
            .as_str()
            .unwrap()
            .contains("cargo hive status"),
        "{receipt}"
    );
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
    assert_early_finish_bytes(&receipt);
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
    assert_eq!(
        outcome.counts.get("clone_bytes_not_shared").copied(),
        Some(0)
    );
    assert!(!outcome.counts.contains_key("store_bytes"));
    assert!(outcome.unmeasured.iter().any(|(field, reason)| {
        field == "store_bytes" && reason.contains("the store's size was not measured")
    }));
}

#[test]
fn a_store_with_no_source_record_clones_nothing_and_says_so() {
    let hive = Hive::ready();
    quiet_cache(&hive);
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let warm = repo.worktree("warm-old");
    let (out, receipt) = hive.json(&["seed", warm.to_str().unwrap()]);
    assert!(out.status.success(), "{receipt}");
    fs::remove_dir_all(warm.join("target")).unwrap();
    // A store an older hive wrote has rows but no source records.
    let db = hive.home.path.join("store.db");
    let mut stored: serde_json::Value = serde_json::from_slice(&fs::read(&db).unwrap()).unwrap();
    stored.as_object_mut().unwrap().remove("sources");
    fs::write(&db, serde_json::to_vec(&stored).unwrap()).unwrap();
    // No target of the repo is left to record either.
    fs::remove_dir_all(repo.main.join("target")).unwrap();
    let lane = repo.worktree("no-record");
    let (out, receipt) = hive.json(&["seed", lane.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "{receipt}");
    assert_eq!(receipt["counts"]["clone_units_cloned"], 0, "{receipt}");
    assert!(
        receipt["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["kind"] == "source.none"),
        "{receipt}"
    );
    assert!(!lane.join("target").exists());
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
    assert_early_finish_bytes(&receipt);
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
    assert_early_finish_bytes(&receipt);
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
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("unrecognized subcommand 'fill'"), "{err}");
}

#[test]
fn a_seed_waits_for_an_exclusive_store_lock_and_then_seeds() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let lane = repo.worktree("store-lock-wait");
    let state = cargo_hive::state::State::open_at(hive.home.path.clone()).unwrap();
    let held = state
        .try_store_exclusive()
        .unwrap()
        .expect("exclusive store lock");
    let before_db = hive.home.path.join("store.db").exists();
    let before_store = hive.home.path.join("store").exists();
    let child = hive
        .command(&["seed", lane.to_str().unwrap(), "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_secs(1));
    assert!(
        pid_alive(child.id()),
        "seed exited while the exclusive store lock was held"
    );
    assert_eq!(hive.home.path.join("store.db").exists(), before_db);
    assert_eq!(hive.home.path.join("store").exists(), before_store);
    drop(held);
    let out = wait_within(child, Duration::from_secs(180));
    assert_eq!(out.status.code(), Some(0), "{out:?}");
    let receipt: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap_or_else(|err| {
        panic!(
            "seed did not print a receipt ({err}): stdout {} stderr {}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )
    });
    assert!(
        receipt["counts"]["clone_units_cloned"].as_u64().unwrap() > 0,
        "{receipt}"
    );
}

#[test]
fn publishing_finishes_while_another_process_holds_the_state_lock() {
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
            "no published rlib appeared under {}",
            store.display()
        );
        if published_rlib(&store) {
            assert!(
                pid_alive(child.id()),
                "seed exited before the state lock was released"
            );
            break;
        }
        if !pid_alive(child.id()) {
            let out = wait_within(child, Duration::from_secs(5));
            panic!("seed exited before a published rlib existed: {out:?}");
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
    assert_early_finish_bytes(&receipt);
    let _ = holder.kill();
    let _ = holder.wait();
}

#[test]
fn the_exit_code_follows_the_result() {
    let ok = Ok(blank_outcome(false));
    assert_eq!(cargo_hive::seed::exit_code(&ok), 0);
    let cancelled = Ok(blank_outcome(true));
    assert_eq!(cargo_hive::seed::exit_code(&cancelled), 130);
    let full = Err(cargo_hive::seed::Error::DiskFull(Box::new(
        cargo_hive::seed::FullDisk::new("no space"),
    )));
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
    assert!(err.to_string().contains("disk is full"), "{err}");

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
    assert_eq!(
        outcome.counts.get("clone_bytes_not_shared").copied(),
        Some(0)
    );
    assert!(outcome.counts.get("store_bytes").copied().is_some());
    assert!(outcome
        .unmeasured
        .iter()
        .all(|(field, _)| field != "clone_bytes_not_shared" && field != "store_bytes"));
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

#[test]
fn before_the_first_build_a_plan_lists_old_seeds_and_apply_skips_the_target() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let lane = repo.worktree("old-seeds");
    let (out, receipt) = hive.json(&["seed", lane.to_str().unwrap()]);
    assert!(out.status.success(), "{receipt}");
    assert!(receipt["counts"]["clone_units_cloned"].as_u64().unwrap() > 0);
    let target = lane.join("target");
    backdate_all(&target, Duration::from_secs(30 * 86_400));
    let (out, planned) = hive.json(&["gc", "--plan"]);
    assert!(out.status.success(), "{planned}");
    let plan = plan_for(&planned, &target);
    assert!(plan["paths"].as_u64().unwrap() > 0, "{planned}");
    let before = file_count(&target);
    let id = plan["id"].as_str().unwrap();
    let (out, applied) = hive.json(&["apply", id]);
    assert_eq!(out.status.code(), Some(0), "{applied}");
    let applied_plan = &applied["plans"][0];
    assert_eq!(applied_plan["outcome"], "partial", "{applied}");
    assert_eq!(applied_plan["reason"], "all_targets_skipped", "{applied}");
    assert_eq!(
        applied_plan["counts"]["targets_skipped_lock_missing"],
        serde_json::json!(1),
        "{applied}"
    );
    for key in [
        "targets_skipped_held",
        "targets_skipped_lock_unopenable",
        "targets_skipped_lock_unsupported",
        "targets_skipped_profile_set",
    ] {
        assert_eq!(
            applied_plan["counts"][key],
            serde_json::json!(0),
            "{key} {applied}"
        );
    }
    assert_eq!(file_count(&target), before, "{applied}");
    let built = cargo_json(&hive, &lane, &["build"]);
    assert!(
        built.registry_recompiled.is_empty(),
        "{:?}",
        built.registry_recompiled
    );
    assert!(
        !built.registry_fresh.is_empty(),
        "{:?}",
        built.registry_fresh
    );
}

#[test]
fn a_plan_frees_the_bytes_the_clone_did_not_share_and_still_counts_shared_bytes() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let lane = repo.worktree("shared-bytes");
    let (out, receipt) = hive.json(&["seed", lane.to_str().unwrap()]);
    assert!(out.status.success(), "{receipt}");
    assert!(receipt["counts"]["clone_units_cloned"].as_u64().unwrap() > 0);
    let target = lane.join("target");
    backdate_all(&target, Duration::from_secs(30 * 86_400));
    let (out, planned) = hive.json(&["gc", "--plan"]);
    assert!(out.status.success(), "{planned}");
    let plan = plan_for(&planned, &target);
    assert!(plan["paths"].as_u64().unwrap() > 0, "{plan}");
    // Every placed file shares its blocks with the store (SEED-2.28), so the plan may
    // claim as freed no more than the clone did not share, here nothing.
    let not_shared = receipt["counts"]["clone_bytes_not_shared"]
        .as_u64()
        .unwrap();
    assert_eq!(not_shared, 0, "{receipt}");
    let freed = plan["counts"]["freed_bytes"].as_u64().unwrap();
    let still_shared = plan["counts"]["deleted_bytes_still_shared"]
        .as_u64()
        .unwrap();
    assert!(freed <= not_shared, "{plan} {receipt}");
    assert!(still_shared > 0, "{plan}");
}

#[test]
fn a_changed_package_cache_or_rustup_names_that_path() {
    let before = sample_home();
    let package_cache = edited(&before, |home| home.package_cache = Some(9));
    let rustup = edited(&before, |home| home.rustup = Some(9));
    let settings = edited(&before, |home| home.rustup_settings = Some(9));
    assert_eq!(
        cargo_home_left_unchanged(&before, &package_cache).unwrap_err(),
        "~/.cargo/.package-cache"
    );
    assert_eq!(
        cargo_home_left_unchanged(&before, &rustup).unwrap_err(),
        "~/.rustup"
    );
    assert_eq!(
        cargo_home_left_unchanged(&before, &settings).unwrap_err(),
        "~/.rustup/settings.toml"
    );
}

#[test]
fn the_user_cache_directory_mtime_may_change() {
    let before = sample_home();
    let after = edited(&before, |home| home.cache = Some(9));
    assert_eq!(cargo_home_left_unchanged(&before, &after), Ok(()));
}

#[test]
fn a_prompt_dump_under_the_user_cache_may_change() {
    let before = edited(&sample_home(), |home| {
        home.cache_children.insert("p10k-jayden".to_string(), 1);
    });
    let after = edited(&before, |home| {
        home.cache_children.insert("p10k-jayden".to_string(), 9);
    });
    assert_eq!(cargo_home_left_unchanged(&before, &after), Ok(()));
}

#[test]
fn a_changed_user_cache_child_names_that_path() {
    let before = edited(&sample_home(), |home| {
        home.cache_children.insert("cargo".to_string(), 1);
    });
    let after = edited(&before, |home| {
        home.cache_children.insert("cargo".to_string(), 9);
    });
    assert_eq!(
        cargo_home_left_unchanged(&before, &after).unwrap_err(),
        "~/.cache/cargo"
    );
}

#[test]
fn a_direct_child_other_than_the_global_cache_names_that_path() {
    let before = sample_home();
    let after = edited(&before, |home| {
        home.cargo_children.insert("registry".to_string(), 9);
    });
    assert_eq!(
        cargo_home_left_unchanged(&before, &after).unwrap_err(),
        "~/.cargo/registry"
    );
}

#[test]
fn more_than_64_direct_children_of_the_user_cache_names_that_directory() {
    let before = edited(&sample_home(), |home| {
        home.cache_children = (0..65).map(|n| (format!("c{n}"), 1)).collect();
    });
    assert_eq!(
        cargo_home_left_unchanged(&before, &before).unwrap_err(),
        "~/.cache"
    );
}

#[test]
fn more_than_64_direct_children_of_the_cargo_home_names_that_directory() {
    let before = edited(&sample_home(), |home| {
        home.cargo_children = (0..65).map(|n| (format!("c{n}"), 1)).collect();
    });
    assert_eq!(
        cargo_home_left_unchanged(&before, &before).unwrap_err(),
        "~/.cargo"
    );
}

#[test]
fn listing_more_than_64_direct_children_returns_the_directory() {
    let scratch = common::Scratch::new("cargo-children");
    for n in 0..65 {
        fs::write(scratch.path.join(format!("c{n}")), b"x").unwrap();
    }
    assert_eq!(
        direct_child_mtimes(&scratch.path, "~/.cargo").unwrap_err(),
        "~/.cargo"
    );
}

#[test]
fn the_global_cache_and_the_cargo_directory_may_change() {
    let before = edited(&sample_home(), |home| {
        home.rustup_settings = None;
        home.cargo_children = (0..63).map(|n| (format!("c{n}"), 1)).collect();
        home.cargo_children.insert(".global-cache".to_string(), 2);
    });
    assert_eq!(before.cargo_children.len(), 64);
    let after = edited(&before, |home| {
        home.cargo = Some(50);
        home.global_cache = Some(60);
        home.cargo_children.insert(".global-cache".to_string(), 60);
    });
    assert_eq!(cargo_home_left_unchanged(&before, &after), Ok(()));
}

#[test]
fn listing_direct_children_does_not_enter_them() {
    let scratch = common::Scratch::new("cargo-children-flat");
    fs::create_dir(scratch.path.join("registry")).unwrap();
    fs::write(scratch.path.join("registry/inner"), b"nope").unwrap();
    fs::write(scratch.path.join(".global-cache"), b"g").unwrap();
    let children = direct_child_mtimes(&scratch.path, "~/.cargo").unwrap();
    assert_eq!(
        children.keys().cloned().collect::<Vec<_>>(),
        vec![".global-cache".to_string(), "registry".to_string()]
    );
}

#[test]
fn the_end_of_a_long_log_is_at_most_the_last_window() {
    let scratch = common::Scratch::new("log-tail");
    let path = scratch.path.join("cargo-build.err");
    let marker = "ended-with-this-marker";
    let mut bytes = vec![b'a'; 4_001];
    bytes.extend_from_slice(marker.as_bytes());
    fs::write(&path, &bytes).unwrap();
    let end = tail(&path);
    assert!(end.len() <= 4_000, "{}", end.len());
    assert!(end.ends_with(marker), "{end}");
}

#[test]
fn after_the_first_build_liveness_keeps_every_unit_that_build_needed() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let lane = repo.worktree("kept-live");
    let (out, receipt) = hive.json(&["seed", lane.to_str().unwrap()]);
    assert!(out.status.success(), "{receipt}");
    let target = lane.join("target");
    let seeded = seeded_dir_names(&hive, &target);
    assert!(!seeded.is_empty(), "{receipt}");
    backdate_all(&target, Duration::from_secs(30 * 86_400));
    let built = cargo_json(&hive, &lane, &["build"]);
    assert!(
        built.registry_recompiled.is_empty(),
        "{:?}",
        built.registry_recompiled
    );
    assert!(
        !built.registry_fresh.is_empty(),
        "{:?}",
        built.registry_fresh
    );
    let mut used = seeded;
    used.extend(dirs_stamped_within(
        &target,
        Duration::from_secs(7 * 86_400),
    ));
    let target = target.canonicalize().unwrap();
    let (out, planned) = hive.json(&["gc", "--plan"]);
    assert!(out.status.success(), "{planned}");
    assert!(
        planned["undecided"].as_array().unwrap().iter().all(|item| {
            item.get("target")
                .and_then(|path| path.as_str())
                .is_none_or(|path| Path::new(path) != target)
        }),
        "{planned}"
    );
    let stored = plans_stored(&hive);
    let under: Vec<String> = stored_paths(&stored)
        .into_iter()
        .filter(|path| path_under(path, &target))
        .collect();
    let hits: Vec<String> = under
        .iter()
        .filter(|path| path_names_unit(path, &used))
        .cloned()
        .collect();
    assert!(
        hits.is_empty(),
        "plan listed a unit the build used: {hits:?}\n{planned}"
    );
    let kept = planned["nothing_to_do"]
        .as_array()
        .unwrap()
        .iter()
        .any(|item| item.as_str().is_some_and(|path| Path::new(path) == target));
    assert!(
        kept || !under.is_empty(),
        "neither nothing_to_do nor a stored plan names {}\n{planned}",
        target.display()
    );
}

#[test]
fn liveness_reports_a_seed_with_fresh_stamps_as_live_before_its_first_build() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let lane = repo.worktree("fresh-stamps");
    let (out, receipt) = hive.json(&["seed", lane.to_str().unwrap()]);
    assert!(out.status.success(), "{receipt}");
    assert!(receipt["counts"]["clone_units_cloned"].as_u64().unwrap() > 0);
    let target = lane.join("target").canonicalize().unwrap();
    let (out, planned) = hive.json(&["gc", "--plan"]);
    assert!(out.status.success(), "{planned}");
    assert!(planned["plans"].as_array().unwrap().is_empty(), "{planned}");
    assert!(
        planned["undecided"].as_array().unwrap().is_empty(),
        "{planned}"
    );
    let nothing = planned["nothing_to_do"].as_array().unwrap();
    assert!(
        nothing
            .iter()
            .any(|item| item.as_str().is_some_and(|path| Path::new(path) == target)),
        "{planned}"
    );
}

#[test]
fn gc_still_lists_rcgu_objects_of_a_dead_unit() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let target = repo.main.join("target");
    let before = fingerprint_units(&target);
    backdate_all(&target, Duration::from_secs(30 * 86_400));
    let manifest = repo.main.join("Cargo.toml");
    let text = fs::read_to_string(&manifest).unwrap();
    fs::write(
        &manifest,
        text.replace("version = \"0.1.0\"", "version = \"0.1.1\""),
    )
    .unwrap();
    let built = cargo_json(&hive, &repo.main, &["build"]);
    assert!(
        !built.registry_fresh.is_empty(),
        "{:?}",
        built.registry_fresh
    );
    let after = fingerprint_units(&target);
    let current = hashes_of(&after, "app", true);
    let left: Vec<String> = hashes_of(&before, "app", false)
        .into_iter()
        .filter(|hash| !current.contains(hash))
        .collect();
    assert!(
        !left.is_empty(),
        "version bump left no app unit hash\nbefore={before:?}\nafter={after:?}"
    );
    let fresh_hashes = hashes_named(&after, &built.registry_fresh);
    assert!(
        !fresh_hashes.is_empty(),
        "no fingerprint hash for {:?}\nafter={after:?}",
        built.registry_fresh
    );
    let (out, planned) = hive.json(&["gc", "--plan"]);
    assert!(out.status.success(), "{planned}");
    assert!(
        planned["plans"]
            .as_array()
            .unwrap()
            .iter()
            .any(|plan| plan["paths"].as_u64().unwrap_or(0) > 0),
        "{planned}"
    );
    let stored = plans_stored(&hive);
    let rcgu: Vec<StoredEntry> = stored_entries(&stored)
        .into_iter()
        .filter(|entry| path_under(&entry.path, &target) && entry.path.ends_with(".rcgu.o"))
        .collect();
    assert!(
        rcgu.iter().any(|entry| {
            label_hash(&entry.unit).is_some_and(|hash| left.iter().any(|left| left == hash))
                && file_embeds_any(&entry.path, &left)
        }),
        "stored plan listed no .rcgu.o of a unit the version bump left behind ({left:?}) among {:?}",
        rcgu.iter().map(|entry| &entry.path).collect::<Vec<_>>()
    );
    let live: Vec<&str> = rcgu
        .iter()
        .filter(|entry| {
            label_hash(&entry.unit)
                .is_some_and(|hash| fresh_hashes.iter().any(|fresh| fresh == hash))
                || file_embeds_any(&entry.path, &fresh_hashes)
        })
        .map(|entry| entry.path.as_str())
        .collect();
    assert!(
        live.is_empty(),
        "plan listed a .rcgu.o of a unit still fresh: {live:?}"
    );
}

#[test]
#[ignore = "seeds and builds the checkout named by HIVE_SEED_CHECKOUT; it does not run by default"]
fn seeding_a_new_worktree_of_a_real_repo_recompiles_no_cloned_unit() {
    let checkout = PathBuf::from(
        std::env::var_os("HIVE_SEED_CHECKOUT").expect("HIVE_SEED_CHECKOUT names the checkout"),
    );
    let checkout = checkout
        .canonicalize()
        .unwrap_or_else(|err| panic!("{}: {err}", checkout.display()));
    assert!(
        checkout.join("Cargo.toml").is_file(),
        "{} has no Cargo.toml",
        checkout.display()
    );
    assert!(
        source_target_is_built(&checkout),
        "{} has no built source target",
        checkout.display()
    );
    let homes_before = read_home_stamps().unwrap_or_else(|err| panic!("{err}"));
    let hive = Hive::ready();
    quiet_cache(&hive);
    hive.config(&format!(
        r#"{{"secondary_roots":["{}"]}}"#,
        hive.home.path.display()
    ));
    register(&hive, &checkout);
    let lane_path = checkout
        .join(".worktrees")
        .join(format!("hive-seed-{}", std::process::id()));
    assert!(
        !lane_path.exists(),
        "{} already exists",
        lane_path.display()
    );
    let lane = LinkedWorktree::prepare(&checkout, &lane_path);
    git_in(
        &hive.home.path,
        &checkout,
        &[
            "worktree",
            "add",
            "--detach",
            lane.path.to_str().unwrap(),
            "HEAD",
        ],
        "worktree-add",
        Duration::from_secs(30 * 60),
    );
    assert!(
        !lane.path.join("target").exists(),
        "the new worktree already has a target"
    );
    let seed_started = Instant::now();
    let seed_status = real_cargo_hive(
        &hive,
        &["seed", lane.path.to_str().unwrap(), "--json"],
        "seed",
        Duration::from_secs(8 * 60 * 60),
    );
    let seed_took = seed_started.elapsed();
    copy_run(&hive, "seed");
    let receipt = json_file(&hive.home.path.join("seed.out"), 32 * 1024 * 1024);
    write_kept(
        "seed-receipt.json",
        &serde_json::to_vec_pretty(&receipt).unwrap(),
    );
    assert!(
        seed_status.success(),
        "seed refused ({}): {}",
        seed_status,
        tail(&hive.home.path.join("seed.err"))
    );
    assert!(
        receipt["counts"]["clone_bytes_not_shared"]
            .as_u64()
            .is_some(),
        "{receipt}"
    );
    let rmeta = placed_deps_rmeta(&lane.path.join("target"));
    assert!(
        !rmeta.is_empty(),
        "no rmeta file was placed under {}",
        lane.path.join("target").display()
    );
    let compressed: Vec<_> = rmeta
        .iter()
        .filter(|(_, flags)| flags & libc::UF_COMPRESSED != 0)
        .map(|(path, _)| path.display().to_string())
        .collect();
    assert!(
        compressed.is_empty(),
        "a placed rmeta still has the compressed-file flag: {compressed:?}"
    );
    assert!(
        receipt["counts"]["clone_units_cloned"]
            .as_u64()
            .unwrap_or(0)
            > 0,
        "{receipt}"
    );
    let hashes = cloned_unit_hashes(&hive, &lane.path);
    assert!(!hashes.is_empty(), "the seed record lists no unit");
    let build_started = Instant::now();
    let build_status = cargo_build_locked(&lane.path, &hive.home.path);
    let build_took = build_started.elapsed();
    copy_run(&hive, "cargo-build");
    assert!(
        build_status.success(),
        "cargo build failed: {}",
        tail(&hive.home.path.join("cargo-build.err"))
    );
    let compiled = cargo_artifact_counts(&hive.home.path.join("cargo-build.out"), &hashes);
    write_kept(
        "cargo-counts.json",
        serde_json::to_vec_pretty(&serde_json::json!({
            "registry_fresh": compiled.registry_fresh,
            "registry_recompiled": compiled.registry_recompiled,
            "other_fresh": compiled.other_fresh,
            "other_recompiled": compiled.other_recompiled,
            "cloned_recompiled": compiled.cloned_recompiled,
        }))
        .unwrap()
        .as_slice(),
    );
    assert!(
        compiled.cloned_recompiled.is_empty(),
        "a cloned unit recompiled: {:?}",
        compiled.cloned_recompiled
    );
    let package = one_lib_package(&lane.path);
    let later = [
        (
            "cargo-check",
            vec!["check", "--all-targets", "--offline", "--locked"],
        ),
        (
            "cargo-clippy",
            vec!["clippy", "--all-targets", "--offline", "--locked"],
        ),
        (
            "cargo-test",
            vec!["test", "--offline", "--locked", "--no-run"],
        ),
        (
            "cargo-check-lib",
            vec![
                "check",
                "-p",
                package.as_str(),
                "--lib",
                "--offline",
                "--locked",
            ],
        ),
    ];
    let mut later_counts = serde_json::Map::new();
    for (label, args) in &later {
        let status = cargo_locked(&lane.path, &hive.home.path, label, args);
        copy_run(&hive, label);
        assert!(
            status.success(),
            "{label} failed: {}",
            tail(&hive.home.path.join(format!("{label}.err")))
        );
        let counted = cargo_artifact_counts(&hive.home.path.join(format!("{label}.out")), &hashes);
        write_kept(
            &format!("{label}-counts.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "cloned_recompiled": counted.cloned_recompiled,
            }))
            .unwrap()
            .as_slice(),
        );
        assert!(
            counted.cloned_recompiled.is_empty(),
            "{label} recompiled a cloned unit: {:?}",
            counted.cloned_recompiled
        );
        later_counts.insert(
            (*label).to_string(),
            serde_json::json!(counted.cloned_recompiled),
        );
    }
    let status_started = Instant::now();
    let status_code = real_cargo_hive(
        &hive,
        &["status", checkout.to_str().unwrap(), "--no-sizes", "--json"],
        "status",
        Duration::from_secs(2 * 60 * 60),
    );
    let status_took = status_started.elapsed();
    copy_run(&hive, "status");
    let status_receipt = json_file(&hive.home.path.join("status.out"), 64 * 1024 * 1024);
    write_kept(
        "status-receipt.json",
        &serde_json::to_vec_pretty(&status_receipt).unwrap(),
    );
    assert!(
        status_code.success(),
        "status failed: {}",
        tail(&hive.home.path.join("status.err"))
    );
    let counts = lane_status_counts(&status_receipt, &lane.path);
    let homes_after = read_home_stamps().unwrap_or_else(|err| panic!("{err}"));
    let summary = serde_json::json!({
        "seed_secs": seed_took.as_secs(),
        "build_secs": build_took.as_secs(),
        "status_secs": status_took.as_secs(),
        "receipt_counts": receipt["counts"],
        "receipt_unmeasured": receipt["unmeasured"],
        "findings": receipt["findings"].as_array().map(Vec::len).unwrap_or(0),
        "clone_units_dropped": receipt["counts"]["clone_units_dropped"],
        "name_metas_with_several_fingerprints": name_metas_with_several_fingerprints(&hive),
        "cargo": {
            "registry_fresh": compiled.registry_fresh,
            "registry_recompiled": compiled.registry_recompiled,
            "other_fresh": compiled.other_fresh,
            "other_recompiled": compiled.other_recompiled,
            "cloned_recompiled": compiled.cloned_recompiled,
        },
        "later_cargo": later_counts,
        "status_counts": counts,
        "homes_before": home_stamp_json(&homes_before),
        "homes_after": home_stamp_json(&homes_after),
    });
    write_kept(
        "summary.json",
        &serde_json::to_vec_pretty(&summary).unwrap(),
    );
    eprintln!("{summary}");
    cargo_home_left_unchanged(&homes_before, &homes_after).unwrap_or_else(|err| panic!("{err}"));
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

fn assert_early_finish_bytes(receipt: &serde_json::Value) {
    assert_eq!(receipt["counts"]["clone_bytes_not_shared"], 0, "{receipt}");
    assert!(
        receipt["counts"]["store_bytes"].as_u64().is_some(),
        "{receipt}"
    );
    let fields: Vec<_> = receipt["unmeasured"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|row| row["field"].as_str())
        .collect();
    assert!(
        !fields.contains(&"clone_bytes_not_shared") && !fields.contains(&"store_bytes"),
        "{receipt}"
    );
}

fn quiet_cache(hive: &Hive) {
    fs::create_dir_all(hive.home.path.join("no-cache")).unwrap();
}

fn placed_deps_rmeta(target: &Path) -> Vec<(PathBuf, u32)> {
    fn walk(dir: &Path, out: &mut Vec<(PathBuf, u32)>, depth: usize) {
        assert!(depth <= 16, "walk bound hit");
        assert!(out.len() < 100_000, "walk bound hit");
        let mut seen = 0usize;
        for entry in fs::read_dir(dir).unwrap_or_else(|err| panic!("{}: {err}", dir.display())) {
            seen += 1;
            assert!(seen < 100_000, "walk bound hit");
            let path = entry.unwrap().path();
            let meta = fs::symlink_metadata(&path)
                .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
            if meta.is_dir() {
                walk(&path, out, depth + 1);
                continue;
            }
            if meta.is_file()
                && path.extension().is_some_and(|ext| ext == "rmeta")
                && path
                    .components()
                    .any(|component| component.as_os_str() == "deps")
            {
                out.push((path, meta.st_flags()));
            }
        }
    }
    let mut out = Vec::new();
    walk(target, &mut out, 0);
    out
}

fn nothing_filled(hive: &Hive) -> bool {
    !hive.home.path.join("store").exists() && !hive.home.path.join("store.db").exists()
}

fn plan_for<'a>(receipt: &'a serde_json::Value, target: &Path) -> &'a serde_json::Value {
    let want = target.canonicalize().unwrap();
    receipt["plans"]
        .as_array()
        .unwrap()
        .iter()
        .find(|plan| {
            plan["targets"]
                .as_array()
                .unwrap()
                .iter()
                .any(|path| path.as_str().is_some_and(|path| Path::new(path) == want))
        })
        .unwrap_or_else(|| panic!("no plan for {} in {receipt}", want.display()))
}

/// The stored plans file, read the way `Planned::plans_json` reads it.
fn plans_stored(hive: &Hive) -> serde_json::Value {
    serde_json::from_slice(&fs::read(hive.home.path.join("plans.json")).unwrap()).unwrap()
}

fn stored_paths(stored: &serde_json::Value) -> Vec<String> {
    let mut paths = Vec::new();
    let plans = stored["plans"].as_array().unwrap();
    assert!(plans.len() < 100_000, "walk bound hit");
    for plan in plans {
        let targets = plan["targets"].as_array().unwrap();
        assert!(targets.len() < 100_000, "walk bound hit");
        for target in targets {
            let entries = target["entries"].as_array().unwrap();
            assert!(entries.len() < 100_000, "walk bound hit");
            for entry in entries {
                paths.push(
                    entry["path"]
                        .as_str()
                        .unwrap_or_else(|| panic!("entry path is not a string: {entry}"))
                        .to_string(),
                );
            }
        }
    }
    paths
}

fn seeded_dir_names(hive: &Hive, target: &Path) -> Vec<String> {
    let target = target.canonicalize().unwrap();
    let state = cargo_hive::state::State::open_at(hive.home.path.clone()).unwrap();
    let file = cargo_hive::seed::record::read(&state).unwrap();
    let record = file
        .targets
        .get(&target)
        .unwrap_or_else(|| panic!("no seed record for {}", target.display()));
    record
        .units
        .iter()
        .map(|unit| unit.dir_name.clone())
        .collect()
}

fn path_under(path: &str, root: &Path) -> bool {
    let root = root.canonicalize().unwrap();
    Path::new(path).starts_with(&root)
}

fn path_names_unit(path: &str, dir_names: &[String]) -> bool {
    Path::new(path).components().any(|part| {
        dir_names
            .iter()
            .any(|dir_name| part.as_os_str() == dir_name.as_str())
    })
}

fn dirs_stamped_within(target: &Path, window: Duration) -> Vec<String> {
    let cutoff = SystemTime::now().checked_sub(window).unwrap_or(UNIX_EPOCH);
    let mut names = Vec::new();
    for profile in common::cargo_fixture::profile_dirs(target) {
        for sub in [".fingerprint", "build"] {
            let dir = profile.join(sub);
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };
            let mut seen = 0usize;
            for entry in entries {
                seen += 1;
                assert!(seen < 100_000, "walk bound hit");
                let path = entry.unwrap().path();
                let stamp = path.join("invoked.timestamp");
                let Ok(meta) = fs::metadata(&stamp) else {
                    continue;
                };
                if meta.modified().unwrap() >= cutoff {
                    names.push(path.file_name().unwrap().to_string_lossy().into_owned());
                }
            }
        }
    }
    names
}

#[derive(Debug)]
struct FingerprintUnit {
    name: String,
    fresh: bool,
}

fn fingerprint_units(target: &Path) -> Vec<FingerprintUnit> {
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(7 * 86_400))
        .unwrap_or(UNIX_EPOCH);
    let mut units = Vec::new();
    for profile in common::cargo_fixture::profile_dirs(target) {
        let dir = profile.join(".fingerprint");
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut seen = 0usize;
        for entry in entries {
            seen += 1;
            assert!(seen < 100_000, "walk bound hit");
            let path = entry.unwrap().path();
            if !path.is_dir() {
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            let fresh = fs::metadata(path.join("invoked.timestamp"))
                .and_then(|meta| meta.modified())
                .is_ok_and(|modified| modified >= cutoff);
            units.push(FingerprintUnit { name, fresh });
        }
    }
    units
}

fn package_name(dir_name: &str) -> Option<String> {
    dir_name
        .rsplit_once('-')
        .map(|(name, _)| name.replace('-', "_"))
}

fn hashes_of(units: &[FingerprintUnit], package: &str, fresh_only: bool) -> Vec<String> {
    let mut hashes = Vec::new();
    for unit in units {
        if fresh_only && !unit.fresh {
            continue;
        }
        if package_name(&unit.name).as_deref() != Some(package) {
            continue;
        }
        let Some(hash) = cargo_hive::fingerprint::unit_hash(&unit.name) else {
            continue;
        };
        if !hashes.iter().any(|seen| seen == hash) {
            hashes.push(hash.to_string());
        }
    }
    hashes
}

fn hashes_named(units: &[FingerprintUnit], packages: &[String]) -> Vec<String> {
    let mut hashes = Vec::new();
    for package in packages {
        for hash in hashes_of(units, package, false) {
            if !hashes.contains(&hash) {
                hashes.push(hash);
            }
        }
    }
    hashes
}

struct StoredEntry {
    path: String,
    unit: String,
}

fn stored_entries(stored: &serde_json::Value) -> Vec<StoredEntry> {
    let mut entries = Vec::new();
    let plans = stored["plans"].as_array().unwrap();
    assert!(plans.len() < 100_000, "walk bound hit");
    for plan in plans {
        let targets = plan["targets"].as_array().unwrap();
        assert!(targets.len() < 100_000, "walk bound hit");
        for target in targets {
            let rows = target["entries"].as_array().unwrap();
            assert!(rows.len() < 100_000, "walk bound hit");
            for entry in rows {
                entries.push(StoredEntry {
                    path: entry["path"]
                        .as_str()
                        .unwrap_or_else(|| panic!("entry path is not a string: {entry}"))
                        .to_string(),
                    unit: entry["unit"]
                        .as_str()
                        .unwrap_or_else(|| panic!("entry unit is not a string: {entry}"))
                        .to_string(),
                });
            }
        }
    }
    entries
}

fn label_hash(unit: &str) -> Option<&str> {
    let name = Path::new(unit).file_name()?.to_str()?;
    cargo_hive::fingerprint::unit_hash(name)
}

fn embedded_hashes(name: &str) -> impl Iterator<Item = &str> {
    name.match_indices('-').filter_map(move |(at, _)| {
        let tail = &name[at + 1..];
        let hash = tail.get(..16)?;
        let rest = &tail[16..];
        let boundary = rest.is_empty() || rest.starts_with('.');
        (boundary
            && hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
        .then_some(hash)
    })
}

fn file_embeds_any(path: &str, hashes: &[String]) -> bool {
    let Some(name) = Path::new(path).file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    embedded_hashes(name).any(|hash| hashes.iter().any(|want| want == hash))
}

fn file_count(root: &Path) -> usize {
    fn walk(dir: &Path, count: &mut usize, depth: usize) {
        assert!(depth < 64, "walk bound hit");
        let mut seen = 0usize;
        for entry in fs::read_dir(dir).unwrap() {
            seen += 1;
            assert!(seen < 100_000, "walk bound hit");
            let entry = entry.unwrap();
            let meta = fs::symlink_metadata(entry.path()).unwrap();
            if meta.is_dir() {
                walk(&entry.path(), count, depth + 1);
            } else {
                *count += 1;
            }
        }
    }
    let mut count = 0usize;
    walk(root, &mut count, 0);
    count
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

/// Lock holders that are killed when the test drops them, pass or fail.
struct Held(Vec<Child>);

impl Drop for Held {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn hold_build_locks(hive: &Hive, root: &Path) -> Held {
    let mut held = Held(Vec::new());
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
                held.0.push(hive.hold_lock_child("flock", &path));
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
    assert!(!held.0.is_empty(), "no build lock under {}", root.display());
    held
}

/// An rlib in a published store entry (outside `.tmp`).
fn published_rlib(store: &Path) -> bool {
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
        if rlib_under(&top, 0) {
            return true;
        }
    }
    false
}

fn rlib_under(dir: &Path, depth: usize) -> bool {
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
        if meta.is_dir() && rlib_under(&path, depth + 1) {
            return true;
        }
        if meta.is_file() && path.extension().is_some_and(|ext| ext == "rlib") {
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

fn locate(lib: &LibCtx, lane: &Path) -> (Repo, Checkout) {
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
    matched.expect("the lane is a discovered checkout")
}

fn lib_ctx<'a>(lib: &'a LibCtx, repos: &'a [Repo]) -> Ctx<'a> {
    Ctx {
        state: &lib.state,
        fs: lib.fs.as_ref(),
        cfg: &lib.cfg,
        now: SystemTime::now(),
        account_bytes: true,
        repos,
        mains: &lib.mains,
    }
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
    let (repo, checkout) = locate(&lib, &lane);
    let toolchain = cargo_hive::doctor::resolve_toolchain(&lane).unwrap();
    let repos = [repo];
    let ctx = lib_ctx(&lib, &repos);
    cargo_hive::seed::run(&ctx, &repos[0], &checkout, &toolchain, cancel)
}

#[test]
fn seeding_through_the_fill_and_clone_halves_reports_what_one_run_reports() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let whole = run_seed(&hive, Recording::over_real(), &repo.worktree("whole")).unwrap();
    let lane = repo.worktree("halves");
    let halves = {
        let _env = env_lock().lock().unwrap_or_else(|err| err.into_inner());
        let _home = CargoHomeGuard::set(&hive.cargo_home.path);
        let lib = LibCtx::open(&hive, Recording::over_real().fs());
        let (repo_row, checkout) = locate(&lib, &lane);
        let toolchain = cargo_hive::doctor::resolve_toolchain(&lane).unwrap();
        let repos = [repo_row];
        let ctx = lib_ctx(&lib, &repos);
        match cargo_hive::seed::run_fill(&ctx, &repos[0], &|| false).unwrap() {
            cargo_hive::seed::Fill::Continue(filled) => {
                cargo_hive::seed::run_clone(&ctx, &repos[0], &checkout, &toolchain, filled, &|| {
                    false
                })
                .unwrap()
            }
            cargo_hive::seed::Fill::Stopped(_) => panic!("nothing cancelled this fill"),
        }
    };
    let keys = |o: &cargo_hive::seed::Outcome| o.counts.keys().cloned().collect::<Vec<_>>();
    assert_eq!(keys(&whole), keys(&halves));
    assert_eq!(
        whole.counts["clone_units_cloned"],
        halves.counts["clone_units_cloned"]
    );
    assert!(!halves.cancelled);
}

#[test]
fn a_fill_half_holds_the_store_lock_until_clone_finishes() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let lane = repo.worktree("fill-holds-lock");
    let _env = env_lock().lock().unwrap_or_else(|err| err.into_inner());
    let _home = CargoHomeGuard::set(&hive.cargo_home.path);
    let lib = LibCtx::open(&hive, Recording::over_real().fs());
    let lane = lane.canonicalize().unwrap();
    let (repo_row, checkout) = locate(&lib, &lane);
    let toolchain = cargo_hive::doctor::resolve_toolchain(&lane).unwrap();
    let repos = [repo_row];
    let ctx = lib_ctx(&lib, &repos);
    let filled = match cargo_hive::seed::run_fill(&ctx, &repos[0], &|| false).unwrap() {
        cargo_hive::seed::Fill::Continue(filled) => filled,
        cargo_hive::seed::Fill::Stopped(_) => panic!("nothing cancelled this fill"),
    };
    assert!(
        lib.state.try_store_exclusive().unwrap().is_none(),
        "fill half must keep the shared store lock so gc cannot evict"
    );
    cargo_hive::seed::run_clone(&ctx, &repos[0], &checkout, &toolchain, filled, &|| false).unwrap();
    assert!(
        lib.state.try_store_exclusive().unwrap().is_some(),
        "clone must release the store lock"
    );
}

struct LinkedWorktree {
    main: PathBuf,
    path: PathBuf,
}

impl LinkedWorktree {
    fn prepare(main: &Path, path: &Path) -> LinkedWorktree {
        LinkedWorktree {
            main: main.to_path_buf(),
            path: path.to_path_buf(),
        }
    }
}

impl Drop for LinkedWorktree {
    fn drop(&mut self) {
        let mut remove = Command::new("git");
        remove
            .args([
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .arg("-C")
            .arg(&self.main)
            .args(["worktree", "remove", "--force", "--"])
            .arg(&self.path);
        let _ = status_within(&mut remove, Duration::from_secs(10 * 60));
        let _ = fs::remove_dir_all(&self.path);
        let mut prune = Command::new("git");
        prune
            .args(["-c", "core.fsmonitor=false"])
            .arg("-C")
            .arg(&self.main)
            .args(["worktree", "prune"]);
        let _ = status_within(&mut prune, Duration::from_secs(60));
    }
}

struct CargoArtifacts {
    registry_fresh: u64,
    registry_recompiled: u64,
    other_fresh: u64,
    other_recompiled: u64,
    cloned_recompiled: Vec<String>,
}

fn source_target_is_built(checkout: &Path) -> bool {
    let Ok(profiles) = fs::read_dir(checkout.join("target")) else {
        return false;
    };
    let mut seen = 0usize;
    for profile in profiles {
        seen += 1;
        assert!(seen < 100_000, "walk bound hit");
        let Ok(profile) = profile else {
            continue;
        };
        let Ok(units) = fs::read_dir(profile.path().join(".fingerprint")) else {
            continue;
        };
        let mut units_seen = 0usize;
        for unit in units {
            units_seen += 1;
            assert!(units_seen < 100_000, "walk bound hit");
            if unit.is_ok() {
                return true;
            }
        }
    }
    false
}

fn real_cargo_hive(hive: &Hive, args: &[&str], label: &str, deadline: Duration) -> ExitStatus {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_cargo-hive"));
    cmd.args(args)
        .env("HIVE_HOME", &hive.home.path)
        .env_remove("CARGO_HOME")
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR")
        .env_remove("CARGO_BUILD_BUILD_DIR")
        .env("RUSTUP_AUTO_INSTALL", "0");
    finish_to_files(&mut cmd, &hive.home.path, label, deadline)
}

fn cargo_build_locked(dir: &Path, scratch: &Path) -> ExitStatus {
    cargo_locked(
        dir,
        scratch,
        "cargo-build",
        &["build", "--offline", "--locked"],
    )
}

fn cargo_locked(dir: &Path, scratch: &Path, label: &str, args: &[&str]) -> ExitStatus {
    let mut cmd = Command::new("cargo");
    cmd.args(args)
        .arg("--message-format=json-render-diagnostics")
        .current_dir(dir)
        .env_remove("CARGO_HOME")
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR")
        .env_remove("CARGO_BUILD_BUILD_DIR")
        .env_remove("CARGO_MANIFEST_DIR")
        .env_remove("CARGO_MANIFEST_PATH")
        .env_remove("CARGO_CRATE_NAME")
        .env_remove("CARGO_PRIMARY_PACKAGE")
        .env_remove("OUT_DIR")
        .env("RUSTUP_AUTO_INSTALL", "0");
    // A build script that recorded these unset rebuilds when this process inherited them.
    // `CARGO`, the cargo binary path, stays.
    let mut seen = 0usize;
    for (key, _) in std::env::vars_os() {
        seen += 1;
        assert!(seen < 100_000, "walk bound hit");
        if key.to_string_lossy().starts_with("CARGO_PKG_") {
            cmd.env_remove(&key);
        }
    }
    finish_to_files(&mut cmd, scratch, label, Duration::from_secs(4 * 60 * 60))
}

fn one_lib_package(checkout: &Path) -> String {
    let manifest = fs::read_to_string(checkout.join("Cargo.toml"))
        .unwrap_or_else(|err| panic!("{}: {err}", checkout.display()));
    assert!(manifest.len() < 16 * 1024 * 1024, "walk bound hit");
    let mut members = Vec::new();
    let mut in_members = false;
    let mut lines = 0usize;
    for line in manifest.lines() {
        lines += 1;
        assert!(lines < 100_000, "walk bound hit");
        let trimmed = line.trim();
        if !in_members {
            if trimmed.starts_with("members") && trimmed.contains('[') {
                in_members = true;
                if let Some((_, rest)) = trimmed.split_once('[') {
                    push_quoted(&mut members, rest);
                    if rest.contains(']') {
                        in_members = false;
                    }
                }
            }
            continue;
        }
        push_quoted(&mut members, trimmed);
        if trimmed.contains(']') {
            in_members = false;
        }
    }
    assert!(!members.is_empty(), "the workspace names no member");
    assert!(members.len() < 100_000, "walk bound hit");
    for member in &members {
        let dir = checkout.join(member);
        let member_manifest = fs::read_to_string(dir.join("Cargo.toml")).unwrap_or_default();
        let has_lib = dir.join("src/lib.rs").is_file()
            || member_manifest.lines().any(|line| line.trim() == "[lib]");
        if !has_lib {
            continue;
        }
        return manifest_package_name(&member_manifest).unwrap_or_else(|| {
            Path::new(member)
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(member)
                .to_string()
        });
    }
    panic!("the workspace has no library package among {members:?}");
}

fn push_quoted(out: &mut Vec<String>, text: &str) {
    let mut rest = text;
    while let Some(start) = rest.find('"') {
        rest = &rest[start + 1..];
        let Some(end) = rest.find('"') else {
            break;
        };
        out.push(rest[..end].to_string());
        rest = &rest[end + 1..];
    }
}

fn manifest_package_name(manifest: &str) -> Option<String> {
    let mut in_package = false;
    let mut lines = 0usize;
    for line in manifest.lines() {
        lines += 1;
        assert!(lines < 100_000, "walk bound hit");
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        let Some(rest) = trimmed.strip_prefix("name") else {
            continue;
        };
        let rest = rest.trim().trim_start_matches('=').trim();
        let name = rest.trim_matches(|c| c == '"' || c == '\'');
        if !name.is_empty() && !name.contains(' ') {
            return Some(name.to_string());
        }
    }
    None
}

fn git_in(scratch: &Path, dir: &Path, args: &[&str], label: &str, deadline: Duration) {
    let mut cmd = Command::new("git");
    cmd.args([
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.hooksPath=/dev/null",
    ])
    .arg("-C")
    .arg(dir)
    .args(args)
    .env("GIT_TERMINAL_PROMPT", "0");
    let status = finish_to_files(&mut cmd, scratch, label, deadline);
    assert!(
        status.success(),
        "{label} failed: {}",
        tail(&scratch.join(format!("{label}.err")))
    );
}

fn finish_to_files(cmd: &mut Command, dir: &Path, label: &str, deadline: Duration) -> ExitStatus {
    let stdout = fs::File::create(dir.join(format!("{label}.out"))).unwrap();
    let stderr = fs::File::create(dir.join(format!("{label}.err"))).unwrap();
    let child = cmd
        .stdin(Stdio::null())
        .stdout(stdout)
        .stderr(stderr)
        .spawn()
        .unwrap_or_else(|err| panic!("cannot start {label}: {err}"));
    wait_within(child, deadline).status
}

fn status_within(cmd: &mut Command, limit: Duration) -> Option<ExitStatus> {
    let mut child = match cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return None,
    };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Some(status),
            Ok(None) if started.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return None,
        }
    }
}

fn kept_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/klynt-run");
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_kept(name: &str, bytes: &[u8]) {
    assert!(
        bytes.len() <= 512 * 1024 * 1024,
        "{name} is over the write bound"
    );
    fs::write(kept_dir().join(name), bytes).unwrap();
}

fn copy_run(hive: &Hive, label: &str) {
    for ext in ["out", "err"] {
        let name = format!("{label}.{ext}");
        let bytes = read_capped_bytes(&hive.home.path.join(&name), 512 * 1024 * 1024);
        write_kept(&name, &bytes);
    }
}

fn json_file(path: &Path, cap: u64) -> serde_json::Value {
    let bytes = read_capped_bytes(path, cap);
    serde_json::from_slice(&bytes).unwrap_or_else(|err| {
        panic!(
            "{} is not one JSON value ({err}): {}",
            path.display(),
            tail(path)
        )
    })
}

fn read_capped_bytes(path: &Path, cap: u64) -> Vec<u8> {
    let len = fs::metadata(path).unwrap().len();
    assert!(
        len <= cap,
        "{} is {len} bytes, over the {cap} byte cap",
        path.display()
    );
    fs::read(path).unwrap()
}

fn tail(path: &Path) -> String {
    let Ok(mut file) = fs::File::open(path) else {
        return String::new();
    };
    let Ok(len) = file.metadata().map(|meta| meta.len()) else {
        return String::new();
    };
    let start = len.saturating_sub(4_000);
    if file.seek(std::io::SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut bytes = Vec::new();
    if (&mut file).take(4_000).read_to_end(&mut bytes).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    if text.len() <= 4_000 {
        return text.into_owned();
    }
    let mut cut = text.len() - 4_000;
    while cut < text.len() && !text.is_char_boundary(cut) {
        cut += 1;
    }
    text[cut..].to_string()
}

fn cloned_unit_hashes(hive: &Hive, lane: &Path) -> Vec<String> {
    let value = json_file(&hive.home.path.join("seeds.json"), 16 * 1024 * 1024);
    let targets = value["targets"].as_object().unwrap();
    assert!(targets.len() < 100_000, "walk bound hit");
    let mut hashes = Vec::new();
    for (target, record) in targets {
        if !Path::new(target).starts_with(lane) {
            continue;
        }
        let units = record["units"].as_array().unwrap();
        assert!(units.len() < 100_000, "walk bound hit");
        for unit in units {
            let Some(dir_name) = unit["dir_name"].as_str() else {
                continue;
            };
            let Some(hash) = cargo_hive::fingerprint::unit_hash(dir_name) else {
                continue;
            };
            if !hashes.iter().any(|seen| seen == hash) {
                hashes.push(hash.to_string());
            }
        }
    }
    hashes
}

fn name_metas_with_several_fingerprints(hive: &Hive) -> usize {
    let value = json_file(&hive.home.path.join("store.db"), 32 * 1024 * 1024);
    let units = value["units"].as_array().unwrap();
    assert!(units.len() < 1_000_000, "walk bound hit");
    let mut grouped: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for unit in units {
        let Some(name) = unit["name_meta"].as_str() else {
            continue;
        };
        let Some(hash) = unit["fingerprint_hash"].as_str() else {
            continue;
        };
        grouped
            .entry(name.to_string())
            .or_default()
            .insert(hash.to_string());
    }
    grouped.values().filter(|hashes| hashes.len() > 1).count()
}

fn cargo_artifact_counts(path: &Path, hashes: &[String]) -> CargoArtifacts {
    let text = String::from_utf8(read_capped_bytes(path, 512 * 1024 * 1024))
        .unwrap_or_else(|err| panic!("{} is not utf-8: {err}", path.display()));
    let mut compiled = CargoArtifacts {
        registry_fresh: 0,
        registry_recompiled: 0,
        other_fresh: 0,
        other_recompiled: 0,
        cloned_recompiled: Vec::new(),
    };
    for (n, line) in text.lines().enumerate() {
        assert!(n < 500_000, "walk bound hit");
        if line.is_empty() {
            continue;
        }
        let message: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|err| panic!("{} line {} is not JSON ({err})", path.display(), n + 1));
        if message["reason"].as_str() != Some("compiler-artifact") {
            continue;
        }
        let package_id = message["package_id"]
            .as_str()
            .unwrap_or_else(|| panic!("compiler-artifact has no package_id"));
        let fresh = message["fresh"]
            .as_bool()
            .unwrap_or_else(|| panic!("compiler-artifact has no fresh flag"));
        let mut files = Vec::new();
        if let Some(names) = message["filenames"].as_array() {
            assert!(names.len() < 10_000, "walk bound hit");
            for name in names {
                if let Some(name) = name.as_str() {
                    files.push(name.to_string());
                }
            }
        }
        if let Some(executable) = message["executable"].as_str() {
            files.push(executable.to_string());
        }
        let cloned = files
            .iter()
            .any(|file| hashes.iter().any(|hash| file_has_hash(file, hash)));
        if !fresh && cloned && compiled.cloned_recompiled.len() < 32 {
            compiled.cloned_recompiled.push(package_id.to_string());
        } else if !fresh && cloned && compiled.cloned_recompiled.len() == 32 {
            compiled.cloned_recompiled.push("and more".to_string());
        }
        match (package_id.starts_with("registry+"), fresh) {
            (true, true) => compiled.registry_fresh += 1,
            (true, false) => compiled.registry_recompiled += 1,
            (false, true) => compiled.other_fresh += 1,
            (false, false) => compiled.other_recompiled += 1,
        }
    }
    compiled
}

fn file_has_hash(file: &str, hash: &str) -> bool {
    file.match_indices(hash).any(|(at, _)| {
        let before = at > 0 && file.as_bytes()[at - 1] == b'-';
        let after = at + hash.len();
        let boundary = after == file.len()
            || matches!(
                file.as_bytes().get(after).copied(),
                Some(b'.' | b'/' | b'-')
            );
        before && boundary
    })
}

fn lane_status_counts(receipt: &serde_json::Value, lane: &Path) -> BTreeMap<String, u64> {
    let counts = receipt["counts"].as_object().unwrap();
    assert!(counts.len() < 100_000, "walk bound hit");
    let mut out = BTreeMap::new();
    for (key, value) in counts {
        let Some((name, path)) = key.split_once(':') else {
            continue;
        };
        if !matches!(
            name,
            "reused_fresh" | "seeded_unneeded" | "seeded_recompiled" | "seeded_missing"
        ) {
            continue;
        }
        if Path::new(path).starts_with(lane) {
            out.insert(
                key.clone(),
                value
                    .as_u64()
                    .unwrap_or_else(|| panic!("{key} is not a count")),
            );
        }
    }
    out
}

#[derive(Clone)]
struct HomeStamps {
    cargo: Option<u128>,
    global_cache: Option<u128>,
    package_cache: Option<u128>,
    rustup: Option<u128>,
    rustup_settings: Option<u128>,
    cache: Option<u128>,
    cargo_children: BTreeMap<String, u128>,
    cache_children: BTreeMap<String, u128>,
}

fn sample_home() -> HomeStamps {
    let mut cargo_children = BTreeMap::new();
    cargo_children.insert(".global-cache".to_string(), 2);
    cargo_children.insert(".package-cache".to_string(), 3);
    cargo_children.insert("registry".to_string(), 4);
    HomeStamps {
        cargo: Some(1),
        global_cache: Some(2),
        package_cache: Some(3),
        rustup: Some(4),
        rustup_settings: Some(5),
        cache: Some(6),
        cargo_children,
        cache_children: BTreeMap::new(),
    }
}

fn edited(home: &HomeStamps, edit: impl FnOnce(&mut HomeStamps)) -> HomeStamps {
    let mut home = home.clone();
    edit(&mut home);
    home
}

const CARGO_CHILD_CAP: usize = 64;

fn cargo_home_left_unchanged(before: &HomeStamps, after: &HomeStamps) -> Result<(), String> {
    if before.cargo_children.len() > CARGO_CHILD_CAP || after.cargo_children.len() > CARGO_CHILD_CAP
    {
        return Err("~/.cargo".to_string());
    }
    same_stamp(
        before.package_cache,
        after.package_cache,
        "~/.cargo/.package-cache",
    )?;
    same_stamp(before.rustup, after.rustup, "~/.rustup")?;
    same_stamp(
        before.rustup_settings,
        after.rustup_settings,
        "~/.rustup/settings.toml",
    )?;
    let mut names = BTreeSet::new();
    names.extend(before.cargo_children.keys().map(String::as_str));
    names.extend(after.cargo_children.keys().map(String::as_str));
    for name in names {
        if name == ".global-cache" {
            continue;
        }
        if before.cargo_children.get(name) != after.cargo_children.get(name) {
            return Err(format!("~/.cargo/{name}"));
        }
    }
    if before.cache_children.len() > CARGO_CHILD_CAP || after.cache_children.len() > CARGO_CHILD_CAP
    {
        return Err("~/.cache".to_string());
    }
    let mut cache_names = BTreeSet::new();
    cache_names.extend(before.cache_children.keys().map(String::as_str));
    cache_names.extend(after.cache_children.keys().map(String::as_str));
    for name in cache_names {
        if name.starts_with("p10k-") {
            continue;
        }
        if before.cache_children.get(name) != after.cache_children.get(name) {
            return Err(format!("~/.cache/{name}"));
        }
    }
    Ok(())
}

fn same_stamp(before: Option<u128>, after: Option<u128>, path: &str) -> Result<(), String> {
    if before == after {
        Ok(())
    } else {
        Err(path.to_string())
    }
}

fn direct_child_mtimes(dir: &Path, label: &str) -> Result<BTreeMap<String, u128>, String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(err) => return Err(format!("{label}: {err}")),
    };
    let mut children = BTreeMap::new();
    for entry in entries {
        let entry = entry.map_err(|err| format!("{label}: {err}"))?;
        if children.len() >= CARGO_CHILD_CAP {
            return Err(label.to_string());
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let meta = entry
            .metadata()
            .map_err(|err| format!("{label}/{name}: {err}"))?;
        let modified = meta
            .modified()
            .map_err(|err| format!("{label}/{name}: {err}"))?;
        children.insert(
            name.into_owned(),
            modified
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        );
    }
    Ok(children)
}

fn mtime_ns(path: &Path) -> Option<u128> {
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    Some(
        modified
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
    )
}

fn read_home_stamps() -> Result<HomeStamps, String> {
    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME is set"));
    let cargo = home.join(".cargo");
    Ok(HomeStamps {
        cargo: mtime_ns(&cargo),
        global_cache: mtime_ns(&cargo.join(".global-cache")),
        package_cache: mtime_ns(&cargo.join(".package-cache")),
        rustup: mtime_ns(&home.join(".rustup")),
        rustup_settings: mtime_ns(&home.join(".rustup").join("settings.toml")),
        cache: mtime_ns(&home.join(".cache")),
        cargo_children: direct_child_mtimes(&cargo, "~/.cargo")?,
        cache_children: direct_child_mtimes(&home.join(".cache"), "~/.cache")?,
    })
}

fn home_stamp_json(stamps: &HomeStamps) -> serde_json::Value {
    serde_json::json!({
        ".cargo": stamps.cargo,
        ".cargo/.global-cache": stamps.global_cache,
        ".cargo/.package-cache": stamps.package_cache,
        ".rustup": stamps.rustup,
        ".rustup/settings.toml": stamps.rustup_settings,
        ".cache": stamps.cache,
        "cargo_children": stamps.cargo_children,
        "cache_children": stamps.cache_children,
    })
}
