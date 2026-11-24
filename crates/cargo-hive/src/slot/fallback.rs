//! Seed placement for a fresh secondary checkout independent of every slot.

use super::inspect::{inspect_fresh_checkout, owned_identity, verify_linked_head};
use super::result::SlotResult;
use super::{
    GIT_OUTPUT_LIMIT, Refusal, VerifiedRepo, canonical_worktree_path, check_cancelled,
    check_namespaces, exists, git_error, registered_repo, slot_identity_present,
    verify_operation_repo, with_slot_state,
};
use crate::cli::{SlotOutputScope, place_absent_registry_seed, verify_slot_output_scope};
use cargo_hive_cargo::{discover::output_paths_may_overlap, git::Git, metadata};
use cargo_hive_gc::checkout::{RequiredRoot, RootPresence, RootRole};
use cargo_hive_platform::signals::cancelled;
use cargo_hive_seed::place::SlotDestination;
use cargo_hive_state::slots;
use cargo_hive_state::{State, StateError};
use std::path::Path;

pub(super) const COMMAND: &str = "slot seed-fallback";

pub(super) fn seed(
    state: &State,
    repo: &Path,
    checkout: &Path,
    commit: &str,
) -> Result<SlotResult, Refusal> {
    if !slots::object_id(commit) || checkout.to_str().is_none() {
        return Err(Refusal::SeedFallback(
            "Name a separate canonical secondary checkout and the full requested commit.",
        ));
    }
    if metadata::TARGET_VARS
        .iter()
        .any(|name| std::env::var_os(name).is_some())
    {
        return Err(Refusal::SeedFallback(
            "Remove Cargo target and build directory overrides before using the seed fallback.",
        ));
    }
    let registered = verify_operation_repo(state, repo)?;
    checked_scope(state, &registered, checkout, None)?;
    let common = &registered.common_git.path;
    let identity = inspect_fresh_checkout(checkout, common, commit).map_err(|_| {
        seed_refused("Could not prove a clean, independent fresh checkout at the requested commit.")
    })?;
    let linked = Git::at(repo)
        .worktrees_complete(GIT_OUTPUT_LIMIT)
        .map_err(|err| git_error(repo, err))?;
    if !linked.iter().any(|record| {
        record.path == checkout
            && record.detached
            && !record.prunable
            && record.head.as_deref() == Some(commit)
    }) {
        return Err(Refusal::SeedFallback(
            "The fresh secondary must be a registered linked worktree detached at the requested commit.",
        ));
    }

    let resolved = metadata::resolve(checkout, cancelled).map_err(|_| {
        seed_refused("Could not establish the fresh checkout's Cargo output roots.")
    })?;
    let target = resolved.target_directory;
    if resolved.workspace_root != checkout
        || target != resolved.build_directory
        || !target.starts_with(checkout)
        || target == checkout
        || canonical_worktree_path(&target).ok().as_ref() != Some(&target)
        || !target.parent().is_some_and(|parent| {
            owned_identity(parent).is_some_and(|parent| parent.dev == identity.directory.dev)
        })
        || !matches!(exists(&target), Ok(false))
    {
        return Err(Refusal::SeedFallback(
            "Seed fallback requires coincident, absent Cargo output roots within the fresh checkout.",
        ));
    }
    let expected = [RootRole::Target, RootRole::Build].map(|role| RequiredRoot {
        path: target.clone(),
        role,
        presence: RootPresence::InitiallyAbsent,
    });
    let scope = verify_slot_output_scope(state, checkout, &expected, &cancelled)
        .map_err(|_| seed_refused("Could not prove an independent seed output scope."))?;
    let current = checked_scope(state, &registered, checkout, Some(&target))?;
    if current.registry != scope.registry || current.config != scope.config {
        return Err(Refusal::SeedFallback(
            "The seed fallback's registration or configuration changed before placement.",
        ));
    }
    verify_linked_head(checkout, &identity, common, commit)
        .map_err(|_| seed_refused("The fresh checkout identity changed before placement."))?;

    // A recorded pool cannot move, and a new pool cannot claim this linked checkout.
    // Placement also checks the registration, configuration, and destination identity.
    let destination = SlotDestination {
        target: &target,
        checkout_identity: identity.directory,
        registry: &scope.registry,
        config: &scope.config,
    };
    let placement =
        place_absent_registry_seed(state, checkout, Some(repo), &destination, &cancelled)
            .map_err(seed_refused)?;
    check_cancelled()?;
    if !scope.is_current(state) || owned_identity(&target) != Some(placement.root.identity) {
        return Err(Refusal::SeedFallback(
            "The seed fallback's output or registration changed during placement.",
        ));
    }
    verify_linked_head(checkout, &identity, common, commit)
        .map_err(|_| seed_refused("The fresh checkout identity changed during placement."))?;

    let mut result = SlotResult::new(COMMAND, "seeded_fallback", Some(repo.to_path_buf()), 0);
    result.path = Some(checkout.to_path_buf());
    result.fallback_commit = Some(commit.to_owned());
    result.registry_units = Some(placement.units.len() as u64);
    Ok(result)
}

fn checked_scope(
    state: &State,
    registered: &VerifiedRepo,
    checkout: &Path,
    target: Option<&Path>,
) -> Result<SlotOutputScope, Refusal> {
    with_slot_state(state, |tx| {
        check_cancelled()?;
        let registry = tx.read_slot_registry()?;
        let config = state.slot_config()?;
        if registered_repo(&registered.registration.main, &registry)? != &registered.registration
            || owned_identity(&registered.registration.main) != Some(registered.main)
            || owned_identity(&registered.common_git.path) != Some(registered.common_git.file_id())
        {
            return Err(Refusal::SeedFallback(
                "The fallback repository's registered identity changed.",
            ));
        }
        let saved = tx.read_slots()?;
        let pools = saved.as_ref().map_or(&[][..], |file| file.pools.as_slice());
        for pool in pools {
            check_cancelled()?;
            if output_paths_may_overlap(checkout, &pool.root)
                || target.is_some_and(|target| output_paths_may_overlap(target, &pool.root))
            {
                return Err(Refusal::SeedFallback(
                    "The seed fallback overlaps a recorded slot pool. Choose a separate secondary checkout.",
                ));
            }
            if slots::owner_directory(&pool.root).is_err()
                || !pool
                    .slots
                    .iter()
                    .all(|slot| slot_identity_present(state.root(), pool, slot))
            {
                return Err(Refusal::SeedFallback(
                    "Could not prove the seed fallback independent of the recorded slot paths.",
                ));
            }
        }
        if target.is_none() {
            check_namespaces(state.root(), &config.secondary_roots, Some(checkout), pools)?;
        }
        Ok(SlotOutputScope { registry, config })
    })
}

fn seed_refused(detail: &'static str) -> Refusal {
    if cancelled() {
        StateError::Cancelled.into()
    } else {
        Refusal::SeedFallback(detail)
    }
}
