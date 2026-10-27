# CORE plan review: tasks.md prove-claimed against a built crate

Reviewer: independent plan reviewer. Date: 2026-10-27. Plan reviewed: `docs/specs/2026-10-25-foundation/tasks.md` (16 tasks, status Draft).

## Summary

**What I built.** A throwaway crate under `/private/tmp/hive-plan-review-*/`. All 16 tasks were applied in order, with each code block extracted mechanically from tasks.md and every "Modify" edit applied as the prose instructs. Every cargo call used a scratch `CARGO_HOME`. Nothing was written to `~/.cargo`, `~/.rustup`, `~/.hive` or any repo. I checked this afterwards with `find -newer`, and `~/.hive` does not exist. One exception: a single read-only `cargo metadata` call, run to check `--locked` behaviour, used the default CARGO_HOME. It created no files. The scratch dirs are deleted.

**What I ran** (rustc 1.97.1, macOS 27, aarch64, git 2.54.0):
- `cargo build`
- `cargo clippy --all-targets -- -D warnings`
- `cargo fmt --check`
- `cargo test`: the full suite, run repeatedly
- `cargo check --target x86_64-unknown-linux-gnu`, also with `--all-targets` (that target is already installed here, so Task 16 Step 3 does run)
- The `#[ignore]` klynt test was not run, as instructed.

**Results.**

| Gate | Result |
|---|---|
| Every "expect: N passed" count in the plan | Matches reality once the defects below are fixed (list below) |
| `cargo build` | Pass, 0 warnings in the final state |
| `cargo test`, full suite, with `GIT_CONFIG_GLOBAL=/dev/null` | 82 passed, 0 failed, 1 ignored. Two consecutive runs, identical |
| `cargo test`, full suite, as the plan writes it | 81 passed, 1 FAILED (finding 1) |
| `cargo clippy --all-targets -- -D warnings` | FAILS as written at four points (finding 3). Clean after the four one-line fixes |
| `cargo fmt --check` | FAILS: 356 diff hunks (finding 2). `max_width = 200` + `use_small_heuristics = "Max"` still leaves 41 |
| `fingerprint` unit tests | Flaky as written: 2 of 40 runs failed (finding 4). Stable after the fix |
| Non-macOS `cargo check --target x86_64-unknown-linux-gnu` | Pass. With `--all-targets`, 5 warnings in `tests/guards.rs` (finding 14) |

Counts per target in the final state: lib 36 (state 6, receipt 3, platform 2, classify 1, fingerprint 5, apfs 4, local_auth 1, doctor 10, cargo_target 2, size 2). Integration: capability_gate 4, doctor 4, guards 4 (+1 ignored), platform_locks 3, receipts 5, repo 8, status_checkouts 8, status_targets 10.

Per-task counts, all as the plan states them:

| Task | Expected | Observed |
|---|---|---|
| 1 | 5 | 5 |
| 2 | 5 state, 3 receipt, 2 platform, 5 receipts | match |
| 3 | 8 | 8 |
| 4 | 1 | 1 |
| 5 | 8 | 8 |
| 6 | 5 | 5 |
| 7 | 3 | 3 |
| 8 | 4 + 3 | 4 + 3 |
| 9 | 1 | 1 |
| 10 | 7 + 1 | 7 + 1 |
| 11 | 2, and 7 unit | 2, 7 |
| 12 | 10 and 4 | 10 and 4 |
| 13 | 4 and 8 | 4 and 8 |
| 14 | 2+6 unit, 9 integration | 2+6, 9 |
| 15 | 2 and 10 | 2 and 10 |
| 16 | 4 passed + 1 ignored | 3+1 FAILED as written; 4 + 1 ignored after the fix |

Findings: **1 blocker, 6 major, 9 minor** (details below).

## Findings

### Blocker

