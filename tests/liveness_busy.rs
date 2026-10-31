#![cfg(target_os = "macos")]
mod common;
use cargo_hive::liveness::{Liveness, Uncertain, UnitRef};
use cargo_hive::platform;
use common::cargo_fixture::*;
use common::prove::{self, WindowOutcome};
use common::Scratch;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const DIR_CAP: usize = 64;
const TWO_DAYS: Duration = Duration::from_secs(2 * 86_400);

/// A path-only workspace driven by real Cargo into a busy state.
pub struct Busy {
    pub _scratch: Scratch,
    pub dir: PathBuf,
    pub target: PathBuf,
    pub logs: PathBuf,
    /// The commands that rebuild the workspace once the source is restored.
    pub rebuild: Vec<Vec<String>>,
    /// The source edit that restores a buildable workspace: the file and its original text.
    pub restore: (PathBuf, String),
    /// The units a `cargo check` of the workspace made, before any build.
    pub checked: BTreeSet<UnitRef>,
}

impl Busy {
    fn cargo(&self, args: &[&str]) {
        assert!(
            cargo_in(&self.dir, &self.target, &self.logs, args),
            "cargo {args:?} failed: {}",
            fs::read_to_string(self.logs.join("stderr")).unwrap()
        );
    }

    fn cargo_fails(&self, args: &[&str]) {
        assert!(
            !cargo_in(&self.dir, &self.target, &self.logs, args),
            "cargo {args:?} should have failed"
        );
    }

    fn src(&self, package: &str) -> PathBuf {
        self.dir.join(package).join("src/lib.rs")
    }

    fn break_source(&self, package: &str) {
        let path = self.src(package);
        let original = fs::read_to_string(&path).unwrap();
        fs::write(&path, format!("{original}\nfn broken( {{\n")).unwrap();
    }
}

fn age_if(aged: bool, target: &Path) {
    if aged {
        backdate_all(target, THIRTY_DAYS);
    }
}

fn commands(list: &[&[&str]]) -> Vec<Vec<String>> {
    list.iter()
        .map(|c| c.iter().map(|a| a.to_string()).collect())
        .collect()
}

/// Writes a workspace of `(name, dependencies, proc_macro)` library members and `(name,
/// dependencies)` binary members under a fresh scratch dir.
fn workspace(
    label: &str,
    libs: &[(&str, &str, bool)],
    bins: &[(&str, &str)],
    rebuild: Vec<Vec<String>>,
    broken: &str,
) -> Busy {
    let scratch = Scratch::new(label);
    let dir = scratch.path.join("ws");
    let members: Vec<String> = libs
        .iter()
        .map(|(n, _, _)| *n)
        .chain(bins.iter().map(|(n, _)| *n))
        .map(|n| format!("\"{n}\""))
        .collect();
    write(
        &dir.join("Cargo.toml"),
        &format!(
            "[workspace]\nmembers = [{}]\nresolver = \"2\"\n",
            members.join(", ")
        ),
    );
    for (name, deps, proc_macro) in libs {
        let extra = if *proc_macro {
            "[lib]\nproc-macro = true\n\n"
        } else {
            ""
        };
        write(
            &dir.join(name).join("Cargo.toml"),
            &manifest(name, &format!("{extra}[dependencies]\n{deps}")),
        );
        let body = if *proc_macro {
            "extern crate proc_macro;\nuse proc_macro::TokenStream;\n#[proc_macro]\npub fn m(_: TokenStream) -> TokenStream { TokenStream::new() }\n".to_string()
        } else {
            "pub fn f() -> u32 { 1 }\n".to_string()
        };
        write(&dir.join(name).join("src/lib.rs"), &body);
    }
    for (name, deps) in bins {
        write(
            &dir.join(name).join("Cargo.toml"),
            &manifest(name, &format!("[dependencies]\n{deps}")),
        );
        write(&dir.join(name).join("src/main.rs"), "fn main() {}\n");
    }
    let logs = scratch.path.join("logs");
    fs::create_dir_all(&logs).unwrap();
    let restore = dir.join(broken).join("src/lib.rs");
    let original = fs::read_to_string(&restore).unwrap();
    Busy {
        target: scratch.path.join("target"),
        _scratch: scratch,
        dir,
        logs,
        rebuild,
        restore: (restore, original),
        checked: BTreeSet::new(),
    }
}

