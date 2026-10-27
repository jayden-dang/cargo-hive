//! `cargo hive doctor`: capability checks in a fresh temp dir, the check → capability map,
//! and the `require` gate every capability-dependent command calls.

use crate::platform::major_version;
use crate::platform::{
    Approve, Availability, CompressOutcome, Fs, LockHolder, LockKind, LockState,
};
use crate::receipt::Measured;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, UNIX_EPOCH};

pub const CAPABILITIES_FILE: &str = "capabilities.json";
pub const MACOS_FLOOR: u32 = 27;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckId {
    A,
    B,
    C,
    D,
    E,
    F,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Seeding,
    StoreCompression,
    EvictionAccounting,
    Deletion,
    Approval,
    FingerprintReading,
}

impl CheckId {
    pub fn letter(self) -> &'static str {
        match self {
            CheckId::A => "a",
            CheckId::B => "b",
            CheckId::C => "c",
            CheckId::D => "d",
            CheckId::E => "e",
            CheckId::F => "f",
        }
    }

    /// The capabilities this check vouches for: a check that does not pass disables them.
    pub fn disables(self) -> &'static [Capability] {
        match self {
            CheckId::A => &[Capability::Seeding],
            CheckId::B => &[Capability::StoreCompression],
            CheckId::C => &[Capability::EvictionAccounting],
            CheckId::D => &[Capability::Deletion],
            CheckId::E => &[Capability::Approval],
            CheckId::F => &[
                Capability::FingerprintReading,
                Capability::Deletion,
                Capability::Seeding,
            ],
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
            Capability::Approval => "approval",
            Capability::FingerprintReading => "fingerprint graph reading",
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Toolchain {
    pub name: Option<String>,
    pub cargo: PathBuf,
    pub rustc: PathBuf,
    pub release: String,
}

pub struct DoctorEnv {
    pub tmp_parent: PathBuf,
    pub macos: Option<String>,
    pub toolchain: Result<Toolchain, String>,
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
}

impl fmt::Display for DoctorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DoctorError::TempDir(e) => {
                write!(f, "could not create doctor's temporary directory: {e}")
            }
        }
    }
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
    for check in report.checks.iter().filter(|c| c.verdict != Verdict::Pass) {
        for cap in check.id.disables() {
            disabled.entry(*cap).or_default().insert(check.id);
        }
    }
    Capabilities {
        toolchain_release: report.toolchain_release.clone(),
        disabled,
    }
}

/// The report line for a macOS below the declared floor; doctor reports it and still runs every check.
pub fn floor_note(report: &DoctorReport) -> Option<String> {
    report.below_floor.then(|| {
        let v = report.macos.clone().unwrap_or_default();
        format!("macOS {v} is below the declared floor {MACOS_FLOOR}; every check still ran, and its results decide support here")
    })
}

