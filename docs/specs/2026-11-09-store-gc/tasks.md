# Tasks: Store GC (bare `cargo hive gc` keeps hive's store bounded)

> **For agentic workers:** after plan approval, pick one execute skill:
> - `build-in-waves`: a continuous, dependency-aware scheduler;
> - `build-by-story`: human-gated story review units;
> - `build-inline`: the controller implements, with no implementer subagents.
>
> The chosen skill writes `Execution-mode:`. Steps use checkbox (`- [ ]`) syntax for tracking.

Feature code: STOREGC
Status: Draft
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
| `docs/architecture/INDEX.md`: this feature relies on ARCH-2, 4, 8, 10, 11, 13, 14, 15, 16, 17, 18, 19 | `36f62957f3f4ca1b` |
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
6. No test-only branch in a production path. The cap is injected through `Limits` (Weigh W10). Time is injected by editing `last_live_at` in a scratch `store.db`. Failures are injected through the `Fs` wrapper `Recording`.
7. Code names the key's parts `toolchain_value`, `name_meta` and `fingerprint_hash`, never `rustc` or `fingerprint` alone.
8. Tests never touch the real `~/.cargo`, `~/.hive` or `~/.cache`. `Hive` gives a scratch `HIVE_HOME` and `CARGO_HOME`, and every test that classifies checkouts owns its `secondary_roots`.
9. `hive.json(&[...])` appends `--json` itself; passing it again exits 2.

## File Structure

| File | Responsibility in this plan |
|---|---|
| `src/state.rs` | Store lock: `StoreLock`, `State::store_shared_or_cancelled`, `State::try_store_exclusive`, on `<state root>/store.lock` |
| `src/doctor.rs` | `caller_toolchain_kept()` extracted from `drop_directory_toolchain`; `resolve_toolchain_own_dir_until` (lookup (a)) |
| `src/seed/source.rs` | Unit-to-key helper extracted from `live_registry_units`; `holds` (held keys, name-metas, whole-store causes, findings) |
| `src/seed/store.rs` | `StoreKey`, `Inventory`, `inventory`, `CommitPlan`, `Committed`, `commit_eviction` |
| `src/seed/evict.rs` (new) | `run`, `EvictCtx`, `Limits`, `EvictOutcome`: plan, rules, cap order, commit, removal, counts |
| `src/seed/mod.rs` | `pub mod evict`; `seed::run` takes the store lock shared before `fill::fill` and drops it after `place` |
| `src/cli.rs` | `live_checkouts` returns `(checkouts, faults)`; `gc_cmd` calls `evict::run` with `CAP_BYTES` and `MAX_AGE_SECS`; the `gc <repo>` note |
| `tests/common/store_fixture.rs` (new) | Test helpers: seed a store from `RegistryRepo`, read and edit `store.db` rows, list entry dirs |
| `tests/common/mod.rs` | `mod store_fixture` declaration only |
| `tests/store_gc.rs` (new) | Integration tests for stories 1 to 7 and 9 |
| `tests/gc_disk_image.rs` | One disk-image test for store freed bytes |
| `plugin/skills/hive/SKILL.md`, `README.md`, `tests/skill.rs` | Store text, a bare `gc --json` command line, the replaced README sentence |
| `docs/specs/2026-11-02-store-seed/design.md`, `docs/specs/2026-11-02-store-seed/requirements.md`, `docs/specs/2026-11-01-gc-plan-apply/requirements.md` | Dated notes: SEED decision 6, SEED-10.1, GCPLAN-9.1 |

Retrieval (load-subgraph, schema 1.1, fsubr-1.2, 2026-11-09): `blast_radius` over these paths names SEED (`src/seed/*`, `src/state.rs`), CORE, GCPLAN (`src/state.rs`, `src/cli.rs`, the skill), AUTOGC (`src/cli.rs`, `src/doctor.rs`, `tests/gc_disk_image.rs`), EXPLAIN (`src/cli.rs`, `src/doctor.rs`) and SKILL (the skill, `tests/skill.rs`, README). `cluster(STOREGC)` returns STOREGC alone: its OWNS is empty until this file lands. OWNS coverage is 7 of 8; observations none.

---

### Task 1: Prefactor the three seams eviction reads, behavior unchanged

