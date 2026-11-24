//! The slot CLI scopes operations to a registered main and its private pool namespace.

mod admission;
mod inspect;
mod prepare;
mod profile;
mod recover;
mod result;

use crate::cli::{SlotCommand, SlotLockedOperation, SlotOperation, SlotReplay, SlotScopeArgs};
use cargo_hive_cargo::{
    git::{Git, SlotGitError, WorktreeRecord},
    registry::{self, Registry},
};
use cargo_hive_platform::flock::{
    InheritedLock, LockFileIdentity, duplicate_inherited_fd, private_file_metadata, try_exclusive,
    try_lock_inherited,
};
use cargo_hive_platform::proc::{self, read_stdin_complete};
use cargo_hive_platform::signals::{self, cancelled};
use cargo_hive_platform::slot::{
    ForegroundExec, ForegroundWatch, ProcessInstance, boot_witness, descriptor_writable,
    parent_instance, verify_guide_lock,
};
use cargo_hive_state::slots::{
    self, Attempt, AttemptStage, BypassReport, CHECKOUT_NAMESPACE, CompletedInvocation,
    CompletedPreparation, EligibleProducer, FileIdentity, ForegroundInstance, GitIdentity,
    LOCK_NAMESPACE, Lease, MAX_OUTCOMES, MAX_POOLS, OperationKind, Outcome, OutcomeRequest,
    OutputEpoch, OutputRoots, PendingProducer, Phase, Pool, PoolProfile, Producer, ProvenanceLoss,
    RootExpectation, Runner, Slot, SlotRequestKey, SlotsFile, TransitionResult,
};
use cargo_hive_state::{Config, State, StateError, Tx};
pub(crate) use inspect::CheckoutProof;
use inspect::{
    CheckoutInventory, ExpectedCheckout, ProducerInventory, SlotScope, directory_stamp,
    inspect_checkout, inspect_producer_checkout, owned_identity,
};
use profile::InvocationInput;
use result::{FRESH_FALLBACK, SlotResult, SlotStatus, SlotTransition};
use serde::Deserialize;
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, Seek, Write};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

const HELD_ACTION: &str = "Preserve the slot files and ask the repository operator to inspect identity, quiescence and unfinished work. Create a separate fresh secondary checkout and run `cargo hive seed <checkout>` there.";
const LOST_PROVENANCE: &str = "This slot does not regain warm capacity for these retained outputs in this version. Create a separate fresh secondary checkout and run `cargo hive seed <checkout>` there.";
const OUTCOME_LIMIT_ACTION: &str = "Retry the exact unresolved request or have the guide confirm its delivered result before reserving.";
const GIT_OUTPUT_LIMIT: usize = 64 * 1024;
const MAX_WORKTREES: usize = 256;
const MAX_ROOT_ENTRIES: usize = 4096;
const MAX_ROOT_DEPTH: usize = 32;
const KEY_HEX_LEN: usize = 64;
const CREDENTIAL_DEADLINE: Duration = Duration::from_secs(2);
const GUIDE_ANCHOR_ACTION: &str = "exec /usr/bin/touch -m -c -- /dev/fd/0";
const PRODUCER_GATE_PROGRAM: &str = "/bin/bash";
const PRODUCER_ENV_PROGRAM: &str = "/usr/bin/env";
const PRODUCER_GATE_ARGS: [&str; 5] = [
    "--noprofile",
    "--norc",
    "-c",
    "builtin kill -STOP \"$$\"; builtin exec -c /usr/bin/env -i \"$@\"",
    "producer-gate",
];

pub(super) fn dispatch(command: SlotCommand) -> i32 {
    let (name, repo, json, init) = match command {
        SlotCommand::Init {
            repo,
            root,
            profile,
            out,
        } => ("slot init", repo, out.json, Some((root, profile))),
        SlotCommand::Status { repo, out } => ("slot status", repo, out.json, None),
        SlotCommand::Recover(op) => {
            return run_scoped("slot recover", &op.scope.repo, op.out.json, |state| {
                recover::run(state, &op)
            });
        }
        SlotCommand::Key { out_fd, out } => return generate_key(out_fd).emit(out.json),
        SlotCommand::Reserve {
            scope,
            expect,
            commit,
            op,
            out,
        } => {
            return run_scoped("slot reserve", &scope.repo, out.json, |state| {
                reserve(state, &scope, expect, &commit, &op)
            });
        }
        SlotCommand::Begin {
            request,
            control_fd,
        } => {
            return run_scoped(
                "slot begin",
                &request.operation.scope.repo,
                request.operation.out.json,
                |state| begin(state, &request, control_fd),
            );
        }
        SlotCommand::StepCheck {
            scope,
            generation,
            attempt,
            lock_fd,
            out,
        } => {
            return run_scoped("slot step-check", &scope.repo, out.json, |state| {
                step_check(state, &scope, generation, attempt, lock_fd)
            });
        }
        SlotCommand::Producer {
            request,
            control_fd,
        } => {
            return run_scoped(
                "slot producer",
                &request.operation.scope.repo,
                request.operation.out.json,
                |state| producer(state, &request, control_fd),
            );
        }
        SlotCommand::Ready { request, attempt } => {
            return run_scoped(
                "slot ready",
                &request.operation.scope.repo,
                request.operation.out.json,
                |state| ready(state, &request, attempt),
            );
        }
        SlotCommand::Return(op) => {
            return run_scoped("slot return", &op.scope.repo, op.out.json, |state| {
                return_slot(state, &op.scope, op.generation, &op.op)
            });
        }
        SlotCommand::Outcome(op) => return dispatch_replay(&op, ReplayAction::Outcome),
        SlotCommand::Ack(op) => return dispatch_replay(&op, ReplayAction::Acknowledge),
    };
    run_scoped(name, &repo, json, |state| {
        with_slot_state(state, |tx| inspect_or_init(tx, state, name, &repo, init))
    })
}

fn generate_key(fd: u32) -> SlotResult {
    const COMMAND: &str = "slot key";
    let result = match write_key(fd) {
        Ok(()) => return SlotResult::new(COMMAND, "key_generated", None, 0),
        Err(_) => {
            "Could not deliver the slot credential. Pass a fresh owner-only 0600 file on a separate inherited descriptor."
        }
    };
    let mut refused = SlotResult::new(COMMAND, "refused", None, 1);
    refused.detail = Some(result.into());
    refused.next_action = Some("Use the guide to create the private slot credential.");
    refused
}

fn private_output(fd: u32, excluded: Option<FileIdentity>) -> io::Result<File> {
    let invalid = || {
        io::Error::new(
            io::ErrorKind::PermissionDenied,
            "invalid private output descriptor",
        )
    };
    let fd = i32::try_from(fd)
        .ok()
        .filter(|&fd| fd > 2)
        .ok_or_else(invalid)?;
    let mut file = File::from(duplicate_inherited_fd(fd)?);
    if !descriptor_writable(file.as_fd())? {
        return Err(invalid());
    }
    let metadata = private_file_metadata(&file)?;
    let identity = FileIdentity::of(&metadata);
    if metadata.mode() & 0o7777 != 0o600
        || metadata.len() != 0
        || file.stream_position()? != 0
        || excluded == Some(identity)
    {
        return Err(invalid());
    }
    for standard in 0..=2 {
        let metadata = File::from(duplicate_inherited_fd(standard)?).metadata()?;
        if FileIdentity::of(&metadata) == identity {
            return Err(invalid());
        }
    }
    Ok(file)
}

fn write_key(fd: u32) -> io::Result<()> {
    let mut file = private_output(fd, None)?;
    let mut key = slots::random_credential()?;
    key.push('\n');
    file.write_all(key.as_bytes())
}

fn with_slot_state<T>(
    state: &State,
    action: impl FnOnce(&Tx<'_>) -> Result<T, Refusal>,
) -> Result<T, Refusal> {
    state.slot_locked_or_cancelled(cancelled, |tx| Ok(action(tx)))?
}

fn run_scoped(
    name: &'static str,
    repo: &Path,
    json: bool,
    action: impl FnOnce(&State) -> Result<SlotResult, Refusal>,
) -> i32 {
    if let Err(err) = signals::install() {
        return refusal(
            name,
            repo,
            Refusal::Scope(format!("Could not install signal handlers: {err}")),
        )
        .emit(json);
    }
    let state = match State::open_slot_state() {
        Ok(state) => state,
        Err(err) => return refusal(name, repo, err.into()).emit(json),
    };
    let result = match action(&state) {
        Ok(result) => result,
        Err(err) => refusal(name, repo, err),
    };
    match serde_json::to_string(&result.audit_receipt()) {
        Ok(line) => {
            if let Err(err) = state.slot_locked_or_cancelled(cancelled, |tx| {
                tx.append_slot_audit_line(&line).map(|_| ())
            }) {
                let _ = writeln!(
                    io::stderr().lock(),
                    "cargo-hive: could not log the slot receipt: {err}"
                );
            }
        }
        Err(_) => {
            let _ = writeln!(
                io::stderr().lock(),
                "cargo-hive: could not encode the slot receipt"
            );
        }
    }
    result.emit(json)
}

enum Refusal {
    State(StateError),
    Scope(String),
    Orphan(PathBuf, PathBuf),
    Capacity(&'static str),
    ReturnWork(&'static str),
    ReturnUncertain(&'static str),
    OutcomeLimit,
    ProducerHistoryLimit,
}

impl From<StateError> for Refusal {
    fn from(error: StateError) -> Self {
        Self::State(error)
    }
}

fn refusal(command: &'static str, repo: &Path, err: Refusal) -> SlotResult {
    let (kind, code, detail, next) = match err {
        Refusal::State(StateError::Cancelled) => (
            "cancelled",
            130,
            "The slot operation was cancelled.".into(),
            "Retry when ready.",
        ),
        Refusal::State(err) => ("refused", 1, err.to_string(), FRESH_FALLBACK),
        Refusal::Scope(detail) => ("refused", 1, detail, FRESH_FALLBACK),
        Refusal::Capacity(detail) => ("unavailable", 1, detail.into(), FRESH_FALLBACK),
        Refusal::ReturnWork(detail) => (
            "refused",
            1,
            detail.into(),
            "Resolve the unfinished work deliberately, then retry the original return.",
        ),
        Refusal::ReturnUncertain(detail) => ("refused", 1, detail.into(), HELD_ACTION),
        Refusal::OutcomeLimit => (
            "unavailable",
            1,
            format!(
                "The slot already has {MAX_OUTCOMES} unacknowledged outcomes. {OUTCOME_LIMIT_ACTION}"
            ),
            FRESH_FALLBACK,
        ),
        Refusal::ProducerHistoryLimit => (
            "unavailable",
            1,
            "The current lease has reached the limit for completed Cargo invocations.".into(),
            "Resolve unfinished work, then return the slot or use a separate fresh seeded checkout.",
        ),
        Refusal::Orphan(record, evidence) => (
            "held",
            1,
            format!(
                "{} cannot establish the last authoritative pool state for {}. Preserve the existing files. Ask the repository operator to restore only a proven current epoch, or explicitly quarantine the old namespace before initializing at a non-overlapping root.",
                record.display(),
                evidence.display()
            ),
            FRESH_FALLBACK,
        ),
    };
    let mut result = SlotResult::new(command, kind, Some(repo.to_path_buf()), code);
    result.detail = Some(detail);
    result.next_action = Some(next);
    result
}

struct SlotOperationInput {
    credential: String,
}

impl SlotOperationInput {
    fn read() -> Result<Self, Refusal> {
        let data = read_stdin_complete(KEY_HEX_LEN + 1, CREDENTIAL_DEADLINE, cancelled).map_err(
            |err| match err.kind() {
                io::ErrorKind::Interrupted => StateError::Cancelled.into(),
                _ => credential_refusal(),
            },
        )?;
        let key = data.strip_suffix(b"\n").unwrap_or(&data);
        let credential = std::str::from_utf8(key)
            .ok()
            .filter(|key| slots::hex(key, KEY_HEX_LEN))
            .ok_or_else(credential_refusal)?;
        Ok(Self {
            credential: credential.into(),
        })
    }
}

fn check_cancelled() -> Result<(), Refusal> {
    if cancelled() {
        return Err(StateError::Cancelled.into());
    }
    Ok(())
}

fn credential_refusal() -> Refusal {
    Refusal::Scope("Could not verify the private slot credential. Use the guide's original credential or ask the repository operator to inspect the lease.".into())
}

fn request_key<'a>(
    scope: &'a SlotScopeArgs,
    generation: u64,
    kind: OperationKind,
    operation: &'a str,
    attempt_id: Option<u64>,
) -> Result<SlotRequestKey<'a>, Refusal> {
    if !slots::hex(&scope.pool, 32) || !(1..=2).contains(&scope.slot) || !slots::hex(operation, 32)
    {
        return Err(Refusal::Scope("Name the current pool and slot from `cargo hive slot status` and supply a valid operation ID.".into()));
    }
    Ok(SlotRequestKey {
        pool_id: &scope.pool,
        slot: scope.slot,
        generation,
        attempt_id,
        kind,
        operation,
    })
}

fn reserve(
    state: &State,
    scope: &SlotScopeArgs,
    expected_generation: u64,
    commit: &str,
    operation: &str,
) -> Result<SlotResult, Refusal> {
    const COMMAND: &str = "slot reserve";
    let generation = expected_generation.checked_add(1).ok_or(Refusal::Capacity(
        "The slot generation counter is exhausted.",
    ))?;
    let key = request_key(scope, generation, OperationKind::Reserve, operation, None)?;
    if !slots::object_id(commit) {
        return Err(commit_refusal());
    }
    let input = SlotOperationInput::read()?;
    let registered = verify_operation_repo(state, &scope.repo)?;
    let commit_verified = match git_scope(
        &scope.repo,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{commit}^{{commit}}"),
        ],
    ) {
        Ok(resolved) => resolved.strip_suffix(b"\n") == Some(commit.as_bytes()),
        Err(err @ Refusal::State(StateError::Cancelled)) => return Err(err),
        Err(_) => false,
    };
    with_slot_state(state, |tx| {
        let (mut saved, index) = checked_pool(tx, state, scope, &registered)?;
        let pool = &mut saved.pools[index];
        let slot_index = usize::from(key.slot - 1);
        let slot = &pool.slots[slot_index];
        let request = OutcomeRequest::Reserve {
            expected_generation,
            commit: commit.into(),
        };
        if let Some(outcome) = slot
            .outcomes
            .iter()
            .find(|outcome| key.matches_identity(outcome))
        {
            if outcome.kind != key.kind
                || outcome.request != request
                || outcome.credential != input.credential
            {
                return Err(Refusal::Scope("The operation ID is already bound to a different request. Use the guide's exact original request.".into()));
            }
            let result =
                reservation_result(COMMAND, "reserved", pool, slot, outcome, state.root())?;
            tx.sync_slots(&saved)?;
            return Ok(result);
        }
        if slot.outcomes.len() >= MAX_OUTCOMES {
            return Err(Refusal::OutcomeLimit);
        }
        if !matches!(slot.phase, Phase::Vacant)
            || !slot_producer_is_eligible(slot)
            || !slot_identity_present(state.root(), pool, slot)
        {
            return Err(Refusal::Capacity(
                "The requested slot is unavailable. Check current pool capacity with `cargo hive slot status`.",
            ));
        }
        if slot.generation != expected_generation {
            return Err(Refusal::Scope("The slot generation is stale. Check the current pool with `cargo hive slot status`.".into()));
        }
        if !commit_verified {
            return Err(commit_refusal());
        }
        check_cancelled()?;
        let slot = &mut pool.slots[slot_index];
        slot.generation = key.generation;
        slot.phase = Phase::Reserved {
            lease: Lease {
                requested_commit: commit.into(),
                credential: input.credential.clone(),
                reservation_op: key.operation.into(),
            },
        };
        let outcome = Outcome {
            pool_id: pool.pool_id.clone(),
            slot: key.slot,
            generation: key.generation,
            operation: key.operation.into(),
            kind: key.kind,
            attempt_id: key.attempt_id,
            request,
            credential: input.credential,
            result: TransitionResult::Reserved,
        };
        let result = reservation_result(
            COMMAND,
            "reserved",
            pool,
            &pool.slots[slot_index],
            &outcome,
            state.root(),
        )?;
        pool.slots[slot_index].outcomes.push(outcome);
        commit_slot_update(tx, &mut saved)?;
        Ok(result)
    })
}

fn commit_refusal() -> Refusal {
    Refusal::Scope(
        "Supply the full object ID of an existing commit in this registered repository.".into(),
    )
}

struct ReturnScope {
    pool: Pool,
    registry: Registry,
    config: Config,
}

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum ProducerInput {
    Start {
        credential: String,
        nonce: String,
        shape: InvocationInput,
    },
    Watch {
        credential: String,
        nonce: String,
        shape: InvocationInput,
        child_pid: u32,
    },
    GuidedStart {
        credential: String,
        nonce: String,
        invocation: String,
    },
    GuidedWatch {
        credential: String,
        nonce: String,
        invocation: String,
        child_pid: u32,
    },
    ReportBypass {
        credential: String,
        nonce: String,
        profile: slots::ProfileBinding,
    },
}

impl ProducerInput {
    fn authentication(&self) -> (&str, &str) {
        match self {
            Self::Start {
                credential, nonce, ..
            }
            | Self::Watch {
                credential, nonce, ..
            }
            | Self::GuidedStart {
                credential, nonce, ..
            }
            | Self::GuidedWatch {
                credential, nonce, ..
            }
            | Self::ReportBypass {
                credential, nonce, ..
            } => (credential, nonce),
        }
    }
}

struct ProducerInvocation {
    credential: String,
    shape: InvocationInput,
}

fn producer(
    state: &State,
    request: &SlotLockedOperation,
    control_fd: Option<u32>,
) -> Result<SlotResult, Refusal> {
    let op = &request.operation;
    let key = request_key(
        &op.scope,
        op.generation,
        OperationKind::ProducerStart,
        &op.op,
        None,
    )?;
    let input = read_stdin_complete(64 * 1024, CREDENTIAL_DEADLINE, cancelled).map_err(|err| {
        if err.kind() == io::ErrorKind::Interrupted {
            StateError::Cancelled.into()
        } else {
            credential_refusal()
        }
    })?;
    let input: ProducerInput = serde_json::from_slice(&input).map_err(|_| credential_refusal())?;
    let (credential, nonce) = input.authentication();
    if !slots::hex(credential, KEY_HEX_LEN) || !slots::hex(nonce, KEY_HEX_LEN) {
        return Err(credential_refusal());
    }
    let runner = producer_runner(nonce)?;
    let registered = verify_operation_repo(state, &op.scope.repo)?;
    let lock_path = state
        .root()
        .join(LOCK_NAMESPACE)
        .join(key.pool_id)
        .join(Slot::lock_name_for(key.slot));
    let lock_id = slots::checked_slot_lock(&lock_path).map_err(|_| producer_fence_refusal())?;
    let fd = i32::try_from(request.lock_fd)
        .ok()
        .filter(|fd| *fd > 2)
        .ok_or_else(producer_fence_refusal)?;
    let inherited = duplicate_inherited_fd(fd).map_err(|_| producer_fence_refusal())?;
    let guided_start = matches!(input, ProducerInput::GuidedStart { .. });
    if guided_start != control_fd.is_some() {
        return Err(producer_fence_refusal());
    }
    let control = control_fd
        .map(|fd| private_output(fd, Some(lock_id)).map_err(|_| producer_fence_refusal()))
        .transpose()?;
    if !guided_start {
        let (independent, _) =
            slots::open_slot_lock(&lock_path).map_err(|_| producer_fence_refusal())?;
        if try_exclusive(&independent)
            .map_err(|_| producer_fence_refusal())?
            .is_some()
        {
            return Err(producer_fence_refusal());
        }
    }
    let lock = LockFileIdentity {
        dev: lock_id.dev,
        ino: lock_id.ino,
    };
    let InheritedLock::Acquired(_guard) =
        try_lock_inherited(inherited.as_fd(), lock).map_err(|_| producer_fence_refusal())?
    else {
        return Err(producer_fence_refusal());
    };
    let command = ProducerCommand {
        state,
        op,
        key,
        registered,
        runner,
        lock_id,
    };
    let snapshot = command.snapshot(credential)?;
    match input {
        ProducerInput::Start {
            credential, shape, ..
        } => command.start(&snapshot, ProducerInvocation { credential, shape }, None),
        ProducerInput::Watch {
            credential,
            shape,
            child_pid,
            ..
        } => command.watch(
            snapshot,
            ProducerInvocation { credential, shape },
            child_pid,
            fd,
            inherited.as_fd(),
            false,
        ),
        ProducerInput::GuidedStart {
            credential,
            invocation,
            ..
        } => {
            let shape = guided_shape(&snapshot.pool.profile, &invocation)?;
            let offset = verify_guide_lock(
                &runner_instance(&command.runner),
                fd,
                lock,
                inherited.as_fd(),
                None,
            )
            .map_err(|_| producer_fence_refusal())?;
            let output = control.ok_or_else(producer_fence_refusal)?;
            command.start(
                &snapshot,
                ProducerInvocation { credential, shape },
                Some((output, offset)),
            )
        }
        ProducerInput::GuidedWatch {
            credential,
            invocation,
            child_pid,
            ..
        } => {
            let shape = guided_shape(&snapshot.pool.profile, &invocation)?;
            command.watch(
                snapshot,
                ProducerInvocation { credential, shape },
                child_pid,
                fd,
                inherited.as_fd(),
                true,
            )
        }
        ProducerInput::ReportBypass {
            credential,
            profile,
            ..
        } => command.report_bypass(&snapshot, &credential, &profile),
    }
}

fn guided_shape(profile: &PoolProfile, id: &str) -> Result<InvocationInput, Refusal> {
    let invocation = profile
        .invocations
        .iter()
        .find(|item| item.id == id)
        .ok_or_else(watch_refusal)?;
    Ok(InvocationInput {
        profile: slots::ProfileBinding {
            id: profile.id.clone(),
            revision: profile.revision,
        },
        invocation: invocation.id.clone(),
        argv: invocation.argv.clone(),
        cwd: invocation.cwd.clone(),
    })
}

struct ProducerCommand<'a> {
    state: &'a State,
    op: &'a SlotOperation,
    key: SlotRequestKey<'a>,
    registered: VerifiedRepo,
    runner: Runner,
    lock_id: FileIdentity,
}

