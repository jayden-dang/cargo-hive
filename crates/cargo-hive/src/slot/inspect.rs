//! Verifies a slot's checkout identity and observed HEAD around a bounded, read-only
//! inventory. A clean proof retains any verified output locks through publication.

use super::{
    GIT_OUTPUT_LIMIT, MAX_ROOT_DEPTH, MAX_ROOT_ENTRIES, Refusal, accepted_root,
    canonical_worktree_path, check_cancelled, check_nested_checkouts, exists, git_scope, overlaps,
    owner_worktree_inventory, verify_no_checkout_ancestor,
};
use cargo_hive_cargo::registry::Registry;
use cargo_hive_core::classify::{CheckoutFacts, CheckoutKind, Class, classify};
use cargo_hive_platform::flock::open_owner_state_file;
use cargo_hive_platform::{EntryKind, Fs, HeldLock, TryLock, host_fs};
use cargo_hive_state::slots::{
    self, CheckoutIdentity, EmptyEpoch, FileIdentity, Locator, OutputRoots, Pool, PoolProfile,
    Producer, RootExpectation, RootProof, Slot, VerifiedRoot,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, Metadata};
use std::io::{ErrorKind, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

const TRACKED_LIMIT: usize = 4 * 1024 * 1024;
const CHECKOUT_ENTRIES: usize = 100_000;
const UNFINISHED_GIT: &str =
    "The slot contains tracked or index changes or an unfinished Git operation.";
const UNFINISHED_FILES: &str =
    "The slot contains untracked or ignored material outside verified generated output.";
const DISGUISED_OUTPUT: &str =
    "The retained output contains a symlink, nested checkout, or possible input.";
const UNCERTAIN_INVENTORY: &str = "Could not establish a complete checkout inventory. Check the index, filesystem and Git operation state.";
const UNCERTAIN_ROOT: &str =
    "Could not prove the retained output root identity and its exclusive generated use.";
const UNCERTAIN_PRODUCER: &str =
    "A prior writer may still change the slot, or its retained output provenance is unproved.";

pub(super) struct SlotScope<'a> {
    pub pool: &'a Pool,
    pub slot: &'a Slot,
    pub registry: &'a Registry,
    pub secondary_roots: &'a [PathBuf],
}

pub(super) struct ExpectedCheckout<'a> {
    pub identity: &'a CheckoutIdentity,
    pub head: Option<&'a str>,
}

pub(crate) struct CheckoutProof {
    pub path: PathBuf,
    pub head: String,
    pub(super) inventory: CheckoutInventory,
    output_locks: Vec<HeldOutputLock>,
}

struct HeldOutputLock {
    path: PathBuf,
    lock: HeldLock,
}

impl CheckoutProof {
    pub(crate) fn release_output_locks(&mut self) {
        self.output_locks.clear();
    }

    pub(super) fn verify_generated_inputs(
        &self,
        profile: &PoolProfile,
        producer: &Producer,
    ) -> Result<(), Refusal> {
        super::generated::verify_retained(profile, producer, |build_root| {
            let mut locks = self
                .output_locks
                .iter()
                .filter(|held| held.path.starts_with(build_root))
                .peekable();
            locks.peek().is_some()
                && locks.all(|held| {
                    fs::symlink_metadata(&held.path).is_ok_and(|metadata| {
                        metadata.is_file()
                            && held.lock.identity() == Some((metadata.dev(), metadata.ino()))
                    })
                })
        })
    }
}