**Files:**
- Modify: `src/cli.rs` (`live_checkouts`, lines 485-520, and its three callers at lines 862, 1141 and 1301)
- Modify: `src/seed/source.rs` (the unit-to-key mapping inside `live_registry_units`, lines 122-171)
- Modify: `src/doctor.rs` (`drop_directory_toolchain`, lines 355-367)
- Test: `src/doctor.rs` (unit tests), `src/seed/source.rs` (unit tests)

**Reuse:** rung 2 — `gc_cmd` (`src/cli.rs:816-977`), `live_checkouts` (`src/cli.rs:485`), `state::window_days` (`src/state.rs:216`)

**Interfaces:**
- Consumes: `discover::WalkFault`, `fingerprint::Profile`, `liveness::UnitRef`
- Produces:
  - `fn live_checkouts(...) -> (Vec<Row>, Vec<WalkFault>)`, with the same findings as today made by each caller from the faults;
  - `pub(crate) fn unit_key(profile: &Profile, dir_name: &str) -> Option<StoreKey>` in `seed::source` (the `StoreKey` type is defined here first and moved to `store.rs` in Task 3);
  - `pub(crate) fn caller_toolchain_kept() -> bool` in `doctor`.

**Depends-on:** none

- [ ] **Step 1: Write the failing tests.**
  - In `src/doctor.rs`: `caller_toolchain_kept` table over `RUSTUP_TOOLCHAIN_SOURCE` values `cli`, `env`, `default` (true) and `toolchain-file`, `path-override`, absent (false).
  - In `src/seed/source.rs`: `unit_key` on a fixture profile returns `(toolchain_value, name_meta, fingerprint_hash)` for a current unit, and `None` for a not-current unit and for a name the graph lacks.

  Run `cargo test --lib`; expect a compile failure.
- [ ] **Step 2: Implement.**
  - `drop_directory_toolchain` calls `caller_toolchain_kept`.
  - `live_registry_units` calls `unit_key`.
  - `live_checkouts` returns its faults; `gc_cmd`, `plan_cmd` and `explain_cmd` each make from them the findings it made before.
- [ ] **Step 3: Run.** `cargo test`; expect pass, with tests/gc.rs, tests/plan_tier.rs, tests/plan_apply.rs, tests/explain.rs, tests/seed_fill.rs and tests/capability_gate.rs unchanged.
- [ ] **Step 4: Commit.** `refactor: expose walk faults, the unit store key and the kept-toolchain predicate`

_Requirements: STOREGC-10.2, STOREGC-10.7, STOREGC-10.10, STOREGC-10.12_

---

### Task 2: The store lock, and seed holding it shared

**Files:**
- Modify: `src/state.rs`
- Modify: `src/seed/mod.rs` (`seed::run`, the guard before `fill::fill` at line 194, dropped after `place`)
- Test: `src/state.rs` (unit tests), `tests/seed.rs`

**Reuse:** rung 2 — the polling `flock` loop of `State::locked_or_cancelled` (`src/state.rs:151-178`), extended with a shared mode and a single exclusive try

**Interfaces:**
- Consumes: none
- Produces: `pub struct StoreLock` (releases on drop); `pub fn store_shared_or_cancelled(&self, cancel: impl FnMut() -> bool) -> Result<StoreLock, StateError>`; `pub fn try_store_exclusive(&self) -> Result<Option<StoreLock>, StateError>`. Both create `<state root>/store.lock` if absent.

**Depends-on:** none

- [ ] **Step 1: Write the failing tests.**
  - In `src/state.rs`:
    - `two_shared_store_locks_coexist`.
    - `an_exclusive_try_returns_none_while_a_shared_lock_is_held`, with a second `State` on the same root in the same process (flock is per open file description).
    - `a_shared_lock_waits_for_an_exclusive_one_and_honors_cancel`: the cancel closure returns true and the call returns `StateError::Cancelled`.
    - `the_lock_file_holds_no_bytes`.
  - In `tests/seed.rs`, `a_seed_waits_for_an_exclusive_store_lock_and_then_seeds`: the test holds `try_store_exclusive` on the fixture's `HIVE_HOME`, starts `cargo hive seed`, releases after 300 ms, and the seed exits 0 with clones.

  Run `cargo test --lib state::` and `cargo test --test seed a_seed_waits`; expect compile failure, then failure.