impl ProducerCommand<'_> {
    fn slot_index(&self) -> usize {
        usize::from(self.key.slot - 1)
    }

    fn snapshot(&self, credential: &str) -> Result<ReturnScope, Refusal> {
        with_slot_state(self.state, |tx| {
            let (mut saved, index) =
                checked_pool(tx, self.state, &self.op.scope, &self.registered)?;
            let pool = &saved.pools[index];
            let slot = &pool.slots[self.slot_index()];
            producer_lease(slot, &self.key, credential)?;
            if slot.lock_id != self.lock_id || !slot_identity_present(self.state.root(), pool, slot)
            {
                return Err(producer_fence_refusal());
            }
            Ok(ReturnScope {
                pool: saved.pools.swap_remove(index),
                registry: tx.read_slot_registry()?,
                config: self.state.slot_config()?,
            })
        })
    }

    fn publish(
        &self,
        snapshot: &ReturnScope,
        write: impl FnOnce(&mut Slot) -> Result<(), Refusal>,
    ) -> Result<(), Refusal> {
        with_slot_state(self.state, |tx| {
            let (mut saved, index) =
                checked_pool(tx, self.state, &self.op.scope, &self.registered)?;
            let current = &saved.pools[index];
            let slot_index = self.slot_index();
            self.verify_snapshot(tx, snapshot, current)?;
            write(&mut saved.pools[index].slots[slot_index])?;
            commit_slot_update(tx, &mut saved)
        })
    }

    fn verify_snapshot(
        &self,
        tx: &Tx<'_>,
        snapshot: &ReturnScope,
        current: &Pool,
    ) -> Result<(), Refusal> {
        let slot_index = self.slot_index();
        if current.slots[slot_index] != snapshot.pool.slots[slot_index]
            || current.profile != snapshot.pool.profile
            || tx.read_slot_registry::<Registry>()? != snapshot.registry
            || self.state.slot_config()? != snapshot.config
            || producer_runner(&self.runner.nonce)? != self.runner
            || !slot_identity_present(self.state.root(), current, &current.slots[slot_index])
        {
            return Err(producer_fence_refusal());
        }
        Ok(())
    }

    fn start(
        &self,
        snapshot: &ReturnScope,
        input: ProducerInvocation,
        control: Option<(File, i64)>,
    ) -> Result<SlotResult, Refusal> {
        let pool = &snapshot.pool;
        let slot = &pool.slots[self.slot_index()];
        let (checkout, ready_commit) = producer_lease(slot, &self.key, &input.credential)?;
        if slot
            .outcomes
            .iter()
            .any(|outcome| self.key.matches_identity(outcome))
        {
            return Err(Refusal::ReturnUncertain(
                "This producer operation already has a recorded start. Wait for its original foreground operation.",
            ));
        }
        let prior = profile::eligible_producer(&slot.producer, slot.generation)?;
        if slot.outcomes.len() == MAX_OUTCOMES {
            return Err(Refusal::OutcomeLimit);
        }
        if prior.completed_in_generation(slot.generation).len() >= MAX_OUTCOMES {
            return Err(Refusal::ProducerHistoryLimit);
        }
        if prior
            .completed_in_generation(slot.generation)
            .iter()
            .any(|record| record.operation == self.key.operation)
        {
            return Err(Refusal::ReturnUncertain(
                "This invocation operation is already recorded as complete.",
            ));
        }
        let roots = prior.roots();
        let mut proof = self.inspect(snapshot, &input, &roots, empty_proof(&prior))?;
        let checked = profile::check_invocation(&pool.profile, &input.shape, &proof, &roots)?;
        if checked.needs_guide && control.is_none() {
            return Err(Refusal::ReturnUncertain(
                "This reviewed producer requires the packaged guide's checked launch.",
            ));
        }
        self.prove_output_scope(snapshot, &proof.path, &roots, empty_proof(&prior))?;
        let permit = control
            .as_ref()
            .map(|_| launch_permit(&checked, &input.shape.argv))
            .transpose()?;
        let guide_guard_offset = control.as_ref().map(|(_, offset)| *offset);
        let pending = PendingProducer {
            epoch: OutputEpoch {
                pool_id: pool.pool_id.clone(),
                slot: slot.number,
                checkout: checkout.clone(),
            },
            generation: slot.generation,
            profile: input.shape.profile,
            invocation: checked.evidence.clone(),
            roots,
            operation: self.key.operation.to_owned(),
            prior,
            runner: self.runner.clone(),
            foreground: None,
            guide_guard_offset,
        };
        let outcome = Outcome {
            pool_id: pool.pool_id.clone(),
            slot: slot.number,
            generation: slot.generation,
            operation: self.key.operation.to_owned(),
            attempt_id: None,
            kind: OperationKind::ProducerStart,
            request: OutcomeRequest::Producer {
                runner: self.runner.clone(),
                invocation: checked.evidence,
                ready_commit: Some(ready_commit.to_owned()),
            },
            credential: input.credential,
            result: TransitionResult::ProducerPending,
        };
        self.publish(snapshot, |slot| {
            slot.producer = Producer::Pending(Box::new(pending));
            slot.outcomes.push(outcome);
            Ok(())
        })?;
        proof.release_output_locks();
        if let Some(((mut file, _), bytes)) = control.zip(permit) {
            file.write_all(&bytes).and_then(|()| file.flush()).map_err(|_| {
                Refusal::ReturnUncertain("Could not deliver the checked producer launch. Its pending start remains held.")
            })?;
        }
        Ok(producer_result(pool, slot, &proof, "producer_started"))
    }

    fn report_bypass(
        &self,
        snapshot: &ReturnScope,
        credential: &str,
        binding: &slots::ProfileBinding,
    ) -> Result<SlotResult, Refusal> {
        let pool = &snapshot.pool;
        let slot = &pool.slots[self.slot_index()];
        let (checkout, ready_commit) = producer_lease(slot, &self.key, credential)?;
        if binding.id != pool.profile.id || binding.revision != pool.profile.revision {
            return Err(Refusal::ReturnUncertain(
                "The producer report does not name the current reviewed profile.",
            ));
        }
        let report = BypassReport {
            operation: self.key.operation.to_owned(),
            runner: self.runner.clone(),
        };
        let mut result = SlotResult::new(
            "slot producer",
            "producer_tainted",
            Some(pool.main.clone()),
            0,
        );
        result.pool_id = Some(pool.pool_id.clone());
        result.transition = Some(SlotTransition {
            slot: slot.number,
            generation: slot.generation,
            phase: "held",
            commit: ready_commit.to_owned(),
            current: false,
        });
        result.next_action = Some(LOST_PROVENANCE);
        if slot.producer.lost() {
            let confirmed = with_slot_state(self.state, |tx| {
                let (saved, index) =
                    checked_pool(tx, self.state, &self.op.scope, &self.registered)?;
                let pool = &saved.pools[index];
                self.verify_snapshot(tx, snapshot, pool)?;
                let slot = &pool.slots[self.slot_index()];
                producer_lease(slot, &self.key, credential)?;
                replay_reported_bypass(slot, &report, || Ok(tx.sync_slots(&saved)?))
            })?;
            return if confirmed {
                Ok(result)
            } else {
                Err(watch_refusal())
            };
        }
        if slot
            .outcomes
            .iter()
            .any(|outcome| self.key.matches_identity(outcome))
        {
            return Err(watch_refusal());
        }
        let previous_roots = if let Producer::Pending(pending) = &slot.producer {
            if pending.runner != self.runner || pending.operation == self.key.operation {
                return Err(producer_fence_refusal());
            }
            Some(pending.roots.clone())
        } else if let Some(prior) = slot.producer.eligible(slot.generation) {
            let completed = prior.completed_in_generation(slot.generation);
            if completed
                .last()
                .is_some_and(|record| record.runner != self.runner)
                || completed
                    .iter()
                    .any(|record| record.operation == self.key.operation)
            {
                return Err(producer_fence_refusal());
            }
            Some(prior.roots())
        } else {
            None
        };
        let observed = |root: RootExpectation| match root {
            RootExpectation::Present { root }
                if owned_identity(&root.path) == Some(root.identity) =>
            {
                Some(root)
            }
            _ => None,
        };
        let roots = previous_roots.map_or(
            OutputRoots {
                target: None,
                build: None,
            },
            |roots| OutputRoots {
                target: observed(roots.target),
                build: observed(roots.build),
            },
        );
        self.publish(snapshot, |slot| {
            slot.producer = Producer::Tainted {
                epoch: OutputEpoch {
                    pool_id: pool.pool_id.clone(),
                    slot: slot.number,
                    checkout: checkout.clone(),
                },
                generation: slot.generation,
                roots,
                reason: ProvenanceLoss::OffProfile,
                report: Some(report),
            };
            Ok(())
        })?;
        Ok(result)
    }

    fn inspect(
        &self,
        snapshot: &ReturnScope,
        input: &ProducerInvocation,
        roots: &OutputRoots<RootExpectation>,
        initial_empty: Option<&slots::EmptyEpoch>,
    ) -> Result<CheckoutProof, Refusal> {
        let pool = &snapshot.pool;
        let slot = &pool.slots[self.slot_index()];
        let (checkout, ready_commit) = producer_lease(slot, &self.key, &input.credential)?;
        let invocation = profile::reviewed_shape(&pool.profile, &input.shape)?;
        let mutable =
            profile::mutable_checkout_paths(&pool.profile, invocation, &pool.checkout_path(slot));
        let proof = inspect_producer_checkout(
            &SlotScope {
                pool,
                slot,
                registry: &snapshot.registry,
                secondary_roots: &snapshot.config.secondary_roots,
            },
            &ExpectedCheckout {
                identity: checkout,
                head: Some(ready_commit),
            },
            &ProducerInventory {
                roots,
                initial_empty,
                mutable: &mutable,
            },
        )?;
        check_producer_inventory(&proof)?;
        Ok(proof)
    }

    fn prove_output_scope(
        &self,
        snapshot: &ReturnScope,
        path: &Path,
        roots: &OutputRoots<RootExpectation>,
        initial_empty: Option<&slots::EmptyEpoch>,
    ) -> Result<(), Refusal> {
        let expected = match initial_empty {
            Some(empty) => empty_gc_roots(empty),
            None => present_gc_roots(roots)?,
        };
        let parent = runner_instance(&self.runner);
        let cancel = || cancelled() || parent_instance().as_ref().ok() != Some(&parent);
        let ownership = crate::cli::verify_slot_output_scope(
            self.state,
            path,
            &expected,
            &cancel,
        )
        .map_err(|err| {
            if err.is_cancelled() {
                Refusal::State(StateError::Cancelled)
            } else {
                Refusal::ReturnUncertain(
                    "Could not prove exclusive current ownership of both producer output roots.",
                )
            }
        })?;
        if ownership.registry != snapshot.registry || ownership.config != snapshot.config {
            return Err(producer_fence_refusal());
        }
        Ok(())
    }

    fn watch(
        &self,
        mut snapshot: ReturnScope,
        input: ProducerInvocation,
        pid: u32,
        fd: i32,
        inherited: BorrowedFd<'_>,
        guided: bool,
    ) -> Result<SlotResult, Refusal> {
        let slot = &snapshot.pool.slots[self.slot_index()];
        let Producer::Pending(pending) = &slot.producer else {
            return Err(watch_refusal());
        };
        if pending.operation != self.key.operation
            || pending.runner != self.runner
            || pending.foreground.is_some()
            || pending.profile != input.shape.profile
            || guided != pending.guide_guard_offset.is_some()
        {
            return Err(watch_refusal());
        }
        let mut pending = pending.clone();
        let mut proof = self.inspect(
            &snapshot,
            &input,
            &pending.roots,
            empty_proof(&pending.prior),
        )?;
        let checked = profile::check_invocation(
            &snapshot.pool.profile,
            &input.shape,
            &proof,
            &pending.roots,
        )?;
        if checked.evidence != pending.invocation {
            return Err(watch_refusal());
        }
        let parent = runner_instance(&self.runner);
        let lock = LockFileIdentity {
            dev: self.lock_id.dev,
            ino: self.lock_id.ino,
        };
        let watcher = if let Some(offset) = pending.guide_guard_offset {
            ForegroundWatch::start_with_offset(parent, pid, fd, lock, inherited, Some(offset))
        } else {
            ForegroundWatch::start(parent, pid, fd, lock, inherited)
        }
        .map_err(foreground_refusal)?;
        let profile = &snapshot.pool.profile;
        let invocation = profile::reviewed_shape(profile, &input.shape)?;
        let gate = watcher.stopped_image().map_err(foreground_refusal)?;
        let environment_args: Vec<_> = checked
            .environment
            .iter()
            .map(|(name, value)| {
                let mut assignment = name.clone();
                assignment.push("=");
                assignment.push(value);
                assignment
            })
            .collect();
        let cargo_args = || {
            std::iter::once(checked.cargo.as_os_str())
                .chain(invocation.argv.iter().map(std::ffi::OsStr::new))
        };
        let environment_and_cargo = || {
            environment_args
                .iter()
                .map(std::ffi::OsString::as_os_str)
                .chain(cargo_args())
        };
        let expected_gate = std::iter::once(std::ffi::OsStr::new(PRODUCER_GATE_PROGRAM))
            .chain(PRODUCER_GATE_ARGS.iter().map(std::ffi::OsStr::new))
            .chain(environment_and_cargo());
        if gate.program != Path::new(PRODUCER_GATE_PROGRAM)
            || !gate.argv.iter().eq(expected_gate)
            || gate.cwd != checked.cwd
        {
            return Err(Refusal::ReturnUncertain(
                "The stopped guide child's executable, arguments or environment allowlist differs from the reviewed Cargo launch.",
            ));
        }
        self.prove_output_scope(
            &snapshot,
            &proof.path,
            &pending.roots,
            empty_proof(&pending.prior),
        )?;
        let child = watcher.child();
        let child = ForegroundInstance {
            pid: child.pid,
            start_secs: child.start_secs,
            start_micros: child.start_micros,
        };
        pending.foreground = Some(child.clone());
        self.publish(&snapshot, |slot| {
            slot.producer = Producer::Pending(pending.clone());
            Ok(())
        })?;
        snapshot.pool.slots[self.slot_index()].producer = Producer::Pending(pending.clone());
        proof.release_output_locks();

        let mut writer_images = vec![(checked.cargo.as_path(), checked.environment.len())];
        let terminal_environment = checked.terminal.as_ref().map(|terminal| {
            let mut expected = checked.environment.clone();
            expected.insert("CARGO".into(), checked.cargo.as_os_str().to_owned());
            writer_images.push((terminal.as_path(), expected.len()));
            expected
        });
        let exited = watcher
            .wait(
                Duration::from_secs(60 * 60),
                &writer_images,
                |image| {
                    if image.cwd != checked.cwd {
                        return None;
                    }
                    if image.program == checked.cargo
                        && image.argv.iter().eq(cargo_args())
                        && image.environment == checked.environment
                    {
                        Some(if checked.terminal.is_some() {
                            ForegroundExec::Dispatcher
                        } else {
                            ForegroundExec::Writer
                        })
                    } else if let Some(terminal) = &checked.terminal
                        && image.program == *terminal
                        && image.argv.iter().eq(std::iter::once(terminal.as_os_str())
                            .chain(invocation.argv.iter().map(std::ffi::OsStr::new)))
                        && terminal_environment.as_ref() == Some(&image.environment)
                    {
                        Some(ForegroundExec::Writer)
                    } else if image.program == Path::new(PRODUCER_ENV_PROGRAM)
                        && image.argv.iter().eq([PRODUCER_ENV_PROGRAM, "-i"]
                            .into_iter()
                            .map(std::ffi::OsStr::new)
                            .chain(environment_and_cargo()))
                    {
                        Some(ForegroundExec::Handoff)
                    } else {
                        None
                    }
                },
                cancelled,
            )
            .map_err(foreground_refusal)?;
        if exited.pid != child.pid
            || exited.start_secs != child.start_secs
            || exited.start_micros != child.start_micros
        {
            return Err(watch_refusal());
        }
        if let Some(offset) = pending.guide_guard_offset {
            verify_guide_lock(
                &runner_instance(&self.runner),
                fd,
                lock,
                inherited,
                Some(offset),
            )
            .map_err(foreground_refusal)?;
        }
        let finished = self.snapshot(&input.credential)?;
        if finished.pool.slots[self.slot_index()] != snapshot.pool.slots[self.slot_index()] {
            return Err(watch_refusal());
        }
        self.finish(&finished, &input, &pending, child)
    }

    fn finish(
        &self,
        snapshot: &ReturnScope,
        input: &ProducerInvocation,
        pending: &PendingProducer,
        child: ForegroundInstance,
    ) -> Result<SlotResult, Refusal> {
        let roots = profile::completed_roots(&pending.roots)?;
        let roots_for_check = OutputRoots {
            target: RootExpectation::Present {
                root: roots.target.clone(),
            },
            build: RootExpectation::Present {
                root: roots.build.clone(),
            },
        };
        let proof = self.inspect(snapshot, input, &roots_for_check, None)?;
        let pool = &snapshot.pool;
        let checked =
            profile::check_invocation(&pool.profile, &input.shape, &proof, &roots_for_check)?;
        if checked.evidence != pending.invocation {
            return Err(watch_refusal());
        }
        profile::verify_output_layout(
            profile::reviewed_shape(&pool.profile, &input.shape)?,
            &roots.build.path,
        )?;
        self.prove_output_scope(snapshot, &proof.path, &roots_for_check, None)?;
        let mut invocations = pending
            .prior
            .completed_in_generation(pending.generation)
            .to_vec();
        invocations.push(CompletedInvocation {
            evidence: pending.invocation.clone(),
            operation: pending.operation.clone(),
            runner: pending.runner.clone(),
            foreground: Some(child),
        });
        self.publish(snapshot, |slot| {
            let outcome = slot
                .outcomes
                .iter_mut()
                .find(|outcome| {
                    self.key.matches_identity(outcome)
                        && outcome.kind == OperationKind::ProducerStart
                        && outcome.credential == input.credential
                })
                .ok_or_else(watch_refusal)?;
            outcome.kind = OperationKind::ProducerComplete;
            outcome.result = TransitionResult::ProducerComplete;
            slot.producer = Producer::Complete {
                epoch: pending.epoch.clone(),
                generation: pending.generation,
                profile: pending.profile.clone(),
                roots,
                invocations,
            };
            Ok(())
        })?;
        Ok(producer_result(
            pool,
            &pool.slots[self.slot_index()],
            &proof,
            "producer_complete",
        ))
    }
}

