#![cfg(target_os = "macos")]
mod common;

use std::collections::{BTreeMap, HashSet};
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cargo_hive::cargo_target::{Owner, TargetDir};
use cargo_hive::fingerprint::read_profile;
use cargo_hive::gc::{CRATE_DIR_CAP, DEPS_CAP};
use cargo_hive::liveness::unit_timestamp;
use cargo_hive::platform::apfs::Apfs;
use cargo_hive::platform::{
    CompressOutcome, EntryKind, EntryStat, Fs, LockHolder, LockKind, LockState, Sharing, TempDir,
    TryLock, VolumeInfo,
};
use cargo_hive::seed::fill::FillReport;
use cargo_hive::seed::place::{self, PlaceReport};
use cargo_hive::seed::source::{live_registry_units, unit_files, SourceUnit};
use cargo_hive::seed::store::{self, tag_path};
use cargo_hive::seed::Error;
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
fn a_hash_reread_longer_than_64_bytes_names_the_path_and_skips_the_unit() {
    let built = built();
    let units = registry_units(&built.hive, &built.target);
    let unit = units[0].clone();
    let hash_path = built
        .target_path
        .join("debug/.fingerprint")
        .join(&unit.dir_name)
        .join(&unit.stem);
    std::fs::write(&hash_path, vec![b'a'; 65]).unwrap();
    let recording = Recording::over_real();
    let lib = narrowed(&built.hive, recording.fs(), &built.repo.main);
    let report = fill_in(&built.hive, &lib, &built.target, &|| false);
    assert!(
        report
            .findings
            .iter()
            .any(|(_, _, detail)| { detail.contains(&hash_path.display().to_string()) }),
        "{:?}",
        report.findings
    );
    let rows = store_rows(&lib);
    assert!(rows.iter().all(|row| {
        row.name_meta != unit.dir_name || row.fingerprint_hash != unit.fingerprint_hash
    }));
    assert!(!rows.is_empty(), "the other units are still filled");
}

#[test]
fn a_unit_left_out_for_an_unresolved_edge_is_named_and_the_others_are_filled() {
    let planted = planted_unresolved();
    let recording = Recording::over_real();
    let lib = narrowed(&planted.hive, recording.fs(), &planted.repo);
    let report = fill_in(&planted.hive, &lib, &planted.target, &|| false);
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    let (_, path, detail) = &report.findings[0];
    assert_eq!(path.as_deref(), Some(planted.target_path.as_path()));
    assert!(
        detail.contains(&planted.left_out)
            && detail.contains(&planted.target_path.display().to_string()),
        "{detail}"
    );
    let rows = store_rows(&lib);
    assert!(
        rows.iter().all(|row| row.name_meta != planted.left_out),
        "the left-out unit is not stored: {rows:?}"
    );
    assert!(
        rows.iter().any(|row| row.name_meta == planted.kept),
        "the other unit is still filled: {rows:?} {report:?}"
    );
    assert!(!report.cancelled);
}

struct Planted {
    hive: Hive,
    repo: PathBuf,
    target_path: PathBuf,
    target: TargetDir,
    left_out: String,
    kept: String,
}

fn planted_unresolved() -> Planted {
    let hive = Hive::ready();
    let repo = common::init_repo(&hive.home.path.join("app"), "");
    common::register(&hive, &repo);
    let target_path = hive.home.path.join("target");
    let profile = target_path.join("debug");
    let hour = Duration::from_secs(3600);
    let month = Duration::from_secs(30 * 86_400);
    let open = "a-1111111111111111";
    let learned = "d-4444444444444444";
    let kept = "liba-aaaaaaaaaaaaaaa1";
    let stale = "liba-aaaaaaaaaaaaaaa2";
    plant_unit(&profile, open, "bin-a", 101, &[("liba", 7, 999)], hour);
    plant_unit(&profile, learned, "bin-d", 104, &[("liba", 7, 201)], hour);
    plant_unit(&profile, kept, "lib-liba", 201, &[], month);
    plant_unit(&profile, stale, "lib-liba", 1, &[], month);
    empty_hash(&profile, stale, "lib-liba", month);
    let registry = hive
        .cargo_home
        .path
        .join("registry/src/index.crates.io-1/a-1.0.0/src/lib.rs");
    let registry = registry.display().to_string();
    plant_dep_info(&profile, "bin-a", open, &registry);
    plant_dep_info(&profile, "bin-d", learned, "src/lib.rs");
    plant_dep_info(&profile, "lib-liba", kept, &registry);
    std::fs::write(profile.join(".cargo-build-lock"), b"lock").unwrap();
    std::fs::write(
        target_path.join("CACHEDIR.TAG"),
        format!(
            "Signature: 8a477f597d28d172789f06886806bc55\n{}\n",
            cargo_hive::cargo_target::TAG_LINE
        ),
    )
    .unwrap();
    plant_rustc_info(&target_path, &repo);
    let profile_report = read_profile(&profile);
    assert!(
        matches!(
            profile_report.format,
            cargo_hive::fingerprint::Format::Supported
        ),
        "{:?}",
        profile_report.format
    );
    let target = TargetDir {
        path: target_path.clone(),
        owner: Owner::Owned {
            checkout: repo.clone(),
        },
        profiles: vec![profile_report],
        size: None,
        unreadable: None,
    };
    Planted {
        hive,
        repo,
        target_path,
        target,
        left_out: open.to_string(),
        kept: kept.to_string(),
    }
}

fn plant_unit(
    profile: &Path,
    name: &str,
    stem: &str,
    fp: u64,
    deps: &[(&str, u64, u64)],
    age: Duration,
) {
    let dir = profile.join(".fingerprint").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(stem), cargo_hive::fingerprint::le_hex(fp)).unwrap();
    let body: Vec<String> = deps
        .iter()
        .map(|(dep, package, hash)| format!("[{package},\"{dep}\",false,{hash}]"))
        .collect();
    std::fs::write(
        dir.join(format!("{stem}.json")),
        format!(
            "{{\"rustc\":1,\"features\":\"[]\",\"declared_features\":\"[]\",\"target\":2,\"profile\":3,\"path\":4,\"deps\":[{}],\"local\":[],\"rustflags\":[],\"config\":5,\"compile_kind\":0}}",
            body.join(",")
        ),
    )
    .unwrap();
    let kind = cargo_hive::fingerprint::kind_of(stem);
    let stamp = cargo_hive::liveness::unit_timestamp(profile, name, &kind);
    std::fs::write(&stamp, b"").unwrap();
    let when = SystemTime::now() - age;
    std::fs::File::options()
        .write(true)
        .open(&stamp)
        .unwrap()
        .set_modified(when)
        .unwrap();
}

fn empty_hash(profile: &Path, name: &str, stem: &str, age: Duration) {
    let hash = profile.join(".fingerprint").join(name).join(stem);
    std::fs::write(&hash, b"").unwrap();
    std::fs::File::options()
        .write(true)
        .open(&hash)
        .unwrap()
        .set_modified(SystemTime::now() - age)
        .unwrap();
}

fn plant_dep_info(profile: &Path, stem: &str, dir_name: &str, first_input: &str) {
    let meta = cargo_hive::fingerprint::unit_hash(dir_name).unwrap();
    let kind = cargo_hive::fingerprint::kind_of(stem);
    let crate_name = stem
        .strip_prefix(&format!("{kind}-"))
        .filter(|name| !name.is_empty())
        .unwrap_or(stem);
    let deps = profile.join("deps");
    std::fs::create_dir_all(&deps).unwrap();
    let name = format!("{crate_name}-{meta}.d");
    std::fs::write(deps.join(name), format!("out: {first_input}\n")).unwrap();
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
    assert!(
        tag_path(&lib.state).is_file(),
        "cancel still clones the tag when it was absent"
    );
}

