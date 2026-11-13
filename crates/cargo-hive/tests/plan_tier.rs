//! One target is locked through Cargo's lock files, or skipped when a lock is not sound.
#![cfg(target_os = "macos")]
#![allow(clippy::disallowed_methods, clippy::disallowed_types)]
mod common;
use std::os::unix::fs::PermissionsExt;

use cargo_hive_core::locks::{Acquired, Reason, acquire};
use cargo_hive_core::receipt::Outcome;
use cargo_hive_gc::plan_tier::apply::apply;
use common::plan_fixture::Planned;
use common::recording_fs::Recording;

fn skipped(a: Acquired) -> Reason {
    match a {
        Acquired::Skipped(r) => r,
        Acquired::Locked(_) => panic!("expected a skip"),
        Acquired::Failed(why) => panic!("expected a skip, listing failed: {why}"),
    }
}

#[test]
fn a_profile_listing_past_the_cap_is_an_error_and_under_the_cap_still_locks() {
    use cargo_hive_core::locks::acquire_capped;
    let p = Planned::new("list-cap");
    let target = p.worktree_target();
    let n = std::fs::read_dir(&target).unwrap().count();
    let fs = Recording::over_real().fs();
    assert!(
        matches!(
            acquire_capped(fs.as_ref(), &target, false, n),
            Acquired::Locked(_)
        ),
        "a listing at the cap did not lock"
    );
    match acquire_capped(fs.as_ref(), &target, false, n - 1) {
        Acquired::Failed(why) => assert!(why.contains("more than"), "{why}"),
        Acquired::Locked(_) | Acquired::Skipped(_) => panic!("expected a listing error"),
    }
    let before = common::plan_fixture::tree(&target);
    let id = Planned::id_of(&p.plan(), "secondary");
    let lib = p.library_ctx();
    let done = apply(&lib.ctx(), &id, &|| false).unwrap();
    assert_eq!(done.outcome, Outcome::Applied, "{:?}", done.findings);
    assert!(common::plan_fixture::tree(&target).len() < before.len());
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
    assert!(common::wait_until_lock_free(
        &lock,
        std::time::Duration::from_secs(5)
    ));
    let replaced = Recording::over_real().replace_after_lock(lock);
    assert_eq!(
        skipped(acquire(replaced.fs().as_ref(), &p.worktree_target(), false)),
        Reason::Missing
    );
}

