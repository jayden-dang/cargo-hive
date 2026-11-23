//! Admits only the actual command and graph covered by the pool's reviewed profile.

use super::admission::{NativeBuild, NativePackage, NativeTarget};
use super::{CheckoutProof, Refusal, canonical_worktree_path, check_cancelled, exists, git_scope};
use cargo_hive_cargo::metadata;
use cargo_hive_platform::{
    flock::open_owner_state_file,
    proc::{self, Capture, Spec},
    signals::cancelled,
    slot::sha256,
};
use cargo_hive_state::slots::{
    self, CargoLayout, Coverage, EligibleProducer, EnvironmentExpectation, GraphExpectation,
    InputIdentity, Invocation, InvocationEvidence, Locator, OutputRoots, PoolProfile, Producer,
    ProfileBinding, RootExpectation, VerifiedRoot,
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
    pub needs_guide: bool,
    pub environment: BTreeMap<std::ffi::OsString, std::ffi::OsString>,
}

pub(super) fn refused(detail: &'static str) -> Refusal {
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
    profile
        .mutable_inputs
        .iter()
        .filter(|input| input.invocations.contains(&invocation.id))
        .filter_map(|input| match &input.locator {
            Locator::Checkout { path } => Some(checkout.join(path)),
            _ => None,
        })
        .collect()
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
    let invocation = profile
        .invocations
        .iter()
        .find(|invocation| {
            invocation.id == input.invocation
                && invocation.argv == input.argv
                && invocation.cwd == input.cwd
        })
        .ok_or_else(|| {
            refused("The producer command or working directory is outside the reviewed invocation.")
        })?;
    if NativeBuild::read(profile, invocation, &native_target())?.is_some() {
        return Ok(invocation);
    }
    if !supported_args(&invocation.argv) {
        return Err(refused(
            "The producer command or working directory is outside the reviewed invocation.",
        ));
    }
    for mutable in profile
        .mutable_inputs
        .iter()
        .filter(|item| item.invocations.contains(&invocation.id))
    {
        let approved = matches!((&mutable.locator, &mutable.coverage),
            (Locator::Checkout { path }, Coverage::Consumers { ids })
                if profile.consumers.iter().any(|consumer| {
                    consumer.invocation == invocation.id && consumer.anchor == *path && ids.contains(&consumer.id)
                })
        );
        if !approved {
            return Err(refused(
                "A mutable input is outside the finite reviewed target-entry source coverage.",
            ));
        }
    }
    Ok(invocation)
}

