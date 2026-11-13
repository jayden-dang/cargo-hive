# Tasks: Workspace split

> **For agentic workers:** after plan approval, pick one execute skill —
> `build-in-waves` (continuous dependency-aware scheduler), `build-by-story` (human-gated story review
> units), or `build-inline` (controller implements, no implementer subagents).
> The chosen skill writes `Execution-mode:`. Steps use checkbox (`- [ ]`) syntax
> for tracking.

Feature code: SPLIT
Status: Draft
Date: 2026-11-13
Execution-mode: unset
Max-concurrency: auto
Requirements: ./requirements.md
Design: ./design.md

**Goal:** Split cargo-hive into a layered Cargo workspace whose crates depend only downward, where only the platform crate touches the OS, on edition 2024, while the installed `cargo-hive` behaves exactly as 0.1.0.

**Architecture:** Approach 1 (design.md Context; approaches.md W5). First capture 0.1.0 baselines. Then untangle inside the one crate (OS calls into `platform`, spawn env into one module, cycles and side edges broken, liveness inverted per ADR-0003) and move to edition 2024. Next make the root a virtual workspace with one member, extract members bottom-up one per task as pure moves, rework the unit data structures, and finish with the release steps and docs. The liveness report on klynt and the v0.1.0 capture comparison run at three checkpoints.

**Tech Stack:** Rust (rustc/cargo 1.97.1), Cargo workspaces, clap 4.6.7, serde/serde_json, libc 0.2.191 and applesauce 0.8.10 (platform only), clippy 0.1.97; tests via `cargo test` (unit in `#[cfg(test)] mod tests`, integration in `tests/*.rs`).

## Global Constraints

Canonical source: `docs/standards/` (INDEX.md `80cd41328c0f`, code.md `afdcc8b89136`, testing.md `6659c2301761`,
errors-logging.md `48f5baac3b52`, sha256 prefixes, 2026-11-13). Commands are in `docs/agents/project.md` (verify
table, task gate). Every task inherits these:

- **Merge gate:** `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, and
  `cargo check --target x86_64-unknown-linux-gnu` (testing.md:10-16). From Task 14 on, each command runs at the
  workspace root with `--workspace`.
- **Code rules** (code.md):
  - bounds on every walk, read and child;
  - `unsafe` only at FFI sites, each with a `// SAFETY:` comment, and only in the platform crate (SPLIT-2.1);
  - no new dependency without the owner's approval (none is planned);
  - no requirement IDs in source, comments or test names;
  - every module opens with a `//!` comment.
- **Behaviour freeze:** no command, flag, receipt field, exit code or state file changes (ROAD-15.md:17). No
  existing test assertion is weakened, deleted or reworded to pass (requirements Out of Scope).
- **Architecture spine** (docs/architecture/INDEX.md `d290fdb59a37`): ARCH-1, ARCH-2, ARCH-9, ARCH-13, ARCH-15,
  ARCH-16, ARCH-17, ARCH-19 bind every task.
- **Machine safety:**
  - tests and manual steps write only inside the worktree, `/private/tmp` or `$TMPDIR`;
  - klynt is read, never written;
  - `HIVE_HOME` always points under `/private/tmp`;
  - cargo builds of the v0.1.0 export run `--offline`;
  - one known exception: `cargo package --workspace` and `cargo publish --workspace --dry-run` (Task 26 Step 3)
    update the crates.io index cache under the cargo home ("Updating crates.io index"), as any release does. That
    step is a release-time manual step, run only by the operator or with the operator's go-ahead.
- **Team:** Solo (docs/agents/project.md `## Team`); the agent pairs, and there are no assignee fields.
- **Commits:** a conventional subject, with no AI or co-author trailer.

## File Structure

Created:
- `tests/fixtures/v0.1.0/cli-tree.txt` (later moved to `crates/cargo-hive/tests/fixtures/v0.1.0/cli-tree.txt`) — 0.1.0 clap tree dump, version line excluded.
- `tests/cli_tree.rs` — dumps `Cli::command()` and compares it with the fixture (SPLIT-3.1).
- `tests/capture.rs` — `#[ignore]` run of one fixed command sequence through the new binary and the v0.1.0 binary (`HIVE_V010_BIN`), comparing receipts, state files and `--json` envelopes (SPLIT-3.2, 4.2, 4.3).
- `tests/liveness_report.rs` (later `crates/gc/tests/liveness_report.rs`) — `#[ignore]` read-only liveness report (SPLIT-4.1).
- `tests/seed_liveness.rs` — seed unit tests that reach the liveness algorithm, moved out of seed (ADR-0003).
- `src/platform/{proc,flock,signals,clock,testing}.rs` — OS mechanism moved into platform.
- `src/env.rs` — every child's environment rule (ARCH-13).
- `src/toolchain.rs`, `src/metadata.rs` — toolchain resolution and `cargo metadata` resolve, out of doctor and cargo_target.
- `src/ctx.rs`, `src/locks.rs`, `src/ledger/{mod,account}.rs`, `src/caps.rs` — shared items out of plan_tier and gc.
- `src/gc/liveness.rs` — the liveness algorithm (the types stay in `src/liveness.rs`).
- `Cargo.toml` (root, rewritten as a virtual workspace), `clippy.toml`.
- `crates/{platform,core,state,cargo,doctor,seed,gc,cargo-hive}/Cargo.toml`, `crates/*/src/lib.rs`, `crates/*/README.md`, `crates/*/LICENSE-MIT`, `crates/*/LICENSE-APACHE` (copies of the root files).
- `crates/cargo-hive/tests/workspace.rs` — layer, edition, pin and bin guard (SPLIT-1.x, 3.3, 3.4, 3.6, 5.1).
- `crates/cargo-hive/tests/os_boundary.rs` — OS-boundary guard (SPLIT-2.2–2.4).
- `crates/cargo-hive/tests/rebuild_scope.rs` — `#[ignore]` gc-edit rebuild check (SPLIT-1.4, 1.5).
- `crates/cargo-hive/tests/version.rs` — `hive_version` equals `--version` (SPLIT-3.5).

Modified (then moved under `crates/<role>/src/` by Tasks 15–21 as pure moves):
- `src/*.rs`, `src/*/*.rs` — every module, per the design's placement.
- `tests/*.rs`, `tests/common/*.rs`, `tests/skill/names.rs` — import paths, repo-root lookup, file-level clippy allows.
- `Cargo.lock` — regenerated with members, third-party versions unchanged.
- `scripts/fresh-probe.sh` — default file and per-member loop.
- `docs/agents/project.md`, `docs/agents/verify.md`, `docs/standards/code.md`, `docs/standards/testing.md`, `README.md` — commands and rules for the workspace.
- `plugin/.claude-plugin/plugin.json`, `plugin/.codex-plugin/plugin.json`, `plugin/skills/hive/SKILL.md` — version only, at release time (no change in this plan's tasks unless the version moves).

---

### Task 1: The 0.1.0 command tree is pinned

**Files:**
- Create: `tests/cli_tree.rs`
- Create: `tests/fixtures/v0.1.0/cli-tree.txt`

**Reuse:** rung 5 — clap's `CommandFactory` on `cargo_hive::cli::Cli` (clap 4.6.7, already a dependency; tests/skill.rs:434 parses with the same `Cli`).

**Interfaces:**
- Consumes: `cargo_hive::cli::Cli` (clap `CommandFactory`), present in 0.1.0 and after every task.
- Produces: `fn dump(cmd: &clap::Command, depth: usize, out: &mut String)` — one line per command, argument,
  flag, default and help string, sorted by name, hidden items included, with `--version` text excluded, plus
  each command's rendered long help (clap's `render_long_help`, which also covers value names, `num_args`,
  conflicts, aliases and `after_help`). The version number appears only in the version flag, which the
  dump filters out.

**Depends-on:** none

- [ ] **Step 1: Write the failing test**

```rust
//! The installed command tree stays as 0.1.0 shipped it.
use cargo_hive::cli::Cli;
use clap::CommandFactory;

fn dump(cmd: &clap::Command, depth: usize, out: &mut String) {
    let pad = "  ".repeat(depth);
    out.push_str(&format!("{pad}cmd {} hidden={} about={:?} long_about={:?}\n", cmd.get_name(),
        cmd.is_hide_set(), cmd.get_about().map(|a| a.to_string()),
        cmd.get_long_about().map(|a| a.to_string())));
    let mut args: Vec<_> = cmd.get_arguments().filter(|a| a.get_id() != "version").collect();
    args.sort_by_key(|a| a.get_id().to_string());
    for a in args {
        out.push_str(&format!(
            "{pad}  arg {} long={:?} short={:?} hidden={} required={} defaults={:?} help={:?} long_help={:?}\n",
            a.get_id(), a.get_long(), a.get_short(), a.is_hide_set(), a.is_required_set(),
            a.get_default_values(), a.get_help().map(|h| h.to_string()),
            a.get_long_help().map(|h| h.to_string())));
    }
    out.push_str(&format!("{pad}  long_help_render:\n{}\n", cmd.clone().render_long_help()));
    let mut subs: Vec<_> = cmd.get_subcommands().collect();
    subs.sort_by_key(|s| s.get_name().to_string());
    for s in subs { dump(s, depth + 1, out); }
}

#[test]
fn the_command_tree_matches_0_1_0() {
    let mut out = String::new();
    dump(&Cli::command(), 0, &mut out);
    let fixture = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/v0.1.0/cli-tree.txt");
    if let Ok(path) = std::env::var("HIVE_WRITE_CLI_TREE") {
        std::fs::write(path, &out).unwrap();
        return;
    }
    assert_eq!(out, std::fs::read_to_string(fixture).unwrap());
}
```

Run: `cargo test --test cli_tree`. Expect: it fails, because `cli-tree.txt` does not exist.
- [ ] **Step 2: Capture the fixture from v0.1.0.** Run `git archive v0.1.0 | tar -x -C /private/tmp/hive-0.1.0`
  (create the dir first), copy `tests/cli_tree.rs` into `/private/tmp/hive-0.1.0/tests/`, then run
  `HIVE_WRITE_CLI_TREE=$PWD/tests/fixtures/v0.1.0/cli-tree.txt cargo test --offline --test cli_tree` inside
  `/private/tmp/hive-0.1.0`.
