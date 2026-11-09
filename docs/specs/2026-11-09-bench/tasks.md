# Tasks: Bench (measured cold-versus-seeded first builds)

> **For agentic workers:** after plan approval, pick one execute skill:
> - `build-in-waves`: a continuous, dependency-aware scheduler;
> - `build-by-story`: human-gated story review units;
> - `build-inline`: the controller implements, with no implementer subagents.
>
> The chosen skill writes `Execution-mode:`. Steps use checkbox (`- [ ]`) syntax for tracking.

Feature code: BENCH
Status: Approved
Approved: 2026-11-09 14:58 by the operator (Jayden), "approve" on the spec sheet for #6 at feat/bench 0be0657, relayed by the conductor (/Users/jayden/Developer/experiments/cargo-zip/.skills/_pending-bench/auto/operator-reply.md). The reply approves the sheet as a whole; the Open Questions in requirements.md stay open as listed, and the owner-reserved sampling pick (S1) stays labelled as such.
Date: 2026-11-09
Execution-mode: continuous
Max-concurrency: auto
Requirements: ./requirements.md
Design: ./design.md

**Goal:** Add `cargo hive bench --cold <checkout>... --seeded <checkout>...`, which primes the store, then
times cold and seeded first builds of fresh secondary checkouts in fixed ABBA order, and reports per sample
and per arm integer milliseconds, load averages and seed counts in a schema-2 receipt.

**Architecture:**
- **`src/bench/`** (new) holds the run (`mod.rs`), the refusals (`check.rs`), the build shapes and env map
  (`command.rs`), and the pure order and statistics (`order.rs`, `stats.rs`). Its public surface is `check`,
  `run`, `Plan`, `Limits` and `Report`.
- **Prefactors** split `seed::run` into `run_fill` and `run_clone`, and move `seed_cmd`'s per-checkout gate
  into `cli::locate_checkout`, both with seed's behavior unchanged.
- **Small extensions:** `cargo_target::run_bounded` (a bounded child runner), `Git::head` and
  `Git::has_tracked_changes`, `Fs::load_average_hundredths` with a default body.

**Tech Stack:** Rust 2021, rustc/cargo 1.97.1 on macOS 27 (APFS). Existing dependencies only: `serde`,
`serde_json`, `clap`, `libc`, `applesauce`. No new dependency. Integration tests run the real binary
(`env!("CARGO_BIN_EXE_cargo-hive")`) or the library in process, on scratch fixtures.

## Global Constraints

The canonical sources (`docs/standards/` is the SSOT). Read them; do not restate them. Hashes were taken
2026-11-09; a mismatch at execute time means the source changed after planning.

| Source | sha256 (first 16) |
|---|---|
| `docs/standards/INDEX.md` | `80cd41328c0f7652` |
| `docs/standards/code.md` (bounds on every walk, read and child process; `unsafe` only at FFI with `// SAFETY:`; no new deps; no requirement IDs in code, comments or test names; `//!` module comments) | `afdcc8b89136f573` |
| `docs/standards/errors-logging.md` (typed errors with hand-written `Display`; expected refusals are data; exit codes) | `48f5baac3b52a552` |
| `docs/standards/testing.md` (merge gate; scratch-only fixtures under `/private/tmp/hive-it-*`; `HIVE_HOME` is a test-only variable; no test-only branches in production paths) | `6659c2301761caaf` |
| `docs/architecture/INDEX.md`: this feature relies on ARCH-1, 2, 4 to 8, 10, 13 to 19 | `36f62957f3f4ca1b` |
| `docs/specs/2026-11-09-bench/design.md` (Decisions 1 to 12, sections 1 to 10) | see git |
| `/Users/jayden/Developer/experiments/cargo-zip/.skills/_pending-bench/auto/design-scan.md` (signatures and line numbers), `design-review.md`, `close-package.md` | not in git |

Merge gate (testing standard). Every task leaves all four green: `cargo fmt --check` (after `cargo fmt`),
`cargo clippy --all-targets -- -D warnings`, `cargo test`, `cargo check --target x86_64-unknown-linux-gnu`
(record "non-macOS compile check not run: target not installed" when it is absent; never install it).

Commits: each commit step is `cargo fmt`, then `git add -A && git commit -m "<subject>"` on `feat/bench`. No
`Co-Authored-By` or generated-by line.

House rules for this plan:
1. `seed`, `status`, `gc`, `apply`, `repo`, `doctor` and `explain` behave and report exactly as before. Their
   existing tests are the guard, and none of them may be edited to pass. One exception:
   `the_cli_has_no_new_subcommand_or_flag` (tests/liveness.rs:186-226) pins the whole CLI surface, and Task 14
   adds bench's five lines to its `expected` list.
2. No requirement ID appears in source, comments or test names.
3. Bench writes no repo file, creates or removes no checkout or target, and adds no state file. Its only
   state writes are the ones `seed::run_fill`/`run_clone` already make and the receipt line.
4. Receipt keys bench adds never contain `seconds`, `duration`, `elapsed`, `saved` or `estimate`. Times end in
   `_ms`, load averages in `_x100`.
5. Adding `Command::Bench` breaks `tests/skill.rs` at compile time. Task 14 adds `Parsed::Bench` in the same
   commit.
6. Failure, cancel and the deadline are injected through `Limits`, the `cancel` closure and fixtures, never
   through an environment variable or a test-only branch.
7. In-process tests that run a seed, `check` or `cargo metadata` take `env_lock()` and set `CARGO_HOME` with
   `CargoHomeGuard`, because registry classification reads the process's `CARGO_HOME`. Task 1 moves both from
   tests/seed.rs:2572-2597 into `tests/common/mod.rs` as `pub`. The timed build gets its environment from the
   `EnvMap` the test builds: the process environment with `CARGO_HOME` set to the scratch home, the same
   environment `cargo_json` gives the fixture's own builds (tests/common/registry_fixture.rs:136-145).
8. `hive.json(&[...])` appends `--json` itself; passing it again exits 2.

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `src/seed/mod.rs` | modify | Split `run` into `run_fill` (`Fill`, `Filled`) and `run_clone`; `run` composes them |
| `src/cli.rs` | modify | `locate_checkout` and `Refusal` extracted from `seed_cmd`; `Command::Bench`, its dispatch arm and `bench_cmd`; a new `#[cfg(test)] mod tests` |
| `src/cargo_target.rs` | modify | `run_bounded`, `Capture`, `Ran`, `kill_pgid`; `kill_group` calls `kill_pgid` |
| `src/git.rs` | modify | `Git::head`, `Git::has_tracked_changes` through a private read-only builder; a new `#[cfg(test)] mod tests` |
| `src/platform/mod.rs` | modify | `Fs::load_average_hundredths` with a default body |
| `src/platform/apfs.rs` | modify | The `libc::getloadavg` override |
| `tests/common/recording_fs.rs` | modify | `Recording` forwards `load_average_hundredths` |
| `src/lib.rs` | modify | `pub mod bench;` |
| `src/bench/mod.rs` | create | `Plan`, `Limits`, `End`, `Report`, `run`: prime, samples, failure, cancel, summaries |
| `src/bench/order.rs` | create | `Arm`, `abba` |
| `src/bench/stats.rs` | create | `Summary`, `summarize` |
| `src/bench/command.rs` | create | `BuildCmd`, `EnvMap`, `env_map` |
| `src/bench/check.rs` | create | `Checked`, `Refusal` list, `check` |
| `tests/bench.rs` | create | In-process `bench::run` and `bench::check` tests, and binary tests of `cargo hive bench` |
| `tests/seed.rs` | modify | One characterization test that `run_fill` then `run_clone` equals `run`; uses the moved helpers |
| `tests/common/mod.rs` | modify | `pub fn env_lock()` and `pub struct CargoHomeGuard`, moved from tests/seed.rs |
| `tests/liveness.rs` | modify | `the_cli_has_no_new_subcommand_or_flag` lists bench's five surface lines |
| `tests/skill.rs` | modify | `Parsed::Bench`, the wants list, the path-required set, a bench teaching test |
| `src/receipt.rs` | modify | Module doc names bench as the one command whose receipt holds integer milliseconds |
| `plugin/skills/hive/SKILL.md` | modify | Bench inside the seven headings |
| `README.md` | modify | A `cargo hive bench` row in the command table |
| `skills/hive/eval.json` | modify | One behavior entry for bench |
| `skills/hive/TESTS.md` | modify | The heading that entry's `derived_from` names |

---

### Task 1: Seed runs as a fill half and a clone half, with nothing seed reports changing

**Files:**
- Modify: `src/seed/mod.rs` (`run`, lines 179 to 261; the early returns at 191 and 195 to 199)
- Modify: `tests/common/mod.rs` (receives `env_lock` and `CargoHomeGuard`)
- Test: `tests/seed.rs`

**Reuse:** rung 2 — extract from the existing `seed::run` (src/seed/mod.rs:179-261) and `seed_cmd`
(src/cli.rs:1432-1506).

**Interfaces:**
- Consumes: `source::owned_targets`, `fill::fill`, `from_fill`, `empty_fill`, `place::place`, `record_placed`
  as today
- Produces: `pub enum Fill { Continue(Filled), Stopped(Outcome) }`; `pub struct Filled` with
  `pub fn report(&self) -> &fill::FillReport` and
  `pub fn findings(&self) -> &[(String, Option<PathBuf>, String)]`;
  `pub fn run_fill(ctx: &Ctx<'_>, repo: &Repo, cancel: &(dyn Fn() -> bool + Sync)) -> Result<Fill, Error>`;
  `pub fn run_clone(ctx: &Ctx<'_>, repo: &Repo, checkout: &Checkout, toolchain: &Toolchain, filled: Filled, cancel: &(dyn Fn() -> bool + Sync)) -> Result<Outcome, Error>`;
  `run` keeps its signature.

