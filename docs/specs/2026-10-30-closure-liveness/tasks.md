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
| `docs/standards/testing.md` (merge gate; scratch-only fixtures under `/private/tmp/hive-it-*`; exception for the ignored proof test added 2026-10-31) | `4bc4585535454d94` |
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

_Requirements: LIVE-2.3, LIVE-2.9, LIVE-5.1, LIVE-5.2, LIVE-5.3_

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

_Requirements: LIVE-1.1, LIVE-1.3, LIVE-1.4, LIVE-1.5, LIVE-1.6, LIVE-1.7, LIVE-1.8, LIVE-1.9, LIVE-1.10_

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

_Requirements: LIVE-2.1, LIVE-2.2, LIVE-2.3, LIVE-2.5, LIVE-2.6, LIVE-2.7, LIVE-2.9, LIVE-2.10, LIVE-2.11_

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

_Requirements: LIVE-2.7, LIVE-2.8, LIVE-3.1, LIVE-3.2, LIVE-3.3, LIVE-3.5, LIVE-3.6_

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

_Requirements: LIVE-1.1, LIVE-1.3, LIVE-3.1, LIVE-5.5, LIVE-5.6, LIVE-5.7_

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

_Requirements: LIVE-4.2, LIVE-4.5, LIVE-4.6, LIVE-4.7, LIVE-4.8, LIVE-4.9, LIVE-4.10, LIVE-4.11, LIVE-4.13, LIVE-4.14, LIVE-4.15, LIVE-4.16_

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

_Requirements: LIVE-4.18, LIVE-4.19, LIVE-4.20, LIVE-4.21, LIVE-4.22_

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

_Requirements: LIVE-1.11, LIVE-2.12, LIVE-2.13, LIVE-4.23, LIVE-4.25, LIVE-4.26, LIVE-4.27_

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