pub(super) enum CheckoutInventory {
    Clean,
    Dirty(&'static str),
    Uncertain(&'static str),
}

pub(super) fn inspect_checkout(
    scope: &SlotScope<'_>,
    expected: &ExpectedCheckout<'_>,
) -> Result<CheckoutProof, Refusal> {
    inspect_with_policy(scope, expected, None)
}

pub(super) struct ProducerInventory<'a> {
    pub roots: &'a OutputRoots<RootExpectation>,
    pub initial_empty: Option<&'a EmptyEpoch>,
    pub mutable: &'a BTreeSet<PathBuf>,
}

pub(super) fn inspect_producer_checkout(
    scope: &SlotScope<'_>,
    expected: &ExpectedCheckout<'_>,
    policy: &ProducerInventory<'_>,
) -> Result<CheckoutProof, Refusal> {
    inspect_with_policy(scope, expected, Some(policy))
}

fn inspect_with_policy(
    scope: &SlotScope<'_>,
    expected: &ExpectedCheckout<'_>,
    policy: Option<&ProducerInventory<'_>>,
) -> Result<CheckoutProof, Refusal> {
    let pool = scope.pool;
    let path = pool.checkout_path(scope.slot);
    let identity = expected.identity;
    if scope.slot.checkout_id.as_ref() != Some(identity) {
        return Err(checkout_refusal());
    }
    let head = verify_checkout_scope(scope, &path, expected)?;
    let (inventory, output_locks) = match inventory(scope, &path, identity, head.len(), policy) {
        Ok(locks) => (CheckoutInventory::Clean, locks),
        Err(Refusal::ReturnWork(detail)) => (CheckoutInventory::Dirty(detail), Vec::new()),
        Err(Refusal::ReturnUncertain(detail)) => (CheckoutInventory::Uncertain(detail), Vec::new()),
        Err(err) => return Err(err),
    };
    let proof = CheckoutProof {
        path,
        head,
        inventory,
        output_locks,
    };
    if matches!(proof.inventory, CheckoutInventory::Clean) {
        proof.verify_generated_inputs(&scope.pool.profile, &scope.slot.producer)?;
    }
    verify_linked_head(&proof.path, identity, &pool.common_git_id.path, &proof.head)?;
    Ok(proof)
}

fn checkout_refusal() -> Refusal {
    Refusal::ReturnUncertain(
        "Could not verify the exact slot checkout, linked Git admin identity or current HEAD.",
    )
}

pub(super) fn owned_identity(path: &Path) -> Option<FileIdentity> {
    slots::owner_directory(path)
        .ok()
        .as_ref()
        .map(FileIdentity::of)
}

pub(super) fn verify_checkout_scope(
    scope: &SlotScope<'_>,
    path: &Path,
    expected: &ExpectedCheckout<'_>,
) -> Result<String, Refusal> {
    let pool = scope.pool;
    let mains: Vec<_> = scope
        .registry
        .repos
        .iter()
        .map(|repo| repo.main.clone())
        .collect();
    let facts = CheckoutFacts {
        realpath: Ok(path.to_path_buf()),
        kind: CheckoutKind::Linked {
            main: pool.main.clone(),
        },
        git: Ok(()),
    };
    let identity = expected.identity;
    if !accepted_root(&pool.root, scope.secondary_roots)
        || !linked_checkout_identity_matches(path, identity, &pool.common_git_id.path)
        || classify(&facts, &mains, scope.secondary_roots).0 != Class::Secondary
    {
        return Err(checkout_refusal());
    }
    verify_no_checkout_ancestor(&pool.root)?;
    let records = owner_worktree_inventory(&pool.root, scope.registry, Some(pool))?;
    let mut checked_paths = Vec::new();
    for slot in &pool.slots {
        let sibling = pool.checkout_path(slot);
        if let Some(checkout) = &slot.checkout_id {
            if !linked_checkout_identity_matches(&sibling, checkout, &pool.common_git_id.path)
                || !records.iter().any(|record| record.path == sibling)
            {
                return Err(checkout_refusal());
            }
            checked_paths.push(sibling);
        } else if exists(&sibling).map_err(|_| checkout_refusal())? {
            return Err(checkout_refusal());
        }
    }
    let mut remaining = MAX_ROOT_ENTRIES;
    check_nested_checkouts(&pool.root, 0, &mut remaining, &checked_paths)?;
    let admin = &identity.admin.path;
    let facts = git_scope(
        path,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-dir",
            "--git-common-dir",
            "--show-superproject-working-tree",
        ],
    )?;
    let resolved = [path, admin, &pool.common_git_id.path]
        .into_iter()
        .flat_map(|arg| {
            arg.as_os_str()
                .as_encoded_bytes()
                .iter()
                .copied()
                .chain(*b"\n")
        })
        .collect::<Vec<_>>();
    if facts != resolved {
        return Err(checkout_refusal());
    }
    let head = observed_head(path)?;
    if expected.head.is_some_and(|expected| expected != head)
        || !records
            .iter()
            .any(|record| record.path == path && record.head.as_ref() == Some(&head))
    {
        return Err(checkout_refusal());
    }
    Ok(head)
}

