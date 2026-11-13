//! Seed unit tests that reach the liveness algorithm through `holds` or
//! `live_registry_units`. They pass the real `gc::liveness::liveness`.

#![allow(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    reason = "test fixtures"
)]

use cargo_hive::caps::DEPS_CAP;
use cargo_hive::cargo_target::{Owner, TargetDir};
use cargo_hive::ctx::Ctx;
use cargo_hive::fingerprint::{self, Format, le_hex, read_profile};
use cargo_hive::seed::source::{Skip, SourceUnit, holds, live_registry_units, unit_key};
use cargo_hive::state::{Config, State};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct Scratch(PathBuf);

impl Scratch {
    fn new(label: &str) -> Scratch {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = PathBuf::from(format!(
            "/private/tmp/hive-it-seed-{label}-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const HOUR: Duration = Duration::from_secs(3600);

const DAY: Duration = Duration::from_secs(86_400);

struct Env {
    state: State,
    cfg: Config,
    platform: cargo_hive::platform::Platform,
    now: SystemTime,
}

impl Env {
    fn new(scratch: &Path) -> Env {
        Env {
            state: State::open_at(scratch.join("hive")).unwrap(),
            cfg: Config {
                secondary_roots: Vec::new(),
                cache_roots: Vec::new(),
                gc_window_days: None,
            },
            platform: cargo_hive::platform::current().unwrap(),
            now: SystemTime::now(),
        }
    }

    fn ctx(&self) -> Ctx<'_> {
        Ctx {
            state: &self.state,
            fs: self.platform.fs.as_ref(),
            cfg: &self.cfg,
            now: self.now,
            account_bytes: false,
            repos: &[],
            mains: &[],
            liveness: cargo_hive::gc::liveness::liveness,
        }
    }
}

fn touch(path: &Path, now: SystemTime, age: Duration) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = fs::File::create(path).unwrap();
    file.set_modified(now - age).unwrap();
}

fn write_unit(
    profile: &Path,
    name: &str,
    stem: &str,
    fp: u64,
    deps: &[(&str, u64, u64)],
    now: SystemTime,
    age: Duration,
) {
    let dir = profile.join(".fingerprint").join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(stem), le_hex(fp)).unwrap();
    let deps: Vec<String> = deps
        .iter()
        .map(|(n, p, h)| format!("[{p},\"{n}\",false,{h}]"))
        .collect();
    fs::write(
        dir.join(format!("{stem}.json")),
        format!(
            "{{\"rustc\":1,\"features\":\"[]\",\"declared_features\":\"[]\",\"target\":2,\"profile\":3,\"path\":4,\"deps\":[{}],\"local\":[],\"rustflags\":[],\"config\":5,\"compile_kind\":0}}",
            deps.join(",")
        ),
    )
    .unwrap();
    let stamp = cargo_hive::liveness::unit_timestamp(profile, name, &fingerprint::kind_of(stem));
    touch(&stamp, now, age);
}

fn empty_hash(profile: &Path, name: &str, stem: &str, now: SystemTime, age: Duration) {
    let hash = profile.join(".fingerprint").join(name).join(stem);
    fs::write(&hash, "").unwrap();
    fs::File::options()
        .write(true)
        .open(&hash)
        .unwrap()
        .set_modified(now - age)
        .unwrap();
}

fn write_dep_info(profile: &Path, stem: &str, dir_name: &str, first_input: &str) -> PathBuf {
    let hash = fingerprint::unit_hash(dir_name).unwrap();
    let kind = fingerprint::kind_of(stem);
    let crate_name = stem
        .strip_prefix(&format!("{kind}-"))
        .filter(|name| !name.is_empty())
        .unwrap_or(stem);
    let deps = profile.join("deps");
    fs::create_dir_all(&deps).unwrap();
    let name = format!("{crate_name}-{hash}.d");
    let path = deps.join(&name);
    fs::write(&path, format!("{name}: {first_input}\n")).unwrap();
    path
}

fn registry_src(home: &Path) -> String {
    home.join("registry/src/index.crates.io-1/a-1.0.0/src/lib.rs")
        .display()
        .to_string()
}

fn target_of(root: &Path, profiles: &[&Path]) -> TargetDir {
    TargetDir {
        path: root.to_path_buf(),
        owner: Owner::Unresolved {
            reason: "test".into(),
        },
        profiles: profiles.iter().map(|p| read_profile(p)).collect(),
        size: None,
        unreadable: None,
    }
}

fn read_units(
    env: &Env,
    root: &Path,
    profiles: &[&Path],
    home: &Path,
) -> Result<(Vec<SourceUnit>, Vec<String>), Skip> {
    live_registry_units(&env.ctx(), &target_of(root, profiles), home, 6 * HOUR)
}

fn by_dir(units: &[SourceUnit]) -> std::collections::BTreeMap<&str, &SourceUnit> {
    units.iter().map(|u| (u.dir_name.as_str(), u)).collect()
}

fn hash_file(profile: &Path, dir_name: &str, stem: &str) -> String {
    fs::read_to_string(profile.join(".fingerprint").join(dir_name).join(stem)).unwrap()
}

#[test]
fn a_registry_lib_dep_info_is_named_by_the_package_not_the_fingerprint_stem() {
    let scratch = Scratch::new("pkg-dep");
    let env = Env::new(&scratch.0);
    let home = scratch.0.join("cargo-home");
    let profile = scratch.0.join("debug");
    let dir = "depcrate-cccccccccccccccc";
    let stem = "lib-depcrate";
    write_unit(&profile, dir, stem, 10, &[], env.now, HOUR);
    let hash = fingerprint::unit_hash(dir).unwrap();
    let deps = profile.join("deps");
    fs::create_dir_all(&deps).unwrap();
    let name = format!("depcrate-{hash}.d");
    fs::write(
        deps.join(&name),
        format!("{name}: {}\n", registry_src(&home)),
    )
    .unwrap();
    assert!(!deps.join(format!("lib-depcrate-{hash}.d")).exists());
    let (units, left_out) = read_units(&env, &scratch.0, &[&profile], &home).unwrap();
    assert!(left_out.is_empty(), "{left_out:?}");
    assert_eq!(units.len(), 1, "{units:?}");
    assert_eq!(units[0].dir_name, dir);
    assert_eq!(units[0].stem, stem);
    assert_eq!(units[0].kind, "lib");
    assert_eq!(units[0].fingerprint_hash, hash_file(&profile, dir, stem));
}

#[test]
fn a_workspace_lib_named_by_its_package_is_not_emitted_for_a_registry_edge() {
    let scratch = Scratch::new("ws-pkg");
    let env = Env::new(&scratch.0);
    let home = scratch.0.join("cargo-home");
    let profile = scratch.0.join("debug");
    let reg = "reg-bbbbbbbbbbbbbbbb";
    let ws = "workpkg-aaaaaaaaaaaaaaaa";
    write_unit(&profile, reg, "lib-reg", 10, &[], env.now, HOUR);
    write_unit(
        &profile,
        ws,
        "lib-workpkg",
        11,
        &[("reg", 1, 10)],
        env.now,
        HOUR,
    );
    let reg_hash = fingerprint::unit_hash(reg).unwrap();
    let build = profile.join("build").join(reg);
    fs::create_dir_all(&build).unwrap();
    let build_name = format!("output-{reg_hash}.d");
    fs::write(
        build.join(&build_name),
        format!("{build_name}: {}\n", registry_src(&home)),
    )
    .unwrap();
    let ws_hash = fingerprint::unit_hash(ws).unwrap();
    let deps = profile.join("deps");
    fs::create_dir_all(&deps).unwrap();
    let ws_name = format!("workpkg-{ws_hash}.d");
    fs::write(deps.join(&ws_name), format!("{ws_name}: src/lib.rs\n")).unwrap();
    assert!(!deps.join(format!("lib-workpkg-{ws_hash}.d")).exists());
    let (units, left_out) = read_units(&env, &scratch.0, &[&profile], &home).unwrap();
    assert!(left_out.is_empty(), "{left_out:?}");
    let got = by_dir(&units);
    assert!(!got.contains_key(ws), "{got:?}");
    assert!(got.contains_key(reg), "{got:?}");
    assert_eq!(got[reg].stem, "lib-reg");
}

#[test]
fn a_unit_with_no_dep_info_needs_a_resolved_edge_and_only_registry_edges() {
    let scratch = Scratch::new("edges");
    let env = Env::new(&scratch.0);
    let home = scratch.0.join("cargo-home");
    let profile = scratch.0.join("debug");
    let reg = "reg-aaaaaaaaaaaaaaaa";
    let ws = "ws-bbbbbbbbbbbbbbbb";
    let rbs0 = "rbs0-cccccccccccccccc";
    let rbsr = "rbsr-dddddddddddddddd";
    let rbsw = "rbsw-eeeeeeeeeeeeeeee";
    let mid = "mid-ffffffffffffffff";
    let outer = "out-1111111111111112";
    let rbs = "run-build-script-build-script-build";
    write_unit(&profile, reg, "lib-reg", 10, &[], env.now, HOUR);
    write_unit(&profile, ws, "lib-ws", 11, &[], env.now, HOUR);
    write_unit(&profile, rbs0, rbs, 20, &[], env.now, HOUR);
    write_unit(&profile, rbsr, rbs, 21, &[("reg", 1, 10)], env.now, HOUR);
    write_unit(&profile, rbsw, rbs, 22, &[("ws", 1, 11)], env.now, HOUR);
    write_unit(&profile, mid, rbs, 23, &[("reg", 1, 10)], env.now, HOUR);
    write_unit(&profile, outer, rbs, 24, &[("mid", 1, 23)], env.now, HOUR);
    write_dep_info(&profile, "lib-reg", reg, &registry_src(&home));
    write_dep_info(&profile, "lib-ws", ws, "src/lib.rs");
    let (units, left_out) = read_units(&env, &scratch.0, &[&profile], &home).unwrap();
    let got = by_dir(&units);
    assert!(left_out.is_empty(), "{left_out:?}");
    assert_eq!(
        got.keys().copied().collect::<Vec<_>>(),
        vec![mid, outer, rbsr, reg]
    );
    let reg_hash = hash_file(&profile, reg, "lib-reg");
    let mid_hash = hash_file(&profile, mid, rbs);
    assert_eq!(got[reg].fingerprint_hash, reg_hash);
    assert_eq!(got[reg].toolchain_value, 1);
    assert_eq!(got[reg].kind, "lib");
    assert_eq!(got[reg].stem, "lib-reg");
    assert_eq!(got[reg].rel_profile, PathBuf::from("debug"));
    assert!(got[reg].registry_deps.is_empty());
    assert_eq!(got[rbsr].registry_deps, vec![reg_hash.clone()]);
    assert_eq!(got[rbsr].kind, "run-build-script");
    assert_eq!(got[mid].registry_deps, vec![reg_hash]);
    assert_eq!(got[outer].registry_deps, vec![mid_hash]);
    assert_eq!(got[outer].fingerprint_hash, hash_file(&profile, outer, rbs));
}

#[test]
fn a_live_unit_with_an_unresolved_edge_is_left_out_and_named() {
    let scratch = Scratch::new("unresolved");
    let env = Env::new(&scratch.0);
    let home = scratch.0.join("cargo-home");
    let profile = scratch.0.join("debug");
    let open = "a-1111111111111111";
    let learned = "d-4444444444444444";
    let liba = "liba-aaaaaaaaaaaaaaa1";
    let stale = "liba-aaaaaaaaaaaaaaa2";
    write_unit(
        &profile,
        open,
        "bin-a",
        101,
        &[("liba", 7, 999)],
        env.now,
        HOUR,
    );
    write_unit(
        &profile,
        learned,
        "bin-d",
        104,
        &[("liba", 7, 201)],
        env.now,
        HOUR,
    );
    write_unit(&profile, liba, "lib-liba", 201, &[], env.now, 30 * DAY);
    write_unit(&profile, stale, "lib-liba", 1, &[], env.now, 30 * DAY);
    empty_hash(&profile, stale, "lib-liba", env.now, 30 * DAY);
    write_dep_info(&profile, "bin-a", open, &registry_src(&home));
    write_dep_info(&profile, "bin-d", learned, "src/lib.rs");
    write_dep_info(&profile, "lib-liba", liba, &registry_src(&home));
    let (units, left_out) = read_units(&env, &scratch.0, &[&profile], &home).unwrap();
    assert_eq!(left_out, vec![open.to_string()]);
    let got = by_dir(&units);
    assert_eq!(got.keys().copied().collect::<Vec<_>>(), vec![liba]);
    assert_eq!(
        got[liba].fingerprint_hash,
        hash_file(&profile, liba, "lib-liba")
    );
    assert!(got[liba].registry_deps.is_empty());
}

#[test]
fn a_not_current_unit_is_not_filled() {
    let scratch = Scratch::new("not-current");
    let env = Env::new(&scratch.0);
    let home = scratch.0.join("cargo-home");
    let profile = scratch.0.join("debug");
    let current = "lib-aaaaaaaaaaaaaaaa";
    let rebuilding = "nc-bbbbbbbbbbbbbbbb";
    write_unit(&profile, current, "lib-cur", 10, &[], env.now, HOUR);
    write_unit(&profile, rebuilding, "lib-nc", 11, &[], env.now, HOUR);
    empty_hash(&profile, rebuilding, "lib-nc", env.now, HOUR);
    write_dep_info(&profile, "lib-cur", current, &registry_src(&home));
    let (units, left_out) = read_units(&env, &scratch.0, &[&profile], &home).unwrap();
    assert!(left_out.is_empty(), "{left_out:?}");
    assert_eq!(
        units
            .iter()
            .map(|u| u.dir_name.as_str())
            .collect::<Vec<_>>(),
        vec![current]
    );
}

#[test]
fn an_undecided_or_unsupported_target_is_a_skip_with_its_reason() {
    let scratch = Scratch::new("skip");
    let env = Env::new(&scratch.0);
    let home = scratch.0.join("cargo-home");
    let bare = scratch.0.join("release");
    fs::create_dir_all(&bare).unwrap();
    let err = read_units(&env, &scratch.0, &[&bare], &home).unwrap_err();
    assert!(
        matches!(err, Skip::Unsupported(ref why) if why.contains("no .fingerprint/")),
        "{err:?}"
    );

    let fresh = Scratch::new("undecided");
    let env = Env::new(&fresh.0);
    let profile = fresh.0.join("debug");
    let dir = profile.join(".fingerprint").join("a-1111111111111111");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("output-lib-a"), "").unwrap();
    touch(&dir.join("invoked.timestamp"), env.now, HOUR);
    let err = read_units(&env, &fresh.0, &[&profile], &home).unwrap_err();
    assert!(
        matches!(err, Skip::Undecided(ref why) if why.contains("never compiled")),
        "{err:?}"
    );
}

