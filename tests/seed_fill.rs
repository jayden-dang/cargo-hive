#![cfg(target_os = "macos")]
mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cargo_hive::cargo_target::{Owner, TargetDir};
use cargo_hive::fingerprint::read_profile;
use cargo_hive::gc::{CRATE_DIR_CAP, DEPS_CAP};
use cargo_hive::platform::{Fs, LockKind, LockState};
use cargo_hive::seed::fill::FillReport;
use cargo_hive::seed::source::{live_registry_units, unit_files, SourceUnit};
use cargo_hive::seed::store::{self, tag_path};
use common::plan_fixture::LibCtx;
use common::recording_fs::Recording;
use common::registry_fixture::{cargo_json, RegistryRepo};
use common::Hive;

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

struct Built {
    hive: Hive,
    repo: RegistryRepo,
    target_path: PathBuf,
    target: TargetDir,
}

fn built() -> Built {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let target_path = repo.main.join("target");
    assert!(target_path.join("CACHEDIR.TAG").is_file());
    let target = TargetDir {
        path: target_path.clone(),
        owner: Owner::Owned {
            checkout: repo.main.clone(),
        },
        profiles: vec![read_profile(&target_path.join("debug"))],
        size: None,
        unreadable: None,
    };
    Built {
        hive,
        repo,
        target_path,
        target,
    }
}

fn narrowed(hive: &Hive, fs: Box<dyn Fs>, main: &Path) -> LibCtx {
    let mut lib = LibCtx::open(hive, fs);
    lib.repos.retain(|repo| repo.main == main);
    lib.mains.retain(|path| path == main);
    assert_eq!(lib.repos.len(), 1, "the fill sees one registered repo");
    lib
}

fn fill_in(
    hive: &Hive,
    lib: &LibCtx,
    target: &TargetDir,
    cancel: &(dyn Fn() -> bool + Sync),
) -> FillReport {
    let _env = env_lock().lock().unwrap_or_else(|err| err.into_inner());
    let _home = CargoHomeGuard::set(&hive.cargo_home.path);
    cargo_hive::seed::fill::fill(
        &lib.ctx(),
        &lib.repos[0],
        std::slice::from_ref(target),
        cancel,
    )
    .unwrap_or_else(|err| panic!("fill failed: {err}"))
}

fn registry_units(hive: &Hive, target: &TargetDir) -> Vec<SourceUnit> {
    let fs = cargo_hive::platform::current().expect("macOS").fs;
    let lib = LibCtx::open(hive, fs);
    let window = Duration::from_secs(7 * 86_400);
    let (units, left_out) = live_registry_units(&lib.ctx(), target, &hive.cargo_home.path, window)
        .unwrap_or_else(|err| panic!("units: {err}"));
    assert!(
        left_out.is_empty(),
        "the fixture's live registry units all resolve: {left_out:?}"
    );
    assert!(
        units.len() >= 2,
        "the fixture publishes more than one registry unit, got {}",
        units.len()
    );
    units
}

fn first_clone_dst(hive: &Hive, target: &TargetDir) -> (SourceUnit, PathBuf) {
    let units = registry_units(hive, target);
    let unit = units[0].clone();
    let fs = cargo_hive::platform::current().expect("macOS").fs;
    let profile = target.path.join(&unit.rel_profile);
    let listing = match fs.read_dir_bounded(&profile.join("deps"), DEPS_CAP) {
        Ok(listing) => listing,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(err) => panic!("deps: {err}"),
    };
    let files = unit_files(fs.as_ref(), &target.path, &unit, &listing, CRATE_DIR_CAP)
        .unwrap_or_else(|err| panic!("files: {err}"));
    assert!(!files.is_empty(), "a unit has at least one file");
    let state = cargo_hive::state::State::open_at(hive.home.path.clone()).unwrap();
    let key = format!("{}-{}", unit.dir_name, unit.fingerprint_hash);
    let dst = store::tmp_dir(&state, &key).join(&files[0]);
    (unit, dst)
}

fn store_rows(lib: &LibCtx) -> Vec<store::Row> {
    store::read(&lib.state).unwrap().units
}

fn published(store_root: &Path) -> bool {
    let Ok(top) = std::fs::read_dir(store_root) else {
        return false;
    };
    let mut seen = 0usize;
    for entry in top {
        seen += 1;
        assert!(seen < 100_000, "walk bound hit");
        let entry = entry.unwrap();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == ".tmp" || name == "CACHEDIR.TAG" {
            continue;
        }
        let Ok(inner) = std::fs::read_dir(entry.path()) else {
            continue;
        };
        if inner.flatten().next().is_some() {
            return true;
        }
    }
    false
}