fn replay_reported_bypass(
    slot: &Slot,
    report: &BypassReport,
    confirm: impl FnOnce() -> Result<(), Refusal>,
) -> Result<bool, Refusal> {
    let matched = matches!(&slot.producer, Producer::Tainted { generation, report: Some(previous), .. }
        if generation == &slot.generation && previous == report);
    if matched {
        confirm()?;
    }
    Ok(matched)
}

fn launch_permit(
    checked: &profile::CheckedInvocation,
    argv: &[String],
) -> Result<Vec<u8>, Refusal> {
    let mut output = Vec::new();
    let mut append = |value: &[u8]| {
        if value.contains(&0) || output.len() + value.len() + 1 > 64 * 1024 {
            return Err(Refusal::ReturnUncertain(
                "The checked producer launch exceeds its control bound.",
            ));
        }
        output.extend_from_slice(value);
        output.push(0);
        Ok(())
    };
    append(b"producer_permit")?;
    append(checked.cwd.as_os_str().as_encoded_bytes())?;
    append(checked.environment.len().to_string().as_bytes())?;
    for (key, value) in &checked.environment {
        let mut assignment = key.clone();
        assignment.push("=");
        assignment.push(value);
        append(assignment.as_encoded_bytes())?;
    }
    append((argv.len() + 1).to_string().as_bytes())?;
    append(checked.cargo.as_os_str().as_encoded_bytes())?;
    for arg in argv {
        append(arg.as_bytes())?;
    }
    Ok(output)
}

fn empty_proof(prior: &EligibleProducer) -> Option<&slots::EmptyEpoch> {
    match prior {
        EligibleProducer::NeverBuilt { evidence } => Some(evidence),
        _ => None,
    }
}

fn runner_instance(runner: &Runner) -> ProcessInstance {
    ProcessInstance {
        boot_session: runner.boot_session.clone(),
        pid: runner.pid,
        start_secs: runner.start_secs,
        start_micros: runner.start_micros,
    }
}

fn watch_refusal() -> Refusal {
    Refusal::ReturnUncertain(
        "The exact pending invocation or its foreground completion could not be verified.",
    )
}

fn foreground_refusal(err: io::Error) -> Refusal {
    if err.kind() == io::ErrorKind::Interrupted && cancelled() {
        return StateError::Cancelled.into();
    }
    Refusal::Scope(format!(
        "Foreground custody or completion could not be established ({err}). Preserve the pending invocation and slot files."
    ))
}

fn producer_result(
    pool: &Pool,
    slot: &Slot,
    proof: &CheckoutProof,
    kind: &'static str,
) -> SlotResult {
    let mut result = SlotResult::new("slot producer", kind, Some(pool.main.clone()), 0);
    result.pool_id = Some(pool.pool_id.clone());
    result.transition = Some(SlotTransition {
        slot: slot.number,
        generation: slot.generation,
        phase: "leased",
        commit: proof.head.clone(),
        current: true,
    });
    result
}

fn producer_lease<'a>(
    slot: &'a Slot,
    key: &SlotRequestKey<'_>,
    credential: &str,
) -> Result<(&'a slots::CheckoutIdentity, &'a str), Refusal> {
    if let Phase::Leased {
        lease,
        ready_commit,
        ..
    } = &slot.phase
        && slot.generation == key.generation
        && lease.credential == credential
        && let Some(checkout) = &slot.checkout_id
    {
        return Ok((checkout, ready_commit));
    }
    Err(Refusal::Scope(
        "A producer requires the original credential and current ready lease generation.".into(),
    ))
}

fn producer_fence_refusal() -> Refusal {
    Refusal::ReturnUncertain("Could not verify the guide runner and inherited slot operation lock.")
}

fn producer_runner(nonce: &str) -> Result<Runner, Refusal> {
    let observed = parent_instance().map_err(|_| producer_fence_refusal())?;
    Ok(Runner {
        boot_session: observed.boot_session,
        pid: observed.pid,
        start_secs: observed.start_secs,
        start_micros: observed.start_micros,
        nonce: nonce.to_owned(),
    })
}