- [ ] **Step 3: Run and pass.** `cargo test --test cli_tree` passes on this branch, which still equals 0.1.0's code.
- [ ] **Step 4: Commit.** `git commit -m "test: pin the 0.1.0 command tree"`

_Requirements: SPLIT-3.1_

---

### Task 2: Output and state are compared with the 0.1.0 binary

**Files:**
- Create: `tests/capture.rs`
- Modify: `tests/common/mod.rs` (add `Hive::command_with`; `Hive::command` at line 97 calls it with `env!("CARGO_BIN_EXE_cargo-hive")`)
- Test: `tests/capture.rs`

**Reuse:** rung 2 — `tests/common/mod.rs` `Hive` (env setup at lines 97-106), `tests/common/registry_fixture.rs` `RegistryRepo::new` and `RegistryRepo::worktree` (line 152), `tests/common/cargo_fixture.rs` `cargo`.

**Interfaces:**
- Consumes: the v0.1.0 export at `/private/tmp/hive-0.1.0` (Task 1) and `HIVE_V010_BIN`
  (`/private/tmp/hive-0.1.0/target/debug/cargo-hive`).
- Produces:
  - `Hive::command_with(&self, bin: &Path, args: &[&str]) -> Command`, which applies the same `HIVE_HOME`,
    `CARGO_HOME` and `env_remove` set as `Hive::command`;
  - a fixed layout `/private/tmp/hive-capture/{golden,work,home}`;
  - `fn normalized(v: &serde_json::Value) -> serde_json::Value`, which replaces the values of these keys,
    wherever they appear, with `"<n>"`:
    - `id`, `at`, `ts`, `made_at`, `attempted_at`, `written_at`;
    - `filled_at`, `last_live_at`, `newest_at`, `stamp`, `identity`, `root_dev`, `root_ino`.

    `hive_version` is compared literally, since both binaries are 0.1.0 until a release bumps it.
  - `fn run_sequence(bin: &Path) -> Vec<(String, serde_json::Value, i32)>`. It first resets `work` and `home`
    (`rm -rf`, then `cp -cR golden work`) so both binaries see the same absolute paths. It writes
    `{"cache_roots": ["/private/tmp/hive-capture/home/no-cache"]}` with `Hive::config`. Then it runs, in order:
    - `doctor --json`
    - `repo add /private/tmp/hive-capture/work/main --json`
    - `status --json`
    - `status /private/tmp/hive-capture/work/main --no-sizes --json`
    - `explain /private/tmp/hive-capture/work/main --json`
    - `gc --plan --json`
    - `seed /private/tmp/hive-capture/work/wt --json`
    - `repo list --json`

**Depends-on:** Task 1

- [ ] **Step 1: Write the failing test.** `#[test] #[ignore] fn same_inputs_give_the_same_output_and_state()`.
  1. Build `golden` once with `RegistryRepo::new` (a git main with a registry dependency), a linked worktree
     `wt` (`RegistryRepo::worktree("wt")`), and one `cargo_fixture::cargo` build in each.
  2. Run `run_sequence` with the current binary, then move `home` to `home-new`.
  3. Run `run_sequence` with `HIVE_V010_BIN`, then move `home` to `home-old`.
  4. Assert the `normalized` JSON and exit codes are pairwise equal, and every file under `home-new` equals its
     twin under `home-old` (JSON files compared after `normalized`, others byte for byte).

  A second test, `state_written_by_0_1_0_reads_unchanged`:
  1. Run `run_sequence` with `HIVE_V010_BIN`.
  2. Copy `home` twice.
  3. Run `status --json` and `gc --plan --json` on one copy with the current binary and on the other with
     `HIVE_V010_BIN`.
  4. Assert both exit 0 and the two homes are equal under the same normalization.

  Run: `cargo test --test capture -- --ignored`. Expect: a panic naming `HIVE_V010_BIN`.
- [ ] **Step 2: Build the v0.1.0 binary.** Run `cargo build --offline` in `/private/tmp/hive-0.1.0`.
- [ ] **Step 3: Run and pass.** `HIVE_V010_BIN=/private/tmp/hive-0.1.0/target/debug/cargo-hive cargo test --test
  capture -- --ignored` passes on this branch.
- [ ] **Step 4: Commit.** `git commit -m "test: compare output and state with the 0.1.0 binary"`

_Requirements: SPLIT-3.2, SPLIT-4.2, SPLIT-4.3_

---

### Task 3: A read-only liveness report and the klynt baseline

**Files:**
- Create: `tests/liveness_report.rs`

**Reuse:** rung 2 — `cargo_target::identify`, `fingerprint::read_profile`, `liveness::liveness` (all `pub` in 0.1.0; built as in liveness.rs:744-754).

**Interfaces:**
- Consumes: env `HIVE_REPORT_TARGET`, `HIVE_REPORT_NOW` (unix seconds), `HIVE_REPORT_WINDOW_DAYS`,
  `HIVE_REPORT_OUT` (a path under `/private/tmp`).
- Produces: a JSON file `{"live": [[profile, dir_name]…], "dead": [[profile, dir_name]…], "undecided": [String…]}`,
  each list sorted. Checkpoint rule (Tasks 12, 22, 25): klynt's target changes between checkpoints, so each
  checkpoint re-runs the report from `/private/tmp/hive-0.1.0` and then from this branch, back to back, with the
  same `HIVE_REPORT_NOW` and window, and compares those two files.

**Depends-on:** Task 1

- [ ] **Step 1: Write the test.** `#[test] #[ignore] fn report()` builds `TargetDir { path, owner:
  Owner::Unresolved { reason: "report".into() }, profiles: <read_profile of each profile dir from identify>,
  size: None, unreadable: None }`, calls `liveness(&*cargo_hive::platform::current().unwrap().fs, &target, now,
  window)`, and writes the JSON. It asserts `HIVE_REPORT_OUT` starts with `/private/tmp/`.
  Run: `cargo test --test liveness_report -- --ignored` without the env vars. Expect: a panic naming
  `HIVE_REPORT_TARGET`.
- [ ] **Step 2: Baseline on klynt (manual, proof machine).** Copy the file into `/private/tmp/hive-0.1.0/tests/`
  and run it there with `HIVE_REPORT_TARGET=~/Developer/klynt/klynt/target`, a fixed `HIVE_REPORT_NOW` and
  `HIVE_REPORT_WINDOW_DAYS=7`, writing `/private/tmp/hive-split/live-0.1.0.json`. Then run it on this branch
  with the same values, writing `/private/tmp/hive-split/live-step0.json`. Expect `cmp` of the two files to print
  nothing.
  Record both commands and the `now` value in `.skills/_pending-workspace-split/auto/evidence.md`.
- [ ] **Step 3: Build-time baseline (manual, same session).** In `/private/tmp/hive-0.1.0`, append one comment
  line to `src/gc/run.rs` (the file Task 28 edits is the same code at `crates/gc/src/gc/run.rs`), run `cargo build --offline` and time it. Do this 5 times, restoring the line each time,
  and record the median in the same evidence file.
- [ ] **Step 4: Commit.** `git commit -m "test: read-only liveness report for comparing releases"`

_Requirements: SPLIT-4.1, SPLIT-8.1_

---

### Task 4: Platform owns process spawning

**Files:**
- Create: `src/platform/proc.rs`
- Modify: `src/platform/mod.rs`, `src/cargo_target.rs` (runner at lines 217-550, through `finish_readers`, plus its `CommandExt`/`mpsc`/`thread` imports, moves out), `src/git.rs`, `src/doctor.rs`, `src/bench/command.rs`, `src/bench/mod.rs`, `src/cli.rs`, and every `tests/*.rs` that imports `cargo_hive::cargo_target::{run_bounded, Capture, ChildOutputError, …}` (find them with `grep -rln 'cargo_target::' tests`)
- Test: `src/platform/proc.rs`

**Reuse:** rung 2 — src/platform/{mod,apfs}.rs stay as they are (`Fs` trait mod.rs:295, `Apfs` apfs.rs:47, lock self-exec apfs.rs:172-245, `current()` mod.rs:444-455); the spawn runner `output_with_deadline`, `run_bounded`, `Capture`, `Ran`, `ChildOutputError`, `kill_pgid` (cargo_target.rs:236-389) moves in unchanged.

**Interfaces:**
- Produces: `platform::proc::Spec { program: OsString, args: Vec<OsString>, cwd: Option<PathBuf>, env_clear: bool,
  env_set: Vec<(OsString, OsString)>, env_remove: Vec<OsString>, stdin_null: bool, own_process_group: bool }`
  with `Spec::new(program)`.
  - `output_with_deadline(&Spec, Duration, impl FnMut() -> bool) -> Result<Output, ChildOutputError>` and
    `run_bounded(&Spec, Duration, Capture, impl FnMut() -> bool) -> Result<Ran, ChildOutputError>` keep 0.1.0's
    behaviour: stdin null, own process group, deadline, pipe drain, group kill.
  - `output(&Spec) -> io::Result<Output>` runs with no deadline, as git.rs:115/:176 do in 0.1.0.
  - `Capture`, `Ran`, `ChildOutputError` are unchanged in shape.
- Consumes: none.

**Depends-on:** Task 1, Task 2

- [ ] **Step 1: Failing test.** In `src/platform/proc.rs` tests:
  - `run_bounded` on `Spec::new("sleep")` with arg `5` and a 100 ms deadline returns `Err(TimedOut)`;
  - `output` on `Spec::new("sh")` with args `-c`, `echo $X`, `env_set` `X=1` and `env_clear: true` prints `1\n`;
  - `env_remove` of `HOME` leaves it unset in the child (`sh -c 'echo ${HOME-unset}'`).

  Run `cargo test --lib platform::proc::`. Expect: a compile error, because `platform::proc` does not exist.
- [ ] **Step 2: Implement.** Move the runner from `cargo_target.rs` into `proc.rs`, taking `&Spec` and building
  `std::process::Command` inside. Rewrite the six production spawn sites outside platform to build a `Spec` with
  the same program, args, cwd and env calls as today: cargo_target.rs:554, git.rs:64, doctor.rs:428, :514, :748
  and bench/command.rs:142. The three sites already in platform (platform/mod.rs:24, apfs.rs:172, :238) move to
  `Spec` too. Re-export nothing from `cargo_target`.
