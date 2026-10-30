# Requirements: Explain (why a crate has N copies, feature divergence, failed units)

Feature code: EXPLAIN
Status: Implemented
Date: 2026-10-29

<!--
Rules:
- Feature code: 2-12 chars, A-Z0-9, starts with a letter, unique repo-wide.
  Register it in docs/specs/INDEX.md before use.
- Every acceptance criterion gets a hierarchical ID: <CODE>-<story>.<criterion>.
- Criteria use EARS phrasing:
    WHEN <event/condition> THE SYSTEM SHALL <behavior>          (event-driven)
    WHILE <state> THE SYSTEM SHALL <behavior>                   (state-driven)
    IF <unwanted condition> THEN THE SYSTEM SHALL <behavior>    (unwanted behavior)
    WHERE <feature is included> THE SYSTEM SHALL <behavior>     (optional feature)
    THE SYSTEM SHALL <behavior>                                 (ubiquitous)
- Guard requirements protect existing behavior this feature touches:
    WHEN <condition> THE SYSTEM SHALL CONTINUE TO <existing behavior>
- IDs are immutable once Status is Approved. Retire a requirement by striking it
  through (~~**CODE-N.M**~~ reason) — never renumber.
-->

Roadmap item: ROAD-3 (MILE-1, `docs/roadmap/INDEX.md`).

Depends on: the CORE/AUTOGC receipt amendment (`Finding.measured`, receipt schema 2, per-entity
numbers moved into `measured`, `not_current` in status), which merges before this feature's
build starts. Every "finding with measured keys" below uses that shape.

Sources:
- `.skills/EXPLAIN/close-package.md` (confirmed 2026-10-29) and `.skills/EXPLAIN/knowns.md`.
- `.skills/EXPLAIN/req-review.md` (code-claim review, 2026-10-29). On klynt, the mode rule splits
  `persistence`'s `lib` units into 7 build and 6 check with features `["testkit"]`, and 3 build and
  2 check with `[]`, with no `unknown`.
- `.skills/research/2026-10-29-cargo-failed-unit-fingerprint-shape.md` and
  `.skills/research/2026-10-29-cargo-output-message-cache.md`.
- Cargo 1.97.1 sources at `c980f4866` (`src/cargo/core/compiler/fingerprint/mod.rs`,
  `src/cargo/core/compiler/mod.rs`), as cited in the foundation design.

Terms used as `CONTEXT.md` defines them: Cargo target dir, Profile dir, Unit, Fingerprint format
signature, Receipt. Further terms used below:
- **Current unit:** a unit counted by CORE-4.7 (one fingerprint JSON beside a non-empty hash file).
- **Not-current unit:** a unit directory counted by CORE-4.16.
- **Copy group:** the current units of one profile dir that share the crate name (the unit
  directory name without its final `-<hash>`), the hash-file name (for example `lib-persistence`,
  `test-integration-test-it`), and the fingerprint's `target` and `path` values. Its **copies**
  are the number of units in it. A copy group with 2 or more copies is a **multi-copy group**.
- **Target kind:** the hash-file name's kind as CORE reads it, by prefix: `run-build-script`,
  `build-script`, `doc-lib`, `doc-bin`, `test`, `lib`, `bin`, `example`, `bench`, or `other`. The
  first prefix that matches wins, so a bench compiled for tests (`test-bench-password_hasher`)
  reads as `test`.
- **Mode:** a unit's mode, read from the files in the profile dir's `deps/` whose name carries the
  unit's directory hash: `build` when one is an `.rlib`, a `.dylib`, or has no extension;
  `check` when one is an `.rmeta` and none is any of those; `unknown` otherwise. Build-script and
  run-build-script units never have files in `deps/`, so their mode is always `unknown`.
- **Summary line:** a line of a unit's `output-*` file that parses as JSON with `level` equal to
  `"error"`, `code` equal to null, and a `message` that starts with `aborting due to`.

## 1. See why a crate has N copies

**Story:** As a developer or an agent, I want `cargo hive explain` to list the crates with the most copies in my targets and what tells those copies apart, so that I know which differences are costing me disk.

