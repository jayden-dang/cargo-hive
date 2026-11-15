# Tasks: Checkout GC

> **For agentic workers:** after plan approval, pick one execute skill —
> `build` (wave mode) (continuous dependency-aware scheduler), `build` (story mode) (human-gated story review
> units), or `build` (inline mode) (controller implements, no implementer subagents).
> The chosen skill writes `Execution-mode:`. Steps use checkbox (`- [ ]`) syntax
> for tracking.

Feature code: CHECKOUTGC
Status: Draft
Date: 2026-11-15
Execution-mode: unset
Max-concurrency: auto
Requirements: ./requirements.md
Design: ./design.md

**Goal:** `cargo hive gc --checkout <PATH>` deletes, in one run under the target's Cargo locks, one registered checkout's dead units and the loose debug objects no linked binary in its profile dirs references, and touches nothing outside that checkout's own targets.

**Architecture:** Two prefactors come first, a ranged read on the platform `Fs` (design "Ranged read") and the per-workspace resolutions in discovery (design "Ownership resolutions"). Beside them sits a pure Mach-O debug-map reader in `crates/gc` (design "Debug-map reader"). The new `gc::checkout` module composes `locks::acquire`, a fresh re-read, `liveness()`, `guard::check_entry` and `ledger::remove_counted`. It runs the dead-unit class, then the object class (design "Checkout GC run"), behind a new `--checkout` flag on `gc` (design "CLI mode"). The skill, the README and a klynt proof come last.

**Tech Stack:** Rust (rustc/cargo 1.97.1, edition 2024), a workspace of eight members, serde/serde_json, clap 4. Tests run through `cargo test --workspace`: unit tests sit in `#[cfg(test)] mod tests`, and integration tests in `crates/cargo-hive/tests/*.rs` use `tests/common`.

## Global Constraints

The canonical source is `docs/standards/`, with sha256 prefixes taken 2026-11-15:
- INDEX.md `80cd41328c0f`;
- code.md `62576fe2b798`;
- testing.md `9cdbc91192aa`;
- errors-logging.md `48f5baac3b52`.

Commands, the task gate and the layer order are in `docs/agents/project.md` (verify table, :76-100). The spine is `docs/architecture/INDEX.md` `300186a11143`. Every task inherits these:

- **Merge gate** (testing.md):
  - `cargo fmt --check`;
  - `cargo clippy --workspace --all-targets -- -D warnings`;
  - `cargo test --workspace`;
  - `cargo check --workspace --target x86_64-unknown-linux-gnu`. If that target is not installed, record "not run: target not installed" and never install it.

  Each task runs the project.md task gate for the members it changes.
- **Code rules** (code.md):
  - every walk, read and child has a bound, and hitting one is an error, never a silent skip;
  - `unsafe`, `libc` and `cfg(target_os)` only in `crates/platform`;
  - **no new third-party dependency** (none planned);
  - no requirement IDs in source, comments or test names;
  - every new module opens with `//!`.
- **Errors** (errors-logging.md):
  - typed enums with a hand-written `Display`, and no `thiserror`/`anyhow`;
  - expected refusals are data;
  - exit codes are 0 ok, 1 refusal or failure, 2 argument error or unsupported OS, 130 cancel.
- **Tests never touch** the real `~/.hive`, `~/.cargo`, `~/.rustup` or `~/.cache`. Scratch homes live under `/private/tmp/hive-it-*` (`Hive::ready`). No test or proof runs `cargo hive gc` against the owner's checkouts.
- **Invariants:** ARCH-1, 2, 3, 4, 8, 9, 10, 11, 14, 15, 16, 17, 18, 19 (design `Respects:` lines).
- **No persisted format:**
  - no new state file and no new field in one;
  - no reference cache;
  - the receipt stays schema 2.
- **Compat freeze:** apart from what a task names, nothing changes. Repo-wide `gc`, `gc --dry-run`, `gc --plan`, `apply` and every other command keep their receipts, count names, finding kinds, exit codes and deletions. No existing assertion is weakened to pass.
- **Public text** (skill, README) names no external comparison tool.
- **Team:** Solo (project.md `## Team`), so tasks carry no assignee.

## File Structure

