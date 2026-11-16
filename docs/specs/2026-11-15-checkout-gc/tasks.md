# Tasks: Checkout GC

> **For agentic workers:** after plan approval, pick one execute skill —
> `build` (wave mode) (continuous dependency-aware scheduler), `build` (story mode) (human-gated story review
> units), or `build` (inline mode) (controller implements, no implementer subagents).
> The chosen skill writes `Execution-mode:`. Steps use checkbox (`- [ ]`) syntax
> for tracking.

Feature code: CHECKOUTGC
Status: Approved
Approved: 2026-11-16, by the operator (Jayden), on the spec sheet at a16e5e0: "approve" (.skills/_pending-checkout-gc/auto/operator-reply.md)
Date: 2026-11-15
Execution-mode: continuous
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
- **Compat freeze:** apart from what a task names, nothing changes. Repo-wide `gc`, `gc --dry-run`, `gc --plan`, `apply` and every other command keep their receipts, count names, finding kinds, exit codes and deletions. No existing assertion is weakened to pass. Two assertions pin the old flag list, and each may be extended for `--checkout` only, in Task 5: `tests/liveness.rs:190-244` gains one entry, and `tests/cli_tree.rs` gets the current-fixture rewrite.
- **Public text** (skill, README) names no external comparison tool.
- **Team:** Solo (project.md `## Team`), so tasks carry no assignee.

## File Structure

| Path | Responsibility in this plan |
|---|---|
| `crates/platform/src/lib.rs` | `Fs::read_at`, a trait method with a default body over `Fs::read` |
| `crates/platform/src/apfs.rs` | `Apfs::read_at`, a positioned read, with unit tests |
| `crates/core/src/caps.rs` | `MACHO_CMDS_CAP`, `MACHO_SYMTAB_CAP`, `MACHO_STRTAB_CAP`, `FAT_ARCH_CAP`, and the `//!` line widened beyond listing bounds |
| `crates/gc/src/debug_map.rs` | the pure Mach-O debug-map reader: `read`, `DebugMap`, `MapError` (new) |
| `crates/gc/src/checkout.rs` | the checkout GC run: `run`, `CheckoutInput`, `CheckoutReport`, `Counts` (new) |
| `crates/gc/src/lib.rs` | `pub mod checkout; pub mod debug_map;` |
| `crates/cargo/src/discover.rs` | `WorkspaceResolution`, `TargetScan.workspaces`, `resolvers_of`, `failed_workspaces` |
| `crates/cargo-hive/src/cli.rs` | the `Gc.checkout` arg and reworded `Gc` doc, the dispatch, `checkout_cmd`, `put_checkout_counts`, and `find_checkout` (extracted from `locate_checkout`) |
| `crates/cargo-hive/tests/common/recording_fs.rs` | `Recording::read_at` (logged as `read_at`), and the `fail_read_at(path)` and `on_remove(path, hook)` hooks |
| `crates/cargo-hive/tests/common/registry_fixture.rs` | `RegistryRepo::with_binaries`, `nested_worktree`, `set_target_dir`, `Compiled.workspace_fresh` |
| `crates/cargo-hive/tests/liveness.rs` | the flag list in `the_cli_has_no_new_subcommand_or_flag` gains `cargo-hive gc --checkout` |
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

### Task 1: The platform `Fs` reads a byte range, and the test `Fs` can watch it

**Files:**
- Modify: `crates/platform/src/lib.rs` (trait `Fs`; the method list is at lines 303-338)
- Modify: `crates/platform/src/apfs.rs` (`impl Fs for Apfs`, line 47)
- Modify: `crates/cargo-hive/tests/common/recording_fs.rs` (`impl Fs for Recording`, line 234; hooks near 120-211)
- Test: `crates/platform/src/apfs.rs` (unit tests)

**Reuse:** rung 3. `std::os::unix::fs::FileExt::read_at` inside `Apfs`. The trait default goes through the existing `Fs::read`.

**Interfaces:**
- Consumes: none.
- Produces:
  - `fn read_at(&self, path: &Path, offset: u64, len: usize) -> io::Result<Vec<u8>>` on `Fs`. It returns up to `len` bytes, fewer at end of file, and an empty vector past it. The default slices `self.read(path)`. `Apfs` sizes its buffer from the file's length, never from `len` alone.
  - `Recording` logs each call as `read_at <path> <offset> <len>`.
  - `Recording::fail_read_at(path)` makes the next `read_at` of `path` fail.
  - `Recording::on_remove(path, hook: impl Fn() + 'static)` runs `hook` just before the `remove_file` of `path`.

**Depends-on:** none