fn between_locks(log: &[String]) {
    let locks: Vec<usize> = log
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with("lock "))
        .map(|(i, _)| i)
        .collect();
    let unlocks: Vec<usize> = log
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with("unlock "))
        .map(|(i, _)| i)
        .collect();
    let clones: Vec<usize> = log
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with("clone "))
        .map(|(i, _)| i)
        .collect();
    let compresses: Vec<usize> = log
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with("compress "))
        .map(|(i, _)| i)
        .collect();
    assert!(!locks.is_empty(), "fill takes the target locks: {log:?}");
    assert!(!unlocks.is_empty(), "fill drops the target locks: {log:?}");
    assert!(!clones.is_empty(), "fill clones: {log:?}");
    assert!(!compresses.is_empty(), "fill compresses: {log:?}");
    let last_lock = *locks.last().unwrap();
    let first_unlock = unlocks[0];
    let last_unlock = *unlocks.last().unwrap();
    assert!(last_lock < first_unlock, "{log:?}");
    for index in clones {
        assert!(
            index > last_lock && index < first_unlock,
            "clone at {index} is outside the locks: {log:?}"
        );
    }
    for index in compresses {
        assert!(
            index > last_unlock,
            "compress at {index} is not after the last unlock: {log:?}"
        );
    }
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
    fn walk(
        dir: &Path,
        root: &Path,
        out: &mut BTreeMap<PathBuf, (Vec<u8>, SystemTime)>,
        depth: usize,
    ) {
        assert!(depth < 64, "walk bound hit");
        let mut seen = 0usize;
        for entry in std::fs::read_dir(dir).unwrap() {
            seen += 1;
            assert!(seen < 100_000, "walk bound hit");
            let path = entry.unwrap().path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            let rel = path.strip_prefix(root).unwrap().to_path_buf();
            let bytes = if meta.file_type().is_symlink() {
                std::fs::read_link(&path)
                    .unwrap()
                    .into_os_string()
                    .into_encoded_bytes()
            } else if meta.is_dir() {
                Vec::new()
            } else {
                assert!(
                    meta.len() <= 32 * 1024 * 1024,
                    "{} is larger than 32 MiB",
                    path.display()
                );
                std::fs::read(&path).unwrap()
            };
            out.insert(rel, (bytes, meta.modified().unwrap()));
            if meta.is_dir() {
                walk(&path, root, out, depth + 1);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out, 0);
    out
}

#[test]
fn a_new_unit_is_cloned_under_the_locks_and_compressed_after_they_are_dropped() {
    let built = built();
    let lock = built.target_path.join("debug/.cargo-build-lock");
    let states = std::sync::Arc::new(Mutex::new(Vec::new()));
    let recorded = std::sync::Arc::clone(&states);
    let recording = Recording::over_real().on_compress(move |_| {
        let fs = cargo_hive::platform::apfs::Apfs::new(env!("CARGO_BIN_EXE_cargo-hive").into());
        let state = fs
            .try_lock_elsewhere(LockKind::Flock, &lock)
            .expect("lock probe");
        recorded.lock().expect("states").push(state);
    });
    let lib = narrowed(&built.hive, recording.fs(), &built.repo.main);
    let report = fill_in(&built.hive, &lib, &built.target, &|| false);
    between_locks(&recording.log());
    let seen = states.lock().expect("states");
    assert!(!seen.is_empty(), "compression probes the build lock");
    assert!(
        seen.iter().all(|state| *state == LockState::Free),
        "the build lock is free while compressing: {seen:?}"
    );
    assert!(report.entries_added >= 1, "{report:?}");
    assert_eq!(report.targets_read, 1);
    assert!(!report.cancelled);
    let rows = store_rows(&lib);
    assert_eq!(rows.len() as u64, report.entries_added);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    for row in &rows {
        assert_eq!(row.repos, vec![built.repo.main.clone()]);
        assert_eq!(row.filled_at, row.last_live_at);
        assert!(row.filled_at.abs_diff(now) < 180, "{}", row.filled_at);
        assert_eq!(row.rel_profile, PathBuf::from("debug"));
        assert!(
            built
                .target_path
                .join("debug/.fingerprint")
                .join(&row.name_meta)
                .is_dir(),
            "name-meta is the unit directory {}",
            row.name_meta
        );
        assert!(store::entry_dir(&lib.state, row).is_dir());
    }
    assert!(rows.iter().any(|row| row.size > 0));
    assert!(tag_path(&lib.state).is_file());
}

#[test]
fn a_held_lock_skips_the_target_and_waits_for_nothing() {
    let built = built();
    let lock = built.target_path.join("debug/.cargo-build-lock");
    let mut child = built.hive.hold_lock_child("flock", &lock);
    let recording = Recording::over_real();
    let lib = narrowed(&built.hive, recording.fs(), &built.repo.main);
    let started = Instant::now();
    let report = fill_in(&built.hive, &lib, &built.target, &|| false);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "a held lock returns without waiting: {:?}",
        started.elapsed()
    );
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(
        report.targets_skipped.get("targets_skipped_held").copied(),
        Some(1),
        "{report:?}"
    );
    assert_eq!(report.entries_added, 0);
    assert_eq!(report.targets_read, 0);
    assert!(store_rows(&lib).is_empty());
    assert!(!tag_path(&lib.state).exists());
    assert!(
        recording
            .log()
            .iter()
            .all(|line| !line.starts_with("clone ") && !line.starts_with("compress ")),
        "{:?}",
        recording.log()
    );
}

