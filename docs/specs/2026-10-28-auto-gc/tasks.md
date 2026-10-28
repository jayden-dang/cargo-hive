# Tasks: Auto-tier GC (superseded incremental sessions and rustc temp entries)

> **For agentic workers:** after plan approval, pick one execute skill:
> - `build-in-waves`: a continuous, dependency-aware scheduler;
> - `build-by-story`: human-gated story review units;
> - `build-inline`: the controller implements, with no implementer subagents.
>
> The chosen skill writes `Execution-mode:`. Steps use checkbox (`- [ ]`) syntax for tracking.

Feature code: AUTOGC
Status: Approved
Date: 2026-10-28
Execution-mode: continuous
Max-concurrency: auto
Requirements: ./requirements.md
Design: ./design.md

**Goal:** Ship `cargo hive gc [<repo>…] [--dry-run]`, which deletes superseded rustc incremental sessions and leftover rustc temp dirs in Cargo target dirs. A session with a lock file is deleted under that lock, re-checked under it; a session with no lock file is deleted without one. Temp entries are deleted under `.cargo-build-lock`. The receipt counts freed bytes per inode, reported as totals, and doctor check (d) proves the lock primitive on each machine.

**Architecture:**
- **Platform** gains one lock primitive: `flock(LOCK_EX|LOCK_NB)` on an fd opened `O_NOFOLLOW|O_NONBLOCK`, held as a guard. It also gains a cross-process try (`__try-lock`) and no-follow stat, list, remove and `statfs`.
- **`src/gc.rs`** holds three parts:
  - a pure planner that mirrors rustc's session rules;
  - a runner that does every effect through `Fs`, in the order lock → re-check → scan → dir → lock file → release;
  - a per-inode ledger.
- **The CLI** `gc` follows the `status` path.

**Tech Stack:** Rust 2021, rustc 1.97.1 on macOS 27 (APFS). Existing dependencies only: `libc` 0.2.191, `serde`, `serde_json`, `clap`. No new dependency.

## Global Constraints

The canonical sources (`docs/standards/` is the SSOT). Read them; do not restate them. Hashes were taken 2026-10-28, and a mismatch at execute time means the source changed after planning.

| Source | sha256 (first 16) |
|---|---|
| `docs/standards/INDEX.md` | `80cd41328c0f7652` |
| `docs/standards/code.md` (bounds and deadlines, `// SAFETY:`, no new deps, no requirement IDs in code or comments, `//!` module comments) | `afdcc8b89136f573` |
| `docs/standards/errors-logging.md` (typed error enums, refusals are data, exit codes 0/1/2, no `process::exit` outside `main.rs`) | `20c605d2caffab13` |
| `docs/standards/testing.md` (merge gate; boundary tests; scratch-only fixtures; the APFS disk-image exception) | `de677bebcc504edd` |
| `docs/architecture/INDEX.md`: this feature relies on ARCH-1, 2, 3, 4, 8, 9, 10, 11, 13, 14, 15, 16, 17, 18, 19 | `8cb275c512d9ab3c` |
| `docs/specs/2026-10-28-auto-gc/design.md` (Decisions 1–16, O1–O5) | `289def7513339b96` (before this plan; re-hash at execute time) |
| `.skills/AUTOGC/close-package.md` | `5bdc37eeaff836b0` |

Merge gate (testing standard). Every task must leave all four commands green:
- `cargo fmt --check`, after `cargo fmt`. The code in this plan is the content; rustfmt owns the layout.
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- `cargo check --target x86_64-unknown-linux-gnu`

Known pre-existing flake, outside this feature: `tests/status_targets.rs::an_interrupted_status_waiting_on_the_state_lock_exits_130_without_writing_the_snapshot` fails intermittently in full-suite runs. It failed 3 of 4 times on an untouched d6e0aff and passes alone, per the plan review. Do not fix it inside an AUTOGC task. If it fails, rerun the suite and report it; it is tracked as a separate change.

Commits: each commit step is `cargo fmt`, then `git add -A && git commit -m "<subject>"`. Work happens on the feature branch the execute skill creates.

House rules for this plan:
1. Code touches files outside `$HIVE_HOME`/`~/.hive` and doctor's temp dir only by deleting entries that `gc`'s rules allow, inside Cargo target dirs (ARCH-2).
2. No requirement ID appears in source, comments or test names.
3. Tests use `/private/tmp/hive-it-*` scratch dirs only. The single exception is the disk image of Task 9, under the testing-standard exception.

## File Structure

| Path | Responsibility |
|---|---|
| `src/platform/mod.rs` | `LockKind::FcntlRead`, `HeldLock`, `TryLock`, `EntryKind`, `EntryStat`, `VolumeInfo`, `container_key`, `CHILD_DEADLINE`, `try_lock_once`, and the new `Fs` methods; `probe_lock` removed |
| `src/platform/apfs.rs` | Apfs implementations, `first_line_within`, and the deadline on lock children |
| `src/doctor.rs` | Check (d) matrix, `Capabilities.lock_check`, `LOCK_CHECK_VERSION`, `require(Deletion)`; the `FakeFs` test double gains the new methods |
| `src/cli.rs` | Hidden `__try-lock`, `gc` command, usage text |
| `src/gc.rs` | Module root: `run`, `Scope`, `RunOpts`, `Counts`, `GcReport`, `Skip`, `is_temp_entry`, `MIN_AGE`, caps |
| `src/gc/plan.rs` | Pure session planner and name parser |
| `src/gc/ledger.rs` | Per-inode freed-byte ledger |
| `src/gc/run.rs` | Effectful runner and its recording-fake tests |
| `src/lib.rs` | `pub mod gc;` |
| `tests/common/mod.rs` | `write_ready_capabilities` writes `lock_check: 1`; new helpers `Hive::build_with_sessions`, `Hive::hold_lock_child`, `plant_session`, `crate_dirs`, `sessions_in`, `wait_within` |
| `tests/platform_locks.rs` | Cross-process lock tests for the new primitive and `__try-lock` |
| `tests/doctor.rs` | Check (d) reports the eight keys and passes |
| `tests/gc.rs` | `gc` through the binary on real-cargo fixtures |
| `tests/gc_disk_image.rs` | Freed bytes ≤ measured increase, on a test-created APFS image |
| `README.md` | `gc` row and section; the "reads only" lines corrected |
| `docs/specs/2026-10-25-foundation/{design,requirements}.md` | CORE realign (Decision 16), through `realign-spec` |
| `docs/agents/verify.md` | `gc` drive; check (d) named as an exclusive lock proof |
| `docs/standards/errors-logging.md` | cancel exit code 130 |
| `docs/specs/catalog/hive.md` | catalog row for this feature |

Retrieval:
- `load-subgraph` `blast_radius` on these paths: every `src/` and `tests/` path is OWNED by CORE (`docs/specs/2026-10-25-foundation/tasks.md`), the only other registered feature.
- `cluster(AUTOGC)`: CORE is the sole member, at weight 8 (shared `src/` paths).
- Coverage: `with_owns` 1 of 2 registered (AUTOGC has no tasks yet). No other plan writes these files, and CORE is `Implemented`.

---

### Task 1: The in-process exclusive lock, and a cross-process lock try

**Files:**
- Modify: `src/platform/mod.rs`: `LockKind::FcntlRead`, `CHILD_DEADLINE`, `HeldLock`, `TryLock`, `try_lock_once`, plus `Fs::try_lock_exclusive` and `Fs::try_lock_elsewhere`.
- Modify: `src/platform/apfs.rs`: the implementations, `first_line_within`, the deadline in `hold_lock`, and the `FcntlRead` arm in `hold_lock_forever`.
- Modify: `src/cli.rs`: the hidden `__try-lock`, handled before `State::open`, and the usage text.
- Modify: `src/doctor.rs`: the `FakeFs` test double implements the two new methods.
- Test: `tests/platform_locks.rs`, rewritten.

**Reuse:** 2 — extends `platform::{Fs, LockKind, LockHolder, hold_lock_forever}` and `apfs::Apfs` (`src/platform/mod.rs:95-185`, `apfs.rs:89-261`); 3 — `std::fs::{remove_dir_all, remove_file, read_dir, symlink_metadata}`; 5 — `libc` (`open` with `O_NOFOLLOW|O_NONBLOCK|O_CLOEXEC`, `fstat`, `flock`, `fcntl`, `statfs`, `getattrlist` with `FSOPT_NOFOLLOW`). No new dependency.

**Interfaces:**
- Consumes: `platform::{Fs, LockKind, LockState, LockHolder, hold_lock_forever}` and `apfs::Apfs::new(holder)` (CORE).
- Produces:
  - `platform::CHILD_DEADLINE: Duration` (5 s)
  - `LockKind::{Flock, Fcntl, FcntlRead}`, where `as_arg` gives `"flock"`, `"fcntl"` and `"fcntl-read"`
  - `HeldLock::new(impl FnOnce() + Send + 'static) -> HeldLock`, which releases on drop
  - `TryLock::{Acquired(HeldLock), Held, Gone, Unopenable(io::Error), Unlockable(io::Error)}`
  - `Fs::try_lock_exclusive(&self, &Path) -> TryLock`. This folds the design's `io::Result` wrapper into `Unopenable` and `Unlockable`, so every outcome is one value.
  - `Fs::try_lock_elsewhere(&self, LockKind, &Path) -> io::Result<LockState>`
  - `platform::try_lock_once(LockKind, &Path) -> io::Result<LockState>`
  - the hidden command `cargo-hive __try-lock <flock|fcntl|fcntl-read> <path>`. It prints `acquired` or `refused`, exits 2 for an unknown kind, and exits 1 for an error.

**Depends-on:** none

- [x] **Step 1: Write the failing tests.** Replace `tests/platform_locks.rs` with:

```rust
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
        assert!(matches!(apfs().try_lock_exclusive(&path), TryLock::Held), "{kind:?}");
        assert!(started.elapsed() < Duration::from_secs(1), "{kind:?}: the try waited");
        holder.release();
        assert!(matches!(apfs().try_lock_exclusive(&path), TryLock::Acquired(_)), "{kind:?}");
    }
}

#[test]
fn a_held_guard_refuses_fcntl_tries_from_another_process() {
    let s = Scratch::new("reverse");
    let path = s.path.join("r.lock");
    std::fs::write(&path, b"").unwrap();
    let TryLock::Acquired(guard) = apfs().try_lock_exclusive(&path) else { panic!("could not lock") };
    for kind in [LockKind::FcntlRead, LockKind::Fcntl, LockKind::Flock] {
        assert_eq!(apfs().try_lock_elsewhere(kind, &path).unwrap(), LockState::Held, "{kind:?}");
    }
    drop(guard);
    for kind in [LockKind::FcntlRead, LockKind::Fcntl, LockKind::Flock] {
        assert_eq!(apfs().try_lock_elsewhere(kind, &path).unwrap(), LockState::Free, "{kind:?}");
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
    assert!(matches!(apfs().try_lock_exclusive(&link), TryLock::Unopenable(_)), "a symlink was followed");
    let fifo = s.path.join("fifo.lock");
    assert!(std::process::Command::new("mkfifo").arg(&fifo).status().unwrap().success());
    let started = Instant::now();
    assert!(matches!(apfs().try_lock_exclusive(&fifo), TryLock::Unopenable(_)));
    assert!(started.elapsed() < Duration::from_secs(1), "opening a FIFO blocked");
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
    assert!(started.elapsed() < Duration::from_secs(15), "the deadline was not applied");
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
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let tried = hive.run(&["__try-lock", "fcntl", path.to_str().unwrap()]);
    child.kill().unwrap();
    child.wait().unwrap();
    assert_eq!(line.trim(), "held");
    assert_eq!(String::from_utf8_lossy(&tried.stdout).trim(), "refused");
    assert!(!hive.home.path.exists());
    std::fs::create_dir_all(&hive.home.path).unwrap(); // so the scratch drop has a dir to clean
}
```

In the `tests` module of `src/platform/mod.rs`, add:

```rust
    #[test]
    fn lock_children_get_five_seconds_to_answer() {
        assert_eq!(CHILD_DEADLINE.as_secs(), 5);
    }

    #[test]
    fn lock_kinds_round_trip_their_argument() {
        for kind in [LockKind::Flock, LockKind::Fcntl, LockKind::FcntlRead] {
            assert_eq!(LockKind::parse(kind.as_arg()), Some(kind));
        }
        assert_eq!(LockKind::FcntlRead.as_arg(), "fcntl-read");
        assert_eq!(LockKind::parse("nope"), None);
    }
```

Run `cargo test --test platform_locks` and `cargo test --lib platform`. Expect compile errors: `TryLock`, `try_lock_exclusive`, `try_lock_elsewhere`, `LockKind::FcntlRead` and `CHILD_DEADLINE` do not exist.

- [x] **Step 2: Add the types and trait methods to `src/platform/mod.rs`**

Extend `LockKind` and add the new items above `pub trait Fs`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockKind {
    Flock,
    Fcntl,
    FcntlRead,
}

impl LockKind {
    pub fn as_arg(self) -> &'static str {
        match self {
            LockKind::Flock => "flock",
            LockKind::Fcntl => "fcntl",
            LockKind::FcntlRead => "fcntl-read",
        }
    }

    pub fn parse(arg: &str) -> Option<LockKind> {
        match arg {
            "flock" => Some(LockKind::Flock),
            "fcntl" => Some(LockKind::Fcntl),
            "fcntl-read" => Some(LockKind::FcntlRead),
            _ => None,
        }
    }
}

/// How long hive waits for a lock child (`__hold-lock`, `__try-lock`) to answer.
pub const CHILD_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);

/// A lock this process holds. Dropping it releases the lock.
pub struct HeldLock {
    release: Option<Box<dyn FnOnce() + Send>>,
}

impl HeldLock {
    pub fn new(release: impl FnOnce() + Send + 'static) -> HeldLock {
        HeldLock { release: Some(Box::new(release)) }
    }
}

impl Drop for HeldLock {
    fn drop(&mut self) {
        if let Some(f) = self.release.take() {
            f();
        }
    }
}

impl std::fmt::Debug for HeldLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HeldLock")
    }
}

/// The outcome of one non-blocking exclusive try on a lock file.
#[derive(Debug)]
pub enum TryLock {
    /// The lock is ours until the guard is dropped.
    Acquired(HeldLock),
    /// Another process holds a lock on the file.
    Held,
    /// The file does not exist.
    Gone,
    /// The file could not be opened, or is not a regular file.
    Unopenable(io::Error),
    /// The file opened, but `flock` failed for a reason other than another holder.
    Unlockable(io::Error),
}

/// Body of the hidden `__try-lock` command: try `kind` on `path` once without waiting.
pub fn try_lock_once(kind: LockKind, path: &Path) -> io::Result<LockState> {
    #[cfg(target_os = "macos")]
    {
        apfs::try_lock_once(kind, path)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (kind, path);
        Err(io::Error::new(io::ErrorKind::Unsupported, MACOS_ONLY_MESSAGE))
    }
}
```

Add these two methods to `trait Fs`. Leave `probe_lock` in place; Task 2 removes it.

```rust
    /// `flock(LOCK_EX | LOCK_NB)` on `path`, opened read-only with `O_NOFOLLOW | O_NONBLOCK`
    /// and never created; the lock lasts until the returned guard is dropped.
    fn try_lock_exclusive(&self, path: &Path) -> TryLock;
    /// Has another process try `kind` on `path` once, without waiting.
    fn try_lock_elsewhere(&self, kind: LockKind, path: &Path) -> io::Result<LockState>;
```

- [x] **Step 3: Implement them in `src/platform/apfs.rs`**

Imports:
- Add `HeldLock`, `TryLock` and `CHILD_DEADLINE` to the `use super::{…}` line.
- Add `use std::os::unix::io::FromRawFd;`, `use std::process::Child;` and `use std::time::Duration;`.

Then add these free functions:

```rust
/// The first stdout line of `child`, or an error once `limit` passes. The caller kills the child.
fn first_line_within(child: &mut Child, limit: Duration) -> io::Result<String> {
    let out = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("the lock child has no stdout"))?;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = tx.send(BufReader::new(out).read_line(&mut line).map(|_| line));
    });
    match rx.recv_timeout(limit) {
        Ok(read) => read,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            format!("the lock child did not answer within {limit:?}"),
        )),
    }
}

/// Body of `__try-lock`: one non-blocking attempt, released when this returns.
pub fn try_lock_once(kind: LockKind, path: &Path) -> io::Result<LockState> {
    let file = fs::OpenOptions::new().read(true).write(true).truncate(false).open(path)?;
    let fd = file.as_raw_fd();
    let rc = match kind {
        // SAFETY: `fd` is valid while `file` lives.
        LockKind::Flock => unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) },
        LockKind::Fcntl | LockKind::FcntlRead => {
            // SAFETY: an all-zero `flock` struct is valid; the fields that matter are set next.
            let mut lock: libc::flock = unsafe { std::mem::zeroed() };
            lock.l_type = (if kind == LockKind::FcntlRead { libc::F_RDLCK } else { libc::F_WRLCK }) as libc::c_short;
            lock.l_whence = libc::SEEK_SET as libc::c_short;
            // SAFETY: `lock` outlives the call and `fd` is valid.
            unsafe { libc::fcntl(fd, libc::F_SETLK, &lock) }
        }
    };
    if rc == 0 {
        return Ok(LockState::Free);
    }
    let err = io::Error::last_os_error();
    match err.raw_os_error() {
        Some(libc::EWOULDBLOCK) | Some(libc::EACCES) => Ok(LockState::Held),
        _ => Err(err),
    }
}
```

On macOS `EAGAIN` equals `EWOULDBLOCK`, so one arm covers both `flock` and `F_SETLK`. POSIX also allows `EACCES` for `F_SETLK`.

In `hold_lock_forever`, replace the `LockKind::Fcntl` arm with this arm, which covers both fcntl kinds:

```rust
        LockKind::Fcntl | LockKind::FcntlRead => {
            // SAFETY: an all-zero `flock` struct is valid; the fields that matter are set next.
            let mut lock: libc::flock = unsafe { std::mem::zeroed() };
            lock.l_type = (if kind == LockKind::FcntlRead { libc::F_RDLCK } else { libc::F_WRLCK }) as libc::c_short;
            lock.l_whence = libc::SEEK_SET as libc::c_short;
            // SAFETY: `lock` outlives the call and `fd` is valid.
            unsafe { libc::fcntl(fd, libc::F_SETLKW, &lock) }
        }
```

In `hold_lock`, replace the `read_line` block with a call bounded by the deadline:

```rust
        let line = first_line_within(&mut child, CHILD_DEADLINE);
        if !matches!(&line, Ok(l) if l.trim() == "held") {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other(format!("the lock holder did not take the lock ({line:?})")));
        }
