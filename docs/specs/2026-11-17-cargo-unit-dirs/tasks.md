# Tasks: Cargo 1.100 unit dirs

> **For agentic workers:** after plan approval, pick one execute skill —
> `build` (wave mode) (continuous dependency-aware scheduler), `build` (story mode) (human-gated story review
> units), or `build` (inline mode) (controller implements, no implementer subagents).
> The chosen skill writes `Execution-mode:`. Steps use checkbox (`- [ ]`) syntax
> for tracking.

Feature code: BUILDDIR
Status: In-progress
Approved: 2026-11-17, by the operator (Jayden), in session: "duyet plan"
Date: 2026-11-17
Execution-mode: continuous
Max-concurrency: auto
Requirements: ./requirements.md
Design: ./design.md

**Goal:** Every hive command reads, collects, cleans, seeds, explains and benches profile dirs whose
units Cargo 1.100 writes as `build/<package>/<hash>/`, beside the flat layout and in the same profile
dir, and fails closed on any other shape.

**Architecture:** A prefactor comes first. `UnitLoc { layout, name_meta }` replaces `dir_name` on
`Unit` and `UnitRef`, and one `cargo_hive_core::unit_path` module becomes the only place a unit's
paths are joined (design Decisions 1 and 2), so every consumer is ported by the compiler. Then
`read_profile` reads both layouts into one graph (Decision 3). Each command is ported on top of
that, in story order:

- status;
- doctor, with the state layout bump (Decision 5);
- dead units and plan/apply, with the ordered unit-dir entries (Decision 4);
- checkout gc;
- the temp sweep;
- seed, with the `u.` store entries (Decision 6);
- explain;
- bench.

CI on both toolchains, the spine docs and a klynt proof close it.

**Tech Stack:** Rust (edition 2024) in a workspace of eight members, with serde/serde_json and
clap 4. Real-cargo tests run whichever `cargo` is on `PATH`, so a run under
`RUSTUP_TOOLCHAIN=1.97.1` checks the flat layout and one under `RUSTUP_TOOLCHAIN=1.100.0` checks the
unit-dir layout. Synthetic profile dirs test both layouts under either toolchain.

## Global Constraints

The canonical source is `docs/standards/`, with sha256 prefixes taken 2026-11-17:
- INDEX.md `80cd41328c0f`;
- code.md `62576fe2b798`;
- testing.md `50a53d5f3c86`;
- errors-logging.md `48f5baac3b52`.

Commands, the task gate and the layer order are in `docs/agents/project.md` (verify table and task
gate). The spine is `docs/architecture/INDEX.md` `f63f7b97d2f8`. Every task inherits these:

- **Merge gate:** `scripts/gate.sh`. Each task runs the project.md task gate for the members it
  changes under `RUSTUP_TOOLCHAIN=1.97.1`. Until Task 14 lands, a run under
  `RUSTUP_TOOLCHAIN=1.100.0` covers only the tests a step names. After that, both toolchains run
  the whole gate.
- **Code rules** (code.md):
  - every walk and read has a bound, and hitting one is an error, never a silent skip;
  - `unsafe`, `libc` and `cfg(target_os)` only in `crates/platform`;
  - no new third-party dependency;
  - no requirement IDs in source, comments or test names;
  - every new module opens with `//!`.
- **Errors** (errors-logging.md): typed enums with a hand-written `Display`, and refusals are data.
  Exit codes are 0, 1, 2 and 130 as today.
- **Spine:**
  - ARCH-9: an exact signature per layout, else the profile is unsupported;
  - ARCH-17: no compile to decide liveness;
  - ARCH-19: try-lock without waiting;
  - ARCH-20: apply rechecks;
  - ARCH-3: never replace an artifact;
  - ARCH-16: state only through `state`;
  - ARCH-14: gate on doctor;
  - ARCH-22, from Task 15: ARCH-18 restated for both layouts.
- **Tests:** `HIVE_HOME` is always a scratch dir. Tests never use the default secondary roots
  (`tests/common/mod.rs` `Hive::ready`).

## File Structure

