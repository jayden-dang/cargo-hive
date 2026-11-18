# Tasks: State upgrade

Delivered-by: PR #28, https://github.com/jayden-dang/cargo-hive/pull/28

> **For agentic workers:** after plan approval, pick one execute skill —
> `build-in-waves` (continuous dependency-aware scheduler), `build-by-story` (human-gated story review
> units), or `build-inline` (controller implements, no implementer subagents).
> The chosen skill writes `Execution-mode:`. Steps use checkbox (`- [ ]`) syntax
> for tracking.

Feature code: UPGRADE
Status: Approved
Approved: 2026-11-15, by the operator (Jayden), reply relayed by the conductor: "approve", clarified "Apply recommendations, then approve" (.skills/_pending-state-upgrade/auto/operator-reply.md)
Date: 2026-11-15
Execution-mode: continuous
Max-concurrency: auto
Requirements: ./requirements.md
Design: ./design.md

**Goal:** Every state file hive writes records its layout; hive reads each file 0.1.0 wrote as before, or stops on
it by name and leaves it untouched; committed v0.1.0 fixtures prove it in every gate.

**Architecture:** The state crate gains `Layout`, a peek that refuses a newer layout before the strict parse,
and a fix hint on `Malformed` (design §A). Each of the six persisted types then gains `layout` as its first field,
one crate per task (§B), followed by the narrower stops in seed, bench and store eviction (§E–§G). Last come the
v0.1.0 fixtures and their suite, the capture-test extension and the release steps (§H–§I).

**Tech Stack:** Rust (rustc/cargo 1.97.1, edition 2024) workspace of eight members, serde/serde_json (no
`preserve_order`), clap 4. Tests via `cargo test --workspace`: unit tests in `#[cfg(test)] mod tests`, integration
tests in `crates/cargo-hive/tests/*.rs` with `tests/common`.

## Global Constraints

The canonical source is `docs/standards/`, with sha256 prefixes taken 2026-11-15: INDEX.md `80cd41328c0f`,
code.md `62576fe2b798`, testing.md `9cdbc91192aa`, errors-logging.md `48f5baac3b52`. Commands, the task gate and
the layer order are in `docs/agents/project.md` (verify table). Spine: `docs/architecture/INDEX.md` `300186a11143`.
Every task inherits these:

- **Merge gate** (testing.md:10-16):
  - `cargo fmt --check`;
  - `cargo clippy --workspace --all-targets -- -D warnings`;
  - `cargo test --workspace`;
  - `cargo check --workspace --target x86_64-unknown-linux-gnu`. If that target is not installed, record "not run:
    target not installed" and never install it.

  Each task runs the project.md task gate for the members it changes.
- **Code rules** (code.md):
  - every walk, read and child has a bound;
  - `unsafe` only in the platform crate;
  - no new third-party dependency (none planned; the doctor→state edge is an internal workspace crate, layer 4
    → layer 2, so downward);
  - no requirement IDs in source, comments or test names;
  - every new module opens with `//!`.
- **Errors** (errors-logging.md): typed enums with a hand-written `Display`, and no `thiserror`/`anyhow`. Exit
  codes are 0 ok, 1 refusal or failure, 130 cancel.
