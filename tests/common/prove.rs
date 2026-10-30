use cargo_hive::cargo_target::{self, Owner, TargetDir};
use cargo_hive::fingerprint;
use cargo_hive::gc::{measure, Footprint};
use cargo_hive::liveness::{liveness, unit_timestamp, Liveness};
use cargo_hive::platform::{self, EntryKind, Fs, TryLock};
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
/// The only subcommands a replay may run: they compile or run inside the checkout and write nothing but the target dir; all of them accept `--locked`.
const ALLOWED: [&str; 14] = [
    "build", "b", "check", "c", "test", "t", "clippy", "doc", "d", "bench", "rustc", "rustdoc",
    "run", "r",
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

/// The owner's commands run for real in the checkout (that is the owner's choice): only the target dir is protected, not build.rs or test side effects. RUSTFLAGS and the owner's cargo config are left as the shell has them, since they are part of what the commands do; RUSTC_WRAPPER and RUSTC_WORKSPACE_WRAPPER are removed because they change fingerprints.
pub struct ProveInput {
    pub target: PathBuf,
    pub checkout: PathBuf,
    pub cmds: Vec<Vec<String>>,
    pub windows: Vec<Duration>,
    pub now: SystemTime,
}

#[derive(Debug)]
pub struct ProveReport {
    pub baseline_rebuilt: Vec<String>,
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
        f.bytes.freed, f.bytes.still_shared, f.unmeasured
    )
}