- **EXPLAIN-1.1** WHEN the user runs `cargo hive explain` THE SYSTEM SHALL form the copy groups of every supported profile dir of every target in scope (EXPLAIN-5.1).
- **EXPLAIN-1.2** WHEN explain reports a multi-copy group THE SYSTEM SHALL report it as one finding whose path is the profile dir and whose measured keys are exactly: `crate`, `target_kind`, `copies`, `feature_sets` (distinct `features` strings), `profile_hashes` (distinct `profile` values), `mode_build`, `mode_check`, `mode_unknown` (copies in each mode), and `copies_sharing_features_and_profile` (copies minus the number of distinct (`features`, `profile`) pairs).
- ~~**EXPLAIN-1.3**~~ superseded by EXPLAIN-1.9 (design measurement 2026-10-29: an empty `features` string, which Cargo writes for run-build-script units, needs its own coverage value) — was: ~~WHEN explain reports a multi-copy group THE SYSTEM SHALL also report one feature-set finding per distinct pair of mode and `features` string among its units, with measured keys exactly `crate`, `target_kind`, `mode`, `features` (Cargo's string, verbatim), `coverage`, and `units` (the group's units with that mode and string). `coverage` is `none` when the string is `[]`, `all_declared` when it names every feature in the unit's `declared_features`, and `partial` otherwise.~~
- **EXPLAIN-1.4** THE SYSTEM SHALL report, across the whole run, the multi-copy groups with the most copies, at most the default count (Open Questions), ordered by copies descending, then by profile dir path, then by crate name, then by hash-file name.
- **EXPLAIN-1.5** WHERE the user passes the show-all flag THE SYSTEM SHALL report every multi-copy group, in the order of EXPLAIN-1.4.
- ~~**EXPLAIN-1.6**~~ superseded by EXPLAIN-1.10 (design measurement 2026-10-29: the totals finding also counts feature-divergence findings) — was: ~~WHEN explain runs THE SYSTEM SHALL report, for every target dir in scope whether or not any of its groups was listed, one totals finding with measured keys exactly `groups` (multi-copy groups), `extra_units` (the sum of copies minus 1 over those groups), `groups_shown`, `failed_units` and `compiling_or_unknown_units` (EXPLAIN-3.4).~~
- ~~**EXPLAIN-1.7**~~ superseded by EXPLAIN-1.11 (design measurement 2026-10-29: the run-wide counts also count feature-divergence findings) — was: ~~WHEN explain runs THE SYSTEM SHALL put the run-wide totals of `groups`, `extra_units` and `groups_shown` in the receipt's counts.~~
- **EXPLAIN-1.8** THE SYSTEM SHALL carry crate names, target kinds, modes and feature strings only as measured values, never inside a measured key or a count name.

- **EXPLAIN-1.9** WHEN explain reports a multi-copy group THE SYSTEM SHALL also report one feature-set finding per distinct pair of mode and `features` string among its units, with measured keys exactly `crate`, `target_kind`, `mode`, `features` (Cargo's string, verbatim), `coverage`, and `units` (the group's units with that mode and string). `coverage` is `not_recorded` when the string is empty (Cargo records no features for a run-build-script unit), `none` when it is `[]`, `all_declared` when it names every feature in the unit's `declared_features`, and `partial` otherwise.
- **EXPLAIN-1.10** WHEN explain runs THE SYSTEM SHALL report, for every target dir in scope whether or not any of its groups was listed, one totals finding with measured keys exactly `groups` (multi-copy groups), `extra_units` (the sum of copies minus 1 over those groups), `groups_shown`, `divergences` (feature-divergence findings, EXPLAIN-2.5), `divergences_shown`, `failed_units` and `compiling_or_unknown_units` (EXPLAIN-3.4).
- **EXPLAIN-1.11** WHEN explain runs THE SYSTEM SHALL put the run-wide totals of `groups`, `extra_units`, `groups_shown`, `divergences` and `divergences_shown` in the receipt's counts.
## 2. See where one crate builds with different feature sets

**Story:** As a developer, I want explain to show me each crate that is built with different feature sets in the same mode and target kind, so that I can find the feature split that multiplies its copies.