fn linked_checkout_identity_matches(
    path: &Path,
    identity: &CheckoutIdentity,
    common: &Path,
) -> bool {
    owned_identity(path) == Some(identity.directory)
        && owned_identity(&identity.admin.path) == Some(identity.admin.file_id())
        && linked_git_files(path, identity, common)
}

fn linked_git_files(path: &Path, identity: &CheckoutIdentity, common: &Path) -> bool {
    let admin = &identity.admin.path;
    let gitfile = path.join(".git");
    let matches_file = |file: &Path, prefix: &[u8], value: &Path| {
        let expected = [prefix, value.as_os_str().as_encoded_bytes(), b"\n"].concat();
        let mut actual = Vec::new();
        open_owner_state_file(file)
            .and_then(|file| {
                file.take(expected.len() as u64 + 1)
                    .read_to_end(&mut actual)
            })
            .is_ok()
            && actual == expected
    };
    matches_file(&gitfile, b"gitdir: ", admin)
        && matches_file(&admin.join("gitdir"), b"", &gitfile)
        && (matches_file(&admin.join("commondir"), b"", Path::new("../.."))
            || matches_file(&admin.join("commondir"), b"", common))
}

fn observed_head(path: &Path) -> Result<String, Refusal> {
    let bytes = git_scope(path, &["rev-parse", "--verify", "HEAD^{commit}"])?;
    let head = bytes
        .strip_suffix(b"\n")
        .and_then(|bytes| std::str::from_utf8(bytes).ok())
        .filter(|head| slots::object_id(head))
        .ok_or_else(checkout_refusal)?;
    Ok(head.into())
}

pub(super) fn verify_linked_head(
    path: &Path,
    identity: &CheckoutIdentity,
    common: &Path,
    commit: &str,
) -> Result<(), Refusal> {
    if !linked_checkout_identity_matches(path, identity, common) || observed_head(path)? != commit {
        return Err(checkout_refusal());
    }
    Ok(())
}

pub(super) fn inspect_fresh_checkout(
    checkout: &Path,
    common: &Path,
    commit: &str,
) -> Result<CheckoutIdentity, Refusal> {
    let identity = super::new_checkout_identity(checkout, common)?;
    verify_linked_head(checkout, &identity, common, commit)?;
    reject_incomplete_operations(&identity.admin.path)?;
    let tracked = tracked_paths(checkout, commit.len())?;
    let fs = host_fs();
    inspect_inventory(
        checkout,
        &identity,
        &tracked,
        BTreeMap::new(),
        None,
        fs.as_ref(),
    )?;
    verify_linked_head(checkout, &identity, common, commit)?;
    Ok(identity)
}

fn inventory(
    scope: &SlotScope<'_>,
    checkout: &Path,
    identity: &CheckoutIdentity,
    oid_len: usize,
    policy: Option<&ProducerInventory<'_>>,
) -> Result<Vec<HeldOutputLock>, Refusal> {
    reject_incomplete_operations(&identity.admin.path)?;
    let tracked = tracked_paths(checkout, oid_len)?;
    let fs = host_fs();
    let retained = retained_roots(scope, checkout, &tracked, fs.as_ref(), policy)?;
    inspect_inventory(
        checkout,
        identity,
        &tracked,
        retained,
        policy.map(|policy| policy.mutable),
        fs.as_ref(),
    )
}

fn inspect_inventory(
    checkout: &Path,
    identity: &CheckoutIdentity,
    tracked: &BTreeMap<PathBuf, EntryKind>,
    retained: BTreeMap<PathBuf, FileIdentity>,
    mutable: Option<&BTreeSet<PathBuf>>,
    fs: &dyn Fs,
) -> Result<Vec<HeldOutputLock>, Refusal> {
    let mut directories = BTreeSet::new();
    for path in tracked
        .keys()
        .chain(retained.keys())
        .chain(mutable.into_iter().flatten())
    {
        directories.extend(path.ancestors().skip(1).map(Path::to_path_buf));
    }
    let mut walk = CheckoutWalk {
        fs,
        checkout,
        device: identity.directory.dev,
        tracked,
        mutable,
        directories,
        retained,
        remaining: CHECKOUT_ENTRIES,
        linked_outputs: BTreeMap::new(),
        output_locks: Vec::new(),
    };
    walk.visit(checkout, 0, false)?;
    if walk
        .linked_outputs
        .values()
        .any(|(expected, found)| expected != found)
    {
        return Err(Refusal::ReturnUncertain(UNCERTAIN_ROOT));
    }
    reject_status_filters(checkout)?;
    let status = inventory_git(
        checkout,
        &[
            "-c",
            "core.filemode=true",
            "-c",
            "core.trustctime=true",
            "-c",
            "core.checkStat=default",
            "status",
            "--porcelain=v2",
            "-z",
            "--no-branch",
            "--no-show-stash",
            "--no-renames",
            "--untracked-files=no",
            "--ignore-submodules=none",
        ],
        GIT_OUTPUT_LIMIT,
    )?;
    if !status.is_empty()
        && !mutable.is_some_and(|paths| reviewed_worktree_edits(&status, checkout, paths))
    {
        return Err(Refusal::ReturnWork(UNFINISHED_GIT));
    }
    reject_incomplete_operations(&identity.admin.path)?;
    Ok(walk.output_locks)
}