#[test]
fn a_deps_listing_over_its_cap_is_a_skip() {
    let scratch = Scratch::new("deps-cap");
    let env = Env::new(&scratch.0);
    let profile = scratch.0.join("debug");
    write_unit(
        &profile,
        "lib-aaaaaaaaaaaaaaaa",
        "lib-a",
        10,
        &[],
        env.now,
        HOUR,
    );
    let deps = profile.join("deps");
    fs::create_dir_all(&deps).unwrap();
    let n = DEPS_CAP + 1;
    let threads = std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(4)
        .clamp(1, 8);
    std::thread::scope(|scope| {
        for thread in 0..threads {
            let deps = deps.clone();
            scope.spawn(move || {
                for i in (thread..n).step_by(threads) {
                    fs::File::create(deps.join(format!("f{i}"))).unwrap();
                }
            });
        }
    });
    let err = read_units(&env, &scratch.0, &[&profile], &scratch.0.join("cargo-home")).unwrap_err();
    assert!(
        matches!(err, Skip::Bound(ref why) if why.contains("more than") && why.contains("build the target again")),
        "{err:?}"
    );
}

#[test]
fn a_dep_info_file_over_its_read_bound_skips_the_target() {
    let home_root = Scratch::new("dep-bound-home");
    let home = home_root.0.join("cargo-home");
    let input = registry_src(&home);
    let bound = 1024 * 1024;

    let at = Scratch::new("dep-bound-at");
    let env = Env::new(&at.0);
    let profile = at.0.join("debug");
    let dir = "lib-aaaaaaaaaaaaaaaa";
    write_unit(&profile, dir, "lib-a", 10, &[], env.now, HOUR);
    let path = write_dep_info(&profile, "lib-a", dir, &input);
    let mut body = fs::read(&path).unwrap();
    body.resize(bound, b'x');
    fs::write(&path, body).unwrap();
    let (units, left_out) = read_units(&env, &at.0, &[&profile], &home).unwrap();
    assert!(left_out.is_empty());
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].dir_name, dir);

    let over = Scratch::new("dep-bound-over");
    let env = Env::new(&over.0);
    let profile = over.0.join("debug");
    write_unit(&profile, dir, "lib-a", 10, &[], env.now, HOUR);
    let path = write_dep_info(&profile, "lib-a", dir, &input);
    let mut body = fs::read(&path).unwrap();
    body.resize(bound + 1, b'x');
    fs::write(&path, body).unwrap();
    let err = read_units(&env, &over.0, &[&profile], &home).unwrap_err();
    assert!(
        matches!(err, Skip::Bound(ref why) if why.contains("1 MiB") && why.contains("build the target again") && why.contains(&path.display().to_string())),
        "{err:?}"
    );
}