- **Tests never touch** the real `~/.hive`, `~/.cargo`, `~/.rustup` or `~/.cache` (testing.md "Mocks and
  boundaries"). Scratch homes live under `/private/tmp/hive-it-*`, and capture.rs keeps its own
  `/private/tmp/hive-capture/*` dirs and isolated `CARGO_HOME` (capture.rs:14-19, :148-158).
- **Invariants:** ARCH-16 (state files only through `cargo-hive-state`, atomically, under its lock); ARCH-2 (no
  write outside the state root and target dirs).
- **Compat freeze:** apart from what a task names, nothing changes. No receipt field, count name, finding kind
  (other than `store.index_missing`, Task 7), exit code or file name changes. No existing assertion is weakened
  to pass.
- **Team:** Solo (project.md `## Team`), so tasks carry no assignee.

## File Structure

| Path | Responsibility in this plan |
|---|---|
| `crates/state/src/lib.rs` | `Layout`, `StateError::NewerLayout`, the per-file `recovery` table in `Malformed`'s `Display`, the layout peek in `read_json`/`read_json_limited`, and a config read with no peek |
| `crates/cargo/src/registry.rs` | `Registry.layout` |
| `crates/cargo/src/discover.rs` | `Snapshot.layout`, production literal :745 and test literals |
| `crates/cargo-hive/src/cli.rs` | Registry test literal (:1961) |
| `crates/doctor/Cargo.toml` | add `cargo-hive-state` (workspace path dependency) |
| `crates/doctor/src/lib.rs` | `Capabilities.layout`, literals :292 and tests |
| `crates/gc/src/plan_tier/record.rs` | `PlansFile.layout`; `PlansFile`, `Plan` and `PLANS_FILE` made `pub` |
| `crates/seed/src/store.rs` | `StoreFile.layout`, test literals |
| `crates/seed/src/choose.rs` | `StoreFile` test literal (:202) |
| `crates/seed/src/record.rs` | `SeedsFile.layout`; `write_capped`/`write` refuse a parse error and return `Result<(), StateError>` |
| `crates/seed/src/lib.rs` | `record_placed` drops the `Ok(Some)` arm |
| `crates/seed/src/evict.rs` | `index_missing` and the `store.index_missing` finding |
| `crates/cargo-hive/src/bench/mod.rs` | `store_lists_repo`, `bench.failed` with the store.db path, and a new `mod tests` |
| `crates/cargo-hive/tests/capture.rs` | layout strip in `read_normalized_file`, the `write_fixture_home` generator, `run_on_home_json`, and seed after upgrade on a third clone |
| `crates/cargo-hive/tests/fixtures/v0.1.0/home/` | committed 0.1.0 state root, store tree included (new) |
| `crates/cargo-hive/tests/fixtures/v0.1.0/PROVENANCE` | tag, commit and `rustc -vV` of the generating build (new) |
| `crates/cargo-hive/tests/fixtures/v0.1.1/` | the same for v0.1.1, the release this one upgrades from (new) |
| `crates/cargo-hive/tests/upgrade.rs` | fixture suite and refusal matrix (new) |
| `crates/cargo-hive/tests/seed.rs` | seeds.json and store.db refusals through `seed` |
| `crates/cargo-hive/tests/store_gc.rs` | store.db missing or unparseable through bare `gc` / `--dry-run` |
| `docs/agents/project.md` | release step 6 binary source, new step 7 "Fixtures", release-notes line (UPGRADE-5.8), and the dev-build `HIVE_HOME` rule (UPGRADE-6.2) |
| `docs/specs/2026-11-13-workspace-split/requirements.md` | SPLIT-4.3 struck through whole, superseded by UPGRADE-2.8 |
| `docs/specs/2026-11-09-store-gc/requirements.md` | STOREGC-1.6 amended with the UPGRADE-4.1 carve-out (operator 2026-11-15) |

---

### Task 1: The state crate reads and checks a layout

**Files:**
- Modify: `crates/state/src/lib.rs`. The read path is at lines 369-403; `StateError` and its `Display` are at
  20-70; `State::config` is at 268.
- Test: `crates/state/src/lib.rs` (unit tests)

**Reuse:** rung 2, which extends `read_json` / `read_json_limited` / `StateError` in crates/state/src/lib.rs:20-70,
:369-403. The atomic write is unchanged.

**Interfaces:**
- Consumes: none
- Produces:
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq)] pub struct Layout(u32)`:
    - `Serialize` writes a bare integer;
    - `Deserialize` refuses 0;
    - `Default` is `Layout::NEWEST`;
    - `pub const NEWEST: Layout = Layout(1)`;
    - `pub fn absent() -> Layout`, which returns `Layout(1)`, frozen.
  - A private `fn recovery(file_name: &str) -> &'static str`. It holds one entry per JSON state file, worded as
    the UPGRADE-3.8 table, and returns "fix or remove it by hand" for any other name, config.json included.
  - `StateError::Malformed { file: PathBuf, detail: String, on_read: bool }`. `Display` is
    "`<path>` is malformed (`<detail>`); `<recovery(file name)>`" when `on_read`, and "…; fix or remove it by
    hand" otherwise. The read-side sites (lib.rs:371, :394, :399) set `on_read: true`; `encode` (:360-366) sets
    `false`.
  - `StateError::NewerLayout { file: PathBuf, found: u64, newest: u32 }`. `Display`: "`<path>` records layout
    `<found>`, but this hive reads layouts up to `<newest>`; install a newer cargo-hive (`cargo install
    cargo-hive`) to read it".
  - `found` is `u64`, so a value above `u32::MAX` still reports `NewerLayout` (design review F4).

**Depends-on:** none

- [x] **Step 1: Write the failing unit tests** in the state crate's `mod tests`, each on a scratch root:
  - `a_file_without_a_layout_key_reads_as_layout_one`
  - `a_newer_layout_is_refused_as_newer_and_left_untouched` (bytes before == after)
  - `a_newer_layout_with_unknown_fields_is_still_reported_as_newer`
  - `layout_zero_is_malformed`
  - `a_top_level_array_is_malformed_not_newer` (`[2]`)
  - `an_over_cap_file_is_malformed_whatever_its_layout`
  - `config_with_a_layout_key_is_malformed_not_newer`
  - `a_current_layout_file_round_trips_to_identical_bytes`
  - `reading_never_writes_the_file` (mtime and bytes unchanged)
  - `each_state_file_refusal_ends_with_its_own_recovery`: table-driven, one row per file in the UPGRADE-3.8
    table, plus config.json keeping "fix or remove it by hand"
  - `an_encode_failure_never_suggests_removing_the_file`
  - grep the workspace tests for "fix or remove it by hand" and update any assertion on a state file's text
  - `a_missing_file_still_reads_as_default`
  - `an_unparseable_config_is_still_refused_as_malformed`

  Run `cargo test -p cargo-hive-state --lib`; expect a compile failure (no `Layout`, no `NewerLayout`).
