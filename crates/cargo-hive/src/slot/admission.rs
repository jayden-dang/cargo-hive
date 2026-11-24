//! Sealed admission for a bounded native Cargo build.

mod checkout;

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
    #[serde(default)]
    metadata_resolution_sha256: Option<String>,
    data: Vec<FiniteData>,
    materialized: Option<MaterializedGraph>,
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize)]
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
    #[serde(default)]
    whole_tree: Option<checkout::TreeManifest>,
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
    #[serde(default)]
    pub metadata: Option<NativeTargetMetadata>,
}

/// Cargo metadata's declared modes do not establish the target's harness.
#[derive(Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct NativeTargetMetadata {
    pub crate_types: Vec<String>,
    pub edition: String,
    pub doc: bool,
    pub doctest: bool,
    pub test: bool,
    #[serde(default)]
    pub required_features: Vec<String>,
}

impl NativeTargetMetadata {
    fn valid(&self, kind: &str) -> bool {
        (match kind {
            "bin" | "test" | "bench" => self.crate_types == ["bin"],
            "proc-macro" => self.crate_types == ["proc-macro"],
            "lib" => {
                bounded_unique(self.crate_types.iter().map(String::as_str))
                    && self.crate_types.iter().all(|value| {
                        matches!(
                            value.as_str(),
                            "lib" | "rlib" | "dylib" | "cdylib" | "staticlib"
                        )
                    })
            }
            _ => false,
        }) && matches!(self.edition.as_str(), "2015" | "2018" | "2021" | "2024")
            && self.required_features.len() <= ENTRY_LIMIT
            && unique(self.required_features.iter().map(String::as_str))
            && self
                .required_features
                .iter()
                .all(|feature| feature.len() <= 128)
    }
}