**Depends-on:** none

- [x] **Step 1: Write the failing test** in `tests/seed.rs`, next to `run_seed` (line 1892), reusing
  `run_seed`'s locate block:

```rust
#[test]
fn seeding_through_the_fill_and_clone_halves_reports_what_one_run_reports() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let whole = run_seed(&hive, Recording::over_real(), &repo.worktree("whole")).unwrap();
    let lane = repo.worktree("halves");
    let halves = {
        let _env = env_lock().lock().unwrap_or_else(|err| err.into_inner());
        let _home = CargoHomeGuard::set(&hive.cargo_home.path);
        let lib = LibCtx::open(&hive, Recording::over_real().fs());
        let (repo_row, checkout) = locate(&lib, &lane);
        let toolchain = cargo_hive::doctor::resolve_toolchain(&lane).unwrap();
        let repos = [repo_row];
        let ctx = lib_ctx(&lib, &repos);
        match cargo_hive::seed::run_fill(&ctx, &repos[0], &|| false).unwrap() {
            cargo_hive::seed::Fill::Continue(filled) => cargo_hive::seed::run_clone(
                &ctx, &repos[0], &checkout, &toolchain, filled, &|| false,
            )
            .unwrap(),
            cargo_hive::seed::Fill::Stopped(_) => panic!("nothing cancelled this fill"),
        }
    };
    let keys = |o: &cargo_hive::seed::Outcome| o.counts.keys().cloned().collect::<Vec<_>>();
    assert_eq!(keys(&whole), keys(&halves));
    assert_eq!(whole.counts["clone_units_cloned"], halves.counts["clone_units_cloned"]);
    assert!(!halves.cancelled);
}
```

  `locate` and `lib_ctx` are the locate loop and the `Ctx` literal of `run_seed_until` (tests/seed.rs:1907-1929),
  moved into two helpers that `run_seed_until` then calls. In the same step, move `env_lock` and
  `CargoHomeGuard` (tests/seed.rs:2572-2597) into `tests/common/mod.rs` as `pub`, and have tests/seed.rs
  import them. Run: `cargo test --test seed seeding_through_the_fill_and_clone_halves`.
  Expect: a compile error, because `seed::run_fill` does not exist.
- [x] **Step 2: Implement.** Move lines 186 to 199 into `run_fill`. A cancel at either point returns
  `Ok(Fill::Stopped(from_fill(...)))` with exactly today's arguments. Otherwise it returns
  `Ok(Fill::Continue(Filled { report, findings }))`. Move lines 200 to 260 into `run_clone`. `run` becomes
  `match run_fill(..)? { Fill::Stopped(o) => Ok(o), Fill::Continue(f) => run_clone(.., f, cancel) }`.
- [x] **Step 3: Run** `cargo test --test seed` and `cargo test --lib seed::`. Expect every existing seed test
  to pass unchanged, plus the new one.
- [x] **Step 4: Commit** `refactor(seed): run as a fill half and a clone half`

_Requirements: BENCH-9.1_

---

### Task 2: Seed's per-checkout gate becomes `locate_checkout`, with seed's refusals unchanged

**Files:**
- Modify: `src/cli.rs` (`seed_cmd`, lines 1403 to 1551; the per-checkout block is 1433 to 1506)
- Test: `tests/seed.rs` (existing refusal tests at lines 164, 173, 187 and 212 are the guard)

**Reuse:** rung 2 — extract from the existing `seed::run` (src/seed/mod.rs:179-261) and `seed_cmd`
(src/cli.rs:1432-1506).

**Interfaces:**
- Consumes: `discover::checkouts`, `doctor::resolve_toolchain`, `doctor::toolchain_failure_exit`,
  `doctor::require`
- Produces: `pub(crate) struct Refusal { pub kind: &'static str, pub path: Option<PathBuf>, pub detail: String, pub code: i32 }`;
  `pub(crate) fn locate_checkout(reg: &Registry, cfg: &Config, caps: &Capabilities, path: &Path) -> Result<(Repo, Checkout, Toolchain), Refusal>`.
  It computes `registered_mains` inside, as line 1445 does. `kind` is `"refusal"`, or `"error"` for a
  cancelled toolchain lookup, as line 1474 decides. `path` is the argument as given for a canonicalize
  failure, else the real path.

**Depends-on:** none

- [ ] **Step 1: Write the failing test** in a new `#[cfg(test)] mod tests` at the end of `src/cli.rs` (it has none
  today):

```rust
#[test]
fn locating_a_missing_path_refuses_with_the_path_as_given() {
    let reg = Registry::default();
    let cfg = Config { secondary_roots: vec![], cache_roots: vec![], gc_window_days: None };
    let caps = Capabilities::default();
    let given = Path::new("/private/tmp/hive-it-no-such-lane");
    let refusal = locate_checkout(&reg, &cfg, &caps, given).unwrap_err();
    assert_eq!(refusal.kind, "refusal");
    assert_eq!(refusal.path.as_deref(), Some(given));
    assert!(refusal.detail.contains("pass a checkout of a registered repository"), "{}", refusal.detail);
    assert_eq!(refusal.code, 1);
}
```

  Run: `cargo test --lib cli::`. Expect: a compile error, because `locate_checkout` does not exist.
  (`Registry` and `Capabilities` derive `Default`; `Config` does not, src/state.rs:192-199.)
- [ ] **Step 2: Implement.** Move lines 1433 to 1506 into `locate_checkout`. `seed_cmd` keeps the
  `window_days` check and the signal install (1420 to 1431), calls `locate_checkout`, and on `Err(r)` does
  `b.finding(r.kind, r.path, r.detail.clone())` and `emit(state, out, b.finish(), &format!("refused: {}\n", r.detail), r.code)`.
  The text, order and codes stay as today.
- [ ] **Step 3: Run** `cargo test --test seed` and `cargo test --lib cli::`. Expect the refusal tests at
  tests/seed.rs:119, 164, 173, 187 and 212 to pass unchanged, plus the new one.
- [ ] **Step 4: Commit** `refactor(cli): locate and gate a checkout in one place`

_Requirements: BENCH-9.1_

---

### Task 3: A child can run to a deadline with capped output and a measured exit time

**Files:**
- Modify: `src/cargo_target.rs` (`kill_group` line 220; add after `output_with_deadline`, line 337)
- Test: `src/cargo_target.rs`

**Reuse:** rung 2 — `ChildOutputError` (src/cargo_target.rs:214) and the group-kill call inside `kill_group`
(:220), which moves into `kill_pgid(pid)` so the waiter thread can own the `Child`; rung 3 — `std::process`,
`std::sync::mpsc`, `std::time::Instant`. The waiter-thread runner itself is new (rung 7).

**Interfaces:**
- Consumes: nothing new
- Produces: `pub struct Capture { pub stdout_cap: usize, pub stderr_cap: usize }` (0 means `Stdio::null()`);
  `#[derive(Debug)] pub struct Ran { pub status: ExitStatus, pub exited_after: Duration, pub stdout: Vec<u8>, pub stderr_tail: Vec<u8> }`;
  `pub fn run_bounded(cmd: &mut Command, deadline: Duration, capture: Capture, cancel: impl FnMut() -> bool) -> Result<Ran, ChildOutputError>`;
  `fn kill_pgid(pid: u32)`.

**Depends-on:** none

- [x] **Step 1: Write the failing tests** in `src/cargo_target.rs`'s test module:

```rust
fn sh(script: &str) -> Command {
    let mut cmd = Command::new("/bin/sh");
    cmd.args(["-c", script]);
    cmd
}

#[test]
fn a_child_that_exits_reports_its_status_and_when_it_exited() {
    let ran = run_bounded(&mut sh("sleep 0.2; exit 3"), Duration::from_secs(10),
        Capture { stdout_cap: 0, stderr_cap: 1024 }, || false).unwrap();
    assert_eq!(ran.status.code(), Some(3));
    assert!(ran.exited_after >= Duration::from_millis(200), "{:?}", ran.exited_after);
    assert!(ran.exited_after < Duration::from_secs(5));
}

#[test]
fn output_past_its_cap_keeps_only_the_cap() {
    let ran = run_bounded(&mut sh("head -c 100000 /dev/zero; head -c 100000 /dev/zero >&2"),
        Duration::from_secs(10), Capture { stdout_cap: 4096, stderr_cap: 4096 }, || false).unwrap();
    assert_eq!(ran.stdout.len(), 4096);
    assert_eq!(ran.stderr_tail.len(), 4096);
}

#[test]
fn output_at_its_cap_is_kept_whole() {
    let ran = run_bounded(&mut sh("head -c 4096 /dev/zero >&2"), Duration::from_secs(10),
        Capture { stdout_cap: 0, stderr_cap: 4096 }, || false).unwrap();
    assert_eq!(ran.stderr_tail.len(), 4096);
}

#[test]
fn a_child_past_its_deadline_is_killed_with_its_group() {
    let start = Instant::now();
    let err = run_bounded(&mut sh("sleep 30 & sleep 30"), Duration::from_millis(300),
        Capture { stdout_cap: 0, stderr_cap: 0 }, || false).unwrap_err();
    assert!(matches!(err, ChildOutputError::TimedOut));
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_cancel_kills_the_child() {
    let start = Instant::now();
    let err = run_bounded(&mut sh("sleep 30"), Duration::from_secs(60),
        Capture { stdout_cap: 0, stderr_cap: 0 }, || start.elapsed() > Duration::from_millis(200)).unwrap_err();
    assert!(matches!(err, ChildOutputError::Cancelled));
    assert!(start.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_grandchild_holding_stderr_open_does_not_delay_the_result() {
    let start = Instant::now();
    let ran = run_bounded(&mut sh("(sleep 20 >&2 &) ; exit 0"), Duration::from_secs(60),
        Capture { stdout_cap: 0, stderr_cap: 1024 }, || false).unwrap();
    assert_eq!(ran.status.code(), Some(0));
    assert!(start.elapsed() < Duration::from_secs(5), "{:?}", start.elapsed());
}
```

  Run: `cargo test --lib cargo_target::`. Expect: a compile error, because `run_bounded` does not exist.