| File | Responsibility |
|---|---|
| `crates/core/src/unit_path.rs` (new) | `UnitLayout`, `UnitLoc`, every join of a unit's paths, and path predicates |
| `crates/core/src/lib.rs` | register `unit_path` |
| `crates/core/src/fingerprint.rs` | `Unit.loc`, `NotCurrentUnit.loc`, a reader for both layouts, `ProfileReport.layouts` / `partial_unit_dirs` |
| `crates/core/src/liveness.rs` | `UnitRef.loc`; `unit_timestamp` delegates to `unit_path::stamp`; `Uncertain` display |
| `crates/core/src/locks.rs` | missing-lock predicate taken from the identified `Layout` |
| `crates/core/src/target.rs` | expose the identified `Layout` to `locks` (no behavior change) |
| `crates/cargo/src/discover.rs` | status reads `layouts` / `partial_unit_dirs` |
| `crates/cargo-hive/src/cli.rs` | status line and counts; refusal hint; `unit_layout` passed to seed |
| `crates/doctor/src/lib.rs` | check (f) layout rule, check (g) via the report, `Capabilities.unit_layout` |
| `crates/state/src/lib.rs` | `Layout::NEWEST` = 2 |
| `crates/gc/src/liveness.rs` | dead unit-dir entries in Decision 4 order |
| `crates/gc/src/plan_tier/build.rs` | unit labels for unit-dir units |
| `crates/gc/src/plan_tier/apply.rs` | ordered unit-dir groups that stop on failure; `is_fingerprint_dir` via `unit_path` |
| `crates/gc/src/checkout.rs` | unit-dir dead units, `out/` objects and binaries, counts |
| `crates/gc/src/gc/run.rs` | temp entries in `out/` |
| `crates/gc/src/explain.rs` | crate and mode for unit-dir units |
| `crates/seed/src/source.rs` | unit-dir `unit_files`, `dep_class`, holds |
| `crates/seed/src/store.rs` | `unit_layout` on `StoreKey` / `Row` / `SourceUnitRef`; `u.` entry names |
| `crates/seed/src/fill.rs` | hash file via `unit_path` |
| `crates/seed/src/place.rs` | `area` by position for unit-dir; record the layout |
| `crates/seed/src/record.rs` | `SeededUnit.unit_layout`; `unit_missing` / `stamp_present` via `unit_path` |
| `crates/seed/src/choose.rs` | filter rows by `unit_layout` |
| `crates/seed/src/evict.rs` | holds of unit-dir name-metas |
| `crates/seed/src/lib.rs` | `run` / `run_clone` take `unit_layout` |
| `crates/cargo-hive/src/bench/mod.rs` | pass `unit_layout` to `run_clone` |
| `crates/cargo-hive/tests/common/unit_dirs.rs` (new) | synthetic profile-dir builder for either layout |
| `crates/cargo-hive/tests/common/mod.rs`, `cargo_fixture.rs`, `prove.rs`, `plan_fixture.rs`, `store_fixture.rs` | port helpers to `UnitLoc` / `unit_path`; layout-aware real-cargo helpers |
| `crates/cargo-hive/tests/*.rs` (status_targets, doctor, liveness, liveness_busy, gc, plan_apply, plan_tier, checkout_gc, seed, seed_fill, seed_liveness, store_gc, status_seed, explain, bench, upgrade) | per-layout tests |
| `.github/workflows/ci.yml`, `scripts/gate.sh` | gate matrix 1.97.1 / 1.100.0 |
| `docs/architecture/INDEX.md`, `docs/adr/0005-read-cargo-unit-dirs-beside-flat.md` (new), `CONTEXT.md` | ARCH-22, ADR-0005, glossary |
| `docs/specs/2026-10-25-foundation/requirements.md`, `docs/specs/2026-11-15-state-upgrade/requirements.md` | strike CORE-4.14 and UPGRADE-7.1 with successors |
| `docs/agents/project.md` | verify commands name both toolchains |
| `.skills/verify/2026-11-17-cargo-unit-dirs/PLAN.md` (local) | klynt proof record |

---

### Task 1: Unit location and one home for unit paths

**Files:**
- Create: `crates/core/src/unit_path.rs`
- Modify:
  - `crates/core/src/lib.rs`, `crates/core/src/fingerprint.rs` (`Unit` at about line 181, `NotCurrentUnit` at about 204, `read_unit` at about 358) and `crates/core/src/liveness.rs` (`UnitRef`, `unit_timestamp` at about 126, the `Uncertain` Display at about 90);
  - every `dir_name` reader in `crates/gc/src/`, `crates/seed/src/` and `crates/cargo-hive/src/`, ported mechanically;
  - the test helpers in `crates/cargo-hive/tests/common/` and every test file that reads `dir_name` or `unit_timestamp`, including `crates/gc/tests/liveness_report.rs` and `crates/cargo-hive/tests/checkout_gc.rs` (at about line 1203).
- Test: `crates/core/src/unit_path.rs` (unit tests)

**Reuse:** rung 2. It extends `core::liveness::unit_timestamp` and `fingerprint::unit_hash`. The `UnitLoc` type and the `unit_path` module are new (rung 7), because no type carries the layout today.

**Interfaces:**
- Consumes: none.
- Produces:
  - `pub enum UnitLayout { Flat, UnitDir }`, deriving Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize and Deserialize, with serde names `"flat"` / `"unit-dir"` and `impl Default` = `Flat`.
  - `pub struct UnitLoc { pub layout: UnitLayout, pub name_meta: String }`, deriving Debug, Clone, PartialEq, Eq, PartialOrd, Ord and Hash (so `UnitRef` can stay in a `BTreeSet`). `Unit` serializes it as `name_meta` and `unit_layout`; no receipt prints `dir_name`.
  - `unit_path::fingerprint_dir(profile, &UnitLoc) -> PathBuf`.
  - `hash_file(profile, &UnitLoc, stem) -> PathBuf`.
  - `stamp(profile, &UnitLoc, UnitKind) -> PathBuf`.
  - `unit_dir(profile, &UnitLoc) -> Option<PathBuf>`.
  - `outputs(profile, &UnitLoc) -> PathBuf`.
  - `flat_deps_dir(profile) -> PathBuf`.
  - `unit_dir_of(path) -> Option<PathBuf>`.
  - `is_fingerprint_dir(path) -> bool`.
  - `UnitLoc::package(&self) -> &str`, the name before the last `-<16hex>`.
  - `UnitLoc::hex(&self) -> &str`.
  - The fields `Unit.loc`, `UnitRef.loc` and `NotCurrentUnit.loc` replace `dir_name`.

**Depends-on:** none

- [x] **Step 1: Failing tests** in `unit_path::tests`. They give, for a flat and a unit-dir `UnitLoc` of the same `probe-0123456789abcdef`:
  - the fingerprint dir;
  - the hash file;
  - the stamp of a lib and of a run-build-script unit (`build/probe-…/invoked.timestamp` versus `build/probe/0123456789abcdef/run/invoked.timestamp`);
  - `unit_dir`;
  - `outputs`.

  They also cover:
  - `unit_dir_of` on paths inside and outside a unit dir, including a package named `out`;
  - `is_fingerprint_dir` on `.fingerprint/x-<hex>` and `<hex>/fingerprint`, and false on `fingerprint/` outside `build/<pkg>/<hex>`.

  Run `cargo test -p cargo-hive-core --lib unit_path::`; expect a compile failure.