fn reviewed_worktree_edits(status: &[u8], checkout: &Path, paths: &BTreeSet<PathBuf>) -> bool {
    nul_records(status).is_ok_and(|rows| {
        rows.into_iter().all(|row| {
            let Ok(row) = std::str::from_utf8(row) else {
                return false;
            };
            let fields = row.splitn(9, ' ').collect::<Vec<_>>();
            matches!(
                fields.as_slice(),
                ["1", ".M" | ".D", "N...", _, _, _, _, _, path]
                    if paths.contains(&checkout.join(path))
            )
        })
    })
}

fn reject_incomplete_operations(admin: &Path) -> Result<(), Refusal> {
    for marker in [
        "MERGE_HEAD",
        "MERGE_AUTOSTASH",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "REBASE_HEAD",
        "BISECT_START",
        "BISECT_LOG",
        "AUTO_MERGE",
        "rebase-apply",
        "rebase-merge",
        "sequencer",
    ] {
        if exists(&admin.join(marker)).map_err(|_| Refusal::ReturnUncertain(UNCERTAIN_INVENTORY))? {
            return Err(Refusal::ReturnWork(UNFINISHED_GIT));
        }
    }
    for marker in ["index.lock", "HEAD.lock"] {
        if exists(&admin.join(marker)).map_err(|_| Refusal::ReturnUncertain(UNCERTAIN_INVENTORY))? {
            return Err(Refusal::ReturnUncertain(UNCERTAIN_PRODUCER));
        }
    }
    Ok(())
}

fn inventory_git(checkout: &Path, args: &[&str], limit: usize) -> Result<Vec<u8>, Refusal> {
    use cargo_hive_cargo::git::{Git, SlotGitError};
    Git::at(checkout)
        .slot_read_only(args, limit)
        .map_err(|err| match err {
            SlotGitError::Cancelled => cargo_hive_state::StateError::Cancelled.into(),
            SlotGitError::Incomplete => Refusal::ReturnUncertain(UNCERTAIN_INVENTORY),
        })
}

fn nul_records(bytes: &[u8]) -> Result<impl Iterator<Item = &[u8]>, Refusal> {
    let body = bytes
        .strip_suffix(b"\0")
        .filter(|body| !body.is_empty())
        .ok_or(Refusal::ReturnUncertain(UNCERTAIN_INVENTORY))?;
    Ok(body.split(|byte| *byte == 0))
}

fn tracked_paths(checkout: &Path, oid_len: usize) -> Result<BTreeMap<PathBuf, EntryKind>, Refusal> {
    let bytes = inventory_git(
        checkout,
        &["ls-files", "--stage", "-v", "-z"],
        TRACKED_LIMIT,
    )?;
    let mut tracked = BTreeMap::new();
    if bytes.is_empty() {
        return Ok(tracked);
    }
    for record in nul_records(&bytes)? {
        let record = std::str::from_utf8(record)
            .map_err(|_| Refusal::ReturnUncertain(UNCERTAIN_INVENTORY))?;
        let (attributes, path) = record
            .split_once('\t')
            .ok_or(Refusal::ReturnUncertain(UNCERTAIN_INVENTORY))?;
        let mut fields = attributes.split(' ');
        let (Some(tag), Some(mode), Some(oid), Some(stage), None) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            return Err(Refusal::ReturnUncertain(UNCERTAIN_INVENTORY));
        };
        if stage != "0" || tag == "M" {
            return Err(Refusal::ReturnWork(UNFINISHED_GIT));
        }
        let kind = match mode {
            "100644" | "100755" => EntryKind::File,
            "120000" => EntryKind::Symlink,
            _ => return Err(Refusal::ReturnUncertain(UNCERTAIN_INVENTORY)),
        };
        let relative = Path::new(path);
        if tag != "H"
            || oid.len() != oid_len
            || !slots::object_id(oid)
            || !relative
                .components()
                .all(|component| matches!(component, Component::Normal(_)))
            || path
                .split('/')
                .any(|part| part.is_empty() || part.eq_ignore_ascii_case(".git"))
            || tracked.insert(checkout.join(relative), kind).is_some()
        {
            return Err(Refusal::ReturnUncertain(UNCERTAIN_INVENTORY));
        }
    }
    Ok(tracked)
}

