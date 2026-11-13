//! `cargo hive doctor`: capability checks in a fresh temp dir, the check → capability map,
//! and the `require` gate every capability-dependent command calls.

use cargo_hive_cargo::toolchain::Toolchain;
use cargo_hive_core::receipt::Measured;
use cargo_hive_platform::major_version;
use cargo_hive_platform::proc::{self, ChildOutputError, Spec};
use cargo_hive_platform::{
    Availability, CompressOutcome, EntryKind, Fs, LockHolder, LockKind, LockState, TryLock,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

pub const CAPABILITIES_FILE: &str = "capabilities.json";
pub const MACOS_FLOOR: u32 = 27;
pub const PROBE_DEADLINE: Duration = Duration::from_secs(120);
pub const LOCK_CHECK_VERSION: u32 = 1;
pub const FAILED_MARKER_CHECK_VERSION: u32 = 1;
const PROBE_LISTING_CAP: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckId {
    A,
    B,
    C,
    D,
    F,
    G,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Seeding,
    StoreCompression,
    EvictionAccounting,
    Deletion,
    FingerprintReading,
    FailedUnitReporting,
}

impl CheckId {
    pub fn letter(self) -> &'static str {
        match self {
            CheckId::A => "a",
            CheckId::B => "b",
            CheckId::C => "c",
            CheckId::D => "d",
            CheckId::F => "f",
            CheckId::G => "g",
        }
    }

    /// The capabilities this check vouches for. A fail disables them.
    pub fn disables(self) -> &'static [Capability] {
        match self {
            CheckId::A => &[Capability::Seeding],
            CheckId::B => &[Capability::StoreCompression],
            CheckId::C => &[Capability::EvictionAccounting],
            CheckId::D => &[Capability::Deletion],
            CheckId::F => &[
                Capability::FingerprintReading,
                Capability::Deletion,
                Capability::Seeding,
            ],
            CheckId::G => &[Capability::FailedUnitReporting],
        }
    }
}

impl Capability {
    pub fn describe(self) -> &'static str {
        match self {
            Capability::Seeding => "seeding",
            Capability::StoreCompression => "store compression",
            Capability::EvictionAccounting => "store eviction accounting and private-byte receipts",
            Capability::Deletion => "deletion",
            Capability::FingerprintReading => "fingerprint graph reading",
            Capability::FailedUnitReporting => "failed-unit reporting",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Pass,
    Fail,
    Unavailable,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Pass => "pass",
            Verdict::Fail => "fail",
            Verdict::Unavailable => "unavailable",
        }
    }
}

type Measures = BTreeMap<String, Measured>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub id: CheckId,
    pub verdict: Verdict,
    pub measured: Measures,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    /// `None` until doctor has run.
    pub toolchain_release: Option<String>,
    pub disabled: BTreeMap<Capability, BTreeSet<CheckId>>,
    /// Version of check (d)'s lock proof these results come from. 0 means the results were
    /// recorded before the exclusive-lock matrix existed.
    #[serde(default)]
    pub lock_check: u32,
    /// Version of check (g)'s failed-compile proof these results come from. 0 means check (g)
    /// has not run.
    #[serde(default)]
    pub failed_marker_check: u32,
}

pub struct DoctorEnv {
    pub tmp_parent: PathBuf,
    pub macos: Option<String>,
    pub toolchain: Result<Toolchain, String>,
    /// Polled between steps and while checks f and g build.
    pub cancel: fn() -> bool,
}

#[derive(Debug, Clone)]
pub struct DoctorReport {
    pub macos: Option<String>,
    pub below_floor: bool,
    pub toolchain_release: Option<String>,
    pub checks: Vec<Check>,
    pub stale_removed: u64,
}

#[derive(Debug)]
pub enum DoctorError {
    TempDir(io::Error),
    Cancelled,
}

impl fmt::Display for DoctorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DoctorError::TempDir(e) => {
                write!(f, "could not create doctor's temporary directory: {e}")
            }
            DoctorError::Cancelled => {
                write!(f, "interrupted; the temporary directory was removed")
            }
        }
    }
}

fn checkpoint(cancel: fn() -> bool) -> Result<(), DoctorError> {
    if cancel() {
        Err(DoctorError::Cancelled)
    } else {
        Ok(())
    }
}

/// Removes `hive-doctor-<pid>-*` dirs under `parent` whose process no longer exists.
pub fn sweep_stale(parent: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .and_then(|n| n.strip_prefix("hive-doctor-"))
            .and_then(|rest| rest.split_once('-'))
            .and_then(|(pid, _)| pid.parse::<i32>().ok())
        else {
            continue;
        };
        if pid <= 0
            || pid as u32 == std::process::id()
            || !entry.file_type().is_ok_and(|t| t.is_dir())
        {
            continue;
        }
        if !cargo_hive_platform::alive(pid) && std::fs::remove_dir_all(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CapabilityRefusal {
    RunDoctor(String),
    Disabled {
        capability: Capability,
        checks: BTreeSet<CheckId>,
    },
}

impl fmt::Display for CapabilityRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CapabilityRefusal::RunDoctor(why) => write!(f, "run `cargo hive doctor` first: {why}"),
            CapabilityRefusal::Disabled { capability, checks } => {
                let ids: Vec<String> = checks.iter().map(|c| format!("({})", c.letter())).collect();
                write!(
                    f,
                    "{} is disabled on this machine because doctor check {} did not pass",
                    capability.describe(),
                    ids.join(", ")
                )
            }
        }
    }
}

/// The gate: capability `cap` may be used only if doctor ran for the active toolchain
/// release and none of the checks vouching for `cap` failed.
pub fn require(
    caps: &Capabilities,
    cap: Capability,
    active_release: Option<&str>,
) -> Result<(), CapabilityRefusal> {
    let Some(recorded) = caps.toolchain_release.as_deref() else {
        return Err(CapabilityRefusal::RunDoctor(
            "no doctor results are recorded for this toolchain".into(),
        ));
    };
    if cap == Capability::Deletion && caps.lock_check < LOCK_CHECK_VERSION {
        return Err(CapabilityRefusal::RunDoctor(
            "no doctor results from the current lock check are recorded for this toolchain".into(),
        ));
    }
    let Some(active) = active_release else {
        return Err(CapabilityRefusal::RunDoctor(
            "the active Rust toolchain could not be determined".into(),
        ));
    };
    if recorded != active {
        return Err(CapabilityRefusal::RunDoctor(format!(
            "doctor ran with Rust {recorded}, but the active toolchain is Rust {active}"
        )));
    }
    match caps.disabled.get(&cap) {
        Some(checks) => Err(CapabilityRefusal::Disabled {
            capability: cap,
            checks: checks.clone(),
        }),
        None => Ok(()),
    }
}

