//! The cold-start proof of closure liveness: clones a target dir, deletes what `liveness` calls
//! dead, replays the owner's cargo commands and reports whether any live unit was rebuilt.
//! Order of a run: lock every profile's build lock; decide and measure every window on the original;
//! clone it once as the snapshot; release the locks; replay a warm-up in a clone of the snapshot and
//! then, for each window, delete that window's dead files through `execute` on a clone of the
//! warmed-up clone and replay the owner's commands there; finally check that every file a window
//! listed is still in the original and that the checkout's `Cargo.lock` is unchanged.
//! Contract: every replay is pointed at a clone, never at the original, and every deletion lands in
//! a clone; the end-of-run check proves only that the listed files still exist and that `Cargo.lock`
//! is unchanged, not that nothing else in the original changed; every `compiler-artifact` a replay
//! reports lies inside its clone (a build script writing elsewhere is not detected); every limit
//! (deadline, output, walk, config size) is an error when hit, and nothing is silently skipped.

use cargo_hive::ctx::Ctx;
use cargo_hive::gc::liveness::liveness;
use cargo_hive::plan_tier::apply::{self, ResolvedTarget};
use cargo_hive::plan_tier::build::{self, TargetPlan};
use cargo_hive::plan_tier::guard::{Deleted, Expect, delete_checked};
use cargo_hive::plan_tier::record::{Plan, PlanClass, PlannedEntry, PlannedTarget, kind_name};
use cargo_hive::state::State;
use cargo_hive_core::fingerprint;
use cargo_hive_core::ledger::{Footprint, measure};
use cargo_hive_core::liveness::{Liveness, unit_timestamp};
use cargo_hive_core::receipt::Outcome;
use cargo_hive_core::target::{self, Owner, TargetDir};
use cargo_hive_platform::{self as platform, EntryKind, Fs, TryLock};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

const COMMAND_DEADLINE: Duration = Duration::from_secs(30 * 60);
const OUTPUT_CAP: u64 = 256 * 1024 * 1024;
const WALK_CAP: usize = 5_000_000;
const FORBIDDEN_FLAGS: [&str; 5] = [
    "--message-format",
    "--target-dir",
    "--artifact-dir",
    "--out-dir",
    "--config",
];
/// The only subcommands a replay may run: long built-in names that cargo 1.97.1 does not let an `[alias]` shadow (short names, `clippy`, `rustc` and `rustdoc` are refused). They only compile, but compiling runs the checkout's build.rs and proc-macros; all of them accept `--locked`, and `test` and `bench` also need `--no-run`, so no test, bench or binary of the checkout executes.
const ALLOWED: [&str; 5] = ["build", "check", "test", "doc", "bench"];
/// Names whose alias definition in a cargo config is refused: the allowed ones, plus the short names and external subcommands an alias could shadow (defense in depth).
const ALIAS_GUARDED: [&str; 14] = [
    "build", "b", "check", "c", "test", "t", "clippy", "doc", "d", "bench", "rustc", "rustdoc",
    "run", "r",
];
/// Short names cargo accepts, and the long name the refusal message points to.
const SHORT_NAMES: [(&str, &str); 5] = [
    ("b", "build"),
    ("c", "check"),
    ("d", "doc"),
    ("r", "run"),
    ("t", "test"),
];
/// Global flags that take no value and may precede the subcommand.
const GLOBAL_SWITCHES: [&str; 8] = [
    "-q",
    "--quiet",
    "-v",
    "-vv",
    "--verbose",
    "--locked",
    "--frozen",
    "--offline",
];
/// Config files are scanned for these keys, which redirect build output outside the clone.
const REDIRECT_KEYS: [&str; 2] = ["artifact-dir", "out-dir"];
const CONFIG_CAP: u64 = 1 << 20;
/// The owner's commands compile in the checkout: only compile steps run (build.rs and proc-macros as in the owner's own build); no test, bench or binary of the checkout is executed. Only the target dir is protected, not build.rs side effects.
///
/// `test` and `bench` need `--no-run` before any `--`, and no command may carry an argument after `--`.
pub struct ProveInput {
    pub target: PathBuf,
    pub checkout: PathBuf,
    pub cmds: Vec<Vec<String>>,
    pub windows: Vec<Duration>,
    pub now: SystemTime,
}

#[derive(Debug, Default, Clone)]
pub struct PhaseReport {
    pub rebuilt: Vec<String>,
    pub commands: Vec<(Vec<String>, i32)>,
    pub elapsed: Duration,
}

#[derive(Debug)]
pub struct ProveReport {
    pub warm_up: PhaseReport,
    pub baseline: PhaseReport,
    pub lock_held: Duration,
    pub windows: Vec<WindowReport>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowOutcome {
    Passed,
    Vacuous(String),
    RebuiltLive(Vec<String>),
}

#[derive(Debug)]
pub struct WindowReport {
    pub window: Duration,
    pub outcome: WindowOutcome,
    pub dead_units: usize,
    pub dead_bytes: Footprint,
    pub unmapped: Footprint,
    pub rebuilt: usize,
    pub rebuilt_live: usize,
    pub reasons: Vec<String>,
    pub commands: Vec<(Vec<String>, i32)>,
}

pub type Stamps = BTreeMap<(PathBuf, String), Option<SystemTime>>;
pub type Tree = BTreeMap<PathBuf, (SystemTime, u64)>;

pub struct Ran {
    pub exit: i32,
    pub artifact_lines: usize,
    pub stdout: PathBuf,
}

fn window_label(window: Duration) -> String {
    let secs = window.as_secs();
    if secs.is_multiple_of(86_400) {
        format!("{}d", secs / 86_400)
    } else if secs.is_multiple_of(3600) {
        format!("{}h", secs / 3600)
    } else {
        format!("{secs}s")
    }
}

fn footprint_line(f: &Footprint) -> String {
    format!(
        "{} B freed, {} B still shared, {} unmeasured",
        f.bytes.freed,
        f.bytes.still_shared,
        f.unmeasured.len() as u64 + f.unmeasured_more
    )
}

fn command_lines(out: &mut String, commands: &[(Vec<String>, i32)]) {
    for (cmd, exit) in commands {
        let _ = writeln!(out, "  cargo {} -> exit {exit}", cmd.join(" "));
    }
}

impl ProveReport {
    pub fn all_vacuous(&self) -> bool {
        self.windows
            .iter()
            .all(|w| matches!(w.outcome, WindowOutcome::Vacuous(_)))
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "locks held: {:.1}s", self.lock_held.as_secs_f64());
        for (name, phase) in [("warm-up", &self.warm_up), ("baseline", &self.baseline)] {
            if phase.commands.is_empty() {
                let _ = writeln!(out, "{name}: not run");
                continue;
            }
            let _ = writeln!(
                out,
                "{name}: {} rebuilt, {:.1}s",
                phase.rebuilt.len(),
                phase.elapsed.as_secs_f64()
            );
            if !phase.rebuilt.is_empty() {
                let _ = writeln!(out, "  rebuilt: {}", phase.rebuilt.join(", "));
            }
            command_lines(&mut out, &phase.commands);
        }
        for w in &self.windows {
            let verdict = match &w.outcome {
                WindowOutcome::Passed => "PASSED".to_string(),
                WindowOutcome::Vacuous(why) => format!("VACUOUS ({why})"),
                WindowOutcome::RebuiltLive(units) => {
                    format!("FAILED, rebuilt live: {}", units.join(", "))
                }
            };
            let _ = writeln!(out, "window {}: {verdict}", window_label(w.window));
            let _ = writeln!(out, "  dead units: {}", w.dead_units);
            let _ = writeln!(out, "  dead files: {}", footprint_line(&w.dead_bytes));
            let _ = writeln!(
                out,
                "  incremental and profile-root files: {}",
                footprint_line(&w.unmapped)
            );
            let _ = writeln!(
                out,
                "  rebuilt: {}, rebuilt live: {}",
                w.rebuilt, w.rebuilt_live
            );
            for reason in &w.reasons {
                let _ = writeln!(out, "  reason: {reason}");
            }
            command_lines(&mut out, &w.commands);
        }
        out.push_str("A passed window covers only the replayed commands.\n");
        out
    }
}

