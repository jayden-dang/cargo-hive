# Tasks: Plan-tier GC (stored plans of dead-unit files and orphan targets, applied after a recheck)

> **For agentic workers:** after plan approval, pick one execute skill:
> - `build-in-waves`: a continuous, dependency-aware scheduler;
> - `build-by-story`: human-gated story review units;
> - `build-inline`: the controller implements, with no implementer subagents.
>
> The chosen skill writes `Execution-mode:`. Steps use checkbox (`- [ ]`) syntax for tracking.

Feature code: GCPLAN
Status: Approved
Date: 2026-11-01
Execution-mode: continuous
Max-concurrency: auto
Requirements: ./requirements.md
Design: ./design.md

**Goal:** Add `cargo hive gc --plan` and `cargo hive apply <id>`: a stored, one-use plan of the dead-unit files (and orphan target dirs) of the targets in scope, applied only after liveness, ownership and locks are rechecked, with `needs-human` for protected and orphan plans.

**Architecture:**
- **`src/plan_tier/`** (new) holds the plan record and its store (`plans.json`), the plan builder, the lock acquisition for one target, the two-phase apply, the orphan removal and the path guard. Its public surface is `make`, `apply`, `execute`, `Authorization` and `Approval`.
- **Platform and gc** gain only what apply needs: `HeldLock` reports the identity of the opened lock file, and the byte-accounting delete of the auto tier is extracted as `gc::account::remove_counted`.
- **Receipt schema 3** is additive and appears only on `gc --plan` and `apply`; every other command keeps schema 2.

**Tech Stack:** Rust 2021, rustc/cargo 1.97.1 on macOS 27 (APFS). Existing dependencies only: `serde`, `serde_json`, `clap`, `libc`. No new dependency. Integration tests run the real binary (`env!("CARGO_BIN_EXE_cargo-hive")`) on scratch fixtures under `/private/tmp/hive-it-*`. Library-level tests (`tests/plan_tier.rs`) are the design's one new seam and a deliberate exception to the binary-only layer, as `tests/liveness.rs` is.

## Global Constraints

The canonical sources (`docs/standards/` is the SSOT). Read them; do not restate them. Hashes were taken 2026-11-01, and a mismatch at execute time means the source changed after planning, except for a file a task lists under Files (Task 11 edits `docs/standards/errors-logging.md`).

| Source | sha256 (first 16) |
|---|---|
| `docs/standards/INDEX.md` | `80cd41328c0f7652` |
| `docs/standards/code.md` (bounds on every walk, read and child process; no new deps; no requirement IDs in code, comments or test names; `//!` module comments) | `afdcc8b89136f573` |
| `docs/standards/errors-logging.md` (typed errors with hand-written `Display`; expected refusals are data; exit codes) | `48f5baac3b52a552` |
| `docs/standards/testing.md` (merge gate; scratch-only fixtures under `/private/tmp/hive-it-*`; `HIVE_HOME` is the only test-only environment variable; no test-only branches in production paths) | `2f60a5b234124d90` |
| `docs/architecture/INDEX.md`: this feature relies on ARCH-4, 7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 18, 19, 20 | `8cb275c512d9ab3c` |
| `docs/specs/2026-11-01-gc-plan-apply/design.md` (Decisions 1–16) | see git |
| `.skills/GCPLAN/close-package.md`, `.skills/GCPLAN/design-scan.md`, `.skills/research/2026-11-01-cargo-build-locks.md` | see git |

Merge gate (testing standard). Every task must leave all four commands green:
- `cargo fmt --check`, after `cargo fmt`.
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- `cargo check --target x86_64-unknown-linux-gnu`

Commits: each commit step is `cargo fmt`, then `git add -A && git commit -m "<subject>"`, on the feature branch the execute skill creates. No `Co-Authored-By` or generated-by line.

House rules for this plan:
1. Plan tier never compiles, never replays a build and never runs a cargo subcommand other than the one `cargo metadata --no-deps --format-version 1 --locked --offline` that `cargo_target::resolve` already runs (ARCH-13, ARCH-17).
2. No requirement ID appears in source, comments or test names.
3. `status`, `explain`, `doctor`, `repo` and `gc` without `--plan` must not change in any task: their receipts keep schema 2. Their existing tests are the guard (`tests/gc.rs`, `tests/status_targets.rs`, `tests/explain.rs`, `tests/doctor.rs`, `tests/receipts.rs`, `tests/repo.rs`).
4. Plan tier removes a path only after `plan_tier::guard::check_entry` (or `delete_checked`) accepted it, and measures and removes through `gc::account::remove_counted` or `guard::delete_checked`, never both on one path. It writes plans only through the `state` module (ARCH-16).
5. Every child process in test code runs with a deadline (`common::wait_within`), every walk has a cap, and a hit is a test failure (code.md Bounds).
6. Adding a `Command` variant breaks `tests/skill.rs` at compile time (`parse` matches `Command` exhaustively); any task that adds a variant also maps it in `Parsed` and `parse` in the same commit.
7. No test-only branch or environment variable in a production path. Failure and cancellation are injected through the `Fs` wrapper and the `cancel` closure the library seam takes, and exit codes are checked on the real binary with a real signal.
8. Unit tests that call `platform::current()` carry `#[cfg(target_os = "macos")]` and create fixtures under `/private/tmp/hive-it-*`; a test removes its scratch directory when it passes.

## File Structure

| Path | Change | Responsibility |
|---|---|---|
| `src/plan_tier/mod.rs` | create | Module header, public re-exports, `Ctx` |
| `src/plan_tier/record.rs` | create | Plan types, `plans.json` store: `insert`, `get`, `consume`, `prune` |
| `src/plan_tier/build.rs` | create | `make`, `plan_for_target`, `Plan::single`, class of a target, undecided and nothing-to-do groups |
| `src/plan_tier/locks.rs` | create | `acquire`: lock set for one target, identity compare, profile-set recheck |
| `src/plan_tier/apply.rs` | create | `apply`, `execute`, `Authorization`, `Approval`, two phases, outcomes |
| `src/plan_tier/orphan.rs` | create | Orphan target removal behind an `Approval` |
| `src/plan_tier/guard.rs` | create | `check_entry`, `delete_checked`: canonical-parent check, identity check, delete by kind |
| `src/lib.rs` | modify | `pub mod plan_tier;` |
| `src/gc.rs`, `src/gc/run.rs` | modify | Re-export `Ledger` as `pub(crate)`; `Runner` calls the extracted delete |
| `src/gc/account.rs` | create | `remove_counted`: measure then remove one path with the shared `Ledger` |
| `src/platform/mod.rs`, `src/platform/apfs.rs` | modify | `HeldLock::with_identity` and `identity()`; `Apfs::try_lock_exclusive` sets it |
| `src/state.rs` | modify | `gc_window_days` in `ConfigFile` and `Config`; `window_days` validation |
| `src/receipt.rs` | modify | Schema 3 body: `plans`, `undecided`, `nothing_to_do` |
| `src/cli.rs` | modify | `gc --plan`, `apply`, `EXIT_NEEDS_HUMAN`, `cross_check_notes` |
| `tests/common/mod.rs` | modify | `pub mod plan_fixture; pub mod recording_fs;` and `Hive::json_env` |
| `tests/common/plan_fixture.rs` | create | `Planned`: a main and linked worktrees with dead units; `tree`; `LibCtx` |
| `tests/common/recording_fs.rs` | create | A recording `Fs` wrapper: logs lock and remove order, injects lock and removal faults |
| `tests/common/prove.rs` | modify | `delete_in_clone` calls the guard; the `TryLock::Gone` arm; `trial_through_execute` |
| `tests/plan_apply.rs` | create | Binary-level tests of `gc --plan` and `apply` |
| `tests/plan_tier.rs` | create | Library-level tests of `locks`, `make`, `apply`, `execute`, orphan removal |
| `tests/platform_locks.rs` | modify | Lock identity test |
| `tests/liveness.rs`, `tests/liveness_prove.rs` | modify | Trial through `execute` |
| `tests/skill.rs` | modify | `Parsed` and `parse` for `Gc { plan }` and `Apply`; documented commands |
| `plugin/skills/hive/SKILL.md`, `skills/hive/eval.json`, `skills/hive/TESTS.md` | modify | Teach `gc --plan` and `apply` |
| `CONTEXT.md`, `docs/standards/errors-logging.md`, `docs/specs/2026-10-30-closure-liveness/design.md` | modify | Terms, exit code 3, corrected lock rule |

