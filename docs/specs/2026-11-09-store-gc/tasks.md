# Tasks: Store GC (bare `cargo hive gc` keeps hive's store bounded)

> **For agentic workers:** after plan approval, pick one execute skill:
> - `build-in-waves`: a continuous, dependency-aware scheduler;
> - `build-by-story`: human-gated story review units;
> - `build-inline`: the controller implements, with no implementer subagents.
>
> The chosen skill writes `Execution-mode:`. Steps use checkbox (`- [ ]`) syntax for tracking.

Feature code: STOREGC
Status: Approved — operator "approve", 2026-11-09
Date: 2026-11-09
Execution-mode: unset
Max-concurrency: auto
Requirements: ./requirements.md
Design: ./design.md

**Goal:** Make bare `cargo hive gc` evict store entries that no target holds after 30 days, drop toolchain generations no checkout uses, and keep the store at or under 40 GB of freeable bytes. It never evicts a held key, never runs while a `seed` uses the store, and reports every count in its receipt.

**Architecture:**
- **`src/seed/evict.rs`** (new) plans with no lock, then commits and removes under an exclusive try of the new store lock. Its one caller is `cli::gc_cmd`.
- **`src/seed/{store,source}.rs`** gain the store inventory and commit, and the held-key rules. **`src/state.rs`** gains the store lock. **`src/doctor.rs`** gains the own-directory toolchain lookup.
- **`seed::run`** holds the store lock shared from before fill until after `place`.

**Tech Stack:** Rust 2021, rustc/cargo 1.97.1 on macOS 27 (APFS). Existing dependencies only: `serde`, `serde_json`, `clap`, `libc`, `applesauce`. Integration tests run the real binary (`env!("CARGO_BIN_EXE_cargo-hive")`) on scratch fixtures under `/private/tmp/hive-it-*`; the cap seam is a library call (`cargo_hive::seed::evict::run`).

## Global Constraints

The canonical sources (`docs/standards/` is the SSOT). Read them; do not restate them. Hashes were taken 2026-11-09; a mismatch at execute time means the source changed after planning.

| Source | sha256 (first 16) |
|---|---|
| `docs/standards/INDEX.md` | `80cd41328c0f7652` |
| `docs/standards/code.md` (bounds on every walk, read and child process; no new deps; no requirement IDs in code, comments or test names; `//!` module comments) | `afdcc8b89136f573` |
| `docs/standards/errors-logging.md` (typed errors with hand-written `Display`; expected refusals are data; exit codes) | `48f5baac3b52a552` |
| `docs/standards/testing.md` (merge gate; scratch-only fixtures; test-only environment variables; no test-only branches in production paths) | `6659c2301761caaf` |
| `docs/architecture/INDEX.md`: this feature relies on ARCH-2, 4, 8, 9, 10, 11, 13, 14, 15, 16, 17, 18; ARCH-19's try-without-waiting shape is borrowed for the store lock | `36f62957f3f4ca1b` |
| `docs/specs/2026-11-09-store-gc/design.md` (Decisions 1 to 18) | see git |
| `.skills/_pending-store-gc/auto/` (close package, ledger, scan digests, spike-k4, measure-u1, reviews) | not in git |

Merge gate (testing standard). Every task must leave all four commands green:
- `cargo fmt --check`, after `cargo fmt`.
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- `cargo check --target x86_64-unknown-linux-gnu`

On a machine without that target installed, record "non-macOS compile check not run: target not installed", as `docs/agents/project.md` says.

Commits: each commit step is `cargo fmt`, then `git add -A && git commit -m "<subject>"`, on branch `feat/store-gc`. No `Co-Authored-By` or generated-by line.

House rules for this plan:
1. Store eviction deletes only under `<state root>/store/` and writes only `store.db` and `<state root>/store.lock` (STOREGC-9.1). Nothing in a Cargo target dir or a checkout is created, modified or deleted by it.
2. `store.db`, `seeds.json`, `plans.json` and `config.json` keep every field and key they have. No new field, key, plan class, flag or environment variable. This is the run's persisted-format hard stop: a task that seems to need one stops and reports.
3. `store.db` is written only through `state` under the state lock. The state lock is never held across a clone, a compression or a tree removal (SEED-9.4). The store lock is always taken before the state lock.
4. `gc <repo>`, `gc --plan`, `apply`, `status`, `explain`, `doctor`, `repo` and `seed` must not change, apart from the note of STOREGC-5.3 and seed's wait on the store lock. Their existing tests are the guard.
5. No requirement ID appears in source, comments or test names.
10. `tests/store_gc.rs` starts with `#![cfg(target_os = "macos")]`, as the other APFS suites do.
11. Unit tests never call `std::env::set_var`. Environment-dependent logic takes its input as a parameter (`caller_toolchain_kept_from`), and child processes are faked by a script passed as a path (the pattern at `src/doctor.rs:1656`).
6. No test-only branch in a production path. The cap is injected through `Limits` (Weigh W10). Time is injected by editing `last_live_at` in a scratch `store.db`. Failures are injected through the `Fs` wrapper `Recording`.
7. Code names the key's parts `toolchain_value`, `name_meta` and `fingerprint_hash`, never `rustc` or `fingerprint` alone.
8. Tests never touch the real `~/.cargo`, `~/.hive` or `~/.cache`. `Hive` gives a scratch `HIVE_HOME` and `CARGO_HOME`, and every test that classifies checkouts owns its `secondary_roots`.
9. `hive.json(&[...])` appends `--json` itself; passing it again exits 2.

## File Structure