- [x] **Step 2: Implement.**
  - A private `Peek { #[serde(default)] layout: Option<u64> }` is parsed from the same bytes before `T`, in both
    readers, after the over-cap check. It runs only when the first non-whitespace byte is `{`, so a top-level
    array never reaches serde's `visit_seq` and stays `Malformed`.
  - `> NEWEST` → `NewerLayout`; `Some(0)` → `Malformed`.
  - `State::config` reads through a private no-peek path.
  - `Malformed`'s `Display` appends `recovery(file.file_name())` only when `on_read`.

  Run `cargo test -p cargo-hive-state`; expect pass. Then run the project.md task gate for `cargo-hive-state`.
- [x] **Step 3: Commit.** `feat(state): record a layout and refuse files from a newer hive by name`

_Requirements: UPGRADE-1.2, UPGRADE-2.2, UPGRADE-2.4, UPGRADE-3.2, UPGRADE-3.3, UPGRADE-3.8, UPGRADE-3.9, UPGRADE-6.1, UPGRADE-7.8_

---

### Task 2: Registry and snapshot record their layout

**Files:**
- Modify: `crates/cargo/src/registry.rs` (struct at line 14)
- Modify: `crates/cargo/src/discover.rs` (struct at 265; literals at 745 and the tests at 778-904)
- Modify: `crates/cargo-hive/src/cli.rs` (the test literal at 1961 only)
- Test: `crates/cargo/src/registry.rs`, `crates/cargo/src/discover.rs` (unit tests)

**Reuse:** rung 2, which extends each type in place:

- `Registry` (crates/cargo/src/registry.rs:14);
- `Capabilities` (crates/doctor/src/lib.rs:120);
- `Snapshot` (crates/cargo/src/discover.rs:265);
- `PlansFile` (crates/gc/src/plan_tier/record.rs:112);
- `StoreFile` (crates/seed/src/store.rs:72);
- `SeedsFile` (crates/seed/src/record.rs:41).

**Interfaces:**
- Consumes: `cargo_hive_state::Layout` (Task 1)
- Produces: `Registry.layout: Layout` and `Snapshot.layout: Layout`. Each is the first field and carries
  `#[serde(default = "Layout::absent")]`.

**Depends-on:** Task 1

- [x] **Step 1: Write the failing tests** (table-driven over both types where the shape is the same):
  - `writes_layout_as_the_first_key`
  - `a_keyless_0_1_0_file_reads_and_its_first_write_only_adds_the_layout_line`, using a hand-copied 0.1.0 body
  - `an_unknown_key_other_than_layout_is_still_refused`

  Run `cargo test -p cargo-hive-cargo --lib`; expect a compile failure.
- [x] **Step 2: Implement** the field and update every literal listed in Files.
  `deny_unknown_fields` stays on `Registry`, `Repo`, `Snapshot`, `CachedMetadata` and `Resolved`. Run the
  project.md task gate for `cargo-hive-cargo`; expect pass.
- [x] **Step 3: Commit.** `feat(cargo): registry and snapshot record their layout`

_Requirements: UPGRADE-1.1, UPGRADE-7.2_

---

### Task 3: Capabilities records its layout and its refusal names doctor

**Files:**
- Modify: `crates/doctor/Cargo.toml` (add `cargo-hive-state = { workspace = true }`, as the other members declare
  it)
- Modify: `crates/doctor/src/lib.rs` (struct at line 120; literals at 292 and the tests at 1272-1345)
- Test: `crates/doctor/src/lib.rs` (unit tests); `crates/cargo-hive/tests/receipts.rs`

**Reuse:** rung 2, which reuses `emit_failure` (cli.rs:293) unchanged and the recovery table in §A.

**Interfaces:**
- Consumes: `Layout` and the recovery table (Task 1)
- Produces: `Capabilities.layout: Layout`, the first field, with `#[serde(default = "Layout::absent")]`

**Depends-on:** Task 1

- [x] **Step 1: Write the failing tests.**
  - Doctor unit test: `capabilities_write_layout_first`.
  - In `tests/receipts.rs`: `an_unparseable_capabilities_file_ends_status_with_a_refusal_naming_doctor`. It asserts
    exit 1, a receipt with an `error` finding whose detail holds the full path and `cargo hive doctor`, and the
    bytes unchanged.
  - Also in `tests/receipts.rs`: `doctor_rewrites_a_damaged_capabilities_file`. This is the only test here
    that runs a real `doctor`, as tests/doctor.rs does.

  Run `cargo test -p cargo-hive-doctor --lib` and `cargo test -p cargo-hive --test receipts`; expect failure.
- [x] **Step 2: Implement** the field and update the literals. Run the task gate for `cargo-hive-doctor`; expect
  pass.
- [x] **Step 3: Commit.** `feat(doctor): capabilities record their layout and a damaged file names doctor`

_Requirements: UPGRADE-1.1, UPGRADE-3.1, UPGRADE-3.8, UPGRADE-7.2, UPGRADE-7.6_

---

### Task 4: Plans record their layout

**Files:**
- Modify: `crates/gc/src/plan_tier/record.rs`. `PLANS_FILE` is at line 12 and `PlansFile` at 112; the test
  literal is at 292.
- Test: `crates/gc/src/plan_tier/record.rs` (unit tests)