pub fn capabilities_for(report: &DoctorReport) -> Capabilities {
    let mut disabled: BTreeMap<Capability, BTreeSet<CheckId>> = BTreeMap::new();
    let mut failed_marker_check = 0;
    for check in &report.checks {
        if check.id == CheckId::G {
            match check.verdict {
                Verdict::Unavailable => {}
                Verdict::Pass => failed_marker_check = FAILED_MARKER_CHECK_VERSION,
                Verdict::Fail => {
                    failed_marker_check = FAILED_MARKER_CHECK_VERSION;
                    for cap in check.id.disables() {
                        disabled.entry(*cap).or_default().insert(check.id);
                    }
                }
            }
            continue;
        }
        if check.verdict != Verdict::Pass {
            for cap in check.id.disables() {
                disabled.entry(*cap).or_default().insert(check.id);
            }
        }
    }
    Capabilities {
        toolchain_release: report.toolchain_release.clone(),
        disabled,
        lock_check: LOCK_CHECK_VERSION,
        failed_marker_check,
    }
}

/// Whether these results allow naming failed units.
pub fn failed_unit_reporting(caps: &Capabilities) -> Availability {
    if caps.failed_marker_check == 0 {
        return Availability::Unavailable(
            "doctor check (g) has not run; run cargo hive doctor".into(),
        );
    }
    if caps.disabled.contains_key(&Capability::FailedUnitReporting) {
        return Availability::Unavailable("doctor check (g) failed".into());
    }
    Availability::Available
}

/// The report line for a macOS below the declared floor; doctor reports it and still runs every check.
pub fn floor_note(report: &DoctorReport) -> Option<String> {
    report.below_floor.then(|| {
        let v = report.macos.clone().unwrap_or_default();
        format!("macOS {v} is below the declared floor {MACOS_FLOOR}; every check still ran, and its results decide support here")
    })
}

/// Seed maps a cancelled `rustup` or `rustc -vV` to exit 130. Any other toolchain
/// failure stays a refusal.
pub(crate) fn toolchain_failure_exit(message: &str) -> i32 {
    if message == "rustup was cancelled" || message == "rustc was cancelled" {
        130
    } else {
        1
    }
}

/// The process cancel flag that `platform::signals::install` sets.
pub fn process_cancel() -> bool {
    cargo_hive_platform::signals::cancelled()
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| ((i % 251) as u8).wrapping_add(seed))
        .collect()
}

fn failed(id: CheckId, e: impl fmt::Display) -> Check {
    Check {
        id,
        verdict: Verdict::Fail,
        measured: Measures::from([("error".to_string(), Measured::Text(e.to_string()))]),
    }
}

fn verdict(ok: bool) -> Verdict {
    if ok { Verdict::Pass } else { Verdict::Fail }
}

/// (a) A clone shares all its blocks: zero private bytes for source and clone.
fn check_clone_sharing(fs: &dyn Fs, dir: &Path) -> Check {
    let attempt = || -> io::Result<Check> {
        let (src, dst) = (dir.join("a-src"), dir.join("a-clone"));
        fs.write_file(&src, &pattern(1 << 20, 0))?;
        fs.clone_file(&src, &dst)?;
        let (s, d) = (fs.sharing(&src)?, fs.sharing(&dst)?);
        let measured = Measures::from([
            (
                "src_private_bytes".to_string(),
                Measured::Int(s.private_bytes),
            ),
            (
                "clone_private_bytes".to_string(),
                Measured::Int(d.private_bytes),
            ),
        ]);
        Ok(Check {
            id: CheckId::A,
            verdict: verdict(s.private_bytes == 0 && d.private_bytes == 0),
            measured,
        })
    };
    attempt().unwrap_or_else(|e| failed(CheckId::A, e))
}

/// (b) Compression round-trips a single-link file and leaves a hard-linked one alone.
fn check_compression(fs: &dyn Fs, dir: &Path) -> Check {
    let attempt = || -> io::Result<Check> {
        let fixed = UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        let single = dir.join("b-single");
        let content = pattern(4 << 20, 0);
        fs.write_file(&single, &content)?;
        fs.set_mtime(&single, fixed)?;
        let single_outcome = fs.compress(&single);
        let single_identical = fs.read(&single)? == content;
        let single_mtime = fs.mtime(&single)? == fixed;
        let (first, second) = (dir.join("b-link1"), dir.join("b-link2"));
        let linked = pattern(1 << 20, 1);
        fs.write_file(&first, &linked)?;
        fs.hardlink(&first, &second)?;
        fs.set_mtime(&first, fixed)?;
        let inode = fs.inode(&first)?;
        let link_outcome = fs.compress(&first);
        let link_unchanged =
            fs.inode(&first)? == inode && fs.read(&first)? == linked && fs.mtime(&first)? == fixed;
        let ok = single_outcome == CompressOutcome::Compressed
            && single_identical
            && single_mtime
            && link_outcome == CompressOutcome::SkippedHardLink
            && link_unchanged;
        let measured = Measures::from([
            (
                "single_outcome".to_string(),
                Measured::Text(single_outcome.to_string()),
            ),
            (
                "single_identical".to_string(),
                Measured::Bool(single_identical),
            ),
            (
                "single_mtime_unchanged".to_string(),
                Measured::Bool(single_mtime),
            ),
            (
                "hardlink_outcome".to_string(),
                Measured::Text(link_outcome.to_string()),
            ),
            (
                "hardlink_unchanged".to_string(),
                Measured::Bool(link_unchanged),
            ),
        ]);
        Ok(Check {
            id: CheckId::B,
            verdict: verdict(ok),
            measured,
        })
    };
    attempt().unwrap_or_else(|e| failed(CheckId::B, e))
}