#[test]
fn a_profile_that_appears_after_the_locks_are_held_skips_that_target() {
    let p = Planned::new("recheck-profile");
    let id = Planned::id_of(&p.plan(), "secondary");
    let target = p.worktree_target().canonicalize().unwrap();
    let before = common::plan_fixture::tree(&target);
    let rec = Recording::over_real().create_before_listing(
        &target,
        target.join("release/.fingerprint"),
        3,
    );
    let lib = p.library_ctx_with(rec.fs());
    let done = apply(&lib.ctx(), &id, &|| false).unwrap();
    assert_eq!(
        done.counts
            .get("targets_skipped_profile_set")
            .copied()
            .unwrap_or(0),
        1,
        "{:?}",
        done.counts
    );
    assert_eq!(
        done.counts.get("paths_deleted").copied().unwrap_or(0),
        0,
        "{:?}",
        done.findings
    );
    let planted = target.join("release");
    let mut after = common::plan_fixture::tree(&target);
    after.retain(|path, _| !path.starts_with(&planted));
    assert_eq!(after, before);
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
fn locks_are_all_held_before_the_first_removal_and_each_unit_is_removed_dot_d_first_fingerprint_last()
 {
    let p = Planned::new("apply-order");
    let id = Planned::id_of(&p.plan(), "secondary");
    let rec = Recording::over_real();
    let lib = p.library_ctx_with(rec.fs());
    let done = apply(&lib.ctx(), &id, &|| false).unwrap();
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
    let done = apply(&lib.ctx(), &id, &|| false).unwrap();
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
    apply(&lib.ctx(), &id, &|| false).unwrap();
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
        let done = apply(&lib.ctx(), &id, &|| false).unwrap();
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
fn a_closure_plan_copies_each_unmeasured_path_and_counts_only_measured_bytes() {
    let p = Planned::new("measure-gap");
    let target = p.worktree_target();
    let hash = p.dead_unit_hash(&target);
    let artifact = rlib_of(&target, &hash);
    let full =
        cargo_hive_gc::plan_tier::build::make(&p.library_ctx().ctx(), &[], &|| false).unwrap();
    let full_freed = secondary_freed(&full);
    let rec = Recording::over_real().fail_sharing(&artifact);
    let made =
        cargo_hive_gc::plan_tier::build::make(&p.library_ctx_with(rec.fs()).ctx(), &[], &|| false)
            .unwrap();
    let freed = secondary_freed(&made);
    assert!(freed > 0, "measured files were dropped from the byte total");
    assert!(
        freed < full_freed,
        "the unmeasured file was still counted: {freed} vs {full_freed}"
    );
    assert!(
        made.unmeasured
            .iter()
            .any(|row| { row.field == artifact.display().to_string() && !row.reason.is_empty() }),
        "{:?}",
        made.unmeasured
    );
}

#[test]
fn an_orphan_plan_copies_each_unmeasured_path_and_counts_only_measured_bytes() {
    let (p, orphan) = Planned::with_orphan("measure-gap-orphan");
    let hash = p.dead_unit_hash(&orphan);
    let artifact = rlib_of(&orphan, &hash);
    let full =
        cargo_hive_gc::plan_tier::build::make(&p.library_ctx().ctx(), &[], &|| false).unwrap();
    let full_freed = orphan_accounted(&full);
    let rec = Recording::over_real().fail_sharing(&artifact);
    let made =
        cargo_hive_gc::plan_tier::build::make(&p.library_ctx_with(rec.fs()).ctx(), &[], &|| false)
            .unwrap();
    let freed = orphan_accounted(&made);
    assert!(freed > 0, "measured files were dropped from the byte total");
    assert!(freed < full_freed, "{freed} vs {full_freed}");
    assert!(
        made.unmeasured
            .iter()
            .any(|row| { row.field == artifact.display().to_string() && !row.reason.is_empty() }),
        "{:?}",
        made.unmeasured
    );
}

fn rlib_of(target: &std::path::Path, hash: &str) -> std::path::PathBuf {
    let deps = target.join("debug/deps");
    for entry in std::fs::read_dir(&deps).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap();
        if name.contains(hash) && name.ends_with(".rlib") {
            return path;
        }
    }
    panic!("no rlib for {hash} in {}", deps.display());
}

fn secondary_freed(made: &cargo_hive_gc::plan_tier::build::Made) -> u64 {
    made.plans
        .iter()
        .find(|plan| plan.class == cargo_hive_gc::plan_tier::record::PlanClass::Secondary)
        .and_then(|plan| plan.planned)
        .map(|bytes| bytes.freed)
        .unwrap_or(0)
}

fn orphan_accounted(made: &cargo_hive_gc::plan_tier::build::Made) -> u64 {
    made.plans
        .iter()
        .find(|plan| plan.class == cargo_hive_gc::plan_tier::record::PlanClass::Orphan)
        .and_then(|plan| plan.planned)
        .map(|bytes| bytes.freed.saturating_add(bytes.still_shared))
        .unwrap_or(0)
}

#[test]
fn a_dead_path_whose_stat_fails_stores_no_plan_and_names_the_path() {
    let p = Planned::new("stat-fail-plan");
    let target = p.worktree_target();
    let hash = p.dead_unit_hash(&target);
    let mut artifact = None;
    let deps = target.join("debug/deps");
    for entry in std::fs::read_dir(&deps).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap();
        if name.contains(&hash) && name.ends_with(".rlib") {
            artifact = Some(path);
        }
    }
    let artifact = artifact.expect("the dead unit has an rlib");
    let rec = Recording::over_real().fail_stat(&artifact);
    let lib = p.library_ctx_with(rec.fs());
    let made = cargo_hive_gc::plan_tier::build::make(&lib.ctx(), &[], &|| false).unwrap();
    assert!(
        made.plans.is_empty(),
        "a plan was returned though a path could not be stat'd"
    );
    assert!(
        made.findings.iter().any(|(kind, path, _)| {
            kind == "plan.unreadable" && path.as_ref() == Some(&artifact)
        }),
        "{:?}",
        made.findings
    );
    assert!(
        !made
            .nothing_to_do
            .iter()
            .any(|path| path.ends_with("wt0/target") || path == &target),
        "the unreadable target was reported as nothing to do: {:?}",
        made.nothing_to_do
    );
    assert_eq!(cargo_hive_gc::plan_tier::build::command_code(&made), 1);
    assert!(
        !p.hive.home.path.join("plans.json").exists(),
        "gc --plan stored a plan"
    );
}