fn supported_args(args: &[String]) -> bool {
    let Some((command, options)) = args.split_first() else {
        return false;
    };
    if command != "check" || !options.iter().any(|arg| arg == "--locked") {
        return false;
    }

    let mut options = options.iter();
    while let Some(arg) = options.next() {
        match arg.as_str() {
            "--locked"
            | "--offline"
            | "--frozen"
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
            "--target" => {
                if options
                    .next()
                    .is_none_or(|target| target != &native_target())
                {
                    return false;
                }
            }
            "--package" | "-p" | "--exclude" | "--features" | "-F" | "--profile" | "--bin"
            | "--example" | "--test" | "--bench" => {
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
    let native = NativeBuild::read(profile, invocation, &native_target())?;
    let variants = native
        .as_ref()
        .map(|seal| seal.verify_checkout(profile, invocation, checkout))
        .transpose()?;
    let cwd = if invocation.cwd == Path::new(".") {
        checkout.path.clone()
    } else {
        checkout.path.join(&invocation.cwd)
    };
    slots::owner_directory(&cwd)
        .map_err(|_| refused("Could not verify the reviewed working directory."))?;

    let environment: BTreeMap<_, _> = std::env::vars_os().collect();
    let mut environment = check_environment(invocation, &environment)?;
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
    let (cargo, rustc) = verified_executables(invocation, &environment)?;
    environment.insert("RUSTC".into(), rustc.as_os_str().to_owned());
    environment.insert("RUSTUP_AUTO_INSTALL".into(), "0".into());
    let native_tools = native
        .as_ref()
        .map(|seal| seal.bind_tools(&cargo, &rustc, &cwd, &mut environment))
        .transpose()?;
    let environment_digest = digest(
        &serde_json::to_vec(
            &environment
                .iter()
                .map(|(name, value)| (name.as_encoded_bytes(), value.as_encoded_bytes()))
                .collect::<Vec<_>>(),
        )
        .map_err(|_| refused("Could not bind the producer environment evidence."))?,
    )?;
    let tools_before = tool_identities(&cargo, &rustc)?;
    verify_tools(invocation, &cwd, &cargo, &rustc, &environment)?;
    if let Some(seal) = &native {
        let expected = seal
            .target_libdir()
            .ok_or_else(|| refused("The reviewed Rust target sysroot is unavailable."))?;
        let actual = cargo_output(
            &rustc,
            &cwd,
            &["--print", "target-libdir"],
            4096,
            &environment,
        )?;
        if actual.strip_suffix(b"\n") != Some(expected.as_os_str().as_encoded_bytes()) {
            return Err(refused(
                "The active Rust target sysroot differs from the reviewed compiler material.",
            ));
        }
    }
    let discovered = discover(&cargo, &cwd, &environment)?;
    if tools_before != tool_identities(&cargo, &rustc)? {
        return Err(refused(
            "The verified Cargo or rustc executable changed during discovery.",
        ));
    }
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
    if let Some(seal) = &native {
        verify_native_graph(&discovered, &checkout.path, seal.packages())?;
    }
    let reviewed_sources = reviewed_source_facts(
        &inputs,
        &discovered,
        checkout,
        profile,
        invocation,
        native.is_some(),
    )?;
    let seal_digest = native.as_ref().map(|seal| &seal.seal_digest);
    let graph_parts = (&inputs, reviewed_sources, &cargo, &rustc, &tools_before);
    let graph_bytes = if seal_digest.is_some() {
        serde_json::to_vec(&(&graph_parts, seal_digest, &native_tools, variants))
    } else {
        serde_json::to_vec(&graph_parts)
    }
    .map_err(|_| refused("Could not bind the producer graph evidence."))?;
    let graph_digest = digest(&graph_bytes)?;

    let fixed_environment: Vec<_> = environment
        .iter()
        .filter(|(key, _)| {
            !invocation.environment.iter().any(|item| {
                key.as_os_str() == std::ffi::OsStr::new(&item.name)
                    && matches!(item.expectation, EnvironmentExpectation::Mutable { .. })
            })
        })
        .map(|(key, value)| (key.as_encoded_bytes(), value.as_encoded_bytes()))
        .collect();
    let stable_parts = (
        &inputs,
        &discovered.graph_digest,
        &cargo,
        &rustc,
        &tools_before,
        &invocation.argv,
        invocation.layout,
        &cwd,
        fixed_environment,
    );
    let stable_bytes = if seal_digest.is_some() {
        serde_json::to_vec(&(&stable_parts, seal_digest, &native_tools))
    } else {
        serde_json::to_vec(&stable_parts)
    }
    .map_err(|_| refused("Could not bind the stable producer profile inputs."))?;
    let stable_digest = digest(&stable_bytes)?;

    Ok(CheckedInvocation {
        evidence: InvocationEvidence {
            invocation: invocation.id.clone(),
            graph_digest,
            environment_digest,
            stable_digest: Some(stable_digest),
        },
        cargo,
        cwd,
        environment,
        needs_guide: native.is_some(),
    })
}

fn check_environment(
    invocation: &Invocation,
    environment: &BTreeMap<std::ffi::OsString, std::ffi::OsString>,
) -> Result<BTreeMap<std::ffi::OsString, std::ffi::OsString>, Refusal> {
    let denied = || refused("The effective environment differs from the reviewed invocation.");
    for name in environment.keys() {
        let name = name.to_str().ok_or_else(denied)?;
        if matches!(
            name,
            "RUSTC" | "RUSTDOC" | "RUSTC_WRAPPER" | "RUSTC_WORKSPACE_WRAPPER"
        ) || matches!(
            name,
            "BASH_ENV" | "ENV" | "CDPATH" | "BASHOPTS" | "SHELLOPTS"
        ) || compiler_override(name)
            || name.starts_with("BASH_FUNC_")
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
        if variable.name.starts_with("GIT_")
            || compiler_override(&variable.name)
            || matches!(
                variable.name.as_str(),
                "RUSTC" | "RUSTDOC" | "RUSTUP_AUTO_INSTALL"
            )
        {
            return Err(denied());
        }
        let value = environment
            .get(std::ffi::OsStr::new(&variable.name))
            .map(|value| value.to_str().ok_or_else(denied))
            .transpose()?;
        let matches = match &variable.expectation {
            EnvironmentExpectation::Present { value: expected } => value == Some(expected.as_str()),
            EnvironmentExpectation::Absent => value.is_none(),
            EnvironmentExpectation::Mutable { .. } => false,
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
    Ok(facts
        .into_iter()
        .filter_map(|(name, value)| value.map(|value| (name.into(), value.into())))
        .collect())
}

fn compiler_override(name: &str) -> bool {
    matches!(
        name,
        "RUSTFLAGS"
            | "RUSTDOCFLAGS"
            | "RUST_TARGET_PATH"
            | "CARGO_ENCODED_RUSTFLAGS"
            | "CARGO_ENCODED_RUSTDOCFLAGS"
            | "CARGO_BUILD_TARGET"
    ) || name.starts_with("CARGO_BUILD_RUST")
        || name.starts_with("CARGO_ALIAS_")
        || name.starts_with("CARGO_TARGET_")
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

pub(super) fn read_input(path: &Path) -> Result<Option<Vec<u8>>, Refusal> {
    read_input_bounded(path, INPUT_LIMIT)
}

pub(super) fn read_input_bounded(path: &Path, cap: usize) -> Result<Option<Vec<u8>>, Refusal> {
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
        .take(cap as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unproved())?;
    let after = fs::symlink_metadata(path).map_err(|_| unproved())?;
    if bytes.len() > cap
        || super::directory_stamp(&before) != super::directory_stamp(&after)
        || before.len() != after.len()
        || !after.is_file()
    {
        return Err(unproved());
    }
    Ok(Some(bytes))
}

pub(super) fn tracked_object(checkout: &CheckoutProof, path: &Path) -> Result<String, Refusal> {
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

pub(super) fn blob(checkout: &CheckoutProof, oid: &str) -> Result<Vec<u8>, Refusal> {
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
    let check = |path: PathBuf, cargo_config: bool| -> Result<(), Refusal> {
        let bytes = read_input(&path)?;
        let observed = bytes.as_deref().map(digest).transpose()?;
        if inputs.get(&path).and_then(Option::as_ref) != observed.as_ref() {
            return Err(omitted());
        }
        if cargo_config
            && bytes
                .as_deref()
                .is_some_and(|bytes| !supported_config(bytes))
        {
            return Err(refused(
                "Reviewed Cargo config contains unsupported keys or tool overrides.",
            ));
        }
        Ok(())
    };
    for ancestor in cwd.ancestors() {
        for name in [".cargo/config", ".cargo/config.toml"] {
            check(ancestor.join(name), true)?;
        }
        for name in ["rust-toolchain", "rust-toolchain.toml"] {
            check(ancestor.join(name), false)?;
        }
        if ancestor != checkout.path {
            check(ancestor.join("Cargo.toml"), false)?;
        }
    }
    for name in ["config", "config.toml"] {
        check(cargo_home.join(name), true)?;
    }
    Ok(())
}

fn supported_config(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    let mut section = "";
    let mut keys = BTreeSet::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if matches!(line, "[build]" | "[net]") {
            section = line;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            return false;
        };
        let key = key.trim();
        let value = value.trim();
        if !keys.insert((section, key)) {
            return false;
        }
        match (section, key) {
            ("[net]", "offline") if matches!(value, "true" | "false") => (),
            ("[build]", "jobs")
                if value
                    .parse::<u16>()
                    .is_ok_and(|jobs| (1..=256).contains(&jobs)) => {}
            ("[build]", "target-dir" | "build-dir") if simple_output_path(value) => (),
            _ => return false,
        }
    }
    true
}

fn simple_output_path(value: &str) -> bool {
    let Some(path) = value
        .strip_prefix('"')
        .and_then(|path| path.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|path| path.strip_suffix('\''))
        })
    else {
        return false;
    };
    !path.is_empty()
        && path.split('/').all(|part| {
            !matches!(part, "" | "." | "..")
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_-. ".contains(&byte))
        })
}

fn native_target() -> String {
    format!("{}-apple-darwin", std::env::consts::ARCH)
}

fn release_for(layout: CargoLayout) -> &'static str {
    match layout {
        CargoLayout::Flat => "1.97.1",
        CargoLayout::UnitDir => "1.100.0",
    }
}

fn verified_executables(
    invocation: &Invocation,
    environment: &BTreeMap<std::ffi::OsString, std::ffi::OsString>,
) -> Result<(PathBuf, PathBuf), Refusal> {
    use std::ffi::OsStr;
    let uncertain =
        || refused("Could not establish the exact reviewed Cargo and rustc executables or layout.");
    let release = release_for(invocation.layout);
    let toolchain = format!("{release}-{}", native_target());
    let get = |name| environment.get(OsStr::new(name));
    if get("RUSTUP_TOOLCHAIN")
        .is_some_and(|actual| actual != release && actual != toolchain.as_str())
        || get("RUSTUP_AUTO_INSTALL").is_some_and(|actual| actual != "0")
    {
        return Err(uncertain());
    }
    let rustup_home = get("RUSTUP_HOME")
        .map(PathBuf::from)
        .or_else(|| get("HOME").map(|home| Path::new(home).join(".rustup")))
        .ok_or_else(uncertain)?;
    slots::owner_directory(&rustup_home).map_err(|_| uncertain())?;
    let bin = rustup_home.join("toolchains").join(toolchain).join("bin");
    slots::owner_directory(&bin).map_err(|_| uncertain())?;
    let cargo = bin.join("cargo");
    let rustc = bin.join("rustc");
    let path = get("PATH").ok_or_else(uncertain)?;
    let locate = |tool| {
        std::env::split_paths(path).find_map(|directory| {
            use std::os::unix::fs::PermissionsExt as _;
            let file = directory.join(tool);
            let metadata = fs::metadata(&file).ok()?;
            (metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
                .then(|| file.canonicalize().ok())
                .flatten()
        })
    };
    let rustc_from_path = locate("rustc").ok_or_else(uncertain)?;
    if rustc_from_path != rustc && locate("rustup").as_ref() != Some(&rustc_from_path) {
        return Err(uncertain());
    }
    Ok((cargo, rustc))
}

fn tool_identities(cargo: &Path, rustc: &Path) -> Result<String, Refusal> {
    let unproved =
        || refused("A reviewed tool executable is not a canonical owner-controlled file.");
    let identity = |path: &Path| {
        let metadata = open_owner_state_file(path)
            .and_then(|file| file.metadata())
            .map_err(|_| unproved())?;
        Ok::<_, Refusal>((super::directory_stamp(&metadata), metadata.len()))
    };
    let bytes =
        serde_json::to_vec(&[identity(cargo)?, identity(rustc)?]).map_err(|_| unproved())?;
    digest(&bytes)
}

fn verify_tools(
    invocation: &Invocation,
    cwd: &Path,
    cargo: &Path,
    rustc: &Path,
    environment: &BTreeMap<std::ffi::OsString, std::ffi::OsString>,
) -> Result<(), Refusal> {
    let incorrect = || {
        refused("Actual Cargo or rustc identity and layout differs from the reviewed invocation.")
    };
    let output = cargo_output(cargo, cwd, &["-V"], 1024, environment)?;
    let cargo_version = std::str::from_utf8(&output)
        .map_err(|_| incorrect())?
        .trim_end();
    let output = cargo_output(rustc, cwd, &["-vV"], 4096, environment)?;
    let rustc_info = std::str::from_utf8(&output).map_err(|_| incorrect())?;
    let rustc_version = rustc_info.lines().next().ok_or_else(incorrect)?;
    let release = release_for(invocation.layout);
    let native = native_target();
    if !version_matches(&invocation.cargo, cargo_version)
        || !version_matches(&format!("cargo {release}"), cargo_version)
        || !version_matches(&invocation.rustc, rustc_version)
        || !rustc_info
            .lines()
            .any(|line| line.strip_prefix("release: ") == Some(release))
        || !rustc_info
            .lines()
            .any(|line| line.strip_prefix("host: ") == Some(native.as_str()))
    {
        return Err(incorrect());
    }
    Ok(())
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
    id: String,
    name: String,
    version: String,
    source: Option<String>,
    targets: Vec<PackageTarget>,
}

#[derive(Deserialize)]
struct PackageTarget {
    name: String,
    kind: Vec<String>,
    src_path: PathBuf,
}

struct Discovered {
    resolved: metadata::Resolved,
    packages: Vec<Package>,
    graph_digest: String,
    workspace_members: Vec<String>,
    nodes: Vec<ResolveNode>,
}

#[derive(Deserialize)]
struct ResolveNode {
    id: String,
    deps: Vec<ResolveDependency>,
    features: Vec<String>,
}

#[derive(Deserialize)]
struct ResolveDependency {
    name: String,
    pkg: String,
    dep_kinds: Vec<ResolveKind>,
}

#[derive(Deserialize)]
struct ResolveKind {
    kind: Option<String>,
    target: Option<String>,
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
    let graph_digest = digest(
        &serde_json::to_vec(&[
            &metadata["packages"],
            &metadata["resolve"],
            &metadata["workspace_members"],
            &metadata["workspace_default_members"],
            &metadata["target_directory"],
            &metadata["build_directory"],
        ])
        .map_err(|_| refused("Could not bind the resolved Cargo graph."))?,
    )?;
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
        graph_digest,
        workspace_members: deserialize_discovery(&metadata["workspace_members"])?,
        nodes: deserialize_discovery(&metadata["resolve"]["nodes"])?,
    })
}

fn verify_native_graph(
    graph: &Discovered,
    checkout: &Path,
    expected: &[NativePackage],
) -> Result<(), Refusal> {
    let unproved = || refused("The actual Cargo graph differs from the sealed native build.");
    let ids: BTreeMap<_, _> = graph
        .packages
        .iter()
        .map(|pkg| (&pkg.id, &pkg.name))
        .collect();
    let members: BTreeSet<_> = graph.workspace_members.iter().collect();
    let nodes: BTreeMap<_, _> = graph.nodes.iter().map(|node| (&node.id, node)).collect();
    if graph.packages.len() != expected.len()
        || ids.len() != expected.len()
        || graph.nodes.len() != expected.len()
        || nodes.len() != expected.len()
        || graph.workspace_members.len() != expected.len()
        || members.len() != expected.len()
    {
        return Err(unproved());
    }
    for package in &graph.packages {
        if package.source.is_some() || !members.contains(&package.id) {
            return Err(unproved());
        }
        let node = nodes.get(&package.id).ok_or_else(unproved)?;
        if !node.features.is_empty() {
            return Err(unproved());
        }
        let mut dependencies = Vec::new();
        for dependency in &node.deps {
            let name = ids.get(&dependency.pkg).ok_or_else(unproved)?;
            if dependency.name != **name
                || dependency.dep_kinds.len() != 1
                || dependency.dep_kinds[0].kind.is_some()
                || dependency.dep_kinds[0].target.is_some()
            {
                return Err(unproved());
            }
            dependencies.push((*name).clone());
        }
        dependencies.sort();
        let mut targets = Vec::new();
        for target in &package.targets {
            let [kind] = target.kind.as_slice() else {
                return Err(unproved());
            };
            targets.push(NativeTarget {
                name: target.name.clone(),
                kind: kind.clone(),
                source: target
                    .src_path
                    .strip_prefix(checkout)
                    .map_err(|_| unproved())?
                    .to_path_buf(),
            });
        }
        targets.sort_by(|left, right| left.name.cmp(&right.name));
        let actual = NativePackage {
            name: package.name.clone(),
            version: package.version.clone(),
            manifest: package
                .manifest_path
                .strip_prefix(checkout)
                .map_err(|_| unproved())?
                .to_path_buf(),
            targets,
            dependencies,
        };
        if !expected.contains(&actual) {
            return Err(unproved());
        }
    }
    Ok(())
}

fn reviewed_source_facts(
    inputs: &BTreeMap<PathBuf, Option<String>>,
    graph: &Discovered,
    checkout: &CheckoutProof,
    profile: &PoolProfile,
    invocation: &Invocation,
    sealed: bool,
) -> Result<BTreeMap<PathBuf, String>, Refusal> {
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
                && (!package.manifest_path.starts_with(&checkout.path)
                    || !fixed(&package.manifest_path))
        })
    {
        return Err(unproved());
    }
    if !sealed
        && graph.packages.iter().any(|package| {
            package.targets.iter().any(|target| {
                target
                    .kind
                    .iter()
                    .any(|kind| matches!(kind.as_str(), "custom-build" | "proc-macro"))
            })
        })
    {
        return Err(refused(
            "The reviewed graph contains an executable build target without foreground custody proof.",
        ));
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
    let mut sources = BTreeMap::new();
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
                    target.src_path == anchor
                        && target.kind.iter().any(|kind| {
                            kind == &consumer.role || (consumer.role == "library" && kind == "lib")
                        })
                })
        });
        let head = tracked_object(checkout, &consumer.anchor)?;
        if !covered || (head != consumer.prior_object && head != consumer.requested_object) {
            return Err(unproved());
        }
        let actual = read_input(&anchor)?.ok_or_else(unproved)?;
        let approved = blob(checkout, &consumer.prior_object)? == actual
            || (consumer.prior_object != consumer.requested_object
                && blob(checkout, &consumer.requested_object)? == actual);
        if !approved {
            return Err(unproved());
        }
        let digest = digest(&actual)?;
        if sources
            .insert(anchor, digest.clone())
            .is_some_and(|previous| previous != digest)
        {
            return Err(unproved());
        }
    }
    Ok(sources)
}

pub(super) fn digest(bytes: &[u8]) -> Result<String, Refusal> {
    let bytes =
        sha256(bytes).map_err(|_| refused("Could not bind the producer evidence digest."))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}