/// Lib `l`, bins `b` and `c` depending on it; `l` is broken, then `cargo build --bin c` fails.
pub fn failed_build(label: &str) -> Busy {
    failed_build_aged(label, false)
}

/// As `failed_build`; with `aged`, the check-mode units are backdated 30 days before the build.
pub fn failed_build_aged(label: &str, aged: bool) -> Busy {
    let mut ws = workspace(
        label,
        &[("l", "", false)],
        &[
            ("b", "l = { path = \"../l\" }\n"),
            ("c", "l = { path = \"../l\" }\n"),
        ],
        commands(&[
            &["check", "--workspace"],
            &["build", "--workspace"],
            &["build", "--bin", "c"],
        ]),
        "l",
    );
    ws.cargo(&["check", "--workspace"]);
    ws.checked = kinds(&ws.target).into_keys().collect();
    age_if(aged, &ws.target);
    ws.cargo(&["build", "--workspace"]);
    ws.break_source("l");
    ws.cargo_fails(&["build", "--bin", "c"]);
    ws
}

/// `n` <- `m` <- `app`, and `m` also depends on `k`; `m` is touched and `n` broken, then
/// `cargo build -p m` fails before `m` starts.
pub fn never_started(label: &str) -> Busy {
    never_started_aged(label, false)
}

/// As `never_started`; with `aged`, the check-mode units are backdated 30 days before the build.
pub fn never_started_aged(label: &str, aged: bool) -> Busy {
    let mut ws = workspace(
        label,
        &[
            ("n", "", false),
            ("k", "", false),
            (
                "m",
                "n = { path = \"../n\" }\nk = { path = \"../k\" }\n",
                false,
            ),
        ],
        &[("app", "m = { path = \"../m\" }\n")],
        commands(&[
            &["check", "--workspace"],
            &["build", "--workspace"],
            &["build", "-p", "m"],
        ]),
        "n",
    );
    ws.cargo(&["check", "--workspace"]);
    ws.checked = kinds(&ws.target).into_keys().collect();
    age_if(aged, &ws.target);
    ws.cargo(&["build", "--workspace"]);
    let m = ws.src("m");
    let text = fs::read_to_string(&m).unwrap();
    fs::write(&m, format!("{text}\n// touched\n")).unwrap();
    ws.break_source("n");
    ws.cargo_fails(&["build", "-p", "m"]);
    ws
}

/// `app` depends on `liba2 = { package = "liba" }`; `tool` depends on `liba`. With `with_tool`,
/// `tool` is checked first so a current check-mode `liba` unit teaches the package id.
pub fn renamed_dep(label: &str, with_tool: bool) -> Busy {
    renamed_dep_aged(label, with_tool, false)
}

/// As `renamed_dep`; with `aged`, the units of the `tool` check are backdated 30 days before the
/// build.
pub fn renamed_dep_aged(label: &str, with_tool: bool, aged: bool) -> Busy {
    assert!(
        with_tool || !aged,
        "without `tool` there is no check-mode unit to age"
    );
    let mut rebuild = vec![vec![
        "build".to_string(),
        "-p".to_string(),
        "app".to_string(),
    ]];
    if with_tool {
        rebuild.insert(
            0,
            vec!["check".to_string(), "-p".to_string(), "tool".to_string()],
        );
    }
    let mut ws = workspace(
        label,
        &[("liba", "", false)],
        &[
            (
                "app",
                "liba2 = { path = \"../liba\", package = \"liba\" }\n",
            ),
            ("tool", "liba = { path = \"../liba\" }\n"),
        ],
        rebuild,
        "liba",
    );
    if with_tool {
        ws.cargo(&["check", "-p", "tool"]);
        ws.checked = kinds(&ws.target).into_keys().collect();
        age_if(aged, &ws.target);
    }
    ws.cargo(&["build", "-p", "app"]);
    ws.break_source("liba");
    ws.cargo_fails(&["build", "-p", "app"]);
    ws
}

/// Proc-macro `pm` used by bin `b` and lib `o`; built for the host triple and once in release,
/// then `pm` is broken and `cargo build --target <host> -p o` fails.
pub fn host_dep(label: &str) -> Busy {
    host_dep_aged(label, false)
}

