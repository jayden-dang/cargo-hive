# Requirements: Cargo and rustc reader fuzzing

Feature code: FUZZ
Status: Implemented
Date: 2026-11-23

Authorization: Jayden selected the proposed cargo-fuzz plus semantic-regression approach and instructed “Plan and implement in this chat” on 2026-11-23. The implementation includes the format-specific truncation interpretation proposed in the investigation. This is not a claim that finite fuzzing proves all possible inputs correct.

## 1. Exercise every interpreted reader

**Story:** As a contributor, I want to run a target for each interpreted Cargo or rustc surface, so that malformed inputs reach the actual production readers.

- **FUZZ-1.1** WHEN the target registry is inspected THE SYSTEM SHALL map each interpreted surface to its production entry points, corpus, oracle and regression location: fingerprints/hash/features/dependency names (including seed staging hash revalidation), diagnostics, dep-info, rustc-info, Mach-O debug maps, target tags, incremental names, artifact/layout names (including explain mode classification) and current/not-current timestamp metadata.
- **FUZZ-1.2** WHEN a target runs THE SYSTEM SHALL execute the same decoding and validation used by its production reader, without copied parsers or fuzz-only validation bypasses.
- **FUZZ-1.3** WHEN a fingerprint case is exercised THE SYSTEM SHALL cover flat and unit-dir identity, including a hash-file value different from its directory hash.
- **FUZZ-1.4** WHEN timestamp cases run THE SYSTEM SHALL vary metadata and I/O errors rather than inventing a parser for timestamp file contents.

## 2. Detect wrong answers and bound reads

**Story:** As a contributor, I want malformed inputs checked against explicit expected results, so that a successful but incorrect parse is detected.

- **FUZZ-2.1** WHEN generated valid cases run THE SYSTEM SHALL compare decoded values or classifications against independently constructed expectations.
- **FUZZ-2.2** WHEN a small valid fixture is truncated at each byte THE SYSTEM SHALL check a format-specific expected outcome, rejecting missing required structure while preserving complete records and documented in-progress states.
- **FUZZ-2.3** WHEN reader limits are exercised THE SYSTEM SHALL test the limit and one byte or entry beyond it without allocating inputs in proportion to untrusted declared lengths or offsets.
- **FUZZ-2.4** IF fingerprint hash or target-tag input exceeds its explicit read limit THEN THE SYSTEM SHALL stop the bounded read and report a named bound refusal without a supported unit or target classification.
- **FUZZ-2.5** WHEN a fat Mach-O slice is interpreted THE SYSTEM SHALL refuse any header or table range outside that declared slice, using checked offset arithmetic.
- **FUZZ-2.6** WHEN a reader receives a short read or I/O error THE SYSTEM SHALL preserve that reader's named refusal, uncertainty or unrecognized-input outcome rather than treating incomplete evidence as a successful parse.

## 3. Replay and retain failures

**Story:** As a contributor, I want a failing input replayed and retained, so that a discovered defect cannot silently return.

- **FUZZ-3.1** IF a target panics, exceeds a configured resource guard or violates its semantic oracle THEN THE SYSTEM SHALL fail the run and retain the input and command needed to replay it.
- **FUZZ-3.2** WHEN a discovered defect is corrected THE SYSTEM SHALL first contain an ordinary regression that fails for that defect before the correction.
- **FUZZ-3.3** WHEN a fixed defect is verified THE SYSTEM SHALL demonstrate that removing the correction makes its regression fail again.
- **FUZZ-3.4** WHEN a campaign report is written THE SYSTEM SHALL record source commit/tree state, tool versions, target, corpus, seed, command, duration, execution count, coverage output and exit status, reporting missing measurements explicitly.

## 4. Run bounded campaigns in isolation

**Story:** As a contributor, I want one documented command for bounded fuzz runs, so that local and CI runs use the same targets and limits.

- **FUZZ-4.1** WHEN fuzz tooling is built THE SYSTEM SHALL keep its dependencies and target artifacts in an independent unpublished workspace with a committed lockfile and pinned tool versions.
- **FUZZ-4.2** WHEN the runner executes THE SYSTEM SHALL replay checked-in seeds before mutation and apply explicit per-input timeout, RSS, maximum-input-length and total-time limits.
- **FUZZ-4.3** WHEN a pull request changes a reader or fuzz surface THE SYSTEM SHALL run corpus replay and a 30-second campaign per registered target on macOS.
- **FUZZ-4.4** WHEN scheduled or manual CI runs execute THE SYSTEM SHALL support a 300-second campaign per target and retain logs, corpus discoveries and failure artifacts even on failure.
- **FUZZ-4.5** WHEN initial acceptance is recorded THE SYSTEM SHALL complete 600 seconds per registered target after successful replay, or report the exact incomplete target and reason without marking acceptance complete.

## 5. Preserve shipped behavior

**Story:** As a hive user, I want reader testing isolated from normal use, so that installing or running hive requires no fuzz tooling.

- **FUZZ-5.1** WHEN normal workspace builds or package checks run THE SYSTEM SHALL CONTINUE TO require neither libFuzzer nor a nightly compiler.
- **FUZZ-5.2** WHEN supported fingerprints are read THE SYSTEM SHALL CONTINUE TO enforce the exact known signature and preserve empty-hash NotCurrent behavior and current hash-file identity rules.
- **FUZZ-5.3** WHEN existing public CLI, receipts, state, seed or checkout-GC operations run THE SYSTEM SHALL CONTINUE TO use their existing schemas and interfaces.
- **FUZZ-5.4** WHEN fuzz fixtures or metadata models run THE SYSTEM SHALL confine file effects to disposable scratch paths, with no host builds, deletion operations, owner-state writes or private workspace corpus.

## 6. Quality attributes

**Section-kind:** nfr

**Story:** As a contributor, I want bounded and reproducible testing, so that campaign results remain interpretable.

- **Performance:** **FUZZ-6.1** WHEN a target is executed THE SYSTEM SHALL use a 5-second input timeout, 2048 MiB RSS guard and 64 KiB raw-input maximum, with larger production boundaries exercised through deterministic or sparse test inputs.
- **Security:** **FUZZ-6.2** WHEN corpus and reports are committed THE SYSTEM SHALL contain only synthetic or redistributable fixture inputs without owner paths, credentials or private source.
- **Reliability:** **FUZZ-6.3** WHEN no failure is found THE SYSTEM SHALL report only the bounded run performed, without claiming universal absence of crashes or parsing errors.
- **Accessibility:** None; this change adds no interactive user interface.

## Guarded surfaces

Core fingerprint/target/name readers: FUZZ-5.2. Seed source/fill readers and GC debug-map/name/metadata readers: FUZZ-5.3. Root lint configuration and separate fuzz workspace/workflow: FUZZ-5.1 and FUZZ-5.4. New corpus, registry and guide: no existing behavior to guard. Existing CI gate workflow is left unchanged.

## Out of Scope

- Hive state files, Git/config readers, subprocess metadata output, arbitrary binary bodies copied without interpretation, and universal corruption detection.
- Changes to the public Fs trait, CLI/state/receipt schemas, slot orchestration or standalone deletion policy.
- Replacing the existing merge gate, mutation testing of deletion code (#12), formal verification or exhaustive model checking.
- Concurrent local campaigns during ROAD-17 timing measurements.

## Open Questions

None. Small reader-local malformed-input corrections are included with regressions. Any correction requiring a changed shared GC/seed/slot contract requires a revised plan. Tool pin feasibility is verified before campaign acceptance; no unverified tool setup counts as a successful run.
