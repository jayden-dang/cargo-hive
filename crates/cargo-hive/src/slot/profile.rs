//! Admits only the actual command and graph covered by the pool's reviewed profile.

use super::{CheckoutProof, Refusal, canonical_worktree_path, check_cancelled, exists, git_scope};
use cargo_hive_cargo::{metadata, toolchain};
use cargo_hive_platform::{
    flock::open_owner_state_file,
    proc::{self, Capture, Spec},
    signals::cancelled,
    slot::sha256,
};
use cargo_hive_state::slots::{
    self, CargoLayout, EligibleProducer, EnvironmentExpectation, GraphExpectation, InputIdentity,
    Invocation, InvocationEvidence, Locator, OutputRoots, PoolProfile, Producer, ProfileBinding,
    RootExpectation, VerifiedRoot,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    io::Read,
    path::{Path, PathBuf},
};

const INPUT_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct InvocationInput {
    pub profile: ProfileBinding,
    pub invocation: String,
    pub argv: Vec<String>,
    pub cwd: PathBuf,
}

pub(super) struct CheckedInvocation {
    pub evidence: InvocationEvidence,
    pub cargo: PathBuf,
    pub cwd: PathBuf,
    pub environment: BTreeMap<std::ffi::OsString, std::ffi::OsString>,
}

fn refused(detail: &'static str) -> Refusal {
    Refusal::ReturnUncertain(detail)
}

pub(super) fn eligible_producer(
    producer: &Producer,
    generation: u64,
) -> Result<EligibleProducer, Refusal> {
    producer.eligible(generation).ok_or_else(|| {
        refused("The retained output has no eligible producer evidence for this generation.")
    })
}

pub(super) fn mutable_checkout_paths(
    profile: &PoolProfile,
    invocation: &Invocation,
    checkout: &Path,
) -> BTreeSet<PathBuf> {
    let mut paths = BTreeSet::new();
    for input in profile
        .mutable_inputs
        .iter()
        .filter(|input| input.invocations.contains(&invocation.id))
    {
        match &input.locator {
            Locator::Checkout { path } => {
                paths.insert(checkout.join(path));
            }
            Locator::DirectoryMembers { path, members } => {
                let directory = checkout.join(path);
                paths.extend(members.iter().map(|member| directory.join(member)));
            }
            _ => (),
        }
    }
    paths
}

pub(super) fn completed_roots(
    roots: &OutputRoots<RootExpectation>,
) -> Result<OutputRoots<VerifiedRoot>, Refusal> {
    let check = |expected: &RootExpectation| {
        let unproved =
            || refused("The completed Cargo output root identity could not be verified.");
        let path = match expected {
            RootExpectation::Absent { path } => path,
            RootExpectation::Present { root } => &root.path,
        };
        let root = VerifiedRoot {
            path: path.clone(),
            identity: super::owned_identity(path).ok_or_else(unproved)?,
        };
        if !expected.accepts(&root) {
            return Err(unproved());
        }
        Ok(root)
    };
    Ok(OutputRoots {
        target: check(&roots.target)?,
        build: check(&roots.build)?,
    })
}

pub(super) fn verify_output_layout(invocation: &Invocation, build: &Path) -> Result<(), Refusal> {
    use cargo_hive_core::target::{self, Layout};
    let layout = match invocation.layout {
        CargoLayout::Flat => Layout::Fingerprint,
        CargoLayout::UnitDir => Layout::NewBuildDir,
    };
    let fs = cargo_hive_platform::host_fs();
    if read_input(&build.join("CACHEDIR.TAG"))?.is_none()
        || !target::identify_bounded(fs.as_ref(), build, 4096).is_some_and(|profiles| {
            profiles.is_ok_and(|profiles| {
                !profiles.is_empty() && profiles.iter().all(|(_, observed)| *observed == layout)
            })
        })
    {
        return Err(refused(
            "The produced Cargo fingerprint layout differs from the reviewed layout.",
        ));
    }
    Ok(())
}

pub(super) fn reviewed_shape<'a>(
    profile: &'a PoolProfile,
    input: &InvocationInput,
) -> Result<&'a Invocation, Refusal> {
    if input.profile.id != profile.id || input.profile.revision != profile.revision {
        return Err(refused(
            "The requested producer profile revision is not current.",
        ));
    }
    profile
        .invocations
        .iter()
        .find(|invocation| {
            invocation.id == input.invocation
                && invocation.argv == input.argv
                && invocation.cwd == input.cwd
                && supported_args(&invocation.argv)
        })
        .ok_or_else(|| {
            refused("The producer command or working directory is outside the reviewed invocation.")
        })
}