fn check_producer_inventory(proof: &CheckoutProof) -> Result<(), Refusal> {
    match proof.inventory {
        CheckoutInventory::Clean => Ok(()),
        CheckoutInventory::Dirty(detail) => Err(Refusal::ReturnWork(detail)),
        CheckoutInventory::Uncertain(detail) => Err(Refusal::ReturnUncertain(detail)),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PreparationInput {
    credential: String,
    nonce: String,
}

impl PreparationInput {
    fn read() -> Result<Self, Refusal> {
        let bytes = read_stdin_complete(512, CREDENTIAL_DEADLINE, cancelled).map_err(|err| {
            if err.kind() == io::ErrorKind::Interrupted {
                StateError::Cancelled.into()
            } else {
                credential_refusal()
            }
        })?;
        let input: Self = serde_json::from_slice(&bytes).map_err(|_| credential_refusal())?;
        if !slots::hex(&input.credential, KEY_HEX_LEN) || !slots::hex(&input.nonce, KEY_HEX_LEN) {
            return Err(credential_refusal());
        }
        Ok(input)
    }
}

fn begin(
    state: &State,
    request: &SlotLockedOperation,
    control_fd: u32,
) -> Result<SlotResult, Refusal> {
    const COMMAND: &str = "slot begin";
    let op = &request.operation;
    let key = request_key(&op.scope, op.generation, OperationKind::Begin, &op.op, None)?;
    let input = PreparationInput::read()?;
    let runner = producer_runner(&input.nonce)?;
    let registered = verify_operation_repo(state, &op.scope.repo)?;
    let lock_path = state
        .root()
        .join(LOCK_NAMESPACE)
        .join(key.pool_id)
        .join(Slot::lock_name_for(key.slot));
    let lock_id = slots::checked_slot_lock(&lock_path).map_err(|_| producer_fence_refusal())?;
    let mut control = private_output(control_fd, Some(lock_id)).map_err(|_| {
        Refusal::ReturnUncertain(
            "Could not verify a separate private preparation output descriptor.",
        )
    })?;
    let fd = i32::try_from(request.lock_fd)
        .ok()
        .filter(|fd| *fd > 2)
        .ok_or_else(producer_fence_refusal)?;
    let inherited = duplicate_inherited_fd(fd).map_err(|_| producer_fence_refusal())?;
    let identity = LockFileIdentity {
        dev: lock_id.dev,
        ino: lock_id.ino,
    };
    let check_runner = |expected_offset| {
        verify_guide_lock(
            &runner_instance(&runner),
            fd,
            identity,
            inherited.as_fd(),
            expected_offset,
        )
        .map_err(|_| producer_fence_refusal())
    };
    let InheritedLock::Acquired(_guard) =
        try_lock_inherited(inherited.as_fd(), identity).map_err(|_| producer_fence_refusal())?
    else {
        return Err(producer_fence_refusal());
    };
    let slot_index = usize::from(key.slot - 1);
    let plan = with_slot_state(state, |tx| {
        let (mut saved, index) = checked_pool(tx, state, &op.scope, &registered)?;
        let pool = &saved.pools[index];
        let slot = &pool.slots[slot_index];
        if slot.lock_id != lock_id {
            return Err(producer_fence_refusal());
        }
        if let Some(outcome) = slot.outcomes.iter().find(|outcome| {
            outcome.generation == key.generation && outcome.operation == key.operation
        }) {
            if outcome.kind != OperationKind::Begin
                || outcome.credential != input.credential
                || outcome.request
                    != (OutcomeRequest::Begin {
                        runner: runner.clone(),
                    })
            {
                return Err(Refusal::Scope(
                    "The operation ID belongs to a different request.".into(),
                ));
            }
            if slot.generation == key.generation
                && let Phase::Preparing { attempt, .. } = &slot.phase
            {
                check_runner(Some(
                    attempt.lock_offset.ok_or_else(producer_fence_refusal)?,
                ))?;
            }
            tx.sync_slots(&saved)?;
            return Ok(ReturnPlan::Replay(Box::new(begin_result(
                COMMAND, pool, outcome, false,
            ))));
        }
        if slot.generation != key.generation
            || !matches!(&slot.phase, Phase::Reserved { lease } if lease.credential == input.credential)
            || slot
                .retry
                .as_ref()
                .is_some_and(|retry| retry.begin_op != key.operation)
            || !slot_producer_is_eligible(slot)
            || !slot_identity_present(state.root(), pool, slot)
        {
            return Err(Refusal::ReturnUncertain(
                "Begin requires the original current reservation and proved checkout identity.",
            ));
        }
        if slot.outcomes.len() >= MAX_OUTCOMES {
            return Err(Refusal::OutcomeLimit);
        }
        Ok(ReturnPlan::Inspect(Box::new(ReturnScope {
            pool: saved.pools.swap_remove(index),
            registry: tx.read_slot_registry()?,
            config: state.slot_config()?,
        })))
    })?;
    let snapshot = match plan {
        ReturnPlan::Replay(result) => return Ok(*result),
        ReturnPlan::Inspect(snapshot) => snapshot,
    };
    let pool = &snapshot.pool;
    let slot = &pool.slots[slot_index];
    let Phase::Reserved { lease } = &slot.phase else {
        return Err(producer_fence_refusal());
    };
    let path = pool.checkout_path(slot);
    if path.as_os_str().as_encoded_bytes().len() > 4096 {
        return Err(Refusal::ReturnUncertain(
            "The preparation path is outside the private protocol bound.",
        ));
    }
    let anchors = prepare::consumers(&pool.profile, &slot.producer);
    let private_len = path.as_os_str().as_encoded_bytes().len()
        + anchors
            .keys()
            .map(|path| path.as_os_str().as_encoded_bytes().len() + 1)
            .sum::<usize>();
    if private_len > 7680 {
        return Err(Refusal::ReturnUncertain(
            "The preparation permit exceeds its private protocol bound.",
        ));
    }
    let lock_offset = check_runner(None)?;
    let witness = boot_witness(key.pool_id).map_err(|_| {
        Refusal::ReturnUncertain("Could not verify the local boot and host before preparation.")
    })?;
    if witness.boot_session != runner.boot_session {
        return Err(producer_fence_refusal());
    }
    let inspection = if let Some(prior) = slot.last_head.as_deref() {
        verify_requested_tree(&pool.main, &lease.requested_commit)?;
        prepare::verify_changes(&pool.main, prior, &lease.requested_commit, &pool.profile)?;
        Some(inspect_prepared_checkout(
            pool, &snapshot, slot_index, prior,
        )?)
    } else {
        verify_no_checkout_ancestor(&pool.root)?;
        let records = owner_worktree_inventory(&pool.root, &snapshot.registry, Some(pool))?;
        let checked_paths = records
            .into_iter()
            .map(|record| record.path)
            .collect::<Vec<_>>();
        let mut remaining = MAX_ROOT_ENTRIES;
        check_nested_checkouts(&pool.root, 0, &mut remaining, &checked_paths)?;
        None
    };
    let (result, attempt) = with_slot_state(state, |tx| {
        let (mut saved, index) = checked_pool(tx, state, &op.scope, &registered)?;
        let current = &saved.pools[index];
        if &current.slots[slot_index] != slot
            || current.profile != pool.profile
            || tx.read_slot_registry::<Registry>()? != snapshot.registry
            || state.slot_config()?.secondary_roots != snapshot.config.secondary_roots
            || !slot_identity_present(state.root(), current, slot)
        {
            return Err(producer_fence_refusal());
        }
        check_runner(Some(lock_offset))?;
        let attempt_id = saved
            .revision
            .checked_add(1)
            .ok_or(Refusal::Capacity("The slot attempt counter is exhausted."))?;
        let outcome = Outcome {
            pool_id: key.pool_id.into(),
            slot: key.slot,
            generation: key.generation,
            operation: key.operation.into(),
            kind: key.kind,
            attempt_id: Some(attempt_id),
            request: OutcomeRequest::Begin {
                runner: runner.clone(),
            },
            credential: input.credential.clone(),
            result: TransitionResult::PermitIssued,
        };
        let result = begin_result(COMMAND, current, &outcome, true);
        let next = &mut saved.pools[index].slots[slot_index];
        next.phase = Phase::Preparing {
            lease: lease.clone(),
            attempt: Attempt {
                attempt_id,
                begin_op: key.operation.into(),
                runner: runner.clone(),
                host_binding: Some(witness.host_binding),
                stage: AttemptStage::PermitIssued,
                lock_offset: Some(lock_offset),
                prior_head: slot.last_head.clone(),
                anchors: Vec::new(),
                anchor_cursor: 0,
            },
        };
        next.outcomes.push(outcome);
        commit_slot_update(tx, &mut saved)?;
        Ok((result, attempt_id))
    })?;
    drop(inspection);
    check_runner(Some(lock_offset))?;
    let mut payload = format!("permit_issued\0{attempt}\0").into_bytes();
    payload.extend(path.as_os_str().as_encoded_bytes());
    payload.extend(b"\0");
    payload.extend(lease.requested_commit.as_bytes());
    payload.extend(b"\0");
    payload.extend(slot.last_head.as_deref().unwrap_or("").as_bytes());
    payload.extend(b"\0");
    for anchor in anchors.keys() {
        payload.extend(anchor.as_os_str().as_encoded_bytes());
        payload.extend(b"\0");
    }

    if payload.len() > 8192 || control.write_all(&payload).is_err() {
        return Err(Refusal::ReturnUncertain(
            "The first preparation permit could not be delivered.",
        ));
    }
    Ok(result)
}

fn begin_result(command: &'static str, pool: &Pool, outcome: &Outcome, live: bool) -> SlotResult {
    let kind = if live {
        "permit_issued"
    } else {
        "permit_recorded"
    };
    let mut result = SlotResult::new(command, kind, Some(pool.main.clone()), 0);
    result.pool_id = Some(pool.pool_id.clone());
    result.attempt = outcome.attempt_id;
    result.current = Some(live);
    if !live {
        result.next_action = Some(HELD_ACTION);
    }
    result
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StepRequest {
    auth: PreparationInput,
    step: PreparationStep,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum PreparationStep {
    Checkout,
    CheckoutDone,
    Anchors,
    AnchorTouch { index: usize, action: String },
    AnchorsDone,
    Seed,
}

struct PreparationFence<'a> {
    state: &'a State,
    scope: &'a SlotScopeArgs,
    registered: VerifiedRepo,
    generation: u64,
    attempt: u64,
    auth: PreparationInput,
    runner: Runner,
    fd: i32,
    inherited: std::os::fd::OwnedFd,
    lock_id: FileIdentity,
    _guard: cargo_hive_platform::flock::InheritedLockGuard,
}

impl<'a> PreparationFence<'a> {
    fn acquire(
        state: &'a State,
        scope: &'a SlotScopeArgs,
        generation: u64,
        attempt: u64,
        fd: u32,
        auth: PreparationInput,
    ) -> Result<Self, Refusal> {
        if !slots::hex(&scope.pool, 32)
            || !(1..=2).contains(&scope.slot)
            || attempt == 0
            || !slots::hex(&auth.credential, KEY_HEX_LEN)
            || !slots::hex(&auth.nonce, KEY_HEX_LEN)
        {
            return Err(producer_fence_refusal());
        }
        let runner = producer_runner(&auth.nonce)?;
        let registered = verify_operation_repo(state, &scope.repo)?;
        let path = state
            .root()
            .join(LOCK_NAMESPACE)
            .join(&scope.pool)
            .join(Slot::lock_name_for(scope.slot));
        let (independent, lock_id) =
            slots::open_slot_lock(&path).map_err(|_| producer_fence_refusal())?;
        if try_exclusive(&independent)
            .map_err(|_| producer_fence_refusal())?
            .is_some()
        {
            return Err(producer_fence_refusal());
        }
        let fd = i32::try_from(fd)
            .ok()
            .filter(|fd| *fd > 2)
            .ok_or_else(producer_fence_refusal)?;
        let inherited = duplicate_inherited_fd(fd).map_err(|_| producer_fence_refusal())?;
        let identity = LockFileIdentity {
            dev: lock_id.dev,
            ino: lock_id.ino,
        };
        let InheritedLock::Acquired(guard) = try_lock_inherited(inherited.as_fd(), identity)
            .map_err(|_| producer_fence_refusal())?
        else {
            return Err(producer_fence_refusal());
        };
        Ok(Self {
            state,
            scope,
            registered,
            generation,
            attempt,
            auth,
            runner,
            fd,
            inherited,
            lock_id,
            _guard: guard,
        })
    }

    fn index(&self) -> usize {
        usize::from(self.scope.slot - 1)
    }

    fn check<'s>(&self, slot: &'s Slot) -> Result<(&'s Lease, &'s Attempt), Refusal> {
        let Phase::Preparing { lease, attempt } = &slot.phase else {
            return Err(producer_fence_refusal());
        };
        if slot.generation != self.generation
            || slot.lock_id != self.lock_id
            || lease.credential != self.auth.credential
            || attempt.attempt_id != self.attempt
            || attempt.runner != self.runner
        {
            return Err(producer_fence_refusal());
        }
        let offset = attempt.lock_offset.ok_or_else(producer_fence_refusal)?;
        verify_guide_lock(
            &runner_instance(&self.runner),
            self.fd,
            LockFileIdentity {
                dev: self.lock_id.dev,
                ino: self.lock_id.ino,
            },
            self.inherited.as_fd(),
            Some(offset),
        )
        .map_err(|_| producer_fence_refusal())?;
        Ok((lease, attempt))
    }

    fn snapshot(&self, stage: AttemptStage) -> Result<ReturnScope, Refusal> {
        with_slot_state(self.state, |tx| {
            let (mut saved, index) = checked_pool(tx, self.state, self.scope, &self.registered)?;
            let (_, attempt) = self.check(&saved.pools[index].slots[self.index()])?;
            if attempt.stage != stage {
                return Err(producer_fence_refusal());
            }
            Ok(ReturnScope {
                pool: saved.pools.swap_remove(index),
                registry: tx.read_slot_registry()?,
                config: self.state.slot_config()?,
            })
        })
    }

    fn update(
        &self,
        snapshot: &ReturnScope,
        update: impl FnOnce(&mut Slot) -> Result<(), Refusal>,
    ) -> Result<(), Refusal> {
        with_slot_state(self.state, |tx| {
            let (mut saved, index) = checked_pool(tx, self.state, self.scope, &self.registered)?;
            let pool = &saved.pools[index];
            let slot = &pool.slots[self.index()];
            if slot != &snapshot.pool.slots[self.index()]
                || pool.profile != snapshot.pool.profile
                || tx.read_slot_registry::<Registry>()? != snapshot.registry
                || self.state.slot_config()?.secondary_roots != snapshot.config.secondary_roots
            {
                return Err(producer_fence_refusal());
            }
            self.check(slot)?;
            update(&mut saved.pools[index].slots[self.index()])?;
            commit_slot_update(tx, &mut saved)
        })
    }
}

fn prepare_stage(slot: &mut Slot, stage: AttemptStage) -> Result<(), Refusal> {
    let Phase::Preparing { attempt, .. } = &mut slot.phase else {
        return Err(producer_fence_refusal());
    };
    attempt.stage = stage;
    Ok(())
}

fn run_guide_anchor_action(descriptor: File, action: String) -> Result<(), Refusal> {
    if cancelled() {
        return Err(StateError::Cancelled.into());
    }
    // Transport the checked file to the guide's fixed action. The action
    // receives an open descriptor and has no checkout pathname argument.
    let mut guide = proc::Spec::new("/bin/bash");
    guide.args = vec![
        "--noprofile".into(),
        "--norc".into(),
        "-c".into(),
        action.into(),
    ];
    guide.cwd = Some(PathBuf::from("/"));
    guide.env_clear = true;
    let ran = proc::run_bounded_complete_with_file_stdin(
        &guide,
        descriptor,
        Duration::from_secs(5),
        proc::Capture {
            stdout_cap: 0,
            stderr_cap: 0,
        },
        cancelled,
    )
    .map_err(|error| match error {
        proc::ChildOutputError::Cancelled => Refusal::State(StateError::Cancelled),
        _ => producer_fence_refusal(),
    })?;
    if !ran.status.success() {
        return Err(producer_fence_refusal());
    }
    Ok(())
}

fn step_check(
    state: &State,
    scope: &SlotScopeArgs,
    generation: u64,
    attempt: u64,
    fd: u32,
) -> Result<SlotResult, Refusal> {
    let input = read_stdin_complete(1024, CREDENTIAL_DEADLINE, cancelled).map_err(|err| {
        if err.kind() == io::ErrorKind::Interrupted {
            StateError::Cancelled.into()
        } else {
            credential_refusal()
        }
    })?;
    let input: StepRequest = serde_json::from_slice(&input).map_err(|_| credential_refusal())?;
    let fence = PreparationFence::acquire(state, scope, generation, attempt, fd, input.auth)?;
    let kind = match input.step {
        PreparationStep::Checkout => {
            let snapshot = fence.snapshot(AttemptStage::PermitIssued)?;
            let index = fence.index();
            let slot = &snapshot.pool.slots[index];
            let (lease, attempt) = fence.check(slot)?;
            if slot.last_head != attempt.prior_head
                || !slot_identity_present(state.root(), &snapshot.pool, slot)
            {
                return Err(producer_fence_refusal());
            }
            verify_requested_tree(&snapshot.pool.main, &lease.requested_commit)?;
            let inspection = if let Some(prior_head) = &attempt.prior_head {
                let proof =
                    inspect_prepared_checkout(&snapshot.pool, &snapshot, index, prior_head)?;
                prepare::verify_write_configuration(&proof.path)?;
                prepare::verify_changes(
                    &snapshot.pool.main,
                    prior_head,
                    &lease.requested_commit,
                    &snapshot.pool.profile,
                )?;
                Some(proof)
            } else {
                verify_no_checkout_ancestor(&snapshot.pool.root)?;
                owner_worktree_inventory(
                    &snapshot.pool.root,
                    &snapshot.registry,
                    Some(&snapshot.pool),
                )?;
                prepare::verify_write_configuration(&snapshot.pool.main)?;
                None
            };
            fence.update(&snapshot, |slot| {
                if !slot_identity_present(state.root(), &snapshot.pool, slot) {
                    return Err(producer_fence_refusal());
                }
                prepare_stage(slot, AttemptStage::CheckoutInProgress)
            })?;
            drop(inspection);
            "checkout_permitted"
        }
        PreparationStep::CheckoutDone => {
            let snapshot = fence.snapshot(AttemptStage::CheckoutInProgress)?;
            let slot = &snapshot.pool.slots[fence.index()];
            let (lease, attempt) = fence.check(slot)?;
            let commit = &lease.requested_commit;
            if attempt.prior_head.is_some() {
                let _proof =
                    inspect_prepared_checkout(&snapshot.pool, &snapshot, fence.index(), commit)?;
                let stage = if prepare::consumers(&snapshot.pool.profile, &slot.producer).is_empty()
                {
                    AttemptStage::CheckoutVerified
                } else {
                    AttemptStage::AwaitingAnchors
                };
                fence.update(&snapshot, |slot| prepare_stage(slot, stage))?;
            } else {
                let mut candidate = snapshot.pool.clone();
                let path = candidate.checkout_path(slot);
                let checkout = new_checkout_identity(&path, &candidate.common_git_id.path)?;
                let epoch = OutputEpoch {
                    pool_id: candidate.pool_id.clone(),
                    slot: slot.number,
                    checkout: checkout.clone(),
                };
                let evidence = first_root_evidence(&candidate.profile, &path, epoch)?;
                let slot = &mut candidate.slots[fence.index()];
                if slot.checkout_id.is_some() {
                    return Err(producer_fence_refusal());
                }
                slot.checkout_id = Some(checkout.clone());
                slot.last_head = Some(commit.clone());
                slot.producer = Producer::NeverBuilt {
                    evidence: Some(evidence.clone()),
                };
                inspect_prepared_checkout(&candidate, &snapshot, fence.index(), commit)?;
                fence.update(&snapshot, |slot| {
                    slot.checkout_id = Some(checkout);
                    slot.last_head = Some(commit.clone());
                    slot.producer = Producer::NeverBuilt {
                        evidence: Some(evidence),
                    };
                    prepare_stage(slot, AttemptStage::CheckoutVerified)
                })?;
            }
            "checkout_verified"
        }
        PreparationStep::Anchors => {
            let snapshot = fence.snapshot(AttemptStage::AwaitingAnchors)?;
            let index = fence.index();
            let slot = &snapshot.pool.slots[index];
            let (lease, _) = fence.check(slot)?;
            let proof = inspect_prepared_checkout(
                &snapshot.pool,
                &snapshot,
                index,
                &lease.requested_commit,
            )?;
            let witnesses = prepare::witness(&proof, &snapshot.pool.profile, &slot.producer)?;
            if witnesses.is_empty() {
                return Err(producer_fence_refusal());
            }
            fence.update(&snapshot, |slot| {
                let Phase::Preparing { attempt, .. } = &mut slot.phase else {
                    return Err(producer_fence_refusal());
                };
                attempt.anchors = witnesses;
                attempt.stage = AttemptStage::AnchorsInProgress;
                Ok(())
            })?;
            "anchor_touch_permitted"
        }
        PreparationStep::AnchorTouch { index, action } => {
            let snapshot = fence.snapshot(AttemptStage::AnchorsInProgress)?;
            let slot = &snapshot.pool.slots[fence.index()];
            let (lease, attempt) = fence.check(slot)?;
            if index != attempt.anchor_cursor || action != GUIDE_ANCHOR_ACTION {
                return Err(producer_fence_refusal());
            }
            let witness = attempt
                .anchors
                .get(index)
                .ok_or_else(producer_fence_refusal)?;
            let checkout = slot
                .checkout_id
                .as_ref()
                .ok_or_else(producer_fence_refusal)?;
            let proof = inspect_checkout(
                &SlotScope {
                    pool: &snapshot.pool,
                    slot,
                    registry: &snapshot.registry,
                    secondary_roots: &snapshot.config.secondary_roots,
                },
                &ExpectedCheckout {
                    identity: checkout,
                    head: Some(&lease.requested_commit),
                },
            )?;
            check_producer_inventory(&proof)?;
            let descriptor = cargo_hive_platform::host_fs()
                .open_regular(&proof.path.join(&witness.path))
                .map_err(|_| producer_fence_refusal())?;
            prepare::verify_anchor_descriptor(&proof, witness, &descriptor)?;
            fence.update(&snapshot, |slot| {
                let Phase::Preparing { attempt, .. } = &mut slot.phase else {
                    return Err(producer_fence_refusal());
                };
                attempt.anchor_cursor += 1;
                Ok(())
            })?;
            run_guide_anchor_action(descriptor, action)?;
            "anchor_touch_complete"
        }
        PreparationStep::AnchorsDone => {
            let snapshot = fence.snapshot(AttemptStage::AnchorsInProgress)?;
            let index = fence.index();
            let slot = &snapshot.pool.slots[index];
            let (lease, attempt) = fence.check(slot)?;
            if attempt.anchor_cursor != attempt.anchors.len() {
                return Err(producer_fence_refusal());
            }
            let proof = inspect_prepared_checkout(
                &snapshot.pool,
                &snapshot,
                index,
                &lease.requested_commit,
            )?;
            let witnesses = prepare::observe_touch(
                &proof,
                &snapshot.pool.profile,
                &slot.producer,
                &attempt.anchors,
            )?;
            fence.update(&snapshot, |slot| {
                let Phase::Preparing { attempt, .. } = &mut slot.phase else {
                    return Err(producer_fence_refusal());
                };
                attempt.anchors = witnesses;
                attempt.stage = AttemptStage::CheckoutVerified;
                Ok(())
            })?;
            "anchor_touch_verified"
        }
        PreparationStep::Seed => {
            let mut snapshot = fence.snapshot(AttemptStage::CheckoutVerified)?;
            let index = fence.index();
            let slot = &snapshot.pool.slots[index];
            let (lease, attempt) = fence.check(slot)?;
            if attempt.prior_head.is_some() {
                return Err(Refusal::ReturnUncertain(
                    "Initial seed requires a newly provisioned checkout.",
                ));
            }
            let commit = lease.requested_commit.clone();
            let Producer::NeverBuilt {
                evidence: Some(evidence),
            } = &slot.producer
            else {
                return Err(producer_fence_refusal());
            };
            let evidence = evidence.clone();
            let mut proof = inspect_prepared_checkout(&snapshot.pool, &snapshot, index, &commit)?;
            fence.update(&snapshot, |slot| {
                prepare_stage(slot, AttemptStage::SeedInProgress)
            })?;
            prepare_stage(
                &mut snapshot.pool.slots[index],
                AttemptStage::SeedInProgress,
            )?;
            proof.release_output_locks();
            let parent = runner_instance(&fence.runner);
            let cancel = || cancelled() || parent_instance().as_ref().ok() != Some(&parent);
            let (seed, ownership) = crate::cli::initial_seed_for_slot(
                state,
                &proof,
                evidence,
                fence.generation,
                &cancel,
            )
            .map_err(|err| {
                if cancel() {
                    Refusal::State(StateError::Cancelled)
                } else {
                    Refusal::ReturnUncertain(err)
                }
            })?;
            let producer = Producer::SeedOnly {
                evidence: seed.evidence,
                generation: seed.generation,
                profile: slots::ProfileBinding {
                    id: snapshot.pool.profile.id.clone(),
                    revision: snapshot.pool.profile.revision,
                },
                final_roots: seed.final_roots,
                registry_units: seed.registry_units,
            };
            let mut candidate = snapshot.pool.clone();
            candidate.slots[index].producer = producer.clone();
            inspect_prepared_checkout(&candidate, &snapshot, index, &commit)?;
            fence.update(&snapshot, |slot| {
                if !ownership.is_current(state) {
                    return Err(producer_fence_refusal());
                }
                slot.producer = producer;
                prepare_stage(slot, AttemptStage::CheckoutVerified)
            })?;
            "initial_seed_verified"
        }
    };
    Ok(SlotResult::new(
        "slot step-check",
        kind,
        Some(scope.repo.clone()),
        0,
    ))
}

fn verify_requested_tree(main: &Path, commit: &str) -> Result<(), Refusal> {
    let root = format!("{commit}^{{tree}}");
    let output = prepare::git_write_scope(
        main,
        &[
            "rev-list",
            "--objects",
            "--no-object-names",
            "--missing=print",
            &root,
        ],
        1024 * 1024,
    )?;
    let mut lines = complete_git_lines(&output).ok_or_else(commit_refusal)?;
    let valid = |line: &str| line.len() == commit.len() && slots::object_id(line);
    if !lines.next().is_some_and(valid) || !lines.all(valid) {
        return Err(commit_refusal());
    }
    Ok(())
}

fn new_checkout_identity(path: &Path, common: &Path) -> Result<slots::CheckoutIdentity, Refusal> {
    let refuse = || {
        Refusal::ReturnUncertain(
            "Could not prove the newly provisioned detached checkout and Git administration path.",
        )
    };
    let directory = owned_identity(path).ok_or_else(refuse)?;
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
    let fields: Vec<_> = complete_git_lines(&facts).ok_or_else(refuse)?.collect();
    let [top, admin, observed_common] = fields.as_slice() else {
        return Err(refuse());
    };
    let admin = PathBuf::from(admin);
    if path.as_os_str() != *top
        || common.as_os_str() != *observed_common
        || admin.parent() != Some(common.join("worktrees").as_path())
    {
        return Err(refuse());
    }
    let identity = slots::owner_directory(&admin).map_err(|_| refuse())?;
    Ok(slots::CheckoutIdentity {
        directory,
        admin: GitIdentity::of(admin, &identity),
    })
}

fn first_root_evidence(
    profile: &PoolProfile,
    path: &Path,
    epoch: OutputEpoch,
) -> Result<slots::EmptyEpoch, Refusal> {
    let unproved = || {
        Refusal::ReturnUncertain(
            "The new checkout's first Cargo output roots are not proved empty.",
        )
    };
    let invocation = profile.invocations.first().ok_or_else(unproved)?;
    let root = |relative: &Path| {
        let root = path.join(relative);
        if canonical_worktree_path(&root).ok().as_ref() != Some(&root) {
            return Err(unproved());
        }
        if !exists(&root).map_err(|_| unproved())? {
            return Ok(slots::RootProof::Absent { path: root });
        }
        let identity = owned_identity(&root)
            .filter(|identity| identity.dev == epoch.checkout.directory.dev)
            .ok_or_else(unproved)?;
        let mut entries = fs::read_dir(&root).map_err(|_| unproved())?;
        if entries.next().is_some() {
            return Err(unproved());
        }
        Ok(slots::RootProof::Empty {
            root: slots::VerifiedRoot {
                path: root,
                identity,
            },
        })
    };
    let roots = OutputRoots {
        target: root(&invocation.target_root)?,
        build: root(&invocation.build_root)?,
    };
    Ok(slots::EmptyEpoch { epoch, roots })
}

fn inspect_detached_checkout(
    pool: &Pool,
    scope: &ReturnScope,
    index: usize,
    commit: Option<&str>,
) -> Result<CheckoutProof, Refusal> {
    let slot = &pool.slots[index];
    let identity = slot
        .checkout_id
        .as_ref()
        .ok_or_else(producer_fence_refusal)?;
    let proof = inspect_checkout(
        &SlotScope {
            pool,
            slot,
            registry: &scope.registry,
            secondary_roots: &scope.config.secondary_roots,
        },
        &ExpectedCheckout {
            identity,
            head: commit,
        },
    )?;
    check_producer_inventory(&proof)?;
    let records = owner_worktree_inventory(&pool.root, &scope.registry, Some(pool))?;
    if !records.iter().any(|record| {
        record.path == proof.path && record.detached && record.head.as_ref() == Some(&proof.head)
    }) {
        return Err(Refusal::ReturnUncertain(
            "The prepared checkout is not detached at the requested commit.",
        ));
    }
    Ok(proof)
}

fn inspect_prepared_checkout(
    pool: &Pool,
    scope: &ReturnScope,
    index: usize,
    commit: &str,
) -> Result<CheckoutProof, Refusal> {
    let proof = inspect_detached_checkout(pool, scope, index, Some(commit))?;
    let slot = &pool.slots[index];
    let eligible = profile::eligible_producer(&slot.producer, slot.generation)?;
    let roots = eligible.roots();
    for invocation in &pool.profile.invocations {
        let shape = InvocationInput {
            profile: slots::ProfileBinding {
                id: pool.profile.id.clone(),
                revision: pool.profile.revision,
            },
            invocation: invocation.id.clone(),
            argv: invocation.argv.clone(),
            cwd: invocation.cwd.clone(),
        };
        let checked = profile::check_invocation(&pool.profile, &shape, &proof, &roots)?;
        if let Producer::Complete { invocations, .. } = &slot.producer
            && invocations
                .iter()
                .filter(|completed| completed.evidence.invocation == invocation.id)
                .any(|completed| {
                    completed.evidence.stable_digest.is_none()
                        || completed.evidence.stable_digest != checked.evidence.stable_digest
                })
        {
            return Err(Refusal::ReturnUncertain(
                "The current producer inputs differ from the retained generation's fixed graph, tools or environment.",
            ));
        }
    }
    Ok(proof)
}

fn empty_gc_roots(evidence: &slots::EmptyEpoch) -> [cargo_hive_gc::checkout::RequiredRoot; 2] {
    use cargo_hive_gc::checkout::{RequiredRoot, RootPresence, RootRole};
    let convert = |role, proof: &slots::RootProof| match proof {
        slots::RootProof::Absent { path } => RequiredRoot {
            role,
            path: path.clone(),
            presence: RootPresence::InitiallyAbsent,
        },
        slots::RootProof::Empty { root } => RequiredRoot {
            role,
            path: root.path.clone(),
            presence: RootPresence::InitiallyEmpty {
                dev: root.identity.dev,
                ino: root.identity.ino,
            },
        },
    };
    [
        convert(RootRole::Target, &evidence.roots.target),
        convert(RootRole::Build, &evidence.roots.build),
    ]
}

fn present_gc_roots(
    roots: &OutputRoots<RootExpectation>,
) -> Result<[cargo_hive_gc::checkout::RequiredRoot; 2], Refusal> {
    use cargo_hive_gc::checkout::{RequiredRoot, RootPresence, RootRole};
    let convert = |role, proof: &RootExpectation| match proof {
        RootExpectation::Present { root } => Ok(RequiredRoot {
            role,
            path: root.path.clone(),
            presence: RootPresence::Present {
                dev: root.identity.dev,
                ino: root.identity.ino,
            },
        }),
        RootExpectation::Absent { .. } => Err(producer_fence_refusal()),
    };
    Ok([
        convert(RootRole::Target, &roots.target)?,
        convert(RootRole::Build, &roots.build)?,
    ])
}

fn ready(
    state: &State,
    request: &SlotLockedOperation,
    attempt: u64,
) -> Result<SlotResult, Refusal> {
    const COMMAND: &str = "slot ready";
    let op = &request.operation;
    let key = request_key(
        &op.scope,
        op.generation,
        OperationKind::Ready,
        &op.op,
        Some(attempt),
    )?;
    let fence = PreparationFence::acquire(
        state,
        &op.scope,
        op.generation,
        attempt,
        request.lock_fd,
        PreparationInput::read()?,
    )?;
    if let Some(result) = replay_ready(
        state,
        COMMAND,
        &op.scope,
        &key,
        ReplayCredential {
            credential: &fence.auth.credential,
            runner: Some(&fence.runner),
        },
        &fence.registered,
        false,
    )? {
        return Ok(result);
    }
    let mut snapshot = fence.snapshot(AttemptStage::CheckoutVerified)?;
    let index = fence.index();
    let slot = &snapshot.pool.slots[index];
    let (lease, attempt) = fence.check(slot)?;
    let anchors = attempt.anchors.clone();
    let preparation = attempt
        .host_binding
        .map(|host_binding| CompletedPreparation {
            attempt_id: attempt.attempt_id,
            begin_op: attempt.begin_op.clone(),
            runner: attempt.runner.clone(),
            host_binding,
        });
    let lease = lease.clone();
    let expected = match &slot.producer {
        Producer::NeverBuilt {
            evidence: Some(evidence),
        } => empty_gc_roots(evidence),
        Producer::SeedOnly { final_roots, .. } => present_gc_roots(final_roots)?,
        Producer::Complete { .. } => {
            present_gc_roots(&profile::eligible_producer(&slot.producer, slot.generation)?.roots())?
        }
        _ => return Err(producer_fence_refusal()),
    };
    let mut proof =
        inspect_prepared_checkout(&snapshot.pool, &snapshot, index, &lease.requested_commit)?;
    let units = prepare::before_gc(&proof, &snapshot.pool.profile, &slot.producer, &anchors)?;
    fence.update(&snapshot, |slot| {
        if slot.outcomes.len() >= MAX_OUTCOMES {
            return Err(Refusal::OutcomeLimit);
        }
        if slot
            .outcomes
            .iter()
            .any(|outcome| key.matches_identity(outcome))
        {
            return Err(producer_fence_refusal());
        }
        prepare_stage(slot, AttemptStage::GcInProgress)
    })?;
    prepare_stage(&mut snapshot.pool.slots[index], AttemptStage::GcInProgress)?;
    proof.release_output_locks();
    let parent = runner_instance(&fence.runner);
    let cancel = || cancelled() || parent_instance().as_ref().ok() != Some(&parent);
    let gc = crate::cli::checkout_gc_for_slot(state, &proof.path, &expected, &cancel).map_err(
        |err| {
            if err.is_cancelled() {
                Refusal::State(StateError::Cancelled)
            } else {
                Refusal::ReturnUncertain(err.detail(&expected))
            }
        },
    )?;
    if gc.completed != expected
        || gc.expected != expected
        || gc.report.failed
        || gc.report.cancelled
        || cancel()
    {
        return Err(producer_fence_refusal());
    }
    let final_proof =
        inspect_prepared_checkout(&snapshot.pool, &snapshot, index, &lease.requested_commit)?;
    prepare::after_gc(
        &final_proof,
        &snapshot.pool.profile,
        &snapshot.pool.slots[index].producer,
        &anchors,
        &units,
    )?;
    let outcome = Outcome {
        pool_id: key.pool_id.into(),
        slot: key.slot,
        generation: key.generation,
        operation: key.operation.into(),
        kind: key.kind,
        attempt_id: key.attempt_id,
        request: OutcomeRequest::Ready {
            runner: fence.runner.clone(),
        },
        credential: fence.auth.credential.clone(),
        result: TransitionResult::Ready {
            commit: lease.requested_commit.clone(),
        },
    };
    let result = make_ready_result(
        COMMAND,
        &snapshot.pool,
        &outcome,
        Some(final_proof.path.clone()),
    )?;
    fence.update(&snapshot, |slot| {
        slot.phase = Phase::Leased {
            ready_commit: lease.requested_commit.clone(),
            lease,
            preparation,
        };
        slot.last_head = Some(final_proof.head);
        slot.outcomes.push(outcome);
        Ok(())
    })?;
    Ok(result)
}

fn make_ready_result(
    command: &'static str,
    pool: &Pool,
    outcome: &Outcome,
    path: Option<PathBuf>,
) -> Result<SlotResult, Refusal> {
    let TransitionResult::Ready { commit } = &outcome.result else {
        return Err(producer_fence_refusal());
    };
    let mut result = SlotResult::new(command, "ready", Some(pool.main.clone()), 0);
    result.pool_id = Some(pool.pool_id.clone());
    result.attempt = outcome.attempt_id;
    result.transition = Some(SlotTransition {
        slot: outcome.slot,
        generation: outcome.generation,
        phase: "leased",
        commit: commit.clone(),
        current: path.is_some(),
    });
    result.use_policy = path.as_ref().map(|_| "guide_guarded_cargo_only");
    result.path = path;
    Ok(result)
}

struct ReplayCredential<'a> {
    credential: &'a str,
    runner: Option<&'a Runner>,
}

fn replay_ready(
    state: &State,
    command: &'static str,
    scope: &SlotScopeArgs,
    key: &SlotRequestKey<'_>,
    auth: ReplayCredential<'_>,
    registered: &VerifiedRepo,
    acknowledge: bool,
) -> Result<Option<SlotResult>, Refusal> {
    let slot_index = usize::from(key.slot - 1);
    let lookup = |slot: &Slot| -> Result<Option<Outcome>, Refusal> {
        let Some(outcome) = slot
            .outcomes
            .iter()
            .find(|entry| key.matches_identity(entry))
        else {
            return Ok(None);
        };
        let OutcomeRequest::Ready {
            runner: saved_runner,
        } = &outcome.request
        else {
            return Ok(None);
        };
        if outcome.credential != auth.credential
            || auth.runner.is_some_and(|runner| runner != saved_runner)
        {
            return Err(producer_fence_refusal());
        }
        Ok(Some(outcome.clone()))
    };
    let snapshot = with_slot_state(state, |tx| {
        let (mut saved, index) = checked_pool(tx, state, scope, registered)?;
        let Some(outcome) = lookup(&saved.pools[index].slots[slot_index])? else {
            return Ok(None);
        };
        Ok(Some((
            outcome,
            ReturnScope {
                pool: saved.pools.swap_remove(index),
                registry: tx.read_slot_registry()?,
                config: state.slot_config()?,
            },
        )))
    })?;
    let Some((outcome, snapshot)) = snapshot else {
        return Ok(None);
    };
    let pool = &snapshot.pool;
    let slot = &pool.slots[slot_index];
    let mut path = None;
    if !acknowledge
        && slot.generation == outcome.generation
        && let Phase::Leased {
            lease,
            ready_commit,
            preparation,
        } = &slot.phase
        && lease.credential == auth.credential
        && preparation
            .as_ref()
            .is_none_or(|preparation| outcome.attempt_id == Some(preparation.attempt_id))
        && outcome.result
            == (TransitionResult::Ready {
                commit: ready_commit.clone(),
            })
    {
        let checkout = pool.checkout_path(slot);
        let identity = slot
            .checkout_id
            .as_ref()
            .ok_or_else(producer_fence_refusal)?;
        inspect::verify_checkout_scope(
            &SlotScope {
                pool,
                slot,
                registry: &snapshot.registry,
                secondary_roots: &snapshot.config.secondary_roots,
            },
            &checkout,
            &ExpectedCheckout {
                identity,
                head: Some(ready_commit),
            },
        )?;
        if git_scope(&checkout, &["rev-parse", "--abbrev-ref", "HEAD"])? != b"HEAD\n" {
            return Err(producer_fence_refusal());
        }
        path = Some(checkout);
    }
    with_slot_state(state, |tx| {
        let (mut saved, index) = checked_pool(tx, state, scope, registered)?;
        let current = &saved.pools[index];
        if &current.slots[slot_index] != slot
            || current.profile != pool.profile
            || tx.read_slot_registry::<Registry>()? != snapshot.registry
            || state.slot_config()?.secondary_roots != snapshot.config.secondary_roots
            || (path.is_some() && !slot_identity_present(state.root(), pool, slot))
        {
            return Err(producer_fence_refusal());
        }
        tx.sync_slots(&saved)?;
        let mut result = make_ready_result(command, pool, &outcome, path)?;
        if acknowledge {
            result.kind = "acknowledged";
            saved.pools[index].slots[slot_index]
                .outcomes
                .retain(|entry| entry != &outcome);
            commit_slot_update(tx, &mut saved)?;
        }
        Ok(Some(result))
    })
}

enum ReturnPlan {
    Inspect(Box<ReturnScope>),
    Replay(Box<SlotResult>),
}

fn return_slot(
    state: &State,
    scope: &SlotScopeArgs,
    generation: u64,
    operation: &str,
) -> Result<SlotResult, Refusal> {
    const COMMAND: &str = "slot return";
    let key = request_key(scope, generation, OperationKind::Return, operation, None)?;
    let input = SlotOperationInput::read()?;
    let registered = verify_operation_repo(state, &scope.repo)?;
    let slot_index = usize::from(key.slot - 1);
    let matches_request =
        |outcome: &Outcome| outcome.kind == key.kind && outcome.credential == input.credential;
    let replay = |tx: &Tx<'_>, saved: &SlotsFile, pool: &Pool, outcome: &Outcome| {
        let result = return_result(COMMAND, "returned", pool, outcome)?;
        tx.sync_slots(saved)?;
        Ok(result)
    };
    // A prior result carries no checkout authority and can outlive the holder's flock.
    let prior = with_slot_state(state, |tx| {
        let (saved, index) = checked_pool(tx, state, scope, &registered)?;
        let pool = &saved.pools[index];
        return_outcome(pool, &key)
            .filter(|outcome| matches_request(outcome))
            .map(|outcome| replay(tx, &saved, pool, outcome))
            .transpose()
    })?;
    if let Some(result) = prior {
        return Ok(result);
    }
    let lock_dir = state.root().join(LOCK_NAMESPACE).join(key.pool_id);
    check_private_dir(&lock_dir).map_err(|err| path_refusal(&lock_dir, err))?;
    let lock_path = lock_dir.join(Slot::lock_name_for(key.slot));
    let (file, lock_id) =
        slots::open_slot_lock(&lock_path).map_err(|err| path_refusal(&lock_path, err))?;
    let _guard = try_exclusive(&file)
        .map_err(|err| path_refusal(&lock_path, err))?
        .ok_or(Refusal::ReturnUncertain(
            "A writer may still hold the slot operation lock.",
        ))?;
    let plan = with_slot_state(state, |tx| {
        let (mut saved, index) = checked_pool(tx, state, scope, &registered)?;
        let pool = &saved.pools[index];
        let slot = &pool.slots[slot_index];
        if slot.lock_id != lock_id || slots::checked_slot_lock(&lock_path).ok() != Some(lock_id) {
            return Err(Refusal::ReturnUncertain(
                "The persisted slot lock identity could not be verified.",
            ));
        }
        if let Some(outcome) = return_outcome(pool, &key) {
            if !matches_request(outcome) {
                return Err(Refusal::Scope("The operation ID is already bound to a different request. Use the guide's exact original request.".into()));
            }
            return replay(tx, &saved, pool, outcome)
                .map(Box::new)
                .map(ReturnPlan::Replay);
        }
        if slot.generation != key.generation
            || !matches!(&slot.phase, Phase::Leased { lease, .. }
            if lease.credential == input.credential)
        {
            return Err(Refusal::Scope(
                "Return requires the original credential and the current ready lease generation."
                    .into(),
            ));
        }
        if slot.outcomes.len() >= MAX_OUTCOMES {
            return Err(Refusal::OutcomeLimit);
        }
        Ok(ReturnPlan::Inspect(Box::new(ReturnScope {
            pool: saved.pools.swap_remove(index),
            registry: tx.read_slot_registry()?,
            config: state.slot_config()?,
        })))
    })?;
    let snapshot = match plan {
        ReturnPlan::Replay(result) => return Ok(*result),
        ReturnPlan::Inspect(snapshot) => snapshot,
    };
    let pool = &snapshot.pool;
    let slot = &pool.slots[slot_index];
    let checkout = slot.checkout_id.as_ref().ok_or(Refusal::ReturnUncertain(
        "The ready lease does not have a verified checkout identity.",
    ))?;
    let proof = inspect_checkout(
        &SlotScope {
            pool,
            slot,
            registry: &snapshot.registry,
            secondary_roots: &snapshot.config.secondary_roots,
        },
        &ExpectedCheckout {
            identity: checkout,
            head: None,
        },
    )?;
    match proof.inventory {
        CheckoutInventory::Clean => (),
        CheckoutInventory::Dirty(detail) => return Err(Refusal::ReturnWork(detail)),
        CheckoutInventory::Uncertain(detail) => return Err(Refusal::ReturnUncertain(detail)),
    }
    let outcome = Outcome {
        pool_id: key.pool_id.into(),
        slot: key.slot,
        generation: key.generation,
        operation: key.operation.into(),
        kind: key.kind,
        attempt_id: key.attempt_id,
        request: OutcomeRequest::Return,
        credential: input.credential,
        result: TransitionResult::Returned {
            commit: proof.head.clone(),
        },
    };
    let result = return_result(COMMAND, "returned", pool, &outcome)?;
    with_slot_state(state, |tx| {
        let (mut saved, index) = checked_pool(tx, state, scope, &registered)?;
        let current = &saved.pools[index];
        if current != pool
            || tx.read_slot_registry::<Registry>()? != snapshot.registry
            || state.slot_config()?.secondary_roots != snapshot.config.secondary_roots
            || current.checkout_path(&current.slots[slot_index]) != proof.path
            || !slot_identity_present(state.root(), current, &current.slots[slot_index])
        {
            return Err(Refusal::ReturnUncertain(
                "The slot or owner scope changed during return inspection. Recheck the current lease.",
            ));
        }
        check_cancelled()?;
        let slot = &mut saved.pools[index].slots[slot_index];
        slot.phase = Phase::Vacant;
        slot.retry = None;
        slot.last_head = Some(proof.head);
        slot.outcomes.push(outcome);
        commit_slot_update(tx, &mut saved)?;
        Ok(result)
    })
}