- [x] **Step 1: Write the failing tests** (table-driven over offset and length) in `apfs.rs`:
  - `read_at_returns_the_requested_range`;
  - `read_at_is_short_at_end_of_file`;
  - `read_at_past_end_of_file_is_empty`;
  - `a_huge_len_on_a_small_file_allocates_only_the_file`;
  - `the_default_read_at_matches_apfs_on_the_same_file`.

  Run `cargo test -p cargo-hive-platform --lib`; expect a compile failure (no `read_at`).
- [x] **Step 2: Implement** the trait method with its default, the `Apfs` override, and the three `Recording` additions. Run the project.md task gate for `cargo-hive-platform`; expect pass.
- [x] **Step 3: Commit.** `feat(platform): read a byte range of a file through Fs`

_Requirements: CHECKOUTGC-2.10_

---

### Task 2: A pure reader lists the loose objects a binary's debug map names

**Files:**
- Create: `crates/gc/src/debug_map.rs`
- Modify: `crates/gc/src/lib.rs` (add `pub mod debug_map;`)
- Modify: `crates/core/src/caps.rs` (lines 3-6, with the `//!` line)
- Test: `crates/gc/src/debug_map.rs` (unit tests on hand-built byte images, served by a local fake `Fs` that holds bytes in memory)

**Reuse:** none — new code (rung 7). There is no Mach-O reader in the workspace or `Cargo.lock`, a dependency needs the owner (docs/standards/code.md:16), and `nm` is a process (ARCH-17).

**Interfaces:**
- Consumes: `Fs::read_at` (Task 1).
- Produces:
  - `pub struct DebugMap { pub loose: BTreeSet<String>, pub archive_entries: u64 }`.
  - `pub fn read(fs: &dyn Fs, path: &Path) -> Result<Option<DebugMap>, MapError>`. It gives `Ok(None)` for no Mach-O magic, for `MH_OBJECT` and for `MH_DSYM`.
  - `pub enum MapError { Read(io::Error), Truncated(&'static str), Bound(&'static str, u64), UnknownSlice(u32), Unsupported(&'static str) }`, with hand-written `Display` and `Error`. Tests match it with `matches!`.
  - In `cargo_hive_core::caps`: `MACHO_CMDS_CAP: u64 = 4 << 20`, `MACHO_SYMTAB_CAP: u64 = 256 << 20`, `MACHO_STRTAB_CAP: u64 = 512 << 20`, `FAT_ARCH_CAP: u32 = 8`.

**Depends-on:** Task 1

- [x] **Step 1: Write the failing tests.** Byte images come from a small in-test builder of `mach_header_64`, `LC_SYMTAB`, `nlist_64` and a string table, laid out as in `.skills/research/2026-11-15-macho-n-oso.md` (main checkout). Tests:
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
  - a boundary table for each of the four caps (exactly at the cap reads; one past is `Bound`).

  Run `cargo test -p cargo-hive-gc --lib debug_map::`; expect a compile failure.
- [x] **Step 2: Implement** `debug_map.rs`, with its `//!` contract, and the four caps. Every read goes through `fs.read_at`, and one binary's tables are held at a time. Run the project.md task gate for `cargo-hive-gc`, with `cargo-hive-core` escalated per the gate; expect pass.
- [x] **Step 3: Commit.** `feat(gc): read the loose objects a Mach-O debug map names`

_Requirements: CHECKOUTGC-2.3, CHECKOUTGC-2.4, CHECKOUTGC-2.7_

---

### Task 3: Discovery keeps every workspace's resolution

**Files:**
- Modify: `crates/cargo/src/discover.rs`. `TargetScan` is at line 518 and built only at line 652; `resolve_workspaces` is at 425; `owner_checkout` is at 509.
- Test: `crates/cargo/src/discover.rs` (unit tests)

**Reuse:** rung 2. It extends `discover::targets` (crates/cargo/src/discover.rs:530-660) and its private `resolve_workspaces` (:425), whose results it already computes and drops.

**Interfaces:**
- Consumes: none.
- Produces:
  - `pub struct WorkspaceResolution { pub dir: PathBuf, pub checkout: PathBuf, pub result: Result<Resolved, String> }`, where `checkout` is the deepest live checkout containing `dir`.
  - `TargetScan.workspaces: Vec<WorkspaceResolution>`.
  - `pub fn resolvers_of(scan: &TargetScan, target: &Path) -> Vec<PathBuf>`: the distinct checkouts whose `target_directory` or `build_directory` equals the canonical `target`.
  - `pub fn failed_workspaces(scan: &TargetScan) -> Vec<(PathBuf, String)>`: every failure.

**Depends-on:** none

- [x] **Step 1: Write the failing unit tests**, over `TargetScan` values built by hand:
  - `resolvers_of_names_every_checkout_that_resolves_to_a_target`;
  - `resolvers_of_matches_through_a_symlinked_target_path`;
  - `failed_workspaces_lists_every_failure_not_only_the_first`;
  - `existing_owner_and_failure_are_unchanged`.

  Run `cargo test -p cargo-hive-cargo --lib discover::`; expect a compile failure.