fn supported_args(args: &[String]) -> bool {
    let Some((command, options)) = args.split_first() else {
        return false;
    };
    if !matches!(command.as_str(), "check" | "build" | "test" | "clippy")
        || !options.iter().any(|arg| arg == "--locked")
        || (command == "test" && !options.iter().any(|arg| arg == "--no-run"))
    {
        return false;
    }

    let mut options = options.iter();
    while let Some(arg) = options.next() {
        match arg.as_str() {
            "--locked"
            | "--offline"
            | "--frozen"
            | "--no-run"
            | "--all-targets"
            | "--all-features"
            | "--no-default-features"
            | "--workspace"
            | "--release"
            | "--lib"
            | "--tests"
            | "--bins"
            | "--examples"
            | "--benches"
            | "--quiet" => (),
            "--package" | "-p" | "--exclude" | "--features" | "-F" | "--profile" | "--target"
            | "--bin" | "--example" | "--test" | "--bench" => {
                if !options.next().is_some_and(|value| {
                    !value.starts_with('-')
                        && !value.is_empty()
                        && value
                            .bytes()
                            .all(|ch| ch.is_ascii_alphanumeric() || b" _.-,/+:".contains(&ch))
                }) {
                    return false;
                }
            }
            "--" if command == "clippy" => {
                return options
                    .all(|flag| matches!(flag.as_str(), "-D" | "warnings" | "-Dwarnings"));
            }
            _ => return false,
        }
    }
    true
}

pub(super) fn check_invocation(
    profile: &PoolProfile,
    input: &InvocationInput,
    checkout: &CheckoutProof,
    roots: &OutputRoots<RootExpectation>,
) -> Result<CheckedInvocation, Refusal> {
    check_cancelled()?;
    let invocation = reviewed_shape(profile, input)?;
    let cwd = if invocation.cwd == Path::new(".") {
        checkout.path.clone()
    } else {
        checkout.path.join(&invocation.cwd)
    };
    slots::owner_directory(&cwd)
        .map_err(|_| refused("Could not verify the reviewed working directory."))?;

    let environment: BTreeMap<_, _> = std::env::vars_os().collect();
    let (environment_digest, environment) = check_environment(profile, invocation, &environment)?;
    let cargo_home = environment
        .get(std::ffi::OsStr::new("CARGO_HOME"))
        .map(PathBuf::from)
        .filter(|_| {
            invocation.environment.iter().any(|variable| {
                variable.name == "CARGO_HOME"
                    && matches!(variable.expectation, EnvironmentExpectation::Present { .. })
            })
        })
        .ok_or_else(|| refused("The reviewed invocation needs an explicit Cargo home."))?;
    slots::owner_directory(&cargo_home)
        .map_err(|_| refused("Could not verify the reviewed Cargo home."))?;

    let inputs = fixed_inputs(profile, checkout)?;
    verify_cargo_configuration(&inputs, checkout, &cwd, &cargo_home)?;
    let toolchain = toolchain::resolve_toolchain(&cwd)
        .map_err(|_| refused("Could not establish the reviewed Cargo and rustc identity."))?;
    let cargo = verify_tools(invocation, &cwd, &toolchain, &environment)?;
    let discovered = discover(&cargo, &cwd, &environment)?;
    let matches_root = |actual: &Path, relative: &Path, expected: &RootExpectation| {
        let checked = checkout.path.join(relative);
        let saved = match expected {
            RootExpectation::Absent { path } => path,
            RootExpectation::Present { root } => &root.path,
        };
        actual.as_os_str() == checked.as_os_str()
            && saved.as_os_str() == checked.as_os_str()
            && canonical_worktree_path(&checked)
                .is_ok_and(|canonical| canonical.as_os_str() == checked.as_os_str())
    };
    if discovered.resolved.workspace_root != checkout.path
        || !matches_root(
            &discovered.resolved.target_directory,
            &invocation.target_root,
            &roots.target,
        )
        || !matches_root(
            &discovered.resolved.build_directory,
            &invocation.build_root,
            &roots.build,
        )
    {
        return Err(refused(
            "The actual workspace or Cargo output roots differ from the reviewed slot paths.",
        ));
    }
    check_graph_members(&inputs, &discovered, checkout, profile, invocation)?;
    let mutable_facts = mutable_facts(profile, invocation, checkout)?;
    let graph_digest = digest(
        &serde_json::to_vec(&(&inputs, mutable_facts, &cargo, &toolchain.rustc))
            .map_err(|_| refused("Could not bind the producer graph evidence."))?,
    )?;

    Ok(CheckedInvocation {
        evidence: InvocationEvidence {
            invocation: invocation.id.clone(),
            graph_digest,
            environment_digest,
        },
        cargo,
        cwd,
        environment,
    })
}