#[test]
fn a_second_fill_moves_last_live_at_and_leaves_filled_at() {
    let built = built();
    let recording = Recording::over_real();
    let lib = narrowed(&built.hive, recording.fs(), &built.repo.main);
    let first = fill_in(&built.hive, &lib, &built.target, &|| false);
    assert!(first.entries_added >= 1, "{first:?}");
    let before = store_rows(&lib);
    std::thread::sleep(Duration::from_secs(2));
    let second = fill_in(&built.hive, &lib, &built.target, &|| false);
    assert_eq!(second.entries_added, 0, "{second:?}");
    assert!(second.entries_held >= 1, "{second:?}");
    let after = store_rows(&lib);
    assert_eq!(after.len(), before.len());
    for row in &after {
        let prev = before
            .iter()
            .find(|kept| {
                kept.name_meta == row.name_meta && kept.fingerprint_hash == row.fingerprint_hash
            })
            .unwrap();
        assert_eq!(row.filled_at, prev.filled_at, "{row:?}");
        assert!(
            row.last_live_at > prev.last_live_at,
            "last_live_at moved: before {} after {} ({})",
            prev.last_live_at,
            row.last_live_at,
            row.name_meta
        );
    }
}

#[test]
fn a_final_directory_with_no_row_is_held_and_adopted() {
    let built = built();
    let recording = Recording::over_real();
    let lib = narrowed(&built.hive, recording.fs(), &built.repo.main);
    let first = fill_in(&built.hive, &lib, &built.target, &|| false);
    assert!(first.entries_added >= 1, "{first:?}");
    let before = store_rows(&lib);
    std::fs::remove_file(lib.state.root().join(store::STORE_FILE)).unwrap();
    assert!(store_rows(&lib).is_empty(), "the row is gone");
    for row in &before {
        assert!(store::entry_dir(&lib.state, row).is_dir());
    }
    let mark = recording.log().len();
    let second = fill_in(&built.hive, &lib, &built.target, &|| false);
    let later = &recording.log()[mark..];
    assert!(
        later.iter().all(|line| !line.starts_with("clone ")),
        "a final directory is not cloned again: {later:?}"
    );
    assert_eq!(second.entries_added, 0, "{second:?}");
    assert!(second.entries_held >= 1, "{second:?}");
    let adopted = store_rows(&lib);
    assert_eq!(adopted.len(), before.len(), "{adopted:?}");
    for row in &adopted {
        assert!(
            before.iter().any(|prev| {
                prev.name_meta == row.name_meta && prev.fingerprint_hash == row.fingerprint_hash
            }),
            "adopted {row:?}"
        );
        assert!(row.filled_at > 0 && row.last_live_at > 0, "{row:?}");
    }
}

fn try_fill(
    hive: &Hive,
    lib: &LibCtx,
    target: &TargetDir,
) -> Result<FillReport, cargo_hive::seed::Error> {
    let _env = env_lock().lock().unwrap_or_else(|err| err.into_inner());
    let _home = CargoHomeGuard::set(&hive.cargo_home.path);
    cargo_hive::seed::fill::fill(
        &lib.ctx(),
        &lib.repos[0],
        std::slice::from_ref(target),
        &|| false,
    )
}

#[test]
fn a_full_disk_cloning_the_tag_is_disk_full() {
    let built = built();
    let state = cargo_hive::state::State::open_at(built.hive.home.path.clone()).unwrap();
    let tag = tag_path(&state);
    let recording = Recording::over_real().fail_clone(&tag, libc::ENOSPC);
    let lib = narrowed(&built.hive, recording.fs(), &built.repo.main);
    let err = try_fill(&built.hive, &lib, &built.target).expect_err("a full disk is not success");
    assert!(matches!(err, cargo_hive::seed::Error::DiskFull(_)), "{err}");
}

#[test]
fn a_full_disk_resetting_the_temp_directory_is_disk_full() {
    let built = built();
    let unit = registry_units(&built.hive, &built.target)
        .into_iter()
        .next()
        .unwrap();
    let key = format!("{}-{}", unit.dir_name, unit.fingerprint_hash);
    let tmp_name = format!("{}-{key}", std::process::id());
    let disk = FullDisk::mount_with(|mount| {
        std::fs::write(mount.join(&tmp_name), b"").unwrap();
    });
    point_store_tmp(&built.hive, &disk.mount);
    let recording = Recording::over_real();
    let lib = narrowed(&built.hive, recording.fs(), &built.repo.main);
    let err = try_fill(&built.hive, &lib, &built.target).expect_err("a full disk is not success");
    assert!(matches!(err, cargo_hive::seed::Error::DiskFull(_)), "{err}");
}

#[test]
fn a_full_disk_creating_the_temp_parent_is_disk_full() {
    let built = built();
    let disk = FullDisk::mount_with(|_| {});
    point_store_tmp(&built.hive, &disk.mount);
    let recording = Recording::over_real();
    let lib = narrowed(&built.hive, recording.fs(), &built.repo.main);
    let err = try_fill(&built.hive, &lib, &built.target).expect_err("a full disk is not success");
    assert!(matches!(err, cargo_hive::seed::Error::DiskFull(_)), "{err}");
}

struct FullDisk {
    _scratch: common::Scratch,
    mount: PathBuf,
    _filler: std::fs::File,
}

impl FullDisk {
    fn mount_with(prepare: impl FnOnce(&Path)) -> FullDisk {
        sweep_full_disks();
        let scratch = common::Scratch::new("diskfull");
        let image = scratch.path.join("hive.dmg");
        let mount = scratch.path.join("mnt");
        std::fs::create_dir_all(&mount).unwrap();
        disk_tool(&[
            "hdiutil",
            "create",
            "-size",
            "12m",
            "-fs",
            "HFS+",
            "-volname",
            "hive-it",
            "-quiet",
            image.to_str().unwrap(),
        ]);
        disk_tool(&[
            "hdiutil",
            "attach",
            "-nobrowse",
            "-quiet",
            "-mountpoint",
            mount.to_str().unwrap(),
            image.to_str().unwrap(),
        ]);
        std::fs::write(mount.join(".metadata_never_index"), b"").unwrap();
        prepare(&mount);
        let mut filler = std::fs::File::create(mount.join("filler")).unwrap();
        pack_until_full(&mut filler);
        match std::fs::create_dir(mount.join("no-room")) {
            Err(err) if err.raw_os_error() == Some(libc::ENOSPC) => {}
            other => panic!("the volume should be full before the fill, got {other:?}"),
        }
        FullDisk {
            _scratch: scratch,
            mount,
            _filler: filler,
        }
    }
}

impl Drop for FullDisk {
    fn drop(&mut self) {
        let _ = std::process::Command::new("hdiutil")
            .args(["detach", "-force", "-quiet"])
            .arg(&self.mount)
            .status();
    }
}

fn pack_until_full(file: &mut std::fs::File) {
    use std::io::Write;
    for size in [1024 * 1024, 4096, 512, 1] {
        let chunk = vec![b'x'; size];
        loop {
            match file.write(&chunk) {
                Ok(0) => break,
                Ok(_) => {
                    let _ = file.flush();
                }
                Err(err) if err.raw_os_error() == Some(libc::ENOSPC) => break,
                Err(err) => panic!("filling the volume: {err}"),
            }
        }
    }
}

fn point_store_tmp(hive: &Hive, mount: &Path) {
    let store = hive.home.path.join("store");
    std::fs::create_dir_all(&store).unwrap();
    std::os::unix::fs::symlink(mount, store.join(".tmp")).unwrap();
}

fn disk_tool(args: &[&str]) {
    let out = std::process::Command::new(args[0])
        .args(&args[1..])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn sweep_full_disks() {
    let Ok(entries) = std::fs::read_dir("/private/tmp") else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Some(rest) = name.strip_prefix("hive-it-diskfull-") else {
            continue;
        };
        let pid = rest.split('-').next().unwrap_or("");
        if pid == std::process::id().to_string() {
            continue;
        }
        let alive = std::process::Command::new("kill")
            .args(["-0", pid])
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if alive {
            continue;
        }
        let _ = std::process::Command::new("hdiutil")
            .args(["detach", "-force", "-quiet"])
            .arg(entry.path().join("mnt"))
            .status();
        let _ = std::fs::remove_dir_all(entry.path());
    }
}

