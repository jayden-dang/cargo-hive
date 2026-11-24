//! Invocation-private gates for the two reviewed native Nextest recipes.

use cargo_hive_platform::{
    flock::{self, LockFileIdentity},
    proc::{self, Capture, Spec},
    slot::{recipe_parent_matches, sha256_reader},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs::{File, OpenOptions},
    io::{self, Read as _, Seek as _, Write as _},
    os::unix::fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _},
    path::{Path, PathBuf},
    time::Duration,
};

pub(super) mod artifact;
mod metadata;

const CARGO_GATE: &str = "hive-recipe-cargo";
const LIST_GATE: &str = "hive-recipe-list";
const COMPLETE_GATE: &str = "hive-recipe-complete";
const CARGO_LAUNCHER: &str = "hive-stock-cargo";
const SLOT_FD: i32 = 9;
const PLAN_LIMIT: usize = 1024 * 1024;

#[derive(Clone, Copy, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Selection {
    Workspace,
    Gateways,
}

#[derive(Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PinnedFile {
    pub path: PathBuf,
    pub sha256: String,
}

#[derive(Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    schema: u32,
    selection: Selection,
    workspace: PathBuf,
    target: PathBuf,
    cargo_home: PathBuf,
    cargo: PinnedFile,
    rustc: PinnedFile,
    nextest: PinnedFile,
    lock_path: PathBuf,
    lock_dev: u64,
    lock_ino: u64,
    lock_offset: i64,
    units: Vec<artifact::ReviewedTestBinary>,
    #[serde(default)]
    retained: Vec<artifact::ListingArtifact>,
    metadata: Option<metadata::MetadataGate>,
    #[serde(default)]
    sqlx_requests: BTreeMap<PathBuf, u16>,
}

#[derive(Deserialize, Eq, Ord, PartialEq, PartialOrd)]
#[serde(deny_unknown_fields)]
pub(super) struct UnitReview {
    pub manifest: PathBuf,
    pub target_name: String,
    pub features: Vec<String>,
}

pub(super) struct ReviewedLaunch {
    pub selection: Selection,
    pub workspace: PathBuf,
    pub target: PathBuf,
    pub cargo_home: PathBuf,
    pub cargo: PinnedFile,
    pub rustc: PinnedFile,
    pub nextest: PinnedFile,
    pub units: Vec<artifact::ReviewedTestBinary>,
}

pub(super) struct ActiveLock {
    pub path: PathBuf,
    pub identity: LockFileIdentity,
    pub offset: i64,
}

impl ReviewedLaunch {
    pub fn configure(
        &self,
        dir: &Path,
        lock: ActiveLock,
        create: bool,
        env: &mut BTreeMap<OsString, OsString>,
        retained: &[artifact::ListingArtifact],
    ) -> io::Result<PathBuf> {
        let plan = Plan {
            schema: 1,
            selection: self.selection,
            workspace: self.workspace.clone(),
            target: self.target.clone(),
            cargo_home: self.cargo_home.clone(),
            cargo: self.cargo.clone(),
            rustc: self.rustc.clone(),
            nextest: self.nextest.clone(),
            lock_path: lock.path,
            lock_dev: lock.identity.dev,
            lock_ino: lock.identity.ino,
            lock_offset: lock.offset,
            units: self.units.clone(),
            retained: retained.to_vec(),
            metadata: None,
            sqlx_requests: BTreeMap::new(),
        };
        plan.lock()?;
        let parent = dir.parent().ok_or_else(|| refused("missing gate parent"))?;
        if parent.canonicalize()? != parent
            || flock::checked_owner_directory(parent)?.mode() & 0o022 != 0
        {
            return Err(refused("unproved gate parent"));
        }
        let hive = std::env::current_exe()?;
        let hive_digest = material_digest(&hive)?;
        let launcher = dir.join(CARGO_LAUNCHER);
        if create {
            std::fs::DirBuilder::new().mode(0o700).create(dir)?;
            copy_material(&self.cargo.path, &launcher, &self.cargo.sha256)?;
            for alias in [CARGO_GATE, LIST_GATE] {
                copy_material(&hive, &dir.join(alias), &hive_digest)?;
            }
            write_json(&dir.join("plan.json"), &plan)?;
        } else if read_json::<Plan>(&dir.join("plan.json"), PLAN_LIMIT)? != plan {
            return Err(refused("private gate plan changed"));
        }
        if dir.canonicalize()? != dir || flock::checked_owner_directory(dir)?.mode() & 0o077 != 0 {
            return Err(refused("unproved private gate directory"));
        }
        verify_material(&launcher, &self.cargo.sha256)?;
        for alias in [CARGO_GATE, LIST_GATE] {
            verify_material(&dir.join(alias), &hive_digest)?;
        }
        env.insert("CARGO".into(), dir.join(CARGO_GATE).into());
        env.insert(
            "CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER".into(),
            dir.join(LIST_GATE).into(),
        );
        Ok(launcher)
    }
}