**Reuse:** rung 2, which extends each type in place:

- `Registry` (crates/cargo/src/registry.rs:14);
- `Capabilities` (crates/doctor/src/lib.rs:120);
- `Snapshot` (crates/cargo/src/discover.rs:265);
- `PlansFile` (crates/gc/src/plan_tier/record.rs:112);
- `StoreFile` (crates/seed/src/store.rs:72);
- `SeedsFile` (crates/seed/src/record.rs:41).

**Interfaces:**
- Consumes: `Layout` (Task 1)
- Produces: `pub struct PlansFile { pub layout: Layout, pub plans: Vec<Plan> }`, with the fields `pub`, `pub
  struct Plan` and `pub const PLANS_FILE`.

**Depends-on:** Task 1

- [x] **Step 1: Write the failing tests**:
  - `plans_write_layout_first`;
  - `insert_and_consume_keep_every_other_field_as_before`;
  - `a_keyless_plans_file_reads_as_layout_one`.

  Run `cargo test -p cargo-hive-gc --lib plan_tier::record::`; expect a compile failure.
- [x] **Step 2: Implement.** Run the task gate for `cargo-hive-gc`; expect pass.
- [x] **Step 3: Commit.** `feat(gc): plans record their layout`

_Requirements: UPGRADE-1.1_

UPGRADE-7.1 was delivered by this task, then superseded by BUILDDIR-6.7 on 2026-11-17 (requirements.md).

---

### Task 5: The store index and the seed record record their layout

**Files:**
- Modify: `crates/seed/src/store.rs`. The struct is at line 72, and the test literals are at 1319-1594.
- Modify: `crates/seed/src/record.rs` (struct at line 41; test literal at 472)
- Modify: `crates/seed/src/choose.rs` (test literal at line 202)
- Test: `crates/seed/src/store.rs`, `crates/seed/src/record.rs` (unit tests)

**Reuse:** rung 2, which extends each type in place:

- `Registry` (crates/cargo/src/registry.rs:14);
- `Capabilities` (crates/doctor/src/lib.rs:120);
- `Snapshot` (crates/cargo/src/discover.rs:265);
- `PlansFile` (crates/gc/src/plan_tier/record.rs:112);
- `StoreFile` (crates/seed/src/store.rs:72);
- `SeedsFile` (crates/seed/src/record.rs:41).

**Interfaces:**
- Consumes: `Layout` (Task 1)
- Produces: `StoreFile.layout: Layout` and `SeedsFile.layout: Layout`. Each is the first field and carries
  `#[serde(default = "Layout::absent")]`.

**Depends-on:** Task 1

- [x] **Step 1: Write the failing tests**:
  - `store_and_seeds_write_layout_first`;
  - `a_keyless_store_db_reads_every_row_whose_entry_dir_exists` (the entry path scheme is unchanged);
  - `merge_and_eviction_commit_keep_every_other_field`.

  Run `cargo test -p cargo-hive-seed --lib`; expect a compile failure.
- [x] **Step 2: Implement** the fields, and update every literal in Files (and any further one the compiler
  names in the seed crate). Run the task gate for `cargo-hive-seed`; expect pass.
- [x] **Step 3: Commit.** `feat(seed): the store index and seed record record their layout`

_Requirements: UPGRADE-1.1, UPGRADE-1.5_

UPGRADE-7.1 was delivered by this task, then superseded by BUILDDIR-6.7 on 2026-11-17 (requirements.md).

---

### Task 6: seed refuses to replace an unreadable seed record

**Files:**
- Modify: `crates/seed/src/record.rs`. `write`/`write_capped` are at lines 46-72; the `replaced` helper and the
  test at about 606 are also here.
- Modify: `crates/seed/src/lib.rs` (`record_placed`, lines 476-488)
- Test: `crates/seed/src/record.rs` (unit tests), `crates/cargo-hive/tests/seed.rs`

**Reuse:** rung 2, which extends `record::write_capped` (crates/seed/src/record.rs:54-72). It reuses
`record_placed`'s `record.failed` path (seed lib.rs:476-488) and status's existing note (cli.rs:753-758).

**Interfaces:**
- Consumes: `SeedsFile` (Task 5)
- Produces: `pub fn write(...) -> Result<(), StateError>` and `pub fn write_capped(...) -> Result<(),
  StateError>`. The `replaced` helper is removed.

**Depends-on:** Task 5

- [x] **Step 1: Write the failing tests.**
  - The unit test at record.rs:~606 becomes `an_unparseable_record_file_is_refused_and_left_untouched`.
  - Two unit tests guard the pruning on a readable file: `a_readable_record_file_still_drops_gone_targets` and
    `a_readable_record_file_still_drops_oldest_to_fit`.
  - In `tests/seed.rs`, using `common::store_fixture::seeded_store`:
    `a_seed_with_an_unparseable_record_file_clones_reports_record_failed_and_exits_0`. It asserts the bytes are
    unchanged and the finding detail holds the seeds.json path.
  - An existing status test stays green, guarding the note.

  Run `cargo test -p cargo-hive-seed --lib record::` and `cargo test -p cargo-hive --test seed`; expect failure.
