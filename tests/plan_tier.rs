//! One target is locked through Cargo's lock files, or skipped when a lock is not sound.
#![cfg(target_os = "macos")]
mod common;
use cargo_hive::plan_tier::apply::{apply, approval_for_tests, Authorization};
use cargo_hive::plan_tier::locks::{acquire, Acquired, Reason};
use cargo_hive::receipt::Outcome;
use common::plan_fixture::Planned;
use common::recording_fs::Recording;

fn skipped(a: Acquired) -> Reason {
    match a {
        Acquired::Skipped(r) => r,
        Acquired::Locked(_) => panic!("expected a skip"),
    }
}

#[test]
fn a_quiet_target_is_locked_through_both_cargo_lock_files() {
    let p = Planned::new("locks-quiet");
    let rec = Recording::over_real();
    let fs = rec.fs();
    let target = p.worktree_target();
    let Acquired::Locked(_held) = acquire(fs.as_ref(), &target, false) else {
        panic!("not locked")
    };
    let log = rec.log();
    assert!(
        log.iter().any(|l| l.ends_with("debug/.cargo-build-lock")),
        "{log:?}"
    );
    assert!(
        log.iter().any(|l| l.ends_with("debug/.cargo-lock")),
        "{log:?}"
    );
}

#[test]
fn a_lock_held_elsewhere_skips_the_target_for_every_lock_file_name() {
    for name in [".cargo-build-lock", ".cargo-lock"] {
        let p = Planned::new("locks-held");
        let lock = p.worktree_target().join("debug").join(name);
        let mut holder = p.hive.hold_lock_child("flock", &lock);
        let rec = Recording::over_real();
        let reason = skipped(acquire(rec.fs().as_ref(), &p.worktree_target(), false));
        let _ = holder.kill();
        let _ = holder.wait();
        assert_eq!(reason, Reason::Held, "{name}");
    }
}

#[test]
fn an_artifact_lock_is_tried_only_when_examples_are_listed() {
    let p = Planned::new("locks-artifact");
    let artifact = p.worktree_target().join("debug/.cargo-artifact-lock");
    std::fs::write(&artifact, b"").unwrap();
    let mut holder = p.hive.hold_lock_child("flock", &artifact);
    let rec = Recording::over_real();
    assert!(matches!(
        acquire(rec.fs().as_ref(), &p.worktree_target(), false),
        Acquired::Locked(_)
    ));
    let reason = skipped(acquire(rec.fs().as_ref(), &p.worktree_target(), true));
    let _ = holder.kill();
    let _ = holder.wait();
    assert_eq!(reason, Reason::Held);
}

#[test]
fn a_symlinked_lock_file_is_unopenable_and_an_unsupported_flock_is_unlockable() {
    let p = Planned::new("locks-unopenable");
    let lock = p.worktree_target().join("debug/.cargo-build-lock");
    std::fs::remove_file(&lock).unwrap();
    std::os::unix::fs::symlink("/etc/hosts", &lock).unwrap();
    assert_eq!(
        skipped(acquire(
            Recording::over_real().fs().as_ref(),
            &p.worktree_target(),
            false
        )),
        Reason::Unopenable
    );
    let q = Planned::new("locks-unlockable");
    let rec =
        Recording::over_real().unlockable(q.worktree_target().join("debug/.cargo-build-lock"));
    assert_eq!(
        skipped(acquire(rec.fs().as_ref(), &q.worktree_target(), false)),
        Reason::Unlockable
    );
}

#[test]
fn a_profile_dir_with_fingerprints_and_no_lock_file_is_missing_a_lock() {
    let p = Planned::new("locks-missing");
    for name in [".cargo-build-lock", ".cargo-lock", ".cargo-artifact-lock"] {
        let _ = std::fs::remove_file(p.worktree_target().join("debug").join(name));
    }
    assert_eq!(
        skipped(acquire(
            Recording::over_real().fs().as_ref(),
            &p.worktree_target(),
            false
        )),
        Reason::Missing
    );
}