- [x] **Step 2: Implement.** `run_bounded` sets stdin null, sets each stream to `Stdio::null()` for a 0 cap or
  piped otherwise, and calls `process_group(0)`. It takes an `Instant` just before `spawn`. A waiter thread
  owns the `Child`, calls `wait()`, and sends `(status, start.elapsed())` on an `mpsc` channel. The main loop
  does `recv_timeout(20 ms)`, checking `cancel()` and the deadline between slices, and calls `kill_pgid(pid)`
  then waits for the waiter on either. Reader threads keep a ring of the last `cap` bytes for stderr and the
  first `cap` bytes for stdout, and keep reading so the pipe never blocks the child. After exit, each reader
  gets 1 s to finish and is then left detached. `kill_group(child)` becomes `kill_pgid(child.id())`.
  Module and function comments describe the bound.
- [x] **Step 3: Run** `cargo test --lib cargo_target::`. Expect pass, including the existing
  `output_with_deadline` tests.
- [x] **Step 4: Commit** `feat(cargo_target): run a child to a deadline with capped output and its exit time`

_Requirements: BENCH-9.6, BENCH-10.2_

---

### Task 4: Git reports a checkout's HEAD and tracked changes without touching its index

**Files:**
- Modify: `src/git.rs` (add beside `Git::command`, line 52, and the pub fns at 90 to 152)
- Test: `src/git.rs`

**Reuse:** rung 2 — `Git` (src/git.rs:41), its `GIT_*` removal pattern (:52-64) and `GitError`; the bounded
runner (section 7) with `METADATA_DEADLINE` (60 s) and `Capture { stdout_cap: 64 KiB, stderr_cap: 4 KiB }`.

**Interfaces:**
- Consumes: `cargo_target::{run_bounded, Capture, Ran, METADATA_DEADLINE}` (Task 3)
- Produces: `pub fn head(&self) -> Result<String, GitError>`;
  `pub fn has_tracked_changes(&self) -> Result<bool, GitError>`; a private `fn read_only(&self) -> Command`
  building `git --no-optional-locks -c core.fsmonitor=false -C <dir>` with `GIT_*` removed

**Depends-on:** Task 3

- [ ] **Step 1: Write the failing tests** in a new `#[cfg(test)] mod tests` at the end of `src/git.rs` (it has
  none today). Its helper `scratch_repo(label) -> ScratchRepo` makes `/private/tmp/hive-it-<label>-<pid>` and
  returns a guard whose `Drop` removes that directory, so no fixture outlives its test
  (docs/standards/testing.md:34); `ScratchRepo::path(&self) -> &Path`. It runs
  `git init -q`, writes `a`, `git add a` and `git -c user.name=t -c user.email=t@t commit -qm a`, every call
  with `GIT_CONFIG_GLOBAL=/dev/null` and `GIT_CONFIG_NOSYSTEM=1` so the machine's git config cannot leak in:

```rust
#[test]
fn reading_a_touched_but_unchanged_tree_leaves_the_index_bytes_unchanged() {
    let repo = scratch_repo("bench-git");
    let dir = repo.path();
    let index = dir.join(".git/index");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(dir.join("a"), std::fs::read(dir.join("a")).unwrap()).unwrap(); // same bytes, new mtime
    let before = std::fs::read(&index).unwrap();   // plain `git status` would refresh this stat into the index
    let g = Git::at(dir);
    assert_eq!(g.head().unwrap().len(), 40);
    assert!(!g.has_tracked_changes().unwrap());
    assert_eq!(std::fs::read(&index).unwrap(), before);
}

#[test]
fn a_changed_tracked_file_is_a_change_and_an_untracked_file_is_not() {
    let repo = scratch_repo("bench-git-changes");
    let dir = repo.path();
    std::fs::write(dir.join("new"), "x").unwrap();
    assert!(!Git::at(dir).has_tracked_changes().unwrap());
    std::fs::write(dir.join("a"), "changed").unwrap();
    assert!(Git::at(dir).has_tracked_changes().unwrap());
}
```

  Run: `cargo test --lib git::`. Expect: a compile error, because `head` does not exist. (The plan review
  checked in a scratch repo that plain `git status` rewrites the index after a same-content touch, and that
  `--no-optional-locks status` does not.)
- [ ] **Step 2: Implement.** `head` runs `rev-parse --verify HEAD` and `has_tracked_changes` runs
  `status --porcelain --untracked-files=no`, both via `read_only()` through
  `run_bounded(cmd, METADATA_DEADLINE, Capture { stdout_cap: 64 * 1024, stderr_cap: 4 * 1024 }, || false)`.
  A non-zero exit maps to `GitError::Failed`, and a timeout to `GitError::Failed` with the deadline in
  `stderr`. `has_tracked_changes` is `!ran.stdout.is_empty()`. `Git::command` and `Git::run` stay as they
  are.
- [ ] **Step 3: Run** `cargo test --lib git::` and `cargo test --test status_checkouts`. Expect pass.
- [ ] **Step 4: Commit** `feat(git): read HEAD and tracked changes with optional locks off`

_Requirements: BENCH-2.10, BENCH-9.5_

---

### Task 5: The filesystem layer reads the 1-minute load average

**Files:**
- Modify: `src/platform/mod.rs` (`trait Fs`, lines 295 to 326)
- Modify: `src/platform/apfs.rs` (`impl Fs for Apfs`, line 47)
- Modify: `tests/common/recording_fs.rs` (`impl Fs for Recording`, line 234)
- Test: `src/platform/apfs.rs`

**Reuse:** rung 5 — `libc::getloadavg` (already a dependency; libc-0.2.191 src/unix/bsd/mod.rs:623)

**Interfaces:**
- Consumes: nothing new
- Produces: `fn load_average_hundredths(&self) -> io::Result<u64>` on `Fs`, with a default body
  `Err(io::Error::from(io::ErrorKind::Unsupported))`; `Apfs` and `Recording` override it

**Depends-on:** none

- [x] **Step 1: Write the failing test** in `src/platform/apfs.rs`'s test module, using its `apfs()` helper:

```rust
#[test]
fn the_load_average_reads_as_hundredths() {
    let first = apfs().load_average_hundredths().unwrap();
    assert!(first < 100_000, "{first}");
}
```

  Run: `cargo test --lib platform::`. Expect: a compile error, because the method does not exist.
- [x] **Step 2: Implement.** Add the defaulted method to `Fs`. In `Apfs`, call `libc::getloadavg` with a
  one-element `[f64; 1]` and `nelem = 1`, under a `// SAFETY:` comment naming the buffer length. A return
  other than 1 is `io::Error::last_os_error()`. Then compute `(value * 100.0).floor() as u64`. `Recording`
  forwards to `self.state.fs` and logs nothing for it, so existing log assertions do not change. No other `Fs` impl changes.
- [x] **Step 3: Run** `cargo test` and `cargo check --target x86_64-unknown-linux-gnu`. Expect pass.
- [x] **Step 4: Commit** `feat(platform): read the 1-minute load average`

_Requirements: BENCH-1.6_

---

### Task 6: Samples have a fixed ABBA order and a series has integer statistics

**Files:**
- Create: `src/bench/order.rs`, `src/bench/stats.rs`, `src/bench/mod.rs` (module declarations and a `//!`
  comment only, for now)
- Modify: `src/lib.rs` (add `pub mod bench;`)
- Test: `src/bench/order.rs`, `src/bench/stats.rs`

**Reuse:** none — new code (rung 7). The repo has no statistics helpers (digest §12), and std has no median.

**Interfaces:**
- Consumes: nothing
- Produces: `pub enum Arm { Cold, Seeded }` (`Copy, Debug, PartialEq, Eq`); `pub fn abba(n: usize) -> Vec<(Arm, usize)>`;
  `pub struct Summary { pub median: u64, pub p90: u64, pub min: u64, pub max: u64, pub spread: u64 }`;
  `pub fn summarize(values: &[u64]) -> Option<Summary>`

**Depends-on:** none

- [x] **Step 1: Write the failing tests**:

```rust
// src/bench/order.rs
#[test]
fn samples_alternate_cold_seeded_seeded_cold() {
    use Arm::*;
    assert_eq!(abba(2), vec![(Cold, 0), (Seeded, 0), (Seeded, 1), (Cold, 1)]);
    assert_eq!(abba(3), vec![(Cold, 0), (Seeded, 0), (Seeded, 1), (Cold, 1), (Cold, 2), (Seeded, 2)]);
    assert_eq!(abba(4)[4..], [(Cold, 2), (Seeded, 2), (Seeded, 3), (Cold, 3)]);
}

// src/bench/stats.rs
#[test]
fn a_series_summarizes_by_nearest_rank_in_integers() {
    // (values, median, p90)
    for (values, median, p90) in [
        (vec![5], 5, 5),
        (vec![10, 30], 20, 30),
        (vec![10, 31], 20, 31),         // floor of the mean of the two middles
        (vec![3, 1, 2], 2, 3),
        ((1..=9).collect::<Vec<u64>>(), 5, 9),   // n = 9: p90 is the max
        ((1..=10).collect::<Vec<u64>>(), 5, 9),  // n = 10: rank 9, not the max
        ((1..=11).collect::<Vec<u64>>(), 6, 10), // ceil(99/10) = 10
    ] {
        let s = summarize(&values).unwrap();
        assert_eq!((s.median, s.p90), (median, p90), "{values:?}");
        assert_eq!(s.spread, s.max - s.min);
    }
    assert!(summarize(&[]).is_none());
}

#[test]
fn the_mean_of_two_huge_middles_does_not_overflow() {
    assert_eq!(summarize(&[u64::MAX - 1, u64::MAX]).unwrap().median, u64::MAX - 1);
}
```

  Run: `cargo test --lib bench::`. Expect: a compile error.
- [x] **Step 2: Implement.** `abba` walks pairs i = 0..n: on even i push `(Cold, i), (Seeded, i)`, on odd i
  push `(Seeded, i), (Cold, i)`. `summarize` sorts a copy. The median at even n is
  `a / 2 + b / 2 + (a % 2 + b % 2) / 2`, and p90 is `sorted[(9 * n).div_ceil(10) - 1]`. Each file opens with a
  `//!` comment.
- [x] **Step 3: Run** `cargo test --lib bench::`. Expect pass.
- [x] **Step 4: Commit** `feat(bench): ABBA sample order and integer statistics`

_Requirements: BENCH-1.2, BENCH-1.9_

---

### Task 7: The timed build is one of five compile-only shapes, always offline and locked

**Files:**
- Create: `src/bench/command.rs` (the `BuildCmd` half)
- Modify: `src/bench/mod.rs` (`pub mod command;`)
- Test: `src/bench/command.rs`

**Reuse:** rung 2 — `cargo_target::TARGET_VARS` (src/cargo_target.rs:15-19) for the ARCH-13 floor, `Toolchain` (src/doctor.rs:133) for the pin, and doctor's `RUSTC` + `RUSTUP_TOOLCHAIN` pin pattern (src/doctor.rs:699-719); the build shapes and wrapper rules are new (rung 7)

**Interfaces:**
- Consumes: nothing
- Produces: `pub enum BuildCmd { Build, CheckAll, ClippyAll, TestNoRun, CheckLib(String) }` (`Clone, Debug,
  PartialEq, Eq, Default` = `Build`); `impl std::str::FromStr for BuildCmd` (`build`, `check-all`,
  `clippy-all`, `test-no-run`, `check-lib=<package>` with the package matching `[A-Za-z0-9_-]+` and not
  starting with `-`); `pub fn args(&self) -> Vec<String>`; `pub fn command_line(&self) -> String`

**Depends-on:** Task 6

- [ ] **Step 1: Write the failing test**:

```rust
#[test]
fn each_shape_is_its_cargo_command_offline_and_locked() {
    for (word, args) in [
        ("build", vec!["build"]),
        ("check-all", vec!["check", "--all-targets"]),
        ("clippy-all", vec!["clippy", "--all-targets"]),
        ("test-no-run", vec!["test", "--no-run"]),
        ("check-lib=app", vec!["check", "-p", "app", "--lib"]),
    ] {
        let cmd: BuildCmd = word.parse().unwrap();
        let mut want: Vec<String> = args.into_iter().map(String::from).collect();
        want.extend(["--offline".into(), "--locked".into()]);
        assert_eq!(cmd.args(), want, "{word}");
        assert_eq!(cmd.command_line(), format!("cargo {}", want.join(" ")));
    }
    assert_eq!(BuildCmd::default(), BuildCmd::Build);
    for bad in ["run", "check-lib=", "check-lib=--features", "check-lib=a b", "bench"] {
        assert!(bad.parse::<BuildCmd>().is_err(), "{bad}");
    }
}
```

  Run: `cargo test --lib bench::command::`. Expect: a compile error.
- [ ] **Step 2: Implement** `BuildCmd`, `FromStr` (its `Err` is a `String` naming the five shapes), `args` and
  `command_line`, with a `//!` comment on the file.
- [ ] **Step 3: Run** `cargo test --lib bench::`. Expect pass.
- [ ] **Step 4: Commit** `feat(bench): the five timed build shapes`

_Requirements: BENCH-3.1, BENCH-3.2, BENCH-3.4_

---

### Task 8: Every timed build gets one environment with no wrapper and a pinned toolchain

**Files:**
- Modify: `src/bench/command.rs` (the `EnvMap` half)
- Test: `src/bench/command.rs`

**Reuse:** rung 2 — `cargo_target::TARGET_VARS` (src/cargo_target.rs:15-19) for the ARCH-13 floor, `Toolchain` (src/doctor.rs:133) for the pin, and doctor's `RUSTC` + `RUSTUP_TOOLCHAIN` pin pattern (src/doctor.rs:699-719); the build shapes and wrapper rules are new (rung 7)

**Interfaces:**
- Consumes: `BuildCmd` (Task 7), `doctor::Toolchain`, `cargo_target::TARGET_VARS`
- Produces: `pub struct EnvMap` (fields private: `vars: BTreeMap<OsString, OsString>`,
  `overridden: Vec<(&'static str, String, String)>` as (kind, variable, value));
  `pub fn env_map(caller: impl IntoIterator<Item = (OsString, OsString)>, tc: &Toolchain) -> EnvMap`;
  `pub fn vars(&self) -> &BTreeMap<OsString, OsString>`;
  `pub fn command(&self, tc: &Toolchain, dir: &Path, build: &BuildCmd) -> std::process::Command`;
  `pub fn findings(&self) -> Vec<(&'static str, String)>` as (kind, detail)

**Depends-on:** Task 7

- [ ] **Step 1: Write the failing test**:

```rust
fn tc(name: Option<&str>) -> Toolchain {
    Toolchain { name: name.map(String::from), cargo: "/tc/bin/cargo".into(),
        rustc: "/tc/bin/rustc".into(), release: "1.97.1".into(), version_text: "rustc 1.97.1".into() }
}

#[test]
fn the_build_environment_drops_wrappers_and_pins_the_resolved_toolchain() {
    let caller = [
        ("PATH", "/bin"), ("CARGO_HOME", "/h"), ("RUSTFLAGS", "-Dwarnings"),
        ("CARGO_TARGET_DIR", "/t"), ("CARGO_BUILD_TARGET_DIR", "/t"), ("CARGO_BUILD_BUILD_DIR", "/b"),
        ("GIT_DIR", "/g"), ("CARGO_PKG_NAME", "x"), ("CARGO_MANIFEST_DIR", "/m"), ("OUT_DIR", "/o"),
        ("RUSTC_WRAPPER", "sccache"), ("RUSTC_WORKSPACE_WRAPPER", ""), ("CARGO_BUILD_RUSTC_WRAPPER", "sccache"),
        ("CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER", "w"), ("RUSTC", "/other/rustc"), ("CARGO_BUILD_RUSTC", "/r"),
        ("RUSTUP_TOOLCHAIN", "nightly"), ("RUSTUP_TOOLCHAIN_SOURCE", "env"),
    ].map(|(k, v)| (OsString::from(k), OsString::from(v)));
    let env = env_map(caller, &tc(Some("stable-aarch64-apple-darwin")));
    let get = |k: &str| env.vars().get(&OsString::from(k)).map(|v| v.to_string_lossy().into_owned());
    for kept in ["PATH", "CARGO_HOME", "RUSTFLAGS"] { assert!(get(kept).is_some(), "{kept}"); }
    for gone in ["CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR", "CARGO_BUILD_BUILD_DIR", "GIT_DIR",
                 "CARGO_PKG_NAME", "CARGO_MANIFEST_DIR", "OUT_DIR", "CARGO_BUILD_RUSTC_WRAPPER",
                 "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_RUSTC", "RUSTUP_TOOLCHAIN_SOURCE"] {
        assert_eq!(get(gone), None, "{gone}");
    }
    assert_eq!(get("RUSTC_WRAPPER").as_deref(), Some(""));
    assert_eq!(get("RUSTC_WORKSPACE_WRAPPER").as_deref(), Some(""));
    assert_eq!(get("RUSTC").as_deref(), Some("/tc/bin/rustc"));
    assert_eq!(get("RUSTUP_TOOLCHAIN").as_deref(), Some("stable-aarch64-apple-darwin"));
    assert_eq!(get("RUSTUP_AUTO_INSTALL").as_deref(), Some("0"));
    let kinds: Vec<_> = env.findings().into_iter().map(|(k, _)| k).collect();
    assert_eq!(kinds.iter().filter(|k| **k == "env.wrapper.overridden").count(), 3);
    assert_eq!(kinds.iter().filter(|k| **k == "env.rustc.overridden").count(), 2);
}

#[test]
fn without_rustup_the_pin_is_the_resolved_rustc_alone() {
    let env = env_map([], &tc(None));
    assert!(env.vars().get(&OsString::from("RUSTUP_TOOLCHAIN")).is_none());
    assert_eq!(env.vars()[&OsString::from("RUSTC")], OsString::from("/tc/bin/rustc"));
    assert!(env.findings().is_empty());
}

#[test]
fn the_build_command_runs_the_resolved_cargo_in_the_checkout_with_exactly_the_map() {
    let env = env_map([(OsString::from("PATH"), OsString::from("/bin"))], &tc(None));
    let cmd = env.command(&tc(None), Path::new("/lane"), &BuildCmd::Build);
    assert_eq!(cmd.get_program(), "/tc/bin/cargo");
    assert_eq!(cmd.get_current_dir(), Some(Path::new("/lane")));
    assert_eq!(cmd.get_args().collect::<Vec<_>>(), ["build", "--offline", "--locked"]);
}
```

  Run: `cargo test --lib bench::command::`. Expect: a compile error.
