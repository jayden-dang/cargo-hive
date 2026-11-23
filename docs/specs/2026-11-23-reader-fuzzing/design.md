# Design: Cargo and rustc reader fuzzing

Feature code: FUZZ
Status: Approved
Date: 2026-11-23
Requirements: ./requirements.md

## Context

Hive interprets compiler output to classify units and collect files. Existing tests cover observed formats, but coverage-guided input exploration and independent semantic oracles are absent. Source inventory and upstream documentation are recorded in the local research note; durable coverage lives in `fuzz/README.md` and `fuzz/targets.txt`.

The implementation reuses production parsers, their current refusal types, and the existing positional-read Fs interface. It adds no public runtime command, persisted format, global toolchain change or normal-workspace fuzz dependency. A raw-byte-only harness was rejected because a parser can return an incorrect successful result without crashing.

## Decisions

1. Use cargo-fuzz 0.13.2 and libfuzzer-sys in an independent unpublished workspace. Pin nightly-2026-11-17 (rustc 1.102.0-nightly, commit 5df8c02af); normal gates stay on Rust 1.97.1 and 1.100.0.
2. Use format-specific truncation expectations; an empty hash remains NotCurrent and a complete JSONL record remains valid.
3. Put fuzz-only adapters behind `cfg(fuzzing)`, with one shared production decoder per behavior. Do not bypass checks under that configuration.
4. Include bounded hash/tag reads and declared Mach-O slice bounds as required input contracts, proven by failing regressions before implementation. Other discovered corrections require the same investigation and regression discipline.
5. Run local acceptance with at most three concurrent single-target runners, each completing its full 600-second mutation budget after replay, and never overlap ROAD-17 timing measurements. The command without a selector still runs targets serially. Keep production slot orchestration and shared Fs/process helpers outside the change. Operator amendment: 2026-11-23, “Run three concurrently (recommended)”.

## Architecture

### Core reader adapters and bounds

Satisfies: FUZZ-1.2, FUZZ-1.3, FUZZ-2.2, FUZZ-2.4, FUZZ-5.2
Reuse: rung 2 — fingerprint::read_unit/check_types/read_capped, target::identify_bounded and unit_path predicates.
Respects: ARCH-9, ARCH-15
Surface: core reader callers in discovery, GC and seed are frozen; retain signatures and supported results, withholding classification only on oversized malformed input. Existing core unit tests are extended.
Interface: proposed fuzz-only `fingerprint::fuzzing::{fingerprint,diagnostics}`, `target::fuzzing::target_tag`, `unit_path::fuzzing::artifact_names`, each `fn(&[u8])`. Extract private byte/stream decoding as needed; adapters return no production data and assert their own independent model expectations.
Depth: n/a; extends core readers.
Locality: extract decoding from I/O in fingerprint and target; extend core-local tests and adapters; leave public reader interfaces unchanged.

Each adapter exercises arbitrary bytes and a small structured case derived from bytes. The fingerprint adapter also reaches dependency-name extraction and feature coverage, including hash-file/directory identity mismatch. Read hash files through a 17-byte bounded stream (16 valid bytes plus one overflow sentinel); tag reads use a 1 MiB plus one sentinel bound in both identify and identify_bounded. Accumulate short positional reads until EOF or the sentinel limit; never accept the first short prefix as the complete file. Only oversize yields Some(Err(named bound reason)); missing, unrecognized and other I/O-error tags preserve their legacy None mapping. This avoids changing discovery recursion for a directory or unreadable entry masquerading as CACHEDIR.TAG. The bounded identify path uses Fs::read_at, not unbounded Fs::read. Add explicit lint recognition for cfg(fuzzing) in the root manifest.

### Seed reader adapters