fn rustup_which(tool: &str, cwd: &Path) -> Result<Option<PathBuf>, String> {
    match Command::new("rustup")
        .args(["which", tool])
        .current_dir(cwd)
        .env("RUSTUP_AUTO_INSTALL", "0")
        .stdin(Stdio::null())
        .output()
    {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("could not run rustup: {e}")),
        Ok(out) if out.status.success() => Ok(Some(PathBuf::from(
            String::from_utf8_lossy(&out.stdout).trim(),
        ))),
        Ok(out) => Err(format!(
            "rustup which {tool} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
    }
}

/// The toolchain a build started in `cwd` would use: rustup's real binaries, or the
/// `cargo` and `rustc` on PATH when rustup is absent.
pub fn resolve_toolchain(cwd: &Path) -> Result<Toolchain, String> {
    let (cargo, rustc) = match (rustup_which("cargo", cwd)?, rustup_which("rustc", cwd)?) {
        (Some(c), Some(r)) => (c, r),
        _ => (PathBuf::from("cargo"), PathBuf::from("rustc")),
    };
    // `…/toolchains/<name>/bin/rustc`
    let name = rustc
        .parent()
        .and_then(Path::parent)
        .filter(|dir| dir.parent().is_some_and(|p| p.ends_with("toolchains")))
        .and_then(Path::file_name)
        .map(|n| n.to_string_lossy().to_string());
    let out = Command::new(&rustc)
        .arg("-vV")
        .current_dir(cwd)
        .env("RUSTUP_AUTO_INSTALL", "0")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("could not run {}: {e}", rustc.display()))?;
    if !out.status.success() {
        return Err(format!(
            "{} -vV failed: {}",
            rustc.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let release = text
        .lines()
        .find_map(|l| l.strip_prefix("release: "))
        .map(str::trim)
        .ok_or("rustc -vV printed no release line")?
        .to_string();
    Ok(Toolchain {
        name,
        cargo,
        rustc,
        release,
    })
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
    if ok {
        Verdict::Pass
    } else {
        Verdict::Fail
    }
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

/// (d) A non-blocking probe sees a lock another process holds, for both lock kinds.
fn check_lock_probe(fs: &dyn Fs, dir: &Path) -> Check {
    let attempt = || -> io::Result<Check> {
        let mut measured = Measures::new();
        let mut ok = true;
        for kind in [LockKind::Flock, LockKind::Fcntl] {
            let path = dir.join(format!("d-{}.lock", kind.as_arg()));
            fs.write_file(&path, b"")?;
            let holder: LockHolder = fs.hold_lock(kind, &path)?;
            let held = fs.probe_lock(&path)? == LockState::Held;
            holder.release();
            let free = fs.probe_lock(&path)? == LockState::Free;
            measured.insert(
                format!("{}_reported_held", kind.as_arg()),
                Measured::Bool(held),
            );
            measured.insert(
                format!("{}_free_after_release", kind.as_arg()),
                Measured::Bool(free),
            );
            ok &= held && free;
        }
        Ok(Check {
            id: CheckId::D,
            verdict: verdict(ok),
            measured,
        })
    };
    attempt().unwrap_or_else(|e| failed(CheckId::D, e))
}

/// (e) Owner authentication: availability only, never a dialog.
fn check_owner_auth(approve: &dyn Approve) -> Check {
    match approve.owner_auth_available() {
        Availability::Available => Check {
            id: CheckId::E,
            verdict: Verdict::Pass,
            measured: Measures::from([("available".to_string(), Measured::Bool(true))]),
        },
        Availability::Unavailable(reason) => Check {
            id: CheckId::E,
            verdict: Verdict::Unavailable,
            measured: Measures::from([("reason".to_string(), Measured::Text(reason))]),
        },
    }
}

pub fn run(
    fs: &dyn Fs,
    approve: &dyn Approve,
    env: &DoctorEnv,
) -> Result<DoctorReport, DoctorError> {
    let tmp = fs
        .temp_dir(
            &env.tmp_parent,
            &format!("hive-doctor-{}-", std::process::id()),
        )
        .map_err(DoctorError::TempDir)?;
    let dir = tmp.path();
    let steps: [&dyn Fn() -> Check; 5] = [
        &|| check_clone_sharing(fs, dir),
        &|| check_compression(fs, dir),
        &|| check_sharing_flags(fs, dir),
        &|| check_lock_probe(fs, dir),
        &|| check_owner_auth(approve),
    ];
    let mut checks = Vec::new();
    for step in steps {
        checks.push(step());
    }
    Ok(DoctorReport {
        macos: env.macos.clone(),
        below_floor: env
            .macos
            .as_deref()
            .and_then(major_version)
            .is_some_and(|m| m < MACOS_FLOOR),
        toolchain_release: env.toolchain.as_ref().ok().map(|t| t.release.clone()),
        checks,
        stale_removed: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{Availability, Sharing, TempDir};
    use std::collections::HashMap;
    use std::os::unix::fs::MetadataExt;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::SystemTime;

    #[derive(Clone)]
    struct Knobs {
        clone_private: u64,
        shares_all: bool,
        rewrite_keeps_id: bool,
        single: CompressOutcome,
        linked: CompressOutcome,
        compress_touches_mtime: bool,
        probe_sees_locks: bool,
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
            }
        }
    }

    /// Real files in a scratch dir; sharing, compression and locks are scripted by `Knobs`.
    struct FakeFs {
        knobs: Knobs,
        ids: Mutex<HashMap<PathBuf, u64>>,
        next: AtomicU64,
        held: Arc<AtomicBool>,
    }

    impl FakeFs {
        fn new(knobs: Knobs) -> FakeFs {
            FakeFs {
                knobs,
                ids: Mutex::new(HashMap::new()),
                next: AtomicU64::new(1),
                held: Arc::new(AtomicBool::new(false)),
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
            if self.knobs.compress_touches_mtime {
                let _ = self.set_mtime(path, SystemTime::now());
            }
            match std::fs::metadata(path) {
                Ok(m) if m.nlink() > 1 => self.knobs.linked.clone(),
                _ => self.knobs.single.clone(),
            }
        }
        fn hold_lock(&self, _kind: LockKind, _path: &Path) -> io::Result<LockHolder> {
            self.held.store(true, Ordering::SeqCst);
            let held = Arc::clone(&self.held);
            Ok(LockHolder::new(move || held.store(false, Ordering::SeqCst)))
        }
        fn probe_lock(&self, _path: &Path) -> io::Result<LockState> {
            Ok(
                if self.knobs.probe_sees_locks && self.held.load(Ordering::SeqCst) {
                    LockState::Held
                } else {
                    LockState::Free
                },
            )
        }
    }

    struct FakeApprove(Availability);

    impl Approve for FakeApprove {
        fn owner_auth_available(&self) -> Availability {
            self.0.clone()
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
        }
    }

    /// Capabilities from checks a–e only: check f needs a real toolchain (Task 11).
    fn caps_a_to_e(report: &DoctorReport) -> Capabilities {
        let mut only = report.clone();
        only.checks.retain(|c| c.id != CheckId::F);
        capabilities_for(&only)
    }

    #[test]
    fn a_healthy_machine_passes_checks_a_to_e_and_disables_nothing() {
        let parent = scratch("healthy");
        let report = run(
            &FakeFs::new(Knobs::default()),
            &FakeApprove(Availability::Available),
            &env(&parent, "27.0.1"),
        )
        .unwrap();
        for c in report.checks.iter().filter(|c| c.id != CheckId::F) {
            assert_eq!(c.verdict, Verdict::Pass, "{:?}: {:?}", c.id, c.measured);
            assert!(!c.measured.is_empty(), "{:?} has no measured value", c.id);
        }
        assert!(caps_a_to_e(&report).disabled.is_empty());
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn each_failing_check_disables_exactly_its_capabilities() {
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
        ];
        for (name, knobs, failing, mut want) in cases {
            let parent = scratch("table");
            let report = run(
                &FakeFs::new(knobs),
                &FakeApprove(Availability::Available),
                &env(&parent, "27.0.1"),
            )
            .unwrap();
            for c in report.checks.iter().filter(|c| c.id != CheckId::F) {
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
                caps_a_to_e(&report)
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
    fn unavailable_owner_authentication_disables_approval() {
        let parent = scratch("noauth");
        let report = run(
            &FakeFs::new(Knobs::default()),
            &FakeApprove(Availability::Unavailable("no passcode".into())),
            &env(&parent, "27.0.1"),
        )
        .unwrap();
        let e = report.checks.iter().find(|c| c.id == CheckId::E).unwrap();
        assert_eq!(e.verdict, Verdict::Unavailable);
        assert_eq!(e.measured["reason"], Measured::Text("no passcode".into()));
        assert_eq!(
            caps_a_to_e(&report).disabled.get(&Capability::Approval),
            Some(&BTreeSet::from([CheckId::E]))
        );
        std::fs::remove_dir_all(parent).unwrap();
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
        let parent = scratch("floor");
        let report = run(
            &FakeFs::new(Knobs::default()),
            &FakeApprove(Availability::Available),
            &env(&parent, "26.4"),
        )
        .unwrap();
        assert!(report.below_floor);
        for id in [CheckId::A, CheckId::B, CheckId::C, CheckId::D, CheckId::E] {
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
        let current = run(
            &FakeFs::new(Knobs::default()),
            &FakeApprove(Availability::Available),
            &env(&parent, "27.0.1"),
        )
        .unwrap();
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
        };
        let e = require(&off, Capability::FingerprintReading, Some("1.97.1"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("(f)"), "{e}");
        assert_eq!(require(&off, Capability::Approval, Some("1.97.1")), Ok(()));
    }

    #[test]
    fn the_active_toolchain_release_comes_from_rustc() {
        let t = resolve_toolchain(&std::env::temp_dir()).unwrap();
        assert!(t.release.starts_with(|c: char| c.is_ascii_digit()), "{t:?}");
    }
}