pub(super) fn verify_completion(dir: &Path) -> io::Result<Vec<artifact::ListingArtifact>> {
    let plan: Plan = read_json(&dir.join("plan.json"), PLAN_LIMIT)?;
    plan.lock()?;
    complete(dir, &plan, &[])
}

fn copy_material(src: &Path, dst: &Path, expected: &str) -> io::Result<()> {
    let mut input = flock::open_owner_state_file(src)?.take(256 * 1024 * 1024 + 1);
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(dst)?;
    io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    verify_material(dst, expected)
}

pub fn recipe_child(args: &[OsString]) -> Option<i32> {
    let name = Path::new(args.first()?).file_name()?.to_str()?;
    if !matches!(name, CARGO_GATE | LIST_GATE | COMPLETE_GATE) {
        return None;
    }
    Some(match child(name, &args[1..]) {
        Ok(()) => 0,
        Err(error) => {
            if let Ok(dir) = private_dir(name) {
                let _ = write_new(&dir.join("refused"), b"");
            }
            eprintln!("reviewed Nextest gate refused: {error}");
            78
        }
    })
}

fn refused(reason: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, reason)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path, limit: usize) -> io::Result<T> {
    let input = flock::open_private_file(path)?;
    let mut bytes = Vec::new();
    input.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(refused("private gate data exceeds its bound"));
    }
    serde_json::from_slice(&bytes).map_err(|_| refused("invalid private gate data"))
}

fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn write_json(path: &Path, data: &impl Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec(data).map_err(|_| refused("could not encode gate data"))?;
    write_new(path, &bytes)
}

fn verify_material(path: &Path, expected: &str) -> io::Result<()> {
    if material_digest(path)? != expected {
        return Err(refused("reviewed executable bytes changed"));
    }
    Ok(())
}

fn material_digest(path: &Path) -> io::Result<String> {
    if !path.is_absolute() || path.canonicalize()? != path {
        return Err(refused("noncanonical reviewed executable"));
    }
    let mut file = flock::open_owner_state_file(path)?;
    let before = file.metadata()?;
    if before.mode() & 0o111 == 0 || before.len() > 256 * 1024 * 1024 {
        return Err(refused("invalid reviewed executable"));
    }
    let hash = sha256_reader(&mut file, before.len(), || false)?;
    let after = std::fs::symlink_metadata(path)?;
    if super::directory_stamp(&before) != super::directory_stamp(&after) {
        return Err(refused("reviewed executable bytes changed"));
    }
    Ok(hash.iter().map(|byte| format!("{byte:02x}")).collect())
}