- [ ] **Step 3: Run.** `cargo test` (whole suite) and `cargo test --test cli_tree` pass, and
  `HIVE_V010_BIN=… cargo test --test capture -- --ignored` passes.
- [ ] **Step 4: Commit.** `git commit -m "refactor(platform): own child-process spawning"`

_Requirements: SPLIT-2.5_

---

### Task 5: One module holds every child's environment

**Files:**
- Create: `src/env.rs`
- Modify: `src/lib.rs`, `src/cargo_target.rs` (lines 564-574), `src/git.rs` (lines 64-75), `src/doctor.rs` (lines 355-374, 428, 514, 748-760), `src/bench/command.rs` (lines 84-143)
- Test: `src/env.rs`

**Reuse:** rung 2 — the toolchain family from doctor.rs (`drop_directory_toolchain`, `caller_toolchain_kept*`, :355-397) and today's env-scrub blocks; `env` itself is rung 7 (design: the minimum that gives ARCH-13 one owner).

**Interfaces:**
- Consumes: `platform::proc::Spec` (Task 4).
- Produces: `env` owns `pub enum ResolveEnv` (moved from doctor.rs, same variants) and takes plain data, never
  `Toolchain`, so `toolchain` may depend on `env` and not the reverse. Each builder takes the parent environment
  as a parameter; production passes `&std::env::vars_os().collect::<Vec<_>>()`:
  - `env::metadata(spec: &mut Spec, parent: &[(OsString, OsString)])`;
  - `env::git(spec: &mut Spec, parent: &[(OsString, OsString)])`;
  - `env::rustup_which(spec: &mut Spec, parent: &[(OsString, OsString)], how: ResolveEnv)`;
  - `env::probe_build(spec: &mut Spec, parent: &[(OsString, OsString)], cargo_home: &Path, rustup_home: &Path,
    toolchain_name: &OsStr)`, with the values doctor.rs:748-760 passes today;
  - `env::timed_build(spec: &mut Spec, parent: &[(OsString, OsString)], cargo: &Path, rustc: &Path,
    toolchain_name: &OsStr)`, with the values bench/command.rs:84-143 reads from `Toolchain` today.

  `#[cfg(test)] fn child_env(spec: &Spec, parent: &[(OsString, OsString)]) -> BTreeMap<OsString, OsString>`
  applies `env_clear`/`env_remove`/`env_set` to the given parent. It never reads the live process environment.

**Depends-on:** Task 4

- [ ] **Step 1: Failing test.** A table-driven test runs each of the five rules on a fixed parent environment
  that contains:
  - `CARGO_TARGET_DIR`, `CARGO_BUILD_TARGET_DIR`, `CARGO_BUILD_BUILD_DIR`;
  - `GIT_DIR`, `GIT_WORK_TREE`;
  - `RUSTUP_TOOLCHAIN=x`, `RUSTUP_TOOLCHAIN_SOURCE=directory`;
  - `HOME`, `PATH`.

  It asserts `child_env` equals the expected map, written out per rule from the 0.1.0 code
  (cargo_target.rs:564-574, git.rs:70-74, doctor.rs:355-374/:428/:514/:748-760, bench/command.rs:84-143). A
  second row per rule uses `RUSTUP_TOOLCHAIN_SOURCE=cli` and expects `RUSTUP_TOOLCHAIN` kept.
  Run `cargo test --lib env::`. Expect: a compile error, because `env` does not exist.
- [ ] **Step 2: Implement.** Move each scrub block into its `env::*` function and call it from the site.
- [ ] **Step 3: Run.** `cargo test` and the capture comparison pass.
- [ ] **Step 4: Commit.** `git commit -m "refactor: one module for every child process environment"`

_Requirements: SPLIT-2.5, SPLIT-2.6_

---

### Task 6: Platform owns locks, signals, process probes and the clock

**Files:**
- Create: `src/platform/flock.rs`, `src/platform/signals.rs`, `src/platform/clock.rs`
- Modify: `src/platform/mod.rs`, `src/state.rs` (lines 156-215, 450-470), `src/doctor.rs` (lines 179-233), `src/receipt.rs` (lines 5, 128), `src/explain.rs` (lines 17, 551), `src/cli.rs`
- Test: `src/platform/flock.rs`, `src/platform/clock.rs`

**Reuse:** rung 2 — the three `flock` loops (state.rs:164-215); signals `CANCEL`, `on_signal`, `install_signal_handlers` and `kill(pid, 0)` (doctor.rs:179-233); `utc_date`/`utc_timestamp` (state.rs).

**Interfaces:**
- Produces:
  - `platform::flock::{exclusive_or_cancelled(&File, impl FnMut() -> bool) -> Result<FlockGuard, FlockError>,
    shared_or_cancelled(&File, impl FnMut() -> bool) -> Result<FlockGuard, FlockError>,
    try_exclusive(&File) -> io::Result<Option<FlockGuard>>}`, where `FlockError` is `Cancelled | Io(io::Error)`
    and `FlockGuard` unlocks on drop;
  - `platform::signals::{install() -> io::Result<()>, cancelled() -> bool}`, `platform::alive(pid: i32) -> bool`;
  - `platform::clock::{utc_timestamp(SystemTime) -> String, utc_date(SystemTime) -> String}`.
- Consumes: none.

**Depends-on:** Task 4

- [ ] **Step 1: Failing test.**
  - flock: a second `try_exclusive` on the same file through another `File` returns `Ok(None)` while the first
    guard lives and `Ok(Some(_))` after it drops; `exclusive_or_cancelled` with a cancel that returns `true`
    returns `Err(Cancelled)` while another guard is held;
  - clock: `utc_timestamp(UNIX_EPOCH + 86_400 s)` equals what `state::utc_timestamp` returned for the same input
    before the move. Read that string from the current code and paste it into the test.

  Run `cargo test --lib platform::`. Expect: a compile error.
- [ ] **Step 2: Implement.** Move the code. `State`'s three lock methods call `flock::*`. doctor and cli call
  `signals`. `sweep_stale` uses `alive`. receipt and explain use `clock`.
- [ ] **Step 3: Run.** `cargo test`, including tests/platform_locks.rs and tests/liveness_busy.rs, and the capture
  comparison pass.
- [ ] **Step 4: Commit.** `git commit -m "refactor(platform): own flock, signals, process probes and the clock"`

_Requirements: SPLIT-2.1, SPLIT-4.2_

---

### Task 7: errno checks use io::ErrorKind; host_fs and test helpers move into platform

**Files:**
- Create: `src/platform/testing.rs`
- Modify: `src/seed/place.rs` (line 610), `src/seed/fill.rs` (lines 506, 509, 750), `src/seed/store.rs` (lines 766-767, test helpers 1215-1237, 1534), `src/seed/mod.rs` (lines 94-98), `src/seed/source.rs` (test lines 829, 837), `src/explain.rs` (lines 674-764), `src/platform/mod.rs` (line 344 `Hooked`)
- Test: `src/platform/mod.rs`, `src/seed/fill.rs`

**Reuse:** rung 2 — `host_fs`/`DirListFs` (explain.rs:674-764) move into platform; the errno checks become `io::ErrorKind` tests (rung 3, std); `Hooked` (mod.rs:344) becomes `#[doc(hidden)] pub mod testing`.

**Interfaces:**
- Produces: `platform::host_fs() -> Box<dyn Fs>`; `#[doc(hidden)] pub mod platform::testing { Hooked, set_env,
  remove_env, openat_raw, unlinkat_raw, flock_raw }` with the signatures of the seed/store.rs and seed/source.rs
  helpers they replace.
- Consumes: none.

**Depends-on:** Task 6

- [ ] **Step 1: Failing test.** The platform test asserts `io::Error::from_raw_os_error(n).kind()` for the rows
  `(28, StorageFull)`, `(18, CrossesDevices)`, `(17, AlreadyExists)`, `(66, DirectoryNotEmpty)`,
  `(20, NotADirectory)`, with `n` taken from `libc::{ENOSPC, EXDEV, EEXIST, ENOTEMPTY, ENOTDIR}`. The seed/fill.rs
  test asserts that a clone error with `ErrorKind::PermissionDenied` takes the non-EXDEV branch.
  Run `cargo test --lib platform:: seed::fill::`. Expect: the fill test fails to compile on the new matcher.
- [ ] **Step 2: Implement.** Replace each errno match with the `ErrorKind` match (keeping any existing `ErrorKind`
  arm). Move `host_fs`/`DirListFs`. Move the unsafe test helpers into `platform::testing` and call them from
  seed's tests.
- [ ] **Step 3: Run.** `cargo test` and the capture comparison pass. Check that
  `grep -rn 'libc::\|unsafe\|cfg(target_os' src --include=*.rs | grep -v '^src/platform/'` lists only test-gated
  lines.
- [ ] **Step 4: Commit.** `git commit -m "refactor: keep raw OS knowledge in platform"`

_Requirements: SPLIT-2.7, SPLIT-2.1_

---

### Task 8: Toolchain resolution and cargo metadata leave doctor and cargo_target

**Files:**
- Create: `src/toolchain.rs`, `src/metadata.rs`
- Modify: `src/doctor.rs` (toolchain family :355-397 and helpers), `src/cargo_target.rs` (`Resolved` :208, `resolve` :553, `caller_overrides` :613, `TARGET_VARS`, `METADATA_DEADLINE`), `src/seed/evict.rs` (lines 12, 572), `src/seed/fill.rs` (line 868), `src/seed/mod.rs` (line 17), `src/discover.rs`, `src/bench/command.rs`, `src/cli.rs`, `src/lib.rs`, and every `tests/*.rs` or `tests/common/*.rs` naming a moved item (`grep -rln 'doctor::\(Toolchain\|resolve_toolchain\)\|cargo_target::\(resolve\|Resolved\|TARGET_VARS\)' tests`)
- Test: existing doctor and cargo_target unit tests move with their items