| File | Responsibility in this plan |
|---|---|
| `src/state.rs` | Store lock: `StoreLock`, `State::store_shared_or_cancelled`, `State::try_store_exclusive`, on `<state root>/store.lock` |
| `src/doctor.rs` | `caller_toolchain_kept_from` (pure) and `caller_toolchain_kept`, extracted from `drop_directory_toolchain`; `resolve_toolchain_own_dir_until` (lookup (a)) |
| `src/seed/store.rs` | `StoreKey`; `Inventory`, `inventory`, `evict_dir`, `rename_out`, `CommitPlan`, `Committed`, `commit_eviction` |
| `src/seed/source.rs` | `unit_key(&Unit) -> StoreKey`, extracted from `live_registry_units`; `holds` (held keys, name-metas, whole-store causes, findings) |
| `src/seed/evict.rs` (new) | `run`, `EvictCtx`, `Limits`, `EvictOutcome`: plan, rules, cap order, commit, removal, counts |
| `src/seed/mod.rs` | `#[doc(hidden)] pub mod evict`; `seed::run` takes the store lock shared before `fill::fill` and drops it after `place` |
| `src/cli.rs` | `live_checkouts` returns `(Vec<Checkout>, Vec<WalkFault>)`; `gc_cmd` keeps the whole `Toolchain` from line 849 and calls `evict::run` with `CAP_BYTES` and `MAX_AGE_SECS`; exit 130 on an evict cancel; the `gc <repo>` note; the `Gc` help text |
| `tests/common/store_fixture.rs` (new) | Test helpers: seed a store from `RegistryRepo`; read and edit `store.db` rows; synthesize entries and rows by hand; list entry dirs |
| `tests/common/mod.rs` | `mod store_fixture` declaration only |
| `tests/store_gc.rs` (new) | Integration tests (binary and library) for stories 1 to 7 and 9 |
| `tests/seed.rs` | One test: seed waits for an exclusive store lock |
| `tests/gc_disk_image.rs` | One disk-image test for store freed bytes |
| `plugin/skills/hive/SKILL.md`, `README.md`, `tests/skill.rs` | Store text, a bare `gc --json` command line, the replaced README sentences |
| `docs/specs/2026-11-02-store-seed/design.md`, `docs/specs/2026-11-02-store-seed/requirements.md`, `docs/specs/2026-11-01-gc-plan-apply/requirements.md` | Dated notes: SEED decision 6, SEED-10.1, GCPLAN-9.1 |

Retrieval (load-subgraph, schema 1.1, fsubr-1.2, 2026-11-09): `blast_radius` over these paths names SEED (`src/seed/*`, `src/state.rs`, `tests/seed.rs`), CORE, GCPLAN (`src/state.rs`, `src/cli.rs`, the skill), AUTOGC (`src/cli.rs`, `src/doctor.rs`, `tests/gc_disk_image.rs`), EXPLAIN (`src/cli.rs`, `src/doctor.rs`) and SKILL (the skill, `tests/skill.rs`, README). `cluster(STOREGC)` returns STOREGC alone: its OWNS is empty until this file lands. OWNS coverage is 7 of 8; observations none.

Order. `src/seed/evict.rs` and `tests/store_gc.rs` are edited by Tasks 4 to 11, so those tasks form one chain; Tasks 1 and 2 share no file and may run together.

---

### Task 1: Prefactor the three seams eviction reads, behavior unchanged

**Files:**
- Modify: `src/cli.rs` (`live_checkouts`, lines 485-503, and its callers at lines 862, 1141 and 1301)
- Modify: `src/seed/source.rs` (the unit-to-key mapping inside `live_registry_units`, lines 122-171)
- Modify: `src/seed/store.rs` (add `StoreKey`)
- Modify: `src/doctor.rs` (`drop_directory_toolchain`, lines 355-367)
- Test: `src/doctor.rs`, `src/seed/source.rs` (unit tests)

**Reuse:** rung 2 — `gc_cmd` (`src/cli.rs:816-977`), `live_checkouts` (`src/cli.rs:485`), `state::window_days` (`src/state.rs:216`)

**Interfaces:**
- Consumes: `discover::{Checkout, WalkFault}`, `fingerprint::{ProfileReport, Unit}`
- Produces:
  - `fn live_checkouts(...) -> (Vec<Checkout>, Vec<WalkFault>)`; each caller makes from the faults the findings it made before.
  - `pub struct StoreKey { pub toolchain_value: u64, pub name_meta: String, pub fingerprint_hash: String }` in `seed::store`, with `Ord`.
  - `pub(crate) fn unit_key(unit: &Unit) -> StoreKey` in `seed::source`.
  - `pub(crate) fn caller_toolchain_kept_from(source: Option<&std::ffi::OsStr>) -> bool` and `pub(crate) fn caller_toolchain_kept() -> bool` in `doctor`.

**Depends-on:** none

- [ ] **Step 1: Write the failing tests.**
  - In `src/doctor.rs`, a table for `caller_toolchain_kept_from`: `cli`, `env` and `default` give true; `toolchain-file`, `path-override`, another value and `None` give false.
  - In `src/seed/source.rs`, `unit_key` on a fixture `Unit` returns its `toolchain_value`, directory name and `le_hex` fingerprint.

  Run `cargo test --lib`; expect a compile failure.
- [ ] **Step 2: Implement.**
  - `drop_directory_toolchain` calls `caller_toolchain_kept`.
  - `live_registry_units` calls `unit_key`.
  - `live_checkouts` returns its faults, and `gc_cmd`, `plan_cmd` and `explain_cmd` each turn them into the findings they made before.
- [ ] **Step 3: Run.** `cargo test`; expect pass, with tests/gc.rs, tests/plan_tier.rs, tests/plan_apply.rs, tests/explain.rs, tests/seed_fill.rs and tests/capability_gate.rs unchanged.
- [ ] **Step 4: Commit.** `refactor: expose walk faults, the unit store key and the kept-toolchain predicate`

_Requirements: STOREGC-10.2, STOREGC-10.7, STOREGC-10.10, STOREGC-10.12_

---

### Task 2: The store lock, and seed holding it shared

**Files:**
- Modify: `src/state.rs`
- Modify: `src/seed/mod.rs` (`seed::run`: guard before `fill::fill` at line 194, dropped after `place`)
- Test: `src/state.rs` (unit tests), `tests/seed.rs`

**Reuse:** rung 2 — the polling `flock` loop of `State::locked_or_cancelled` (`src/state.rs:151-178`), extended with a shared mode and a single exclusive try