- ~~**EXPLAIN-2.1**~~ superseded by EXPLAIN-2.5 (design measurement 2026-10-29: it cited the struck EXPLAIN-1.3) — was: ~~WHEN a copy group holds, among its units of one mode, two or more distinct `features` strings THE SYSTEM SHALL report a feature-divergence finding whose path is the profile dir, with measured keys exactly `crate`, `target_kind`, `mode` and `feature_sets`, together with the EXPLAIN-1.3 feature-set findings of that group and mode.~~
- ~~**EXPLAIN-2.2**~~ superseded by EXPLAIN-2.6 (design measurement 2026-10-29: 139 uncut feature-divergence findings made the default klynt receipt about 142 KB) — was: ~~THE SYSTEM SHALL report every feature-divergence finding, and the feature-set findings that go with it, whatever the default count and whether or not its copy group is listed by EXPLAIN-1.4, and SHALL report each feature-set finding at most once.~~
- **EXPLAIN-2.3** THE SYSTEM SHALL give every feature-divergence finding the same detail text, which names both possible sources (different feature flags between invocations, and dev-dependency feature unification between build and test) and names no tool.
- **EXPLAIN-2.4** THE SYSTEM SHALL decide feature divergence only from the fingerprints' `features` and `declared_features` values, and read no editor or tool configuration file to do so.

- **EXPLAIN-2.5** WHEN a copy group holds, among its units of one mode, two or more distinct `features` strings THE SYSTEM SHALL count one feature divergence for that group and mode, reported as a finding whose path is the profile dir, with measured keys exactly `crate`, `target_kind`, `mode` and `feature_sets`, together with the EXPLAIN-1.9 feature-set findings of that group and mode.
- **EXPLAIN-2.6** THE SYSTEM SHALL report, across the whole run, the feature divergences whose copy groups have the most copies, at most the divergence default count (Open Questions), ordered by copies descending, then by profile dir path, crate name, hash-file name and mode. WHERE the user passes the show-all flag it SHALL report every one. It SHALL report each feature-set finding at most once.
## 3. See which crates failed their last compile

**Story:** As a developer, I want explain to tell me which units failed their last compile and with which rustc error codes or lint names, so that I can fix the crate whose last compile failed.

- **EXPLAIN-3.1** WHEN a not-current unit's `output-*` file holds at least one summary line, at any position, THE SYSTEM SHALL report it as one failed-unit finding whose path is the unit directory, with measured keys exactly `crate`, `target_kind`, `features`, `attempted_at` (the modification time of its `invoked.timestamp`, in UTC) and `error_codes`. Where the unit directory holds a fingerprint JSON beside an emptied hash file, `features` comes from that JSON, which the unit's last successful build wrote.
- **EXPLAIN-3.2** WHEN explain reports a failed unit THE SYSTEM SHALL set `error_codes` to the distinct non-null `code.code` values (rustc error codes such as `E0308`, or lint names such as `dead_code`) of the file's other `"error"` lines, comma-joined in order of first appearance, and SHALL set it to the empty string when there are none.
- **EXPLAIN-3.3** WHEN explain reports a failed unit whose directory holds no fingerprint JSON THE SYSTEM SHALL take `crate` from the directory name and `target_kind` from the `output-*` file name, and SHALL omit `features` and record in `unmeasured` that it has no fingerprint.
- **EXPLAIN-3.4** WHEN a not-current unit's `output-*` file holds no summary line, or it has no `output-*` file (which happens only to a unit with an emptied hash file, for example while it compiles), THE SYSTEM SHALL count it in its target's `compiling_or_unknown_units` and report no finding for it. A failed unit is counted in `failed_units`.
- **EXPLAIN-3.5** IF a line of an `output-*` file does not parse as JSON THEN THE SYSTEM SHALL skip that line, use the remaining lines for EXPLAIN-3.1 and EXPLAIN-3.2, and record in `unmeasured` the unit directory and that a line did not parse.
- **EXPLAIN-3.6** THE SYSTEM SHALL describe a failed unit only as having failed its last compile, and never state that it always fails or why it was built.

## 4. Check that failed compiles can be recognized on this machine