- [x] **Step 2: Implement.** On a read error, `write_capped` returns `Err(err)`. `record_placed` keeps only the
  `Ok(())` and `Err` arms. Run the task gate for `cargo-hive-seed`; expect pass.
- [x] **Step 3: Commit.** `fix(seed): never replace an unreadable seed record`

_Requirements: UPGRADE-3.4, UPGRADE-3.5, UPGRADE-7.3_

---

### Task 7: bare gc leaves the store when store.db is missing

**Files:**
- Modify: `crates/seed/src/evict.rs` (`run`, after `inventory` at line 79)
- Modify: `docs/specs/2026-11-09-store-gc/requirements.md` (STOREGC-1.6 at line 47)
- Test: `crates/cargo-hive/tests/store_gc.rs`

**Reuse:** rung 2, which extends `evict::run` (crates/seed/src/evict.rs:79) with `inventory`'s own `rowless` list.
Clearing that list sends the run down the existing empty-candidates return (evict.rs:248).

**Interfaces:**
- Consumes: `store::STORE_FILE`, `Inventory.rowless`
- Produces:
  - a private `fn index_missing(state: &State, inv: &Inventory) -> bool`;
  - finding kind `store.index_missing`, with path `<state root>/store.db`. Its detail is "`<path>` is missing
    while the store holds `<n>` entries, so no store entry was evicted; build a checkout of the repository with
    the current toolchain, then run `cargo hive seed` to rebuild it".

**Depends-on:** Task 5

- [x] **Step 1: Write the failing tests** in `tests/store_gc.rs`, using `common::store_fixture::seeded_store`, then
  removing `store.db`:
  - `bare_gc_with_store_db_missing_evicts_no_entry_and_names_it`, with every entry dir present after, and the
    count names equal to an empty-store run's;
  - `gc_dry_run_with_store_db_missing_previews_no_eviction`;
  - `the_target_tier_still_runs_when_store_db_is_missing`;
  - `bare_gc_with_an_unparseable_store_db_keeps_its_store_write_failed_stop`;
  - `a_seed_after_store_db_was_lost_adopts_equal_entries_and_rewrites_store_db`.
  - An existing store_gc age/toolchain/cap test stays green.

  Run `cargo test -p cargo-hive --test store_gc`; expect the first two to fail.
- [x] **Step 2: Implement.** If `index_missing`, push the finding and clear `inv.rowless`. Run the task gate for
  `cargo-hive-seed`; expect pass.
- [x] **Step 3: Amend STOREGC-1.6** in place. Append "except when store.db does not exist and the store holds
  entry directories (UPGRADE-4.1)", then a note "(amended 2026-11-15 by operator decision; see
  docs/specs/2026-11-15-state-upgrade/requirements.md)". The ID keeps its number. Run `audit-trace`; expect no
  new finding.
- [x] **Step 4: Commit.** `fix(seed): bare gc no longer empties a store whose index is missing`

_Requirements: UPGRADE-3.10, UPGRADE-4.1, UPGRADE-4.2, UPGRADE-4.3, UPGRADE-7.4_

---

### Task 8: bench names an unreadable store.db

**Files:**
- Modify: `crates/cargo-hive/src/bench/mod.rs` (lines 292-313)
- Test: `crates/cargo-hive/src/bench/mod.rs` (a new `#[cfg(test)] mod tests`, since none exists),
  `crates/cargo-hive/tests/bench.rs`

**Reuse:** rung 2, which extends bench/mod.rs:292-313 with the existing `fail_run` and `Finding`.

**Interfaces:**
- Consumes: `cargo_hive_seed::store::{read, STORE_FILE}`
- Produces: a private `fn store_lists_repo(state: &State, repo: &Path) -> Result<bool, StateError>`. On `Err`,
  `run` calls `fail_run` with `Finding { kind: "bench.failed", path: Some(<state root>/store.db), detail:
  err.to_string(), .. }`.

**Depends-on:** Task 1

- [x] **Step 1: Write the failing unit tests**:
  - `store_lists_repo_reports_an_unparseable_store_db_as_an_error`;
  - `store_lists_repo_is_false_for_a_readable_store_without_the_repo`.
  - The existing `tests/bench.rs` nothing-to-seed and seeds-unmeasured tests stay green.

  Run `cargo test -p cargo-hive --lib bench::`; expect a compile failure.
- [x] **Step 2: Implement.** Run the task gate for `cargo-hive`; expect pass.
- [x] **Step 3: Commit.** `fix(bench): an unreadable store index fails the run by name`

_Requirements: UPGRADE-3.6, UPGRADE-3.7, UPGRADE-7.5_

---

### Task 9: Commit the v0.1.0 fixture home and its generator

**Files:**
- Modify: `crates/cargo-hive/tests/capture.rs`. It gains `write_fixture_home`, `#[test] #[ignore]`, with its own
  step list. It reuses `ensure_golden`, `reset_work_and_home`, `capture_hive`, `cp_cr`, `repair_worktree` and
  `move_home`, but not `run_sequence`: that function's seed targets the already-built `wt`, which places nothing
  (seed lib.rs:462-464), and it makes no superseded unit.