**Interfaces:**
- Consumes: none
- Produces:
  - `pub struct StoreLock`, which releases on drop.
  - `pub fn store_shared_or_cancelled(&self, cancel: impl FnMut() -> bool) -> Result<StoreLock, StateError>`.
  - `pub fn try_store_exclusive(&self) -> Result<Option<StoreLock>, StateError>`.
  - Both create `<state root>/store.lock` if absent.

**Depends-on:** none

- [ ] **Step 1: Write the failing tests.**
  - In `src/state.rs`, with two `State` values on one scratch root, since flock is per open file description:
    - `two_shared_store_locks_coexist`.
    - `an_exclusive_try_returns_none_while_a_shared_lock_is_held`.
    - `a_shared_lock_waits_for_an_exclusive_one_and_honors_cancel`: the closure returns true on its third call, and the result is `StateError::Cancelled`.
    - `the_lock_file_holds_no_bytes`.
  - In `tests/seed.rs`, `a_seed_waits_for_an_exclusive_store_lock_and_then_seeds`. The test holds `try_store_exclusive` on the fixture's `HIVE_HOME` and starts `cargo hive seed`. After 1 s it asserts that the seed is still running and that `store.db` and `store/` are unchanged, which shows the seed is waiting before fill. It then releases, and the seed exits 0 with clones.

  Run `cargo test --lib state::` and `cargo test --test seed a_seed_waits`; expect a compile failure, then a failure.
- [ ] **Step 2: Implement** the two methods on the `locked_or_cancelled` pattern: `LOCK_SH | LOCK_NB` polled every `LOCK_POLL`, and `LOCK_EX | LOCK_NB` tried once. `seed::run` takes the shared lock with the seed's cancel closure before `fill::fill` and holds it until `place` returns. A cancel while waiting takes the existing SEED-7.5 path.
- [ ] **Step 3: Run.** `cargo test`; expect pass, with tests/seed.rs, tests/seed_fill.rs and tests/status_seed.rs otherwise unchanged.
- [ ] **Step 4: Commit.** `feat(state): add a store lock that seed holds shared while it fills and clones`

_Requirements: STOREGC-10.3_

---

### Task 3: Store primitives: inventory, rename out, and one-Tx commit

**Files:**
- Modify: `src/seed/store.rs`
- Test: `src/seed/store.rs` (unit tests)

**Reuse:** rung 2 — `store::read`, `merge`'s pruning, `entry_dir`, `store_dir`, `tmp_dir`, `Tx::write_bounded` (`src/seed/store.rs:43-236`)

**Interfaces:**
- Consumes: `StoreKey` (Task 1)
- Produces:
  - `pub fn inventory(fs: &dyn Fs, state: &State) -> Result<Inventory, StateError>`, with `Inventory { rows: Vec<Row>, rowless: Vec<(StoreKey, PathBuf)>, tmp_bytes: u64 }`. It lists only `store/<u64>/<name-meta>-<hash>/`, with `read_dir_bounded(CRATE_DIR_CAP)`, and skips `.tmp`, `CACHEDIR.TAG` and any other name.
  - `pub fn evict_dir(state: &State, pid: u32, seq: usize) -> PathBuf`, which returns `store/.tmp/evict-<pid>-<seq>`. `tmp_dir` stays fill's.
  - `pub fn rename_out(state: &State, key: &StoreKey, to: &Path) -> io::Result<()>`.
  - `pub struct CommitPlan { pub candidates: Vec<(StoreKey, Option<u64>)>, pub refreshed: Vec<StoreKey>, pub now: u64 }`. The `Option<u64>` is the row's `last_live_at` at plan time, or `None` for a rowless directory.
  - `pub struct Committed { pub renamed: Vec<(StoreKey, PathBuf)>, pub kept_changed: usize, pub dropped_values: Vec<u64> }`.
  - `pub fn commit_eviction(state: &State, plan: &CommitPlan, rename: impl FnMut(&StoreKey) -> io::Result<Option<PathBuf>>) -> Result<Committed, StateError>`. It takes `State::locked` and, in one `Tx`:
    1. reads rows;
    2. drops candidates whose `last_live_at` rose or that gained a row;
    3. calls `rename` for each survivor, where `Ok(None)` means "stop", used for cancel;
    4. drops renamed rows and raises refreshed rows' `last_live_at`;
    5. drops the toolchain texts of every value with no row and no entry directory left;
    6. writes.

**Depends-on:** Task 1

- [ ] **Step 1: Write the failing tests** in `src/seed/store.rs`, on scratch state roots with hand-written `store.db` and entry directories:
  - `inventory_lists_rows_rowless_dirs_and_tmp_bytes_and_skips_other_names`.
  - `commit_drops_a_renamed_row_and_raises_a_refreshed_one`.
  - `commit_keeps_a_candidate_whose_last_live_at_rose_since_the_plan`.
  - `commit_keeps_a_rowless_candidate_that_gained_a_row`.
  - `commit_drops_a_value_text_only_when_no_row_and_no_dir_of_it_remain`.
  - `commit_stops_renaming_when_rename_returns_none_and_still_writes`.
  - `a_failed_rename_keeps_that_entry_and_its_row`.

  Run `cargo test --lib seed::store::`; expect a compile failure.
- [ ] **Step 2: Implement** the interfaces above. `rename_out` is `std::fs::rename(entry_dir, to)` after `create_dir_all(store/.tmp)`.
- [ ] **Step 3: Run.** `cargo test`; expect pass, with tests/seed.rs and tests/seed_fill.rs unchanged.
- [ ] **Step 4: Commit.** `feat(seed): list, rename out and commit store entries for eviction`

_Requirements: STOREGC-1.5, STOREGC-2.3, STOREGC-10.4, STOREGC-10.5_

---

### Task 4: Bare gc evicts entries nothing has held for 30 days