#[test]
fn a_file_that_is_not_regular_when_cloned_fills_nothing_for_that_unit() {
    let built = built();
    let units = registry_units(&built.hive, &built.target);
    let unit = units[0].clone();
    let outside = built.hive.home.path.join("outside-the-target");
    std::fs::write(&outside, b"not-for-the-store").unwrap();
    let extra = built
        .target_path
        .join("debug/.fingerprint")
        .join(&unit.dir_name)
        .join("extra");
    std::fs::write(&extra, b"regular-at-listing").unwrap();
    let recording = Recording::over_real().symlink_after_stat(&extra, &outside);
    let lib = narrowed(&built.hive, recording.fs(), &built.repo.main);
    let report = fill_in(&built.hive, &lib, &built.target, &|| false);
    assert!(
        report.findings.iter().any(|(_, _, detail)| {
            detail.contains("not a regular file") && detail.contains(&extra.display().to_string())
        }),
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
    assert!(extra.symlink_metadata().unwrap().file_type().is_symlink());
}

fn toolchains(lib: &LibCtx) -> BTreeMap<String, u64> {
    store::read(&lib.state).unwrap().toolchains
}

fn unit_json_path(target: &Path, unit: &SourceUnit) -> PathBuf {
    target
        .join(&unit.rel_profile)
        .join(".fingerprint")
        .join(&unit.dir_name)
        .join(format!("{}.json", unit.stem))
}

fn plant_rustc_info(target: &Path, checkout: &Path) {
    let text = cargo_hive::doctor::resolve_toolchain(checkout)
        .unwrap()
        .version_text;
    let body = serde_json::json!({
        "outputs": {
            "version": {"stdout": text, "stderr": ""},
            "probe": {"stdout": "lib___.rlib\n", "stderr": ""}
        }
    });
    std::fs::write(
        target.join(".rustc_info.json"),
        serde_json::to_string(&body).unwrap(),
    )
    .unwrap();
}

fn reread(target: &mut TargetDir) {
    target.profiles = target
        .profiles
        .iter()
        .map(|profile| read_profile(&profile.path))
        .collect();
}

fn one_toolchain_finding(report: &FillReport, target: &Path) {
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    let (_, path, detail) = &report.findings[0];
    assert_eq!(path.as_deref(), Some(target));
    assert!(detail.contains(&target.display().to_string()), "{detail}");
}

#[test]
fn a_clean_target_writes_a_row_matching_its_toolchain() {
    let built = built();
    let units = registry_units(&built.hive, &built.target);
    let value = units[0].toolchain_value;
    assert!(
        units.iter().all(|unit| unit.toolchain_value == value),
        "a clean target's units share one toolchain value"
    );
    let text = cargo_hive::doctor::resolve_toolchain(&built.repo.main)
        .unwrap()
        .version_text;
    assert_eq!(
        cargo_hive::seed::fill::toolchain_row(&built.target, &built.repo.main).unwrap(),
        (text.clone(), value)
    );
    let recording = Recording::over_real();
    let lib = narrowed(&built.hive, recording.fs(), &built.repo.main);
    let report = fill_in(&built.hive, &lib, &built.target, &|| false);
    assert!(report.findings.is_empty(), "{:?}", report.findings);
    assert!(report.entries_added >= 1, "{report:?}");
    assert_eq!(
        toolchains(&lib),
        BTreeMap::from([(text, value)]),
        "the table key is the checkout's version text and the value is the units'"
    );
}

#[test]
fn a_target_with_two_toolchain_values_writes_no_row_and_one_finding() {
    let built = built();
    let units = registry_units(&built.hive, &built.target);
    let value = units[0].toolchain_value;
    assert!(units.iter().all(|unit| unit.toolchain_value == value));
    let json = unit_json_path(&built.target_path, &units[0]);
    let raw = std::fs::read_to_string(&json).unwrap();
    let mut parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    parsed["rustc"] = serde_json::json!(value.wrapping_add(1));
    std::fs::write(&json, serde_json::to_vec(&parsed).unwrap()).unwrap();
    let mut target = built.target.clone();
    reread(&mut target);
    let err = cargo_hive::seed::fill::toolchain_row(&target, &built.repo.main).unwrap_err();
    assert!(
        err.contains(&built.target_path.display().to_string()),
        "{err}"
    );
    let recording = Recording::over_real();
    let lib = narrowed(&built.hive, recording.fs(), &built.repo.main);
    let report = fill_in(&built.hive, &lib, &target, &|| false);
    one_toolchain_finding(&report, &built.target_path);
    assert!(toolchains(&lib).is_empty(), "{:?}", toolchains(&lib));
    assert!(
        report.entries_added >= 1,
        "the units are still filled: {report:?}"
    );
}

#[test]
fn a_malformed_rustc_info_writes_no_row_and_one_finding() {
    let built = built();
    let path = built.target_path.join(".rustc_info.json");
    std::fs::write(&path, b"not json").unwrap();
    let err = cargo_hive::seed::fill::toolchain_row(&built.target, &built.repo.main).unwrap_err();
    assert!(err.contains(&path.display().to_string()), "{err}");
    let recording = Recording::over_real();
    let lib = narrowed(&built.hive, recording.fs(), &built.repo.main);
    let report = fill_in(&built.hive, &lib, &built.target, &|| false);
    one_toolchain_finding(&report, &built.target_path);
    assert!(toolchains(&lib).is_empty(), "{:?}", toolchains(&lib));
    assert!(report.entries_added >= 1, "{report:?}");
}

#[test]
fn a_missing_rustc_info_writes_no_row_and_one_finding() {
    let built = built();
    let path = built.target_path.join(".rustc_info.json");
    std::fs::remove_file(&path).unwrap();
    let err = cargo_hive::seed::fill::toolchain_row(&built.target, &built.repo.main).unwrap_err();
    assert!(err.contains(&path.display().to_string()), "{err}");
    let recording = Recording::over_real();
    let lib = narrowed(&built.hive, recording.fs(), &built.repo.main);
    let report = fill_in(&built.hive, &lib, &built.target, &|| false);
    one_toolchain_finding(&report, &built.target_path);
    assert!(toolchains(&lib).is_empty(), "{:?}", toolchains(&lib));
    assert!(report.entries_added >= 1, "{report:?}");
}

#[test]
fn text_that_differs_from_the_source_checkout_writes_no_row() {
    let built = built();
    let path = built.target_path.join(".rustc_info.json");
    let raw = std::fs::read_to_string(&path).unwrap();
    let mut parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let outputs = parsed["outputs"].as_object_mut().unwrap();
    let mut changed = 0;
    for entry in outputs.values_mut() {
        let Some(stdout) = entry.get("stdout").and_then(|value| value.as_str()) else {
            continue;
        };
        if stdout.contains("release:") {
            entry["stdout"] =
                serde_json::Value::String("rustc 0.0.0\nrelease: 0.0.0\n".to_string());
            changed += 1;
        }
    }
    assert_eq!(changed, 1, "the built target has one release stdout");
    std::fs::write(&path, serde_json::to_string(&parsed).unwrap()).unwrap();
    let checkout = cargo_hive::doctor::resolve_toolchain(&built.repo.main)
        .unwrap()
        .version_text;
    let parsed_text =
        cargo_hive::seed::fill::rustc_info_version(&std::fs::read_to_string(&path).unwrap())
            .unwrap();
    assert_ne!(parsed_text, checkout);
    let err = cargo_hive::seed::fill::toolchain_row(&built.target, &built.repo.main).unwrap_err();
    assert!(
        err.contains(&built.target_path.display().to_string()),
        "{err}"
    );
    let recording = Recording::over_real();
    let lib = narrowed(&built.hive, recording.fs(), &built.repo.main);
    let report = fill_in(&built.hive, &lib, &built.target, &|| false);
    one_toolchain_finding(&report, &built.target_path);
    assert!(toolchains(&lib).is_empty(), "{:?}", toolchains(&lib));
    assert!(report.entries_added >= 1, "{report:?}");
}

#[test]
fn two_targets_with_one_toolchain_share_one_row() {
    let built = built();
    let units = registry_units(&built.hive, &built.target);
    let value = units[0].toolchain_value;
    assert!(units.iter().all(|unit| unit.toolchain_value == value));
    let copy_path = built.hive.home.path.join("target-copy");
    let status = std::process::Command::new("cp")
        .args(["-cR"])
        .arg(&built.target_path)
        .arg(&copy_path)
        .status()
        .unwrap();
    assert!(status.success(), "cp the target");
    let copy = TargetDir {
        path: copy_path.clone(),
        owner: Owner::Owned {
            checkout: built.repo.main.clone(),
        },
        profiles: vec![read_profile(&copy_path.join("debug"))],
        size: None,
        unreadable: None,
    };
    let text = cargo_hive::doctor::resolve_toolchain(&built.repo.main)
        .unwrap()
        .version_text;
    assert_eq!(
        cargo_hive::seed::fill::toolchain_row(&built.target, &built.repo.main).unwrap(),
        (text.clone(), value)
    );
    assert_eq!(
        cargo_hive::seed::fill::toolchain_row(&copy, &built.repo.main).unwrap(),
        (text.clone(), value)
    );
    let recording = Recording::over_real();
    let lib = narrowed(&built.hive, recording.fs(), &built.repo.main);
    let first = fill_in(&built.hive, &lib, &built.target, &|| false);
    assert!(first.entries_added >= 1, "{first:?}");
    assert_eq!(toolchains(&lib), BTreeMap::from([(text.clone(), value)]));
    let mut cleared = store::read(&lib.state).unwrap();
    cleared.toolchains.clear();
    lib.state
        .locked(|tx| tx.write_bounded(store::STORE_FILE, &cleared, store::MAX_STORE_BYTES))
        .unwrap();
    assert!(toolchains(&lib).is_empty());
    let second = fill_in(&built.hive, &lib, &copy, &|| false);
    assert_eq!(second.entries_added, 0, "{second:?}");
    assert!(second.entries_held >= 1, "{second:?}");
    assert!(second.findings.is_empty(), "{:?}", second.findings);
    assert_eq!(toolchains(&lib), BTreeMap::from([(text, value)]));
}

const PLACE_WINDOW: Duration = Duration::from_secs(7 * 86_400);

struct Filled {
    built: Built,
    rows: Vec<store::Row>,
}

fn filled() -> Filled {
    let built = built();
    let recording = Recording::over_real();
    let lib = narrowed(&built.hive, recording.fs(), &built.repo.main);
    let report = fill_in(&built.hive, &lib, &built.target, &|| false);
    assert!(report.entries_added >= 2, "{report:?}");
    let rows = store_rows(&lib);
    assert!(rows.len() >= 2, "place needs more than one chosen unit");
    Filled { built, rows }
}

fn run_place(
    filled: &Filled,
    checkout: &Path,
    units: &[store::Row],
    dir_cap: usize,
    recording: &Recording,
    cancel: &(dyn Fn() -> bool + Sync),
) -> Result<PlaceReport, Error> {
    let lib = narrowed(&filled.built.hive, recording.fs(), &filled.built.repo.main);
    place::place(&lib.ctx(), checkout, units, PLACE_WINDOW, dir_cap, cancel)
}

fn file_bodies(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
    fn walk(
        dir: &Path,
        root: &Path,
        out: &mut BTreeMap<PathBuf, (Vec<u8>, SystemTime)>,
        depth: usize,
    ) {
        assert!(depth < 64, "walk bound hit");
        let mut seen = 0usize;
        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound && depth == 0 => return,
            Err(err) => panic!("{}: {err}", dir.display()),
        };
        for entry in entries {
            seen += 1;
            assert!(seen < 100_000, "walk bound hit");
            let path = entry.unwrap().path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                walk(&path, root, out, depth + 1);
                continue;
            }
            if !meta.is_file() {
                continue;
            }
            assert!(
                meta.len() <= 32 * 1024 * 1024,
                "{} is larger than 32 MiB",
                path.display()
            );
            let rel = path.strip_prefix(root).unwrap().to_path_buf();
            out.insert(
                rel,
                (std::fs::read(&path).unwrap(), meta.modified().unwrap()),
            );
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out, 0);
    out
}

