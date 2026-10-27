#![cfg(target_os = "macos")]
mod common;
use cargo_hive::platform::apfs::Apfs;
use cargo_hive::platform::{Fs, LockKind, LockState};
use common::{Hive, Scratch};
use std::io::{BufRead, BufReader};
use std::process::Stdio;
use std::time::{Duration, Instant};

fn apfs() -> Apfs {
    Apfs::new(env!("CARGO_BIN_EXE_cargo-hive").into())
}

#[test]
fn a_lock_held_by_another_process_is_reported_held_without_waiting() {
    let s = Scratch::new("locks");
    for kind in [LockKind::Flock, LockKind::Fcntl] {
        let path = s.path.join(format!("{}.lock", kind.as_arg()));
        std::fs::write(&path, b"").unwrap();
        let holder = apfs().hold_lock(kind, &path).unwrap();
        let started = Instant::now();
        assert_eq!(
            apfs().probe_lock(&path).unwrap(),
            LockState::Held,
            "{kind:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{kind:?}: the probe waited"
        );
        holder.release();
        assert_eq!(
            apfs().probe_lock(&path).unwrap(),
            LockState::Free,
            "{kind:?}"
        );
    }
}

#[test]
fn an_unknown_lock_kind_is_rejected() {
    let hive = Hive::new();
    let s = Scratch::new("badkind");
    let out = hive.run(&["__hold-lock", "nope", s.path.join("x").to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn the_lock_holder_never_creates_hive_state() {
    let hive = Hive::new();
    let s = Scratch::new("holder-state");
    std::fs::remove_dir_all(&hive.home.path).unwrap();
    let path = s.path.join("x.lock");
    std::fs::write(&path, b"").unwrap();
    let mut child = hive
        .command(&["__hold-lock", "flock", path.to_str().unwrap()])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(line.trim(), "held");
    assert!(!hive.home.path.exists());
}