pub fn check_command(cmd: &[String]) -> Result<usize, String> {
    let before_dashes = cmd.iter().position(|a| a == "--").unwrap_or(cmd.len());
    for arg in &cmd[..before_dashes] {
        if let Some(flag) = FORBIDDEN_FLAGS
            .iter()
            .find(|f| arg == **f || arg.starts_with(&format!("{f}=")))
        {
            return Err(format!("carries {flag}, which the proof sets itself"));
        }
        if arg.starts_with("-Z") {
            return Err("carries -Z, which can redirect cargo's output".to_string());
        }
    }
    let mut i = 0;
    while i < before_dashes {
        let arg = cmd[i].as_str();
        if arg.starts_with('+') || GLOBAL_SWITCHES.contains(&arg) || arg.starts_with("--color=") {
            i += 1;
        } else if arg == "--color" {
            match cmd.get(i + 1).filter(|_| i + 1 < before_dashes) {
                Some(value) if !value.starts_with('-') => i += 2,
                _ => return Err("has --color without a value".to_string()),
            }
        } else if arg.starts_with('-') {
            return Err(format!(
                "has {arg} before the subcommand; only +toolchain, -q, -v, --color, --locked, --frozen and --offline may precede it"
            ));
        } else if ALLOWED.contains(&arg) {
            if let Some(bad) = cmd.get(before_dashes + 1..).and_then(|rest| rest.first()) {
                return Err(format!(
                    "passes {bad} after `--`; the replay is compile-only, so `cargo {arg}` takes no arguments there"
                ));
            }
            if matches!(arg, "test" | "bench")
                && !cmd[i + 1..before_dashes].iter().any(|a| a == "--no-run")
            {
                return Err(format!(
                    "runs `cargo {arg}` without --no-run; --no-run is required (before any `--`) so no test or benchmark of the checkout executes"
                ));
            }
            return Ok(i);
        } else if let Some((_, long)) = SHORT_NAMES.iter().find(|(short, _)| *short == arg) {
            return Err(format!(
                "runs `cargo {arg}`, a short name an alias can shadow; use `{long}`"
            ));
        } else {
            return Err(format!(
                "runs `cargo {arg}`, which is not replayable; allowed: {}",
                ALLOWED.join(", ")
            ));
        }
    }
    Err("has no cargo subcommand".to_string())
}

/// The vector cargo runs: `--locked` and `--message-format=json` go right after the subcommand. `--locked` is added and only when the vector has neither `--locked` nor `--frozen` before `--`, so a replay cannot rewrite the checkout's Cargo.lock.
pub fn replay_args(cmd: &[String]) -> Vec<String> {
    let mut out = with_json(cmd);
    let Ok(sub) = check_command(cmd) else {
        return out;
    };
    let before_dashes = cmd.iter().position(|a| a == "--").unwrap_or(cmd.len());
    let has_lock = cmd[..before_dashes]
        .iter()
        .any(|a| a == "--locked" || a == "--frozen");
    if !has_lock {
        out.insert(sub + 1, "--locked".to_string());
    }
    out
}

pub fn with_json(cmd: &[String]) -> Vec<String> {
    let at = check_command(cmd).map_or(cmd.len(), |i| i + 1);
    let mut out = cmd.to_vec();
    out.insert(at, "--message-format=json".to_string());
    out
}

/// `path` relative to the target dir, or unchanged when it is not under it.
fn rel(path: &Path, target: &Path) -> PathBuf {
    path.strip_prefix(target).unwrap_or(path).to_path_buf()
}

fn stamps_of(fs: &dyn Fs, target_path: &Path) -> Result<Stamps, String> {
    stamps(fs, &read_target(target_path)?)
}

pub fn stamps(fs: &dyn Fs, target: &TargetDir) -> Result<Stamps, String> {
    let mut out = Stamps::new();
    for profile in &target.profiles {
        let rel = rel(&profile.path, &target.path);
        let stamp =
            |dir: &str, kind: &str| fs.mtime(&unit_timestamp(&profile.path, dir, kind)).ok();
        if let Some(graph) = &profile.graph {
            for unit in &graph.units {
                out.insert(
                    (rel.clone(), unit.dir_name.clone()),
                    stamp(&unit.dir_name, &unit.kind),
                );
            }
        }
        for nc in &profile.not_current_units {
            let Some(dir) = nc.dir.file_name().and_then(|n| n.to_str()) else {
                return Err(format!(
                    "{}: directory name is not UTF-8, so it has no unit hash",
                    nc.dir.display()
                ));
            };
            let kind = nc
                .stem
                .as_deref()
                .map_or_else(|| "other".to_string(), fingerprint::kind_of);
            out.insert((rel.clone(), dir.to_string()), stamp(dir, &kind));
        }
    }
    Ok(out)
}

pub fn rebuilt(before: &Stamps, after: &Stamps) -> Vec<(PathBuf, String)> {
    after
        .iter()
        .filter(|(key, stamp)| before.get(*key) != Some(*stamp))
        .map(|(key, _)| key.clone())
        .collect()
}

fn unit_name(key: &(PathBuf, String)) -> String {
    format!("{}/{}", key.0.display(), key.1)
}