- Create: `crates/cargo-hive/tests/fixtures/v0.1.0/home/` (written by the generator) and
  `crates/cargo-hive/tests/fixtures/v0.1.0/PROVENANCE`
- Create: `crates/cargo-hive/tests/fixtures/v0.1.1/home/` and `crates/cargo-hive/tests/fixtures/v0.1.1/PROVENANCE`
- Test: `crates/cargo-hive/tests/capture.rs`

**Reuse:**
- rung 2: `tests/common` (the `Hive` scratch-home helper, `store_fixture`) and capture.rs's scratch repo,
  registry and scenario steps (capture.rs:14-19, :395-454);
- rung 7: the new `tests/upgrade.rs`, which no existing test file does: none loads committed state.

**Interfaces:**
- Consumes these env vars:
  - `HIVE_FIXTURE_BIN`: absolute path of the binary to run.
  - `HIVE_FIXTURE_OUT`: absolute path of the `fixtures/v<release>` directory. The generator writes only
    `<out>/home/` (replaced whole) and `<out>/PROVENANCE`, and never touches other files there, such as the
    tracked `fixtures/v0.1.0/cli-tree.txt`.
  - `HIVE_FIXTURE_TAG` and `HIVE_FIXTURE_COMMIT`: written into PROVENANCE. The commit comes from the repo
    (`git rev-parse <tag>^{commit}`), since the archive has no `.git`.
  - PROVENANCE's `rustc -vV` is the generator's own `rustc -vV` output.
- Produces: `fixtures/v<release>/home/`, a whole state root. It holds `registry.json`, `capabilities.json`,
  `snapshot.json`, `plans.json` (≥1 plan), `store.db` with its `store/` tree, `seeds.json` (≥1 record),
  `config.json` and `receipts/<date>.jsonl`. `fixtures/v<release>/PROVENANCE` carries `tag`, `commit` and
  `rustc -vV`.

**Depends-on:** none

- [x] **Step 1: Write the generator.** It runs, with `HIVE_FIXTURE_BIN` and capture's isolated `CARGO_HOME`:
  - `ensure_golden`, `reset_work_and_home`, `doctor --json` and `repo add <main>`;
  - `common::cargo_fixture::backdate_all` on main's target, which backdates the whole target out of the GC window;
  - a version bump of main's workspace crate, so the rebuild supersedes a unit, and a rebuild of main;
  - `gc --plan --json`, which stores at least one plan;
  - a fresh secondary worktree `wt2` under the capture work dir (`repair_worktree`), and `seed --json` into
    `wt2`, which writes seeds.json.

  It then asserts that every file listed in Produces exists and is non-empty, and that no file under the home
  contains the running user's home directory path (`$HOME`) or `/Users/<login>`. The login is matched only in
  path form, so a short login cannot false-positive inside binary store entries. The home is committed to a public
  repo. It then copies the home to `HIVE_FIXTURE_OUT`. Run `cargo test -p cargo-hive --test capture` (ignored, so not run); expect it to compile.
- [x] **Step 2: Build 0.1.0 from its tag in a temporary directory and run the generator.**
  - `mkdir -p /private/tmp/hive-fixture-0.1.0/src && git archive v0.1.0 | tar -x -C /private/tmp/hive-fixture-0.1.0/src`
  - `CARGO_HOME=/private/tmp/hive-fixture-0.1.0/cargo-home CARGO_TARGET_DIR=/private/tmp/hive-fixture-0.1.0/target
    cargo build --locked --manifest-path /private/tmp/hive-fixture-0.1.0/src/Cargo.toml`. This needs network,
    since the isolated `CARGO_HOME` downloads every dependency into the temporary dir.
  - `HIVE_FIXTURE_BIN=/private/tmp/hive-fixture-0.1.0/target/debug/cargo-hive
    HIVE_FIXTURE_OUT="$PWD/crates/cargo-hive/tests/fixtures/v0.1.0" HIVE_FIXTURE_TAG=v0.1.0
    HIVE_FIXTURE_COMMIT="$(git rev-parse 'v0.1.0^{commit}')" cargo test -p cargo-hive --test capture --
    --ignored write_fixture_home`

  Expect: pass. `PROVENANCE` names `v0.1.0` and `fb8fabb95ae4354314d8ed1e5878ee32864a0eb2`. `du -sh` of the home
  is about 1 MB.
- [x] **Step 3: Do the same for v0.1.1**, the release this one upgrades from *(agent-decided)*. Use
  `/private/tmp/hive-fixture-0.1.1/`. The build is `cargo build --locked -p cargo-hive` with that tree's root
  `Cargo.toml` (0.1.1 is a workspace), and the output is
  `HIVE_FIXTURE_OUT="$PWD/crates/cargo-hive/tests/fixtures/v0.1.1"` with `HIVE_FIXTURE_TAG=v0.1.1`. Expect: pass.
- [x] **Step 4: Commit.** `test: commit the state a 0.1.0 hive writes, and the generator that made it`

_Requirements: UPGRADE-5.1, UPGRADE-6.2_

---

### Task 10: The fixture suite reads every 0.1.0 file as before

