#![cfg(target_os = "macos")]
mod common;
use cargo_hive::platform::apfs::Apfs;
use cargo_hive::platform::{Fs, LockKind, LockState, TryLock};
use common::{Hive, Scratch};
use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;
use std::time::{Duration, Instant};

fn apfs() -> Apfs {
    Apfs::new(env!("CARGO_BIN_EXE_cargo-hive").into())
}

#[test]
fn every_lock_kind_held_elsewhere_refuses_the_exclusive_try_without_waiting() {
    let s = Scratch::new("locks");
    for kind in [LockKind::Flock, LockKind::Fcntl, LockKind::FcntlRead] {
        let path = s.path.join(format!("{}.lock", kind.as_arg()));
        std::fs::write(&path, b"").unwrap();
        let holder = apfs().hold_lock(kind, &path).unwrap();
        let started = Instant::now();
        assert!(
            matches!(apfs().try_lock_exclusive(&path), TryLock::Held),
            "{kind:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{kind:?}: the try waited"
        );
        holder.release();
        assert!(
            matches!(apfs().try_lock_exclusive(&path), TryLock::Acquired(_)),
            "{kind:?}"
        );
    }
}

#[test]
fn a_held_guard_refuses_fcntl_tries_from_another_process() {
    let s = Scratch::new("reverse");
    let path = s.path.join("r.lock");
    std::fs::write(&path, b"").unwrap();
    let TryLock::Acquired(guard) = apfs().try_lock_exclusive(&path) else {
        panic!("could not lock")
    };
    for kind in [LockKind::FcntlRead, LockKind::Fcntl, LockKind::Flock] {
        assert_eq!(
            apfs().try_lock_elsewhere(kind, &path).unwrap(),
            LockState::Held,
            "{kind:?}"
        );
    }
    drop(guard);
    for kind in [LockKind::FcntlRead, LockKind::Fcntl, LockKind::Flock] {
        assert_eq!(
            apfs().try_lock_elsewhere(kind, &path).unwrap(),
            LockState::Free,
            "{kind:?}"
        );
    }
}

#[test]
fn the_exclusive_try_never_creates_follows_or_blocks() {
    let s = Scratch::new("trylock-shapes");
    let missing = s.path.join("missing.lock");
    assert!(matches!(apfs().try_lock_exclusive(&missing), TryLock::Gone));
    assert!(!missing.exists(), "the try created the file");
    let real = s.path.join("real.lock");
    std::fs::write(&real, b"").unwrap();
    let link = s.path.join("link.lock");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(
        matches!(apfs().try_lock_exclusive(&link), TryLock::Unopenable(_)),
        "a symlink was followed"
    );
    let fifo = s.path.join("fifo.lock");
    assert!(std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap()
        .success());
    let started = Instant::now();
    assert!(matches!(
        apfs().try_lock_exclusive(&fifo),
        TryLock::Unopenable(_)
    ));
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "opening a FIFO blocked"
    );
}

#[test]
fn a_lock_child_that_never_answers_is_abandoned_after_the_deadline() {
    let s = Scratch::new("deadline");
    let script = s.path.join("silent");
    std::fs::write(&script, "#!/bin/sh\nexec sleep 30\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = s.path.join("x.lock");
    std::fs::write(&path, b"").unwrap();
    let silent = Apfs::new(script);
    let started = Instant::now();
    assert!(silent.hold_lock(LockKind::Flock, &path).is_err());
    assert!(silent.try_lock_elsewhere(LockKind::Fcntl, &path).is_err());
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "the deadline was not applied"
    );
}

#[test]
fn an_unknown_lock_kind_is_rejected_by_both_lock_commands() {
    let hive = Hive::new();
    let s = Scratch::new("badkind");
    for cmd in ["__hold-lock", "__try-lock"] {
        let out = hive.run(&[cmd, "nope", s.path.join("x").to_str().unwrap()]);
        assert_eq!(out.status.code(), Some(2), "{cmd}");
    }
}

#[test]
fn the_lock_commands_never_create_hive_state() {
    let hive = Hive::new();
    let s = Scratch::new("holder-state");
    std::fs::remove_dir_all(&hive.home.path).unwrap();
    let path = s.path.join("x.lock");
    std::fs::write(&path, b"").unwrap();
    let mut child = hive
        .command(&["__hold-lock", "fcntl-read", path.to_str().unwrap()])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let tried = hive.run(&["__try-lock", "fcntl", path.to_str().unwrap()]);
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(line.trim(), "held");
    assert_eq!(String::from_utf8_lossy(&tried.stdout).trim(), "refused");
    assert!(!hive.home.path.exists());
    std::fs::create_dir_all(&hive.home.path).unwrap(); // so the scratch drop has a dir to clean
}