fn first_file_rel(root: &Path) -> PathBuf {
    fn walk(dir: &Path, root: &Path, depth: usize) -> Option<PathBuf> {
        assert!(depth < 64, "walk bound hit");
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect();
        assert!(entries.len() < 100_000, "walk bound hit");
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                if let Some(found) = walk(&path, root, depth + 1) {
                    return Some(found);
                }
            } else if meta.is_file() {
                return Some(path.strip_prefix(root).unwrap().to_path_buf());
            }
        }
        None
    }
    walk(root, root, 0).expect("the entry has a file")
}

fn open_state(filled: &Filled) -> cargo_hive::state::State {
    cargo_hive::state::State::open_at(filled.built.hive.home.path.clone()).unwrap()
}

fn rewrite_entry(state: &cargo_hive::state::State, row: &store::Row, files: &[&str]) {
    let root = store::entry_dir(state, row);
    std::fs::remove_dir_all(&root).unwrap();
    assert!(files.len() < 100_000, "walk bound hit");
    for rel in files {
        let path = root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, rel.as_bytes()).unwrap();
    }
}

fn names_path(report: &PlaceReport, path: &Path) {
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    let (_, found, detail) = &report.findings[0];
    assert_eq!(found.as_deref(), Some(path), "{detail}");
    assert!(detail.contains(&path.display().to_string()), "{detail}");
}