| Path | Responsibility in this plan |
|---|---|
| `crates/platform/src/lib.rs` | `Fs::read_at`, a trait method with a default body over `Fs::read` |
| `crates/platform/src/apfs.rs` | `Apfs::read_at`, a positioned read, with unit tests |
| `crates/core/src/caps.rs` | `MACHO_CMDS_CAP`, `MACHO_SYMTAB_CAP`, `MACHO_STRTAB_CAP`, `FAT_ARCH_CAP` |
| `crates/gc/src/debug_map.rs` | the pure Mach-O debug-map reader: `read`, `DebugMap`, `MapError` (new) |
| `crates/gc/src/checkout.rs` | the checkout GC run: `run`, `CheckoutInput`, `CheckoutReport`, `Counts` (new) |
| `crates/gc/src/lib.rs` | `pub mod checkout; pub mod debug_map;` |
| `crates/cargo/src/discover.rs` | `WorkspaceResolution`, `TargetScan.workspaces`, `resolvers_of`, `failed_workspaces` |
| `crates/cargo-hive/src/cli.rs` | the `Gc.checkout` arg and reworded `Gc` doc, the dispatch, `checkout_cmd`, `put_checkout_counts`, and `find_checkout` (extracted from `locate_checkout`) |
| `crates/cargo-hive/tests/common/recording_fs.rs` | `Recording::read_at` (logged) and the `fail_read_at(path)` hook |
| `crates/cargo-hive/tests/checkout_gc.rs` | integration and library tests for `gc --checkout` (new) |
| `crates/cargo-hive/tests/cli_tree.rs` | the current-tree assertion, plus the diff from v0.1.0 confined to the `gc` block and the root-list `gc` line |
| `crates/cargo-hive/tests/fixtures/current/cli-tree.txt` | today's command tree (new) |
| `crates/cargo-hive/tests/skill.rs` | `Parsed::Gc { dry_run, plan, checkout }`, and the documented-commands combos |
| `plugin/skills/hive/SKILL.md` | the "Keep a reused worktree fast" section, a Reading-table row, and the "Free disk" line |
| `skills/hive/eval.json`, `skills/hive/TESTS.md` | the trigger and receipt-reading evals for the new section |
| `README.md` | the `gc --checkout` subsection |
| `.skills/verify/2026-11-15-checkout-gc/PLAN.md` | the klynt proof record (git-ignored evidence; no tracked file) |

Retrieval (load-subgraph, run once on this map):
- **blast_radius.** SPLIT owns every `crates/` path. UPGRADE also owns `crates/cargo/src/discover.rs` and `crates/cargo-hive/src/cli.rs`. CORE, GCPLAN, SEED, SKILL, SKILLPASS and STOREGC own `plugin/skills/hive/SKILL.md`. GCPLAN, SEED, SKILL and SKILLPASS own `skills/hive/{eval.json,TESTS.md}`. AUTOGC, CORE, EXPLAIN, SEED, SKILL, SKILLPASS, SPLIT and STOREGC own `README.md`.

  All of these features are Shipped or Implemented, so no Draft plan collides on a file. Their guards bind through CHECKOUTGC-6.x and 5.4.
- **cluster(CHECKOUTGC).** No members: CHECKOUTGC had no OWNS before this file. owns_coverage is 12/13.

---

### Task 1: The platform `Fs` reads a byte range

**Files:**
- Modify: `crates/platform/src/lib.rs` (trait `Fs`; the method list is at lines 303-338)
- Modify: `crates/platform/src/apfs.rs` (`impl Fs for Apfs`, line 47)
- Modify: `crates/cargo-hive/tests/common/recording_fs.rs` (`impl Fs for Recording`, line 234; hooks near 120-184)
- Test: `crates/platform/src/apfs.rs` (unit tests)

**Reuse:** rung 3. `std::os::unix::fs::FileExt::read_at` inside `Apfs`. The trait default goes through the existing `Fs::read`.

**Interfaces:**
- Consumes: none.
- Produces: `fn read_at(&self, path: &Path, offset: u64, len: usize) -> io::Result<Vec<u8>>` on `Fs`. It returns up to `len` bytes, fewer at end of file, and an empty vector past it. The default slices `self.read(path)`. `Recording::fail_read_at(path)` makes the next `read_at` of `path` fail.

**Depends-on:** none

- [ ] **Step 1: Write the failing tests** (table-driven over offset and length) in `apfs.rs`:
  - `read_at_returns_the_requested_range`;
  - `read_at_is_short_at_end_of_file`;
  - `read_at_past_end_of_file_is_empty`;
  - `the_default_read_at_matches_apfs_on_the_same_file`.

  Run `cargo test -p cargo-hive-platform --lib`; expect a compile failure (no `read_at`).