/// As `host_dep`; with `aged`, the release build comes first and is backdated 30 days before the
/// build for the host triple.
pub fn host_dep_aged(label: &str, aged: bool) -> Busy {
    let triple = host_triple();
    let ws = workspace(
        label,
        &[
            ("pm", "", true),
            ("o", "pm = { path = \"../pm\" }\n", false),
        ],
        &[("b", "pm = { path = \"../pm\" }\n")],
        commands(&[
            &["build", "--workspace", "--target", &triple],
            &["build", "--workspace", "--release"],
            &["build", "--target", &triple, "-p", "o"],
        ]),
        "pm",
    );
    if aged {
        ws.cargo(&["build", "--workspace", "--release"]);
        backdate_all(&ws.target, THIRTY_DAYS);
        ws.cargo(&["build", "--workspace", "--target", &triple]);
    } else {
        ws.cargo(&["build", "--workspace", "--target", &triple]);
        ws.cargo(&["build", "--workspace", "--release"]);
    }
    ws.break_source("pm");
    ws.cargo_fails(&["build", "--target", &triple, "-p", "o"]);
    ws
}

/// Not-current unit dirs of the profiles `keep` accepts whose dir name starts with `prefix`.
fn not_current(target: &Path, prefix: &str, keep: impl Fn(&Path) -> bool) -> Vec<UnitRef> {
    let mut out = Vec::new();
    for profile in read_target(target).profiles {
        if !keep(&profile.path) {
            continue;
        }
        for unit in profile.not_current_units {
            let name = unit.dir.file_name().unwrap().to_string_lossy().into_owned();
            if name.starts_with(prefix) {
                out.push(UnitRef {
                    profile: profile.path.clone(),
                    dir_name: name,
                });
            }
        }
    }
    out
}

fn hash_file(unit: &UnitRef) -> PathBuf {
    let dir = unit.profile.join(".fingerprint").join(&unit.dir_name);
    fs::read_dir(dir)
        .unwrap()
        .take(DIR_CAP)
        .map(|e| e.unwrap().path())
        .find(|p| {
            let name = p.file_name().unwrap().to_string_lossy();
            p.extension().is_none()
                && name != "invoked.timestamp"
                && !name.starts_with("dep-")
                && !name.starts_with("output-")
        })
        .expect("a hash file")
}

fn hash_file_mtime(unit: &UnitRef) -> SystemTime {
    fs::metadata(hash_file(unit)).unwrap().modified().unwrap()
}

fn has_empty_hash_file(unit: &UnitRef) -> bool {
    fs::metadata(hash_file(unit)).unwrap().len() == 0
}

/// The one not-current unit of package `name` in the profiles `keep` accepts, proven to be at
/// the state a failed Cargo build leaves: an empty hash file.
fn reached_not_current(ws: &Busy, name: &str, keep: impl Fn(&Path) -> bool) -> UnitRef {
    let unit = only(not_current(&ws.target, &format!("{name}-"), keep));
    assert!(
        has_empty_hash_file(&unit),
        "{unit:?} has no empty hash file"
    );
    unit
}

fn decided(ws: &Busy, now: SystemTime, window: Duration) -> (BTreeSet<UnitRef>, Vec<UnitRef>) {
    match decide_at(&ws.target, now, window) {
        Liveness::Decided { live, dead } => (
            live.into_iter().collect(),
            dead.into_iter().map(|d| d.unit).collect(),
        ),
        Liveness::Undecided { reasons } => panic!("liveness was undecided: {reasons:#?}"),
    }
}

fn edge_packages(ws: &Busy, unit: &UnitRef) -> BTreeSet<u64> {
    let profile = read_target(&ws.target)
        .profiles
        .into_iter()
        .find(|p| p.path == unit.profile)
        .expect("the unit's profile");
    let current = profile
        .graph
        .into_iter()
        .flat_map(|g| g.units)
        .filter(|u| u.dir_name == unit.dir_name)
        .flat_map(|u| u.deps);
    let not_current = profile
        .not_current_units
        .into_iter()
        .filter(|u| u.dir.file_name().is_some_and(|n| *n == *unit.dir_name))
        .flat_map(|u| u.deps);
    current.chain(not_current).map(|d| d.package).collect()
}