#[test]
fn a_lock_without_identity_or_with_a_replaced_file_skips_the_target() {
    let p = Planned::new("locks-identity");
    let lock = p.worktree_target().join("debug/.cargo-build-lock");
    let none = Recording::over_real().no_identity(lock.clone());
    assert_eq!(
        skipped(acquire(none.fs().as_ref(), &p.worktree_target(), false)),
        Reason::Missing
    );
    let replaced = Recording::over_real().replace_after_lock(lock);
    assert_eq!(
        skipped(acquire(replaced.fs().as_ref(), &p.worktree_target(), false)),
        Reason::Missing
    );
}

#[test]
fn a_profile_dir_that_appears_after_the_locks_are_taken_skips_the_target() {
    let p = Planned::new("locks-profile-set");
    let new_dir = p.worktree_target().join("release/.fingerprint");
    let rec = Recording::over_real().create_dir_after_lock(new_dir);
    let reason = skipped(acquire(rec.fs().as_ref(), &p.worktree_target(), false));
    assert_eq!(reason, Reason::ProfileSet);
}

#[test]
fn locks_are_all_held_before_the_first_removal_and_each_unit_is_removed_dot_d_first_fingerprint_last(
) {
    let p = Planned::new("apply-order");
    let id = Planned::id_of(&p.plan(), "secondary");
    let rec = Recording::over_real();
    let lib = p.library_ctx_with(rec.fs());
    let done = apply(&lib.ctx(), &id, Authorization::Secondary, &|| false).unwrap();
    assert_eq!(done.outcome, Outcome::Applied, "{:?}", done.findings);
    let log = rec.log();
    let last_lock = log.iter().rposition(|l| l.starts_with("lock ")).unwrap();
    let first_remove = log.iter().position(|l| l.starts_with("remove ")).unwrap();
    assert!(last_lock < first_remove, "{log:?}");
    let removes: Vec<&String> = log.iter().filter(|l| l.starts_with("remove ")).collect();
    let last_d = removes
        .iter()
        .rposition(|l| l.ends_with(".d"))
        .expect("a .d file is removed");
    let first_other = removes
        .iter()
        .position(|l| !l.ends_with(".d") && !l.contains("/.fingerprint/"))
        .expect("an artifact other than .d is removed");
    assert!(
        last_d < first_other,
        "every .d goes before any other artifact: {removes:?}"
    );
    let fingerprint = removes
        .iter()
        .position(|l| l.contains("/.fingerprint/"))
        .expect("the fingerprint dir is removed");
    assert_eq!(
        fingerprint,
        removes.len() - 1,
        "the fingerprint directory goes last: {removes:?}"
    );
    let first_unit_read = log
        .iter()
        .position(|l| {
            l.starts_with("read ") && (l.contains("/.fingerprint/") || l.contains("/deps/"))
        })
        .expect("liveness reads unit files through the Fs");
    assert!(
        last_lock < first_unit_read,
        "a unit file was read before every lock was held: {log:?}"
    );
}

#[test]
fn a_removal_failure_is_partial_and_names_delete_failed() {
    let p = Planned::new("apply-delete-failed");
    let id = Planned::id_of(&p.plan(), "secondary");
    let rec = Recording::over_real().fail_removes_after(0);
    let lib = p.library_ctx_with(rec.fs());
    let done = apply(&lib.ctx(), &id, Authorization::Secondary, &|| false).unwrap();
    assert_eq!(done.outcome, Outcome::Partial);
    assert_eq!(done.reason.as_deref(), Some("delete failed"));
}

#[test]
fn applying_ownership_is_resolved_without_touching_the_snapshot_cache() {
    let p = Planned::new("apply-snapshot");
    let id = Planned::id_of(&p.plan(), "secondary");
    let snap = p.hive.home.path.join("snapshot.json");
    let before = std::fs::read(&snap).ok();
    let lib = p.library_ctx();
    apply(&lib.ctx(), &id, Authorization::Secondary, &|| false).unwrap();
    assert_eq!(std::fs::read(&snap).ok(), before);
}