```

Add to `impl Fs for Apfs`:

```rust
    fn try_lock_exclusive(&self, path: &Path) -> TryLock {
        let p = match c_path(path) {
            Ok(p) => p,
            Err(e) => return TryLock::Unopenable(e),
        };
        // SAFETY: `p` is a valid NUL-terminated path for the duration of the call.
        let fd = unsafe {
            libc::open(p.as_ptr(), libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        };
        if fd < 0 {
            let err = io::Error::last_os_error();
            return if err.kind() == io::ErrorKind::NotFound { TryLock::Gone } else { TryLock::Unopenable(err) };
        }
        // SAFETY: `fd` was just opened by this process and nothing else owns it.
        let file = unsafe { fs::File::from_raw_fd(fd) };
        match file.metadata() {
            Ok(m) if m.file_type().is_file() => {}
            Ok(_) => return TryLock::Unopenable(io::Error::other("the lock path is not a regular file")),
            Err(e) => return TryLock::Unopenable(e),
        }
        // SAFETY: `file` owns a valid descriptor for the call.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            let err = io::Error::last_os_error();
            return if err.raw_os_error() == Some(libc::EWOULDBLOCK) { TryLock::Held } else { TryLock::Unlockable(err) };
        }
        TryLock::Acquired(HeldLock::new(move || drop(file)))
    }

    fn try_lock_elsewhere(&self, kind: LockKind, path: &Path) -> io::Result<LockState> {
        let mut child = Command::new(&self.holder)
            .arg("__try-lock")
            .arg(kind.as_arg())
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let line = first_line_within(&mut child, CHILD_DEADLINE);
        let _ = child.kill();
        let _ = child.wait();
        match line?.trim() {
            "acquired" => Ok(LockState::Free),
            "refused" => Ok(LockState::Held),
            other => Err(io::Error::other(format!("the lock child answered {other:?}"))),
        }
    }
```

In the `tests` module of `src/doctor.rs`, add to `impl Fs for FakeFs`. Task 2 replaces these with per-kind knobs.

```rust
        fn try_lock_exclusive(&self, _path: &Path) -> TryLock {
            if self.knobs.probe_sees_locks && self.held.load(Ordering::SeqCst) {
                TryLock::Held
            } else {
                TryLock::Acquired(HeldLock::new(|| {}))
            }
        }
        fn try_lock_elsewhere(&self, _kind: LockKind, _path: &Path) -> io::Result<LockState> {
            Ok(LockState::Held)
        }
```

Add `HeldLock` and `TryLock` to that test module's `use crate::platform::{…}` line.

- [x] **Step 4: Add the hidden `__try-lock` command to `src/cli.rs`**

Add the variant beside `HoldLock`:

```rust
    /// Try a lock once without waiting and print `acquired` or `refused` (doctor's lock check)
    #[command(name = "__try-lock", hide = true)]
    TryLock {
        kind: String,
        path: std::path::PathBuf,
    },
```

In `main_with`, right after the `HoldLock` early return, add:

```rust
    if let Command::TryLock { kind, path } = &cli.command {
        return try_lock(kind, path);
    }
```

Then:
- Add the arm `Command::TryLock { .. } => unreachable!("handled before the state root is opened"),` to the dispatch `match`.
- Change the `hold_lock` usage text to `"(expected flock, fcntl or fcntl-read)"`.
- Add this function:

```rust
fn try_lock(kind: &str, path: &std::path::Path) -> i32 {
    let Some(kind) = platform::LockKind::parse(kind) else {
        eprintln!("cargo-hive: unknown lock kind {kind:?} (expected flock, fcntl or fcntl-read)");
        return 2;
    };
    match platform::try_lock_once(kind, path) {
        Ok(platform::LockState::Free) => {
            println!("acquired");
            0
        }
        Ok(platform::LockState::Held) => {
            println!("refused");
            0
        }
        Err(e) => {
            eprintln!("cargo-hive: could not try the lock on {}: {e}", path.display());
            1
        }
    }
}
```

Run `cargo test --test platform_locks` and `cargo test --lib`. Expect 6 integration tests passing and every unit test passing. Then run the merge gate.

- [x] **Step 5: Commit**

Run `cargo fmt`, then `git add -A && git commit -m "feat(platform): exclusive in-process lock try and a cross-process lock probe"`.

_Requirements: AUTOGC-1.1, AUTOGC-1.2_

---

### Task 2: Doctor check (d) proves the deletion lock, and Deletion requires that proof

**Files:**
- Modify: `src/doctor.rs`:
  - `check_lock_probe`, which becomes the eight-key matrix;
  - `Capabilities.lock_check`, `LOCK_CHECK_VERSION`, `capabilities_for` and `require`;
  - in the test module, `FakeFs` gains the `reverse_refused` knob, and new unit tests are added.
- Modify: `src/platform/mod.rs` and `src/platform/apfs.rs`: remove `Fs::probe_lock`.
- Modify: `tests/common/mod.rs`: `write_ready_capabilities` writes `"lock_check": 1`.
- Test: `src/doctor.rs` (`#[cfg(test)]`) and `tests/doctor.rs`.

**Reuse:** 2 — extends `doctor::{check_lock_probe, Capabilities, capabilities_for, require}` and `FakeFs` (`doctor.rs:115-121, 254-281, 514-542, ~777`)

**Interfaces:**
- Consumes: `Fs::{hold_lock, try_lock_exclusive, try_lock_elsewhere}` and `TryLock` (Task 1).
- Produces:
  - `doctor::LOCK_CHECK_VERSION: u32` (1);
  - `Capabilities { toolchain_release, disabled, lock_check: u32 }`, with `lock_check` under `#[serde(default)]`;
  - `require(&caps, Capability::Deletion, release)` returns `Err(CapabilityRefusal::RunDoctor(_))` while `caps.lock_check < LOCK_CHECK_VERSION`;
  - check (d) measured keys, all `Bool`: `flock_holder_refuses_try`, `flock_holder_free_after_exit`, `fcntl_read_holder_refuses_try`, `fcntl_read_holder_free_after_exit`, `fcntl_write_holder_refuses_try`, `fcntl_write_holder_free_after_exit`, `held_flock_refuses_fcntl_read`, `held_flock_refuses_fcntl_write`.

**Depends-on:** Task 1

- [x] **Step 1: Write the failing tests**

In the `tests` module of `src/doctor.rs`:
1. Add two knobs to `Knobs`:
   - `reverse_refused: bool`, defaulting to `true`;
   - `blind_kind: Option<LockKind>`, defaulting to `None`. This is a holder kind whose lock the try does not see.

   Add `held_kind: Arc<Mutex<Option<LockKind>>>` to `FakeFs` (initialised to `None` in `FakeFs::new`). `hold_lock` stores its `kind` there and its release clears it.
2. Replace the two `FakeFs` lock methods from Task 1 with:

```rust
        fn try_lock_exclusive(&self, _path: &Path) -> TryLock {
            let held_kind = *self.held_kind.lock().unwrap();
            let seen = self.knobs.probe_sees_locks && held_kind.is_some() && held_kind != self.knobs.blind_kind;
            if seen {
                TryLock::Held
            } else {
                TryLock::Acquired(HeldLock::new(|| {}))
            }
        }
        fn try_lock_elsewhere(&self, _kind: LockKind, _path: &Path) -> io::Result<LockState> {
            Ok(if self.knobs.reverse_refused { LockState::Held } else { LockState::Free })
        }
```

3. Delete `FakeFs::probe_lock`.
4. Add `lock_check: 0,` to the two `Capabilities { … }` literals in `require_refuses_without_results_for_another_toolchain_and_for_a_disabled_capability`.
5. Replace `FakeFs::hold_lock` with a version that records the kind:

```rust
        fn hold_lock(&self, kind: LockKind, _path: &Path) -> io::Result<LockHolder> {
            *self.held_kind.lock().unwrap() = Some(kind);
            let held = Arc::clone(&self.held_kind);
            Ok(LockHolder::new(move || *held.lock().unwrap() = None))
        }
```

   Delete the now-unused `held: Arc<AtomicBool>` field and its initialiser in `FakeFs::new`, which becomes `held_kind: Arc::new(Mutex::new(None))`. Remove `AtomicBool` from the test module's `use std::sync::atomic::{…}` line if nothing else uses it, because clippy denies unused imports.
6. Add these rows to the table in `each_failing_check_disables_exactly_its_capabilities`, after "probe misses held locks":

```rust
            (
                "an fcntl read lock held elsewhere goes unseen",
                Knobs {
                    blind_kind: Some(LockKind::FcntlRead),
                    ..Knobs::default()
                },
                CheckId::D,
                vec![Deletion],
            ),
            (
                "another process can lock while hive holds the lock",
                Knobs {
                    reverse_refused: false,
                    ..Knobs::default()
                },
                CheckId::D,
                vec![Deletion],
            ),
```

Then add:

```rust
    #[test]
    fn check_d_reports_the_full_lock_matrix() {
        let _serial = SERIAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let parent = scratch("matrix");
        let report = run(&FakeFs::new(Knobs::default()), &FakeApprove(Availability::Available), &env(&parent, "27.0.1")).unwrap();
        let d = report.checks.iter().find(|c| c.id == CheckId::D).unwrap();
        let mut keys: Vec<&str> = d.measured.keys().map(String::as_str).collect();
        keys.sort();
        let mut want = vec![
            "fcntl_read_holder_free_after_exit",
            "fcntl_read_holder_refuses_try",
            "fcntl_write_holder_free_after_exit",
            "fcntl_write_holder_refuses_try",
            "flock_holder_free_after_exit",
            "flock_holder_refuses_try",
            "held_flock_refuses_fcntl_read",
            "held_flock_refuses_fcntl_write",
        ];
        want.sort();
        assert_eq!(keys, want);
        assert_eq!(d.verdict, Verdict::Pass, "{:?}", d.measured);
        std::fs::remove_dir_all(parent).unwrap();
    }

    #[test]
    fn deletion_needs_results_from_the_current_lock_check() {
        let old = Capabilities { toolchain_release: Some("1.97.1".into()), disabled: BTreeMap::new(), lock_check: 0 };
        let e = require(&old, Capability::Deletion, Some("1.97.1")).unwrap_err();
        assert!(matches!(e, CapabilityRefusal::RunDoctor(_)), "{e}");
        assert!(e.to_string().contains("cargo hive doctor"), "{e}");
        assert_eq!(require(&old, Capability::FingerprintReading, Some("1.97.1")), Ok(()));
        let current = Capabilities { lock_check: LOCK_CHECK_VERSION, ..old };
        assert_eq!(require(&current, Capability::Deletion, Some("1.97.1")), Ok(()));
    }

    #[test]
    fn capabilities_recorded_before_the_lock_check_still_parse() {
        let caps: Capabilities = serde_json::from_str(r#"{"toolchain_release":"1.97.1","disabled":{}}"#).unwrap();
        assert_eq!(caps.lock_check, 0);
        assert_eq!(LOCK_CHECK_VERSION, 1);
    }
```

In `tests/doctor.rs`, add:

```rust
#[test]
fn check_d_reports_the_lock_matrix_and_passes_on_this_mac() {
    let hive = Hive::new();
    let (_, receipt) = doctor(&hive);
    let d = receipt["checks"].as_array().unwrap().iter().find(|c| c["id"] == "d").unwrap().clone();
    assert_eq!(d["measured"].as_object().unwrap().len(), 8, "{d}");
    for (key, value) in d["measured"].as_object().unwrap() {
        assert_eq!(value, &serde_json::json!(true), "{key}: {d}");
    }
    assert_eq!(d["verdict"], "pass", "{d}");
    let caps: serde_json::Value =
        serde_json::from_slice(&std::fs::read(hive.home.path.join("capabilities.json")).unwrap()).unwrap();
    assert_eq!(caps["lock_check"], 1);
}
```

Run `cargo test --lib doctor` and `cargo test --test doctor`. Expect compile errors (`reverse_refused`, `lock_check` and `LOCK_CHECK_VERSION` do not exist) and failures from the old measured keys.

- [x] **Step 2: Implement in `src/doctor.rs`**

Add `pub const LOCK_CHECK_VERSION: u32 = 1;`, then add this field to `Capabilities`:

```rust
    /// Version of check (d)'s lock proof these results come from. 0 means the results were
    /// recorded before the exclusive-lock matrix existed.
    #[serde(default)]
    pub lock_check: u32,
```

In `capabilities_for`, add `lock_check: LOCK_CHECK_VERSION,` to the struct literal. In `require`, right after the `let Some(recorded) = …` block, add:

```rust
    if cap == Capability::Deletion && caps.lock_check < LOCK_CHECK_VERSION {
        return Err(CapabilityRefusal::RunDoctor(
            "no doctor results from the current lock check are recorded for this toolchain".into(),
        ));
    }
```

Replace `check_lock_probe` with:

```rust
/// (d) The lock gc deletes under is refused while another process holds any lock kind rustc
/// or Cargo use; while hive holds it, another process's fcntl and flock tries are refused.
fn check_lock_probe(fs: &dyn Fs, dir: &Path) -> Check {
    let attempt = || -> io::Result<Check> {
        let mut measured = Measures::new();
        let mut ok = true;
        for (kind, name) in [
            (LockKind::Flock, "flock"),
            (LockKind::FcntlRead, "fcntl_read"),
            (LockKind::Fcntl, "fcntl_write"),
        ] {
            let path = dir.join(format!("d-{name}.lock"));
            fs.write_file(&path, b"")?;
            let holder: LockHolder = fs.hold_lock(kind, &path)?;
            let refused = matches!(fs.try_lock_exclusive(&path), TryLock::Held);
            holder.release();
            let free = matches!(fs.try_lock_exclusive(&path), TryLock::Acquired(_));
            measured.insert(format!("{name}_holder_refuses_try"), Measured::Bool(refused));
            measured.insert(format!("{name}_holder_free_after_exit"), Measured::Bool(free));
            ok &= refused && free;
        }
        let path = dir.join("d-reverse.lock");
        fs.write_file(&path, b"")?;
        let guard = match fs.try_lock_exclusive(&path) {
            TryLock::Acquired(guard) => guard,
            other => {
                return Err(io::Error::other(format!(
                    "could not take the lock to test the reverse direction: {other:?}"
                )))
            }
        };
        for (kind, name) in [(LockKind::FcntlRead, "fcntl_read"), (LockKind::Fcntl, "fcntl_write")] {
            let refused = fs.try_lock_elsewhere(kind, &path)? == LockState::Held;
            measured.insert(format!("held_flock_refuses_{name}"), Measured::Bool(refused));
            ok &= refused;
        }
        drop(guard);
        Ok(Check { id: CheckId::D, verdict: verdict(ok), measured })
    };
    attempt().unwrap_or_else(|e| failed(CheckId::D, e))
}
```

Add `TryLock` to `doctor.rs`'s `use crate::platform::{…}` line.

- [x] **Step 3: Remove `probe_lock`, and update the test helper**

Delete `fn probe_lock` from `trait Fs` (`src/platform/mod.rs`) and from `impl Fs for Apfs` (`src/platform/apfs.rs`). In `tests/common/mod.rs`, `write_ready_capabilities` writes:

```rust
        let caps = serde_json::json!({ "toolchain_release": release, "disabled": {}, "lock_check": 1 });
```

Run `cargo test --lib`, `cargo test --test doctor` and `cargo test --test capability_gate`. Expect every test passing, including the three new unit tests, the new table row and the new doctor integration test. Then run the merge gate.

- [x] **Step 4: Commit**

Run `cargo fmt`, then `git add -A && git commit -m "feat(doctor): check (d) proves the exclusive lock both ways; deletion requires that proof"`.

_Requirements: AUTOGC-1.1, AUTOGC-1.2, AUTOGC-1.3, AUTOGC-7.4_

---

### Task 3: No-follow stat, bounded listing, removal and volume info through `Fs`

**Files:**
- Modify: `src/platform/mod.rs`:
  - `EntryKind`, `EntryStat`, `VolumeInfo` and `container_key`;
  - the trait methods `entry_stat`, `read_dir_bounded`, `remove_tree`, `remove_file` and `volume`.
- Modify: `src/platform/apfs.rs`: the implementations; `sharing` gains `FSOPT_NOFOLLOW`.
- Modify: `src/doctor.rs`: the `FakeFs` test double implements the five new methods.
- Test: `src/platform/mod.rs` and `src/platform/apfs.rs` (`#[cfg(test)]`).

**Reuse:** 2 — extends `platform::{Fs, LockKind, LockHolder, hold_lock_forever}` and `apfs::Apfs` (`src/platform/mod.rs:95-185`, `apfs.rs:89-261`); 3 — `std::fs::{remove_dir_all, remove_file, read_dir, symlink_metadata}`; 5 — `libc` (`open` with `O_NOFOLLOW|O_NONBLOCK|O_CLOEXEC`, `fstat`, `flock`, `fcntl`, `statfs`, `getattrlist` with `FSOPT_NOFOLLOW`). No new dependency.

**Interfaces:**
- Consumes: `platform::Fs` and `apfs::Apfs` (Task 1).
- Produces:
  - `EntryKind::{File, Dir, Symlink, Other}`
  - `EntryStat { dev: u64, ino: u64, nlink: u64, blocks: u64, kind: EntryKind, compressed: bool }`
  - `VolumeInfo { fs_type: String, container: String, mount: PathBuf, available_bytes: u64 }`
  - `platform::container_key(mntfromname: &str) -> Option<String>`
  - `Fs::entry_stat(&self, &Path) -> io::Result<EntryStat>`, using `lstat`
  - `Fs::read_dir_bounded(&self, &Path, cap: usize) -> io::Result<Vec<(String, EntryKind)>>`. It errors past `cap` entries, and skips non-UTF-8 names while still counting them.
  - `Fs::remove_tree(&self, &Path) -> io::Result<()>`
  - `Fs::remove_file(&self, &Path) -> io::Result<()>`
  - `Fs::volume(&self, &Path) -> io::Result<VolumeInfo>`, where `available_bytes = f_bavail × f_bsize`

**Depends-on:** Task 2

- [x] **Step 1: Write the failing tests**

In the `tests` module of `src/platform/mod.rs`:

```rust
    #[test]
    fn the_container_is_the_leading_bsd_disk() {
        let cases = [
            ("/dev/disk3s1s1", Some("disk3")),
            ("/dev/disk3s5", Some("disk3")),
            ("/dev/disk12s2", Some("disk12")),
            ("com.apple.TimeMachine.2026-10-28-120000.backup@/dev/disk3s5", Some("disk3")),
            ("map auto_home", None),
            ("//user@host/share", None),
            ("/dev/diskX", None),
        ];
        for (from, want) in cases {
            assert_eq!(container_key(from).as_deref(), want, "{from}");
        }
    }
```

In the `tests` module of `src/platform/apfs.rs`:

```rust
    use crate::platform::EntryKind;

    #[test]
    fn entry_stat_never_follows_a_link_and_sees_links_and_compression() {
        let t = tmp("stat");
        let (a, b, link) = (t.path().join("a"), t.path().join("b"), t.path().join("link"));
        apfs().write_file(&a, &vec![3u8; 8192]).unwrap();
        let one = apfs().entry_stat(&a).unwrap();
        assert_eq!((one.kind, one.nlink, one.compressed), (EntryKind::File, 1, false));
        apfs().hardlink(&a, &b).unwrap();
        let two = apfs().entry_stat(&b).unwrap();
        assert_eq!((two.ino, two.nlink), (one.ino, 2));
        std::os::unix::fs::symlink(&a, &link).unwrap();
        assert_eq!(apfs().entry_stat(&link).unwrap().kind, EntryKind::Symlink);
        assert_eq!(apfs().entry_stat(t.path()).unwrap().kind, EntryKind::Dir);
        let packed = t.path().join("packed");
        let content: Vec<u8> = (0..4 << 20).map(|i| (i % 251) as u8).collect();
        apfs().write_file(&packed, &content).unwrap();
        assert_eq!(apfs().compress(&packed), CompressOutcome::Compressed);
        assert!(apfs().entry_stat(&packed).unwrap().compressed);
    }

    #[test]
    fn a_bounded_listing_fails_one_entry_past_its_cap() {
        let t = tmp("list");
        for name in ["x", "y"] {
            apfs().write_file(&t.path().join(name), b"").unwrap();
        }
        std::os::unix::fs::symlink("x", t.path().join("z")).unwrap();
        let mut listed = apfs().read_dir_bounded(t.path(), 3).unwrap();
        listed.sort();
        assert_eq!(
            listed,
            vec![("x".into(), EntryKind::File), ("y".into(), EntryKind::File), ("z".into(), EntryKind::Symlink)]
        );
        assert!(apfs().read_dir_bounded(t.path(), 2).is_err());
    }

    #[test]
    fn removing_a_tree_never_follows_a_link_out_of_it() {
        let t = tmp("remove");
        let outside = t.path().join("outside");
        apfs().write_file(&outside, b"keep").unwrap();
        let tree = t.path().join("tree");
        std::fs::create_dir_all(tree.join("sub")).unwrap();
        std::os::unix::fs::symlink(&outside, tree.join("sub/link")).unwrap();
        std::os::unix::fs::symlink(t.path(), tree.join("up")).unwrap();
        apfs().remove_tree(&tree).unwrap();
        assert!(!tree.exists());
        assert_eq!(apfs().read(&outside).unwrap(), b"keep");
        let root_link = t.path().join("root-link");
        std::os::unix::fs::symlink(&outside, &root_link).unwrap();
        apfs().remove_tree(&root_link).unwrap();
        assert!(outside.exists() && std::fs::symlink_metadata(&root_link).is_err());
    }

    #[test]
    fn the_scratch_volume_is_apfs_with_a_disk_container() {
        let v = apfs().volume(Path::new("/private/tmp")).unwrap();
        assert_eq!(v.fs_type, "apfs");
        assert!(v.container.starts_with("disk"), "{v:?}");
        assert!(v.available_bytes > 0);
    }

    #[test]
    fn sharing_does_not_follow_a_final_symlink() {
        let t = tmp("share-link");
        let a = t.path().join("a");
        apfs().write_file(&a, &vec![1u8; 1 << 20]).unwrap();
        let link = t.path().join("link");
        std::os::unix::fs::symlink(&a, &link).unwrap();
        // With FSOPT_NOFOLLOW the call describes the link itself (or refuses it), never the 1 MiB target.
        if let Ok(s) = apfs().sharing(&link) {
            assert_ne!(s.private_bytes, 1 << 20);
        }
    }
```

The last test passes once `sharing` stops following the link: the call then describes the link itself, or fails, and never reports the target's 1 MiB. Run `cargo test --lib platform`. Expect compile errors: `container_key`, `entry_stat`, `read_dir_bounded`, `remove_tree`, `volume` and `EntryKind` do not exist.

- [x] **Step 2: Add the types and trait methods to `src/platform/mod.rs`**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    Other,
}

/// `lstat` of one entry: never the target of a symbolic link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EntryStat {
    pub dev: u64,
    pub ino: u64,
    pub nlink: u64,
    pub blocks: u64,
    pub kind: EntryKind,
    /// APFS transparent compression (`UF_COMPRESSED`).
    pub compressed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeInfo {
    pub fs_type: String,
    /// The APFS container (`disk<N>`) the volume belongs to, or `mount:<path>` when unknown.
    pub container: String,
    pub mount: PathBuf,
    pub available_bytes: u64,
}

/// The container of a volume from `statfs.f_mntfromname`: the leading `disk<N>` after any
/// `…@` snapshot prefix. Volumes of one APFS container share free space.
pub fn container_key(mntfromname: &str) -> Option<String> {
    let device = mntfromname.rsplit_once('@').map_or(mntfromname, |(_, d)| d);
    let rest = device.strip_prefix("/dev/disk")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (!digits.is_empty()).then(|| format!("disk{digits}"))
}
```

Add to `trait Fs`:

```rust
    /// `lstat`: never follows a symbolic link.
    fn entry_stat(&self, path: &Path) -> io::Result<EntryStat>;
    /// The UTF-8 names and no-follow kinds of `dir`'s entries; more than `cap` entries is an error.
    fn read_dir_bounded(&self, dir: &Path, cap: usize) -> io::Result<Vec<(String, EntryKind)>>;
    /// Removes a directory tree, or a symbolic link given as the root, never following a link.
    fn remove_tree(&self, path: &Path) -> io::Result<()>;
    fn remove_file(&self, path: &Path) -> io::Result<()>;
    /// `statfs` of the volume holding `path`.
    fn volume(&self, path: &Path) -> io::Result<VolumeInfo>;