impl Plan {
    fn lock(&self) -> io::Result<LockFileIdentity> {
        if self.schema != 1
            || !self.workspace.is_absolute()
            || self.workspace.canonicalize()? != self.workspace
            || self.target != self.workspace.join("target")
            || self.lock_offset < (1 << 32)
            || self.sqlx_requests.len() > 512
            || self.sqlx_requests.values().any(|count| *count == 0)
            || self
                .sqlx_requests
                .values()
                .map(|count| usize::from(*count))
                .sum::<usize>()
                > 1024
            || (!self.sqlx_requests.is_empty() && self.metadata.is_none())
        {
            return Err(refused("invalid private recipe scope"));
        }
        let named = flock::open_private_file(&self.lock_path)?;
        let lock_id = |file| -> io::Result<LockFileIdentity> {
            let metadata = flock::private_file_metadata(file)?;
            Ok(LockFileIdentity {
                dev: metadata.dev(),
                ino: metadata.ino(),
            })
        };
        let named_id = lock_id(&named)?;
        let mut inherited = File::from(flock::duplicate_inherited_fd(SLOT_FD)?);
        let inherited_id = lock_id(&inherited)?;
        if named_id.dev != self.lock_dev
            || named_id.ino != self.lock_ino
            || named_id != inherited_id
            || inherited.stream_position()? != self.lock_offset as u64
            || flock::try_exclusive(&named)?.is_some()
        {
            return Err(refused("expected inherited held slot description"));
        }
        Ok(named_id)
    }

    fn nextest_metadata_args() -> [&'static str; 7] {
        [
            "--color=auto",
            "metadata",
            "--format-version=1",
            "--all-features",
            "--filter-platform",
            "aarch64-apple-darwin",
            "--locked",
        ]
    }

    fn nextest_build_args(&self) -> [&'static str; 8] {
        let selection = match self.selection {
            Selection::Workspace => ["--workspace", "--all-features"],
            Selection::Gateways => ["--package", "gateways"],
        };
        [
            "--color=auto",
            "test",
            "--no-run",
            "--message-format",
            "json-render-diagnostics",
            selection[0],
            selection[1],
            "--locked",
        ]
    }
}

fn private_dir(name: &str) -> io::Result<PathBuf> {
    let executable = std::env::current_exe()?;
    let dir = executable
        .parent()
        .ok_or_else(|| refused("missing gate directory"))?;
    if executable.file_name() != Some(name.as_ref())
        || dir.canonicalize()? != dir
        || flock::checked_owner_directory(dir)?.mode() & 0o077 != 0
    {
        return Err(refused("unexpected physical gate executable"));
    }
    Ok(dir.to_path_buf())
}