/// Asserts the package ids `unit` depends on were all seen on edges of the `teachers`.
fn assert_learned(ws: &Busy, unit: &UnitRef, teachers: &BTreeSet<UnitRef>) {
    let wanted = edge_packages(ws, unit);
    assert!(!wanted.is_empty(), "{unit:?} has no edges");
    let taught: BTreeSet<u64> = teachers.iter().flat_map(|t| edge_packages(ws, t)).collect();
    assert!(wanted.is_subset(&taught), "{wanted:?} not in {taught:?}");
}

/// Backdates every unit timestamp and the hash file of each `aged` unit by two days.
fn age_out(ws: &Busy, aged: &[&UnitRef]) {
    backdate_all(&ws.target, TWO_DAYS);
    for unit in aged {
        set_mtime(&hash_file(unit), SystemTime::now() - TWO_DAYS);
    }
}

fn freshen(unit: &UnitRef, kind: &str) {
    set_mtime(&timestamp_of(unit, kind), SystemTime::now());
}

fn assert_split(
    live: &BTreeSet<UnitRef>,
    dead: &[UnitRef],
    expect_live: &[&UnitRef],
    expect_dead: &[&UnitRef],
) {
    for unit in expect_live {
        assert!(live.contains(*unit), "{unit:?} should be live: {live:#?}");
        assert!(
            !dead.contains(*unit),
            "{unit:?} should not be dead: {dead:#?}"
        );
    }
    for unit in expect_dead {
        assert!(dead.contains(*unit), "{unit:?} should be dead: {dead:#?}");
        assert!(
            !live.contains(*unit),
            "{unit:?} should not be live: {live:#?}"
        );
    }
}

/// A not-current unit outside the window with nothing wanting it is neither live nor dead.
fn assert_ignored(live: &BTreeSet<UnitRef>, dead: &[UnitRef], units: &[&UnitRef]) {
    for unit in units {
        assert!(
            !live.contains(*unit),
            "{unit:?} should not be live: {live:#?}"
        );
        assert!(
            !dead.contains(*unit),
            "{unit:?} should not be dead: {dead:#?}"
        );
    }
}

fn current(ws: &Busy, kind: &str, stem: &str, triple: Option<&str>) -> UnitRef {
    only(
        find(&kinds(&ws.target), kind, stem, triple)
            .into_iter()
            .filter(|u| !ws.checked.contains(u))
            .collect(),
    )
}

fn checked(ws: &Busy, kind: &str, stem: &str) -> UnitRef {
    only(
        find(&kinds(&ws.target), kind, stem, None)
            .into_iter()
            .filter(|u| ws.checked.contains(u))
            .collect(),
    )
}

#[test]
fn a_failed_build_leaves_the_library_unit_live() {
    let ws = failed_build("busy-failed");

    let broken = reached_not_current(&ws, "l", |_| true);
    let check = checked(&ws, "lib", "l");
    let b = current(&ws, "bin", "b", None);
    assert_ne!(check, broken);

    let (live, dead) = decided(&ws, SystemTime::now(), WINDOW);

    assert_split(&live, &dead, &[&broken, &check, &b], &[]);
}

#[test]
fn a_failed_build_keeps_the_library_live_through_an_unmatched_edge_alone() {
    let ws = failed_build("busy-failed-edge");
    let broken = reached_not_current(&ws, "l", |_| true);
    let b = current(&ws, "bin", "b", None);
    let teachers: BTreeSet<UnitRef> = ws.checked.iter().cloned().collect();
    assert_learned(&ws, &b, &teachers);
    age_out(&ws, &[&broken]);
    freshen(&b, "bin");

    let (live, dead) = decided(&ws, SystemTime::now(), WINDOW);

    let check_units: Vec<&UnitRef> = ws.checked.iter().collect();
    assert_split(&live, &dead, &[&broken, &b], &check_units);
}

#[test]
fn a_failed_build_with_nothing_wanted_leaves_the_library_dead() {
    let ws = failed_build("busy-failed-dead");
    let broken = reached_not_current(&ws, "l", |_| true);
    let c = reached_not_current(&ws, "c", |_| true);
    age_out(&ws, &[&broken, &c]);

    let (live, dead) = decided(&ws, SystemTime::now(), WINDOW);

    assert_ignored(&live, &dead, &[&broken]);
}

