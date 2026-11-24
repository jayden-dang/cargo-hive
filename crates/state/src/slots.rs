//! Strict, bounded private storage for worktree pools and their transition history.

use crate::{StateError, Tx, check_layout, encode, io_err};
use cargo_hive_platform::flock::{
    LockFileIdentity, checked_owner_directory, checked_stable_ancestor, open_owner_state_file,
    open_owner_state_lock, open_private_file,
};
use cargo_hive_platform::slot::random_slot_bytes;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};

pub const FILE: &str = "slots.json";
pub const LOCK_NAMESPACE: &str = "slot-locks";
pub const CHECKOUT_NAMESPACE: &str = "hive-slots";
pub const MAX_BYTES: u64 = 1_048_576;
pub const MAX_POOLS: usize = 128;
pub const MAX_OUTCOMES: usize = 32;
const MAX_PROFILE_ENTRIES: usize = 256;
const MAX_PATH_BYTES: usize = 4_096;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlotsFile {
    layout: u32,
    pub revision: u64,
    pub pools: Vec<Pool>,
}

impl SlotsFile {
    pub fn new(revision: u64, pools: Vec<Pool>) -> Self {
        Self {
            layout: 2,
            revision,
            pools,
        }
    }

    fn validate(&self) -> FormatResult {
        require(self.layout == 2, "layout must be 2")?;
        require(self.revision > 0, "revision must be positive")?;
        require(self.pools.len() <= MAX_POOLS, "too many pools")?;
        let mut pool_ids = BTreeSet::new();
        let mut mains = BTreeSet::new();
        let mut commons = BTreeSet::new();
        let mut inodes = BTreeSet::new();
        let mut paths = BTreeSet::new();
        for pool in &self.pools {
            require(hex(&pool.pool_id, 32), "invalid pool ID")?;
            require(
                absolute(&pool.main) && absolute(&pool.root),
                "invalid pool path",
            )?;
            require(
                !pool.root.starts_with(&pool.main) && !pool.main.starts_with(&pool.root),
                "pool root overlaps its main checkout",
            )?;
            pool.common_git_id.validate()?;
            require(
                pool.common_git_id.path != pool.main,
                "invalid main Git directory",
            )?;
            require(
                pool_ids.insert(&pool.pool_id)
                    && mains.insert(&pool.main)
                    && commons.insert(&pool.common_git_id.path)
                    && inodes.insert(pool.common_git_id.file_id()),
                "duplicate pool or repository identity",
            )?;
            pool.profile.validate()?;
            require(
                pool.profile.common_git_id == pool.common_git_id,
                "profile repository differs from pool",
            )?;
            let mut numbers = BTreeSet::new();
            for slot in &pool.slots {
                require(numbers.insert(slot.number), "duplicate slot number")?;
                require(
                    paths.insert(pool.checkout_path(slot)),
                    "duplicate checkout path",
                )?;
                require(inodes.insert(slot.lock_id), "reused slot lock identity")?;
                if let Some(checkout) = &slot.checkout_id {
                    require(
                        inodes.insert(checkout.directory)
                            && inodes.insert(checkout.admin.file_id()),
                        "reused checkout or Git admin identity",
                    )?;
                }
                slot.validate(pool)?;
            }
        }
        for pool in &self.pools {
            require(
                mains
                    .iter()
                    .chain(&commons)
                    .all(|scope| !scope.starts_with(&pool.root) && !pool.root.starts_with(scope)),
                "pool root overlaps a registered pool checkout",
            )?;
        }
        Ok(())
    }

    fn validate_successor(&self, prior: &Self) -> FormatResult {
        for pool in &prior.pools {
            let Some(next) = self.pools.iter().find(|next| next.pool_id == pool.pool_id) else {
                return Err("an existing pool cannot be removed or replaced");
            };
            require(
                next.main == pool.main
                    && next.common_git_id == pool.common_git_id
                    && next.root == pool.root
                    && next.profile == pool.profile,
                "an existing pool cannot be rebound",
            )?;
            for (old, new) in pool.slots.iter().zip(&next.slots) {
                require(
                    old.number == new.number
                        && old.path == new.path
                        && old.lock_id == new.lock_id
                        && (old.checkout_id.is_none() || old.checkout_id == new.checkout_id),
                    "an existing slot cannot be rebound",
                )?;
                require(
                    new.generation == old.generation
                        || old.generation.checked_add(1) == Some(new.generation),
                    "slot generation is stale or skipped",
                )?;
                old.producer.validate_successor(&new.producer, old, new)?;
            }
        }
        Ok(())
    }