fn no_forbidden_writes(log: &[String], target: &Path) {
    for line in log {
        if let Some(rest) = line.strip_prefix("write_file ") {
            let path = Path::new(rest);
            if path.starts_with(target) {
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("");
                assert!(
                    name.starts_with(".hive-expand-") && !path.exists(),
                    "place left a file in the target that is not the expanded replacement: {line}"
                );
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix("set_mtime ") {
            let path = Path::new(rest);
            if path.starts_with(target) {
                let name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("");
                assert!(
                    path.is_file() && !name.starts_with(".hive-expand-"),
                    "mtime was not restored on the placed file: {line}"
                );
            }
            continue;
        }
        let rest = line
            .strip_prefix("hardlink ")
            .or_else(|| line.strip_prefix("compress "))
            .or_else(|| line.strip_prefix("remove "));
        let Some(rest) = rest else {
            continue;
        };
        assert!(
            !Path::new(rest).starts_with(target),
            "place wrote the target by a means other than create_dir or clone_file: {line}"
        );
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Hold {
    Dir(BTreeMap<PathBuf, (Vec<u8>, SystemTime)>),
    File(Vec<u8>, SystemTime),
    Link(PathBuf),
}

fn hold(path: &Path) -> Hold {
    let meta = std::fs::symlink_metadata(path).unwrap();
    if meta.file_type().is_symlink() {
        Hold::Link(std::fs::read_link(path).unwrap())
    } else if meta.is_dir() {
        Hold::Dir(snapshot(path))
    } else {
        Hold::File(std::fs::read(path).unwrap(), meta.modified().unwrap())
    }
}

#[test]
fn an_empty_set_creates_nothing() {
    let filled = filled();
    let work = filled.built.repo.worktree("empty");
    let recording = Recording::over_real();
    let report = run_place(&filled, &work, &[], CRATE_DIR_CAP, &recording, &|| false).unwrap();
    assert!(!work.join("target").exists());
    assert!(report.placed.is_empty());
    assert_eq!(report.cloned, 0);
    assert_eq!(report.existing, 0);
    assert_eq!(report.failed, 0);
    assert!(report.findings.is_empty(), "{:?}", report.findings);
    assert!(!report.cancelled);
    assert!(
        recording
            .log()
            .iter()
            .all(|line| !line.starts_with("create_dir ") && !line.starts_with("clone ")),
        "{:?}",
        recording.log()
    );
}

#[test]
fn an_absent_target_gets_directories_tag_and_units_and_identify_accepts_it() {
    let filled = filled();
    let work = filled.built.repo.worktree("fresh");
    let recording = Recording::over_real();
    let report = run_place(
        &filled,
        &work,
        &filled.rows,
        CRATE_DIR_CAP,
        &recording,
        &|| false,
    )
    .unwrap();
    let target = work.join("target");
    assert!(
        cargo_hive::cargo_target::identify(&target).is_some_and(|found| found.is_ok()),
        "identify accepts the seeded target before any build"
    );
    let state = open_state(&filled);
    let mut expect = file_bodies(&store::entry_dir(&state, &filled.rows[0]));
    for row in filled.rows.iter().skip(1) {
        expect.extend(file_bodies(&store::entry_dir(&state, row)));
    }
    let tag = tag_path(&state);
    let tag_meta = std::fs::symlink_metadata(&tag).unwrap();
    expect.insert(
        PathBuf::from("CACHEDIR.TAG"),
        (std::fs::read(&tag).unwrap(), tag_meta.modified().unwrap()),
    );
    assert_eq!(file_bodies(&target), expect);
    assert_eq!(report.cloned, filled.rows.len() as u64, "{report:?}");
    assert_eq!(report.placed.len() as u64, report.cloned);
    assert_eq!(report.existing, 0);
    assert_eq!(report.failed, 0);
    assert_eq!(report.outside_window, 0, "{report:?}");
    assert!(report.findings.is_empty(), "{:?}", report.findings);
    assert!(!report.cancelled);
    let meta = std::fs::symlink_metadata(&target).unwrap();
    assert_eq!(report.target_identity, Some((meta.dev(), meta.ino())));
    for row in &filled.rows {
        let unit = report
            .placed
            .iter()
            .find(|unit| unit.dir_name == row.name_meta && unit.rel_profile == row.rel_profile)
            .unwrap_or_else(|| panic!("missing {}", row.name_meta));
        let entry = store::entry_dir(&state, row);
        let bodies = file_bodies(&entry);
        let deps = bodies
            .keys()
            .filter(|rel| {
                rel.components()
                    .any(|component| component.as_os_str() == "deps")
            })
            .count() as u32;
        let has_build = bodies.keys().any(|rel| {
            rel.components()
                .any(|component| component.as_os_str() == "build")
        });
        assert_eq!(unit.deps_files, deps, "{}", row.name_meta);
        assert_eq!(unit.has_build, has_build, "{}", row.name_meta);
        let stored = unit_timestamp(&entry.join(&row.rel_profile), &row.name_meta, &row.kind);
        let expected_path = unit_timestamp(Path::new(&row.rel_profile), &row.name_meta, &row.kind);
        match std::fs::symlink_metadata(&stored) {
            Ok(meta) => {
                let (path, ns) = unit.stamp.clone().unwrap_or_else(|| {
                    panic!("{} has a store stamp but no recorded stamp", row.name_meta)
                });
                assert_eq!(path, expected_path);
                let expect_ns = meta
                    .modified()
                    .unwrap()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos() as u64;
                assert_eq!(ns, expect_ns, "{}", row.name_meta);
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                assert!(unit.stamp.is_none(), "{}", row.name_meta);
            }
            Err(err) => panic!("{}: {err}", stored.display()),
        }
    }
}

#[test]
fn an_existing_path_gets_nothing_whatever_it_holds() {
    let filled = filled();
    for name in ["empty-dir", "plain-file", "dangling-link"] {
        let work = filled.built.repo.worktree(name);
        let target = work.join("target");
        match name {
            "empty-dir" => std::fs::create_dir(&target).unwrap(),
            "plain-file" => std::fs::write(&target, b"keep").unwrap(),
            "dangling-link" => {
                std::os::unix::fs::symlink(work.join("missing"), &target).unwrap();
            }
            _ => unreachable!(),
        }
        let before = hold(&target);
        let recording = Recording::over_real();
        let report = run_place(
            &filled,
            &work,
            &filled.rows,
            CRATE_DIR_CAP,
            &recording,
            &|| false,
        )
        .unwrap();
        assert_eq!(hold(&target), before, "{name}");
        assert_eq!(report.cloned, 0, "{name}");
        assert_eq!(report.existing, 0, "{name}");
        assert_eq!(report.failed, 0, "{name}");
        assert!(report.placed.is_empty(), "{name}");
        names_path(&report, &target);
        no_forbidden_writes(&recording.log(), &target);
    }
}

#[test]
fn a_missing_parent_creates_nothing() {
    let filled = filled();
    let work = filled.built.repo.worktree("missing-parent");
    std::fs::create_dir_all(work.join(".cargo")).unwrap();
    std::fs::write(
        work.join(".cargo/config.toml"),
        "[build]\ntarget-dir = \"absent-parent/target\"\n",
    )
    .unwrap();
    let recording = Recording::over_real();
    let report = run_place(
        &filled,
        &work,
        &filled.rows,
        CRATE_DIR_CAP,
        &recording,
        &|| false,
    )
    .unwrap();
    let target = work.join("absent-parent/target");
    assert!(!work.join("absent-parent").exists());
    assert_eq!(report.cloned, 0);
    assert!(report.placed.is_empty());
    names_path(&report, &target);
}

#[test]
fn a_root_with_no_cargo_toml_and_a_split_build_directory_clone_nothing() {
    let filled = filled();
    let bare = filled.built.repo.scratch.path.join("not-a-crate");
    std::fs::create_dir_all(&bare).unwrap();
    std::fs::write(bare.join("note"), b"leave me").unwrap();
    let recording = Recording::over_real();
    let unresolved = run_place(
        &filled,
        &bare,
        &filled.rows,
        CRATE_DIR_CAP,
        &recording,
        &|| false,
    )
    .unwrap();
    assert_eq!(std::fs::read(bare.join("note")).unwrap(), b"leave me");
    assert!(!bare.join("target").exists());
    assert_eq!(unresolved.cloned, 0);
    assert!(unresolved.placed.is_empty());
    names_path(&unresolved, &bare);

    let work = filled.built.repo.worktree("split");
    let build_out = work.join("build-out");
    std::fs::create_dir_all(work.join(".cargo")).unwrap();
    std::fs::write(
        work.join(".cargo/config.toml"),
        format!("[build]\nbuild-dir = \"{}\"\n", build_out.display()),
    )
    .unwrap();
    let split = run_place(
        &filled,
        &work,
        &filled.rows,
        CRATE_DIR_CAP,
        &recording,
        &|| false,
    )
    .unwrap();
    assert!(!work.join("target").exists());
    assert!(!build_out.exists());
    assert_eq!(split.cloned, 0);
    assert!(split.placed.is_empty());
    assert_eq!(split.findings.len(), 1, "{:?}", split.findings);
    let (_, path, detail) = &split.findings[0];
    assert_eq!(
        path.as_deref(),
        Some(work.join("target").as_path()),
        "{detail}"
    );
    assert!(
        detail.contains(&build_out.display().to_string())
            && detail.contains(&work.join("target").display().to_string()),
        "{detail}"
    );
}

#[test]
fn only_create_dir_and_clone_write_into_the_target() {
    let filled = filled();
    let work = filled.built.repo.worktree("writes");
    let recording = Recording::over_real();
    let report = run_place(
        &filled,
        &work,
        &filled.rows,
        CRATE_DIR_CAP,
        &recording,
        &|| false,
    )
    .unwrap();
    assert!(report.cloned >= 1, "{report:?}");
    let target = work.join("target");
    let log = recording.log();
    no_forbidden_writes(&log, &target);
    assert!(
        log.iter().any(|line| line.starts_with("create_dir ")
            && Path::new(line.strip_prefix("create_dir ").unwrap()).starts_with(&target)),
        "{log:?}"
    );
    assert!(
        log.iter()
            .any(|line| line.starts_with("clone ") && line.contains(&*target.to_string_lossy())),
        "{log:?}"
    );
}

#[test]
fn a_clone_finding_an_existing_file_leaves_it_and_counts_it() {
    let filled = filled();
    let state = open_state(&filled);
    let row = &filled.rows[0];
    let rel = first_file_rel(&store::entry_dir(&state, row));
    let work = filled.built.repo.worktree("existing-file");
    let dst = work.join("target").join(&rel);
    let recording = Recording::over_real().plant_before_clone(&dst, b"already-there");
    let report = run_place(
        &filled,
        &work,
        &filled.rows,
        CRATE_DIR_CAP,
        &recording,
        &|| false,
    )
    .unwrap();
    assert_eq!(std::fs::read(&dst).unwrap(), b"already-there");
    assert_eq!(report.existing, 1, "{report:?}");
    assert_eq!(report.failed, 0, "{report:?}");
    assert_eq!(report.cloned, filled.rows.len() as u64 - 1, "{report:?}");
    assert_eq!(report.placed.len() as u64, report.cloned);
    assert!(report
        .placed
        .iter()
        .all(|unit| unit.dir_name != row.name_meta || unit.rel_profile != row.rel_profile));
}

#[test]
fn a_clone_failure_counts_the_unit_and_continues() {
    let filled = filled();
    let state = open_state(&filled);
    let row = &filled.rows[0];
    let rel = first_file_rel(&store::entry_dir(&state, row));
    let work = filled.built.repo.worktree("clone-fails");
    let dst = work.join("target").join(&rel);
    let recording = Recording::over_real().fail_clone(&dst, libc::EIO);
    let report = run_place(
        &filled,
        &work,
        &filled.rows,
        CRATE_DIR_CAP,
        &recording,
        &|| false,
    )
    .unwrap();
    assert_eq!(report.failed, 1, "{report:?}");
    assert_eq!(report.cloned, filled.rows.len() as u64 - 1, "{report:?}");
    assert_eq!(report.existing, 0);
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    let (_, path, detail) = &report.findings[0];
    assert_eq!(path.as_deref(), Some(dst.as_path()), "{detail}");
    assert!(
        detail.contains(&dst.display().to_string())
            && (detail.contains("Input/output error") || detail.contains("os error 5")),
        "{detail}"
    );
    assert!(report
        .placed
        .iter()
        .all(|unit| unit.dir_name != row.name_meta || unit.rel_profile != row.rel_profile));
    assert!(report.placed.len() as u64 >= 1);
}

#[test]
fn a_full_disk_is_an_error_not_a_finding() {
    let filled = filled();
    let state = open_state(&filled);
    let rel = first_file_rel(&store::entry_dir(&state, &filled.rows[0]));
    let work = filled.built.repo.worktree("full-disk");
    let dst = work.join("target").join(rel);
    let recording = Recording::over_real().fail_clone(&dst, libc::ENOSPC);
    let err = run_place(
        &filled,
        &work,
        &filled.rows,
        CRATE_DIR_CAP,
        &recording,
        &|| false,
    )
    .unwrap_err();
    assert!(matches!(err, Error::DiskFull(_)), "{err}");
    assert!(err.to_string().contains("disk is full"), "{err}");
}

#[test]
fn a_cancel_stops_before_the_next_unit() {
    let filled = filled();
    let work = filled.built.repo.worktree("cancel");
    let target = work.join("target");
    let cancel = || target.join("debug").is_dir();
    let recording = Recording::over_real();
    let report = run_place(
        &filled,
        &work,
        &filled.rows,
        CRATE_DIR_CAP,
        &recording,
        &cancel,
    )
    .unwrap();
    assert!(report.cancelled, "{report:?}");
    assert!(report.cloned >= 1, "{report:?}");
    assert!(
        report.cloned < filled.rows.len() as u64,
        "cloned {} of {}",
        report.cloned,
        filled.rows.len()
    );
}

#[test]
fn an_entry_at_its_directory_cap_clones_and_one_past_it_fails_that_unit() {
    let filled = filled();
    let state = open_state(&filled);
    let wide = filled.rows[0].clone();
    let narrow = filled.rows[1].clone();
    rewrite_entry(&state, &wide, &["debug/a", "debug/b", "debug/c"]);
    rewrite_entry(&state, &narrow, &["debug/only"]);
    let units = [wide, narrow];
    let fits = filled.built.repo.worktree("cap-fits");
    let recording = Recording::over_real();
    let at_cap = run_place(&filled, &fits, &units, 3, &recording, &|| false).unwrap();
    assert_eq!(at_cap.cloned, 2, "{at_cap:?}");
    assert_eq!(at_cap.failed, 0, "{at_cap:?}");
    assert!(at_cap.findings.is_empty(), "{:?}", at_cap.findings);
    for name in ["debug/a", "debug/b", "debug/c", "debug/only"] {
        assert_eq!(
            std::fs::read(fits.join("target").join(name)).unwrap(),
            name.as_bytes(),
            "{name}"
        );
    }
    let past = filled.built.repo.worktree("cap-past");
    let over = run_place(&filled, &past, &units, 2, &recording, &|| false).unwrap();
    assert_eq!(over.failed, 1, "{over:?}");
    assert_eq!(over.cloned, 1, "{over:?}");
    assert_eq!(over.findings.len(), 1, "{:?}", over.findings);
    let (_, _, detail) = &over.findings[0];
    assert!(detail.contains("more than 2"), "{detail}");
    let target = past.join("target");
    assert_eq!(
        std::fs::read(target.join("debug/only")).unwrap(),
        b"debug/only"
    );
    for name in ["debug/a", "debug/b", "debug/c"] {
        assert!(
            !target.join(name).exists(),
            "{name} was cloned past the cap"
        );
    }
}

#[test]
fn an_entry_path_with_a_parent_component_is_refused() {
    let filled = filled();
    let state = open_state(&filled);
    let mut row = filled.rows[0].clone();
    rewrite_entry(&state, &row, &["leaf"]);
    row.rel_profile = PathBuf::from("../escape");
    let work = filled.built.repo.worktree("escape");
    let recording = Recording::over_real();
    let report = run_place(&filled, &work, &[row], CRATE_DIR_CAP, &recording, &|| false).unwrap();
    assert_eq!(report.cloned, 0, "{report:?}");
    assert_eq!(report.failed, 1, "{report:?}");
    assert!(report.placed.is_empty());
    assert_eq!(report.findings.len(), 1, "{:?}", report.findings);
    let (_, _, detail) = &report.findings[0];
    assert!(
        detail.contains("..") && detail.contains("escape"),
        "{detail}"
    );
    assert!(!work.join("escape").exists());
    assert!(!work.join("target").join("leaf").exists());
}

#[test]
fn units_outside_the_window_are_counted() {
    let filled = filled();
    let state = open_state(&filled);
    let row = &filled.rows[0];
    let stored = unit_timestamp(
        &store::entry_dir(&state, row).join(&row.rel_profile),
        &row.name_meta,
        &row.kind,
    );
    assert!(
        stored.is_file(),
        "the fixture has a stamp at {}",
        stored.display()
    );
    let old = SystemTime::now() - Duration::from_secs(40 * 86_400);
    common::cargo_fixture::set_mtime(&stored, old);
    let expected_ns = std::fs::symlink_metadata(&stored)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    let mut outside = 0u64;
    for row in &filled.rows {
        let stamp = unit_timestamp(
            &store::entry_dir(&state, row).join(&row.rel_profile),
            &row.name_meta,
            &row.kind,
        );
        match std::fs::symlink_metadata(&stamp) {
            Ok(meta) => {
                let modified = meta.modified().unwrap();
                let aged = SystemTime::now()
                    .duration_since(modified)
                    .unwrap_or(Duration::ZERO);
                if aged > PLACE_WINDOW {
                    outside += 1;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => outside += 1,
            Err(err) => panic!("{}: {err}", stamp.display()),
        }
    }
    assert!(outside >= 1, "the backdated stamp is outside the window");
    let work = filled.built.repo.worktree("window");
    let recording = Recording::over_real();
    let report = run_place(
        &filled,
        &work,
        &filled.rows,
        CRATE_DIR_CAP,
        &recording,
        &|| false,
    )
    .unwrap();
    assert_eq!(report.outside_window, outside, "{report:?}");
    assert_eq!(report.cloned, filled.rows.len() as u64, "{report:?}");
    let unit = report
        .placed
        .iter()
        .find(|unit| unit.dir_name == row.name_meta)
        .unwrap();
    let (_, ns) = unit.stamp.clone().unwrap();
    assert_eq!(ns, expected_ns);
}

fn real_apfs() -> Apfs {
    Apfs::new(PathBuf::from(env!("CARGO_BIN_EXE_cargo-hive")))
}

fn place_with(
    filled: &Filled,
    checkout: &Path,
    units: &[store::Row],
    fs: Box<dyn Fs>,
    account_bytes: bool,
) -> PlaceReport {
    let lib = narrowed(&filled.built.hive, fs, &filled.built.repo.main);
    let mut ctx = lib.ctx();
    ctx.account_bytes = account_bytes;
    place::place(&ctx, checkout, units, PLACE_WINDOW, CRATE_DIR_CAP, &|| {
        false
    })
    .unwrap_or_else(|err| panic!("place failed: {err}"))
}

fn cloned_pairs(log: &[String]) -> Vec<(PathBuf, PathBuf)> {
    log.iter()
        .filter_map(|line| {
            let rest = line.strip_prefix("clone ")?;
            let (src, dst) = rest.split_once(" -> ")?;
            Some((PathBuf::from(src), PathBuf::from(dst)))
        })
        .collect()
}

fn rel_with_blocks(root: &Path) -> Option<PathBuf> {
    fn walk(dir: &Path, root: &Path, depth: usize) -> Option<PathBuf> {
        assert!(depth < 64, "walk bound hit");
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect();
        assert!(entries.len() < 100_000, "walk bound hit");
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                if let Some(found) = walk(&path, root, depth + 1) {
                    return Some(found);
                }
            } else if meta.is_file() && meta.blocks() > 0 {
                return Some(path.strip_prefix(root).unwrap().to_path_buf());
            }
        }
        None
    }
    walk(root, root, 0)
}

fn heavy_store_file(filled: &Filled) -> (store::Row, PathBuf) {
    let state = open_state(filled);
    for row in &filled.rows {
        let root = store::entry_dir(&state, row);
        if let Some(rel) = rel_with_blocks(&root) {
            return (row.clone(), rel);
        }
    }
    panic!("a filled store has a file that allocates blocks");
}

fn regular_files(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
        assert!(depth <= 64, "walk bound hit");
        let mut seen = 0usize;
        for entry in std::fs::read_dir(dir).unwrap() {
            seen += 1;
            assert!(seen < 100_000, "walk bound hit");
            let path = entry.unwrap().path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                walk(&path, out, depth + 1);
            } else if meta.is_file() {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out, 0);
    out
}

fn allocated_from_metadata(root: &Path) -> u64 {
    let mut seen = HashSet::new();
    let mut total = 0u64;
    for path in regular_files(root) {
        let meta = std::fs::symlink_metadata(&path).unwrap();
        if seen.insert((meta.dev(), meta.ino())) {
            total += meta.blocks() * 512;
        }
    }
    total
}

/// `sharing` of one path returns a fixed value. Every other call goes to `inner`.
struct NamedShare {
    inner: Box<dyn Fs>,
    path: PathBuf,
    sharing: Sharing,
}

impl Fs for NamedShare {
    fn temp_dir(&self, parent: &Path, prefix: &str) -> io::Result<TempDir> {
        self.inner.temp_dir(parent, prefix)
    }
    fn write_file(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        self.inner.write_file(path, bytes)
    }
    fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
        self.inner.read(path)
    }
    fn hardlink(&self, src: &Path, dst: &Path) -> io::Result<()> {
        self.inner.hardlink(src, dst)
    }
    fn set_mtime(&self, path: &Path, t: SystemTime) -> io::Result<()> {
        self.inner.set_mtime(path, t)
    }
    fn mtime(&self, path: &Path) -> io::Result<SystemTime> {
        self.inner.mtime(path)
    }
    fn inode(&self, path: &Path) -> io::Result<u64> {
        self.inner.inode(path)
    }
    fn clone_file(&self, src: &Path, dst: &Path) -> io::Result<()> {
        self.inner.clone_file(src, dst)
    }
    fn create_dir(&self, path: &Path) -> io::Result<()> {
        self.inner.create_dir(path)
    }
    fn sharing(&self, path: &Path) -> io::Result<Sharing> {
        if path == self.path {
            Ok(self.sharing)
        } else {
            self.inner.sharing(path)
        }
    }
    fn compress(&self, path: &Path) -> CompressOutcome {
        self.inner.compress(path)
    }
    fn hold_lock(&self, kind: LockKind, path: &Path) -> io::Result<LockHolder> {
        self.inner.hold_lock(kind, path)
    }
    fn try_lock_exclusive(&self, path: &Path) -> TryLock {
        self.inner.try_lock_exclusive(path)
    }
    fn try_lock_elsewhere(&self, kind: LockKind, path: &Path) -> io::Result<LockState> {
        self.inner.try_lock_elsewhere(kind, path)
    }
    fn entry_stat(&self, path: &Path) -> io::Result<EntryStat> {
        self.inner.entry_stat(path)
    }
    fn read_dir_bounded(&self, dir: &Path, cap: usize) -> io::Result<Vec<(String, EntryKind)>> {
        self.inner.read_dir_bounded(dir, cap)
    }
    fn remove_tree(&self, path: &Path) -> io::Result<()> {
        self.inner.remove_tree(path)
    }
    fn remove_file(&self, path: &Path) -> io::Result<()> {
        self.inner.remove_file(path)
    }
    fn volume(&self, path: &Path) -> io::Result<VolumeInfo> {
        self.inner.volume(path)
    }
}

fn place_one_overridden(
    filled: &Filled,
    label: &str,
    pick: impl Fn(&EntryStat) -> bool,
    sharing_of: impl Fn(&Sharing) -> Sharing,
) -> (PlaceReport, PathBuf, Apfs, Vec<(PathBuf, PathBuf)>) {
    let (row, rel) = store_file_matching(filled, pick);
    let src = store::entry_dir(&open_state(filled), &row).join(&rel);
    let work = filled.built.repo.worktree(label);
    let dst = work.join("target").join(&rel);
    let apfs = real_apfs();
    let store_sharing = apfs.sharing(&src).unwrap();
    let recording = Recording::over_real();
    let fs = NamedShare {
        inner: recording.fs(),
        path: dst.clone(),
        sharing: sharing_of(&store_sharing),
    };
    let report = place_with(filled, &work, &filled.rows, Box::new(fs), true);
    assert!(
        report.cloned >= 1,
        "the overridden file's unit was cloned: {report:?}"
    );
    let pairs = cloned_pairs(&recording.log());
    (report, dst, apfs, pairs)
}

fn store_file_matching(
    filled: &Filled,
    pick: impl Fn(&EntryStat) -> bool,
) -> (store::Row, PathBuf) {
    let state = open_state(filled);
    let apfs = real_apfs();
    for row in &filled.rows {
        let root = store::entry_dir(&state, row);
        if let Some(rel) = rel_matching(&root, &apfs, &pick) {
            return (row.clone(), rel);
        }
    }
    panic!("no store file matched");
}

fn rel_matching(root: &Path, apfs: &Apfs, pick: &impl Fn(&EntryStat) -> bool) -> Option<PathBuf> {
    fn walk(
        dir: &Path,
        root: &Path,
        apfs: &Apfs,
        pick: &impl Fn(&EntryStat) -> bool,
        depth: usize,
    ) -> Option<PathBuf> {
        assert!(depth < 64, "walk bound hit");
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect();
        assert!(entries.len() < 100_000, "walk bound hit");
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let meta = std::fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                if let Some(found) = walk(&path, root, apfs, pick, depth + 1) {
                    return Some(found);
                }
            } else if meta.is_file() {
                let stat = apfs
                    .entry_stat(&path)
                    .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
                if pick(&stat) {
                    return Some(path.strip_prefix(root).unwrap().to_path_buf());
                }
            }
        }
        None
    }
    walk(root, root, apfs, pick, 0)
}

fn blocks_counted_for(pairs: &[(PathBuf, PathBuf)], also: Option<&Path>, apfs: &Apfs) -> u64 {
    let mut total = 0u64;
    for (src, dst) in pairs {
        let store = apfs
            .entry_stat(src)
            .unwrap_or_else(|err| panic!("{}: {err}", src.display()));
        let placed = apfs
            .entry_stat(dst)
            .unwrap_or_else(|err| panic!("{}: {err}", dst.display()));
        if store.compressed || also.is_some_and(|path| path == dst) {
            total = total.saturating_add(placed.blocks.saturating_mul(512));
        }
    }
    total
}

fn read_capped(path: &Path) -> Vec<u8> {
    let meta = std::fs::symlink_metadata(path).unwrap();
    assert!(
        meta.len() <= 32 * 1024 * 1024,
        "{} is larger than 32 MiB",
        path.display()
    );
    std::fs::read(path).unwrap()
}

#[test]
fn a_real_clone_of_a_compressed_entry_adds_no_unshared_bytes() {
    let filled = filled();
    let work = filled.built.repo.worktree("shared");
    let recording = Recording::over_real();
    let report = place_with(&filled, &work, &filled.rows, recording.fs(), true);
    let apfs = real_apfs();
    let pairs = cloned_pairs(&recording.log());
    assert!(pairs.len() >= 2, "the tag and a unit file were cloned");
    let mut saw_compressed = false;
    let mut saw_plain = false;
    let mut still_compressed = 0u64;
    let mut unshared = 0u64;
    for (src, dst) in &pairs {
        let store = apfs
            .entry_stat(src)
            .unwrap_or_else(|err| panic!("{}: {err}", src.display()));
        let placed = apfs
            .entry_stat(dst)
            .unwrap_or_else(|err| panic!("{}: {err}", dst.display()));
        if store.compressed {
            saw_compressed = true;
            if placed.compressed {
                still_compressed += 1;
            }
            assert_eq!(
                read_capped(dst),
                read_capped(src),
                "{} bytes differ from {}",
                dst.display(),
                src.display()
            );
            unshared = unshared.saturating_add(placed.blocks.saturating_mul(512));
        } else if placed.blocks > 0 {
            saw_plain = true;
            let clone = apfs
                .sharing(dst)
                .unwrap_or_else(|err| panic!("{}: {err}", dst.display()));
            let stored = apfs
                .sharing(src)
                .unwrap_or_else(|err| panic!("{}: {err}", src.display()));
            assert!(
                clone.shares_all,
                "{} does not share every block",
                dst.display()
            );
            assert_eq!(
                clone.clone_id,
                stored.clone_id,
                "{} clone id differs from {}",
                dst.display(),
                src.display()
            );
        }
    }
    assert!(saw_compressed, "a placed file's store file is compressed");
    assert!(
        saw_plain,
        "a placed file that was not compressed and allocates blocks is missing; still_compressed {still_compressed}"
    );
    assert_eq!(
        still_compressed, 0,
        "{still_compressed} placed files are still compressed"
    );
    assert!(unshared > 0, "a compressed file's blocks were not counted");
    assert_eq!(report.bytes_not_shared, Some(unshared), "{report:?}");
}

#[test]
fn a_file_that_does_not_share_is_counted_by_its_blocks_and_never_by_private_bytes() {
    let filled = filled();
    let (report, dst, apfs, pairs) = place_one_overridden(
        &filled,
        "not-shared",
        |stat| stat.blocks > 0,
        |store| Sharing {
            private_bytes: 0,
            clone_id: store.clone_id,
            may_share: true,
            shares_all: false,
        },
    );
    let blocks = apfs.entry_stat(&dst).unwrap().blocks;
    assert!(blocks > 0, "{} allocates no blocks", dst.display());
    let expected = blocks_counted_for(&pairs, Some(&dst), &apfs);
    assert!(expected >= blocks * 512, "{expected} omits {dst:?}");
    assert_eq!(report.bytes_not_shared, Some(expected), "{report:?}");
    assert_ne!(
        report.bytes_not_shared,
        Some(expected - blocks * 512),
        "the receipt used the override's private bytes of 0 for {}",
        dst.display()
    );
}

#[test]
fn a_different_clone_id_is_not_shared() {
    let filled = filled();
    let (report, dst, apfs, pairs) = place_one_overridden(
        &filled,
        "other-id",
        |stat| !stat.compressed && stat.blocks > 0,
        |store| Sharing {
            private_bytes: 0,
            clone_id: store.clone_id.wrapping_add(1),
            may_share: true,
            shares_all: true,
        },
    );
    let blocks = apfs.entry_stat(&dst).unwrap().blocks;
    assert!(blocks > 0, "{} allocates no blocks", dst.display());
    let expected = blocks_counted_for(&pairs, Some(&dst), &apfs);
    assert!(expected >= blocks * 512, "{expected} omits {dst:?}");
    assert_eq!(report.bytes_not_shared, Some(expected), "{report:?}");
}

#[test]
fn a_failed_sharing_read_is_unmeasured_not_zero() {
    let filled = filled();
    let (_, rel) = heavy_store_file(&filled);
    let work = filled.built.repo.worktree("share-unread");
    let dst = work.join("target").join(rel);
    let recording = Recording::over_real().fail_sharing(&dst);
    let report = place_with(&filled, &work, &filled.rows, recording.fs(), true);
    assert!(report.cloned >= 1, "{report:?}");
    assert_eq!(report.bytes_not_shared, None, "{report:?}");
    assert_ne!(report.bytes_not_shared, Some(0));
    assert!(
        report.unmeasured.iter().any(|(field, reason)| {
            field == "clone_bytes_not_shared"
                && reason.contains(&dst.display().to_string())
                && reason.contains("sharing attributes could not be read")
        }),
        "{:?}",
        report.unmeasured
    );
    assert!(report.store_bytes.is_some(), "{report:?}");
}

#[test]
fn with_accounting_off_the_number_is_unmeasured() {
    let filled = filled();
    let work = filled.built.repo.worktree("no-account");
    let recording = Recording::over_real();
    let report = place_with(&filled, &work, &filled.rows, recording.fs(), false);
    assert!(report.cloned >= 1, "{report:?}");
    assert_eq!(report.bytes_not_shared, None, "{report:?}");
    assert_eq!(
        report
            .unmeasured
            .iter()
            .find(|(field, _)| field == "clone_bytes_not_shared"),
        Some(&(
            "clone_bytes_not_shared".to_string(),
            "doctor check (c) did not pass".to_string(),
        )),
        "{:?}",
        report.unmeasured
    );
    assert!(report.store_bytes.unwrap_or(0) > 0, "{report:?}");
}

#[test]
fn store_bytes_counts_allocated_blocks() {
    let filled = filled();
    let work = filled.built.repo.worktree("store-bytes");
    let recording = Recording::over_real();
    let report = place_with(&filled, &work, &filled.rows, recording.fs(), true);
    let store_root = store::store_dir(&open_state(&filled));
    let apfs = real_apfs();
    let files = regular_files(&store_root);
    assert!(!files.is_empty(), "the store has a file");
    let mut compressed_blocks = None;
    for path in &files {
        let sharing = apfs
            .sharing(path)
            .unwrap_or_else(|err| panic!("{}: {err}", path.display()));
        assert_eq!(
            sharing.private_bytes,
            0,
            "{} private bytes are not zero",
            path.display()
        );
        let stat = apfs.entry_stat(path).unwrap();
        if stat.compressed && stat.blocks > 0 {
            compressed_blocks = Some(stat.blocks);
        }
    }
    let compressed_blocks = compressed_blocks.expect("a compressed store file allocates blocks");
    let got = report.store_bytes.expect("the store size is measured");
    assert!(got > 0, "{report:?}");
    assert!(
        got >= compressed_blocks * 512,
        "store size {got} is below one compressed file's {} blocks",
        compressed_blocks
    );
    assert_eq!(got, allocated_from_metadata(&store_root), "{report:?}");
}
