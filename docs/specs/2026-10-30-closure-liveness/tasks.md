# Tasks: Closure liveness (live and dead units of a Cargo target dir, proven on a copy)

> **For agentic workers:** after plan approval, pick one execute skill:
> - `build-in-waves`: a continuous, dependency-aware scheduler;
> - `build-by-story`: human-gated story review units;
> - `build-inline`: the controller implements, with no implementer subagents.
>
> The chosen skill writes `Execution-mode:`. Steps use checkbox (`- [ ]`) syntax for tracking.

Feature code: LIVE
Status: Approved
Date: 2026-10-30
Execution-mode: continuous
Max-concurrency: auto
Requirements: ./requirements.md
Design: ./design.md
Delivered-by: 6b75d9ca4cfbf21fe7fafd8fa9aa9c4f4a756ae8

**Goal:** Add `cargo_hive::liveness`, a read-only function that says which units of a Cargo target dir are live and which dead units' files may be collected, or why it cannot decide, and prove it with an ignored test that deletes those files in an APFS clone of a real target and replays the owner's commands.

**Architecture:**
- **`src/liveness.rs`** (new) is a pure function over an already-read `TargetDir`. It builds a target-wide fingerprint index, selects roots by unit timestamp, traverses, and either lists dead units' hash-keyed files or returns every Undecided reason.
- **`fingerprint`** gains `unit_hash` and `dep_name`.
- **`gc`** gains a public `measure`, built on its existing scan and `Ledger`.
- **The proof** is test code: `tests/common/prove.rs`, driven by `tests/liveness.rs` on fixtures in CI and by `tests/liveness_prove.rs` (`#[ignore]`) on a real target.

**Tech Stack:** Rust 2021, rustc/cargo 1.97.1 on macOS 27 (APFS). Existing dependencies only: `serde`, `serde_json`, `clap`, `libc`. No new dependency, and no dev-dependency. Mtimes are planted with std `File::set_modified`.

## Global Constraints

The canonical sources (`docs/standards/` is the SSOT). Read them; do not restate them. Hashes were taken 2026-10-30, and a mismatch at execute time means the source changed after planning.

| Source | sha256 (first 16) |
|---|---|
| `docs/standards/INDEX.md` | `80cd41328c0f7652` |
| `docs/standards/code.md` (bounds on every walk, read and child process; no new deps; no requirement IDs in code, comments or test names; `//!` module comments) | `afdcc8b89136f573` |
| `docs/standards/errors-logging.md` | `48f5baac3b52a552` |
| `docs/standards/testing.md` (merge gate; scratch-only fixtures under `/private/tmp/hive-it-*`; exception for the ignored proof test added 2026-10-31) | `4bc4585535454d94` (amended 2026-10-31 for LIVE-8: `a87c6784d5c0328a`) |
| `docs/architecture/INDEX.md`: this feature relies on ARCH-1, 9, 11, 15, 16, 17, 18, 20 | `8cb275c512d9ab3c` |
| `docs/specs/2026-10-30-closure-liveness/design.md` (Decisions 1–11) | see git |
| `.skills/LIVE/close-package.md`, `.skills/LIVE/fresh-build-trace.md`, `.skills/LIVE/design-review.md` | see git |

Merge gate (testing standard). Every task must leave all four commands green:
- `cargo fmt --check`, after `cargo fmt`.
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- `cargo check --target x86_64-unknown-linux-gnu`

Commits: each commit step is `cargo fmt`, then `git add -A && git commit -m "<subject>"`, on the feature branch the execute skill creates. No `Co-Authored-By` line.

House rules for this plan:
1. Liveness never writes, never runs a process and never touches `build/` except to read a run-build-script unit's `invoked.timestamp` (ARCH-16, ARCH-17, ARCH-18).
2. No requirement ID appears in source, comments or test names.
3. `status`, `explain`, `gc` and `snapshot.json` must not change in any task. Their existing tests are the guard (`tests/status_targets.rs`, `tests/explain.rs`, `tests/gc.rs`, and the `src/fingerprint.rs` and `src/gc/run.rs` unit tests).
4. Tests that drive the library directly (`tests/liveness.rs`) are a deliberate exception to testing.md's integration layer, which is binary-only. Liveness has no binary surface (LIVE-5.5), and the design's seam table agrees.
5. Every child process in test code (`cargo`, `cp`) runs with a deadline (`common::wait_within`), and every walk has a cap. Hitting either is a test failure, never a skip (code.md Bounds). A child whose output can exceed a pipe buffer (64 KiB) writes stdout and stderr to files in its scratch dir, never to a pipe, because `wait_within` polls `try_wait` before reading. Its output is read back with a size cap.

## File Structure

| Path | Change | Responsibility |
|---|---|---|
| `src/fingerprint.rs` | modify | `pub fn unit_hash(dir_name) -> Option<&str>` (reuses `is_hash`); `pub fn dep_name(fingerprint_dir, stem, dep) -> Option<String>` (reuses `read_capped`) |
| `src/explain.rs` | modify | `directory_suffix` calls `fingerprint::unit_hash`; nothing else |
| `src/gc/run.rs` | modify | module-level, directory-only `scan(fs, root, account_bytes)`; `Runner::remove_measured` calls it; `pub fn measure` (lstats each path: file → ledger, dir → `scan`) and `pub struct Footprint` |
| `src/gc.rs` | modify | re-export `measure`, `Footprint` |
| `src/liveness.rs` | create | `liveness`, `Liveness`, `UnitRef`, `DeadUnit`, `Uncertain`, `DEFAULT_WINDOW`, and in-module tests on synthetic fingerprint dirs |
| `src/lib.rs` | modify | `pub mod liveness;` after `git` |
| `tests/common/mod.rs` | modify | `pub mod prove;` |
| `tests/common/prove.rs` | create | `ProveInput`, `prove`, `prove_with`, `ProveReport`, `WindowReport`, `WindowOutcome`, and pure helpers (command check and insertion, rebuilt detection, tree and mtime comparison, vacuous reasons, report rendering) |
| `tests/liveness.rs` | create | real-Cargo liveness cases, the proof on a fixture crate, the CLI-surface, `snapshot.json` keys and `cargo package --list` guards |
| `tests/liveness_prove.rs` | create | the `#[ignore]` env-driven proof on a real target |
| `tests/guards.rs` | modify | a source guard: `src/liveness.rs` uses no process API |
| `docs/specs/2026-10-30-closure-liveness/proof-runs.md` | create | recorded results of the ignored proof on klynt (acceptance evidence) |

---

### Task 1: Unit hash and dependency name in the reader

Prefactor (design "Fingerprint helpers", Decisions 4 and 5). Gives liveness the unit-hash rule and a lazy dependency-name read, and makes explain use the same unit-hash rule.

**Files:**
- Modify: `src/fingerprint.rs` (`is_hash` around line 140, `read_capped` around line 182, the `deps` entry check around lines 171-176)
- Modify: `src/explain.rs` (`directory_suffix` around line 315)
- Test: `src/fingerprint.rs` (`#[cfg(test)]`); `tests/explain.rs`, `tests/status_targets.rs` (existing, unchanged)

**Reuse:** rung 2 — extends `fingerprint`: `is_hash` (:140) for the 16-hex test, `read_capped` (:182) for the JSON re-read, and the `deps` entry shape `[u64, string, bool, u64]` checked at :171-176.

**Interfaces:**
- Consumes: none new
- Produces:
  - `pub fn fingerprint::unit_hash(dir_name: &str) -> Option<&str>`: the text after the last `-`, when it is 16 lowercase hex; else `None`.
  - `pub fn fingerprint::dep_name(fingerprint_dir: &Path, stem: &str, dep: u64) -> Option<String>`: reads `<fingerprint_dir>/<stem>.json` through `read_capped`, and returns the name (element 1) of the first `deps` entry whose element 3 equals `dep`; `None` on any read or parse failure.

**Depends-on:** none

- [x] **Step 1: Write the failing tests** in `src/fingerprint.rs`:
  - `the_unit_hash_is_the_sixteen_hex_tail_of_the_dir_name`: table of `dep-4360aef667e9273e` → `Some("4360aef667e9273e")`, `build_script_build-9a4d89fe046fe1ba` → `Some(..)`, `dep-4360AEF667E9273E` → `None`, `dep-4360aef667e9273` (15) → `None`, `nodash` → `None`.
  - `a_dependency_name_is_read_back_from_the_unit_json`: the existing `unit_json` writes every name as `"dep"`, so add `unit_json_named(deps: &[(u64, &str)])` beside it, and write a unit with `[(7, "alpha"), (9, "beta")]`. Then `dep_name(dir, "lib-q", 9)` → `Some("beta")`, `dep_name(dir, "lib-q", 7)` → `Some("alpha")`, `dep_name(dir, "lib-q", 8)` → `None`, and a missing JSON → `None`.

  Run `cargo test --lib fingerprint::`; expect compile errors: `unit_hash` and `dep_name` are not found.
- [x] **Step 2: Implement** both functions; `directory_suffix` in `src/explain.rs` becomes `fingerprint::unit_hash(dir_name)`.
- [x] **Step 3: Verify.** `cargo test --lib fingerprint::` passes. `cargo test --test explain --test status_targets` passes unchanged. Run the merge gate.
- [x] **Step 4: Commit** `refactor(fingerprint): share the unit-hash rule and read a dependency's name on demand`.

_Requirements: LIVE-2.15, LIVE-2.9, LIVE-5.1, LIVE-5.2, LIVE-5.3_