Retrieval (manual approximation of `load-subgraph`'s `blast_radius` and `cluster(GCPLAN)`): every file above under `src/` is also named by CORE, AUTOGC, EXPLAIN, SKILL or LIVE `tasks.md` (`grep -ln` over `docs/specs/*/tasks.md`); all of those specs are `Implemented`, so there is no collision with an unshipped plan. `owns_coverage` is unverified beyond those five files.

---

### Task 1: A held lock reports which file it holds

**Files:**
- Modify: `src/platform/mod.rs` (`HeldLock`)
- Modify: `src/platform/apfs.rs` (`Apfs::try_lock_exclusive`)
- Test: `tests/platform_locks.rs`

**Reuse:** extends — `HeldLock` and `Apfs::try_lock_exclusive` (rung 2); no new `Fs` method.

**Interfaces:**
- Consumes: `Fs::try_lock_exclusive(&Path) -> TryLock`, `Fs::entry_stat(&Path) -> io::Result<EntryStat>`
- Produces: `HeldLock::with_identity(dev: u64, ino: u64, release: impl FnOnce() + Send + 'static) -> HeldLock`; `HeldLock::identity(&self) -> Option<(u64, u64)>` (`None` for `HeldLock::new`)

**Depends-on:** none

- [x] **Step 1: Write the failing test** in `tests/platform_locks.rs`

```rust
#[test]
fn an_acquired_lock_reports_the_identity_of_the_file_it_opened() {
    let s = Scratch::new("lock-identity");
    let path = s.path.join(".cargo-build-lock");
    std::fs::write(&path, b"").unwrap();
    let fs = apfs();
    let stat = fs.entry_stat(&path).unwrap();
    match fs.try_lock_exclusive(&path) {
        TryLock::Acquired(held) => assert_eq!(held.identity(), Some((stat.dev, stat.ino))),
        other => panic!("expected Acquired, got {other:?}"),
    }
}

#[test]
fn a_lock_whose_file_was_replaced_after_it_was_opened_reports_a_different_identity() {
    let s = Scratch::new("lock-replaced");
    let path = s.path.join(".cargo-build-lock");
    std::fs::write(&path, b"").unwrap();
    let fs = apfs();
    let TryLock::Acquired(held) = fs.try_lock_exclusive(&path) else {
        panic!("not acquired")
    };
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, b"").unwrap();
    let now = fs.entry_stat(&path).unwrap();
    assert_ne!(held.identity(), Some((now.dev, now.ino)));
}
```

Run: `cargo test --test platform_locks identity` — expect: compile error, no method `identity` on `HeldLock`.

- [x] **Step 2: Implement**

In `src/platform/mod.rs` give `HeldLock` an `identity: Option<(u64, u64)>` field that `HeldLock::new` sets to `None`; add `pub fn with_identity(dev: u64, ino: u64, release: impl FnOnce() + Send + 'static) -> HeldLock` and `pub fn identity(&self) -> Option<(u64, u64)>`. In `Apfs::try_lock_exclusive` (`src/platform/apfs.rs`, where the opened file's metadata `m` is already read for the regular-file check) return `TryLock::Acquired(HeldLock::with_identity(m.dev(), m.ino(), move || drop(file)))` (bind `m` outside the `match file.metadata()` arm so it is in scope here) using `std::os::unix::fs::MetadataExt`, without a second `metadata()` call. Leave the `HeldLock::new` calls in `src/doctor.rs` and `src/gc/run.rs` untouched.

Run: `cargo test --test platform_locks` — expect: pass, existing lock tests still pass.

- [x] **Step 3: Commit**

`git commit -m "platform: a held lock reports the identity of the file it opened"`

_Requirements: GCPLAN-2.4_

---

### Task 2: The auto tier's counted delete becomes a shared helper

**Files:**
- Create: `src/gc/account.rs`
- Modify: `src/gc.rs` (declare `mod account;`, re-export `remove_counted` as `pub(crate)`)
- Modify: `src/gc/run.rs` (`Runner::remove_measured` calls the helper)
- Modify: `src/cli.rs` (`cross_check_notes`)
- Test: `src/gc/account.rs` (`#[cfg(test)]`), existing `tests/gc.rs`

**Reuse:** extends — `scan`, `record`, `Runner::remove_measured` (`src/gc/run.rs`), `Ledger` (`src/gc/ledger.rs`) and the cross-check block of `gc_cmd` (`src/cli.rs`) (rung 2).

**Interfaces:**
- Consumes: `Fs::{entry_stat, read_dir_bounded, remove_tree, remove_file}`, `Ledger::new()` (no arguments), `Ledger::finish() -> Tally { per_target, total: Bytes, unmeasured, unmeasured_more }`
- Produces: `pub(crate) fn remove_counted(fs: &dyn Fs, ledger: &mut Ledger, target: &Path, path: &Path, account_bytes: bool, dry_run: bool) -> Result<Removed, RemoveFailed>` where `Removed { files: u64, existed: bool }` (`existed` is false for a path that was already gone) and `RemoveFailed::{Scan(String), Remove(io::Error)}`; a `NotFound` removal counts nothing and returns `Removed { files: 0, existed: false }`; `pub(crate) fn cross_check_notes(before: &BTreeMap<String, (PathBuf, u64)>, after: &BTreeMap<String, (PathBuf, u64)>, touched: &BTreeSet<String>) -> Vec<String>` in `src/cli.rs`

**Depends-on:** none

- [x] **Step 1: Write the failing test** in `src/gc/account.rs`

```rust
#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::platform::{self, Fs};
    use std::path::PathBuf;

    fn scratch(label: &str) -> PathBuf {
        let d = PathBuf::from(format!("/private/tmp/hive-it-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_file_root_is_measured_and_removed() {
        let platform = platform::current().unwrap();
        let fs: &dyn Fs = platform.fs.as_ref();
        let dir = scratch("account-file");
        let file = dir.join("libdep-0123456789abcdef.rlib");
        std::fs::write(&file, vec![7u8; 8192]).unwrap();
        let mut ledger = Ledger::new();
        let removed = remove_counted(fs, &mut ledger, &dir, &file, true, false).unwrap();
        assert_eq!(removed.files, 1);
        assert!(!file.exists());
        let tally = ledger.finish();
        assert!(tally.total.freed >= 8192, "freed {}", tally.total.freed);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_root_is_walked_without_following_symlinks_and_removed() {
        let platform = platform::current().unwrap();
        let fs: &dyn Fs = platform.fs.as_ref();
        let dir = scratch("account-dir");
        let unit = dir.join("unit");
        std::fs::create_dir_all(unit.join("sub")).unwrap();
        std::fs::write(unit.join("sub/a"), b"aaaa").unwrap();
        std::os::unix::fs::symlink(&dir, unit.join("link-out")).unwrap();
        let mut ledger = Ledger::new();
        let removed = remove_counted(fs, &mut ledger, &dir, &unit, true, false).unwrap();
        assert_eq!(removed.files, 1);
        assert!(!unit.exists());
        assert!(dir.exists(), "the symlink target must survive");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_dry_run_measures_and_removes_nothing() {
        let platform = platform::current().unwrap();
        let fs: &dyn Fs = platform.fs.as_ref();
        let dir = scratch("account-dry");
        let file = dir.join("a.rlib");
        std::fs::write(&file, vec![1u8; 4096]).unwrap();
        let mut ledger = Ledger::new();
        let removed = remove_counted(fs, &mut ledger, &dir, &file, true, true).unwrap();
        assert_eq!(removed.files, 1);
        assert!(file.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_path_that_is_already_gone_counts_nothing() {
        let platform = platform::current().unwrap();
        let fs: &dyn Fs = platform.fs.as_ref();
        let dir = scratch("account-gone");
        let mut ledger = Ledger::new();
        let removed = remove_counted(fs, &mut ledger, &dir, &dir.join("missing"), true, false).unwrap();
        assert_eq!(removed.files, 0);
        assert!(!removed.existed);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
```

Run: `cargo test -p cargo-hive account` — expect: compile error, module `account` not found.

- [x] **Step 2: Implement `remove_counted`**

For `EntryKind::File` or `Symlink` roots: `entry_stat` once, record that single file into the ledger as `record` does for a scanned file, then `fs.remove_file` (skipped when `dry_run`). For `Dir`: reuse the existing `scan(fs, root, account_bytes)` and `record(ledger, target, files)` free functions (make `scan` and `record` `pub(super)`), then `fs.remove_tree` (skipped when `dry_run`). Keep exactly the error handling of `Runner::remove_measured` (`src/gc/run.rs`): a scan error is `RemoveFailed::Scan`, a removal error is `RemoveFailed::Remove` after `ledger.unmeasured(..)` for a part-way removal, `NotFound` counts nothing. Make `Runner::remove_measured` a thin wrapper that maps the result onto its existing `skip(..)` calls and `bool`, with byte-identical counts, findings and `unmeasured`. Move the volume cross-check block of `gc_cmd` into `cross_check_notes` with the same text: `cross-check, not a count: available bytes in APFS container {c} (mounted at {m}) changed by {change:+} during this run`.

Run: `cargo test` — expect: pass, including all of `tests/gc.rs` unchanged (dry-run parity included).

- [x] **Step 3: Commit**

`git commit -m "gc: extract the counted delete and the volume cross-check for reuse"`

_Requirements: GCPLAN-9.1_

---

### Task 3: A path guard that checks an entry, then deletes by kind

**Files:**
- Create: `src/plan_tier/mod.rs` (module header and `pub mod guard;`)
- Create: `src/plan_tier/guard.rs`
- Modify: `src/lib.rs` (`pub mod plan_tier;`)
- Modify: `tests/common/prove.rs` (`delete_in_clone`)
- Test: `src/plan_tier/guard.rs` (`#[cfg(test)]`), existing `tests/liveness.rs`

**Reuse:** existing — `delete_in_clone` in `tests/common/prove.rs` (canonical parent under root, lstat, `remove_tree` for a directory, `remove_file` otherwise) (rung 2).

**Interfaces:**
- Consumes: `Fs::{entry_stat, remove_tree, remove_file}`, `EntryKind`, `EntryStat`
- Produces: `pub enum Expect { Entry { kind: EntryKind, dev: u64, ino: u64 }, Any }`; `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub enum Deleted { Removed, Missing, Changed, Outside }`; `pub fn check_entry(fs: &dyn Fs, root: &Path, path: &Path, expect: Expect) -> Result<EntryStat, Deleted>` (never mutates; `Err` carries `Missing`, `Changed` or `Outside`); `pub fn delete_checked(fs: &dyn Fs, root: &Path, path: &Path, expect: Expect) -> Deleted`

**Depends-on:** none

- [x] **Step 1: Write the failing test** in `src/plan_tier/guard.rs`

```rust
#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::platform::{self, Fs};
    use std::os::unix::fs::symlink;
    use std::path::PathBuf;

    fn root(label: &str) -> PathBuf {
        let d = PathBuf::from(format!("/private/tmp/hive-it-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("target/deps")).unwrap();
        d
    }

    #[test]
    fn removes_a_file_a_directory_and_a_symlink_by_kind_and_reports_missing() {
        let p = platform::current().unwrap();
        let fs: &dyn Fs = p.fs.as_ref();
        let d = root("guard-kinds");
        let t = d.join("target");
        std::fs::write(t.join("deps/a.rlib"), b"x").unwrap();
        std::fs::create_dir_all(t.join("deps/unit")).unwrap();
        std::fs::write(t.join("deps/unit/f"), b"x").unwrap();
        symlink("/etc", t.join("deps/link")).unwrap();
        for name in ["a.rlib", "unit", "link"] {
            assert_eq!(delete_checked(fs, &t, &t.join("deps").join(name), Expect::Any), Deleted::Removed, "{name}");
        }
        assert!(std::path::Path::new("/etc").exists(), "the link target must survive");
        assert_eq!(delete_checked(fs, &t, &t.join("deps/a.rlib"), Expect::Any), Deleted::Missing);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn refuses_a_path_outside_the_target_or_reached_through_a_symlinked_parent() {
        let p = platform::current().unwrap();
        let fs: &dyn Fs = p.fs.as_ref();
        let d = root("guard-outside");
        let t = d.join("target");
        let outside = d.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("keep"), b"x").unwrap();
        assert_eq!(delete_checked(fs, &t, &outside.join("keep"), Expect::Any), Deleted::Outside);
        symlink(&outside, t.join("deps/escape")).unwrap();
        assert_eq!(delete_checked(fs, &t, &t.join("deps/escape/keep"), Expect::Any), Deleted::Outside);
        assert!(outside.join("keep").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn refuses_an_entry_whose_identity_differs_and_check_entry_never_mutates() {
        let p = platform::current().unwrap();
        let fs: &dyn Fs = p.fs.as_ref();
        let d = root("guard-changed");
        let t = d.join("target");
        let f = t.join("deps/a.rlib");
        std::fs::write(&f, b"x").unwrap();
        let st = fs.entry_stat(&f).unwrap();
        let wrong = Expect::Entry { kind: st.kind, dev: st.dev, ino: st.ino + 1 };
        assert_eq!(delete_checked(fs, &t, &f, wrong), Deleted::Changed);
        assert!(f.exists());
        let right = Expect::Entry { kind: st.kind, dev: st.dev, ino: st.ino };
        assert!(check_entry(fs, &t, &f, right).is_ok());
        assert!(f.exists(), "check_entry must not remove");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_whole_target_dir_is_checked_against_its_parent() {
        let p = platform::current().unwrap();
        let fs: &dyn Fs = p.fs.as_ref();
        let d = root("guard-whole");
        let t = d.join("target");
        assert!(check_entry(fs, &d, &t, Expect::Any).is_ok());
        assert_eq!(check_entry(fs, &t, &t, Expect::Any).unwrap_err(), Deleted::Outside);
        let _ = std::fs::remove_dir_all(&d);
    }
}
```

Run: `cargo test -p cargo-hive guard` — expect: compile error, module `plan_tier::guard` not found.

- [x] **Step 2: Implement**

`check_entry`: canonicalize the path's parent with `std::fs::canonicalize` (an `ErrorKind::NotFound` → `Err(Missing)`, any other error → `Err(Outside)`); require the canonical parent to start with the canonicalized `root`, else `Err(Outside)`; `entry_stat(path)` (lstat) — not found → `Err(Missing)`; with `Expect::Entry`, any difference in kind, device or inode → `Err(Changed)`; an `Other` kind → `Err(Changed)`; else `Ok(stat)`. `delete_checked`: `check_entry`, then `Dir` → `fs.remove_tree`, `File`/`Symlink` → `fs.remove_file`; a removal error → `Missing` when its kind is `NotFound`, else `Changed`; `Removed` otherwise. Nothing follows a link. Give `src/plan_tier/mod.rs` a `//!` header saying what the module is.

- [x] **Step 3: Move the proof's guard onto it**

In `tests/common/prove.rs` change `delete_in_clone` to call `cargo_hive::plan_tier::guard::delete_checked(fs, clone_root, path, Expect::Any)` for each file, treating `Missing` as it does today, and mapping `Outside` to `format!("{} reaches outside the clone through a link", path.display())` and `Changed` to `format!("cannot delete {}", path.display())` — the substrings `tests/liveness.rs` asserts for `debug/deps` and the cannot-delete case must still match.

Run: `cargo test` — expect: pass.

- [x] **Step 4: Commit**

`git commit -m "plan_tier: a path guard that checks an entry and deletes by kind inside one target dir"`

_Requirements: GCPLAN-8.1_

---

### Task 4: The GC window is a validated config value

**Files:**
- Modify: `src/state.rs` (`ConfigFile`, `Config`, `window_days`)
- Test: `src/state.rs` (`#[cfg(test)]`)

**Reuse:** extends — `ConfigFile`/`Config`/`State::config()` (`src/state.rs`) (rung 2).

**Interfaces:**
- Consumes: `config.json`
- Produces: `Config::gc_window_days: Option<u64>` (raw); `pub fn window_days(raw: Option<u64>) -> Result<u64, WindowError>` (`None` → 7; 1 to 365 inclusive; else `WindowError(raw)`); `WindowError` with a hand-written `Display`

**Depends-on:** none

- [x] **Step 1: Write the failing test** in the existing `#[cfg(test)]` module of `src/state.rs`, which has `fn scratch(label) -> PathBuf` (returns an uncreated path under the OS temp dir)

```rust
#[test]
fn the_window_defaults_to_seven_days_and_accepts_one_to_365() {
    assert_eq!(window_days(None).unwrap(), 7);
    assert_eq!(window_days(Some(1)).unwrap(), 1);
    assert_eq!(window_days(Some(365)).unwrap(), 365);
    for bad in [0u64, 366, 10_000] {
        assert!(window_days(Some(bad)).is_err(), "{bad}");
    }
}

#[test]
fn config_reads_the_window_key_and_still_refuses_unknown_keys() {
    let root = scratch("window-key");
    let state = State::open_at(root.clone()).unwrap();
    std::fs::write(root.join("config.json"), r#"{"gc_window_days": 30}"#).unwrap();
    assert_eq!(state.config().unwrap().gc_window_days, Some(30));
    std::fs::write(root.join("config.json"), r#"{"gc_window": 30}"#).unwrap();
    assert!(state.config().is_err());
}

#[test]
fn an_out_of_range_window_does_not_fail_config_for_commands_that_ignore_it() {
    let root = scratch("window-ignored");
    let state = State::open_at(root.clone()).unwrap();
    std::fs::write(root.join("config.json"), r#"{"gc_window_days": 0}"#).unwrap();
    assert!(state.config().is_ok());
}
```

Run: `cargo test -p cargo-hive state::` — expect: compile error, no field `gc_window_days`.

- [x] **Step 2: Implement**

Add `gc_window_days: Option<u64>` to `ConfigFile` (still `deny_unknown_fields`) and to `Config`, copied through `State::config()` without validation. Add `window_days` and `WindowError`.

Run: `cargo test` — expect: pass.

- [x] **Step 3: Commit**

`git commit -m "state: a gc_window_days config key, validated where plans use it"`

_Requirements: GCPLAN-1.8, GCPLAN-1.9, GCPLAN-9.7_

---

### Task 5: Plans are stored, found and consumed once

**Files:**
- Create: `src/plan_tier/record.rs`
- Modify: `src/plan_tier/mod.rs` (`pub mod record;`)
- Test: `src/plan_tier/record.rs` (`#[cfg(test)]`)

**Reuse:** existing — `State::locked_or_cancelled`, `Tx::{read, write}` (`src/state.rs`) (rung 2); the plan types are new (rung 7).

**Interfaces:**
- Consumes: `State`, `Tx`, `StateError`
- Produces: `PlanClass { Secondary, Protected, Orphan }` (serde lowercase); `BytesRec { freed: u64, still_shared: u64 }`; `PlannedEntry { path: PathBuf, unit: String, kind: String, dev: u64, ino: u64 }` with `kind` one of `file`, `dir`, `symlink`, `other` via `pub fn kind_name(EntryKind) -> &'static str` and `pub fn kind_of(&str) -> Option<EntryKind>`; `OrphanScope { root_kind: String, root_dev: u64, root_ino: u64, dirs: Vec<PathBuf> }`; `PlannedTarget { path: PathBuf, owner: Option<PathBuf>, planned: Option<BytesRec>, entries: Vec<PlannedEntry>, orphan: Option<OrphanScope> }`; `PlanState { Live, Consumed { at: u64 } }`; `Plan { id: String, class: PlanClass, window_days: u64, made_at: u64, scope_repos: Vec<PathBuf>, planned: Option<BytesRec>, targets: Vec<PlannedTarget>, state: PlanState }`; `pub fn insert(state: &State, cancel: &dyn Fn() -> bool, plans: Vec<Plan>, now: u64) -> Result<(), StateError>` (prunes first; a duplicate id is `StateError::Malformed`); `pub enum Lookup { Live(Plan), Consumed, Unknown }`; `pub fn get(state: &State, id: &str) -> Result<Lookup, StateError>`; `pub enum Taken { Plan(Plan), AlreadyConsumed, Unknown }`; `pub fn consume(state: &State, cancel: &dyn Fn() -> bool, id: &str, now: u64) -> Result<Taken, StateError>`; `pub fn new_id(now_nanos: u128, pid: u32, counter: u64) -> String` (16 lowercase hex)

**Depends-on:** Task 3

- [x] **Step 1: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn state(label: &str) -> State {
        let d = std::path::PathBuf::from(format!("/private/tmp/hive-it-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        State::open_at(d).unwrap()
    }

    fn plan(id: &str, class: PlanClass, now: u64) -> Plan {
        Plan {
            id: id.into(), class, window_days: 7, made_at: now, scope_repos: vec![],
            planned: Some(BytesRec { freed: 10, still_shared: 0 }),
            targets: vec![PlannedTarget {
                path: "/t".into(), owner: Some("/o".into()), planned: None,
                entries: vec![PlannedEntry { path: "/t/a".into(), unit: "debug/x-1".into(), kind: "file".into(), dev: 1, ino: 2 }],
                orphan: None,
            }],
            state: PlanState::Live,
        }
    }

    #[test]
    fn a_stored_plan_is_found_and_consumed_exactly_once() {
        let s = state("record-once");
        insert(&s, &|| false, vec![plan("aaaaaaaaaaaaaaaa", PlanClass::Secondary, 100)], 100).unwrap();
        assert!(matches!(get(&s, "aaaaaaaaaaaaaaaa").unwrap(), Lookup::Live(_)));
        let first = consume(&s, &|| false, "aaaaaaaaaaaaaaaa", 101).unwrap();
        assert!(matches!(first, Taken::Plan(p) if p.targets.len() == 1));
        assert!(matches!(consume(&s, &|| false, "aaaaaaaaaaaaaaaa", 102).unwrap(), Taken::AlreadyConsumed));
        assert!(matches!(get(&s, "aaaaaaaaaaaaaaaa").unwrap(), Lookup::Consumed));
        assert!(matches!(get(&s, "bbbbbbbbbbbbbbbb").unwrap(), Lookup::Unknown));
    }

    #[test]
    fn a_consumed_plan_keeps_a_tombstone_without_its_paths() {
        let s = state("record-tombstone");
        insert(&s, &|| false, vec![plan("aaaaaaaaaaaaaaaa", PlanClass::Protected, 100)], 100).unwrap();
        consume(&s, &|| false, "aaaaaaaaaaaaaaaa", 101).unwrap();
        let text = std::fs::read_to_string(s.root().join("plans.json")).unwrap();
        assert!(text.contains("aaaaaaaaaaaaaaaa") && !text.contains("/t/a"), "{text}");
    }

    #[test]
    fn prune_drops_old_entries_and_extra_tombstones_but_never_a_young_live_plan() {
        let s = state("record-prune");
        let day = 86_400;
        let mut plans = vec![plan("0000000000000000", PlanClass::Secondary, 0)]; // 60 days old, live
        for n in 1..=300u64 {
            plans.push(plan(&format!("{n:016x}"), PlanClass::Secondary, 60 * day));
        }
        insert(&s, &|| false, plans, 60 * day).unwrap();
        for n in 1..=300u64 {
            consume(&s, &|| false, &format!("{n:016x}"), 60 * day).unwrap();
        }
        insert(&s, &|| false, vec![plan("ffffffffffffffff", PlanClass::Secondary, 60 * day)], 60 * day).unwrap();
        assert!(matches!(get(&s, "0000000000000000").unwrap(), Lookup::Unknown), "a 60-day-old live plan is dropped");
        assert!(matches!(get(&s, "ffffffffffffffff").unwrap(), Lookup::Live(_)));
        let kept = (1..=300u64).filter(|n| matches!(get(&s, &format!("{n:016x}")).unwrap(), Lookup::Consumed)).count();
        assert_eq!(kept, 256);
    }

    #[test]
    fn a_cancelled_write_leaves_the_file_as_it_was() {
        let s = state("record-cancelled");
        insert(&s, &|| false, vec![plan("aaaaaaaaaaaaaaaa", PlanClass::Secondary, 100)], 100).unwrap();
        let before = std::fs::read_to_string(s.root().join("plans.json")).unwrap();
        // another holder of the state lock, then a cancelled insert: the lock is contended, so cancel is honoured
        let r = s.locked(|_| Ok(insert(&s, &|| true, vec![plan("bbbbbbbbbbbbbbbb", PlanClass::Secondary, 100)], 100)));
        assert!(matches!(r.unwrap(), Err(StateError::Cancelled)));
        assert_eq!(std::fs::read_to_string(s.root().join("plans.json")).unwrap(), before);
    }

    #[test]
    fn a_duplicate_id_is_refused() {
        let s = state("record-duplicate");
        insert(&s, &|| false, vec![plan("aaaaaaaaaaaaaaaa", PlanClass::Secondary, 100)], 100).unwrap();
        let r = insert(&s, &|| false, vec![plan("aaaaaaaaaaaaaaaa", PlanClass::Secondary, 100)], 100);
        assert!(matches!(r, Err(StateError::Malformed { .. })));
    }

    #[test]
    fn ids_are_sixteen_lowercase_hex_characters_and_entry_kinds_round_trip() {
        let id = new_id(1_700_000_000_123_456_789, 4242, 7);
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
        assert_ne!(id, new_id(1_700_000_000_123_456_789, 4242, 8));
        for k in ["file", "dir", "symlink", "other"] {
            assert_eq!(kind_name(kind_of(k).unwrap()), k);
        }
    }
}
```

Run: `cargo test -p cargo-hive record::` — expect: compile error, `plan_tier::record` not found.

- [x] **Step 2: Implement**

`plans.json` is `{ "plans": [Plan, ...] }` with a `Default` wrapper type; read and write it only through `Tx` inside `State::locked_or_cancelled(cancel, ...)`. `insert`: prune first, refuse a duplicate id with `StateError::Malformed { file: "plans.json".into(), detail }`, then append. `consume`: in one locked section find the plan; `Live` → replace `targets` with an empty vec, set `state = Consumed { at: now }`, write, return `Taken::Plan(copy_before_stripping)`; `Consumed` → `AlreadyConsumed`; missing → `Unknown`. `prune(now)`: drop any entry with `made_at` older than 30 days; then keep only the newest 256 consumed tombstones; never drop a `Live` plan younger than 30 days. `new_id` mixes its three inputs (FNV-1a over their bytes) into 16 hex characters; callers retry with a new counter if `insert` reports a duplicate.

Run: `cargo test` — expect: pass.

- [x] **Step 3: Commit**

`git commit -m "plan_tier: store plans in state and consume each once"`

_Requirements: GCPLAN-1.7, GCPLAN-1.14, GCPLAN-2.11, GCPLAN-3.7, GCPLAN-3.9, GCPLAN-8.4_

---

### Task 6: Plan and apply receipts carry schema 3

**Files:**
- Modify: `src/receipt.rs`
- Test: `src/receipt.rs` (`#[cfg(test)]`); `tests/receipts.rs` stays unchanged and must pass

**Reuse:** extends — `Receipt`, `ReceiptBuilder` (`src/receipt.rs`) (rung 2).

**Interfaces:**
- Consumes: the existing builder
- Produces: `Outcome { Applied, Partial, Refused, NeedsHuman }` (serde kebab-case: `applied`, `partial`, `refused`, `needs-human`; derive `Debug, Clone, Copy, PartialEq, Eq`); `PlanRecord { id: String, class: String, outcome: Option<Outcome>, reason: Option<String>, targets: Vec<PathBuf>, paths: u64, counts: BTreeMap<String, u64> }`; `ReasonRecord { what: String, clears_when: String }`; `UndecidedRecord { target: PathBuf, reasons: Vec<ReasonRecord> }`; `PlanTierBody { plans: Vec<PlanRecord>, undecided: Vec<UndecidedRecord>, nothing_to_do: Vec<PathBuf> }`; `ReceiptBuilder::plan_tier(&mut self, body: PlanTierBody)`; `Receipt` fields `plans`, `undecided`, `nothing_to_do` as `Option<Vec<_>>` with `#[serde(default, skip_serializing_if = "Option::is_none")]`

**Depends-on:** none

- [x] **Step 1: Write the failing test**

```rust
#[test]
fn a_plan_tier_receipt_is_schema_three_and_carries_its_groups() {
    let mut b = Receipt::with_os("gc --plan", Scope::default(), Some("27.0.1".into()));
    b.plan_tier(PlanTierBody { plans: vec![], undecided: vec![], nothing_to_do: vec![] });
    let v = serde_json::to_value(b.finish()).unwrap();
    assert_eq!(v["schema"], 3);
    for key in ["plans", "undecided", "nothing_to_do"] {
        assert!(v[key].as_array().unwrap().is_empty(), "{key}");
    }
}

#[test]
fn other_receipts_keep_schema_two_and_omit_the_plan_keys() {
    let v = serde_json::to_value(Receipt::with_os("status", Scope::default(), None).finish()).unwrap();
    assert_eq!(v["schema"], 2);
    for key in ["plans", "undecided", "nothing_to_do"] {
        assert!(v.get(key).is_none(), "{key}");
    }
}

#[test]
fn outcomes_serialize_by_their_names_and_old_log_lines_still_parse() {
    assert_eq!(serde_json::to_string(&Outcome::NeedsHuman).unwrap(), "\"needs-human\"");
    let old = serde_json::to_string(&Receipt::with_os("status", Scope::default(), None).finish()).unwrap();
    let parsed: Receipt = serde_json::from_str(&old).unwrap();
    assert!(parsed.plans.is_none());
}

#[test]
fn a_populated_plan_tier_receipt_names_no_time_or_estimate() {
    let mut counts = std::collections::BTreeMap::new();
    counts.insert("paths_deleted".to_string(), 3u64);
    counts.insert("freed_bytes".to_string(), 4096u64);
    let mut b = Receipt::with_os("apply", Scope::default(), None);
    b.plan_tier(PlanTierBody {
        plans: vec![PlanRecord {
            id: "0123456789abcdef".into(), class: "secondary".into(), outcome: Some(Outcome::Applied),
            reason: None, targets: vec!["/t".into()], paths: 3, counts,
        }],
        undecided: vec![UndecidedRecord { target: "/u".into(), reasons: vec![ReasonRecord { what: "w".into(), clears_when: "c".into() }] }],
        nothing_to_do: vec!["/n".into()],
    });
    let text = serde_json::to_string(&b.finish()).unwrap();
    for word in ["saved", "duration", "seconds", "elapsed", "estimate"] {
        assert!(!text.contains(word), "{word}");
    }
}
```

Run: `cargo test -p cargo-hive receipt::` — expect: compile error, no `plan_tier` on `ReceiptBuilder`.

- [x] **Step 2: Implement**

Add the three optional fields to `Receipt` (initialised to `None` in `with_os`) and the types above. `plan_tier` sets `schema = 3` and `Some(...)` on all three fields, so an empty body still prints `"plans": []`. Do not touch `SCHEMA` (`2`); update its doc comment to say plan and apply receipts are version 3. `tests/receipts.rs` is not edited.

Run: `cargo test` — expect: pass, including the unchanged `tests/receipts.rs`.

- [x] **Step 3: Commit**

`git commit -m "receipt: schema 3 body for plan and apply, schema 2 elsewhere"`

_Requirements: GCPLAN-6.1, GCPLAN-9.2, GCPLAN-9.3_

---

### Task 7: `cargo hive gc --plan` stores plans and deletes nothing

**Files:**
- Create: `src/plan_tier/build.rs`
- Modify: `src/plan_tier/mod.rs` (`pub mod build;`, `Ctx`)
- Modify: `src/cli.rs` (`Gc { plan }`, `plan_cmd`)
- Modify: `tests/common/mod.rs` (`pub mod plan_fixture;`, `Hive::json_env`)
- Create: `tests/common/plan_fixture.rs`
- Modify: `tests/skill.rs` (`Parsed::Gc { dry_run, plan }`)
- Modify: `CONTEXT.md` (Plan, Plan class, Outcome)
- Test: `tests/plan_apply.rs` (create), `src/plan_tier/build.rs` (`#[cfg(test)]`)

**Reuse:** existing — `discover::checkouts` and `discover::targets(.., &mut Snapshot::default(), ..)`, `liveness::liveness`, `gc::measure`, `Fs::entry_stat`, `select`/`live_checkouts`/`emit`/`doctor::require` in `src/cli.rs` (rung 2); the plan builder and the class-of-target lookup are new (rung 7).

**Interfaces:**
- Consumes: `record::{Plan, PlannedTarget, PlannedEntry, kind_name, insert, new_id}`, `state::window_days`, `receipt::{PlanTierBody, PlanRecord, UndecidedRecord}`, `Liveness`
- Produces: `pub struct Ctx<'a> { pub state: &'a State, pub fs: &'a dyn Fs, pub cfg: &'a Config, pub now: SystemTime, pub account_bytes: bool, pub repos: &'a [Repo], pub mains: &'a [PathBuf] }`; `pub enum TargetClass { Class(PlanClass), Unresolved }`; `pub fn class_of_target(owner: &Owner, checkouts: &[Checkout]) -> TargetClass`; `pub enum TargetPlan { Planned(PlannedTarget), NothingToDo, Undecided(Vec<Uncertain>) }`; `pub fn plan_for_target(ctx: &Ctx, target: &TargetDir, window: Duration) -> TargetPlan`; `Plan::single(class: PlanClass, target: PlannedTarget, window_days: u64, now: u64) -> Plan`; `pub struct Made { pub plans: Vec<Plan>, pub undecided: Vec<UndecidedRecord>, pub nothing_to_do: Vec<PathBuf>, pub unmeasured: Vec<Unmeasured>, pub findings: Vec<(String, Option<PathBuf>, String)> }`; `pub fn make(ctx: &Ctx, scope_repos: &[PathBuf], cancel: &(dyn Fn() -> bool + Sync)) -> Result<Made, MakeError>`; CLI `cargo hive gc --plan [repos…] [--json]`; `Hive::json_env(&self, args: &[&str], env: &[(&str, &str)]) -> (Output, serde_json::Value)`; `tests::common::plan_fixture::{Planned, tree}` with `Planned::{new, with_dead_units(label, n), with_worktrees(label, n), main_target, worktree_target, plan, plan_for, id_of, dead_unit_hash, plans_json}`

**Depends-on:** Task 2, Task 4, Task 5, Task 6

- [x] **Step 1: Write the fixture and the failing tests**

`tests/common/plan_fixture.rs`:

```rust
use super::cargo_fixture::{backdate_all, set_mtime};
use super::{git, init_repo, register, Hive, Scratch};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// A registered main checkout and linked worktrees under `/private/tmp` (a default
/// secondary root, so hive classes the worktrees Secondary and the main Protected; never
/// set `secondary_roots` in tests that rely on this). Each checkout has a built target whose
/// earlier builds are older than the 7-day window and were superseded by rebuilds under new
/// package versions, so every target holds dead units.
pub struct Planned {
    pub hive: Hive,
    pub scratch: Scratch,
    pub main: PathBuf,
    pub worktree: PathBuf,
    pub more_worktrees: Vec<PathBuf>,
}

impl Planned {
    pub fn new(label: &str) -> Planned {
        Planned::build(label, 1, 1)
    }

    /// The first worktree is superseded `n` times, so it holds `n` dead units.
    pub fn with_dead_units(label: &str, n: usize) -> Planned {
        Planned::build(label, n, 1)
    }

    /// `n` linked worktrees, all Secondary.
    pub fn with_worktrees(label: &str, n: usize) -> Planned {
        Planned::build(label, 1, n)
    }

    fn build(label: &str, supersessions: usize, worktrees: usize) -> Planned {
        let scratch = Scratch::new(label);
        let hive = Hive::ready();
        let main = init_repo(&scratch.path.join("main"), "");
        register(&hive, &main);
        let mut made = Vec::new();
        for i in 0..worktrees {
            let wt = scratch.path.join(format!("wt{i}"));
            git(&main, &["worktree", "add", "-q", "-b", &format!("wt{i}"), wt.to_str().unwrap()]);
            made.push(wt);
        }
        let p = Planned { hive, scratch, main, worktree: made[0].clone(), more_worktrees: made[1..].to_vec() };
        p.supersede(&p.main, 1);
        p.supersede(&p.worktree, supersessions);
        for wt in &p.more_worktrees {
            p.supersede(wt, 1);
        }
        p
    }

    /// Builds `dir`, then `times` more versions, backdating every earlier build past the window.
    fn supersede(&self, dir: &Path, times: usize) {
        self.hive.build_crate(dir, None);
        let manifest = dir.join("Cargo.toml");
        for n in 1..=times {
            backdate_all(&dir.join("target"), Duration::from_secs(10 * 86_400));
            let text = std::fs::read_to_string(&manifest).unwrap();
            let next = text.replace(&format!("0.1.{}", n - 1), &format!("0.1.{n}"));
            std::fs::write(&manifest, next).unwrap();
            self.hive.build_crate(dir, None);
        }
    }

    pub fn main_target(&self) -> PathBuf { self.main.join("target") }
    pub fn worktree_target(&self) -> PathBuf { self.worktree.join("target") }

    /// `gc --plan` over every registered repo; returns the receipt.
    pub fn plan(&self) -> serde_json::Value {
        self.plan_for(&[])
    }

    pub fn plan_for(&self, repos: &[&str]) -> serde_json::Value {
        let mut args = vec!["gc", "--plan"];
        args.extend_from_slice(repos);
        let (out, r) = self.hive.json(&args);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        r
    }

    /// The id of the plan of `class` in a `gc --plan` or `apply` receipt.
    pub fn id_of(receipt: &serde_json::Value, class: &str) -> String {
        receipt["plans"].as_array().unwrap().iter()
            .find(|p| p["class"] == class)
            .unwrap_or_else(|| panic!("no {class} plan in {receipt}"))["id"]
            .as_str().unwrap().to_string()
    }

    /// The hash tail of the `.fingerprint/fixture-*` unit whose `invoked.timestamp` is oldest
    /// in `target/debug`: the dead unit.
    pub fn dead_unit_hash(&self, target: &Path) -> String {
        let fp = target.join("debug/.fingerprint");
        let mut best: Option<(SystemTime, String)> = None;
        for e in std::fs::read_dir(&fp).unwrap() {
            let dir = e.unwrap().path();
            let name = dir.file_name().unwrap().to_str().unwrap().to_string();
            if !name.starts_with("fixture-") { continue; }
            let t = std::fs::metadata(dir.join("invoked.timestamp")).unwrap().modified().unwrap();
            if best.as_ref().is_none_or(|(b, _)| t < *b) {
                best = Some((t, name.rsplit('-').next().unwrap().to_string()));
            }
        }
        best.expect("a fixture unit").1
    }

    /// The stored plans file under `HIVE_HOME`.
    pub fn plans_json(&self) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(self.hive.home.path.join("plans.json")).unwrap()).unwrap()
    }

    /// Makes the dead unit `hash` of `target` live again by touching its stamp.
    pub fn revive(&self, target: &Path, hash: &str) {
        let stamp = target.join(format!("debug/.fingerprint/fixture-{hash}/invoked.timestamp"));
        set_mtime(&stamp, SystemTime::now());
    }
}

/// Every path under `root` with its length and mtime, not following symlinks.
pub fn tree(root: &Path) -> BTreeMap<PathBuf, (u64, SystemTime)> {
    fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, (u64, SystemTime)>, depth: usize) {
        assert!(depth < 64, "walk bound hit");
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let md = std::fs::symlink_metadata(entry.path()).unwrap();
            out.insert(entry.path(), (md.len(), md.modified().unwrap()));
            if md.is_dir() { walk(&entry.path(), out, depth + 1); }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, &mut out, 0);
    out
}
```

Add `pub mod plan_fixture;` after `pub mod cargo_fixture;` in `tests/common/mod.rs`, and to `Hive`:

```rust
    /// Like `json`, with extra environment variables on the command.
    pub fn json_env(&self, args: &[&str], env: &[(&str, &str)]) -> (Output, serde_json::Value) {
        let mut all = args.to_vec();
        all.push("--json");
        let mut cmd = self.command(&all);
        for (k, v) in env { cmd.env(k, v); }
        let out = cmd.output().unwrap();
        let value = serde_json::from_slice(&out.stdout).unwrap_or_else(|e| panic!("not one receipt ({e}): {}", String::from_utf8_lossy(&out.stdout)));
        (out, value)
    }
```

`tests/plan_apply.rs`:

```rust
#![cfg(target_os = "macos")]
mod common;
use common::plan_fixture::{tree, Planned};
use common::{init_repo, register, Hive, Scratch};

fn write_caps(p: &Planned, disabled: serde_json::Value) {
    let release = cargo_hive::doctor::resolve_toolchain(&p.hive.home.path).unwrap().release;
    let caps = serde_json::json!({"toolchain_release": release, "disabled": disabled, "lock_check": 1, "failed_marker_check": 1});
    std::fs::write(p.hive.home.path.join("capabilities.json"), caps.to_string()).unwrap();
}

#[test]
fn gc_plan_stores_one_plan_per_class_and_deletes_nothing() {
    let p = Planned::new("plan-per-class");
    let before = (tree(&p.main_target()), tree(&p.worktree_target()));
    let snapshot = std::fs::read(p.hive.home.path.join("snapshot.json")).ok();
    let r = p.plan();
    assert_eq!(r["schema"], 3);
    let plans = r["plans"].as_array().unwrap();
    let mut classes: Vec<&str> = plans.iter().map(|x| x["class"].as_str().unwrap()).collect();
    classes.sort();
    assert_eq!(classes, ["protected", "secondary"]);
    for plan in plans {
        assert_eq!(plan["id"].as_str().unwrap().len(), 16);
        assert!(plan["outcome"].is_null());
        assert!(plan["paths"].as_u64().unwrap() > 0, "{plan}");
        assert!(plan["counts"]["freed_bytes"].as_u64().is_some(), "planned bytes are in the receipt: {plan}");
        assert_eq!(plan["targets"].as_array().unwrap().len(), 1);
    }
    assert_eq!((tree(&p.main_target()), tree(&p.worktree_target())), before, "gc --plan touched a file");
    assert_eq!(std::fs::read(p.hive.home.path.join("snapshot.json")).ok(), snapshot, "plan tier must not touch the snapshot cache");
}

#[test]
fn a_stored_plan_records_what_apply_must_compare_against() {
    let p = Planned::new("plan-recorded");
    let r = p.plan();
    let id = Planned::id_of(&r, "secondary");
    let stored = p.plans_json();
    let plan = stored["plans"].as_array().unwrap().iter().find(|x| x["id"] == id.as_str()).unwrap();
    assert_eq!(plan["window_days"], 7);
    assert_eq!(plan["class"], "secondary");
    assert!(plan["scope_repos"].is_array());
    let target = &plan["targets"][0];
    assert_eq!(target["path"].as_str().unwrap(), p.worktree_target().canonicalize().unwrap().to_str().unwrap());
    assert!(target["owner"].as_str().is_some());
    let entry = &target["entries"][0];
    for key in ["path", "unit", "kind", "dev", "ino"] {
        assert!(!entry[key].is_null(), "entry lacks {key}: {entry}");
    }
    assert!(plan["planned"]["freed"].as_u64().is_some());
}

#[test]
fn a_target_with_no_dead_unit_is_reported_as_nothing_to_do() {
    let hive = Hive::ready();
    let s = Scratch::new("plan-nothing");
    let repo = init_repo(&s.path.join("main"), "");
    register(&hive, &repo);
    hive.build_crate(&repo, None);
    let (out, r) = hive.json(&["gc", "--plan"]);
    assert!(out.status.success());
    assert_eq!(r["plans"].as_array().unwrap().len(), 0);
    let nothing: Vec<&str> = r["nothing_to_do"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(nothing.len(), 1, "{r}");
    assert!(nothing[0].ends_with("/target"));
}

#[test]
fn a_target_the_graph_cannot_decide_is_reported_undecided_with_what_clears_it() {
    let hive = Hive::ready();
    let s = Scratch::new("plan-undecided");
    let repo = init_repo(&s.path.join("main"), "");
    register(&hive, &repo);
    hive.build_crate(&repo, None);
    // a unit directory Cargo planned but never compiled, inside the window: stamp, no hash file
    let unit = repo.join("target/debug/.fingerprint/zz-0123456789abcdef");
    std::fs::create_dir_all(&unit).unwrap();
    std::fs::write(unit.join("invoked.timestamp"), b"").unwrap();
    std::fs::write(unit.join("output-lib-zz"), b"").unwrap();
    let (_, r) = hive.json(&["gc", "--plan"]);
    assert_eq!(r["plans"].as_array().unwrap().len(), 0, "{r}");
    let u = &r["undecided"][0];
    assert!(u["target"].as_str().unwrap().ends_with("/target"));
    assert!(!u["reasons"][0]["clears_when"].as_str().unwrap().is_empty(), "{r}");
}

#[test]
fn plan_refuses_a_window_out_of_range_and_has_no_window_option() {
    let p = Planned::new("plan-window");
    p.hive.config(r#"{"gc_window_days": 0}"#);
    let (out, _) = p.hive.json(&["gc", "--plan"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(!p.hive.run(&["gc", "--plan", "--window", "3"]).status.success(), "an option set the window");
}

#[test]
fn a_target_shared_with_a_protected_checkout_goes_in_the_protected_plan() {
    let p = Planned::new("plan-shared");
    // the worktree is configured to build into the main checkout's target dir
    std::fs::create_dir_all(p.worktree.join(".cargo")).unwrap();
    std::fs::write(p.worktree.join(".cargo/config.toml"), format!("[build]\ntarget-dir = \"{}\"\n", p.main_target().display())).unwrap();
    p.hive.build_crate(&p.worktree, None);
    let r = p.plan();
    let main_target = p.main_target().canonicalize().unwrap();
    let in_plan = |class: &str| r["plans"].as_array().unwrap().iter().filter(|x| x["class"] == class)
        .any(|x| x["targets"].as_array().unwrap().iter().any(|t| t.as_str().unwrap() == main_target.to_str().unwrap()));
    assert!(in_plan("protected"), "{r}");
    assert!(!in_plan("secondary"), "{r}");
}

#[test]
fn gc_plan_conflicts_with_dry_run_and_gc_without_plan_is_unchanged() {
    let p = Planned::new("plan-conflict");
    assert!(!p.hive.run(&["gc", "--plan", "--dry-run"]).status.success());
    let (_, r) = p.hive.json(&["gc", "--dry-run"]);
    assert_eq!(r["schema"], 2);
    assert!(r.get("plans").is_none());
}

#[test]
fn gc_plan_needs_only_the_fingerprint_capability() {
    let p = Planned::new("plan-capability");
    write_caps(&p, serde_json::json!({"fingerprint_reading": ["f"]}));
    assert_eq!(p.hive.json(&["gc", "--plan"]).0.status.code(), Some(1));
    write_caps(&p, serde_json::json!({"deletion": ["d"]}));
    assert_eq!(p.hive.json(&["gc", "--plan"]).0.status.code(), Some(0));
}
```

#[test]
fn without_the_accounting_capability_plans_are_made_and_the_bytes_are_unmeasured() {
    let p = Planned::new("plan-unmeasured");
    write_caps(&p, serde_json::json!({"eviction_accounting": ["c"]}));
    let (out, r) = p.hive.json(&["gc", "--plan"]);
    assert_eq!(out.status.code(), Some(0));
    let plans = r["plans"].as_array().unwrap();
    assert!(!plans.is_empty());
    for plan in plans {
        assert!(plan["counts"].get("freed_bytes").is_none(), "{plan}");
    }
    assert!(!r["unmeasured"].as_array().unwrap().is_empty(), "{r}");
}

#[test]
fn a_dead_unit_that_owns_no_artifact_is_never_planned_for_its_lone_fingerprint_dir() {
    let p = Planned::new("plan-no-artifact");
    let hash = p.dead_unit_hash(&p.worktree_target());
    for e in std::fs::read_dir(p.worktree_target().join("debug/deps")).unwrap() {
        let path = e.unwrap().path();
        if path.to_str().unwrap().contains(&hash) { std::fs::remove_file(path).unwrap(); }
    }
    let r = p.plan_for(&[p.worktree.to_str().unwrap()]);
    assert!(r["plans"].as_array().unwrap().is_empty(), "{r}");
    assert_eq!(r["nothing_to_do"].as_array().unwrap().len(), 1, "{r}");
}

Run: `cargo test --test plan_apply` — expect: failure, `error: unexpected argument '--plan'`.

- [x] **Step 2: Implement `class_of_target`, `Ctx` and `plan_for_target`**

In `src/plan_tier/mod.rs` add `Ctx`. In `src/plan_tier/build.rs`: `class_of_target(owner, checkouts)` — `Owner::Owned { checkout }` → the `Checkout` whose `path == checkout`: `Class::Secondary` → `Class(Secondary)`; `Class::Protected` or not found (ARCH-7) → `Class(Protected)`; `Owner::OrphanCandidate` → `Class(Orphan)`; `Owner::Unresolved` → `Unresolved` (the `protected_owner` rule is already applied by `discover::targets` when it sets the owner). Unit-test the five cases in `build.rs` with hand-made `Owner` and `Checkout` values. `plan_for_target(ctx, target, window)` runs `liveness`: `Undecided` → `TargetPlan::Undecided(reasons)`; `Decided` with no listed path → `NothingToDo`; else a `PlannedTarget` with one `PlannedEntry` per path of `DeadUnit.files` in order (skipping a path an earlier unit already listed), `fs.entry_stat` kind/dev/ino, `unit = "<profile dir relative to the target>/<unit dir name>"`, and `planned` bytes from `gc::measure(fs, &target.path, &paths, ctx.account_bytes)` when `account_bytes`. `Plan::single` builds a one-target plan with a fresh id.

- [x] **Step 3: Implement `make` and the CLI**

`make`: for every registered repo `discover::checkouts` (destructure the `(checkouts, faults)` tuple), then `discover::targets(&checkouts, cfg, true, &mut Snapshot::default(), cancel)`, then keep only targets inside `scope_repos` (all when empty) via `Scope::from_paths`. Validate the window with `state::window_days(cfg.gc_window_days)`; an error becomes `MakeError::Window`, which the CLI turns into a refusal finding and exit 1. For each target: `class_of_target`; `Unresolved` → a finding, no plan; `plan_for_target`; group the `Planned` targets by class into one `Plan` each (`Plan::single` for the first target of a class, then push the rest and sum `planned`). `Orphan` targets are skipped with a finding until Task 14. Add `Command::Gc { plan }` with `#[arg(long, conflicts_with = "dry_run")]`; `plan_cmd` (new, in `src/cli.rs`) reuses the load closure of `gc_cmd`, `doctor::require(.., FingerprintReading, ..)` only, `select`, `live_checkouts`, `emit`; it calls `make`, `record::insert`, builds the `PlanTierBody` (a `PlanRecord` per plan with `outcome: None`, `targets`, `paths`, and planned `freed_bytes`/`deleted_bytes_still_shared` counts when measured), calls `b.plan_tier(body)` and exits 0. `gc` without `--plan` keeps its exact path. Update `tests/skill.rs`: `Parsed::Gc { dry_run: bool, plan: bool }` and `parse` (the existing `want` entries become `Parsed::Gc { dry_run: _, plan: false }`; Task 16 adds the `plan: true` entry when it documents the command).

Add to `CONTEXT.md` the terms Plan (a one-use stored record of the paths one `apply` may delete in the targets of one class), Plan class and Outcome, in the file's existing format.

- [x] **Step 4: Run and commit**

Run: `cargo test` — expect: pass.

`git commit -m "gc: --plan stores one plan per checkout class and deletes nothing"`

_Requirements: GCPLAN-1.1, GCPLAN-1.2, GCPLAN-1.3, GCPLAN-1.4, GCPLAN-1.5, GCPLAN-1.6, GCPLAN-1.10, GCPLAN-1.11, GCPLAN-1.12, GCPLAN-1.13, GCPLAN-6.5, GCPLAN-9.8_

---

### Task 8: Lock one target the way Cargo's builds lock it

**Files:**
- Create: `src/plan_tier/locks.rs`
- Modify: `src/plan_tier/mod.rs` (`pub mod locks;`)
- Create: `tests/common/recording_fs.rs`; Modify: `tests/common/mod.rs` (`pub mod recording_fs;`)
- Create: `tests/plan_tier.rs`
- Modify: `docs/specs/2026-10-30-closure-liveness/design.md` (lines 352-353)
- Test: `tests/plan_tier.rs`

**Reuse:** existing — `Fs::try_lock_exclusive`/`TryLock`, `HeldLock::identity` (Task 1), `Fs::entry_stat`, `cargo_target::identify` (`src/cargo_target.rs`) (rung 2); the candidate-file rule, the skip reasons and the recording wrapper are new (rung 7).

**Interfaces:**
- Consumes: Task 1's `HeldLock::identity`, `Planned` (Task 7)
- Produces: `pub enum Reason { Held, Unopenable, Unlockable, Missing, ProfileSet }`; `pub struct Locked { pub profiles: Vec<PathBuf>, /* the HeldLocks, released on drop */ }`; `pub enum Acquired { Locked(Locked), Skipped(Reason) }`; `pub fn acquire(fs: &dyn Fs, target: &Path, lists_examples: bool) -> Acquired`; `Reason::count_name(self) -> &'static str` (`targets_skipped_held`, `targets_skipped_lock_unopenable`, `targets_skipped_lock_unsupported`, `targets_skipped_lock_missing`, `targets_skipped_profile_set`); `tests::common::recording_fs::Recording` with `over_real()`, `fs(&self) -> Box<dyn Fs>`, `log(&self) -> Vec<String>` (entries `lock <path>`, `remove <path>`, and `read <path>` for `entry_stat`, `mtime`, `read` and `read_dir_bounded` calls), and options `unlockable(path)`, `no_identity(path)`, `replace_after_lock(path)`, `create_dir_after_lock(dir)` (calls `create_dir_all`)

**Depends-on:** Task 1, Task 3, Task 7

- [ ] **Step 1: Write the recording wrapper and the failing tests**

`tests/common/recording_fs.rs`: `Recording` implements `cargo_hive::platform::Fs` by delegating every method to `platform::current().unwrap().fs` — model it on `Volumes` in `tests/liveness.rs` — behind an `Arc<Mutex<Vec<String>>>` log. `try_lock_exclusive(path)` logs `lock <path>` and applies the options for that path: `unlockable` returns `TryLock::Unlockable(io::Error::other("flock unsupported"))`; `no_identity` returns `TryLock::Acquired(HeldLock::new(|| {}))` after really locking; `replace_after_lock` locks, then deletes and recreates the file; `create_dir_after_lock` locks, then creates the directory. `remove_tree`/`remove_file` log `remove <path>` and delegate.

`tests/plan_tier.rs`:

```rust
#![cfg(target_os = "macos")]
mod common;
use cargo_hive::plan_tier::locks::{acquire, Acquired, Reason};
use common::plan_fixture::Planned;
use common::recording_fs::Recording;

fn skipped(a: Acquired) -> Reason {
    match a { Acquired::Skipped(r) => r, Acquired::Locked(_) => panic!("expected a skip") }
}

#[test]
fn a_quiet_target_is_locked_through_both_cargo_lock_files() {
    let p = Planned::new("locks-quiet");
    let rec = Recording::over_real();
    let fs = rec.fs();
    let target = p.worktree_target();
    let Acquired::Locked(_held) = acquire(fs.as_ref(), &target, false) else { panic!("not locked") };
    let log = rec.log();
    assert!(log.iter().any(|l| l.ends_with("debug/.cargo-build-lock")), "{log:?}");
    assert!(log.iter().any(|l| l.ends_with("debug/.cargo-lock")), "{log:?}");
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
    assert!(matches!(acquire(rec.fs().as_ref(), &p.worktree_target(), false), Acquired::Locked(_)));
    let reason = skipped(acquire(rec.fs().as_ref(), &p.worktree_target(), true));
    let _ = holder.kill();
    assert_eq!(reason, Reason::Held);
}

#[test]
fn a_symlinked_lock_file_is_unopenable_and_an_unsupported_flock_is_unlockable() {
    let p = Planned::new("locks-unopenable");
    let lock = p.worktree_target().join("debug/.cargo-build-lock");
    std::fs::remove_file(&lock).unwrap();
    std::os::unix::fs::symlink("/etc/hosts", &lock).unwrap();
    assert_eq!(skipped(acquire(Recording::over_real().fs().as_ref(), &p.worktree_target(), false)), Reason::Unopenable);
    let q = Planned::new("locks-unlockable");
    let rec = Recording::over_real().unlockable(q.worktree_target().join("debug/.cargo-build-lock"));
    assert_eq!(skipped(acquire(rec.fs().as_ref(), &q.worktree_target(), false)), Reason::Unlockable);
}

#[test]
fn a_profile_dir_with_fingerprints_and_no_lock_file_is_missing_a_lock() {
    let p = Planned::new("locks-missing");
    for name in [".cargo-build-lock", ".cargo-lock", ".cargo-artifact-lock"] {
        let _ = std::fs::remove_file(p.worktree_target().join("debug").join(name));
    }
    assert_eq!(skipped(acquire(Recording::over_real().fs().as_ref(), &p.worktree_target(), false)), Reason::Missing);
}

#[test]
fn a_lock_without_identity_or_with_a_replaced_file_skips_the_target() {
    let p = Planned::new("locks-identity");
    let lock = p.worktree_target().join("debug/.cargo-build-lock");
    let none = Recording::over_real().no_identity(lock.clone());
    assert_eq!(skipped(acquire(none.fs().as_ref(), &p.worktree_target(), false)), Reason::Missing);
    let replaced = Recording::over_real().replace_after_lock(lock);
    assert_eq!(skipped(acquire(replaced.fs().as_ref(), &p.worktree_target(), false)), Reason::Missing);
}

#[test]
fn a_profile_dir_that_appears_after_the_locks_are_taken_skips_the_target() {
    let p = Planned::new("locks-profile-set");
    let new_dir = p.worktree_target().join("release/.fingerprint");
    let rec = Recording::over_real().create_dir_after_lock(new_dir);
    let reason = skipped(acquire(rec.fs().as_ref(), &p.worktree_target(), false));
    assert_eq!(reason, Reason::ProfileSet);
}
```

Run: `cargo test --test plan_tier` — expect: compile error, module `plan_tier::locks` not found.

- [ ] **Step 2: Implement `acquire`**

`cargo_target::identify(target)` returns `Option<Result<Vec<(PathBuf, Layout)>, String>>`; `None` or `Err` → `Skipped(Missing)`. For each returned profile and triple directory, candidates are `.cargo-build-lock`, `.cargo-lock` and, when `lists_examples`, `.cargo-artifact-lock`, keeping those that `entry_stat` finds. A directory with a `.fingerprint` child and no candidate → `Skipped(Missing)`. `try_lock_exclusive` each: `Held` → `Held`, `Unopenable` → `Unopenable`, `Unlockable` → `Unlockable`, `Gone` → `Missing`; `Acquired(h)` → compare `h.identity()` with `entry_stat(path)` (`None`, an error or a mismatch → `Missing`); on any skip drop every lock taken. Once all are held, `identify` again; a different set of directories → `Skipped(ProfileSet)`. Return `Locked { profiles, held }`.

In `docs/specs/2026-10-30-closure-liveness/design.md` lines 352-353 replace the two-line parenthetical `(a missing lock file` / `   needs no lock)` (3-space indent on the second line) with `(a missing lock file is not proof that no build runs: a profile dir with a `.fingerprint` directory and no lock file fails the test)`. This edits an Implemented spec by hand; the execute skill records a `realign-spec` follow-up.

Run: `cargo test` — expect: pass.

- [ ] **Step 3: Commit**

`git commit -m "plan_tier: take every Cargo lock of one target, and skip when any is not sound"`

_Requirements: GCPLAN-2.2, GCPLAN-2.3, GCPLAN-2.12_

---

### Task 9: `cargo hive apply` deletes a secondary plan under locks

**Files:**
- Create: `src/plan_tier/apply.rs`
- Modify: `src/gc.rs` (re-export `Ledger` as `pub(crate)`)
- Modify: `src/plan_tier/mod.rs` (`pub mod apply;`)
- Modify: `src/cli.rs` (`Command::Apply`, `apply_cmd`, `EXIT_NEEDS_HUMAN`)
- Modify: `tests/skill.rs` (`Parsed::Apply`)
- Modify: `tests/common/plan_fixture.rs` (`LibCtx`, `Planned::library_ctx`, `Planned::library_ctx_with`)
- Test: `tests/plan_apply.rs`, `tests/plan_tier.rs`

**Reuse:** existing — `plan_tier::locks::acquire`, `plan_tier::guard::check_entry`, `gc::account::remove_counted`, `cargo_target::identify`, `fingerprint::read_profile`, `liveness::liveness`, `discover::targets` with `Snapshot::default()`, `record::{get, consume}`, `cross_check_notes`, `doctor::require` (rung 2); the two-phase driver is new (rung 7).

**Interfaces:**
- Consumes: Tasks 1, 2, 3, 5, 7, 8 (`Ctx`, `class_of_target`, `locks::acquire`, `check_entry`, `remove_counted`, `consume`)
- Produces: `pub enum ApplyError { State(StateError), Platform(String) }` with a hand-written `Display`; every refusal (`unknown_plan`, `already_consumed`, `owner_changed`, …) is `Ok(Applied { outcome: Refused, reason: Some(..) })`, never an `Err`; `pub enum Authorization { Secondary }` (Task 13 adds `Approved(Approval)`, so no never-read field exists before it is used); `pub struct ResolvedTarget { pub path: PathBuf, pub owner: Option<PathBuf>, pub class: PlanClass }`; `pub struct Applied { pub outcome: Outcome, pub reason: Option<String>, pub class: PlanClass, pub id: String, pub targets: Vec<PathBuf>, pub counts: BTreeMap<String, u64>, pub bytes: Option<BytesRec>, pub notes: Vec<String>, pub unmeasured: Vec<Unmeasured>, pub findings: Vec<(String, Option<PathBuf>, String)> }`; `pub fn apply(ctx: &Ctx, id: &str, auth: Authorization, cancel: &(dyn Fn() -> bool + Sync)) -> Result<Applied, ApplyError>`; `#[doc(hidden)] pub fn execute(ctx: &Ctx, plan: Plan, resolved: &[ResolvedTarget], cancel: &(dyn Fn() -> bool + Sync)) -> Applied` (the class check, consume and ownership comparison stay in `apply`; capability gates stay in `apply_cmd`, because the library `Ctx` has no `Capabilities`); `pub const EXIT_NEEDS_HUMAN: i32 = 3` in `src/cli.rs`; `cargo hive apply <id> [--json]`; counts `paths_deleted`, `freed_bytes`, `deleted_bytes_still_shared`, `targets_applied` plus the five `Reason::count_name` counts; `tests::common::plan_fixture::LibCtx { state, cfg, repos, mains, fs }` with `fn ctx(&self) -> Ctx<'_>`

**Depends-on:** Task 2, Task 5, Task 8

- [ ] **Step 1: Write the failing tests**

In `tests/plan_apply.rs`:

```rust
use std::process::Stdio;

fn removed_count(before: &std::collections::BTreeMap<std::path::PathBuf, (u64, std::time::SystemTime)>,
                 after: &std::collections::BTreeMap<std::path::PathBuf, (u64, std::time::SystemTime)>) -> usize {
    before.keys().filter(|k| !after.contains_key(*k)).count()
}

#[test]
fn applying_a_secondary_plan_removes_its_dead_files_and_the_next_build_rebuilds_nothing_live() {
    let p = Planned::new("apply-secondary");
    let planned = p.plan();
    let id = Planned::id_of(&planned, "secondary");
    let listed = planned["plans"].as_array().unwrap().iter().find(|x| x["class"] == "secondary").unwrap()["paths"].as_u64().unwrap() as usize;
    let main_before = tree(&p.main_target());
    let wt_before = tree(&p.worktree_target());
    let (out, r) = p.hive.json(&["apply", &id]);
    assert_eq!(out.status.code(), Some(0), "{r}");
    let plan = &r["plans"][0];
    assert_eq!(plan["outcome"], "applied");
    assert_eq!(plan["counts"]["paths_deleted"].as_u64().unwrap() as usize, listed);
    assert!(plan["counts"]["freed_bytes"].as_u64().is_some());
    assert_eq!(tree(&p.main_target()), main_before, "the protected target must not change");
    let wt_after = tree(&p.worktree_target());
    assert!(removed_count(&wt_before, &wt_after) >= listed);
    p.hive.build_crate(&p.worktree, None);
    assert_eq!(tree(&p.worktree_target()).len(), wt_after.len(), "the next build rebuilt something");
}

#[test]
fn only_the_dead_units_files_are_removed_and_incremental_and_live_files_stay() {
    let p = Planned::new("apply-only-dead");
    let hash = p.dead_unit_hash(&p.worktree_target());
    let id = Planned::id_of(&p.plan(), "secondary");
    let before = tree(&p.worktree_target());
    assert_eq!(p.hive.json(&["apply", &id]).0.status.code(), Some(0));
    let after = tree(&p.worktree_target());
    for gone in before.keys().filter(|k| !after.contains_key(*k)) {
        let s = gone.to_str().unwrap();
        assert!(s.contains(&hash), "{s} is not a file of the dead unit {hash}");
        assert!(!s.contains("/incremental/"), "{s}");
    }
    for (path, meta) in &after {
        if std::fs::symlink_metadata(path).unwrap().is_dir() { continue; }
        assert_eq!(before.get(path), Some(meta), "{} changed", path.display());
    }
}

#[test]
fn an_applied_plan_cannot_be_applied_again() {
    let p = Planned::new("apply-once");
    let id = Planned::id_of(&p.plan(), "secondary");
    assert_eq!(p.hive.json(&["apply", &id]).0.status.code(), Some(0));
    let (out, r) = p.hive.json(&["apply", &id]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(r["plans"][0]["outcome"], "refused");
    assert_eq!(r["plans"][0]["reason"], "already_consumed");
}

#[test]
fn a_target_whose_build_lock_is_held_is_skipped_and_nothing_in_it_is_deleted() {
    let p = Planned::new("apply-held");
    let id = Planned::id_of(&p.plan(), "secondary");
    let lock = p.worktree_target().join("debug/.cargo-build-lock");
    let mut holder = p.hive.hold_lock_child("flock", &lock);
    let before = tree(&p.worktree_target());
    let (out, r) = p.hive.json(&["apply", &id]);
    let _ = holder.kill();
    assert_eq!(out.status.code(), Some(0), "{r}");
    let plan = &r["plans"][0];
    assert_eq!(plan["outcome"], "partial");
    assert_eq!(plan["reason"], "all_targets_skipped");
    assert_eq!(plan["counts"]["targets_skipped_held"], 1);
    assert_eq!(tree(&p.worktree_target()), before);
}

#[test]
fn apply_takes_exactly_one_id_and_refuses_an_unknown_one() {
    let p = Planned::new("apply-ids");
    assert!(!p.hive.run(&["apply"]).status.success());
    assert!(!p.hive.run(&["apply", "a", "b"]).status.success());
    let (out, r) = p.hive.json(&["apply", "0123456789abcdef"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(r["plans"][0]["reason"], "unknown_plan");
}

#[test]
fn apply_refuses_without_the_deletion_capability_and_leaves_the_plan_unconsumed() {
    let p = Planned::new("apply-capability");
    let id = Planned::id_of(&p.plan(), "secondary");
    write_caps(&p, serde_json::json!({"deletion": ["d"]}));
    assert_eq!(p.hive.json(&["apply", &id]).0.status.code(), Some(1));
    p.hive.write_ready_capabilities();
    assert_eq!(p.hive.json(&["apply", &id]).0.status.code(), Some(0));
}

#[test]
fn a_non_secondary_plan_is_not_applied_until_the_needs_human_task() {
    let p = Planned::new("apply-unsupported");
    let id = Planned::id_of(&p.plan(), "protected");
    let (out, r) = p.hive.json(&["apply", &id]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(r["plans"][0]["reason"], "unsupported_class");
}
```

The last test is replaced in Task 11. In `tests/plan_tier.rs` (library seam, via `LibCtx`):

```rust
use cargo_hive::plan_tier::apply::{apply, Authorization};
use cargo_hive::receipt::Outcome;

#[test]
fn locks_are_all_held_before_the_first_removal_and_each_unit_is_removed_dot_d_first_fingerprint_last() {
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
    let last_d = removes.iter().rposition(|l| l.ends_with(".d")).expect("a .d file is removed");
    let first_other = removes.iter().position(|l| !l.ends_with(".d") && !l.contains("/.fingerprint/")).expect("an artifact other than .d is removed");
    assert!(last_d < first_other, "every .d goes before any other artifact: {removes:?}");
    let fingerprint = removes.iter().position(|l| l.contains("/.fingerprint/")).expect("the fingerprint dir is removed");
    assert_eq!(fingerprint, removes.len() - 1, "the fingerprint directory goes last: {removes:?}");
    // liveness is recomputed only while the locks are held: no read of unit files before the last lock
    let first_unit_read = log.iter().position(|l| l.starts_with("read ") && (l.contains("/.fingerprint/") || l.contains("/deps/"))).expect("liveness reads unit files through the Fs");
    assert!(last_lock < first_unit_read, "a unit file was read before every lock was held: {log:?}");
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
```

`LibCtx` (in `tests/common/plan_fixture.rs`): `pub struct LibCtx { pub state: cargo_hive::state::State, pub cfg: cargo_hive::state::Config, pub repos: Vec<cargo_hive::registry::Repo>, pub mains: Vec<PathBuf>, pub fs: Box<dyn cargo_hive::platform::Fs> }`, built by `Planned::library_ctx()` (real `Apfs` from `platform::current()`) and `Planned::library_ctx_with(fs)` by opening `State::open_at(HIVE_HOME)`, `state.config()`, and loading the registry and mains exactly as `gc_cmd` does (`src/cli.rs`, the load closure at its start); `fn ctx(&self) -> Ctx<'_>` returns a `Ctx` borrowing the fields, with `now: SystemTime::now()` and `account_bytes: true`.

Run: `cargo test --test plan_apply --test plan_tier apply` — expect: failure, `error: unrecognized subcommand 'apply'`.

- [ ] **Step 2: Add the command and the gates**

`Command::Apply { id: String, #[command(flatten)] out: OutputArgs }`. `apply_cmd`: install signal handlers, load registry/config/capabilities, `doctor::require` for `FingerprintReading` and `Deletion` (refusal → exit 1, plan untouched), build `Ctx` for all registered repos, call `plan_tier::apply(&ctx, &id, Authorization::Secondary, &cancelled)`, map `Applied` into a `PlanRecord` (`class` as its lowercase name, `outcome`, `reason`, `counts`), call `b.plan_tier(..)`, add `notes`, `unmeasured` and findings, and return 1 for `Refused` or `ApplyError`, otherwise 0 (needs-human, cancel and deletion failure arrive in Tasks 11 and 12). Map `Parsed::Apply` in `tests/skill.rs`.

- [ ] **Step 3: Implement `apply` and `execute` (secondary path)**

`apply`, in order: (1) `record::get`: `Unknown` → `Refused("unknown_plan")`; `Consumed` → `Refused("already_consumed")`; a class other than `Secondary` under `Authorization::Secondary` → `Refused("unsupported_class")` (Task 11 replaces this with `needs-human`). (2) `record::consume`. (3) Resolve ownership afresh over every registered repo with `discover::checkouts` and `discover::targets(.., &mut Snapshot::default(), ..)`; build a `ResolvedTarget` for each plan target (owner path, class via `class_of_target`); Task 10 adds the distinct refusals — here a mismatch returns `Refused("owner_changed")`. (4) Call `execute`.

`execute`, for each target sorted by path: `locks::acquire(fs, &target.path, lists_examples)` where `lists_examples` is true when a planned path has a parent directory named `examples`; a `Skipped(reason)` skips the target and counts `reason.count_name()`. Phase A, for every locked target: `identify` the profiles, `read_profile` each, build a `TargetDir { path, owner, profiles, size: None, unreadable: None }`, run `liveness` with the plan's window and a fresh `now`; every planned path must be in the dead set and `check_entry(fs, &target, path, Expect::Entry{..})` must accept it (Task 10 distinguishes the refusal reasons — here any failure returns `Refused("stale_item")`). Phase B, only when no target refused, per target and per unit in plan order: remove the unit's `.d` entries first, then its other artifacts, then its fingerprint directory (first when the plan lists no `.d` for the unit), the fingerprint directory only if the unit has at least one artifact entry; each path goes through `check_entry` then `remove_counted(fs, &mut ledger, &target, path, account_bytes, false)` into one `Ledger` shared by the whole plan. Outcome: every target applied → `applied`; otherwise `partial` with reason `skipped`, or `all_targets_skipped` when none applied. Counts: `paths_deleted` is the number of planned entries whose `check_entry` and `remove_counted` both succeeded with `existed == true` (one per entry; a directory entry counts once whatever it holds; it is not `Removed.files`), and `PlanRecord.paths` of a plan is its number of `PlannedEntry` over all targets; `freed_bytes` and `deleted_bytes_still_shared` from `ledger.finish().total`, `targets_applied`, the five skip counts; add `cross_check_notes` once per APFS container deleted on.

Run: `cargo test` — expect: pass.

- [ ] **Step 4: Commit**

`git commit -m "apply: delete a secondary plan under every profile lock, once"`

_Requirements: GCPLAN-2.1, GCPLAN-2.5, GCPLAN-2.6, GCPLAN-2.7, GCPLAN-2.8, GCPLAN-2.9, GCPLAN-2.10, GCPLAN-2.13, GCPLAN-3.10, GCPLAN-6.2, GCPLAN-6.6, GCPLAN-6.7, GCPLAN-9.6_

---

### Task 10: `apply` refuses a plan that is no longer true

**Files:**
- Modify: `src/plan_tier/apply.rs`
- Test: `tests/plan_apply.rs`

**Reuse:** extends — `apply.rs` from Task 9 (rung 2).

**Interfaces:**
- Consumes: Task 9's phases
- Produces: refusal reasons `stale_item`, `undecided`, `entry_changed`, `owner_changed`, `class_changed`, `ownership_unresolved`, `unsupported_format`; the whole plan refused before any deletion; exit 1 with outcome `refused`; checks run in the order owner, class, format, then per path entry identity, then dead set

**Depends-on:** Task 9

- [ ] **Step 1: Write the failing tests**

```rust
fn refused(p: &Planned, id: &str, reason: &str) {
    let before = (tree(&p.main_target()), tree(&p.worktree_target()));
    let (out, r) = p.hive.json(&["apply", id]);
    assert_eq!(out.status.code(), Some(1), "{r}");
    assert_eq!(r["plans"][0]["outcome"], "refused");
    assert_eq!(r["plans"][0]["reason"], reason, "{r}");
    assert_eq!((tree(&p.main_target()), tree(&p.worktree_target())), before, "a refused plan deleted something");
    let (_, again) = p.hive.json(&["apply", id]);
    assert_eq!(again["plans"][0]["reason"], "already_consumed", "a refused plan stays consumed");
}

#[test]
fn a_planned_unit_that_a_command_made_live_again_refuses_the_whole_plan() {
    let p = Planned::new("stale-live");
    let hash = p.dead_unit_hash(&p.worktree_target());
    let id = Planned::id_of(&p.plan(), "secondary");
    p.revive(&p.worktree_target(), &hash);
    refused(&p, &id, "stale_item");
}

#[test]
fn a_planned_path_that_vanished_refuses_the_plan() {
    let p = Planned::new("stale-entry");
    let hash = p.dead_unit_hash(&p.worktree_target());
    let id = Planned::id_of(&p.plan(), "secondary");
    let deps = p.worktree_target().join("debug/deps");
    let victim = std::fs::read_dir(&deps).unwrap().map(|e| e.unwrap().path())
        .find(|f| f.to_str().unwrap().contains(&hash) && f.extension().is_some_and(|e| e == "rlib")).unwrap();
    std::fs::remove_file(&victim).unwrap();
    refused(&p, &id, "entry_changed");
}

#[test]
fn a_unit_that_became_dead_after_the_plan_is_left_alone() {
    let p = Planned::new("stale-newly-dead");
    let id = Planned::id_of(&p.plan(), "secondary");
    common::cargo_fixture::backdate_all(&p.worktree_target(), std::time::Duration::from_secs(10 * 86_400));
    let manifest = p.worktree.join("Cargo.toml");
    let text = std::fs::read_to_string(&manifest).unwrap().replace("0.1.1", "0.1.2");
    std::fs::write(&manifest, text).unwrap();
    p.hive.build_crate(&p.worktree, None);
    let (out, r) = p.hive.json(&["apply", &id]);
    assert_eq!(out.status.code(), Some(0), "{r}");
    assert_eq!(r["plans"][0]["outcome"], "applied");
}

#[test]
fn a_changed_class_refuses_the_plan() {
    let p = Planned::new("stale-class");
    let id = Planned::id_of(&p.plan(), "secondary");
    // with no secondary roots the worktree is no longer a secondary checkout
    p.hive.config(r#"{"secondary_roots": []}"#);
    refused(&p, &id, "class_changed");
}

#[test]
fn a_changed_owner_refuses_the_plan() {
    let p = Planned::new("stale-owner");
    let id = Planned::id_of(&p.plan(), "secondary");
    // the main checkout now builds into the worktree's target dir, and a protected owner wins
    std::fs::create_dir_all(p.main.join(".cargo")).unwrap();
    std::fs::write(p.main.join(".cargo/config.toml"), format!("[build]\ntarget-dir = \"{}\"\n", p.worktree_target().display())).unwrap();
    refused(&p, &id, "owner_changed");
}

#[test]
fn a_profile_with_an_unknown_fingerprint_format_refuses_the_plan() {
    let p = Planned::new("stale-format");
    let hash = p.dead_unit_hash(&p.worktree_target());
    let id = Planned::id_of(&p.plan(), "secondary");
    let unit = p.worktree_target().join(format!("debug/.fingerprint/fixture-{hash}"));
    for entry in std::fs::read_dir(&unit).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "json") {
            std::fs::write(&path, r#"{"unexpected": true}"#).unwrap();
        }
    }
    refused(&p, &id, "unsupported_format");
}

#[test]
fn a_target_that_became_undecided_after_the_plan_refuses_it_with_undecided() {
    let p = Planned::new("stale-undecided");
    let id = Planned::id_of(&p.plan(), "secondary");
    let unit = p.worktree_target().join("debug/.fingerprint/zz-0123456789abcdef");
    std::fs::create_dir_all(&unit).unwrap();
    std::fs::write(unit.join("invoked.timestamp"), b"").unwrap();
    std::fs::write(unit.join("output-lib-zz"), b"").unwrap();
    refused(&p, &id, "undecided");
}

#[test]
fn a_workspace_that_no_longer_resolves_refuses_the_plan_as_ownership_unresolved() {
    let p = Planned::new("stale-unresolved");
    let id = Planned::id_of(&p.plan(), "secondary");
    std::fs::write(p.worktree.join("Cargo.toml"), "this is not a manifest").unwrap();
    refused(&p, &id, "ownership_unresolved");
}

#[test]
fn a_plan_edited_to_name_a_file_outside_its_target_is_refused_and_the_file_survives() {
    let p = Planned::new("stale-outside");
    let id = Planned::id_of(&p.plan(), "secondary");
    let outside = p.scratch.path.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let keep = outside.join("keep");
    std::fs::write(&keep, b"mine").unwrap();
    let md = std::fs::metadata(&keep).unwrap();
    use std::os::unix::fs::MetadataExt;
    let mut stored = p.plans_json();
    for plan in stored["plans"].as_array_mut().unwrap() {
        if plan["id"] == id.as_str() {
            let e = &mut plan["targets"][0]["entries"][0];
            e["path"] = serde_json::json!(keep.to_str().unwrap());
            e["kind"] = serde_json::json!("file");
            e["dev"] = serde_json::json!(md.dev());
            e["ino"] = serde_json::json!(md.ino());
        }
    }
    std::fs::write(p.hive.home.path.join("plans.json"), stored.to_string()).unwrap();
    let (out, r) = p.hive.json(&["apply", &id]);
    assert_eq!(out.status.code(), Some(1), "{r}");
    assert_eq!(r["plans"][0]["outcome"], "refused");
    assert!(keep.exists(), "a path outside the target was deleted");
}

#[test]
fn every_target_is_checked_before_any_is_deleted() {
    let p = Planned::with_worktrees("stale-two-targets", 2);
    let id = Planned::id_of(&p.plan(), "secondary");
    let second = p.more_worktrees[0].join("target");
    let hash = p.dead_unit_hash(&second);
    p.revive(&second, &hash);
    let first_before = tree(&p.worktree_target());
    let (out, r) = p.hive.json(&["apply", &id]);
    assert_eq!(out.status.code(), Some(1), "{r}");
    assert_eq!(tree(&p.worktree_target()), first_before, "the first target was deleted although the second was stale");
}
```

Run: `cargo test --test plan_apply stale` — expect: failures with `stale_item`/`owner_changed` where another reason is named (Task 9 returns coarse reasons).

- [ ] **Step 2: Implement the distinct refusals**

Phase A returns, for the first failing check across all targets in sorted order, a typed `Refusal { reason, target, path }`, checking in this order: `Owner::Unresolved` or a resolution failure → `ownership_unresolved`; owner path differs → `owner_changed`; class differs → `class_changed`; a profile whose `Format` is unsupported → `unsupported_format`; for each planned path a `check_entry` failure (missing, other kind, device or inode, or `Outside`) → `entry_changed`; `Undecided` (carry its reasons into a finding) → `undecided`; a planned path not in the recomputed dead set → `stale_item`. Release every lock, delete nothing, report the target and first path in a finding. The plan stays consumed.

Run: `cargo test` — expect: pass.

- [ ] **Step 3: Commit**

`git commit -m "apply: refuse a stale plan whole, with a reason per cause"`

_Requirements: GCPLAN-3.1, GCPLAN-3.2, GCPLAN-3.3, GCPLAN-3.4, GCPLAN-3.5, GCPLAN-3.6, GCPLAN-3.8, GCPLAN-6.4_

---

### Task 11: Protected and orphan plans stop at needs-human

**Files:**
- Modify: `src/plan_tier/apply.rs`
- Modify: `src/cli.rs` (exit 3)
- Modify: `docs/standards/errors-logging.md` (exit code 3)
- Test: `tests/plan_apply.rs`

**Reuse:** extends — `apply` step (1) and `apply_cmd` (Task 9) (rung 2).

**Interfaces:**
- Consumes: `record::get`
- Produces: `Applied { outcome: NeedsHuman, .. }` for `PlanClass::Protected` and `PlanClass::Orphan` plans under `Authorization::Secondary`; exit 3; the plan stays `Live`; counts include the plan's `paths` and planned `freed_bytes`

**Depends-on:** Task 10

- [ ] **Step 1: Write the failing tests** — first delete `a_non_secondary_plan_is_not_applied_until_the_needs_human_task` from `tests/plan_apply.rs`

```rust
#[test]
fn a_protected_plan_returns_needs_human_with_what_would_be_approved_and_stays_unconsumed() {
    let p = Planned::new("needs-human");
    let id = Planned::id_of(&p.plan(), "protected");
    let before = tree(&p.main_target());
    for _ in 0..2 {
        let (out, r) = p.hive.json(&["apply", &id]);
        assert_eq!(out.status.code(), Some(3), "{r}");
        let plan = &r["plans"][0];
        assert_eq!(plan["outcome"], "needs-human");
        assert_eq!(plan["class"], "protected");
        assert_eq!(plan["id"], id.as_str());
        assert!(plan["paths"].as_u64().unwrap() > 0);
        assert_eq!(plan["targets"].as_array().unwrap().len(), 1);
        assert!(plan["counts"]["freed_bytes"].as_u64().is_some(), "planned bytes are reported: {plan}");
        assert_eq!(tree(&p.main_target()), before);
    }
}

#[test]
fn apply_has_no_option_or_environment_variable_that_deletes_for_a_protected_plan() {
    let p = Planned::new("no-bypass");
    let id = Planned::id_of(&p.plan(), "protected");
    for extra in [vec!["--yes"], vec!["--force"], vec!["--approve"]] {
        let mut args = vec!["apply", id.as_str()];
        args.extend(extra);
        assert!(!p.hive.run(&args).status.success());
    }
    let out = p.hive.command(&["apply", &id]).env("HIVE_APPROVE", "1").output().unwrap();
    assert_eq!(out.status.code(), Some(3));
}

#[test]
fn apply_never_blocks_on_an_owner_authentication_prompt() {
    let p = Planned::new("no-dialog");
    let id = Planned::id_of(&p.plan(), "protected");
    let child = p.hive.command(&["apply", &id, "--json"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let out = common::wait_within(child, std::time::Duration::from_secs(60));
    assert_eq!(out.status.code(), Some(3));
}
```

Run: `cargo test --test plan_apply needs_human` — expect: failure, exit code is 1, not 3.

- [ ] **Step 2: Implement**

At step (1) of `apply`, a `Live` plan whose class is not `Secondary` and under `Authorization::Secondary` (the only value until Task 13) returns `Applied { outcome: NeedsHuman, id, class, targets, bytes: plan.planned, counts: { paths, freed_bytes, deleted_bytes_still_shared when present } }` before `record::consume` and before any lock or `cargo metadata` call. `apply_cmd` returns `EXIT_NEEDS_HUMAN` (3) for it; an unlogged receipt still forces 1 (`emit`). Remove the `unsupported_class` refusal. `apply` takes no option other than `--json`, so no clap change. In `docs/standards/errors-logging.md`, in the exit-code row, add `3 needs-human (apply only)`.

Run: `cargo test` — expect: pass, including the unchanged `tests/receipts.rs`.

- [ ] **Step 3: Commit**

`git commit -m "apply: protected plans return needs-human, exit 3, and stay unconsumed"`

_Requirements: GCPLAN-4.1, GCPLAN-4.2, GCPLAN-4.3, GCPLAN-4.4, GCPLAN-6.3, GCPLAN-9.4_

---

### Task 12: An interrupted apply leaves a buildable target

**Files:**
- Modify: `src/plan_tier/apply.rs`
- Modify: `src/cli.rs` (exit 130, deletion-failure exit 1)
- Modify: `tests/common/recording_fs.rs` (`fail_removes_after(n)`)
- Test: `tests/plan_tier.rs`, `tests/plan_apply.rs`

**Reuse:** extends — Phase B of `execute` (Task 9); `doctor::CANCEL` and `install_signal_handlers` (rung 2); the `an_interrupted_gc_exits_130` pattern of `tests/gc.rs` (fake `cargo` on `PATH` that sleeps, SIGINT while it runs).

**Interfaces:**
- Consumes: `check_entry`, `remove_counted`, the per-unit order of Task 9, `Recording`
- Produces: `partial` with reason `cancelled` and exit 130 after a cancel (polled between units); `partial` with exit 1 and a `plan.delete_failed` finding when a deletion fails; `Recording::fail_removes_after(n)` makes every `remove_tree`/`remove_file` after the first `n` return `Err(io::Error::other("simulated stop"))`

**Depends-on:** Task 11

- [ ] **Step 1: Write the failing tests**

In `tests/plan_tier.rs`:

```rust
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn stopping_after_any_one_deletion_leaves_a_target_cargo_can_build() {
    let probe = Planned::new("prefix-probe");
    let r = probe.hive.json(&["apply", &Planned::id_of(&probe.plan(), "secondary")]).1;
    let steps = r["plans"][0]["counts"]["paths_deleted"].as_u64().unwrap() as usize;
    assert!(steps >= 2, "a unit needs at least two deletions to have a prefix");
    for stop_after in 0..steps {
        let p = Planned::new(&format!("prefix-{stop_after}"));
        let id = Planned::id_of(&p.plan(), "secondary");
        let rec = Recording::over_real().fail_removes_after(stop_after);
        let lib = p.library_ctx_with(rec.fs());
        let done = apply(&lib.ctx(), &id, Authorization::Secondary, &|| false).unwrap();
        assert_eq!(done.outcome, Outcome::Partial, "stop {stop_after}");
        assert!(done.findings.iter().any(|f| f.0 == "plan.delete_failed"), "{:?}", done.findings);
        p.hive.build_crate(&p.worktree, None);
        let rest: u64 = p.plan()["plans"].as_array().unwrap().iter().filter(|x| x["class"] == "secondary").map(|x| x["paths"].as_u64().unwrap()).sum();
        assert!(rest > 0 || stop_after + 1 == steps, "what a stop left behind is planned again (stop {stop_after})");
    }
}

#[test]
fn a_cancel_between_units_keeps_the_first_unit_gone_and_the_second_intact_and_the_rest_can_be_planned_again() {
    use cargo_hive::plan_tier::apply::{execute, ResolvedTarget};
    use cargo_hive::plan_tier::record::PlanClass;
    let p = Planned::with_dead_units("cancel-between", 2);
    let lib = p.library_ctx();
    let ctx = lib.ctx();
    let made = cargo_hive::plan_tier::build::make(&ctx, &[], &|| false).unwrap();
    let plan = made.plans.into_iter().find(|x| x.class == PlanClass::Secondary).unwrap();
    let resolved = vec![ResolvedTarget { path: plan.targets[0].path.clone(), owner: plan.targets[0].owner.clone(), class: PlanClass::Secondary }];
    let before = common::plan_fixture::tree(&p.worktree_target());
    let polls = AtomicUsize::new(0);
    let done = execute(&ctx, plan, &resolved, &|| polls.fetch_add(1, Ordering::SeqCst) >= 1);
    assert_eq!((done.outcome, done.reason.as_deref()), (Outcome::Partial, Some("cancelled")));
    let after = common::plan_fixture::tree(&p.worktree_target());
    let gone: Vec<_> = before.keys().filter(|k| !after.contains_key(*k)).collect();
    let hashes: std::collections::BTreeSet<&str> = gone.iter().filter_map(|k| k.to_str())
        .filter_map(|s| s.rsplit('-').next().map(|t| t.trim_end_matches(".d").trim_end_matches(".rlib").trim_end_matches(".rmeta"))).collect();
    assert_eq!(hashes.len(), 1, "exactly the first unit is gone: {gone:?}");
    let again = cargo_hive::plan_tier::build::make(&ctx, &[], &|| false).unwrap();
    assert!(again.plans.iter().any(|x| x.class == PlanClass::Secondary), "the second unit can be planned again");
}
```

In `tests/plan_apply.rs`, adapt `an_interrupted_gc_exits_130` from `tests/gc.rs` (fake `cargo` that writes its pid to a sentinel and sleeps, `PATH` prefixed, wait for the sentinel, `kill -INT`, `wait_within`), run `apply <id> --json` instead of `gc`, and assert exit 130, a receipt line with `"command":"apply"` in `receipts/`, and `already_consumed` on a second `apply`:

```rust
use std::os::unix::fs::PermissionsExt;

fn pid_alive(pid: u32) -> bool {
    std::process::Command::new("kill").args(["-0", &pid.to_string()])
        .stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

#[test]
fn a_sigint_during_apply_exits_130_logs_a_receipt_and_leaves_the_plan_consumed() {
    let p = Planned::new("apply-sigint");
    let id = Planned::id_of(&p.plan(), "secondary");
    // a fake cargo that records its pid and sleeps stands in for `cargo metadata`
    let bin = p.scratch.path.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let sentinel = p.scratch.path.join("cargo.pid");
    std::fs::write(bin.join("cargo"), "#!/bin/sh\nprintf '%s\\n' \"$$\" > \"$HIVE_CARGO_SENTINEL\"\nsleep 300\n").unwrap();
    std::fs::set_permissions(bin.join("cargo"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let child = p.hive.command(&["apply", &id, "--json"])
        .env("PATH", &path).env("HIVE_CARGO_SENTINEL", &sentinel)
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().unwrap();
    let hive_pid = child.id();
    let started = std::time::Instant::now();
    loop {
        assert!(started.elapsed() < std::time::Duration::from_secs(30), "cargo metadata never wrote its pid");
        if std::fs::read_to_string(&sentinel).is_ok_and(|t| t.trim().parse::<u32>().is_ok()) { break; }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(pid_alive(hive_pid), "apply should be running before SIGINT");
    assert!(std::process::Command::new("kill").args(["-INT", &hive_pid.to_string()]).status().unwrap().success());
    let out = common::wait_within(child, std::time::Duration::from_secs(30));
    assert_eq!(out.status.code(), Some(130), "{out:?}");
    let logged = std::fs::read_dir(p.hive.home.path.join("receipts")).unwrap().filter_map(|e| e.ok())
        .any(|e| std::fs::read_to_string(e.path()).unwrap_or_default().contains("\"command\":\"apply\""));
    assert!(logged, "a cancelled apply appends a receipt");
    let (again, r) = p.hive.json(&["apply", &id]);
    assert_eq!(again.status.code(), Some(1));
    assert_eq!(r["plans"][0]["reason"], "already_consumed");
}
```

Run: `cargo test --test plan_tier --test plan_apply` — expect: failure, no `fail_removes_after`, then outcomes not `Partial`.

- [ ] **Step 2: Implement**

Poll `cancel` after `consume`, again immediately after `discover::targets` returns (before any `Unresolved` owner is turned into `ownership_unresolved`), and between units in Phase B; on true release the locks and return `Ok(Applied { outcome: Partial, reason: Some("cancelled"), .. })`. `apply_cmd` passes 130 to `emit` for it, because `emit` returns whatever code it is given, and still logs the receipt. Add `pub(crate) fn exit_code(a: &Applied) -> i32` in `apply.rs`: 3 for `NeedsHuman`; 130 for reason `cancelled`; 1 for `Refused` and for `Partial` with a `plan.delete_failed` finding; 0 otherwise; `apply_cmd` calls it, and a unit test covers the four outcomes with and without a delete failure, a cancel and a skip. A `Missing`, `Changed` or `Outside` from `check_entry` or a `RemoveFailed` in Phase B stops that target at the end of its current unit, adds a `plan.delete_failed` finding with the path and reason, continues with the next target, and the command exits 1 with outcome `partial`. Add `fail_removes_after` to the recording wrapper.

Run: `cargo test` — expect: pass.

- [ ] **Step 3: Commit**

`git commit -m "apply: stop between units on cancel and fail closed on a deletion error"`

_Requirements: GCPLAN-6.8, GCPLAN-8.3, GCPLAN-8.5_

---

### Task 13: A protected plan deletes when given an approval only tests can make

**Files:**
- Modify: `src/plan_tier/apply.rs` (`Approval`, `Authorization::Approved`)
- Modify: `CONTEXT.md` (Approval value)
- Test: `tests/plan_tier.rs`

**Reuse:** extends — `apply`, `execute` (Tasks 9 to 12) (rung 2).

**Interfaces:**
- Consumes: `execute`, `record`, `LibCtx`
- Produces: `Authorization::Approved(Approval)`; `Approval { plan_id }` with no public constructor except `#[doc(hidden)] pub fn approval_for_tests(plan_id: &str) -> Approval`; `apply(.., Authorization::Approved(a), ..)` runs a `Protected` plan by the same steps as a `Secondary` one when `a.plan_id == id`, and returns `Refused("approval_mismatch")` otherwise, leaving the plan unconsumed

**Depends-on:** Task 12

- [ ] **Step 1: Write the failing test** in `tests/plan_tier.rs`

```rust
use cargo_hive::plan_tier::apply::approval_for_tests;

#[test]
fn an_approved_protected_plan_is_deleted_by_the_same_steps_as_a_secondary_one() {
    let p = Planned::new("approved-protected");
    let id = Planned::id_of(&p.plan(), "protected");
    let before = common::plan_fixture::tree(&p.main_target());
    let lib = p.library_ctx();
    let wrong = apply(&lib.ctx(), &id, Authorization::Approved(approval_for_tests("0000000000000000")), &|| false).unwrap();
    assert_eq!(wrong.reason.as_deref(), Some("approval_mismatch"));
    assert_eq!(common::plan_fixture::tree(&p.main_target()), before);
    let done = apply(&lib.ctx(), &id, Authorization::Approved(approval_for_tests(&id)), &|| false).unwrap();
    assert_eq!(done.outcome, Outcome::Applied, "{:?}", done.findings);
    assert!(common::plan_fixture::tree(&p.main_target()).len() < before.len());
}
```

Run: `cargo test --test plan_tier approved` — expect: compile error, no `approval_for_tests`.

- [ ] **Step 2: Implement**

`Approval` has a private field and no `pub` constructor besides the `#[doc(hidden)]` test function. In `apply`, `Authorization::Approved(a)` skips the needs-human return only for a `Protected` or `Orphan` plan whose id equals `a.plan_id`; a different id returns `Refused("approval_mismatch")` before consuming. The CLI still passes only `Authorization::Secondary`. Add the term Approval value to `CONTEXT.md`.

Run: `cargo test` — expect: pass.

- [ ] **Step 3: Commit**

`git commit -m "plan_tier: protected plans delete only behind an approval value"`

_Requirements: GCPLAN-4.5_

---

### Task 14: `gc --plan` proposes orphan target dirs

**Files:**
- Modify: `src/plan_tier/build.rs`
- Modify: `tests/common/plan_fixture.rs` (`Planned::with_orphan`, `orphan_plan_id`)
- Test: `tests/plan_apply.rs`

**Reuse:** existing — `Owner::OrphanCandidate`, `discover::targets` with `all_repos = true`, `Fs::read_dir_bounded`, `Fs::entry_stat`, `gc::measure`, `cargo_target::identify` (rung 2); the orphan plan content is new (rung 7).

**Interfaces:**
- Consumes: Task 7's `make`, `record::OrphanScope`
- Produces: an `orphan` plan whose target holds one entry (the target dir) and an `OrphanScope` with its profile and triple dirs; a finding `plan.caller_env` when `owned_under_caller_env` is set; `Planned::with_orphan(label) -> (Planned, PathBuf)` and `Planned::orphan_plan_id(&self, orphan: &Path) -> String`

**Depends-on:** Task 7, Task 12 (Task 12 also edits `tests/plan_apply.rs`; Task 11's needs-human return for the `Orphan` class is already in place)

- [ ] **Step 1: Write the failing tests**

Add to `tests/common/plan_fixture.rs` (inside `impl Planned`; imports `init_repo` and `Path` are already there):

```rust
    /// A built target dir no registered checkout owns, inside the default cache root
    /// (`Hive::config` defaults `cache_roots` to `<home>/no-cache`). The throwaway repo is never registered.
    pub fn with_orphan(label: &str) -> (Planned, PathBuf) {
        let p = Planned::new(label);
        let gone = init_repo(&p.scratch.path.join("gone-repo"), "");
        let orphan = p.hive.home.path.join("no-cache/gone-repo-target");
        std::fs::create_dir_all(orphan.parent().unwrap()).unwrap();
        p.hive.build_crate(&gone, Some(&orphan));
        let orphan = orphan.canonicalize().unwrap();
        (p, orphan)
    }

    /// Runs `gc --plan` over every registered repo (`Hive::config` overwrites config.json) and returns the one orphan plan's id.
    pub fn orphan_plan_id(&self, orphan: &Path) -> String {
        self.hive.config(&format!(r#"{{"cache_roots": ["{}"]}}"#, orphan.parent().unwrap().display()));
        let r = self.plan();
        assert_eq!(r["plans"].as_array().unwrap().iter().filter(|x| x["class"] == "orphan").count(), 1, "{r}");
        Planned::id_of(&r, "orphan")
    }
```

```rust
#[test]
fn an_unowned_target_dir_becomes_one_whole_directory_entry_in_an_orphan_plan() {
    let (p, orphan) = Planned::with_orphan("orphan-plan");
    let before = tree(&orphan);
    let id = p.orphan_plan_id(&orphan);
    let stored = p.plans_json();
    let plan = stored["plans"].as_array().unwrap().iter().find(|x| x["id"] == id.as_str()).unwrap();
    assert_eq!(plan["class"], "orphan");
    let target = &plan["targets"][0];
    assert_eq!(target["entries"].as_array().unwrap().len(), 1, "the target dir itself and nothing inside it");
    assert_eq!(target["entries"][0]["path"].as_str().unwrap(), orphan.canonicalize().unwrap().to_str().unwrap());
    let dirs = target["orphan"]["dirs"].as_array().unwrap();
    assert!(dirs.iter().any(|d| d.as_str().unwrap().ends_with("/debug")), "{target}");
    for key in ["root_kind", "root_dev", "root_ino"] {
        assert!(!target["orphan"][key].is_null(), "{key}");
    }
    assert_eq!(tree(&orphan), before);
}

#[test]
fn no_other_class_plans_the_removal_of_a_whole_target_dir() {
    let (p, orphan) = Planned::with_orphan("orphan-only");
    p.orphan_plan_id(&orphan);
    let r = p.plan();
    for other in r["plans"].as_array().unwrap().iter().filter(|x| x["class"] != "orphan") {
        assert!(!other["targets"].to_string().contains("gone-repo-target"), "{other}");
    }
}

#[test]
fn an_orphan_plan_ends_at_needs_human() {
    let (p, orphan) = Planned::with_orphan("orphan-human");
    let id = p.orphan_plan_id(&orphan);
    let (out, r) = p.hive.json(&["apply", &id]);
    assert_eq!(out.status.code(), Some(3), "{r}");
    assert!(orphan.exists());
}

#[test]
fn a_target_owned_by_a_repo_outside_the_named_scope_is_not_an_orphan() {
    let p = Planned::new("orphan-scope");
    // repo B (a second registered repo) is configured to build into a directory inside A's tree
    let b_dir = p.scratch.path.join("b");
    let b = init_repo(&b_dir, "");
    register(&p.hive, &b);
    let shared = p.main.join("shared-target");
    std::fs::create_dir_all(b.join(".cargo")).unwrap();
    std::fs::write(b.join(".cargo/config.toml"), format!("[build]\ntarget-dir = \"{}\"\n", shared.display())).unwrap();
    p.hive.build_crate(&b, None);
    let r = p.plan_for(&[p.main.to_str().unwrap()]); // only A is named
    assert!(r["plans"].as_array().unwrap().iter().all(|x| x["class"] != "orphan"), "{r}");
    let shared_c = shared.canonicalize().unwrap();
    let listed = r["plans"].as_array().unwrap().iter().any(|x| x["targets"].to_string().contains(shared_c.to_str().unwrap()));
    assert!(listed || r["nothing_to_do"].to_string().contains(shared_c.to_str().unwrap()), "the shared target is owned by B and planned or nothing-to-do, not orphan: {r}");
}

#[test]
fn an_orphan_is_not_planned_while_a_registered_workspace_does_not_resolve() {
    let (p, orphan) = Planned::with_orphan("orphan-unresolved");
    std::fs::write(p.worktree.join("Cargo.toml"), "this is not a manifest").unwrap();
    p.hive.config(&format!(r#"{{"cache_roots": ["{}"]}}"#, orphan.parent().unwrap().display()));
    let r = p.plan();
    assert!(r["plans"].as_array().unwrap().iter().all(|x| x["class"] != "orphan"), "an unresolved workspace means ownership is unknown: {r}");
}

#[test]
fn an_orphan_owned_under_the_callers_environment_is_planned_with_a_note() {
    let (p, orphan) = Planned::with_orphan("orphan-caller-env");
    p.hive.config(&format!(r#"{{"cache_roots": ["{}"]}}"#, orphan.parent().unwrap().display()));
    let (out, r) = p.hive.json_env(&["gc", "--plan"], &[("CARGO_TARGET_DIR", orphan.to_str().unwrap())]);
    assert!(out.status.success());
    assert!(r["findings"].as_array().unwrap().iter().any(|f| f["kind"] == "plan.caller_env"), "{r}");
    assert!(r["plans"].as_array().unwrap().iter().any(|x| x["class"] == "orphan"), "{r}");
}
```

Run: `cargo test --test plan_apply orphan` — expect: failure, no `orphan` plan.

- [ ] **Step 2: Implement**

In `make`, for `TargetClass::Class(PlanClass::Orphan)` add a `PlannedTarget` whose `entries` holds one `PlannedEntry` for the target dir (`fs.entry_stat`, `unit` = the target path) and whose `orphan` is `OrphanScope { root_kind, root_dev, root_ino, dirs }`, `dirs` being the profile and triple dirs from `cargo_target::identify`. Planned bytes come from `gc::measure(.., &[target.path], ..)`; hitting its cap becomes an `Unmeasured`. Add a finding `plan.caller_env` with the variable name when the owner is `OrphanCandidate { owned_under_caller_env: Some(_) }`. Remove the "skipped with a finding" branch of Task 7. `apply` already returns `needs-human` for the `Orphan` class (Task 11).

Run: `cargo test` — expect: pass.

- [ ] **Step 3: Commit**

`git commit -m "gc: --plan proposes unowned target dirs as one orphan entry"`

_Requirements: GCPLAN-5.1, GCPLAN-5.2, GCPLAN-5.3, GCPLAN-5.4, GCPLAN-5.5, GCPLAN-5.6_

---

### Task 15: An approved orphan plan removes the whole target dir safely

**Files:**
- Create: `src/plan_tier/orphan.rs`
- Modify: `src/plan_tier/mod.rs` (`pub mod orphan;`), `src/plan_tier/apply.rs` (call it for `Orphan` plans under `Approved`)
- Test: `tests/plan_tier.rs`

**Reuse:** existing — `plan_tier::locks::acquire`, the shared `Ledger`, `plan_tier::guard::check_entry`, `gc::account::remove_counted`, `cargo_target::identify`, `Fs::read_dir_bounded`, `discover::targets` for fresh ownership (rung 2); the relist-after-lock orchestration is new (rung 7).

**Interfaces:**
- Consumes: Tasks 1, 8, 13, 14
- Produces: `pub fn remove(ctx: &Ctx, target: &PlannedTarget, approval: &Approval, cancel: &(dyn Fn() -> bool + Sync)) -> Removal` with `Removal { Removed(BytesRec), Skipped(String), Refused(String) }` (before any lock, `entry_stat(&target.path)` must equal the recorded kind, device and inode, else `Refused`); in `apply`, an `Orphan` plan under `Approved` ends `applied` on `Removed`, `partial` on `Skipped`, `refused` on `Refused`

**Depends-on:** Task 13, Task 14

- [ ] **Step 1: Write the failing tests** in `tests/plan_tier.rs`

```rust
#[test]
fn an_approved_orphan_plan_removes_the_whole_target_dir_after_taking_every_lock() {
    let (p, orphan) = Planned::with_orphan("orphan-remove");
    let outside_link_target = p.scratch.path.join("link-target");
    std::fs::write(&outside_link_target, b"keep").unwrap();
    std::os::unix::fs::symlink(&outside_link_target, orphan.join("debug/link-out")).unwrap();
    let id = p.orphan_plan_id(&orphan);
    let rec = Recording::over_real();
    let lib = p.library_ctx_with(rec.fs());
    let done = apply(&lib.ctx(), &id, Authorization::Approved(approval_for_tests(&id)), &|| false).unwrap();
    assert_eq!(done.outcome, Outcome::Applied, "{:?}", done.findings);
    assert!(!orphan.exists());
    assert!(outside_link_target.exists(), "a symlink inside the orphan must not be followed");
    assert!(done.bytes.is_some(), "freed and still-shared bytes are reported");
    let log = rec.log();
    let last_lock = log.iter().rposition(|e| e.starts_with("lock ")).unwrap();
    let first_remove = log.iter().position(|e| e.starts_with("remove ")).unwrap();
    assert!(last_lock < first_remove, "every lock is held before the removal: {log:?}");
}

#[test]
fn a_held_lock_skips_the_orphan_removal_and_deletes_nothing() {
    let (p, orphan) = Planned::with_orphan("orphan-held");
    let id = p.orphan_plan_id(&orphan);
    let mut holder = p.hive.hold_lock_child("flock", &orphan.join("debug/.cargo-build-lock"));
    let before = common::plan_fixture::tree(&orphan);
    let done = apply(&p.library_ctx().ctx(), &id, Authorization::Approved(approval_for_tests(&id)), &|| false).unwrap();
    let _ = holder.kill();
    assert_ne!(done.outcome, Outcome::Applied);
    assert_eq!(common::plan_fixture::tree(&orphan), before);
}

#[test]
fn a_profile_dir_that_appeared_after_the_plan_refuses_the_removal() {
    let (p, orphan) = Planned::with_orphan("orphan-relist");
    let id = p.orphan_plan_id(&orphan);
    std::fs::create_dir_all(orphan.join("release/.fingerprint")).unwrap();
    std::fs::write(orphan.join("release/.cargo-build-lock"), b"").unwrap();
    let done = apply(&p.library_ctx().ctx(), &id, Authorization::Approved(approval_for_tests(&id)), &|| false).unwrap();
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
    std::fs::write(orphan.join("CACHEDIR.TAG"), b"Signature: 8a477f597d28d172789f06886806bc55").unwrap();
    let done = apply(&p.library_ctx().ctx(), &id, Authorization::Approved(approval_for_tests(&id)), &|| false).unwrap();
    assert_eq!(done.outcome, Outcome::Refused);
    assert!(orphan.exists());
}

#[test]
fn a_target_dir_that_gained_an_owner_after_the_plan_is_not_removed() {
    let (p, orphan) = Planned::with_orphan("orphan-owned");
    let id = p.orphan_plan_id(&orphan);
    // a registered checkout is now configured to build into that target dir
    std::fs::create_dir_all(p.worktree.join(".cargo")).unwrap();
    std::fs::write(p.worktree.join(".cargo/config.toml"), format!("[build]\ntarget-dir = \"{}\"\n", orphan.display())).unwrap();
    let done = apply(&p.library_ctx().ctx(), &id, Authorization::Approved(approval_for_tests(&id)), &|| false).unwrap();
    assert_eq!(done.outcome, Outcome::Refused);
    assert!(orphan.exists());
}
```

Run: `cargo test --test plan_tier orphan` — expect: failure, orphan removal not implemented.

- [ ] **Step 2: Implement**

`remove`: `locks::acquire(fs, &target.path, false)`; any `Skipped(reason)` → `Skipped(reason.count_name())`. Under the locks `identify` again and list the profile and triple dirs: a dir not in `OrphanScope::dirs`, or `entry_stat(target)` differing from the recorded kind/device/inode → `Refused`. Re-resolve ownership (`discover::targets`, empty snapshot, all repos): the path must still be an `OrphanCandidate`, else `Refused`. Then `check_entry(fs, target.parent(), &target.path, Expect::Entry{..})` and one `remove_counted(fs, &mut ledger, parent, &target.path, account_bytes, false)`; a `measure` cap hit goes to `unmeasured`. In `apply`, an `Orphan` plan under `Approved` calls `remove` instead of the unit loop.

Run: `cargo test` — expect: pass.

- [ ] **Step 3: Commit**

`git commit -m "plan_tier: remove an approved orphan target whole, after relisting under every lock"`

_Requirements: GCPLAN-5.7, GCPLAN-5.8, GCPLAN-5.9, GCPLAN-5.10_

---

### Task 16: The hive skill teaches the plan flow

**Files:**
- Modify: `plugin/skills/hive/SKILL.md`
- Modify: `skills/hive/eval.json`, `skills/hive/TESTS.md`
- Modify: `tests/skill.rs`
- Test: `tests/skill.rs`

**Reuse:** extends — the existing skill, its drift tests and eval format (rung 2).

**Interfaces:**
- Consumes: the final CLI (`gc --plan`, `apply`)
- Produces: skill text with two fenced command lines carrying `--json` and a path word; the `needs-human`/exit 3 rule; the plan-again rule; eval entries each with a `derived_from` heading present in `skills/hive/TESTS.md`

**Depends-on:** Task 11

- [ ] **Step 1: Write the failing test** — extend `tests/skill.rs`

Add `Parsed::Apply` and `Parsed::Gc { dry_run: false, plan: true }` to the expected set in `skill_documents_every_shipped_command` (the `want` list of `Parsed` values near the end of that test) and add:

```rust
#[test]
fn the_skill_teaches_the_plan_flow_and_its_exit_code() {
    let body = skill_body();
    for needle in ["needs-human", "exit 3", "plan again", "already consumed"] {
        assert!(body.contains(needle), "SKILL.md lacks {needle}");
    }
    assert!(!body.contains("not available yet"), "the plan tier is available now");
}
```

Run: `cargo test --test skill` — expect: failure, the documented commands do not cover `apply`.

- [ ] **Step 2: Implement**

In `SKILL.md`: replace the sentence ending "…is not available yet" (line 24) with the flow, using fenced command lines only (the drift scanner rejects CLI mentions outside command lines): `cargo hive gc --plan <repo-path> --json` returns plan ids per class and `undecided` targets; `cargo hive apply <plan-id> --json` applies one plan. State in prose: an agent applies only `secondary` plans; `needs-human` or exit 3 means report it to the owner and never try to approve a plan yourself; after `refused` plan again and never reuse a consumed id (already consumed); `partial` is not an error, check its reason. Update the Exit codes sentence (line 59) with 3. Keep the body within its 150-line budget. Add the two eval entries to `skills/hive/eval.json` in the existing entry form, each with a `derived_from` naming a new `## ` heading added to `skills/hive/TESTS.md`.

Run: `cargo test --test skill` — expect: pass.

- [ ] **Step 3: Commit**

`git commit -m "skill: teach gc --plan, apply and the needs-human hand-off"`

_Requirements: GCPLAN-7.1, GCPLAN-7.2, GCPLAN-7.3, GCPLAN-7.4, GCPLAN-7.5, GCPLAN-9.9_

---

### Task 17: The deletion trial runs through `execute` on a copy of a real target

**Files:**
- Modify: `tests/common/prove.rs` (`trial_through_execute`, `TryLock::Gone` arm)
- Modify: `tests/liveness.rs`, `tests/liveness_prove.rs`
- Test: `tests/liveness.rs`, `tests/liveness_prove.rs` (`#[ignore]`)

**Reuse:** new — `trial_through_execute` is built on the existing clone, compile-only replay and rebuilt-unit counting of `tests/common/prove.rs`, calling `plan_tier::build::{plan_for_target, Plan::single}` and `plan_tier::apply::execute` (rung 7).

**Interfaces:**
- Consumes: Tasks 3, 7, 9, 12 (`execute`, `ResolvedTarget`, `plan_for_target`)
- Produces: `pub fn trial_through_execute(fx: &Fixture) -> Result<ExecuteTrial, String>` with `#[derive(Debug)] pub struct ExecuteTrial { pub deleted_files: usize, pub rebuilt_live_units: usize }`

**Depends-on:** Task 7, Task 12

- [ ] **Step 1: Write the failing test** in `tests/liveness.rs`

```rust
#[test]
fn deleting_through_execute_on_a_fixture_clone_rebuilds_no_live_unit() {
    let fx = common::cargo_fixture::fixture("execute-trial");
    common::cargo_fixture::cargo(&fx, &["build"]);
    common::cargo_fixture::backdate_all(&fx.target, std::time::Duration::from_secs(10 * 86_400));
    common::cargo_fixture::cargo(&fx, &["build", "--features", "f"]);
    let report = common::prove::trial_through_execute(&fx).expect("trial");
    assert!(report.deleted_files > 0, "a trial that deletes nothing proves nothing");
    assert_eq!(report.rebuilt_live_units, 0, "{report:?}");
}
```

Run: `cargo test --test liveness execute_trial` — expect: compile error, no `trial_through_execute`.

- [ ] **Step 2: Implement**

`trial_through_execute` copies the fixture target with `cp -c -R -p` (the proof's existing clone helper), builds a `TargetDir` of the clone with `read_target`, makes a one-target plan with `plan_for_target` (`cargo_fixture::WINDOW`) and `Plan::single(PlanClass::Secondary, ..)`, resolves it as a `ResolvedTarget` for the clone, calls `execute` with the real `Apfs` and a `Ctx` over a scratch `State`, then replays compile-only exactly as the existing trial does (`cargo_in(&fx.app, &clone, &fx.logs, ..)`) and counts rebuilt units that were live (`prove::rebuilt_live`). Point `tests/liveness_prove.rs` (ignored, real target) at it too, keeping the trial's vacuity checks (no dead file → vacuous).

In `tests/common/prove.rs` change the `TryLock::Gone => {}` arm (line 1002) to `TryLock::Gone if profile.join(".fingerprint").is_dir() => return Err(format!("{}: no build lock file; cannot prove the target idle", profile.display()))`, keeping `TryLock::Gone => {}` otherwise.

Run: `cargo test` — expect: pass; `cargo test --test liveness_prove -- --ignored` runs on the owner's real target when present (owner present).

- [ ] **Step 3: Commit**

`git commit -m "tests: run the deletion trial through execute"`

_Requirements: GCPLAN-8.2, GCPLAN-9.5, GCPLAN-9.10_