**Files:**
- Create: `crates/cargo-hive/tests/upgrade.rs`
- Test: `crates/cargo-hive/tests/upgrade.rs`

**Reuse:**
- rung 2: `tests/common` (the `Hive` scratch-home helper, `store_fixture`) and capture.rs's scratch repo,
  registry and scenario steps (capture.rs:14-19, :395-454);
- rung 7: the new `tests/upgrade.rs`, which no existing test file does: none loads committed state.

**Interfaces:**
- Consumes the six types and their constants:
  - `cargo_hive_cargo::registry::{Registry, FILE}`;
  - `cargo_hive_cargo::discover::{Snapshot, SNAPSHOT_FILE}`;
  - `cargo_hive_doctor::{Capabilities, CAPABILITIES_FILE}`;
  - `cargo_hive_gc::plan_tier::record::{PlansFile, PLANS_FILE}`;
  - `cargo_hive_seed::store::{StoreFile, STORE_FILE}`;
  - `cargo_hive_seed::record::{SeedsFile, SEEDS_FILE}`.

  It also uses `cargo_hive_core::receipt::Receipt` and `cargo_hive_state::State`.
- Produces: none

**Depends-on:** Task 2, Task 3, Task 4, Task 5, Task 9

- [x] **Step 1: Write the tests.**
  - `every_fixture_set_is_present_and_complete`: at least one `fixtures/v*/home/`, all six files plus
    `receipts/`, at least one plan and at least one seed record.
  - `each_state_file_reads_the_values_its_release_wrote`: parse with its type, re-serialize, strip `layout` on the
    written side only, and compare to the fixture's `serde_json::Value`.
  - `each_state_files_first_write_adds_only_the_layout_line`: write via `State::write` into a
    `/private/tmp/hive-it-*` copy. The bytes equal the fixture bytes with `  "layout": 1,` inserted after line 1.
  - `a_second_write_gives_identical_bytes`.
  - `appending_a_receipt_keeps_every_earlier_line_and_each_parses`. This is a separate test; schema is 2 or 3.
  - `command_json_output_carries_no_layout_key`: run `status --json` on a copy and assert that no object at any
    depth has a `layout` key. Exit 0 is not asserted, because the fixture's paths and pinned toolchain depend on
    the machine. It also runs `status --json` on a `common::Hive::ready()` home with a registered repo, which
    exits 0, so the check cannot pass on a refusal receipt alone.
  - `config_json_is_read_without_a_layout_key_and_refused_with_one`.

  Run `cargo test -p cargo-hive --test upgrade`; expect pass, since Tasks 2–5 and 9 have landed. Then break the
  absent-key default locally to confirm the suite fails, and revert.
- [x] **Step 2: Commit.** `test: the state a 0.1.0 hive wrote reads as before`

_Requirements: UPGRADE-1.3, UPGRADE-1.4, UPGRADE-2.1, UPGRADE-2.3, UPGRADE-2.5, UPGRADE-2.6, UPGRADE-2.7, UPGRADE-5.2, UPGRADE-5.3_

---

### Task 11: The refusal matrix over every state file

**Files:**
- Modify: `crates/cargo-hive/tests/upgrade.rs`
- Modify: `crates/cargo-hive/tests/seed.rs`
- Modify: `crates/cargo-hive/tests/store_gc.rs`
- Test: the same three files

**Reuse:**
- rung 2: `tests/common` (the `Hive` scratch-home helper, `store_fixture`) and capture.rs's scratch repo,
  registry and scenario steps (capture.rs:14-19, :395-454);
- rung 7: the new `tests/upgrade.rs`, which no existing test file does: none loads committed state.

**Interfaces:**
- Consumes: `common::Hive`, `common::store_fixture::seeded_store`, `common::registry_fixture::RegistryRepo`
- Produces: none

**Depends-on:** Task 3, Task 6, Task 7, Task 10

- [x] **Step 1: Write one table-driven test per harness.** Each row covers an unparseable copy and a
  `"layout": 2` copy, and asserts the stop below plus unchanged bytes:

  | File | Harness | Command | Stop |
  |---|---|---|---|
  | registry, capabilities, snapshot | `common::Hive` with a registered repo (upgrade.rs) | `status` | exit 1, error finding names the file and its UPGRADE-3.8 recovery |
  | plans.json | `common::Hive::ready()` (upgrade.rs), so the capability gate passes | `gc --plan` | exit 1, detail ends with the plans recovery |
  | seeds.json | `seeded_store` (seed.rs), with a fresh worktree per case so the seed places units | `seed` | exit 0, `record.failed` carrying the path |
  | store.db | `seeded_store` (seed.rs), with a fresh worktree per case | `seed` | exit 1, error names store.db and its recovery |
  | store.db | `seeded_store` (store_gc.rs) | bare `gc` and `gc --dry-run` | `store.write_failed`, target tier runs |

  Run `cargo test -p cargo-hive --test upgrade --test seed --test store_gc`; expect pass.
- [x] **Step 2: Commit.** `test: every state file refused by name and left untouched`

_Requirements: UPGRADE-5.4_

---

### Task 12: The capture test checks the upgrade, and SPLIT-4.3 is superseded