#[test]
fn stopping_after_any_one_deletion_leaves_a_target_cargo_can_build() {
    let probe = Planned::new("prefix-probe");
    let r = probe
        .hive
        .json(&["apply", &Planned::id_of(&probe.plan(), "secondary")])
        .1;
    let steps = r["plans"][0]["counts"]["paths_deleted"].as_u64().unwrap() as usize;
    assert!(
        steps >= 2,
        "a unit needs at least two deletions to have a prefix"
    );
    for stop_after in 0..steps {
        let p = Planned::new(&format!("prefix-{stop_after}"));
        let id = Planned::id_of(&p.plan(), "secondary");
        let rec = Recording::over_real().fail_removes_after(stop_after);
        let lib = p.library_ctx_with(rec.fs());
        let done = apply(&lib.ctx(), &id, Authorization::Secondary, &|| false).unwrap();
        assert_eq!(done.outcome, Outcome::Partial, "stop {stop_after}");
        assert!(
            done.findings.iter().any(|f| f.0 == "plan.delete_failed"),
            "{:?}",
            done.findings
        );
        p.hive.build_crate(&p.worktree, None);
        let rest: u64 = p.plan()["plans"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|x| x["class"] == "secondary")
            .map(|x| x["paths"].as_u64().unwrap())
            .sum();
        assert!(
            rest > 0 || stop_after + 1 == steps,
            "what a stop left behind is planned again (stop {stop_after})"
        );
    }
}

#[test]
fn a_cancel_between_units_keeps_the_first_unit_gone_and_the_second_intact_and_the_rest_can_be_planned_again(
) {
    use cargo_hive::plan_tier::apply::{execute, ResolvedTarget};
    use cargo_hive::plan_tier::record::PlanClass;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let p = Planned::with_dead_units("cancel-between", 2);
    let lib = p.library_ctx();
    let ctx = lib.ctx();
    let made = cargo_hive::plan_tier::build::make(&ctx, &[], &|| false).unwrap();
    let plan = made
        .plans
        .into_iter()
        .find(|x| x.class == PlanClass::Secondary)
        .unwrap();
    let resolved = vec![ResolvedTarget {
        path: plan.targets[0].path.clone(),
        owner: plan.targets[0].owner.clone(),
        class: PlanClass::Secondary,
    }];
    let before = common::plan_fixture::tree(&p.worktree_target());
    let polls = AtomicUsize::new(0);
    let done = execute(&ctx, plan, &resolved, &|| {
        polls.fetch_add(1, Ordering::SeqCst) >= 1
    });
    assert_eq!(
        (done.outcome, done.reason.as_deref()),
        (Outcome::Partial, Some("cancelled"))
    );
    let after = common::plan_fixture::tree(&p.worktree_target());
    let gone: Vec<_> = before.keys().filter(|k| !after.contains_key(*k)).collect();
    let hashes: std::collections::BTreeSet<&str> = gone
        .iter()
        .filter_map(|k| k.to_str())
        .flat_map(|s| {
            s.match_indices('-').filter_map(move |(at, _)| {
                let tail = &s[at + 1..];
                let hash = tail.get(..16)?;
                let rest = &tail[16..];
                let boundary = rest.is_empty() || rest.starts_with('.') || rest.starts_with('/');
                (boundary
                    && hash
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
                .then_some(hash)
            })
        })
        .collect();
    assert_eq!(hashes.len(), 1, "exactly the first unit is gone: {gone:?}");
    let again = cargo_hive::plan_tier::build::make(&ctx, &[], &|| false).unwrap();
    assert!(
        again.plans.iter().any(|x| x.class == PlanClass::Secondary),
        "the second unit can be planned again"
    );
}

#[test]
fn an_approved_protected_plan_is_deleted_by_the_same_steps_as_a_secondary_one() {
    let p = Planned::new("approved-protected");
    let id = Planned::id_of(&p.plan(), "protected");
    let before = common::plan_fixture::tree(&p.main_target());
    let lib = p.library_ctx();
    let wrong = apply(
        &lib.ctx(),
        &id,
        Authorization::Approved(approval_for_tests("0000000000000000")),
        &|| false,
    )
    .unwrap();
    assert_eq!(wrong.reason.as_deref(), Some("approval_mismatch"));
    assert_eq!(common::plan_fixture::tree(&p.main_target()), before);
    let done = apply(
        &lib.ctx(),
        &id,
        Authorization::Approved(approval_for_tests(&id)),
        &|| false,
    )
    .unwrap();
    assert_eq!(done.outcome, Outcome::Applied, "{:?}", done.findings);
    assert!(common::plan_fixture::tree(&p.main_target()).len() < before.len());
}