**Reuse:** rung 2 — `Resolved` (cargo_target.rs:208), `resolve`, `caller_overrides`, `TARGET_VARS`, `METADATA_DEADLINE` from cargo_target.rs (:208, :553-620); the toolchain family from doctor.rs (`Toolchain`, `resolve_toolchain*`, `drop_directory_toolchain`, `caller_toolchain_kept*`, :355-397 and helpers).

**Interfaces:**
- Produces: `toolchain::{Toolchain, ResolveEnv, resolve_toolchain, resolve_toolchain_until,
  resolve_toolchain_own_dir_until, drop_directory_toolchain, caller_toolchain_kept}`;
  `metadata::{Resolved, resolve, caller_overrides, TARGET_VARS, METADATA_DEADLINE}`.
- Consumes: `env::*` and `env::ResolveEnv` (Task 5). `toolchain` depends on `env`; `env` never names `Toolchain`.

**Depends-on:** Task 5

- [ ] **Step 1: Failing check.** Add `src/lib.rs` `pub mod toolchain; pub mod metadata;` and change seed's imports
  to `crate::toolchain::Toolchain`. Run `cargo check`. Expect: unresolved-import errors.
- [ ] **Step 2: Implement.** Move the items. Nothing in `cargo_target` imports `doctor`, and nothing in `seed`
  imports `doctor`. Check with `grep -n 'crate::doctor' src/cargo_target.rs src/seed`, which must print nothing.
- [ ] **Step 3: Run.** `cargo test` and the capture comparison pass.
- [ ] **Step 4: Commit.** `git commit -m "refactor: move toolchain and metadata out of doctor and cargo_target"`

_Requirements: SPLIT-1.2, SPLIT-1.3_

---

### Task 9: Shared context, walks and target locks leave plan_tier and cli

**Files:**
- Create: `src/ctx.rs`, `src/locks.rs`
- Modify: `src/plan_tier/mod.rs` (lines 18-68), `src/plan_tier/locks.rs` (removed), `src/plan_tier/apply.rs` (line 11), `src/cli.rs` (`cross_check_notes` :915), `src/seed/{mod,fill,place,source,evict}.rs`, `src/bench/mod.rs`, `src/lib.rs`, and every `tests/*.rs` or `tests/common/*.rs` naming `plan_tier::{Ctx, locks, checkout_walk, …}` (`grep -rln 'plan_tier::\(Ctx\|locks\|checkout_walk\|split_checkout_faults\|faults_that\)' tests`)
- Test: existing plan_tier and seed unit tests

**Reuse:** rung 2 — `Ctx`, `checkout_walk`, `split_checkout_faults`, `faults_that_can_hide_an_owner` from plan_tier/mod.rs:18-68; plan_tier/locks.rs; `cross_check_notes` (cli.rs:915). Moving it removes the plan_tier → cli edge.

**Interfaces:**
- Produces: `ctx::{Ctx, checkout_walk, split_checkout_faults, faults_that_can_hide_an_owner}`;
  `locks::{acquire, acquire_capped, Acquired, Locked, Reason}` (the names plan_tier/locks.rs has today, lines 12-47);
  `plan_tier::apply::cross_check_notes`.
- Consumes: none new.

**Depends-on:** Task 8

- [ ] **Step 1: Failing check.** Point seed's imports at `crate::ctx::Ctx` and `crate::locks`, and run
  `cargo check`. Expect: unresolved imports.
- [ ] **Step 2: Implement.** Move the items. `grep -n 'crate::cli' src/plan_tier src/seed` and
  `grep -n 'crate::plan_tier' src/seed` must both print nothing.
- [ ] **Step 3: Run.** `cargo test` and the capture comparison pass.
- [ ] **Step 4: Commit.** `git commit -m "refactor: move shared context and target locks below plan_tier"`

_Requirements: SPLIT-1.2, SPLIT-1.3_

---

### Task 10: Accounting and caps leave gc

**Files:**
- Create: `src/ledger/mod.rs`, `src/ledger/account.rs`, `src/caps.rs`
- Modify: `src/gc.rs`, `src/gc/run.rs` (lines 20-22, 59, 384-456), `src/gc/ledger.rs` and `src/gc/account.rs` (removed), `src/liveness.rs` (line 10), `src/explain.rs` (line 11), `src/locks.rs` (line 44, `CRATE_DIR_CAP`), `src/cli.rs` (line 859), `src/seed/{evict,fill,place,source,store,mod}.rs`, `src/plan_tier/{apply,orphan,build}.rs`, `src/bench/mod.rs`, `src/lib.rs`, and every `tests/*.rs` or `tests/common/*.rs` naming `gc::{Ledger, account, DEPS_CAP, CRATE_DIR_CAP, measure, …}` (`grep -rln 'gc::\(Ledger\|account\|DEPS_CAP\|CRATE_DIR_CAP\|measure\|Footprint\|scan\|record\)' tests`)
- Test: existing gc/ledger and gc/account unit tests move with the files

**Reuse:** rung 2 — gc accounting: gc/ledger.rs, gc/account.rs, and `Scanned`, `Footprint`, `measure`, `record`, `scan`, `CRATE_DIR_CAP`, `DEPS_CAP` from gc/run.rs:20-456.

**Interfaces:**
- Produces: `ledger::{Ledger, Tally, Bytes, UNMEASURED_CAP, measure, Footprint, Scanned, scan, record,
  remove_counted, Removed, RemoveFailed}`; `caps::{CRATE_DIR_CAP, DEPS_CAP}`.
- Consumes: none new.

**Depends-on:** Task 9

- [ ] **Step 1: Failing check.** Point seed, liveness and explain at `crate::caps` and `crate::ledger`, and run
  `cargo check`. Expect: unresolved imports.
- [ ] **Step 2: Implement.** Move the items. `grep -rn 'crate::gc' src --include='*.rs' | grep -v '^src/gc'`
  must print only `src/plan_tier/*`, `src/cli.rs` and `src/bench/*` lines (callers above gc).
- [ ] **Step 3: Run.** `cargo test` and the capture comparison pass.
- [ ] **Step 4: Commit.** `git commit -m "refactor: move accounting and caps out of gc"`

_Requirements: SPLIT-1.3_

---

### Task 11: Seed receives the liveness verdict through a function

**Files:**
- Create: `src/gc/liveness.rs`, `tests/seed_liveness.rs`
- Modify: `src/liveness.rs` (the algorithm `liveness()` :210 and its private walk move out; types :31-60, `embedded_hashes`, `unit_timestamp` stay), `src/ctx.rs`, `src/seed/source.rs` (lines 163, 183, 310, 327; test module), `src/seed/evict.rs` (line 24 `EvictCtx`, line 67), `src/seed/fill.rs` (test module :986-1232), `src/seed/mod.rs` (test module :539-580), `src/cli.rs` (line 1050), `src/bench/mod.rs`, `src/plan_tier/build.rs`, `src/plan_tier/apply.rs` (line 807), `src/gc.rs`, `tests/store_gc.rs`, `tests/seed.rs`, `tests/seed_fill.rs`, `tests/bench.rs`, `tests/common/prove.rs`, `tests/common/cargo_fixture.rs`, `tests/common/plan_fixture.rs`, `tests/liveness*.rs`, `tests/guards.rs` (line 437: the no-process guard reads both `../src/liveness.rs` and `../src/gc/liveness.rs`)
- Test: `tests/seed_liveness.rs`

**Reuse:** rung 2 — the liveness algorithm from liveness.rs (`liveness()` :210 and its private graph walk) moves out; the liveness *result types* (liveness.rs:31-60), `embedded_hashes`, `unit_timestamp` stay (design: `cargo-hive-gc` and `cargo-hive-core` sections; ADR-0003).

**Interfaces:**
- Produces: `liveness::LivenessFn = fn(&dyn Fs, &TargetDir, SystemTime, Duration) -> Liveness`;
  `gc::liveness::liveness` (the same signature); `Ctx.liveness: LivenessFn`, `EvictCtx.liveness: LivenessFn`;
  `seed::source::holds(…, liveness: LivenessFn)` and `live_registry_units`, both `#[doc(hidden)] pub`;
  `#[cfg(test)] fn no_liveness(_: &dyn Fs, _: &TargetDir, _: SystemTime, _: Duration) -> Liveness` in seed, which
  panics with "this test reaches liveness; move it to tests/seed_liveness.rs".
- Consumes: `ctx::Ctx` (Task 9).

**Depends-on:** Task 10

- [ ] **Step 1: Failing test.** Create `tests/seed_liveness.rs` holding the seed unit tests that reach `holds` or
  `live_registry_units`, copied with assertions unchanged and constructing `Ctx`/`EvictCtx` with
  `liveness: cargo_hive::gc::liveness::liveness`. Run `cargo test --test seed_liveness`. Expect: compile errors on
  the missing `liveness` field and module.
- [ ] **Step 2: Implement.** Move the algorithm to `src/gc/liveness.rs`, together with the unit tests in
  `src/liveness.rs`'s test module that call `liveness()` or the walk; tests of the types, `embedded_hashes` and
  `unit_timestamp` stay. Add the fields and the `holds`
  parameter. Call through the field at seed/source.rs:183 and :327. Production sites (cli.rs, bench/mod.rs) pass
  `gc::liveness::liveness`. Seed unit tests that stay pass `no_liveness`. Delete the moved tests from seed's test
  modules.
- [ ] **Step 3: Run.** `cargo test`. Every remaining seed unit test passes without hitting the `no_liveness` panic.
  `grep -n 'liveness::liveness\|gc::' src/seed` prints nothing.
- [ ] **Step 4: Commit.** `git commit -m "refactor(seed): take the liveness verdict as a function (ADR-0003)"`

_Requirements: SPLIT-1.3, SPLIT-1.4_

---

### Task 12: Checkpoint after the untangling

**Files:**
- Modify: `.skills/_pending-workspace-split/auto/evidence.md` (local evidence, not committed)

**Reuse:** rung 2 — `tests/liveness_report.rs` (Task 3), `tests/capture.rs` (Task 2).

**Interfaces:**
- Consumes: the report and capture tests.
- Produces: evidence lines `step1 live/dead equal: yes|no`, `step1 capture equal: yes|no`.

**Depends-on:** Task 11