- [ ] **Step 2: Implement** the trait method with its default, the `Apfs` override, and the `Recording` override, which logs `read_at <path> <offset> <len>` and honors `fail_read_at`. Run the project.md task gate for `cargo-hive-platform`; expect pass.
- [ ] **Step 3: Commit.** `feat(platform): read a byte range of a file through Fs`

_Requirements: CHECKOUTGC-2.10_

---

### Task 2: A pure reader lists the loose objects a binary's debug map names

**Files:**
- Create: `crates/gc/src/debug_map.rs`
- Modify: `crates/gc/src/lib.rs` (add `pub mod debug_map;`)
- Modify: `crates/core/src/caps.rs` (lines 4-6 hold the existing caps)
- Test: `crates/gc/src/debug_map.rs` (unit tests on hand-built byte images, served by a local fake `Fs`)

**Reuse:** none — new code (rung 7). There is no Mach-O reader in the workspace or `Cargo.lock`, a dependency needs the owner (docs/standards/code.md:16), and `nm` is a process (ARCH-17).

**Interfaces:**
- Consumes: `Fs::read_at` (Task 1).
- Produces:
  - `pub struct DebugMap { pub loose: BTreeSet<String>, pub archive_entries: u64 }`.
  - `pub fn read(fs: &dyn Fs, path: &Path) -> Result<Option<DebugMap>, MapError>`. It gives `Ok(None)` for no Mach-O magic, for `MH_OBJECT` and for `MH_DSYM`.
  - `pub enum MapError { Read(io::Error), Truncated(&'static str), Bound(&'static str, u64), UnknownSlice(u32), Unsupported(&'static str) }`, with `Display`.
  - In `cargo_hive_core::caps`: `MACHO_CMDS_CAP: u64 = 4 << 20`, `MACHO_SYMTAB_CAP: u64 = 256 << 20`, `MACHO_STRTAB_CAP: u64 = 512 << 20`, `FAT_ARCH_CAP: u32 = 8`.

**Depends-on:** Task 1

- [ ] **Step 1: Write the failing tests.** Byte images come from a small in-test builder of `mach_header_64`, `LC_SYMTAB`, `nlist_64` and a string table, laid out as in `.skills/research/2026-11-15-macho-n-oso.md` (main checkout). Tests:
  - `a_thin_executable_yields_its_loose_object_names`;
  - `archive_entries_name_no_loose_object`;
  - `a_path_with_spaces_keeps_its_whole_file_name`;
  - `object_and_dsym_files_are_not_linked_binaries`;
  - `a_file_without_mach_o_magic_is_not_a_binary`;
  - `a_binary_with_no_symtab_has_an_empty_map`;
  - `fat_and_fat64_slices_are_read_relative_to_the_slice`;
  - `a_32_bit_or_big_endian_thin_header_is_unsupported`;
  - `an_unknown_fat_slice_is_an_error`;
  - `nfat_arch_of_zero_or_past_the_cap_is_unsupported`;
  - `a_magic_with_a_short_header_is_truncated`;
  - `cmdsize_not_a_multiple_of_8_or_past_sizeofcmds_is_truncated`;
  - `an_n_strx_past_the_string_table_is_truncated`;
  - a boundary table for each cap (exactly at the cap reads; one past is `Bound`).

  Run `cargo test -p cargo-hive-gc --lib debug_map::`; expect a compile failure.
- [ ] **Step 2: Implement** `debug_map.rs`, with its `//!` contract, and the four caps. Every read goes through `fs.read_at`, and one binary's tables are held at a time. Run the project.md task gate for `cargo-hive-gc`, with `cargo-hive-core` escalated per the gate; expect pass.
- [ ] **Step 3: Commit.** `feat(gc): read the loose objects a Mach-O debug map names`

_Requirements: CHECKOUTGC-2.3, CHECKOUTGC-2.4, CHECKOUTGC-2.7_

---

### Task 3: Discovery keeps every workspace's resolution

**Files:**
- Modify: `crates/cargo/src/discover.rs`. `TargetScan` is at line 518 and built only at line 652; `resolve_workspaces` is at 425; `owner_checkout` is at 509.
- Test: `crates/cargo/src/discover.rs` (unit tests); `crates/cargo-hive/tests/checkout_gc.rs` (library test over real fixtures)

**Reuse:** rung 2. It extends `discover::targets` (crates/cargo/src/discover.rs:530-660) and its private `resolve_workspaces` (:425), whose results it already computes and drops.