- [x] **Step 2:** Implement `unit_path.rs`. Rename the fields and port every reader to `loc.name_meta` or a `unit_path` call, with flat behavior unchanged: every `Unit` built today gets `UnitLayout::Flat`. `unit_timestamp` becomes a thin call to `stamp`.
- [x] **Step 3:** Run the whole suite under `RUSTUP_TOOLCHAIN=1.97.1` (`scripts/gate.sh`). Expect green with no assertion changed: only fixture literals change, from `dir_name` to `loc`. Run `grep -rn '"\.fingerprint"\|"deps"' crates/*/src | grep -v unit_path.rs` and list each remaining hit in the task report (Task 14 closes them).
- [x] **Step 4: Commit.** `refactor(core): carry a unit's layout in UnitLoc and join its paths in unit_path`

_Requirements: BUILDDIR-1.9, BUILDDIR-1.10_

---

### Task 2: Read unit dirs, mixed profile dirs and partial unit dirs

**Files:**
- Create: `crates/cargo-hive/tests/common/unit_dirs.rs`
- Modify:
  - `crates/core/src/fingerprint.rs` (`read_profile` at about 512, `never_compiled`);
  - `crates/core/src/locks.rs` (`acquire_capped`, the missing-lock test at about 57);
  - `crates/cargo-hive/tests/common/mod.rs` (register the module);
  - `crates/cargo-hive/tests/common/cargo_fixture.rs` (add `layout_of`).
- Test: `crates/core/src/fingerprint.rs` (unit tests)

**Reuse:** rung 2. It extends `read_profile`, `read_unit`, `never_compiled` and `target::profile_layout_with`, whose package-dir walk already exists.

**Interfaces:**
- Consumes: `UnitLoc`, `UnitLayout`, `unit_path::*` (Task 1).
- Produces:
  - `ProfileReport.layouts: BTreeSet<UnitLayout>` and `ProfileReport.partial_unit_dirs: usize`.
  - In `tests/common/unit_dirs.rs`:
    - `pub fn plant_unit(profile: &Path, layout: UnitLayout, package: &str, hex: &str, stem: &str, fingerprint: u64, deps: &[(u64, &str)], outputs: &[&str]) -> UnitLoc`, which writes the JSON, hash file, `invoked.timestamp` and the named outputs in `deps/` or `out/`;
    - `pub fn plant_partial(profile, package, hex)`;
    - `pub fn plant_not_current(profile, layout, package, hex, stem)`.
  - In `tests/common/cargo_fixture.rs`: `pub fn layout_of(target: &Path) -> UnitLayout`, the single layout `read_profile` found in the fixture's `debug/`. Real-cargo tests branch on it.

**Depends-on:** Task 1

- [x] **Step 1: Failing unit tests**, one behavior each:
  - a unit-dir profile reads its units and resolves an edge between two packages;
  - a mixed profile reads both layouts into one graph, with an edge from a unit-dir unit to a flat one resolved;
  - a unit-dir `fingerprint/` with only `invoked.timestamp` and `output-*` is not current;
  - an empty hash file is not current;
  - a 16-hex dir with no `fingerprint/`, or an empty one, is a partial unit dir that does not make the profile unsupported;
  - each of these makes the profile unsupported:
    - two JSONs in one `fingerprint/`;
    - a file directly in `build/`;
    - a package dir holding a non-hex dir;
    - a dir under `build/` that is neither a package dir nor named `-<16hex>`;
  - an empty package dir is accepted;
  - a flat and a unit-dir unit with the same unit hash give one edge `ambiguous` (or `DuplicateUnitHash` in liveness);
  - `layouts` is `{Flat}`, `{UnitDir}` or both.

  Invert `a_profile_without_a_fingerprint_dir_is_unsupported`, which is now supported with one unit. Run `cargo test -p cargo-hive-core --lib fingerprint::`; expect failures.
- [x] **Step 2: Failing lock test** in `locks::tests`: a unit-dir profile with no lock files is `Skipped(Missing)`. Expect failure.
- [x] **Step 3: Implement** `read_profile` as flat dirs plus unit dirs feeding one shared per-dir reader and one graph build. Implement the lock predicate from the identified `Layout`. Run the core task gate; expect pass.
- [x] **Step 4: Commit.** `feat(core): read Cargo 1.100 unit dirs beside flat fingerprints in one profile`

_Requirements: BUILDDIR-1.1, BUILDDIR-1.2, BUILDDIR-1.3, BUILDDIR-1.4, BUILDDIR-1.5, BUILDDIR-1.6, BUILDDIR-1.7_

---

### Task 3: Status names each profile's unit layouts

**Files:**
- Modify: `crates/cargo/src/discover.rs` (the `read_profile` caller at about 635), `crates/cargo-hive/src/cli.rs` (`report_targets` at about 793, the line at about 850)
- Test: `crates/cargo-hive/tests/status_targets.rs`

**Reuse:** rung 2. It uses `report_targets`, `check_format`, `probe_output_texts` and `capabilities_for`.

**Interfaces:**
- Consumes: `ProfileReport.layouts` and `partial_unit_dirs` (Task 2), and `plant_unit` / `plant_partial` (Task 2).
- Produces:
  - the receipt counts `units_flat:<profile>`, `units_unit_dir:<profile>` and `partial_unit_dirs:<profile>`, each present only when non-zero;
  - the human suffix ` (flat)` / ` (unit-dir)` / ` (flat + unit-dir)` and `, N partial`.