/// (c) A clone carries SHARES_ALL and its source's clone id; a whole-file rewrite gives it a new id.
fn check_sharing_flags(fs: &dyn Fs, dir: &Path) -> Check {
    let attempt = || -> io::Result<Check> {
        let (src, dst) = (dir.join("c-src"), dir.join("c-clone"));
        fs.write_file(&src, &pattern(1 << 20, 2))?;
        fs.clone_file(&src, &dst)?;
        let (s, d) = (fs.sharing(&src)?, fs.sharing(&dst)?);
        fs.write_file(&dst, &pattern(1 << 20, 3))?;
        let after = fs.sharing(&dst)?;
        let same_id = d.clone_id == s.clone_id;
        let new_id = after.clone_id != s.clone_id;
        let measured = Measures::from([
            ("clone_shares_all".to_string(), Measured::Bool(d.shares_all)),
            ("same_clone_id".to_string(), Measured::Bool(same_id)),
            (
                "new_clone_id_after_rewrite".to_string(),
                Measured::Bool(new_id),
            ),
        ]);
        Ok(Check {
            id: CheckId::C,
            verdict: verdict(d.shares_all && same_id && new_id),
            measured,
        })
    };
    attempt().unwrap_or_else(|e| failed(CheckId::C, e))
}

/// (d) The lock gc deletes under is refused while another process holds any lock kind rustc
/// or Cargo use; while hive holds it, another process's fcntl read and fcntl write tries are refused.
fn check_lock_probe(fs: &dyn Fs, dir: &Path) -> Check {
    let attempt = || -> io::Result<Check> {
        let mut measured = Measures::new();
        let mut ok = true;
        for (kind, name) in [
            (LockKind::Flock, "flock"),
            (LockKind::FcntlRead, "fcntl_read"),
            (LockKind::Fcntl, "fcntl_write"),
        ] {
            let path = dir.join(format!("d-{name}.lock"));
            fs.write_file(&path, b"")?;
            let holder: LockHolder = fs.hold_lock(kind, &path)?;
            let refused = matches!(fs.try_lock_exclusive(&path), TryLock::Held);
            holder.release();
            let free = matches!(fs.try_lock_exclusive(&path), TryLock::Acquired(_));
            measured.insert(
                format!("{name}_holder_refuses_try"),
                Measured::Bool(refused),
            );
            measured.insert(
                format!("{name}_holder_free_after_exit"),
                Measured::Bool(free),
            );
            ok &= refused && free;
        }
        let path = dir.join("d-reverse.lock");
        fs.write_file(&path, b"")?;
        let guard = match fs.try_lock_exclusive(&path) {
            TryLock::Acquired(guard) => guard,
            other => {
                return Err(io::Error::other(format!(
                    "could not take the lock to test the reverse direction: {other:?}"
                )));
            }
        };
        for (kind, name) in [
            (LockKind::FcntlRead, "fcntl_read"),
            (LockKind::Fcntl, "fcntl_write"),
        ] {
            let refused = fs.try_lock_elsewhere(kind, &path)? == LockState::Held;
            measured.insert(
                format!("held_flock_refuses_{name}"),
                Measured::Bool(refused),
            );
            ok &= refused;
        }
        drop(guard);
        Ok(Check {
            id: CheckId::D,
            verdict: verdict(ok),
            measured,
        })
    };
    attempt().unwrap_or_else(|e| failed(CheckId::D, e))
}

/// A crate with a lib, a bin, a build script and an integration test, and no dependencies.
pub const PROBE_CRATE: [(&str, &str); 5] = [
    (
        "Cargo.toml",
        "[package]\nname = \"hive-doctor-probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n",
    ),
    (
        "build.rs",
        "fn main() {\n    println!(\"cargo:rerun-if-changed=build.rs\");\n}\n",
    ),
    ("src/lib.rs", "pub fn answer() -> u32 {\n    42\n}\n"),
    (
        "src/main.rs",
        "fn main() {\n    println!(\"{}\", hive_doctor_probe::answer());\n}\n",
    ),
    (
        "tests/it.rs",
        "#[test]\nfn answers() {\n    assert_eq!(hive_doctor_probe::answer(), 42);\n}\n",
    ),
];

