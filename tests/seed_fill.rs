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