    fn canonicalize(&mut self) {
        self.pools.sort_by(|left, right| left.main.cmp(&right.main));
        for pool in &mut self.pools {
            pool.slots.sort_by_key(|slot| slot.number);
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pool {
    pub pool_id: String,
    pub main: PathBuf,
    pub common_git_id: GitIdentity,
    pub root: PathBuf,
    pub profile: PoolProfile,
    pub slots: [Slot; 2],
}

impl Pool {
    pub fn checkout_dir(&self) -> PathBuf {
        self.root.join(CHECKOUT_NAMESPACE).join(&self.pool_id)
    }

    pub fn checkout_path(&self, slot: &Slot) -> PathBuf {
        self.checkout_dir().join(&slot.path)
    }

    pub fn lock_path(&self, state: &Path, slot: &Slot) -> PathBuf {
        state
            .join(LOCK_NAMESPACE)
            .join(&self.pool_id)
            .join(slot.lock_name())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Slot {
    pub number: u8,
    pub path: PathBuf,
    pub lock_id: FileIdentity,
    pub checkout_id: Option<CheckoutIdentity>,
    pub last_head: Option<String>,
    pub generation: u64,
    pub phase: Phase,
    pub producer: Producer,
    pub outcomes: Vec<Outcome>,
}

impl Slot {
    pub fn unused(number: u8, lock_id: FileIdentity) -> Self {
        Self {
            number,
            path: PathBuf::from(format!("slot-{number}")),
            lock_id,
            checkout_id: None,
            last_head: None,
            generation: 0,
            phase: Phase::Vacant,
            producer: Producer::NeverBuilt { evidence: None },
            outcomes: Vec::new(),
        }
    }

    pub fn lock_name(&self) -> String {
        Self::lock_name_for(self.number)
    }

    pub fn lock_name_for(number: u8) -> String {
        format!("slot-{number}.lock")
    }

    fn validate(&self, pool: &Pool) -> FormatResult {
        require((1..=2).contains(&self.number), "invalid slot number")?;
        require(
            self.path.as_os_str() == format!("slot-{}", self.number).as_str(),
            "invalid slot path",
        )?;
        require(self.lock_id.valid(), "invalid slot lock identity")?;
        require(
            self.checkout_id.is_some() == self.last_head.is_some(),
            "incomplete checkout/HEAD binding",
        )?;
        if let Some(checkout) = &self.checkout_id {
            require(
                checkout.directory.valid(),
                "invalid checkout directory identity",
            )?;
            checkout.admin.validate()?;
            require(
                checkout
                    .admin
                    .path
                    .starts_with(pool.common_git_id.path.join("worktrees"))
                    && checkout.admin.path.parent()
                        == Some(pool.common_git_id.path.join("worktrees").as_path()),
                "invalid linked Git admin directory",
            )?;
        }
        if let Some(head) = &self.last_head {
            require(object_id(head), "invalid prior HEAD")?;
        }
        require(
            self.generation > 0
                || (self.checkout_id.is_none()
                    && matches!(
                        self.phase,
                        Phase::Vacant
                            | Phase::Held {
                                prior: PriorPhase::Vacant,
                                ..
                            }
                    )
                    && self.outcomes.is_empty()),
            "unused generation cannot carry prior checkout or lease",
        )?;
        self.phase.validate(self)?;
        self.producer.validate(pool, self)?;
        require(
            self.outcomes.len() <= MAX_OUTCOMES,
            "too many unacknowledged outcomes",
        )?;
        let mut keys = BTreeSet::new();
        for outcome in &self.outcomes {
            outcome.validate(pool, self)?;
            require(
                keys.insert((
                    outcome.generation,
                    outcome.attempt_id,
                    outcome.operation.as_str(),
                )),
                "duplicate operation identity",
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileIdentity {
    pub dev: u64,
    pub ino: u64,
}

impl FileIdentity {
    pub fn of(metadata: &fs::Metadata) -> Self {
        Self {
            dev: metadata.dev(),
            ino: metadata.ino(),
        }
    }

    fn valid(self) -> bool {
        self.ino != 0
    }
}

impl From<LockFileIdentity> for FileIdentity {
    fn from(id: LockFileIdentity) -> Self {
        Self {
            dev: id.dev,
            ino: id.ino,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GitIdentity {
    pub path: PathBuf,
    pub dev: u64,
    pub ino: u64,
}

impl GitIdentity {
    pub fn of(path: PathBuf, metadata: &fs::Metadata) -> Self {
        Self {
            path,
            dev: metadata.dev(),
            ino: metadata.ino(),
        }
    }

    pub fn file_id(&self) -> FileIdentity {
        FileIdentity {
            dev: self.dev,
            ino: self.ino,
        }
    }

    fn validate(&self) -> FormatResult {
        require(
            absolute(&self.path) && self.file_id().valid(),
            "invalid Git identity",
        )
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckoutIdentity {
    pub directory: FileIdentity,
    pub admin: GitIdentity,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Phase {
    Vacant,
    Reserved {
        lease: Lease,
    },
    Preparing {
        lease: Lease,
        attempt: Attempt,
    },
    Leased {
        lease: Lease,
        ready_commit: String,
    },
    Held {
        reason: HoldReason,
        prior: PriorPhase,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PriorPhase {
    Vacant,
    Reserved { lease: Lease },
    Preparing { lease: Lease, attempt: Attempt },
    Leased { lease: Lease, ready_commit: String },
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HoldReason {
    IdentityUnproved,
    Interrupted,
    WriterUnproved,
    UnfinishedWork,
    ProducerUnproved,
}

impl Phase {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Vacant => "vacant",
            Self::Reserved { .. } => "reserved",
            Self::Preparing { .. } => "preparing",
            Self::Leased { .. } => "leased",
            Self::Held { .. } => "held",
        }
    }

    fn active_lease(&self) -> Option<&Lease> {
        match self {
            Self::Reserved { lease }
            | Self::Preparing { lease, .. }
            | Self::Leased { lease, .. }
            | Self::Held {
                prior:
                    PriorPhase::Reserved { lease }
                    | PriorPhase::Preparing { lease, .. }
                    | PriorPhase::Leased { lease, .. },
                ..
            } => Some(lease),
            Self::Vacant
            | Self::Held {
                prior: PriorPhase::Vacant,
                ..
            } => None,
        }
    }

    fn validate(&self, slot: &Slot) -> FormatResult {
        let (lease, attempt, ready) = match self {
            Self::Vacant
            | Self::Held {
                prior: PriorPhase::Vacant,
                ..
            } => return Ok(()),
            Self::Reserved { lease }
            | Self::Held {
                prior: PriorPhase::Reserved { lease },
                ..
            } => (lease, None, None),
            Self::Preparing { lease, attempt }
            | Self::Held {
                prior: PriorPhase::Preparing { lease, attempt },
                ..
            } => (lease, Some(attempt), None),
            Self::Leased {
                lease,
                ready_commit,
            }
            | Self::Held {
                prior:
                    PriorPhase::Leased {
                        lease,
                        ready_commit,
                    },
                ..
            } => (lease, None, Some(ready_commit)),
        };
        lease.validate()?;
        if let Some(attempt) = attempt {
            attempt.validate()?;
        }
        if let Some(commit) = ready {
            require(
                commit == &lease.requested_commit && slot.last_head.as_ref() == Some(commit),
                "invalid ready HEAD binding",
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lease {
    pub requested_commit: String,
    pub credential: String,
    pub reservation_op: String,
}

impl Lease {
    fn validate(&self) -> FormatResult {
        require(
            object_id(&self.requested_commit)
                && hex(&self.credential, 64)
                && hex(&self.reservation_op, 32),
            "invalid lease binding",
        )
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attempt {
    pub attempt_id: u64,
    pub begin_op: String,
    pub runner: Runner,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_binding: Option<[u8; 32]>,
    pub stage: AttemptStage,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock_offset: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_head: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub anchors: Vec<AnchorWitness>,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub anchor_cursor: usize,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

impl Attempt {
    fn validate(&self) -> FormatResult {
        require(
            self.attempt_id > 0
                && hex(&self.begin_op, 32)
                && self.host_binding.is_none_or(|digest| digest != [0; 32])
                && self.lock_offset.is_none_or(|offset| offset >= (1 << 32)),
            "invalid attempt identity",
        )?;
        require(
            self.prior_head.as_ref().is_none_or(|head| object_id(head))
                && self.anchors.len() <= MAX_PROFILE_ENTRIES
                && self.anchor_cursor <= self.anchors.len()
                && unique(self.anchors.iter().map(|anchor| &anchor.path))
                && self.anchors.iter().all(AnchorWitness::valid),
            "invalid preparation input witness",
        )?;
        self.runner.validate()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptStage {
    PermitIssued,
    CheckoutInProgress,
    AwaitingAnchors,
    AnchorsInProgress,
    CheckoutVerified,
    SeedInProgress,
    GcInProgress,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnchorWitness {
    pub path: PathBuf,
    pub identity: FileIdentity,
    pub object: String,
    pub mode: u32,
    pub prior_unit: (i64, i64),
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unit_digest: Option<String>,
    pub floor: (i64, i64),
    pub touched: Option<(i64, i64)>,
}

impl AnchorWitness {
    fn valid(&self) -> bool {
        let valid_stamp =
            |&(seconds, nanos): &(i64, i64)| seconds > 0 && (0..1_000_000_000).contains(&nanos);
        relative(&self.path)
            && self.identity.valid()
            && object_id(&self.object)
            && self
                .unit_digest
                .as_ref()
                .is_none_or(|digest| hex(digest, 64))
            && valid_stamp(&self.prior_unit)
            && valid_stamp(&self.floor)
            && self.floor >= self.prior_unit
            && self
                .touched
                .as_ref()
                .is_none_or(|stamp| valid_stamp(stamp) && *stamp > self.floor)
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Runner {
    pub boot_session: String,
    pub pid: u32,
    pub start_secs: u64,
    pub start_micros: u64,
    pub nonce: String,
}

impl Runner {
    fn validate(&self) -> FormatResult {
        require(
            identifier(&self.boot_session)
                && self.pid > 0
                && self.start_secs > 0
                && self.start_micros < 1_000_000
                && hex(&self.nonce, 64),
            "invalid process instance",
        )
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Producer {
    NeverBuilt {
        evidence: Option<EmptyEpoch>,
    },
    SeedOnly {
        evidence: EmptyEpoch,
        generation: u64,
        profile: ProfileBinding,
        final_roots: OutputRoots<RootExpectation>,
        registry_units: u64,
    },
    Pending(Box<PendingProducer>),
    Complete {
        epoch: OutputEpoch,
        generation: u64,
        profile: ProfileBinding,
        roots: OutputRoots<VerifiedRoot>,
        invocations: Vec<CompletedInvocation>,
    },
    Tainted {
        epoch: OutputEpoch,
        generation: u64,
        roots: OutputRoots<Option<VerifiedRoot>>,
        reason: ProvenanceLoss,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        report: Option<BypassReport>,
    },
    Unknown {
        epoch: OutputEpoch,
        generation: u64,
        roots: OutputRoots<Option<VerifiedRoot>>,
        reason: ProvenanceLoss,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BypassReport {
    pub operation: String,
    pub runner: Runner,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingProducer {
    pub epoch: OutputEpoch,
    pub generation: u64,
    pub profile: ProfileBinding,
    pub invocation: InvocationEvidence,
    pub roots: OutputRoots<RootExpectation>,
    pub operation: String,
    pub runner: Runner,
    pub prior: EligibleProducer,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground: Option<ForegroundInstance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guide_guard_offset: Option<i64>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EligibleProducer {
    NeverBuilt {
        evidence: EmptyEpoch,
    },
    SeedOnly(SeedEvidence),
    Complete {
        generation: u64,
        roots: OutputRoots<VerifiedRoot>,
        invocations: Vec<CompletedInvocation>,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SeedEvidence {
    pub evidence: EmptyEpoch,
    pub generation: u64,
    pub final_roots: OutputRoots<RootExpectation>,
    pub registry_units: u64,
}

impl SeedEvidence {
    fn derives_from_empty(
        evidence: &EmptyEpoch,
        final_roots: &OutputRoots<RootExpectation>,
    ) -> bool {
        evidence.roots.target.accepts_seed(&final_roots.target)
            && evidence.roots.build.accepts_seed(&final_roots.build)
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmptyEpoch {
    pub epoch: OutputEpoch,
    pub roots: OutputRoots<RootProof>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputEpoch {
    pub pool_id: String,
    pub slot: u8,
    pub checkout: CheckoutIdentity,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputRoots<T> {
    pub target: T,
    pub build: T,
}

impl<T> OutputRoots<T> {
    fn validate(&self, check: impl Fn(&T) -> FormatResult) -> FormatResult {
        check(&self.target)?;
        check(&self.build)
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RootProof {
    Absent { path: PathBuf },
    Empty { root: VerifiedRoot },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RootExpectation {
    Absent { path: PathBuf },
    Present { root: VerifiedRoot },
}

impl RootExpectation {
    fn validate(&self, slot_path: &Path) -> FormatResult {
        match self {
            Self::Absent { path } => validate_output_path(path, slot_path),
            Self::Present { root } => root.validate(slot_path),
        }
    }
}

impl RootProof {
    fn accepts_seed(&self, later: &RootExpectation) -> bool {
        match (self, later) {
            (Self::Absent { path }, RootExpectation::Absent { path: final_path }) => {
                path == final_path
            }
            (Self::Absent { path }, RootExpectation::Present { root }) => path == &root.path,
            (Self::Empty { root }, RootExpectation::Present { root: final_root }) => {
                root == final_root
            }
            _ => false,
        }
    }

    fn validate(&self, slot_path: &Path) -> FormatResult {
        match self {
            Self::Absent { path } => validate_output_path(path, slot_path),
            Self::Empty { root } => root.validate(slot_path),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerifiedRoot {
    pub path: PathBuf,
    pub identity: FileIdentity,
}

impl VerifiedRoot {
    fn validate(&self, slot_path: &Path) -> FormatResult {
        require(self.identity.valid(), "invalid output root identity")?;
        validate_output_path(&self.path, slot_path)
    }
}

fn validate_output_path(path: &Path, slot_path: &Path) -> FormatResult {
    require(
        absolute(path) && path.starts_with(slot_path) && path != slot_path,
        "invalid output root",
    )
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileBinding {
    pub id: String,
    pub revision: u64,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvocationEvidence {
    pub invocation: String,
    pub graph_digest: String,
    pub environment_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stable_digest: Option<String>,
}

impl InvocationEvidence {
    fn is_valid_for(&self, profile: &PoolProfile) -> bool {
        profile
            .invocations
            .iter()
            .any(|item| item.id == self.invocation)
            && hex(&self.graph_digest, 64)
            && hex(&self.environment_digest, 64)
            && self
                .stable_digest
                .as_ref()
                .is_none_or(|digest| hex(digest, 64))
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompletedInvocation {
    pub evidence: InvocationEvidence,
    pub operation: String,
    pub runner: Runner,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreground: Option<ForegroundInstance>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForegroundInstance {
    pub pid: u32,
    pub start_secs: u64,
    pub start_micros: u64,
}

impl ForegroundInstance {
    fn validate(&self, runner: &Runner) -> FormatResult {
        require(
            self.pid > 1
                && self.pid != runner.pid
                && (self.start_secs, self.start_micros) >= (runner.start_secs, runner.start_micros)
                && self.start_micros < 1_000_000,
            "invalid foreground writer instance",
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceLoss {
    OffProfile,
    LostWriter,
    UnprovedInvocation,
    UnaccountedOutput,
}

impl Producer {
    pub fn lost(&self) -> bool {
        matches!(self, Self::Tainted { .. } | Self::Unknown { .. })
    }

    pub fn has_eligible_evidence(&self, generation: u64) -> bool {
        let current_or_previous =
            |prior: u64| prior == generation || prior.checked_add(1) == Some(generation);
        match self {
            Self::NeverBuilt { evidence } => evidence.is_some(),
            Self::SeedOnly { generation, .. } => current_or_previous(*generation),
            Self::Complete {
                generation,
                invocations,
                ..
            } => {
                current_or_previous(*generation)
                    && !invocations.is_empty()
                    && invocations.iter().all(|record| record.foreground.is_some())
            }
            _ => false,
        }
    }

    pub fn eligible(&self, generation: u64) -> Option<EligibleProducer> {
        if !self.has_eligible_evidence(generation) {
            return None;
        }
        match self {
            Self::NeverBuilt {
                evidence: Some(evidence),
            } => Some(EligibleProducer::NeverBuilt {
                evidence: evidence.clone(),
            }),
            Self::SeedOnly {
                evidence,
                generation,
                final_roots,
                registry_units,
                ..
            } => Some(EligibleProducer::SeedOnly(SeedEvidence {
                evidence: evidence.clone(),
                generation: *generation,
                final_roots: final_roots.clone(),
                registry_units: *registry_units,
            })),
            Self::Complete {
                generation,
                roots,
                invocations,
                ..
            } => Some(EligibleProducer::Complete {
                generation: *generation,
                roots: roots.clone(),
                invocations: invocations.clone(),
            }),
            _ => None,
        }
    }

    fn validate_successor(&self, next: &Self, old: &Slot, new: &Slot) -> FormatResult {
        if self == next {
            return Ok(());
        }
        if self.lost() {
            return Err("lost producer provenance cannot be reset on the retained checkout");
        }
        if let Self::Pending(pending) = self {
            return match next {
                Self::Pending(watching)
                    if pending.foreground.is_none()
                        && watching.foreground.is_some()
                        && new.generation == old.generation
                        && new.phase == old.phase
                        && pending.matches_start(watching) =>
                {
                    Ok(())
                }
                Self::Complete {
                    epoch,
                    generation,
                    profile,
                    roots,
                    invocations,
                } if epoch == &pending.epoch
                    && generation == &pending.generation
                    && profile == &pending.profile
                    && new.generation == old.generation
                    && new.phase == old.phase
                    && pending.roots.target.accepts(&roots.target)
                    && pending.roots.build.accepts(&roots.build)
                    && pending.completed_records_match(invocations) =>
                {
                    Ok(())
                }
                Self::Unknown {
                    epoch, generation, ..
                } if epoch == &pending.epoch && generation == &pending.generation => Ok(()),
                Self::Tainted {
                    epoch,
                    generation,
                    report: Some(report),
                    ..
                } if epoch == &pending.epoch
                    && generation == &pending.generation
                    && report.runner == pending.runner
                    && new.phase == old.phase =>
                {
                    Ok(())
                }
                _ => {
                    Err("pending producer may only complete its exact invocation or become unknown")
                }
            };
        }
        match next {
            Self::Tainted {
                generation,
                report: Some(_),
                ..
            } => require(
                *generation == old.generation
                    && new.generation == old.generation
                    && new.phase == old.phase
                    && matches!(old.phase, Phase::Leased { .. }),
                "off-profile report does not match the current ready lease",
            ),
            Self::Pending(pending) => require(
                old.generation == new.generation
                    && old.phase == new.phase
                    && matches!(old.phase, Phase::Leased { .. })
                    && pending.foreground.is_none()
                    && self.eligible(new.generation).as_ref() == Some(&pending.prior)
                    && pending.prior.roots() == pending.roots,
                "pending producer does not match the current eligible lease and output",
            ),
            Self::Complete { .. } => Err("producer completion requires its durable pending start"),
            Self::NeverBuilt { evidence: Some(_) } => require(
                matches!(self, Self::NeverBuilt { evidence: None }) && old.checkout_id.is_none(),
                "initial output evidence requires a new checkout",
            ),
            Self::SeedOnly { evidence, .. } => require(
                match self {
                    Self::NeverBuilt { evidence: None } => old.checkout_id.is_none(),
                    Self::NeverBuilt {
                        evidence: Some(empty),
                    } => empty == evidence,
                    _ => false,
                },
                "registry-only seed evidence requires the initial empty checkout",
            ),
            _ => Ok(()),
        }
    }

    fn validate(&self, pool: &Pool, slot: &Slot) -> FormatResult {
        let slot_path = pool.checkout_path(slot);
        let epoch_matches = |epoch: &OutputEpoch| {
            require(
                epoch.pool_id == pool.pool_id
                    && epoch.slot == slot.number
                    && slot.checkout_id.as_ref() == Some(&epoch.checkout),
                "output epoch identity differs from slot",
            )
        };
        let validate_empty = |evidence: &EmptyEpoch| {
            epoch_matches(&evidence.epoch)?;
            evidence.roots.validate(|root| root.validate(&slot_path))
        };
        let validate_generation = |generation: u64| {
            require(
                generation > 0 && generation <= slot.generation,
                "invalid producer generation",
            )
        };
        let validate_profile = |profile: &ProfileBinding| {
            require(
                profile.id == pool.profile.id && profile.revision == pool.profile.revision,
                "invalid producer profile binding",
            )
        };
        let validate_invocation = |evidence: &InvocationEvidence| {
            require(
                evidence.is_valid_for(&pool.profile),
                "invalid invocation evidence",
            )
        };
        let validate_completed = |invocations: &[CompletedInvocation]| {
            require(
                !invocations.is_empty() && invocations.len() <= MAX_PROFILE_ENTRIES,
                "invalid completed invocation count",
            )?;
            let mut operations = BTreeSet::new();
            for item in invocations {
                require(
                    hex(&item.operation, 32) && operations.insert(&item.operation),
                    "invalid completed invocation operation",
                )?;
                validate_invocation(&item.evidence)?;
                item.runner.validate()?;
                if let Some(foreground) = &item.foreground {
                    foreground.validate(&item.runner)?;
                }
            }
            Ok(())
        };
        match self {
            Self::NeverBuilt { evidence: None } => require(
                slot.checkout_id.is_none(),
                "provisioned checkout lacks initial output evidence",
            ),
            Self::NeverBuilt {
                evidence: Some(evidence),
            } => validate_empty(evidence),
            Self::SeedOnly {
                evidence,
                generation,
                profile,
                final_roots,
                ..
            } => {
                validate_generation(*generation)?;
                validate_empty(evidence)?;
                validate_profile(profile)?;
                final_roots.validate(|root| root.validate(&slot_path))?;
                require(
                    SeedEvidence::derives_from_empty(evidence, final_roots),
                    "seed output roots differ from their empty proof",
                )
            }
            Self::Pending(pending) => {
                require(
                    matches!(
                        slot.phase,
                        Phase::Leased { .. }
                            | Phase::Held {
                                prior: PriorPhase::Leased { .. },
                                ..
                            }
                    ),
                    "pending producer requires a ready lease",
                )?;
                let PendingProducer {
                    epoch,
                    generation,
                    profile,
                    invocation,
                    roots,
                    operation,
                    runner,
                    prior,
                    foreground,
                    guide_guard_offset,
                } = pending.as_ref();
                epoch_matches(epoch)?;
                require(
                    guide_guard_offset.is_none_or(|offset| offset > 0),
                    "invalid producer guide guard",
                )?;
                validate_generation(*generation)?;
                validate_profile(profile)?;
                require(
                    *generation == slot.generation && hex(operation, 32),
                    "invalid pending invocation binding",
                )?;
                runner.validate()?;
                if let Some(foreground) = foreground {
                    foreground.validate(runner)?;
                }
                validate_invocation(invocation)?;
                roots.validate(|root| root.validate(&slot_path))?;
                match prior {
                    EligibleProducer::NeverBuilt { evidence } => validate_empty(evidence),
                    EligibleProducer::SeedOnly(seed) => {
                        validate_generation(seed.generation)?;
                        validate_empty(&seed.evidence)?;
                        seed.final_roots
                            .validate(|root| root.validate(&slot_path))?;
                        require(
                            SeedEvidence::derives_from_empty(&seed.evidence, &seed.final_roots),
                            "seed output roots differ from their empty proof",
                        )
                    }
                    EligibleProducer::Complete {
                        generation,
                        roots,
                        invocations,
                    } => {
                        validate_generation(*generation)?;
                        roots.validate(|root| root.validate(&slot_path))?;
                        validate_completed(invocations)
                    }
                }
            }
            Self::Complete {
                epoch,
                generation,
                profile,
                roots,
                invocations,
            } => {
                epoch_matches(epoch)?;
                validate_generation(*generation)?;
                validate_profile(profile)?;
                roots.validate(|root| root.validate(&slot_path))?;
                validate_completed(invocations)
            }
            Self::Tainted {
                epoch,
                generation,
                roots,
                reason,
                report,
            } => {
                if let Some(report) = report {
                    require(
                        *reason == ProvenanceLoss::OffProfile && hex(&report.operation, 32),
                        "invalid reported off-profile write",
                    )?;
                    report.runner.validate()?;
                }
                epoch_matches(epoch)?;
                validate_generation(*generation)?;
                roots.validate(|root| {
                    root.as_ref()
                        .map_or(Ok(()), |root| root.validate(&slot_path))
                })
            }
            Self::Unknown {
                epoch,
                generation,
                roots,
                ..
            } => {
                epoch_matches(epoch)?;
                validate_generation(*generation)?;
                roots.validate(|root| {
                    root.as_ref()
                        .map_or(Ok(()), |root| root.validate(&slot_path))
                })
            }
        }
    }
}

impl EligibleProducer {
    pub fn roots(&self) -> OutputRoots<RootExpectation> {
        let occupied = |root: &VerifiedRoot| RootExpectation::Present { root: root.clone() };
        let initial = |proof: &RootProof| match proof {
            RootProof::Absent { path } => RootExpectation::Absent { path: path.clone() },
            RootProof::Empty { root } => occupied(root),
        };
        match self {
            Self::NeverBuilt { evidence } => OutputRoots {
                target: initial(&evidence.roots.target),
                build: initial(&evidence.roots.build),
            },
            Self::SeedOnly(seed) => seed.final_roots.clone(),
            Self::Complete { roots, .. } => OutputRoots {
                target: occupied(&roots.target),
                build: occupied(&roots.build),
            },
        }
    }

    pub fn completed_in_generation(&self, current: u64) -> &[CompletedInvocation] {
        match self {
            Self::Complete {
                generation,
                invocations,
                ..
            } if *generation == current => invocations,
            _ => &[],
        }
    }
}

impl RootExpectation {
    pub fn accepts(&self, observed: &VerifiedRoot) -> bool {
        match self {
            Self::Absent { path } => path == &observed.path,
            Self::Present { root } => root == observed,
        }
    }
}

impl PendingProducer {
    fn matches_start(&self, watching: &Self) -> bool {
        let mut start = watching.clone();
        start.foreground = None;
        self == &start
    }

    fn completed_records_match(&self, records: &[CompletedInvocation]) -> bool {
        let Some((record, prior)) = records.split_last() else {
            return false;
        };
        prior == self.prior.completed_in_generation(self.generation)
            && record.operation == self.operation
            && record.runner == self.runner
            && record.evidence == self.invocation
            && self.foreground.is_some()
            && record.foreground == self.foreground
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Outcome {
    pub pool_id: String,
    pub slot: u8,
    pub generation: u64,
    pub operation: String,
    pub kind: OperationKind,
    pub attempt_id: Option<u64>,
    pub request: OutcomeRequest,
    pub credential: String,
    pub result: TransitionResult,
}

/// Operation identity within a specific pool and slot generation.
pub struct SlotRequestKey<'a> {
    pub pool_id: &'a str,
    pub slot: u8,
    pub generation: u64,
    pub attempt_id: Option<u64>,
    pub kind: OperationKind,
    pub operation: &'a str,
}

impl SlotRequestKey<'_> {
    /// Finds the operation before checking kind, so a reused ID cannot hide a conflict.
    pub fn matches_identity(&self, outcome: &Outcome) -> bool {
        self.pool_id == outcome.pool_id
            && self.slot == outcome.slot
            && self.generation == outcome.generation
            && self.attempt_id == outcome.attempt_id
            && self.operation == outcome.operation
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Reserve,
    Begin,
    Ready,
    Return,
    ProducerStart,
    ProducerComplete,
    Recover,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OutcomeRequest {
    Reserve {
        expected_generation: u64,
        commit: String,
    },
    Begin {
        runner: Runner,
    },
    Ready {
        runner: Runner,
    },
    Return,
    Producer {
        runner: Runner,
        invocation: InvocationEvidence,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ready_commit: Option<String>,
    },
    Recover {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        recovery: Option<RecoveryRequest>,
    },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryRequest {
    pub observed_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub begin_op: Option<String>,
    pub authority: RecoveryAuthorization,
    pub quiescence: RecoveryQuiescence,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecoveryAuthorization {
    Holder {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        nonce: Option<String>,
    },
    Operator,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RecoveryQuiescence {
    NoPermitIssued,
    BeforeFirstEffect,
    VerifiedLaterBoot,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum TransitionResult {
    Reserved,
    PermitIssued,
    Ready { commit: String },
    Returned { commit: String },
    ProducerPending,
    ProducerComplete,
    Recovered,
}

impl Outcome {
    fn validate(&self, pool: &Pool, slot: &Slot) -> FormatResult {
        require(
            self.pool_id == pool.pool_id
                && self.slot == slot.number
                && (1..=slot.generation).contains(&self.generation)
                && hex(&self.operation, 32)
                && hex(&self.credential, 64),
            "invalid outcome identity",
        )?;
        if self.generation == slot.generation
            && let Some(lease) = slot.phase.active_lease()
        {
            if self.kind != OperationKind::Recover {
                require(
                    self.credential == lease.credential,
                    "outcome credential differs from current lease",
                )?;
            }
            if let OutcomeRequest::Reserve { commit, .. } = &self.request {
                require(
                    self.operation == lease.reservation_op && commit == &lease.requested_commit,
                    "reservation outcome differs from current lease",
                )?;
            }
            if let TransitionResult::Ready { commit } = &self.result {
                require(
                    commit == &lease.requested_commit,
                    "ready outcome differs from current lease",
                )?;
            }
        }
        let preparing = self.attempt_id.is_some_and(|attempt| attempt > 0);
        match (&self.kind, &self.request, &self.result) {
            (
                OperationKind::Reserve,
                OutcomeRequest::Reserve {
                    expected_generation,
                    commit,
                },
                TransitionResult::Reserved,
            ) => require(
                self.attempt_id.is_none()
                    && expected_generation.checked_add(1) == Some(self.generation)
                    && object_id(commit),
                "invalid reservation outcome",
            ),
            (
                OperationKind::Begin,
                OutcomeRequest::Begin { runner },
                TransitionResult::PermitIssued,
            )
            | (
                OperationKind::Ready,
                OutcomeRequest::Ready { runner },
                TransitionResult::Ready { .. },
            ) => {
                require(preparing, "preparation outcome lacks an attempt")?;
                runner.validate()?;
                if let TransitionResult::Ready { commit } = &self.result {
                    require(object_id(commit), "invalid ready outcome commit")?;
                }
                Ok(())
            }
            (
                OperationKind::Return,
                OutcomeRequest::Return,
                TransitionResult::Returned { commit },
            ) => require(
                self.attempt_id.is_none() && object_id(commit),
                "invalid return outcome",
            ),
            (
                OperationKind::Recover,
                OutcomeRequest::Recover { recovery },
                TransitionResult::Recovered,
            ) => match recovery {
                Some(recovery) => require(
                    self.attempt_id.is_none_or(|attempt| attempt > 0)
                        && recovery.observed_revision > 0
                        && preparing == recovery.begin_op.is_some()
                        && preparing == (recovery.quiescence != RecoveryQuiescence::NoPermitIssued)
                        && recovery.begin_op.as_ref().is_none_or(|begin_op| {
                            hex(begin_op, 32) && begin_op != &self.operation
                        })
                        && match &recovery.authority {
                            RecoveryAuthorization::Operator => true,
                            RecoveryAuthorization::Holder { nonce } => {
                                preparing == nonce.is_some()
                                    && nonce.as_ref().is_none_or(|nonce| hex(nonce, 64))
                            }
                        },
                    "invalid recovery attempt binding",
                ),
                None => require(self.attempt_id.is_none(), "unexpected attempt on outcome"),
            },
            (
                OperationKind::ProducerStart,
                OutcomeRequest::Producer {
                    runner,
                    invocation,
                    ready_commit,
                },
                TransitionResult::ProducerPending,
            )
            | (
                OperationKind::ProducerComplete,
                OutcomeRequest::Producer {
                    runner,
                    invocation,
                    ready_commit,
                },
                TransitionResult::ProducerComplete,
            ) => {
                require(
                    self.attempt_id.is_none()
                        && invocation.is_valid_for(&pool.profile)
                        && ready_commit.as_deref().is_none_or(|commit| {
                            object_id(commit)
                                && (self.generation != slot.generation
                                    || slot
                                        .phase
                                        .active_lease()
                                        .is_none_or(|lease| lease.requested_commit == commit))
                        }),
                    "invalid producer outcome",
                )?;
                runner.validate()
            }
            _ => Err("outcome kind, request and result disagree"),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PoolProfile {
    pub schema: u32,
    pub id: String,
    pub revision: u64,
    pub common_git_id: GitIdentity,
    pub graph_inputs: Vec<GraphInput>,
    pub invocations: Vec<Invocation>,
    pub mutable_inputs: Vec<MutableInput>,
    pub consumers: Vec<Consumer>,
    pub proofs: Vec<Proof>,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Locator {
    Checkout {
        path: PathBuf,
    },
    External {
        path: PathBuf,
    },
    DirectoryMembers {
        path: PathBuf,
        members: Vec<PathBuf>,
    },
    Environment {
        name: String,
    },
}

impl Locator {
    fn validate(&self, exact_file: bool) -> FormatResult {
        match self {
            Self::Checkout { path } => require(relative(path), "invalid relative input path"),
            Self::External { path } => require(absolute(path), "invalid external input path"),
            Self::DirectoryMembers { path, members } => require(
                !exact_file
                    && relative(path)
                    && bounded_unique(members.iter())
                    && members.iter().all(|member| relative(member)),
                "invalid reviewed directory membership",
            ),
            Self::Environment { name } => {
                require(!exact_file && env_name(name), "invalid environment input")
            }
        }
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphInput {
    pub locator: Locator,
    pub expectation: GraphExpectation,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GraphExpectation {
    Present { identity: InputIdentity },
    Absent,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InputIdentity {
    GitObject { oid: String },
    ReviewedBytes { sha256: String },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invocation {
    pub id: String,
    pub cargo: String,
    pub rustc: String,
    pub layout: CargoLayout,
    pub argv: Vec<String>,
    pub cwd: PathBuf,
    pub environment: Vec<Environment>,
    pub target_root: PathBuf,
    pub build_root: PathBuf,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CargoLayout {
    Flat,
    UnitDir,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Environment {
    pub name: String,
    pub expectation: EnvironmentExpectation,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EnvironmentExpectation {
    Present { value: String },
    Absent,
    Mutable { input: String },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MutableInput {
    pub id: String,
    pub locator: Locator,
    pub presence: MutablePresence,
    pub invocations: Vec<String>,
    pub coverage: Coverage,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutablePresence {
    Present,
    PresentOrAbsent,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Coverage {
    Consumers { ids: Vec<String> },
    Excluded { proofs: Vec<String> },
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Consumer {
    pub id: String,
    pub package: String,
    pub role: String,
    pub invocation: String,
    pub anchor: PathBuf,
    pub prior_object: String,
    pub requested_object: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proof {
    pub id: String,
    pub invocation: String,
    pub layout: CargoLayout,
    pub evidence: String,
    pub unaffected_unit: String,
}

impl PoolProfile {
    /// Reads the submitted structural profile without writing to its source.
    pub fn read(path: &Path) -> Result<Self, StateError> {
        let directory = parent(path).map_err(|err| io_err(path, err))?;
        if !absolute(directory) {
            return Err(io_err(path, noncanonical_directory()));
        }
        check_stable_ancestors(path).map_err(|err| io_err(path, err))?;
        let file = open_owner_state_file(path).map_err(|err| io_err(path, err))?;
        if path
            .canonicalize()
            .map_err(|err| io_err(path, err))?
            .as_os_str()
            != path.as_os_str()
        {
            return Err(format_error(
                path,
                "profile must be a canonical regular file",
            ));
        }
        let bytes = read_bounded(path, file)?;
        let profile: Self = parse_exact(path, &bytes)?;
        profile
            .validate()
            .map_err(|detail| format_error(path, detail))?;
        Ok(profile)
    }

    fn validate(&self) -> FormatResult {
        require(self.schema == 1, "profile schema must be 1")?;
        require(
            identifier(&self.id) && self.revision > 0,
            "invalid profile identity",
        )?;
        self.common_git_id.validate()?;
        require(
            bounded_unique(self.graph_inputs.iter().map(|item| &item.locator)),
            "invalid graph input inventory",
        )?;
        require(
            bounded_unique(self.invocations.iter().map(|item| &item.id)),
            "invalid invocation inventory",
        )?;
        require(
            bounded_unique(self.mutable_inputs.iter().map(|item| &item.id)),
            "invalid mutable input inventory",
        )?;
        require(
            bounded_unique(self.mutable_inputs.iter().map(|item| &item.locator)),
            "duplicate mutable input locator",
        )?;
        require(
            self.consumers.len() <= MAX_PROFILE_ENTRIES
                && unique(self.consumers.iter().map(|item| &item.id)),
            "invalid consumer inventory",
        )?;
        require(
            bounded_unique(self.proofs.iter().map(|item| &item.id)),
            "invalid proof inventory",
        )?;

        for input in &self.graph_inputs {
            input.locator.validate(true)?;
            require(
                !self
                    .mutable_inputs
                    .iter()
                    .any(|mutable| mutable.locator == input.locator),
                "a fixed graph input is also declared mutable",
            )?;
            match &input.expectation {
                GraphExpectation::Present {
                    identity: InputIdentity::GitObject { oid },
                } => require(object_id(oid), "invalid graph input Git object")?,
                GraphExpectation::Present {
                    identity: InputIdentity::ReviewedBytes { sha256 },
                } => require(hex(sha256, 64), "invalid graph input digest")?,
                GraphExpectation::Absent => (),
            }
        }

        for invocation in &self.invocations {
            invocation.validate(&self.mutable_inputs)?;
            require(
                self.proofs.iter().any(|proof| {
                    proof.invocation == invocation.id && proof.layout == invocation.layout
                }),
                "invocation lacks a matching causal proof",
            )?;
        }

        for input in &self.mutable_inputs {
            input.validate(self)?;
        }

        for consumer in &self.consumers {
            require(
                identifier(&consumer.id)
                    && identifier(&consumer.package)
                    && identifier(&consumer.role)
                    && self
                        .invocations
                        .iter()
                        .any(|item| item.id == consumer.invocation)
                    && relative(&consumer.anchor)
                    && object_id(&consumer.prior_object)
                    && object_id(&consumer.requested_object),
                "invalid consumer anchor or invocation reference",
            )?;
        }

        for proof in &self.proofs {
            require(
                identifier(&proof.id)
                    && text(&proof.evidence, 1_024)
                    && identifier(&proof.unaffected_unit)
                    && self
                        .invocations
                        .iter()
                        .any(|item| item.id == proof.invocation && item.layout == proof.layout),
                "invalid causal proof or invocation reference",
            )?;
        }
        Ok(())
    }
}

impl Invocation {
    fn validate(&self, inputs: &[MutableInput]) -> FormatResult {
        require(
            identifier(&self.id)
                && text(&self.cargo, 256)
                && text(&self.rustc, 256)
                && !self.argv.is_empty()
                && self.argv.len() <= 64
                && self.argv.iter().all(|arg| text(arg, 512))
                && (self.cwd.as_os_str() == "." || relative(&self.cwd))
                && relative(&self.target_root)
                && relative(&self.build_root),
            "invalid invocation identity or shape",
        )?;
        require(
            self.environment.len() <= MAX_PROFILE_ENTRIES
                && unique(self.environment.iter().map(|item| &item.name)),
            "invalid environment inventory",
        )?;
        for variable in &self.environment {
            require(
                env_name(&variable.name),
                "invalid environment variable name",
            )?;
            match &variable.expectation {
                EnvironmentExpectation::Present { value } =>
                    require(value.is_empty() || text(value, 1_024), "invalid fixed environment value")?,
                EnvironmentExpectation::Mutable { input } => require(
                    inputs.iter().any(|item| {
                        item.id == *input && item.invocations.contains(&self.id)
                            && matches!(&item.locator, Locator::Environment { name } if name == &variable.name)
                    }),
                    "unbound mutable environment variable",
                )?,
                EnvironmentExpectation::Absent => (),
            }
        }
        Ok(())
    }
}

impl MutableInput {
    fn validate(&self, profile: &PoolProfile) -> FormatResult {
        self.locator.validate(false)?;
        require(
            identifier(&self.id)
                && bounded_unique(self.invocations.iter())
                && self
                    .invocations
                    .iter()
                    .all(|id| profile.invocations.iter().any(|item| item.id == *id)),
            "invalid mutable input or invocation binding",
        )?;
        if let Locator::Environment { name } = &self.locator {
            require(
                self.invocations.iter().all(|id| profile.invocations.iter().any(|item| {
                    item.id == *id && item.environment.iter().any(|variable| {
                        variable.name == *name
                            && matches!(&variable.expectation, EnvironmentExpectation::Mutable { input } if input == &self.id)
                    })
                })),
                "mutable environment input is missing from an invocation",
            )?;
        }
        match &self.coverage {
            Coverage::Consumers { ids } => {
                require(
                    bounded_unique(ids.iter()),
                    "invalid mutable input consumer set",
                )?;
                let mut covered = BTreeSet::new();
                for id in ids {
                    let Some(consumer) = profile.consumers.iter().find(|item| item.id == *id)
                    else {
                        return Err("mutable input references an unknown consumer");
                    };
                    require(
                        self.invocations.contains(&consumer.invocation),
                        "consumer invocation does not cover this input",
                    )?;
                    covered.insert(&consumer.invocation);
                }
                require(
                    covered.len() == self.invocations.len(),
                    "mutable input coverage is incomplete",
                )
            }
            Coverage::Excluded { proofs } => {
                require(
                    bounded_unique(proofs.iter()),
                    "invalid mutable input exclusion proof set",
                )?;
                let mut covered = BTreeSet::new();
                for id in proofs {
                    let Some(proof) = profile.proofs.iter().find(|item| item.id == *id) else {
                        return Err("mutable input exclusion references an unknown proof");
                    };
                    require(
                        self.invocations.contains(&proof.invocation),
                        "proof invocation does not cover this input",
                    )?;
                    covered.insert(&proof.invocation);
                }
                require(
                    covered.len() == self.invocations.len(),
                    "mutable input exclusion coverage is incomplete",
                )
            }
        }
    }
}

type FormatResult = Result<(), &'static str>;

fn require(condition: bool, detail: &'static str) -> FormatResult {
    if condition { Ok(()) } else { Err(detail) }
}

fn unique<T: Ord>(values: impl Iterator<Item = T>) -> bool {
    let mut seen = BTreeSet::new();
    values.into_iter().all(|value| seen.insert(value))
}

fn bounded_unique<T: Ord>(mut values: impl ExactSizeIterator<Item = T>) -> bool {
    (1..=MAX_PROFILE_ENTRIES).contains(&values.len()) && unique(&mut values)
}

pub fn hex(value: &str, size: usize) -> bool {
    value.len() == size
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub fn object_id(value: &str) -> bool {
    hex(value, 40) || hex(value, 64)
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
}

fn env_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.bytes().enumerate().all(|(idx, byte)| {
            byte == b'_' || byte.is_ascii_alphabetic() || (idx > 0 && byte.is_ascii_digit())
        })
}

fn text(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

fn absolute(path: &Path) -> bool {
    path.is_absolute() && valid_components(path)
}

fn relative(path: &Path) -> bool {
    !path.is_absolute() && valid_components(path)
}

fn valid_components(path: &Path) -> bool {
    let Some(value) = path.to_str() else {
        return false;
    };
    let relative = value.strip_prefix('/').unwrap_or(value);
    text(value, MAX_PATH_BYTES)
        && relative
            .split('/')
            .all(|part| !matches!(part, "" | "." | ".."))
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::RootDir))
}

pub fn owner_directory(path: &Path) -> io::Result<fs::Metadata> {
    if !absolute(path) {
        return Err(noncanonical_directory());
    }
    check_stable_ancestors(path)?;
    let metadata = checked_owner_directory(path)?;
    if path.canonicalize()?.as_os_str() != path.as_os_str() {
        return Err(noncanonical_directory());
    }
    Ok(metadata)
}

fn noncanonical_directory() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "expected a canonical owner-controlled directory",
    )
}

fn check_stable_ancestors(path: &Path) -> io::Result<()> {
    let ancestors = path.ancestors().skip(1).collect::<Vec<_>>();
    for ancestor in ancestors.into_iter().rev() {
        checked_stable_ancestor(ancestor)
            .map_err(|err| io::Error::new(err.kind(), format!("{}: {err}", ancestor.display())))?;
    }
    Ok(())
}

pub fn make_private_directory(path: &Path) -> io::Result<()> {
    fs::DirBuilder::new().mode(0o700).create(path)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    owner_directory(path)?;
    sync_directory(path)?;
    sync_directory(parent(path)?)
}

pub fn new_slot_lock(path: &Path) -> io::Result<FileIdentity> {
    let file = new_private_file(path)?;
    file.sync_all()?;
    sync_directory(parent(path)?)?;
    Ok(FileIdentity::of(&file.metadata()?))
}

pub fn checked_slot_lock(path: &Path) -> io::Result<FileIdentity> {
    open_slot_lock(path).map(|(_, identity)| identity)
}

pub fn open_slot_lock(path: &Path) -> io::Result<(File, FileIdentity)> {
    let file = open_exact_private(path)?;
    let metadata = file.metadata()?;
    if metadata.len() != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "private slot lock must be empty",
        ));
    }
    Ok((file, FileIdentity::of(&metadata)))
}

pub fn random_pool_id() -> io::Result<String> {
    let bytes = random_slot_bytes()?;
    Ok(encode_hex(&bytes[..16]))
}

/// Returns a slot credential from 32 bytes of platform entropy.
pub fn random_credential() -> io::Result<String> {
    random_slot_bytes().map(|bytes| encode_hex(&bytes))
}

fn encode_hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0xf)]));
    }
    encoded
}

fn open_exact_private(path: &Path) -> io::Result<File> {
    let file = open_private_file(path)?;
    if file.metadata()?.mode() & 0o7777 != 0o600 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "private file must have mode 0600",
        ));
    }
    Ok(file)
}

fn new_private_file(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    let result = (|| {
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        let checked = open_exact_private(path)?;
        if FileIdentity::of(&file.metadata()?) != FileIdentity::of(&checked.metadata()?) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "private file changed while opening",
            ));
        }
        Ok(())
    })();
    if let Err(err) = result {
        unlink_if_same_empty(path, &file);
        return Err(err);
    }
    Ok(file)
}

fn unlink_if_same_empty(path: &Path, file: &File) {
    if let (Ok(named), Ok(created)) = (fs::symlink_metadata(path), file.metadata())
        && named.len() == 0
        && FileIdentity::of(&named) == FileIdentity::of(&created)
    {
        let _ = fs::remove_file(path);
    }
}

fn parent(path: &Path) -> io::Result<&Path> {
    path.parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "private path lacks a parent"))
}

fn sync_directory(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

fn read_bounded(path: &Path, file: File) -> Result<Vec<u8>, StateError> {
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| format_error(path, err.kind().to_string()))?;
    if bytes.len() as u64 > MAX_BYTES {
        return Err(format_error(path, format!("larger than {MAX_BYTES} bytes")));
    }
    Ok(bytes)
}

fn parse_exact<T: DeserializeOwned + Serialize>(
    path: &Path,
    bytes: &[u8],
) -> Result<T, StateError> {
    let invalid = |err: serde_json::Error| {
        format_error(
            path,
            format!(
                "invalid JSON shape at line {} column {}",
                err.line(),
                err.column()
            ),
        )
    };
    let parsed = serde_json::from_slice(bytes).map_err(invalid)?;
    let original = serde_json::from_slice::<serde_json::Value>(bytes).map_err(invalid)?;
    let normalized = serde_json::to_value(&parsed).map_err(invalid)?;
    if normalized != original {
        return Err(format_error(
            path,
            "expected strict objects with all required fields",
        ));
    }
    Ok(parsed)
}

fn format_error(path: &Path, detail: impl Into<String>) -> StateError {
    StateError::Malformed {
        file: path.to_path_buf(),
        detail: detail.into(),
        on_read: true,
    }
}

fn decode(path: &Path, bytes: &[u8]) -> Result<SlotsFile, StateError> {
    check_layout(path, bytes)?;
    let mut slots: SlotsFile = parse_exact(path, bytes)?;
    slots
        .validate()
        .map_err(|detail| format_error(path, detail))?;
    slots.canonicalize();
    Ok(slots)
}

impl Tx<'_> {
    /// Missing is distinct from any unsafe, unsupported or malformed record.
    pub fn read_slots(&self) -> Result<Option<SlotsFile>, StateError> {
        let path = self.root.join(FILE);
        owner_directory(self.root).map_err(|err| format_error(&path, err.to_string()))?;
        let file = match open_exact_private(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return match fs::symlink_metadata(&path) {
                    Err(missing) if missing.kind() == io::ErrorKind::NotFound => Ok(None),
                    _ => Err(format_error(&path, "private inode changed while opening")),
                };
            }
            Err(err) => return Err(format_error(&path, err.to_string())),
        };
        decode(&path, &read_bounded(&path, file)?).map(Some)
    }

    /// Publishes a checked successor while the state lock remains held.
    pub fn commit_slots(&self, previous: Option<u64>, next: &SlotsFile) -> Result<(), StateError> {
        self.commit_slots_with(previous, next, |tmp, path| {
            fs::rename(tmp, path)?;
            sync_directory(parent(path)?)
        })
    }

    /// Syncs the exact visible record before a caller acknowledges a prior outcome.
    pub fn sync_slots(&self, expected: &SlotsFile) -> Result<(), StateError> {
        let path = self.root.join(FILE);
        owner_directory(self.root).map_err(|err| format_error(&path, err.to_string()))?;
        let file = open_exact_private(&path).map_err(|err| format_error(&path, err.to_string()))?;
        let reader = file.try_clone().map_err(|err| io_err(&path, err))?;
        let mut canonical = expected.clone();
        canonical.canonicalize();
        if decode(&path, &read_bounded(&path, reader)?)? != canonical {
            return Err(format_error(&path, "authoritative revision changed"));
        }
        file.sync_all()
            .and_then(|()| sync_directory(self.root))
            .map_err(|err| io_err(&path, err))
    }

    fn commit_slots_with(
        &self,
        previous: Option<u64>,
        next: &SlotsFile,
        publish: impl FnOnce(&Path, &Path) -> io::Result<()>,
    ) -> Result<(), StateError> {
        let path = self.root.join(FILE);
        let current = self.read_slots()?;
        if current.as_ref().map(|file| file.revision) != previous
            || previous.unwrap_or(0).checked_add(1) != Some(next.revision)
        {
            return Err(format_error(&path, "revision is stale or exhausted"));
        }
        let mut canonical = next.clone();
        canonical
            .validate()
            .map_err(|detail| format_error(&path, detail))?;
        canonical.canonicalize();
        if let Some(prior) = &current {
            canonical
                .validate_successor(prior)
                .map_err(|detail| format_error(&path, detail))?;
        }
        let bytes = encode(&path, &canonical)?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err(format_error(
                &path,
                format!("would be larger than {MAX_BYTES} bytes"),
            ));
        }

        let suffix = random_pool_id().map_err(|err| io_err(&path, err))?;
        let tmp = self.root.join(format!(".{FILE}.tmp-{suffix}"));
        let mut file =
            new_private_file(&tmp).map_err(|err| format_error(&path, err.to_string()))?;
        let result = file
            .write_all(&bytes)
            .and_then(|()| file.sync_all())
            .and_then(|()| publish(&tmp, &path));
        if let Err(err) = result {
            if let Ok(named) = fs::symlink_metadata(&tmp)
                && let Ok(created) = file.metadata()
                && FileIdentity::of(&named) == FileIdentity::of(&created)
            {
                let _ = fs::remove_file(&tmp);
            }
            if self.sync_slots(&canonical).is_ok() {
                return Ok(());
            }
            return Err(format_error(
                &path,
                format!("publication was not confirmed: {err}"),
            ));
        }
        Ok(())
    }
}

impl crate::State {
    /// Opens existing registered state without following a slot-state root alias.
    pub fn open_slot_state() -> Result<Self, StateError> {
        let root = Self::configured_root()?;
        let path = root.join(FILE);
        owner_directory(&root).map_err(|err| format_error(&path, err.to_string()))?;
        Ok(Self { root })
    }

    /// Shares the existing, positively identified store lock for a guarded seed.
    pub fn slot_store_shared_or_cancelled(
        &self,
        cancel: impl FnMut() -> bool,
    ) -> Result<crate::StoreLock, StateError> {
        use cargo_hive_platform::flock::{FlockError, shared_or_cancelled};
        let path = self.root.join("store.lock");
        owner_directory(&self.root).map_err(|err| io_err(&path, err))?;
        let file = open_owner_state_lock(&path).map_err(|err| io_err(&path, err))?;
        let guard = shared_or_cancelled(&file, cancel).map_err(|err| match err {
            FlockError::Cancelled => StateError::Cancelled,
            FlockError::Io(err) => io_err(&path, err),
        })?;
        Ok(crate::StoreLock {
            _guard: guard,
            _file: file,
        })
    }

    /// Uses the ordinary state lock through checked independent opens.
    pub fn slot_locked_or_cancelled<T>(
        &self,
        cancel: impl FnMut() -> bool,
        body: impl FnOnce(&Tx<'_>) -> Result<T, StateError>,
    ) -> Result<T, StateError> {
        let path = self.root.join(".lock");
        let root = owner_directory(&self.root).map_err(|err| io_err(&self.root, err))?;
        let file = match open_owner_state_lock(&path) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
            {
                Ok(_) => open_owner_state_lock(&path),
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                    open_owner_state_lock(&path)
                }
                Err(err) => Err(err),
            },
            result => result,
        }
        .map_err(|err| io_err(&path, err))?;
        let identity = file.metadata().map_err(|err| io_err(&path, err))?;
        self.with_state_lock(&path, &file, cancel, |tx| {
            let current_root =
                owner_directory(&self.root).map_err(|err| io_err(&self.root, err))?;
            let current = open_owner_state_lock(&path)
                .and_then(|file| file.metadata())
                .map_err(|err| io_err(&path, err))?;
            if FileIdentity::of(&root) != FileIdentity::of(&current_root)
                || FileIdentity::of(&identity) != FileIdentity::of(&current)
            {
                return Err(io_err(
                    &path,
                    io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "state lock scope changed while acquiring it. Preserve the lock files and ask the state owner to inspect them.",
                    ),
                ));
            }
            body(tx)
        })
    }

    /// Reads the shipped configuration within the slot state read bound.
    pub fn slot_config(&self) -> Result<crate::Config, StateError> {
        let path = self.root.join(crate::CONFIG_FILE);
        let file = match open_owner_state_file(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok(crate::resolve_config(crate::ConfigFile::default()));
            }
            Err(err) => return Err(io_err(&path, err)),
        };
        let config = crate::parse_json(&path, &read_bounded(&path, file)?)?;
        Ok(crate::resolve_config(config))
    }
}

impl Tx<'_> {
    /// Reads the existing registry for slot operations without accepting an unsafe inode.
    pub fn read_slot_registry<T: DeserializeOwned + Default>(&self) -> Result<T, StateError> {
        let path = self.root.join("registry.json");
        let file = match open_owner_state_file(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(T::default()),
            Err(err) => return Err(io_err(&path, err)),
        };
        let bytes = read_bounded(&path, file)?;
        check_layout(&path, &bytes)?;
        crate::parse_json(&path, &bytes)
    }

    /// Appends an ordinary receipt after verifying the existing write scope.
    pub fn append_slot_audit_line(&self, line: &str) -> Result<PathBuf, StateError> {
        let dir = self.root.join("receipts");
        owner_directory(&dir).map_err(|err| io_err(&dir, err))?;
        let path = self.receipt_path();
        let bytes = match open_owner_state_file(&path) {
            Ok(file) => read_bounded(&path, file)?,
            Err(err) if err.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(err) => return Err(io_err(&path, err)),
        };
        let temp = crate::atomic_temp_path(&path);
        match fs::symlink_metadata(&temp) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                crate::append_receipt_bytes(&path, bytes, line)
            }
            Err(err) => Err(io_err(&temp, err)),
            Ok(_) => Err(io_err(
                &temp,
                io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "receipt temporary inode already exists",
                ),
            )),
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::State;
    use cargo_hive_platform::slot::effective_uid;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let path = Path::new("/private/tmp")
                .join(format!("hive-slot-state-{}", random_pool_id().unwrap()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn slot_transactions_contend_with_the_legacy_state_lock() {
        let scratch = Scratch::new();
        let state = State::open_at(scratch.0.clone()).unwrap();
        state
            .locked(|_| {
                let err = state
                    .slot_locked_or_cancelled::<()>(
                        || true,
                        |_| panic!("legacy lock is still held"),
                    )
                    .unwrap_err();
                assert!(matches!(err, StateError::Cancelled));
                Ok(())
            })
            .unwrap();
        state
            .slot_locked_or_cancelled(
                || true,
                |tx| {
                    assert!(tx.read_slots()?.is_none());
                    Ok(())
                },
            )
            .unwrap();
    }

    #[test]
    fn a_slot_waiter_refuses_a_legacy_lock_replaced_while_waiting() {
        let scratch = Scratch::new();
        let state = State::open_at(scratch.0.clone()).unwrap();
        let lock = state.root().join(".lock");
        let previous = state.root().join("previous-lock");
        let (waiting, observed) = std::sync::mpsc::channel();
        std::thread::scope(|scope| {
            let (waiter, original) = state
                .locked(|_| {
                    let original = fs::metadata(&lock).unwrap().ino();
                    let waiter = scope.spawn(|| {
                        let mut first_poll = true;
                        state.slot_locked_or_cancelled::<()>(
                            || {
                                if std::mem::take(&mut first_poll) {
                                    waiting.send(()).unwrap();
                                }
                                false
                            },
                            |_| panic!("a transaction cannot use a replaced state lock"),
                        )
                    });
                    observed
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                    fs::rename(&lock, &previous).unwrap();
                    fs::write(&lock, []).unwrap();
                    Ok((waiter, original))
                })
                .unwrap();
            let error = waiter.join().unwrap().unwrap_err();
            let detail = error.to_string();
            assert!(detail.contains("state lock scope changed"), "{detail}");
            assert!(detail.contains("Preserve the lock files"), "{detail}");
            assert!(!detail.contains("remove"), "{detail}");
            assert_eq!(fs::metadata(&previous).unwrap().ino(), original);
            assert_ne!(fs::metadata(&lock).unwrap().ino(), original);
        });
    }

    #[test]
    fn a_failure_before_rename_keeps_the_old_revision_and_removes_the_private_temp_file() {
        let scratch = Scratch::new();
        let state = State::open_at(scratch.0.clone()).unwrap();
        state
            .locked(|tx| {
                let prior = SlotsFile::new(1, Vec::new());
                tx.commit_slots(None, &prior)?;
                let old = fs::read(state.root().join(FILE)).unwrap();
                let proposed = SlotsFile::new(2, Vec::new());
                let error = tx
                    .commit_slots_with(Some(1), &proposed, |temp, _| {
                        let metadata = fs::symlink_metadata(temp).unwrap();
                        assert_eq!(metadata.mode() & 0o7777, 0o600);
                        assert_eq!(metadata.nlink(), 1);
                        assert_eq!(metadata.uid(), effective_uid());
                        assert_eq!(fs::read(temp).unwrap(), encode(temp, &proposed).unwrap());
                        Err(io::Error::other("injected failure before rename"))
                    })
                    .unwrap_err();
                assert!(error.to_string().contains("publication was not confirmed"));
                assert_eq!(fs::read(state.root().join(FILE)).unwrap(), old);
                assert_eq!(fs::read_dir(state.root()).unwrap().count(), 3);
                assert_eq!(tx.read_slots()?.map(|file| file.revision), Some(1));
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn a_visible_post_rename_error_reconciles_only_the_exact_revision() {
        let scratch = Scratch::new();
        let state = State::open_at(scratch.0.clone()).unwrap();
        state
            .locked(|tx| {
                let initial = SlotsFile::new(1, Vec::new());
                tx.commit_slots_with(None, &initial, |temp, final_path| {
                    fs::rename(temp, final_path)?;
                    Err(io::Error::other("injected failure before directory sync"))
                })?;
                let next = SlotsFile::new(2, Vec::new());
                let different = SlotsFile::new(3, Vec::new());
                let error = tx
                    .commit_slots_with(Some(1), &next, |_, final_path| {
                        fs::write(final_path, encode(final_path, &different).unwrap())?;
                        Err(io::Error::other("injected unknown revision"))
                    })
                    .unwrap_err();
                assert!(error.to_string().contains("publication was not confirmed"));
                assert_eq!(tx.read_slots()?.map(|file| file.revision), Some(3));
                assert!(tx.commit_slots(Some(1), &next).is_err());
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn reservation_publication_faults_are_read_and_retried_in_a_new_process() {
        const RESTART_CASE: &str = "CARGO_HIVE_TEST_RESERVATION_RESTART";
        const RESTART_GENERATION: &str = "CARGO_HIVE_TEST_RESERVATION_GENERATION";
        if let Some(root) = std::env::var_os(RESTART_CASE) {
            let root = PathBuf::from(root);
            assert_eq!(root.parent(), Some(Path::new("/private/tmp")));
            assert!(
                root.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("hive-slot-state-")
            );
            let generation = std::env::var(RESTART_GENERATION).unwrap().parse().unwrap();
            retry_faulted_reservation(root, generation);
            return;
        }

        for rename_visible in [false, true] {
            let scratch = Scratch::new();
            let state = State::open_at(scratch.0.clone()).unwrap();
            let (initial, reserved) = reservation_records(state.root());
            let expected = state.root().join("expected-reservation.json");
            new_private_file(&expected)
                .unwrap()
                .write_all(&encode(&expected, &reserved).unwrap())
                .unwrap();
            state
                .locked(|tx| {
                    tx.commit_slots(None, &initial)?;
                    let result = tx.commit_slots_with(Some(1), &reserved, |temp, final_path| {
                        if rename_visible {
                            fs::rename(temp, final_path)?;
                        }
                        Err(io::Error::other("injected reservation publication failure"))
                    });
                    assert_eq!(result.is_ok(), rename_visible);
                    Ok(())
                })
                .unwrap();

            let mut spec = cargo_hive_platform::proc::Spec::new(std::env::current_exe().unwrap());
            spec.args.extend(["--exact".into(), "slots::tests::reservation_publication_faults_are_read_and_retried_in_a_new_process".into()]);
            spec.env_set
                .push((RESTART_CASE.into(), scratch.0.as_os_str().to_owned()));
            spec.env_set.push((
                RESTART_GENERATION.into(),
                u8::from(rename_visible).to_string().into(),
            ));
            let observed = cargo_hive_platform::proc::run_bounded(
                &spec,
                std::time::Duration::from_secs(10),
                cargo_hive_platform::proc::Capture {
                    stdout_cap: 4096,
                    stderr_cap: 4096,
                },
                || false,
            )
            .unwrap();
            assert!(
                observed.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&observed.stdout),
                String::from_utf8_lossy(&observed.stderr_tail)
            );
            state
                .locked(|tx| {
                    assert!(tx.read_slots()?.as_ref() == Some(&reserved));
                    assert!(tx.commit_slots(Some(1), &reserved).is_err());
                    Ok(())
                })
                .unwrap();
        }
    }

    fn retry_faulted_reservation(root: PathBuf, initial_generation: u64) {
        let state = State::open_at(root).unwrap();
        let expected = state.root().join("expected-reservation.json");
        let file = open_exact_private(&expected).unwrap();
        let proposed = decode(&expected, &read_bounded(&expected, file).unwrap()).unwrap();
        state
            .slot_locked_or_cancelled(
                || false,
                |tx| {
                    let observed = tx.read_slots()?.unwrap();
                    let slot = &observed.pools[0].slots[0];
                    assert_eq!(slot.generation, initial_generation);
                    if initial_generation == 0 {
                        assert_eq!(observed.revision, 1);
                        assert!(matches!(slot.phase, Phase::Vacant));
                        assert!(slot.outcomes.is_empty());
                        tx.commit_slots(Some(observed.revision), &proposed)?;
                    } else {
                        assert_eq!(observed.revision, 2);
                        assert!(observed == proposed);
                        tx.sync_slots(&observed)?;
                    }
                    let authoritative = tx.read_slots()?.unwrap();
                    assert!(authoritative == proposed);
                    let slot = &authoritative.pools[0].slots[0];
                    assert_eq!(slot.generation, 1);
                    assert_eq!(slot.outcomes.len(), 1);
                    assert!(matches!(slot.phase, Phase::Reserved { .. }));
                    Ok(())
                },
            )
            .unwrap();
    }

    #[test]
    fn ready_publication_reopens_only_the_exact_committed_outcome() {
        for rename_visible in [false, true] {
            let scratch = Scratch::new();
            let (initial, reserved) = reservation_records(&scratch.0);
            let (preparing, ready) = ready_publication_records(&reserved);
            State::open_at(scratch.0.clone())
                .unwrap()
                .locked(|tx| {
                    tx.commit_slots(None, &initial)?;
                    tx.commit_slots(Some(initial.revision), &reserved)?;
                    tx.commit_slots(Some(reserved.revision), &preparing)?;
                    let publication =
                        tx.commit_slots_with(Some(preparing.revision), &ready, |temp, path| {
                            if rename_visible {
                                fs::rename(temp, path)?;
                            }
                            Err(io::Error::other("injected ready publication failure"))
                        });
                    if rename_visible {
                        publication?;
                    } else {
                        let error = publication.unwrap_err().to_string();
                        assert!(
                            error.contains("injected ready publication failure"),
                            "{error}"
                        );
                    }
                    Ok(())
                })
                .unwrap();

            let reader = State::open_at(scratch.0.clone()).unwrap();
            reader
                .slot_locked_or_cancelled(
                    || false,
                    |tx| {
                        let visible = tx.read_slots()?.unwrap();
                        if !rename_visible {
                            assert!(
                                visible == preparing,
                                "the prior lease and permit must survive"
                            );
                            assert!(tx.sync_slots(&ready).is_err());
                            tx.commit_slots(Some(preparing.revision), &ready)?;
                        } else {
                            assert!(visible == ready, "the published Ready record must survive");
                        }

                        let recovered = tx.read_slots()?.unwrap();
                        tx.sync_slots(&recovered)?;
                        assert!(recovered == ready, "one exact Ready outcome must survive");
                        assert!(tx.commit_slots(Some(preparing.revision), &ready).is_err());

                        let mut wrong_runner = recovered.clone();
                        let outcome = wrong_runner.pools[0].slots[0].outcomes.last_mut().unwrap();
                        let OutcomeRequest::Ready { runner } = &mut outcome.request else {
                            panic!("expected a Ready request");
                        };
                        runner.nonce = "f".repeat(64);
                        assert!(tx.sync_slots(&wrong_runner).is_err());

                        let mut vacant = recovered;
                        vacant.revision += 1;
                        vacant.pools[0].slots[0].phase = Phase::Vacant;
                        tx.commit_slots(Some(ready.revision), &vacant)?;
                        let mut renewed = vacant.clone();
                        renewed.revision += 1;
                        let renewed_slot = &mut renewed.pools[0].slots[0];
                        renewed_slot.generation += 1;
                        renewed_slot.phase = Phase::Reserved {
                            lease: Lease {
                                requested_commit: "d".repeat(40),
                                credential: "e".repeat(64),
                                reservation_op: "4".repeat(32),
                            },
                        };
                        tx.commit_slots(Some(vacant.revision), &renewed)?;
                        assert!(tx.sync_slots(&ready).is_err());
                        let mut stale_ready = ready.clone();
                        stale_ready.revision = renewed.revision + 1;
                        let error = tx
                            .commit_slots(Some(renewed.revision), &stale_ready)
                            .unwrap_err()
                            .to_string();
                        assert!(
                            error.contains("slot generation is stale or skipped"),
                            "{error}"
                        );
                        assert!(tx.read_slots()?.as_ref() == Some(&renewed));
                        Ok(())
                    },
                )
                .unwrap();
        }
    }

    fn ready_publication_records(reserved: &SlotsFile) -> (SlotsFile, SlotsFile) {
        let mut preparing = reserved.clone();
        preparing.revision += 1;
        let pool = &mut preparing.pools[0];
        let root = pool.checkout_path(&pool.slots[0]).join("target");
        let slot = &mut pool.slots[0];
        let Phase::Reserved { lease } = &slot.phase else {
            panic!("fixture must be reserved");
        };
        let lease = lease.clone();
        let runner = Runner {
            boot_session: "boot".into(),
            pid: 2,
            start_secs: 1,
            start_micros: 0,
            nonce: "d".repeat(64),
        };
        let attempt_id = 1;
        let begin_op = "2".repeat(32);
        slot.outcomes.push(Outcome {
            pool_id: pool.pool_id.clone(),
            slot: slot.number,
            generation: slot.generation,
            operation: begin_op.clone(),
            kind: OperationKind::Begin,
            attempt_id: Some(attempt_id),
            request: OutcomeRequest::Begin {
                runner: runner.clone(),
            },
            credential: lease.credential.clone(),
            result: TransitionResult::PermitIssued,
        });
        let checkout = CheckoutIdentity {
            directory: FileIdentity { dev: 1, ino: 4 },
            admin: GitIdentity {
                path: pool.common_git_id.path.join("worktrees/slot-1"),
                dev: 1,
                ino: 5,
            },
        };
        slot.checkout_id = Some(checkout.clone());
        slot.producer = Producer::NeverBuilt {
            evidence: Some(EmptyEpoch {
                epoch: OutputEpoch {
                    pool_id: pool.pool_id.clone(),
                    slot: slot.number,
                    checkout,
                },
                roots: OutputRoots {
                    target: RootProof::Absent { path: root.clone() },
                    build: RootProof::Absent { path: root },
                },
            }),
        };
        slot.last_head = Some(lease.requested_commit.clone());
        slot.phase = Phase::Preparing {
            lease: lease.clone(),
            attempt: Attempt {
                attempt_id,
                begin_op,
                runner: runner.clone(),
                host_binding: None,
                stage: AttemptStage::GcInProgress,
                lock_offset: Some(1 << 32),
                prior_head: None,
                anchors: Vec::new(),
                anchor_cursor: 0,
            },
        };

        let mut ready = preparing.clone();
        ready.revision += 1;
        let pool = &mut ready.pools[0];
        let slot = &mut pool.slots[0];
        let commit = lease.requested_commit.clone();
        let credential = lease.credential.clone();
        slot.phase = Phase::Leased {
            lease,
            ready_commit: commit.clone(),
        };
        slot.outcomes.push(Outcome {
            pool_id: pool.pool_id.clone(),
            slot: slot.number,
            generation: slot.generation,
            operation: "3".repeat(32),
            kind: OperationKind::Ready,
            attempt_id: Some(attempt_id),
            request: OutcomeRequest::Ready { runner },
            credential,
            result: TransitionResult::Ready { commit },
        });
        (preparing, ready)
    }

    #[test]
    fn retained_output_cannot_reset_lost_or_interrupted_provenance() {
        let scratch = Scratch::new();
        let state = State::open_at(scratch.0.clone()).unwrap();
        let (_, mut initial) = reservation_records(state.root());
        let pool = &mut initial.pools[0];
        let root = pool.checkout_path(&pool.slots[0]).join("target");
        let slot = &mut pool.slots[0];
        let checkout = CheckoutIdentity {
            directory: FileIdentity { dev: 1, ino: 4 },
            admin: GitIdentity {
                path: pool.main.join(".git/worktrees/other"),
                dev: 1,
                ino: 5,
            },
        };
        let epoch = OutputEpoch {
            pool_id: pool.pool_id.clone(),
            slot: 1,
            checkout: checkout.clone(),
        };
        let Phase::Reserved { lease } = &slot.phase else {
            panic!("fixture must be reserved");
        };
        slot.checkout_id = Some(checkout);
        slot.last_head = Some(lease.requested_commit.clone());
        slot.phase = Phase::Leased {
            lease: lease.clone(),
            ready_commit: lease.requested_commit.clone(),
        };
        let evidence = EmptyEpoch {
            epoch: epoch.clone(),
            roots: OutputRoots {
                target: RootProof::Absent { path: root.clone() },
                build: RootProof::Absent { path: root.clone() },
            },
        };
        let reasons = OutputRoots {
            target: None,
            build: None,
        };
        let pending = Producer::Pending(Box::new(PendingProducer {
            epoch: epoch.clone(),
            generation: 1,
            profile: ProfileBinding {
                id: "fixture".into(),
                revision: 1,
            },
            invocation: InvocationEvidence {
                invocation: "check".into(),
                graph_digest: "a".repeat(64),
                environment_digest: "b".repeat(64),
                stable_digest: None,
            },
            roots: OutputRoots {
                target: RootExpectation::Absent { path: root.clone() },
                build: RootExpectation::Absent { path: root },
            },
            operation: "4".repeat(32),
            runner: Runner {
                boot_session: "boot".into(),
                pid: 2,
                start_secs: 1,
                start_micros: 0,
                nonce: "c".repeat(64),
            },
            prior: EligibleProducer::NeverBuilt {
                evidence: evidence.clone(),
            },
            foreground: None,
            guide_guard_offset: None,
        }));
        for producer in [
            pending,
            Producer::Unknown {
                epoch: epoch.clone(),
                generation: 1,
                roots: reasons.clone(),
                reason: ProvenanceLoss::LostWriter,
            },
            Producer::Tainted {
                epoch,
                generation: 1,
                roots: reasons,
                reason: ProvenanceLoss::OffProfile,
                report: None,
            },
        ] {
            let isolated = Scratch::new();
            let state = State::open_at(isolated.0.clone()).unwrap();
            initial.revision = 1;
            initial.pools[0].slots[0].producer = producer;
            state
                .locked(|tx| {
                    tx.commit_slots(None, &initial)?;
                    let mut proposed = initial.clone();
                    proposed.revision = 2;
                    proposed.pools[0].slots[0].producer = Producer::NeverBuilt {
                        evidence: Some(evidence.clone()),
                    };
                    assert!(tx.commit_slots(Some(1), &proposed).is_err());
                    assert!(tx.read_slots()?.as_ref() == Some(&initial));
                    Ok(())
                })
                .unwrap();
        }
    }

    fn reservation_records(root: &Path) -> (SlotsFile, SlotsFile) {
        let main = root.join("main");
        let common = GitIdentity {
            path: main.join(".git"),
            dev: 1,
            ino: 1,
        };
        let profile: PoolProfile = serde_json::from_value(serde_json::json!({
            "schema": 1, "id": "fixture", "revision": 1, "common_git_id": common,
            "graph_inputs": [{"locator": {"kind": "checkout", "path": "Cargo.lock"}, "expectation": {"kind": "absent"}}],
            "invocations": [{"id": "check", "cargo": "cargo", "rustc": "rustc", "layout": "flat", "argv": ["check"], "cwd": ".", "environment": [], "target_root": "target", "build_root": "target"}],
            "mutable_inputs": [{"id": "source", "locator": {"kind": "checkout", "path": "src/lib.rs"}, "presence": "present", "invocations": ["check"], "coverage": {"kind": "excluded", "proofs": ["fixture-proof"]}}],
            "consumers": [],
            "proofs": [{"id": "fixture-proof", "invocation": "check", "layout": "flat", "evidence": "fixture", "unaffected_unit": "fixture"}]
        })).unwrap();
        let pool_id = "a".repeat(32);
        let pool = Pool {
            pool_id: pool_id.clone(),
            main,
            common_git_id: common,
            root: root.join("secondary"),
            profile,
            slots: [
                Slot::unused(1, FileIdentity { dev: 1, ino: 2 }),
                Slot::unused(2, FileIdentity { dev: 1, ino: 3 }),
            ],
        };
        let initial = SlotsFile::new(1, vec![pool]);
        let mut reserved = initial.clone();
        reserved.revision = 2;
        let slot = &mut reserved.pools[0].slots[0];
        let operation = "1".repeat(32);
        let credential = "b".repeat(64);
        let commit = "c".repeat(40);
        slot.generation = 1;
        slot.phase = Phase::Reserved {
            lease: Lease {
                requested_commit: commit.clone(),
                credential: credential.clone(),
                reservation_op: operation.clone(),
            },
        };
        slot.outcomes.push(Outcome {
            pool_id,
            slot: 1,
            generation: 1,
            operation,
            kind: OperationKind::Reserve,
            attempt_id: None,
            request: OutcomeRequest::Reserve {
                expected_generation: 0,
                commit,
            },
            credential,
            result: TransitionResult::Reserved,
        });
        (initial, reserved)
    }
}