/// The allowlisted environment for the probe build (Decision 7); nothing else is inherited.
fn probe_command(tc: &Toolchain, krate: &Path, target: &Path, cargo_home: &Path) -> Spec {
    let mut spec = Spec::new(&tc.cargo);
    spec.args = ["build", "--offline", "--all-targets", "--quiet"]
        .into_iter()
        .map(Into::into)
        .collect();
    spec.cwd = Some(krate.to_path_buf());
    let parent: Vec<_> = std::env::vars_os().collect();
    let rustup_home = std::env::var_os("RUSTUP_HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let name = tc
        .name
        .as_deref()
        .map(std::ffi::OsStr::new)
        .unwrap_or_else(|| std::ffi::OsStr::new(""));
    cargo_hive_cargo::env::probe_build(&mut spec, &parent, cargo_home, &rustup_home, name);
    spec.env_set
        .push(("CARGO_TARGET_DIR".into(), target.as_os_str().into()));
    spec.env_set
        .push(("RUSTC".into(), tc.rustc.as_os_str().into()));
    spec.stdin_null = true;
    spec
}

fn tail(text: &str, max: usize) -> String {
    let start = text.len().saturating_sub(max);
    let start = (start..=text.len())
        .find(|i| text.is_char_boundary(*i))
        .unwrap_or(text.len());
    text[start..].trim().to_string()
}

/// (f) The fingerprints the active toolchain writes match the supported signature.
fn check_format(
    fs: &dyn Fs,
    dir: &Path,
    toolchain: &Result<Toolchain, String>,
    cancel: fn() -> bool,
) -> Result<Check, DoctorError> {
    let tc = match toolchain {
        Ok(t) => t,
        Err(reason) => {
            return Ok(Check {
                id: CheckId::F,
                verdict: Verdict::Unavailable,
                measured: Measures::from([("reason".to_string(), Measured::Text(reason.clone()))]),
            });
        }
    };
    let attempt = || -> io::Result<Result<Check, DoctorError>> {
        let (krate, target, cargo_home) = (
            dir.join("f-crate"),
            dir.join("f-target"),
            dir.join("f-cargo-home"),
        );
        for (rel, body) in PROBE_CRATE {
            let path = krate.join(rel);
            std::fs::create_dir_all(path.parent().expect("probe files live in the crate"))?;
            fs.write_file(&path, body.as_bytes())?;
        }
        std::fs::create_dir_all(&cargo_home)?;
        let cmd = probe_command(tc, &krate, &target, &cargo_home);
        let output = match proc::output_with_deadline(&cmd, PROBE_DEADLINE, cancel) {
            Err(ChildOutputError::Cancelled) => {
                return Ok(Err(DoctorError::Cancelled));
            }
            Err(ChildOutputError::TimedOut) => {
                return Ok(Ok(failed(
                    CheckId::F,
                    "the probe build exceeded its deadline",
                )));
            }
            Err(ChildOutputError::Failed(message)) => {
                return Ok(Ok(failed(CheckId::F, message)));
            }
            Ok(output) => output,
        };
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Ok(Ok(Check {
                id: CheckId::F,
                verdict: Verdict::Fail,
                measured: Measures::from([
                    (
                        "cargo_status".to_string(),
                        Measured::Text(output.status.to_string()),
                    ),
                    (
                        "stderr_tail".to_string(),
                        Measured::Text(tail(&stderr, 600)),
                    ),
                ]),
            }));
        }
        let report = cargo_hive_core::fingerprint::read_profile(&target.join("debug"));
        let mut measured = Measures::from([
            ("toolchain".to_string(), Measured::Text(tc.release.clone())),
            ("units".to_string(), Measured::Int(report.units as u64)),
        ]);
        let ok = match (&report.format, &report.edges) {
            (cargo_hive_core::fingerprint::Format::Supported, Some(e)) => {
                measured.insert("format".into(), Measured::Text("supported".into()));
                measured.insert("edges_resolved".into(), Measured::Int(e.resolved));
                measured.insert("edges_unresolved".into(), Measured::Int(e.unresolved));
                measured.insert("edges_ambiguous".into(), Measured::Int(e.ambiguous));
                report.units > 0 && e.resolved > 0 && e.unresolved == 0 && e.ambiguous == 0
            }
            (cargo_hive_core::fingerprint::Format::Unsupported(why), _) => {
                measured.insert(
                    "format".into(),
                    Measured::Text(format!("unsupported: {why}")),
                );
                false
            }
            (cargo_hive_core::fingerprint::Format::Supported, None) => false,
        };
        Ok(Ok(Check {
            id: CheckId::F,
            verdict: verdict(ok),
            measured,
        }))
    };
    match attempt() {
        Ok(result) => result,
        Err(e) => Ok(failed(CheckId::F, e)),
    }
}

fn unavailable_g(reason: impl fmt::Display) -> Check {
    Check {
        id: CheckId::G,
        verdict: Verdict::Unavailable,
        measured: Measures::from([("reason".to_string(), Measured::Text(reason.to_string()))]),
    }
}

fn summary_line_count(outputs: &[String]) -> u64 {
    outputs
        .iter()
        .flat_map(|text| text.lines())
        .filter(|line| cargo_hive_core::fingerprint::is_summary_line(line) == Some(true))
        .count() as u64
}

fn judge_failed_marker(exit_ok: bool, outputs: &[String]) -> Verdict {
    if exit_ok {
        Verdict::Unavailable
    } else if summary_line_count(outputs) > 0 {
        Verdict::Pass
    } else {
        Verdict::Fail
    }
}

fn probe_output_texts(fs: &dyn Fs, fingerprint: &Path) -> Result<Vec<String>, String> {
    if !fingerprint.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths = Vec::new();
    let root = fs
        .read_dir_bounded(fingerprint, PROBE_LISTING_CAP)
        .map_err(|e| e.to_string())?;
    for (name, kind) in root {
        match kind {
            EntryKind::File if name.starts_with("output-") => paths.push(fingerprint.join(name)),
            EntryKind::Dir => {
                let dir = fingerprint.join(&name);
                let children = fs
                    .read_dir_bounded(&dir, PROBE_LISTING_CAP)
                    .map_err(|e| e.to_string())?;
                for (child, child_kind) in children {
                    if child_kind == EntryKind::File && child.starts_with("output-") {
                        paths.push(dir.join(child));
                    }
                }
            }
            _ => {}
        }
    }
    paths.sort();
    let mut texts = Vec::with_capacity(paths.len());
    for path in paths {
        let mut file =
            std::fs::File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let bytes = cargo_hive_core::fingerprint::read_capped(&mut file)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        texts.push(String::from_utf8(bytes).map_err(|e| format!("{}: {e}", path.display()))?);
    }
    Ok(texts)
}

fn write_failing_probe(fs: &dyn Fs, krate: &Path, cargo_home: &Path) -> io::Result<()> {
    for (rel, body) in PROBE_CRATE {
        let path = krate.join(rel);
        std::fs::create_dir_all(path.parent().expect("probe files live in the crate"))?;
        let mut bytes = body.as_bytes().to_vec();
        if rel == "src/lib.rs" {
            bytes.extend_from_slice(b"pub fn hive_probe_failure() -> u32 { \"x\" }\n");
        }
        fs.write_file(&path, &bytes)?;
    }
    std::fs::create_dir_all(cargo_home)
}

/// (g) A failed compile leaves rustc's summary line in an `output-*` file.
fn check_failed_marker(
    fs: &dyn Fs,
    dir: &Path,
    toolchain: &Result<Toolchain, String>,
    cancel: fn() -> bool,
) -> Result<Check, DoctorError> {
    let tc = match toolchain {
        Ok(t) => t,
        Err(reason) => return Ok(unavailable_g(reason)),
    };
    let (krate, target, cargo_home) = (
        dir.join("g"),
        dir.join("g-target"),
        dir.join("g-cargo-home"),
    );
    if let Err(e) = write_failing_probe(fs, &krate, &cargo_home) {
        return Ok(unavailable_g(e));
    }
    let cmd = probe_command(tc, &krate, &target, &cargo_home);
    let output = match proc::output_with_deadline(&cmd, PROBE_DEADLINE, cancel) {
        Err(ChildOutputError::Cancelled) => {
            return Err(DoctorError::Cancelled);
        }
        Err(ChildOutputError::TimedOut) => {
            return Ok(unavailable_g("the probe build exceeded its deadline"));
        }
        Err(ChildOutputError::Failed(message)) => {
            return Ok(unavailable_g(message));
        }
        Ok(output) => output,
    };
    let texts = match probe_output_texts(fs, &target.join("debug/.fingerprint")) {
        Ok(texts) => texts,
        Err(reason) => return Ok(unavailable_g(reason)),
    };
    let exit_ok = output.status.success();
    Ok(Check {
        id: CheckId::G,
        verdict: judge_failed_marker(exit_ok, &texts),
        measured: Measures::from([
            ("exit_ok".to_string(), Measured::Bool(exit_ok)),
            ("outputs".to_string(), Measured::Int(texts.len() as u64)),
            (
                "summary_lines".to_string(),
                Measured::Int(summary_line_count(&texts)),
            ),
        ]),
    })
}