#[test]
fn a_build_that_never_started_keeps_its_dependencies_live_through_the_old_json() {
    let ws = never_started("busy-never");

    let m = reached_not_current(&ws, "m", |_| true);
    let app = current(&ws, "bin", "app", None);
    let k = current(&ws, "lib", "k", None);
    backdate_all(&ws.target, TWO_DAYS);
    let stamp = timestamp_of(&m, "lib");
    let old = SystemTime::now() - WINDOW;
    assert!(fs::metadata(&stamp).unwrap().modified().unwrap() < old);
    assert!(hash_file_mtime(&m) > old);

    let (live, dead) = decided(&ws, SystemTime::now(), WINDOW);

    let check_units: Vec<&UnitRef> = ws.checked.iter().collect();
    assert_split(&live, &dead, &[&m, &k], &[&app]);
    assert_split(&live, &dead, &[], &check_units);
}

#[test]
fn a_renamed_dependency_resolves_through_a_checked_sibling() {
    let ws = renamed_dep("busy-renamed", true);

    let liba = reached_not_current(&ws, "liba", |_| true);

    let (live, dead) = decided(&ws, SystemTime::now(), WINDOW);

    assert_split(&live, &dead, &[&liba], &[]);
}

#[test]
fn a_renamed_dependency_stays_live_through_an_unmatched_edge_alone() {
    let ws = renamed_dep("busy-renamed-edge", true);
    let liba = reached_not_current(&ws, "liba", |_| true);
    let app = reached_not_current(&ws, "app", |_| true);
    let teachers: BTreeSet<UnitRef> = ws.checked.iter().cloned().collect();
    assert_learned(&ws, &app, &teachers);
    age_out(&ws, &[&liba]);

    let (live, dead) = decided(&ws, SystemTime::now(), WINDOW);

    let check_units: Vec<&UnitRef> = ws.checked.iter().collect();
    assert_split(&live, &dead, &[&liba, &app], &check_units);
}

#[test]
fn a_renamed_dependency_with_nothing_wanted_is_dead() {
    let ws = renamed_dep("busy-renamed-dead", true);
    let liba = reached_not_current(&ws, "liba", |_| true);
    let app = reached_not_current(&ws, "app", |_| true);
    age_out(&ws, &[&liba, &app]);

    let (live, dead) = decided(&ws, SystemTime::now(), WINDOW);

    assert_ignored(&live, &dead, &[&liba, &app]);
}

#[test]
fn a_renamed_dependency_without_a_taught_package_id_is_unresolved() {
    let ws = renamed_dep("busy-renamed-alone", false);

    reached_not_current(&ws, "liba", |_| true);
    let app = reached_not_current(&ws, "app", |_| true);

    match decide_at(&ws.target, SystemTime::now(), WINDOW) {
        Liveness::Undecided { reasons } => assert!(
            reasons
                .iter()
                .any(|r| matches!(r, Uncertain::Unresolved { unit, .. } if *unit == app)),
            "{reasons:#?}"
        ),
        Liveness::Decided { .. } => panic!("expected Undecided"),
    }
}

fn host_pm(ws: &Busy, triple: &str) -> UnitRef {
    reached_not_current(ws, "pm", |p| {
        !p.to_string_lossy().contains(triple) && p.ends_with("debug")
    })
}

fn release_units(ws: &Busy) -> BTreeSet<UnitRef> {
    kinds(&ws.target)
        .into_keys()
        .filter(|u| u.profile.ends_with("release"))
        .collect()
}

#[test]
fn a_host_dependency_not_current_under_a_target_triple_stays_live() {
    let ws = host_dep("busy-host");
    let triple = host_triple();

    let pm = host_pm(&ws, &triple);
    let b = current(&ws, "bin", "b", Some(&triple));

    let (live, dead) = decided(&ws, SystemTime::now(), WINDOW);

    assert_split(&live, &dead, &[&pm, &b], &[]);
}