**Files:**
- Create: `src/seed/evict.rs`
- Create: `tests/common/store_fixture.rs`
- Create: `tests/store_gc.rs`
- Modify: `src/seed/source.rs` (`holds`, Decided targets only), `src/seed/mod.rs` (`#[doc(hidden)] pub mod evict`), `src/cli.rs` (`gc_cmd`), `tests/common/mod.rs`

**Reuse:** none — new code (rung 7), because no module orders evictions or phases a store commit. Inside it: rung 2 for `gc::measure` (per-entry freeable), `gc::account::remove_counted` with `Ledger` (removal and freed bytes), `split_checkout_faults` and `faults_that_can_hide_an_owner` (`src/plan_tier/mod.rs:50-64`), and `Receipt` builders.

**Interfaces:**
- Consumes:
  - `live_checkouts` faults and `unit_key` (Task 1);
  - `State::try_store_exclusive` (Task 2);
  - `inventory`, `evict_dir`, `rename_out`, `commit_eviction` (Task 3);
  - `liveness::liveness`, `gc::account::remove_counted`.
- Produces:
  - `pub(crate) fn holds(fs: &dyn Fs, targets: &[TargetDir], now: SystemTime, window: Duration) -> Holds`, with `Holds { keys: BTreeSet<StoreKey>, name_metas: BTreeSet<String>, by_target: BTreeMap<PathBuf, TargetHolds>, whole_store: Vec<Finding>, findings: Vec<Finding>, by_reason: BTreeMap<&'static str, u64> }`, with `TargetHolds { keys: BTreeSet<StoreKey>, name_metas: BTreeSet<String> }`, recording which target held each key or name-meta, in memory only. Task 5 fills `name_metas`. In this task an unreadable target or any non-Decided verdict goes to `whole_store`; Task 5 narrows that.
  - `pub fn run(ctx: &EvictCtx<'_>, limits: Limits, dry_run: bool, cancel: &dyn Fn() -> bool) -> EvictOutcome` in `seed::evict`.
  - `pub struct EvictCtx<'a> { pub state: &'a State, pub fs: &'a dyn Fs, pub targets: &'a [TargetDir], pub checkouts: &'a [Checkout], pub checkout_faults: &'a [WalkFault], pub scan_faults: &'a [WalkFault], pub cache_roots: &'a [PathBuf], pub window: Duration, pub account_bytes: bool, pub now: SystemTime, pub caller_toolchain: Option<Toolchain>, pub caller_toolchain_kept: bool, pub rustup: &'a Path }`. `gc_cmd` fills `caller_toolchain_kept` from `doctor::caller_toolchain_kept()`, so `evict::run` reads no environment (house rule 11). `gc_cmd` passes `Path::new("rustup")`; tests pass a fake script, as `resolve_toolchain_with` already takes a rustup path (`src/doctor.rs:439,447`).
  - `pub struct Limits { pub cap_bytes: u64, pub max_age_secs: u64 }`.
  - `pub struct EvictOutcome { pub counts: Vec<(String, u64)>, pub findings: Vec<Finding>, pub notes: Vec<String>, pub unmeasured: Vec<(String, String)>, pub cancelled: bool }`.
  - In `cli.rs`: `const CAP_BYTES: u64 = 40_000_000_000;` and `const MAX_AGE_SECS: u64 = 30 * 86_400;`.
  - In `tests/common/store_fixture.rs` (every `store.db` write goes through `cargo_hive::state::State::open_at(hive.home.path.clone())` and `State::locked` with `Tx::write_bounded`, so a fixture write never races a seed's `merge`):
    - `seeded_store(hive: &Hive) -> RegistryRepo`
    - `rows(hive: &Hive) -> Vec<serde_json::Value>`
    - `set_last_live_at(hive: &Hive, name_meta_prefix: &str, secs_ago: u64)`
    - `synth_entry(hive: &Hive, key: (u64, &str, &str), registry_deps: &[&str], files: &[(&str, Vec<u8>)], last_live_secs_ago: u64)`
    - `entry_dirs(hive: &Hive) -> Vec<PathBuf>`

**Depends-on:** Task 2, Task 3

- [ ] **Step 1: Write the failing tests** in `tests/store_gc.rs`, on a store that `seed` filled from a `RegistryRepo`:
  - `an_entry_no_target_holds_for_31_days_is_evicted_with_its_row`. The repo's target is removed. The entry dir and row are gone, and `store_entries_evicted_age` is the number evicted.
  - `an_entry_held_by_a_live_target_stays_and_its_last_live_at_is_refreshed`. The row is backdated 31 days and the target was built inside the window. Afterwards the row's `last_live_at` is at least the test's start time.
  - `an_entry_29_days_old_stays`.
  - `a_rowless_entry_dir_no_target_holds_is_evicted`.
  - `a_rowless_entry_dir_a_target_holds_is_left_for_fill`.
  - `tmp_is_left_alone_and_its_bytes_are_counted`: `store_tmp_bytes` > 0 and `.tmp` is unchanged.
  - `gc_window_days_out_of_range_evicts_nothing_and_still_deletes_sessions`: `store.window_invalid` is reported, and AUTOGC's session deletion still happens.
  - `a_live_unit_in_an_orphan_candidate_target_holds_its_key`.
  - `store_db_keeps_its_fields_after_eviction`: every remaining row has exactly the keys it had.
  - `eviction_touches_nothing_outside_the_store`: a snapshot of the repo, its targets, a seeded checkout, `CARGO_HOME`, `seeds.json`, `plans.json` and `config.json` is unchanged.

  Run `cargo test --test store_gc`; expect a compile failure.
- [ ] **Step 2: Implement** `holds` for Decided targets: live refs map through `unit_key` to keys, and a live ref with no graph unit maps to its name-meta. Every walk fault `split_checkout_faults` or `faults_that_can_hide_an_owner` keeps also goes to `whole_store`, so no intermediate commit evicts past a fault. Then implement `evict::run` for the age rule (design decisions 3, 4, 11). When `try_store_exclusive` returns `None`, evict nothing, rename nothing and write nothing (Task 9 adds the finding, the count and the tests):
  - Plan: holds, `inventory`, candidates.
  - Commit: `try_store_exclusive`, remove stale `store/.tmp/evict-*`, `commit_eviction` with `rename_out` into `evict_dir(std::process::id(), seq)`, then remove emptied `store/<value>/`.
  - Removal: `remove_counted` on each renamed tree, still under the store lock, then release.
- [ ] **Step 3: Wire it in `gc_cmd`.**
  - Keep the whole `Toolchain` from line 849 (`b.toolchain` still takes `.release`) as `caller_toolchain`.
  - Call `evict::run` after the cross-check notes (lines 903-912) and before `put_counts` (line 916). Call it only when `args.is_empty()`, `--plan` is absent and `report.cancelled` is false.
  - Before that call, check `window_days(cfg.gc_window_days)`; failing it is STOREGC-1.7.
  - Add the outcome's counts, findings, notes and `unmeasured` to the receipt.
- [ ] **Step 4: Run.** `cargo test`; expect pass, tests/gc.rs included.
- [ ] **Step 5: Commit.** `feat(gc): evict store entries no target has held for 30 days`

_Requirements: STOREGC-1.1, STOREGC-1.2, STOREGC-1.3, STOREGC-1.4, STOREGC-1.6, STOREGC-1.7, STOREGC-1.8, STOREGC-9.1, STOREGC-10.1_

---

### Task 5: Keep every key an Undecided or unsupported target may use

**Files:**
- Modify: `src/seed/source.rs` (`holds`), `src/seed/evict.rs`
- Test: `src/seed/source.rs` (unit tests), `tests/store_gc.rs`

**Reuse:** rung 2 — the unit-to-key mapping of `source::live_registry_units` (`src/seed/source.rs:122-171`) and `liveness::liveness`; extract the mapping into a helper both callers use. (The extraction is `unit_key`, done in Task 1.)

**Interfaces:**
- Consumes: `holds`, `Holds`, `evict::run` (Task 4); `Uncertain::clears_when` (`src/liveness.rs:66-85`)
- Produces:
  - `holds` covering the Undecided, unsupported, not-current and unreadable cases;
  - findings `store.target_undecided`, `store.target_unreadable` and `store.walk_fault`;
  - counts `store_entries_held_live`, `_undecided`, `_name_meta` and `_whole_store`.

**Depends-on:** Task 4

- [ ] **Step 1: Write the failing tests.**
  - In `src/seed/source.rs`, `holds` on fixture targets:
    - an Undecided target (a not-current unit inside the window) holds every graph unit's key and every not-current name-meta;
    - an unsupported profile with `.fingerprint/` holds each directory name;
    - an unsupported profile with no `.fingerprint/` holds nothing;
    - an unreadable target gives `whole_store`;
    - a failed `.fingerprint/`, `deps/` or `examples/` listing gives `whole_store` (mode 000 on that dir).
  - In `tests/store_gc.rs`:
    - `an_undecided_lane_keeps_its_entries_past_30_days`, where the finding names what clears it;
    - `a_live_unit_in_a_target_gc_skips_for_an_unsupported_profile_still_holds_its_key`, using a rewritten fingerprint JSON;
    - `an_unreadable_target_holds_the_whole_store_and_gc_still_deletes_sessions_with_its_usual_exit_code`;
    - `a_checkout_walk_fault_apply_would_refuse_on_holds_the_whole_store`. Its `secondary_roots` is a scratch dir with a direct child at mode `0o100`: searchable but not listable, so the walk reports a fault that is not `cannot_enter`;
    - `liveness_writes_nothing_during_eviction`: target mtimes are unchanged.

  Run; expect failures.
- [ ] **Step 2: Implement** the rules of the design's "Held keys" section. `evict::run` passes every fault that `split_checkout_faults` or `faults_that_can_hide_an_owner` keeps into `whole_store`. A non-empty `whole_store` evicts nothing and reports each cause, and `gc`'s exit code stays as it would be.
- [ ] **Step 3: Run.** `cargo test`; expect pass.
- [ ] **Step 4: Commit.** `feat(gc): keep every store key a target whose liveness is undecided may use`

_Requirements: STOREGC-4.1, STOREGC-4.2, STOREGC-4.3, STOREGC-4.4, STOREGC-4.5, STOREGC-10.6_

---

### Task 6: Drop a toolchain generation no checkout uses

**Files:**
- Modify: `src/doctor.rs`, `src/seed/evict.rs`
- Test: `src/doctor.rs` (unit tests), `tests/store_gc.rs`

**Reuse:** rung 2 — `resolve_toolchain_with` and `rustup_which` (`src/doctor.rs:397-472`), parameterized by how the environment is cleaned

**Interfaces:**
- Consumes: `caller_toolchain_kept` (Task 1); `evict::run`, `EvictCtx.checkouts`, `EvictCtx.caller_toolchain` (Task 4); `commit_eviction`'s text drop (Task 3); `choose::choose` (`src/seed/choose.rs:18`)
- Produces:
  - `pub fn resolve_toolchain_own_dir_until(cwd: &Path, rustup: &Path, cancel: &dyn Fn() -> bool) -> Result<Toolchain, String>`. It calls `resolve_toolchain_with` in a mode that removes `RUSTUP_TOOLCHAIN` and `RUSTUP_TOOLCHAIN_SOURCE` from every child, `rustup` and `rustc -vV` alike, whatever their source, and keeps `RUSTUP_AUTO_INSTALL=0`.
  - Counts `store_entries_evicted_toolchain` and `store_toolchains_dropped`; finding `store.toolchain_unresolved`; note `store.no_checkout_resolved`.

**Depends-on:** Task 5

- [ ] **Step 1: Write the failing tests.**
  - In `src/doctor.rs`, `own_dir_mode_passes_no_rustup_toolchain_to_its_children`. Run the own-directory mode of `resolve_toolchain_with` against a fake rustup script (the pattern at `src/doctor.rs:1656`) that writes `${RUSTUP_TOOLCHAIN-unset}` to a file and prints a fake path. The file reads `unset` while the test's own environment has `RUSTUP_TOOLCHAIN` set, as `cargo test` sets it.
  - In `tests/store_gc.rs`:
    - `entries_of_a_value_no_checkout_maps_to_are_dropped_with_their_table_text`. Synthesize entries of a fake value `V'` with a table text `T'` that no checkout resolves. After `gc`, the `V'` entries and `T'` are gone, the fixture's real entries stay, and `choose::choose` on the stored file with `T'` returns `NoRow`.
    - `a_value_still_used_by_one_checkout_is_kept`.
    - `held_entries_of_a_dropped_value_keep_its_text_until_none_remain`.
    - `a_checkout_whose_toolchain_cannot_resolve_blocks_every_drop_but_age_still_runs`: the checkout's `rust-toolchain.toml` names an uninstalled toolchain.
    - `no_registered_repo_drops_no_value`.
    - `an_empty_store_runs_no_own_dir_lookup`: a library call with `EvictCtx.rustup` set to a fake script that logs each call; the log stays empty.
    - `no_caller_toolchain_while_it_is_kept_blocks_every_drop`: a library call with `caller_toolchain_kept: true` and `caller_toolchain: None`. The finding is `store.toolchain_unresolved`, no value is dropped, and age still runs.

  Run; expect failures.
- [ ] **Step 2: Implement** lookups (a) and (b) as design decision 9 describes, with `ctx.rustup` as the rustup path. (b) is `ctx.caller_toolchain` when `ctx.caller_toolchain_kept`, else it is (a) per checkout. When (b) is needed and `ctx.caller_toolchain` is `None`, that is a failed lookup (b): `store.toolchain_unresolved`, and no value is dropped (STOREGC-2.4). The generation step runs after age, and only when the inventory is non-empty.
- [ ] **Step 3: Run.** `cargo test`; expect pass, tests/capability_gate.rs and tests/doctor.rs included.
- [ ] **Step 4: Commit.** `feat(gc): drop store entries of a toolchain no checkout selects`

_Requirements: STOREGC-2.1, STOREGC-2.2, STOREGC-2.4, STOREGC-2.5, STOREGC-10.8_

---

### Task 7: Keep the store under its cap, dependents first

**Files:**
- Modify: `src/seed/evict.rs`
- Test: `tests/store_gc.rs` (library calls to `cargo_hive::seed::evict::run` with a small `Limits.cap_bytes`)

**Reuse:** none — new code (rung 7), because no module orders evictions or phases a store commit. Inside it: rung 2 for `gc::measure` (per-entry freeable), `gc::account::remove_counted` with `Ledger` (removal and freed bytes), `split_checkout_faults` and `faults_that_can_hide_an_owner` (`src/plan_tier/mod.rs:50-64`), and `Receipt` builders.

**Interfaces:**
- Consumes: `evict::run`, `EvictCtx`, `Limits`, `synth_entry` (Task 4); `Holds` (Task 5); `Row.registry_deps`
- Produces:
  - counts `store_entries_evicted_cap` and `store_freeable_bytes_remaining`;
  - finding `store.held_over_cap`, naming up to 10 targets, each with the freeable bytes of the held entries it holds by key or by name-meta, ordered by that number, with ties by path;
  - an `unmeasured` entry for the cap step when `account_bytes` is false.

**Depends-on:** Task 6

- [ ] **Step 1: Write the failing tests.** Each builds `EvictCtx` from a scratch `State`, `Apfs::new`, an empty target list, and entries made with `synth_entry` from 256 KiB files of random bytes, so each frees more than 0 bytes:
  - `the_cap_evicts_unheld_entries_oldest_first_until_under`: three age cohorts and a cap between their sums.
  - `within_one_last_live_at_an_entry_goes_before_the_entries_its_registry_deps_name`. One cohort holds `aaa-dep`, which `zzz-user`'s `registry_deps` names, so alphabetical order would evict the dependency first. The cap is set to stop after one eviction. `zzz-user` goes and `aaa-dep` stays.
  - `held_entries_over_the_cap_are_kept_and_reported`: one target holds entries by key and a second, Undecided target holds another entry only by name-meta. The finding gives the held total and the cap, and names both targets, larger first, each with the exact freeable bytes of the held entries it holds; one entry held by both counts toward each.
  - `an_entry_that_frees_0_bytes_is_not_evicted_for_the_cap`: every file of the entry is `clone_file`d into a scratch dir the test keeps.
  - `with_accounting_off_the_cap_evicts_nothing_and_says_so`: age still evicts.

  Run; expect failures.
- [ ] **Step 2: Implement** design decisions 6 and 7.
  - Measure each remaining entry with `gc::measure(fs, &dir, &[dir.clone()], account_bytes)` and sum every remaining entry's freeable bytes.
  - Sort unheld candidates by `last_live_at`, then reverse-topologically over the cohort's `registry_deps` (dependents first), then by `StoreKey`.
  - Evict while the sum exceeds `cap_bytes`, skipping entries that free 0 bytes.
- [ ] **Step 3: Run.** `cargo test`; expect pass.
- [ ] **Step 4: Commit.** `feat(gc): keep the store under its cap, evicting dependents before what they need`

_Requirements: STOREGC-3.1, STOREGC-3.2, STOREGC-3.3, STOREGC-3.4_

---

### Task 8: Preview with --dry-run, and keep scoped gc off the store

**Files:**
- Modify: `src/seed/evict.rs`, `src/cli.rs`
- Test: `tests/store_gc.rs`

**Reuse:** rung 2 — `gc_cmd` (`src/cli.rs:816-977`), `live_checkouts` (`src/cli.rs:485`), `state::window_days` (`src/state.rs:216`)

**Interfaces:**
- Consumes: `evict::run(…, dry_run, …)` (Task 4)
- Produces: the note `store.skipped_scoped` on `gc <repo>…`

**Depends-on:** Task 7

- [ ] **Step 1: Write the failing tests:**
  - `a_dry_run_reports_the_counts_the_next_real_run_reports`. On an idle fixture with age, toolchain and rowless candidates, every `store_*` count of `gc --dry-run` equals the same count of the `gc` that follows.
  - `a_dry_run_changes_nothing_under_the_store_or_in_store_db`: a byte-for-byte snapshot of `store/` and `store.db`.
  - `a_scoped_gc_reads_and_writes_no_store_state_and_says_why`. The bytes and mtimes of `store/` and `store.db` are unchanged, the note is present, and the receipt has no `store_*` count.

  Run; expect failures.
- [ ] **Step 2: Implement** design decision 12 (plan, then compute the commit's outcome with no lock, no rename and no write), and the scoped note in `gc_cmd` when `args` is not empty.
- [ ] **Step 3: Run.** `cargo test`; expect pass.
- [ ] **Step 4: Commit.** `feat(gc): preview store eviction in a dry run and leave the store to bare gc`

_Requirements: STOREGC-5.1, STOREGC-5.2, STOREGC-5.3_

---

### Task 9: Run gc beside a seed

**Files:**
- Modify: `src/seed/evict.rs`
- Test: `tests/store_gc.rs`

**Reuse:** none — new code (rung 7), because no module orders evictions or phases a store commit. Inside it: rung 2 for `gc::measure` (per-entry freeable), `gc::account::remove_counted` with `Ledger` (removal and freed bytes), `split_checkout_faults` and `faults_that_can_hide_an_owner` (`src/plan_tier/mod.rs:50-64`), and `Receipt` builders.

**Interfaces:**
- Consumes: `State::store_shared_or_cancelled` and `try_store_exclusive` (Task 2), `evict::run` (Task 4)
- Produces: finding `store.busy`; count `store_entries_held_busy`

**Depends-on:** Task 8

- [ ] **Step 1: Write the failing tests:**
  - `while_a_seed_holds_the_store_gc_evicts_nothing_and_keeps_its_exit_code`. The test process holds `store_shared_or_cancelled` on the fixture's `HIVE_HOME` while it runs the `gc` binary. Age candidates exist, yet nothing is evicted. `store.busy` is present, session deletion still happens, and the exit code equals that of a run with no lock held.
  - `seeds_and_gcs_in_a_loop_leave_no_partial_unit`. Run 12 cycles, with `seed` on a new lane in one thread and bare `gc` in another, backdating the store's rows each cycle. Every seeded unit has every file its entry holds, or has no `.fingerprint/<name-meta>/`. No `seed` exits non-zero, and no seed receipt has a `unit.refused` finding.
  - `a_stale_evict_tree_leaves_seed_store_bytes_working_and_the_next_gc_removes_it`. Create a `store/.tmp/evict-1-0/` tree with current mtimes, so fill's one-day sweep keeps it. `seed` exits 0 with `store_bytes` measured, and the next bare `gc` removes the tree.

  Run; expect failures.
- [ ] **Step 2: Implement** the busy path: when `try_store_exclusive` returns `None`, report the `store.busy` finding, whose text says to run `gc` again when no seed runs, and the count; rename nothing and write nothing.
- [ ] **Step 3: Run.** `cargo test --test store_gc`, then `cargo test`; expect pass.
- [ ] **Step 4: Commit.** `feat(gc): skip store eviction while a seed uses the store`

_Requirements: STOREGC-6.1, STOREGC-6.2, STOREGC-9.4, STOREGC-10.13_

---

### Task 10: Count what store eviction freed

**Files:**
- Modify: `src/seed/evict.rs`
- Test: `tests/store_gc.rs`, `tests/gc_disk_image.rs`

**Reuse:** none — new code (rung 7), because no module orders evictions or phases a store commit. Inside it: rung 2 for `gc::measure` (per-entry freeable), `gc::account::remove_counted` with `Ledger` (removal and freed bytes), `split_checkout_faults` and `faults_that_can_hide_an_owner` (`src/plan_tier/mod.rs:50-64`), and `Receipt` builders.

**Interfaces:**
- Consumes: `evict::run` and `synth_entry` (Task 4); `Image` (`tests/gc_disk_image.rs`); `Recording::fail_sharing` (`tests/common/recording_fs.rs:167`)
- Produces:
  - counts `store_freed_bytes`, `store_bytes_still_shared` and `store_entries_held`;
  - `unmeasured` entries with the reason "doctor check (c) did not pass";
  - per-file `unmeasured` paths.

**Depends-on:** Task 9

- [ ] **Step 1: Write the failing tests.** Every entry is made by hand with 2 MiB files, as `spike-k4.md` measured:
  - In `tests/store_gc.rs`, as library calls:
    - `freed_and_still_shared_bytes_split_by_sharing`. One evicted entry holds a compressed file nothing clones and an incompressible file that a kept scratch clone still shares. `store_freed_bytes` equals the compressed file's blocks, and `store_bytes_still_shared` equals the incompressible file's blocks.
    - `with_check_c_failed_the_store_bytes_are_unmeasured`: `account_bytes` is false.
    - `an_unreadable_file_in_an_evicted_entry_is_unmeasured_not_counted`. `Recording::fail_sharing` is applied to the renamed path `store/.tmp/evict-<std::process::id()>-0/<file>`.
  - In `tests/gc_disk_image.rs`, `store_freed_bytes_never_exceed_what_the_volume_regained`. `HIVE_HOME` sits on the image, with a compressed entry and an incompressible entry that a clone on the image shares, made by hand and backdated. Run bare `gc`; `store_freed_bytes` is at most the image's available-bytes increase.

  Run; expect failures.
- [ ] **Step 2: Implement** the receipt counts of STOREGC-7.1 that remain, from the ledger's `freed` and `still_shared` as `remove_counted` reports them. Without `account_bytes`, each bytes count goes to `unmeasured` with the reason.
- [ ] **Step 3: Run.** `cargo test`, then `cargo test --test gc_disk_image`; expect pass.
- [ ] **Step 4: Commit.** `feat(gc): report the bytes store eviction freed and what clones still share`

_Requirements: STOREGC-7.1, STOREGC-7.2, STOREGC-7.3, STOREGC-7.4, STOREGC-9.3, STOREGC-10.9_

---

### Task 11: A killed or cancelled gc leaves every store key whole or absent

**Files:**
- Modify: `src/seed/evict.rs`, `src/cli.rs` (exit 130 when `EvictOutcome.cancelled`)
- Test: `tests/store_gc.rs`

**Reuse:** none — new code (rung 7), because no module orders evictions or phases a store commit. Inside it: rung 2 for `gc::measure` (per-entry freeable), `gc::account::remove_counted` with `Ledger` (removal and freed bytes), `split_checkout_faults` and `faults_that_can_hide_an_owner` (`src/plan_tier/mod.rs:50-64`), and `Receipt` builders.

**Interfaces:**
- Consumes: `evict::run`, `synth_entry` (Task 4); `commit_eviction` (Task 3); the kill-test pattern of `killing_gc_at_any_point_never_costs_a_rebuild` (`tests/gc.rs:393`)
- Produces: none new

**Depends-on:** Task 10

- [ ] **Step 1: Write the failing tests:**
  - `a_cancel_during_commit_records_what_was_renamed_and_releases_both_locks`. This is a library call whose cancel closure turns true on its Nth call, for N across the commit. Afterwards:
    - `cancelled` is true;
    - `store.db` has no row for a renamed key;
    - every remaining row's directory is complete;
    - `try_store_exclusive` and `State::locked` both succeed.
  - `killing_gc_during_store_eviction_never_leaves_a_partial_entry_listed_or_adoptable`. Synthesize 2,000 backdated entries of a few files each. For a range of kill delays, SIGKILL a bare `gc`. After each kill:
    - every row's directory holds every file;
    - `store::read` lists only complete entries;
    - a `seed` exits 0 with no `unit.refused`;
    - no entry path holds a partial directory.
  - `gc_exits_130_when_store_eviction_is_cancelled`: SIGINT is sent while the test holds the state lock, so commit waits. The exit is 130.

  Run; expect failures.
- [ ] **Step 2: Implement** design decision 13.
  - Check cancel before each rename, inside `commit_eviction`'s rename closure.
  - Take the state lock non-cancellably.
  - Skip the removal on cancel.
  - In `gc_cmd`, exit 130 when `EvictOutcome.cancelled`.
- [ ] **Step 3: Run.** `cargo test`; expect pass.
- [ ] **Step 4: Commit.** `fix(gc): keep store entries whole when gc is killed or cancelled mid-eviction`

_Requirements: STOREGC-9.2_

---

### Task 12: Tell agents and readers how the store is bounded

**Files:**
- Modify: `plugin/skills/hive/SKILL.md`, `README.md` (lines 5-8, line 10, and the seed row at line 54), `tests/skill.rs` (lines 569-572), `src/cli.rs` (the `Gc` doc comment, lines 44-46)
- Test: `tests/skill.rs`

**Reuse:** rung 2 — the existing skill rows on `gc` (`plugin/skills/hive/SKILL.md:19,24,35-36,40,53-55,71`) and README lines 10 and 54

**Interfaces:**
- Consumes: count and finding names (Tasks 4 to 11)
- Produces: none in code

**Depends-on:** Task 11

- [ ] **Step 1: Write the failing test.** Change the README assertion in `tests/skill.rs` to the new sentence "Bare `cargo hive gc` keeps the store under 40 GB". Add an assertion that SKILL.md names `store` on a gc line. Run `cargo test --test skill`; expect failure.
- [ ] **Step 2: Edit SKILL.md.**
  - The gc lines say that bare `gc` also evicts from the store by age, toolchain and the 40 GB cap; that `gc --dry-run` previews it; and that `gc <repo>` leaves the store alone.
  - "Reading a receipt" names the `store_*` counts, and says that `store.busy` means to run `gc` again when no seed runs and that `store.held_over_cap` names the targets to retire.
  - Say when to run bare `gc`: after a batch of lanes finishes, and whenever `seed`'s `store_bytes` passes 40 GB.
  - "Do not" adds "delete `~/.hive/store/` by hand".
  - Add a `cargo hive gc --json` command line.
  - Stay within 150 lines and the seven headings.
- [ ] **Step 3: Edit the README and the help text.**
  - Replace the README sentence at line 10 and in the seed row at line 54 with the new sentence.
  - Lines 5-8 now say that bare `gc` also bounds the store.
  - The `Gc` doc comment adds "and evicts from the store when given no repo".
  - Run `cargo test --test skill` and `cargo test`; expect pass.
- [ ] **Step 4: Commit.** `docs(skill): teach agents that bare gc bounds the store`

_Requirements: STOREGC-8.1, STOREGC-8.2, STOREGC-10.11_

---

### Task 13: Dated notes in the specs this feature narrows

**Files:**
- Modify: `docs/specs/2026-11-02-store-seed/design.md` (decision 6), `docs/specs/2026-11-02-store-seed/requirements.md` (SEED-10.1), `docs/specs/2026-11-01-gc-plan-apply/requirements.md` (GCPLAN-9.1)

**Reuse:** rung 2 — `docs/specs/2026-11-02-store-seed/design.md` decisions 5 and 6

**Interfaces:**
- Consumes: none
- Produces: none

**Depends-on:** Task 4

- [ ] **Step 1: Edit.**
  - Add to SEED design decision 6: "(2026-11-09, STOREGC: never modified; store eviction may remove a published directory whole.)"
  - Add to SEED-10.1 and GCPLAN-9.1: "(2026-11-09: STOREGC adds store counts to bare `gc`; target-deletion counts are unchanged, STOREGC-10.1.)"
  - Change no ID and no other words.
- [ ] **Step 2: Check.** Run the `audit-trace` skill over `docs/specs/`; expect no new finding.
- [ ] **Step 3: Commit.** `docs(specs): note where store GC narrows SEED and GCPLAN guards`

_Requirements: STOREGC-10.1, STOREGC-10.4_
