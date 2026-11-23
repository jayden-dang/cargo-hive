# Tasks: Cargo and rustc reader fuzzing

Feature code: FUZZ
Status: Approved
Date: 2026-11-23
Execution-mode: continuous
Max-concurrency: 3
Requirements: ./requirements.md
Design: ./design.md

**Goal:** Exercise each interpreted Cargo/rustc reader with bounded, replayable fuzzing and independent semantic checks.

**Architecture:** Reader-local cfg(fuzzing) adapters call shared production decoding. A separate unpublished fuzz workspace supplies nine binaries; one bounded runner drives replay, local acceptance and macOS CI.

**Tech Stack:** Rust 2024, existing serde_json, cargo-fuzz 0.13.2/libfuzzer-sys, pinned tested nightly, Bash 3.2 and GitHub Actions. New dependencies are confined to the fuzz workspace under the operator's instruction to implement the proposed solution.

## Global Constraints

Canonical rules: `docs/standards/INDEX.md` SHA256 `80cd41328c0f76529b415828021ea1c9b4f4a10f79dd6ec78d6361469a9d100a`; `code.md` `62576fe2b7985bb01ff4e6c1d3ef4e35ecddee5d9725af8454b68388bedada64`; `testing.md` `50a53d5f3c8623dedfcaa848b3298cc07dda14282382859be73054ac5c4c81c1`; `errors-logging.md` `48f5baac3b52a552e4e5e8a48977374125363b822a00fcb8a82ffa73f27f1e8f`. Architecture spine `docs/architecture/INDEX.md` SHA256 `c87db5b96371c951a1b601465510e528b7ea6390dfb9e1b9764ebcfd6910b6b8` binds every task.

No production implementation before a failing behavior test. Existing supported results and public reader signatures stay stable. No source/test requirement IDs. Fuzz input cannot trigger host effects. Do not edit slot modules, checkout.rs, CLI/receipts/state, shared process helpers or the Fs trait. Local timing campaigns must not overlap ROAD-17 measurements. Each discovered correction requires a deterministic regression and reversal check. Standalone existing gate script stays unchanged.

Baseline Rustup default differs from the 1.97.1 compiler; use a scratch RUSTUP_HOME pointing to installed toolchains with its own matching default. Never change the owner's Rustup default or toolchain files.

## File Structure

| Files | Responsibility |
|---|---|
| `Cargo.toml` | Recognize cfg(fuzzing) without adding fuzz workspace members/dependencies |
| `crates/core/src/fingerprint.rs`, `crates/core/src/target.rs`, `crates/core/src/unit_path.rs` | Core bounds, shared decoding, fuzz adapters and ordinary regressions |
| `crates/seed/src/source.rs`, `crates/seed/src/fill.rs` | Seed shared decoding, adapters and ordinary regressions |
| `crates/gc/src/debug_map.rs`, `crates/gc/src/gc/plan.rs`, `crates/gc/src/gc.rs`, `crates/gc/src/liveness.rs`, `crates/gc/src/explain.rs`, `crates/gc/src/lib.rs` | GC slice bounds, adapters and ordinary regressions |
| `crates/gc/src/reader_testing.rs` | Optional cfg(test/fuzzing) read-only sparse/metadata fixture shared by GC reader tests |
| `fuzz/Cargo.toml`, `fuzz/Cargo.lock`, `fuzz/rust-toolchain.toml`, `fuzz/.gitignore`, `fuzz/targets.txt`, `fuzz/README.md` | Independent fuzz workspace, pins, target registry, limits and instructions |
| `fuzz/fuzz_targets/fingerprint.rs`, `diagnostics.rs`, `dep_info.rs`, `rustc_info.rs`, `debug_map.rs`, `target_tag.rs`, `incremental_names.rs`, `artifact_names.rs`, `timestamp_metadata.rs` | Nine thin executables under fuzz/fuzz_targets/ |
| `fuzz/corpus/<target>/seed` | One or more small synthetic seed inputs per registered target |
| `scripts/fuzz-readers.sh`, `scripts/test-fuzz-readers.sh`, `.github/workflows/fuzz.yml` | Shared bounded runner and isolated macOS CI |
| `docs/specs/catalog/hive.md`, this feature's requirements/design/tasks | Feature registration, traceability and progress |

The corpus placeholder denotes each exact registered target's seed file, not permission to edit arbitrary project files. Candidate paths are reader-local; ROAD-17 consumes them without owning these edits. Validate any newly necessary path before adding it to this map.