Satisfies: FUZZ-1.1, FUZZ-2.1, FUZZ-2.6
Reuse: rung 2 — source::is_registry_dep_info/read_dep_info and fill::rustc_info_version/read_capped/hash_unchanged.
Respects: ARCH-9, ARCH-15, ARCH-21
Surface: source classification, fill::toolchain_row and seed integration callers are frozen; preserve missing/workspace/registry and toolchain refusal outcomes. Existing source/fill tests are extended.
Interface: proposed fuzz-only `source::fuzzing::dep_info` and `fill::fuzzing::rustc_info`, each `fn(&[u8])`, invoking shared production decoding and bounded-stream helpers.
Depth: n/a; extends seed readers.
Locality: extract tiny private decoding/stream helpers in source/fill; no seed orchestration, store schema or public API change.

Independent generated first-prerequisite cases cover escaped spaces and registry-prefix boundaries. Preserve the current textual prefix classification: an apparent truncated path that still satisfies that grammar cannot be distinguished from a valid token. Assert the unit-dir aggregation Workspace veto when any .d belongs to the workspace. Do not invent a requirement that every dep-info prefix is rejected. The rustc_info adapter also exercises the distinct bounded hash_unchanged store-staging re-reader through its shared stream decoder. Rustc-info cases encode independently expected whole-stdout selection: exactly one output entry contains release: anywhere; multiple matching lines inside that one entry do not create a second output. Include absent and ambiguous output entries. Invalid byte paths still run UTF-8/read-limit validation. Do not assert universal corruption detection or change Cargo's input grammar as part of test scaffolding.

### GC reader and metadata adapters

Satisfies: FUZZ-1.4, FUZZ-2.3, FUZZ-2.5
Reuse: rung 2 — debug_map::read, gc::plan classifiers, liveness::file_time/in_window and explain::read_output_texts; existing debug-map MemFs pattern.
Respects: ARCH-9, ARCH-15, ARCH-19, ARCH-22
Surface: checkout::clean_profile_objects's debug_map::read call is frozen; preserve DebugMap and MapError interfaces. Public GC and liveness entry points are frozen. Existing GC reader unit tests are extended.
Interface: proposed fuzz-only `debug_map::fuzzing::debug_map`, `gc::fuzzing::incremental_names`, `liveness::fuzzing::timestamp_metadata`, `explain::fuzzing::{diagnostics,artifact_names}`, each `fn(&[u8])`. A test-only read model may live in `reader_testing.rs`; it implements Fs without exposing host mutation.
Depth: n/a; extends GC readers and test fixtures.
Locality: extend the reader modules and test-only fixture; leave checkout.rs, slot code and the production Fs trait unchanged.

Mach-O uses checked base/range arithmetic and an optional declared slice boundary inside private thin parsing. Reject fat-slice headers, commands and tables extending past that boundary, including offset-plus-length overflow. A sparse fake represents extreme offsets and short reads without giant allocations. Metadata cases distinguish NotFound from permission errors and exercise exact cutoff/future/overflow times. Both current-unit and NotCurrentNode::last_wanted timestamp paths are exercised. Artifact-name cases include explain::mode_index, unit_mode, directory_crate and crate_and_kind, with independently expected Build/Check/Unknown outcomes. gc.rs explicitly re-exports the private plan module's fuzz adapter under cfg(fuzzing). Session classifiers operate on modeled names and entry kinds and never call deletion.

### Fuzz workspace and corpus

Satisfies: FUZZ-3.1, FUZZ-4.1, FUZZ-5.1, FUZZ-5.4, FUZZ-6.2
Reuse: rung 3 — ordinary Rust assertions, byte slices and the production adapter functions; libfuzzer-sys drives input generation.
Respects: ARCH-1, ARCH-2, ARCH-15
Surface: normal workspace members, published crate dependency sets and package lists are frozen; fuzz is a separate workspace and never included in normal member manifests.
Interface: nine executables named in `fuzz/targets.txt`; each invokes its owning reader adapter. The diagnostics executable invokes both core line and GC aggregate adapters; artifact_names invokes both core and explain adapters. `fuzz/Cargo.lock` freezes fuzz dependency resolution.
Depth: callers need only a target name, seed files and resource policy; the workspace hides instrumentation and adapter wiring.
Locality: new fuzz workspace, seed corpus, dictionaries where useful and documentation; no parser copies.