- [ ] **Step 1: Run the report.** Following Task 3's checkpoint rule, run the 0.1.0 report
  (`live-0.1.0-step1.json`) and then this branch's (`live-step1.json`) back to back on klynt, with one fresh
  `now` and the same window. Run `cmp` on the two files. Expect: no output.
- [ ] **Step 2: Run the capture comparison and the full gate.** Expect: all pass.
- [ ] **Step 3: Record.** Write both results in the evidence file. A mismatch stops the plan: report it and do not
  start Task 13.

_Requirements: SPLIT-4.1_

---

### Task 13: Edition 2024 on the single crate

**Files:**
- Modify: `Cargo.toml` (`edition = "2024"`), `src/**/*.rs` and `tests/**/*.rs` where `cargo fix --edition` rewrites
- Test: whole suite

**Reuse:** rung 4 — `cargo fix --edition` (Cargo).

**Interfaces:**
- Consumes: none. Produces: none.

**Depends-on:** Task 12

- [ ] **Step 1: Run the migration lints.** With `edition = "2021"` still set, run `cargo fix --edition
  --all-targets --allow-dirty 2>&1 | tee /private/tmp/hive-split/edition.log`. Expect: it completes. Save `git diff > /private/tmp/hive-split/edition-fix.diff`.
  `cargo fix` applies the `if-let-rescope` rewrite itself and logs only per-file fix counts, so the diff, not the
  log, lists every rewritten scrutinee. Check each hunk keeps the 2021 drop point and record the diff summary in
  the evidence file.
- [ ] **Step 2: Resolve each drop-order warning while still on edition 2021.** For every `if-let-rescope` or
  `tail-expr-drop-order` hit, rewrite the site to keep 0.1.0's drop point (a `match` or an explicit binding), as
  the lint suggests. Then run `cargo clippy --all-targets -- -W rust-2024-compatibility -D warnings` with
  `edition = "2021"`. Expect: exit 0. This still catches `tail-expr-drop-order`, which has no auto-fix. The diff
  and this exit 0 together are SPLIT-5.2's evidence in the evidence file. On edition 2024 these lints no
  longer fire, so the check must run before the flip.
- [ ] **Step 3: Flip and format.** Set `edition = "2024"`, then run `cargo fmt`, because the 2024 style edition
  reorders some imports. As a second check, inspect the guard sites found by
  `grep -rn 'try_lock_exclusive\|store_shared_or_cancelled\|try_store_exclusive\|\.locked(\|locked_or_cancelled\|acquire(' src`;
  the line numbers have moved since design-scan.md §6.
- [ ] **Step 4: Run.** The full gate (including `cargo fmt --check`), tests/platform_locks.rs,
  tests/liveness_busy.rs and the capture comparison pass.
- [ ] **Step 5: Commit.** `git commit -m "build: move to edition 2024"`

_Requirements: SPLIT-5.2_

---

### Task 14: The root becomes a workspace with one member

**Files:**
- Create: `crates/cargo-hive/Cargo.toml`, `crates/cargo-hive/tests/workspace.rs`, `crates/cargo-hive/tests/version.rs`
- Modify: `Cargo.toml` (virtual root), `Cargo.lock`, `src/` → `crates/cargo-hive/src/` (move), `tests/` → `crates/cargo-hive/tests/` (move), `crates/cargo-hive/tests/skill.rs` (lines 10-25 repo root), `crates/cargo-hive/tests/skill/names.rs` (line 265 sweep), `crates/cargo-hive/tests/liveness.rs` (lines 270-293 manifest test), `crates/cargo-hive/tests/seed.rs` (line 2533: `repo_root()/target/klynt-run`), `.gitignore` (add `/crates/*/target`), `scripts/fresh-probe.sh` (lines 9-13 default). The guards.rs `include_str!` paths stay relative to the test file and still resolve, because `src/` and `tests/` move together.
- Test: `crates/cargo-hive/tests/workspace.rs`, `crates/cargo-hive/tests/version.rs`

**Reuse:** rung 4 — Cargo workspaces (`[workspace]`, `[workspace.package]`, `[workspace.dependencies]`, `[workspace.lints]`, `resolver = "3"`); the guard test reuses `cargo metadata --format-version 1`, already parsed with `serde_json` in src/cargo_target.rs.

**Interfaces:**
- Produces:
  - `workspace.rs`: a `LAYERS: &[(&str, u8)]` table (now `[("cargo-hive", 5)]`), a `FEATURES: &[&str]` list (now
    empty), and `fn metadata() -> serde_json::Value` (`cargo metadata --format-version 1 --no-deps`, run from
    `env!("CARGO_MANIFEST_DIR")`);
  - `fn repo_root() -> PathBuf` in tests/skill.rs and tests/skill/names.rs: the nearest ancestor of
    `CARGO_MANIFEST_DIR` whose `Cargo.toml` contains `[workspace]`.
- Consumes: none.

**Depends-on:** Task 13

- [ ] **Step 1: Failing test.** Write `workspace.rs` with six tests over `metadata()` (packages and workspace root):
  - `every_member_is_edition_2024`;
  - `members_depend_only_on_lower_layers_and_no_feature_on_another`: for each package, each dependency whose name
    is a member must have a strictly lower layer, and must not be in `FEATURES` when the package is;
  - `member_dependencies_are_pinned_exactly`: each dependency on a member has `req == format!("={version}")`;
  - `only_cargo_hive_has_a_binary`: `targets[].kind` contains `"bin"` only for `cargo-hive`;
  - `the_root_is_a_virtual_workspace`: the root `Cargo.toml` (from `metadata()["workspace_root"]`) has a
    `[workspace]` table and no `[package]` table;
  - `every_member_inherits_version_and_edition`: each member's `Cargo.toml` text contains
    `version.workspace = true` and `edition.workspace = true`.

  Write `version.rs`: run `cargo-hive --version`, take the version word, run `HIVE_HOME=<scratch> cargo-hive
  doctor --json`, and assert the receipt's `hive_version` equals it. Run `cargo test --test workspace`, with the file
  still at `tests/workspace.rs` in the single crate. Expect: `the_root_is_a_virtual_workspace` and
  `every_member_inherits_version_and_edition` fail, because the root is still a package.
- [ ] **Step 2: Implement.**
  - Root `Cargo.toml`: `[workspace] members = ["crates/cargo-hive"] resolver = "3"`, with `[workspace.package]`
    holding version 0.1.0, edition 2024, license, repository and description (no `rust-version`, as 0.1.0 has none), and
    `[workspace.dependencies]` holding clap, serde, serde_json, libc and applesauce at today's versions, and an empty
    `[workspace.lints.clippy]` table so later members can set `[lints] workspace = true`.
  - Move `src/` and `tests/` with `git mv`. `crates/cargo-hive/Cargo.toml` inherits the package fields, keeps
    `[lib]`/`[[bin]]`, and has `include = ["/src/**/*", "/Cargo.toml", "/README.md", "/LICENSE-MIT",
    "/LICENSE-APACHE"]`; copy README.md and both license files into `crates/cargo-hive/`.
  - Rewrite the repo-root tests (tests/skill.rs:15, :25; tests/skill/names.rs:265; tests/liveness.rs:279-293;
    tests/seed.rs:2533) to use `repo_root()`, and make names.rs sweep `crates/*/src`.
  - `fresh-probe.sh` defaults to `crates/cargo-hive/src/size.rs cargo-hive` and accepts several `<file>
    <package>` pairs.
- [ ] **Step 3: Run.** At the root: `cargo test --workspace`, `cargo test --test workspace`, `cargo test --test
  version`, `cargo test --test cli_tree`, the capture comparison, `cargo clippy --workspace --all-targets -- -D
  warnings`, and `cargo check --workspace --target x86_64-unknown-linux-gnu` all pass. `scripts/fresh-probe.sh`
  prints `GREEN`.
- [ ] **Step 4: Commit.** `git commit -m "build: make the root a workspace with cargo-hive as its member"`

_Requirements: SPLIT-1.1, SPLIT-3.3, SPLIT-3.4, SPLIT-3.5, SPLIT-3.6, SPLIT-5.1, SPLIT-6.2, SPLIT-7.4, SPLIT-7.5_

---

### Task 15: Extract the platform crate and switch on the OS boundary

**Files:**
- Create: `crates/platform/Cargo.toml`, `crates/platform/src/lib.rs`, `crates/platform/README.md`, `crates/platform/LICENSE-MIT`, `crates/platform/LICENSE-APACHE`, `clippy.toml`, `crates/cargo-hive/tests/os_boundary.rs`
- Modify: `crates/cargo-hive/src/platform/` → `crates/platform/src/` (move), `crates/cargo-hive/src/lib.rs` (forbid attribute; drop `pub mod platform`), every `use crate::platform` in `crates/cargo-hive/src` → `cargo_hive_platform`, `crates/cargo-hive/tests/*.rs` (imports; file-level clippy allow), unit test modules with `Command` (cargo_target.rs, doctor.rs, git.rs, cli.rs, seed/source.rs, seed/store.rs, fingerprint.rs; test-module allow), `Cargo.toml` (member, workspace dependency `cargo-hive-platform = { path = "crates/platform", version = "=0.1.0" }`), `crates/cargo-hive/tests/workspace.rs` (`LAYERS` gains `("cargo-hive-platform", 0)`)
- Test: `crates/cargo-hive/tests/os_boundary.rs`, `crates/platform` unit tests

**Reuse:** rung 2 — src/platform/{mod,apfs}.rs stay as they are (`Fs` trait mod.rs:295, `Apfs` apfs.rs:47, lock self-exec apfs.rs:172-245, `current()` mod.rs:444-455); rung 4 — rustc `forbid(unsafe_code, reason)`, clippy `disallowed-types`/`disallowed-methods` with `reason`, and Cargo `[lints]`.

**Interfaces:**
- Produces:
  - crate `cargo_hive_platform` (working name), exposing what `crate::platform` exposed;
  - `os_boundary.rs` with `const RULE: &str = "only the platform crate may touch the OS"`;
  - `fn non_test_cfg_target_os(src: &str) -> Vec<usize>`, which returns line numbers, treats a line as test code
    when it lies inside a `#[cfg(test)]` item tracked by brace depth or its attribute predicate contains `test`, and
    skips `tests/` files;
  - `fn members() -> Vec<(String, PathBuf, serde_json::Value)>` from `cargo metadata`.
