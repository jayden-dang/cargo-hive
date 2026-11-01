//! One target is locked through Cargo's lock files, or skipped when a lock is not sound.
#![cfg(target_os = "macos")]
mod common;
use cargo_hive::plan_tier::locks::{acquire, Acquired, Reason};
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