fn check_environment(
    profile: &PoolProfile,
    invocation: &Invocation,
    environment: &BTreeMap<std::ffi::OsString, std::ffi::OsString>,
) -> Result<(String, BTreeMap<std::ffi::OsString, std::ffi::OsString>), Refusal> {
    let denied = || refused("The effective environment differs from the reviewed invocation.");
    for name in environment.keys() {
        let name = name.to_str().ok_or_else(denied)?;
        if matches!(
            name,
            "RUSTC" | "RUSTDOC" | "RUSTC_WRAPPER" | "RUSTC_WORKSPACE_WRAPPER"
        ) || matches!(
            name,
            "BASH_ENV" | "ENV" | "CDPATH" | "BASHOPTS" | "SHELLOPTS"
        ) || name.starts_with("BASH_FUNC_")
            || name.starts_with("DYLD_")
            || name.starts_with("LD_")
            || metadata::TARGET_VARS.contains(&name)
        {
            return Err(denied());
        }
        if (name.starts_with("CARGO_") || name.starts_with("RUST") || name.starts_with("SQLX_"))
            && !matches!(
                name,
                "RUSTUP_HOME" | "RUSTUP_TOOLCHAIN" | "RUSTUP_AUTO_INSTALL"
            )
            && !invocation
                .environment
                .iter()
                .any(|variable| variable.name == name)
        {
            return Err(denied());
        }
    }
    let mut facts = BTreeMap::new();
    for variable in &invocation.environment {
        let value = environment
            .get(std::ffi::OsStr::new(&variable.name))
            .map(|value| value.to_str().ok_or_else(denied))
            .transpose()?;
        let matches = match &variable.expectation {
            EnvironmentExpectation::Present { value: expected } => value == Some(expected.as_str()),
            EnvironmentExpectation::Absent => value.is_none(),
            EnvironmentExpectation::Mutable { input } => {
                profile.mutable_inputs.iter().any(|item| {
                    &item.id == input
                        && (value.is_some()
                            || item.presence == slots::MutablePresence::PresentOrAbsent)
                })
            }
        };
        if !matches {
            return Err(denied());
        }
        facts.insert(variable.name.as_str(), value);
    }
    for name in [
        "PATH",
        "HOME",
        "RUSTUP_HOME",
        "RUSTUP_TOOLCHAIN",
        "RUSTUP_AUTO_INSTALL",
    ] {
        let value = environment
            .get(std::ffi::OsStr::new(name))
            .map(|value| value.to_str().ok_or_else(denied))
            .transpose()?;
        facts.insert(name, value);
    }
    let digest = digest(&serde_json::to_vec(&facts).map_err(|_| denied())?)?;
    let effective = facts
        .into_iter()
        .filter_map(|(name, value)| value.map(|value| (name.into(), value.into())))
        .collect();
    Ok((digest, effective))
}

fn fixed_inputs(
    profile: &PoolProfile,
    checkout: &CheckoutProof,
) -> Result<BTreeMap<PathBuf, Option<String>>, Refusal> {
    let unproved = || {
        refused(
            "A fixed Cargo manifest, lockfile or configuration input differs from the reviewed graph.",
        )
    };
    profile
        .graph_inputs
        .iter()
        .map(|item| {
            let path = match &item.locator {
                Locator::Checkout { path } => checkout.path.join(path),
                Locator::External { path } if !path.starts_with(&checkout.path) => path.clone(),
                _ => return Err(unproved()),
            };
            let fact = read_input(&path)?;
            match (&item.expectation, fact) {
                (GraphExpectation::Absent, None) => Ok((path, None)),
                (GraphExpectation::Present { identity }, Some(bytes)) => {
                    let hash = digest(&bytes)?;
                    let matches = match identity {
                        InputIdentity::ReviewedBytes { sha256 } => hash == *sha256,
                        InputIdentity::GitObject { oid } => {
                            let relative =
                                path.strip_prefix(&checkout.path).map_err(|_| unproved())?;
                            let object = tracked_object(checkout, relative)?;
                            object == *oid && blob(checkout, oid)? == bytes
                        }
                    };
                    if !matches {
                        return Err(unproved());
                    }
                    Ok((path, Some(hash)))
                }
                _ => Err(unproved()),
            }
        })
        .collect()
}