- Consumes: `workspace.rs` `metadata()` (Task 14).

**Depends-on:** Task 14

- [ ] **Step 1: Failing test.** Write `os_boundary.rs` with three tests over every member except
  `cargo-hive-platform`. Each fails with `"{RULE}: {member} …"`:
  - `no_member_but_platform_depends_on_libc`: `libc` in normal deps (kind null);
  - `no_member_but_platform_has_non_test_cfg_target_os`: run over every `src/**/*.rs`;
  - `every_member_but_platform_forbids_unsafe`: for every target of kind `lib` or `bin` (`targets[].src_path`),
    the file's leading block of `//!` comments and inner attributes contains
    `#![forbid(unsafe_code, reason = "only the platform crate may touch the OS (ARCH-15)")]`.

  Add a unit test for `non_test_cfg_target_os` covering:
  - `#[cfg(test)]\nmod t {\n #[cfg(target_os = "macos")]\n fn f(){}\n}` → `[]`;
  - `#[cfg(all(test, target_os = "macos"))]\nfn f(){}` → `[]`;
  - `#[cfg(test)]\n#[cfg(target_os = "macos")]\nmod t {\n}` → `[]`;
  - `#[cfg(target_os = "macos")]\nfn f(){}` → `[1]`.

  Run `cargo test --test os_boundary`. Expect: the forbid test fails for `cargo-hive`.
- [ ] **Step 2: Implement.** Create the crate and move the files. Its `Cargo.toml` depends on `libc` and on
  `applesauce` for macOS, with its own `[lints.clippy] disallowed_types = "allow"`,
  `disallowed_methods = "allow"` and no `[lints] workspace = true`. Cargo does not allow a member to combine
  inherited and local lints.
  - Root `clippy.toml`: `disallowed-types = [{ path = "std::process::Command", reason = "only the platform crate
    may touch the OS" }]` and `disallowed-methods = [{ path = "std::process::Command::new", reason = "only the
    platform crate may touch the OS" }]`.
  - `unsafe` is forbidden only by each non-platform crate root's attribute, not by `[workspace.lints]`: a lint in
    `[lints]` would also cover `cargo-hive`'s integration tests, which keep `unsafe { set_var }`
    (tests/common/mod.rs:466) as separate test crates.
  - Add the forbid attribute to `crates/cargo-hive/src/lib.rs` and `main.rs`.
  - Add the allows to test modules and test files. `libc` becomes a dev-dependency of `cargo-hive`.
- [ ] **Step 3: Prove it fails with the rule's name (manual, temp copy).** Copy the workspace to
  `/private/tmp/hive-split/bad`:
  - add `pub fn f() { unsafe {} }` to `crates/cargo-hive/src/size.rs`, run `cargo build`, and expect an error
    containing the rule;
  - revert it, add `let _ = std::process::Command::new("true");` and run `cargo clippy -- -D warnings`; expect an
    error with `note: only the platform crate may touch the OS`.
- [ ] **Step 4: Run.** The full gate at the root, `cargo test --test os_boundary`, the capture comparison, and
  `cargo check --workspace --target x86_64-unknown-linux-gnu` all pass. On non-macOS the stub still returns
  `Err(NotMacOs)`.
- [ ] **Step 5: Commit.** `git commit -m "build: extract the platform crate and enforce the OS boundary"`

_Requirements: SPLIT-1.2, SPLIT-2.1, SPLIT-2.2, SPLIT-2.3, SPLIT-2.4, SPLIT-3.7, SPLIT-3.8_

---

### Task 16: Extract the core crate

**Files:**
- Create: `crates/core/{Cargo.toml,src/lib.rs,README.md,LICENSE-MIT,LICENSE-APACHE}`
- Modify: move `fingerprint.rs`, `classify.rs`, `size.rs`, `receipt.rs`, `liveness.rs` (types and helpers), `ledger/`, `caps.rs`, `locks.rs` and the target-dir part of `cargo_target.rs` (`TAG_LINE`, `Layout`, `identify`, `identify_bounded`, `realpath_lossy` made `pub`, `Owner`, `TargetDir`) from `crates/cargo-hive/src/` into `crates/core/src/` (target-dir part as `target.rs`); `crates/cargo-hive/tests/guards.rs` (line 437: read `../../core/src/liveness.rs` and `../src/gc/liveness.rs`); `Cargo.toml` (member + workspace dep); importers in `crates/cargo-hive/src` and `tests/`; `crates/cargo-hive/tests/workspace.rs` (`("cargo-hive-core", 1)`)
- Test: moved unit tests; `crates/cargo-hive/tests/workspace.rs`

**Reuse:** rung 2 — these move whole: fingerprint.rs, classify.rs, size.rs, receipt.rs; the target-dir types and identification from cargo_target.rs (`TAG_LINE` :17, `Layout` :26, `identify` :120, `identify_bounded` :129, `realpath_lossy` :189 (made `pub`), `Owner` :626, `TargetDir` :639); the liveness *result types* (liveness.rs:31-60), `embedded_hashes`, `unit_timestamp`; gc accounting: gc/ledger.rs, gc/account.rs, and `Scanned`, `Footprint`, `measure`, `record`, `scan`, `CRATE_DIR_CAP`, `DEPS_CAP` from gc/run.rs:20-456; plan_tier/locks.rs.

**Interfaces:**
- Produces: crate `cargo_hive_core` with modules `fingerprint`, `target`, `liveness` (types, `LivenessFn`,
  `embedded_hashes`, `unit_timestamp`), `ledger`, `caps`, `receipt`, `classify`, `size`, `locks`.
  `pub(crate)` items used across crates become `pub` (with `#[doc(hidden)]` where only hive uses them).
- Consumes: `cargo_hive_platform` (Task 15).

**Depends-on:** Task 15

- [ ] **Step 1: Failing test.** Add `("cargo-hive-core", 1)` to `LAYERS` and run `cargo test --test workspace`.
  Expect: it fails, because the package `cargo-hive-core` does not exist.
- [ ] **Step 2: Implement.** Create the crate (forbid attribute, `[lints] workspace = true`, deps:
  `cargo-hive-platform`, serde, serde_json) and move the files with `git mv`. Rewrite imports.
- [ ] **Step 3: Run.** The full gate, `cargo test -p cargo-hive-core`, `cargo test --test os_boundary`, the
  capture comparison and `cli_tree` pass.
- [ ] **Step 4: Commit.** `git commit -m "build: extract the core crate"`

_Requirements: SPLIT-1.1, SPLIT-1.2, SPLIT-4.3_

---

### Task 17: Extract the state crate

**Files:**
- Create: `crates/state/{Cargo.toml,src/lib.rs,README.md,LICENSE-MIT,LICENSE-APACHE}`
- Modify: move `crates/cargo-hive/src/state.rs` → `crates/state/src/lib.rs`; `Cargo.toml`; importers; `workspace.rs` (`("cargo-hive-state", 2)`)
- Test: moved unit tests; `crates/cargo-hive/tests/workspace.rs`

**Reuse:** rung 2 — src/state.rs moves whole, minus its `flock` loops and `utc_*` functions (now in platform).

**Interfaces:**
- Produces: crate `cargo_hive_state`: `State`, `Tx`, `StoreLock`, `Config`, `StateError`.
- Consumes: `cargo_hive_platform::{flock, clock}` only. `state.rs` has no `crate::` import, so its
  `Cargo.toml` depends on platform (plus serde, serde_json) and not on core. Its `LAYERS` row stays 2.

**Depends-on:** Task 16

- [ ] **Step 1: Failing test.** Add the `LAYERS` row and run `cargo test --test workspace`. Expect: the package is
  missing.
- [ ] **Step 2: Implement.** Create the crate and move the file.
- [ ] **Step 3: Run.** The full gate, `cargo test -p cargo-hive-state` and the capture comparison (including
  `state_written_by_0_1_0_reads_unchanged`) pass.
- [ ] **Step 4: Commit.** `git commit -m "build: extract the state crate"`

_Requirements: SPLIT-1.2, SPLIT-4.2_

---

### Task 18: Extract the cargo adapter crate

**Files:**
- Create: `crates/cargo/{Cargo.toml,src/lib.rs,README.md,LICENSE-MIT,LICENSE-APACHE}`
- Modify: move `git.rs`, `registry.rs`, `discover.rs`, `metadata.rs`, `toolchain.rs`, `ctx.rs`, `env.rs` from `crates/cargo-hive/src/` into `crates/cargo/src/`; `cargo_target.rs` is now empty and is deleted; `Cargo.toml`; importers; `workspace.rs` (`("cargo-hive-cargo", 3)`)
- Test: moved unit tests (env table test included)

**Reuse:** rung 2 — these move whole: git.rs, registry.rs, discover.rs; `resolve`, `caller_overrides`, `TARGET_VARS`, `METADATA_DEADLINE` from cargo_target.rs; the toolchain family from doctor.rs; `Ctx`, `checkout_walk`, `split_checkout_faults`, `faults_that_can_hide_an_owner` from plan_tier/mod.rs:18-68. New: the `env` module (rung 7).