---

### Task 2: gc measures any set of paths with its per-inode ledger

Prefactor (design "gc byte measurement", Decision 8).

**Files:**
- Modify: `src/gc/run.rs` (`Runner::remove_measured` around lines 356-386, `Runner::scan` around line 388)
- Modify: `src/gc.rs` (re-exports around lines 11-13)
- Test: `src/gc/run.rs` (`#[cfg(test)]`, the existing `Rec` fake); `tests/gc.rs` (existing, unchanged)

**Reuse:** rung 2 — extends `gc`: `Ledger` (src/gc/ledger.rs:27-80), and `Runner::scan` (src/gc/run.rs:388) extracted to a module-level, directory-only `scan(fs, root, account_bytes)`; `measure` handles file roots itself.

**Interfaces:**
- Consumes: `Ledger::{new, file, unmeasured, finish}`, `Fs::{entry_stat, sharing, read_dir_bounded}`, `CRATE_DIR_CAP`
- Produces:
  - `pub struct gc::Footprint { pub bytes: Bytes, pub unmeasured: u64 }` (derive `Debug, Clone, Copy, PartialEq, Eq, Default`).
  - `pub fn gc::measure(fs: &dyn Fs, target: &Path, paths: &[PathBuf], account_bytes: bool) -> Footprint`. It dedups `paths` and drops any path under another listed path. It lstats each remaining path (`Fs::entry_stat`): a regular file goes straight to one shared `Ledger` (with `Fs::sharing` when `account_bytes`), a directory goes through `scan`, and NotFound counts nothing. It returns that target's `Bytes` and the count of unmeasured entries.
  - private `fn scan(fs: &dyn Fs, root: &Path, account_bytes: bool) -> Result<Option<Scanned>, String>`: today's `Runner::scan` body, directory-only, making the same `Fs` calls in the same order.

**Depends-on:** none

- [x] **Step 1: Write the failing tests** in `src/gc/run.rs`:
  - `measure_counts_a_file_root_and_a_directory_root`: one file path and one directory path → freed equals the sum of their blocks.
  - `measure_counts_a_hardlinked_file_as_still_shared_unless_every_link_is_listed`: two links of one inode, listing one → `still_shared`; listing both → `freed`.
  - `measure_ignores_a_duplicate_and_a_nested_path`: the same file twice, and a directory plus a file inside it, give the same totals as listing each once.
  - `measure_counts_nothing_for_a_missing_path`.

  Run `cargo test --lib gc::`; expect compile errors: `measure` and `Footprint` are not found.
- [x] **Step 2: Implement.** Move `scan` out of `Runner` unchanged apart from the `account_bytes` parameter, and have `remove_measured` call it. Add `measure` and `Footprint`, and re-export both from `src/gc.rs`.
- [x] **Step 3: Verify.** `cargo test --lib gc::` passes, including the existing recorded-operation tests (e.g. the `lock, stat old, remove_tree old` sequence around lines 748-762), unchanged. `cargo test --test gc` passes unchanged. Run the merge gate.
- [x] **Step 4: Commit** `refactor(gc): let callers measure what deleting a set of paths would free`.

_Requirements: LIVE-4.9, LIVE-5.4_

---

### Task 3: Live and dead units of a target dir

Design "Liveness", flow steps 2 to 5 and 9, Decisions 1, 2 and 6. This task ships `Liveness::Decided` with live and dead units and empty `files`, and an `Undecided` only for what the flow finds without listings.

**Files:**
- Create: `src/liveness.rs`
- Modify: `src/lib.rs` (add `pub mod liveness;` after `pub mod git;`)
- Modify: `tests/guards.rs` (one new test)
- Test: `src/liveness.rs` (`#[cfg(test)]`, synthetic fingerprint dirs in a `/private/tmp/hive-it-liveness-*` scratch dir, removed on drop)

**Reuse:** none — new code (rung 7). Nothing computes reachability or lists files per unit. The inputs come from rung 2: `TargetDir`/`ProfileReport`/`Unit`/`NotCurrentUnit`, `fingerprint::{le_hex, kind_of, unit_hash, dep_name}`, `Fs::{mtime, read_dir_bounded}`, and `gc::DEPS_CAP` as the listing cap.

**Interfaces:**
- Consumes: `fingerprint::{le_hex, kind_of, unit_hash, read_profile, Unit, NotCurrentUnit, ProfileReport, Format}`, `cargo_target::{TargetDir, Owner}`, `platform::Fs`
- Produces (all `pub` in `src/liveness.rs`):
  - `const DEFAULT_WINDOW: Duration` (7 days)
  - `fn liveness(fs: &dyn Fs, target: &TargetDir, now: SystemTime, window: Duration) -> Liveness`
  - `enum Liveness { Decided { live: Vec<UnitRef>, dead: Vec<DeadUnit> }, Undecided { reasons: Vec<Uncertain> } }` (derive `Debug, Clone, PartialEq, Eq`)
  - `struct UnitRef { pub profile: PathBuf, pub dir_name: String }` (derive `Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash`)
  - `struct DeadUnit { pub unit: UnitRef, pub files: Vec<PathBuf> }` (derive `Debug, Clone, PartialEq, Eq, PartialOrd, Ord`)
  - `enum Uncertain { ProfileUnsupported { profile: PathBuf, why: String }, NotCurrentInWindow { dir: PathBuf }, Unresolved { unit: UnitRef, dependency: String }, ListingFailed { dir: PathBuf, why: String }, NoUnitHash { dir: PathBuf }, DuplicateUnitHash { a: PathBuf, b: PathBuf }, TargetUnreadable { target: PathBuf, why: String } }` (derive `Debug, Clone, PartialEq, Eq, PartialOrd, Ord`), with `fn clears_when(&self) -> &'static str` and `impl Display` (one line naming the subject, per errors-logging.md)
  - `pub fn unit_timestamp(profile: &Path, dir_name: &str, kind: &str) -> PathBuf`: `build/<dir>/invoked.timestamp` for `run-build-script`, else `.fingerprint/<dir>/invoked.timestamp`. It is public because the proof uses the same rule.
  - private `fn in_window(mtime: SystemTime, now: SystemTime, window: Duration) -> bool` (with `checked_sub`)

**Depends-on:** Task 1

- [x] **Step 1: Write the failing tests** in `src/liveness.rs`. Write a new helper, not a copy of `tests/explain.rs::write_unit`: that one uses one value for both the dir suffix and the hash content, which would hide the unit hash versus fingerprint split. The helper takes a dir name with its own 16-hex unit hash, a *different* fingerprint u64 (written with `le_hex` into the hash file), a kind stem, `deps` as `[pkg, name, false, u64]`, and a timestamp age set with `File::options().write(true).open(..)?.set_modified(..)`. Each test runs `read_profile` per profile, builds a `TargetDir` by hand, and calls `liveness` with the real `Fs`:
  - `a_unit_inside_the_window_and_everything_it_reaches_are_live`: A (age 1 h) → B → C (age 30 d), plus D (30 d) unreached; window 6 h → live A, B, C; dead D.
  - `a_dependency_in_another_profile_dir_of_the_same_target_is_reached`: a bin under `aarch64-apple-darwin/debug` depending on a proc-macro under `debug`.
  - `an_edge_matching_two_units_reaches_both`.
  - `a_unit_without_a_readable_timestamp_is_not_a_root`.
  - `a_timestamp_later_than_now_makes_a_root`, and a window larger than the time since the epoch does not panic.
  - `never_collected_kinds_are_roots_and_carry_edges_but_are_never_dead`: table over `build-script`, `run-build-script` (timestamp under `build/`), `doc-lib`, `doc-bin`, `other`.
  - `no_window_means_seven_days`: `DEFAULT_WINDOW == 7 * 86_400 s`.
  - `two_calls_on_an_unchanged_target_are_equal`, and the target tree is byte-identical before and after (no write).

  Then add a new test, `liveness_spawns_no_process`, to `tests/guards.rs`: the text of `src/liveness.rs` (via `include_str!("../src/liveness.rs")`) before its first `#[cfg(test)]` contains neither `std::process` nor `Command`. Run `cargo test --lib liveness::` and `cargo test --test guards liveness_spawns_no_process`; expect compile errors: module `liveness` not found.
- [x] **Step 2: Implement** the module per design flow steps 2–5 and 9: the target-wide `hash_hex` index across supported profiles, root selection with `unit_timestamp`/`in_window`, breadth-first traversal over `le_hex` lookups, and dead units as not-live, unit-hashed, collectible-kind current units with `files` empty for now. Open with a `//!` comment stating the read-only contract.
- [x] **Step 3: Verify.** The new tests pass. Run the merge gate.
- [x] **Step 4: Commit** `feat(liveness): decide which units of a target dir are live from the window and the fingerprint graph`.

_Requirements: LIVE-1.12, LIVE-1.3, LIVE-1.4, LIVE-1.5, LIVE-1.6, LIVE-1.7, LIVE-1.8, LIVE-1.9, LIVE-1.10_

---

### Task 4: Undecided when the graph cannot prove reachability

Design "Liveness", flow steps 1–3, 5 and 7, Decision 3.

**Files:**
- Modify: `src/liveness.rs`
- Test: `src/liveness.rs` (`#[cfg(test)]`)