**1. Task 16, Step 1, `repo_add_status_and_doctor_leave_checkouts_targets_and_cargo_home_unchanged` fails on the proof machine.**
- Evidence: `thread '...' panicked at tests/guards.rs:29:33: called Result::unwrap() on an Err value: Os { code: 102, kind: Unsupported, message: "Operation not supported on socket" }`. Line 29 is `fs::read(&path).unwrap()` in `tree()`.
- Cause: `git config --global core.fsmonitor` is `true` on this Mac (git 2.54.0). The fixtures' `git add`/`git commit`, run from `tests/common::git`, start the fsmonitor daemon. That creates the socket `.git/fsmonitor--daemon.ipc`, which I reproduced with `git init; git add -A` followed by `find . -type s`. `tree()` reads every non-dir, non-symlink entry.
- The plan says these tests "should pass on first run" and that a failure means "stop and report". On the author's machine it fails.
- Fix, three parts:
  - In `tests/common/mod.rs::git`, add `"-c", "core.fsmonitor=false"` to the `-c` list and `.env("GIT_CONFIG_GLOBAL", "/dev/null")` next to `GIT_CONFIG_NOSYSTEM`. This isolates the fixtures from the user's global git config, and also stops them leaving fsmonitor daemons running.
  - In `tree()`, replace the final `else` with `else if meta.is_file() { ...read... "file" } else { "special" }`, so a socket or fifo is never read.
  - Add the same `-c core.fsmonitor=false` to `init_repo`, which already goes through `git()`.
- Verified: with `GIT_CONFIG_GLOBAL=/dev/null` the guard passes (4 passed, 1 ignored).

### Major

**2. `cargo fmt --check` fails on code copied verbatim, and it is a stated per-task gate.**
- Evidence: 356 `Diff in` hunks. First is `src/classify.rs:32`, where `pub fn classify(...)` is a single long line. The plan's Global Constraints say every task must leave `cargo fmt --check` green. No `rustfmt.toml` exists, and none is planned.
- A rustfmt.toml with `max_width = 200` and `use_small_heuristics = "Max"` still leaves 41 hunks, so it is not a one-line fix.
- Fix: either (a) add a "run `cargo fmt`" step before each commit and say the plan's code is a content spec, not rustfmt output, or (b) paste rustfmt-formatted code. Also drop `cargo fmt --check` from "commands every task must leave green" until the code is formatted.

**3. `cargo clippy --all-targets -- -D warnings` fails at four points (the gate is named in every task).**
- (a) Task 2, Step 3/4, `src/receipt.rs`:
  ```
  error: methods called `new` usually return `Self`
    --> src/receipt.rs:66:5   pub fn new(command: &str, scope: Scope) -> ReceiptBuilder
  ```
  `-D clippy::new-ret-no-self`. Fix: add `#[allow(clippy::new_ret_no_self)]` on `Receipt::new`, or rename it to `Receipt::build(...)` (that touches every caller).
- (b) Task 5, Step 3, `src/discover.rs`: `error: variable does not need to be mutable` at `let mut push = |out: &mut Vec<Checkout>, ...|`. Fix: `let push = ...`.
- (c) Task 6, Step 1, `src/fingerprint.rs` tests: `error: very complex type used` at `let cases: Vec<(&str, Box<dyn Fn(&Path)>)>` (`clippy::type_complexity`). Fix: add `#[allow(clippy::type_complexity)]` on that `let`, or `type Corrupt = Box<dyn Fn(&Path)>;`.
- (d) Task 8, Step 1 vs Step 3, `src/platform/apfs.rs`: `error: unused import: std::os::macos::fs::MetadataExt --> ...tests` (`use std::os::macos::fs::MetadataExt as _;` in the test module). Step 3 adds the same import at file level, so `use super::*` already supplies it. Fix: delete that line from the Step 1 test imports (keep `use std::time::{Duration, UNIX_EPOCH};`).
- After these four edits clippy is clean through Task 16.

**4. Flaky unit test: `fingerprint::tests` `scratch()` can collide (about 5%, 2 of 40 runs).**
- Task 6, Step 1. `scratch()` is `temp_dir().join(format!("hive-fp-{pid}-{nanos}"))` with no label or counter, and five tests run in parallel threads in one process.
- Failing runs:
  ```
  edges_resolve_through_...: assertion `left == right` failed: the empty unit dir is ignored   left: 6   right: 3
  unresolved_and_ambiguous_edges_are_counted: left: Some(EdgeCounts { resolved: 3, unresolved: 1, ambiguous: 1 })  right: Some(EdgeCounts { resolved: 0, unresolved: 1, ambiguous: 1 })
  ```
  Two tests got the same directory. Unpatched, 2 of 40 runs of `cargo test --lib fingerprint` failed.
- Fix, with `static NEXT: AtomicU32` added to the other imports:
  ```rust
  fn scratch() -> PathBuf {
      static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
      let k = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
      let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
      let p = std::env::temp_dir().join(format!("hive-fp-{}-{k}-{n}", std::process::id()));
      fs::create_dir_all(p.join(".fingerprint")).unwrap();
      p
  }
  ```
  The other `scratch(label)` helpers (state, doctor, cargo_target, size) use distinct labels per test, so they are not affected.