- [ ] **Step 2: Implement** the two methods on the `locked_or_cancelled` pattern with `LOCK_SH | LOCK_NB` polled every `LOCK_POLL`, and `LOCK_EX | LOCK_NB` tried once. `seed::run` takes the shared lock with the seed's cancel closure before `fill::fill` and keeps it until `place` returns. A cancel while waiting is the existing SEED-7.5 path.
- [ ] **Step 3: Run.** `cargo test`; expect pass, with every tests/seed.rs, tests/seed_fill.rs and tests/status_seed.rs test unchanged.
- [ ] **Step 4: Commit.** `feat(state): add a store lock that seed holds shared while it fills and clones`

_Requirements: STOREGC-10.3_

---

### Task 3: Bare gc evicts entries nothing has held for 30 days

**Files:**
- Create: `src/seed/evict.rs`
- Create: `tests/common/store_fixture.rs`
- Create: `tests/store_gc.rs`
- Modify: `src/seed/store.rs`, `src/seed/source.rs`, `src/seed/mod.rs` (`pub mod evict`, `#[doc(hidden)]`), `src/cli.rs` (`gc_cmd`), `tests/common/mod.rs`

**Reuse:** none — new code (rung 7), because no module orders evictions or phases a store commit. Inside it: rung 2 for `gc::measure` (per-entry freeable), `gc::account::remove_counted` with `Ledger` (removal and freed bytes), `split_checkout_faults` and `faults_that_can_hide_an_owner` (`src/plan_tier/mod.rs:50-64`), and `Receipt` builders.

**Interfaces:**
- Consumes:
  - `unit_key` (Task 1) and `live_checkouts` faults (Task 1);
  - `State::try_store_exclusive` (Task 2);
  - `liveness::liveness`, `store::read`, `store::entry_dir`, `store::tmp_dir`, `gc::account::remove_counted`.