**Depends-on:** Task 2

- [x] **Step 1: Failing tests** in `status_targets.rs`, on synthetic registered targets:
  - a unit-dir target prints `2 units, edges 1 resolved / 0 unresolved / 0 ambiguous (unit-dir)` and has `units_unit_dir:<profile>` = 2 with no `units_flat:` key;
  - a mixed target prints `(flat + unit-dir)`;
  - a partial unit dir shows `, 1 partial` and `partial_unit_dirs:<profile>` = 1;
  - a flat-only target keeps every 0.1.2 key and line, with ` (flat)` appended.

  Run `cargo test -p cargo-hive --test status_targets`; expect failures.
- [x] **Step 2: Implement.** Run the task gate for `cargo-hive-cargo` and `cargo-hive`; expect pass.
- [x] **Step 3: Commit.** `feat(status): name each profile dir's unit layouts`

_Requirements: BUILDDIR-1.8, BUILDDIR-1.4, BUILDDIR-1.10_

---

### Task 4: Doctor passes on Cargo 1.100 and records the unit layout

**Files:**
- Modify:
  - `crates/doctor/src/lib.rs` (`check_format` at about 583, `probe_output_texts` at about 704, `Capabilities` at about 121, `capabilities_for` at about 272);
  - `crates/state/src/lib.rs` (`Layout::NEWEST` at about 20, the test at about 914);
  - `crates/cargo-hive/src/cli.rs` (the refusal text for a failed check (f));
  - `crates/cargo-hive/tests/upgrade.rs` (the first-write expectation becomes `"layout": 2`);
  - the unit tests that assert a written `"layout": 1`: `crates/cargo/src/registry.rs` (about lines 251 and 263), `crates/gc/src/plan_tier/record.rs` (about 638) and `crates/cargo/src/discover.rs` (about 1056 and 1071).
- Test: `crates/doctor/src/lib.rs` (unit tests), `crates/cargo-hive/tests/doctor.rs`, `crates/cargo-hive/tests/upgrade.rs`

**Reuse:** rung 2. It uses `report_targets`, `check_format`, `probe_output_texts` and `capabilities_for`.

**Interfaces:**
- Consumes: `ProfileReport.layouts` (Task 2).
- Produces:
  - `Capabilities.unit_layout: Option<UnitLayout>` (`#[serde(default)]`);
  - `pub fn Capabilities::unit_layout_or_flat(&self) -> Option<UnitLayout>`: the stored value; else `Flat` when check (f) has not disabled `FingerprintReading`; else `None`;
  - check (f) prints the measure `unit_layout`;
  - `Layout::NEWEST == Layout(2)`.

**Depends-on:** Task 2, Task 3

- [ ] **Step 1: Failing unit tests.**
  - On a synthetic unit-dir probe report, (f) passes and measures `unit_layout="unit-dir"`.
  - On a mixed report, (f) fails with a reason naming both layouts. On a report with no unit, it fails naming that.
  - After a passing (f), the written `capabilities.json` holds `"unit_layout": "unit-dir"` (or `"flat"`).
  - Check (g) reads `output-*` from a unit-dir not-current unit's `fingerprint/`, using the probed shape: `output-lib-gf_lib` with an E0308 line.
  - `unit_layout_or_flat` covers stored, absent with (f) passed, and absent with (f) failed.
- [ ] **Step 2: Failing upgrade tests.**
  - Each v0.1.0–v0.1.2 `capabilities.json` reads with `unit_layout_or_flat() == Some(Flat)` when its (f) passed.
  - The first write of every state file adds `"layout": 2`.
  - 0.1.2's reader refusal stays as tested (UPGRADE-3.2).

  Run `cargo test -p cargo-hive --test upgrade`; expect failures.
- [ ] **Step 3: Implement.** Under `RUSTUP_TOOLCHAIN=1.100.0`, `cargo test -p cargo-hive --test doctor` passes (f) and (g) on the real probe. Under 1.97.1 it records `unit_layout="flat"`. Run the doctor and state task gates; expect pass.
- [ ] **Step 4: Commit.** `feat(doctor): pass checks f and g on Cargo 1.100 unit dirs and record the unit layout`

_Requirements: BUILDDIR-2.1, BUILDDIR-2.2, BUILDDIR-2.3, BUILDDIR-2.4, BUILDDIR-2.5, BUILDDIR-2.6_

---

### Task 5: Liveness lists a dead unit dir as ordered entries

**Files:**
- Modify: `crates/gc/src/liveness.rs` (`assemble` at about 413, `list_dead_files` at about 380, `fingerprint_dir` at about 502, the not-current name at about 145)
- Test: `crates/gc/src/liveness.rs` (unit tests), `crates/cargo-hive/tests/liveness.rs`

**Reuse:** rung 2. It uses `liveness()`, `dead_indices` (kinds unchanged), `assemble`, `plan_for_target`, `recheck`, `removal_order` and `ledger::remove_counted`, which already removes dirs recursively.

**Interfaces:**
- Consumes: `unit_path::*`, `UnitLoc` (Task 1), and the reader (Task 2).
- Produces: for a dead unit-dir unit, `DeadUnit.files` is `[out/*.d sorted…, out/ (if present), run/ (if present), fingerprint/, <unit dir>]`.

**Depends-on:** Task 2

- [x] **Step 1: Failing tests** on synthetic targets:
  - a dead unit-dir lib unit lists exactly that order;
  - a unit with no `out/` lists `fingerprint/` and the unit dir;
  - an `out/` listing error makes the target Undecided with `ListingFailed`;
  - build-script, run-build-script, doc-lib, doc-bin, other and not-current unit-dir units are never dead, and a partial unit dir is never listed;
  - nothing under `build/` outside the unit dir is listed, so a flat build-script dir and a package dir are untouched;
  - in a mixed target, flat dead units list exactly their 0.1.2 files.

  Run `cargo test -p cargo-hive-gc --lib liveness::`; expect failures.