fn reject_status_filters(checkout: &Path) -> Result<(), Refusal> {
    let settings = inventory_git(checkout, &["config", "--null", "--list"], GIT_OUTPUT_LIMIT)?;
    for record in nul_records(&settings)? {
        let record = std::str::from_utf8(record)
            .map_err(|_| Refusal::ReturnUncertain(UNCERTAIN_INVENTORY))?;
        let key = record.split_once('\n').map_or(record, |(key, _)| key);
        if key.is_empty()
            || (key.starts_with("filter.")
                && (key.ends_with(".clean") || key.ends_with(".process")))
        {
            return Err(Refusal::ReturnUncertain(
                "A configured Git clean filter or incomplete configuration prevents a read-only status check.",
            ));
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum OutputRoot<'a> {
    Absent(&'a Path),
    Empty(&'a VerifiedRoot),
    Retained(&'a VerifiedRoot),
}

impl<'a> OutputRoot<'a> {
    fn path(&self) -> &'a Path {
        match self {
            Self::Absent(path) => path,
            Self::Empty(root) | Self::Retained(root) => &root.path,
        }
    }
}

impl<'a> From<&'a RootProof> for OutputRoot<'a> {
    fn from(proof: &'a RootProof) -> Self {
        match proof {
            RootProof::Absent { path } => Self::Absent(path),
            RootProof::Empty { root } => Self::Empty(root),
        }
    }
}

impl<'a> From<&'a RootExpectation> for OutputRoot<'a> {
    fn from(expectation: &'a RootExpectation) -> Self {
        match expectation {
            RootExpectation::Absent { path } => Self::Absent(path),
            RootExpectation::Present { root } => Self::Retained(root),
        }
    }
}

fn retained_roots(
    scope: &SlotScope<'_>,
    checkout: &Path,
    tracked: &BTreeMap<PathBuf, EntryKind>,
    fs: &dyn Fs,
    policy: Option<&ProducerInventory<'_>>,
) -> Result<BTreeMap<PathBuf, FileIdentity>, Refusal> {
    let incomplete = || Refusal::ReturnUncertain(UNCERTAIN_ROOT);
    let profile = &scope.pool.profile;
    let roots = if let Some(policy) = policy {
        match policy.initial_empty {
            Some(initial) => [
                (&initial.roots.target).into(),
                (&initial.roots.build).into(),
            ],
            None => [(&policy.roots.target).into(), (&policy.roots.build).into()],
        }
    } else {
        retained_root_records(scope, checkout)?
    };
    let [target, build] = roots.each_ref().map(OutputRoot::path);
    if !profile.invocations.iter().any(|invocation| {
        checkout.join(&invocation.target_root) == target
            && checkout.join(&invocation.build_root) == build
    }) || (target != build && folded_overlap(target, build))
    {
        return Err(incomplete());
    }
    let probes = input_paths(profile, checkout)?;
    let mut verified = BTreeMap::new();
    for root in roots {
        let path = root.path();
        let relative = path.strip_prefix(checkout).map_err(|_| incomplete())?;
        if relative
            .components()
            .any(|component| forbidden_output_name(component.as_os_str().as_encoded_bytes()))
            || probes
                .iter()
                .chain(tracked.keys())
                .any(|input| folded_overlap(path, input))
        {
            return Err(Refusal::ReturnWork(DISGUISED_OUTPUT));
        }
        match root {
            OutputRoot::Absent(path) if !exists(path).map_err(|_| incomplete())? => (),
            OutputRoot::Empty(expected) | OutputRoot::Retained(expected) => {
                if owned_identity(path) != Some(expected.identity) {
                    return Err(incomplete());
                }
                if matches!(root, OutputRoot::Empty(_)) {
                    let entries = fs.read_dir_bounded(path, 1).map_err(|_| incomplete())?;
                    if !entries.is_empty() {
                        return Err(Refusal::ReturnWork(DISGUISED_OUTPUT));
                    }
                }
                verified.insert(path.to_path_buf(), expected.identity);
            }
            _ => return Err(incomplete()),
        }
    }
    Ok(verified)
}

fn retained_root_records<'a>(
    scope: &SlotScope<'a>,
    checkout: &Path,
) -> Result<[OutputRoot<'a>; 2], Refusal> {
    if !scope
        .slot
        .producer
        .has_eligible_evidence(scope.slot.generation)
    {
        return Err(Refusal::ReturnUncertain(UNCERTAIN_PRODUCER));
    }
    let profile = &scope.pool.profile;
    Ok(match &scope.slot.producer {
        Producer::NeverBuilt {
            evidence: Some(evidence),
        } => [
            (&evidence.roots.target).into(),
            (&evidence.roots.build).into(),
        ],
        Producer::SeedOnly { final_roots, .. } => {
            [(&final_roots.target).into(), (&final_roots.build).into()]
        }
        Producer::Complete {
            roots, invocations, ..
        } => {
            if !invocations.iter().all(|item| {
                profile.invocations.iter().any(|invocation| {
                    invocation.id == item.evidence.invocation
                        && checkout.join(&invocation.target_root) == roots.target.path
                        && checkout.join(&invocation.build_root) == roots.build.path
                })
            }) {
                return Err(Refusal::ReturnUncertain(UNCERTAIN_ROOT));
            }
            [
                OutputRoot::Retained(&roots.target),
                OutputRoot::Retained(&roots.build),
            ]
        }
        _ => return Err(Refusal::ReturnUncertain(UNCERTAIN_PRODUCER)),
    })
}

fn input_paths(profile: &PoolProfile, checkout: &Path) -> Result<BTreeSet<PathBuf>, Refusal> {
    let mut paths = BTreeSet::new();
    for locator in profile
        .graph_inputs
        .iter()
        .map(|input| &input.locator)
        .chain(profile.mutable_inputs.iter().map(|input| &input.locator))
    {
        let path = match locator {
            Locator::Checkout { path } | Locator::DirectoryMembers { path, .. } => {
                checkout.join(path)
            }
            Locator::External { path } => path.clone(),
            Locator::Environment { .. } => continue,
        };
        let canonical =
            canonical_worktree_path(&path).map_err(|_| Refusal::ReturnUncertain(UNCERTAIN_ROOT))?;
        paths.extend([path, canonical]);
    }
    Ok(paths)
}

fn folded_overlap(left: &Path, right: &Path) -> bool {
    if overlaps(left, right) {
        return true;
    }
    fn folded(path: &Path) -> PathBuf {
        PathBuf::from(path.to_string_lossy().to_lowercase())
    }
    overlaps(&folded(left), &folded(right))
}

fn forbidden_output_name(name: &[u8]) -> bool {
    [b".git".as_slice(), b".env", b".sqlx", b"node_modules"]
        .iter()
        .any(|blocked| name.eq_ignore_ascii_case(blocked))
}

struct CheckoutWalk<'a> {
    fs: &'a dyn Fs,
    checkout: &'a Path,
    device: u64,
    tracked: &'a BTreeMap<PathBuf, EntryKind>,
    mutable: Option<&'a BTreeSet<PathBuf>>,
    directories: BTreeSet<PathBuf>,
    retained: BTreeMap<PathBuf, FileIdentity>,
    remaining: usize,
    linked_outputs: BTreeMap<(u64, u64), (u64, u64)>,
    output_locks: Vec<HeldOutputLock>,
}