pub fn rebuilt_live(
    rebuilt: &[(PathBuf, String)],
    live: &BTreeSet<(PathBuf, String)>,
) -> Vec<String> {
    rebuilt
        .iter()
        .filter(|key| live.contains(*key))
        .map(unit_name)
        .collect()
}

pub fn mtime_mismatches(original: &Stamps, clone: &Stamps) -> Vec<(PathBuf, String)> {
    let keys: BTreeSet<_> = original.keys().chain(clone.keys()).collect();
    keys.into_iter()
        .filter(|k| original.get(*k) != clone.get(*k))
        .cloned()
        .collect()
}

pub fn walk(root: &Path, cap: usize) -> Result<Tree, String> {
    walk_links(root, cap).map(|(tree, _)| tree)
}

/// Like `walk`, never following a link, and also returning every symbolic link that points at a directory. A symlinked root is an error.
pub fn walk_links(root: &Path, cap: usize) -> Result<(Tree, Vec<PathBuf>), String> {
    let root_meta = fs::symlink_metadata(root).map_err(|e| format!("{}: {e}", root.display()))?;
    if root_meta.file_type().is_symlink() {
        return Err(format!("{} is a symbolic link", root.display()));
    }
    let mut linked_dirs = Vec::new();
    let mut tree = Tree::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let meta = fs::symlink_metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mtime = meta
            .modified()
            .map_err(|e| format!("{}: {e}", path.display()))?;
        tree.insert(path.clone(), (mtime, meta.len()));
        let linked_dir =
            meta.file_type().is_symlink() && fs::metadata(&path).is_ok_and(|m| m.is_dir());
        if tree.len() > cap {
            return Err(format!(
                "{}: more than {cap} entries, refusing to walk further",
                root.display()
            ));
        }
        if linked_dir {
            // A link is never a directory by `symlink_metadata`, so `path` is not needed again.
            linked_dirs.push(path);
        } else if meta.is_dir() {
            for entry in fs::read_dir(&path).map_err(|e| format!("{}: {e}", path.display()))? {
                pending.push(
                    entry
                        .map_err(|e| format!("{}: {e}", path.display()))?
                        .path(),
                );
            }
        }
    }
    Ok((tree, linked_dirs))
}

pub fn vacuous_reason(
    result: &Liveness,
    exits: &[i32],
    artifact_lines: &[usize],
) -> Option<String> {
    match result {
        Liveness::Undecided { reasons } => {
            return Some(format!("liveness is undecided ({} reasons)", reasons.len()));
        }
        Liveness::Decided { dead, .. } => {
            if dead.iter().all(|d| d.files.is_empty()) {
                return Some("no dead unit lists a file to delete".to_string());
            }
        }
    }
    replay_problem(exits, artifact_lines)
}

fn replay_problem(exits: &[i32], artifact_lines: &[usize]) -> Option<String> {
    if let Some((i, exit)) = exits.iter().enumerate().find(|(_, e)| **e != 0) {
        return Some(format!("command {} exited with exit status {exit}", i + 1));
    }
    artifact_lines
        .iter()
        .position(|n| *n == 0)
        .map(|i| format!("command {} emitted no compiler-artifact message", i + 1))
}

pub fn outcome(rebuilt_live: &[String], vacuous: Option<String>) -> WindowOutcome {
    if !rebuilt_live.is_empty() {
        WindowOutcome::RebuiltLive(rebuilt_live.to_vec())
    } else if let Some(why) = vacuous {
        WindowOutcome::Vacuous(why)
    } else {
        WindowOutcome::Passed
    }
}

/// Calls `each(line number, line text, message)` for every `compiler-artifact` message of `stdout`, whatever its key order. A line that is not JSON is skipped unless it mentions `compiler-artifact`, which is an error naming the line.
fn for_each_artifact(
    stdout: &Path,
    mut each: impl FnMut(usize, &str, &serde_json::Value) -> Result<(), String>,
) -> Result<(), String> {
    let mut reader =
        BufReader::new(fs::File::open(stdout).map_err(|e| format!("{}: {e}", stdout.display()))?);
    let mut line = Vec::new();
    let mut number = 0;
    loop {
        line.clear();
        if reader
            .read_until(b'\n', &mut line)
            .map_err(|e| format!("{}: {e}", stdout.display()))?
            == 0
        {
            return Ok(());
        }
        number += 1;
        let text = String::from_utf8_lossy(&line);
        let text = text.trim_end();
        let message = match serde_json::from_str::<serde_json::Value>(text) {
            Ok(message) => message,
            Err(e) if text.contains("compiler-artifact") => {
                return Err(format!(
                    "{} line {number}: cannot check the compiler-artifact message ({e}): {text}",
                    stdout.display()
                ));
            }
            Err(_) => continue,
        };
        if message["reason"].as_str() == Some("compiler-artifact") {
            each(number, text, &message)?;
        }
    }
}

pub fn count_artifacts(path: &Path, cap: u64) -> Result<usize, String> {
    let size = fs::metadata(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .len();
    if size > cap {
        return Err(format!(
            "{}: output is {size} bytes, over the {cap} byte cap",
            path.display()
        ));
    }
    let mut count = 0;
    for_each_artifact(path, |_, _, _| {
        count += 1;
        Ok(())
    })?;
    Ok(count)
}

/// Every file a `compiler-artifact` message of `stdout` names (`filenames` and `executable`) must be an absolute path with no `..` that lies under `clone` (as given or canonical). A compiler-artifact message whose `filenames` or `executable` is missing or of the wrong type, and the first file outside the clone, are errors naming the line or the file.
pub fn check_artifact_paths(stdout: &Path, clone: &Path) -> Result<(), String> {
    let roots: Vec<PathBuf> = [Some(clone.to_path_buf()), clone.canonicalize().ok()]
        .into_iter()
        .flatten()
        .collect();
    let inside = |file: &str| {
        let path = Path::new(file);
        path.is_absolute()
            && !path.components().any(|c| matches!(c, Component::ParentDir))
            && roots.iter().any(|root| path.starts_with(root))
    };
    for_each_artifact(stdout, |number, text, message| {
        let unreadable = |why: &str| {
            format!(
                "{} line {number}: cannot check the compiler-artifact message ({why}): {text}",
                stdout.display()
            )
        };
        let mut files: Vec<&serde_json::Value> = Vec::new();
        match message["filenames"].as_array() {
            Some(names) => files.extend(names),
            None => return Err(unreadable("filenames is missing or not an array")),
        }
        match message.get("executable") {
            Some(serde_json::Value::Null) => {}
            Some(executable @ serde_json::Value::String(_)) => files.push(executable),
            _ => return Err(unreadable("executable is missing or not a string or null")),
        }
        for file in files {
            let Some(file) = file.as_str() else {
                return Err(unreadable("a filename is not a string"));
            };
            if !inside(file) {
                return Err(format!(
                    "a replayed command wrote {file}, outside its clone {}",
                    clone.display()
                ));
            }
        }
        Ok(())
    })
}

pub fn run_logged(
    scratch: &Path,
    label: &str,
    mut cmd: Command,
    deadline: Duration,
    cap: u64,
) -> Result<Ran, String> {
    let stdout = scratch.join(format!("{label}.out"));
    let stderr = scratch.join(format!("{label}.err"));
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(fs::File::create(&stdout).map_err(|e| e.to_string())?)
        .stderr(fs::File::create(&stderr).map_err(|e| e.to_string())?)
        .spawn()
        .map_err(|e| format!("cannot start {cmd:?}: {e}"))?;
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            break status;
        }
        let over = [&stdout, &stderr]
            .into_iter()
            .find(|log| fs::metadata(log).is_ok_and(|m| m.len() > cap));
        let problem = if let Some(log) = over {
            Some(format!(
                "{}: output passed the {cap} byte cap; killed {cmd:?}",
                log.display()
            ))
        } else if started.elapsed() > deadline {
            Some(format!(
                "{cmd:?} did not finish within {deadline:?}; killed"
            ))
        } else {
            None
        };
        if let Some(problem) = problem {
            let _ = child.kill();
            let _ = child.wait();
            return Err(problem);
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    Ok(Ran {
        exit: status.code().unwrap_or(-1),
        artifact_lines: count_artifacts(&stdout, cap)?,
        stdout,
    })
}

struct CloneDir<'a> {
    fs: &'a dyn Fs,
    path: PathBuf,
}