- [x] **Step 2: Implement** the field, the struct and the two helpers. `owner`, `failure`, `protected_owner` and `Scope` stay as they are. Run the project.md task gate for `cargo-hive-cargo`; expect pass, with every existing discover and plan test green.
- [x] **Step 3: Commit.** `feat(cargo): keep each workspace's metadata resolution in the target scan`

_Requirements: CHECKOUTGC-3.1, CHECKOUTGC-3.2, CHECKOUTGC-3.5_

---

### Task 4: A registered fixture repo that links binaries and nests a worktree

**Files:**
- Modify: `crates/cargo-hive/tests/common/registry_fixture.rs`. `RegistryRepo` is at lines 70-158, `Compiled` and `cargo_json` at 162-180, and `parse_compiled` near 236-262.
- Create: `crates/cargo-hive/tests/checkout_gc.rs` (`#![cfg(target_os = "macos")]`, as the other integration files do)
- Test: `crates/cargo-hive/tests/checkout_gc.rs`

**Reuse:** rung 2. `RegistryRepo::new` and `worktree`, `cargo_json` (registry_fixture.rs:76-180), and the `cargo_fixture.rs:36-82` crate shapes (a bin, an example, a build script, a proc-macro). It also uses the `.cargo/config.toml` `build.target-dir` precedent (`gc.rs:623`, `plan_apply.rs:210`).

**Interfaces:**
- Consumes: none.
- Produces:
  - `RegistryRepo::with_binaries(hive: &Hive) -> RegistryRepo`. Its workspace crate `app` has `src/lib.rs`, `src/main.rs`, `tests/it.rs`, `examples/ex.rs` and `build.rs`, and a workspace proc-macro member `appmacro` that `app` uses.
  - `RegistryRepo::nested_worktree(&self, name: &str) -> PathBuf`, at `<main>/.worktrees/<name>`.
  - `set_target_dir(checkout: &Path, dir: &Path)`, which writes `.cargo/config.toml` `[build] target-dir`.
  - `Compiled.workspace_fresh: Vec<String>`.

**Depends-on:** none

- [x] **Step 1: Write the failing fixture test** `the_binaries_fixture_links_a_bin_a_test_an_example_a_build_script_and_a_proc_macro_offline`. It runs `cargo_json(…, ["test", "--no-run"])` in `main` and asserts each linked file exists, with loose `deps/*.rcgu.o` beside them. Run `cargo test -p cargo-hive --test checkout_gc`; expect a compile failure.
- [x] **Step 2: Implement** the four helpers. Run `cargo test -p cargo-hive --test checkout_gc` and the suites that use `RegistryRepo` (`--test seed`, `--test plan_apply`); expect pass.
- [x] **Step 3: Commit.** `test: a registry fixture that links binaries and nests a worktree`

_Requirements: CHECKOUTGC-1.4, CHECKOUTGC-2.2, CHECKOUTGC-3.7_

---

### Task 5: `gc --checkout` deletes one checkout's dead units, only in its own targets

**Files:**
- Create: `crates/gc/src/checkout.rs`
- Modify: `crates/gc/src/lib.rs` (add `pub mod checkout;`)
- Modify: `crates/cargo-hive/src/cli.rs`. The pieces:
  - `Command::Gc` at lines 44-58 gets its new arg and reworded doc;
  - the dispatch at 192-202;
  - `checkout_cmd` and `put_checkout_counts` go beside `gc_cmd` (914-1130);
  - `find_checkout` is extracted from `locate_checkout` (1569-1608), keeping its refusal text (1612-1618), and `locate_checkout` then calls it.
- Create: `crates/cargo-hive/tests/fixtures/current/cli-tree.txt`
- Modify: `crates/cargo-hive/tests/cli_tree.rs` (the fixture test at lines 34-51)
- Modify: `crates/cargo-hive/tests/liveness.rs` (the flag list at lines 190-244)
- Test: `crates/gc/src/checkout.rs` (inline unit tests of the pure `unit_order` and target-selection helpers); `crates/cargo-hive/tests/checkout_gc.rs`; `crates/cargo-hive/tests/cli_tree.rs`

**Reuse:** rung 2:
- `locks::acquire(fs, target, true)` (crates/core/src/locks.rs:43);
- `target::identify_bounded` and `fingerprint::read_profile` (the fresh read, as `recheck`, apply.rs:719-800, does);
- `liveness()` (crates/gc/src/liveness.rs:77);
- `guard::check_entry` with `Expect::Entry` (guard.rs:24);
- `ledger::{Ledger, remove_counted}` (crates/core/src/ledger/account.rs:44).