- Produces:
  - `pub struct StoreKey { pub toolchain_value: u64, pub name_meta: String, pub fingerprint_hash: String }` in `seed::store`.
  - `pub fn inventory(fs: &dyn Fs, state: &State) -> Result<Inventory, StateError>`, with `Inventory { rows: Vec<Row>, rowless: Vec<(StoreKey, PathBuf)>, tmp_bytes: u64 }`.
  - `pub fn commit_eviction(state: &State, plan: &CommitPlan, rename: impl FnMut(&StoreKey) -> io::Result<PathBuf>) -> Result<Committed, StateError>`, with `CommitPlan { candidates: Vec<(StoreKey, Option<u64>)>, refreshed: Vec<StoreKey>, now: u64 }` (the `Option<u64>` is the row's `last_live_at` at plan time, `None` for a rowless directory) and `Committed { renamed: Vec<(StoreKey, PathBuf)>, kept_changed: usize, dropped_values: Vec<u64> }`.
  - `pub(crate) fn holds(fs: &dyn Fs, targets: &[TargetDir], now: SystemTime, window: Duration) -> Holds`, with `Holds { keys: BTreeSet<StoreKey>, name_metas: BTreeSet<String>, whole_store: Vec<Finding>, findings: Vec<Finding>, by_reason: BTreeMap<&'static str, u64> }` (Decided targets only in this task).
  - `pub fn run(ctx: &EvictCtx<'_>, limits: Limits, dry_run: bool, cancel: &dyn Fn() -> bool) -> EvictOutcome` in `seed::evict`.
  - `EvictCtx { state, fs, targets, checkouts, checkout_faults, scan_faults, cache_roots, window, account_bytes, now }`.
  - `Limits { cap_bytes: u64, max_age_secs: u64 }`.
  - `EvictOutcome { counts: Vec<(String, u64)>, findings: Vec<Finding>, notes: Vec<String>, unmeasured: Vec<(String, String)>, cancelled: bool }`.
  - In `cli.rs`: `const CAP_BYTES: u64 = 40_000_000_000;` and `const MAX_AGE_SECS: u64 = 30 * 86_400;`.
  - `tests/common/store_fixture.rs`: `seeded_store(hive: &Hive) -> (RegistryRepo, PathBuf)`, `rows(hive) -> Vec<serde_json::Value>`, `set_last_live_at(hive, name_meta_prefix, secs_ago)`, `entry_dirs(hive) -> Vec<PathBuf>`.

**Depends-on:** Task 1, Task 2

- [ ] **Step 1: Write the failing tests** in `tests/store_gc.rs`, each on a store filled by `seed` from a `RegistryRepo` whose own target is then deleted or rebuilt as the test needs:
  - `an_entry_no_target_holds_for_31_days_is_evicted_with_its_row`: the entry dir and row are gone and `store_entries_evicted_age` is 1.
  - `an_entry_held_by_a_live_target_stays_and_its_last_live_at_is_refreshed`: the row has `last_live_at` 31 days old, the target has built inside the window, and after `gc` the row's `last_live_at` is at least the test's start time.
  - `an_entry_29_days_old_stays`.
  - `a_rowless_entry_dir_no_target_holds_is_evicted`.
  - `a_rowless_entry_dir_a_target_holds_is_left_for_fill`.
  - `tmp_is_left_alone_and_its_bytes_are_counted`: `store_tmp_bytes` > 0 and `.tmp` is unchanged.
  - `gc_window_days_out_of_range_evicts_nothing_and_still_deletes_sessions`: `store.window_invalid`, and AUTOGC's session deletion still happens.
  - `a_live_unit_in_an_orphan_candidate_target_holds_its_key`.
  - `a_live_unit_in_a_target_gc_skips_for_its_volume_or_profile_still_holds_its_key` (unsupported profile via a rewritten fingerprint JSON).
  - `store_db_keeps_its_fields_after_eviction`: every remaining row has exactly the keys it had before.
  - `eviction_touches_nothing_outside_the_store`: a snapshot of the repo, its targets, a seeded checkout, `CARGO_HOME`, `seeds.json`, `plans.json` and `config.json` is unchanged.

  Run `cargo test --test store_gc`; expect a compile failure.
- [ ] **Step 2: Implement** `inventory` (lists `store/<u64>/<name-meta>-<hash>/` only, with `read_dir_bounded` and `CRATE_DIR_CAP`, skipping `.tmp`, `CACHEDIR.TAG` and any other name), `holds` for Decided targets, and `commit_eviction`. `commit_eviction` takes `State::locked` before the first rename and, in one `Tx`, re-reads rows, drops candidates whose `last_live_at` rose or that gained a row, calls `rename`, writes rows with the refreshed `last_live_at`, and returns what it renamed.
- [ ] **Step 3: Implement** `evict::run` for the age rule (design decisions 3, 4, 11):
  - Plan: holds, inventory, candidates.
  - Commit: `try_store_exclusive`, remove stale `store/.tmp/evict-*`, `commit_eviction`, remove emptied `store/<value>/`.
  - Removal: `remove_counted` each renamed tree under the lock, then release.

  Wire it into `gc_cmd` after the cross-check notes (`src/cli.rs:903-912`), only when `repos` is empty, `--plan` is absent and `report.cancelled` is false. Call `window_days` first (STOREGC-1.7). Add the counts with `put_counts`-style names from the design.
- [ ] **Step 4: Run.** `cargo test`; expect pass, tests/gc.rs included.
- [ ] **Step 5: Commit.** `feat(gc): evict store entries no target has held for 30 days`

_Requirements: STOREGC-1.1, STOREGC-1.2, STOREGC-1.3, STOREGC-1.4, STOREGC-1.5, STOREGC-1.6, STOREGC-1.7, STOREGC-1.8, STOREGC-9.1, STOREGC-10.1, STOREGC-10.5_

---

### Task 4: Keep every key an Undecided or unsupported target may use

**Files:**
- Modify: `src/seed/source.rs` (`holds`), `src/seed/evict.rs`
- Test: `src/seed/source.rs` (unit tests), `tests/store_gc.rs`

**Reuse:** rung 2 — the unit-to-key mapping of `source::live_registry_units` (`src/seed/source.rs:122-171`) and `liveness::liveness`; extract the mapping into a helper both callers use

**Interfaces:**
- Consumes: `holds`, `Holds` and `evict::run` (Task 3); `Uncertain::clears_when` (`src/liveness.rs:66-85`)
- Produces: `holds` covering Undecided, unsupported, not-current and unreadable cases; finding kinds `store.target_undecided`, `store.target_unreadable` and `store.walk_fault`; counts `store_entries_held_live`, `_undecided`, `_name_meta` and `_whole_store`.

**Depends-on:** Task 3

- [ ] **Step 1: Write the failing tests.**
  - In `src/seed/source.rs`, `holds` on fixture targets:
    - A Decided target with a live not-current node holds that node's name-meta.
    - An Undecided target (a not-current unit inside the window) holds every graph unit's key and every not-current name-meta.
    - An unsupported profile with `.fingerprint/` holds each directory name.
    - An unsupported profile with no `.fingerprint/` holds nothing.
    - An unreadable target, or a failed `.fingerprint/` listing, gives `whole_store`.
  - In `tests/store_gc.rs`:
    - `an_undecided_lane_keeps_its_entries_past_30_days` (finding names what clears it).
    - `an_unreadable_target_holds_the_whole_store_and_gc_still_deletes_sessions_with_its_usual_exit_code`.
    - `a_checkout_walk_fault_apply_would_refuse_on_holds_the_whole_store`, with `secondary_roots` set to a scratch dir holding an unreadable subdirectory that is not `cannot_enter`.
    - `liveness_writes_nothing_during_eviction`: target mtimes are unchanged.

  Run; expect failures.
- [ ] **Step 2: Implement** the rules of the design's "Held keys" section. `evict::run` passes every fault `split_checkout_faults` or `faults_that_can_hide_an_owner` keeps into `whole_store`. A non-empty `whole_store` evicts nothing, reports each cause, and leaves `gc`'s exit code as it would be.
- [ ] **Step 3: Run.** `cargo test`; expect pass.
- [ ] **Step 4: Commit.** `feat(gc): keep every store key a target whose liveness is undecided may use`

_Requirements: STOREGC-4.1, STOREGC-4.2, STOREGC-4.3, STOREGC-4.4, STOREGC-4.5, STOREGC-10.6_

---

### Task 5: Drop a toolchain generation no checkout uses

**Files:**
- Modify: `src/doctor.rs`, `src/seed/evict.rs`, `src/seed/store.rs` (`commit_eviction` drops texts)
- Test: `src/doctor.rs` (unit tests), `tests/store_gc.rs`

**Reuse:** rung 2 — `resolve_toolchain_with` and `rustup_which` (`src/doctor.rs:397-472`), parameterized by how the environment is cleaned

**Interfaces:**
- Consumes: `caller_toolchain_kept` (Task 1); `evict::run`, `commit_eviction`, `EvictCtx.checkouts` (Task 3)
- Produces:
  - `pub fn resolve_toolchain_own_dir_until(cwd: &Path, cancel: &dyn Fn() -> bool) -> Result<Toolchain, String>`, which removes `RUSTUP_TOOLCHAIN` and `RUSTUP_TOOLCHAIN_SOURCE` whatever their source and keeps `RUSTUP_AUTO_INSTALL=0`;
  - `Committed.dropped_values` populated;
  - counts `store_entries_evicted_toolchain` and `store_toolchains_dropped`, finding `store.toolchain_unresolved`, note `store.no_checkout_resolved`.

**Depends-on:** Task 3

- [ ] **Step 1: Write the failing tests.**
  - In `src/doctor.rs`, `own_dir_lookup_ignores_a_default_source_rustup_toolchain`: a scratch dir with a `rust-toolchain.toml` naming an installed toolchain other than the default, run with `RUSTUP_TOOLCHAIN` set and `RUSTUP_TOOLCHAIN_SOURCE=default`; the result's `release` is the pinned one. Skip with a printed reason when only one toolchain is installed.
  - In `tests/store_gc.rs`:
    - `entries_of_a_value_no_checkout_maps_to_are_dropped_with_their_table_text`: edit the table text of the fixture's value in `store.db` to a text no checkout resolves. After `gc`, those entries and that text are gone, and a `seed` on a checkout re-pinned to the old text reports `toolchain.missing`.
    - `a_value_still_used_by_one_checkout_is_kept`.
    - `held_entries_of_a_dropped_value_keep_its_text_until_none_remain`.
    - `a_checkout_whose_toolchain_cannot_resolve_blocks_every_drop_but_age_still_runs` (pin an uninstalled toolchain).
    - `no_registered_repo_drops_no_value`.
    - `an_empty_store_spawns_no_rustup`: a `Recording`-free check that the receipt shows no `store.toolchain_*` finding and the run completes with an empty `store.db`.

  Run; expect failures.
- [ ] **Step 2: Implement** lookups (a) and (b) per design decision 9, reusing `gc_cmd`'s `resolve_toolchain(state.root())` result as (b) when `caller_toolchain_kept()`. Do the generation step after age. Drop texts in `commit_eviction` for every value with no row and no entry directory left.
- [ ] **Step 3: Run.** `cargo test`; expect pass, tests/capability_gate.rs and tests/doctor.rs included.
- [ ] **Step 4: Commit.** `feat(gc): drop store entries of a toolchain no checkout selects`

_Requirements: STOREGC-2.1, STOREGC-2.2, STOREGC-2.3, STOREGC-2.4, STOREGC-2.5, STOREGC-10.8_

---

### Task 6: Keep the store under its cap, dependents first

**Files:**
- Modify: `src/seed/evict.rs`
- Test: `tests/store_gc.rs` (library calls to `cargo_hive::seed::evict::run` with a small `Limits.cap_bytes`)

**Reuse:** rung 2 — `gc::measure` (`src/gc/run.rs:393`) per entry directory

**Interfaces:**
- Consumes: `evict::run`, `EvictCtx`, `Limits` (Task 3); `Holds` (Task 4); `Row.registry_deps`
- Produces: counts `store_entries_evicted_cap` and `store_freeable_bytes_remaining`; finding `store.held_over_cap`; `unmeasured` entry for the cap step when `account_bytes` is false.

**Depends-on:** Task 4

- [ ] **Step 1: Write the failing tests**, each building `EvictCtx` from a scratch `State`, `Apfs` and the fixture's scanned targets:
  - `the_cap_evicts_unheld_entries_oldest_first_until_under`: three age cohorts and a cap between their sums.
  - `within_one_last_live_at_an_entry_goes_before_the_entries_its_registry_deps_name`: a cohort where alphabetical order would evict a dependency first. Every entry frees more than 0 bytes; assert no remaining row names an evicted fingerprint hash in `registry_deps`.
  - `held_entries_over_the_cap_are_kept_and_reported`.
  - `an_entry_that_frees_0_bytes_is_not_evicted_for_the_cap`: a store file cloned into a kept scratch file.
  - `with_accounting_off_the_cap_evicts_nothing_and_says_so`, while age still evicts.

  Run; expect failures.
- [ ] **Step 2: Implement** design decisions 6 and 7: measure each remaining entry with `gc::measure(fs, &dir, &[dir.clone()], account_bytes)`, sum every remaining entry's freeable bytes, and sort unheld candidates by `last_live_at`, then reverse-topologically over the cohort's `registry_deps`, then by key. Evict while the sum exceeds `cap_bytes`, skipping 0-freeable entries.
- [ ] **Step 3: Run.** `cargo test`; expect pass.
- [ ] **Step 4: Commit.** `feat(gc): keep the store under its cap, evicting dependents before what they need`

_Requirements: STOREGC-3.1, STOREGC-3.2, STOREGC-3.3, STOREGC-3.4_

---

### Task 7: Preview with --dry-run, and keep scoped gc off the store

**Files:**
- Modify: `src/seed/evict.rs`, `src/cli.rs`
- Test: `tests/store_gc.rs`

**Reuse:** rung 2 — `gc_cmd` (`src/cli.rs:816-977`), `live_checkouts` (`src/cli.rs:485`), `state::window_days` (`src/state.rs:216`)

**Interfaces:**
- Consumes: `evict::run(…, dry_run, …)` (Task 3)
- Produces: note `store.skipped_scoped` on `gc <repo>…`

**Depends-on:** Task 6, Task 5

- [ ] **Step 1: Write the failing tests:**
  - `a_dry_run_reports_the_counts_the_next_real_run_reports`: compare every `store_*` count of `gc --dry-run` and then `gc` on an idle fixture with age, toolchain and rowless candidates.
  - `a_dry_run_changes_nothing_under_the_store_or_in_store_db`: a byte-for-byte snapshot of `store/` and `store.db` before and after.
  - `a_scoped_gc_reads_and_writes_no_store_state_and_says_why`: `store/` and `store.db` mtimes and bytes are unchanged, the note is present, and the receipt has no `store_*` count.

  Run; expect failures.
- [ ] **Step 2: Implement** design decision 12 (plan, then compute the commit's outcome with no lock, no rename and no write) and the scoped note in `gc_cmd`.
- [ ] **Step 3: Run.** `cargo test`; expect pass.
- [ ] **Step 4: Commit.** `feat(gc): preview store eviction in a dry run and leave the store to bare gc`

_Requirements: STOREGC-5.1, STOREGC-5.2, STOREGC-5.3_

---

### Task 8: Run gc beside a seed

**Files:**
- Modify: `src/seed/evict.rs`
- Test: `tests/store_gc.rs`

**Reuse:** rung 2 — `State::try_store_exclusive` (Task 2)

**Interfaces:**
- Consumes: `State::store_shared_or_cancelled` (Task 2, from the test process), `evict::run` (Task 3)
- Produces: finding `store.busy`; count `store_entries_held_busy`

**Depends-on:** Task 6

- [ ] **Step 1: Write the failing tests:**
  - `while_a_seed_holds_the_store_gc_evicts_nothing_and_keeps_its_exit_code`: the test process holds `store_shared_or_cancelled` on the fixture's `HIVE_HOME` while it runs the `gc` binary. Assert age candidates exist, nothing is evicted, `store.busy` is present, session deletion still happens, and the exit code equals a run with no seed.
  - `seeds_and_gcs_in_a_loop_leave_no_partial_unit`: 12 cycles of `seed` on a new lane in one thread and bare `gc` in another, on a store whose entries are backdated each cycle. Each seeded unit has every file its entry holds, or no `.fingerprint/<name-meta>/`. No `seed` exits non-zero, and no seed receipt has a `unit.refused` finding.
  - `stale_evict_trees_leave_seed_store_bytes_and_the_tmp_sweep_working`: a `store/.tmp/evict-1-0/` tree with backdated mtimes is present; `seed` exits 0 with `store_bytes` measured, and the next `gc` removes the tree.

  Run; expect failures.
- [ ] **Step 2: Implement** the busy path: `try_store_exclusive` returning `None` gives the finding and the count, renames nothing and writes nothing.
- [ ] **Step 3: Run.** `cargo test --test store_gc`, then `cargo test`; expect pass.
- [ ] **Step 4: Commit.** `feat(gc): skip store eviction while a seed uses the store`

_Requirements: STOREGC-6.1, STOREGC-6.2, STOREGC-9.4, STOREGC-10.13_

---

### Task 9: Count what store eviction freed

**Files:**
- Modify: `src/seed/evict.rs`
- Test: `tests/store_gc.rs`, `tests/gc_disk_image.rs`

**Reuse:** rung 2 — `gc::account::remove_counted` and `Ledger` (`src/gc/account.rs:43`, `src/gc/ledger.rs:50-107`)

**Interfaces:**
- Consumes: `evict::run` (Task 3), `Image` (`tests/gc_disk_image.rs`)
- Produces: counts `store_freed_bytes`, `store_bytes_still_shared` and `store_entries_held`; `unmeasured` entries with "doctor check (c) did not pass"; per-file `unmeasured` paths.

**Depends-on:** Task 6

- [ ] **Step 1: Write the failing tests:**
  - In `tests/store_gc.rs`:
    - `freed_and_still_shared_bytes_split_by_sharing`: one evicted entry whose incompressible file a kept scratch clone still shares. `store_bytes_still_shared` > 0, and `store_freed_bytes` is the compressed entry's blocks.
    - `with_check_c_failed_the_store_bytes_are_unmeasured`: write capabilities with `EvictionAccounting` disabled.
    - `an_unreadable_file_in_an_evicted_entry_is_unmeasured_not_counted`, injecting a `sharing` error through `Recording` in a library call.
  - In `tests/gc_disk_image.rs`, `store_freed_bytes_never_exceed_what_the_volume_regained`: a `HIVE_HOME` on the image, a compressed entry and an incompressible entry shared by a clone, backdated, then `gc`. `store_freed_bytes` ≤ the image's available-bytes increase.

  Run; expect failures.
- [ ] **Step 2: Implement** the receipt counts of STOREGC-7.1 that remain, using the ledger's `freed` and `still_shared` from `remove_counted`. Without `account_bytes`, put each bytes count in `unmeasured` with the reason.
- [ ] **Step 3: Run.** `cargo test`, plus `cargo test --test gc_disk_image`; expect pass.
- [ ] **Step 4: Commit.** `feat(gc): report the bytes store eviction freed and what clones still share`

_Requirements: STOREGC-7.1, STOREGC-7.2, STOREGC-7.3, STOREGC-7.4, STOREGC-9.3, STOREGC-10.9_

---

### Task 10: A killed gc leaves every store key whole or absent

**Files:**
- Modify: `src/seed/evict.rs` (cancel checks per design decision 13)
- Test: `tests/store_gc.rs`

**Reuse:** rung 2 — the kill test pattern of `killing_gc_at_any_point_never_costs_a_rebuild` (`tests/gc.rs:393`)

**Interfaces:**
- Consumes: `evict::run` (Task 3), `commit_eviction` (Task 3)
- Produces: none new

**Depends-on:** Task 9

- [ ] **Step 1: Write the failing tests:**
  - `killing_gc_during_store_eviction_never_leaves_a_partial_entry_listed_or_adoptable`: for a range of kill delays, SIGKILL a bare `gc` on a large backdated store. Then every row's directory holds every file, `store::read` lists only complete entries, a `seed` exits 0 with no `unit.refused`, and a fill adopts no partial directory.
  - `a_cancel_during_commit_records_what_was_renamed_and_releases_both_locks`: SIGINT, exit 130, `store.db` has no row for a renamed key, and `try_store_exclusive` then succeeds.

  Run; expect failures.
- [ ] **Step 2: Implement** cancel checks before each rename inside `commit_eviction`'s rename closure, the non-cancellable state lock, and the removal skip on cancel.
- [ ] **Step 3: Run.** `cargo test`; expect pass.
- [ ] **Step 4: Commit.** `fix(gc): keep store entries whole when gc is killed or cancelled mid-eviction`

_Requirements: STOREGC-9.2, STOREGC-10.4_

---

### Task 11: Tell agents and readers how the store is bounded

**Files:**
- Modify: `plugin/skills/hive/SKILL.md`, `README.md` (line 10 and the seed row at line 54), `tests/skill.rs` (lines 569-572)
- Test: `tests/skill.rs`

**Reuse:** rung 2 — the existing skill rows on `gc` (`plugin/skills/hive/SKILL.md:19,24,35-36,40,53-55,71`) and README lines 10 and 54

**Interfaces:**
- Consumes: count and finding names (Tasks 3 to 9)
- Produces: none in code

**Depends-on:** Task 9, Task 8, Task 7

- [ ] **Step 1: Write the failing test.** Change the README assertion in `tests/skill.rs` to the new sentence "Bare `cargo hive gc` keeps the store under 40 GB" and add an assertion that SKILL.md names `store` in the gc row. Run `cargo test --test skill`; expect failure.
- [ ] **Step 2: Edit SKILL.md.**
  - The gc lines say that bare `gc` also evicts from the store by age, toolchain and the 40 GB cap; that `gc --dry-run` previews it; and that `gc <repo>` leaves the store alone.
  - "Reading a receipt" names the `store_*` counts.
  - "Do not" adds "delete `~/.hive/store/` by hand".
  - Add a `cargo hive gc --json` command line.
  - Stay within 150 lines and the seven headings.
- [ ] **Step 3: Edit README** lines 10 and 54 to the new sentence. Run `cargo test --test skill` and `cargo test`; expect pass.
- [ ] **Step 4: Commit.** `docs(skill): teach agents that bare gc bounds the store`

_Requirements: STOREGC-8.1, STOREGC-8.2, STOREGC-10.11_

---

### Task 12: Dated notes in the specs this feature narrows

**Files:**
- Modify: `docs/specs/2026-11-02-store-seed/design.md` (decision 6), `docs/specs/2026-11-02-store-seed/requirements.md` (SEED-10.1), `docs/specs/2026-11-01-gc-plan-apply/requirements.md` (GCPLAN-9.1)

**Reuse:** rung 2 — `docs/specs/2026-11-02-store-seed/design.md` decisions 5 and 6

**Interfaces:**
- Consumes: none
- Produces: none

**Depends-on:** Task 9

- [ ] **Step 1: Edit.** Add to SEED design decision 6: "(2026-11-09, STOREGC: never modified; store eviction may remove a published directory whole.)" Add to SEED-10.1 and GCPLAN-9.1: "(2026-11-09: STOREGC adds store counts to bare `gc`; target-deletion counts are unchanged, STOREGC-10.1.)" Change no ID and no other words.
- [ ] **Step 2: Check.** Run the `audit-trace` skill over `docs/specs/`; expect no new finding.
- [ ] **Step 3: Commit.** `docs(specs): note where store GC narrows SEED and GCPLAN guards`

_Requirements: STOREGC-10.1, STOREGC-10.4_