**Interfaces:**
- Consumes: none.
- Produces:
  - `pub struct WorkspaceResolution { pub dir: PathBuf, pub checkout: PathBuf, pub result: Result<Resolved, String> }`, where `checkout` is the deepest live checkout containing `dir`.
  - `TargetScan.workspaces: Vec<WorkspaceResolution>`.
  - `pub fn resolvers_of(scan: &TargetScan, target: &Path) -> Vec<PathBuf>`: the distinct checkouts whose `target_directory` or `build_directory` equals the canonical `target`.
  - `pub fn failed_workspaces(scan: &TargetScan) -> Vec<(PathBuf, String)>`: every failure.

**Depends-on:** none

- [ ] **Step 1: Write the failing tests.**
  - Unit tests:
    - `resolvers_of_names_every_checkout_that_resolves_to_a_target`;
    - `failed_workspaces_lists_every_failure_not_only_the_first`;
    - `existing_owner_and_failure_are_unchanged`.
  - Library test in `checkout_gc.rs` (`RegistryRepo`, two worktrees sharing a target through `.cargo/config.toml` `build.target-dir`, and one worktree whose `Cargo.toml` does not parse): `two_worktrees_sharing_a_config_target_dir_both_resolve_to_it`.

  Run `cargo test -p cargo-hive-cargo --lib discover::`; expect a compile failure.
- [ ] **Step 2: Implement** the field, the struct and the two helpers. `owner`, `failure`, `protected_owner` and `Scope` stay as they are. Run the project.md task gate for `cargo-hive-cargo`; expect pass, with every existing discover and plan test green.
- [ ] **Step 3: Commit.** `feat(cargo): keep each workspace's metadata resolution in the target scan`

_Requirements: CHECKOUTGC-3.1, CHECKOUTGC-3.2, CHECKOUTGC-3.5_

---

### Task 4: `gc --checkout` deletes one checkout's dead units in one run

**Files:**
- Create: `crates/gc/src/checkout.rs`
- Modify: `crates/gc/src/lib.rs` (add `pub mod checkout;`)
- Modify: `crates/cargo-hive/src/cli.rs`. The pieces:
  - `Command::Gc` at lines 44-58 gets its new arg and reworded doc;
  - the dispatch at 192-202;
  - `checkout_cmd` and `put_checkout_counts` go beside `gc_cmd` (914-1130);
  - `find_checkout` is extracted from `locate_checkout` (1569-1608), and `locate_checkout` then calls it.
- Test: `crates/gc/src/checkout.rs` (inline unit tests with a local fake `Fs`); `crates/cargo-hive/tests/checkout_gc.rs`

**Reuse:** rung 2:
- `locks::acquire(fs, target, true)` (crates/core/src/locks.rs:43);
- `target::identify_bounded` and `fingerprint::read_profile` (the fresh read, as `recheck`, apply.rs:719-800, does);
- `liveness()` (crates/gc/src/liveness.rs:77);
- `guard::check_entry` with `Expect::Entry` (guard.rs:24);
- `ledger::{Ledger, remove_counted}` (crates/core/src/ledger/account.rs:44).

The per-unit deletion order follows apply's `removal_order` rule (apply.rs:927). On the CLI side it reuses:
- the `gc_cmd` flow (cli.rs:914-1130);
- `cargo_hive_state::window_days`, refused like `gc --plan` (build.rs:191);
- the first half of `locate_checkout` (cli.rs:1569-1608), extracted.

**Interfaces:**
- Consumes: `resolvers_of`, `failed_workspaces`, `TargetScan.workspaces` (Task 3).
- Produces:
  - `gc::checkout::run(input: &CheckoutInput, cancel: &dyn Fn() -> bool) -> CheckoutReport`, with the `pub` fields given in design "Checkout GC run":
    - `CheckoutInput { fs, scan, checkout, nested, now, window, account_bytes }`;
    - `CheckoutReport { per_target, totals, findings, unmeasured, unmeasured_more, cancelled, failed }`;
    - `Counts { units_deleted, unit_files_deleted, objects_deleted, objects_referenced, binaries_read, targets_shared, targets_outside, bytes, skips }`.
  - `find_checkout(reg, cfg, path) -> Result<(Repo, Checkout), Refusal>`.
  - The receipt command `"gc --checkout"`.
  - Count names `units_deleted`, `unit_files_deleted`, `freed_bytes` and `deleted_bytes_still_shared`, each also as `<name>:<target>`.
  - Finding kinds `gc.undecided`, `gc.delete_failed`, `refusal` and `error`.

**Depends-on:** Task 3