- [x] **Step 2: Implement.** Run the gc task gate; expect pass.
- [x] **Step 3: Commit.** `feat(gc): list a dead unit dir's files in deletion order`

_Requirements: BUILDDIR-3.1, BUILDDIR-3.2, BUILDDIR-3.4_

---

### Task 6: Plan and apply delete unit dirs whole, fingerprint last

**Files:**
- Modify: `crates/gc/src/plan_tier/build.rs` (`unit_label` at about 462), `crates/gc/src/plan_tier/apply.rs` (`removal_order` at about 927, the deletion loop at about 443, `is_fingerprint_dir` at about 986)
- Test: `crates/cargo-hive/tests/plan_apply.rs`, `crates/cargo-hive/tests/plan_tier.rs`, `crates/cargo-hive/tests/liveness_busy.rs`

**Reuse:** rung 2. It uses `liveness()`, `dead_indices` (kinds unchanged), `assemble`, `plan_for_target`, `recheck`, `removal_order` and `ledger::remove_counted`, which already removes dirs recursively.

**Interfaces:**
- Consumes: the `DeadUnit.files` order (Task 5) and `unit_path::unit_dir_of` / `is_fingerprint_dir` (Task 1).
- Produces: `PlannedEntry.unit` = `<rel_profile>/build/<pkg>/<hex>` for unit-dir. Groups in one unit dir keep plan order, are never skipped for having no `deps/`, and stop at the first failed or changed entry.

**Depends-on:** Task 5

- [ ] **Step 1: Failing tests** on synthetic registered targets, through `gc --plan` then `apply`:
  - the unit dir is gone and its package dir stays;
  - entries are deleted in plan order, checked with `tests/common/recording_fs.rs` or the `Fs` the existing apply tests use to fail a removal;
  - a failed `out/` removal leaves `fingerprint/` present;
  - a unit dir replaced after the plan (new inode) is refused as stale;
  - a unit revived after the plan (a fresh stamp) is refused;
  - `paths_deleted` and freed bytes count the entries;
  - a 0.1.2 flat plan fixture applies as before.
- [ ] **Step 2: Failing lock tests.**
  - A synthetic unit-dir apply, through the recording `Fs`, try-locks `.cargo-build-lock` and `.cargo-lock` and not `.cargo-artifact-lock`.
  - Under `RUSTUP_TOOLCHAIN=1.100.0` only (skipped by name otherwise), a real-cargo build held open by a slow `build.rs` makes `apply` report `targets_skipped_held` and delete nothing.
- [ ] **Step 3: Implement.** Run the gc and cargo-hive task gates; expect pass.
- [ ] **Step 4: Commit.** `feat(gc): plan and apply delete dead unit dirs whole, fingerprint last`

_Requirements: BUILDDIR-3.3, BUILDDIR-3.5, BUILDDIR-3.6, BUILDDIR-3.7, BUILDDIR-3.8, BUILDDIR-3.9_

---

### Task 7: Checkout gc on unit dirs and their debug objects

**Files:**
- Modify: `crates/gc/src/checkout.rs` (`unit_order` at about 95, `clean_target` at about 222, the counts at about 362, `clean_profile_objects` at about 446 with the layout refusal at about 455, `list_profile` at about 620, `push_dir_candidates`)
- Test: `crates/cargo-hive/tests/checkout_gc.rs`

**Reuse:** rung 2. It uses `clean_target`, `clean_profile_objects`, `list_profile`, `push_dir_candidates` and `debug_map::read`, which is layout-free.

**Interfaces:**
- Consumes: the `DeadUnit.files` order (Task 5), `unit_path::*` (Task 1), and `ProfileReport.layouts` (Task 2).
- Produces: `list_profile` per layout; `unit_files_deleted` adds `Removed.files` for unit-dir entries.

**Depends-on:** Task 5

- [ ] **Step 1: Failing tests.**
  - On a synthetic unit-dir target, a dead unit dir is deleted in order and `unit_files_deleted` counts its files.
  - On a 1.100 real-cargo fixture, gated on the layout the fixture got, `gc --checkout` deletes the unreferenced `out/*.rcgu.o` of a lib unit. It keeps every object a test or example binary in `out/` names, and counts `binaries_read` > 0.
  - An `out/` listing past `DEPS_CAP`, and an unreadable package dir, each give `gc.objects_undecided` naming the dir and delete no object.
  - A missing `deps/` with unit dirs present still cleans objects.
  - A flat target behaves as the existing checkout_gc tests assert.
- [ ] **Step 2: Implement.** Run the gc and cargo-hive task gates; expect pass.
- [ ] **Step 3: Commit.** `feat(gc): clean a checkout's unit dirs and their unreferenced debug objects`

_Requirements: BUILDDIR-4.1, BUILDDIR-4.2, BUILDDIR-4.3, BUILDDIR-4.4, BUILDDIR-4.5, BUILDDIR-3.3, BUILDDIR-3.6_

---

### Task 8: Sweep rustc temp entries in unit dirs

**Files:**
- Modify: `crates/gc/src/gc/run.rs` (`Runner::temp_entries` at about 307)
- Test: `crates/cargo-hive/tests/gc.rs`

**Reuse:** rung 2. It uses `Runner::temp_entries`, `is_temp_entry` and `remove_measured`.