**Reuse:** none — new code (rung 7). Nothing computes reachability or lists files per unit. The inputs come from rung 2: `TargetDir`/`ProfileReport`/`Unit`/`NotCurrentUnit`, `fingerprint::{le_hex, kind_of, unit_hash, dep_name}`, `Fs::{mtime, read_dir_bounded}`, and `gc::DEPS_CAP` as the listing cap.

**Interfaces:**
- Consumes: Task 3's `Liveness`, `Uncertain`, `unit_timestamp`, `in_window`; `fingerprint::dep_name`
- Produces: the `Uncertain` variants `ProfileUnsupported`, `NotCurrentInWindow`, `Unresolved`, `NoUnitHash`, `DuplicateUnitHash`, `TargetUnreadable`, each with its `clears_when` text; reasons sorted by (variant order, path), and all of them returned.

**Depends-on:** Task 3

- [x] **Step 1: Write the failing tests** in `src/liveness.rs`:
  - `an_unsupported_profile_makes_the_whole_target_undecided` (a profile dir with no `.fingerprint/`).
  - `a_not_current_unit_inside_the_window_makes_the_target_undecided`: table with a never-compiled dir (`invoked.timestamp` + `output-lib-x`), an emptied-hash dir, and a not-current run-build-script whose timestamp is under `build/`.
  - `a_not_current_unit_outside_the_window_or_without_a_timestamp_is_ignored`: Decided, and its dir is in no dead unit.
  - `an_unresolved_edge_from_a_live_unit_names_the_unit_and_the_dependency`: with the name, and with the fingerprint hex when the JSON is unreadable by then; an unresolved edge from a dead unit leaves it Decided.
  - `a_dir_without_a_unit_hash_or_two_units_sharing_one_make_the_target_undecided`.
  - `an_unreadable_target_is_undecided` (`TargetDir.unreadable = Some(..)`, no profiles).
  - `every_reason_is_reported_with_what_clears_it`: three conditions at once give three sorted reasons, each with a non-empty `clears_when`, and no dead units.

  Run `cargo test --lib liveness::`; expect these tests to fail on `Decided` results.
- [x] **Step 2: Implement** the checks, collecting into one list with no early return, and `clears_when` for each variant.
- [x] **Step 3: Verify** `cargo test --lib liveness::` passes. Run the merge gate.
- [x] **Step 4: Commit** `feat(liveness): refuse to decide, with every reason, when the graph cannot prove reachability`.

_Requirements: LIVE-2.1, LIVE-2.14, LIVE-2.15, LIVE-2.5, LIVE-2.6, LIVE-2.16, LIVE-2.9, LIVE-2.10, LIVE-2.11_

---

### Task 5: The files of each dead unit

Design "Liveness", flow steps 6 to 8, Decisions 5 and 7.

**Files:**
- Modify: `src/liveness.rs`
- Test: `src/liveness.rs` (`#[cfg(test)]`)

**Reuse:** none — new code (rung 7). Nothing computes reachability or lists files per unit. The inputs come from rung 2: `TargetDir`/`ProfileReport`/`Unit`/`NotCurrentUnit`, `fingerprint::{le_hex, kind_of, unit_hash, dep_name}`, `Fs::{mtime, read_dir_bounded}`, and `gc::DEPS_CAP` as the listing cap.

**Interfaces:**
- Consumes: Task 3's dead units; `Fs::read_dir_bounded`, `gc::DEPS_CAP`
- Produces: `DeadUnit.files` filled (`.fingerprint/<dir>` first, then matching `deps/` and `examples/` entries sorted by name, or empty); `Uncertain::ListingFailed`; private `fn carries(name: &str, unit_hash: &str) -> bool` (`-<hash>` followed by `.` or the end).

**Depends-on:** Task 4

- [x] **Step 1: Write the failing tests** in `src/liveness.rs`. Each plants entries in `deps/` and `examples/` next to synthetic units:
  - `a_dead_unit_lists_its_fingerprint_dir_and_every_entry_carrying_its_hash`: table of entries `libdep-<h>.rlib`, `libdep-<h>.rmeta`, `dep-<h>.d`, `dep-<h>`, `dep-<h>.abc.rcgu.o`, `dep-<h>.dep.<16hex>-cgu.02.rcgu.o`, `examples/ex-<h>`, `examples/ex-<h>.d` → listed. `dep-<h>x`, `<h>` (no dash), `ex` and `ex.d` (uplifted, examples root) → not listed.
  - `nothing_outside_the_units_own_deps_and_examples_is_listed`: entries with the hash under `build/`, `incremental/`, the profile root, and another profile dir stay unlisted.
  - `an_entry_carrying_the_hash_of_a_unit_that_is_not_dead_is_not_listed`: a table where the other hash belongs to a live unit, a never-collected unit (`build-script`), or a not-current unit dir. Each such entry stays unlisted, and the fingerprint dirs of those three units are never listed.
  - `a_dead_unit_with_no_entries_lists_no_file`: `files` is empty, and the fingerprint dir is not listed.
  - `a_listing_that_fails_makes_the_target_undecided`: `deps/` replaced by a file (not NotFound) → `ListingFailed`; a missing `examples/` is fine.
  - `file_lists_do_not_depend_on_listing_order`: a fake `Fs` returning shuffled `read_dir_bounded` results gives equal results.

  Run `cargo test --lib liveness::`; expect failures: `files` is empty.
- [x] **Step 2: Implement** the listings (only for profiles holding a dead unit, with NotFound treated as empty), `carries`, the non-dead-hash exclusion, the empty-files rule, `BTreeMap` grouping and sorting, and `ListingFailed`, added to the same reason list before the Decided/Undecided choice.
- [x] **Step 3: Verify** `cargo test --lib liveness::` passes. Run the merge gate.
- [x] **Step 4: Commit** `feat(liveness): list exactly the hash-keyed files of each dead unit`.

_Requirements: LIVE-2.16, LIVE-2.8, LIVE-3.1, LIVE-3.2, LIVE-3.3, LIVE-3.5, LIVE-3.6_

---

### Task 6: Liveness on real Cargo output

Design "Fixture coverage", real-cargo cases.

**Files:**
- Create: `tests/liveness.rs`
- Test: `tests/liveness.rs`

**Reuse:** rung 2 — `tests/common` (`Scratch`, the `Hive::build_crate`/`check_crate` command pattern), and std `File::set_modified` for backdating. The `tag_target`/`write_unit` pattern from tests/explain.rs is copied in for synthetic dirs, with deps entries and timestamps added.

**Interfaces:**
- Consumes: `cargo_hive::liveness::{liveness, Liveness, DEFAULT_WINDOW}`, `cargo_hive::{cargo_target, fingerprint, platform}`, `common::{Scratch, wait_within}`
- Consumes also: `cargo_hive::cli::Cli` with clap's `CommandFactory`
- Produces (private to `tests/liveness.rs`):
  - `fn cargo(dir: &Path, args: &[&str], target_dir: &Path)`: env scrubbed like `Hive::build_crate`, `--offline`, output to files, with a deadline.
  - `fn backdate_all(target: &Path, age: Duration)`: sets every `invoked.timestamp` under `.fingerprint/*/` and `build/*/`, opening each with `.write(true)`.
  - `fn read_target(path: &Path) -> TargetDir`: `cargo_target::identify`, where `None` fails the test, `Err` sets `unreadable`, and `Ok` runs `fingerprint::read_profile` per profile dir. `owner` is `Owner::Unresolved { reason: "test".into() }`.

**Depends-on:** Task 5

- [x] **Step 1: Write the failing tests** in `tests/liveness.rs`, each in a `/private/tmp/hive-it-liveness-*` scratch workspace of path-only crates:
  - `a_proc_macro_in_the_host_profile_stays_live_for_a_target_triple_build`: `app` (bin, `build.rs`) → path proc-macro `pm`, which has a real `#[proc_macro] pub fn m(_: TokenStream) -> TokenStream` used by `app`. Build with `--target <host triple from rustc -vV>`, backdate all 30 d, then set the triple-dir bin's timestamp to now; window 6 h. The bin, its run-build-script, `pm` and the build-script compile unit are live, and nothing is dead.
  - `a_backdated_check_variant_is_dead_with_its_rmeta_and_dep_info`: `app` → lib `dep`. Run `cargo check`, backdate every timestamp 30 d, then run `cargo build --bins --examples`. The new build units are roots, and the shared build-script and proc-macro units stay old but are live through traversal. Window 6 h: the check units are dead, and their `files` are exactly their fingerprint dir plus `deps/lib*-<h>.rmeta` and `deps/*-<h>.d`.
  - `a_dead_example_lists_its_examples_entries` and `a_build_script_unit_is_never_dead_and_its_run_timestamp_in_build_is_a_root`.
  - `the_cli_has_no_new_subcommand_or_flag`: walk `Cli::command()` recursively, collecting every subcommand name (hidden ones such as `__hold-lock` and `__try-lock` included) and every argument id. The result equals the list committed in this test, written from `main` before this feature.
  - `status_writes_a_snapshot_with_only_its_two_fields`: run `cargo-hive status` on a registered fixture with `HIVE_HOME` in scratch, then parse `snapshot.json`; its top-level keys are exactly `metadata` and `unsupported_profiles`.
  - `the_package_ships_no_test_file`: `cargo package --list --allow-dirty` lists no path under `tests/`.

  Run `cargo test --test liveness`; expect the two guard tests to pass and the four liveness cases to fail only where liveness disagrees with real Cargo output (if all four pass at once, record that in the task report).