- [ ] **Step 1: Write the failing integration tests** in `checkout_gc.rs` (`Hive::ready`, `RegistryRepo`, `cargo_json(…, ["test", "--no-run"])`, `backdate_all`):
  - `a_checkouts_dead_units_are_deleted_in_one_run_with_no_plan_stored`;
  - `units_compiled_inside_the_window_are_kept_and_the_next_build_recompiles_none`, which reruns the same cargo command and asserts every unit is `fresh`;
  - `an_undecided_target_deletes_no_unit_and_names_each_reason_with_what_clears_it`;
  - `an_out_of_range_window_is_refused_with_exit_1`;
  - `checkout_with_repos_plan_or_dry_run_is_an_argument_error_with_exit_2_and_no_receipt` (table);
  - `a_missing_capability_refuses_with_exit_1`;
  - `without_eviction_accounting_bytes_are_unmeasured_but_units_are_deleted`;
  - `the_receipt_is_schema_2_with_command_gc_checkout_and_only_that_checkout_in_scope`;
  - `an_interrupted_checkout_gc_exits_130_with_a_receipt` (the `gc.rs:485` pattern);
  - `a_failed_deletion_exits_1`.

  Unit tests in `checkout.rs`:
  - `unit_entries_are_deleted_dep_info_first_and_fingerprint_dir_last`;
  - `a_unit_without_dep_info_loses_its_fingerprint_dir_first`;
  - `a_unit_with_only_a_fingerprint_dir_is_left`;
  - `a_dsym_dir_of_a_dead_unit_is_deleted`;
  - `an_entry_swapped_after_its_stat_is_refused`;
  - `a_stop_after_each_position_in_a_unit_leaves_no_unit_fresh_with_a_file_missing`.

  Run `cargo test -p cargo-hive --test checkout_gc`; expect a compile failure (no `--checkout`).
- [ ] **Step 2: Implement** `checkout.rs` up to the dead-unit class and `checkout_cmd` as in design "CLI mode", steps 1–7. Target selection takes only targets the checkout alone resolves to, inside its own directory and not inside a nested checkout. Bind the whole `Locked` value for the life of each target's deletions. Run the project.md task gate for `cargo-hive-gc`, then `cargo-hive`; expect pass.
- [ ] **Step 3: Run** the seed suite (`cargo test -p cargo-hive --test seed`) to confirm the `find_checkout` extraction leaves seed's refusals unchanged; expect pass.
- [ ] **Step 4: Commit.** `feat(gc): delete one checkout's dead units with gc --checkout`

_Requirements: CHECKOUTGC-1.1, CHECKOUTGC-1.2, CHECKOUTGC-1.3, CHECKOUTGC-1.4, CHECKOUTGC-1.5, CHECKOUTGC-1.6, CHECKOUTGC-1.7, CHECKOUTGC-1.8, CHECKOUTGC-1.9, CHECKOUTGC-1.10, CHECKOUTGC-1.11, CHECKOUTGC-1.12, CHECKOUTGC-7.2, CHECKOUTGC-7.3_

---

### Task 5: `gc --checkout` touches only the named checkout

**Files:**
- Modify: `crates/gc/src/checkout.rs` (target selection)
- Modify: `crates/cargo-hive/src/cli.rs` (`checkout_cmd`: refusal wording, `nested`, `ignored_vars` notes)
- Test: `crates/cargo-hive/tests/checkout_gc.rs`

**Reuse:** rung 2. It extends `gc::checkout` (Task 4) and `find_checkout`. `owning_repo` (cli.rs:522) is used only to word the refusal for a directory inside a checkout.

**Interfaces:**
- Consumes: `gc::checkout::run`, `find_checkout` (Task 4); `resolvers_of`, `failed_workspaces` (Task 3).
- Produces:
  - finding kinds `gc.shared_target`, `gc.outside_checkout` and `ownership.unresolved`;
  - counts `targets_shared` and `targets_outside`;
  - refusal details `"<path> is not a checkout of <main>; name the checkout's root directory"` and `"not a registered repository; run cargo hive repo add first"`.

**Depends-on:** Task 4

- [ ] **Step 1: Write the failing integration tests:**
  - `gc_on_a_worktree_leaves_main_and_every_other_lane_byte_for_byte_unchanged`, which compares the `tree()` of every other target before and after;
  - `a_target_two_worktrees_resolve_to_is_left_and_named`;
  - `a_target_dir_outside_the_checkout_is_left_and_named`;
  - `gc_on_main_leaves_the_worktrees_nested_under_it`;
  - `a_broken_manifest_in_any_worktree_blocks_every_deletion_and_exits_1`;
  - `an_unregistered_path_is_refused_with_repo_add` and `a_directory_inside_a_checkout_is_refused` (table);
  - `only_the_receipt_line_and_snapshot_change_under_the_state_root`, which diffs the state root and the store before and after.

  Run `cargo test -p cargo-hive --test checkout_gc`; expect the new tests to fail.