fn read_input(path: &Path) -> Result<Option<Vec<u8>>, Refusal> {
    let unproved =
        || refused("Could not establish a canonical regular reviewed input or prove its absence.");
    let canonical = canonical_worktree_path(path).map_err(|_| unproved())?;
    if canonical.as_os_str() != path.as_os_str() {
        return Err(unproved());
    }
    let file = match open_owner_state_file(path) {
        Ok(file) => file,
        Err(err)
            if err.kind() == io::ErrorKind::NotFound
                && !exists(path).map_err(|_| unproved())? =>
        {
            return Ok(None);
        }
        Err(_) => return Err(unproved()),
    };
    let before = file.metadata().map_err(|_| unproved())?;
    let mut bytes = Vec::new();
    (&file)
        .take(INPUT_LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unproved())?;
    let after = fs::symlink_metadata(path).map_err(|_| unproved())?;
    if bytes.len() > INPUT_LIMIT
        || super::directory_stamp(&before) != super::directory_stamp(&after)
        || before.len() != after.len()
        || !after.is_file()
    {
        return Err(unproved());
    }
    Ok(Some(bytes))
}

fn tracked_object(checkout: &CheckoutProof, path: &Path) -> Result<String, Refusal> {
    let unproved = || {
        refused("A reviewed consumer or Cargo graph input lacks its tracked regular Git anchor.")
    };
    let relative = path.to_str().ok_or_else(unproved)?;
    let raw = git_scope(
        &checkout.path,
        &["ls-tree", "-z", &checkout.head, "--", relative],
    )?;
    let row = raw
        .strip_suffix(b"\0")
        .and_then(|row| std::str::from_utf8(row).ok())
        .ok_or_else(unproved)?;
    let (attributes, name) = row.split_once('\t').ok_or_else(unproved)?;
    let mut fields = attributes.split(' ');
    match (fields.next(), fields.next(), fields.next(), fields.next()) {
        (Some("100644" | "100755"), Some("blob"), Some(oid), None)
            if name == relative && slots::object_id(oid) =>
        {
            Ok(oid.into())
        }
        _ => Err(unproved()),
    }
}

fn blob(checkout: &CheckoutProof, oid: &str) -> Result<Vec<u8>, Refusal> {
    use cargo_hive_cargo::git::Git;
    Git::at(&checkout.path)
        .slot_read_only(&["cat-file", "blob", oid], INPUT_LIMIT)
        .map_err(|err| super::git_error(&checkout.path, err))
}

fn verify_cargo_configuration(
    inputs: &BTreeMap<PathBuf, Option<String>>,
    checkout: &CheckoutProof,
    cwd: &Path,
    cargo_home: &Path,
) -> Result<(), Refusal> {
    let omitted = || {
        refused(
            "Cargo is probing a manifest or configuration path outside the reviewed graph inventory.",
        )
    };
    let check = |path: PathBuf| -> Result<(), Refusal> {
        if !inputs.contains_key(&path) && read_input(&path)?.is_some() {
            return Err(omitted());
        }
        Ok(())
    };
    for ancestor in cwd.ancestors() {
        for name in [
            ".cargo/config",
            ".cargo/config.toml",
            "rust-toolchain",
            "rust-toolchain.toml",
        ] {
            check(ancestor.join(name))?;
        }
        if ancestor != checkout.path {
            check(ancestor.join("Cargo.toml"))?;
        }
    }
    for name in ["config", "config.toml"] {
        check(cargo_home.join(name))?;
    }
    Ok(())
}