#[test]
fn a_host_dependency_stays_live_through_an_unmatched_edge_alone() {
    let ws = host_dep("busy-host-edge");
    let triple = host_triple();
    let pm = host_pm(&ws, &triple);
    let b = current(&ws, "bin", "b", Some(&triple));
    let release = release_units(&ws);
    let release_pm: Vec<UnitRef> = find(&kinds(&ws.target), "lib", "pm", None)
        .into_iter()
        .filter(|u| release.contains(u))
        .collect();
    assert!(!release_pm.is_empty());
    assert_learned(&ws, &b, &release);
    age_out(&ws, &[&pm]);
    freshen(&b, "bin");

    let (live, dead) = decided(&ws, SystemTime::now(), WINDOW);

    let release_pm: Vec<&UnitRef> = release_pm.iter().collect();
    assert_split(&live, &dead, &[&pm, &b], &release_pm);
}

#[test]
fn a_host_dependency_with_nothing_wanted_is_dead() {
    let ws = host_dep("busy-host-dead");
    let triple = host_triple();
    let pm = host_pm(&ws, &triple);
    let o = reached_not_current(&ws, "o", |p| p.to_string_lossy().contains(&triple));
    age_out(&ws, &[&pm, &o]);

    let (live, dead) = decided(&ws, SystemTime::now(), WINDOW);

    assert_ignored(&live, &dead, &[&pm]);
}

/// Restores the source that last compiled, deletes the dead units in a clone, replays the
/// fixture's commands, and returns the 6 h window's report.
fn replay(ws: &Busy) -> prove::WindowReport {
    fs::write(&ws.restore.0, &ws.restore.1).unwrap();
    let scratch = Scratch::new("busy-prove");
    let platform_fs = platform::current().unwrap().fs;
    let input = prove::ProveInput {
        target: ws.target.clone(),
        checkout: ws.dir.clone(),
        cmds: ws.rebuild.clone(),
        windows: vec![WINDOW],
        now: SystemTime::now(),
    };
    let report = prove::prove_with(platform_fs.as_ref(), &scratch.path, &input).unwrap();
    println!("{}", report.render());
    assert_eq!(report.baseline.rebuilt, Vec::<String>::new());
    report.windows.into_iter().next().unwrap()
}

fn assert_passed_with_dead_units(window: &prove::WindowReport) {
    assert_eq!(window.outcome, WindowOutcome::Passed, "{window:#?}");
    assert!(window.dead_units > 0, "{window:#?}");
    assert!(
        window.rebuilt > 0,
        "the deleted dead units were not rebuilt: {window:#?}"
    );
    assert_eq!(window.rebuilt_live, 0, "{window:#?}");
}

#[test]
fn deleting_the_dead_units_of_a_failed_build_rebuilds_no_live_unit() {
    let ws = failed_build_aged("busy-replay-failed", true);
    reached_not_current(&ws, "l", |_| true);

    assert_passed_with_dead_units(&replay(&ws));
}

#[test]
fn deleting_the_dead_units_of_a_build_that_never_started_rebuilds_no_live_unit() {
    let ws = never_started_aged("busy-replay-never", true);
    reached_not_current(&ws, "m", |_| true);

    assert_passed_with_dead_units(&replay(&ws));
}

#[test]
fn deleting_the_dead_units_beside_a_renamed_dependency_rebuilds_no_live_unit() {
    let ws = renamed_dep_aged("busy-replay-renamed", true, true);
    reached_not_current(&ws, "liba", |_| true);

    assert_passed_with_dead_units(&replay(&ws));
}

#[test]
fn deleting_the_dead_units_beside_a_host_dependency_rebuilds_no_live_unit() {
    let ws = host_dep_aged("busy-replay-host", true);
    let triple = host_triple();
    host_pm(&ws, &triple);

    assert_passed_with_dead_units(&replay(&ws));
}

#[test]
fn a_replay_with_nothing_dead_is_vacuous_and_rebuilds_nothing() {
    let ws = failed_build("busy-replay-unaged");
    reached_not_current(&ws, "l", |_| true);

    let window = replay(&ws);

    assert_eq!(window.dead_units, 0, "{window:#?}");
    assert_eq!(window.rebuilt, 0, "{window:#?}");
    assert_eq!(
        window.outcome,
        WindowOutcome::Vacuous("no dead unit lists a file to delete".to_string()),
        "{window:#?}"
    );
}