- Also: `doc_and_run_build_script_units_are_accepted` never removes its scratch dir, so it leaks `hive-fp-*` into `$TMPDIR` on every run. Add `fs::remove_dir_all(p).unwrap();`.

**5. Task 12, Step 2 is not implementable as written.**
- It contains literal placeholders: `let tmp = fs.temp_dir(…)?;            // as before` and a bare `…` line.
- It says "the `attempt` closure now returns `Result<io::Result<Check>, DoctorError>`. Use `Ok(Err(e))` for IO failures … wrap the closure's IO `?` uses accordingly". With that return type, `?` on an `io::Error` does not compile, because `DoctorError` has no `From<io::Error>`. Every `?` on `File::create`, `try_wait`, `spawn` and `create_dir_all` would need a hand-written `map_err`.
- It also never says where `sweep_stale(&env.tmp_parent)` goes relative to `fs.temp_dir`, and the Display text for the `Cancelled` arm is only implied.
- What I implemented, which compiles and passes the plan's 10 unit and 4 integration tests: the closure returns `io::Result<Result<Check, DoctorError>>`, so `?` still works for I/O. The tail is
  ```rust
  match attempt() { Ok(Ok(c)) => Ok(c), Ok(Err(c)) => Err(c), Err(e) => Ok(failed(CheckId::F, e)) }
  ```
  and `return Ok(Err(DoctorError::Cancelled));` replaces the plan's `return Err(...)` inside the wait loop. In `run`: `let stale_removed = sweep_stale(&env.tmp_parent);` goes first, then `temp_dir(...)`, then `checkpoint()?;` before each step and before `check_format(...)?`.
- Fix: paste the complete final `check_format` and `run`, not prose plus `…`.

**6. The crate location and the commit mechanics are unspecified.**
- `cargo-zip/` is not a git repo. The only git repo is the empty `cargo-zip/cargo-hive/` (just `.git`, no commits). The plan's paths (`Cargo.toml`, `src/...`) say "repo root", while the plan also reads `docs/`, `.skills/` and the README refers to `docs/adr/0001`. Those live in `cargo-zip/`, outside the git repo.
- The plan never says whether the crate goes in `cargo-zip/` or `cargo-zip/cargo-hive/`, or what to do about the docs the README links to.
- Every "Commit" step is just `git commit -m "..."` with no `git add` (`grep 'git add\|git init'` finds nothing). `Cargo.lock` is never mentioned. For a binary crate it should be committed.
- Fix: add a preamble such as "Create the crate in `/Users/jayden/Developer/experiments/cargo-zip/cargo-hive` (git repo, `main`). Every commit step is `git add -A && git commit -m ...`." Resolve the docs-outside-the-repo question, and decide on Cargo.lock.

**7. CORE-1.7's observable (the report) is not tested at any seam.**
- This is the known deviation the review brief asked about. The seam table promises integration for CORE-1.7. The plan tests it only as `report.below_floor == true` in a unit test with a fake `DoctorEnv`.
- The user-visible behaviour, "report the version as below the declared floor", is in `doctor_cmd`: the human line and `b.note(line)`. No test asserts that text or note. Delete the `if report.below_floor {...}` block in `cli.rs` and every test still passes (I did not run that mutation).
- Fix, cheapest first:
  - Extract the floor note into a pure `doctor::floor_note(&DoctorReport) -> Option<String>` and unit-test it with `macos = "26.4"` and `"27.0.1"`. This is the best fit for a unit seam.
  - Or add a test-only env override read in `doctor_cmd` (for example `HIVE_TEST_MACOS_VERSION`) and an integration test asserting the receipt `notes`.
- Also note CORE-1.7 is in the Task 10 footer, so that is where the test belongs.

### Minor

**8. Dependencies `objc2 = "0.6.5"` and `objc2-foundation = "0.3.2"` are never used directly.**
- Removing both from Cargo.toml still builds (`cargo build` finished) and nothing in `src/` references `objc2::` or `objc2_foundation`. `objc2-local-authentication` pulls what it needs transitively.
- The Tech Stack paragraph and Task 1 manifest list them as needed. Drop them, or state why they are pinned. `block2` is correctly left out.