- [x] **Step 2: Fix** anything the real output exposes in `src/liveness.rs`; a new sub-case gets a synthetic unit test there first.
- [x] **Step 3: Verify** `cargo test --test liveness` passes. Run the merge gate.
- [x] **Step 4: Commit** `test(liveness): check live and dead units against real cargo builds`.

_Requirements: LIVE-1.12, LIVE-1.3, LIVE-3.1, LIVE-5.5, LIVE-5.6, LIVE-5.7_

---

### Task 7: The proof procedure

Design "Proof procedure" and Decisions 9 and 10.

**Files:**
- Create: `tests/common/prove.rs`
- Modify: `tests/common/mod.rs` (add `pub mod prove;`)
- Modify: `tests/liveness.rs` (new tests)
- Test: `tests/liveness.rs`

**Reuse:** none — new code (rung 7) for the procedure. It reuses `liveness::liveness`, `gc::measure`, `cargo_target::identify` + `fingerprint::read_profile` (it builds the `TargetDir` by hand, since no function does that for one path), `platform::current()` for `Fs` (`volume`, `try_lock_exclusive`), `serde_json`, the system `cp`, and the tree-walk pattern of `tests/guards.rs`.

**Interfaces:**
- Consumes: `cargo_hive::liveness::{liveness, unit_timestamp, Liveness, DeadUnit, UnitRef}`, `cargo_hive::gc::{measure, Footprint}`, `cargo_hive::platform::{current, Fs, TryLock, VolumeInfo}`, `common::{Scratch, wait_within}`
- Produces (in `tests/common/prove.rs`):
  - `pub struct ProveInput { pub target: PathBuf, pub checkout: PathBuf, pub cmds: Vec<Vec<String>>, pub windows: Vec<Duration>, pub now: SystemTime }`
  - `pub fn prove(input: &ProveInput) -> Result<ProveReport, String>`, which calls `prove_with(&*platform::current()?.fs, &Scratch::new("prove"), input)`
  - `pub fn prove_with(fs: &dyn Fs, scratch: &Path, input: &ProveInput) -> Result<ProveReport, String>`, where `Err` is a test-level failure (bad command, lock held, not APFS or not the same volume, original changed)
  - `pub struct ProveReport { pub baseline_rebuilt: Vec<String>, pub windows: Vec<WindowReport> }` *(replaced in Task 9 by `warm_up` and `baseline` phase reports)*, with `fn all_vacuous(&self) -> bool` and `fn render(&self) -> String` (the test prints it)
  - `pub enum WindowOutcome { Passed, Vacuous(String), RebuiltLive(Vec<String>) }`
  - `pub struct WindowReport { pub window: Duration, pub outcome: WindowOutcome, pub dead_units: usize, pub dead_bytes: Footprint, pub unmapped: Footprint, pub rebuilt: usize, pub rebuilt_live: usize, pub reasons: Vec<String>, pub commands: Vec<(Vec<String>, i32)> }`
  - pure helpers, all `pub`:
    - `check_command(cmd: &[String]) -> Result<usize, String>`: the subcommand index, or why the vector is refused.
    - `with_json(cmd: &[String]) -> Vec<String>`
    - `type Stamps = BTreeMap<(PathBuf, String), Option<SystemTime>>`, keyed by (profile relative to the target, dir name), and `stamps(fs: &dyn Fs, target: &TargetDir) -> Stamps`, which uses `unit_timestamp`.
    - `rebuilt(before: &Stamps, after: &Stamps) -> Vec<(PathBuf, String)>`
    - `rebuilt_live(rebuilt: &[(PathBuf, String)], live: &BTreeSet<(PathBuf, String)>) -> Vec<String>`
    - `mtime_mismatches(original: &Stamps, clone: &Stamps) -> Vec<(PathBuf, String)>`
    - `type Tree = BTreeMap<PathBuf, (SystemTime, u64)>`, `walk(root: &Path, cap: usize) -> Result<Tree, String>` (metadata only, never contents), and `tree_changes(before: &Tree, after: &Tree) -> Vec<PathBuf>`
    - `vacuous_reason(result: &Liveness, exits: &[i32], artifact_lines: &[usize]) -> Option<String>`
    - `outcome(rebuilt_live: &[String], vacuous: Option<String>) -> WindowOutcome`

**Depends-on:** Task 2, Task 6

- [x] **Step 1: Write the failing helper tests** in `tests/liveness.rs`:
  - `commands_are_checked_and_json_is_inserted_after_the_subcommand`: a table where `["build"]` → 0; `["+stable","build","--offline"]` → 1; `["-q","check"]` → 1. `["test","--","--nocapture"]` becomes `["test","--message-format=json","--","--nocapture"]`. These are refused: `[]`; `["--offline"]`; and any vector carrying `--message-format`, `--message-format=short`, `--target-dir x`, `--target-dir=x` or `--artifact-dir=x` before `--`. `["test","--","--target-dir"]` is accepted.
  - `rebuilt_units_are_new_dirs_or_changed_timestamps`, and `a_rebuilt_live_unit_fails_the_window`: hand-made maps where a changed key in the live set gives `RebuiltLive`, and one outside it gives `Passed`.
  - `a_window_is_vacuous_when_undecided_empty_failed_or_silent`: a table over `vacuous_reason` with Undecided, Decided with no dead unit, an exit of 101, and a command with 0 artifact lines, each giving a reason. A Decided result with dead units, all exits 0 and lines > 0 gives `None`.
  - `clone_timestamps_and_original_trees_are_compared_exactly`: `mtime_mismatches` and `tree_changes` find one changed mtime, one size change, one added path and one removed path.
  - `all_vacuous_and_render_name_each_window_command_and_exit`: `render` contains every command's argv and exit status, and `all_vacuous` is true only when every outcome is Vacuous.

  Run `cargo test --test liveness`; expect compile errors: module `prove` not found.
- [x] **Step 2: Write the failing procedure tests** in `tests/liveness.rs`. Each uses a fixture in which `check` and then `build --bins --examples` ran and the check variant was backdated, with commands `[["build","--offline"]]`:
  - `the_proof_passes_a_window_whose_dead_units_cargo_does_not_need_and_calls_an_empty_window_vacuous`: the 6 h window is `Passed` with `dead_units > 0`, `dead_bytes.bytes.freed > 0`, `unmapped.bytes.freed > 0` and `rebuilt_live == 0`. The 3650 d window is `Vacuous`. The original tree is unchanged, and the scratch dir holds no clone afterwards.
  - `a_held_build_lock_fails_the_proof_and_leaves_no_clone`: hold `.cargo-build-lock` with `Fs::try_lock_exclusive` from the test. Expect `Err` naming the profile dir, and an empty scratch dir.
  - `a_volume_that_cannot_clone_fails_before_copying`: `prove_with` with a wrapper `Fs` whose `volume` reports `fs_type: "hfs"`, and then a different `mount` for the scratch dir. Each gives `Err`, and nothing is copied.
  - `a_baseline_rebuild_makes_every_window_vacuous`: touch a source file of the fixture after the build, so the baseline replay rebuilds. Expect every window `Vacuous`, naming the unit.
- [x] **Step 3: Implement** `prove_with` in the design's order:
  1. Validate the commands.
  2. Lock every profile's `.cargo-build-lock`; `Gone` needs no lock.
  3. Preflight: `volume(target).fs_type == "apfs"`, and the `mount` of the target and of the scratch dir are equal.
  4. `walk` the original and stat `checkout/Cargo.lock`.
  5. Decide and `measure` every window.
  6. Baseline: clone, replay, take `stamps` and `rebuilt`, remove the clone.
  7. Per window: clone, `mtime_mismatches`, delete, `stamps`, replay, `rebuilt`, `rebuilt_live`, remove the clone.
  8. Re-walk the original, compare with `tree_changes`, then release the locks.

  Clones go in `scratch`. `cp -c -R -p` and each cargo run go through `wait_within` (30 min each), with stdout and stderr redirected to files in `scratch` and read back capped at 256 MiB. `compiler-artifact` lines are counted from that file. Walks are capped at 5,000,000 entries.
- [x] **Step 4: Verify** `cargo test --test liveness` passes. Run the merge gate.
- [x] **Step 5: Commit** `test(liveness): prove on an APFS clone that deleting dead units rebuilds no live unit`.

_Requirements: LIVE-4.2, LIVE-4.5, LIVE-4.6, LIVE-4.7, LIVE-4.8, LIVE-4.9, LIVE-4.10, LIVE-4.11, LIVE-8.5, LIVE-4.14, LIVE-8.1, LIVE-4.16_

---

### Task 8: The ignored proof on a real target, run on klynt

Design "Proof procedure", step 1, and the success signals in `.skills/LIVE/close-package.md`.

**Files:**
- Create: `tests/liveness_prove.rs`
- Create: `docs/specs/2026-10-30-closure-liveness/proof-runs.md`
- Test: `tests/liveness_prove.rs`

**Reuse:** none — new code (rung 7) for the procedure. It reuses `liveness::liveness`, `gc::measure`, `cargo_target::identify` + `fingerprint::read_profile` (it builds the `TargetDir` by hand, since no function does that for one path), `platform::current()` for `Fs` (`volume`, `try_lock_exclusive`), `serde_json`, the system `cp`, and the tree-walk pattern of `tests/guards.rs`.

