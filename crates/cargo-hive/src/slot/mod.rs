//! The slot CLI scopes operations to a registered main and its private pool namespace.

mod result;

use crate::cli::{SlotCommand, SlotReplay, SlotScopeArgs};
use cargo_hive_cargo::{
    env,
    registry::{self, Registry},
};
use cargo_hive_platform::flock::{duplicate_inherited_fd, private_file_metadata};
use cargo_hive_platform::proc::{self, Capture, Spec};
use cargo_hive_platform::signals::{self, cancelled};
use cargo_hive_state::slots::{
    self, CHECKOUT_NAMESPACE, FileIdentity, GitIdentity, LOCK_NAMESPACE, Lease, MAX_OUTCOMES,
    MAX_POOLS, OperationKind, Outcome, OutcomeRequest, Phase, Pool, PoolProfile, Slot,
    SlotRequestKey, SlotsFile, TransitionResult,
};
use cargo_hive_state::{State, StateError, Tx};
use result::{FRESH_FALLBACK, SlotResult, SlotStatus, SlotTransition};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, Read, Seek, Write};
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
            return unavailable("slot recover", Some(op.scope.repo), op.out.json);
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
        SlotCommand::Begin(op) => {
            return unavailable(
                "slot begin",
                Some(op.operation.scope.repo),
                op.operation.out.json,
            );
        }
        SlotCommand::StepCheck { scope, out, .. } => {
            return unavailable("slot step-check", Some(scope.repo), out.json);
        }
        SlotCommand::Producer(op) => {
            return unavailable(
                "slot producer",
                Some(op.operation.scope.repo),
                op.operation.out.json,
            );
        }
        SlotCommand::Ready { request, .. } => {
            return unavailable(
                "slot ready",
                Some(request.operation.scope.repo),
                request.operation.out.json,
            );
        }
        SlotCommand::Return(op) => {
            return unavailable("slot return", Some(op.scope.repo), op.out.json);
        }
        SlotCommand::Outcome(op) => return dispatch_replay(&op, ReplayAction::Outcome),
        SlotCommand::Ack(op) => return dispatch_replay(&op, ReplayAction::Acknowledge),
    };
    run_scoped(name, &repo, json, |state| {
        with_slot_state(state, |tx| inspect_or_init(tx, state, name, &repo, init))
    })
}

fn unavailable(command: &'static str, repo: Option<PathBuf>, json: bool) -> i32 {
    let mut result = SlotResult::new(command, "unavailable", repo, 1);
    result.detail = Some("This slot operation is unavailable in this version.".into());
    result.next_action = Some(FRESH_FALLBACK);
    result.emit(json)
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

fn write_key(fd: u32) -> io::Result<()> {
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
    let metadata = private_file_metadata(&file)?;
    let identity = FileIdentity::of(&metadata);
    if metadata.mode() & 0o7777 != 0o600 || metadata.len() != 0 || file.stream_position()? != 0 {
        return Err(invalid());
    }
    for standard in 0..=2 {
        let metadata = File::from(duplicate_inherited_fd(standard)?).metadata()?;
        if FileIdentity::of(&metadata) == identity {
            return Err(invalid());
        }
    }
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
    OutcomeLimit,
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
            StateError::Cancelled.to_string(),
            "Retry when ready.",
        ),
        Refusal::State(err) => ("refused", 1, err.to_string(), FRESH_FALLBACK),
        Refusal::Scope(detail) => ("refused", 1, detail, FRESH_FALLBACK),
        Refusal::Capacity(detail) => ("unavailable", 1, detail.into(), FRESH_FALLBACK),
        Refusal::OutcomeLimit => (
            "unavailable",
            1,
            format!(
                "The slot already has {MAX_OUTCOMES} unacknowledged outcomes. {OUTCOME_LIMIT_ACTION}"
            ),
            FRESH_FALLBACK,
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
        let mut stdin = io::stdin().lock();
        let mut data = [0; KEY_HEX_LEN + 2];
        let mut used = 0;
        while used < data.len() {
            check_cancelled()?;
            match stdin.read(&mut data[used..]) {
                Ok(0) => break,
                Ok(size) => used += size,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return Err(credential_refusal()),
            }
        }
        let data = &data[..used];
        let key = data.strip_suffix(b"\n").unwrap_or(data);
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
            || slot.producer.lost()
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
        replay_reservation(state, command, replay, action)
    })
}