fn return_outcome<'a>(pool: &'a Pool, key: &SlotRequestKey<'_>) -> Option<&'a Outcome> {
    pool.slots[usize::from(key.slot - 1)]
        .outcomes
        .iter()
        .find(|outcome| key.matches_identity(outcome))
}

fn return_result(
    command: &'static str,
    kind: &'static str,
    pool: &Pool,
    outcome: &Outcome,
) -> Result<SlotResult, Refusal> {
    let TransitionResult::Returned { commit } = &outcome.result else {
        return Err(Refusal::Scope(
            "The recorded operation is not a return.".into(),
        ));
    };
    let mut result = SlotResult::new(command, kind, Some(pool.main.clone()), 0);
    result.pool_id = Some(pool.pool_id.clone());
    result.transition = Some(SlotTransition {
        slot: outcome.slot,
        generation: outcome.generation,
        phase: "vacant",
        commit: commit.clone(),
        current: false,
    });
    Ok(result)
}

enum ReplayAction {
    Outcome,
    Acknowledge,
}

fn dispatch_replay(replay: &SlotReplay, action: ReplayAction) -> i32 {
    let command = match action {
        ReplayAction::Outcome => "slot outcome",
        ReplayAction::Acknowledge => "slot ack",
    };
    let request = &replay.operation;
    run_scoped(command, &request.scope.repo, request.out.json, |state| {
        replay_outcome(state, command, replay, action)
    })
}