#[test]
fn a_target_with_nothing_new_takes_no_lock() {
    let built = built();
    let recording = Recording::over_real();
    let lib = narrowed(&built.hive, recording.fs(), &built.repo.main);
    let first = fill_in(&built.hive, &lib, &built.target, &|| false);
    assert!(first.entries_added >= 1, "{first:?}");
    let mark = recording.log().len();
    let second = fill_in(&built.hive, &lib, &built.target, &|| false);
    let later = &recording.log()[mark..];
    assert!(
        later.iter().all(|line| !line.starts_with("lock ")),
        "a second fill takes no lock: {later:?}"
    );
    assert_eq!(second.entries_added, 0, "{second:?}");
    assert!(second.entries_held >= 1, "{second:?}");
    assert_eq!(store_rows(&lib).len() as u64, first.entries_added);
}

#[test]
fn a_clone_failure_leaves_the_entry_out_with_one_finding() {
    let built = built();
    let (unit, dst) = first_clone_dst(&built.hive, &built.target);
    let recording = Recording::over_real().fail_clone(&dst, libc::EIO);
    let lib = narrowed(&built.hive, recording.fs(), &built.repo.main);
    let report = fill_in(&built.hive, &lib, &built.target, &|| false);
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    let (_, path, detail) = &report.findings[0];
    assert_eq!(path.as_deref(), Some(built.target_path.as_path()));
    assert!(
        detail.contains("Input/output error") || detail.contains("os error 5"),
        "{detail}"
    );
    let rows = store_rows(&lib);
    assert!(rows.iter().all(|row| {
        row.name_meta != unit.dir_name || row.fingerprint_hash != unit.fingerprint_hash
    }));
    assert!(
        !rows.is_empty(),
        "other units of the target are still filled"
    );
    assert!(!report.cancelled);
}

#[test]
fn cross_volume_stops_the_target_with_one_finding() {
    let built = built();
    let (_unit, dst) = first_clone_dst(&built.hive, &built.target);
    let recording = Recording::over_real().fail_clone(&dst, libc::EXDEV);
    let lib = narrowed(&built.hive, recording.fs(), &built.repo.main);
    let report = fill_in(&built.hive, &lib, &built.target, &|| false);
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    let (_, path, detail) = &report.findings[0];
    assert_eq!(path.as_deref(), Some(built.target_path.as_path()));
    assert!(
        detail.contains("Cross-device link") || detail.contains("os error 18"),
        "{detail}"
    );
    assert_eq!(report.entries_added, 0, "{report:?}");
    assert!(store_rows(&lib).is_empty(), "the target publishes nothing");
    assert!(!tag_path(&lib.state).exists());
}

#[test]
fn a_symlink_in_a_source_unit_fills_nothing_for_it() {
    let built = built();
    let units = registry_units(&built.hive, &built.target);
    let unit = &units[0];
    let outside = built.hive.home.path.join("outside-the-target");
    std::fs::write(&outside, b"not-for-the-store").unwrap();
    let link = built
        .target_path
        .join("debug/.fingerprint")
        .join(&unit.dir_name)
        .join("linked");
    std::os::unix::fs::symlink(&outside, &link).unwrap();
    let recording = Recording::over_real();
    let lib = narrowed(&built.hive, recording.fs(), &built.repo.main);
    let report = fill_in(&built.hive, &lib, &built.target, &|| false);
    assert!(
        report
            .findings
            .iter()
            .any(|(_, _, detail)| detail.contains("symbolic link")),
        "{:?}",
        report.findings
    );
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    let rows = store_rows(&lib);
    assert!(rows.iter().all(|row| {
        row.name_meta != unit.dir_name || row.fingerprint_hash != unit.fingerprint_hash
    }));
    assert!(!rows.is_empty(), "the other units are still filled");
    assert_eq!(std::fs::read(&outside).unwrap(), b"not-for-the-store");
    assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
}