fn replay_reservation(
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
        Refusal::Scope("No matching unresolved reservation was found. Use the guide's original request or ask the repository operator to inspect the current slot.".into())
    };
    if key.attempt_id.is_some() {
        return Err(missing());
    }
    let input = SlotOperationInput::read()?;
    let registered = verify_operation_repo(state, &scope.repo)?;
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
                    && outcome.kind == key.kind
                    && outcome.credential == input.credential
            })
            .ok_or_else(missing)?;
        let kind = match action {
            ReplayAction::Outcome => "reserved",
            ReplayAction::Acknowledge => "acknowledged",
        };
        let result = reservation_result(command, kind, pool, slot, outcome, state.root())?;
        tx.sync_slots(&saved)?;
        if matches!(action, ReplayAction::Acknowledge) {
            check_cancelled()?;
            saved.pools[pool_index].slots[slot_index]
                .outcomes
                .remove(outcome_index);
            commit_slot_update(tx, &mut saved)?;
        }
        Ok(result)
    })
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
        || slots::owner_directory(&pool.common_git_id.path)
            .ok()
            .as_ref()
            .map(FileIdentity::of)
            != Some(pool.common_git_id.file_id())
        || slots::owner_directory(&pool.main)
            .ok()
            .as_ref()
            .map(FileIdentity::of)
            != Some(registered.main)
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
        && matches!(&slot.phase, Phase::Reserved { lease }
            if lease.reservation_op == outcome.operation && lease.credential == outcome.credential)
        && !slot.producer.lost()
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
    let decoded = std::str::from_utf8(&output).map_err(|_| git_scope_refusal(main))?;
    let fields: Vec<_> = decoded.lines().collect();
    let [top, admin, common] = fields.as_slice() else {
        return Err(git_scope_refusal(main));
    };
    if main.as_os_str() != *top || admin != common {
        return Err(git_scope_refusal(main));
    }
    let common = PathBuf::from(common);
    let metadata = slots::owner_directory(&common).map_err(|err| path_refusal(&common, err))?;
    let roots = git_scope(main, &["rev-list", "--max-parents=0", "HEAD"])?;
    let roots = std::str::from_utf8(&roots).map_err(|_| git_scope_refusal(main))?;
    let roots: BTreeSet<_> = roots.lines().collect();
    if roots.is_empty()
        || !roots
            .into_iter()
            .eq(repo.root_commits.iter().map(String::as_str))
    {
        return Err(git_scope_refusal(main));
    }
    Ok(GitIdentity::of(common, &metadata))
}