**Interfaces:**
- Consumes: `common::prove::{ProveInput, prove, check_command}`
- Produces: `#[test] #[ignore] fn deleting_dead_units_rebuilds_no_live_unit_on_a_real_target()`, reading `HIVE_PROVE_TARGET`, `HIVE_PROVE_CHECKOUT` and `HIVE_PROVE_CMDS` (a JSON array of string arrays), with windows 6 h, 24 h and 7 d; plus `fn missing_vars(get: impl Fn(&str) -> Option<String>) -> Vec<&'static str>` with a non-ignored unit test

**Depends-on:** Task 7

- [x] **Step 1: Write the failing test** `missing_proof_variables_are_named` (not ignored): a table over `missing_vars` with none, one, or all three set. Run `cargo test --test liveness_prove`; expect a compile error.
- [x] **Step 2: Implement** the ignored test. It fails with the missing names, parses `HIVE_PROVE_CMDS` and fails naming any command `check_command` refuses, runs `prove`, prints the report, and fails when `prove` returns `Err`, any window is `RebuiltLive`, or `all_vacuous()`.
- [x] **Step 3: Verify.** `cargo test --test liveness_prove` passes, with the ignored test skipped. Then run `env -u HIVE_PROVE_TARGET -u HIVE_PROVE_CHECKOUT -u HIVE_PROVE_CMDS cargo test --test liveness_prove -- --ignored`, and expect a failure naming all three, which shows the ignored test really runs. Run the merge gate.
- [x] **Step 4: Acceptance on a real target (manual, owner present).** *Superseded by Task 9 Step 5: the first run on klynt (2026-10-30, `proof-runs.md`) was Vacuous because the baseline ran on a target the commands had never built, and the owner chose to accept on a scratch build of the mailgate project instead. The original text follows for reference.* With nothing building in klynt, run:
  ```
  HIVE_PROVE_TARGET=~/Developer/klynt/klynt/target \
  HIVE_PROVE_CHECKOUT=~/Developer/klynt/klynt \
  HIVE_PROVE_CMDS='[["build","--offline"],["check","--all-targets","--offline"],["test","--no-run","--offline"]]' \
  cargo test --release --test liveness_prove -- --ignored --nocapture
  ```
  The replay accepts only `build`, `check`, `test`, `doc`, `bench` and `run`, by their long names (owner decision 2026-10-30); `clippy`, `rustc` and `rustdoc` are refused. Expect the 6 h and 24 h windows `Passed` with `rebuilt_live == 0`, and the 7 d window `Vacuous` until the target is older than 7 days. Record the date, the commands, each window's report and the elapsed time in `proof-runs.md`. A `RebuiltLive` result is a stop-and-report, never something to edit away.
- [x] **Step 5: Commit** `test(liveness): run the deletion proof on a real target and record the klynt results`.

_Requirements: LIVE-4.1, LIVE-4.2, LIVE-4.8, LIVE-4.11_

---

### Task 9: Warm the target up before the baseline, then accept on a scratch build of mailgate

Owner amendment 2026-10-31, after the first klynt run (see `proof-runs.md`): the proof gains a warm-up replay in a clone, the baseline runs on a clone of the warmed-up clone, and every window clone is taken from the warmed-up clone.

**Files:**
- Modify: `tests/common/prove.rs` (the order in `prove_with`, `ProveReport`, `render`)
- Modify: `tests/liveness.rs` (tests of the new order and report)
- Modify: `docs/specs/2026-10-30-closure-liveness/proof-runs.md` (results of the acceptance run)
- Test: `tests/liveness.rs`

**Reuse:** none — new code (rung 7) for the procedure. It reuses `liveness::liveness`, `gc::measure`, `cargo_target::identify` + `fingerprint::read_profile` (it builds the `TargetDir` by hand, since no function does that for one path), `platform::current()` for `Fs` (`volume`, `try_lock_exclusive`), `serde_json`, the system `cp`, and the tree-walk pattern of `tests/guards.rs`.

**Interfaces:**
- Consumes: Task 7's `prove_with`, `prove_with_cloner`, `Cloner`, `CloneDir`, `stamps`, `rebuilt`, `mtime_mismatches`, `replay_command_with_env`, `run_logged`, `vacuous_reason`, `outcome`, `render`
- Produces:
  - `ProveReport` gains `warm_up: PhaseReport` and `baseline: PhaseReport` (replacing `baseline_rebuilt`), where `PhaseReport { rebuilt: Vec<String>, commands: Vec<(Vec<String>, i32)>, elapsed: Duration }`. `render` prints both.
  - The warmed-up clone is a `CloneDir` kept until the end of `prove_with`; the baseline clone and every window clone are `cp -c -R -p` copies of it. `mtime_mismatches` compares a window clone with the warmed-up clone's `stamps`.

**Depends-on:** Task 8

- [x] **Step 1: Write the failing tests** in `tests/liveness.rs`:
  - `the_warm_up_builds_what_the_commands_need_and_the_baseline_then_rebuilds_nothing`: a fixture whose target holds only `cargo check` output, commands `[["build","--offline"]]`. The warm-up report lists rebuilt units, the baseline report lists none, and a window is not Vacuous for "baseline rebuilt".
  - `a_baseline_that_still_rebuilds_after_the_warm_up_makes_every_window_vacuous`: a command whose output changes each run (a build script that writes the time and has no `rerun-if-changed`), so the baseline rebuilds. Every window is Vacuous, naming the unit.
  - `a_failing_warm_up_command_makes_every_window_vacuous_and_deletes_nothing`.
  - `window_clones_come_from_the_warmed_up_clone_and_are_compared_with_it`: a window clone whose unit timestamp differs from the warmed-up clone's is Vacuous (pure helper); the original is untouched and no clone remains.
  - `the_report_shows_the_warm_up_and_the_baseline`: `render` contains both phases' rebuilt counts, argv, exit status and elapsed time.

  Run `cargo test --test liveness`; expect failures because the order is still the old one.