### Task 1: Exercise bounded core readers

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/core/src/fingerprint.rs`
- Modify: `crates/core/src/target.rs`
- Modify: `crates/core/src/unit_path.rs`

**Reuse:** rung 2 — fingerprint::read_unit/check_types/read_capped, target::identify_bounded and unit_path predicates.
**Interfaces:** Consumes existing UnitLoc, UnitHash, Unit and ProfileReport. Produces fuzz-only `fingerprint::fuzzing::{fingerprint,diagnostics}`, `target::fuzzing::target_tag`, `unit_path::fuzzing::artifact_names`, all `fn(&[u8])`, plus private shared decoder/stream helpers used by production and tests. Root lint recognizes cfg(fuzzing).
**Depends-on:** none

- [x] Add a failing oversize/stream-budget behavior test at each new hash/tag read bound, retaining empty hashes and valid tags. Cover both identify entry points and multiple short positional reads; an oversize tag returns a named Some(Err) refusal.
- [x] Run `cargo test -p cargo-hive-core --lib`; record expected RED, investigate the read path, then implement only bounded reads and minimal pure decoder extraction.
- [x] Add independent valid/corrupt/truncated/name/layout cases, including mismatched directory/hash-file identity; wire adapters to those same decoders.
- [x] Run core tests and fuzz-config core check; preserve public signatures and record correction reversal checks.
- [x] Commit `test(core): exercise and bound compiler file readers` with evidence and touched-file list.

_Requirements: FUZZ-1.2, FUZZ-1.3, FUZZ-2.2, FUZZ-2.4, FUZZ-5.2_

### Task 2: Exercise seed input classification

**Files:**
- Modify: `crates/seed/src/source.rs`
- Modify: `crates/seed/src/fill.rs`

**Reuse:** rung 2 — source::is_registry_dep_info/read_dep_info and fill::rustc_info_version/read_capped/hash_unchanged.
**Interfaces:** Consumes existing seed functions and Skip/error outcomes. Produces `source::fuzzing::dep_info` and `fill::fuzzing::rustc_info`, both `fn(&[u8])`, using shared production byte/stream decoding.
**Depends-on:** Task 1

- [x] Assert independently generated registry/workspace/escaped-space classifications, current textual-prefix behavior, the unit-dir Workspace veto, and absent/ambiguous rustc-info outputs. Matching output entries, not the number of release lines inside one entry, decides ambiguity.
- [x] Add invalid UTF-8, exact-cap, over-cap and short/error-stream cases, including the distinct fill::hash_unchanged staging hash re-reader. If a behavior correction is needed, reproduce RED before changing production decoding.
- [x] Add adapters exercising raw and structured cases without host filesystem or subprocess effects.
- [x] Run `cargo test -p cargo-hive-seed --lib` and fuzz-config seed check, then commit `test(seed): fuzz dependency and toolchain readers` with evidence.

_Requirements: FUZZ-1.1, FUZZ-2.1, FUZZ-2.6_

### Task 3: Exercise GC binary, name and timestamp readers

**Files:**
- Modify: `crates/gc/src/debug_map.rs`
- Modify: `crates/gc/src/gc/plan.rs`
- Modify: `crates/gc/src/gc.rs`
- Modify: `crates/gc/src/liveness.rs`
- Modify: `crates/gc/src/explain.rs`
- Modify: `crates/gc/src/lib.rs`
- Create if needed: `crates/gc/src/reader_testing.rs`

**Reuse:** rung 2 — debug_map::read, gc::plan classifiers, liveness::file_time/in_window and explain::read_output_texts; existing debug-map MemFs pattern.
**Interfaces:** Produces `debug_map::fuzzing::debug_map`, `gc::fuzzing::incremental_names`, `liveness::fuzzing::timestamp_metadata`, `explain::fuzzing::{diagnostics,artifact_names}`, all `fn(&[u8])`. Existing public Fs, DebugMap and MapError remain unchanged; only private thin-reader bounds change.
**Depends-on:** Task 1

- [x] Add a deterministic fat-slice fixture whose tables lie beyond the declared slice, plus checked-overflow and short-read controls; record RED with `cargo test -p cargo-hive-gc --lib debug_map` before correction.
- [x] Investigate and enforce declared slice bounds and checked offsets; prove regression reversal.
- [x] Exercise exact decoded object sets, archive counts and fat unions through sparse bounded Fs reads.
- [x] Add classifier and timestamp metadata oracles covering current and not-current timestamp paths, missing versus unreadable, future/cutoff times and extreme durations; extend aggregate diagnostics and explain artifact-mode/name checks. Explicitly re-export plan::fuzzing from gc.rs.
- [x] Run GC library tests and fuzz-config GC check, then commit `test(gc): exercise bounded binary and metadata readers` with evidence.

_Requirements: FUZZ-1.4, FUZZ-2.3, FUZZ-2.5_

### Task 4: Drive all readers through the fuzz workspace

**Files:**
- Create: `fuzz/Cargo.toml`
- Create: `fuzz/Cargo.lock`
- Create: `fuzz/rust-toolchain.toml`
- Create: `fuzz/.gitignore`
- Create: `fuzz/targets.txt`
- Create: `fuzz/README.md`
- Create: `fuzz/fuzz_targets/fingerprint.rs`
- Create: `fuzz/fuzz_targets/diagnostics.rs`
- Create: `fuzz/fuzz_targets/dep_info.rs`
- Create: `fuzz/fuzz_targets/rustc_info.rs`
- Create: `fuzz/fuzz_targets/debug_map.rs`
- Create: `fuzz/fuzz_targets/target_tag.rs`
- Create: `fuzz/fuzz_targets/incremental_names.rs`
- Create: `fuzz/fuzz_targets/artifact_names.rs`
- Create: `fuzz/fuzz_targets/timestamp_metadata.rs`
- Create: `fuzz/corpus/fingerprint/seed`
- Create: `fuzz/corpus/diagnostics/seed`
- Create: `fuzz/corpus/dep_info/seed`
- Create: `fuzz/corpus/rustc_info/seed`
- Create: `fuzz/corpus/debug_map/seed`
- Create: `fuzz/corpus/target_tag/seed`
- Create: `fuzz/corpus/incremental_names/seed`
- Create: `fuzz/corpus/artifact_names/seed`
- Create: `fuzz/corpus/timestamp_metadata/seed`
- Create: `scripts/fuzz-readers.sh`
- Create: `scripts/test-fuzz-readers.sh`
- Create: `.github/workflows/fuzz.yml`

**Reuse:** rung 3 — ordinary Rust assertions, byte slices and the production adapter functions; libfuzzer-sys drives input generation.
**Interfaces:** Consumes all nine target groups' adapters. Produces `scripts/fuzz-readers.sh <seconds> [target]`, complete target registry, pinned cargo-fuzz/nightly policy, per-target logs and replay artifacts. FUZZ_REPORT_DIR selects scratch output; CARGO_FUZZ selects an isolated cargo-fuzz executable.
**Depends-on:** Task 1, Task 2, Task 3

- [x] Wire all real adapters, generated synthetic seeds and a registry covering every inventory entry; pin nightly-2026-11-17 / rustc commit 5df8c02af and cargo-fuzz 0.13.2 and commit the separate lockfile.
- [x] Prove the runner refuses invalid durations/unknown targets and reports a deliberately failing process without swallowing its status or artifact.
- [x] Build every executable and replay seeds, then run 30-second smoke campaigns with all resource flags; retain any discovered input before minimization.
- [x] Add separate macOS PR/schedule/manual workflow using the same runner and unconditional artifact upload; document failures, limits, replay and regression promotion.
- [x] Run normal workspace/package compile controls without nightly/fuzz dependencies and commit `test(fuzz): run compiler reader campaigns in isolation`.

_Requirements: FUZZ-3.1, FUZZ-3.4, FUZZ-4.1, FUZZ-4.2, FUZZ-4.3, FUZZ-4.4, FUZZ-5.1, FUZZ-5.4, FUZZ-6.1, FUZZ-6.2, FUZZ-6.3_

### Task 5: Verify acceptance and close the branch

**Files:**
- Modify: `fuzz/README.md`
- Modify: `docs/specs/2026-11-23-reader-fuzzing/tasks.md`
- Modify: `docs/specs/2026-11-23-reader-fuzzing/requirements.md`
- Modify: `docs/specs/catalog/hive.md`

**Reuse:** rung 2 — existing per-module unit tests, workspace gates and package-check scripts.
**Interfaces:** Consumes all target artifacts and regression evidence. Produces a revision-bound acceptance report, full-range Standards/Spec review and final branch disposition. Newly discovered corrections must first be explicitly mapped into the relevant earlier task's reader file list.
**Depends-on:** Task 4

- [ ] Run each registered target for 600 seconds after replay, with at most three concurrent single-target runners, retaining source/tree/tool identity and complete results; a failed or incomplete target prevents acceptance.
- [ ] Verify ordinary regressions and reversal evidence for every correction; resolve all discovered failures with regression-first changes before rerunning affected campaigns.
- [ ] Run both toolchain gates, docs-only trace audit and package verification; independently review the whole branch and apply required polish before final acceptance.
- [ ] Report finite campaign coverage and any limitations accurately; finish through the repository's branch integration workflow only after the complete reviewed result is ready.

_Requirements: FUZZ-3.2, FUZZ-3.3, FUZZ-4.5, FUZZ-5.3_