- [ ] **Step 2: Implement** the shared, outside and unresolved branches of target selection, the refusal details, and the `ignored_vars` notes (as cli.rs:886). Run the project.md task gate for `cargo-hive-gc`, then `cargo-hive`; expect pass.
- [ ] **Step 3: Commit.** `feat(gc): gc --checkout refuses shared, outside and unresolved targets`

_Requirements: CHECKOUTGC-3.1, CHECKOUTGC-3.2, CHECKOUTGC-3.3, CHECKOUTGC-3.4, CHECKOUTGC-3.5, CHECKOUTGC-3.6, CHECKOUTGC-3.7_

---

### Task 6: A target a build holds is skipped and named

**Files:**
- Modify: `crates/gc/src/checkout.rs` (the acquire branch)
- Test: `crates/cargo-hive/tests/checkout_gc.rs`

**Reuse:** rung 2. `locks::acquire(fs, target, true)` and `Reason::count_name` (crates/core/src/locks.rs:19-43). The held-lock test pattern is `plan_apply.rs:393-408` with `hold_lock_child` (tests/common/mod.rs:235).

**Interfaces:**
- Consumes: `gc::checkout::run` (Task 4).
- Produces:
  - the `gc.skipped` finding (path = target, detail = reason);
  - the `targets_skipped_held`, `targets_skipped_lock_unopenable`, `targets_skipped_lock_unsupported`, `targets_skipped_lock_missing` and `targets_skipped_profile_set` counts;
  - the `gc.target_failed` finding.

**Depends-on:** Task 4

- [ ] **Step 1: Write the failing tests:**
  - `a_held_build_lock_skips_the_target_names_it_and_deletes_nothing`, a table over `.cargo-build-lock`, `.cargo-lock` and `.cargo-artifact-lock`;
  - `a_profile_with_fingerprints_and_no_lock_file_is_skipped_as_missing`;
  - `a_profile_listing_failure_while_locking_exits_1`;
  - `the_locks_stay_held_until_the_last_deletion`, in which a `Recording` hook at the last `remove_file` probes `try-lock` from another process and expects held.

  Run `cargo test -p cargo-hive --test checkout_gc`; expect the new tests to fail.
- [ ] **Step 2: Implement** the skip and failure findings and counts, and the per-target lock lifetime check. Run the project.md task gate for `cargo-hive-gc`, then `cargo-hive`; expect pass.
- [ ] **Step 3: Commit.** `feat(gc): gc --checkout skips and names a target a build holds`

_Requirements: CHECKOUTGC-4.1, CHECKOUTGC-4.2, CHECKOUTGC-4.3, CHECKOUTGC-4.4_

---

### Task 7: `gc --checkout` deletes the debug objects no linked binary references

**Files:**
- Modify: `crates/gc/src/checkout.rs` (the object phase)
- Modify: `crates/cargo-hive/src/cli.rs` (`put_checkout_counts`: the object counts)
- Test: `crates/gc/src/checkout.rs` (unit tests); `crates/cargo-hive/tests/checkout_gc.rs`

**Reuse:** rung 2. It extends `gc::checkout` (Task 4) with `debug_map::read` (Task 2), `Fs::read_dir_bounded` with `DEPS_CAP` and `CRATE_DIR_CAP` (crates/core/src/caps.rs:4-6), `guard::check_entry` with `Expect::Entry { kind: File, dev, ino }` and `ledger::remove_counted`.

**Interfaces:**
- Consumes: `debug_map::{read, DebugMap, MapError}` (Task 2); `gc::checkout::run` (Task 4); `Recording::fail_read_at` (Task 1).
- Produces: the counts `objects_deleted`, `objects_referenced` and `binaries_read`, each also as `<name>:<target>`, and the finding kind `gc.objects_undecided`.

**Depends-on:** Task 2, Task 4