**9. Task 8's Depends-on is over-declared, and parallel waves would conflict.**
- Task 8 uses no interface from Task 3. The edge only serializes `cli.rs` and `lib.rs` edits (`Depends-on: Task 3, Task 7`).
- Conversely, Tasks 4, 6 and 7 are independent by Depends-on yet Tasks 4 and 6 both edit `src/lib.rs` (and 3/5/8/10/13/14/15 all edit `cli.rs`). Under `build-in-waves` with parallel lanes they would produce textual merge conflicts.
- Fix: note "Tasks 4 and 6 edit lib.rs; run serially or merge by hand", or add the real ordering edges.

**10. Toolchain resolution deviates from design line 56 and ignores per-checkout pins.**
- Design line 56: `rustup which` runs "with cwd set to the temp dir". The plan's `doctor_cmd` and `status` call `resolve_toolchain(state.root())`, so the cwd is `~/.hive`.
- Separately, the gate compares one toolchain release (resolved from `~/.hive`) for all checkouts. A checkout with its own `rust-toolchain.toml` (klynt has one) is judged against the wrong release if it pins something else. Today klynt's pin and the rustup default are both 1.97.1, so nothing breaks, but CORE-1.10 says "the active one".
- Fix: either say that this is the intended meaning of "active" (the toolchain when run outside any checkout), or resolve per workspace dir.

**11. House rule 3 versus the code.**
- Rule 3: "Requirement IDs never appear in source or test names." Comments in the plan's code contain IDs, for example `src/size.rs:2 (CORE-6.2)`, `discover.rs`, `cargo_target.rs`, `doctor.rs:107`. Test names are clean (checked), so the rule holds if it means names only.
- Fix: state "names" explicitly, or strip the IDs from comments.

**12. Weak or missing assertions against the requirement text.**
- CORE-4.6 (Task 15): the integration test compares the receipt against `allocated_bytes` from the same library. That is a tautology. The unit test checks `st_blocks × 512` and hardlink dedup, which is the real check. An independent check, for example `du`-style via `stat`, would be better.
- CORE-2.5 (Task 2/3): `repo list` human output (path plus root commits) is never asserted with a repo registered. Only `scope.repos` is.
- CORE-3.7 (Task 5): the test asserts `missing` is listed and `counts.missing == 1`. It does not assert the missing worktree is excluded from the protected/secondary counts or from the target scan ("exclude it from every other status result").
- CORE-5.5 (Task 15): the test asserts the `unmeasured` entry. It never asserts the value is absent from `counts`.
- CORE-4.3: the seam table lists integration. The default `~/.cache/cargo-target` is tested only as a state unit test, because `Hive::ready()` replaces `cache_roots` everywhere.
- CORE-6.4: "completes without blocking" is tested with stdin from `/dev/null`, but the process still has the user's GUI session. The requirement says "no GUI session". The test is a smoke test, not the stated verification.
- CORE-6.5 (Task 16): the kill test sleeps 0–25 ms and kills. Nothing guarantees a kill lands mid-write, so it has little power to catch a non-atomic write. Passes, but the claim is probabilistic.
- None of these is a failing test. These are gaps between the requirement text and what the tests prove.

**13. Task 16 guard uses the real `CARGO_HOME`.**
- The guard test overrides `CARGO_HOME` with `cargo_home()` (the real `~/.cargo` when not set), so `status`'s `cargo metadata` runs against the real cargo home. I checked that `cargo metadata --no-deps --locked --offline` writes nothing into an empty home. This conflicts with house rule 4 in spirit ("Tests never write under the real ~/.cargo"). Cargo is not obliged to stay read-only here.
- The `~/.cargo` guard itself only watches `config`, `config.toml`, `env`, `env.fish` and `bin/` names. That is narrower than CORE-7.1 "no file created, modified, or deleted". Say so in the test comment, or snapshot the whole directory listing.

**14. Non-macOS `--all-targets` compile warns.**
- `cargo check --target x86_64-unknown-linux-gnu --all-targets` gives 5 warnings in `tests/guards.rs` (`unused import: git`, and `Entry`, `tree`, `changed`, `cargo_home_marks` never used), because their only user is `#[cfg(target_os = "macos")]`. It is harmless for a macOS-only tool, but `clippy --all-targets -D warnings` on Linux would fail. Put `#![cfg_attr(not(target_os = "macos"), allow(dead_code, unused_imports))]` at the top of the file.
- Task 16 Step 3 as written passes: the target is installed here and `cargo check --target x86_64-unknown-linux-gnu` succeeds.