#[test]
fn a_cancel_between_two_apply_deletions_is_partial_consumed_and_replanned() {
    use cargo_hive_gc::plan_tier::record::{self, Lookup};
    let p = Planned::with_dead_units("apply-cancel-two", 2);
    let id = Planned::id_of(&p.plan(), "secondary");
    let lib = p.library_ctx();
    let plan = match record::get(&lib.state, &|| false, &id).unwrap() {
        Lookup::Live(plan) => plan,
        other => panic!("plan was not live: {other:?}"),
    };
    let target = &plan.targets[0];
    let mut units = Vec::new();
    for entry in &target.entries {
        if !units.iter().any(|unit| unit == &entry.unit) {
            units.push(entry.unit.clone());
        }
    }
    assert!(units.len() >= 2, "need two units, got {units:?}");
    let first: Vec<_> = target
        .entries
        .iter()
        .filter(|entry| entry.unit == units[0])
        .map(|entry| entry.path.clone())
        .collect();
    let second: Vec<_> = target
        .entries
        .iter()
        .filter(|entry| entry.unit == units[1])
        .map(|entry| entry.path.clone())
        .collect();
    let marker = first[0].clone();
    let survivor = second[0].clone();
    let done = apply(&lib.ctx(), &id, &|| !marker.exists()).unwrap();
    assert_eq!(
        (done.outcome, done.reason.as_deref()),
        (Outcome::Partial, Some("cancelled")),
        "{:?}",
        done.findings
    );
    assert!(!marker.exists(), "the first deletion did not happen");
    assert!(survivor.exists(), "the second unit was deleted");
    let again = apply(&lib.ctx(), &id, &|| false).unwrap();
    assert_eq!(again.reason.as_deref(), Some("already_consumed"));
    let listed = p.plan();
    let stored = p.plans_json();
    let fresh = stored["plans"]
        .as_array()
        .unwrap()
        .iter()
        .find(|plan| plan["id"] == Planned::id_of(&listed, "secondary"))
        .unwrap();
    let paths: Vec<_> = fresh["targets"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|target| target["entries"].as_array().unwrap())
        .map(|entry| entry["path"].as_str().unwrap().to_string())
        .collect();
    assert!(
        paths.iter().any(|path| path == survivor.to_str().unwrap()),
        "the remaining unit was not planned again: {paths:?}"
    );
    assert!(
        paths.iter().all(|path| path != marker.to_str().unwrap()),
        "the deleted path was planned again: {paths:?}"
    );
}

#[test]
fn a_cancel_between_units_keeps_the_first_unit_gone_and_the_second_intact_and_the_rest_can_be_planned_again()
 {
    use cargo_hive_gc::plan_tier::apply::{ResolvedTarget, execute};
    use cargo_hive_gc::plan_tier::record::PlanClass;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let p = Planned::with_dead_units("cancel-between", 2);
    let lib = p.library_ctx();
    let ctx = lib.ctx();
    let made = cargo_hive_gc::plan_tier::build::make(&ctx, &[], &|| false).unwrap();
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
    let again = cargo_hive_gc::plan_tier::build::make(&ctx, &[], &|| false).unwrap();
    assert!(
        again.plans.iter().any(|x| x.class == PlanClass::Secondary),
        "the second unit can be planned again"
    );
}