**Story:** As a developer on a new Mac, I want `cargo hive doctor` to prove that a failed compile leaves the summary line hive looks for, so that explain only names failed units on a toolchain where that works.

- **EXPLAIN-4.1** WHEN the user runs `cargo hive doctor` THE SYSTEM SHALL run a check (g) in its temporary directory: build, with the active toolchain, a crate that has one compile error, and measure whether that unit's `output-*` file holds a summary line.
- **EXPLAIN-4.2** WHEN check (g) finds no summary line THE SYSTEM SHALL record failed-unit reporting as disabled for the active toolchain, through the same capability record CORE-1.3 uses.
- **EXPLAIN-4.3** WHILE failed-unit reporting is disabled, or the recorded doctor results for the active toolchain hold no result for check (g), THE SYSTEM SHALL report no failed-unit finding, count every not-current unit in `compiling_or_unknown_units`, and record in `unmeasured` that `failed_units` is not measured and why: check (g) did not pass, or has not run. The rest of the command runs; this alone never refuses it.

## 5. Explain the targets of one repository, or of all of them

**Story:** As an agent, I want to run `cargo hive explain <repo>` and get the copies of exactly that repository's targets, so that I explain only what I was asked about.

- **EXPLAIN-5.1** WHEN the user runs `cargo hive explain` with or without repository arguments THE SYSTEM SHALL take as its targets in scope exactly the targets `cargo hive gc` would process for the same arguments (AUTOGC-4.1 and AUTOGC-4.2), whatever their owner state. This borrows gc's scope only: explain does not require gc's Deletion capability.
- **EXPLAIN-5.2** IF a repository argument does not resolve to the main checkout of a registered repository THEN THE SYSTEM SHALL refuse that argument with a finding, still process the other arguments, and exit with status 1, as `status` does.
- **EXPLAIN-5.3** IF a target in scope is reported unreadable, or one of its profile dirs is reported unsupported, THEN THE SYSTEM SHALL form no copy group from it and add a finding naming it and the reason.
- **EXPLAIN-5.4** IF the fingerprint reading capability is disabled, or the recorded doctor results are missing or for a different toolchain, THEN THE SYSTEM SHALL refuse the command as CORE-1.4 and CORE-1.10 require.
- **EXPLAIN-5.5** THE SYSTEM SHALL let `cargo hive explain` run with `--json` and print the same receipt the receipts log records (CORE-5.1, CORE-5.2), including under the show-all flag.

## 6. Quality attributes

**Section-kind:** nfr

**Story:** As a stakeholder, I want measurable quality targets for this feature, so that how-well is not left implicit.

Consult outcome: no `docs/product/metrics.md`, `docs/ops/reliability.md`, `docs/security/threat-model.md` or `docs/standards/accessibility.md` exists, so the consult is a no-op. The targets below come from the close package's locks and done signals. No system IDs were invented.

- **Performance:** **EXPLAIN-6.1** WHEN `cargo hive explain <klynt> --json` runs on the proof machine against klynt (`~/Developer/klynt/klynt`) THE SYSTEM SHALL finish within 2 s more than `cargo hive status <klynt> --no-sizes --json` does, both for the median of 5 runs with a warm cache and for one run of each starting from an empty `snapshot.json`, verified by an `#[ignore]` timing test in `tests/guards.rs` in the style of CORE-6.1's.
- **Security:** **EXPLAIN-6.2** WHEN `cargo hive explain` runs THE SYSTEM SHALL create, modify or delete files only under `~/.hive/`, verified by extending CORE-6.3's tests in `tests/guards.rs` to run `explain` on a fixture checkout whose target holds a failed unit: the checkout and its target dir are byte-for-byte unchanged, `~/.cargo` shows the same marks that test already checks (the mtimes of its config and env files and the listing of `bin/`), and a scratch `CARGO_HOME` is byte-for-byte unchanged.
- **Reliability:** None, because explain deletes and rewrites nothing outside `~/.hive/` (EXPLAIN-6.2), and its writes there go through CORE's state module (ARCH-16), which CORE-6.5 already covers.
- **Accessibility:** None, because this is a headless CLI with JSON output.