fn verify_tools(
    invocation: &Invocation,
    cwd: &Path,
    toolchain: &toolchain::Toolchain,
    environment: &BTreeMap<std::ffi::OsString, std::ffi::OsString>,
) -> Result<PathBuf, Refusal> {
    let incorrect = || {
        refused("Actual Cargo or rustc identity and layout differs from the reviewed invocation.")
    };
    let cargo = toolchain.cargo.canonicalize().map_err(|_| incorrect())?;
    let rustc = toolchain.rustc.canonicalize().map_err(|_| incorrect())?;
    let layout = match toolchain.release.as_str() {
        "1.97.1" => CargoLayout::Flat,
        "1.100.0" => CargoLayout::UnitDir,
        _ => return Err(incorrect()),
    };
    if !toolchain.cargo.is_absolute()
        || !toolchain.rustc.is_absolute()
        || cargo.parent() != rustc.parent()
        || layout != invocation.layout
        || !version_matches(
            &invocation.rustc,
            toolchain.version_text.lines().next().unwrap_or(""),
        )
    {
        return Err(incorrect());
    }
    let output = cargo_output(&cargo, cwd, &["-V"], 1024, environment)?;
    let version = std::str::from_utf8(&output)
        .map_err(|_| incorrect())?
        .trim_end();
    if !version_matches(&invocation.cargo, version)
        || !version_matches(&format!("cargo {}", toolchain.release), version)
    {
        return Err(incorrect());
    }
    Ok(cargo)
}

fn version_matches(expected: &str, actual: &str) -> bool {
    expected == actual
        || actual
            .strip_prefix(expected)
            .is_some_and(|tail| tail.starts_with(" ("))
}