impl ProveReport {
    pub fn all_vacuous(&self) -> bool {
        self.windows
            .iter()
            .all(|w| matches!(w.outcome, WindowOutcome::Vacuous(_)))
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        if !self.baseline_rebuilt.is_empty() {
            let _ = writeln!(
                out,
                "baseline rebuilt: {}",
                self.baseline_rebuilt.join(", ")
            );
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
            for (cmd, exit) in &w.commands {
                let _ = writeln!(out, "  cargo {} -> exit {exit}", cmd.join(" "));
            }
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
            return Ok(i);
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

fn kind_for(stem: &str) -> &'static str {
    if stem.starts_with("run-build-script") {
        "run-build-script"
    } else {
        "other"
    }
}

pub fn stamps(fs: &dyn Fs, target: &TargetDir) -> Stamps {
    let mut out = Stamps::new();
    for profile in &target.profiles {
        let rel = profile
            .path
            .strip_prefix(&target.path)
            .unwrap_or(&profile.path)
            .to_path_buf();
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
                continue;
            };
            let kind = nc.stem.as_deref().map_or("other", kind_for);
            out.insert((rel.clone(), dir.to_string()), stamp(dir, kind));
        }
    }
    out
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
        if meta.file_type().is_symlink() && fs::metadata(&path).is_ok_and(|m| m.is_dir()) {
            linked_dirs.push(path.clone());
        }
        if tree.len() > cap {
            return Err(format!(
                "{}: more than {cap} entries, refusing to walk further",
                root.display()
            ));
        }
        if meta.is_dir() {
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

pub fn tree_changes(before: &Tree, after: &Tree) -> Vec<PathBuf> {
    let paths: BTreeSet<_> = before.keys().chain(after.keys()).collect();
    paths
        .into_iter()
        .filter(|p| before.get(*p) != after.get(*p))
        .cloned()
        .collect()
}

pub fn vacuous_reason(
    result: &Liveness,
    exits: &[i32],
    artifact_lines: &[usize],
) -> Option<String> {
    match result {
        Liveness::Undecided { reasons } => {
            return Some(format!("liveness is undecided ({} reasons)", reasons.len()))
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
    let mut reader = BufReader::new(fs::File::open(path).map_err(|e| e.to_string())?);
    let mut line = Vec::new();
    let mut count = 0;
    loop {
        line.clear();
        if reader
            .read_until(b'\n', &mut line)
            .map_err(|e| e.to_string())?
            == 0
        {
            return Ok(count);
        }
        if line.starts_with(br#"{"reason":"compiler-artifact""#) {
            count += 1;
        }
    }
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

/// Refuses a checkout whose `.cargo/config(.toml)` (here, in any ancestor, or in CARGO_HOME) names `artifact-dir` or `out-dir`: stable cargo rejects those options, nightly would write outside the clone, and an env override would break stable. A text scan of at most 1 MiB per file; a larger file is refused.
pub fn check_config(checkout: &Path) -> Result<(), String> {
    let mut dirs: Vec<PathBuf> = checkout.ancestors().map(|a| a.join(".cargo")).collect();
    if let Some(home) = std::env::var_os("CARGO_HOME") {
        dirs.push(PathBuf::from(home));
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
                    "{} is over 1 MiB; the proof cannot check it for artifact-dir or out-dir",
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
        }
    }
    Ok(())
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

pub fn read_target(path: &Path) -> Result<TargetDir, String> {
    let profiles = match cargo_target::identify(path) {
        None => return Err(format!("{}: not a Cargo target dir", path.display())),
        Some(Err(why)) => return Err(format!("{}: {why}", path.display())),
        Some(Ok(profiles)) => profiles,
    };
    Ok(TargetDir {
        path: path.to_path_buf(),
        owner: Owner::Unresolved {
            reason: "proof".into(),
        },
        profiles: profiles
            .iter()
            .map(|(dir, _)| fingerprint::read_profile(dir))
            .collect(),
        size: None,
        unreadable: None,
    })
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

/// `env_names` are the variables present in the environment; every `CARGO_UNSTABLE_*` among them is removed so a nightly opt-in cannot enable artifact redirection.
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
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR")
        .env_remove("CARGO_BUILD_BUILD_DIR")
        .env_remove("CARGO_BUILD_ARTIFACT_DIR")
        .env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER");
    for name in env_names {
        if name.to_string_lossy().starts_with("CARGO_UNSTABLE_") {
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
        out.commands.push((args, ran.exit));
        out.exits.push(ran.exit);
        out.artifact_lines.push(ran.artifact_lines);
    }
    Ok(out)
}

fn snapshot(input: &ProveInput) -> Result<Tree, String> {
    let (mut tree, linked) = walk_links(&input.target, WALK_CAP)?;
    if let Some(link) = linked.first() {
        return Err(format!(
            "{} is a symbolic link to a directory inside the target; a clone would alias its destination",
            link.display()
        ));
    }
    let lock = input.checkout.join("Cargo.lock");
    if let Ok(meta) = fs::symlink_metadata(&lock) {
        let mtime = meta.modified().map_err(|e| e.to_string())?;
        tree.insert(lock, (mtime, meta.len()));
    }
    Ok(tree)
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

pub fn delete_in_clone(
    fs: &dyn Fs,
    files: &[PathBuf],
    target: &Path,
    clone: &Path,
) -> Result<(), String> {
    let clone_real = clone
        .canonicalize()
        .map_err(|e| format!("{}: {e}", clone.display()))?;
    for file in files {
        let path = rebase(file, target, clone)?;
        if let Some(parent) = path.parent() {
            match parent.canonicalize() {
                Ok(real) if real.starts_with(&clone_real) => {}
                Ok(real) => {
                    return Err(format!(
                        "{} reaches {} through a link, outside the clone",
                        path.display(),
                        real.display()
                    ))
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(format!("cannot resolve {}: {e}", parent.display())),
            }
        }
        let removed = match fs.entry_stat(&path) {
            Ok(stat) if stat.kind == EntryKind::Dir => fs.remove_tree(&path),
            Ok(_) => fs.remove_file(&path),
            Err(e) => Err(e),
        };
        match removed {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("cannot delete {}: {e}", path.display())),
        }
    }
    Ok(())
}

fn unmapped_paths(fs: &dyn Fs, target: &TargetDir) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for profile in &target.profiles {
        paths.push(profile.path.join("incremental"));
        if let Ok(entries) = fs.read_dir_bounded(&profile.path, WALK_CAP) {
            paths.extend(
                entries
                    .into_iter()
                    .filter(|(_, kind)| *kind == EntryKind::File)
                    .map(|(name, _)| profile.path.join(name)),
            );
        }
    }
    paths
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
    let target = read_target(&input.target)?;

    let mut locks = Vec::new();
    for profile in &target.profiles {
        match fs.try_lock_exclusive(&profile.path.join(".cargo-build-lock")) {
            TryLock::Acquired(guard) => locks.push(guard),
            TryLock::Gone => {}
            TryLock::Held => {
                return Err(format!(
                    "{}: the build lock is held by another process; stop other builds first",
                    profile.path.display()
                ))
            }
            TryLock::Unopenable(e) | TryLock::Unlockable(e) => {
                return Err(format!("{}: cannot lock: {e}", profile.path.display()))
            }
        }
    }

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

    let original_tree = snapshot(input)?;
    let original_stamps = stamps(fs, &target);
    let unmapped = measure(fs, &input.target, &unmapped_paths(fs, &target), true);

    let mut decisions = Vec::new();
    for &window in &input.windows {
        let result = liveness(fs, &target, input.now, window);
        let mut live = BTreeSet::new();
        let mut dead_files = Vec::new();
        if let Liveness::Decided { live: l, dead } = &result {
            for unit in l {
                let rel = unit
                    .profile
                    .strip_prefix(&input.target)
                    .unwrap_or(&unit.profile);
                live.insert((rel.to_path_buf(), unit.dir_name.clone()));
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

    let mut baseline_rebuilt = Vec::new();
    let mut baseline_problem = None;
    if decisions.iter().any(|d| d.pre_vacuous.is_none()) {
        let clone = clone_target(fs, scratch, "baseline", &input.target, cloner)?;
        let clone_target_dir = read_target(&clone.path)?;
        let before = stamps(fs, &clone_target_dir);
        let ran = replay(scratch, "baseline", input, &clone.path)?;
        let after = stamps(fs, &read_target(&clone.path)?);
        baseline_rebuilt = rebuilt(&before, &after).iter().map(unit_name).collect();
        clone.remove()?;
        if let Some(why) = replay_problem(&ran.exits, &ran.artifact_lines) {
            baseline_problem = Some(format!("baseline replay: {why}"));
        }
        if !baseline_rebuilt.is_empty() {
            baseline_problem = Some(format!(
                "baseline replay rebuilt {}",
                baseline_rebuilt.join(", ")
            ));
        }
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
            unmapped,
            rebuilt: 0,
            rebuilt_live: 0,
            reasons,
            commands: Vec::new(),
        };
        if let Some(why) = d.pre_vacuous.or(baseline_problem.clone()) {
            report.outcome = WindowOutcome::Vacuous(why);
            windows.push(report);
            continue;
        }

        let clone = clone_target(fs, scratch, &format!("w{i}"), &input.target, cloner)?;
        let clone_target_dir = read_target(&clone.path)?;
        let mismatches = mtime_mismatches(&original_stamps, &stamps(fs, &clone_target_dir));
        if !mismatches.is_empty() {
            let names: Vec<String> = mismatches.iter().take(5).map(unit_name).collect();
            report.outcome = WindowOutcome::Vacuous(format!(
                "the clone's unit timestamps differ from the original's: {}",
                names.join(", ")
            ));
            clone.remove()?;
            windows.push(report);
            continue;
        }
        delete_in_clone(fs, &d.dead_files, &input.target, &clone.path)?;
        let before = stamps(fs, &read_target(&clone.path)?);
        let ran = replay(scratch, &format!("w{i}"), input, &clone.path)?;
        let after = stamps(fs, &read_target(&clone.path)?);
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

    let after_tree = snapshot(input)?;
    let changed = tree_changes(&original_tree, &after_tree);
    if !changed.is_empty() {
        let first: Vec<String> = changed
            .iter()
            .take(10)
            .map(|p| p.display().to_string())
            .collect();
        return Err(format!(
            "the original changed during the proof ({} entries): {}",
            changed.len(),
            first.join(", ")
        ));
    }
    drop(locks);
    Ok(ProveReport {
        baseline_rebuilt,
        windows,
    })
}