## 7. Existing behavior this feature must not change

Files and surfaces this change touches:
- `src/fingerprint.rs`: the reader may keep more of each fingerprint's fields. `status`'s units, edges and support verdicts are at risk. Guarded by EXPLAIN-7.1.
- `src/doctor.rs`: a new check (g); checks (a) to (f) must not change. Guarded by EXPLAIN-7.2.
- `src/cli.rs`: a new `explain` command; `status`, `gc`, `repo add`, `repo list`, `doctor` and the hidden commands exist. Guarded by EXPLAIN-7.1, EXPLAIN-7.3 and EXPLAIN-7.4.
- `src/discover.rs`, `src/cargo_target.rs`: target discovery is reused unchanged. `status` and `gc` scope are at risk. Guarded by EXPLAIN-7.1 and EXPLAIN-7.3.
- `src/receipt.rs`: no behavior to guard; explain only adds findings and counts in the shape the receipt amendment defines.
- `README.md`, `docs/adr/0001-read-cargo-internal-fingerprints.md`: no behavior to guard; documentation only.
- Registered repositories' checkouts, their targets, and `~/.cargo`: behavior at risk. Guarded by EXPLAIN-6.2 and EXPLAIN-7.5.

- **EXPLAIN-7.1** (guard) WHEN `cargo hive status` runs THE SYSTEM SHALL CONTINUE TO report the same checkouts, targets, owners, units, not-current units, edges, unsupported profile dirs and findings as before this feature, and never read an `output-*` file.
- **EXPLAIN-7.2** (guard) WHEN `cargo hive doctor` runs THE SYSTEM SHALL CONTINUE TO run checks (a) to (f) with the same pass rules and measured values, and to disable capabilities by the CORE-1.3 map.
- **EXPLAIN-7.3** (guard) WHEN `cargo hive gc` runs THE SYSTEM SHALL CONTINUE TO process the same targets and delete the same entries as before this feature.
- **EXPLAIN-7.4** (guard) WHEN any hive command runs once hive's state is open, `explain` included, THE SYSTEM SHALL CONTINUE TO satisfy CORE-5.1 through CORE-5.7.
- **EXPLAIN-7.5** (guard) WHEN hive is installed and repositories are registered THE SYSTEM SHALL CONTINUE TO let `cargo` and rust-analyzer build every checkout exactly as without hive, with no Cargo configuration, environment variable, wrapper or hook added (CORE-7.2).

## Out of Scope

- Editing any repository, editor or tool configuration file.
- Any byte, GB or seconds figure in explain's findings, including bytes per unit (deferred until after ROAD-5) and artifact dir bytes under `build.build-dir` (status, through a separate CORE amendment).
- Re-reporting orphan candidates; status already reports them.
- "Emptied protected targets"; dropped from ROAD-3, never defined.
- Naming a tool (rust-analyzer, moon, nextest, a terminal) or a single cause for a feature split.
- Calling copies that share features and profile "older builds"; the cause is unproven.
- Stating that a unit "always fails", or replaying a build to find out.
- Telling whether a build is running through Cargo's build-dir lock.
- Deleting anything (ROAD-5, ROAD-6).
- The MCP tool for explain (ROAD-4).
- Moving status's and gc's per-entity numbers into `measured`, and status's `not_current` key: the receipt amendment.

## Open Questions

None. The owned unknowns from `.skills/EXPLAIN/close-package.md` §3 were measured in design and
decided by the owner on 2026-10-29 (`design.md`, Decisions):

- The default count of EXPLAIN-1.4 and the divergence default count of EXPLAIN-2.6 are one shared
  default of 10 (about 16 KB on klynt).
- The show-all flag of EXPLAIN-1.5, 2.6 and 5.5 is `-n, --top <N>` with `N = 0`.
- Finding kind names: `copies.group`, `copies.features`, `copies.divergence`, `unit.failed`,
  `copies.totals`.
- The coverage value for an empty `features` string is `not_recorded`.
- "Measured keys exactly" names the full key set; a value explain cannot measure is omitted and
  named in `unmeasured` (CORE-5.5, kept by EXPLAIN-7.4).