The per-unit deletion order follows apply's `removal_order` rule (apply.rs:927). On the CLI side it reuses:
- the `gc_cmd` flow (cli.rs:914-1130): signals, state open, `doctor::require`, `discover::targets`, volume cross-check notes (`apply::cross_check_notes`), the `ignored_vars` notes (cli.rs:886), `merge_snapshot` under `locked_or_cancelled`, `emit`;
- `cargo_hive_state::window_days`, refused like `gc --plan` (build.rs:191);
- the first half of `locate_checkout` (cli.rs:1569-1608), extracted.

**Interfaces:**
- Consumes: `resolvers_of`, `failed_workspaces`, `TargetScan.workspaces` (Task 3); `RegistryRepo::with_binaries`, `nested_worktree`, `set_target_dir`, `Compiled.workspace_fresh` (Task 4).
- Produces:
  - `gc::checkout::run(input: &CheckoutInput, cancel: &dyn Fn() -> bool) -> CheckoutReport`, with the `pub` fields of design "Checkout GC run":
    - `CheckoutInput { fs, scan, checkout, nested, now, window, account_bytes }`;
    - `CheckoutReport { per_target, totals, findings, unmeasured, unmeasured_more, cancelled, failed }`;
    - `Counts { units_deleted, unit_files_deleted, objects_deleted, objects_referenced, binaries_read, targets_shared, targets_outside, bytes, skips }`.
  - The pure helpers `fn unit_order(files: &[PathBuf]) -> Vec<PathBuf>` and `fn select_targets(scan, checkout, nested) -> Selection { own, shared, outside, failed }`.
  - `find_checkout(reg, cfg, path) -> Result<(Repo, Checkout), Refusal>`.
  - The receipt command `"gc --checkout"`.
  - `put_checkout_counts` writes every `Counts` field as `<name>` and `<name>:<target>`:
    - `units_deleted`, `unit_files_deleted`, `objects_deleted`, `objects_referenced`, `binaries_read`;
    - `targets_shared`, `targets_outside`;
    - the five `targets_skipped_*` names of `Reason::count_name` (locks.rs:19-27);
    - `freed_bytes`, `deleted_bytes_still_shared`.
  - New finding kinds: `gc.undecided`, `gc.delete_failed`, `gc.shared_target`, `gc.outside_checkout`. Existing kinds it reuses: `ownership.unresolved` (build.rs:250), `walk.unreadable`, `refusal` and `error`.

**Depends-on:** Task 2, Task 3, Task 4 (Task 2 shares `crates/gc/src/lib.rs`)

- [x] **Step 1: Write the failing integration tests** in `checkout_gc.rs` (`Hive::ready`, `RegistryRepo::with_binaries`, `cargo_json(…, ["test", "--no-run"])`, `backdate_all`):
  - `a_checkouts_dead_units_are_deleted_in_one_run_with_no_plan_stored`;
  - `units_compiled_inside_the_window_are_kept_and_the_next_build_recompiles_none`, which reruns the same cargo command and asserts `workspace_recompiled` and `registry_recompiled` are empty;
  - `nothing_under_build_is_touched`;
  - `an_undecided_target_deletes_no_unit_and_names_each_reason_with_what_clears_it`, made Undecided by emptying one unit's hash file and deleting its JSON inside the window;
  - `an_out_of_range_window_is_refused_with_exit_1`;
  - `checkout_with_repos_plan_or_dry_run_is_an_argument_error_with_exit_2_and_no_receipt` (table);
  - `a_missing_capability_refuses_with_exit_1`;
  - `without_eviction_accounting_bytes_are_unmeasured_but_units_are_deleted`;
  - `the_receipt_is_schema_2_with_command_gc_checkout_and_only_that_checkout_in_scope`;
  - `an_interrupted_checkout_gc_exits_130_with_a_receipt` (the `gc.rs:485` pattern);
  - `a_cancel_right_after_discovery_deletes_nothing_and_exits_130`;
  - `a_failed_deletion_exits_1`, done by running `chmod 500` on `deps/`;
  - `a_receipt_that_cannot_be_recorded_exits_1`, done by running `chmod 500` on `HIVE_HOME/receipts`;
  - `walk_faults_become_walk_unreadable_findings`;
  - `a_target_two_worktrees_resolve_to_is_left_and_named`;
  - `a_target_dir_outside_the_checkout_is_left_and_named`;
  - `a_broken_manifest_in_any_worktree_blocks_every_deletion_and_exits_1`.

  Library tests through `gc::checkout::run` with `Recording`, on real tmp dirs:
  - `a_stop_after_each_position_in_a_unit_leaves_no_unit_fresh_with_a_file_missing`, using `fail_removes_after(n)` over every n of one unit;
  - `a_dsym_dir_of_a_dead_unit_is_deleted`;
  - `an_entry_swapped_after_its_stat_is_refused` (`symlink_after_stat`);
  - `a_unit_changed_between_scan_and_lock_is_judged_on_the_fresh_read` (`replace_after_lock`).

  Unit tests:
  - `unit_entries_are_ordered_dep_info_first_and_fingerprint_dir_last`;
  - `a_unit_without_dep_info_orders_its_fingerprint_dir_first`;
  - `a_unit_with_only_a_fingerprint_dir_yields_nothing`;
  - `select_targets_splits_own_shared_outside_and_failed`.

  Then `the_tree_differs_from_0_1_0_only_in_gc` in `cli_tree.rs`. It allows changed lines only inside the block from `cmd gc` to the next `cmd` line at the same indent, and on the root `long_help` line naming `gc`. Inside the block, the only new argument may be `checkout`. Run `cargo test -p cargo-hive --test checkout_gc --test cli_tree`; expect a compile failure (no `--checkout`).