- [ ] **Step 2: Implement** `env_map` and `EnvMap` as the test reads, with `command` doing
  `Command::new(&tc.cargo)`, `.env_clear()`, `.envs(&self.vars)`, `.current_dir(dir)` and
  `.args(build.args())`. Each wrapper variable the caller set non-empty is one `env.wrapper.overridden`.
  Each caller `RUSTC` or `CARGO_BUILD_RUSTC` is one `env.rustc.overridden`, with detail
  `"<VAR>=<value>; the build used the resolved toolchain"`.
- [ ] **Step 3: Run** `cargo test --lib bench::`. Expect pass.
- [ ] **Step 4: Commit** `feat(bench): one build environment with wrappers off and the toolchain pinned`

_Requirements: BENCH-5.1, BENCH-5.2, BENCH-5.3, BENCH-5.4_

---

### Task 9: An unfair set of checkouts is refused before anything builds

**Files:**
- Create: `src/bench/check.rs`
- Modify: `src/bench/mod.rs` (`pub mod check;`)
- Create: `tests/bench.rs` (with `mod common;` and the `env_lock` / `CargoHomeGuard` pair copied from
  tests/seed.rs:2572-2597)
- Test: `tests/bench.rs`

**Reuse:** rung 2 — `cli::locate_checkout` (section 6) for 2.1, 2.7 and the toolchain half of 2.8;
`cargo_target::resolve` (src/cargo_target.rs:339) for the target and build directories in 2.5 and 2.11;
the git readers (section 8) for 2.3 and 2.4. The cross-checkout rules (same repo, same HEAD, same toolchain
text, arm sizes, duplicates, a shared target) are new code (rung 7).

**Interfaces:**
- Consumes: `cli::{locate_checkout, Refusal}` (Task 2; `pub(crate)` is enough, since both are in the same crate),
  `Git::{head, has_tracked_changes}` (Task 4), `cargo_target::resolve`, `env_lock` and `CargoHomeGuard` in
  `tests/common` (Task 1)
- Produces: `pub struct Checked { pub repo: Repo, pub cold: Vec<Checkout>, pub seeded: Vec<Checkout>, pub toolchain: Toolchain }`;
  `pub struct Unfair { pub path: Option<PathBuf>, pub reason: String }`;
  `pub fn check(reg: &Registry, cfg: &Config, caps: &Capabilities, cold: &[PathBuf], seeded: &[PathBuf], cancel: &(dyn Fn() -> bool + Sync)) -> Result<Checked, Vec<Unfair>>`.
  The reason texts, which tests match by substring (`<…>` is filled in):
  `each arm needs at least 2 checkouts` · `the two arms must have equal counts` ·
  `<path> is given more than once` · the `locate_checkout` detail as is (protected, unregistered, missing,
  toolchain, capability) · `<path> belongs to <main>, not <main of the first>` ·
  `<path> is at HEAD <x>, not <y>` · `<path> has uncommitted changes to tracked files` ·
  `<path> resolves a different toolchain: <first line of rustc -vV>` ·
  `the target directory <dir> already exists` ·
  `the build directory <b> differs from the target directory <t>` ·
  `<path> shares the target directory <dir> with <other>` ·
  `<workspace root> has no Cargo.lock; bench builds --locked`

**Depends-on:** Task 1, Task 2, Task 4, Task 6

- [ ] **Step 1: Write the failing tests** in `tests/bench.rs`, one table-driven test over the refusals. Each
  row builds its own `RegistryRepo` worktrees and expects `Err` with a reason containing its word:

```rust
fn checked(hive: &Hive, cold: &[PathBuf], seeded: &[PathBuf]) -> Result<Checked, Vec<Unfair>> {
    let _env = env_lock().lock().unwrap_or_else(|err| err.into_inner());
    let _home = CargoHomeGuard::set(&hive.cargo_home.path);
    let state = State::open_at(hive.home.path.clone()).unwrap();
    let reg: Registry = state.read(cargo_hive::registry::FILE).unwrap();
    let cfg = state.config().unwrap();
    let caps = state.read(cargo_hive::doctor::CAPABILITIES_FILE).unwrap();
    cargo_hive::bench::check::check(&reg, &cfg, &caps, cold, seeded, &|| false)
}

#[test]
fn an_unfair_set_of_checkouts_is_refused_with_a_reason_for_each() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    let [a, b, c, d] = ["a", "b", "c", "d"].map(|n| repo.worktree(n));
    let reason = |r: Result<Checked, Vec<Unfair>>| match r {
        Err(list) => list.iter().map(|x| x.reason.clone()).collect::<Vec<_>>().join(" | "),
        Ok(_) => "accepted".into(),
    };
    // arm sizes and duplicates
    assert!(reason(checked(&hive, &[a.clone()], &[b.clone()])).contains("at least 2"));
    assert!(reason(checked(&hive, &[a.clone(), b.clone()], &[c.clone()])).contains("equal"));
    assert!(reason(checked(&hive, &[a.clone(), a.clone()], &[c.clone(), d.clone()])).contains("more than once"));
    // protected: the registered main
    assert!(reason(checked(&hive, &[repo.main.clone(), b.clone()], &[c.clone(), d.clone()])).contains("protected"));
    // a dirty tree
    fs::write(c.join("src/lib.rs"), "pub fn changed() {}\n").unwrap();
    assert!(reason(checked(&hive, &[a.clone(), b.clone()], &[c.clone(), d.clone()])).contains("uncommitted"));
    git(&c, &["checkout", "--", "."]);
    // another HEAD
    git(&d, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "x"]);
    assert!(reason(checked(&hive, &[a.clone(), b.clone()], &[c.clone(), d.clone()])).contains("at HEAD"));
    // an existing target
    let e = repo.worktree("e");
    let f = repo.worktree("f");
    fs::create_dir(e.join("target")).unwrap();
    assert!(reason(checked(&hive, &[a.clone(), b.clone()], &[e.clone(), f.clone()])).contains("already exists"));
    // a fair set passes
    let g = repo.worktree("g");
    assert!(checked(&hive, &[a, b], &[f, g]).is_ok());
}
```

  Add one test per remaining rule in the same file: a checkout of another registered repo (2.2); a disabled
  Seeding capability via `hive.write_capabilities(json!({"seeding": ["f"]}))` (2.7, shaped like
  tests/seed.rs:187); two checkouts sharing a target through `.cargo/config.toml` `build.target-dir` set to
  an **absolute** path under the fixture's scratch dir, since a relative one resolves per worktree (2.11);
  a split build directory through `build.build-dir`, which is stable in cargo 1.97.1 and appears as
  `build_directory` in `cargo metadata` (2.5); and a repo whose main ran `git rm -q Cargo.lock` and
  committed before the worktrees were added, refused with `has no Cargo.lock` (2.12). The config file is committed in the fixture, so the trees
  stay clean. Run: `cargo test --test bench`. Expect: a compile error, because
  `bench::check` does not exist.
- [ ] **Step 2: Implement** `check` in the order design §3 gives: arm sizes and duplicates from the arguments,
  `locate_checkout` per path, one repo `main`, one `head()`, no `has_tracked_changes()`, one `version_text`,
  then `resolve` per checkout for an existing target, `target.split`, a shared target, and a `Cargo.lock` at
  its `workspace_root`. Collect every
  failure before returning.
- [ ] **Step 3: Run** `cargo test --test bench`. Expect pass.
- [ ] **Step 4: Commit** `feat(bench): refuse an unfair set of checkouts before anything builds`

_Requirements: BENCH-2.1, BENCH-2.2, BENCH-2.3, BENCH-2.4, BENCH-2.5, BENCH-2.6, BENCH-2.7, BENCH-2.8, BENCH-2.11, BENCH-2.12_

---

### Task 10: A bench primes the store, then times cold and seeded samples in ABBA order

**Files:**
- Modify: `src/bench/mod.rs` (`Plan`, `Limits`, `End`, `Report`, `run`)
- Test: `tests/bench.rs`

**Reuse:** rung 2 — `seed::run_fill` / `seed::run_clone` (section 6), `record::read` + `record::counts` with
`gc::DEPS_CAP` (src/seed/record.rs:74,86; src/gc/run.rs:22), `seed::store::read` (src/seed/store.rs:130), `discover::targets` with an in-memory
`Snapshot::default()` (src/discover.rs:523, as src/seed/source.rs:86 does), the bounded runner (section 7),
`ReceiptBuilder` (src/receipt.rs:140-215). The sequencing itself is new code (rung 7), because nothing
sequences builds and seeds.

**Interfaces:**
- Consumes: `seed::{run_fill, run_clone, Fill, Filled}` (Task 1), `cargo_target::{run_bounded, Capture, Ran}` (Task 3),
  `Fs::load_average_hundredths` (Task 5), `order::{abba, Arm}` and `stats::summarize` (Task 6),
  `command::{BuildCmd, EnvMap}` (Tasks 7, 8), `check::Checked` (Task 9)