pub fn run(fs: &dyn Fs, env: &DoctorEnv) -> Result<DoctorReport, DoctorError> {
    let stale_removed = sweep_stale(&env.tmp_parent);
    let tmp = fs
        .temp_dir(
            &env.tmp_parent,
            &format!("hive-doctor-{}-", std::process::id()),
        )
        .map_err(DoctorError::TempDir)?;
    let dir = tmp.path();
    let steps: [&dyn Fn() -> Check; 4] = [
        &|| check_clone_sharing(fs, dir),
        &|| check_compression(fs, dir),
        &|| check_sharing_flags(fs, dir),
        &|| check_lock_probe(fs, dir),
    ];
    let mut checks = Vec::new();
    for step in steps {
        checkpoint(env.cancel)?;
        checks.push(step());
    }
    checkpoint(env.cancel)?;
    checks.push(check_format(fs, dir, &env.toolchain, env.cancel)?);
    checkpoint(env.cancel)?;
    checks.push(check_failed_marker(fs, dir, &env.toolchain, env.cancel)?);
    Ok(DoctorReport {
        macos: env.macos.clone(),
        below_floor: env
            .macos
            .as_deref()
            .and_then(major_version)
            .is_some_and(|m| m < MACOS_FLOOR),
        toolchain_release: env.toolchain.as_ref().ok().map(|t| t.release.clone()),
        checks,
        stale_removed,
    })
}