impl CloneDir<'_> {
    fn remove(self) -> Result<(), String> {
        self.fs
            .remove_tree(&self.path)
            .map_err(|e| format!("cannot remove {}: {e}", self.path.display()))
    }
}

impl Drop for CloneDir<'_> {
    fn drop(&mut self) {
        let _ = self.fs.remove_tree(&self.path);
    }
}

pub type Cloner = dyn Fn(&Path, &str, &Path, &Path) -> Result<(), String>;

/// The default clone: an APFS copy-on-write `cp`. Tests inject others to prove the root check holds.
fn cp_clone(scratch: &Path, label: &str, target: &Path, dest: &Path) -> Result<(), String> {
    let mut cmd = Command::new("cp");
    cmd.args(["-c", "-R", "-p"]).arg(target).arg(dest);
    let ran = run_logged(
        scratch,
        &format!("cp-{label}"),
        cmd,
        COMMAND_DEADLINE,
        OUTPUT_CAP,
    )?;
    if ran.exit != 0 {
        return Err(format!(
            "cp -c -R -p {} exited with {}",
            target.display(),
            ran.exit
        ));
    }
    Ok(())
}

/// Refuses a checkout whose cargo config (`.cargo/config(.toml)` here, in any ancestor, in CARGO_HOME, or in `$HOME/.cargo` when CARGO_HOME is unset) names `artifact-dir` or `out-dir` (stable cargo rejects those, nightly would write outside the clone), has an `include` key (the included file is not followed), or has an `[alias]` table that defines a name the allow-list accepts (notably `clippy`, an external subcommand an alias can shadow). A relative checkout is resolved against the process cwd and a relative CARGO_HOME against the checkout. A text scan of at most 1 MiB per file; a larger file is refused.
pub fn check_config(checkout: &Path) -> Result<(), String> {
    let cwd = std::env::current_dir().map_err(|e| format!("cwd: {e}"))?;
    check_config_in(
        checkout,
        &cwd,
        std::env::var_os("CARGO_HOME"),
        std::env::var_os("HOME"),
    )
}