**Files:**
- Modify: `crates/cargo-hive/tests/capture.rs`. `read_normalized_file` is at lines 294-310, and
  `state_written_by_0_1_0_reads_unchanged` at 426-454.
- Modify: `docs/specs/2026-11-13-workspace-split/requirements.md` (SPLIT-4.3 at lines 129-132)
- Test: `crates/cargo-hive/tests/capture.rs`

**Reuse:** rung 2, which extends capture.rs (`read_normalized_file`, `state_written_by_0_1_0_reads_unchanged`) and
the release steps in docs/agents/project.md:130-168.

**Interfaces:**
- Consumes: env `HIVE_V010_BIN` (a tag-built 0.1.0), `HIVE_NEW_BIN`
- Produces: none

**Depends-on:** Task 2, Task 3, Task 4, Task 5, Task 9

- [x] **Step 1: Change the tests.**
  - `read_normalized_file` strips a top-level `layout` key from JSON state files only, never from receipt lines.
  - A new helper `run_on_home_json(bin, home, args) -> (i32, Value)` returns the exit code and the parsed
    receipt.
  - `state_written_by_0_1_0_reads_unchanged` makes a third `cp_cr` clone of the 0.1.0 home after its two-home
    comparison. It runs `seed --json` with the new binary there and asserts exit 0 and
    `counts.fill_entries_added == 0`. `assert_homes_equal` keeps comparing the first two homes, untouched by the
    seed.
  - It also makes a fourth and a fifth clone. It runs bare `gc --json` with the new binary on one and with
    `HIVE_V010_BIN` on the other, and asserts both receipts equal under `normalized_when_versions_differ`. The
    new receipt's `hive_version` must be the new binary's version (ROAD-14 example 3).

  Run `HIVE_V010_BIN=/private/tmp/hive-fixture-0.1.0/target/debug/cargo-hive cargo test -p cargo-hive --test capture
  -- --ignored`; expect pass.
- [x] **Step 2: Strike SPLIT-4.3 whole**, `--json` clause included, as `~~**SPLIT-4.3**~~ superseded by UPGRADE-2.8
  (docs/specs/2026-11-15-state-upgrade/requirements.md): state files and `--json` output are now compared by
  field names and values, not byte order (operator 2026-11-15)`. Keep its text struck through. Run `audit-trace`;
  expect no unknown citation.
- [x] **Step 3: Commit.** `test: the capture test reads 0.1.0 state after upgrade and seeds without refilling`

_Requirements: UPGRADE-2.8, UPGRADE-2.9, UPGRADE-5.7_

---

### Task 13: Release steps build old binaries from tags and add fixtures

**Files:**
- Modify: `docs/agents/project.md`. These are the release steps at lines 130-168; step 6's `HIVE_V010_BIN` is at
  158.

**Reuse:** rung 2, which extends capture.rs (`read_normalized_file`, `state_written_by_0_1_0_reads_unchanged`) and
the release steps in docs/agents/project.md:130-168.

**Interfaces:**
- Consumes: `write_fixture_home`, `HIVE_FIXTURE_BIN` and `HIVE_FIXTURE_OUT` (Task 9)
- Produces: release step 7, "Fixtures"

**Depends-on:** Task 9

- [x] **Step 1: Edit step 6.**
  - `HIVE_V010_BIN` comes from `/private/tmp/hive-fixture-0.1.0/target/debug/cargo-hive`, built by the Task 9
    Step 2 commands from tag `v0.1.0`.
  - The step says the build uses an isolated `CARGO_HOME` in that dir.
- [x] **Step 2: Add step 7, "Fixtures".**
  - Build the release binary from its own tag the same way, under `/private/tmp/hive-fixture-<release>/`.
  - Run `write_fixture_home` with the Task 9 Step 2 variables. `HIVE_FIXTURE_OUT` is the absolute
    `"$PWD/crates/cargo-hive/tests/fixtures/v<release>"`.
  - Run `cargo test -p cargo-hive --test upgrade`, then commit `test: v<release> state fixtures`.
  - Never delete an earlier `fixtures/v*`.
  - The step states that the generator uses capture.rs's isolated `CARGO_HOME`, never `~/.cargo`.
- [x] **Step 3: Add the release-notes line** to the tag step (step 4): the GitHub release notes for this release
  say that once it writes `registry.json`, `capabilities.json` or `snapshot.json` (snapshot is written by an
  ordinary `status` or `gc`), cargo-hive 0.1.x cannot read that file.
- [x] **Step 4: State the dev-build rule** in project.md's "Run locally" and verify sections, and in step 6:
  every run of a development build sets `HIVE_HOME` to a scratch dir under `/private/tmp`, never `~/.hive`.
- [x] **Step 5: Check.** Read the diff: steps 1–5 are unchanged except for step 4's release-notes line, and step 6
  differs only in the binary's source and the `HIVE_HOME` sentence.
  Commit `docs(release): build old binaries from tags and add each release's state fixtures`.

_Requirements: UPGRADE-5.5, UPGRADE-5.6, UPGRADE-5.8, UPGRADE-6.2, UPGRADE-7.7_