#[test]
fn the_source_keeps_its_bytes_and_modification_times() {
    let built = built();
    let before = snapshot(&built.target_path);
    let recording = Recording::over_real();
    let lib = narrowed(&built.hive, recording.fs(), &built.repo.main);
    let report = fill_in(&built.hive, &lib, &built.target, &|| false);
    assert!(report.entries_added >= 1, "{report:?}");
    assert_eq!(snapshot(&built.target_path), before);
    for line in recording.log() {
        if !(line.starts_with("write_file ")
            || line.starts_with("set_mtime ")
            || line.starts_with("create_dir "))
        {
            continue;
        }
        let path = Path::new(line.split_once(' ').unwrap().1);
        assert!(
            !path.starts_with(&built.target_path),
            "fill wrote the source: {line}"
        );
    }
}

#[test]
fn the_tag_is_cloned_once_even_when_no_key_is_new() {
    let built = built();
    let recording = Recording::over_real();
    let lib = narrowed(&built.hive, recording.fs(), &built.repo.main);
    let first = fill_in(&built.hive, &lib, &built.target, &|| false);
    assert!(first.entries_added >= 1, "{first:?}");
    let tag = tag_path(&lib.state);
    let source = std::fs::read(built.target_path.join("CACHEDIR.TAG")).unwrap();
    assert_eq!(std::fs::read(&tag).unwrap(), source);
    std::fs::remove_file(&tag).unwrap();
    let mark = recording.log().len();
    let second = fill_in(&built.hive, &lib, &built.target, &|| false);
    assert_eq!(std::fs::read(&tag).unwrap(), source);
    assert_eq!(second.entries_added, 0, "{second:?}");
    let later = &recording.log()[mark..];
    let locks: Vec<usize> = later
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with("lock "))
        .map(|(i, _)| i)
        .collect();
    let unlocks: Vec<usize> = later
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with("unlock "))
        .map(|(i, _)| i)
        .collect();
    let clones: Vec<usize> = later
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with("clone ") && line.contains("CACHEDIR.TAG"))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(clones.len(), 1, "{later:?}");
    assert!(!locks.is_empty() && !unlocks.is_empty(), "{later:?}");
    assert!(clones[0] > *locks.last().unwrap() && clones[0] < unlocks[0]);
}

#[test]
fn a_seeded_target_no_build_has_touched_counts_as_lock_missing() {
    let built = built();
    for name in [".cargo-build-lock", ".cargo-lock", ".cargo-artifact-lock"] {
        let path = built.target_path.join("debug").join(name);
        if path.exists() {
            std::fs::remove_file(&path).unwrap();
        }
    }
    assert!(built.target_path.join("debug/.fingerprint").is_dir());
    let recording = Recording::over_real();
    let lib = narrowed(&built.hive, recording.fs(), &built.repo.main);
    let report = fill_in(&built.hive, &lib, &built.target, &|| false);
    assert_eq!(
        report
            .targets_skipped
            .get("targets_skipped_lock_missing")
            .copied(),
        Some(1),
        "{report:?}"
    );
    assert_eq!(report.entries_added, 0);
    assert_eq!(report.targets_read, 0);
    assert!(store_rows(&lib).is_empty());
    assert!(
        recording
            .log()
            .iter()
            .all(|line| !line.starts_with("clone ")),
        "{:?}",
        recording.log()
    );
}

#[test]
fn a_cancelled_fill_merges_what_it_published() {
    let built = built();
    let _units = registry_units(&built.hive, &built.target);
    let store_root = built.hive.home.path.join("store");
    let recording = Recording::over_real();
    let lib = narrowed(&built.hive, recording.fs(), &built.repo.main);
    let report = fill_in(&built.hive, &lib, &built.target, &|| published(&store_root));
    assert!(report.cancelled, "{report:?}");
    assert_eq!(report.entries_added, 1, "{report:?}");
    let rows = store_rows(&lib);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!(store::entry_dir(&lib.state, &rows[0]).is_dir());
}