const PACKAGE_LIMIT: usize = 1024;
const ROOT_ENTRY_LIMIT: usize = 4096;
const DEPENDENCY_ENTRY_LIMIT: usize = 65536;
const DEPENDENCY_BYTES_LIMIT: u64 = 1024 * 1024 * 1024;
const SOURCE_FILE_LIMIT: u64 = 16 * 1024 * 1024;
const SOURCE_DEPTH_LIMIT: usize = 32;
const CRATES_IO: &str = "registry+https://github.com/rust-lang/crates.io-index";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MaterializedGraph {
    pub resolve_sha256: String,
    pub packages: Vec<MaterializedPackage>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MaterializedPackage {
    pub id: String,
    pub name: String,
    pub version: String,
    pub source: String,
    pub root: PathBuf,
    inventory_sha256: String,
    origin: Origin,
    #[serde(default)]
    executables: Vec<ExecutableApproval>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum Origin {
    Registry {
        checksum: String,
    },
    Git {
        revision: String,
        checkout: PathBuf,
        tree_sha256: String,
    },
}

#[derive(Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
enum ExecutableKind {
    CustomBuild,
    ProcMacro,
}

impl ExecutableKind {
    fn cargo_kind(&self) -> &'static str {
        match self {
            Self::CustomBuild => "custom-build",
            Self::ProcMacro => "proc-macro",
        }
    }
}

#[derive(Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
struct ExecutableTarget {
    name: String,
    kind: ExecutableKind,
    source: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutableApproval {
    target: ExecutableTarget,
    review_sha256: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum ReviewedBehavior {
    MetadataOnlyBuildScript,
    SealedInputsTokenTransform,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecutableReview {
    schema: u32,
    profile: ProfileBinding,
    invocation: String,
    recipe: Recipe,
    package_id: String,
    inventory_sha256: String,
    git_checkout_sha256: Option<String>,
    source_sha256: String,
    target: ExecutableTarget,
    behavior: ReviewedBehavior,
}

struct DependencyReviewScope<'a> {
    profile: &'a PoolProfile,
    invocation: &'a str,
    recipe: Recipe,
    home: &'a Path,
    checkout: &'a Path,
}

impl MaterializedGraph {
    fn valid(&self, workspace_packages: usize) -> bool {
        slots::hex(&self.resolve_sha256, 64)
            && !self.packages.is_empty()
            && workspace_packages.saturating_add(self.packages.len()) <= PACKAGE_LIMIT
            && unique(self.packages.iter().map(|entry| entry.id.as_str()))
            && self.packages.iter().all(MaterializedPackage::valid)
            && self
                .packages
                .iter()
                .map(|entry| entry.executables.len())
                .sum::<usize>()
                <= ENTRY_LIMIT
            && self.packages.iter().enumerate().all(|(index, entry)| {
                self.packages[..index].iter().all(|other| {
                    !entry.root.starts_with(&other.root) && !other.root.starts_with(&entry.root)
                })
            })
    }

    fn verify_material(&self, scope: &DependencyReviewScope<'_>) -> Result<String, Refusal> {
        let home = scope.home;
        let checkout = scope.checkout;
        let unproved = dependency_refusal;
        if home.canonicalize().as_deref().ok() != Some(home)
            || home.starts_with(checkout)
            || checkout.starts_with(home)
        {
            return Err(unproved());
        }
        let bytes = read_input_bounded(&checkout.join("Cargo.lock"), slots::MAX_BYTES as usize)?
            .ok_or_else(unproved)?;
        let lock = lock_sources(&bytes)?;
        if lock.len() != self.packages.len() {
            return Err(unproved());
        }
        let fs = host_fs();
        let mut budget = InventoryBudget {
            entries: DEPENDENCY_ENTRY_LIMIT,
            bytes: DEPENDENCY_BYTES_LIMIT,
        };
        let mut witnesses = BTreeMap::new();
        let mut git_checkouts = BTreeMap::<_, GitInventory>::new();
        for package in &self.packages {
            super::check_cancelled()?;
            let path = home.join(&package.root);
            let tuple = (
                package.name.as_str(),
                package.version.as_str(),
                package.source.as_str(),
            );
            let checksum = lock.get(&tuple).ok_or_else(unproved)?;
            let (inventory, runtime, origin_witness) = match &package.origin {
                Origin::Registry { checksum: reviewed } if *checksum == Some(reviewed.as_str()) => {
                    check_registry_marker(&path)?;
                    let index = package
                        .root
                        .parent()
                        .and_then(Path::file_name)
                        .ok_or_else(unproved)?;
                    let archive = home
                        .join("registry/cache")
                        .join(index)
                        .join(format!("{}-{}.crate", package.name, package.version));
                    let (hash, stamp) =
                        dependency_file(fs.as_ref(), &archive, &mut budget, FILE_LIMIT)?;
                    if &hash != reviewed {
                        return Err(unproved());
                    }
                    let tree = scan_inventory(fs.as_ref(), &path, &mut budget, false)?;
                    (
                        tree.content(Path::new(""))?,
                        tree.runtime,
                        digest(&serde_json::to_vec(&stamp).map_err(|_| unproved())?)?,
                    )
                }
                Origin::Git {
                    revision,
                    checkout,
                    tree_sha256,
                } if checksum.is_none() => {
                    let root = home.join(checkout);
                    let git = match git_checkouts.entry(root) {
                        std::collections::btree_map::Entry::Vacant(entry) => {
                            let inventory = GitInventory::read(
                                fs.as_ref(),
                                entry.key(),
                                revision,
                                tree_sha256,
                                &package.source,
                                &mut budget,
                            )?;
                            entry.insert(inventory)
                        }
                        std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
                    };
                    if git.revision != revision
                        || git.reviewed != tree_sha256
                        || git.source != package.source
                    {
                        return Err(unproved());
                    }
                    let subdirectory = package
                        .root
                        .strip_prefix(checkout)
                        .map_err(|_| unproved())?;
                    (
                        git.tree.content(subdirectory)?,
                        git.tree.runtime.clone(),
                        git.admin.clone(),
                    )
                }
                _ => return Err(unproved()),
            };
            if inventory != package.inventory_sha256 {
                return Err(unproved());
            }
            package.verify_reviews(&path, scope)?;
            witnesses.insert(&package.id, (runtime, origin_witness));
        }
        digest(&serde_json::to_vec(&witnesses).map_err(|_| unproved())?)
    }
}

impl MaterializedPackage {
    pub fn matches_executables<'a>(
        &self,
        targets: impl Iterator<Item = (&'a str, &'a [String], &'a Path)>,
    ) -> bool {
        let mut matched = BTreeSet::new();
        targets
            .filter(|(_, kinds, _)| {
                kinds
                    .iter()
                    .any(|kind| matches!(kind.as_str(), "custom-build" | "proc-macro"))
            })
            .all(|(name, kinds, source)| {
                let [kind] = kinds else {
                    return false;
                };
                self.executables
                    .iter()
                    .find(|item| {
                        item.target.name == name
                            && item.target.kind.cargo_kind() == kind
                            && item.target.source == source
                    })
                    .is_some_and(|item| matched.insert(&item.target))
            })
            && matched.len() == self.executables.len()
    }

    fn verify_reviews(
        &self,
        root: &Path,
        scope: &DependencyReviewScope<'_>,
    ) -> Result<(), Refusal> {
        let unproved =
            || refused("A materialized executable is missing its exact sealed behavior review.");
        let git_checkout_sha256 = match &self.origin {
            Origin::Registry { .. } => None,
            Origin::Git { tree_sha256, .. } => Some(tree_sha256.as_str()),
        };
        for approval in &self.executables {
            super::check_cancelled()?;
            let mut references = scope.profile.graph_inputs.iter().filter_map(|input| {
                match (&input.locator, &input.expectation) {
                    (
                        Locator::External { path },
                        GraphExpectation::Present {
                            identity: InputIdentity::ReviewedBytes { sha256 },
                        },
                    ) if sha256 == &approval.review_sha256 => Some(path),
                    _ => None,
                }
            });
            let path = references.next().ok_or_else(unproved)?;
            if references.next().is_some()
                || path.starts_with(scope.home)
                || path.starts_with(scope.checkout)
            {
                return Err(unproved());
            }
            let bytes = read_input_bounded(path, 64 * 1024)?.ok_or_else(unproved)?;
            if digest(&bytes)? != approval.review_sha256 {
                return Err(unproved());
            }
            let review: ExecutableReview =
                serde_json::from_slice(&bytes).map_err(|_| unproved())?;
            let source = read_input_bounded(
                &root.join(&approval.target.source),
                SOURCE_FILE_LIMIT as usize,
            )?
            .ok_or_else(unproved)?;
            if review.schema != 1
                || review.profile.id != scope.profile.id
                || review.profile.revision != scope.profile.revision
                || review.invocation != scope.invocation
                || review.recipe != scope.recipe
                || review.package_id != self.id
                || review.inventory_sha256 != self.inventory_sha256
                || review.git_checkout_sha256.as_deref() != git_checkout_sha256
                || review.target != approval.target
                || review.source_sha256 != digest(&source)?
                || !matches!(
                    (&review.target.kind, review.behavior),
                    (
                        ExecutableKind::CustomBuild,
                        ReviewedBehavior::MetadataOnlyBuildScript
                    ) | (
                        ExecutableKind::ProcMacro,
                        ReviewedBehavior::SealedInputsTokenTransform
                    )
                )
            {
                return Err(unproved());
            }
        }
        Ok(())
    }

    fn valid(&self) -> bool {
        if !relative(&self.root)
            || !slots::hex(&self.inventory_sha256, 64)
            || !portable_leaf(&self.name)
            || !portable_leaf(&self.version)
            || self.id.is_empty()
            || self.id.len() > 2048
            || self.source.len() > 1024
        {
            return false;
        }
        if self.executables.len() > 16
            || self.executables.iter().any(|item| {
                !portable_leaf(&item.target.name)
                    || !relative(&item.target.source)
                    || !slots::hex(&item.review_sha256, 64)
            })
            || self
                .executables
                .iter()
                .map(|item| &item.target)
                .collect::<BTreeSet<_>>()
                .len()
                != self.executables.len()
        {
            return false;
        }
        let components: Vec<_> = self.root.iter().collect();
        match (&self.origin, components.as_slice()) {
            (Origin::Registry { checksum }, [registry, src, index, leaf]) => {
                self.source == CRATES_IO
                    && *registry == "registry"
                    && *src == "src"
                    && index
                        .to_str()
                        .and_then(|index| index.strip_prefix("index.crates.io-"))
                        .is_some_and(|hash| slots::hex(hash, 16))
                    && *leaf == format!("{}-{}", self.name, self.version).as_str()
                    && slots::hex(checksum, 64)
            }
            (
                Origin::Git {
                    revision,
                    checkout,
                    tree_sha256,
                },
                [git, checkouts, key, short, ..],
            ) => {
                slots::hex(tree_sha256, 64)
                    && (*git == "git" && *checkouts == "checkouts")
                    && (self.source.starts_with("git+https://")
                        || self.source.starts_with("git+file://"))
                    && key.to_str().is_some_and(portable_leaf)
                    && short
                        .to_str()
                        .is_some_and(|short| short.len() >= 7 && revision.starts_with(short))
                    && checkout.as_path() == Path::new(git).join(checkouts).join(key).join(short)
                    && self.root.starts_with(checkout)
                    && relative(checkout)
                    && slots::object_id(revision)
                    && self
                        .source
                        .rsplit_once('#')
                        .is_some_and(|(_, commit)| commit == revision)
            }
            _ => false,
        }
    }
}

fn portable_leaf(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !matches!(value, "." | "..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-.+".contains(&byte))
}

fn dependency_refusal() -> Refusal {
    refused(
        "The selected registry or Git dependency material differs from the sealed inventory or its bounds.",
    )
}

type LockSources<'a> = BTreeMap<(&'a str, &'a str, &'a str), Option<&'a str>>;

fn lock_sources(bytes: &[u8]) -> Result<LockSources<'_>, Refusal> {
    let unproved = dependency_refusal;
    let text = std::str::from_utf8(bytes).map_err(|_| unproved())?;
    let mut packages = text.split("\n[[package]]\n");
    let header = packages.next().ok_or_else(unproved)?;
    if !header
        .lines()
        .any(|row| matches!(row, "version = 3" | "version = 4"))
    {
        return Err(unproved());
    }
    let mut sources = BTreeMap::new();
    for package in packages {
        let mut fields = BTreeMap::new();
        for line in package.lines() {
            let Some((name, value)) = line.split_once(" = ") else {
                continue;
            };
            if matches!(name, "name" | "version" | "source" | "checksum") {
                let value = value
                    .strip_prefix('"')
                    .and_then(|value| value.strip_suffix('"'))
                    .ok_or_else(unproved)?;
                if value.contains(['"', '\\']) || fields.insert(name, value).is_some() {
                    return Err(unproved());
                }
            }
        }
        if let Some(source) = fields.get("source") {
            let name = *fields.get("name").ok_or_else(unproved)?;
            let version = *fields.get("version").ok_or_else(unproved)?;
            if sources
                .insert((name, version, *source), fields.get("checksum").copied())
                .is_some()
            {
                return Err(unproved());
            }
        }
    }
    Ok(sources)
}

struct InventoryBudget {
    entries: usize,
    bytes: u64,
}

type SourceStamp = ((slots::FileIdentity, i64, i64, i64, i64), u64, u32);

fn dependency_file(
    fs: &dyn cargo_hive_platform::Fs,
    path: &Path,
    budget: &mut InventoryBudget,
    limit: u64,
) -> Result<(String, SourceStamp), Refusal> {
    let unproved = dependency_refusal;
    if path.canonicalize().as_deref().ok() != Some(path) {
        return Err(unproved());
    }
    let mut file = fs.open_regular(path).map_err(|_| unproved())?;
    let metadata = file.metadata().map_err(|_| unproved())?;
    if metadata.len() > limit
        || metadata.mode() & 0o7022 != 0
        || metadata.uid() != cargo_hive_platform::slot::effective_uid()
        || metadata.nlink() != 1
    {
        return Err(unproved());
    }
    budget.bytes = budget
        .bytes
        .checked_sub(metadata.len())
        .ok_or_else(unproved)?;
    let stamp = |meta: &std::fs::Metadata| (directory_stamp(meta), meta.len(), meta.mode());
    let before = stamp(&metadata);
    let hash = sha256_reader(&mut file, metadata.len(), cancelled).map_err(|err| {
        if err.kind() == std::io::ErrorKind::Interrupted && cancelled() {
            cargo_hive_state::StateError::Cancelled.into()
        } else {
            unproved()
        }
    })?;
    let after = std::fs::symlink_metadata(path).map_err(|_| unproved())?;
    if !after.is_file()
        || before != stamp(&after)
        || before != stamp(&file.metadata().map_err(|_| unproved())?)
    {
        return Err(unproved());
    }
    Ok((
        hash.iter().map(|byte| format!("{byte:02x}")).collect(),
        before,
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryMarker {
    v: u32,
}

fn check_registry_marker(root: &Path) -> Result<(), Refusal> {
    let unproved =
        || refused("The sealed registry archive has no verified completed extraction marker.");
    let bytes = read_input_bounded(&root.join(".cargo-ok"), 4096)?.ok_or_else(unproved)?;
    let marker: RegistryMarker = serde_json::from_slice(&bytes).map_err(|_| unproved())?;
    if marker.v != 1 {
        return Err(unproved());
    }
    Ok(())
}

struct SourceInventory {
    rows: Vec<(PathBuf, u64, Option<String>)>,
    runtime: String,
}

impl SourceInventory {
    fn content(&self, subdirectory: &Path) -> Result<String, Refusal> {
        let unproved = dependency_refusal;
        if !subdirectory.as_os_str().is_empty()
            && !self
                .rows
                .iter()
                .any(|(path, _, hash)| path == subdirectory && hash.is_none())
        {
            return Err(unproved());
        }
        let rows: Vec<_> = self
            .rows
            .iter()
            .filter_map(|(path, length, hash)| {
                let relative = path.strip_prefix(subdirectory).ok()?;
                (!relative.as_os_str().is_empty()).then_some((relative, length, hash))
            })
            .collect();
        digest(&serde_json::to_vec(&rows).map_err(|_| unproved())?)
    }
}

struct GitInventory<'a> {
    revision: &'a str,
    reviewed: &'a str,
    source: &'a str,
    tree: SourceInventory,
    admin: String,
}

impl<'a> GitInventory<'a> {
    fn read(
        fs: &dyn cargo_hive_platform::Fs,
        root: &Path,
        revision: &'a str,
        reviewed: &'a str,
        source: &'a str,
        budget: &mut InventoryBudget,
    ) -> Result<Self, Refusal> {
        let unproved = dependency_refusal;
        if root.canonicalize().as_deref().ok() != Some(root) {
            return Err(unproved());
        }
        let admin = root.join(".git");
        let stamp = || {
            slots::owner_directory(&admin)
                .map(|metadata| directory_stamp(&metadata))
                .map_err(|_| unproved())
        };
        let before = stamp()?;
        let head = || -> Result<(), Refusal> {
            let bytes = super::prepare::git_write_scope(
                root,
                &["rev-parse", "--verify", "HEAD^{commit}"],
                128,
            )?;
            if bytes.strip_suffix(b"\n") != Some(revision.as_bytes()) {
                return Err(unproved());
            }
            Ok(())
        };
        head()?;
        let tree = scan_inventory(fs, root, budget, true)?;
        if tree.content(Path::new(""))? != reviewed || stamp()? != before {
            return Err(unproved());
        }
        head()?;
        let admin = digest(&serde_json::to_vec(&(root, before)).map_err(|_| unproved())?)?;
        Ok(Self {
            revision,
            reviewed,
            source,
            tree,
            admin,
        })
    }
}

fn scan_inventory(
    fs: &dyn cargo_hive_platform::Fs,
    root: &Path,
    budget: &mut InventoryBudget,
    exclude_git: bool,
) -> Result<SourceInventory, Refusal> {
    let unproved = dependency_refusal;
    let mut rows = Vec::new();
    let mut snapshots = BTreeMap::new();
    let mut stack = vec![(root.to_path_buf(), 0)];
    let mut left = ROOT_ENTRY_LIMIT;
    while let Some((directory, depth)) = stack.pop() {
        super::check_cancelled()?;
        if depth > SOURCE_DEPTH_LIMIT || directory.canonicalize().as_ref().ok() != Some(&directory)
        {
            return Err(unproved());
        }
        let before = slots::owner_directory(&directory).map_err(|_| unproved())?;
        let entries = fs
            .read_dir_bounded(&directory, left.min(budget.entries))
            .map_err(|_| unproved())?;
        left = left.checked_sub(entries.len()).ok_or_else(unproved)?;
        budget.entries = budget
            .entries
            .checked_sub(entries.len())
            .ok_or_else(unproved)?;
        for (name, kind) in entries {
            if name.is_empty() || matches!(name.as_str(), "." | "..") || name.contains('/') {
                return Err(unproved());
            }
            let path = directory.join(&name);
            let relative = path
                .strip_prefix(root)
                .map_err(|_| unproved())?
                .to_path_buf();
            if name == ".git" {
                if exclude_git && depth == 0 && kind == EntryKind::Dir {
                    continue;
                }
                return Err(unproved());
            }
            match kind {
                EntryKind::Dir => {
                    rows.push((relative, 0, None));
                    stack.push((path, depth + 1));
                }
                EntryKind::File => {
                    let (hash, stamp) = dependency_file(fs, &path, budget, SOURCE_FILE_LIMIT)?;
                    rows.push((relative.clone(), stamp.1, Some(hash)));
                    snapshots.insert(relative, stamp);
                }
                _ => return Err(unproved()),
            }
        }
        let after = slots::owner_directory(&directory).map_err(|_| unproved())?;
        if directory_stamp(&before) != directory_stamp(&after) {
            return Err(unproved());
        }
        snapshots.insert(
            directory
                .strip_prefix(root)
                .map_err(|_| unproved())?
                .to_path_buf(),
            (directory_stamp(&after), 0, after.mode()),
        );
    }
    for (relative, expected) in &snapshots {
        super::check_cancelled()?;
        let path = root.join(relative);
        let metadata = std::fs::symlink_metadata(&path).map_err(|_| unproved())?;
        if !metadata.is_file() && !metadata.is_dir()
            || (
                directory_stamp(&metadata),
                if metadata.is_dir() { 0 } else { metadata.len() },
                metadata.mode(),
            ) != *expected
        {
            return Err(unproved());
        }
    }
    rows.sort();
    Ok(SourceInventory {
        rows,
        runtime: digest(&serde_json::to_vec(&snapshots).map_err(|_| unproved())?)?,
    })
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

impl Seal {
    fn valid_local_target_catalog(&self) -> bool {
        let mut targets = self.packages.iter().flat_map(|package| &package.targets);
        match &self.metadata_resolution_sha256 {
            Some(expected) => {
                slots::hex(expected, 64)
                    && self.recipe != Recipe::NativeBuild
                    && self
                        .endpoints
                        .iter()
                        .all(|endpoint| endpoint.whole_tree.is_some())
                    && self
                        .materialized
                        .as_ref()
                        .is_none_or(|graph| &graph.resolve_sha256 == expected)
                    && targets.all(|target| target.metadata.is_some())
            }
            None => targets.all(|target| target.metadata.is_none()),
        }
    }
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
                slots::object_id(&endpoint.commit)
                    && slots::object_id(&endpoint.tree)
                    && endpoint
                        .whole_tree
                        .as_ref()
                        .is_none_or(checkout::TreeManifest::valid)
            })
            || !bounded_unique(
                seal.endpoints
                    .iter()
                    .map(|endpoint| endpoint.commit.as_str()),
            )
            || !bounded_unique(seal.packages.iter().map(|item| item.name.as_str()))
            || seal
                .materialized
                .as_ref()
                .is_some_and(|graph| !graph.valid(seal.packages.len()))
            || !seal.packages.iter().all(valid_package)
            || !seal.valid_local_target_catalog()
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

    pub fn verify_dependency_material(
        &self,
        profile: &PoolProfile,
        home: &Path,
        checkout: &Path,
    ) -> Result<Option<String>, Refusal> {
        let scope = DependencyReviewScope {
            profile,
            invocation: &self.seal.invocation,
            recipe: self.seal.recipe,
            home,
            checkout,
        };
        self.materialized()
            .map(|material| material.verify_material(&scope))
            .transpose()
    }

    pub fn materialized(&self) -> Option<&MaterializedGraph> {
        self.seal.materialized.as_ref()
    }

    pub fn packages(&self) -> &[NativePackage] {
        &self.seal.packages
    }

    /// Binds the all-feature metadata probe. Exact compile and launch selections are separate.
    pub fn metadata_resolution(&self) -> Option<&str> {
        self.seal.metadata_resolution_sha256.as_deref()
    }

    pub fn require_supported_target_mode(&self) -> Result<(), Refusal> {
        if self.metadata_resolution().is_some() {
            return Err(refused(if self.seal.recipe.is_nextest() {
                "The reviewed Cargo target catalog lacks exact Nextest selection, harness and child proof."
            } else {
                "The reviewed Cargo target catalog lacks exact Clippy compiler-unit selection proof."
            }));
        }
        Ok(())
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
        if let Some(manifest) = &endpoint.whole_tree {
            checkout::verify_whole_tree(&checkout.path, &checkout.head, manifest, &observed)?;
            return Ok(observed);
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
    if !relative(&package.manifest)
        || !(1..=ENTRY_LIMIT).contains(&package.targets.len())
        || package
            .targets
            .iter()
            .any(|target| !relative(&target.source))
        || package.dependencies.len() > ENTRY_LIMIT
        || !unique(package.dependencies.iter().map(String::as_str))
    {
        return false;
    }
    if package
        .targets
        .iter()
        .any(|target| target.metadata.is_some())
    {
        let mut identities = BTreeSet::new();
        package.targets.iter().all(|target| {
            portable_leaf(&target.name)
                && identities.insert((&target.name, &target.kind))
                && target
                    .metadata
                    .as_ref()
                    .is_some_and(|metadata| metadata.valid(&target.kind))
        })
    } else {
        bounded_unique(package.targets.iter().map(|target| target.name.as_str()))
            && package
                .targets
                .iter()
                .all(|target| matches!(target.kind.as_str(), "bin" | "lib" | "proc-macro"))
    }
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

#[cfg(test)]
mod tests {
    use super::{Endpoint, NativeBuild, NativePackage, Seal, valid_package};
    use serde_json::json;

    #[test]
    fn a_sealed_endpoint_can_name_reviewed_whole_tree_contents() {
        let endpoint = json!({
            "commit": "a".repeat(40),
            "tree": "b".repeat(40),
            "whole_tree": {
                "entries": 9222,
                "sha256": "c".repeat(64),
            },
        });

        if let Err(error) = serde_json::from_value::<Endpoint>(endpoint) {
            panic!("the sealed endpoint refused its reviewed whole-tree inventory: {error}");
        }
    }

    #[test]
    fn a_reviewed_target_inventory_distinguishes_integration_tests_and_benches() {
        let target = |kind, source, test| {
            json!({
                "name": "exercise",
                "kind": kind,
                "source": source,
                "metadata": {
                    "crate_types": ["bin"],
                    "edition": "2021",
                    "doc": false,
                    "doctest": false,
                    "test": test,
                    "required_features": ["testkit"],
                },
            })
        };
        let mut package = json!({
            "name": "gateways",
            "version": "1.0.0",
            "manifest": "Cargo.toml",
            "targets": [
                target("test", "tests/exercise.rs", true),
                target("bench", "benches/exercise.rs", false),
            ],
            "dependencies": [],
        });
        let reviewed: NativePackage = serde_json::from_value(package.clone())
            .unwrap_or_else(|error| panic!("reviewed target inventory refused: {error}"));
        assert!(valid_package(&reviewed));

        package["targets"][1]
            .as_object_mut()
            .unwrap()
            .remove("metadata");
        let incomplete: NativePackage = serde_json::from_value(package).unwrap();
        assert!(!valid_package(&incomplete));
    }

    #[test]
    fn metadata_catalogs_carry_no_clippy_or_nextest_launch_authority() {
        let tool = json!({ "path": "/reviewed/tool", "sha256": "a".repeat(64) });
        for recipe in ["clippy", "nextest_workspace", "nextest_gateways"] {
            let seal: Seal = serde_json::from_value(json!({
                "schema": 1,
                "recipe": recipe,
                "profile": {"id": "reviewed", "revision": 1},
                "proof": "test-catalog",
                "invocation": recipe,
                "layout": "flat",
                "native_target": "aarch64-apple-darwin",
                "endpoints": [{
                    "commit": "b".repeat(40),
                    "tree": "c".repeat(40),
                    "whole_tree": {"entries": 1, "sha256": "d".repeat(64)},
                }],
                "tools": {
                    "cargo": tool,
                    "rustc": tool,
                    "rust_runtime": [],
                    "target_sysroot": [],
                    "clang": tool,
                    "ld": tool,
                    "otool": tool,
                    "sdk": "/reviewed/sdk",
                    "sdk_settings": tool,
                    "system_library": tool,
                },
                "packages": [{
                    "name": "gateways",
                    "version": "1.0.0",
                    "manifest": "Cargo.toml",
                    "targets": [{
                        "name": "it",
                        "kind": "test",
                        "source": "tests/it.rs",
                        "metadata": {
                            "crate_types": ["bin"],
                            "edition": "2021",
                            "doc": false,
                            "doctest": false,
                            "test": true,
                        },
                    }],
                    "dependencies": [],
                }],
                "metadata_resolution_sha256": "e".repeat(64),
                "data": [],
            }))
            .unwrap();
            assert!(seal.valid_local_target_catalog());
            let candidate = NativeBuild {
                seal,
                seal_digest: "f".repeat(64),
            };
            assert!(candidate.require_supported_target_mode().is_err());
        }
    }
}