#[test]
fn an_undecided_target_holds_every_graph_unit_key_and_not_current_name_meta() {
    let scratch = Scratch::new("holds-undecided");
    let env = Env::new(&scratch.0);
    let profile = scratch.0.join("debug");
    let current = "lib-aaaaaaaaaaaaaaaa";
    let rebuilding = "nc-bbbbbbbbbbbbbbbb";
    write_unit(&profile, current, "lib-cur", 10, &[], env.now, HOUR);
    let nc_dir = profile.join(".fingerprint").join(rebuilding);
    fs::create_dir_all(&nc_dir).unwrap();
    fs::write(nc_dir.join("output-lib-nc"), "").unwrap();
    touch(&nc_dir.join("invoked.timestamp"), env.now, HOUR);
    let target = target_of(&scratch.0, &[&profile]);
    let got = holds(
        env.platform.fs.as_ref(),
        std::slice::from_ref(&target),
        env.now,
        6 * HOUR,
        cargo_hive::gc::liveness::liveness,
    );
    assert!(got.whole_store.is_empty(), "{got:?}");
    let graph_key = unit_key(
        target.profiles[0]
            .graph
            .as_ref()
            .unwrap()
            .units
            .iter()
            .find(|u| u.dir_name == current)
            .unwrap(),
    );
    assert!(got.keys.contains(&graph_key), "{got:?}");
    assert!(got.name_metas.contains(rebuilding), "{got:?}");
    assert!(
        got.findings
            .iter()
            .any(|f| f.kind == "store.target_undecided"
                && f.path.as_ref() == Some(&target.path)
                && f.detail.contains(
                    "Cargo builds the never compiled unit or abandons it, or its timestamp ages out of the window"
                )),
        "{got:?}"
    );
    assert_eq!(got.by_reason.get("undecided"), Some(&1), "{got:?}");
    assert!(
        got.by_reason.get("name_meta").copied().unwrap_or(0) >= 1,
        "{got:?}"
    );
}

