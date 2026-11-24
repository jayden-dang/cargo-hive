//! Retire explicitly confirmed slot authority under the existing physical fence.

use super::{
    Attempt, AttemptStage, CREDENTIAL_DEADLINE, CompletedPreparation, KEY_HEX_LEN, LOCK_NAMESPACE,
    Lease, MAX_OUTCOMES, MAX_ROOT_ENTRIES, OperationKind, Outcome, OutcomeRequest, Phase, Pool,
    Refusal, Registry, ReturnScope, Runner, Slot, SlotOperation, SlotResult, State, StateError,
    TransitionResult, boot_witness, cancelled, check_cancelled, check_nested_checkouts,
    check_private_dir, checked_pool, commit_slot_update, credential_refusal,
    inspect_detached_checkout, owner_worktree_inventory, path_refusal, read_stdin_complete,
    request_key, slot_identity_present, slot_producer_is_eligible, slots, try_exclusive,
    verify_no_checkout_ancestor, verify_operation_repo, with_slot_state,
};
use cargo_hive_platform::slot::BootWitness;
use cargo_hive_state::slots::{
    PreparationRetry, PriorPhase, RecoveryAuthorization, RecoveryQuiescence, RecoveryRequest,
};
use serde::Deserialize;
use std::io;

const COMMAND: &str = "slot recover";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryInput {
    observed_revision: u64,
    attempt_id: Option<u64>,
    begin_op: Option<String>,
    authority: RecoveryAuthority,
    quiescence: RecoveryQuiescence,
    next_begin_op: Option<String>,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum RecoveryAuthority {
    Holder {
        credential: String,
        nonce: Option<String>,
    },
    Operator {
        confirmation: OperatorConfirmation,
        result_credential: String,
    },
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum OperatorConfirmation {
    AbandonResolvedWork,
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
    let (credential, authority) = match input.authority {
        RecoveryAuthority::Holder { credential, nonce } => {
            if nonce.is_some() != input.attempt_id.is_some()
                || !nonce
                    .as_ref()
                    .is_none_or(|value| slots::hex(value, KEY_HEX_LEN))
            {
                return Err(credential_refusal());
            }
            (credential, RecoveryAuthorization::Holder { nonce })
        }
        RecoveryAuthority::Operator {
            confirmation: OperatorConfirmation::AbandonResolvedWork,
            result_credential,
        } => (result_credential, RecoveryAuthorization::Operator),
    };
    if input.observed_revision == 0
        || input.attempt_id == Some(0)
        || input.attempt_id.is_some() != input.begin_op.is_some()
        || !input
            .begin_op
            .as_ref()
            .is_none_or(|operation| slots::hex(operation, 32))
        || input.attempt_id.is_some() != (input.quiescence != RecoveryQuiescence::NoPermitIssued)
        || input.next_begin_op.as_ref().is_some_and(|op| {
            !slots::hex(op, 32)
                || op == &operation.op
                || input.begin_op.as_ref() == Some(op)
                || input.quiescence != RecoveryQuiescence::VerifiedLaterBoot
                || !matches!(authority, RecoveryAuthorization::Holder { .. })
        })
        || !slots::hex(&credential, KEY_HEX_LEN)
    {
        return Err(credential_refusal());
    }
    let scope = &operation.scope;
    let key = request_key(
        scope,
        operation.generation,
        OperationKind::Recover,
        &operation.op,
        input.attempt_id,
    )?;
    let original = RecoveryRequest {
        observed_revision: input.observed_revision,
        begin_op: input.begin_op,
        authority,
        quiescence: input.quiescence,
        next_begin_op: input.next_begin_op,
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
        let (lease, preparation) = prior_lease(&slot.phase).ok_or_else(stale)?;
        let authority_matches = match &original.authority {
            RecoveryAuthorization::Holder { nonce } => {
                lease.credential == outcome.credential
                    && preparation.map(|preparation| &preparation.identity().2.nonce)
                        == nonce.as_ref()
            }
            RecoveryAuthorization::Operator => {
                lease.credential != outcome.credential
                    && !slot
                        .outcomes
                        .iter()
                        .any(|entry| entry.credential == outcome.credential)
            }
        };
        if !authority_matches
            || preparation.map(|preparation| preparation.identity().0) != key.attempt_id
            || preparation.map(|preparation| preparation.identity().1)
                != original.begin_op.as_deref()
        {
            return Err(stale());
        }
        let evidence_matches_phase = match (original.quiescence, preparation) {
            (RecoveryQuiescence::NoPermitIssued, None) => true,
            (RecoveryQuiescence::BeforeFirstEffect, Some(PriorPreparation::Preparing(attempt))) => {
                attempt.stage == AttemptStage::PermitIssued
                    && attempt.prior_head == slot.last_head
                    && attempt.lock_offset.is_some()
                    && attempt.anchors.is_empty()
                    && attempt.anchor_cursor == 0
            }
            (RecoveryQuiescence::ForegroundCompleted, Some(PriorPreparation::Completed(_))) => true,
            (RecoveryQuiescence::VerifiedLaterBoot, Some(_)) => true,
            _ => false,
        };
        if !evidence_matches_phase {
            return Err(quiescence_unproved());
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
    if slot.producer.lost() {
        return Err(Refusal::ReturnUncertain(
            "The output provenance is lost and this slot does not regain warm capacity for the retained outputs.",
        ));
    }
    let mut vacant = slot.clone();
    vacant.phase = Phase::Vacant;
    let retry = if let Some(begin_op) = &original.next_begin_op {
        let Phase::Preparing { lease, attempt } = &slot.phase else {
            return Err(stale());
        };
        if attempt.stage != AttemptStage::GcInProgress
            || begin_op == &lease.reservation_op
            || slot
                .outcomes
                .iter()
                .any(|entry| entry.generation == slot.generation && &entry.operation == begin_op)
            || !slot_producer_is_eligible(slot)
        {
            return Err(stale());
        }
        Some(PreparationRetry {
            retired_attempt: attempt.attempt_id,
            begin_op: begin_op.clone(),
        })
    } else {
        None
    };
    if retry.is_none() && !slot_producer_is_eligible(&vacant) {
        return Err(Refusal::ReturnUncertain(
            "The retained output provenance cannot be established for a new reservation.",
        ));
    }
    let (previous_lease, previous_preparation) = prior_lease(&slot.phase).ok_or_else(stale)?;
    let boot_required = matches!(
        original.quiescence,
        RecoveryQuiescence::ForegroundCompleted | RecoveryQuiescence::VerifiedLaterBoot
    );
    if boot_required {
        let observed = boot_witness(key.pool_id).map_err(|_| quiescence_unproved())?;
        if !previous_preparation
            .is_some_and(|preparation| preparation.matches_boot(original.quiescence, &observed))
        {
            return Err(quiescence_unproved());
        }
    }
    let checkout_in_progress = matches!(
        (original.quiescence, previous_preparation),
        (RecoveryQuiescence::VerifiedLaterBoot, Some(PriorPreparation::Preparing(attempt)))
            if attempt.stage == AttemptStage::CheckoutInProgress
    );
    let expected_head = match (original.quiescence, previous_preparation) {
        (RecoveryQuiescence::VerifiedLaterBoot, Some(PriorPreparation::Preparing(attempt)))
            if attempt.stage == AttemptStage::CheckoutInProgress =>
        {
            None
        }
        (RecoveryQuiescence::VerifiedLaterBoot, Some(PriorPreparation::Preparing(attempt)))
            if attempt.stage != AttemptStage::PermitIssued =>
        {
            Some(previous_lease.requested_commit.as_str())
        }
        _ => slot.last_head.as_deref(),
    };
    let proof = if slot.checkout_id.is_some() {
        let proof = inspect_detached_checkout(pool, &snapshot, slot_index, expected_head)?;
        if checkout_in_progress
            && proof.head != previous_lease.requested_commit
            && Some(proof.head.as_str()) != slot.last_head.as_deref()
        {
            return Err(Refusal::ReturnUncertain(
                "The interrupted checkout is outside its prior and requested HEAD scope.",
            ));
        }
        if retry.is_some() {
            // A fresh preparation needs a positive baseline for every reviewed
            // consumer. An interrupted cleanup cannot supply that proof.
            super::prepare::witness(&proof, &pool.profile, &slot.producer)?;
        }
        Some(proof)
    } else {
        if expected_head.is_some() {
            return Err(Refusal::ReturnUncertain(
                "The prior checkpoint lacks its durable checkout identity.",
            ));
        }
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
        slot.phase = if retry.is_some() {
            Phase::Reserved {
                lease: previous_lease.clone(),
            }
        } else {
            Phase::Vacant
        };
        slot.retry = retry;
        if let Some(proof) = &proof {
            slot.last_head = Some(proof.head.clone());
        }
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

fn quiescence_unproved() -> Refusal {
    Refusal::ReturnUncertain(
        "The prior slot writers could not be proved quiescent for the requested recovery.",
    )
}

#[derive(Clone, Copy)]
enum PriorPreparation<'a> {
    Preparing(&'a Attempt),
    Completed(&'a CompletedPreparation),
}

impl<'a> PriorPreparation<'a> {
    fn identity(self) -> (u64, &'a str, &'a Runner) {
        match self {
            Self::Preparing(attempt) => (attempt.attempt_id, &attempt.begin_op, &attempt.runner),
            Self::Completed(attempt) => (attempt.attempt_id, &attempt.begin_op, &attempt.runner),
        }
    }

    fn matches_boot(self, proof: RecoveryQuiescence, observed: &BootWitness) -> bool {
        let binding = match self {
            Self::Preparing(attempt) => attempt.host_binding,
            Self::Completed(preparation) => Some(preparation.host_binding),
        };
        if binding != Some(observed.host_binding) {
            return false;
        }
        let same_boot = self.identity().2.boot_session == observed.boot_session;
        match proof {
            RecoveryQuiescence::ForegroundCompleted => {
                matches!(self, Self::Completed(_)) && same_boot
            }
            RecoveryQuiescence::VerifiedLaterBoot => !same_boot,
            _ => false,
        }
    }
}

fn prior_lease(phase: &Phase) -> Option<(&Lease, Option<PriorPreparation<'_>>)> {
    match phase {
        Phase::Reserved { lease }
        | Phase::Held {
            prior: PriorPhase::Reserved { lease },
            ..
        } => Some((lease, None)),
        Phase::Preparing { attempt, lease }
        | Phase::Held {
            prior: PriorPhase::Preparing { attempt, lease },
            ..
        } => Some((lease, Some(PriorPreparation::Preparing(attempt)))),
        Phase::Leased {
            preparation: Some(preparation),
            lease,
            ..
        }
        | Phase::Held {
            prior:
                PriorPhase::Leased {
                    preparation: Some(preparation),
                    lease,
                    ..
                },
            ..
        } => Some((lease, Some(PriorPreparation::Completed(preparation)))),
        _ => None,
    }
}

pub(super) fn recorded_result(
    command: &'static str,
    acknowledgement: Option<&'static str>,
    pool: &Pool,
    outcome: &Outcome,
) -> Option<SlotResult> {
    let (
        OperationKind::Recover,
        OutcomeRequest::Recover {
            recovery: Some(request),
        },
        TransitionResult::Recovered,
    ) = (outcome.kind, &outcome.request, &outcome.result)
    else {
        return None;
    };
    let mut result = SlotResult::new(
        command,
        acknowledgement.unwrap_or("recovered"),
        Some(pool.main.clone()),
        0,
    );
    result.pool_id = Some(pool.pool_id.clone());
    result.attempt = outcome.attempt_id;
    result.current = Some(false);
    result.next_action = Some(if request.next_begin_op.is_some() {
        "Begin the same lease with the requested fresh operation and complete the normal preparation checks."
    } else {
        "Use a new reservation and the normal preparation checks before using the slot."
    });
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_later_boot_requires_a_different_session_and_the_same_native_host() {
        let mut prior = Attempt {
            attempt_id: 2,
            begin_op: "1".repeat(32),
            runner: Runner {
                boot_session: "earlier-boot".into(),
                pid: 2,
                start_secs: 1,
                start_micros: 0,
                nonce: "a".repeat(64),
            },
            host_binding: Some([7; 32]),
            stage: AttemptStage::GcInProgress,
            lock_offset: Some(1 << 32),
            prior_head: Some("b".repeat(40)),
            anchors: Vec::new(),
            anchor_cursor: 0,
        };
        let proves = |attempt: &Attempt, boot: &str, digest| {
            PriorPreparation::Preparing(attempt).matches_boot(
                RecoveryQuiescence::VerifiedLaterBoot,
                &BootWitness {
                    boot_session: boot.into(),
                    host_binding: digest,
                },
            )
        };
        assert!(proves(&prior, "later-boot", [7; 32]));
        assert!(!proves(&prior, "earlier-boot", [7; 32]));
        assert!(!proves(&prior, "later-boot", [8; 32]));
        prior.host_binding = None;
        assert!(!proves(&prior, "later-boot", [7; 32]));
    }
}