fn child(name: &str, args: &[OsString]) -> io::Result<()> {
    let dir = private_dir(name)?;
    let plan: Plan = read_json(&dir.join("plan.json"), PLAN_LIMIT)?;
    let id = plan.lock()?;
    if name == COMPLETE_GATE {
        return complete(&dir, &plan, args).map(|_| ());
    }
    let cwd = std::env::current_dir()?;
    let env: BTreeMap<_, _> = std::env::vars_os().collect();
    let parent_matches = |tool: &PinnedFile| -> io::Result<bool> {
        if !recipe_parent_matches(&tool.path, SLOT_FD, id, plan.lock_offset)? {
            return Ok(false);
        }
        verify_material(&tool.path, &tool.sha256)?;
        Ok(true)
    };
    let value = |key: &str| env.get(OsStr::new(key)).map(OsString::as_os_str);
    if value("CARGO").map(Path::new) != Some(&dir.join(CARGO_GATE))
        || value("CARGO_NET_OFFLINE") != Some(OsStr::new("true"))
        || value("CARGO_HOME").map(Path::new) != Some(&plan.cargo_home)
        || value("RUSTC").map(Path::new) != Some(&plan.rustc.path)
        || value("CARGO_TARGET_DIR").is_some_and(|target| Path::new(target) != plan.target)
        || [
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
            "DYLD_INSERT_LIBRARIES",
            "LD_PRELOAD",
        ]
        .iter()
        .any(|key| value(key).is_some())
    {
        return Err(refused("unexpected recipe environment"));
    }
    if name == LIST_GATE {
        if !parent_matches(&plan.nextest)? {
            return Err(refused("unexpected listing parent"));
        }
        let (binary, argv) = args
            .split_first()
            .ok_or_else(|| refused("missing listing binary"))?;
        let binary = Path::new(binary);
        let artifacts: Vec<artifact::ListingArtifact> =
            read_json(&dir.join("artifacts.json"), PLAN_LIMIT)?;
        let mode = artifact::verify_listing(&artifacts, binary, &cwd, argv)?;
        let index = artifacts
            .iter()
            .position(|entry| entry.executable == binary)
            .ok_or_else(|| refused("listing binary outside capture"))?;
        let receipt = listing_receipt(index, mode);
        write_new(&dir.join(&receipt), b"")?;
        let ran = run(binary, argv, cwd, env, Duration::from_secs(60), PLAN_LIMIT)?;
        artifact::verify_listing(&artifacts, binary, &std::env::current_dir()?, argv)?;
        write_new(&dir.join(format!("{receipt}.complete")), b"")?;
        return forward(ran);
    }
    let launcher = dir.join(CARGO_LAUNCHER);
    verify_material(&plan.cargo.path, &plan.cargo.sha256)?;
    verify_material(&launcher, &plan.cargo.sha256)?;
    let top_metadata = args.iter().eq(Plan::nextest_metadata_args().iter());
    if top_metadata || args.iter().eq(plan.nextest_build_args().iter()) {
        if cwd != plan.workspace || !parent_matches(&plan.nextest)? {
            return Err(refused("unexpected Nextest Cargo parent or cwd"));
        }
        let claim = if top_metadata {
            "metadata.started"
        } else {
            "build.started"
        };
        write_new(&dir.join(claim), b"")?;
        let ran = run(
            &launcher,
            args,
            cwd,
            env,
            Duration::from_secs(3600),
            artifact::MAX_CARGO_OUTPUT_BYTES,
        )?;
        verify_material(&launcher, &plan.cargo.sha256)?;
        if top_metadata {
            let data: serde_json::Value = serde_json::from_slice(&ran.stdout)
                .map_err(|_| refused("malformed workspace metadata"))?;
            if data["workspace_root"].as_str().map(Path::new) != Some(&plan.workspace)
                || data["target_directory"].as_str().map(Path::new) != Some(&plan.target)
            {
                return Err(refused("metadata crossed recipe roots"));
            }
            write_new(&dir.join("metadata.complete"), b"")?;
        } else {
            let artifacts =
                artifact::capture(&ran.stdout, &plan.units, &[plan.target], &plan.retained)?;
            write_json(&dir.join("artifacts.json"), &artifacts)?;
        }
        return forward(ran);
    }
    let metadata = plan
        .metadata
        .as_ref()
        .ok_or_else(|| refused("unreviewed Cargo request"))?;
    if metadata.workspace_root != plan.workspace
        || metadata.cargo != plan.cargo.path
        || metadata.cargo_sha256 != plan.cargo.sha256
        || !parent_matches(&plan.rustc)?
    {
        return Err(refused("unexpected nested Cargo parent or policy"));
    }
    metadata.validate_request(args, &cwd, &env)?;
    let fds = metadata::jobserver_descriptors(
        value("CARGO_MAKEFLAGS").ok_or_else(|| refused("missing jobserver"))?,
    )?;
    for fd in [fds.0, fds.1] {
        if fd == SLOT_FD {
            return Err(refused("jobserver conflicts with slot guard"));
        }
        flock::duplicate_inherited_fd(fd)?;
    }
    let (index, (_, expected)) = plan
        .sqlx_requests
        .iter()
        .enumerate()
        .find(|(_, (package, _))| *package == &cwd)
        .ok_or_else(|| refused("unreviewed SQLx consumer directory"))?;
    let receipt = claim_sqlx(&dir, index, *expected)?;
    let ran = run(
        &plan.cargo.path,
        args,
        cwd,
        env,
        Duration::from_secs(10),
        metadata::OUTPUT_LIMIT,
    )?;
    metadata.validate_response(ran.status, &ran.stdout, &ran.stderr_tail)?;
    verify_material(&plan.cargo.path, &plan.cargo.sha256)?;
    write_new(&dir.join(format!("{receipt}.complete")), b"")?;
    forward(ran)
}

