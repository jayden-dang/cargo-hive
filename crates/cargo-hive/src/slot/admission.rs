//! Sealed admission for a bounded native Cargo build.

use super::profile::{digest, read_input_bounded, refused};
use super::{CheckoutProof, Refusal, directory_stamp, git_scope};
use cargo_hive_platform::{
    EntryKind, host_fs,
    proc::{self, Capture, Spec},
    signals::cancelled,
    slot::sha256_reader,
};
use cargo_hive_state::slots::{
    self, CargoLayout, Coverage, EnvironmentExpectation, GraphExpectation, InputIdentity,
    Invocation, Locator, MutablePresence, PoolProfile, ProfileBinding,
};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::{OsStr, OsString};
use std::os::unix::fs::MetadataExt as _;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

const SEAL_PREFIX: &str = "closure-v1:sha256=";
const NATIVE_ARGV: [&str; 5] = [
    "build",
    "--workspace",
    "--locked",
    "--offline",
    "--message-format=json",
];
const FILE_LIMIT: u64 = 256 * 1024 * 1024;
const TOTAL_MATERIAL_LIMIT: u64 = 1024 * 1024 * 1024;
const ENTRY_LIMIT: usize = 256;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Seal {
    schema: u32,
    recipe: Recipe,
    profile: ProfileBinding,
    proof: String,
    invocation: String,
    layout: CargoLayout,
    native_target: String,
    endpoints: Vec<Endpoint>,
    tools: Tools,
    packages: Vec<NativePackage>,
    data: Vec<FiniteData>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum Recipe {
    NativeBuild,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Endpoint {
    commit: String,
    tree: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Tools {
    cargo: Material,
    rustc: Material,
    rust_runtime: Vec<Material>,
    target_sysroot: Vec<Material>,
    clang: Material,
    ld: Material,
    sdk: PathBuf,
    sdk_settings: Material,
    system_library: Material,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Material {
    path: PathBuf,
    sha256: String,
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct NativePackage {
    pub name: String,
    pub version: String,
    pub manifest: PathBuf,
    pub targets: Vec<NativeTarget>,
    pub dependencies: Vec<String>,
}

#[derive(Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct NativeTarget {
    pub name: String,
    pub kind: String,
    pub source: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FiniteData {
    id: String,
    sha256: Vec<String>,
    absent: bool,
}

pub(super) struct NativeBuild {
    pub seal_digest: String,
    seal: Seal,
}

impl NativeBuild {
    pub fn read(
        profile: &PoolProfile,
        invocation: &Invocation,
        native: &str,
    ) -> Result<Option<Self>, Refusal> {
        let malformed =
            || refused("The reviewed native build closure is missing, ambiguous or invalid.");
        let mut proofs = profile
            .proofs
            .iter()
            .filter(|proof| proof.invocation == invocation.id && proof.layout == invocation.layout);
        let Some(proof) = proofs.next() else {
            return Err(malformed());
        };
        if !proof.evidence.starts_with("closure-v1:") {
            if proofs.any(|item| item.evidence.starts_with("closure-v1:")) {
                return Err(malformed());
            }
            return Ok(None);
        }
        let hash = proof
            .evidence
            .strip_prefix(SEAL_PREFIX)
            .filter(|value| slots::hex(value, 64))
            .ok_or_else(malformed)?;
        if proofs.next().is_some() {
            return Err(malformed());
        }
        let mut references = profile.graph_inputs.iter().filter_map(|input| {
            match (&input.locator, &input.expectation) {
                (
                    Locator::External { path },
                    GraphExpectation::Present {
                        identity: InputIdentity::ReviewedBytes { sha256 },
                    },
                ) if sha256 == hash => Some(path),
                _ => None,
            }
        });
        let path = references.next().ok_or_else(malformed)?;
        if references.next().is_some() {
            return Err(malformed());
        }
        let bytes = read_input_bounded(path, slots::MAX_BYTES as usize)?.ok_or_else(malformed)?;
        if digest(&bytes)? != hash {
            return Err(malformed());
        }
        let seal: Seal = serde_json::from_slice(&bytes).map_err(|_| malformed())?;
        if seal.schema != 1
            || seal.profile.id != profile.id
            || seal.profile.revision != profile.revision
            || seal.proof != proof.id
            || seal.invocation != invocation.id
            || seal.layout != invocation.layout
            || seal.native_target != native
            || !matches!(seal.recipe, Recipe::NativeBuild)
            || invocation.argv.iter().map(String::as_str).ne(NATIVE_ARGV)
            || invocation.cwd != Path::new(".")
            || !(1..=2).contains(&seal.endpoints.len())
            || !seal.endpoints.iter().all(|endpoint| {
                slots::object_id(&endpoint.commit) && slots::object_id(&endpoint.tree)
            })
            || !bounded_unique(
                seal.endpoints
                    .iter()
                    .map(|endpoint| endpoint.commit.as_str()),
            )
            || !bounded_unique(seal.packages.iter().map(|item| item.name.as_str()))
            || !seal.packages.iter().all(valid_package)
            || !bounded_unique(seal.data.iter().map(|item| item.id.as_str()))
            || !seal.data.iter().all(|item| {
                bounded_unique(item.sha256.iter().map(String::as_str))
                    && item.sha256.iter().all(|hash| slots::hex(hash, 64))
            })
            || !valid_inventory(&seal.tools.rust_runtime)
            || !valid_inventory(&seal.tools.target_sysroot)
            || !native_environment(&invocation.environment)
        {
            return Err(malformed());
        }
        let mut covered = BTreeSet::new();
        for input in profile
            .mutable_inputs
            .iter()
            .filter(|item| item.invocations.contains(&invocation.id))
        {
            if !matches!(input.locator, Locator::Checkout { .. }) {
                return Err(malformed());
            }
            let variant = seal
                .data
                .iter()
                .find(|data| data.id == input.id)
                .ok_or_else(malformed)?;
            if variant.absent != (input.presence == MutablePresence::PresentOrAbsent)
                || !matches!(&input.coverage, Coverage::Consumers { ids } if ids.iter().any(|id|
                    profile.consumers.iter().any(|item| item.id == *id && item.invocation == invocation.id)))
            {
                return Err(malformed());
            }
            covered.insert(input.id.as_str());
        }
        if covered.len() != seal.data.len() {
            return Err(malformed());
        }
        Ok(Some(Self {
            seal_digest: hash.to_owned(),
            seal,
        }))
    }

    pub fn packages(&self) -> &[NativePackage] {
        &self.seal.packages
    }

    pub fn verify_checkout(
        &self,
        profile: &PoolProfile,
        invocation: &Invocation,
        checkout: &CheckoutProof,
    ) -> Result<BTreeMap<PathBuf, Option<String>>, Refusal> {
        let unproved = || {
            refused(
                "The sealed native build's checkout, source or mutable data bytes could not be verified.",
            )
        };
        let endpoint = self
            .seal
            .endpoints
            .iter()
            .find(|endpoint| endpoint.commit == checkout.head)
            .ok_or_else(unproved)?;
        let tree = git_scope(&checkout.path, &["rev-parse", "--verify", "HEAD^{tree}"])?;
        if tree.strip_suffix(b"\n") != Some(endpoint.tree.as_bytes()) {
            return Err(unproved());
        }
        let mut observed = BTreeMap::new();
        for field in &self.seal.data {
            let input = profile
                .mutable_inputs
                .iter()
                .find(|item| item.id == field.id && item.invocations.contains(&invocation.id))
                .ok_or_else(unproved)?;
            let Locator::Checkout { path } = &input.locator else {
                return Err(unproved());
            };
            let value = read_input_bounded(&checkout.path.join(path), slots::MAX_BYTES as usize)?
                .as_deref()
                .map(digest)
                .transpose()?;
            if !value
                .as_ref()
                .map_or(field.absent, |hash| field.sha256.contains(hash))
                || observed.insert(path.clone(), value).is_some()
            {
                return Err(unproved());
            }
        }
        let tracked = git_scope(
            &checkout.path,
            &["ls-tree", "-r", "-z", "--full-tree", &checkout.head],
        )?;
        let body = tracked.strip_suffix(b"\0").ok_or_else(unproved)?;
        for row in body.split(|byte| *byte == 0) {
            let text = std::str::from_utf8(row).map_err(|_| unproved())?;
            let (meta, name) = text.split_once('\t').ok_or_else(unproved)?;
            let path = Path::new(name);
            if !relative(path)
                || !meta.starts_with("100644 blob ") && !meta.starts_with("100755 blob ")
            {
                return Err(unproved());
            }
            if !observed.contains_key(path) && !profile.graph_inputs.iter().any(|item| matches!(
                (&item.locator, &item.expectation),
                (Locator::Checkout { path: fixed }, GraphExpectation::Present { .. }) if fixed == path
            )) {
                return Err(unproved());
            }
        }
        Ok(observed)
    }

    pub fn bind_tools(
        &self,
        cargo: &Path,
        rustc: &Path,
        cwd: &Path,
        environment: &mut BTreeMap<OsString, OsString>,
    ) -> Result<String, Refusal> {
        let unproved =
            || refused("The sealed native compiler, linker or SDK identity could not be verified.");
        let tools = &self.seal.tools;
        if tools.cargo.path != cargo || tools.rustc.path != rustc {
            return Err(unproved());
        }
        for name in ["SDKROOT", "DEVELOPER_DIR", "CC", "CXX", "LD", "AR"] {
            if std::env::var_os(name).is_some() {
                return Err(unproved());
            }
        }
        let cargo_home = environment
            .get(std::ffi::OsStr::new("CARGO_HOME"))
            .ok_or_else(unproved)?;
        let clang = xcode_path(cwd, cargo_home, &["--find", "clang"])?;
        let ld = xcode_path(cwd, cargo_home, &["--find", "ld"])?;
        let sdk = xcode_path(cwd, cargo_home, &["--show-sdk-path"])?;
        if clang != tools.clang.path
            || ld != tools.ld.path
            || sdk != tools.sdk
            || tools.sdk_settings.path != sdk.join("SDKSettings.json")
            || sdk
                .join("usr/lib/libSystem.tbd")
                .canonicalize()
                .ok()
                .as_ref()
                != Some(&tools.system_library.path)
            || !tools.system_library.path.starts_with(&sdk)
        {
            return Err(unproved());
        }
        let mut remaining = TOTAL_MATERIAL_LIMIT;
        let mut stamps = Vec::new();
        for material in [
            &tools.cargo,
            &tools.rustc,
            &tools.clang,
            &tools.ld,
            &tools.sdk_settings,
            &tools.system_library,
        ] {
            stamps.push(material.verify(&mut remaining)?);
        }
        let compiler_unproved =
            || refused("The reviewed native Rust compiler material could not be verified.");
        let lib = rustc
            .parent()
            .and_then(Path::parent)
            .ok_or_else(compiler_unproved)?
            .join("lib");
        let target_libdir = self.target_libdir().ok_or_else(compiler_unproved)?;
        verify_inventory(&lib, &tools.rust_runtime, true, &mut remaining, &mut stamps)?;
        verify_inventory(
            &target_libdir,
            &tools.target_sysroot,
            false,
            &mut remaining,
            &mut stamps,
        )?;
        verify_rust_loader(cwd, cargo_home, rustc, &tools.rust_runtime)?;
        let linker = format!(
            "CARGO_TARGET_{}_LINKER",
            self.seal
                .native_target
                .replace('-', "_")
                .to_ascii_uppercase()
        );
        let mut linker_flags = OsString::from("-Clink-arg=--ld-path=");
        linker_flags.push(ld.as_os_str());
        environment.insert("PATH".into(), "/usr/bin:/bin".into());
        environment.insert("SDKROOT".into(), sdk.into_os_string());
        environment.insert(linker.into(), clang.into_os_string());
        environment.insert("CARGO_ENCODED_RUSTFLAGS".into(), linker_flags);
        digest(&serde_json::to_vec(&stamps).map_err(|_| unproved())?)
    }
    pub fn target_libdir(&self) -> Option<PathBuf> {
        Some(
            self.seal
                .tools
                .rustc
                .path
                .parent()?
                .parent()?
                .join("lib/rustlib")
                .join(&self.seal.native_target)
                .join("lib"),
        )
    }
}

impl Material {
    fn verify(&self, remaining: &mut u64) -> Result<String, Refusal> {
        let unproved =
            || refused("A sealed native tool or SDK file changed or is outside its byte bound.");
        let path = &self.path;
        if !slots::hex(&self.sha256, 64) || path.canonicalize().ok().as_ref() != Some(path) {
            return Err(unproved());
        }
        let fs = host_fs();
        let mut file = fs.open_regular(path).map_err(|_| unproved())?;
        let before = file.metadata().map_err(|_| unproved())?;
        if before.mode() & 0o7022 != 0 || before.len() > FILE_LIMIT {
            return Err(unproved());
        }
        *remaining = remaining.checked_sub(before.len()).ok_or_else(unproved)?;
        let hash = sha256_reader(&mut file, before.len(), cancelled).map_err(|err| {
            if err.kind() == std::io::ErrorKind::Interrupted && cancelled() {
                cargo_hive_state::StateError::Cancelled.into()
            } else {
                unproved()
            }
        })?;
        let actual: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
        let after = std::fs::symlink_metadata(path).map_err(|_| unproved())?;
        if after.len() != before.len()
            || directory_stamp(&after) != directory_stamp(&before)
            || !after.is_file()
            || actual != self.sha256
        {
            return Err(unproved());
        }
        digest(
            &serde_json::to_vec(&(directory_stamp(&before), before.len(), &self.sha256))
                .map_err(|_| unproved())?,
        )
    }
}

fn valid_inventory(materials: &[Material]) -> bool {
    (1..=ENTRY_LIMIT).contains(&materials.len())
        && materials.windows(2).all(|pair| pair[0].path < pair[1].path)
        && materials
            .iter()
            .all(|material| material.path.is_absolute() && slots::hex(&material.sha256, 64))
}

fn verify_inventory(
    directory: &Path,
    materials: &[Material],
    skip_rustlib: bool,
    remaining: &mut u64,
    stamps: &mut Vec<String>,
) -> Result<(), Refusal> {
    let unproved = || refused("The reviewed native Rust compiler material could not be verified.");
    if directory.canonicalize().ok().as_deref() != Some(directory) {
        return Err(unproved());
    }
    let fs = host_fs();
    let mut paths = Vec::new();
    for (name, kind) in fs
        .read_dir_bounded(directory, ENTRY_LIMIT + usize::from(skip_rustlib))
        .map_err(|_| unproved())?
    {
        if skip_rustlib && name == "rustlib" && kind == EntryKind::Dir {
            continue;
        }
        if kind != EntryKind::File {
            return Err(unproved());
        }
        paths.push(directory.join(name));
    }
    paths.sort();
    if paths
        .iter()
        .ne(materials.iter().map(|material| &material.path))
    {
        return Err(unproved());
    }
    for material in materials {
        match material.verify(remaining) {
            Ok(stamp) => stamps.push(stamp),
            Err(error @ Refusal::State(cargo_hive_state::StateError::Cancelled)) => {
                return Err(error);
            }
            Err(_) => return Err(unproved()),
        }
    }
    Ok(())
}

fn verify_rust_loader(
    cwd: &Path,
    home: &OsStr,
    rustc: &Path,
    runtime: &[Material],
) -> Result<(), Refusal> {
    let unproved =
        || refused("The installed Rust compiler loader differs from the sealed compiler material.");
    let otool = xcode_path(cwd, home, &["--find", "otool"])?;
    let image = |flag| {
        native_output(
            &otool,
            [OsStr::new(flag), rustc.as_os_str()],
            cwd,
            home,
            128 * 1024,
        )
    };
    let commands = image("-l")?;
    let commands = std::str::from_utf8(&commands).map_err(|_| unproved())?;
    let lines: Vec<_> = commands.lines().map(str::trim).collect();
    let mut rpaths = lines.windows(3).filter(|row| row[0] == "cmd LC_RPATH");
    let expected = "path @loader_path/../lib (offset 12)";
    if !rpaths.next().is_some_and(|row| row[2] == expected) || rpaths.next().is_some() {
        return Err(unproved());
    }
    let dependencies = image("-L")?;
    let dependencies = std::str::from_utf8(&dependencies).map_err(|_| unproved())?;
    let mut drivers = 0;
    for line in dependencies.lines().skip(1) {
        let (name, _) = line
            .trim()
            .split_once(" (compatibility version ")
            .ok_or_else(unproved)?;
        if let Some(name) = name.strip_prefix("@rpath/") {
            if !runtime
                .iter()
                .any(|material| material.path.file_name() == Some(OsStr::new(name)))
            {
                return Err(unproved());
            }
            drivers +=
                usize::from(name.starts_with("librustc_driver-") && name.ends_with(".dylib"));
        } else if !name.starts_with("/usr/lib/") && !name.starts_with("/System/Library/") {
            return Err(unproved());
        }
    }
    if drivers != 1 {
        return Err(unproved());
    }
    Ok(())
}

fn native_output<'a>(
    program: &Path,
    args: impl IntoIterator<Item = &'a OsStr>,
    cwd: &Path,
    home: &OsStr,
    cap: usize,
) -> Result<Vec<u8>, Refusal> {
    let unproved = || refused("Could not resolve the reviewed installed native toolchain.");
    let mut spec = Spec::new(program);
    spec.args.extend(args.into_iter().map(OsStr::to_owned));
    spec.cwd = Some(cwd.to_path_buf());
    spec.env_clear = true;
    spec.env_set = vec![
        ("HOME".into(), home.to_owned()),
        ("TMPDIR".into(), home.to_owned()),
        ("PATH".into(), "/usr/bin:/bin".into()),
    ];
    let ran = proc::run_bounded_complete(
        &spec,
        Duration::from_secs(10),
        Capture {
            stdout_cap: cap + 1,
            stderr_cap: 1024,
        },
        cancelled,
    )
    .map_err(|err| match err {
        proc::ChildOutputError::Cancelled => cargo_hive_state::StateError::Cancelled.into(),
        _ => unproved(),
    })?;
    if !ran.status.success() || ran.stdout.len() > cap {
        return Err(unproved());
    }
    Ok(ran.stdout)
}

fn xcode_path(cwd: &Path, home: &OsStr, args: &[&str]) -> Result<PathBuf, Refusal> {
    let unproved = || refused("Could not resolve the reviewed installed native toolchain.");
    let argv = std::iter::once(OsStr::new("--no-cache")).chain(args.iter().map(OsStr::new));
    let stdout = native_output(Path::new("/usr/bin/xcrun"), argv, cwd, home, 4096)?;
    let raw = std::str::from_utf8(&stdout).map_err(|_| unproved())?;
    let path = Path::new(raw.strip_suffix('\n').ok_or_else(unproved)?);
    if !path.is_absolute() {
        return Err(unproved());
    }
    path.canonicalize().map_err(|_| unproved())
}

fn native_environment(entries: &[slots::Environment]) -> bool {
    entries.len() == 2
        && entries
            .iter()
            .all(|entry| match (&*entry.name, &entry.expectation) {
                ("CARGO_HOME", EnvironmentExpectation::Present { value }) => {
                    Path::new(value).is_absolute()
                }
                ("CARGO_NET_OFFLINE", EnvironmentExpectation::Present { value }) => value == "true",
                _ => false,
            })
}

fn valid_package(package: &NativePackage) -> bool {
    relative(&package.manifest)
        && bounded_unique(package.targets.iter().map(|target| target.name.as_str()))
        && package.targets.iter().all(|target| {
            relative(&target.source) && matches!(target.kind.as_str(), "bin" | "lib" | "proc-macro")
        })
        && package.dependencies.len() <= ENTRY_LIMIT
        && unique(package.dependencies.iter().map(String::as_str))
}

fn relative(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
}

fn bounded_unique<'a>(values: impl ExactSizeIterator<Item = &'a str>) -> bool {
    (1..=ENTRY_LIMIT).contains(&values.len()) && unique(values)
}

fn unique<'a>(values: impl Iterator<Item = &'a str>) -> bool {
    let mut seen = BTreeSet::new();
    values
        .into_iter()
        .all(|value| !value.is_empty() && seen.insert(value))
}