pub fn check_config_in(
    checkout: &Path,
    cwd: &Path,
    cargo_home: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Result<(), String> {
    let checkout = cwd.join(checkout);
    let mut dirs: Vec<PathBuf> = checkout.ancestors().map(|a| a.join(".cargo")).collect();
    match cargo_home {
        Some(h) => dirs.push(checkout.join(h)),
        None => dirs.extend(home.map(|h| PathBuf::from(h).join(".cargo"))),
    }
    for dir in dirs {
        for name in ["config.toml", "config"] {
            let path = dir.join(name);
            let Ok(file) = fs::File::open(&path) else {
                continue;
            };
            let mut text = Vec::new();
            std::io::Read::read_to_end(&mut std::io::Read::take(file, CONFIG_CAP + 1), &mut text)
                .map_err(|e| format!("{}: {e}", path.display()))?;
            if text.len() as u64 > CONFIG_CAP {
                return Err(format!(
                    "{} is over 1 MiB; the proof cannot check it",
                    path.display()
                ));
            }
            let text = String::from_utf8_lossy(&text);
            if let Some(key) = REDIRECT_KEYS.iter().find(|k| text.contains(**k)) {
                return Err(format!(
                    "{} names {key}, which writes build output outside the clone",
                    path.display()
                ));
            }
            let keys: Vec<String> = text.lines().filter_map(config_key).collect();
            if keys.iter().any(|k| k == "include") {
                return Err(format!(
                    "{} has an include key; the proof does not follow included configs",
                    path.display()
                ));
            }
            let has_alias = text.lines().any(|l| {
                let l = l.trim();
                l == "[alias]" || l.starts_with("alias.")
            });
            if has_alias
                && let Some(name) = keys.iter().find(|k| ALIAS_GUARDED.contains(&k.as_str()))
            {
                return Err(format!(
                    "{} defines the alias `{name}`, which can turn an allowed subcommand into another command",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

/// The key of a `key = value` line, without an `alias.` prefix or quotes; None for other lines.
fn config_key(line: &str) -> Option<String> {
    let line = line.trim_start();
    let line = line.strip_prefix("alias.").unwrap_or(line);
    let (key, _) = line.split_once('=')?;
    let key = key.trim().trim_matches(|c| c == '"' || c == '\'');
    (!key.is_empty() && !key.contains(char::is_whitespace)).then(|| key.to_string())
}

fn clone_target<'a>(
    fs: &'a dyn Fs,
    scratch: &Path,
    label: &str,
    target: &Path,
    cloner: &Cloner,
) -> Result<CloneDir<'a>, String> {
    let path = scratch.join(format!("clone-{label}"));
    let guard = CloneDir {
        fs,
        path: path.clone(),
    };
    cloner(scratch, label, target, &path)?;
    check_clone_root(&path, scratch, target)?;
    Ok(guard)
}

pub fn profile_dirs_of(path: &Path) -> Result<Vec<PathBuf>, String> {
    match target::identify(path) {
        None => Err(format!("{}: not a Cargo target dir", path.display())),
        Some(Err(why)) => Err(format!("{}: {why}", path.display())),
        Some(Ok(profiles)) => Ok(profiles.into_iter().map(|(dir, _)| dir).collect()),
    }
}

pub fn read_target(path: &Path) -> Result<TargetDir, String> {
    Ok(target_of(path, &profile_dirs_of(path)?))
}

fn target_of(path: &Path, profiles: &[PathBuf]) -> TargetDir {
    TargetDir {
        path: path.to_path_buf(),
        owner: Owner::Unresolved {
            reason: "proof".into(),
        },
        profiles: profiles
            .iter()
            .map(|dir| fingerprint::read_profile(dir))
            .collect(),
        size: None,
        unreadable: None,
    }
}

/// The clone must be a real directory (not a link) whose canonical path is inside the canonical scratch dir and is not the target itself.
pub fn check_clone_root(clone: &Path, scratch: &Path, target: &Path) -> Result<(), String> {
    let meta = fs::symlink_metadata(clone).map_err(|e| format!("{}: {e}", clone.display()))?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(format!(
            "{} is not a real directory; the clone would alias another path",
            clone.display()
        ));
    }
    let real = clone
        .canonicalize()
        .map_err(|e| format!("{}: {e}", clone.display()))?;
    let scratch_real = scratch
        .canonicalize()
        .map_err(|e| format!("{}: {e}", scratch.display()))?;
    if !real.starts_with(&scratch_real) || target.canonicalize().is_ok_and(|t| t == real) {
        return Err(format!(
            "{} resolves to {}, outside the scratch dir {}",
            clone.display(),
            real.display(),
            scratch_real.display()
        ));
    }
    Ok(())
}

pub fn replay_command(args: &[String], checkout: &Path, clone: &Path) -> Command {
    let names: Vec<std::ffi::OsString> = std::env::vars_os().map(|(k, _)| k).collect();
    replay_command_with_env(args, checkout, clone, &names)
}

/// RUSTFLAGS and the owner's cargo config are left as the shell has them, since they are part of what the commands do; RUSTC_WRAPPER, RUSTC_WORKSPACE_WRAPPER and their CARGO_BUILD_* forms, and CARGO_BUILD_RUSTC, are removed because they change fingerprints. The target and build dirs are pointed at `clone`.
///
/// `env_names` are the variables present in the environment; every `CARGO_UNSTABLE_*` among them is removed so a nightly opt-in cannot enable artifact redirection, and every `CARGO_ALIAS_*` so an env alias cannot turn an allowed name into another subcommand.
pub fn replay_command_with_env(
    args: &[String],
    checkout: &Path,
    clone: &Path,
    env_names: &[std::ffi::OsString],
) -> Command {
    let mut process = Command::new("cargo");
    process
        .args(args)
        .current_dir(checkout)
        .env_remove("CARGO_BUILD_TARGET_DIR")
        .env_remove("CARGO_BUILD_ARTIFACT_DIR")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env_remove("CARGO_BUILD_RUSTC_WRAPPER")
        .env_remove("CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER")
        .env_remove("CARGO_BUILD_RUSTC");
    for name in env_names {
        let name_lossy = name.to_string_lossy();
        if name_lossy.starts_with("CARGO_UNSTABLE_") || name_lossy.starts_with("CARGO_ALIAS_") {
            process.env_remove(name);
        }
    }
    process
        .env("CARGO_TARGET_DIR", clone)
        .env("CARGO_BUILD_BUILD_DIR", clone);
    process
}

struct Replay {
    commands: Vec<(Vec<String>, i32)>,
    exits: Vec<i32>,
    artifact_lines: Vec<usize>,
}

fn replay(scratch: &Path, label: &str, input: &ProveInput, clone: &Path) -> Result<Replay, String> {
    let mut out = Replay {
        commands: Vec::new(),
        exits: Vec::new(),
        artifact_lines: Vec::new(),
    };
    for (i, cmd) in input.cmds.iter().enumerate() {
        let args = replay_args(cmd);
        let process = replay_command(&args, &input.checkout, clone);
        let ran = run_logged(
            scratch,
            &format!("{label}-{i}"),
            process,
            COMMAND_DEADLINE,
            OUTPUT_CAP,
        )?;
        check_artifact_paths(&ran.stdout, clone)?;
        out.commands.push((args, ran.exit));
        out.exits.push(ran.exit);
        out.artifact_lines.push(ran.artifact_lines);
    }
    Ok(out)
}

fn refuse_linked_dirs(input: &ProveInput) -> Result<(), String> {
    let (_, linked) = walk_links(&input.target, WALK_CAP)?;
    match linked.first() {
        Some(link) => Err(format!(
            "{} is a symbolic link to a directory inside the target; a clone would alias its destination",
            link.display()
        )),
        None => Ok(()),
    }
}

/// What the end-of-run check compares: the files windows listed for deletion and the checkout's `Cargo.lock`.
struct Recorded {
    listed: BTreeSet<PathBuf>,
    lockfile: PathBuf,
    lockfile_stamp: Option<(SystemTime, u64)>,
}

fn lockfile_stamp(lockfile: &Path) -> Result<Option<(SystemTime, u64)>, String> {
    match fs::symlink_metadata(lockfile) {
        Ok(meta) => Ok(Some((
            meta.modified()
                .map_err(|e| format!("{}: {e}", lockfile.display()))?,
            meta.len(),
        ))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", lockfile.display())),
    }
}

fn check_recorded(recorded: &Recorded) -> Result<(), String> {
    let missing: Vec<String> = recorded
        .listed
        .iter()
        .filter(|f| fs::symlink_metadata(f).is_err())
        .take(10)
        .map(|f| f.display().to_string())
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "files a window listed for deletion are missing from the original after the proof: {} \
             (a build in the owner's checkout after the locks were released is a likely cause; rerun with nothing building)",
            missing.join(", ")
        ));
    }
    if lockfile_stamp(&recorded.lockfile)? != recorded.lockfile_stamp {
        return Err(format!(
            "{} changed during the proof (a build or `cargo update` in the owner's checkout after the locks were released is a likely cause; rerun with nothing building)",
            recorded.lockfile.display()
        ));
    }
    Ok(())
}

fn rebase(path: &Path, target: &Path, clone: &Path) -> Result<PathBuf, String> {
    let rel = path
        .strip_prefix(target)
        .map_err(|_| format!("{} is outside the target dir", path.display()))?;
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(format!("{} is not a plain path", path.display()));
    }
    Ok(clone.join(rel))
}

#[derive(Debug)]
pub struct ExecuteTrial {
    pub deleted_files: usize,
    pub rebuilt_live_units: usize,
}

/// Deletes the clone's dead units through `execute`, then replays the fixture's
/// feature build. No dead file is a vacuous trial: nothing is deleted or rebuilt.
pub fn trial_through_execute(fx: &super::cargo_fixture::Fixture) -> Result<ExecuteTrial, String> {
    let platform = platform::current().map_err(|_| platform::MACOS_ONLY_MESSAGE.to_string())?;
    let fs = platform.fs.as_ref();
    let scratch = super::Scratch::new("execute-trial");
    let clone = clone_target(fs, &scratch.path, "target", &fx.target, &cp_clone)?;
    let target = read_target(&clone.path)?;
    let now = SystemTime::now();
    let window = super::cargo_fixture::WINDOW;
    let state = State::open_at(scratch.path.join("state")).map_err(|err| err.to_string())?;
    let cfg = state.config().map_err(|err| err.to_string())?;
    let repos = Vec::new();
    let mains = Vec::new();
    let ctx = Ctx {
        state: &state,
        fs,
        cfg: &cfg,
        now,
        account_bytes: false,
        repos: &repos,
        mains: &mains,
        liveness: cargo_hive::gc::liveness::liveness,
    };
    let verdict = liveness(fs, &target, now, window);
    let (live, has_dead_file) = match &verdict {
        Liveness::Decided { live, dead } => {
            let mut keys = BTreeSet::new();
            for unit in live {
                keys.insert((rel(&unit.profile, &target.path), unit.dir_name.clone()));
            }
            let has_dead_file = dead.iter().any(|unit| !unit.files.is_empty());
            (keys, has_dead_file)
        }
        Liveness::Undecided { reasons } => {
            let text: Vec<String> = reasons.iter().map(ToString::to_string).collect();
            return Err(format!("liveness is undecided ({})", text.join("; ")));
        }
    };
    if !has_dead_file {
        return Ok(ExecuteTrial {
            deleted_files: 0,
            rebuilt_live_units: 0,
        });
    }
    let planned = match build::plan_for_target(&ctx, &target, window) {
        TargetPlan::Planned(built) => built.target,
        TargetPlan::NothingToDo => {
            return Ok(ExecuteTrial {
                deleted_files: 0,
                rebuilt_live_units: 0,
            });
        }
        TargetPlan::Undecided(reasons) => {
            let text: Vec<String> = reasons.iter().map(ToString::to_string).collect();
            return Err(format!("liveness is undecided ({})", text.join("; ")));
        }
        TargetPlan::Unreadable(rows) => {
            let text: Vec<String> = rows
                .iter()
                .map(|(path, err)| format!("{}: {err}", path.display()))
                .collect();
            return Err(format!(
                "could not stat a dead unit's file: {}",
                text.join("; ")
            ));
        }
    };
    // execute rechecks liveness in whole days. One day covers the six-hour
    // window, and units dead for longer than a day stay dead.
    let window_days = window.as_secs().div_ceil(86_400);
    let made_at = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let plan = Plan::single(PlanClass::Secondary, planned, window_days, made_at);
    let resolved = [ResolvedTarget {
        path: clone.path.clone(),
        owner: Some(fx.app.clone()),
        class: PlanClass::Secondary,
    }];
    let applied = apply::execute(&ctx, plan, &resolved, &|| false);
    if applied.outcome != Outcome::Applied {
        return Err(format!(
            "execute finished as {:?} ({:?}); findings: {:?}",
            applied.outcome, applied.reason, applied.findings
        ));
    }
    let deleted_files = applied.counts.get("paths_deleted").copied().unwrap_or(0) as usize;
    let before = stamps_of(fs, &clone.path)?;
    if !super::cargo_fixture::cargo_in(
        &fx.app,
        &clone.path,
        &fx.logs,
        &["build", "--features", "f"],
    ) {
        let err = fs::read_to_string(fx.logs.join("stderr")).unwrap_or_default();
        return Err(format!("replay failed: {err}"));
    }
    let after = stamps_of(fs, &clone.path)?;
    let rebuilt_units = rebuilt(&before, &after);
    Ok(ExecuteTrial {
        deleted_files,
        rebuilt_live_units: rebuilt_live(&rebuilt_units, &live).len(),
    })
}

/// Ages every unit stamp on `clone` that the proof did not call live, to two days ago.
/// A one-day recheck then still calls the planned files dead. A warm-up can refresh a
/// build script the plan never lists; that stamp would keep a dead dependency live.
fn backdate_dead_stamps(
    fs: &dyn Fs,
    clone: &Path,
    live: &BTreeSet<(PathBuf, String)>,
) -> Result<(), String> {
    let when = SystemTime::now()
        .checked_sub(Duration::from_secs(2 * 86_400))
        .ok_or("the clock is too early to backdate dead units")?;
    for profile in profile_dirs_of(clone)? {
        let rel_profile = rel(&profile, clone);
        backdate_stamp_tree(
            fs,
            &profile.join(".fingerprint"),
            &rel_profile,
            live,
            when,
            true,
        )?;
        backdate_stamp_tree(fs, &profile.join("build"), &rel_profile, live, when, false)?;
    }
    Ok(())
}

fn backdate_stamp_tree(
    fs: &dyn Fs,
    root: &Path,
    rel_profile: &Path,
    live: &BTreeSet<(PathBuf, String)>,
    when: SystemTime,
    fingerprint: bool,
) -> Result<(), String> {
    let listed = match fs.read_dir_bounded(root, 100_000) {
        Ok(listed) => listed,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("{}: {err}", root.display())),
    };
    for (name, kind) in listed {
        if kind != EntryKind::Dir {
            continue;
        }
        if live.contains(&(rel_profile.to_path_buf(), name.clone())) {
            continue;
        }
        let dir = root.join(&name);
        set_stamp(fs, &dir.join("invoked.timestamp"), when)?;
        if !fingerprint {
            continue;
        }
        let inner = match fs.read_dir_bounded(&dir, 100_000) {
            Ok(inner) => inner,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(format!("{}: {err}", dir.display())),
        };
        for (child, child_kind) in inner {
            if child_kind != EntryKind::File
                || child.ends_with(".json")
                || child == "invoked.timestamp"
            {
                continue;
            }
            set_stamp(fs, &dir.join(child), when)?;
        }
    }
    Ok(())
}