#[test]
fn an_unsupported_profile_with_fingerprint_holds_each_directory_name() {
    let scratch = Scratch::new("holds-unsup-fp");
    let env = Env::new(&scratch.0);
    let profile = scratch.0.join("debug");
    let kept = "kept-cccccccccccccccc";
    let other = "other-dddddddddddddddd";
    for name in [kept, other] {
        let dir = profile.join(".fingerprint").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("invoked.timestamp"), "").unwrap();
        fs::write(dir.join("lib-x.json"), "{}").unwrap();
    }
    let target = target_of(&scratch.0, &[&profile]);
    assert!(
        matches!(target.profiles[0].format, Format::Unsupported(_)),
        "{:?}",
        target.profiles[0].format
    );
    let got = holds(
        env.platform.fs.as_ref(),
        &[target],
        env.now,
        6 * HOUR,
        cargo_hive::gc::liveness::liveness,
    );
    assert!(got.whole_store.is_empty(), "{got:?}");
    assert!(got.keys.is_empty(), "{got:?}");
    assert_eq!(
        got.name_metas,
        [kept, other].into_iter().map(str::to_string).collect(),
        "{got:?}"
    );
}

#[test]
fn an_unsupported_profile_without_fingerprint_holds_nothing() {
    let scratch = Scratch::new("holds-unsup-bare");
    let env = Env::new(&scratch.0);
    let profile = scratch.0.join("release");
    fs::create_dir_all(&profile).unwrap();
    let target = target_of(&scratch.0, &[&profile]);
    assert!(
        matches!(target.profiles[0].format, Format::Unsupported(_)),
        "{:?}",
        target.profiles[0].format
    );
    let got = holds(
        env.platform.fs.as_ref(),
        &[target],
        env.now,
        6 * HOUR,
        cargo_hive::gc::liveness::liveness,
    );
    assert!(got.whole_store.is_empty(), "{got:?}");
    assert!(got.keys.is_empty(), "{got:?}");
    assert!(got.name_metas.is_empty(), "{got:?}");
}