- [x] **Step 2: Implement** `checkout.rs` up to the dead-unit class, with the whole target selection and its findings and counts, as design "Checkout GC run" steps 1, 2 (acquire, re-read, units) and 3. Also implement `checkout_cmd` as design "CLI mode" steps 1–7, the volume cross-check notes and the `ignored_vars` notes included. Bind the whole `Locked` value for the life of each target's deletions.
- [x] **Step 3: Pin the new CLI surface.**
  - Regenerate `fixtures/current/cli-tree.txt` with `HIVE_WRITE_CLI_TREE` and read the diff by eye.
  - Replace `the_command_tree_matches_0_1_0` with `the_command_tree_matches_the_current_fixture` plus the diff test.
  - Add `"cargo-hive gc --checkout"` to `tests/liveness.rs`'s list.
  - Leave `fixtures/v0.1.0/` unchanged.

  Run the project.md task gate for `cargo-hive-gc`, then `cargo-hive`, then `cargo test -p cargo-hive --test seed` (seed's refusals are unchanged by `find_checkout`); expect pass.
- [x] **Step 4: Commit.** `feat(gc): delete one checkout's dead units with gc --checkout`

_Requirements: CHECKOUTGC-1.1, CHECKOUTGC-1.2, CHECKOUTGC-1.3, CHECKOUTGC-1.4, CHECKOUTGC-1.5, CHECKOUTGC-1.6, CHECKOUTGC-1.7, CHECKOUTGC-1.8, CHECKOUTGC-1.9, CHECKOUTGC-1.10, CHECKOUTGC-1.11, CHECKOUTGC-1.12, CHECKOUTGC-3.1, CHECKOUTGC-3.2, CHECKOUTGC-3.5, CHECKOUTGC-3.7, CHECKOUTGC-6.6, CHECKOUTGC-7.2, CHECKOUTGC-7.3_

---

### Task 6: `gc --checkout` refuses a path that is not one checkout, and touches nothing else

**Files:**
- Modify: `crates/cargo-hive/src/cli.rs` (`checkout_cmd`: the refusal for a directory inside a checkout)
- Test: `crates/cargo-hive/tests/checkout_gc.rs`

**Reuse:** rung 2. It extends `checkout_cmd` and `find_checkout` (Task 5). `owning_repo` (cli.rs:522) is used only to word the refusal for a directory inside a checkout.

**Interfaces:**
- Consumes: `checkout_cmd`, `find_checkout` (Task 5).
- Produces: the refusal detail `"<path> is not a checkout of <main>; name the checkout's root directory"`. `find_checkout`'s existing text (cli.rs:1612-1618) stays for a path no registered repo contains.

**Depends-on:** Task 5

- [x] **Step 1: Write the failing integration tests:**
  - `gc_on_a_worktree_leaves_main_and_every_other_lane_byte_for_byte_unchanged`, which compares the `tree()` of every other target before and after;
  - `gc_on_main_leaves_the_worktrees_nested_under_it` (`nested_worktree`);
  - `an_unregistered_path_is_refused_naming_repo_add` and `a_directory_inside_a_checkout_is_refused_naming_its_root` (table);
  - `only_the_receipt_line_and_snapshot_change_under_the_state_root`, which diffs the state root and the store before and after.

  Run `cargo test -p cargo-hive --test checkout_gc`; expect the refusal test to fail and the others to pass or fail per Task 5.
- [x] **Step 2: Implement** the refusal wording. Run the project.md task gate for `cargo-hive`; expect pass.
- [x] **Step 3: Commit.** `feat(cli): gc --checkout names the checkout root a path sits in`

_Requirements: CHECKOUTGC-3.3, CHECKOUTGC-3.4, CHECKOUTGC-3.6_

---

### Task 7: A target a build holds is skipped and named

**Files:**
- Modify: `crates/gc/src/checkout.rs` (the acquire branch and the `gc.target_failed` finding)
- Test: `crates/cargo-hive/tests/checkout_gc.rs`

**Reuse:** rung 2. `locks::acquire(fs, target, true)` and `Reason::count_name` (crates/core/src/locks.rs:19-43). The held-lock test pattern is `plan_apply.rs:393-408` with `hold_lock_child` (tests/common/mod.rs:235). The `Recording` hooks used are `unlockable`, `no_identity`, `create_dir_after_lock` and `on_remove` (Task 1).

**Interfaces:**
- Consumes: `gc::checkout::run`, `put_checkout_counts` (Task 5); `Recording::on_remove` (Task 1).
- Produces: the `gc.skipped` finding (path = target, detail = reason) and the new `gc.target_failed` finding. The counts are already written by `put_checkout_counts` (Task 5).

**Depends-on:** Task 6

- [x] **Step 1: Write the failing tests.**
  - `a_held_lock_skips_the_target_names_it_and_deletes_nothing`, a table over `.cargo-build-lock`, `.cargo-lock` and `.cargo-artifact-lock`, each asserted to exist in the fixture first.
  - The other skip reasons:
    - `a_profile_with_fingerprints_and_no_lock_file_is_skipped_as_missing`;
    - `an_unopenable_or_unlockable_lock_is_skipped_and_named` (`unlockable`, `no_identity`);
    - `a_profile_appearing_while_locking_is_skipped_as_profile_set` (`create_dir_after_lock`).
  - `a_target_whose_profile_listing_fails_while_locking_exits_1`, done by running `chmod 000` on the target dir.
  - `every_lock_is_taken_before_the_first_read_of_the_target`, checked on the `Recording` log order (`lock` lines before the first listing of `deps/` or `read_at`).
  - `the_locks_stay_held_until_the_last_deletion`. An `on_remove` hook on the last deletion spawns `env!("CARGO_BIN_EXE_cargo-hive") __try-lock --kind flock --path <lock>` and expects held.

  Run `cargo test -p cargo-hive --test checkout_gc`; expect the new tests to fail.
- [x] **Step 2: Implement** the skip findings, the `Failed` finding with `failed = true`, and any lock-lifetime fix the probe shows. Run the project.md task gate for `cargo-hive-gc`, then `cargo-hive`; expect pass.
- [x] **Step 3: Commit.** `feat(gc): gc --checkout skips and names a target a build holds`

_Requirements: CHECKOUTGC-4.1, CHECKOUTGC-4.2, CHECKOUTGC-4.3, CHECKOUTGC-4.4_

---

### Task 8: `gc --checkout` deletes the debug objects no linked binary references

**Files:**
- Modify: `crates/gc/src/checkout.rs` (the object phase, and the bounded listing helper)
- Test: `crates/gc/src/checkout.rs` (unit tests); `crates/cargo-hive/tests/checkout_gc.rs`

**Reuse:** rung 2. It extends `gc::checkout` (Task 5) with:
- `debug_map::read` (Task 2);
- `Fs::read_dir_bounded` with `DEPS_CAP` and `CRATE_DIR_CAP` (crates/core/src/caps.rs:4-6);
- `guard::check_entry` with `Expect::Entry { kind: File, dev, ino }`;
- `ledger::remove_counted`.

**Interfaces:**
- Consumes: `debug_map::{read, DebugMap, MapError}` (Task 2); `gc::checkout::run` and `put_checkout_counts` (Task 5); `Recording::fail_read_at` (Task 1).
- Produces:
  - `fn list_profile(fs: &dyn Fs, profile: &Path, deps_cap: usize, dir_cap: usize) -> Result<Listing, String>`, where `Listing { objects: Vec<(PathBuf, u64, u64)>, candidates: Vec<PathBuf> }`.
  - The finding kind `gc.objects_undecided`.

**Depends-on:** Task 7 (shares `checkout.rs`), Task 2

- [x] **Step 1: Write the failing tests.** Integration tests on `RegistryRepo::with_binaries` builds:
  - `unreferenced_objects_are_deleted_and_every_object_a_binary_names_is_kept`;
  - `a_test_binary_that_was_not_relinked_still_keeps_its_objects`, which rebuilds after an edit that relinks one binary and leaves another;
  - `objects_named_only_by_a_dead_units_binary_go_in_the_same_run`;
  - `a_cloned_target_keeps_the_objects_its_binaries_name_under_the_old_path`, made with `cp -c -R` of one worktree's target into another worktree's target;
  - `an_undecided_target_still_loses_its_unreferenced_objects`;
  - `a_profile_with_an_unsupported_fingerprint_format_keeps_every_object`;
  - `the_receipt_counts_objects_deleted_kept_and_binaries_read`;
  - `objects_are_read_with_ranged_reads_and_no_child_process`, which checks the `Recording` log for `read_at` lines on each binary and puts a failing `nm` first on `PATH` that writes a marker file the test asserts absent;
  - `a_binary_whose_read_fails_keeps_every_object_in_its_profile` (`fail_read_at`).

  Unit tests:
  - `a_profile_with_binaries_and_objects_but_no_debug_map_keeps_every_object`;
  - `a_missing_examples_or_build_dir_reads_as_empty`;
  - `list_profile_at_its_cap_lists_and_one_past_fails` (small caps; exact and one past);
  - `an_object_swapped_for_a_directory_is_refused`;
  - `only_deps_rcgu_o_files_are_candidates_for_deletion`;
  - `names_ending_in_dot_o_are_not_read`.

  Run `cargo test -p cargo-hive --test checkout_gc`; expect the new tests to fail.
- [x] **Step 2: Implement** the object phase as in design "Checkout GC run" step 2, "Objects": the layout and format check, `list_profile`, the reads, the no-debug-map rule and the deletion. Run the project.md task gate for `cargo-hive-gc`, then `cargo-hive`; expect pass.
- [x] **Step 3: Commit.** `feat(gc): gc --checkout deletes debug objects no linked binary references`

_Requirements: CHECKOUTGC-2.1, CHECKOUTGC-2.2, CHECKOUTGC-2.5, CHECKOUTGC-2.6, CHECKOUTGC-2.8, CHECKOUTGC-2.9, CHECKOUTGC-2.10, CHECKOUTGC-2.11_

---

### Task 9: The other commands keep working

**Files:**
- Test: the existing suites `crates/cargo-hive/tests/gc.rs`, `plan_apply.rs`, `plan_tier.rs`, `liveness*.rs`, `seed.rs`, `receipts.rs` and `upgrade.rs`; the `crates/core/src/locks.rs` and `crates/seed/src/record.rs` unit tests; the Linux `cargo check`

**Reuse:** rung 2. These are the existing tests that pin each guard:
- `tests/gc.rs`, `plan_apply.rs` and `plan_tier.rs` (6.1);
- the GCPLAN-9.2 tests (6.2);
- `liveness*.rs` and `seed.rs:1520` (6.3);
- `locks.rs` unit tests (6.4);
- `seed` record tests (record.rs:813-817, 906; 6.5);
- `cli_tree.rs` (6.6);
- the Linux `cargo check` in the merge gate (6.7).

**Interfaces:**
- Consumes: the whole feature (Tasks 1–8).
- Produces: none.

**Depends-on:** Task 8

- [x] **Step 1: Run** `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check` and `cargo check --workspace --target x86_64-unknown-linux-gnu`. Expect every existing suite green, with no assertion edited apart from the two Task 5 names in Global Constraints.
- [x] **Step 2: Confirm** with `git diff main -- crates/cargo-hive/tests` that no existing test changed beyond those two and the `skill.rs` work of Task 10. Record the run in the task report.

_Requirements: CHECKOUTGC-6.1, CHECKOUTGC-6.2, CHECKOUTGC-6.3, CHECKOUTGC-6.4, CHECKOUTGC-6.5, CHECKOUTGC-6.7_

---

### Task 10: The hive skill and README say when to run it and how to read it

**Files:**
- Modify: `plugin/skills/hive/SKILL.md`: a new section "Keep a reused worktree fast", a Reading-table row, and a line under "Free disk".
- Modify: `skills/hive/eval.json`, `skills/hive/TESTS.md`
- Modify: `README.md` (a `gc --checkout` subsection; the `crates/*/README.md` copies are refreshed by the next release step, as in 2f7d029)
- Modify: `crates/cargo-hive/tests/skill.rs`: `Parsed` at line 441, `parse()` at 455-483, and the documented-command combos at 657-735.
- Test: `crates/cargo-hive/tests/skill.rs`; the Sonnet and Haiku evals run through `author-skills`

**Reuse:** rung 2. It reuses the SKILLPASS guide structure (`plugin/skills/hive/SKILL.md`, its "Free disk" section and Reading table) and the README's per-command sections. The existing `tests/skill.rs` checks (:539, :657, :778, :899, :1125, :1131, :949) check them.

**Interfaces:**
- Consumes: every count and finding name from Tasks 5–8, and the `--checkout` flag.
- Produces: `Parsed::Gc { dry_run, plan, checkout }`, and the skill line `cargo hive gc --checkout <worktree> --json`. The count step reads `ls -f <the checkout's real target dir>/debug/deps | wc -l`; in klynt the target dir is `backend/target`.

**Depends-on:** Task 9

- [ ] **Step 1: Write the failing tests.** Extend `parse()` and the three `Parsed::Gc` combos in `skill_documents_every_shipped_command` so that a `gc --checkout` line is told apart from bare `gc`. Add the eval entries: trigger "before a test build with deps/ over 20,000 entries", and reading "a `gc.skipped`, `gc.shared_target` or `gc.outside_checkout` receipt". The last must lead to repo-wide `gc --plan` then `apply`, not to silence. The skill line must also say that this fallback covers main and every lane of the repo and deletes no debug objects, so the agent does not read it as a scoped retry. Run `cargo test -p cargo-hive --test skill`; expect failure (the skill and README lack the line).
- [ ] **Step 2: Write the skill text through the `author-skills` workflow** (RED baseline, then GREEN, Sonnet and Haiku evals), staying within the 150-line budget (118 used). Also write the README subsection. Run `cargo test -p cargo-hive --test skill`; expect pass.
- [ ] **Step 3: Commit.** `docs(skill): run gc --checkout before a test build in a bloated worktree`

_Requirements: CHECKOUTGC-5.1, CHECKOUTGC-5.2, CHECKOUTGC-5.3, CHECKOUTGC-5.4_

---

### Task 11: klynt proof on a copy of an aged slot

**Files:**
- Create: `.skills/verify/2026-11-15-checkout-gc/PLAN.md` (git-ignored evidence: commands, receipts, timings, counts)
- Test: manual proof (no tracked file)

**Reuse:** rung 2. It reuses the warm-slots and pool spike procedure (main checkout `.skills/verify/2026-11-14-warm-slots/PLAN.md`, `.skills/verify/2026-11-14-pool-vs-wrapper/PLAN.md`) and the `liveness_prove.rs` replay idea (tests/common/prove.rs:1-14).

**Interfaces:**
- Consumes: the built `cargo-hive` binary (Tasks 5–8).
- Produces: the measured `deps/` entry counts, the next test build's time, the command's wall time with a cold and a warm snapshot, the receipt, and the recompile count.

**Depends-on:** Task 10

- [ ] **Step 1: Prepare two copies.**
  - Clone an aged klynt slot and its main twice into `/private/tmp`, as a **control** copy and a **gc** copy. Use `cp -c -p -R` (APFS clone, mtimes kept), so the stamps the window reads are the originals'.
  - **Never run `git worktree repair` or any other git command that follows the copied admin paths.** klynt's worktree admin paths are absolute, so a repair in a cloned main would rewrite the owner's real worktrees' `.git` files. Instead, rewrite only the copy's own admin files, with a text edit of each copied path:
    - every `<copy main>/.git/worktrees/*/gitdir`;
    - each copied worktree's `.git` file.
  - Then run `git -C <copy main> worktree list --porcelain` and check that every path it prints lies under `/private/tmp`. Stop if any does not. Nothing else runs before this check passes.
  - Set up a scratch `HIVE_HOME` per copy, run `cargo hive doctor`, then register the copy's main.
  - Check that `cargo metadata --no-deps --locked --offline` loads in **every** live checkout of each copy. If one fails, 3.5 blocks the run, so fix or remove that worktree in the copy first.
- [ ] **Step 2: Count before** (count only, no build).
  - In both copies, record `ls -f backend/target/debug/deps | wc -l`, plus the entry count and `du -sk` of `backend/target/debug/incremental`.
  - Check a sample of copied `invoked.timestamp` mtimes against the originals; they must be equal.
  - Every proof cargo command in this task runs with `--offline`.
- [ ] **Step 3: Run** `cargo hive gc --checkout <gc copy slot> --json` in the gc copy only.
  - Record the wall time with a cold snapshot (15 s target, recorded, not a bound). The time includes every `check_entry` and `remove_counted` call. A warm-snapshot rerun on the cleaned copy is labelled discovery cost.
  - Record the receipt.
  - Re-run the slot's last task's cargo commands, offline, in **both** copies. A unit counts as gc-caused only if it recompiles in the gc copy and not in the control. Expect 0 such units among units compiled inside the window.
- [ ] **Step 4: Measure the edit's test build.**
  - After Step 3, apply the same source edit to the slot in both copies.
  - Run the same offline test build in each. The control copy's time is "before" and the gc copy's is "after".
  - Record the gc copy's `deps/` count against the recorded evidence in Open Questions (104,749 → 14,753 referenced objects; 72,896 against 338,539 files; 42 s against 110 s edit wall). The card's 4.2k files and 28 s are named as the card's. None of these is pass/fail.
  - Record the `incremental/` entry count and `du -sk` in both copies again. Its share is the reopen check for the auto-tier exclusion (Open Questions).
  - If the 15 s ceiling is exceeded, profile first. The reopen path for the read is an on-demand read of each N_OSO string at its `n_strx`, with no cache.
- [ ] **Step 5: Write the PLAN.md record** with dated numbers. No commit (git-ignored).

_Requirements: CHECKOUTGC-7.1_