- [ ] **Step 1: Write the failing tests.** Integration tests on real `cargo test --no-run` builds, with a bin, an example, a build script and a proc-macro:
  - `unreferenced_objects_are_deleted_and_every_object_a_binary_names_is_kept`;
  - `a_test_binary_that_was_not_relinked_still_keeps_its_objects`, which rebuilds after an edit that relinks one binary and leaves another;
  - `objects_named_only_by_a_dead_units_binary_go_in_the_same_run`;
  - `an_undecided_target_still_loses_its_unreferenced_objects`;
  - `a_profile_with_an_unsupported_fingerprint_format_keeps_every_object`;
  - `the_receipt_counts_objects_deleted_kept_and_binaries_read`;
  - `objects_are_read_with_ranged_reads_and_no_child_process`, which checks the `Recording` log for `read_at` and no full read of a binary;
  - `a_binary_whose_read_fails_keeps_every_object_in_its_profile`, which uses `fail_read_at`.

  Unit tests:
  - `a_profile_with_binaries_and_objects_but_no_debug_map_keeps_every_object`;
  - `a_missing_examples_or_build_dir_reads_as_empty`;
  - `a_deps_listing_past_its_cap_keeps_every_object`;
  - `an_object_swapped_for_a_directory_is_refused`;
  - `only_deps_rcgu_o_files_are_candidates`.

  Run `cargo test -p cargo-hive --test checkout_gc`; expect the new tests to fail.
- [ ] **Step 2: Implement** the object phase as in design "Checkout GC run" step 2, "Objects": the layout and format check, the listings, the reads, the no-debug-map rule and the deletion. Run the project.md task gate for `cargo-hive-gc`, then `cargo-hive`; expect pass.
- [ ] **Step 3: Commit.** `feat(gc): gc --checkout deletes debug objects no linked binary references`

_Requirements: CHECKOUTGC-2.1, CHECKOUTGC-2.2, CHECKOUTGC-2.5, CHECKOUTGC-2.6, CHECKOUTGC-2.8, CHECKOUTGC-2.9, CHECKOUTGC-2.10, CHECKOUTGC-2.11_

---

### Task 8: The other commands and the CLI tree stay as they were

**Files:**
- Create: `crates/cargo-hive/tests/fixtures/current/cli-tree.txt`
- Modify: `crates/cargo-hive/tests/cli_tree.rs`. The fixture test is at lines 34-51, and the v0.1.0 fixture's root command list is at line 11.
- Test: `crates/cargo-hive/tests/cli_tree.rs`; the existing suites `tests/gc.rs`, `tests/plan_apply.rs`, `tests/plan_tier.rs`, `tests/liveness*.rs`, `tests/seed.rs`, `tests/receipts.rs` and `tests/upgrade.rs`

**Reuse:** rung 2. These are the existing tests that pin each guard:
- `tests/gc.rs`, `plan_apply.rs` and `plan_tier.rs` (6.1);
- the GCPLAN-9.2 tests (6.2);
- `liveness*.rs` and `seed.rs:1520` (6.3);
- `locks.rs` unit tests (6.4);
- `seed` record tests (record.rs:813-817, 906; 6.5);
- `cli_tree.rs` (6.6);
- the Linux `cargo check` in the merge gate (6.7).

**Interfaces:**
- Consumes: the `Gc.checkout` arg (Task 4).
- Produces: `the_command_tree_matches_the_current_fixture`, and `the_tree_differs_from_0_1_0_only_in_gc` (the `gc` block plus the root-list `gc` line).

**Depends-on:** Task 4

- [ ] **Step 1: Write the failing test** `the_tree_differs_from_0_1_0_only_in_gc`. It diffs the dump against `fixtures/v0.1.0/cli-tree.txt` and allows changed lines only inside the `gc` block and on the root list's `gc` line. Regenerate `fixtures/current/cli-tree.txt` with `HIVE_WRITE_CLI_TREE`, and read the diff by eye. Run `cargo test -p cargo-hive --test cli_tree`; expect the current-fixture test to pass and the diff test to fail until it is written.
- [ ] **Step 2: Implement** the two assertions, leaving the v0.1.0 fixture unchanged. Run `cargo test --workspace` and `cargo check --workspace --target x86_64-unknown-linux-gnu`; expect every existing suite green with no assertion edited.
- [ ] **Step 3: Commit.** `test(cli): pin the current command tree and confine its change to gc`

_Requirements: CHECKOUTGC-6.1, CHECKOUTGC-6.2, CHECKOUTGC-6.3, CHECKOUTGC-6.4, CHECKOUTGC-6.5, CHECKOUTGC-6.6, CHECKOUTGC-6.7_

---

### Task 9: The hive skill and README say when to run it and how to read it