#[test]
fn an_unreadable_target_holds_the_whole_store() {
    let scratch = Scratch::new("holds-unreadable");
    let env = Env::new(&scratch.0);
    let mut target = target_of(&scratch.0, &[]);
    target.unreadable = Some("permission denied".into());
    let got = holds(
        env.platform.fs.as_ref(),
        &[target.clone()],
        env.now,
        6 * HOUR,
        cargo_hive::gc::liveness::liveness,
    );
    assert_eq!(got.whole_store.len(), 1, "{got:?}");
    assert_eq!(got.whole_store[0].kind, "store.target_unreadable");
    assert_eq!(got.whole_store[0].path.as_ref(), Some(&target.path));
    assert!(got.keys.is_empty(), "{got:?}");
    assert_eq!(got.by_reason.get("whole_store"), Some(&1));
}

#[test]
fn a_failed_fingerprint_deps_or_examples_listing_holds_the_whole_store() {
    {
        let scratch = Scratch::new("holds-list-fp");
        let env = Env::new(&scratch.0);
        let profile = scratch.0.join("debug");
        write_unit(
            &profile,
            "lib-aaaaaaaaaaaaaaaa",
            "lib-a",
            1,
            &[],
            env.now,
            HOUR,
        );
        let blocked = profile.join(".fingerprint");
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();
        let target = target_of(&scratch.0, &[&profile]);
        let got = holds(
            env.platform.fs.as_ref(),
            &[target],
            env.now,
            6 * HOUR,
            cargo_hive::gc::liveness::liveness,
        );
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!got.whole_store.is_empty(), "fingerprint: {got:?}");
    }

    for area in ["deps", "examples"] {
        let scratch = Scratch::new(&format!("holds-list-{area}"));
        let env = Env::new(&scratch.0);
        let profile = scratch.0.join("debug");
        write_unit(
            &profile,
            "dead-aaaaaaaaaaaaaaaa",
            "lib-dead",
            1,
            &[],
            env.now,
            30 * DAY,
        );
        write_unit(
            &profile,
            "live-bbbbbbbbbbbbbbbb",
            "lib-live",
            2,
            &[],
            env.now,
            HOUR,
        );
        fs::create_dir_all(profile.join("deps")).unwrap();
        fs::create_dir_all(profile.join("examples")).unwrap();
        let blocked = profile.join(area);
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o000)).unwrap();
        let target = target_of(&scratch.0, &[&profile]);
        let got = holds(
            env.platform.fs.as_ref(),
            &[target],
            env.now,
            6 * HOUR,
            cargo_hive::gc::liveness::liveness,
        );
        fs::set_permissions(&blocked, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(
            !got.whole_store.is_empty(),
            "{area}: expected whole_store, got {got:?}"
        );
        assert!(
            got.findings
                .iter()
                .any(|f| f.kind == "store.target_undecided"),
            "{area}: {got:?}"
        );
    }
}