**Interfaces:**
- Consumes: `unit_path::*` (Task 1) and the reader (Task 2).
- Produces: the temp-entry counts include `out/` entries.

**Depends-on:** Task 2

- [x] **Step 1: Failing tests**, on a synthetic unit-dir profile with `.cargo-build-lock`:
  - `out/rmetaAb12` and `out/rustcX9` dirs are deleted and counted in `temp_entries_deleted`;
  - `out/rmeta-not` and a symlink named `rmetaZ` stay;
  - with the build lock held, nothing is deleted and the target is counted `skipped_held`;
  - incremental sessions are handled as before.

  Run `cargo test -p cargo-hive --test gc`; expect failures.
- [x] **Step 2: Implement.** Run the gc and cargo-hive task gates; expect pass.
- [x] **Step 3: Commit.** `feat(gc): delete rustc temp dirs left in unit dirs`

_Requirements: BUILDDIR-5.1, BUILDDIR-5.2_

---

### Task 9: Store keys and seed records carry the unit layout

**Files:**
- Modify: `crates/seed/src/store.rs` (`StoreKey` at about 26, `Row` at about 36, `SourceUnitRef` at about 53, `entry_name` at about 118, `parse_entry_key` at about 610), `crates/seed/src/record.rs` (`SeededUnit`), `crates/cargo-hive/tests/common/store_fixture.rs`
- Test: `crates/seed/src/store.rs` (unit tests), `crates/cargo-hive/tests/upgrade.rs`, `crates/cargo-hive/tests/store_gc.rs`

**Reuse:** rung 2. It uses `live_registry_units`, `unit_files`, `dep_class`, `entry_name`/`parse_entry_key`, `place_unit`, `area`, `unit_missing`, `stamp_present`, `holds` and `hold_unsupported_fingerprint`.

**Interfaces:**
- Consumes: `UnitLayout` (Task 1) and `Layout::NEWEST` = 2 (Task 4).
- Produces:
  - `StoreKey.unit_layout`, `Row.unit_layout` and `SourceUnitRef.unit_layout`, each `#[serde(default)]` Flat;
  - `SeededUnit.unit_layout` (`#[serde(default)]`), with `dir_name` kept as the persisted serde name of its name-meta;
  - `entry_name` gives `u.{name_meta}-{fp}[~n]` for `UnitDir`, and `parse_entry_key` reads the prefix back.

**Depends-on:** Task 4

- [ ] **Step 1: Failing tests.**
  - `entry_name` and `parse_entry_key` round-trip both layouts and every variant.
  - A flat and a unit-dir row with the same toolchain value, name-meta and fingerprint hash are two keys and two entry dirs.
  - Store gc keeps a `u.` entry that a row holds, and evicts a rowless `u.` entry as it does a flat one.
  - The v0.1.0–v0.1.2 `store.db` and `seeds.json` fixtures read every row and seeded unit as Flat.
- [ ] **Step 2: Implement.** Run the seed and cargo-hive task gates; expect pass.
- [ ] **Step 3: Commit.** `feat(seed): key store entries and seed records by unit layout`

_Requirements: BUILDDIR-6.4, BUILDDIR-6.6, BUILDDIR-6.7_

---

### Task 10: Fill the store from unit-dir registry units

**Files:**
- Modify: `crates/seed/src/source.rs` (`unit_key` at about 125, `holds` at about 165, the not-current name at about 263, `hold_unsupported_fingerprint` at about 275, `unit_files` at about 425, `dep_class` at about 505), `crates/seed/src/fill.rs` (hash file at about 174 and 409), `crates/seed/src/evict.rs`
- Test: `crates/cargo-hive/tests/seed_fill.rs`, `crates/cargo-hive/tests/seed_liveness.rs`, `crates/cargo-hive/tests/store_gc.rs`

**Reuse:** rung 2. It uses `live_registry_units`, `unit_files`, `dep_class`, `entry_name`/`parse_entry_key`, `place_unit`, `area`, `unit_missing`, `stamp_present`, `holds` and `hold_unsupported_fingerprint`.

**Interfaces:**
- Consumes:
  - `StoreKey.unit_layout`, `Row.unit_layout` and the `u.` entry names (Task 9);
  - `unit_path::*` (Task 1);
  - `plant_unit` (Task 2).
- Produces: a unit-dir `SourceUnit` whose files are its unit dir minus `out/*.rcgu.o`, whose registry class comes from every `.d` in its `out/`, and whose holds cover the name-metas of an unsupported profile's unit dirs.

**Depends-on:** Task 9

- [ ] **Step 1: Failing tests.**
  - Fill on a synthetic unit-dir target with a registry lib unit and a registry build-script pair stores three `u.` entries. Each holds `fingerprint/`, `out/` minus `*.rcgu.o`, and `run/` for the run unit.
  - A build-script unit whose `out/build_script_build.d` names only registry paths is a registry unit.
  - A workspace unit is left out.
  - A changed hash file between listing and clone aborts that unit.
  - An unsupported unit-dir profile holds `<pkg>-<hex>` against eviction.
  - A partial unit dir is never filled from.
- [ ] **Step 2: Implement.** Run the seed and cargo-hive task gates; expect pass.
- [ ] **Step 3: Commit.** `feat(seed): fill the store from Cargo 1.100 unit dirs`

_Requirements: BUILDDIR-6.1, BUILDDIR-6.2, BUILDDIR-6.5_

---

### Task 11: Seed places only entries of the checkout's unit layout