**15. Leftover process and file hygiene.**
- Because of finding 1, every fixture `git` call leaves a running `git fsmonitor--daemon` until its repo dir disappears. Setting `core.fsmonitor=false` in the helper fixes it.
- Test fixtures in `$TMPDIR` that are not removed on failure (`fingerprint` scratch) leak.

**16. Small wording issues in tasks.**
- Task 9, Step 3 tells the implementer what to do if clippy reports `unused_unsafe`. In reality it does not fire, so the sentence can go.
- Task 14 "Consumes" lists `discover::skipped`, which is private to the same module, so it is not an interface.
- Task 12 Step 1 says to add `use std::process::Command;` "if not already in scope". It is, through `super::*`.

## Verified correct (what was checked)

**Dependency APIs and signatures (all compiled and ran):**
- `libc` 0.2.191: `clonefile`, `getattrlist`, `attrlist`, `ATTR_CMNEXT_{PRIVATESIZE,CLONEID,EXT_FLAGS}`, `FSOPT_ATTR_CMN_EXTENDED`, `ATTR_BIT_MAP_COUNT`, `mkdtemp`, `flock`, `fcntl`/`F_SETLKW`, `UF_COMPRESSED`, `sigaction`/`sigemptyset`, `kill`, `ESRCH`.
- `applesauce` 0.8.10: `FileCompressor::new` / `recursive_compress(paths, Kind::Lzfse, 0.95, 5, &progress, true)`; the `Progress` and `Task` trait methods used (`error`, `file_skipped`, `file_task`, `increment`, `not_compressible_enough`); `SkipReason::HardLink`.
- `objc2-local-authentication` 0.3.2: `LAContext::new`, `canEvaluatePolicy_error`, `LAPolicy::DeviceOwnerAuthentication`, and `localizedDescription`/`code` on the error.
- `clap` 4.6.7 derive: `Cli::try_parse_from`, hidden `#[command(name = "__hold-lock", hide = true)]`, the `flatten`ed `OutputArgs`. `serde_json` 1.0.154 and `serde` 1.0.229.

**Hardcoded test values (right on this machine):**
- `getattrlist` returns 28 bytes. Offsets 4..12 / 12..20 / 20..28 and flags `0x1` / `0x40` hold: clone gives private 0, same clone id, shares-all; rewrite gives a new id.
- A lib+bin fixture crate has `units:<debug>` = 2, `edges_resolved` = 1, 0 unresolved, 0 ambiguous.
- Doctor check (f) on the probe crate: `units=7` (≥ 4), `edges_resolved=9`, `edges_unresolved=0`, `edges_ambiguous=0`, `format=supported`. The fingerprint signature (11 keys, `deps` entries `[u64, string, bool, u64]`, 16 lowercase-hex hash files, little-endian hash resolution) matches Cargo 1.97.1's real output.
- `utc_timestamp` known instants (1_700_000_000 → 2023-11-14T22:13:20Z; 951_782_400 → 2000-02-29).
- All four source hashes in Global Constraints match: `efb97ca2fbc6607e`, `b06c89ff85ddb3e1`, `49753555c2cb1f39`, `6d39ad7c7a2c334e`.

**Behaviour:**
- Registration refusals (all six) and the 2-level `Cargo.toml` rule; `git worktree list -z` parsing including a locked, missing worktree; classification C1 table; realpath comparison for symlinked secondary roots; default `/private/tmp`; bounded 2-level and 3-level walks that skip `node_modules`.
- Doctor: `a–e` pass on this Mac; no TTY; stale sweep; SIGINT and SIGTERM exit 130 and remove the temp dir (repeated runs, no flake); panic leaves no temp dir; `unavailable` disables the capability (the fail-closed rule).
- `cargo metadata --no-deps --locked --offline` works without a `Cargo.lock` and writes nothing. `build.build-dir` yields `build_directory` and the artifact dir is correctly not a target. The metadata cache hits and invalidates on mtime.
- Corrupted/extra-key fingerprint and a newer-layout profile are marked unsupported with no edges. Unreadable target: reported, the others still listed. `CARGO_TARGET_DIR` in the caller env is ignored and noted.
- Concurrent registrations all kept (8 of 8). Kill-during-write leaves parseable files. Cargo builds fresh after hive runs.