**Interfaces:**
- Produces: crate `cargo_hive_cargo` (working name; the role word is the operator's): `discover`, `registry`,
  `git`, `metadata`, `toolchain`, `ctx`, `env`.
- Consumes: platform, core, state.

**Depends-on:** Task 17

- [ ] **Step 1: Failing test.** Add the `LAYERS` row and run `cargo test --test workspace`. Expect: the package is
  missing.
- [ ] **Step 2: Implement.** Create the crate and move the files.
- [ ] **Step 3: Run.** The full gate, `cargo test -p cargo-hive-cargo` (the env table passes) and the capture
  comparison pass.
- [ ] **Step 4: Commit.** `git commit -m "build: extract the cargo adapter crate"`

_Requirements: SPLIT-1.2, SPLIT-2.6_

---

### Task 19: Extract the doctor crate

**Files:**
- Create: `crates/doctor/{Cargo.toml,src/lib.rs,README.md,LICENSE-MIT,LICENSE-APACHE}`
- Modify: move `crates/cargo-hive/src/doctor.rs` → `crates/doctor/src/lib.rs`; `Cargo.toml`; importers; `workspace.rs` (`("cargo-hive-doctor", 4)`, `FEATURES` gains `"cargo-hive-doctor"`)
- Test: moved unit tests

**Reuse:** rung 2 — doctor.rs minus the toolchain family (now in the adapter) and the signal/kill code (now in platform).

**Interfaces:**
- Produces: crate `cargo_hive_doctor`: `run`, `Capabilities`, `Capability`, `require`, `DoctorEnv`,
  `toolchain_failure_exit`.
- Consumes: platform, core, state, adapter.

**Depends-on:** Task 18

- [ ] **Step 1: Failing test.** Add the rows and run `cargo test --test workspace`. Expect: the package is missing.
- [ ] **Step 2: Implement.** Create the crate and move the file.
- [ ] **Step 3: Run.** The full gate, `cargo test -p cargo-hive-doctor` and the capture comparison pass.
- [ ] **Step 4: Commit.** `git commit -m "build: extract the doctor crate"`

_Requirements: SPLIT-1.2, SPLIT-1.3_

---

### Task 20: Extract the seed crate

**Files:**
- Create: `crates/seed/{Cargo.toml,src/lib.rs,README.md,LICENSE-MIT,LICENSE-APACHE}`
- Modify: move `crates/cargo-hive/src/seed/` → `crates/seed/src/`; `Cargo.toml`; importers; `workspace.rs` (`("cargo-hive-seed", 4)`, `FEATURES` gains it)
- Test: moved unit tests; `cargo test -p cargo-hive-seed`

**Reuse:** rung 2 — src/seed/* moves whole; its edges are rewired downward: liveness result types, caps, ledger and locks from core; toolchain, `Ctx` and the walk fns from the adapter.

**Interfaces:**
- Produces: crate `cargo_hive_seed` with the same entry points.
- Consumes: platform, core, state, adapter. No gc, no doctor, including dev-dependencies.

**Depends-on:** Task 19

- [ ] **Step 1: Failing test.** Add the rows and run `cargo test --test workspace`. Expect: the package is missing.
- [ ] **Step 2: Implement.** Create the crate with no dependency on gc or doctor in any table, and move the files.
- [ ] **Step 3: Run.** `cargo tree -p cargo-hive-seed -e normal,dev --prefix none | grep -E '^cargo-hive-(gc|doctor) '`
  prints nothing (the dependency graph, not build output, proves seed reaches neither gc nor doctor, dev-deps
  included). The
  full gate, `tests/seed_liveness.rs` and the capture comparison pass.
- [ ] **Step 4: Commit.** `git commit -m "build: extract the seed crate"`

_Requirements: SPLIT-1.3, SPLIT-1.5_

---

### Task 21: Extract the gc crate

**Files:**
- Create: `crates/gc/{Cargo.toml,src/lib.rs,README.md,LICENSE-MIT,LICENSE-APACHE}`
- Modify: move `crates/cargo-hive/src/gc.rs`, `gc/`, `plan_tier/`, `explain.rs` into `crates/gc/src/` (`gc/liveness.rs` becomes `liveness.rs`); move `crates/cargo-hive/tests/liveness_report.rs` → `crates/gc/tests/liveness_report.rs`; `crates/cargo-hive/tests/guards.rs` (line 437: read `../../core/src/liveness.rs` and `../../gc/src/liveness.rs`); `Cargo.toml`; importers (cli.rs, bench/mod.rs pass `cargo_hive_gc::liveness`); `workspace.rs` (`("cargo-hive-gc", 4)`, `FEATURES` gains it)
- Test: moved unit tests; `cargo test -p cargo-hive-gc`

**Reuse:** rung 2 — these move whole: gc.rs, gc/{plan,run}.rs (minus the accounting moved to core); the liveness algorithm from liveness.rs (`liveness()` :210 and its private graph walk); plan_tier/{apply,build,guard,orphan,record}.rs; explain.rs; `cross_check_notes` (cli.rs:915).

**Interfaces:**
- Produces: crate `cargo_hive_gc`: `liveness`, `gc`, `plan_tier` (with `plan_tier::apply::cross_check_notes`,
  moved there by Task 9), `explain`.
- Consumes: platform, core, state, adapter.

**Depends-on:** Task 20

- [ ] **Step 1: Failing test.** Add the rows and run `cargo test --test workspace`. Expect: the package is missing.
- [ ] **Step 2: Implement.** Create the crate and move the files. `crates/cargo-hive/src/lib.rs` keeps only
  `pub mod cli; pub mod bench;`.
- [ ] **Step 3: Run.** The full gate, `cargo test -p cargo-hive-gc`, `cargo test --test workspace` (all layer rows,
  no feature-to-feature edge) and the capture comparison pass.
- [ ] **Step 4: Commit.** `git commit -m "build: extract the gc crate"`

_Requirements: SPLIT-1.1, SPLIT-1.2, SPLIT-1.3, SPLIT-1.5_

---

### Task 22: A gc edit rebuilds only gc and the binary; checkpoint after the extractions

**Files:**
- Create: `crates/cargo-hive/tests/rebuild_scope.rs`
- Modify: `.skills/_pending-workspace-split/auto/evidence.md` (local)

**Reuse:** rung 2 — `scripts/fresh-probe.sh` pattern (append a comment, `cargo build -v`, restore).

**Interfaces:**
- Produces: `#[test] #[ignore] fn a_gc_edit_leaves_seed_fresh()`. It copies the tracked tree with
  `git ls-files -z | tar --null -T - -cf - | tar -xf - -C /private/tmp/hive-split/rebuild` from the repo root,
  after `mkdir -p` (probed 2026-11-13 on macOS bsdtar: the copy equals `git ls-files`). It then runs `cargo build -v --offline` with `CARGO_TARGET_DIR=/private/tmp/hive-split/rebuild-target` in that copy. The
  shared `~/.cargo` registry is read, not written, as in any offline build. It
  appends a comment to `crates/gc/src/liveness.rs`, builds again, and asserts the output contains
  `Compiling cargo-hive-gc`, `Compiling cargo-hive v` and `Fresh cargo-hive-seed`. A second case runs
  `cargo test -p cargo-hive-gc --no-run -v` and asserts no `Compiling cargo-hive-seed` and no `Compiling
  cargo-hive v` (the binary).

**Depends-on:** Task 21

- [ ] **Step 1: Write the test.** Run `cargo test --test rebuild_scope -- --ignored`. Expect: it passes. Before
  Task 21 it would fail, because there is no `cargo-hive-gc` package.
- [ ] **Step 2: Checkpoint.** Following Task 3's checkpoint rule, run the 0.1.0 report and this branch's
  (`live-step4.json`) back to back on klynt, `cmp` them, run the capture comparison, and record both. A mismatch stops the plan.
- [ ] **Step 3: Commit.** `git commit -m "test: a gc edit rebuilds only gc and the binary"`

_Requirements: SPLIT-1.4, SPLIT-1.5, SPLIT-4.1_

---

### Task 23: Unit hashes are a typed value that prints as before

**Files:**
- Modify: `crates/core/src/fingerprint.rs` (lines 78-120, 162, 255, 293), readers in `crates/core/src/liveness.rs`, `crates/gc/src/{liveness,explain}.rs`, `crates/seed/src/{record,source}.rs`, `crates/gc/src/plan_tier/build.rs`, `crates/cargo-hive/tests/{seed,liveness,explain}.rs`
- Test: `crates/core/src/fingerprint.rs`

**Reuse:** rung 2 — fingerprint.rs (`unit_hash` :162, hash validation :255); rung 3 — `u64::from_str_radix`, `{:016x}`.

**Interfaces:**
- Produces: `fingerprint::UnitHash(u64)` with `UnitHash::parse(&str) -> Option<UnitHash>` (exactly 16 lowercase hex)
  and `Display` as `{:016x}`; `Unit.hash: UnitHash` serialized under the key `hash_hex` as the same string.

**Depends-on:** Task 22

- [ ] **Step 1: Failing test.**
  - `UnitHash::parse("00000000000000ab").unwrap().to_string() == "00000000000000ab"`;
  - `parse` rejects `"00000000000000AB"`, a 15-char and a 17-char input;
  - `serde_json::to_value(&unit)["hash_hex"]` equals the 16-hex string of a unit read from a fixture profile.

  Run `cargo test -p cargo-hive-core fingerprint::`. Expect: a compile error.
- [ ] **Step 2: Implement.** Replace `hash_hex: String`, and the `HashMap<&str, …>` keys built from it, with
  `UnitHash`.
- [ ] **Step 3: Run.** The full gate and the capture comparison pass.
- [ ] **Step 4: Commit.** `git commit -m "refactor(core): type unit hashes"`

_Requirements: SPLIT-4.4, SPLIT-4.6_

---

### Task 24: Unit kinds are an enum that keeps `other`

**Files:**
- Modify: `crates/core/src/fingerprint.rs` (`KINDS` :31, `kind_of` :146-152, `Unit.kind` :81), `crates/gc/src/liveness.rs` (`NEVER_COLLECTED` :21, :502), `crates/core/src/liveness.rs` (`unit_timestamp`), and every other reader found by `grep -rn 'kind_of\|\.kind\b' crates --include='*.rs'`
- Test: `crates/core/src/fingerprint.rs`

**Reuse:** rung 2 — `KINDS` and `kind_of` (fingerprint.rs:31, :146-152).

**Interfaces:**
- Produces: `fingerprint::UnitKind { RunBuildScript, BuildScript, DocLib, DocBin, Test, Lib, Bin, Example, Bench,
  Other }` with `as_str()` returning `"run-build-script"`, `"build-script"`, `"doc-lib"`, `"doc-bin"`, `"test"`,
  `"lib"`, `"bin"`, `"example"`, `"bench"`, `"other"`, and `UnitKind::of(stem: &str) -> UnitKind`, which matches
  `kind_of`: exact name or `name-` prefix, otherwise `Other`. Serialized as `as_str()`.

**Depends-on:** Task 23

- [ ] **Step 1: Failing test.** A table test covers each of the nine names, `"build-script-build"` →
  `BuildScript`, `"weird"` → `Other` printing `"other"`, and `"lib-foo"` → `Lib`. Run
  `cargo test -p cargo-hive-core fingerprint::`. Expect: a compile error.
- [ ] **Step 2: Implement.** Replace `kind: String` with `UnitKind`, and `NEVER_COLLECTED` with a `matches!` on
  the enum.
- [ ] **Step 3: Run.** The full gate and the capture comparison pass.
- [ ] **Step 4: Commit.** `git commit -m "refactor(core): unit kinds as an enum"`

_Requirements: SPLIT-4.4, SPLIT-4.5_

---

### Task 25: The fingerprint graph is CSR and liveness marks with a bitset; checkpoint after the rework

**Files:**
- Modify: `crates/core/src/fingerprint.rs` (`UnitGraph` :116-120, edge build and index :520-540, `EdgeCounts` :109-114), `crates/gc/src/liveness.rs` (`mark_live` :396 and its `live`/`node_live` vectors; `index_units` :313 keyed by `UnitHash`)
- Test: `crates/core/src/fingerprint.rs`, `crates/gc/src/liveness.rs`

**Reuse:** rung 2 — the existing edge build (fingerprint.rs:520-540) and `mark_live`; rung 7 for the CSR and
bitset types (design Decision 9).

**Interfaces:**
- Produces:
  - `fingerprint::UnitId(u32)`, an index into `UnitGraph.units` within one profile;
  - `UnitGraph { units: Vec<Unit>, offsets: Vec<u32>, targets: Vec<UnitId> }`, built from exactly the resolved
    edges the 0.1.0 loop pushes, in the same order, with `successors(id) -> &[UnitId]` and `edge_count()`;
  - the per-profile index `HashMap<UnitHash, Vec<UnitId>>` in place of `HashMap<&str, Vec<usize>>`;
  - `core::bitset::BitSet` (`with_len`, `insert -> bool`, `contains`) in place of `mark_live`'s `Vec<bool>`.

  `EdgeCounts` and `ProfileReport`'s serialized counts keep their values. `mark_live` keeps its lazy,
  target-wide resolution through `index_units`/`learn_packages`, so an `Uncertain` reason is still recorded only
  for an edge the walk reaches. A precomputed target-wide CSR would resolve edges of unreached units and add
  reasons, so it is not used.

**Depends-on:** Task 24

- [ ] **Step 1: Failing test.**
  - in `crates/gc/src/liveness.rs`'s test module, where the liveness fixtures live by now (`target_of`,
    moved by Task 11 from liveness.rs:744): for each fixture profile, `successors` of each unit equals the 0.1.0
    `edges` filtered by source, and `edge_count` equals `edges.len()`. Capture the 0.1.0 lists by keeping the old
    builder as a `#[doc(hidden)]` function in core until Step 3.
  - BitSet: table cases at bit 0, 63, 64 and `len - 1`.

  Run `cargo test -p cargo-hive-gc liveness::` and `cargo test -p cargo-hive-core bitset::`. Expect: a compile
  error.
- [ ] **Step 2: Implement.** Build the CSR, switch the index keys, and swap `mark_live`'s vectors for `BitSet`.
  Delete the old builder.
- [ ] **Step 3: Checkpoint.** The full gate (all liveness suites, tests/liveness_prove.rs) and the capture
  comparison pass. Following Task 3's checkpoint rule, run the 0.1.0 report and this branch's (`live-step5.json`)
  back to back on klynt, `cmp` them and record the result. A mismatch stops the plan and goes to the operator.
- [ ] **Step 4: Commit.** `git commit -m "refactor: CSR fingerprint graph and bitset liveness marks"`

_Requirements: SPLIT-4.4, SPLIT-4.1_

---

### Rename path (applies only if the operator changes a crate name at the sheet)

If the sheet changes any working name, one mechanical commit lands after Task 25 and before Task 26. It renames:
- the package name in `crates/<role>/Cargo.toml`;
- the matching `[workspace.dependencies]` key and every `cargo-hive-<role>` dependency line;
- every `cargo_hive_<role>` `use` path in `crates/` and `tests/`;
- the `LAYERS`/`FEATURES` rows in `crates/cargo-hive/tests/workspace.rs`;
- the crate READMEs, and the member list Task 26 writes.

The directory names (`crates/<role>/`) do not change. The commit passes the full gate and the capture comparison
and changes nothing else (`git commit -m "build: rename workspace crates to the approved names"`). It cites no
requirement of its own; it keeps SPLIT-1.1's names in step with the operator's decision.

### Task 26: Release steps publish every member

**Files:**
- Modify: `docs/agents/project.md` (release steps, lines 130-137 and the closing `include` paragraph), `scripts/fresh-probe.sh` (usage text), `README.md` (line 113), `crates/cargo-hive/tests/liveness.rs` (manifest test asserts every member), each `crates/*/Cargo.toml` `include`
- Test: `crates/cargo-hive/tests/liveness.rs` manifest test; manual release dry-run

**Reuse:** rung 2 — the existing release steps (docs/agents/project.md:130-137), scripts/fresh-probe.sh, tests/skill.rs, tests/skill/names.rs, tests/liveness.rs:279-293.

**Interfaces:**
- Produces: release steps 1–6:
  - 2: set `[workspace.package] version` and the three plugin files;
  - 3: `cargo package --workspace --list`, then `CARGO_TARGET_DIR=target/package-verify cargo publish
    --workspace --dry-run`, then `scripts/fresh-probe.sh` with one pair per member;
  - 5: stop unless the line `Crate names approved:` in docs/agents/project.md holds a date; then the same
    publish without `--dry-run`, and on a partial failure stop and report which members uploaded. Steps 1–4,
    including the dry-run, work on working names.
  - 6: smoke as today.

  The line `Crate names approved:` (no date) is added under `## release steps` in docs/agents/project.md, just
  above step 5.

**Depends-on:** Task 25

- [ ] **Step 1: Failing test.** Change the manifest test to iterate every member's `Cargo.toml` and assert its
  `include` holds `/src/**/*`, `/Cargo.toml`, `/README.md` and both license files, and no `tests` entry. Run
  `cargo test --test liveness the_package_ships_no_test_file`. Expect: it fails for a member missing an entry, if
  any.
- [ ] **Step 2: Implement.** Write the steps, the README path `cargo install --path crates/cargo-hive`, and the
  includes.
- [ ] **Step 3: Dry-run (manual).** Run `cargo package --workspace --list` and expect every member to list only
  `src/` files, `Cargo.toml`, `Cargo.toml.orig`, `.cargo_vcs_info.json`, `Cargo.lock`, `README.md` and the two
  license files. Run `CARGO_TARGET_DIR=target/package-verify
  cargo publish --workspace --dry-run` and expect "aborting upload due to dry run" for each member, in dependency
  order. `scripts/fresh-probe.sh` with every member pair prints `GREEN` for each. Run `cargo install --path
  crates/cargo-hive --root /private/tmp/hive-split/inst`, then `/private/tmp/hive-split/inst/bin/cargo-hive
  --version` and `HIVE_V010_BIN=… cargo test --test capture -- --ignored` against that binary. Expect: pass.
- [ ] **Step 4: Commit.** `git commit -m "docs(release): publish every workspace member"`

_Requirements: SPLIT-6.1, SPLIT-6.2, SPLIT-6.3, SPLIT-6.4, SPLIT-3.9_

---

### Task 27: Documented commands and standards match the workspace

**Files:**
- Modify: `docs/agents/project.md` (verify table :79-84, single-test :90-92, task gate :94-95, run locally :115-124), `docs/agents/verify.md` (lines 16-24), `docs/standards/code.md` (line 15), `docs/standards/testing.md` (lines 10-25)
- Test: manual run of each documented command

**Reuse:** rung 2 — the existing docs.

**Interfaces:**
- Produces:
  - verify table with `--workspace`;
  - single-test lines `cargo test -p <member> --test <file>` and `cargo test -p <member> --lib <module>::`;
  - task gate "tests of the changed member and the members above it, then compile every other member";
  - code.md `unsafe` row: "only in the platform crate, at FFI sites, each with a `// SAFETY:` comment; no other
    crate uses `libc`, non-test `cfg(target_os)` or `std::process::Command`";
  - testing.md: integration tests live in `crates/cargo-hive/tests/` and share `tests/common`; unit tests live
    beside their code in each member; document tests read files from the repo root.

**Depends-on:** Task 26

- [ ] **Step 1: Write the doc changes.**
- [ ] **Step 2: Run each documented command as written** at the root on the green tree: the verify table, a
  single-test example per member, the task gate example, `verify.md`'s cold start (`cargo build`, then
  `target/debug/cargo-hive --version` printing `cargo-hive 0.1.0`). Expect: each passes. Record any that fails and
  fix the doc, not the command's meaning.