**Files:**
- Modify: `crates/seed/src/lib.rs` (`run_clone` at about 254, `run` at about 344), `crates/seed/src/choose.rs` (`choose` at about 58), `crates/seed/src/place.rs` (`area` at about 600, `place_unit` at about 356, `read_stamp` at about 566), `crates/seed/src/record.rs` (`unit_missing` at about 194, `stamp_present` at about 273), `crates/cargo-hive/src/cli.rs` (the seed command's calls to `seed::run` / `run_clone`), `crates/cargo-hive/src/bench/mod.rs` (the `run_clone` call at about 371)
- Test: `crates/cargo-hive/tests/seed.rs`, `crates/cargo-hive/tests/status_seed.rs`

**Reuse:** rung 2. It uses `live_registry_units`, `unit_files`, `dep_class`, `entry_name`/`parse_entry_key`, `place_unit`, `area`, `unit_missing`, `stamp_present`, `holds` and `hold_unsupported_fingerprint`.

**Interfaces:**
- Consumes:
  - `Capabilities::unit_layout_or_flat` (Task 4);
  - `Row.unit_layout` and `SeededUnit.unit_layout` (Task 9);
  - the filled `u.` entries (Task 10).
- Produces: `seed::run(…, unit_layout: Option<UnitLayout>)` and `seed::run_clone(…, unit_layout: Option<UnitLayout>)`. `choose(store, repo, version_text, order, unit_layout)` keeps only rows of that layout, and returns a new `Choice::NoUnitLayout` when it is `None`. Seed reports that as a refusal naming `cargo hive doctor`; `Choice::NoRow { release }` keeps its release-line meaning. `area` ranks unit-dir paths by the component after `build/<pkg>/<hex>/`: `out` 0, `run` 1, `fingerprint` 2.

**Depends-on:** Task 4, Task 10

- [ ] **Step 1: Failing tests.**
  - With capabilities recording `unit-dir`, seed into a fresh secondary checkout places only `u.` entries.
  - With `flat`, it places only flat entries from the same store.
  - A placement cut short by a failing clone leaves no `fingerprint/` without its `out/` files, and a package named `out` ranks correctly.
  - `status` counts a seeded unit-dir unit fresh, then missing after its unit dir is removed.
  - Under `RUSTUP_TOOLCHAIN=1.100.0`, a real-cargo seed then build reuses every placed unit (`seeded_recompiled` = 0).
- [ ] **Step 2: Implement.** Run the seed and cargo-hive task gates; expect pass.
- [ ] **Step 3: Commit.** `feat(seed): place only store entries in the checkout toolchain's unit layout`

_Requirements: BUILDDIR-6.3, BUILDDIR-6.8_

---

### Task 12: Explain reads unit-dir units

**Files:**
- Modify: `crates/gc/src/explain.rs` (`directory_crate` at about 298, `unit_mode` at about 324, `mode_index` at about 338, `profile_inputs` at about 688, `list_deps` at about 730)
- Test: `crates/cargo-hive/tests/explain.rs`

**Reuse:** rung 2. It uses `directory_crate` (which works on `name_meta`), `mode_index`, `read_not_current` and `profile_inputs`.

**Interfaces:**
- Consumes: `UnitLoc`, `unit_path::outputs` (Task 1), and the reader (Task 2).
- Produces: none.

**Depends-on:** Task 2

- [x] **Step 1: Failing tests** on a synthetic unit-dir target:
  - a crate built with two feature sets shows two copies in one copy group, with mode from each unit's `out/`: an rlib means build and an rmeta alone means check;
  - a failed unit-dir unit with an E0308 `output-*` is reported under `unit.failed`;
  - flat explain output is unchanged.

  Run `cargo test -p cargo-hive --test explain`; expect failures.
- [x] **Step 2: Implement.** Run the gc and cargo-hive task gates; expect pass.
- [x] **Step 3: Commit.** `feat(explain): read copies, splits and failed units in Cargo 1.100 unit dirs`

_Requirements: BUILDDIR-7.1, BUILDDIR-7.2_

---

### Task 13: Bench on Cargo 1.100

**Files:**
- Modify: none in `src/` beyond Task 11's `Ctx` plumbing
- Test: `crates/cargo-hive/tests/bench.rs`

**Reuse:** rung 1. No bench code changes. Bench primes, seeds and counts through seed and `record::counts`, which Tasks 10 and 11 port. Its build-dir check (`check.rs:164`) stays.

**Interfaces:**
- Consumes: `seed::run_clone`'s `unit_layout` parameter (Task 11).
- Produces: none.

**Depends-on:** Task 11

- [ ] **Step 1:** Run `RUSTUP_TOOLCHAIN=1.100.0 cargo test -p cargo-hive --test bench`. Expect the bench failures #33 records from the 1.100 CI run (11 of 22 bench tests) to be gone. Seeded samples place `u.` entries, with seeded unit counts > 0. If any fail, add the failing assertion as a test first and fix through seed, not bench.
- [ ] **Step 2:** Run the existing build-directory refusal test under both toolchains; expect pass.
- [ ] **Step 3: Commit.** Only if tests changed: `test(bench): run cold and seeded samples on Cargo 1.100`

_Requirements: BUILDDIR-8.1, BUILDDIR-8.2_

---

### Task 14: The suite passes on both toolchains and CI requires both

**Files:**
- Modify:
  - `.github/workflows/ci.yml` (matrix `gate (1.97.1)` / `gate (1.100.0)`; the non-blocking newest-stable job stays);
  - `scripts/gate.sh`;
  - `docs/agents/project.md` (verify commands name both toolchains);
  - `crates/cargo-hive/tests/common/prove.rs` (the lock predicate at about 1040 and 1284 via `unit_path`);
  - any remaining test file that hard-codes `.fingerprint`, `deps` or `build/<dir>` for real-cargo output.
- Test: the whole suite

**Reuse:** rung 2. It reuses the CI workflow and `scripts/gate.sh` from #22. The klynt proof follows the CHECKOUTGC Task 11 procedure (`.skills/verify/2026-11-15-checkout-gc/PLAN.md`).

**Interfaces:**
- Consumes: every earlier task.
- Produces: the required check names `gate (1.97.1)` and `gate (1.100.0)`.

**Depends-on:** Task 3, Task 6, Task 7, Task 8, Task 12, Task 13

- [ ] **Step 1:** Run `RUSTUP_TOOLCHAIN=1.100.0 scripts/gate.sh` and list each failure. Port each real-cargo assertion to `cargo_fixture::layout_of` (Task 2), never weakening what it checks on flat.
- [ ] **Step 2:** Run `grep -rn '"\.fingerprint"\|"deps"\|join("out")\|join("run")' crates/*/src | grep -v unit_path.rs`. Expect only the profile-wide listings that `unit_path::flat_deps_dir` cannot express, each named in the report, or nothing.
- [ ] **Step 3:** Run `scripts/gate.sh` under 1.97.1 and under 1.100.0; expect both green.
- [ ] **Step 4:** Update `ci.yml` to the matrix, push the branch, and confirm both jobs pass on the PR.
- [ ] **Step 5:** After merge, the operator approves `gh api` branch protection with `contexts: ["gate (1.97.1)", "gate (1.100.0)"]`.
- [ ] **Step 6: Commit.** `ci: require the gate on rustc 1.97.1 and 1.100.0`

_Requirements: BUILDDIR-9.3_

---

### Task 15: Spine, glossary and superseded requirements

**Files:**
- Create: `docs/adr/0005-read-cargo-unit-dirs-beside-flat.md`
- Modify:
  - `docs/architecture/INDEX.md` (strike ARCH-18 and add ARCH-22, worded as in design Decision 7);
  - `CONTEXT.md` (Unit, Unit hash, Stamp, Dead unit, Debug object, Linked binary and Entry cover both layouts; add Flat layout, Unit-dir layout and Unit layout; the Layout entry's note on `target::Layout` stays);
  - `docs/specs/2026-10-25-foundation/requirements.md` (strike CORE-4.14 → BUILDDIR-1.5, 1.7);
  - `docs/specs/2026-11-15-state-upgrade/requirements.md` (strike UPGRADE-7.1 → BUILDDIR-6.7);
  - `docs/adr/0001-read-cargo-internal-fingerprints.md` (a superseded-in-part line pointing to ADR-0005).
- Test: the audit-trace pass (`/tmp/trace.sh` or the audit-trace skill) shows 0 errors

**Reuse:** rung 2. It reuses the CI workflow and `scripts/gate.sh` from #22. The klynt proof follows the CHECKOUTGC Task 11 procedure (`.skills/verify/2026-11-15-checkout-gc/PLAN.md`).

**Interfaces:**
- Consumes: none.
- Produces: ARCH-22 and ADR-0005, which later designs cite.

**Depends-on:** none

- [x] **Step 1:** Write ADR-0005:
  - **Context:** 1.100 stabilised the build-dir layout, cargo issue 15010.
  - **Decision:** one reader for both layouts and one graph, `UnitLoc` and `unit_path`, and unit-dir units deleted whole with the fingerprint last.
  - **Rejected:**
    - flat-looking `dir_name`s;
    - a separate 1.100 code path;
    - refusing mixed profile dirs;
    - pinning users to 1.99.
  - **Revisit:** when Cargo changes either layout again.
- [x] **Step 2:** Edit the spine, the glossary and the two struck requirements. Run audit-trace; expect 0 errors and no new warning.
- [x] **Step 3: Commit.** `docs: ARCH-22 and ADR-0005 for Cargo 1.100 unit dirs`

_Requirements: BUILDDIR-1.7, BUILDDIR-3.4, BUILDDIR-6.7_

---

### Task 16: Proof on a klynt worktree built with Cargo 1.100

**Files:**
- Create: `.skills/verify/2026-11-17-cargo-unit-dirs/PLAN.md` (local, untracked)
- Test: manual and recorded

**Reuse:** rung 2. It reuses the CI workflow and `scripts/gate.sh` from #22. The klynt proof follows the CHECKOUTGC Task 11 procedure (`.skills/verify/2026-11-15-checkout-gc/PLAN.md`).

**Interfaces:**
- Consumes: the built branch binary.
- Produces: the proof record.

**Depends-on:** Task 14

- [ ] **Step 1:** Prepare the copies.
  1. Copy a klynt worktree to `/private/var/tmp/hive-proof-1100/a`, outside the secondary roots, with a scratch `HIVE_HOME`.
  2. Build its tests with `cargo +1.100.0 test --no-run`, then backdate the stamps 8 days (the CHECKOUTGC procedure).
  3. Make a second copy `b` as the cold baseline.
- [ ] **Step 2:** Run `cargo hive doctor` and `gc --checkout a`.
  - Time `gc --checkout` outside hive and record its wall time against 15 s.
  - Record units deleted, objects deleted and freed bytes.
  - Rerun `cargo +1.100.0 test --no-run -v` in `a` and `b` and count `Compiling` lines. Expect equal counts, which means 0 gc-caused recompiles.
- [ ] **Step 3:** Seed a fresh secondary copy `c` from `a`'s repo and build it; record `seeded_recompiled` = 0. Then run `gc --plan` and `apply` on a further backdated copy, and rerun its build; expect 0 gc-caused recompiles.
- [ ] **Step 4:** Write PLAN.md with every number and command, and summarise it on #33 without naming any other tool.

_Requirements: BUILDDIR-9.1, BUILDDIR-9.2_