#[test]
fn a_protected_plan_is_deleted_by_the_same_steps_as_a_secondary_one() {
    let p = Planned::new("protected-steps");
    let id = Planned::id_of(&p.plan(), "protected");
    let before = common::plan_fixture::tree(&p.main_target());
    let lib = p.library_ctx();
    let done = apply(&lib.ctx(), &id, &|| false).unwrap();
    assert_eq!(done.outcome, Outcome::Applied, "{:?}", done.findings);
    assert!(common::plan_fixture::tree(&p.main_target()).len() < before.len());
}

#[test]
fn an_orphan_plan_removes_the_whole_target_dir_after_taking_every_lock() {
    let (p, orphan) = Planned::with_orphan("orphan-remove");
    let outside_link_target = p.scratch.path.join("link-target");
    std::fs::write(&outside_link_target, b"keep").unwrap();
    std::os::unix::fs::symlink(&outside_link_target, orphan.join("debug/link-out")).unwrap();
    let id = p.orphan_plan_id(&orphan);
    let rec = Recording::over_real();
    let lib = p.library_ctx_with(rec.fs());
    let done = apply(&lib.ctx(), &id, &|| false).unwrap();
    assert_eq!(done.outcome, Outcome::Applied, "{:?}", done.findings);
    assert!(!orphan.exists());
    assert!(
        outside_link_target.exists(),
        "a symlink inside the orphan must not be followed"
    );
    assert!(
        done.bytes.is_some(),
        "freed and still-shared bytes are reported"
    );
    let log = rec.log();
    let last_lock = log.iter().rposition(|e| e.starts_with("lock ")).unwrap();
    let first_remove = log.iter().position(|e| e.starts_with("remove ")).unwrap();
    assert!(
        last_lock < first_remove,
        "every lock is held before the removal: {log:?}"
    );
}

#[test]
fn a_file_whose_sharing_cannot_be_read_is_named_after_the_orphan_is_removed() {
    let (p, orphan) = Planned::with_orphan("orphan-unmeasured");
    let missed = orphan.join("debug/deps/missed.rlib");
    std::fs::write(&missed, b"xxxx").unwrap();
    let missed = missed.canonicalize().unwrap();
    let id = p.orphan_plan_id(&orphan);
    let rec = Recording::over_real().fail_sharing(&missed);
    let lib = p.library_ctx_with(rec.fs());
    let done = apply(&lib.ctx(), &id, &|| false).unwrap();
    assert_eq!(done.outcome, Outcome::Applied, "{:?}", done.findings);
    assert!(!orphan.exists(), "the directory is still removed");
    assert!(
        done.unmeasured
            .iter()
            .any(|row| row.field == missed.display().to_string()),
        "the unreadable file is named: {:?}",
        done.unmeasured
    );
}