- [ ] **Step 3: Commit.** `git commit -m "docs: commands and standards for the workspace"`

_Requirements: SPLIT-7.1, SPLIT-7.2_

---

### Task 28: Build time and status time are measured

**Files:**
- Modify: `.skills/_pending-workspace-split/auto/evidence.md` (local, dated evidence per code.md "Measured claims")

**Reuse:** rung 2 — tests/guards.rs `#[ignore]` timing tests (:329, :375) move unchanged with the binary package.

**Interfaces:**
- Consumes: Task 3's baseline median.
- Produces: evidence lines `build-after-gc-edit median: <ms> (0.1.0: <ms>)`, `guards --ignored: pass|fail`.

**Depends-on:** Task 27

- [ ] **Step 1: Measure.** Append one comment line to `crates/gc/src/gc/run.rs`, run `cargo build --offline` (the
  same flags as Task 3) and time it. Do
  this 5 times, restoring the line each time, and record the median beside Task 3's 0.1.0 median. Re-run the 0.1.0 median in `/private/tmp/hive-0.1.0`
  back to back as a second reading. If the new
  median is greater, write both medians as an open question for the operator. Do not change the target.
- [ ] **Step 2: Status timing.** Run `cargo test -p cargo-hive --test guards -- --ignored` on the proof machine.
  Expect: both klynt timing tests pass (CORE-6.1: median of 5 warm runs and one cold run within 2 s; explain
  within 2 s of status).
- [ ] **Step 3: Record.** Write both results, with the date, in the evidence file.

_Requirements: SPLIT-8.1, SPLIT-8.2_