fn git_scope(main: &Path, args: &[&str]) -> Result<Vec<u8>, Refusal> {
    let mut spec = Spec::new("git");
    spec.args.extend([
        "--no-optional-locks".into(),
        "--no-replace-objects".into(),
        "-c".into(),
        "core.fsmonitor=false".into(),
        "-C".into(),
    ]);
    spec.args.push(main.as_os_str().to_os_string());
    spec.args.extend(args.iter().map(|arg| (*arg).into()));
    let parent: Vec<_> = std::env::vars_os().collect();
    env::git(&mut spec, &parent);
    // Git versions without the lazy-fetch switch still honor the transport allowlist.
    let controlled = [("GIT_NO_LAZY_FETCH", "1"), ("GIT_ALLOW_PROTOCOL", "")];
    spec.env_remove
        .retain(|key| !controlled.iter().any(|(name, _)| key == *name));
    spec.env_remove.extend(
        [
            "CARGO_TARGET_DIR",
            "CARGO_BUILD_TARGET_DIR",
            "CARGO_BUILD_BUILD_DIR",
        ]
        .map(Into::into),
    );
    spec.env_set
        .extend(controlled.map(|(key, value)| (key.into(), value.into())));
    match proc::run_bounded(
        &spec,
        Duration::from_secs(10),
        Capture {
            stdout_cap: GIT_OUTPUT_LIMIT + 1,
            stderr_cap: 1,
        },
        cancelled,
    ) {
        Ok(ran)
            if ran.status.success()
                && ran.stderr_tail.is_empty()
                && ran.stdout.len() <= GIT_OUTPUT_LIMIT =>
        {
            Ok(ran.stdout)
        }
        Err(proc::ChildOutputError::Cancelled) => Err(StateError::Cancelled.into()),
        _ => Err(git_scope_refusal(main)),
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
    for ancestor in root.ancestors() {
        if exists(&ancestor.join(".git")).map_err(|err| path_refusal(ancestor, err))? {
            return Err(Refusal::Scope(format!(
                "{} is within an existing Git checkout.",
                root.display()
            )));
        }
    }
    let mut remaining = MAX_ROOT_ENTRIES;
    check_nested_checkouts(root, 0, &mut remaining)?;
    if registry.repos.len() > MAX_POOLS {
        return Err(Refusal::Scope(
            "Too many registered repositories to check the complete owner scope.".into(),
        ));
    }
    for repo in &registry.repos {
        let common = current_git_identity(repo)?;
        if overlaps(root, &repo.main) || overlaps(root, &common.path) {
            return Err(overlap_refusal(root));
        }
        let inventory = git_scope(&repo.main, &["worktree", "list", "--porcelain", "-z"])?;
        let oid_len = repo.root_commits.first().map_or(0, String::len);
        verify_worktree_inventory(root, &repo.main, oid_len, &inventory)?;
    }
    Ok(())
}

fn check_nested_checkouts(path: &Path, depth: usize, remaining: &mut usize) -> Result<(), Refusal> {
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
        if entry.file_name() == ".git" {
            return Err(Refusal::Scope(format!(
                "{} contains an existing Git checkout.",
                path.display()
            )));
        }
        let kind = entry
            .file_type()
            .map_err(|err| path_refusal(&entry.path(), err))?;
        if kind.is_dir() {
            check_nested_checkouts(&entry.path(), depth + 1, remaining)?;
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

fn verify_worktree_inventory(
    root: &Path,
    main: &Path,
    oid_len: usize,
    inventory: &[u8],
) -> Result<(), Refusal> {
    if !matches!(oid_len, 40 | 64) || inventory.len() > GIT_OUTPUT_LIMIT {
        return Err(git_scope_refusal(main));
    }
    let decoded = std::str::from_utf8(inventory).map_err(|_| git_scope_refusal(main))?;
    let records = decoded
        .strip_suffix("\0\0")
        .ok_or_else(|| git_scope_refusal(main))?;
    let mut paths = BTreeSet::new();
    for (index, record) in records.split("\0\0").enumerate() {
        if index >= MAX_WORKTREES {
            return Err(git_scope_refusal(main));
        }
        let mut fields = record.split('\0');
        let path = fields
            .next()
            .and_then(|first| first.strip_prefix("worktree "))
            .map(Path::new)
            .filter(|path| path.is_absolute())
            .ok_or_else(|| git_scope_refusal(main))?;
        let head = fields.next().ok_or_else(|| git_scope_refusal(main))?;
        // The registered main was already verified as non-bare.
        if (index == 0) != (path == main)
            || !valid_worktree_fields(head, fields, oid_len, index == 0)
            || !paths.insert(path)
        {
            return Err(git_scope_refusal(main));
        }
        if overlaps(root, path) {
            return Err(overlap_refusal(root));
        }
        match path.canonicalize() {
            Ok(canonical) if overlaps(root, &canonical) => return Err(overlap_refusal(root)),
            Ok(_) => (),
            Err(err) if err.kind() == io::ErrorKind::NotFound => (),
            Err(_) => return Err(git_scope_refusal(main)),
        }
    }
    Ok(())
}

fn valid_worktree_fields<'a>(
    head: &str,
    mut fields: impl Iterator<Item = &'a str>,
    oid_len: usize,
    is_main: bool,
) -> bool {
    let Some(oid) = head.strip_prefix("HEAD ") else {
        return false;
    };
    if oid.len() != oid_len
        || !oid
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        || (is_main && oid.bytes().all(|byte| byte == b'0'))
    {
        return false;
    }
    let Some(kind) = fields.next() else {
        return false;
    };
    if kind != "detached" && !kind.strip_prefix("branch ").is_some_and(valid_git_ref) {
        return false;
    }
    let mut locked = false;
    let mut prunable = false;
    fields.all(|field| {
        let (name, reason) = field.split_once(' ').unwrap_or((field, ""));
        let seen = match name {
            "locked" => &mut locked,
            "prunable" => &mut prunable,
            _ => return false,
        };
        (!reason.is_empty() || name == field) && !std::mem::replace(seen, true)
    })
}

fn valid_git_ref(name: &str) -> bool {
    name.contains('/')
        && !name.ends_with('.')
        && !name.contains("..")
        && !name.contains("@{")
        && !name.bytes().any(|byte| {
            byte.is_ascii_control()
                || matches!(byte, b' ' | b'~' | b'^' | b':' | b'?' | b'*' | b'[' | b'\\')
        })
        && name
            .split('/')
            .all(|part| !part.is_empty() && !part.starts_with('.') && !part.ends_with(".lock"))
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
    if FileIdentity::of(&before) != FileIdentity::of(&after)
        || (
            before.mtime(),
            before.mtime_nsec(),
            before.ctime(),
            before.ctime_nsec(),
        ) != (
            after.mtime(),
            after.mtime_nsec(),
            after.ctime(),
            after.ctime_nsec(),
        )
    {
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
        let path = lock_dir.join(format!("slot-{number}.lock"));
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
                slots::owner_directory(&pool.checkout_path(slot))
                    .ok()
                    .as_ref()
                    .map(FileIdentity::of)
                    == Some(expected.directory)
                    && slots::owner_directory(&expected.admin.path)
                        .ok()
                        .as_ref()
                        .map(FileIdentity::of)
                        == Some(expected.admin.file_id())
            }
        }
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
            } else if held || matches!(slot.phase, Phase::Held { .. }) {
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
    fn worktree_inventory_requires_complete_exclusive_fields() {
        let root = Path::new("/unused-slot-root");
        let main = Path::new("/registered-main");
        let head = "a".repeat(40);
        let valid_main =
            format!("worktree /registered-main\0HEAD {head}\0branch refs/heads/main\0\0");
        let linked = format!(
            "worktree /linked\0HEAD {head}\0detached\0locked because\nof this\0prunable reason\0\0"
        );
        assert!(
            verify_worktree_inventory(root, main, 40, format!("{valid_main}{linked}").as_bytes())
                .is_ok()
        );
        assert!(
            verify_worktree_inventory(root, main, 40, format!("{linked}{valid_main}").as_bytes())
                .is_err()
        );
        assert!(
            verify_worktree_inventory(root, main, 40, b"worktree /registered-main\0bare\0\0")
                .is_err()
        );

        for fields in [
            "branch refs/heads/main".to_owned(),
            "HEAD short\0detached".to_owned(),
            format!("HEAD {}\0detached", "a".repeat(64)),
            format!("HEAD {}\0detached", "z".repeat(40)),
            format!("HEAD {head}"),
            format!("HEAD {head}\0HEAD {head}\0detached"),
            format!("HEAD {head}\0detached extra"),
            format!("HEAD {head}\0branch refs/heads/m..ain"),
            format!("HEAD {head}\0branch refs/heads/main\0detached"),
            "bare".to_owned(),
            format!("HEAD {head}\0bare"),
            format!("HEAD {head}\0detached\0locked\0locked again"),
            format!("HEAD {head}\0detached\0prunable one\0prunable two"),
        ] {
            let malformed = format!("{valid_main}worktree /linked\0{fields}\0\0");
            assert!(
                verify_worktree_inventory(root, main, 40, malformed.as_bytes()).is_err(),
                "accepted {fields:?}"
            );
        }
    }
}