#[cfg(test)]
#[allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod tests {
    use super::*;
    use cargo_hive_platform::{
        Availability, EntryKind, EntryStat, HeldLock, Sharing, TempDir, TryLock, VolumeInfo,
        entry_kind,
    };
    use std::collections::HashMap;
    use std::os::unix::fs::MetadataExt;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::SystemTime;

    #[test]
    fn the_probe_build_deadline_is_two_minutes() {
        assert_eq!(PROBE_DEADLINE.as_secs(), 120);
    }

    #[derive(Clone)]
    struct Knobs {
        clone_private: u64,
        shares_all: bool,
        rewrite_keeps_id: bool,
        single: CompressOutcome,
        linked: CompressOutcome,
        compress_touches_mtime: bool,
        probe_sees_locks: bool,
        panic_in_compress: bool,
        reverse_refused: bool,
        blind_kind: Option<LockKind>,
    }

    impl Default for Knobs {
        fn default() -> Knobs {
            Knobs {
                clone_private: 0,
                shares_all: true,
                rewrite_keeps_id: false,
                single: CompressOutcome::Compressed,
                linked: CompressOutcome::SkippedHardLink,
                compress_touches_mtime: false,
                probe_sees_locks: true,
                panic_in_compress: false,
                reverse_refused: true,
                blind_kind: None,
            }
        }
    }

    /// Real files in a scratch dir; sharing, compression and locks are scripted by `Knobs`.
    struct FakeFs {
        knobs: Knobs,
        ids: Mutex<HashMap<PathBuf, u64>>,
        next: AtomicU64,
        held_kind: Arc<Mutex<Option<LockKind>>>,
    }

    impl FakeFs {
        fn new(knobs: Knobs) -> FakeFs {
            FakeFs {
                knobs,
                ids: Mutex::new(HashMap::new()),
                next: AtomicU64::new(1),
                held_kind: Arc::new(Mutex::new(None)),
            }
        }
    }

    impl Fs for FakeFs {
        fn temp_dir(&self, parent: &Path, prefix: &str) -> io::Result<TempDir> {
            let path = parent.join(format!(
                "{prefix}{}",
                self.next.fetch_add(1, Ordering::SeqCst)
            ));
            std::fs::create_dir_all(&path)?;
            Ok(TempDir::adopt(path))
        }
        fn write_file(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
            std::fs::write(path, bytes)?;
            let mut ids = self.ids.lock().unwrap();
            if !(self.knobs.rewrite_keeps_id && ids.contains_key(path)) {
                ids.insert(path.to_path_buf(), self.next.fetch_add(1, Ordering::SeqCst));
            }
            Ok(())
        }
        fn read(&self, path: &Path) -> io::Result<Vec<u8>> {
            std::fs::read(path)
        }
        fn hardlink(&self, src: &Path, dst: &Path) -> io::Result<()> {
            std::fs::hard_link(src, dst)
        }
        fn set_mtime(&self, path: &Path, t: SystemTime) -> io::Result<()> {
            std::fs::File::options()
                .write(true)
                .open(path)?
                .set_modified(t)
        }
        fn mtime(&self, path: &Path) -> io::Result<SystemTime> {
            std::fs::metadata(path)?.modified()
        }
        fn inode(&self, path: &Path) -> io::Result<u64> {
            Ok(std::fs::metadata(path)?.ino())
        }
        fn clone_file(&self, src: &Path, dst: &Path) -> io::Result<()> {
            std::fs::copy(src, dst)?;
            let mut ids = self.ids.lock().unwrap();
            let id = ids[src];
            ids.insert(dst.to_path_buf(), id);
            Ok(())
        }
        fn create_dir(&self, path: &Path) -> io::Result<()> {
            std::fs::create_dir(path)
        }
        fn sharing(&self, path: &Path) -> io::Result<Sharing> {
            let ids = self.ids.lock().unwrap();
            let id = ids.get(path).copied().unwrap_or(0);
            let shared = ids.values().filter(|v| **v == id).count() > 1;
            let len = std::fs::metadata(path)?.len();
            Ok(Sharing {
                private_bytes: if shared {
                    self.knobs.clone_private
                } else {
                    len
                },
                clone_id: id,
                may_share: shared,
                shares_all: shared && self.knobs.shares_all,
            })
        }
        fn compress(&self, path: &Path) -> CompressOutcome {
            assert!(!self.knobs.panic_in_compress, "injected panic");
            if self.knobs.compress_touches_mtime {
                let _ = self.set_mtime(path, SystemTime::now());
            }
            match std::fs::metadata(path) {
                Ok(m) if m.nlink() > 1 => self.knobs.linked.clone(),
                _ => self.knobs.single.clone(),
            }
        }
        fn hold_lock(&self, kind: LockKind, _path: &Path) -> io::Result<LockHolder> {
            *self.held_kind.lock().unwrap() = Some(kind);
            let held = Arc::clone(&self.held_kind);
            Ok(LockHolder::new(move || *held.lock().unwrap() = None))
        }
        fn try_lock_exclusive(&self, _path: &Path) -> TryLock {
            let held_kind = *self.held_kind.lock().unwrap();
            let seen = self.knobs.probe_sees_locks
                && held_kind.is_some()
                && held_kind != self.knobs.blind_kind;
            if seen {
                TryLock::Held
            } else {
                TryLock::Acquired(HeldLock::new(|| {}))
            }
        }
        fn try_lock_elsewhere(&self, _kind: LockKind, _path: &Path) -> io::Result<LockState> {
            Ok(if self.knobs.reverse_refused {
                LockState::Held
            } else {
                LockState::Free
            })
        }
        fn entry_stat(&self, path: &Path) -> io::Result<EntryStat> {
            let m = std::fs::symlink_metadata(path)?;
            let kind = entry_kind(m.file_type());
            Ok(EntryStat {
                dev: m.dev(),
                ino: m.ino(),
                nlink: m.nlink(),
                blocks: m.blocks(),
                kind,
                compressed: false,
            })
        }
        fn read_dir_bounded(
            &self,
            dir: &Path,
            _cap: usize,
        ) -> io::Result<Vec<(String, EntryKind)>> {
            std::fs::read_dir(dir)?
                .map(|e| {
                    let e = e?;
                    let kind = if e.file_type()?.is_dir() {
                        EntryKind::Dir
                    } else {
                        EntryKind::File
                    };
                    Ok((e.file_name().to_string_lossy().into_owned(), kind))
                })
                .collect()
        }
        fn remove_tree(&self, path: &Path) -> io::Result<()> {
            std::fs::remove_dir_all(path)
        }
        fn remove_file(&self, path: &Path) -> io::Result<()> {
            std::fs::remove_file(path)
        }
        fn volume(&self, _path: &Path) -> io::Result<VolumeInfo> {
            Ok(VolumeInfo {
                fs_type: "apfs".into(),
                container: "disk0".into(),
                mount: PathBuf::from("/"),
                available_bytes: 0,
            })
        }
    }

    fn scratch(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!(
            "hive-doctor-unit-{label}-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn env(parent: &Path, macos: &str) -> DoctorEnv {
        DoctorEnv {
            tmp_parent: parent.to_path_buf(),
            macos: Some(macos.into()),
            toolchain: Err("not used by this test".into()),
            cancel: || false,
        }
    }

    /// Capabilities from checks a–d only: checks f and g need a real toolchain.
    fn caps_a_to_d(report: &DoctorReport) -> Capabilities {
        let mut only = report.clone();
        only.checks
            .retain(|c| c.id != CheckId::F && c.id != CheckId::G);
        capabilities_for(&only)
    }

    static SERIAL: Mutex<()> = Mutex::new(());

    fn leftovers(parent: &Path) -> Vec<String> {
        std::fs::read_dir(parent)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with("hive-doctor-"))
            .collect()
    }

    #[test]
    fn a_healthy_machine_passes_checks_a_to_e_and_disables_nothing() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let parent = scratch("healthy");
        let report = run(&FakeFs::new(Knobs::default()), &env(&parent, "27.0.1")).unwrap();
        for c in report
            .checks
            .iter()
            .filter(|c| c.id != CheckId::F && c.id != CheckId::G)
        {
            assert_eq!(c.verdict, Verdict::Pass, "{:?}: {:?}", c.id, c.measured);
            assert!(!c.measured.is_empty(), "{:?} has no measured value", c.id);
        }
        assert!(caps_a_to_d(&report).disabled.is_empty());
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn each_failing_check_disables_exactly_its_capabilities() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        use Capability::*;
        let cases: Vec<(&str, Knobs, CheckId, Vec<Capability>)> = vec![
            (
                "clone keeps private bytes",
                Knobs {
                    clone_private: 4096,
                    ..Knobs::default()
                },
                CheckId::A,
                vec![Seeding],
            ),
            (
                "compression fails",
                Knobs {
                    single: CompressOutcome::Failed("x".into()),
                    ..Knobs::default()
                },
                CheckId::B,
                vec![StoreCompression],
            ),
            (
                "hard-linked file compressed",
                Knobs {
                    linked: CompressOutcome::Compressed,
                    ..Knobs::default()
                },
                CheckId::B,
                vec![StoreCompression],
            ),
            (
                "compression moves the mtime",
                Knobs {
                    compress_touches_mtime: true,
                    ..Knobs::default()
                },
                CheckId::B,
                vec![StoreCompression],
            ),
            (
                "no shares-all flag",
                Knobs {
                    shares_all: false,
                    ..Knobs::default()
                },
                CheckId::C,
                vec![EvictionAccounting],
            ),
            (
                "rewrite keeps the clone id",
                Knobs {
                    rewrite_keeps_id: true,
                    ..Knobs::default()
                },
                CheckId::C,
                vec![EvictionAccounting],
            ),
            (
                "probe misses held locks",
                Knobs {
                    probe_sees_locks: false,
                    ..Knobs::default()
                },
                CheckId::D,
                vec![Deletion],
            ),
            (
                "an fcntl read lock held elsewhere goes unseen",
                Knobs {
                    blind_kind: Some(LockKind::FcntlRead),
                    ..Knobs::default()
                },
                CheckId::D,
                vec![Deletion],
            ),
            (
                "another process can lock while hive holds the lock",
                Knobs {
                    reverse_refused: false,
                    ..Knobs::default()
                },
                CheckId::D,
                vec![Deletion],
            ),
        ];
        for (name, knobs, failing, mut want) in cases {
            let parent = scratch("table");
            let report = run(&FakeFs::new(knobs), &env(&parent, "27.0.1")).unwrap();
            for c in report
                .checks
                .iter()
                .filter(|c| c.id != CheckId::F && c.id != CheckId::G)
            {
                let expected = if c.id == failing {
                    Verdict::Fail
                } else {
                    Verdict::Pass
                };
                assert_eq!(
                    c.verdict, expected,
                    "{name}: check {:?} {:?}",
                    c.id, c.measured
                );
            }
            want.sort();
            assert_eq!(
                caps_a_to_d(&report)
                    .disabled
                    .keys()
                    .copied()
                    .collect::<Vec<_>>(),
                want,
                "{name}"
            );
            std::fs::remove_dir_all(parent).unwrap();
        }
    }

    #[test]
    fn check_f_disables_reading_deletion_and_seeding() {
        let mut want = vec![
            Capability::FingerprintReading,
            Capability::Deletion,
            Capability::Seeding,
        ];
        want.sort();
        let mut got = CheckId::F.disables().to_vec();
        got.sort();
        assert_eq!(got, want);
    }

    #[test]
    fn a_macos_below_the_floor_is_flagged_and_every_check_still_runs() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let parent = scratch("floor");
        let report = run(&FakeFs::new(Knobs::default()), &env(&parent, "26.4")).unwrap();
        assert!(report.below_floor);
        for id in [CheckId::A, CheckId::B, CheckId::C, CheckId::D] {
            assert!(
                report.checks.iter().any(|c| c.id == id),
                "{id:?} did not run"
            );
        }
        let note = floor_note(&report).expect("a note for a version below the floor");
        assert!(
            note.contains("26.4") && note.contains("below the declared floor 27"),
            "{note}"
        );
        let current = run(&FakeFs::new(Knobs::default()), &env(&parent, "27.0.1")).unwrap();
        assert_eq!(floor_note(&current), None);
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn require_refuses_without_results_for_another_toolchain_and_for_a_disabled_capability() {
        let none = Capabilities::default();
        assert!(matches!(
            require(&none, Capability::FingerprintReading, Some("1.97.1")),
            Err(CapabilityRefusal::RunDoctor(_))
        ));
        let other = Capabilities {
            toolchain_release: Some("1.96.1".into()),
            disabled: BTreeMap::new(),
            lock_check: 0,
            failed_marker_check: 0,
        };
        let e = require(&other, Capability::FingerprintReading, Some("1.97.1"))
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("cargo hive doctor") && e.contains("1.96.1"),
            "{e}"
        );
        assert!(matches!(
            require(&other, Capability::FingerprintReading, None),
            Err(CapabilityRefusal::RunDoctor(_))
        ));
        let disabled =
            BTreeMap::from([(Capability::FingerprintReading, BTreeSet::from([CheckId::F]))]);
        let off = Capabilities {
            toolchain_release: Some("1.97.1".into()),
            disabled,
            lock_check: 0,
            failed_marker_check: 0,
        };
        let e = require(&off, Capability::FingerprintReading, Some("1.97.1"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("(f)"), "{e}");
        assert_eq!(require(&off, Capability::Seeding, Some("1.97.1")), Ok(()));
    }

    #[test]
    fn check_d_reports_the_full_lock_matrix() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let parent = scratch("matrix");
        let report = run(&FakeFs::new(Knobs::default()), &env(&parent, "27.0.1")).unwrap();
        let d = report.checks.iter().find(|c| c.id == CheckId::D).unwrap();
        let mut keys: Vec<&str> = d.measured.keys().map(String::as_str).collect();
        keys.sort();
        let mut want = vec![
            "fcntl_read_holder_free_after_exit",
            "fcntl_read_holder_refuses_try",
            "fcntl_write_holder_free_after_exit",
            "fcntl_write_holder_refuses_try",
            "flock_holder_free_after_exit",
            "flock_holder_refuses_try",
            "held_flock_refuses_fcntl_read",
            "held_flock_refuses_fcntl_write",
        ];
        want.sort();
        assert_eq!(keys, want);
        assert_eq!(d.verdict, Verdict::Pass, "{:?}", d.measured);
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn deletion_needs_results_from_the_current_lock_check() {
        let old = Capabilities {
            toolchain_release: Some("1.97.1".into()),
            disabled: BTreeMap::new(),
            lock_check: 0,
            failed_marker_check: 0,
        };
        let e = require(&old, Capability::Deletion, Some("1.97.1")).unwrap_err();
        assert!(matches!(e, CapabilityRefusal::RunDoctor(_)), "{e}");
        assert!(e.to_string().contains("cargo hive doctor"), "{e}");
        assert_eq!(
            require(&old, Capability::FingerprintReading, Some("1.97.1")),
            Ok(())
        );
        let current = Capabilities {
            lock_check: LOCK_CHECK_VERSION,
            ..old
        };
        assert_eq!(
            require(&current, Capability::Deletion, Some("1.97.1")),
            Ok(())
        );
    }

    #[test]
    fn capabilities_recorded_before_the_lock_check_still_parse() {
        let caps: Capabilities =
            serde_json::from_str(r#"{"toolchain_release":"1.97.1","disabled":{}}"#).unwrap();
        assert_eq!(caps.lock_check, 0);
        assert_eq!(LOCK_CHECK_VERSION, 1);
    }

    fn report_with(checks: Vec<Check>) -> DoctorReport {
        DoctorReport {
            macos: None,
            below_floor: false,
            toolchain_release: Some("1.97.1".into()),
            checks,
            stale_removed: 0,
        }
    }

    fn marker_check(verdict: Verdict) -> Check {
        Check {
            id: CheckId::G,
            verdict,
            measured: Measures::new(),
        }
    }

    #[test]
    fn judge_failed_marker_passes_only_when_the_build_failed_and_left_a_summary_line() {
        let summary = concat!(
            "warning only\n",
            r#"{"$message_type":"diagnostic","message":"aborting due to 1 previous error","code":null,"level":"error","spans":[]}"#,
            "\n",
        );
        let cases = [
            (false, vec![summary.to_string()], Verdict::Pass),
            (false, vec!["warning only".to_string()], Verdict::Fail),
            (true, Vec::<String>::new(), Verdict::Unavailable),
            (false, Vec::<String>::new(), Verdict::Fail),
        ];
        for (exit_ok, outputs, want) in cases {
            assert_eq!(
                judge_failed_marker(exit_ok, &outputs),
                want,
                "exit_ok={exit_ok} outputs={outputs:?}"
            );
        }
    }

    #[test]
    fn an_unavailable_check_g_disables_nothing_and_leaves_g_not_run() {
        let unavailable = capabilities_for(&report_with(vec![marker_check(Verdict::Unavailable)]));
        assert!(unavailable.disabled.is_empty());
        assert_eq!(unavailable.failed_marker_check, 0);
        assert_eq!(unavailable.lock_check, LOCK_CHECK_VERSION);
        assert_eq!(
            failed_unit_reporting(&unavailable),
            Availability::Unavailable("doctor check (g) has not run; run cargo hive doctor".into())
        );

        let failed = capabilities_for(&report_with(vec![marker_check(Verdict::Fail)]));
        assert_eq!(
            failed.disabled.keys().copied().collect::<Vec<_>>(),
            vec![Capability::FailedUnitReporting]
        );
        assert_eq!(failed.failed_marker_check, FAILED_MARKER_CHECK_VERSION);
        assert_eq!(
            failed_unit_reporting(&failed),
            Availability::Unavailable("doctor check (g) failed".into())
        );

        let passed = capabilities_for(&report_with(vec![marker_check(Verdict::Pass)]));
        assert!(passed.disabled.is_empty());
        assert_eq!(passed.failed_marker_check, FAILED_MARKER_CHECK_VERSION);
        assert_eq!(failed_unit_reporting(&passed), Availability::Available);
    }

    #[test]
    fn a_capabilities_record_without_check_g_loads_and_reads_as_not_run() {
        let caps: Capabilities =
            serde_json::from_str(r#"{"toolchain_release":"1.97.1","disabled":{}}"#).unwrap();
        assert_eq!(caps.failed_marker_check, 0);
        assert_eq!(FAILED_MARKER_CHECK_VERSION, 1);
        assert_eq!(
            failed_unit_reporting(&caps),
            Availability::Unavailable("doctor check (g) has not run; run cargo hive doctor".into())
        );
    }

    #[test]
    fn a_failed_check_g_disables_only_failed_unit_reporting() {
        let caps = capabilities_for(&report_with(vec![marker_check(Verdict::Fail)]));
        assert_eq!(
            caps.disabled.get(&Capability::FailedUnitReporting),
            Some(&BTreeSet::from([CheckId::G]))
        );
        assert!(!caps.disabled.contains_key(&Capability::FingerprintReading));
        assert!(!caps.disabled.contains_key(&Capability::Deletion));
        assert!(!caps.disabled.contains_key(&Capability::Seeding));
        assert_eq!(caps.disabled.len(), 1);
        assert_eq!(caps.failed_marker_check, FAILED_MARKER_CHECK_VERSION);
    }

    #[test]
    fn a_cancelled_toolchain_lookup_exits_130() {
        assert_eq!(toolchain_failure_exit("rustup was cancelled"), 130);
        assert_eq!(toolchain_failure_exit("rustc was cancelled"), 130);
        assert_eq!(toolchain_failure_exit("rustup exceeded its deadline"), 1);
        assert_eq!(
            toolchain_failure_exit(
                "could not run rustup: could not spawn: Not a directory (os error 20)"
            ),
            1
        );
    }

    #[test]
    fn the_temp_dir_is_removed_after_success_after_a_failed_check_and_after_a_panic() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let parent = scratch("cleanup");
        run(&FakeFs::new(Knobs::default()), &env(&parent, "27.0.1")).unwrap();
        assert_eq!(leftovers(&parent), Vec::<String>::new(), "after success");
        run(
            &FakeFs::new(Knobs {
                probe_sees_locks: false,
                ..Knobs::default()
            }),
            &env(&parent, "27.0.1"),
        )
        .unwrap();
        assert_eq!(
            leftovers(&parent),
            Vec::<String>::new(),
            "after a failed check"
        );
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run(
                &FakeFs::new(Knobs {
                    panic_in_compress: true,
                    ..Knobs::default()
                }),
                &env(&parent, "27.0.1"),
            )
        }));
        assert!(panicked.is_err());
        assert_eq!(leftovers(&parent), Vec::<String>::new(), "after a panic");
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn a_cancelled_run_stops_and_removes_its_temp_dir() {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let parent = scratch("cancel");
        let cancelled = DoctorEnv {
            cancel: || true,
            ..env(&parent, "27.0.1")
        };
        let result = run(&FakeFs::new(Knobs::default()), &cancelled);
        assert!(matches!(result, Err(DoctorError::Cancelled)));
        assert!(
            !cargo_hive_platform::signals::cancelled(),
            "the test cancels through its own env, not the process flag"
        );
        assert_eq!(leftovers(&parent), Vec::<String>::new());
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn the_sweep_removes_only_dirs_of_dead_doctor_processes() {
        let parent = scratch("sweep");
        let mut dead = Command::new("true").spawn().unwrap();
        let dead_pid = dead.id();
        dead.wait().unwrap();
        let mut alive = Command::new("sleep").arg("30").spawn().unwrap();
        let stale = parent.join(format!("hive-doctor-{dead_pid}-abcd1234"));
        let live = parent.join(format!("hive-doctor-{}-abcd1234", alive.id()));
        let odd = parent.join("hive-doctor-notapid-abcd1234");
        let file = parent.join(format!("hive-doctor-{dead_pid}-file"));
        for d in [&stale, &live, &odd] {
            std::fs::create_dir_all(d.join("inner")).unwrap();
        }
        std::fs::write(&file, b"x").unwrap();
        assert_eq!(sweep_stale(&parent), 1);
        assert!(!stale.exists() && live.exists() && odd.exists() && file.exists());
        alive.kill().unwrap();
        alive.wait().unwrap();
        std::fs::remove_dir_all(parent).unwrap();
    }
}