**Files:**
- Modify: `plugin/skills/hive/SKILL.md`: a new section "Keep a reused worktree fast", a Reading-table row, and a line under "Free disk".
- Modify: `skills/hive/eval.json`, `skills/hive/TESTS.md`
- Modify: `README.md` (a `gc --checkout` subsection)
- Modify: `crates/cargo-hive/tests/skill.rs`: `Parsed` at line 441, `parse()` at 455-483, and the documented-command combos at 657-735.
- Test: `crates/cargo-hive/tests/skill.rs`; the Sonnet and Haiku evals run through `author-skills`

**Reuse:** rung 2. It reuses the SKILLPASS guide structure (`plugin/skills/hive/SKILL.md`, its "Free disk" section and Reading table) and the README's per-command sections. The existing `tests/skill.rs` checks (:539, :657, :778, :899, :1125, :1131, :949) check them.

**Interfaces:**
- Consumes: every count and finding name from Tasks 4–7, and the `--checkout` flag.
- Produces: `Parsed::Gc { dry_run, plan, checkout }`, and the skill line `cargo hive gc --checkout <worktree> --json`. The count step reads `ls -f <the checkout's real target dir>/debug/deps | wc -l`; in klynt the target dir is `backend/target`.

**Depends-on:** Task 5, Task 6, Task 7

- [ ] **Step 1: Write the failing tests.** Extend `parse()` and `skill_documents_every_shipped_command` so that a `gc --checkout` line is told apart from bare `gc`. Add the eval entries: trigger "before a test build with deps/ over 20,000 entries", and reading "a `gc.skipped` or `gc.shared_target` receipt". Run `cargo test -p cargo-hive --test skill`; expect failure (the skill and README lack the line).
- [ ] **Step 2: Write the skill text through the `author-skills` workflow** (RED baseline, then GREEN, Sonnet and Haiku evals), staying within the 150-line budget (118 used). Also write the README subsection. Run `cargo test -p cargo-hive --test skill`; expect pass.
- [ ] **Step 3: Commit.** `docs(skill): run gc --checkout before a test build in a bloated worktree`

_Requirements: CHECKOUTGC-5.1, CHECKOUTGC-5.2, CHECKOUTGC-5.3, CHECKOUTGC-5.4_

---

### Task 10: klynt proof on a copy of an aged slot

**Files:**
- Create: `.skills/verify/2026-11-15-checkout-gc/PLAN.md` (git-ignored evidence: commands, receipts, timings, counts)
- Test: manual proof (no tracked file)

**Reuse:** rung 2. It reuses the warm-slots and pool spike procedure (main checkout `.skills/verify/2026-11-14-warm-slots/PLAN.md`, `.skills/verify/2026-11-14-pool-vs-wrapper/PLAN.md`) and the `liveness_prove.rs` replay idea (tests/common/prove.rs:1-14).

**Interfaces:**
- Consumes: the built `cargo-hive` binary (Tasks 4–7).
- Produces: the measured `deps/` entry counts, the next test build's time, the command's wall time with a cold and a warm snapshot, the receipt, and the recompile count.

**Depends-on:** Task 7, Task 9

- [ ] **Step 1: Prepare a copy.**
  - Clone (APFS `clonefile`) an aged klynt slot and its main into `/private/tmp`, with a scratch `HIVE_HOME`.
  - Register the copy's main.
  - Check that `cargo metadata --no-deps --locked --offline` loads in **every** live checkout of the copy. If one fails, 3.5 blocks the run, so fix or remove that worktree in the copy first.
- [ ] **Step 2: Measure before.** Record `ls -f backend/target/debug/deps | wc -l` and the edit's test build time.
- [ ] **Step 3: Run** `cargo hive gc --checkout <slot copy> --json`.
  - Record the wall time with a cold and a warm snapshot (15 s target).
  - Record the receipt.
  - Re-run the slot's last task's cargo commands, and count recompiles that come from the gc. Expect 0 for units compiled inside the window.
- [ ] **Step 4: Measure after.**
  - Record the `deps/` count and the same test build's time. The evidence targets are about 4.2k files and about 28 s, from the Open Questions; these are not pass/fail.
  - Record the share left by `incremental/`, which is the reopen check for the auto-tier exclusion.
  - If the 15 s ceiling is exceeded, the reopen path is an on-demand read of each N_OSO string at its `n_strx` instead of the whole string table, with no cache.
- [ ] **Step 5: Write the PLAN.md record** with dated numbers. No commit (git-ignored).

_Requirements: CHECKOUTGC-7.1_