**Document-level:**
- Every live requirement ID in requirements.md appears in at least one task's `_Requirements:` footer, and every footer ID is defined (scripted check, both directions).
- Interface names and signatures agree between each "Interfaces", its code and the later users. I checked `Receipt` builder methods, `register`, `Checkout`, `checkouts`, `Config`, `Fs` (9 + 3 methods), `Apfs::new(holder)`, `LockKind`/`LockState`/`LockHolder`, `Platform`/`current()`, `DoctorEnv`/`DoctorReport`/`DoctorError`, `require`/`Capabilities`, `Snapshot`/`TargetScan`/`targets`, `size::allocated_bytes`, and the receipt count names `units:<profile>`, `edges_*:<profile>`, `bytes:<path>`.
- No task uses an interface from a task it does not transitively depend on (apart from over-declared edges in finding 9).
- The Task 12 `SERIAL` rule works: with every `run`-calling test taking the lock, the cancel test does not leak into others.
- No `TODO`/`TBD` placeholders other than the `…` in Task 12 (finding 5).
- Test names carry no requirement IDs.

**Requirement-by-seam coverage:** every ID promised at the integration seam has an integration test, except CORE-1.7 (finding 7) and CORE-4.3 (finding 12). Unit-seam IDs (1.3, 1.6, 3.2, 3.4, 4.7, 4.8, 4.14, 5.3, 5.4) are all tested at the unit seam.

## Disposition (plan author, 2026-10-27)

| # | Outcome |
|---|---|
| 1 | Fixed, in three places. `common::git` uses `-c core.fsmonitor=false` and `GIT_CONFIG_GLOBAL=/dev/null`. `tree()` never reads special files. Production `Git::command` also passes `-c core.fsmonitor=false`, because a user's fsmonitor would otherwise write a socket into a checkout's `.git` (CORE-7.1). |
| 2 | Fixed. The gate is now "run `cargo fmt`, then `cargo fmt --check`", and the plan says rustfmt owns layout. Every commit step runs `cargo fmt` first. |
| 3 | Fixed: all four, as suggested. |
| 4 | Fixed: a counter in `scratch()`, and the leaked dir is removed. |
| 5 | Fixed. Task 12 Step 2 now carries the complete `check_format` and `run`, with nesting `io::Result<Result<Check, DoctorError>>`, the sweep first, and a `checkpoint` before each step. |
| 6 | Planned, pending the user. A "Repository and setup" section puts the crate at the `cargo-zip/` root, beside `docs/`. Setup is `git init -b main` with user approval and an initial commit of the existing files. `.gitignore` covers `/.remember/` and `/target`, `Cargo.lock` is committed, and commits use `git add -A`. The empty `cargo-hive/` repo is left for the user to decide on. |
| 7 | Fixed. `doctor::floor_note` is pure, unit-tested for 26.4 and 27.0.1, and used by `doctor_cmd`. |
| 8 | Fixed. `objc2` and `objc2-foundation` are dropped from Cargo.toml; Tech Stack says they arrive transitively. |
| 9 | Kept Task 8 → Task 3, which serializes `cli.rs` dispatch edits. Tasks 4 and 6 now depend on Task 1. Added a "Shared files" note for `lib.rs` and `cli.rs`. |
| 10 | Documented. "Active toolchain" means the one resolved outside any checkout, from the state dir, for both doctor and the gate. The deviation from Decision 7 is recorded in Global Constraints. |
| 11 | Fixed. IDs are stripped from code comments, and house rule 3 now says "identifiers or comments". |
| 12 | Strengthened CORE-2.5 (human `repo list`), CORE-3.7 (missing appears in no other result) and CORE-5.5 (omitted from counts). Accepted as-is: CORE-4.6, where the unit test against `st_blocks × 512` is the real check; CORE-4.3, covered by the state unit test, because integration tests must not walk the real cache; CORE-6.4, which cannot drop the GUI session on the proof machine, so the no-TTY run is the closest test; and CORE-6.5, where the kill test is probabilistic while the atomic-write protocol is unit-tested by construction. |
| 13 | Kept on purpose: the guard has to watch the real `~/.cargo`, and you verified `cargo metadata` writes nothing. A comment now explains the narrow marks. |
| 14 | Fixed: `cfg_attr` allow at the top of `tests/guards.rs`. |
| 15 | Fixed by 1 and 4. |
| 16 | Fixed: all three wording issues. |