fn replay_outcome(
    state: &State,
    command: &'static str,
    replay: &SlotReplay,
    action: ReplayAction,
) -> Result<SlotResult, Refusal> {
    let request = &replay.operation;
    let scope = &request.scope;
    let key = request_key(
        scope,
        request.generation,
        OperationKind::Reserve,
        &request.op,
        replay.attempt,
    )?;
    let missing = || {
        Refusal::Scope("No matching unresolved slot operation was found. Use the guide's original request or ask the repository operator to inspect the current slot.".into())
    };
    let acknowledging = matches!(action, ReplayAction::Acknowledge);
    let input = SlotOperationInput::read()?;
    let registered = verify_operation_repo(state, &scope.repo)?;
    if key.attempt_id.is_some()
        && let Some(result) = replay_ready(
            state,
            command,
            scope,
            &key,
            ReplayCredential {
                credential: &input.credential,
                runner: None,
            },
            &registered,
            acknowledging,
        )?
    {
        return Ok(result);
    }
    with_slot_state(state, |tx| {
        let (mut saved, pool_index) = checked_pool(tx, state, scope, &registered)?;
        let pool = &saved.pools[pool_index];
        let slot_index = usize::from(key.slot - 1);
        let slot = &pool.slots[slot_index];
        let (outcome_index, outcome) = slot
            .outcomes
            .iter()
            .enumerate()
            .find(|(_, outcome)| {
                key.matches_identity(outcome)
                    && (matches!(
                        outcome.kind,
                        OperationKind::Reserve
                            | OperationKind::Return
                            | OperationKind::Begin
                            | OperationKind::Recover
                            | OperationKind::ProducerComplete
                    ) || (!acknowledging && outcome.kind == OperationKind::ProducerStart))
                    && outcome.credential == input.credential
            })
            .or_else(|| {
                if acknowledging || key.attempt_id.is_some() {
                    return None;
                }
                // The original begin request precedes allocation of its attempt ID.
                // Only its historical result can be looked up without that ID.
                slot.outcomes.iter().enumerate().find(|(_, outcome)| {
                    outcome.kind == OperationKind::Begin
                        && outcome.pool_id == key.pool_id
                        && outcome.slot == key.slot
                        && outcome.generation == key.generation
                        && outcome.operation == key.operation
                        && outcome.credential == input.credential
                })
            })
            .ok_or_else(missing)?;
        let acknowledgement = acknowledging.then_some("acknowledged");
        let result = match outcome.kind {
            OperationKind::Reserve => reservation_result(
                command,
                acknowledgement.unwrap_or("reserved"),
                pool,
                slot,
                outcome,
                state.root(),
            ),
            OperationKind::Begin => {
                let mut result = begin_result(command, pool, outcome, false);
                if let Some(kind) = acknowledgement {
                    result.kind = kind;
                }
                Ok(result)
            }
            OperationKind::Return => return_result(
                command,
                acknowledgement.unwrap_or("returned"),
                pool,
                outcome,
            ),
            OperationKind::Recover => {
                recover::recorded_result(command, acknowledgement, pool, outcome)
                    .ok_or_else(missing)
            }
            OperationKind::ProducerStart | OperationKind::ProducerComplete => {
                producer_outcome_result(command, acknowledgement, pool, outcome)
            }
            _ => return Err(missing()),
        };
        let result = result?;
        tx.sync_slots(&saved)?;
        if acknowledging {
            check_cancelled()?;
            saved.pools[pool_index].slots[slot_index]
                .outcomes
                .remove(outcome_index);
            commit_slot_update(tx, &mut saved)?;
        }
        Ok(result)
    })
}