fn set_stamp(fs: &dyn Fs, path: &Path, when: SystemTime) -> Result<(), String> {
    if fs.mtime(path).is_err() {
        return Ok(());
    }
    fs.set_mtime(path, when)
        .map_err(|err| format!("{}: {err}", path.display()))
}

/// Deletes one window's dead files on the owner's clone through `execute`.
/// The caller's replay is what runs the owner's command list. A file that is
/// already gone is skipped, matching a missing entry. No remaining file means
/// there is nothing to plan.
fn delete_window_through_execute(
    fs: &dyn Fs,
    files: &[PathBuf],
    live: &BTreeSet<(PathBuf, String)>,
    input: &ProveInput,
    clone: &Path,
    scratch: &Path,
) -> Result<(), String> {
    let mut entries = Vec::new();
    for file in files {
        let path = rebase(file, &input.target, clone)?;
        let stat = match fs.entry_stat(&path) {
            Ok(stat) => stat,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(format!("{}: {err}", path.display())),
        };
        if stat.kind == EntryKind::Other {
            return Err(format!("cannot delete {}", path.display()));
        }
        entries.push(PlannedEntry {
            path,
            // One label, so a fingerprint directory is removed with the artifacts
            // instead of being skipped as a fingerprint-only group.
            unit: "window".to_string(),
            kind: kind_name(stat.kind).to_string(),
            dev: stat.dev,
            ino: stat.ino,
        });
    }
    if entries.is_empty() {
        return Ok(());
    }
    let now = SystemTime::now();
    let made_at = now
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    let planned = PlannedTarget {
        path: clone.to_path_buf(),
        owner: Some(input.checkout.clone()),
        planned: None,
        entries,
        orphan: None,
    };
    // `execute` accepts only a window of 1..=365 days. The warm-up refreshed stamps
    // the proof did not call live, including a build script the plan never lists.
    // Those stamps are backdated past one day so the planned files stay dead.
    // Live units are left as the warm-up wrote them.
    backdate_dead_stamps(fs, clone, live)?;
    let plan = Plan::single(PlanClass::Secondary, planned, 1, made_at);
    let state = State::open_at(scratch.join("prove-state")).map_err(|err| err.to_string())?;
    let cfg = state.config().map_err(|err| err.to_string())?;
    let repos = Vec::new();
    let mains = Vec::new();
    let ctx = Ctx {
        state: &state,
        fs,
        cfg: &cfg,
        now,
        account_bytes: false,
        repos: &repos,
        mains: &mains,
        liveness: cargo_hive::gc::liveness::liveness,
    };
    let resolved = [ResolvedTarget {
        path: clone.to_path_buf(),
        owner: Some(input.checkout.clone()),
        class: PlanClass::Secondary,
    }];
    let applied = apply::execute(&ctx, plan, &resolved, &|| false);
    if applied.outcome != Outcome::Applied {
        return Err(execute_did_not_apply(&applied));
    }
    Ok(())
}

