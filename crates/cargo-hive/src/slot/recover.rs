//! Retire explicitly confirmed slot authority under the existing physical fence.

use super::{
    AttemptStage, CREDENTIAL_DEADLINE, CheckoutInventory, ExpectedCheckout, KEY_HEX_LEN,
    LOCK_NAMESPACE, MAX_OUTCOMES, MAX_ROOT_ENTRIES, OperationKind, Outcome, OutcomeRequest, Phase,
    Pool, Refusal, Registry, ReturnScope, Slot, SlotOperation, SlotResult, SlotScope, State,
    StateError, TransitionResult, cancelled, check_cancelled, check_nested_checkouts,
    check_private_dir, checked_pool, commit_slot_update, credential_refusal, inspect_checkout,
    owner_worktree_inventory, path_refusal, read_stdin_complete, request_key,
    slot_identity_present, slot_producer_is_eligible, slots, try_exclusive,
    verify_no_checkout_ancestor, verify_operation_repo, with_slot_state,
};
use cargo_hive_state::slots::{RecoveryQuiescence, RecoveryRequest};
use serde::Deserialize;
use std::io;

const COMMAND: &str = "slot recover";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryInput {
    observed_revision: u64,
    attempt_id: u64,
    begin_op: String,
    authority: RecoveryAuthority,
    quiescence: RecoveryQuiescence,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum RecoveryAuthority {
    Holder { credential: String, nonce: String },
}

impl RecoveryInput {
    fn read() -> Result<Self, Refusal> {
        let bytes = read_stdin_complete(512, CREDENTIAL_DEADLINE, cancelled).map_err(|err| {
            if err.kind() == io::ErrorKind::Interrupted {
                StateError::Cancelled.into()
            } else {
                credential_refusal()
            }
        })?;
        serde_json::from_slice(&bytes).map_err(|_| credential_refusal())
    }
}

pub(super) fn run(state: &State, operation: &SlotOperation) -> Result<SlotResult, Refusal> {
    let input = RecoveryInput::read()?;
    let RecoveryAuthority::Holder { credential, nonce } = input.authority;
    if input.observed_revision == 0
        || input.attempt_id == 0
        || !slots::hex(&input.begin_op, 32)
        || !slots::hex(&credential, KEY_HEX_LEN)
        || !slots::hex(&nonce, KEY_HEX_LEN)
    {
        return Err(credential_refusal());
    }
    let scope = &operation.scope;
    let key = request_key(
        scope,
        operation.generation,
        OperationKind::Recover,
        &operation.op,
        Some(input.attempt_id),
    )?;
    let original = RecoveryRequest {
        observed_revision: input.observed_revision,
        begin_op: input.begin_op,
        nonce,
        quiescence: input.quiescence,
    };
    let request = OutcomeRequest::Recover {
        recovery: Some(original.clone()),
    };
    let outcome = Outcome {
        pool_id: key.pool_id.into(),
        slot: key.slot,
        generation: key.generation,
        operation: key.operation.into(),
        kind: key.kind,
        attempt_id: key.attempt_id,
        request,
        credential,
        result: TransitionResult::Recovered,
    };
    let registered = verify_operation_repo(state, &scope.repo)?;
    let slot_index = usize::from(key.slot - 1);
    let lookup = |pool: &Pool| {
        let slot = &pool.slots[slot_index];
        slot.outcomes
            .iter()
            .find(|saved| saved.generation == key.generation && saved.operation == key.operation)
            .map(|saved| {
                if key.matches_identity(saved) && saved == &outcome {
                    recorded_result(COMMAND, None, pool, saved).ok_or_else(stale)
                } else {
                    Err(stale())
                }
            })
            .transpose()
    };
    if let Some(result) = with_slot_state(state, |tx| {
        let (saved, index) = checked_pool(tx, state, scope, &registered)?;
        let result = lookup(&saved.pools[index])?;
        if result.is_some() {
            tx.sync_slots(&saved)?;
        }
        Ok(result)
    })? {
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

    let snapshot = with_slot_state(state, |tx| {
        let (mut saved, index) = checked_pool(tx, state, scope, &registered)?;
        let pool = &saved.pools[index];
        if let Some(result) = lookup(pool)? {
            tx.sync_slots(&saved)?;
            return Ok(super::ReturnPlan::Replay(Box::new(result)));
        }
        let slot = &pool.slots[slot_index];
        if saved.revision != original.observed_revision
            || slot.generation != key.generation
            || slot.lock_id != lock_id
            || !slot_identity_present(state.root(), pool, slot)
        {
            return Err(stale());
        }
        let Phase::Preparing { attempt, lease } = &slot.phase else {
            return Err(stale());
        };
        if Some(attempt.attempt_id) != key.attempt_id
            || attempt.begin_op != original.begin_op
            || attempt.runner.nonce != original.nonce
            || lease.credential != outcome.credential
        {
            return Err(stale());
        }
        if original.quiescence != RecoveryQuiescence::BeforeFirstEffect
            || attempt.stage != AttemptStage::PermitIssued
            || attempt.prior_head != slot.last_head
            || attempt.lock_offset.is_none()
            || !attempt.anchors.is_empty()
            || attempt.anchor_cursor != 0
        {
            return Err(Refusal::ReturnUncertain(
                "Recovery could not establish that the prior preparation authorized no writer.",
            ));
        }
        if slot.outcomes.len() >= MAX_OUTCOMES {
            return Err(Refusal::OutcomeLimit);
        }
        Ok(super::ReturnPlan::Inspect(Box::new(ReturnScope {
            pool: saved.pools.swap_remove(index),
            registry: tx.read_slot_registry()?,
            config: state.slot_config()?,
        })))
    })?;
    let snapshot = match snapshot {
        super::ReturnPlan::Replay(result) => return Ok(*result),
        super::ReturnPlan::Inspect(snapshot) => snapshot,
    };
    let pool = &snapshot.pool;
    let slot = &pool.slots[slot_index];
    let mut vacant = slot.clone();
    vacant.phase = Phase::Vacant;
    if !slot_producer_is_eligible(&vacant) {
        return Err(Refusal::ReturnUncertain(
            "The retained outputs cannot be used by a new reservation. Create a separate seeded checkout.",
        ));
    }
    let proof = if let Some(checkout) = &slot.checkout_id {
        let proof = inspect_checkout(
            &SlotScope {
                pool,
                slot,
                registry: &snapshot.registry,
                secondary_roots: &snapshot.config.secondary_roots,
            },
            &ExpectedCheckout {
                identity: checkout,
                head: slot.last_head.as_deref(),
            },
        )?;
        match proof.inventory {
            CheckoutInventory::Clean => (),
            CheckoutInventory::Dirty(detail) => return Err(Refusal::ReturnWork(detail)),
            CheckoutInventory::Uncertain(detail) => return Err(Refusal::ReturnUncertain(detail)),
        }
        Some(proof)
    } else {
        verify_no_checkout_ancestor(&pool.root)?;
        let paths = owner_worktree_inventory(&pool.root, &snapshot.registry, Some(pool))?
            .into_iter()
            .map(|record| record.path)
            .collect::<Vec<_>>();
        let mut remaining = MAX_ROOT_ENTRIES;
        check_nested_checkouts(&pool.root, 0, &mut remaining, &paths)?;
        None
    };
    let result = recorded_result(COMMAND, None, pool, &outcome).ok_or_else(stale)?;
    with_slot_state(state, |tx| {
        let (mut saved, index) = checked_pool(tx, state, scope, &registered)?;
        let current = &saved.pools[index];
        if saved.revision != original.observed_revision
            || current != pool
            || tx.read_slot_registry::<Registry>()? != snapshot.registry
            || state.slot_config()?.secondary_roots != snapshot.config.secondary_roots
            || !slot_identity_present(state.root(), current, &current.slots[slot_index])
        {
            return Err(stale());
        }
        check_cancelled()?;
        let slot = &mut saved.pools[index].slots[slot_index];
        slot.phase = Phase::Vacant;
        slot.outcomes.push(outcome);
        commit_slot_update(tx, &mut saved)
    })?;
    drop(proof);
    Ok(result)
}

fn stale() -> Refusal {
    Refusal::Scope(
        "Recovery requires the exact current slot revision, authority and preparation attempt."
            .into(),
    )
}

pub(super) fn recorded_result(
    command: &'static str,
    acknowledgement: Option<&'static str>,
    pool: &Pool,
    outcome: &Outcome,
) -> Option<SlotResult> {
    if outcome.kind != OperationKind::Recover
        || !matches!(
            (&outcome.request, &outcome.result),
            (
                OutcomeRequest::Recover { recovery: Some(_) },
                TransitionResult::Recovered
            )
        )
    {
        return None;
    }
    let mut result = SlotResult::new(
        command,
        acknowledgement.unwrap_or("recovered"),
        Some(pool.main.clone()),
        0,
    );
    result.pool_id = Some(pool.pool_id.clone());
    result.attempt = outcome.attempt_id;
    result.current = Some(false);
    result.next_action =
        Some("Use a new reservation and the normal preparation checks before using the slot.");
    Some(result)
}