- Produces: `pub struct Plan { pub repo: Repo, pub cold: Vec<Checkout>, pub seeded: Vec<Checkout>, pub toolchain: Toolchain, pub build: BuildCmd, pub env: EnvMap }`;
  `pub struct Limits { pub build_deadline: Duration }`; `pub const BUILD_DEADLINE: Duration` (60 min);
  `pub enum End { Finished, Failed, Cancelled }`;
  `pub struct Report { pub end: End, pub counts: BTreeMap<String, u64>, pub findings: Vec<receipt::Finding>, pub unmeasured: Vec<(String, String)> }`.
  Per seeded sample, `fill_ms` and `clone_ms` are each `as_millis` of their `Instant` span, and `seed_ms` is
  defined as `fill_ms + clone_ms` in integers, so the three always agree. `seeded_total_ms` is the series of
  `seed_ms + build_ms` per seeded sample.
  with `pub fn write_into(&self, b: &mut ReceiptBuilder)` and `pub fn exit_code(&self) -> i32`;
  `pub fn run(ctx: &Ctx<'_>, plan: &Plan, limits: Limits, cancel: &(dyn Fn() -> bool + Sync)) -> Report`

**Depends-on:** Task 1, Task 3, Task 5, Task 6, Task 8, Task 9

- [ ] **Step 1: Write the failing test** in `tests/bench.rs`. `bench_plan(hive, repo, n)` makes 2n worktrees,
  runs `checked` (Task 9), and builds `Plan` with `env_map` over `std::env::vars_os()` with `CARGO_HOME`
  replaced by `hive.cargo_home.path` (house rule 7). `run_bench(hive, plan, limits, cancel)` calls
  `run_bench_cfg(hive, plan, limits, cancel, |_| {})`. `run_bench_cfg` holds `env_lock()` and
  `CargoHomeGuard`, opens `LibCtx::open(hive, Recording::over_real().fs())`, applies the closure to
  `lib.cfg`, builds a `Ctx` with `repos = [plan.repo]`, and calls `bench::run`.

```rust
#[test]
fn a_bench_primes_once_then_reports_every_sample_in_abba_order() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let plan = bench_plan(&hive, &repo, 2);
    let report = run_bench(&hive, &plan, Limits { build_deadline: BUILD_DEADLINE }, &|| false);
    assert!(matches!(report.end, End::Finished));
    assert_eq!(report.exit_code(), 0);
    for key in ["prime_ms", "prime_entries_added", "prime_targets_read"] {
        assert!(report.counts.contains_key(key), "{key}");
    }
    let samples: Vec<_> = report.findings.iter().filter(|f| f.kind == "bench.sample").collect();
    let arms: Vec<_> = samples.iter().map(|f| f.measured["arm"].clone()).collect();
    assert_eq!(arms, ["cold", "seeded", "seeded", "cold"].map(|a| Measured::Text(a.into())));
    for (i, f) in samples.iter().enumerate() {
        assert_eq!(f.measured["position"], Measured::Int(i as u64 + 1));
        for key in ["build_ms", "load_before_x100", "load_after_x100"] {
            assert!(matches!(f.measured.get(key), Some(Measured::Int(_))), "{key}");
        }
    }
    for f in samples.iter().filter(|f| f.measured["arm"] == Measured::Text("seeded".into())) {
        let ms = |k: &str| match f.measured[k] { Measured::Int(v) => v, _ => panic!("{k}") };
        assert_eq!(ms("seed_ms"), ms("fill_ms") + ms("clone_ms"));
        assert!(f.measured.contains_key("fill_targets_read"));
    }
    for series in ["cold_build_ms", "seeded_build_ms", "seeded_seed_ms", "seeded_total_ms"] {
        for stat in ["median", "p90", "min", "max", "spread"] {
            assert!(report.counts.contains_key(&format!("{series}_{stat}")), "{series}_{stat}");
        }
    }
    for f in &report.findings { for key in f.measured.keys() {
        for banned in ["seconds", "duration", "elapsed", "saved", "estimate"] { assert!(!key.contains(banned)); }
    }}
}
```

  Run: `cargo test --test bench a_bench_primes_once`. Expect: a compile error, because `bench::run` does not
  exist.