fn execute_did_not_apply(applied: &apply::Applied) -> String {
    if let Some((_, Some(path), _)) = applied
        .findings
        .iter()
        .find(|(kind, _, _)| kind == "plan.delete_failed")
    {
        return format!("cannot delete {}", path.display());
    }
    format!(
        "execute finished as {:?} ({:?}); findings: {:?}",
        applied.outcome, applied.reason, applied.findings
    )
}

pub fn delete_in_clone(
    fs: &dyn Fs,
    files: &[PathBuf],
    target: &Path,
    clone: &Path,
) -> Result<(), String> {
    clone
        .canonicalize()
        .map_err(|e| format!("{}: {e}", clone.display()))?;
    for file in files {
        let path = rebase(file, target, clone)?;
        match delete_checked(fs, clone, &path, Expect::Any) {
            Deleted::Removed | Deleted::Missing => {}
            Deleted::Outside => {
                return Err(format!(
                    "{} reaches outside the clone through a link",
                    path.display()
                ));
            }
            Deleted::Changed => return Err(format!("cannot delete {}", path.display())),
        }
    }
    Ok(())
}

pub fn unmapped_paths(fs: &dyn Fs, target: &TargetDir, cap: usize) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    for profile in &target.profiles {
        paths.push(profile.path.join("incremental"));
        let entries = fs
            .read_dir_bounded(&profile.path, cap)
            .map_err(|e| format!("{}: cannot list: {e}", profile.path.display()))?;
        paths.extend(
            entries
                .into_iter()
                .filter(|(_, kind)| *kind == EntryKind::File)
                .map(|(name, _)| profile.path.join(name)),
        );
    }
    Ok(paths)
}

struct Decision {
    window: Duration,
    result: Liveness,
    live: BTreeSet<(PathBuf, String)>,
    dead_files: Vec<PathBuf>,
    dead_bytes: Footprint,
    pre_vacuous: Option<String>,
}

pub fn prove(input: &ProveInput) -> Result<ProveReport, String> {
    let platform = platform::current().map_err(|_| platform::MACOS_ONLY_MESSAGE.to_string())?;
    let scratch = super::Scratch::new("prove");
    prove_with(platform.fs.as_ref(), &scratch.path, input)
}

pub fn prove_with(fs: &dyn Fs, scratch: &Path, input: &ProveInput) -> Result<ProveReport, String> {
    prove_with_cloner(fs, scratch, input, &cp_clone)
}

