#![cfg(target_os = "macos")]
mod common;
use cargo_hive::liveness::{Liveness, Uncertain, UnitRef};
use common::cargo_fixture::*;
use common::Scratch;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

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
    ws.cargo(&["build", "--workspace"]);
    ws.break_source("l");
    ws.cargo_fails(&["build", "--bin", "c"]);
    ws
}

/// `n` <- `m` <- `app`, and `m` also depends on `k`; `m` is touched and `n` broken, then
/// `cargo build -p m` fails before `m` starts.
pub fn never_started(label: &str) -> Busy {
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
    let ws = workspace(
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
    }
    ws.cargo(&["build", "-p", "app"]);
    ws.break_source("liba");
    ws.cargo_fails(&["build", "-p", "app"]);
    ws
}

/// Proc-macro `pm` used by bin `b` and lib `o`; built for the host triple and once in release,
/// then `pm` is broken and `cargo build --target <host> -p o` fails.
pub fn host_dep(label: &str) -> Busy {
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
    ws.cargo(&["build", "--workspace", "--target", &triple]);
    ws.cargo(&["build", "--workspace", "--release"]);
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
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_none() && p.file_name().unwrap() != "invoked.timestamp")
        .expect("a hash file")
}

fn hash_file_mtime(unit: &UnitRef) -> SystemTime {
    fs::metadata(hash_file(unit)).unwrap().modified().unwrap()
}

fn has_empty_hash_file(unit: &UnitRef) -> bool {
    let dir = unit.profile.join(".fingerprint").join(&unit.dir_name);
    fs::read_dir(dir).unwrap().any(|e| {
        let e = e.unwrap();
        e.file_name() != "invoked.timestamp"
            && e.path().extension().is_none()
            && e.metadata().unwrap().len() == 0
    })
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

    let (live, _) = decided(&ws, SystemTime::now(), WINDOW);

    assert!(live.contains(&broken), "{broken:?} not in {live:#?}");
    assert!(live.contains(&check) && live.contains(&b), "{live:#?}");
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

    let (live, _) = decided(&ws, SystemTime::now(), WINDOW);

    assert!(live.contains(&m), "{m:?} not in {live:#?}");
    assert!(live.contains(&k), "the build-mode k is not live: {live:#?}");
    assert!(
        !live.contains(&app),
        "the backdated bin should not be live: {live:#?}"
    );
}

#[test]
fn a_renamed_dependency_resolves_through_a_checked_sibling() {
    let ws = renamed_dep("busy-renamed", true);

    let liba = reached_not_current(&ws, "liba", |_| true);

    let (live, _) = decided(&ws, SystemTime::now(), WINDOW);

    assert!(live.contains(&liba), "{liba:?} not in {live:#?}");
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

#[test]
fn a_host_dependency_not_current_under_a_target_triple_stays_live() {
    let ws = host_dep("busy-host");
    let triple = host_triple();

    let pm = reached_not_current(&ws, "pm", |p| {
        !p.to_string_lossy().contains(&triple) && p.ends_with("debug")
    });
    let b = current(&ws, "bin", "b", Some(&triple));

    let (live, _) = decided(&ws, SystemTime::now(), WINDOW);

    assert!(live.contains(&pm), "{pm:?} not in {live:#?}");
    assert!(live.contains(&b), "{b:?} not in {live:#?}");
}