fn producer_outcome_result(
    command: &'static str,
    acknowledgement: Option<&'static str>,
    pool: &Pool,
    outcome: &Outcome,
) -> Result<SlotResult, Refusal> {
    let (label, phase) = match outcome.result {
        TransitionResult::ProducerPending => ("producer_pending", "pending"),
        TransitionResult::ProducerComplete => ("producer_complete", "complete"),
        _ => return Err(watch_refusal()),
    };
    let OutcomeRequest::Producer { ready_commit, .. } = &outcome.request else {
        return Err(watch_refusal());
    };
    let mut result = SlotResult::new(
        command,
        acknowledgement.unwrap_or(label),
        Some(pool.main.clone()),
        0,
    );
    result.pool_id = Some(pool.pool_id.clone());
    result.transition = ready_commit.as_ref().map(|commit| SlotTransition {
        slot: outcome.slot,
        generation: outcome.generation,
        phase,
        commit: commit.clone(),
        current: false,
    });
    Ok(result)
}

fn commit_slot_update(tx: &Tx<'_>, saved: &mut SlotsFile) -> Result<(), Refusal> {
    let previous = saved.revision;
    saved.revision = previous.checked_add(1).ok_or_else(|| {
        Refusal::Scope(
            "slots.json has exhausted its revision counter. Ask the state owner to inspect it."
                .into(),
        )
    })?;
    tx.commit_slots(Some(previous), saved)?;
    Ok(())
}

struct VerifiedRepo {
    registration: registry::Repo,
    main: FileIdentity,
    common_git: GitIdentity,
}

fn verify_operation_repo(state: &State, main: &Path) -> Result<VerifiedRepo, Refusal> {
    let registry: Registry = with_slot_state(state, |tx| Ok(tx.read_slot_registry()?))?;
    let repo = registered_repo(main, &registry)?;
    let metadata = slots::owner_directory(main).map_err(|err| path_refusal(main, err))?;
    Ok(VerifiedRepo {
        registration: repo.clone(),
        main: FileIdentity::of(&metadata),
        common_git: current_git_identity(repo)?,
    })
}

fn checked_pool(
    tx: &Tx<'_>,
    state: &State,
    scope: &SlotScopeArgs,
    registered: &VerifiedRepo,
) -> Result<(SlotsFile, usize), Refusal> {
    check_cancelled()?;
    let registry: Registry = tx.read_slot_registry()?;
    if registered_repo(&scope.repo, &registry)? != &registered.registration {
        return Err(git_scope_refusal(&scope.repo));
    }
    let missing = || {
        Refusal::Scope("The pool namespace is stale or absent. Check the current pool with `cargo hive slot status`.".into())
    };
    let saved = tx.read_slots()?.ok_or_else(missing)?;
    let index = saved
        .pools
        .iter()
        .position(|pool| {
            pool.main.as_os_str() == scope.repo.as_os_str() && pool.pool_id == scope.pool
        })
        .ok_or_else(missing)?;
    let pool = &saved.pools[index];
    let config = state.slot_config()?;
    if pool.common_git_id != registered.common_git
        || owned_identity(&pool.common_git_id.path) != Some(pool.common_git_id.file_id())
        || owned_identity(&pool.main) != Some(registered.main)
        || !accepted_root(&pool.root, &config.secondary_roots)
        || slots::owner_directory(&pool.root).is_err()
        || !pool_directories_present(state.root(), pool)
    {
        return Err(Refusal::Scope("The registered pool's identity or configured root could not be verified. Preserve its files and ask the repository operator to inspect it.".into()));
    }
    Ok((saved, index))
}

fn reservation_result(
    command: &'static str,
    kind: &'static str,
    pool: &Pool,
    slot: &Slot,
    outcome: &Outcome,
    state_root: &Path,
) -> Result<SlotResult, Refusal> {
    let OutcomeRequest::Reserve { commit, .. } = &outcome.request else {
        return Err(Refusal::Scope(
            "The recorded operation is not a reservation.".into(),
        ));
    };
    let mut result = SlotResult::new(command, kind, Some(pool.main.clone()), 0);
    result.pool_id = Some(pool.pool_id.clone());
    let current = slot.generation == outcome.generation
        && slot.retry.is_none()
        && matches!(&slot.phase, Phase::Reserved { lease }
            if lease.reservation_op == outcome.operation && lease.credential == outcome.credential)
        && slot_producer_is_eligible(slot)
        && slot_identity_present(state_root, pool, slot);
    result.transition = Some(SlotTransition {
        slot: outcome.slot,
        generation: outcome.generation,
        phase: "reserved",
        commit: commit.clone(),
        current,
    });
    Ok(result)
}

fn inspect_or_init(
    tx: &Tx<'_>,
    state: &State,
    command: &'static str,
    main: &Path,
    init: Option<(PathBuf, PathBuf)>,
) -> Result<SlotResult, Refusal> {
    if cancelled() {
        return Err(StateError::Cancelled.into());
    }
    let saved = tx.read_slots()?;
    let pools = saved.as_ref().map_or(&[][..], |file| file.pools.as_slice());
    let registry: Registry = tx.read_slot_registry()?;
    let config = state.slot_config()?;
    let existing = pools.iter().find(|pool| pool.main == main);
    let Some((root, profile)) = init else {
        if let Some(pool) = existing {
            let git_verified = match registered_main(main, &registry) {
                Ok(identity) => pool.common_git_id == identity,
                Err(err @ Refusal::State(StateError::Cancelled)) => return Err(err),
                Err(_) => false,
            };
            let scope_verified = git_verified
                && accepted_root(&pool.root, &config.secondary_roots)
                && slots::owner_directory(&pool.root).is_ok()
                && pool_directories_present(state.root(), pool);
            return Ok(project_pool(
                command,
                "status",
                state.root(),
                pool,
                scope_verified,
            ));
        }
        registered_main(main, &registry)?;
        check_namespaces(state.root(), &config.secondary_roots, None, pools)?;
        let mut result = SlotResult::new(command, "unconfigured", Some(main.to_path_buf()), 0);
        result.next_action = Some(
            "Use `cargo hive slot init` with a durable secondary root and a reviewed profile.",
        );
        return Ok(result);
    };

    let identity = registered_main(main, &registry)?;
    if existing.is_some()
        || pools
            .iter()
            .any(|pool| pool.common_git_id.file_id() == identity.file_id())
    {
        return Err(Refusal::Scope(format!(
            "{} already records a pool for this repository. Use `cargo hive slot status` to inspect its capacity.",
            state.root().join(slots::FILE).display()
        )));
    }
    if pools.len() >= MAX_POOLS {
        return Err(Refusal::Scope(
            "The saved pool count is at its limit.".into(),
        ));
    }
    slots::owner_directory(&root).map_err(|err| path_refusal(&root, err))?;
    check_namespaces(state.root(), &config.secondary_roots, Some(&root), pools)?;
    verify_root(&root, state.root(), &config.secondary_roots, &registry)?;
    let profile = PoolProfile::read(&profile).map_err(|err| match err {
        StateError::Malformed { detail, .. } => Refusal::Scope(format!(
            "The reviewed profile {} is invalid ({detail}). Supply a compatible schema 1 profile.",
            profile.display()
        )),
        other => other.into(),
    })?;
    if profile.common_git_id != identity {
        return Err(Refusal::Scope(
            "The reviewed profile names a different registered Git identity.".into(),
        ));
    }
    let revision = match saved.as_ref() {
        None => 1,
        Some(file) => file.revision.checked_add(1).ok_or_else(|| {
            Refusal::Scope(format!(
                "{} has exhausted its revision counter.",
                state.root().join(slots::FILE).display()
            ))
        })?,
    };
    if cancelled() {
        return Err(StateError::Cancelled.into());
    }

    let pool = provision_pool(state.root(), main, root, identity, profile)?;
    let result = project_pool(command, "initialized", state.root(), &pool, true);
    let previous = saved.as_ref().map(|file| file.revision);
    let mut pools = saved.map_or_else(Vec::new, |file| file.pools);
    pools.push(pool);
    tx.commit_slots(previous, &SlotsFile::new(revision, pools))?;
    Ok(result)
}

fn registered_main(main: &Path, registry: &Registry) -> Result<GitIdentity, Refusal> {
    current_git_identity(registered_repo(main, registry)?)
}

fn registered_repo<'a>(main: &Path, registry: &'a Registry) -> Result<&'a registry::Repo, Refusal> {
    registry
        .repos
        .iter()
        .find(|repo| repo.main.as_os_str() == main.as_os_str())
        .ok_or_else(|| {
            Refusal::Scope(
                "Name an exact registered main checkout from `cargo hive repo list`.".into(),
            )
        })
}

fn current_git_identity(repo: &registry::Repo) -> Result<GitIdentity, Refusal> {
    let main = &repo.main;
    slots::owner_directory(main).map_err(|err| path_refusal(main, err))?;
    let output = git_scope(
        main,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-dir",
            "--git-common-dir",
            "--show-superproject-working-tree",
        ],
    )?;
    let fields: Vec<_> = complete_git_lines(&output)
        .ok_or_else(|| git_scope_refusal(main))?
        .collect();
    let [top, admin, common] = fields.as_slice() else {
        return Err(git_scope_refusal(main));
    };
    if main.as_os_str() != *top || admin != common {
        return Err(git_scope_refusal(main));
    }
    let common = PathBuf::from(common);
    let metadata = slots::owner_directory(&common).map_err(|err| path_refusal(&common, err))?;
    let roots = git_scope(main, &["rev-list", "--max-parents=0", "HEAD"])?;
    let mut lines = complete_git_lines(&roots).ok_or_else(|| git_scope_refusal(main))?;
    let mut observed = BTreeSet::new();
    if !lines.all(|line| observed.insert(line))
        || observed.is_empty()
        || !observed
            .into_iter()
            .eq(repo.root_commits.iter().map(String::as_str))
    {
        return Err(git_scope_refusal(main));
    }
    Ok(GitIdentity::of(common, &metadata))
}

fn complete_git_lines(bytes: &[u8]) -> Option<impl Iterator<Item = &str>> {
    Some(
        std::str::from_utf8(bytes)
            .ok()?
            .strip_suffix('\n')?
            .split('\n'),
    )
}

fn git_scope(main: &Path, args: &[&str]) -> Result<Vec<u8>, Refusal> {
    Git::at(main)
        .slot_read_only(args, GIT_OUTPUT_LIMIT)
        .map_err(|err| git_error(main, err))
}

fn git_error(main: &Path, error: SlotGitError) -> Refusal {
    match error {
        SlotGitError::Cancelled => StateError::Cancelled.into(),
        SlotGitError::Incomplete => git_scope_refusal(main),
    }
}

fn git_scope_refusal(main: &Path) -> Refusal {
    Refusal::Scope(format!(
        "Could not establish the complete registered Git scope for {}.",
        main.display()
    ))
}