fn cargo_output(
    cargo: &Path,
    cwd: &Path,
    args: &[&str],
    cap: usize,
    environment: &BTreeMap<std::ffi::OsString, std::ffi::OsString>,
) -> Result<Vec<u8>, Refusal> {
    let mut spec = Spec::new(cargo);
    spec.cwd = Some(cwd.to_path_buf());
    spec.args.extend(args.iter().map(|arg| (*arg).into()));
    spec.env_clear = true;
    spec.env_set.extend(
        environment
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    let ran = proc::run_bounded_complete(
        &spec,
        metadata::METADATA_DEADLINE,
        Capture {
            stdout_cap: cap + 1,
            stderr_cap: 1024,
        },
        cancelled,
    )
    .map_err(|err| match err {
        proc::ChildOutputError::Cancelled => cargo_hive_state::StateError::Cancelled.into(),
        _ => refused("Could not establish complete Cargo command discovery evidence."),
    })?;
    if !ran.status.success() || ran.stdout.len() > cap {
        return Err(refused(
            "Could not establish complete Cargo command discovery evidence.",
        ));
    }
    Ok(ran.stdout)
}

#[derive(Deserialize)]
struct Package {
    manifest_path: PathBuf,
    name: String,
    source: Option<String>,
    targets: Vec<PackageTarget>,
}

#[derive(Deserialize)]
struct PackageTarget {
    kind: Vec<String>,
}

struct Discovered {
    resolved: metadata::Resolved,
    packages: Vec<Package>,
}

fn deserialize_discovery<T: DeserializeOwned>(value: &serde_json::Value) -> Result<T, Refusal> {
    serde_json::from_value(value.clone())
        .map_err(|_| refused("Cargo reported an incomplete reviewed graph."))
}

fn discover(
    cargo: &Path,
    cwd: &Path,
    environment: &BTreeMap<std::ffi::OsString, std::ffi::OsString>,
) -> Result<Discovered, Refusal> {
    let output = cargo_output(
        cargo,
        cwd,
        &[
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--offline",
            "--all-features",
        ],
        INPUT_LIMIT,
        environment,
    )?;
    let metadata: serde_json::Value = serde_json::from_slice(&output)
        .map_err(|_| refused("Cargo reported an incomplete reviewed graph."))?;
    Ok(Discovered {
        resolved: metadata::Resolved {
            workspace_root: deserialize_discovery(&metadata["workspace_root"])?,
            target_directory: deserialize_discovery(&metadata["target_directory"])?,
            build_directory: deserialize_discovery(
                metadata
                    .get("build_directory")
                    .unwrap_or(&metadata["target_directory"]),
            )?,
        },
        packages: deserialize_discovery(&metadata["packages"])?,
    })
}

fn check_graph_members(
    inputs: &BTreeMap<PathBuf, Option<String>>,
    graph: &Discovered,
    checkout: &CheckoutProof,
    profile: &PoolProfile,
    invocation: &Invocation,
) -> Result<(), Refusal> {
    let unproved = || {
        refused(
            "The actual Cargo graph has an unreviewed manifest, local build script or consumer anchor.",
        )
    };
    let fixed = |path: &Path| inputs.get(path).is_some_and(Option::is_some);
    if !fixed(&checkout.path.join("Cargo.toml"))
        || !fixed(&checkout.path.join("Cargo.lock"))
        || graph.packages.iter().any(|package| {
            package.source.is_none()
                && (!fixed(&package.manifest_path)
                    || package
                        .targets
                        .iter()
                        .any(|target| target.kind.iter().any(|kind| kind == "custom-build")))
        })
    {
        return Err(unproved());
    }
    for proof in profile
        .proofs
        .iter()
        .filter(|proof| proof.invocation == invocation.id)
    {
        let independent = graph
            .packages
            .iter()
            .any(|package| package.source.is_none() && package.name == proof.unaffected_unit);
        let affected = profile.consumers.iter().any(|consumer| {
            consumer.invocation == invocation.id && consumer.package == proof.unaffected_unit
        });
        if !independent || affected {
            return Err(unproved());
        }
    }
    for consumer in profile
        .consumers
        .iter()
        .filter(|item| item.invocation == invocation.id)
    {
        let anchor = checkout.path.join(&consumer.anchor);
        let covered = graph.packages.iter().any(|package| {
            package.source.is_none()
                && package.name == consumer.package
                && package.targets.iter().any(|target| {
                    target.kind.iter().any(|kind| {
                        kind == &consumer.role || (consumer.role == "library" && kind == "lib")
                    })
                })
                && package
                    .manifest_path
                    .parent()
                    .is_some_and(|dir| anchor.starts_with(dir))
        });
        let object = tracked_object(checkout, &consumer.anchor)?;
        if !covered || (object != consumer.prior_object && object != consumer.requested_object) {
            return Err(unproved());
        }
    }
    Ok(())
}

fn mutable_facts(
    profile: &PoolProfile,
    invocation: &Invocation,
    checkout: &CheckoutProof,
) -> Result<BTreeMap<PathBuf, Option<String>>, Refusal> {
    let mut facts = BTreeMap::new();
    for input in profile
        .mutable_inputs
        .iter()
        .filter(|item| item.invocations.contains(&invocation.id))
    {
        let paths = match &input.locator {
            Locator::Checkout { path } => vec![checkout.path.join(path)],
            Locator::External { path } => vec![path.clone()],
            Locator::DirectoryMembers { path, members } => {
                let dir = checkout.path.join(path);
                let reviewed = members
                    .iter()
                    .map(|member| dir.join(member))
                    .collect::<BTreeSet<_>>();
                check_members(&dir, &reviewed, &mut 0)?;
                reviewed.into_iter().collect()
            }
            Locator::Environment { .. } => continue,
        };
        for path in paths {
            let fact = read_input(&path)?.map(|bytes| digest(&bytes)).transpose()?;
            if fact.is_none() && input.presence == slots::MutablePresence::Present {
                return Err(refused("A required mutable input is missing."));
            }
            if facts.insert(path, fact).is_some() {
                return Err(refused("The reviewed mutable input paths overlap."));
            }
        }
    }
    Ok(facts)
}

fn check_members(
    dir: &Path,
    reviewed: &BTreeSet<PathBuf>,
    visited: &mut usize,
) -> Result<(), Refusal> {
    let unproved =
        || refused("A reviewed input directory contains an undeclared or unverified member.");
    if !exists(dir).map_err(|_| unproved())? {
        return Ok(());
    }
    slots::owner_directory(dir).map_err(|_| unproved())?;
    for entry in fs::read_dir(dir).map_err(|_| unproved())? {
        *visited += 1;
        if *visited > 4096 {
            return Err(unproved());
        }
        let entry = entry.map_err(|_| unproved())?;
        let path = entry.path();
        let kind = entry.file_type().map_err(|_| unproved())?;
        if kind.is_dir() && reviewed.iter().any(|member| member.starts_with(&path)) {
            check_members(&path, reviewed, visited)?;
        } else if !kind.is_file() || !reviewed.contains(&path) {
            return Err(unproved());
        }
    }
    Ok(())
}

fn digest(bytes: &[u8]) -> Result<String, Refusal> {
    let bytes =
        sha256(bytes).map_err(|_| refused("Could not bind the producer evidence digest."))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