#[test]
fn a_lock_that_cannot_be_read_is_skipped_and_fills_nothing() {
    let scratch = Scratch::new("lock-failed");
    let target_path = scratch.0.join("target");
    std::fs::create_dir_all(&target_path).unwrap();
    std::fs::write(
        target_path.join("CACHEDIR.TAG"),
        "# This file is a cache directory tag created by cargo.\n",
    )
    .unwrap();
    let mode = std::os::unix::fs::PermissionsExt::from_mode(0o111);
    std::fs::set_permissions(&target_path, mode).unwrap();
    struct Restore(PathBuf);
    impl Drop for Restore {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(
                &self.0,
                std::os::unix::fs::PermissionsExt::from_mode(0o755),
            );
        }
    }
    let _restore = Restore(target_path.clone());
    let home = scratch.0.join("home");
    let state = cargo_hive::state::State::open_at(home).unwrap();
    let platform = cargo_hive::platform::current().unwrap();
    let cfg = cargo_hive::state::Config {
        secondary_roots: Vec::new(),
        cache_roots: Vec::new(),
        gc_window_days: None,
    };
    let repo = cargo_hive::registry::Repo {
        main: scratch.0.clone(),
        root_commits: std::collections::BTreeSet::new(),
    };
    let repos = [repo.clone()];
    let mains = [scratch.0.clone()];
    let ctx = cargo_hive::ctx::Ctx {
        state: &state,
        fs: platform.fs.as_ref(),
        cfg: &cfg,
        now: SystemTime::UNIX_EPOCH,
        account_bytes: true,
        repos: &repos,
        mains: &mains,
        liveness: cargo_hive::gc::liveness::liveness,
    };
    let target = TargetDir {
        path: target_path.clone(),
        owner: Owner::Owned {
            checkout: scratch.0.clone(),
        },
        profiles: Vec::new(),
        size: None,
        unreadable: None,
    };
    let report = cargo_hive::seed::fill::fill(&ctx, &repo, &[target], &|| false).unwrap();
    assert_eq!(report.targets_skipped.get("lock.failed").copied(), Some(1));
    assert_eq!(report.entries_added, 0);
    assert_eq!(report.entries_held, 0);
    assert!(
        report
            .findings
            .iter()
            .any(|(kind, _, _)| kind == "lock.failed")
    );
    assert!(
        cargo_hive::seed::store::read(&state)
            .unwrap()
            .units
            .is_empty()
    );
}