The registry documents every interpreted surface, actual reader entry point, expected outcomes and regression location. Seed files are small and synthetic. Binary input is passed as bytes; string-only production entry points receive only valid UTF-8 while byte-to-text adapters exercise invalid UTF-8. New corpus discoveries are retained separately from checked-in seeds.

### Runner and CI

Satisfies: FUZZ-3.4, FUZZ-4.2, FUZZ-4.3, FUZZ-4.4, FUZZ-4.5, FUZZ-6.1, FUZZ-6.3
Reuse: rung 3 — shell sequential execution and libFuzzer resource/replay flags; existing macOS CI conventions.
Respects: ARCH-1, ARCH-2
Surface: `.github/workflows/ci.yml` is frozen; add a separate workflow. Fuzz reports are external test artifacts, never hive receipts or measured product findings.
Interface: proposed `scripts/fuzz-readers.sh <seconds> [target]` runs replay then mutation, exiting nonzero on any failed target while retaining per-target artifacts and reports. Environment variables select a scratch report directory and isolated cargo-fuzz binary; safe defaults are inside fuzz/.
Depth: the caller supplies duration and optional target; the runner hides target enumeration, replay, resource flags and report capture.
Locality: new runner and workflow plus `fuzz/README.md` instructions.

Set RUSTUP_TOOLCHAIN explicitly when invoking the isolated cargo-fuzz executable from the repository root. Stage writable corpus and artifact directories for both replay and mutation. Pin toolchain and cargo-fuzz identities in the fuzz workspace policy, verify them rather than silently upgrading, and document installation into an isolated tool root. The runner validates numeric durations and target membership. Use process status and complete logs; missing stats remain explicitly unavailable. Run 30 seconds on relevant PRs, 300 seconds on scheduled/manual CI, and 600 seconds per target for local acceptance. Input timeout is 5 seconds, RSS guard 2048 MiB, raw input cap 65536 bytes; sparse and deterministic tests cover larger real boundaries.

### Regression and compatibility verification

Satisfies: FUZZ-3.2, FUZZ-3.3, FUZZ-5.3
Reuse: rung 2 — existing per-module unit tests, workspace gates and package-check scripts.
Respects: ARCH-9, ARCH-16, ARCH-20
Surface: existing CLI/state/receipt/standalone seed and checkout-GC contracts are frozen and checked through their existing tests; no fixture/schema rewrite is planned.
Interface: ordinary cargo test regressions run without fuzz tooling; reports map behavior to requirements without IDs in source or test titles.
Depth: n/a; extends existing tests.
Locality: reader-local regressions and documentation only.

## Seams for testing

| Seam | Kind | Covers |
|---|---|---|
| Core shared decoders/read_capped/target identification and name predicates | unit, bounded streams and fake read-only Fs | FUZZ-1.2, FUZZ-1.3, FUZZ-2.2, FUZZ-2.4, FUZZ-5.2 |
| Seed dep-info decoder/classifier and rustc_info_version | unit, bounded streams | FUZZ-1.1, FUZZ-2.1, FUZZ-2.6 |
| debug_map::read, gc plan classifiers, metadata window and explain aggregation | unit, sparse read-only Fs | FUZZ-1.4, FUZZ-2.3, FUZZ-2.5 |
| Nine actual fuzz executables and synthetic seed replay | process acceptance | FUZZ-3.1, FUZZ-4.1, FUZZ-5.1, FUZZ-5.4, FUZZ-6.2 |
| Runner and separate macOS workflow | runner refusal controls and actual bounded campaigns | FUZZ-3.4, FUZZ-4.2, FUZZ-4.3, FUZZ-4.4, FUZZ-4.5, FUZZ-6.1, FUZZ-6.3 |
| Regression reversal and existing full gates/package checks | unit plus integration | FUZZ-3.2, FUZZ-3.3, FUZZ-5.3 |

## Coverage check

Every requirement appears in one Satisfies line and at least one seam row. No browser UI applies. Optional product metrics, security, ops and codebase-system documents are absent; approved standards and the architecture spine supply constraints. No new domain term or irreversible architecture choice needs an ADR.