impl CheckoutWalk<'_> {
    fn visit(&mut self, dir: &Path, depth: usize, generated: bool) -> Result<(), Refusal> {
        check_cancelled()?;
        let incomplete = || Refusal::ReturnUncertain(UNCERTAIN_INVENTORY);
        if depth > MAX_ROOT_DEPTH {
            return Err(incomplete());
        }
        let before = fs::symlink_metadata(dir).map_err(|_| incomplete())?;
        if !before.is_dir()
            || before.dev() != self.device
            || before.uid() != cargo_hive_platform::slot::effective_uid()
            || before.mode() & 0o022 != 0
            || self
                .retained
                .get(dir)
                .is_some_and(|identity| *identity != FileIdentity::of(&before))
        {
            return Err(incomplete());
        }
        let entries = self
            .fs
            .read_dir_bounded(dir, self.remaining)
            .map_err(|_| incomplete())?;
        self.remaining -= entries.len();
        for (name, kind) in entries {
            let path = dir.join(&name);
            if dir == self.checkout && name == ".git" && kind == EntryKind::File {
                continue;
            }
            if name.eq_ignore_ascii_case(".git")
                || (generated && forbidden_output_name(name.as_bytes()))
            {
                return Err(Refusal::ReturnWork(DISGUISED_OUTPUT));
            }
            let generated = generated || self.retained.contains_key(&path);
            match kind {
                EntryKind::Dir if generated || self.directories.contains(&path) => {
                    self.visit(&path, depth + 1, generated)?;
                }
                EntryKind::File if generated => self.generated_file(&path)?,
                EntryKind::File | EntryKind::Symlink if !generated => {
                    match self.tracked.get(&path) {
                        Some(expected) if *expected == kind => {}
                        Some(_) => return Err(Refusal::ReturnWork(UNFINISHED_GIT)),
                        None if kind == EntryKind::File
                            && self.mutable.is_some_and(|paths| paths.contains(&path)) => {}
                        None => return Err(Refusal::ReturnWork(UNFINISHED_FILES)),
                    }
                }
                _ if generated => return Err(Refusal::ReturnWork(DISGUISED_OUTPUT)),
                _ => return Err(Refusal::ReturnWork(UNFINISHED_FILES)),
            }
        }
        if generated {
            self.retain_cargo_locks(dir)?;
        }
        let after = fs::symlink_metadata(dir).map_err(|_| incomplete())?;
        if directory_stamp(&before) != directory_stamp(&after) {
            return Err(incomplete());
        }
        Ok(())
    }

    fn generated_file(&mut self, path: &Path) -> Result<(), Refusal> {
        let entry = self
            .fs
            .entry_stat(path)
            .map_err(|_| Refusal::ReturnUncertain(UNCERTAIN_ROOT))?;
        if entry.kind != EntryKind::File || entry.dev != self.device {
            return Err(Refusal::ReturnUncertain(UNCERTAIN_ROOT));
        }
        if entry.nlink > 1 {
            let (expected, found) = self
                .linked_outputs
                .entry((entry.dev, entry.ino))
                .or_insert((entry.nlink, 0));
            if *expected != entry.nlink {
                return Err(Refusal::ReturnUncertain(UNCERTAIN_ROOT));
            }
            *found += 1;
        }
        Ok(())
    }

    fn retain_cargo_locks(&mut self, dir: &Path) -> Result<(), Refusal> {
        let refusal = || {
            Refusal::ReturnUncertain(
                "An active or unverified Cargo lock prevents proving the slot is quiescent.",
            )
        };
        // Resolve the names Cargo opens so the filesystem determines equivalent spellings.
        for name in [".cargo-build-lock", ".cargo-lock", ".cargo-artifact-lock"] {
            let path = dir.join(name);
            let entry = match self.fs.entry_stat(&path) {
                Ok(entry) if entry.kind == EntryKind::File && entry.dev == self.device => entry,
                Err(err) if err.kind() == ErrorKind::NotFound => continue,
                _ => return Err(refusal()),
            };
            let TryLock::Acquired(lock) = self.fs.try_lock_exclusive(&path) else {
                return Err(refusal());
            };
            let actual = self.fs.entry_stat(&path).map_err(|_| refusal())?;
            if lock.identity() != Some((actual.dev, actual.ino)) || actual != entry {
                return Err(refusal());
            }
            self.output_locks.push(HeldOutputLock { path, lock });
        }
        Ok(())
    }
}

pub(super) fn directory_stamp(metadata: &Metadata) -> (FileIdentity, i64, i64, i64, i64) {
    (
        FileIdentity::of(metadata),
        metadata.mtime(),
        metadata.mtime_nsec(),
        metadata.ctime(),
        metadata.ctime_nsec(),
    )
}