```

- [x] **Step 3: Implement them in `src/platform/apfs.rs`, and in the `FakeFs` test double**

Add `EntryKind`, `EntryStat`, `VolumeInfo` and `container_key` to the `use super::{…}` line, plus `use std::ffi::CStr;`. In `sharing`, change the last argument of `getattrlist` to `libc::FSOPT_ATTR_CMN_EXTENDED | libc::FSOPT_NOFOLLOW`. That option affects only the last path component, which doctor needs because its temp dir is under the `/var` symlink. Then add to `impl Fs for Apfs`:

```rust
    fn entry_stat(&self, path: &Path) -> io::Result<EntryStat> {
        let m = fs::symlink_metadata(path)?;
        let t = m.file_type();
        let kind = if t.is_symlink() {
            EntryKind::Symlink
        } else if t.is_dir() {
            EntryKind::Dir
        } else if t.is_file() {
            EntryKind::File
        } else {
            EntryKind::Other
        };
        Ok(EntryStat {
            dev: m.dev(),
            ino: m.ino(),
            nlink: m.nlink(),
            blocks: m.blocks(),
            kind,
            compressed: m.st_flags() & libc::UF_COMPRESSED != 0,
        })
    }

    fn read_dir_bounded(&self, dir: &Path, cap: usize) -> io::Result<Vec<(String, EntryKind)>> {
        let mut out = Vec::new();
        for (seen, entry) in fs::read_dir(dir)?.enumerate() {
            if seen >= cap {
                return Err(io::Error::other(format!("{} has more than {cap} entries", dir.display())));
            }
            let entry = entry?;
            let t = entry.file_type()?;
            let kind = if t.is_symlink() {
                EntryKind::Symlink
            } else if t.is_dir() {
                EntryKind::Dir
            } else if t.is_file() {
                EntryKind::File
            } else {
                EntryKind::Other
            };
            if let Ok(name) = entry.file_name().into_string() {
                out.push((name, kind));
            }
        }
        Ok(out)
    }

    fn remove_tree(&self, path: &Path) -> io::Result<()> {
        // std's remove_dir_all opens every level with O_NOFOLLOW and unlinks a symlink root.
        fs::remove_dir_all(path)
    }

    fn remove_file(&self, path: &Path) -> io::Result<()> {
        fs::remove_file(path)
    }

    fn volume(&self, path: &Path) -> io::Result<VolumeInfo> {
        let p = c_path(path)?;
        // SAFETY: an all-zero `statfs` struct is valid output space.
        let mut s: libc::statfs = unsafe { std::mem::zeroed() };
        // SAFETY: `p` is NUL-terminated and `s` is writable; both outlive the call.
        if unsafe { libc::statfs(p.as_ptr(), &mut s) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the kernel NUL-terminates these fixed-size name fields.
        let text = |field: &[libc::c_char]| unsafe { CStr::from_ptr(field.as_ptr()) }.to_string_lossy().into_owned();
        let (fs_type, from, on) = (text(&s.f_fstypename), text(&s.f_mntfromname), text(&s.f_mntonname));
        Ok(VolumeInfo {
            container: container_key(&from).unwrap_or_else(|| format!("mount:{on}")),
            fs_type,
            mount: PathBuf::from(on),
            available_bytes: s.f_bavail.saturating_mul(u64::from(s.f_bsize)),
        })
    }
```

In the `tests` module of `src/doctor.rs`:
- add `EntryKind`, `EntryStat` and `VolumeInfo` to the `use crate::platform::{…}` line;
- add these methods to `impl Fs for FakeFs`. Doctor never calls them; they keep the trait implementable.

```rust
        fn entry_stat(&self, path: &Path) -> io::Result<EntryStat> {
            let m = std::fs::symlink_metadata(path)?;
            let kind = if m.file_type().is_symlink() {
                EntryKind::Symlink
            } else if m.is_dir() {
                EntryKind::Dir
            } else if m.is_file() {
                EntryKind::File
            } else {
                EntryKind::Other
            };
            Ok(EntryStat { dev: m.dev(), ino: m.ino(), nlink: m.nlink(), blocks: m.blocks(), kind, compressed: false })
        }
        fn read_dir_bounded(&self, dir: &Path, _cap: usize) -> io::Result<Vec<(String, EntryKind)>> {
            std::fs::read_dir(dir)?
                .map(|e| {
                    let e = e?;
                    let kind = if e.file_type()?.is_dir() { EntryKind::Dir } else { EntryKind::File };
                    Ok((e.file_name().to_string_lossy().into_owned(), kind))
                })
                .collect()
        }
        fn remove_tree(&self, path: &Path) -> io::Result<()> {
            std::fs::remove_dir_all(path)
        }
        fn remove_file(&self, path: &Path) -> io::Result<()> {
            std::fs::remove_file(path)
        }
        fn volume(&self, _path: &Path) -> io::Result<VolumeInfo> {
            Ok(VolumeInfo { fs_type: "apfs".into(), container: "disk0".into(), mount: PathBuf::from("/"), available_bytes: 0 })
        }
```

Run `cargo test --lib`. Expect every test passing: the 6 new platform tests, and the CORE `sharing` tests with unchanged results. Then run the merge gate, including `cargo check --target x86_64-unknown-linux-gnu`. `container_key` and the types are portable, and `Apfs` is macOS-only.

- [x] **Step 4: Commit**

Run `cargo fmt`, then `git add -A && git commit -m "feat(platform): no-follow stat, bounded listing, removal and statfs through Fs"`.

_Requirements: AUTOGC-2.13, AUTOGC-4.5, AUTOGC-5.4_

---

### Task 4: The session planner and the temp-entry rule (pure)

**Files:**
- Create: `src/gc.rs`, the module root.
- Create: `src/gc/plan.rs`.
- Modify: `src/lib.rs`, adding `pub mod gc;`.
- Test: `src/gc/plan.rs` (`#[cfg(test)]`).

**Reuse:** none — new code (rung 7). No library models rustc's session naming or an inode ledger. It composes `platform::Fs`, `cargo_target::{TargetDir, Owner}`, `fingerprint::Format` and `size::MAX_DEPTH`.

**Interfaces:**
- Consumes: `platform::EntryKind` (Task 3).
- Produces:
  - `gc::MIN_AGE: Duration` (10 s)
  - `gc::is_temp_entry(&str, EntryKind) -> bool`
  - `gc::plan::Action::{DeleteLocked{dir, lock}, DeleteOrphanLock{lock}, DeleteLockless{dir, lock}}` (crate-private)
  - `gc::plan::crate_dir(&[(String, EntryKind)], SystemTime) -> Vec<Action>` (crate-private)

**Depends-on:** Task 3

- [x] **Step 1: Write the failing tests** at the bottom of the new `src/gc/plan.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// Base-36 lowercase, as rustc writes name timestamps.
    fn b36(mut n: u64) -> String {
        let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let mut out = Vec::new();
        loop {
            out.push(digits[(n % 36) as usize]);
            n /= 36;
            if n == 0 {
                break;
            }
        }
        out.reverse();
        String::from_utf8(out).unwrap()
    }

    const NOW_S: u64 = 1_000_000_000;

    fn now() -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(NOW_S)
    }

    /// Microseconds `secs_ago` before `now()`, minus `minus_us` more.
    fn ts(secs_ago: u64, minus_us: u64) -> u64 {
        (NOW_S - secs_ago) * 1_000_000 - minus_us
    }

    fn session(t: u64, rand: &str, last: &str) -> (String, EntryKind) {
        (format!("s-{}-{rand}-{last}", b36(t)), EntryKind::Dir)
    }

    fn lock(t: u64, rand: &str) -> (String, EntryKind) {
        (format!("s-{}-{rand}.lock", b36(t)), EntryKind::File)
    }

    fn deleted(actions: &[Action]) -> Vec<String> {
        let mut names: Vec<String> = actions
            .iter()
            .map(|a| match a {
                Action::DeleteLocked { dir, .. } | Action::DeleteLockless { dir, .. } => dir.clone(),
                Action::DeleteOrphanLock { lock } => lock.clone(),
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn the_planner_follows_rustcs_session_rules() {
        let old = ts(3600, 0);
        let new = ts(60, 0);
        type Case = (&'static str, Vec<(String, EntryKind)>, Vec<String>);
        let cases: Vec<Case> = vec![
            (
                "the older finalized session goes, the newest stays",
                vec![session(old, "aa", "svh1"), lock(old, "aa"), session(new, "bb", "svh2"), lock(new, "bb")],
                vec![session(old, "aa", "svh1").0],
            ),
            (
                "equal timestamps keep both",
                vec![session(new, "aa", "svh1"), lock(new, "aa"), session(new, "bb", "svh2"), lock(new, "bb")],
                vec![],
            ),
            (
                "a -working session exactly 10 s old goes",
                vec![session(ts(10, 0), "cc", "working"), lock(ts(10, 0), "cc")],
                vec![session(ts(10, 0), "cc", "working").0],
            ),
            (
                "a -working session one microsecond younger than 10 s stays",
                vec![session(ts(10, 0) + 1, "cc", "working"), lock(ts(10, 0) + 1, "cc")],
                vec![],
            ),
            (
                "a -working session from the future stays",
                vec![session(ts(0, 0) + 5_000_000, "dd", "working"), lock(ts(0, 0) + 5_000_000, "dd")],
                vec![],
            ),
            (
                "an orphan lock 10 s old goes, a younger one stays",
                vec![lock(ts(10, 0), "ee"), lock(ts(9, 0), "ff")],
                vec![lock(ts(10, 0), "ee").0],
            ),
            (
                "a lockless session goes even when it is the newest",
                vec![session(old, "aa", "svh1"), lock(old, "aa"), session(new, "bb", "svh2")],
                vec![session(new, "bb", "svh2").0],
            ),
            (
                "malformed and foreign names are never touched",
                vec![
                    ("s-ABC-aa-svh".into(), EntryKind::Dir),
                    ("s-+1-aa-svh".into(), EntryKind::Dir),
                    ("s-zzzzzzzzzzzzzzzzzz-aa-svh".into(), EntryKind::Dir),
                    ("s-1-2".into(), EntryKind::Dir),
                    ("query-cache.bin".into(), EntryKind::File),
                    ("foo.lock".into(), EntryKind::File),
                    (session(old, "aa", "svh1").0, EntryKind::File),
                    (session(old, "gg", "svh1").0, EntryKind::Symlink),
                ],
                vec![],
            ),
        ];
        for (name, entries, want) in cases {
            let mut want = want;
            want.sort();
            assert_eq!(deleted(&crate_dir(&entries, now())), want, "{name}");
        }
    }

    #[test]
    fn locked_and_lockless_actions_name_the_lock_to_use_or_recheck() {
        let old = ts(3600, 0);
        let new = ts(60, 0);
        let entries = vec![session(old, "aa", "svh1"), lock(old, "aa"), session(new, "bb", "svh2"), lock(new, "bb"), session(old, "cc", "svh3")];
        let actions = crate_dir(&entries, now());
        assert!(actions.contains(&Action::DeleteLocked { dir: session(old, "aa", "svh1").0, lock: lock(old, "aa").0 }));
        assert!(actions.contains(&Action::DeleteLockless { dir: session(old, "cc", "svh3").0, lock: lock(old, "cc").0 }));
    }

    #[test]
    fn the_minimum_age_is_ten_seconds() {
        assert_eq!(MIN_AGE, Duration::from_secs(10));
    }

    #[test]
    fn temp_entries_are_rustc_tempfile_dirs_and_nothing_else() {
        let cases = [
            ("rmeta9Ab3x", EntryKind::Dir, true),
            ("rustcAbC123", EntryKind::Dir, true),
            ("rmeta", EntryKind::Dir, false),
            ("rustc_hash-0123456789abcdef.d", EntryKind::File, false),
            ("rustc_hash-0123456789abcdef", EntryKind::Dir, false),
            ("rustcfoo-1.dSYM", EntryKind::Dir, false),
            ("rmetaABC", EntryKind::Symlink, false),
            ("rmeta1", EntryKind::File, false),
        ];
        for (name, kind, want) in cases {
            assert_eq!(is_temp_entry(name, kind), want, "{name} {kind:?}");
        }
    }
}
```

Run `cargo test --lib gc::plan`. Expect a compile error: the module does not exist.

- [x] **Step 2: Implement `src/gc/plan.rs`** (above the tests)

```rust
//! rustc's incremental-session rules for one crate directory, as a pure function of the
//! directory's listing and the current time. They mirror rustc 1.97.1's own session GC
//! (`rustc_incremental/src/persist/fs.rs`). This module decides; it never touches a file.

use crate::platform::EntryKind;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A `-working` session that still has its lock file, or an orphan lock file, must be at
/// least this old by its name. A session directory with no lock file has no age gate.
pub const MIN_AGE: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum Action {
    /// A superseded finalized session, or a `-working` one old enough; delete it under `lock`.
    DeleteLocked { dir: String, lock: String },
    /// A lock file with no session; delete it under itself.
    DeleteOrphanLock { lock: String },
    /// A session directory with no lock file; `lock` is the path to re-check before deleting.
    DeleteLockless { dir: String, lock: String },
}

fn base36_lower(field: &str) -> bool {
    !field.is_empty() && field.bytes().all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
}

/// The name timestamp, in microseconds since the epoch. Anything but lowercase base 36
/// that fits a u64 is malformed.
fn timestamp(field: &str) -> Option<u64> {
    base36_lower(field).then(|| u64::from_str_radix(field, 36).ok()).flatten()
}

/// `s-<ts>-<rand>-<working|svh>` → (key `s-<ts>-<rand>`, timestamp, finalized).
fn parse_session(name: &str) -> Option<(String, u64, bool)> {
    let parts: Vec<&str> = name.split('-').collect();
    let [s, ts, rand, last] = parts.as_slice() else { return None };
    if *s != "s" || !base36_lower(rand) || !base36_lower(last) {
        return None;
    }
    Some((format!("s-{ts}-{rand}"), timestamp(ts)?, *last != "working"))
}

/// `s-<ts>-<rand>.lock` → (key, timestamp).
fn parse_lock(name: &str) -> Option<(String, u64)> {
    let stem = name.strip_suffix(".lock")?;
    let parts: Vec<&str> = stem.split('-').collect();
    let [s, ts, rand] = parts.as_slice() else { return None };
    if *s != "s" || !base36_lower(rand) {
        return None;
    }
    Some((stem.to_string(), timestamp(ts)?))
}

pub(crate) fn crate_dir(entries: &[(String, EntryKind)], now: SystemTime) -> Vec<Action> {
    let now_us = now
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_micros()).unwrap_or(u64::MAX));
    let min_age_us = u64::try_from(MIN_AGE.as_micros()).expect("ten seconds fit in u64");
    let old_enough = |ts: u64| ts.checked_add(min_age_us).is_some_and(|t| t <= now_us);

    let mut sessions: BTreeMap<String, (String, u64, bool)> = BTreeMap::new();
    let mut ambiguous: BTreeSet<String> = BTreeSet::new();
    let mut locks: BTreeMap<String, (String, u64)> = BTreeMap::new();
    for (name, kind) in entries {
        match kind {
            EntryKind::Dir => {
                if let Some((key, ts, finalized)) = parse_session(name) {
                    if sessions.insert(key.clone(), (name.clone(), ts, finalized)).is_some() {
                        ambiguous.insert(key);
                    }
                }
            }
            EntryKind::File => {
                if let Some((key, ts)) = parse_lock(name) {
                    locks.insert(key, (name.clone(), ts));
                }
            }
            EntryKind::Symlink | EntryKind::Other => {}
        }
    }
    let newest = sessions
        .iter()
        .filter(|(key, (_, _, finalized))| *finalized && locks.contains_key(*key) && !ambiguous.contains(*key))
        .map(|(_, (_, ts, _))| *ts)
        .max();

    let mut actions = Vec::new();
    for (key, (dir, ts, finalized)) in &sessions {
        if ambiguous.contains(key) {
            continue;
        }
        match locks.get(key) {
            Some((lock, _)) => {
                let superseded = *finalized && newest.is_some_and(|n| *ts < n);
                let abandoned = !*finalized && old_enough(*ts);
                if superseded || abandoned {
                    actions.push(Action::DeleteLocked { dir: dir.clone(), lock: lock.clone() });
                }
            }
            None => actions.push(Action::DeleteLockless { dir: dir.clone(), lock: format!("{key}.lock") }),
        }
    }
    for (key, (lock, ts)) in &locks {
        if !sessions.contains_key(key) && old_enough(*ts) {
            actions.push(Action::DeleteOrphanLock { lock: lock.clone() });
        }
    }
    actions
}

/// A rustc `tempfile` staging directory in `deps/`: `rustc` or `rmeta` followed only by ASCII
/// letters and digits. Real artifacts always contain `-`, `_` or `.` in their names.
pub fn is_temp_entry(name: &str, kind: EntryKind) -> bool {
    kind == EntryKind::Dir
        && ["rustc", "rmeta"].iter().any(|prefix| {
            name.strip_prefix(prefix)
                .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric()))
        })
}
```

`src/gc.rs`. Task 6 replaces the `allow`, once the runner uses the planner.

```rust
//! `cargo hive gc`: deletes superseded rustc incremental sessions and leftover rustc temp
//! dirs in Cargo target dirs, each under its owner's lock, and counts the bytes freed per inode.

#[cfg_attr(not(test), allow(dead_code))]
mod plan;

pub use plan::{is_temp_entry, MIN_AGE};
```

Add `pub mod gc;` to `src/lib.rs`, in alphabetical order.

Run `cargo test --lib gc::plan`. Expect 4 tests passing. Then run the merge gate.

- [x] **Step 3: Commit**

Run `cargo fmt`, then `git add -A && git commit -m "feat(gc): pure planner for rustc's session rules and the temp-entry rule"`.

_Requirements: AUTOGC-2.1, AUTOGC-2.5, AUTOGC-2.6, AUTOGC-2.7, AUTOGC-2.8, AUTOGC-2.9_

---

### Task 5: The per-inode freed-byte ledger

**Files:**
- Create: `src/gc/ledger.rs`
- Modify: `src/gc.rs`, adding `mod ledger;` and its re-exports.
- Test: `src/gc/ledger.rs` (`#[cfg(test)]`).

**Reuse:** none — new code (rung 7). No library models rustc's session naming or an inode ledger. It composes `platform::Fs`, `cargo_target::{TargetDir, Owner}`, `fingerprint::Format` and `size::MAX_DEPTH`.

**Interfaces:**
- Consumes: `platform::{EntryStat, Sharing}` (Task 3).
- Produces:
  - `gc::UNMEASURED_CAP: usize` (100)
  - `gc::Bytes { freed: u64, still_shared: u64 }`
  - `gc::ledger::Ledger::{new() -> Ledger, file(&mut self, target: &Path, &EntryStat, Option<&Sharing>), unmeasured(&mut self, PathBuf, String), finish(self) -> Tally}` (crate-private)
  - `gc::ledger::Tally { per_target: BTreeMap<PathBuf, Bytes>, total: Bytes, unmeasured: Vec<(PathBuf, String)>, unmeasured_more: u64 }` (crate-private)

The rules (design Decision 8):
- An inode is freed iff the links seen in deleted paths are at least its `nlink` *at first sight*.
- A freed inode counts its private size when it has a sharing flag and is not compressed, and `st_blocks × 512` otherwise.
- Every other byte of a deleted inode is still shared.
- A freed inode is attributed to the target holding its last-seen link.

**Depends-on:** Task 4

- [x] **Step 1: Write the failing tests** at the bottom of the new `src/gc/ledger.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::EntryKind;

    fn stat(ino: u64, nlink: u64, blocks: u64, compressed: bool) -> EntryStat {
        EntryStat { dev: 1, ino, nlink, blocks, kind: EntryKind::File, compressed }
    }

    fn plain() -> Sharing {
        Sharing { private_bytes: 0, clone_id: 0, may_share: false, shares_all: false }
    }

    fn clone_flagged(private_bytes: u64) -> Sharing {
        Sharing { private_bytes, clone_id: 7, may_share: true, shares_all: false }
    }

    #[test]
    fn freed_bytes_follow_links_and_sharing() {
        let a = Path::new("/t/a");
        type Case = (&'static str, Vec<(EntryStat, Sharing)>, Bytes);
        let cases: Vec<Case> = vec![
            ("one link, deleted", vec![(stat(1, 1, 8, false), plain())], Bytes { freed: 4096, still_shared: 0 }),
            ("a link survives elsewhere", vec![(stat(2, 2, 8, false), plain())], Bytes { freed: 0, still_shared: 4096 }),
            (
                "both links deleted, real order (nlink drops in between)",
                vec![(stat(3, 2, 8, false), plain()), (stat(3, 1, 8, false), plain())],
                Bytes { freed: 4096, still_shared: 0 },
            ),
            (
                "both links deleted, dry-run order (nlink unchanged)",
                vec![(stat(4, 2, 8, false), plain()), (stat(4, 2, 8, false), plain())],
                Bytes { freed: 4096, still_shared: 0 },
            ),
            ("a clone keeps the shared extent", vec![(stat(5, 1, 8, false), clone_flagged(1024))], Bytes { freed: 1024, still_shared: 3072 }),
            ("compressed: private size is not trusted", vec![(stat(6, 1, 8, true), clone_flagged(0))], Bytes { freed: 4096, still_shared: 0 }),
        ];
        for (name, files, want) in cases {
            let mut ledger = Ledger::new();
            for (s, sh) in &files {
                ledger.file(a, s, Some(sh));
            }
            assert_eq!(ledger.finish().total, want, "{name}");
        }
    }

    #[test]
    fn a_freed_inode_is_attributed_to_the_target_of_its_last_link() {
        let (a, b) = (Path::new("/t/a"), Path::new("/t/b"));
        let mut ledger = Ledger::new();
        ledger.file(a, &stat(9, 2, 8, false), Some(&plain()));
        ledger.file(b, &stat(9, 1, 8, false), Some(&plain()));
        ledger.file(a, &stat(10, 1, 16, false), Some(&plain()));
        let tally = ledger.finish();
        assert_eq!(tally.per_target[b], Bytes { freed: 4096, still_shared: 0 });
        assert_eq!(tally.per_target[a], Bytes { freed: 8192, still_shared: 0 });
        let sum = tally.per_target.values().fold(Bytes::default(), |s, x| Bytes { freed: s.freed + x.freed, still_shared: s.still_shared + x.still_shared });
        assert_eq!(sum, tally.total);
    }

    #[test]
    fn unmeasured_paths_are_capped_and_the_rest_counted() {
        assert_eq!(UNMEASURED_CAP, 100);
        for (given, listed, more) in [(UNMEASURED_CAP, UNMEASURED_CAP, 0), (UNMEASURED_CAP + 1, UNMEASURED_CAP, 1)] {
            let mut ledger = Ledger::new();
            for i in 0..given {
                ledger.unmeasured(PathBuf::from(format!("/t/{i}")), "unreadable".into());
            }
            let tally = ledger.finish();
            assert_eq!((tally.unmeasured.len(), tally.unmeasured_more), (listed, more as u64), "{given}");
        }
    }
}
```

Run `cargo test --lib gc::ledger`. Expect a compile error: the module does not exist.

- [x] **Step 2: Implement `src/gc/ledger.rs`** (above the tests)

```rust
//! Per-inode accounting of the bytes a gc run frees. Bytes a surviving hard link or clone
//! still holds are counted apart, never as freed.

use crate::platform::{EntryStat, Sharing};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// At most this many unmeasured paths are listed in a receipt; the rest are only counted.
pub const UNMEASURED_CAP: usize = 100;

/// Bytes freed and bytes deleted but still held elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Bytes {
    pub freed: u64,
    pub still_shared: u64,
}

struct Seen {
    first_nlink: u64,
    links: u64,
    blocks: u64,
    /// Private size, kept only for a sharing-flagged, uncompressed file.
    private: Option<u64>,
    target: PathBuf,
}

pub(crate) struct Ledger {
    inodes: HashMap<(u64, u64), Seen>,
    unmeasured: Vec<(PathBuf, String)>,
    unmeasured_more: u64,
}

pub(crate) struct Tally {
    pub per_target: BTreeMap<PathBuf, Bytes>,
    pub total: Bytes,
    pub unmeasured: Vec<(PathBuf, String)>,
    pub unmeasured_more: u64,
}

impl Ledger {
    pub(crate) fn new() -> Ledger {
        Ledger { inodes: HashMap::new(), unmeasured: Vec::new(), unmeasured_more: 0 }
    }

    /// One regular file, inside `target`, that this run deletes (or a dry run would).
    pub(crate) fn file(&mut self, target: &Path, stat: &EntryStat, sharing: Option<&Sharing>) {
        let seen = self.inodes.entry((stat.dev, stat.ino)).or_insert_with(|| Seen {
            first_nlink: stat.nlink,
            links: 0,
            blocks: stat.blocks,
            private: None,
            target: target.to_path_buf(),
        });
        seen.links += 1;
        seen.target = target.to_path_buf();
        if let Some(s) = sharing {
            if (s.may_share || s.shares_all) && !stat.compressed {
                seen.private = Some(s.private_bytes);
            }
        }
    }

    pub(crate) fn unmeasured(&mut self, path: PathBuf, why: String) {
        if self.unmeasured.len() < UNMEASURED_CAP {
            self.unmeasured.push((path, why));
        } else {
            self.unmeasured_more += 1;
        }
    }

    pub(crate) fn finish(self) -> Tally {
        let mut per_target: BTreeMap<PathBuf, Bytes> = BTreeMap::new();
        let mut total = Bytes::default();
        for seen in self.inodes.into_values() {
            let all = seen.blocks.saturating_mul(512);
            let freed = if seen.links >= seen.first_nlink { seen.private.map_or(all, |p| p.min(all)) } else { 0 };
            let entry = per_target.entry(seen.target).or_default();
            entry.freed += freed;
            entry.still_shared += all - freed;
            total.freed += freed;
            total.still_shared += all - freed;
        }
        Tally { per_target, total, unmeasured: self.unmeasured, unmeasured_more: self.unmeasured_more }
    }
}
```

In `src/gc.rs`, add `#[cfg_attr(not(test), allow(dead_code))] mod ledger;` and `pub use ledger::{Bytes, UNMEASURED_CAP};`.

Run `cargo test --lib gc::ledger`. Expect 3 tests passing. Then run the merge gate.

- [x] **Step 3: Commit**

Run `cargo fmt`, then `git add -A && git commit -m "feat(gc): per-inode ledger that never counts surviving links or clones as freed"`.

_Requirements: AUTOGC-5.1, AUTOGC-5.2, AUTOGC-5.5_

---

### Task 6: The runner: every deletion under its owner's lock, re-checked, through `Fs`

**Files:**
- Create: `src/gc/run.rs`
- Modify: `src/gc.rs`, adding `mod run;`, re-exporting the runner API, and removing the two `dead_code` allows.
- Test: `src/gc/run.rs` (`#[cfg(test)]`, with a recording `Fs` fake).

**Reuse:** none — new code (rung 7). No library models rustc's session naming or an inode ledger. It composes `platform::Fs`, `cargo_target::{TargetDir, Owner}`, `fingerprint::Format` and `size::MAX_DEPTH`.

**Interfaces:**
- Consumes:
  - `gc::plan::{crate_dir, Action}` and `gc::is_temp_entry` (Task 4);
  - `gc::ledger::{Ledger, Tally}` and `gc::Bytes` (Task 5);
  - `platform::{Fs, TryLock, HeldLock, EntryKind, EntryStat, Sharing}` (Tasks 1 and 3);
  - `cargo_target::{TargetDir, Owner}`, `fingerprint::{Format, ProfileReport}` and `size::MAX_DEPTH` (CORE).
- Produces:
  - `gc::{CRATE_DIR_CAP (100_000), DEPS_CAP (1_000_000)}`
  - `gc::Scope::{All, Checkouts(Vec<PathBuf>)}`
  - `gc::RunOpts { dry_run: bool, now: SystemTime, account_bytes: bool }`
  - `gc::Counts { sessions_deleted, session_lock_files_deleted, temp_entries_deleted, skipped_held, skipped_lock_unopenable: u64, freed_bytes: Option<u64>, deleted_bytes_still_shared: Option<u64> }`
  - `gc::Skip { path: PathBuf, reason: String }`
  - `gc::GcReport { per_target: Vec<(PathBuf, Counts)>, totals: Counts, skipped: Vec<Skip>, unmeasured: Vec<(PathBuf, String)>, unmeasured_more: u64, cancelled: bool }`
  - `gc::run(fs: &dyn Fs, targets: &[TargetDir], scope: &Scope, opts: &RunOpts, cancel: &dyn Fn() -> bool) -> GcReport`

**Depends-on:** Task 5

- [x] **Step 1: Write the failing tests** at the bottom of the new `src/gc/run.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::fingerprint::ProfileReport;
    use crate::platform::{CompressOutcome, HeldLock, LockHolder, LockKind, LockState, TempDir, VolumeInfo};
    use std::collections::{BTreeSet, HashMap};
    use std::os::unix::fs::MetadataExt;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, UNIX_EPOCH};

    #[derive(Clone, Copy)]
    enum Scripted {
        Held,
        Unopenable,
        Unlockable,
    }

    type Hook = Box<dyn FnOnce() + Send>;

    /// Real files in a scratch dir. Every effect is recorded, and lock outcomes and hooks are scripted by file name.
    struct Rec {
        events: Arc<Mutex<Vec<String>>>,
        scripted: Mutex<HashMap<String, Scripted>>,
        on_lock: Mutex<HashMap<String, Hook>>,
        on_stat: Mutex<HashMap<String, Hook>>,
        caps: Mutex<BTreeSet<usize>>,
        stat_fails: Mutex<BTreeSet<String>>,
        cap_override: Option<usize>,
        fs_type: String,
    }

    fn name(p: &Path) -> String {
        p.file_name().unwrap().to_string_lossy().into_owned()
    }

    fn kind_of(m: &std::fs::Metadata) -> EntryKind {
        if m.file_type().is_symlink() {
            EntryKind::Symlink
        } else if m.is_dir() {
            EntryKind::Dir
        } else if m.is_file() {
            EntryKind::File
        } else {
            EntryKind::Other
        }
    }

    impl Rec {
        fn new() -> Rec {
            Rec {
                events: Arc::new(Mutex::new(Vec::new())),
                scripted: Mutex::new(HashMap::new()),
                on_lock: Mutex::new(HashMap::new()),
                on_stat: Mutex::new(HashMap::new()),
                caps: Mutex::new(BTreeSet::new()),
                stat_fails: Mutex::new(BTreeSet::new()),
                cap_override: None,
                fs_type: "apfs".into(),
            }
        }
        fn log(&self, e: String) {
            self.events.lock().unwrap().push(e);
        }
        /// The recorded events that mention one of `names`, in order.
        fn about(&self, names: &[&str]) -> Vec<String> {
            self.events.lock().unwrap().iter().filter(|e| names.iter().any(|n| e.ends_with(&format!(" {n}")))).cloned().collect()
        }
        fn removals(&self) -> usize {
            self.events.lock().unwrap().iter().filter(|e| e.starts_with("remove_")).count()
        }
    }

    impl Fs for Rec {
        fn temp_dir(&self, _: &Path, _: &str) -> io::Result<TempDir> { Err(io::Error::other("not used by gc")) }
        fn write_file(&self, _: &Path, _: &[u8]) -> io::Result<()> { Err(io::Error::other("not used by gc")) }
        fn read(&self, _: &Path) -> io::Result<Vec<u8>> { Err(io::Error::other("not used by gc")) }
        fn hardlink(&self, _: &Path, _: &Path) -> io::Result<()> { Err(io::Error::other("not used by gc")) }
        fn set_mtime(&self, _: &Path, _: SystemTime) -> io::Result<()> { Err(io::Error::other("not used by gc")) }
        fn mtime(&self, _: &Path) -> io::Result<SystemTime> { Err(io::Error::other("not used by gc")) }
        fn inode(&self, _: &Path) -> io::Result<u64> { Err(io::Error::other("not used by gc")) }
        fn clone_file(&self, _: &Path, _: &Path) -> io::Result<()> { Err(io::Error::other("not used by gc")) }
        fn compress(&self, _: &Path) -> CompressOutcome { CompressOutcome::Failed("not used by gc".into()) }
        fn hold_lock(&self, _: LockKind, _: &Path) -> io::Result<LockHolder> { Err(io::Error::other("not used by gc")) }
        fn try_lock_elsewhere(&self, _: LockKind, _: &Path) -> io::Result<LockState> { Err(io::Error::other("not used by gc")) }
        fn sharing(&self, _: &Path) -> io::Result<Sharing> {
            Ok(Sharing { private_bytes: 0, clone_id: 0, may_share: false, shares_all: false })
        }
        fn try_lock_exclusive(&self, path: &Path) -> TryLock {
            let n = name(path);
            self.log(format!("lock {n}"));
            if let Some(s) = self.scripted.lock().unwrap().get(&n).copied() {
                return match s {
                    Scripted::Held => TryLock::Held,
                    Scripted::Unopenable => TryLock::Unopenable(io::Error::other("scripted")),
                    Scripted::Unlockable => TryLock::Unlockable(io::Error::other("scripted")),
                };
            }
            if std::fs::symlink_metadata(path).is_err() {
                return TryLock::Gone;
            }
            if let Some(hook) = self.on_lock.lock().unwrap().remove(&n) {
                hook();
            }
            let events = Arc::clone(&self.events);
            TryLock::Acquired(HeldLock::new(move || events.lock().unwrap().push(format!("unlock {n}"))))
        }
        fn entry_stat(&self, path: &Path) -> io::Result<EntryStat> {
            let n = name(path);
            if let Some(hook) = self.on_stat.lock().unwrap().remove(&n) {
                hook();
            }
            self.log(format!("stat {n}"));
            if self.stat_fails.lock().unwrap().contains(&n) {
                return Err(io::Error::other("scripted stat failure"));
            }
            let m = std::fs::symlink_metadata(path)?;
            Ok(EntryStat { dev: m.dev(), ino: m.ino(), nlink: m.nlink(), blocks: m.blocks(), kind: kind_of(&m), compressed: false })
        }
        fn read_dir_bounded(&self, dir: &Path, cap: usize) -> io::Result<Vec<(String, EntryKind)>> {
            self.caps.lock().unwrap().insert(cap);
            let entries: Vec<(String, EntryKind)> = std::fs::read_dir(dir)?
                .map(|e| {
                    let e = e?;
                    Ok((e.file_name().to_string_lossy().into_owned(), kind_of(&std::fs::symlink_metadata(e.path())?)))
                })
                .collect::<io::Result<_>>()?;
            if entries.len() > self.cap_override.unwrap_or(cap) {
                return Err(io::Error::other(format!("{} has more than {cap} entries", dir.display())));
            }
            Ok(entries)
        }
        fn remove_tree(&self, path: &Path) -> io::Result<()> {
            self.log(format!("remove_tree {}", name(path)));
            std::fs::remove_dir_all(path)
        }
        fn remove_file(&self, path: &Path) -> io::Result<()> {
            self.log(format!("remove_file {}", name(path)));
            std::fs::remove_file(path)
        }
        fn volume(&self, _: &Path) -> io::Result<VolumeInfo> {
            Ok(VolumeInfo { fs_type: self.fs_type.clone(), container: "disk1".into(), mount: PathBuf::from("/"), available_bytes: 0 })
        }
    }

    fn b36(mut n: u64) -> String {
        let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
        let mut out = Vec::new();
        loop {
            out.push(digits[(n % 36) as usize]);
            n /= 36;
            if n == 0 {
                break;
            }
        }
        out.reverse();
        String::from_utf8(out).unwrap()
    }

    struct Fixture {
        root: PathBuf,
        profile: PathBuf,
        krate: PathBuf,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn fixture(label: &str) -> Fixture {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, Ordering::SeqCst);
        let root = PathBuf::from(format!("/private/tmp/hive-it-gcrun-{label}-{}-{n}", std::process::id()));
        let profile = root.join("target/debug");
        let krate = profile.join("incremental/fixture-0abc");
        std::fs::create_dir_all(&krate).unwrap();
        std::fs::create_dir_all(profile.join("deps")).unwrap();
        std::fs::write(profile.join(".cargo-build-lock"), b"").unwrap();
        Fixture { root: root.canonicalize().unwrap(), profile: profile.canonicalize().unwrap(), krate: krate.canonicalize().unwrap() }
    }

    /// A session `secs_ago` old with one 8 KiB file and (unless `lockless`) its lock file.
    fn session(f: &Fixture, secs_ago: u64, rand: &str, last: &str, lockless: bool) -> (String, String) {
        let ts = (SystemTime::now().duration_since(UNIX_EPOCH).unwrap() - Duration::from_secs(secs_ago)).as_micros() as u64;
        let key = format!("s-{}-{rand}", b36(ts));
        let dir = format!("{key}-{last}");
        std::fs::create_dir_all(f.krate.join(&dir)).unwrap();
        std::fs::write(f.krate.join(&dir).join("query-cache.bin"), vec![1u8; 8192]).unwrap();
        let lock = format!("{key}.lock");
        if !lockless {
            std::fs::write(f.krate.join(&lock), b"").unwrap();
        }
        (dir, lock)
    }

    fn target(path: &Path, profile: &Path, owner: Owner) -> TargetDir {
        TargetDir {
            path: path.to_path_buf(),
            owner,
            profiles: vec![ProfileReport { path: profile.to_path_buf(), format: Format::Supported, units: 0, edges: None, graph: None }],
            size: None,
            unreadable: None,
        }
    }

    fn orphan(f: &Fixture) -> TargetDir {
        target(&f.root.join("target"), &f.profile, Owner::OrphanCandidate { owned_under_caller_env: None })
    }

    fn opts(dry_run: bool) -> RunOpts {
        RunOpts { dry_run, now: SystemTime::now(), account_bytes: true }
    }

    fn never() -> impl Fn() -> bool {
        || false
    }

    #[test]
    fn a_superseded_session_is_removed_dir_first_under_its_lock() {
        let f = fixture("order");
        let (old, old_lock) = session(&f, 3600, "aa", "svh1", false);
        let (new, _) = session(&f, 60, "bb", "svh2", false);
        let rec = Rec::new();
        let report = run(&rec, &[orphan(&f)], &Scope::All, &opts(false), &never());
        assert_eq!(
            rec.about(&[&old, &old_lock]),
            vec![format!("lock {old_lock}"), format!("stat {old}"), format!("remove_tree {old}"), format!("remove_file {old_lock}"), format!("unlock {old_lock}")]
        );
        assert!(f.krate.join(&new).exists() && !f.krate.join(&old).exists());
        assert_eq!((report.totals.sessions_deleted, report.totals.session_lock_files_deleted), (1, 1));
        assert!(report.totals.freed_bytes.unwrap() > 0);
    }

    #[test]
    fn a_session_that_finalizes_after_the_listing_keeps_its_lock_file() {
        let f = fixture("race");
        let (working, lock) = session(&f, 3600, "cc", "working", false);
        let finalized = working.replace("-working", "-svh9");
        let (from, to) = (f.krate.join(&working), f.krate.join(&finalized));
        let rec = Rec::new();
        rec.on_lock.lock().unwrap().insert(lock.clone(), Box::new(move || std::fs::rename(from, to).unwrap()));
        let report = run(&rec, &[orphan(&f)], &Scope::All, &opts(false), &never());
        assert!(f.krate.join(&lock).exists(), "the finalized session lost its lock file");
        assert!(f.krate.join(&finalized).exists());
        assert_eq!(rec.removals(), 0);
        assert_eq!(report.totals.sessions_deleted, 0);
    }

    #[test]
    fn held_unopenable_and_unlockable_locks_leave_their_sessions() {
        let f = fixture("refusals");
        let s1 = session(&f, 3600, "a1", "svh1", false);
        let s2 = session(&f, 3500, "a2", "svh2", false);
        let s3 = session(&f, 3400, "a3", "svh3", false);
        session(&f, 60, "bb", "svh4", false);
        let rec = Rec::new();
        rec.scripted.lock().unwrap().extend([(s1.1.clone(), Scripted::Held), (s2.1.clone(), Scripted::Unopenable), (s3.1.clone(), Scripted::Unlockable)]);
        let report = run(&rec, &[orphan(&f)], &Scope::All, &opts(false), &never());
        assert_eq!((report.totals.skipped_held, report.totals.skipped_lock_unopenable), (1, 1));
        assert!(report.skipped.iter().any(|s| s.path.ends_with(&s3.1) && s.reason.contains("could not be taken")), "{:?}", report.skipped);
        for (dir, _) in [&s1, &s2, &s3] {
            assert!(f.krate.join(dir).exists(), "{dir}");
        }
    }

    #[test]
    fn a_lockless_dir_is_removed_unless_its_lock_appears() {
        let f = fixture("lockless");
        let (gone, _) = session(&f, 3600, "d1", "svh1", true);
        let (kept, kept_lock) = session(&f, 3500, "d2", "svh2", true);
        let appears = f.krate.join(&kept_lock);
        let rec = Rec::new();
        rec.on_stat.lock().unwrap().insert(kept_lock.clone(), Box::new(move || std::fs::write(appears, b"").unwrap()));
        let report = run(&rec, &[orphan(&f)], &Scope::All, &opts(false), &never());
        assert!(!f.krate.join(&gone).exists());
        assert!(f.krate.join(&kept).exists());
        assert_eq!(report.totals.sessions_deleted, 1);
    }

    #[test]
    fn an_orphan_lock_is_removed_unless_a_session_claims_it() {
        let f = fixture("orphan");
        let (d1, l1) = session(&f, 3600, "e1", "svh1", false);
        let (d2, l2) = session(&f, 3500, "e2", "svh2", false);
        std::fs::remove_dir_all(f.krate.join(&d1)).unwrap();
        std::fs::remove_dir_all(f.krate.join(&d2)).unwrap();
        let claimant = f.krate.join(d2.replace("-svh2", "-working"));
        let rec = Rec::new();
        rec.on_lock.lock().unwrap().insert(l2.clone(), Box::new(move || std::fs::create_dir_all(claimant).unwrap()));
        let report = run(&rec, &[orphan(&f)], &Scope::All, &opts(false), &never());
        assert!(!f.krate.join(&l1).exists());
        assert!(f.krate.join(&l2).exists(), "a lock file a session claims was removed");
        assert_eq!(report.totals.session_lock_files_deleted, 1);
    }

    #[test]
    fn temp_entries_go_under_the_build_lock_and_look_alikes_stay() {
        let f = fixture("temps");
        let deps = f.profile.join("deps");
        for d in ["rmetaAb1", "rustcXyz"] {
            std::fs::create_dir_all(deps.join(d)).unwrap();
            std::fs::write(deps.join(d).join("lib.rmeta"), vec![2u8; 4096]).unwrap();
        }
        std::fs::write(deps.join("rustc_hash-0123456789abcdef.d"), b"x").unwrap();
        std::fs::create_dir_all(deps.join("rustc_hash-0123456789abcdef")).unwrap();
        std::os::unix::fs::symlink(&f.root, deps.join("rmetaLnk")).unwrap();
        let rec = Rec::new();
        let report = run(&rec, &[orphan(&f)], &Scope::All, &opts(false), &never());
        assert!(!deps.join("rmetaAb1").exists() && !deps.join("rustcXyz").exists());
        assert!(deps.join("rustc_hash-0123456789abcdef.d").exists() && deps.join("rustc_hash-0123456789abcdef").exists());
        assert!(std::fs::symlink_metadata(deps.join("rmetaLnk")).is_ok() && f.root.exists());
        assert_eq!(report.totals.temp_entries_deleted, 2);
        let around = rec.about(&[".cargo-build-lock", "rmetaAb1", "rustcXyz"]);
        assert_eq!(around.first().unwrap(), "lock .cargo-build-lock");
        assert_eq!(around.last().unwrap(), "unlock .cargo-build-lock");
    }

    #[test]
    fn a_held_or_missing_build_lock_leaves_temp_entries() {
        let f = fixture("buildlock");
        std::fs::create_dir_all(f.profile.join("deps/rmetaQ1")).unwrap();
        let rec = Rec::new();
        rec.scripted.lock().unwrap().insert(".cargo-build-lock".into(), Scripted::Held);
        let report = run(&rec, &[orphan(&f)], &Scope::All, &opts(false), &never());
        assert_eq!(report.totals.skipped_held, 1);
        assert!(f.profile.join("deps/rmetaQ1").exists());
        std::fs::remove_file(f.profile.join(".cargo-build-lock")).unwrap();
        let report = run(&Rec::new(), &[orphan(&f)], &Scope::All, &opts(false), &never());
        assert!(f.profile.join("deps/rmetaQ1").exists());
        assert!(report.skipped.iter().any(|s| s.reason.contains(".cargo-build-lock")), "{:?}", report.skipped);
    }

    #[test]
    fn a_dry_run_counts_what_a_real_run_deletes_and_removes_nothing() {
        let build = |label: &str| {
            let f = fixture(label);
            session(&f, 3600, "aa", "svh1", false);
            session(&f, 60, "bb", "svh2", false);
            session(&f, 3500, "cc", "svh3", true);
            std::fs::create_dir_all(f.profile.join("deps/rmetaZ9")).unwrap();
            std::fs::write(f.profile.join("deps/rmetaZ9/x"), vec![3u8; 4096]).unwrap();
            f
        };
        let (a, b) = (build("dry"), build("real"));
        let rec = Rec::new();
        let dry = run(&rec, &[orphan(&a)], &Scope::All, &opts(true), &never());
        assert_eq!(rec.removals(), 0);
        let real = run(&Rec::new(), &[orphan(&b)], &Scope::All, &opts(false), &never());
        assert_eq!(dry.totals, real.totals);
        assert!(dry.totals.sessions_deleted == 2 && dry.totals.temp_entries_deleted == 1, "{:?}", dry.totals);
    }

    #[test]
    fn scope_selects_owned_or_contained_targets_and_skips_what_it_cannot_judge() {
        let f = fixture("scope");
        let checkout = f.root.clone();
        let mut unreadable = target(&f.root.join("u"), &f.profile, Owner::Unresolved { reason: "x".into() });
        unreadable.unreadable = Some("permission denied".into());
        let mut unsupported = target(&f.root.join("n"), &f.profile, Owner::Owned { checkout: checkout.clone() });
        unsupported.profiles[0].format = Format::Unsupported("no .fingerprint/".into());
        let targets = vec![
            target(Path::new("/private/tmp/elsewhere-owned"), &f.profile, Owner::Owned { checkout: checkout.clone() }),
            orphan(&f),
            target(Path::new("/private/tmp/cache-root/t"), &f.profile, Owner::Unresolved { reason: "scope".into() }),
            unreadable,
            unsupported,
        ];
        let paths = |r: &GcReport| r.per_target.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>();
        let scoped = run(&Rec::new(), &targets, &Scope::Checkouts(vec![checkout.clone()]), &opts(true), &never());
        assert_eq!(paths(&scoped), vec![PathBuf::from("/private/tmp/elsewhere-owned"), f.root.join("target"), f.root.join("n")]);
        assert!(scoped.skipped.iter().any(|s| s.path == f.root.join("u") && s.reason.contains("unreadable")));
        assert!(scoped.skipped.iter().any(|s| s.path == f.profile && s.reason.contains("unsupported")));
        let all = run(&Rec::new(), &targets, &Scope::All, &opts(true), &never());
        assert!(paths(&all).contains(&PathBuf::from("/private/tmp/cache-root/t")));
        let mut hfs = Rec::new();
        hfs.fs_type = "hfs".into();
        let skipped = run(&hfs, &[orphan(&f)], &Scope::All, &opts(true), &never());
        assert!(skipped.per_target.is_empty());
        assert!(skipped.skipped.iter().any(|s| s.reason.contains("not on an APFS volume (filesystem type hfs)")));
    }

    #[test]
    fn a_cancel_stops_before_the_next_entry() {
        let f = fixture("cancel");
        for (i, r) in ["c1", "c2", "c3"].iter().enumerate() {
            session(&f, 3600 - i as u64 * 10, r, "svh1", false);
        }
        session(&f, 60, "zz", "svh9", false);
        let calls = AtomicUsize::new(0);
        let cancel = || calls.fetch_add(1, Ordering::SeqCst) >= 3;
        let report = run(&Rec::new(), &[orphan(&f)], &Scope::All, &opts(false), &cancel);
        assert!(report.cancelled);
        assert!(report.totals.sessions_deleted < 3, "{:?}", report.totals);
    }

    #[test]
    fn listings_use_the_designed_caps() {
        assert_eq!((CRATE_DIR_CAP, DEPS_CAP), (100_000, 1_000_000));
        let f = fixture("caps");
        let rec = Rec::new();
        run(&rec, &[orphan(&f)], &Scope::All, &opts(true), &never());
        let caps = rec.caps.lock().unwrap().clone();
        assert_eq!(caps, BTreeSet::from([CRATE_DIR_CAP, DEPS_CAP]));
    }

    #[test]
    fn without_byte_accounting_the_byte_counts_are_absent() {
        let f = fixture("noaccount");
        session(&f, 3600, "aa", "svh1", false);
        session(&f, 60, "bb", "svh2", false);
        let off = RunOpts { account_bytes: false, ..opts(true) };
        let report = run(&Rec::new(), &[orphan(&f)], &Scope::All, &off, &never());
        assert_eq!((report.totals.freed_bytes, report.totals.deleted_bytes_still_shared), (None, None));
        assert_eq!(report.totals.sessions_deleted, 1);
    }

    #[test]
    fn a_session_nested_past_the_depth_limit_is_left_with_a_finding() {
        let nest = |levels: usize| {
            let f = fixture("depth");
            let (old, _) = session(&f, 3600, "aa", "svh1", false);
            session(&f, 60, "bb", "svh2", false);
            let mut deep = f.krate.join(&old);
            for i in 0..levels {
                deep = deep.join(format!("d{i}"));
            }
            std::fs::create_dir_all(&deep).unwrap();
            let report = run(&Rec::new(), &[orphan(&f)], &Scope::All, &opts(false), &never());
            (f.krate.join(&old).exists(), report)
        };
        let (left, report) = nest(MAX_DEPTH);
        assert!(!left && report.totals.sessions_deleted == 1, "{:?}", report.skipped);
        let (left, report) = nest(MAX_DEPTH + 1);
        assert!(left && report.totals.sessions_deleted == 0);
        assert!(report.skipped.iter().any(|s| s.reason.contains("deeper than")), "{:?}", report.skipped);
    }

    #[test]
    fn a_listing_one_past_its_cap_deletes_nothing_and_is_reported() {
        let build = |label: &str| {
            let f = fixture(label);
            session(&f, 3600, "aa", "svh1", false);
            session(&f, 60, "bb", "svh2", false);
            f
        };
        // The crate dir holds 4 entries: two sessions and their lock files.
        let at = build("cap-at");
        let mut rec = Rec::new();
        rec.cap_override = Some(4);
        let report = run(&rec, &[orphan(&at)], &Scope::All, &opts(false), &never());
        assert_eq!(report.totals.sessions_deleted, 1);
        let past = build("cap-past");
        let mut rec = Rec::new();
        rec.cap_override = Some(3);
        let report = run(&rec, &[orphan(&past)], &Scope::All, &opts(false), &never());
        assert_eq!(report.totals.sessions_deleted, 0);
        assert_eq!(rec.removals(), 0);
        assert!(report.skipped.iter().any(|s| s.reason.contains("could not list it")), "{:?}", report.skipped);
    }

    #[test]
    fn an_orphan_lock_is_kept_when_its_crate_dir_cannot_be_relisted() {
        use std::os::unix::fs::PermissionsExt;
        let f = fixture("orphan-unlistable");
        let (dir, lock) = session(&f, 3600, "e1", "svh1", false);
        std::fs::remove_dir_all(f.krate.join(&dir)).unwrap();
        let krate = f.krate.clone();
        let rec = Rec::new();
        rec.on_lock.lock().unwrap().insert(lock.clone(), Box::new(move || std::fs::set_permissions(krate, std::fs::Permissions::from_mode(0o000)).unwrap()));
        run(&rec, &[orphan(&f)], &Scope::All, &opts(false), &never());
        std::fs::set_permissions(&f.krate, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(f.krate.join(&lock).exists(), "a lock file was removed although the re-check could not list its dir");
    }

    #[test]
    fn a_file_that_cannot_be_measured_is_counted_in_neither_byte_count() {
        let f = fixture("unmeasured");
        let (old, _) = session(&f, 3600, "aa", "svh1", false);
        session(&f, 60, "bb", "svh2", false);
        std::fs::write(f.krate.join(&old).join("opaque.bin"), vec![4u8; 65536]).unwrap();
        let rec = Rec::new();
        rec.stat_fails.lock().unwrap().insert("opaque.bin".into());
        let report = run(&rec, &[orphan(&f)], &Scope::All, &opts(false), &never());
        assert!(report.unmeasured.iter().any(|(p, _)| p.ends_with("opaque.bin")), "{:?}", report.unmeasured);
        let counted = report.totals.freed_bytes.unwrap() + report.totals.deleted_bytes_still_shared.unwrap();
        assert!(counted < 65536, "the unmeasured file was counted: {counted}");
        assert_eq!(report.totals.sessions_deleted, 1);
    }

    #[test]
    fn nothing_outside_the_allowed_entries_changes() {
        let f = fixture("allowed");
        let (old, old_lock) = session(&f, 3600, "aa", "svh1", false);
        session(&f, 60, "bb", "svh2", false);
        for p in [".fingerprint/fixture-0abc/lib-fixture", "build/fixture-0abc/output", "deps/libfixture-0abc.rlib", "fixture"] {
            std::fs::create_dir_all(f.profile.join(p).parent().unwrap()).unwrap();
            std::fs::write(f.profile.join(p), b"keep").unwrap();
        }
        std::fs::create_dir_all(f.profile.join("deps/rmetaT1")).unwrap();
        let listing = |root: &Path| {
            let mut out = BTreeSet::new();
            let mut stack = vec![root.to_path_buf()];
            while let Some(d) = stack.pop() {
                for e in std::fs::read_dir(&d).unwrap() {
                    let p = e.unwrap().path();
                    if std::fs::symlink_metadata(&p).unwrap().is_dir() {
                        stack.push(p.clone());
                    }
                    out.insert(p);
                }
            }
            out
        };
        let before = listing(&f.root);
        run(&Rec::new(), &[orphan(&f)], &Scope::All, &opts(false), &never());
        let after = listing(&f.root);
        let removed: BTreeSet<PathBuf> = before.difference(&after).cloned().collect();
        let allowed = |p: &Path| p.starts_with(f.krate.join(&old)) || p == f.krate.join(&old_lock) || p.starts_with(f.profile.join("deps/rmetaT1"));
        assert!(removed.iter().all(|p| allowed(p)), "{removed:?}");
        assert!(after.is_subset(&before));
    }
}
```

Run `cargo test --lib gc::run`. Expect a compile error: `run`, `Scope`, `RunOpts`, `Counts` and `GcReport` do not exist.

- [x] **Step 2: Implement `src/gc/run.rs`** (above the tests)

```rust
//! Runs the planner over selected targets. Every effect goes through `Fs` in a fixed order:
//! lock, re-check under the lock, measure, remove the dir, remove the lock file, release. A
//! plan made from a stale listing therefore never deletes what replaced its entry.

use super::is_temp_entry;
use super::ledger::Ledger;
use super::plan::{self, Action};
use crate::cargo_target::{Owner, TargetDir};
use crate::fingerprint::Format;
use crate::platform::{EntryKind, EntryStat, Fs, HeldLock, Sharing, TryLock};
use crate::size::MAX_DEPTH;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Most entries listed from one `incremental/` level, crate dir or scanned directory.
pub const CRATE_DIR_CAP: usize = 100_000;
/// Most entries listed from one `deps/` directory.
pub const DEPS_CAP: usize = 1_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// Every target the scan found.
    All,
    /// Targets owned by, or lying inside, these checkouts (realpaths).
    Checkouts(Vec<PathBuf>),
}

#[derive(Debug, Clone, Copy)]
pub struct RunOpts {
    pub dry_run: bool,
    pub now: SystemTime,
    /// False when doctor check (c) has not passed; the byte counts are then left out.
    pub account_bytes: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counts {
    pub sessions_deleted: u64,
    pub session_lock_files_deleted: u64,
    pub temp_entries_deleted: u64,
    pub skipped_held: u64,
    pub skipped_lock_unopenable: u64,
    pub freed_bytes: Option<u64>,
    pub deleted_bytes_still_shared: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skip {
    pub path: PathBuf,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct GcReport {
    pub per_target: Vec<(PathBuf, Counts)>,
    pub totals: Counts,
    pub skipped: Vec<Skip>,
    pub unmeasured: Vec<(PathBuf, String)>,
    pub unmeasured_more: u64,
    pub cancelled: bool,
}

type Scanned = Vec<(PathBuf, io::Result<(EntryStat, Option<Sharing>)>)>;

struct Runner<'a> {
    fs: &'a dyn Fs,
    opts: &'a RunOpts,
    cancel: &'a dyn Fn() -> bool,
    ledger: Ledger,
    skipped: Vec<Skip>,
    cancelled: bool,
}

fn in_scope(t: &TargetDir, scope: &Scope) -> bool {
    match scope {
        Scope::All => true,
        Scope::Checkouts(checkouts) => {
            matches!(&t.owner, Owner::Owned { checkout } if checkouts.contains(checkout))
                || checkouts.iter().any(|c| t.path.starts_with(c))
        }
    }
}

pub fn run(fs: &dyn Fs, targets: &[TargetDir], scope: &Scope, opts: &RunOpts, cancel: &dyn Fn() -> bool) -> GcReport {
    let mut r = Runner { fs, opts, cancel, ledger: Ledger::new(), skipped: Vec::new(), cancelled: false };
    let mut per_target = Vec::new();
    for t in targets.iter().filter(|t| in_scope(t, scope)) {
        if r.stop() {
            break;
        }
        if let Some(why) = &t.unreadable {
            r.skip(&t.path, format!("the target dir is unreadable: {why}"));
            continue;
        }
        match fs.volume(&t.path) {
            Ok(v) if v.fs_type == "apfs" => {}
            Ok(v) => {
                r.skip(&t.path, format!("not on an APFS volume (filesystem type {})", v.fs_type));
                continue;
            }
            Err(e) => {
                r.skip(&t.path, format!("its filesystem type could not be read: {e}"));
                continue;
            }
        }
        let mut counts = Counts::default();
        for p in &t.profiles {
            if r.stop() {
                break;
            }
            if let Format::Unsupported(why) = &p.format {
                r.skip(&p.path, format!("unsupported fingerprint format: {why}"));
                continue;
            }
            r.sessions(&t.path, &p.path, &mut counts);
            r.temp_entries(&t.path, &p.path, &mut counts);
        }
        per_target.push((t.path.clone(), counts));
    }
    let Runner { ledger, skipped, cancelled, .. } = r;
    let tally = ledger.finish();
    let mut totals = Counts::default();
    for (path, c) in &mut per_target {
        if opts.account_bytes {
            let b = tally.per_target.get(path).copied().unwrap_or_default();
            c.freed_bytes = Some(b.freed);
            c.deleted_bytes_still_shared = Some(b.still_shared);
        }
        totals.sessions_deleted += c.sessions_deleted;
        totals.session_lock_files_deleted += c.session_lock_files_deleted;
        totals.temp_entries_deleted += c.temp_entries_deleted;
        totals.skipped_held += c.skipped_held;
        totals.skipped_lock_unopenable += c.skipped_lock_unopenable;
    }
    if opts.account_bytes {
        totals.freed_bytes = Some(tally.total.freed);
        totals.deleted_bytes_still_shared = Some(tally.total.still_shared);
    }
    GcReport { per_target, totals, skipped, unmeasured: tally.unmeasured, unmeasured_more: tally.unmeasured_more, cancelled }
}

impl Runner<'_> {
    fn stop(&mut self) -> bool {
        if !self.cancelled && (self.cancel)() {
            self.cancelled = true;
        }
        self.cancelled
    }

    fn skip(&mut self, path: &Path, reason: String) {
        self.skipped.push(Skip { path: path.to_path_buf(), reason });
    }

    /// Takes a session lock, counting why not when it cannot.
    fn take(&mut self, lock: &Path, c: &mut Counts) -> Option<HeldLock> {
        match self.fs.try_lock_exclusive(lock) {
            TryLock::Acquired(guard) => Some(guard),
            TryLock::Held => {
                c.skipped_held += 1;
                None
            }
            TryLock::Gone => None,
            TryLock::Unopenable(_) => {
                c.skipped_lock_unopenable += 1;
                None
            }
            TryLock::Unlockable(e) => {
                self.skip(lock, format!("the lock could not be taken: {e}"));
                None
            }
        }
    }

    fn sessions(&mut self, target: &Path, profile: &Path, c: &mut Counts) {
        let incremental = profile.join("incremental");
        let crates = match self.fs.read_dir_bounded(&incremental, CRATE_DIR_CAP) {
            Ok(v) => v,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return,
            Err(e) => return self.skip(&incremental, format!("could not list it: {e}")),
        };
        for (name, kind) in crates {
            if kind != EntryKind::Dir {
                continue;
            }
            if self.stop() {
                return;
            }
            let crate_dir = incremental.join(&name);
            let listing = match self.fs.read_dir_bounded(&crate_dir, CRATE_DIR_CAP) {
                Ok(v) => v,
                Err(e) => {
                    self.skip(&crate_dir, format!("could not list it: {e}"));
                    continue;
                }
            };
            for action in plan::crate_dir(&listing, self.opts.now) {
                if self.stop() {
                    return;
                }
                match action {
                    Action::DeleteLocked { dir, lock } => self.delete_locked(target, &crate_dir.join(dir), &crate_dir.join(lock), c),
                    Action::DeleteOrphanLock { lock } => self.delete_orphan_lock(&crate_dir, &lock, c),
                    Action::DeleteLockless { dir, lock } => self.delete_lockless(target, &crate_dir.join(dir), &crate_dir.join(lock), c),
                }
            }
        }
    }

    fn delete_locked(&mut self, target: &Path, dir: &Path, lock: &Path, c: &mut Counts) {
        let Some(guard) = self.take(lock, c) else { return };
        // Re-check under the lock. A session that finalized after the listing has a new name,
        // and this lock file now belongs to it, so neither is touched.
        if !matches!(self.fs.entry_stat(dir), Ok(s) if s.kind == EntryKind::Dir) {
            drop(guard);
            return;
        }
        if self.remove_measured(target, dir) {
            c.sessions_deleted += 1;
            self.remove_lock_file(lock, c);
        }
        drop(guard);
    }

    fn delete_orphan_lock(&mut self, crate_dir: &Path, lock_name: &str, c: &mut Counts) {
        let lock = crate_dir.join(lock_name);
        let Some(guard) = self.take(&lock, c) else { return };
        let prefix = format!("{}-", lock_name.trim_end_matches(".lock"));
        // Re-check under the lock: a session that appeared since the listing owns this file.
        let claimed = match self.fs.read_dir_bounded(crate_dir, CRATE_DIR_CAP) {
            Ok(entries) => entries.iter().any(|(n, _)| n.starts_with(&prefix)),
            Err(_) => true,
        };
        if !claimed {
            self.remove_lock_file(&lock, c);
        }
        drop(guard);
    }

    fn delete_lockless(&mut self, target: &Path, dir: &Path, lock: &Path, c: &mut Counts) {
        // rustc creates a session's lock file before its dir, so a lock file that exists now
        // means the listing missed it: leave the dir for a later run.
        match self.fs.entry_stat(lock) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            _ => return,
        }
        if self.remove_measured(target, dir) {
            c.sessions_deleted += 1;
        }
    }

    fn remove_lock_file(&mut self, lock: &Path, c: &mut Counts) {
        if self.opts.dry_run {
            c.session_lock_files_deleted += 1;
            return;
        }
        match self.fs.remove_file(lock) {
            Ok(()) => c.session_lock_files_deleted += 1,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => self.skip(lock, format!("could not remove the lock file: {e}")),
        }
    }

    fn temp_entries(&mut self, target: &Path, profile: &Path, c: &mut Counts) {
        let lock = profile.join(".cargo-build-lock");
        let guard = match self.fs.try_lock_exclusive(&lock) {
            TryLock::Acquired(guard) => guard,
            TryLock::Held => {
                c.skipped_held += 1;
                return;
            }
            TryLock::Gone => return self.skip(profile, "it has no .cargo-build-lock, so its temp entries are left alone".into()),
            TryLock::Unopenable(e) => return self.skip(&lock, format!("could not open it: {e}")),
            TryLock::Unlockable(e) => return self.skip(&lock, format!("the lock could not be taken: {e}")),
        };
        let deps = profile.join("deps");
        match self.fs.read_dir_bounded(&deps, DEPS_CAP) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => self.skip(&deps, format!("could not list it: {e}")),
            Ok(entries) => {
                for (name, kind) in entries {
                    if !is_temp_entry(&name, kind) {
                        continue;
                    }
                    if self.stop() {
                        break;
                    }
                    if self.remove_measured(target, &deps.join(&name)) {
                        c.temp_entries_deleted += 1;
                    }
                }
            }
        }
        drop(guard);
    }

    /// Measures `path` for the ledger, then removes it (a dry run only measures). True when
    /// the entry is, or in a dry run would be, gone. A vanished entry counts nothing.
    fn remove_measured(&mut self, target: &Path, path: &Path) -> bool {
        let files = match self.scan(path) {
            Ok(files) => files,
            Err(why) => {
                self.skip(path, why);
                return false;
            }
        };
        if !self.opts.dry_run {
            match self.fs.remove_tree(path) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => return false,
                Err(e) => {
                    self.skip(path, format!("could not remove it: {e}"));
                    self.ledger.unmeasured(path.to_path_buf(), format!("removal failed part-way: {e}"));
                    return false;
                }
            }
        }
        for (file, measured) in files {
            match measured {
                Ok((stat, sharing)) => self.ledger.file(target, &stat, sharing.as_ref()),
                Err(e) => self.ledger.unmeasured(file, e.to_string()),
            }
        }
        true
    }

    /// The regular files under `root`, without following links and at most `MAX_DEPTH` deep.
    fn scan(&self, root: &Path) -> Result<Scanned, String> {
        let mut files = Vec::new();
        let mut stack = vec![(root.to_path_buf(), 0usize)];
        while let Some((dir, depth)) = stack.pop() {
            let entries = match self.fs.read_dir_bounded(&dir, CRATE_DIR_CAP) {
                Ok(v) => v,
                Err(e) if e.kind() == io::ErrorKind::NotFound && dir == root => return Ok(Vec::new()),
                Err(e) => return Err(format!("could not list {}: {e}", dir.display())),
            };
            for (name, kind) in entries {
                let path = dir.join(&name);
                match kind {
                    EntryKind::Dir if depth + 1 > MAX_DEPTH => return Err(format!("{} is deeper than {MAX_DEPTH} levels", path.display())),
                    EntryKind::Dir => stack.push((path, depth + 1)),
                    EntryKind::File => {
                        let measured = self.fs.entry_stat(&path).and_then(|stat| {
                            let sharing = if self.opts.account_bytes { Some(self.fs.sharing(&path)?) } else { None };
                            Ok((stat, sharing))
                        });
                        files.push((path, measured));
                    }
                    EntryKind::Symlink | EntryKind::Other => {}
                }
            }
        }
        Ok(files)
    }
}
```

Replace `src/gc.rs` with:

```rust
//! `cargo hive gc`: deletes superseded rustc incremental sessions and leftover rustc temp
//! dirs in Cargo target dirs, each under its owner's lock, and counts the bytes freed per inode.

mod ledger;
mod plan;
mod run;

pub use ledger::{Bytes, UNMEASURED_CAP};
pub use plan::{is_temp_entry, MIN_AGE};
pub use run::{run, Counts, GcReport, RunOpts, Scope, Skip, CRATE_DIR_CAP, DEPS_CAP};
```

Run `cargo test --lib gc`. Expect every test passing: 4 planner tests, 3 ledger tests and 17 runner tests. Then run the merge gate.

- [x] **Step 3: Commit**

Run `cargo fmt`, then `git add -A && git commit -m "feat(gc): runner deletes under the owner's lock, re-checked, in an observable order"`.

_Requirements: AUTOGC-2.2, AUTOGC-2.3, AUTOGC-2.4, AUTOGC-2.9, AUTOGC-2.10, AUTOGC-2.11, AUTOGC-2.12, AUTOGC-2.13, AUTOGC-2.14, AUTOGC-3.1, AUTOGC-3.2, AUTOGC-4.1, AUTOGC-4.2, AUTOGC-4.3, AUTOGC-4.5, AUTOGC-5.3, AUTOGC-5.5, AUTOGC-5.6, AUTOGC-6.1_

---

### Task 7: `cargo hive gc` through the CLI, on real-cargo fixtures

**Files:**
- Modify: `src/cli.rs`, adding the `Gc` command, `gc_cmd` and `put_counts`.
- Modify: `tests/common/mod.rs`, adding `Hive::build_with_sessions`, `crate_dirs`, `sessions_in`, `plant_session`, `Hive::hold_lock_child` and `wait_within`.
- Test: `tests/gc.rs`

**Reuse:** 2 — mirrors `cli::status` (`src/cli.rs:342-518` at d6e0aff; the design cites 329-504), `emit`, `discover::merge_snapshot` (`discover.rs:653`) and `doctor::{require, install_signal_handlers, CANCEL}`; 5 — `clap` derive

**Interfaces:**
- Consumes:
  - `gc::{run, Scope, RunOpts, Counts, GcReport}` (Task 6)
  - `doctor::{require, Capability::{FingerprintReading, Deletion, EvictionAccounting}, install_signal_handlers, CANCEL, resolve_toolchain, CAPABILITIES_FILE, Capabilities}` (Task 2, CORE)
  - `discover::{checkouts, targets, merge_snapshot, Snapshot, SNAPSHOT_FILE}`
  - `platform::{current, Fs::volume}` (Task 3)
  - `registry::{Registry, FILE}`; `emit`, `emit_failure`, `OutputArgs`
- Produces:
  - `cargo hive gc [<repo>…] [--dry-run] [--json]`. Receipt command: `gc` or `gc --dry-run`.
  - Counts: `sessions_deleted`, `session_lock_files_deleted`, `temp_entries_deleted`, `skipped_held`, `skipped_lock_unopenable`, `freed_bytes` and `deleted_bytes_still_shared`, each bare and as `<name>:<target path>`.
  - Findings: `refusal`, `gc.skipped`, `walk.unreadable`, `error`.
  - Notes: `cross-check, not a count: …`, one per container.
  - Exit codes: 0; 1 for any refusal; 130 when cancelled.
  - In `tests/common`:
    - `Hive::build_with_sessions(&self, repo: &Path, target_dir: Option<&Path>)`
    - `crate_dirs(target: &Path) -> Vec<PathBuf>`
    - `sessions_in(crate_dir: &Path) -> Vec<String>`
    - `plant_session(crate_dir: &Path, secs_ago: u64, last: &str, lockless: bool) -> (String, String)`
    - `Hive::hold_lock_child(&self, kind: &str, path: &Path) -> Child`
    - `wait_within(Child, Duration) -> Output`

**Depends-on:** Task 6

- [x] **Step 1: Add the shared test helpers to `tests/common/mod.rs`**

Add `use std::process::{Child, Stdio};` and `use std::time::{Duration, Instant};` beside the existing imports. Then add these two methods to `impl Hive`:

```rust
    /// Builds `repo` three times, editing its lib in between, so each incremental crate dir
    /// holds a superseded session beside the newest one.
    pub fn build_with_sessions(&self, repo: &Path, target_dir: Option<&Path>) {
        for i in 0..3 {
            fs::write(repo.join("src/lib.rs"), format!("pub fn answer() -> u32 {{ {} }}\n", 42 + i)).unwrap();
            self.build_crate(repo, target_dir);
        }
    }

    /// Starts `__hold-lock <kind> <path>` and returns once it reports `held`.
    pub fn hold_lock_child(&self, kind: &str, path: &Path) -> Child {
        let mut child = self
            .command(&["__hold-lock", kind, path.to_str().unwrap()])
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut line = String::new();
        std::io::BufRead::read_line(&mut std::io::BufReader::new(child.stdout.take().unwrap()), &mut line).unwrap();
        assert_eq!(line.trim(), "held");
        child
    }
```

Then add these free functions:

```rust
/// Crate dirs under `<target>/debug/incremental`.
pub fn crate_dirs(target: &Path) -> Vec<PathBuf> {
    let inc = target.join("debug/incremental");
    let mut dirs: Vec<PathBuf> = fs::read_dir(&inc).unwrap().map(|e| e.unwrap().path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    dirs
}

/// Session dir names in one crate dir (lock files excluded).
pub fn sessions_in(crate_dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(crate_dir)
        .unwrap()
        .map(|e| e.unwrap())
        .filter(|e| e.file_type().unwrap().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn b36(mut n: u64) -> String {
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    loop {
        out.push(digits[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).unwrap()
}

/// Plants a session `secs_ago` old (named as rustc names them) holding one 8 KiB file, plus
/// its lock file unless `lockless`. Returns the (dir, lock) names.
pub fn plant_session(crate_dir: &Path, secs_ago: u64, last: &str, lockless: bool) -> (String, String) {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let micros = (SystemTime::now().duration_since(UNIX_EPOCH).unwrap() - Duration::from_secs(secs_ago)).as_micros() as u64;
    let key = format!("s-{}-p{}", b36(micros), NEXT.fetch_add(1, Ordering::SeqCst));
    let dir = format!("{key}-{last}");
    fs::create_dir_all(crate_dir.join(&dir)).unwrap();
    fs::write(crate_dir.join(&dir).join("query-cache.bin"), vec![5u8; 8192]).unwrap();
    let lock = format!("{key}.lock");
    if !lockless {
        fs::write(crate_dir.join(&lock), b"").unwrap();
    }
    (dir, lock)
}

/// Waits for `child` up to `limit`; a hang fails the test instead of blocking the suite.
pub fn wait_within(mut child: Child, limit: Duration) -> Output {
    let started = Instant::now();
    while child.try_wait().unwrap().is_none() {
        if started.elapsed() > limit {
            let _ = child.kill();
            panic!("the command did not finish within {limit:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child.wait_with_output().unwrap()
}
```

`AtomicU32`, `Ordering`, `SystemTime` and `UNIX_EPOCH` are already imported by `Scratch`. If any is missing, add it to the `use` lines.

- [x] **Step 2: Write the failing integration tests:** the new `tests/gc.rs`

```rust
#![cfg(target_os = "macos")]
mod common;
use common::{crate_dirs, init_repo, plant_session, sessions_in, wait_within, Hive, Scratch};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

fn register(hive: &Hive, repo: &Path) {
    let out = hive.run(&["repo", "add", repo.to_str().unwrap()]);
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

fn gc(hive: &Hive, args: &[&str]) -> (std::process::Output, serde_json::Value) {
    let mut all = vec!["gc"];
    all.extend_from_slice(args);
    hive.json(&all)
}

fn count(r: &serde_json::Value, key: &str) -> u64 {
    r["counts"][key].as_u64().unwrap_or_else(|| panic!("no count {key}: {r}"))
}

fn session_counts(target: &Path) -> BTreeMap<PathBuf, usize> {
    crate_dirs(target).into_iter().map(|d| { let n = sessions_in(&d).len(); (d, n) }).collect()
}

/// Every path under `root` with its length and mtime.
fn tree(root: &Path) -> BTreeMap<PathBuf, (u64, std::time::SystemTime)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            let m = fs::symlink_metadata(&p).unwrap();
            if m.is_dir() {
                stack.push(p.clone());
            }
            out.insert(p, (m.len(), m.modified().unwrap()));
        }
    }
    out
}

fn fresh_build_compiles_nothing(hive: &Hive, repo: &Path) -> bool {
    let child = Command::new("cargo")
        .args(["build", "--offline"])
        .current_dir(repo)
        .env("CARGO_HOME", &hive.cargo_home.path)
        .env_remove("CARGO_TARGET_DIR")
        .env_remove("CARGO_BUILD_TARGET_DIR")
        .env_remove("CARGO_BUILD_BUILD_DIR")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out = wait_within(child, Duration::from_secs(300));
    out.status.success() && !String::from_utf8_lossy(&out.stderr).contains("Compiling")
}

#[test]
fn gc_deletes_superseded_sessions_and_the_next_build_compiles_nothing() {
    let hive = Hive::ready();
    let s = Scratch::new("gc-basic");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_with_sessions(&repo, None);
    register(&hive, &repo);
    let target = repo.join("target");
    assert!(session_counts(&target).values().any(|n| *n > 1), "the fixture has no superseded session");
    let (out, r) = gc(&hive, &[repo.to_str().unwrap()]);
    assert!(out.status.success(), "{r}");
    assert_eq!(r["command"], "gc");
    let deleted = count(&r, "sessions_deleted");
    assert!(deleted >= 1, "{r}");
    assert_eq!(count(&r, &format!("sessions_deleted:{}", target.display())), deleted);
    assert_eq!(count(&r, "session_lock_files_deleted"), deleted);
    assert!(count(&r, "freed_bytes") > 0, "{r}");
    assert!(r["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().starts_with("cross-check, not a count")), "{r}");
    assert!(session_counts(&target).values().all(|n| *n == 1), "{:?}", session_counts(&target));
    assert!(fresh_build_compiles_nothing(&hive, &repo));
}

#[test]
fn a_dry_run_reports_what_a_real_run_then_deletes_and_changes_nothing() {
    let hive = Hive::ready();
    let s = Scratch::new("gc-dry");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_with_sessions(&repo, None);
    fs::create_dir_all(repo.join("target/debug/deps/rmetaDry1")).unwrap();
    register(&hive, &repo);
    let before = tree(&repo.join("target"));
    let (_, dry) = gc(&hive, &["--dry-run", repo.to_str().unwrap()]);
    assert_eq!(dry["command"], "gc --dry-run");
    assert_eq!(tree(&repo.join("target")), before, "the dry run changed the target");
    let (_, real) = gc(&hive, &[repo.to_str().unwrap()]);
    for key in ["sessions_deleted", "session_lock_files_deleted", "temp_entries_deleted", "freed_bytes", "deleted_bytes_still_shared"] {
        assert_eq!(count(&dry, key), count(&real, key), "{key}");
    }
}

#[test]
fn held_sessions_and_a_held_build_lock_are_left_until_released() {
    let hive = Hive::ready();
    let s = Scratch::new("gc-held");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_with_sessions(&repo, None);
    register(&hive, &repo);
    let target = repo.join("target");
    let krate = crate_dirs(&target).remove(0);
    let (working, lock) = plant_session(&krate, 3600, "working", false);
    let temp = target.join("debug/deps/rmetaHeld1");
    fs::create_dir_all(&temp).unwrap();
    let mut session_holder = hive.hold_lock_child("fcntl", &krate.join(&lock));
    let mut build_holder = hive.hold_lock_child("flock", &target.join("debug/.cargo-build-lock"));
    let (_, r) = gc(&hive, &[repo.to_str().unwrap()]);
    assert!(krate.join(&working).exists() && temp.exists(), "{r}");
    assert!(count(&r, "skipped_held") >= 2, "{r}");
    for child in [&mut session_holder, &mut build_holder] {
        child.kill().unwrap();
        child.wait().unwrap();
    }
    gc(&hive, &[repo.to_str().unwrap()]);
    assert!(!krate.join(&working).exists() && !temp.exists());
}

#[test]
fn temp_entries_go_and_look_alikes_stay() {
    let hive = Hive::ready();
    let s = Scratch::new("gc-temps");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_crate(&repo, None);
    register(&hive, &repo);
    let deps = repo.join("target/debug/deps");
    fs::create_dir_all(deps.join("rmetaAb12")).unwrap();
    fs::write(deps.join("rmetaAb12/lib.rmeta"), vec![1u8; 4096]).unwrap();
    fs::create_dir_all(deps.join("rustcQw3")).unwrap();
    fs::write(deps.join("rustc_hash-0123456789abcdef.d"), b"x").unwrap();
    let (_, r) = gc(&hive, &[repo.to_str().unwrap()]);
    assert_eq!(count(&r, "temp_entries_deleted"), 2, "{r}");
    assert!(!deps.join("rmetaAb12").exists() && !deps.join("rustcQw3").exists());
    assert!(deps.join("rustc_hash-0123456789abcdef.d").exists());
}

#[test]
fn scope_follows_the_arguments() {
    let hive = Hive::ready();
    let s = Scratch::new("gc-scope");
    let cache = s.path.join("cache");
    hive.config(&format!("{{\"cache_roots\": [\"{}\"]}}", cache.display()));
    let a = init_repo(&s.path.join("a"), "");
    let b = init_repo(&s.path.join("b"), "");
    hive.build_with_sessions(&a, None);
    hive.build_with_sessions(&b, None);
    hive.build_with_sessions(&a, Some(&cache.join("old")));
    register(&hive, &a);
    register(&hive, &b);
    let (b0, c0) = (session_counts(&b.join("target")), session_counts(&cache.join("old")));
    let (out, _) = gc(&hive, &[a.to_str().unwrap()]);
    assert!(out.status.success());
    assert!(session_counts(&a.join("target")).values().all(|n| *n == 1));
    assert_eq!(session_counts(&b.join("target")), b0, "gc <a> touched b");
    assert_eq!(session_counts(&cache.join("old")), c0, "gc <a> touched a cache-root target");
    fs::create_dir_all(s.path.join("nope")).unwrap();
    let (out, r) = gc(&hive, &[s.path.join("nope").to_str().unwrap(), b.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert!(r["findings"].as_array().unwrap().iter().any(|f| f["kind"] == "refusal"), "{r}");
    assert!(session_counts(&b.join("target")).values().all(|n| *n == 1), "the valid argument was not processed");
    gc(&hive, &[]);
    assert!(session_counts(&cache.join("old")).values().all(|n| *n == 1), "bare gc left the cache-root target");
}

#[test]
fn gc_is_refused_without_doctor_results_from_the_current_lock_check() {
    let hive = Hive::ready();
    let s = Scratch::new("gc-gate");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_with_sessions(&repo, None);
    register(&hive, &repo);
    let path = hive.home.path.join("capabilities.json");
    let mut caps: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    caps.as_object_mut().unwrap().remove("lock_check");
    fs::write(&path, caps.to_string()).unwrap();
    let before = session_counts(&repo.join("target"));
    let (out, r) = gc(&hive, &[repo.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let refusal = r["findings"].as_array().unwrap().iter().find(|f| f["kind"] == "refusal").unwrap()["detail"].as_str().unwrap().to_string();
    assert!(refusal.contains("cargo hive doctor"), "{refusal}");
    assert_eq!(session_counts(&repo.join("target")), before);
}

#[test]
fn gc_reads_no_input_and_prints_exactly_one_receipt() {
    let hive = Hive::ready();
    let s = Scratch::new("gc-noinput");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_crate(&repo, None);
    register(&hive, &repo);
    let child = hive
        .command(&["gc", "--json", repo.to_str().unwrap()])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let out = wait_within(child, Duration::from_secs(120));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let receipt: serde_json::Value = serde_json::from_slice(&out.stdout).expect("exactly one JSON receipt on stdout");
    for key in ["schema", "hive_version", "macos_version", "scope"] {
        assert!(!receipt[key].is_null(), "{key}: {receipt}");
    }
    let logged: Vec<String> = fs::read_dir(hive.home.path.join("receipts"))
        .unwrap()
        .flat_map(|e| fs::read_to_string(e.unwrap().path()).unwrap().lines().map(str::to_string).collect::<Vec<_>>())
        .collect();
    assert!(logged.iter().any(|l| l.contains("\"command\":\"gc\"")));
}

#[test]
fn an_unsupported_profile_is_reported_and_left_alone() {
    let hive = Hive::ready();
    let s = Scratch::new("gc-unsupported");
    let repo = init_repo(&s.path.join("app"), "");
    register(&hive, &repo);
    let weird = repo.join("weird");
    fs::create_dir_all(weird.join("debug/build/fixture/0123456789abcdef/fingerprint")).unwrap();
    fs::write(weird.join("CACHEDIR.TAG"), "# This file is a cache directory tag created by cargo.\n").unwrap();
    let krate = weird.join("debug/incremental/fixture-0abc");
    fs::create_dir_all(&krate).unwrap();
    let (old, _) = plant_session(&krate, 3600, "svh1", false);
    plant_session(&krate, 60, "svh2", false);
    let (_, r) = gc(&hive, &[]);
    assert!(krate.join(&old).exists(), "an unsupported profile was cleaned");
    assert!(
        r["findings"].as_array().unwrap().iter().any(|f| f["kind"] == "gc.skipped" && f["path"] == serde_json::json!(weird.join("debug"))),
        "{r}"
    );
}

#[test]
fn builds_running_beside_a_looping_gc_never_fail() {
    let hive = Hive::ready();
    let s = Scratch::new("gc-concurrent");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_with_sessions(&repo, None);
    register(&hive, &repo);
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let looping = {
        let (stop, cmd_home, cargo_home, repo) = (stop.clone(), hive.home.path.clone(), hive.cargo_home.path.clone(), repo.clone());
        std::thread::spawn(move || {
            let mut runs = 0;
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                let out = Command::new(env!("CARGO_BIN_EXE_cargo-hive"))
                    .args(["gc", repo.to_str().unwrap()])
                    .env("HIVE_HOME", &cmd_home)
                    .env("CARGO_HOME", &cargo_home)
                    .output()
                    .unwrap();
                assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
                runs += 1;
            }
            runs
        })
    };
    for i in 0..12 {
        fs::write(repo.join("src/lib.rs"), format!("pub fn answer() -> u32 {{ {} }}\n", 100 + i)).unwrap();
        let out = Command::new("cargo")
            .args(["build", "--offline"])
            .current_dir(&repo)
            .env("CARGO_HOME", &hive.cargo_home.path)
            .env_remove("CARGO_TARGET_DIR")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "build {i} failed: {stderr}");
        assert!(!stderr.contains("incremental"), "build {i} warned about incremental state: {stderr}");
    }
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(looping.join().unwrap() > 0, "gc never ran");
    assert!(fresh_build_compiles_nothing(&hive, &repo));
}

#[test]
fn killing_gc_at_any_point_never_costs_a_rebuild() {
    let hive = Hive::ready();
    let s = Scratch::new("gc-kill");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_with_sessions(&repo, None);
    register(&hive, &repo);
    let target = repo.join("target");
    // The kept session per crate dir: the newest by name timestamp (same-length base 36).
    let kept: BTreeMap<PathBuf, String> = crate_dirs(&target)
        .into_iter()
        .map(|d| {
            let newest = sessions_in(&d).into_iter().max_by_key(|n| n.split('-').nth(1).unwrap().to_string()).unwrap();
            (d, newest)
        })
        .collect();
    let snapshot = |k: &BTreeMap<PathBuf, String>| -> BTreeMap<PathBuf, u64> {
        let mut out = BTreeMap::new();
        for (d, n) in k {
            for e in fs::read_dir(d.join(n)).unwrap() {
                let p = e.unwrap().path();
                out.insert(p.clone(), std::os::unix::fs::MetadataExt::ino(&fs::symlink_metadata(&p).unwrap()));
            }
        }
        out
    };
    let kept_files = snapshot(&kept);
    for d in kept.keys() {
        for i in 0..40 {
            plant_session(d, 7200 + i, "svh0", false);
        }
    }
    for round in 0..8u64 {
        let mut child = hive.command(&["gc", repo.to_str().unwrap()]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        std::thread::sleep(Duration::from_millis(round * 7));
        let _ = Command::new("kill").args(["-9", &child.id().to_string()]).status();
        let _ = child.wait();
        assert_eq!(snapshot(&kept), kept_files, "round {round}: the kept session changed");
    }
    let (out, _) = gc(&hive, &[repo.to_str().unwrap()]);
    assert!(out.status.success());
    for (d, n) in &kept {
        assert_eq!(sessions_in(d), vec![n.clone()], "gc did not converge in {}", d.display());
    }
    assert!(fresh_build_compiles_nothing(&hive, &repo));
    fs::write(repo.join("src/lib.rs"), "pub fn answer() -> u32 { 7 }\n").unwrap();
    hive.build_crate(&repo, None);
}

#[test]
fn without_check_c_gc_still_deletes_and_records_the_byte_counts_as_unmeasured() {
    let hive = Hive::ready();
    let s = Scratch::new("gc-nocheckc");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_with_sessions(&repo, None);
    register(&hive, &repo);
    let path = hive.home.path.join("capabilities.json");
    let mut caps: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    caps["disabled"] = serde_json::json!({"eviction_accounting": ["c"]});
    fs::write(&path, caps.to_string()).unwrap();
    let (out, r) = gc(&hive, &[repo.to_str().unwrap()]);
    assert!(out.status.success(), "{r}");
    assert!(count(&r, "sessions_deleted") >= 1, "{r}");
    assert!(r["counts"].get("freed_bytes").is_none(), "{r}");
    assert!(r["unmeasured"].to_string().contains("doctor check (c) did not pass"), "{r}");
}

#[test]
fn lockless_sessions_and_orphan_locks_are_cleaned_through_the_binary() {
    let hive = Hive::ready();
    let s = Scratch::new("gc-lockless");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_with_sessions(&repo, None);
    register(&hive, &repo);
    let krate = crate_dirs(&repo.join("target")).remove(0);
    let (lockless, _) = plant_session(&krate, 3600, "svh0", true);
    let (gone, orphan_lock) = plant_session(&krate, 3600, "svh0", false);
    fs::remove_dir_all(krate.join(&gone)).unwrap();
    let (_, r) = gc(&hive, &[repo.to_str().unwrap()]);
    assert!(!krate.join(&lockless).exists(), "{r}");
    assert!(!krate.join(&orphan_lock).exists(), "{r}");
}

#[test]
fn an_unreadable_target_is_reported_and_left_alone() {
    use std::os::unix::fs::PermissionsExt;
    let hive = Hive::ready();
    let s = Scratch::new("gc-unreadable");
    let repo = init_repo(&s.path.join("app"), "");
    hive.build_with_sessions(&repo, None);
    register(&hive, &repo);
    let debug = repo.join("target/debug");
    let before = session_counts(&repo.join("target"));
    fs::set_permissions(&debug, fs::Permissions::from_mode(0o000)).unwrap();
    let (_, r) = gc(&hive, &[repo.to_str().unwrap()]);
    fs::set_permissions(&debug, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(session_counts(&repo.join("target")), before);
    assert!(
        r["findings"].as_array().unwrap().iter().any(|f| f["kind"] == "gc.skipped" && f["path"] == serde_json::json!(repo.join("target"))),
        "{r}"
    );
}

#[test]
fn a_target_two_repositories_share_is_cleaned_by_either() {
    let hive = Hive::ready();
    let s = Scratch::new("gc-shared");
    let shared = s.path.join("shared-target");
    let a = init_repo(&s.path.join("a"), "");
    let b = init_repo(&s.path.join("b"), "");
    for repo in [&a, &b] {
        fs::create_dir_all(repo.join(".cargo")).unwrap();
        fs::write(repo.join(".cargo/config.toml"), format!("[build]\ntarget-dir = \"{}\"\n", shared.display())).unwrap();
    }
    hive.build_with_sessions(&a, None);
    register(&hive, &a);
    register(&hive, &b);
    assert!(session_counts(&shared).values().any(|n| *n > 1));
    let (out, r) = gc(&hive, &[b.to_str().unwrap()]);
    assert!(out.status.success(), "{r}");
    assert!(session_counts(&shared).values().all(|n| *n == 1), "gc <b> left the shared target: {r}");
}

#[test]
fn gc_changes_nothing_outside_its_scope_and_status_reports_the_same() {
    let hive = Hive::ready();
    let s = Scratch::new("gc-outside");
    let a = init_repo(&s.path.join("a"), "");
    let b = init_repo(&s.path.join("b"), "");
    hive.build_with_sessions(&a, None);
    hive.build_with_sessions(&b, None);
    register(&hive, &a);
    register(&hive, &b);
    let (b_tree, cargo_home) = (tree(&b), tree(&hive.cargo_home.path));
    let status_b = |hive: &Hive| {
        let (_, r) = hive.json(&["status", "--no-sizes", b.to_str().unwrap()]);
        let mut keys: Vec<String> = r["counts"].as_object().unwrap().keys().cloned().collect();
        keys.sort();
        let findings: Vec<String> = r["findings"].as_array().unwrap().iter().map(|f| format!("{} {}", f["kind"], f["path"])).collect();
        (keys, findings)
    };
    let before = status_b(&hive);
    gc(&hive, &[a.to_str().unwrap()]);
    assert_eq!(tree(&b), b_tree, "gc <a> changed repository b");
    assert_eq!(tree(&hive.cargo_home.path), cargo_home, "gc changed CARGO_HOME");
    assert_eq!(status_b(&hive), before);
}
```

The existing `tests/guards.rs` (repo add, status and doctor stay write-free; cargo builds as before) must keep passing unchanged.

Run `cargo test --test gc`. Expect every test failing: `gc` is an unrecognized subcommand.

- [x] **Step 3: Add `gc` to `src/cli.rs`**

Add the variant:

```rust
    /// Delete superseded incremental sessions and leftover rustc temp dirs, under their owners' locks
    Gc {
        /// Registered repos to clean; none means every target hive knows
        repos: Vec<std::path::PathBuf>,
        /// Report what would be deleted and freed, deleting nothing
        #[arg(long)]
        dry_run: bool,
        #[command(flatten)]
        out: OutputArgs,
    },
```

Add the dispatch arm `Command::Gc { repos, dry_run, out } => gc_cmd(&state, &repos, dry_run, out),`. Then add:

```rust
fn put_counts(b: &mut ReceiptBuilder, suffix: &str, c: &crate::gc::Counts) {
    b.count(&format!("sessions_deleted{suffix}"), c.sessions_deleted)
        .count(&format!("session_lock_files_deleted{suffix}"), c.session_lock_files_deleted)
        .count(&format!("temp_entries_deleted{suffix}"), c.temp_entries_deleted)
        .count(&format!("skipped_held{suffix}"), c.skipped_held)
        .count(&format!("skipped_lock_unopenable{suffix}"), c.skipped_lock_unopenable);
    if let Some(n) = c.freed_bytes {
        b.count(&format!("freed_bytes{suffix}"), n);
    }
    if let Some(n) = c.deleted_bytes_still_shared {
        b.count(&format!("deleted_bytes_still_shared{suffix}"), n);
    }
}

fn gc_cmd(state: &State, args: &[std::path::PathBuf], dry_run: bool, out: OutputArgs) -> i32 {
    use crate::discover::{self, Snapshot, SNAPSHOT_FILE};
    use crate::doctor::{self, Capabilities, Capability};
    use crate::gc;
    use std::collections::{BTreeMap, BTreeSet};
    let command = if dry_run { "gc --dry-run" } else { "gc" };
    if let Err(e) = doctor::install_signal_handlers() {
        return emit_failure(state, out, command, &format!("could not install signal handlers: {e}"), 1);
    }
    let cancelled = || doctor::CANCEL.load(std::sync::atomic::Ordering::SeqCst);
    let loaded = (|| -> Result<_, StateError> {
        Ok((
            state.read::<Registry>(registry::FILE)?,
            state.config()?,
            state.read::<Capabilities>(doctor::CAPABILITIES_FILE)?,
            state.read::<Snapshot>(SNAPSHOT_FILE)?,
        ))
    })();
    let (reg, cfg, caps, mut snapshot) = match loaded {
        Ok(v) => v,
        Err(e) => return emit_failure(state, out, command, &e.to_string(), 1),
    };
    let Ok(platform) = platform::current() else {
        eprintln!("{MACOS_ONLY_MESSAGE}");
        return 2;
    };
    let mut b = Receipt::new(command, Scope::default());
    let release = doctor::resolve_toolchain(state.root()).ok().map(|t| t.release);
    b.toolchain(release.clone());
    for cap in [Capability::FingerprintReading, Capability::Deletion] {
        if let Err(refusal) = doctor::require(&caps, cap, release.as_deref()) {
            b.finding("refusal", None, refusal.to_string());
            return emit(state, out, b.finish(), &format!("refused: {refusal}\n"), 1);
        }
    }
    let mut code = 0;
    let mut selected = Vec::new();
    if args.is_empty() {
        selected = reg.repos.clone();
    }
    for arg in args {
        let real = arg.canonicalize().unwrap_or(arg.clone());
        match reg.repos.iter().find(|r| r.main == real) {
            Some(r) => {
                if !selected.iter().any(|s: &crate::registry::Repo| s.main == r.main) {
                    selected.push(r.clone());
                }
            }
            None => {
                b.finding("refusal", Some(arg.clone()), "not a registered repository; run `cargo hive repo add` first");
                code = 1;
            }
        }
    }
    let mains: Vec<_> = reg.repos.iter().map(|r| r.main.clone()).collect();
    let mut checkouts = Vec::new();
    for repo in &selected {
        b.scope_mut().repos.push(repo.main.clone());
        let (rows, faults) = discover::checkouts(repo, &mains, &cfg);
        for fault in faults {
            b.finding("walk.unreadable", Some(fault.path), fault.detail);
        }
        for c in rows {
            if !c.missing {
                b.scope_mut().checkouts.push(c.path.clone());
            }
            checkouts.push(c);
        }
    }
    let all_repos = selected.len() == reg.repos.len() && selected.iter().all(|s| reg.repos.iter().any(|r| r.main == s.main));
    let scan = discover::targets(&checkouts, &cfg, all_repos, &mut snapshot, cancelled);
    if cancelled() {
        b.finding("error", None, "gc was cancelled");
        return emit(state, out, b.finish(), "gc was cancelled\n", 130);
    }
    for fault in &scan.walk_faults {
        b.finding("walk.unreadable", Some(fault.path.clone()), fault.detail.clone());
    }
    let scope = if args.is_empty() {
        gc::Scope::All
    } else {
        gc::Scope::Checkouts(checkouts.iter().filter(|c| !c.missing).map(|c| c.path.clone()).collect())
    };
    let account_bytes = !caps.disabled.contains_key(&Capability::EvictionAccounting);
    let volumes = |fs: &dyn platform::Fs| -> BTreeMap<String, (std::path::PathBuf, u64)> {
        scan.targets.iter().filter_map(|t| fs.volume(&t.path).ok()).map(|v| (v.container, (v.mount, v.available_bytes))).collect()
    };
    let before = if dry_run { BTreeMap::new() } else { volumes(&*platform.fs) };
    let opts = gc::RunOpts { dry_run, now: std::time::SystemTime::now(), account_bytes };
    let report = gc::run(&*platform.fs, &scan.targets, &scope, &opts, &cancelled);
    if !dry_run {
        let after = volumes(&*platform.fs);
        let deleted_on: BTreeSet<String> = report
            .per_target
            .iter()
            .filter(|(_, c)| c.sessions_deleted + c.session_lock_files_deleted + c.temp_entries_deleted > 0)
            .filter_map(|(p, _)| platform.fs.volume(p).ok().map(|v| v.container))
            .collect();
        for container in &deleted_on {
            if let (Some((mount, was)), Some((_, now))) = (before.get(container), after.get(container)) {
                let change = i128::from(*now) - i128::from(*was);
                b.note(format!(
                    "cross-check, not a count: available bytes in APFS container {container} (mounted at {}) changed by {change:+} during this run",
                    mount.display()
                ));
            }
        }
    }
    put_counts(&mut b, "", &report.totals);
    let mut human = String::new();
    for (path, c) in &report.per_target {
        put_counts(&mut b, &format!(":{}", path.display()), c);
        human.push_str(&format!(
            "{}  {} sessions, {} temp entries{}\n",
            path.display(),
            c.sessions_deleted,
            c.temp_entries_deleted,
            c.freed_bytes.map(|n| format!(", {n} bytes freed")).unwrap_or_default()
        ));
    }
    if !account_bytes {
        for name in ["freed_bytes", "deleted_bytes_still_shared"] {
            b.unmeasured(name, "doctor check (c) did not pass");
        }
    }
    for skip in &report.skipped {
        b.finding("gc.skipped", Some(skip.path.clone()), skip.reason.clone());
    }
    for (path, why) in &report.unmeasured {
        b.unmeasured(&path.display().to_string(), why.clone());
    }
    if report.unmeasured_more > 0 {
        b.unmeasured("unmeasured_paths", format!("{} more paths could not be measured and are not listed", report.unmeasured_more));
    }
    if report.cancelled {
        b.finding("error", None, "gc was cancelled");
        return emit(state, out, b.finish(), &human, 130);
    }
    let wrote = match state.locked_or_cancelled(cancelled, |tx| {
        if cancelled() {
            return Ok(false);
        }
        let disk = tx.read(SNAPSHOT_FILE)?;
        tx.write(SNAPSHOT_FILE, &discover::merge_snapshot(snapshot, disk, &scan.cleared_profiles))?;
        Ok(true)
    }) {
        Ok(wrote) => wrote,
        Err(StateError::Cancelled) => false,
        Err(e) => {
            b.finding("error", None, e.to_string());
            return emit(state, out, b.finish(), &human, 1);
        }
    };
    if !wrote {
        b.finding("error", None, "gc was cancelled");
        return emit(state, out, b.finish(), &human, 130);
    }
    emit(state, out, b.finish(), &human, code)
}
```

Run `cargo test --test gc` and `cargo test --test guards`. Expect 15 and all passed. Then run the merge gate.

- [x] **Step 4: Commit**

Run `cargo fmt`, then `git add -A && git commit -m "feat(cli): cargo hive gc, with --dry-run, scoped by arguments and gated on doctor"`.

_Requirements: AUTOGC-2.7, AUTOGC-2.6, AUTOGC-2.15, AUTOGC-3.3, AUTOGC-3.4, AUTOGC-4.2, AUTOGC-4.3, AUTOGC-4.4, AUTOGC-5.3, AUTOGC-5.4, AUTOGC-5.6, AUTOGC-6.2, AUTOGC-6.3, AUTOGC-6.5, AUTOGC-7.1, AUTOGC-7.2, AUTOGC-7.3, AUTOGC-7.5_

---

### Task 8: Freed bytes never exceed what the volume gains (test-created APFS image)

**Files:**
- Test: `tests/gc_disk_image.rs`

**Reuse:** existing — `tests/common::{Hive, Scratch, init_repo, crate_dirs, plant_session}`; `platform::apfs::Apfs::volume` (Task 3); the `hdiutil` exception in `docs/standards/testing.md`

**Interfaces:**
- Consumes: `cargo hive gc` (Task 7) and `Fs::volume` (Task 3).
- Produces: no new code, only the test.

**Depends-on:** Task 7

- [x] **Step 1: Write the test.** The new `tests/gc_disk_image.rs`:

```rust
#![cfg(target_os = "macos")]
mod common;
use cargo_hive::platform::apfs::Apfs;
use cargo_hive::platform::Fs;
use common::{crate_dirs, init_repo, plant_session, sessions_in, Hive, Scratch};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn sh(args: &[&str]) {
    let out = Command::new(args[0]).args(&args[1..]).output().unwrap();
    assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// Detaches and removes disk-image scratch dirs left by test processes that no longer exist.
fn sweep_stale() {
    for e in fs::read_dir("/private/tmp").unwrap().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(pid) = name.strip_prefix("hive-it-diskimage-").and_then(|r| r.split('-').next()) else { continue };
        let alive = Command::new("kill").args(["-0", pid]).status().map(|s| s.success()).unwrap_or(false);
        if !alive {
            let _ = Command::new("hdiutil").args(["detach", "-force", "-quiet"]).arg(e.path().join("mnt")).status();
            let _ = fs::remove_dir_all(e.path());
        }
    }
}

/// An APFS image attached inside its scratch dir, never under /Volumes, unindexed; detached on drop.
struct Image {
    _scratch: Scratch,
    mount: PathBuf,
}

impl Image {
    fn new() -> Image {
        sweep_stale();
        let scratch = Scratch::new("diskimage");
        let (img, mount) = (scratch.path.join("hive.dmg"), scratch.path.join("mnt"));
        fs::create_dir_all(&mount).unwrap();
        sh(&["hdiutil", "create", "-size", "512m", "-fs", "APFS", "-volname", "hive-it", "-quiet", img.to_str().unwrap()]);
        sh(&["hdiutil", "attach", "-nobrowse", "-quiet", "-mountpoint", mount.to_str().unwrap(), img.to_str().unwrap()]);
        fs::write(mount.join(".metadata_never_index"), b"").unwrap();
        Image { mount: mount.canonicalize().unwrap(), _scratch: scratch }
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        let _ = Command::new("hdiutil").args(["detach", "-force", "-quiet"]).arg(&self.mount).status();
    }
}

fn big(path: &Path, mib: usize) {
    fs::write(path, vec![7u8; mib << 20]).unwrap();
}

#[test]
fn freed_bytes_never_exceed_the_space_the_volume_gains() {
    let image = Image::new();
    let hive = Hive::ready();
    let repo = init_repo(&image.mount.join("app"), "");
    hive.build_with_sessions(&repo, None);
    let target = repo.join("target");
    let deps = target.join("debug/deps");
    for (i, d) in crate_dirs(&target).iter().enumerate() {
        let kept = sessions_in(d).into_iter().max_by_key(|n| n.split('-').nth(1).unwrap().to_string()).unwrap();
        for j in 0..8 {
            let (old, _) = plant_session(d, 7200 + j, "svh0", false);
            big(&d.join(&old).join("big.bin"), 4);
        }
        // Old-session files still held elsewhere must never count as freed.
        let (linked, _) = plant_session(d, 9000, "svh0", false);
        big(&deps.join(format!("hive-linked-{i}.o")), 16);
        fs::hard_link(deps.join(format!("hive-linked-{i}.o")), d.join(&linked).join("in-deps.o")).unwrap();
        big(&d.join(&kept).join(format!("hive-both-{i}.o")), 16);
        fs::hard_link(d.join(&kept).join(format!("hive-both-{i}.o")), d.join(&linked).join("in-both.o")).unwrap();
        fs::hard_link(d.join(&kept).join(format!("hive-both-{i}.o")), deps.join(format!("hive-both-{i}.o"))).unwrap();
    }
    assert!(hive.run(&["repo", "add", repo.to_str().unwrap()]).status.success());
    sh(&["sync"]);
    let apfs = Apfs::new(env!("CARGO_BIN_EXE_cargo-hive").into());
    let before = apfs.volume(&image.mount).unwrap();
    let (out, r) = hive.json(&["gc", repo.to_str().unwrap()]);
    assert!(out.status.success(), "{r}");
    sh(&["sync"]);
    let after = apfs.volume(&image.mount).unwrap();
    let freed = r["counts"]["freed_bytes"].as_u64().unwrap();
    let gained = after.available_bytes.saturating_sub(before.available_bytes);
    // Writes by system daemons between the two samples; far below the bytes under test.
    const SLACK: u64 = 1 << 20;
    assert!(freed >= 32 << 20, "the fixture freed too little to test the bound: {freed}");
    assert!(freed <= gained + SLACK, "freed {freed} > gained {gained} + slack");
    assert!(r["counts"]["deleted_bytes_still_shared"].as_u64().unwrap() >= 32 << 20, "{r}");
    let note = format!("container {} ", before.container);
    assert!(r["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains(&note)), "{r}");
}
```

Run `cargo test --test gc_disk_image`. Expect 1 test passing. The test exercises Tasks 1–7. A failure means a real defect in those tasks: stop and report it, and do not loosen the bound.

- [x] **Step 2: Commit**

Run `cargo fmt`, then `git add -A && git commit -m "test(gc): freed bytes never exceed the space an APFS volume gains"`.

_Requirements: AUTOGC-5.4, AUTOGC-6.4_

---

### Task 9: README, and the CORE realign

**Files:**
- Modify: `README.md`
- Modify: `docs/specs/2026-10-25-foundation/design.md` (lines 66-69 and 179) and `docs/specs/2026-10-25-foundation/requirements.md` (CORE-1.1(d)), through `realign-spec`

**Reuse:** existing — the `realign-spec` skill (design Decision 16)

**Interfaces:**
- Consumes: the shipped behavior of Tasks 1–8.
- Produces: documentation only.

**Depends-on:** Task 8

- [x] **Step 1: Edit `README.md`**
  1. Replace the paragraph starting "This release (foundation) reads only" with: "It registers repositories, lists their checkouts and Cargo target dirs, and says how hive may treat each one. `cargo hive gc` deletes superseded rustc incremental sessions and leftover rustc temp dirs, each under its owner's lock. It compresses and seeds nothing yet."
  2. Add the table row: `` | `cargo hive gc [<repo>…] [--dry-run]` | Deletes superseded incremental sessions and `deps/rustc*`/`rmeta*` temp dirs under their owners' locks; reports freed bytes per inode. `--dry-run` deletes nothing | ``
  3. Replace "It writes only under `~/.hive/` and inside doctor's own temporary directory." with: "It writes only under `~/.hive/` and doctor's temporary directory. Outside those, it only deletes, through `gc`, inside Cargo target dirs."
  4. Add a `## gc` section saying four things:
     - each session is deleted under its own lock, and temp entries under `.cargo-build-lock`;
     - held entries are skipped;
     - output kept with `-C save-temps` matches the temp-entry rule and is reclaimed;
     - targets off APFS are skipped.
- [x] **Step 2: Realign CORE.** Invoke the `realign-spec` skill for feature CORE. It must make three changes:
  - reword CORE `design.md:66-69` and `:179` so doctor (d) tries `flock(LOCK_EX|LOCK_NB)` on a read-only fd against flock, fcntl-read and fcntl-write holders, and measures the reverse;
  - drop `probe_lock` from the interface line;
  - align CORE-1.1(d)'s wording with AUTOGC-1.1 and AUTOGC-1.2.

  Leave the open question of whether check (a) should gate private-byte receipts as an Open Question in CORE (owner decision O1).
- [x] **Step 3: Commit**

Run `git add -A && git commit -m "docs: gc in the README; realign CORE's doctor lock check"`.

_Requirements: AUTOGC-2.9, AUTOGC-7.2_