fn accepted_root(root: &Path, configured: &[PathBuf]) -> bool {
    configured
        .iter()
        .any(|candidate| candidate.is_absolute() && root.starts_with(candidate))
}

fn overlaps(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn verify_root(
    root: &Path,
    state: &Path,
    configured: &[PathBuf],
    registry: &Registry,
) -> Result<(), Refusal> {
    if !accepted_root(root, configured) || overlaps(root, state) {
        return Err(Refusal::Scope(format!(
            "{} must be an independent owner-controlled path in a configured secondary root.",
            root.display()
        )));
    }
    verify_no_checkout_ancestor(root)?;
    let mut remaining = MAX_ROOT_ENTRIES;
    check_nested_checkouts(root, 0, &mut remaining, &[])?;
    owner_worktree_inventory(root, registry, None)?;
    Ok(())
}

fn verify_no_checkout_ancestor(root: &Path) -> Result<(), Refusal> {
    for ancestor in root.ancestors() {
        if exists(&ancestor.join(".git")).map_err(|err| path_refusal(ancestor, err))? {
            return Err(Refusal::Scope(format!(
                "{} is within an existing Git checkout.",
                root.display()
            )));
        }
    }
    Ok(())
}

fn owner_worktree_inventory(
    root: &Path,
    registry: &Registry,
    pool: Option<&Pool>,
) -> Result<Vec<WorktreeRecord>, Refusal> {
    if registry.repos.len() > MAX_POOLS {
        return Err(Refusal::Scope(
            "Too many registered repositories to check the complete owner scope.".into(),
        ));
    }
    let mut linked_slots = Vec::new();
    for repo in &registry.repos {
        check_cancelled()?;
        let common = current_git_identity(repo)?;
        if overlaps(root, &repo.main) || overlaps(root, &common.path) {
            return Err(overlap_refusal(root));
        }
        let own_pool = pool.filter(|pool| pool.main == repo.main);
        if own_pool.is_some_and(|pool| pool.common_git_id != common) {
            return Err(git_scope_refusal(&repo.main));
        }
        let entries = Git::at(&repo.main)
            .worktrees_complete(GIT_OUTPUT_LIMIT)
            .map_err(|err| git_error(&repo.main, err))?;
        let oid_len = repo.root_commits.first().map_or(0, String::len);
        let valid_main = entries.first().is_some_and(|record| {
            record.path.as_os_str() == repo.main.as_os_str()
                && record.head.as_ref().is_some_and(|head| {
                    head.len() == oid_len && head.bytes().any(|byte| byte != b'0')
                })
        });
        if !valid_main || entries.len() > MAX_WORKTREES {
            return Err(git_scope_refusal(&repo.main));
        }
        for record in entries {
            let canonical =
                canonical_worktree_path(&record.path).map_err(|_| git_scope_refusal(&repo.main))?;
            if !overlaps(root, &record.path) && !overlaps(root, &canonical) {
                continue;
            }
            if let Some(pool) = own_pool
                && record.path.as_os_str() == canonical.as_os_str()
                && pool.slots.iter().any(|slot| {
                    slot.checkout_id.is_some() && pool.checkout_path(slot) == record.path
                })
                && !record.prunable
            {
                linked_slots.push(record);
            } else {
                return Err(overlap_refusal(root));
            }
        }
    }
    Ok(linked_slots)
}

fn canonical_worktree_path(path: &Path) -> io::Result<PathBuf> {
    let mut missing = Vec::new();
    for ancestor in path.ancestors() {
        match ancestor.canonicalize() {
            Ok(mut canonical) => {
                canonical.extend(missing.into_iter().rev());
                return Ok(canonical);
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                if exists(ancestor)? {
                    return Err(err);
                }
                missing.push(ancestor.file_name().ok_or(err)?);
            }
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::from(io::ErrorKind::NotFound))
}

fn check_nested_checkouts(
    path: &Path,
    depth: usize,
    remaining: &mut usize,
    skip: &[PathBuf],
) -> Result<(), Refusal> {
    check_cancelled()?;
    if depth > MAX_ROOT_DEPTH {
        return Err(Refusal::Scope(
            "Could not check the complete secondary root within the directory depth limit.".into(),
        ));
    }
    for entry in fs::read_dir(path).map_err(|err| path_refusal(path, err))? {
        let entry = entry.map_err(|err| path_refusal(path, err))?;
        *remaining = remaining.checked_sub(1).ok_or_else(|| {
            Refusal::Scope(
                "Could not check the complete secondary root within the directory entry limit."
                    .into(),
            )
        })?;
        if entry
            .file_name()
            .as_encoded_bytes()
            .eq_ignore_ascii_case(b".git")
        {
            return Err(Refusal::Scope(format!(
                "{} contains an existing Git checkout.",
                path.display()
            )));
        }
        let kind = entry
            .file_type()
            .map_err(|err| path_refusal(&entry.path(), err))?;
        if kind.is_dir() && !skip.contains(&entry.path()) {
            check_nested_checkouts(&entry.path(), depth + 1, remaining, skip)?;
        }
    }
    Ok(())
}

fn overlap_refusal(root: &Path) -> Refusal {
    Refusal::Scope(format!(
        "{} overlaps a registered main or owner worktree.",
        root.display()
    ))
}

fn path_refusal(path: &Path, err: io::Error) -> Refusal {
    Refusal::Scope(format!("Could not verify {}: {err}", path.display()))
}

fn exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

fn check_namespaces(
    state: &Path,
    roots: &[PathBuf],
    selected: Option<&Path>,
    pools: &[Pool],
) -> Result<(), Refusal> {
    let orphan = |path: &Path| Refusal::Orphan(state.join(slots::FILE), path.to_path_buf());
    let checked_namespace = |path: &Path, root: Option<&Path>| {
        if !exists(path).map_err(|_| orphan(path))? {
            return Ok(());
        }
        let mut known = pools
            .iter()
            .filter(|pool| root.is_none_or(|root| pool.root == root))
            .peekable();
        if known.peek().is_none() {
            return Err(orphan(path));
        }
        check_private_dir(path).map_err(|_| orphan(path))?;
        for (index, entry) in fs::read_dir(path).map_err(|_| orphan(path))?.enumerate() {
            let entry = entry.map_err(|_| orphan(path))?;
            if index >= MAX_POOLS
                || !known
                    .clone()
                    .any(|pool| entry.file_name() == pool.pool_id.as_str())
                || check_private_dir(&entry.path()).is_err()
            {
                return Err(orphan(&entry.path()));
            }
        }
        Ok(())
    };
    checked_namespace(&state.join(LOCK_NAMESPACE), None)?;
    let check_checkout_root =
        |root: &Path| checked_namespace(&root.join(CHECKOUT_NAMESPACE), Some(root));
    if roots.len() > MAX_POOLS {
        return Err(Refusal::Scope(
            "Too many configured secondary roots to check for orphaned pool paths.".into(),
        ));
    }
    let mut remaining = MAX_ROOT_ENTRIES;
    for root in roots {
        if !root.is_absolute() {
            return Err(orphan(root));
        }
        if selected.is_some() {
            check_checkout_root(root)?;
        } else if exists(root).map_err(|_| inventory_refusal(root))? {
            scan_checkout_roots(root, 0, &mut remaining, &check_checkout_root)?;
        }
    }
    if let Some(selected) = selected {
        for ancestor in selected.ancestors().skip(1) {
            if !accepted_root(ancestor, roots) {
                break;
            }
            check_checkout_root(ancestor)?;
        }
        scan_checkout_roots(selected, 0, &mut remaining, &check_checkout_root)?;
    }
    let authority = state.join(slots::AUTHORITY_FILE);
    if pools.is_empty() && exists(&authority).map_err(|_| orphan(&authority))? {
        return Err(orphan(&authority));
    }
    Ok(())
}

fn scan_checkout_roots(
    path: &Path,
    depth: usize,
    remaining: &mut usize,
    check_root: &impl Fn(&Path) -> Result<(), Refusal>,
) -> Result<(), Refusal> {
    if cancelled() {
        return Err(StateError::Cancelled.into());
    }
    let incomplete = |_| inventory_refusal(path);
    if depth > MAX_ROOT_DEPTH {
        return Err(inventory_refusal(path));
    }
    let before = fs::symlink_metadata(path).map_err(incomplete)?;
    if !before.is_dir() {
        return Err(inventory_refusal(path));
    }
    check_root(path)?;
    for entry in fs::read_dir(path).map_err(incomplete)? {
        *remaining = remaining
            .checked_sub(1)
            .ok_or_else(|| inventory_refusal(path))?;
        let entry = entry.map_err(incomplete)?;
        if entry.file_type().map_err(incomplete)?.is_dir() {
            scan_checkout_roots(&entry.path(), depth + 1, remaining, check_root)?;
        }
    }
    let after = fs::symlink_metadata(path).map_err(incomplete)?;
    if directory_stamp(&before) != directory_stamp(&after) {
        return Err(inventory_refusal(path));
    }
    Ok(())
}

fn inventory_refusal(path: &Path) -> Refusal {
    Refusal::Scope(format!(
        "Could not establish a complete bounded slot namespace inventory at {}. Preserve the existing files.",
        path.display()
    ))
}

fn check_private_dir(path: &Path) -> io::Result<()> {
    if slots::owner_directory(path)?.mode() & 0o7777 == 0o700 {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "pool directory must have mode 0700",
        ))
    }
}

fn ensure_private_dir(path: &Path) -> Result<(), Refusal> {
    if exists(path).map_err(|err| path_refusal(path, err))? {
        check_private_dir(path).map_err(|err| path_refusal(path, err))
    } else {
        slots::make_private_directory(path).map_err(|err| path_refusal(path, err))
    }
}

fn provision_pool(
    state: &Path,
    main: &Path,
    root: PathBuf,
    common_git_id: GitIdentity,
    profile: PoolProfile,
) -> Result<Pool, Refusal> {
    let pool_id = slots::random_pool_id().map_err(|err| path_refusal(state, err))?;
    let locks = state.join(LOCK_NAMESPACE);
    let checkout_root = root.join(CHECKOUT_NAMESPACE);
    let lock_dir = locks.join(&pool_id);
    let checkout_dir = checkout_root.join(&pool_id);
    for path in [&lock_dir, &checkout_dir] {
        if exists(path).map_err(|err| path_refusal(path, err))? {
            return Err(Refusal::Orphan(state.join(slots::FILE), path.to_path_buf()));
        }
    }
    ensure_private_dir(&locks)?;
    slots::make_private_directory(&lock_dir).map_err(|err| path_refusal(&lock_dir, err))?;
    ensure_private_dir(&checkout_root)?;
    slots::make_private_directory(&checkout_dir).map_err(|err| path_refusal(&checkout_dir, err))?;

    let unused = |number| {
        let path = lock_dir.join(Slot::lock_name_for(number));
        slots::new_slot_lock(&path)
            .map(|lock| Slot::unused(number, lock))
            .map_err(|err| path_refusal(&path, err))
    };
    Ok(Pool {
        pool_id,
        main: main.to_path_buf(),
        common_git_id,
        root,
        profile,
        slots: [unused(1)?, unused(2)?],
    })
}

fn pool_directories_present(state: &Path, pool: &Pool) -> bool {
    let locks = state.join(LOCK_NAMESPACE);
    let checkouts = pool.root.join(CHECKOUT_NAMESPACE);
    [
        locks.join(&pool.pool_id),
        locks,
        checkouts.join(&pool.pool_id),
        checkouts,
    ]
    .iter()
    .all(|path| check_private_dir(path).is_ok())
}

fn slot_identity_present(state: &Path, pool: &Pool, slot: &Slot) -> bool {
    slots::checked_slot_lock(&pool.lock_path(state, slot)).ok() == Some(slot.lock_id)
        && match &slot.checkout_id {
            None => matches!(exists(&pool.checkout_path(slot)), Ok(false)),
            Some(expected) => {
                owned_identity(&pool.checkout_path(slot)) == Some(expected.directory)
                    && owned_identity(&expected.admin.path) == Some(expected.admin.file_id())
            }
        }
}

fn slot_producer_is_eligible(slot: &Slot) -> bool {
    let generation = match slot.phase {
        Phase::Vacant => slot.generation.checked_add(1),
        _ => Some(slot.generation),
    };
    generation.is_some_and(|generation| match slot.checkout_id {
        None => matches!(slot.producer, Producer::NeverBuilt { evidence: None }),
        Some(_) => slot.producer.has_eligible_evidence(generation),
    })
}

fn project_pool(
    command: &'static str,
    kind: &'static str,
    state: &Path,
    pool: &Pool,
    scope_verified: bool,
) -> SlotResult {
    let mut result = SlotResult::new(command, kind, Some(pool.main.clone()), 0);
    result.pool_id = Some(pool.pool_id.clone());
    result.slots = pool
        .slots
        .iter()
        .map(|slot| {
            let path = pool.checkout_path(slot);
            let held = !scope_verified || !slot_identity_present(state, pool, slot);
            let (phase, action) = if slot.producer.lost() {
                ("held", Some(LOST_PROVENANCE))
            } else if held
                || matches!(slot.phase, Phase::Held { .. })
                || !slot_producer_is_eligible(slot)
            {
                ("held", Some(HELD_ACTION))
            } else if matches!(slot.phase, Phase::Vacant) && slot.outcomes.len() >= MAX_OUTCOMES {
                ("held", Some(OUTCOME_LIMIT_ACTION))
            } else {
                (
                    slot.phase.name(),
                    match slot.phase {
                        Phase::Vacant => None,
                        Phase::Leased { .. } => {
                            Some("The current holder must resolve work before returning the slot.")
                        }
                        _ => Some(HELD_ACTION),
                    },
                )
            };
            SlotStatus {
                number: slot.number,
                path,
                phase,
                generation: slot.generation,
                next_action: action,
            }
        })
        .collect();
    if result.slots.iter().all(|slot| slot.phase != "vacant") {
        result.next_action = Some(FRESH_FALLBACK);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bypass_replay_refuses_a_visible_report_until_publication_is_confirmed() {
        let mut slot = Slot::unused(1, FileIdentity { dev: 1, ino: 7 });
        slot.generation = 5;
        let report = BypassReport {
            operation: "b".repeat(32),
            runner: Runner {
                boot_session: "boot".into(),
                pid: 3,
                start_secs: 1,
                start_micros: 0,
                nonce: "c".repeat(64),
            },
        };
        slot.producer = Producer::Tainted {
            epoch: OutputEpoch {
                pool_id: "a".repeat(32),
                slot: 1,
                checkout: slots::CheckoutIdentity {
                    directory: FileIdentity { dev: 1, ino: 2 },
                    admin: GitIdentity {
                        path: "/scratch/owner/.git/worktrees/slot".into(),
                        dev: 1,
                        ino: 3,
                    },
                },
            },
            generation: 5,
            roots: OutputRoots {
                target: None,
                build: None,
            },
            reason: ProvenanceLoss::OffProfile,
            report: Some(report.clone()),
        };
        let result = replay_reported_bypass(&slot, &report, || {
            Err(Refusal::ReturnUncertain(
                "directory sync failed after visible rename",
            ))
        });
        assert!(matches!(
            result,
            Err(Refusal::ReturnUncertain(
                "directory sync failed after visible rename"
            ))
        ));
        assert!(matches!(
            replay_reported_bypass(&slot, &report, || Ok(())),
            Ok(true)
        ));
    }
}