#[test]
fn a_later_refusal_keeps_the_measured_bytes_of_the_orphan_already_removed() {
    let (p, first) = Planned::with_orphan("orphan-partial-bytes");
    let later = first.parent().unwrap().join("later-target");
    let child = std::process::Command::new("cp")
        .args([
            "-R",
            &first.display().to_string(),
            &later.display().to_string(),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let copied = common::wait_within(child, std::time::Duration::from_secs(30));
    assert!(
        copied.status.success(),
        "the second orphan target was not copied"
    );
    let id = p.orphan_plan_id(&first);
    std::fs::create_dir_all(later.join("release/.fingerprint")).unwrap();
    std::fs::write(later.join("release/.cargo-build-lock"), b"").unwrap();
    let done = apply(&p.library_ctx().ctx(), &id, &|| false).unwrap();
    assert_eq!(done.targets.len(), 2, "one plan holds both orphan targets");
    assert_eq!(
        done.outcome,
        Outcome::Partial,
        "{:?} {:?}",
        done.reason,
        done.findings
    );
    assert!(!first.exists(), "the first target was already removed");
    assert!(later.exists(), "the refused target stays");
    let bytes = done
        .bytes
        .expect("measured totals stay after a later refusal");
    assert!(
        bytes.freed + bytes.still_shared > 0,
        "the proceeded removal measured some bytes"
    );
    assert!(done.counts.contains_key("freed_bytes"), "{:?}", done.counts);
    let per_target = format!("freed_bytes:{}", first.display());
    assert!(
        done.counts.contains_key(&per_target),
        "per-target bytes stay: {:?}",
        done.counts.keys()
    );
}

#[test]
fn a_first_orphan_remove_that_fails_is_partial_not_refused() {
    let (p, orphan) = Planned::with_orphan("orphan-first-delete-failed");
    let id = p.orphan_plan_id(&orphan);
    let rec = Recording::over_real().fail_removes_after(0);
    let lib = p.library_ctx_with(rec.fs());
    let done = apply(&lib.ctx(), &id, &|| false).unwrap();
    assert_eq!(
        done.outcome,
        Outcome::Partial,
        "{:?} {:?}",
        done.reason,
        done.findings
    );
    assert_ne!(done.outcome, Outcome::Refused);
    assert_eq!(done.reason.as_deref(), Some("delete failed"));
    assert!(
        done.findings.iter().any(|(kind, path, _)| {
            kind == "plan.delete_failed" && path.as_ref() == Some(&orphan)
        }),
        "{:?}",
        done.findings
    );
    assert!(
        done.unmeasured.iter().any(|row| {
            row.field == orphan.display().to_string()
                && row.reason.contains("removal failed part-way")
        }),
        "a delete that stops part-way names that path: {:?}",
        done.unmeasured
    );
    assert!(
        done.bytes.is_none(),
        "bytes are not invented when the removal never recorded them: {:?}",
        done.bytes
    );
}

#[test]
fn a_later_orphan_is_still_removed_when_an_earlier_remove_fails() {
    let (p, first) = Planned::with_orphan("orphan-continue-after-fail");
    let later = first.parent().unwrap().join("later-target");
    assert!(
        first < later,
        "the failing target is attempted before the one that can succeed"
    );
    let child = std::process::Command::new("cp")
        .args([
            "-R",
            &first.display().to_string(),
            &later.display().to_string(),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let copied = common::wait_within(child, std::time::Duration::from_secs(30));
    assert!(
        copied.status.success(),
        "the second orphan target was not copied"
    );
    let id = p.orphan_plan_id(&first);
    let rec = Recording::over_real().fail_remove(&first);
    let lib = p.library_ctx_with(rec.fs());
    let done = apply(&lib.ctx(), &id, &|| false).unwrap();
    assert_eq!(done.targets.len(), 2, "one plan holds both orphan targets");
    assert!(!later.exists(), "the later target is still removed");
    assert!(first.exists(), "the failed target stays");
    assert_eq!(
        done.outcome,
        Outcome::Partial,
        "{:?} {:?}",
        done.reason,
        done.findings
    );
    assert_eq!(done.reason.as_deref(), Some("delete failed"));
    assert!(
        done.findings.iter().any(|(kind, path, _)| {
            kind == "plan.delete_failed" && path.as_ref() == Some(&first)
        }),
        "{:?}",
        done.findings
    );
    let bytes = done
        .bytes
        .expect("the removal that succeeded still reports its bytes");
    assert!(bytes.freed + bytes.still_shared > 0);
}

#[test]
fn a_later_delete_failure_names_delete_failed_and_keeps_measured_bytes() {
    let (p, first) = Planned::with_orphan("orphan-delete-failed");
    let later = first.parent().unwrap().join("later-target");
    assert!(
        first < later,
        "the first target is removed before the one that fails"
    );
    let child = std::process::Command::new("cp")
        .args([
            "-R",
            &first.display().to_string(),
            &later.display().to_string(),
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let copied = common::wait_within(child, std::time::Duration::from_secs(30));
    assert!(
        copied.status.success(),
        "the second orphan target was not copied"
    );
    let id = p.orphan_plan_id(&first);
    let rec = Recording::over_real().fail_removes_after(1);
    let lib = p.library_ctx_with(rec.fs());
    let done = apply(&lib.ctx(), &id, &|| false).unwrap();
    assert_eq!(done.targets.len(), 2, "one plan holds both orphan targets");
    assert_eq!(
        done.outcome,
        Outcome::Partial,
        "{:?} {:?}",
        done.reason,
        done.findings
    );
    assert_eq!(done.reason.as_deref(), Some("delete failed"));
    assert!(
        done.findings
            .iter()
            .any(|(kind, path, _)| kind == "plan.delete_failed" && path.as_ref() == Some(&later)),
        "{:?}",
        done.findings
    );
    assert!(
        done.findings
            .iter()
            .all(|(kind, _, _)| !kind.starts_with("could not measure:")),
        "a remove error is not a measure refusal: {:?}",
        done.findings
    );
    assert!(!first.exists(), "the first target was already removed");
    assert!(later.exists(), "the failed target stays");
    let bytes = done
        .bytes
        .expect("measured totals stay after a later delete failure");
    assert!(
        bytes.freed + bytes.still_shared > 0,
        "the proceeded removal measured some bytes"
    );
    assert!(done.counts.contains_key("freed_bytes"), "{:?}", done.counts);
}

#[test]
fn a_held_lock_skips_the_orphan_removal_and_deletes_nothing() {
    let (p, orphan) = Planned::with_orphan("orphan-held");
    let id = p.orphan_plan_id(&orphan);
    let mut holder = p
        .hive
        .hold_lock_child("flock", &orphan.join("debug/.cargo-build-lock"));
    let before = common::plan_fixture::tree(&orphan);
    let done = apply(&p.library_ctx().ctx(), &id, &|| false).unwrap();
    let _ = holder.kill();
    let _ = holder.wait();
    assert_ne!(done.outcome, Outcome::Applied);
    assert_eq!(common::plan_fixture::tree(&orphan), before);
}

#[test]
fn a_held_artifact_lock_skips_the_orphan_removal_and_deletes_nothing() {
    let (p, orphan) = Planned::with_orphan("orphan-artifact-lock");
    let id = p.orphan_plan_id(&orphan);
    let artifact = orphan.join("debug/.cargo-artifact-lock");
    std::fs::write(&artifact, b"").unwrap();
    let mut holder = p.hive.hold_lock_child("flock", &artifact);
    let before = common::plan_fixture::tree(&orphan);
    let done = apply(&p.library_ctx().ctx(), &id, &|| false).unwrap();
    let _ = holder.kill();
    let _ = holder.wait();
    assert_ne!(done.outcome, Outcome::Applied, "{:?}", done.reason);
    assert_eq!(
        done.counts.get("paths_deleted").copied().unwrap_or(0),
        0,
        "{:?}",
        done.counts
    );
    assert_eq!(common::plan_fixture::tree(&orphan), before);
}

#[test]
fn a_profile_dir_that_appeared_after_the_plan_refuses_the_removal() {
    let (p, orphan) = Planned::with_orphan("orphan-relist");
    let id = p.orphan_plan_id(&orphan);
    std::fs::create_dir_all(orphan.join("release/.fingerprint")).unwrap();
    std::fs::write(orphan.join("release/.cargo-build-lock"), b"").unwrap();
    let done = apply(&p.library_ctx().ctx(), &id, &|| false).unwrap();
    assert_eq!(done.outcome, Outcome::Refused);
    assert!(orphan.exists());
}

#[test]
fn a_target_dir_whose_identity_changed_after_the_plan_is_not_removed() {
    let (p, orphan) = Planned::with_orphan("orphan-identity");
    let id = p.orphan_plan_id(&orphan);
    let moved = orphan.with_file_name("moved-aside");
    std::fs::rename(&orphan, &moved).unwrap();
    std::fs::create_dir_all(orphan.join("debug/.fingerprint")).unwrap();
    std::fs::write(orphan.join("debug/.cargo-build-lock"), b"").unwrap();
    std::fs::write(
        orphan.join("CACHEDIR.TAG"),
        b"Signature: 8a477f597d28d172789f06886806bc55",
    )
    .unwrap();
    let done = apply(&p.library_ctx().ctx(), &id, &|| false).unwrap();
    assert_eq!(done.outcome, Outcome::Refused);
    assert!(orphan.exists());
}

#[test]
fn a_directory_that_cannot_be_entered_under_a_secondary_root_does_not_refuse_the_orphan_removal() {
    let (p, orphan) = Planned::with_orphan("orphan-cannot-enter");
    let id = p.orphan_plan_id(&orphan);
    let sealed = p.scratch.path.join("sealed");
    std::fs::create_dir_all(&sealed).unwrap();
    std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o222)).unwrap();
    let done = apply(&p.library_ctx().ctx(), &id, &|| false).unwrap();
    std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(done.outcome, Outcome::Applied, "{:?}", done.reason);
    assert!(!orphan.exists(), "the orphan is still there");
    assert!(
        done.findings.iter().any(|(kind, path, _)| {
            kind == "walk.unreadable"
                && path
                    .as_ref()
                    .is_some_and(|path| path.display().to_string().contains("sealed"))
        }),
        "{:?}",
        done.findings
    );
}

#[test]
fn a_checkout_walk_fault_refuses_the_orphan_removal_before_any_delete() {
    let (p, orphan) = Planned::with_orphan("orphan-walk-fault");
    let id = p.orphan_plan_id(&orphan);
    // Enterable but not listable: a build can still run in a checkout inside it.
    let blocked = p.scratch.path.join("blocked-checkouts");
    std::fs::create_dir_all(&blocked).unwrap();
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o111)).unwrap();
    let done = apply(&p.library_ctx().ctx(), &id, &|| false).unwrap();
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(done.outcome, Outcome::Refused, "{:?}", done.reason);
    assert!(orphan.exists(), "a walk fault was treated as no owner");
    assert!(
        done.findings.iter().any(|(_, path, detail)| {
            path.as_ref()
                .is_some_and(|path| path.display().to_string().contains("blocked-checkouts"))
                || detail.contains("blocked-checkouts")
        }),
        "{:?}",
        done.findings
    );
}

#[test]
fn an_unresolved_owner_refuses_the_orphan_and_names_that_reason() {
    let (p, orphan) = Planned::with_orphan("orphan-unresolved");
    let id = p.orphan_plan_id(&orphan);
    std::fs::write(p.worktree.join("Cargo.toml"), b"not a manifest\n").unwrap();
    let done = apply(&p.library_ctx().ctx(), &id, &|| false).unwrap();
    assert_eq!(done.outcome, Outcome::Refused);
    assert_eq!(done.reason.as_deref(), Some("ownership_unresolved"));
    assert!(orphan.exists());
}

#[test]
fn a_target_scan_fault_refuses_the_orphan_removal_before_any_delete() {
    let (p, orphan) = Planned::with_orphan("orphan-scan-fault");
    let id = p.orphan_plan_id(&orphan);
    let blocked = p.worktree.join("unreadable-child");
    std::fs::create_dir_all(&blocked).unwrap();
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o000)).unwrap();
    let done = apply(&p.library_ctx().ctx(), &id, &|| false).unwrap();
    std::fs::set_permissions(&blocked, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(done.outcome, Outcome::Refused, "{:?}", done.reason);
    assert_eq!(done.reason.as_deref(), Some("walk.unreadable"));
    assert!(orphan.exists());
    assert!(
        done.findings.iter().any(|(_, path, detail)| {
            path.as_ref()
                .is_some_and(|path| path.display().to_string().contains("unreadable-child"))
                || detail.contains("unreadable-child")
        }),
        "{:?}",
        done.findings
    );
}

#[test]
fn a_target_dir_that_gained_an_owner_after_the_plan_is_not_removed() {
    let (p, orphan) = Planned::with_orphan("orphan-owned");
    let id = p.orphan_plan_id(&orphan);
    // a registered checkout is now configured to build into that target dir
    std::fs::create_dir_all(p.worktree.join(".cargo")).unwrap();
    std::fs::write(
        p.worktree.join(".cargo/config.toml"),
        format!("[build]\ntarget-dir = \"{}\"\n", orphan.display()),
    )
    .unwrap();
    let done = apply(&p.library_ctx().ctx(), &id, &|| false).unwrap();
    assert_eq!(done.outcome, Outcome::Refused);
    assert!(orphan.exists());
}