fn listing_receipt(index: usize, mode: artifact::ListingMode) -> String {
    match mode {
        artifact::ListingMode::Regular => format!("list-{index}-regular"),
        artifact::ListingMode::Ignored => format!("list-{index}-ignored"),
    }
}

fn claim_sqlx(dir: &Path, index: usize, expected: u16) -> io::Result<String> {
    for number in 0..expected {
        let name = format!("sqlx-{index}-{number}");
        match write_new(&dir.join(&name), b"") {
            Ok(()) => return Ok(name),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err),
        }
    }
    Err(refused("unexpected additional SQLx metadata request"))
}

fn require_receipt(dir: &Path, name: &str) -> io::Result<()> {
    if flock::open_private_file(&dir.join(name))?.metadata()?.len() == 0 {
        Ok(())
    } else {
        Err(refused("invalid private gate receipt"))
    }
}

fn complete(
    dir: &Path,
    plan: &Plan,
    args: &[OsString],
) -> io::Result<Vec<artifact::ListingArtifact>> {
    let no_refusal = std::fs::symlink_metadata(dir.join("refused"))
        .is_err_and(|error| error.kind() == io::ErrorKind::NotFound);
    if !args.is_empty() || !no_refusal {
        return Err(refused("one or more recipe gate requests failed"));
    }
    require_receipt(dir, "metadata.complete")?;
    let artifacts: Vec<artifact::ListingArtifact> =
        read_json(&dir.join("artifacts.json"), PLAN_LIMIT)?;
    let captured: std::collections::BTreeSet<_> =
        artifacts.iter().map(|item| &item.target).collect();
    if artifacts.is_empty()
        || artifacts.len() != plan.units.len()
        || captured != plan.units.iter().collect()
        || captured.len() != artifacts.len()
    {
        return Err(refused("compiled selection differs from reviewed units"));
    }
    for (index, item) in artifacts.iter().enumerate() {
        let cwd = item
            .target
            .manifest_path
            .parent()
            .ok_or_else(|| refused("missing package cwd"))?;
        let argv = ["--list", "--format", "terse"].map(OsString::from);
        artifact::verify_listing(&artifacts, &item.executable, cwd, &argv)?;
        for mode in [
            artifact::ListingMode::Regular,
            artifact::ListingMode::Ignored,
        ] {
            require_receipt(dir, &format!("{}.complete", listing_receipt(index, mode)))?;
        }
    }
    for (index, count) in plan.sqlx_requests.values().enumerate() {
        for number in 0..*count {
            require_receipt(dir, &format!("sqlx-{index}-{number}.complete"))?;
        }
    }
    Ok(artifacts)
}

fn run(
    program: &Path,
    args: &[OsString],
    cwd: PathBuf,
    env: BTreeMap<OsString, OsString>,
    deadline: Duration,
    limit: usize,
) -> io::Result<proc::Ran> {
    let mut command = Spec::new(program);
    command.args = args.to_vec();
    command.cwd = Some(cwd);
    command.env_clear = true;
    command.env_set = env.into_iter().collect();
    let ran = proc::run_bounded_complete_in_writer_group(
        &command,
        deadline,
        Capture {
            stdout_cap: limit + 1,
            stderr_cap: PLAN_LIMIT + 1,
        },
        || false,
    )
    .map_err(io::Error::other)?;
    if !ran.status.success() || ran.stdout.len() > limit || ran.stderr_tail.len() > PLAN_LIMIT {
        return Err(refused("failed or oversized reviewed child response"));
    }
    Ok(ran)
}

fn forward(ran: proc::Ran) -> io::Result<()> {
    io::stdout().write_all(&ran.stdout)?;
    io::stderr().write_all(&ran.stderr_tail)
}