- [ ] **Step 2: Implement** `run` as design §1's flow, for the finished path: the prime through `run_fill`
  (its findings copied); then for each `abba(n)` entry, load before, `run_fill` + `run_clone` for a seeded
  sample (seed findings copied with the sample's checkout as path), `run_bounded` with
  `Capture { stdout_cap: 0, stderr_cap: 64 * 1024 }`, load after, and the `bench.sample` finding. Then the
  `<series>_<stat>` counts from `summarize`. A failed load reading goes in `unmeasured` as
  `sample.<position>.load_before` or `.load_after`. Report's `exit_code` maps Finished 0, Failed 1,
  Cancelled 130.
- [ ] **Step 3: Run** `cargo test --test bench`. Expect pass.
- [ ] **Step 4: Commit** `feat(bench): prime, then time cold and seeded samples in ABBA order`

_Requirements: BENCH-1.1, BENCH-1.2, BENCH-1.3, BENCH-1.4, BENCH-1.5, BENCH-1.6, BENCH-1.7, BENCH-1.8, BENCH-1.10, BENCH-1.12_

---

### Task 11: Each seeded sample shows how many seeded units its build reused

**Files:**
- Modify: `src/bench/mod.rs` (the post-build counts of a seeded sample)
- Test: `tests/bench.rs`

**Reuse:** rung 2 — `seed::run_fill` / `seed::run_clone` (section 6), `record::read` + `record::counts` with
`gc::DEPS_CAP` (src/seed/record.rs:74,86; src/gc/run.rs:22), `seed::store::read` (src/seed/store.rs:130), `discover::targets` with an in-memory
`Snapshot::default()` (src/discover.rs:523, as src/seed/source.rs:86 does), the bounded runner (section 7),
`ReceiptBuilder` (src/receipt.rs:140-215). The sequencing itself is new code (rung 7), because nothing
sequences builds and seeds.

**Interfaces:**
- Consumes: `seed::record::{read, counts, SeedCounts}`, `discover::{targets, Snapshot}`, `gc::DEPS_CAP`,
  `Report` and `run` (Task 10)
- Produces: the measured keys `seeded_fresh`, `seeded_unneeded`, `seeded_recompiled` and `seeded_missing` on
  a seeded `bench.sample`, or `unmeasured` entries `sample.<position>.seed_counts`

**Depends-on:** Task 10

- [ ] **Step 1: Write the failing tests** in `tests/bench.rs`:

```rust
#[test]
fn a_seeded_sample_carries_the_four_seed_counts_and_recompiles_none() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let report = run_bench(&hive, &bench_plan(&hive, &repo, 2), Limits { build_deadline: BUILD_DEADLINE }, &|| false);
    for f in report.findings.iter().filter(|f| f.kind == "bench.sample"
        && f.measured["arm"] == Measured::Text("seeded".into())) {
        for key in ["seeded_fresh", "seeded_unneeded", "seeded_recompiled", "seeded_missing"] {
            assert!(matches!(f.measured.get(key), Some(Measured::Int(_))), "{key}");
        }
        assert_eq!(f.measured["seeded_recompiled"], Measured::Int(0));
    }
}

#[test]
fn a_seed_that_clones_nothing_still_builds_and_leaves_its_counts_unmeasured() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let mut plan = bench_plan(&hive, &repo, 2);
    plan.toolchain.version_text = "rustc 0.0.0 (no such toolchain)".into(); // the store has no row for it
    let report = run_bench(&hive, &plan, Limits { build_deadline: BUILD_DEADLINE }, &|| false);
    assert!(matches!(report.end, End::Finished));
    assert!(report.findings.iter().any(|f| f.kind == "toolchain.missing"));
    assert!(report.unmeasured.iter().any(|(field, _)| field.ends_with(".seed_counts")));
}
```

  Run: `cargo test --test bench seed_counts`, then `cargo test --test bench a_seed`. Expect the first test to
  fail on the missing `seeded_fresh` key.
- [ ] **Step 2: Implement.** After a seeded build, `record::read(ctx.state)`, then
  `discover::targets(&[row.clone()], ctx.cfg, false, &mut Snapshot::default(), cancel)`. Pick the
  `TargetDir` whose path is the seed outcome's target, look up its record, and call
  `record::counts(target, record, gc::DEPS_CAP)`. `Ok(Some(c))` gives the four keys. `Ok(None)`, no record,
  or `Err(why)` gives `unmeasured` `sample.<position>.seed_counts` with the reason.
- [ ] **Step 3: Run** `cargo test --test bench`, `cargo test --test status_targets` and
  `cargo test --test status_seed`. Expect pass.
- [ ] **Step 4: Commit** `feat(bench): report the seed counts of each seeded sample`

_Requirements: BENCH-4.1, BENCH-4.2, BENCH-4.3, BENCH-9.2_

---

### Task 12: A failed or overdue step ends the run with no summary

**Files:**
- Modify: `src/bench/mod.rs` (the Failed paths)
- Test: `tests/bench.rs`

**Reuse:** rung 2 — `seed::run_fill` / `seed::run_clone` (section 6), `record::read` + `record::counts` with
`gc::DEPS_CAP` (src/seed/record.rs:74,86; src/gc/run.rs:22), `seed::store::read` (src/seed/store.rs:130), `discover::targets` with an in-memory
`Snapshot::default()` (src/discover.rs:523, as src/seed/source.rs:86 does), the bounded runner (section 7),
`ReceiptBuilder` (src/receipt.rs:140-215). The sequencing itself is new code (rung 7), because nothing
sequences builds and seeds.

**Interfaces:**
- Consumes: `Report`, `End`, `Limits` (Task 10), `ChildOutputError::TimedOut` (Task 3)
- Produces: the `bench.failed` finding (detail names the step, the checkout and the Cargo exit code or the
  deadline), and `unmeasured` `sample.<position>` with `"not run: an earlier step failed"`

**Depends-on:** Task 11

- [ ] **Step 1: Write the failing tests** in `tests/bench.rs`:

```rust
#[test]
fn a_build_that_fails_stops_the_run_and_reports_no_summary() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let mut plan = bench_plan(&hive, &repo, 2);
    plan.build = "check-lib=no_such_package".parse().unwrap();
    let report = run_bench(&hive, &plan, Limits { build_deadline: BUILD_DEADLINE }, &|| false);
    assert!(matches!(report.end, End::Failed));
    assert_eq!(report.exit_code(), 1);
    let failed = report.findings.iter().find(|f| f.kind == "bench.failed").unwrap();
    assert!(failed.detail.contains("exit"), "{}", failed.detail);
    assert!(!report.counts.keys().any(|k| k.ends_with("_median")));
    assert_eq!(report.unmeasured.iter().filter(|(f, _)| f.starts_with("sample.")).count(), 3);
}

#[test]
fn a_build_past_its_deadline_is_killed_and_ends_the_run() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let plan = bench_plan(&hive, &repo, 2);
    let report = run_bench(&hive, &plan, Limits { build_deadline: Duration::from_millis(1) }, &|| false);
    assert!(matches!(report.end, End::Failed));
    let failed = report.findings.iter().find(|f| f.kind == "bench.failed").unwrap();
    assert!(failed.detail.contains("deadline"), "{}", failed.detail);
    for c in plan.cold.iter().chain(&plan.seeded) { assert!(c.path.exists()); }
}

#[test]
fn a_seed_that_errors_stops_the_run() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let plan = bench_plan(&hive, &repo, 2);
    // A window outside 1..=365 makes `run_clone` return an error (src/seed/mod.rs:200-203, src/state.rs:216).
    let report = run_bench_cfg(&hive, &plan, Limits { build_deadline: BUILD_DEADLINE }, &|| false,
        |cfg| cfg.gc_window_days = Some(0));
    assert!(matches!(report.end, End::Failed));
    let failed = report.findings.iter().find(|f| f.kind == "bench.failed").unwrap();
    assert!(failed.detail.contains("seed"), "{}", failed.detail);
    assert_eq!(report.findings.iter().filter(|f| f.kind == "bench.sample").count(), 1); // cold_1 only
}
```

#[test]
fn an_empty_store_for_the_repo_after_the_prime_stops_before_any_sample() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);                    // main never built: the prime fills nothing
    let plan = bench_plan(&hive, &repo, 2);
    let report = run_bench(&hive, &plan, Limits { build_deadline: BUILD_DEADLINE }, &|| false);
    assert!(matches!(report.end, End::Failed));
    assert_eq!(report.exit_code(), 1);
    assert!(report.counts.contains_key("prime_ms"));
    assert!(report.findings.iter().any(|f| f.kind == "bench.nothing_to_seed"));
    assert_eq!(report.findings.iter().filter(|f| f.kind == "bench.sample").count(), 0);
    for c in plan.cold.iter().chain(&plan.seeded) { assert!(!c.path.join("target").exists()); }
}
```

  A failing prime takes the same `Err` path as a failing seed. No cheap fixture makes `run_fill` fail, so that
  branch is covered by the shared code, not by its own test. Run: `cargo test --test bench stops_the_run`. Expect: a failure, because the run does not stop yet.
- [ ] **Step 2: Implement** the Failed paths of design §1: a prime or seed `Err` (a `DiskFull` keeps its
  `kept` outcome's findings), a non-zero Cargo exit (the exit code and the last stderr lines, capped at
  4 KiB, in the detail, worded `exit code <n>`), and `TimedOut` (worded `past its deadline of <ms> ms`).
  The detail always names the step: `prime`, `seed of <checkout>` or `build of <checkout>`. After the prime,
  `seed::store::read(ctx.state)` is checked once for a `Row` whose `repos` holds `plan.repo.main` (the check
  is on the repo, not the toolchain; see design Upstream sync, Still-open S2); none gives
  `bench.nothing_to_seed` with detail `no stored unit lists <main>; build it once with <command line>, then
  bench again`, and the run ends Failed before the first sample. Each starts no further sample, lists the
  rest as unmeasured, and skips the summaries.
- [ ] **Step 3: Run** `cargo test --test bench`. Expect pass.
- [ ] **Step 4: Commit** `feat(bench): a failed or overdue step ends the run`

_Requirements: BENCH-7.1, BENCH-7.2, BENCH-7.3, BENCH-6.3, BENCH-10.2_

---

### Task 13: A cancel stops at once, keeps what finished, and removes nothing

**Files:**
- Modify: `src/bench/mod.rs` (the Cancelled paths)
- Test: `tests/bench.rs`

**Reuse:** rung 2 — `seed::run_fill` / `seed::run_clone` (section 6), `record::read` + `record::counts` with
`gc::DEPS_CAP` (src/seed/record.rs:74,86; src/gc/run.rs:22), `seed::store::read` (src/seed/store.rs:130), `discover::targets` with an in-memory
`Snapshot::default()` (src/discover.rs:523, as src/seed/source.rs:86 does), the bounded runner (section 7),
`ReceiptBuilder` (src/receipt.rs:140-215). The sequencing itself is new code (rung 7), because nothing
sequences builds and seeds.

**Interfaces:**
- Consumes: `Report`, `End` (Task 10), `Fill::Stopped`, `Outcome::cancelled` (Task 1),
  `ChildOutputError::Cancelled` (Task 3)
- Produces: `End::Cancelled`, and `unmeasured` `sample.<position>` `"cancelled"`

**Depends-on:** Task 12

- [ ] **Step 1: Write the failing test** in `tests/bench.rs`:

```rust
#[test]
fn a_cancel_after_the_first_sample_keeps_it_and_lists_the_rest() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let plan = bench_plan(&hive, &repo, 2);
    // cold_1 builds first and finishes; seeded_1's seed then clones into its target/debug, which trips the
    // cancel during seeded_1's build.
    let seeded_debug = plan.seeded[0].path.join("target/debug");
    let cancel = move || seeded_debug.exists();
    let report = run_bench(&hive, &plan, Limits { build_deadline: BUILD_DEADLINE }, &cancel);
    assert!(matches!(report.end, End::Cancelled));
    assert_eq!(report.exit_code(), 130);
    let samples: Vec<_> = report.findings.iter().filter(|f| f.kind == "bench.sample").collect();
    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].measured["arm"], Measured::Text("cold".into()));
    assert!(!report.counts.keys().any(|k| k.ends_with("_median")));
    assert!(report.unmeasured.iter().any(|(f, r)| f.starts_with("sample.") && r == "cancelled"));
    for c in plan.cold.iter().chain(&plan.seeded) { assert!(c.path.exists()); }
}
```

  Run: `cargo test --test bench a_cancel_after`. Expect: a failure.
- [ ] **Step 2: Implement** the Cancelled paths: `cancel()` before a sample, `Fill::Stopped`, an outcome with
  `cancelled`, and `ChildOutputError::Cancelled`. Each records the finished samples, lists every unfinished
  one as `cancelled`, and reports no summary. Nothing is removed on any path.
- [ ] **Step 3: Run** `cargo test --test bench`. Expect pass.
- [ ] **Step 4: Commit** `feat(bench): a cancel keeps finished samples and removes nothing`

_Requirements: BENCH-6.2, BENCH-6.4, BENCH-9.7_

---

### Task 14: `cargo hive bench` runs from the command line and writes one schema-2 receipt

**Files:**
- Modify: `src/cli.rs` (`Command` enum lines 23 to 94; `main_with` arm near line 173; new `bench_cmd`)
- Modify: `tests/skill.rs` (`enum Parsed` line 412; `parse()` lines 424 to 452)
- Modify: `tests/liveness.rs` (`the_cli_has_no_new_subcommand_or_flag`, lines 186 to 226: add
  `cargo-hive bench`, `cargo-hive bench --build_cmd`, `cargo-hive bench --cold`, `cargo-hive bench --json`,
  `cargo-hive bench --seeded` to `expected`, keeping it sorted)
- Test: `tests/bench.rs`

**Reuse:** rung 2 — the `seed_cmd` shape: the `window_days` check and `install_signal_handlers`
(src/cli.rs:1420-1431), `platform::current`, `Ctx`, `Receipt::new` and `emit` (src/cli.rs:222); clap derive
for the arguments.

**Interfaces:**
- Consumes: `bench::{check::check, run, Plan, Limits, BUILD_DEADLINE, Report}`, `command::{BuildCmd, env_map}`
- Produces (the `--json` and `--build_cmd` ids above come from these field names):
  `Command::Bench { #[arg(long, num_args = 1.., required = true)] cold: Vec<PathBuf>, #[arg(long, num_args = 1.., required = true)] seeded: Vec<PathBuf>, #[arg(long, default_value = "build")] build_cmd: BuildCmd, #[command(flatten)] out: OutputArgs }`;
  `fn bench_cmd(state: &State, cold: &[PathBuf], seeded: &[PathBuf], build: BuildCmd, out: OutputArgs) -> i32`;
  `Parsed::Bench`

**Depends-on:** Task 13

- [ ] **Step 1: Write the failing tests** in `tests/bench.rs`, through the binary:

```rust
#[test]
fn bench_writes_one_schema_two_receipt_and_exits_zero() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let lanes = ["c1", "s1", "s2", "c2"].map(|n| repo.worktree(n));
    let tracked = git(&repo.main, &["ls-files", "-s"]);
    // each linked worktree's index lives where `git rev-parse --git-path index` says
    let index_of = |lane: &PathBuf| lane.join(git(lane, &["rev-parse", "--git-path", "index"]).trim());
    let indexes: Vec<Vec<u8>> = lanes.iter().map(|l| fs::read(index_of(l)).unwrap()).collect();
    let s = |p: &PathBuf| p.to_str().unwrap().to_string();
    let (out, receipt) = hive.json(&["bench", "--cold", &s(&lanes[0]), &s(&lanes[3]),
                                      "--seeded", &s(&lanes[1]), &s(&lanes[2])]);
    assert!(out.status.success(), "{receipt}");
    assert_eq!(receipt["command"], "bench");
    assert_eq!(receipt["schema"], 2);
    let keys: BTreeSet<_> = receipt.as_object().unwrap().keys().cloned().collect();
    let allowed: BTreeSet<_> = ["schema", "hive_version", "macos_version", "toolchain", "command", "ts",
        "scope", "counts", "checks", "findings", "unmeasured", "notes"].map(String::from).into();
    assert!(keys.is_subset(&allowed), "{keys:?}");
    assert!(receipt["findings"].as_array().unwrap().iter().any(|f| f["kind"] == "bench.command"));
    assert_eq!(git(&repo.main, &["ls-files", "-s"]), tracked);
    for (lane, before) in lanes.iter().zip(&indexes) {
        assert!(lane.join("target").exists(), "{}: bench removed nothing, so its target stays", lane.display());
        assert_eq!(git(lane, &["--no-optional-locks", "status", "--porcelain", "--untracked-files=no"]), "",
                   "{}: a tracked file changed", lane.display());
        assert_eq!(&fs::read(index_of(lane)).unwrap(), before, "{}", lane.display());
    }
    let allowed: BTreeSet<&str> = [".lock", "capabilities.json", "config.json", "receipts",
        "registry.json", "snapshot.json", "seeds.json", "store"].into();
    for entry in fs::read_dir(&hive.home.path).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        assert!(allowed.contains(name.as_str()), "unexpected state file {name}");
    }
}

#[test]
fn bench_refuses_with_exit_one_and_rejects_an_unknown_shape_with_exit_two() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    let a = repo.worktree("a"); let b = repo.worktree("b");
    let s = |p: &PathBuf| p.to_str().unwrap().to_string();
    let (out, receipt) = hive.json(&["bench", "--cold", &s(&a), "--seeded", &s(&b)]);
    assert_eq!(out.status.code(), Some(1), "{receipt}");
    assert!(receipt["findings"].as_array().unwrap().iter().any(|f| f["kind"] == "refusal"));
    let out = hive.run(&["bench", "--cold", &s(&a), "--seeded", &s(&b), "--build-cmd", "run"]);
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn ctrl_c_stops_bench_with_exit_130() {
    let hive = Hive::ready();
    let repo = RegistryRepo::new(&hive);
    cargo_json(&hive, &repo.main, &["build"]);
    let lanes = ["c1", "s1", "s2", "c2"].map(|n| repo.worktree(n));
    let s = |p: &PathBuf| p.to_str().unwrap().to_string();
    let child = hive
        .command(&["bench", "--cold", &s(&lanes[0]), &s(&lanes[3]),
                   "--seeded", &s(&lanes[1]), &s(&lanes[2]), "--json"])
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().unwrap();
    let pid = child.id();
    let started = Instant::now();
    while !lanes[0].join("target/debug").exists() {
        assert!(started.elapsed() < Duration::from_secs(60), "the first build never started");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(Command::new("kill").args(["-INT", &pid.to_string()]).status().unwrap().success());
    let out = wait_within(child, Duration::from_secs(30));
    assert_eq!(out.status.code(), Some(130), "{out:?}");
    for lane in &lanes { assert!(lane.exists()); }
}
```

  The SIGINT test follows tests/status_targets.rs:540-575 (`kill -INT`, then `wait_within`). `RegistryRepo`
  already sets `secondary_roots` to its scratch dir. Run:
  `cargo test --test bench bench_`. Expect: exit 2, because `bench` is not a subcommand yet.
- [ ] **Step 2: Implement** `Command::Bench` (`BuildCmd` parses through its `FromStr`, so an unknown shape is a
  clap error, exit 2), the `main_with` arm, and `bench_cmd`. `bench_cmd` reads the registry, config and
  capabilities; checks `window_days`; installs the signal handlers; calls `check`; on refusal writes one
  `refusal` finding per `Unfair` with exit 1; otherwise builds `env_map(std::env::vars_os(), &toolchain)`,
  the `Plan`, the `Ctx` with `repos = [repo]` and `Limits { build_deadline: BUILD_DEADLINE }`; runs; writes
  the report, the env findings and a `bench.command` finding into `Receipt::new("bench", scope)`; and
  emits with `report.exit_code()`. Add `Parsed::Bench` and its arm in `tests/skill.rs` in the same commit.
- [ ] **Step 3: Run** `cargo test --test bench`, `cargo test --test skill` and `cargo test --test liveness`.
  Expect pass. (`skill_documents_every_shipped_command` checks only its wants list, which Task 15 extends.)
- [ ] **Step 4: Commit** `feat(cli): cargo hive bench`

_Requirements: BENCH-1.11, BENCH-2.9, BENCH-3.3, BENCH-6.1, BENCH-9.7, BENCH-10.1_

---

### Task 15: The hive skill, the README and the receipt docs teach bench

**Files:**
- Modify: `plugin/skills/hive/SKILL.md` (line 19 hold list; "Which command" table lines 30 to 38; "Command
  lines" lines 48 to 59; "Reading a receipt" lines 61 to 67; "Do not" line 69)
- Modify: `README.md` (command table lines 44 to 54)
- Modify: `src/receipt.rs` (module doc lines 1 and 2)
- Modify: `skills/hive/eval.json` (an eleventh entry), `skills/hive/TESTS.md` (its `derived_from` heading)
- Modify: `tests/skill.rs` (wants list lines 624 to 640; path-required set lines 607 to 610; a new teaching
  test beside `the_skill_teaches_seeding_a_new_lane`, line 531)
- Test: `tests/skill.rs`

**Reuse:** rung 2 — the existing seven headings and the per-command test pattern
(`the_skill_teaches_seeding_a_new_lane`, tests/skill.rs:531).

**Interfaces:**
- Consumes: `Parsed::Bench` (Task 14)
- Produces: nothing other tasks read

**Depends-on:** Task 14

- [ ] **Step 1: Write the failing test** in `tests/skill.rs`, shaped like `the_skill_teaches_seeding_a_new_lane`
  (read the skill body the way that test does):

```rust
#[test]
fn the_skill_teaches_benching_cold_against_seeded() {
    let body = skill_body();
    for needle in ["cargo hive bench --cold /path/to/", "--seeded /path/to/", "git worktree add",
                   "git worktree remove", "even number", "p90", "only when", "fill_targets_read",
                   "Cargo.lock", "--offline"] {
        assert!(body.contains(needle), "{needle}");
    }
    assert!(body.lines().count() <= 150);
}
```

  Add `Bench` to the wants list and to the path-required set. Run: `cargo test --test skill`. Expect: a
  failure on the first missing needle.
- [ ] **Step 2: Write the docs.** Put these in SKILL.md, inside the existing headings: a "Which command" row;
  one fenced line, `cargo hive bench --cold /path/to/cold1 /path/to/cold2 --seeded /path/to/seeded1 /path/to/seeded2 --json`
  (no `<…>` words, because `command_words` stops at any word containing `>`, tests/skill.rs:365; and no
  backticked `cargo hive bench` in prose, which the skill test would read as a command line); the setup and cleanup lines
  (2n `git worktree add`, then `git worktree remove`, an even number per arm); a "Reading a receipt" paragraph (the
  prime, `bench.sample`, the `_ms` series, p90 is the max for n ≤ 9, the four seed counts, and that seed
  and fill times rise through a run, shown by `fill_targets_read`); and `bench` in the line 19 hold list.
  Keep line 63's rule and add that times come only from `bench`. Add the setup line: bench builds
  `--offline --locked`, so the repo needs a committed Cargo.lock and its dependencies in Cargo's cache, and
  building the main checkout once with the same command does both and fills the store. Add a "Do not" line: run `bench` only
  when asked, because it makes 2n full builds. Add a README row. Reword `src/receipt.rs`'s module doc to
  say bench's receipt is the one that holds integer milliseconds. Add the eval entry and its TESTS.md
  heading.
- [ ] **Step 3: Run** `cargo test --test skill` and `cargo test --lib receipt::`. Expect pass, with the body
  at most 150 lines and the seven headings unchanged.
- [ ] **Step 4: Commit** `docs(skill): teach agents to bench a cold lane against a seeded one`

_Requirements: BENCH-8.1, BENCH-8.2, BENCH-8.3, BENCH-9.3, BENCH-9.4_