pub fn prove_with_cloner(
    fs: &dyn Fs,
    scratch: &Path,
    input: &ProveInput,
    cloner: &Cloner,
) -> Result<ProveReport, String> {
    for cmd in &input.cmds {
        check_command(cmd).map_err(|why| format!("command {cmd:?} {why}"))?;
    }
    if input.cmds.is_empty() {
        return Err("no commands to replay".to_string());
    }
    check_config(&input.checkout)?;
    let root = fs::symlink_metadata(&input.target)
        .map_err(|e| format!("{}: {e}", input.target.display()))?;
    if root.file_type().is_symlink() {
        return Err(format!(
            "{} is a symbolic link; pass the real target directory",
            input.target.display()
        ));
    }
    let profile_dirs = profile_dirs_of(&input.target)?;

    let mut locks = Vec::new();
    for profile in &profile_dirs {
        match fs.try_lock_exclusive(&profile.join(".cargo-build-lock")) {
            TryLock::Acquired(guard) => locks.push(guard),
            TryLock::Gone if profile.join(".fingerprint").is_dir() => {
                return Err(format!(
                    "{}: no build lock file; cannot prove the target idle",
                    profile.display()
                ));
            }
            TryLock::Gone => {}
            TryLock::Held => {
                return Err(format!(
                    "{}: the build lock is held by another process; stop other builds first",
                    profile.display()
                ));
            }
            TryLock::Unopenable(e) | TryLock::Unlockable(e) => {
                return Err(format!("{}: cannot lock: {e}", profile.display()));
            }
        }
    }
    let locked_at = Instant::now();

    let target_volume = fs
        .volume(&input.target)
        .map_err(|e| format!("{}: {e}", input.target.display()))?;
    if target_volume.fs_type != "apfs" {
        return Err(format!(
            "{} is on a {} volume; the proof needs apfs to clone",
            input.target.display(),
            target_volume.fs_type
        ));
    }
    let scratch_volume = fs
        .volume(scratch)
        .map_err(|e| format!("{}: {e}", scratch.display()))?;
    if scratch_volume.mount != target_volume.mount {
        return Err(format!(
            "the scratch dir {} must be on the same volume as the target ({})",
            scratch.display(),
            target_volume.mount.display()
        ));
    }

    refuse_linked_dirs(input)?;
    let target = target_of(&input.target, &profile_dirs);
    let unmapped = measure(
        fs,
        &input.target,
        &unmapped_paths(fs, &target, WALK_CAP)?,
        true,
    );

    let mut decisions = Vec::new();
    for &window in &input.windows {
        let result = liveness(fs, &target, input.now, window);
        let mut live = BTreeSet::new();
        let mut dead_files = Vec::new();
        if let Liveness::Decided { live: l, dead } = &result {
            for unit in l {
                live.insert((rel(&unit.profile, &input.target), unit.dir_name.clone()));
            }
            for d in dead {
                dead_files.extend(d.files.iter().cloned());
            }
        }
        let dead_bytes = measure(fs, &input.target, &dead_files, true);
        let pre_vacuous = vacuous_reason(&result, &[], &[]);
        decisions.push(Decision {
            window,
            result,
            live,
            dead_files,
            dead_bytes,
            pre_vacuous,
        });
    }

    let lockfile = input.checkout.join("Cargo.lock");
    let recorded = Recorded {
        listed: decisions
            .iter()
            .flat_map(|d| d.dead_files.iter())
            .filter(|f| fs::symlink_metadata(f).is_ok())
            .cloned()
            .collect(),
        lockfile_stamp: lockfile_stamp(&lockfile)?,
        lockfile,
    };
    let snapshot = clone_target(fs, scratch, "snapshot", &input.target, cloner)?;
    drop(locks);
    let lock_held = locked_at.elapsed();

    let mut warm_up = PhaseReport::default();
    let mut baseline = PhaseReport::default();
    let mut phase_problem = None;
    let mut warmed = None;
    let mut warmed_stamps = Stamps::new();
    if decisions.iter().any(|d| d.pre_vacuous.is_none()) {
        let started = Instant::now();
        let clone = clone_target(fs, scratch, "warm-up", &snapshot.path, cloner)?;
        snapshot.remove()?;
        let before = stamps_of(fs, &clone.path)?;
        let ran = replay(scratch, "warm-up", input, &clone.path)?;
        warmed_stamps = stamps_of(fs, &clone.path)?;
        warm_up = PhaseReport {
            rebuilt: rebuilt(&before, &warmed_stamps)
                .iter()
                .map(unit_name)
                .collect(),
            commands: ran.commands,
            elapsed: started.elapsed(),
        };
        if let Some(why) = replay_problem(&ran.exits, &ran.artifact_lines) {
            phase_problem = Some(format!("warm-up replay: {why}"));
        } else {
            let started = Instant::now();
            let copy = clone_target(fs, scratch, "baseline", &clone.path, cloner)?;
            let before = stamps_of(fs, &copy.path)?;
            let ran = replay(scratch, "baseline", input, &copy.path)?;
            let after = stamps_of(fs, &copy.path)?;
            baseline = PhaseReport {
                rebuilt: rebuilt(&before, &after).iter().map(unit_name).collect(),
                commands: ran.commands,
                elapsed: started.elapsed(),
            };
            copy.remove()?;
            if let Some(why) = replay_problem(&ran.exits, &ran.artifact_lines) {
                phase_problem = Some(format!("baseline replay: {why}"));
            }
            if !baseline.rebuilt.is_empty() {
                phase_problem = Some(format!(
                    "baseline replay rebuilt {}",
                    baseline.rebuilt.join(", ")
                ));
            }
        }
        warmed = Some(clone);
    }

    let mut windows = Vec::new();
    for (i, d) in decisions.into_iter().enumerate() {
        let (dead_units, reasons) = match &d.result {
            Liveness::Decided { dead, .. } => (dead.len(), Vec::new()),
            Liveness::Undecided { reasons } => {
                (0, reasons.iter().map(ToString::to_string).collect())
            }
        };
        let mut report = WindowReport {
            window: d.window,
            outcome: WindowOutcome::Passed,
            dead_units,
            dead_bytes: d.dead_bytes,
            unmapped: unmapped.clone(),
            rebuilt: 0,
            rebuilt_live: 0,
            reasons,
            commands: Vec::new(),
        };
        if let Some(why) = d.pre_vacuous.or(phase_problem.clone()) {
            report.outcome = WindowOutcome::Vacuous(why);
            windows.push(report);
            continue;
        }

        let Some(warmed_clone) = &warmed else {
            return Err("no warmed-up clone to copy a window from".to_string());
        };
        let clone = clone_target(fs, scratch, &format!("w{i}"), &warmed_clone.path, cloner)?;
        let clone_target_dir = read_target(&clone.path)?;
        let mismatches = mtime_mismatches(&warmed_stamps, &stamps(fs, &clone_target_dir)?);
        if !mismatches.is_empty() {
            let names: Vec<String> = mismatches.iter().take(5).map(unit_name).collect();
            report.outcome = WindowOutcome::Vacuous(format!(
                "the clone's unit timestamps differ from the warmed-up clone's: {}",
                names.join(", ")
            ));
            clone.remove()?;
            windows.push(report);
            continue;
        }
        delete_window_through_execute(fs, &d.dead_files, &d.live, input, &clone.path, scratch)?;
        let before = stamps_of(fs, &clone.path)?;
        let ran = replay(scratch, &format!("w{i}"), input, &clone.path)?;
        let after = stamps_of(fs, &clone.path)?;
        let changed = rebuilt(&before, &after);
        let live_hit = rebuilt_live(&changed, &d.live);
        let vacuous = vacuous_reason(&d.result, &ran.exits, &ran.artifact_lines);
        report.rebuilt = changed.len();
        report.rebuilt_live = live_hit.len();
        report.commands = ran.commands;
        if let Some(why) = &vacuous {
            report.reasons.push(why.clone());
        }
        report.outcome = outcome(&live_hit, vacuous);
        clone.remove()?;
        windows.push(report);
    }

    if let Some(clone) = warmed {
        clone.remove()?;
    }
    check_recorded(&recorded)?;
    Ok(ProveReport {
        warm_up,
        baseline,
        lock_held,
        windows,
    })
}
