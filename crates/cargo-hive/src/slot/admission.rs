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
const CLIPPY_ARGV: [&str; 8] = [
    "clippy",
    "--workspace",
    "--all-targets",
    "--all-features",
    "--locked",
    "--",
    "-D",
    "warnings",
];
const NEXTEST_WORKSPACE_ARGV: [&str; 8] = [
    "nextest",
    "run",
    "--workspace",
    "--all-features",
    "--no-run",
    "--locked",
    "--user-config-file",
    "none",
];
const NEXTEST_GATEWAYS_ARGV: [&str; 8] = [
    "nextest",
    "run",
    "-p",
    "gateways",
    "--no-run",
    "--locked",
    "--user-config-file",
    "none",
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

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Recipe {
    NativeBuild,
    Clippy,
    NextestWorkspace,
    NextestGateways,
}

impl Recipe {
    fn argv(self) -> &'static [&'static str] {
        match self {
            Self::NativeBuild => &NATIVE_ARGV,
            Self::Clippy => &CLIPPY_ARGV,
            Self::NextestWorkspace => &NEXTEST_WORKSPACE_ARGV,
            Self::NextestGateways => &NEXTEST_GATEWAYS_ARGV,
        }
    }

    fn is_nextest(self) -> bool {
        matches!(self, Self::NextestWorkspace | Self::NextestGateways)
    }
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
    otool: Material,
    sdk: PathBuf,
    sdk_settings: Material,
    system_library: Material,
    cargo_clippy: Option<Material>,
    clippy_driver: Option<Material>,
    cargo_nextest: Option<Material>,
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
            || invocation
                .argv
                .iter()
                .map(String::as_str)
                .ne(seal.recipe.argv().iter().copied())
            || !seal.tools.valid_for(seal.recipe)
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
            || (!matches!(seal.recipe, Recipe::NativeBuild)
                && seal.packages.iter().any(|package| {
                    package
                        .targets
                        .iter()
                        .any(|target| target.kind == "proc-macro")
                }))
            || (seal.recipe.is_nextest()
                && (invocation.target_root != Path::new("target")
                    || invocation.build_root != Path::new("target")))
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
        if self.seal.recipe.is_nextest()
            && read_input_bounded(&checkout.path.join(".config/nextest.toml"), 64 * 1024)?.is_some()
        {
            return Err(unproved());
        }
        match self.seal.recipe {
            Recipe::Clippy => self.verify_clippy_config(&checkout.path)?,
            Recipe::NextestWorkspace | Recipe::NextestGateways => {
                self.verify_default_harness(&checkout.path)?
            }
            Recipe::NativeBuild => (),
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

    fn verify_clippy_config(&self, checkout: &Path) -> Result<(), Refusal> {
        let unproved =
            || refused("A Clippy configuration is outside the sealed default lint recipe.");
        let mut checked = BTreeSet::new();
        for package in &self.seal.packages {
            let manifest = checkout.join(&package.manifest);
            for ancestor in manifest.parent().ok_or_else(unproved)?.ancestors() {
                if checked.insert(ancestor.to_path_buf()) {
                    for name in ["clippy.toml", ".clippy.toml"] {
                        if read_input_bounded(&ancestor.join(name), 64 * 1024)?.is_some() {
                            return Err(unproved());
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn verify_default_harness(&self, checkout: &Path) -> Result<(), Refusal> {
        let unproved = || {
            refused(
                "The sealed nextest recipe requires the reviewed default native harness and package scope.",
            )
        };
        if matches!(self.seal.recipe, Recipe::NextestGateways)
            && !self
                .seal
                .packages
                .iter()
                .any(|package| package.name == "gateways")
        {
            return Err(unproved());
        }
        for package in &self.seal.packages {
            let bytes = read_input_bounded(&checkout.join(&package.manifest), 64 * 1024)?
                .ok_or_else(unproved)?;
            let text = std::str::from_utf8(&bytes).map_err(|_| unproved())?;
            let mut table = "";
            for line in text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
            {
                if matches!(line, "[package]" | "[workspace]" | "[dependencies]") {
                    table = line;
                    continue;
                }
                let (key, value) = line.split_once('=').ok_or_else(unproved)?;
                let valid = match (table, key.trim(), value.trim()) {
                    ("[package]", "name", value) => {
                        serde_json::from_str::<String>(value).is_ok_and(|name| name == package.name)
                    }
                    ("[package]", "version", value) => serde_json::from_str::<String>(value)
                        .is_ok_and(|version| version == package.version),
                    ("[package]", "edition", "\"2021\"" | "\"2024\"") => true,
                    ("[workspace]", "resolver", "\"2\"" | "\"3\"") => true,
                    ("[workspace]", "members", value) => {
                        serde_json::from_str::<Vec<PathBuf>>(value)
                            .is_ok_and(|members| members.iter().all(|path| relative(path)))
                    }
                    _ => false,
                };
                if !valid {
                    return Err(unproved());
                }
            }
        }
        Ok(())
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
        let otool = xcode_path(cwd, cargo_home, &["--find", "otool"])?;
        let sdk = xcode_path(cwd, cargo_home, &["--show-sdk-path"])?;
        if clang != tools.clang.path
            || ld != tools.ld.path
            || otool != tools.otool.path
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
            &tools.otool,
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
        verify_rust_loader(&otool, cwd, cargo_home, rustc, &tools.rust_runtime)?;
        let linker = format!(
            "CARGO_TARGET_{}_LINKER",
            self.seal
                .native_target
                .replace('-', "_")
                .to_ascii_uppercase()
        );
        let mut linker_flags = OsString::from("-Clink-arg=--ld-path=");
        linker_flags.push(ld.as_os_str());
        let path = self.bind_external(cargo, cwd, cargo_home, &mut remaining, &mut stamps)?;
        environment.insert("PATH".into(), path);
        if self.terminal().is_some() {
            // Foundation can otherwise manufacture this extra environment entry between
            // Cargo and its external subcommand. Seal the conventional native encoding.
            environment.insert(
                "__CF_USER_TEXT_ENCODING".into(),
                format!("0x{:X}:0x0:0x0", cargo_hive_platform::slot::effective_uid()).into(),
            );
        }
        environment.insert("SDKROOT".into(), sdk.into_os_string());
        environment.insert(linker.into(), clang.into_os_string());
        environment.insert("CARGO_ENCODED_RUSTFLAGS".into(), linker_flags);
        digest(&serde_json::to_vec(&stamps).map_err(|_| unproved())?)
    }
    pub fn terminal(&self) -> Option<&Path> {
        match self.seal.recipe {
            Recipe::NativeBuild => None,
            Recipe::Clippy => self
                .seal
                .tools
                .cargo_clippy
                .as_ref()
                .map(|tool| tool.path.as_path()),
            Recipe::NextestWorkspace | Recipe::NextestGateways => self
                .seal
                .tools
                .cargo_nextest
                .as_ref()
                .map(|tool| tool.path.as_path()),
        }
    }

    fn bind_external(
        &self,
        cargo: &Path,
        cwd: &Path,
        cargo_home: &OsStr,
        remaining: &mut u64,
        stamps: &mut Vec<String>,
    ) -> Result<OsString, Refusal> {
        let Some(terminal) = self.terminal() else {
            return Ok("/usr/bin:/bin".into());
        };
        let refused =
            || refused("The sealed Cargo external command or executable lookup is unproved.");
        if std::env::vars_os().any(|(name, _)| {
            let name = name.to_string_lossy();
            matches!(name.as_ref(), "CARGO" | "NEXTEST")
                || name.starts_with("CLIPPY_")
                || name.starts_with("NEXTEST_")
        }) {
            return Err(refused());
        }
        let toolchain_bin = cargo.parent().ok_or_else(refused)?;
        let home_bin = Path::new(cargo_home).join("bin");
        let (name, selected_dir) = if self.seal.recipe.is_nextest() {
            ("cargo-nextest", home_bin.as_path())
        } else {
            ("cargo-clippy", toolchain_bin)
        };
        if selected_dir.join(name) != terminal {
            return Err(refused());
        }
        for dir in [
            home_bin.as_path(),
            toolchain_bin,
            Path::new("/usr/bin"),
            Path::new("/bin"),
        ] {
            if dir != selected_dir
                && !std::fs::symlink_metadata(dir.join(name))
                    .is_err_and(|err| err.kind() == std::io::ErrorKind::NotFound)
            {
                return Err(refused());
            }
        }
        let tools = &self.seal.tools;
        if let Some(driver) = &tools.clippy_driver {
            let launcher = tools.cargo_clippy.as_ref().ok_or_else(refused)?;
            if driver.path != toolchain_bin.join("clippy-driver") {
                return Err(refused());
            }
            for tool in [launcher, driver] {
                stamps.push(tool.verify(remaining)?);
                verify_rust_loader(
                    &tools.otool.path,
                    cwd,
                    cargo_home,
                    &tool.path,
                    &tools.rust_runtime,
                )?;
            }
            std::env::join_paths([toolchain_bin, Path::new("/usr/bin"), Path::new("/bin")])
                .map_err(|_| refused())
        } else {
            let nextest = tools.cargo_nextest.as_ref().ok_or_else(refused)?;
            stamps.push(nextest.verify(remaining)?);
            verify_nextest_loader(&tools.otool.path, cwd, cargo_home, &nextest.path)?;
            let observed =
                native_output(terminal, [OsStr::new("--version")], cwd, cargo_home, 4096)?;
            if observed.split(|byte| *byte == b'\n').next()
                != Some(b"cargo-nextest 0.9.144 (9718c77af 2026-09-10)")
            {
                return Err(refused());
            }
            Ok("/usr/bin:/bin".into())
        }
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

impl Tools {
    fn valid_for(&self, recipe: Recipe) -> bool {
        let clippy = self.cargo_clippy.is_some() && self.clippy_driver.is_some();
        let no_clippy = self.cargo_clippy.is_none() && self.clippy_driver.is_none();
        match recipe {
            Recipe::NativeBuild => no_clippy && self.cargo_nextest.is_none(),
            Recipe::Clippy => clippy && self.cargo_nextest.is_none(),
            Recipe::NextestWorkspace | Recipe::NextestGateways => {
                no_clippy && self.cargo_nextest.is_some()
            }
        }
    }
}

fn verify_nextest_loader(
    otool: &Path,
    cwd: &Path,
    home: &OsStr,
    nextest: &Path,
) -> Result<(), Refusal> {
    let unproved =
        || refused("The installed nextest loader differs from the sealed native runtime.");
    let output = native_output(
        otool,
        [OsStr::new("-L"), nextest.as_os_str()],
        cwd,
        home,
        128 * 1024,
    )?;
    let output = std::str::from_utf8(&output).map_err(|_| unproved())?;
    let mut dependencies = output.lines().skip(1).peekable();
    if dependencies.peek().is_none()
        || !dependencies.all(|line| {
            line.trim()
                .split_once(" (compatibility version ")
                .is_some_and(|(name, _)| {
                    name.starts_with("/usr/lib/") || name.starts_with("/System/Library/")
                })
        })
    {
        return Err(unproved());
    }
    Ok(())
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
    otool: &Path,
    cwd: &Path,
    home: &OsStr,
    rustc: &Path,
    runtime: &[Material],
) -> Result<(), Refusal> {
    let unproved =
        || refused("The installed Rust compiler loader differs from the sealed compiler material.");
    let image = |flag| {
        native_output(
            otool,
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