- [x] **Step 2: Implement** the new order in `prove_with` (warm-up clone and replay; baseline clone of it; per window a clone of it), with `PhaseReport` and the report text. Keep every earlier guard: symlink and clone-root checks, canonical-parent check on delete, the allow-list, `--locked`, the env scrubs and the config scan.
- [x] **Step 3: Verify.** `cargo test --test liveness` and `cargo test --lib liveness::` pass. Run the merge gate.
- [x] **Step 4: Commit** `test(liveness): warm the target up in a clone before the baseline`.
- [x] **Step 5: Acceptance on a scratch build of mailgate (manual, owner present).**
  1. *(Not done: mailgate has no target and 526 of its 1,004 registry crates are missing offline. The owner chose cargo-hive's own target instead; see `proof-runs.md` run 2.)* Make a scratch copy of the mailgate HEAD without touching its checkout or `.git`: `git -C /Users/jayden/Developer/work/CommandOss/mailgate archive HEAD | tar -x -C /private/tmp/hive-it-mailgate-<id>/`.
  2. In the copy, build with the same commands the proof will replay, so the target holds real history: `cargo check --all-targets --offline`, then backdate every `invoked.timestamp` in the copy's target by 30 days (the copy is ours to modify), then `cargo build --offline`. If dependencies are missing offline, stop and report instead of using the network.
  3. Run the ignored test with `HIVE_PROVE_TARGET` set to the copy's `target`, `HIVE_PROVE_CHECKOUT` to the copy, and `HIVE_PROVE_CMDS='[["build","--offline"],["check","--all-targets","--offline"]]'`.
  4. Expect the 6 h and 24 h windows `Passed` with `rebuilt_live == 0`. Record the date, commands, each window's report, the warm-up and baseline reports and the elapsed time in `proof-runs.md`; a `RebuiltLive` result is a stop-and-report. Remove the scratch copy afterwards.

_Requirements: LIVE-8.4, LIVE-4.19, LIVE-4.20, LIVE-4.21, LIVE-4.22_

---

### Task 10: Fix the whole-branch review findings

Owner-visible amendment 2026-10-31 after the whole-branch review (`.skills/LIVE/review-branch-*.md`): requirements LIVE-1.2 and LIVE-2.4 are superseded because an unreadable timestamp is doubt, not absence; the replay guards that four review cycles added are written down as LIVE-4.23 to LIVE-4.27.

**Files:**
- Modify: `src/liveness.rs` (timestamp errors, non-UTF-8 names, `TimestampUnreadable`)
- Modify: `src/lib.rs`, `src/gc.rs`, `src/fingerprint.rs` (`#[doc(hidden)]` on items nothing in the binary uses yet)
- Modify: `tests/common/prove.rs`, `tests/liveness.rs`, `tests/liveness_prove.rs`, `tests/guards.rs` (as the findings list)
- Test: `src/liveness.rs`, `tests/liveness.rs`

**Reuse:** none — new code (rung 7) for the procedure. It reuses `liveness::liveness`, `gc::measure`, `cargo_target::identify` + `fingerprint::read_profile` (it builds the `TargetDir` by hand, since no function does that for one path), `platform::current()` for `Fs` (`volume`, `try_lock_exclusive`), `serde_json`, the system `cp`, and the tree-walk pattern of `tests/guards.rs`.

**Interfaces:**
- Consumes: Tasks 3 to 9
- Produces: `Uncertain::TimestampUnreadable { path, why }`; a non-UTF-8 unit dir name gives `Uncertain::NoUnitHash`; the report prints `not run` for a phase that did not run.

**Depends-on:** Task 9

- [x] **Step 1: Write the failing tests** for each finding (see `.skills/LIVE/branch-fix-report.md` for the mapping).
- [x] **Step 2: Implement** the fixes.
- [x] **Step 3: Verify.** `cargo test --lib`, `cargo test --test liveness --test liveness_prove --test guards`, and the merge gate.
- [x] **Step 4: Commit** with conventional subjects.

_Requirements: LIVE-1.11, LIVE-2.12, LIVE-2.17, LIVE-4.23, LIVE-4.25, LIVE-4.26, LIVE-4.27_

---

### Task 11: Make the replay compile-only

Owner decision 2026-10-31: the proof only needs to compile, so no test or binary of the owner's checkout may run (LIVE-4.28), and `docs/standards/testing.md` records the narrow exception for the ignored test.

**Files:**
- Modify: `tests/common/prove.rs` (`check_command`, the allow-list, the `ProveInput` doc line)
- Modify: `tests/liveness.rs`, `tests/liveness_prove.rs` (refusal tables and the example commands)
- Modify: `docs/specs/2026-10-30-closure-liveness/proof-runs.md` (the command list)
- Test: `tests/liveness.rs`

**Reuse:** none — new code (rung 7) for the procedure. It reuses `liveness::liveness`, `gc::measure`, `cargo_target::identify` + `fingerprint::read_profile` (it builds the `TargetDir` by hand, since no function does that for one path), `platform::current()` for `Fs` (`volume`, `try_lock_exclusive`), `serde_json`, the system `cp`, and the tree-walk pattern of `tests/guards.rs`.

**Interfaces:**
- Consumes: Task 7's `check_command`, `replay_args`
- Produces: `check_command` accepts exactly `build`, `check`, `doc`, and `test` or `bench` only when `--no-run` is present before any `--`; it refuses `run` and every argument after `--`. `--no-run` is not added for the owner, who must write it.

**Depends-on:** Task 10

- [x] **Step 1: Write the failing tests** in `tests/liveness.rs`: a table of accepted vectors (`["build"]`, `["check","--all-targets"]`, `["doc"]`, `["test","--no-run"]`, `["bench","--no-run","--offline"]`, `["+stable","test","--no-run"]`) and refused vectors (`["run"]`, `["test"]`, `["bench"]`, `["test","--","--nocapture"]`, `["build","--","-v"]`, `["test","--no-run","--","x"]`), each refusal naming the command; the example command lists in the ignored test's docs and `proof-runs.md` still pass `check_command`.
- [x] **Step 2: Implement** in `check_command`; update the `ProveInput` doc line (only compile steps of the owner's checkout run).
- [x] **Step 3: Verify.** `cargo test --test liveness --test liveness_prove --lib` and the merge gate.
- [x] **Step 4: Commit** `fix(liveness): replay only compile-only commands`.

_Requirements: LIVE-4.28_

---

## ROAD-5 amendment: decide a busy workspace's target dir

Owner decision 2026-10-31 (frame-change, close package `.skills/LIVE/amend-close-package.md`):
a build that is mid-way or failed keeps its doubt local instead of making the whole target dir
Undecided, and the proof locks the owner's target only while it decides and takes a snapshot.
Requirements LIVE-1.12 to 1.14, 2.14 to 2.18, 5.8, 5.9, 7.1 to 7.6 and 8.1 to 8.7; design
Decisions 12 to 17. Footers of Tasks 3 to 10 were repointed from the struck IDs to the IDs that
supersede them. Scan digest `.skills/LIVE/scan-amend.md`; design review
`.skills/LIVE/design-review-amend.md` (experiment log).

Global Constraints above apply unchanged, with `docs/standards/testing.md` at its amended hash.
Additional house rules for this group:
6. A target dir with no not-current unit decides exactly as before: every existing test in
   `src/liveness.rs` and `tests/liveness.rs` passes unedited, except assertions on `clears_when`
   and `Display` text, the two tests that assert today's not-current behavior
   (`src/liveness.rs` around lines 826 and 852, rewritten by Task 13), and the tests Task 17 lists.
9. A package id is learned only from an edge that matches a current unit, so a package whose only
   unit is not current cannot be learned (plan review, 2026-10-31). Every Decided fixture therefore
   also builds a second, current unit of the broken package (a `cargo check` or `--release` build)
   and asserts the id is learned before deciding.
7. The edge package comes only from package ids learned from matching edges; the crate name is
   never used to guess it.
8. Fixture tests that run `cargo` stay under `/private/tmp/hive-it-*` (`Scratch`), with the scratch
   `CARGO_HOME`, `--offline`, and the existing 300 s deadline per command.

### Amendment file structure

| Path | Change | Responsibility |
|---|---|---|
| `src/fingerprint.rs` | modify | `pub struct DepEdge { package, fingerprint }`; `Unit.deps: Vec<DepEdge>`; `check_types` returns `Vec<DepEdge>`; `ParsedUnit::NotCurrent` and `NotCurrentUnit` carry `deps`; edge counts read `.fingerprint` |
| `src/explain.rs` | modify | the `Unit` literal (around line 943) and `NotCurrentUnit` test literals gain the new field types; no output change |
| `src/liveness.rs` | modify | not-current nodes and last-wanted time; roots; package-learning map; unmatched-edge resolution; new `clears_when` and `NotCurrentInWindow` texts; in-module tests |
| `tests/common/cargo_fixture.rs` | create | real-cargo fixture helpers moved out of `tests/liveness.rs`: `Fixture`, `cargo`, `set_mtime`, `backdate_all`, `profile_dirs`, `host_triple`, `read_target`, `decide` |
| `tests/common/mod.rs` | modify | `pub mod cargo_fixture;` |
| `tests/liveness.rs` | modify | uses `common::cargo_fixture`; the tree-comparison and whole-run-lock tests are replaced by snapshot, listed-file and artifact-path tests |
| `tests/common/prove.rs` | modify | lock before reading; snapshot clone under the locks; release and `lock_held`; warm-up from the snapshot; listed-files, `Cargo.lock` and artifact-path checks replace the tree comparison |
| `tests/liveness_busy.rs` | create | busy-workspace fixtures: failed build, never started, renamed dependency (both branches), host dependency under `--target`, and delete-and-replay |
| `docs/specs/2026-10-30-closure-liveness/proof-runs.md` | modify | run 4 on klynt with the short lock, and the corrected sentence on the 2026-10-29 batch |

---

### Task 12: The reader keeps each edge's package id and a not-current unit's edges

Prefactor (design Decision 12). Liveness needs the package id of every edge and the `deps` of the
JSON left beside an emptied hash; status must not change.

**Files:**
- Modify: `src/fingerprint.rs` (`Unit` around line 72, `NotCurrentUnit` around line 92, `check_types` around line 174 and its `deps` shape check around lines 198-205, `ParsedUnit` around line 224, `read_unit` around lines 229-287, `read_profile` constructors around lines 445 and 472, edge counting around line 499; the test JSON helpers `unit_json*` around lines 542-592 gain a package-id parameter)
- Modify: `src/explain.rs` (the `Unit` literal around line 943; `NotCurrentUnit` literals around lines 1145, 2547, 3241 and 3247)
- Modify: `src/liveness.rs` (`mark_live` reads `dep.fingerprint`; the `NotCurrentUnit` test literal around line 1420)
- Modify: `tests/liveness.rs` (the `NotCurrentUnit` literal around line 1998)
- Test: `src/fingerprint.rs` (`#[cfg(test)]`); `tests/status_targets.rs`, `tests/explain.rs` (existing, unchanged)

**Reuse:** rung 2 — extends `fingerprint`: `is_hash` (:140) for the 16-hex test, `read_capped` (:182) for the JSON re-read, and the `deps` entry shape `[u64, string, bool, u64]` checked at :171-176.

**Interfaces:**
- Consumes: none new
- Produces:
  - `#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)] pub struct fingerprint::DepEdge { pub package: u64, pub fingerprint: u64 }` — elements 0 and 3 of a `deps` entry.
  - `Unit.deps: Vec<DepEdge>`.
  - `NotCurrentUnit.deps: Vec<DepEdge>` — from the stale JSON when `stem` is `Some`; empty for a unit that never compiled.

**Depends-on:** none

- [x] **Step 1: Write the failing tests** in `src/fingerprint.rs`: a unit whose JSON has `deps` `[[11, "a", false, 7], [12, "b", false, 9]]` reads back as `[DepEdge { package: 11, fingerprint: 7 }, DepEdge { package: 12, fingerprint: 9 }]`; a unit with an emptied hash file lists the same edges in its `NotCurrentUnit.deps`; a never-compiled dir has empty `deps`; the per-profile `EdgeCounts` of an existing edge-count fixture are unchanged and count no not-current unit's edges. Run `cargo test --lib fingerprint::`; expect compile errors (`DepEdge` not found).
- [x] **Step 2: Implement** the type change and update every literal listed in Files.
- [x] **Step 3: Verify.** `cargo test --lib`, `cargo test --test status_targets --test explain --test liveness` pass with no expected-value edits. Run the merge gate.
- [x] **Step 4: Commit** `refactor(fingerprint): keep each edge's package id and a not-current unit's edges`.

_Requirements: LIVE-5.1, LIVE-5.2, LIVE-5.3, LIVE-5.9_

---

### Task 13: A not-current unit a command wanted is a root, and its edges are followed

Design Decision 13 and the Liveness "Flow amendment" steps 3' and 4'.

**Files:**
- Modify: `src/liveness.rs` (`collect_units` around line 199, `mark_live` around line 275, `Uncertain::clears_when` and `Display` around lines 65-120)
- Test: `src/liveness.rs` (`#[cfg(test)]`, helpers `write_unit`, `emptied_hash` (gains a hash-file age), `never_compiled`, `touch` around lines 528-810; `FakeFs` around line 1351 gains a variant whose `mtime` fails for one named path only; the tests around lines 826 and 852 that assert today's not-current behavior are rewritten to the new rule)

**Reuse:** none — new code (rung 7). Nothing computes reachability or lists files per unit. The inputs come from rung 2: `TargetDir`/`ProfileReport`/`Unit`/`NotCurrentUnit`, `fingerprint::{le_hex, kind_of, unit_hash, dep_name}`, `Fs::{mtime, read_dir_bounded}`, and `gc::DEPS_CAP` as the listing cap.

**Interfaces:**
- Consumes: Task 12's `DepEdge`, `NotCurrentUnit.deps`
- Produces: `Liveness::Decided.live` includes the not-current units that are live; `Uncertain::NotCurrentInWindow` arises only for a not-current unit without a JSON; the hash-file time is read with `fs.mtime(<fingerprint dir>/<stem>)`.

**Depends-on:** Task 12

- [x] **Step 1: Write the failing tests** (synthetic dirs):
  - a not-current unit whose unit timestamp is 2 days old but whose emptied hash file is 1 hour old is a root at a 6 h window, and a dependency reachable only through its JSON is live and not dead;
  - the same with the unit timestamp inside the window and the hash file old;
  - both files older than the window: the unit is not a root, the result is Decided, and its unit hash still keeps entries out of dead units' files;
  - a not-current unit with a JSON whose unit timestamp and hash file both read as NotFound (through `FakeFs`) is not a root and no reason;
  - a never-compiled dir with a unit timestamp inside the window gives `NotCurrentInWindow`, and one outside it gives nothing;
  - a hash-file mtime that cannot be read (the new per-path `FakeFs` variant) gives `TimestampUnreadable` naming that hash file, and its `Display` and `clears_when` speak of a file, not only a timestamp;
  - a not-current unit is never in `dead`.

  Run `cargo test --lib liveness::`; expect failures (today any not-current unit inside the window gives `NotCurrentInWindow`).
- [x] **Step 2: Implement** not-current nodes, last-wanted time, roots and traversal over their `deps`; update the `NotCurrentInWindow` `Display` ("never compiled, invoked inside the window") and its `clears_when`, and `TimestampUnreadable`'s texts.
- [x] **Step 3: Verify.** All other existing `liveness::` tests pass unedited except text assertions (house rule 6); `cargo test --test liveness`; the merge gate.
- [x] **Step 4: Commit** `feat(liveness): treat a not-current unit a command wanted as a root`.

_Requirements: LIVE-1.12, LIVE-1.13, LIVE-2.14, LIVE-2.17, LIVE-2.18, LIVE-5.8_

---

### Task 14: An unmatched edge keeps the not-current units of its package

Design Decision 14 and the Liveness "Flow amendment" steps 4'' and 5'.

**Files:**
- Modify: `src/liveness.rs` (a package-learning step before `mark_live`; `mark_live`'s unmatched branch around lines 300-317; `Uncertain::Unresolved`'s `clears_when`)
- Test: `src/liveness.rs` (`#[cfg(test)]`; `write_unit` around line 535 gains a package id per dependency, today fixed at 1)

**Reuse:** none — new code (rung 7). Nothing computes reachability or lists files per unit. The inputs come from rung 2: `TargetDir`/`ProfileReport`/`Unit`/`NotCurrentUnit`, `fingerprint::{le_hex, kind_of, unit_hash, dep_name}`, `Fs::{mtime, read_dir_bounded}`, and `gc::DEPS_CAP` as the listing cap.

**Interfaces:**
- Consumes: Task 13's not-current nodes
- Produces: a private `BTreeMap<u64, BTreeSet<String>>` from package id to normalized package names (directory name before the last `-`, `-` read as `_`), learned from every node's edges that match a current unit; `Uncertain::Unresolved { unit, dependency }` keeps its shape, `Display` and `dependency` text (`dep_name`, else the fingerprint hex).

**Depends-on:** Task 13

- [x] **Step 1: Write the failing tests** (synthetic dirs; each package id is a chosen u64):
  - a live unit's unmatched edge with package id P, where another unit's matching edge with id P points at `liba-<h1>`, marks the not-current `liba-<h2>` live and follows its JSON; the result is Decided;
  - the not-current candidate sits in another profile dir of the target and is still found;
  - `my-lib` and `my_lib` compare equal;
  - id P never learned → `Unresolved`, even when a not-current dir is named like the edge's crate name;
  - id P learned with two different names → `Unresolved`;
  - id P learned but no not-current unit of that package → `Unresolved`, with the same `Display` text as today;
  - a not-current dir with no `-` is never a candidate and never a reason;
  - `clears_when` for `Unresolved` names Cargo building the dependency or the depending unit no longer being live;
  - two runs over shuffled directory order give equal results.

  Run `cargo test --lib liveness::`; expect failures (today every unmatched edge from a live unit is `Unresolved`).
- [x] **Step 2: Implement** the learning map and the unmatched-edge branch; keep the map unused when the edge matches.
- [x] **Step 3: Verify.** `cargo test --lib`, `cargo test --test liveness`, the merge gate.
- [x] **Step 4: Commit** `feat(liveness): keep the not-current units of an unmatched edge's package`.

_Requirements: LIVE-1.14, LIVE-2.15, LIVE-2.16, LIVE-1.9_

---

### Task 15: Share the real-cargo fixture helpers

Prefactor for Tasks 16 to 18 (design "Busy-workspace fixtures", Locality: extract). No behavior
change.

**Files:**
- Create: `tests/common/cargo_fixture.rs`
- Modify: `tests/common/mod.rs` (`pub mod cargo_fixture;`)
- Modify: `tests/liveness.rs` (move `Fixture` around line 29 with `pub` fields, `fixture` :36, `write`, `manifest`, `cargo` :84, `host_triple` :110, `profile_dirs` :120, `set_mtime` :129, `backdate_all` :138, `read_target` :155, `decide` :159, `kinds` :167, `find` :183, `only` :200, `timestamp_of` :205, and the constants `CARGO_DEADLINE`, `WINDOW`, `THIRTY_DAYS`; import them)
- Test: `tests/liveness.rs` (existing, unchanged)

**Reuse:** rung 2 — `tests/liveness.rs`'s real-cargo pattern (`cargo()` isolation with a scratch `CARGO_HOME`, `--offline`, `CARGO_DEADLINE`; `backdate_all`, `set_mtime`, `read_target`, `host_triple`) moved into `tests/common` where both files need it; `tests/common/prove.rs` `prove_with` for LIVE-7.6; `Scratch` for cleanup.

**Interfaces:**
- Consumes: none
- Produces: `common::cargo_fixture::{Fixture, fixture, write, manifest, cargo, set_mtime, backdate_all, profile_dirs, host_triple, read_target, decide, kinds, find, only, timestamp_of, CARGO_DEADLINE, WINDOW, THIRTY_DAYS}` with today's behavior; plus `cargo_in(dir, target, logs, args) -> bool` (the same isolation, returning success instead of asserting it, for builds a fixture expects to fail) and `decide_at(target, now, window) -> Liveness` (no panic on Undecided). `cargo` keeps asserting success.

**Depends-on:** Task 12

- [x] **Step 1: Move** the helpers unchanged except `cargo`'s parameters; every test in `tests/liveness.rs` compiles against the moved helpers.
- [x] **Step 2: Verify.** `cargo test --test liveness` passes with the same test count as before the move; the merge gate.
- [x] **Step 3: Commit** `refactor(tests): share the real-cargo fixture helpers`.

_Requirements: LIVE-5.8_

---

### Task 16: Busy-workspace states decided on real Cargo builds

Design "Busy-workspace fixtures" and Decision 17. Each fixture is a path-only workspace (no
registry dependency) and asserts it reached its state (an empty hash file in the expected dir)
before deciding.

**Files:**
- Create: `tests/liveness_busy.rs`
- Test: `tests/liveness_busy.rs`

**Reuse:** rung 2 — `tests/liveness.rs`'s real-cargo pattern (`cargo()` isolation with a scratch `CARGO_HOME`, `--offline`, `CARGO_DEADLINE`; `backdate_all`, `set_mtime`, `read_target`, `host_triple`) moved into `tests/common` where both files need it; `tests/common/prove.rs` `prove_with` for LIVE-7.6; `Scratch` for cleanup.

**Interfaces:**
- Consumes: Task 14's liveness; Task 15's `common::cargo_fixture`
- Produces: fixture builders reused by Task 18: `failed_build(label)`, `never_started(label)`, `renamed_dep(label, with_tool: bool)`, `host_dep(label)`, each returning the workspace dir, the target dir, the commands that rebuild it, and the source edit that restores it.

**Depends-on:** Task 14, Task 15

- [x] **Step 1: Write the tests**, one per state:
  - failed build: lib `l`, bins `b` and `c` depending on it; `cargo check` the workspace, then build all; break `l`; `cargo build --bin c` fails; `l`'s build-mode hash file is empty, its check-mode unit and `b` are current, and `l`'s package id is learned; liveness is Decided and `l`'s not-current unit is live;
  - never started: `n` ← `m` ← bin, and `m` also depends on `k`; `cargo check` the workspace, then build all; touch `m`, break `n`; `cargo build -p m` fails; `m`'s build-mode hash file is empty and the bin is current; backdate `m`'s unit timestamp and every unit of `k`, the bin and the check-mode units by 2 days; at a 6 h window `m` is live as a root and `k` is live through `m`'s JSON;
  - renamed dependency, with `tool`: `app` depends on `liba2 = { package = "liba" }`, `tool` on `liba`; `cargo check -p tool`, `cargo build -p app`; break `liba`; `cargo build -p app` fails; Decided, `liba`'s not-current build-mode unit live;
  - renamed dependency, without `tool` and with no 30-day variant: the same breakage gives Undecided with an `Unresolved` reason naming `app`;
  - host dependency: proc-macro `pm` used by bin `b` and lib `o`; build all with `--target <host triple>`, and once more with `--release`, so a current `pm` unit teaches its package id; break `pm`; `cargo build --target <host triple> -p o` fails; `pm`'s debug not-current unit is in the host profile dir and is live, and `b` in the triple dir is current.

  Builds that must fail use `cargo_in` and assert it returned false; each Decided case uses `decide_at`. Run `cargo test --test liveness_busy`; expect them to pass on Task 14's code. Red-first evidence for the rule is the unit tests of Tasks 13 and 14; these fixtures prove Cargo produces the states those tests model.
- [x] **Step 2: Verify.** `cargo test --test liveness_busy` passes; the merge gate.
- [x] **Step 3: Commit** `test(liveness): decide busy-workspace states on real Cargo builds`.

_Requirements: LIVE-7.1, LIVE-7.2, LIVE-7.3, LIVE-7.4, LIVE-7.5_

---

### Task 17: The proof locks only while it decides and takes a snapshot

Design Decision 15 and the Proof "Order amendment".

**Files:**
- Modify: `tests/common/prove.rs` (`prove_with_cloner` around lines 861-1083: locks around 884-899 move before `read_target` around 882; a snapshot clone after the decisions around 930-953; `drop(locks)` right after it; warm-up clones the snapshot around 962; the final block around 1063-1076; `snapshot` around 760 and `tree_changes` around 384 removed, `walk_links` kept for the symlink refusal; `ProveReport` around 77 and `render` around 143 gain `lock_held`; `replay` around 737 checks artifact paths)
- Modify: `tests/liveness.rs` (replace the tests around lines 1048, 1268-1317, 1319-1337, 1410, 1633-1652 and 1688; keep the one around 1085, since `prove::walk` stays; update the `ProveReport` literals around lines 1111, 1553 and 1576 for `lock_held`, and the clone-label assertion around lines 1486-1544 to `["snapshot", "warm-up", "baseline", "w0"]`)
- Modify: `tests/common/prove.rs` module doc (the order of a run); a new artifact-path check beside `count_artifacts` and `run_logged` (around lines 434 and 462), leaving their signatures, which tests around lines 1143, 1941 and 1945 pin, unchanged
- Test: `tests/liveness.rs`

**Reuse:** none — new code (rung 7) for the procedure. It reuses `liveness::liveness`, `gc::measure`, `cargo_target::identify` + `fingerprint::read_profile` (it builds the `TargetDir` by hand, since no function does that for one path), `platform::current()` for `Fs` (`volume`, `try_lock_exclusive`), `serde_json`, the system `cp`, and the tree-walk pattern of `tests/guards.rs`.

**Interfaces:**
- Consumes: Task 15's `common::cargo_fixture`
- Produces: `ProveReport.lock_held: Duration` (printed by `render`); errors naming a held lock, a listed file missing from the original, a changed `Cargo.lock` of `HIVE_PROVE_CHECKOUT`, or a `compiler-artifact` file outside the replay's clone.

**Depends-on:** Task 15

- [x] **Step 1: Write the failing tests** on the existing proof fixture:
  - the profile locks are free while the warm-up runs: the recording cloner, on the label `warm-up`, takes each profile's lock in-process with `try_lock_exclusive` and succeeds; `lock_held` is printed and shorter than the whole run;
  - a lock held by another process fails the test naming the profile dir (that the locks are taken before `read_profile` reads anything is checked by review: `read_profile` reads through `std::fs`, outside the `Fs` seam);
  - the recording cloner sees the snapshot cloned from the original, and the warm-up cloned from the snapshot, never from the original;
  - removing a listed file from the original during the run (from the recording cloner, after the snapshot) fails naming it; touching an unlisted file of the original does not fail;
  - changing the checkout's `Cargo.lock` during the run fails naming it;
  - a `compiler-artifact` line whose filename lies outside the clone fails naming it (unit test on the artifact-path check);
  - no clone is left behind on pass or failure;
  - nothing in the original or the snapshot is deleted or modified: every deletion lands in a window clone (the recording cloner's destinations), and the original's listed files and `Cargo.lock` are intact at the end.

  Run `cargo test --test liveness`; expect failures.
- [x] **Step 2: Implement** the new order, the checks, and remove the tree comparison.
- [x] **Step 3: Verify.** `cargo test --test liveness --test liveness_prove`; the merge gate.
- [x] **Step 4: Commit** `fix(liveness): lock the target only while deciding and taking a snapshot`.

_Requirements: LIVE-8.1, LIVE-8.2, LIVE-8.3, LIVE-8.4, LIVE-8.5, LIVE-8.6, LIVE-8.7_

---

### Task 18: Delete and replay on the busy-workspace fixtures

Design "Busy-workspace fixtures", the delete-and-replay case.

**Files:**
- Modify: `tests/liveness_busy.rs`
- Test: `tests/liveness_busy.rs`

**Reuse:** rung 2 — `tests/liveness.rs`'s real-cargo pattern (`cargo()` isolation with a scratch `CARGO_HOME`, `--offline`, `CARGO_DEADLINE`; `backdate_all`, `set_mtime`, `read_target`, `host_triple`) moved into `tests/common` where both files need it; `tests/common/prove.rs` `prove_with` for LIVE-7.6; `Scratch` for cleanup.

**Interfaces:**
- Consumes: Task 16's fixture builders; Task 17's `prove_with`
- Produces: none new

**Depends-on:** Task 16, Task 17

- [ ] **Step 1: Write the tests:** for each Decided fixture of Task 16, backdate the check-mode (or `--release`) units it built by 30 days with `backdate_all`, so they are the dead units while still teaching the package id; break and build as in Task 16, restore the broken source, then call `prove_with` with that fixture's rebuild commands and a 6 h window. Each window is `Passed` (not Vacuous: at least one dead unit is listed and deleted), and no live unit rebuilds.
- [ ] **Step 2: Verify.** `cargo test --test liveness_busy`; the merge gate.
- [ ] **Step 3: Commit** `test(liveness): delete and replay on busy-workspace fixtures`.

_Requirements: LIVE-7.6_

---

### Task 19: Prove it on klynt with the short lock (owner present)

Acceptance (design Decisions 15 and 16). The owner chooses the time; `cargo watch` may keep
running. Only the ignored test touches klynt's target, and only under the lock while it decides
and clones.

**Files:**
- Modify: `docs/specs/2026-10-30-closure-liveness/proof-runs.md` (Command section; a run 4 section; the sentence about the 2026-10-29 batch, corrected per `.skills/LIVE/cargo-notcurrent-research.md`)
- Test: `tests/liveness_prove.rs` (existing ignored test, unchanged)

**Reuse:** none — new code (rung 7) for the procedure. It reuses `liveness::liveness`, `gc::measure`, `cargo_target::identify` + `fingerprint::read_profile` (it builds the `TargetDir` by hand, since no function does that for one path), `platform::current()` for `Fs` (`volume`, `try_lock_exclusive`), `serde_json`, the system `cp`, and the tree-walk pattern of `tests/guards.rs`.

**Interfaces:**
- Consumes: Tasks 12 to 18 merged
- Produces: the run 4 record

**Depends-on:** Task 14, Task 17, Task 18

- [ ] **Step 1: Prepare the checkout** with the owner: pick a klynt commit that compiles, extract it with `git -C ~/Developer/klynt/klynt archive <sha> | tar -x -C ~/Library/Caches/klynt-replay-<sha>` (outside `/private/tmp` and `$TMPDIR`), and agree the compile-only commands and the environment the owner's build needs (for example `SQLX_OFFLINE=true`).
- [ ] **Step 2: Run** `HIVE_PROVE_TARGET=~/Developer/klynt/klynt/target HIVE_PROVE_CHECKOUT=~/Library/Caches/klynt-replay-<sha> HIVE_PROVE_CMDS='<agreed JSON>' cargo test --release --test liveness_prove -- --ignored --nocapture`, keeping the full output in `.skills/LIVE/proof-run-4.log`.
- [ ] **Step 3: Judge.** Passed windows with 0 rebuilt live units meet the amendment's success signal. Any `RebuiltLive`, or 6 h and 1 d both Vacuous because of Undecided, is the stop condition: report it to the owner and go to `reroute-plan`, do not adjust the rule here.
- [ ] **Step 4: Record** run 4 in `proof-runs.md`: commit, commands, lock-held time, warm-up and baseline counts and times, per window the outcome, dead units, freed bytes and bytes shared with other inodes, rebuilt and rebuilt-live counts, and the Vacuous reasons; correct the 2026-10-29 sentence; then remove `~/Library/Caches/klynt-replay-<sha>`.
- [ ] **Step 5: Commit** `docs(LIVE): record the short-lock proof run on klynt`.

_Requirements: LIVE-4.1, LIVE-8.1, LIVE-8.3, LIVE-1.12, LIVE-1.14_
