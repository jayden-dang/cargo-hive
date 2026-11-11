# Requirements: Store and seed (a compressed store of registry units, cloned into a new secondary checkout)

Feature code: SEED
Status: Implemented
Date: 2026-11-02

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

Roadmap item: ROAD-8 (MILE-3, `docs/roadmap/INDEX.md`), tracker issue #4. ROAD-9 is merged into ROAD-8.

Sources:
- `.skills/SEED/close-package.md` and the Locked section of `.skills/SEED/decisions.md` (confirmed by the owner 2026-11-02). The Locked section is the full version.
- Measurements: `.skills/SEED/spike-compress.md`, `.skills/SEED/spike-seed.md`, the measurement sections of `.skills/SEED/decisions.md`. All of them are small samples; none ran the feature on klynt.
- Locks this feature builds on: GOAL-2, GOAL-3, GOAL-4, GOAL-7; ARCH-1 to 11, 13 to 17, 19; D1, D2, D7, D8 of `.skills/CORE/close-package.md`; the liveness verdicts of LIVE (`docs/specs/2026-10-30-closure-liveness/`).

Terms (defined in `CONTEXT.md`, whose Store and Fill entries this feature corrects): store, seed, fill, unit, unit hash, secondary checkout, protected checkout, Cargo target dir, profile dir, roots, closure liveness, receipt. This file adds seven:
- **Registry unit:** a unit whose dep-info file names, as its first input, a path under a Cargo registry source directory, or a unit with no dep-info file that has at least one dependency edge, every one of them resolved to a registry unit.
- **Name-meta:** a unit's directory name under `.fingerprint/`, `<name>-<metadata hash>`.
- **Toolchain value:** the `rustc` number in a unit's fingerprint JSON.
- **Store key:** (toolchain value, name-meta, fingerprint hash), the key of D1.
- **Entry:** the files the store holds for one store key, each with its path relative to the Cargo target dir: the whole `.fingerprint/<dir>/`, the whole `build/<dir>/` if present, and the `deps/` files in whose names `embedded_hashes` finds the unit's hash, except names ending in `.rcgu.o`.
- **Stamp:** a unit's `invoked.timestamp`, at the path `liveness::unit_timestamp` gives for its kind: under `build/<dir>/` for a run-build-script unit, under `.fingerprint/<dir>/` otherwise.
- **Toolchain table:** the rows hive learns at fill, each from the whole `rustc -vV` text to one toolchain value.

## 1. Seed a new secondary checkout

**Story:** As a developer or agent, I want to run `cargo hive seed <checkout>` on a new secondary checkout, so that its first build recompiles none of the units hive cloned.

- **SEED-1.1** THE SYSTEM SHALL offer `cargo hive seed <checkout> [--json]`, taking exactly one checkout path and reading no scope from the current directory.
- **SEED-1.2** WHEN seed runs THE SYSTEM SHALL first fill the store as story 2 requires, then clone as this story and story 3 require, in one command.
- **SEED-1.3** WHEN the checkout's `target_directory`, resolved by `cargo metadata --no-deps --format-version 1 --locked --offline` at the checkout's root, is a path that does not exist THE SYSTEM SHALL create that directory and the directories inside it that the clones need, clone the store's `CACHEDIR.TAG` to `<target_directory>/CACHEDIR.TAG`, and clone each chosen entry's files to the same paths relative to the Cargo target dir.
- **SEED-1.4** WHEN the checkout's `target_directory` path exists, whatever it holds, THE SYSTEM SHALL clone 0 units, create and modify nothing in it, and report one finding naming the path.
- **SEED-1.5** THE SYSTEM SHALL write into the checkout's target only by creating directories inside it and by `clone_file`, which never overwrites a file that exists, and SHALL call no `write_file` on the tag. A placed file that still carries the compressed-file flag is replaced, in that same directory, by a new regular file of the same logical bytes. The store entry stays compressed. The tag is not written with `write_file` except through that same replacement when the tag itself still carries the flag.
- **SEED-1.6** WHEN seed clones a file THE SYSTEM SHALL leave its modification time as the store holds it, and SHALL set no modification time on any stamp. The replacement in SEED-1.5 puts the clone's modification time back on the new file. Seed sets no modification time on a file it did not replace.
- **SEED-1.7** WHEN a clone returns that the destination file exists THE SYSTEM SHALL leave that file as it is, count the unit as not cloned, and continue.
- **SEED-1.8** WHEN seed has cloned into a new target THE SYSTEM SHALL leave a directory that `cargo_target::identify` accepts as a Cargo target dir before any build has run.
- **SEED-1.9** WHEN the first build of a checkout seeded by this command runs and no `apply` has deleted seed files between seed and that build THE SYSTEM SHALL have left every unit it cloned so that Cargo recompiles none of them, for `cargo build`, `cargo check --all-targets`, `cargo clippy --all-targets`, `cargo test --no-run` and `cargo check -p <package> --lib` run at the checkout's root when the source target had built that same unit.
- **SEED-1.10** IF the checkout is in a registered repo's discovered checkouts and is protected, including anything hive cannot classify THEN THE SYSTEM SHALL clone 0 units, modify nothing in it, and report the refusal with exit code 1.
- **SEED-1.11** IF the checkout is in no registered repo's discovered checkouts, whatever class its path would have, including a clone below a secondary root whose root commits match no registered repo and a checkout deeper below a secondary root than discovery looks THEN THE SYSTEM SHALL clone 0 units, modify nothing in it, and report the refusal with exit code 1.
- **SEED-1.12** IF `target_directory` cannot be resolved at the checkout's root, including a root with no `Cargo.toml`, or the resolved build directory differs from it THEN THE SYSTEM SHALL clone 0 units, modify nothing in the checkout, and report one finding with the reason.
- ~~**SEED-1.13**~~ superseded by SEED-1.17 (design review, 2026-11-02: fill relies on the lock proof of doctor's check d, which vouches only for the deletion capability) — was: ~~IF the seeding capability or the store-compression capability has not passed for the release `resolve_toolchain` returns for the checkout's directory THEN THE SYSTEM SHALL fill nothing, clone 0 units, and report the refusal with exit code 1.~~
- **SEED-1.14** WHEN seed clones 0 units for a reason other than a refusal in SEED-1.10, SEED-1.11, SEED-1.15 or SEED-1.17 THE SYSTEM SHALL exit with code 0 and report the reason as a finding, so that an agent can go on to build.
- **SEED-1.15** IF `resolve_toolchain` fails for the checkout's directory for a reason other than a cancel THEN THE SYSTEM SHALL fill nothing, clone 0 units, and report the refusal with exit code 1. A cancel during that lookup is SEED-7.5.
- **SEED-1.16** IF `clone_file` into the target fails for a reason other than the destination existing, or a full disk THEN THE SYSTEM SHALL count that unit as not cloned, report one finding naming the path and the error, and continue with the next unit.
- ~~**SEED-1.17**~~ superseded by SEED-1.20 (root-cause and owner decision 2026-11-11: rustc rejects an rmeta that keeps the compressed-file flag, so every clone from a compressed store was expanded into a private copy, about 4.28 GiB per klynt lane against GOAL-3) — was: ~~IF the seeding, store-compression or deletion capability has not passed for the release `resolve_toolchain` returns for the checkout's directory THEN THE SYSTEM SHALL fill nothing, clone 0 units, and report the refusal with exit code 1.~~
- **SEED-1.18** WHEN the set of units to clone is empty THE SYSTEM SHALL create nothing in the checkout, so that a later seed can still clone into it.
- **SEED-1.19** IF a parent of `target_directory` does not exist THEN THE SYSTEM SHALL create nothing, clone 0 units, and report one finding naming the path.
- **SEED-1.20** (fix, 2026-11-11) IF the seeding or deletion capability has not passed for the release `resolve_toolchain` returns for the checkout's directory THEN THE SYSTEM SHALL fill nothing, clone 0 units, and report the refusal with exit code 1. The store-compression capability is no longer required.

## 2. Fill the store from live targets

**Story:** As a developer or agent, I want seed to bring the live registry units of my repo into the compressed store, so that later seeds have them without my running another command.

- **SEED-2.1** THE SYSTEM SHALL offer no separate fill command.
- **SEED-2.2** WHEN seed fills THE SYSTEM SHALL read as sources the Cargo target dirs that the checkouts of the checkout's registered repo own, with ownership resolved by `cargo metadata` and not read from the snapshot cache.
- **SEED-2.3** WHEN a source target is `Decided` THE SYSTEM SHALL fill only the registry units `liveness()` lists as live and whose fingerprint hash file is not empty.
- **SEED-2.4** IF a source target is `Undecided` or its profile is unsupported, including a toolchain that leaves no `.fingerprint/` directory THEN THE SYSTEM SHALL fill nothing from it and report one finding naming the target and the reason.
- **SEED-2.5** WHEN seed fills from a source target THE SYSTEM SHALL hold the target's lock files, tried without waiting, while it clones that target's new entries into the store, and SHALL release them before it compresses any file.
- **SEED-2.6** IF `locks::acquire` returns any skipped reason or fails for a source target THEN THE SYSTEM SHALL fill nothing from that target, wait for nothing, and count the target as skipped with the reason.
- **SEED-2.7** THE SYSTEM SHALL bring a file into the store only with `clone_file`, and SHALL make no copy of the file's bytes.
- **SEED-2.8** IF `clone_file` fails for a source file, including because the store and the source are on different volumes THEN THE SYSTEM SHALL leave that entry out of the store and report one finding naming the source target and the error.
- ~~**SEED-2.9**~~ superseded by SEED-2.28 (root-cause and owner decision 2026-11-11: rustc rejects an rmeta that keeps the compressed-file flag, so every clone from a compressed store was expanded into a private copy, about 4.28 GiB per klynt lane against GOAL-3) — was: ~~WHEN a file is in the store and no checkout shares it THE SYSTEM SHALL compress it with Applesauce, and SHALL compress no file that a checkout already shares.~~
- ~~**SEED-2.10**~~ superseded by SEED-2.29 (root-cause and owner decision 2026-11-11: rustc rejects an rmeta that keeps the compressed-file flag, so every clone from a compressed store was expanded into a private copy, about 4.28 GiB per klynt lane against GOAL-3) — was: ~~THE SYSTEM SHALL make an entry available to seed only after every file in it finished compressing, and SHALL never clone into a checkout from an entry still being filled.~~
- ~~**SEED-2.11**~~ superseded by SEED-2.22 (design, 2026-11-02: every entry holds an empty stamp and a 16-byte hash file, which Applesauce refuses as too small, so this wording made every entry unavailable) — was: ~~IF compressing a file of an entry fails THEN THE SYSTEM SHALL leave that entry unavailable and report one finding naming the entry and the error.~~
- ~~**SEED-2.12**~~ superseded by SEED-2.30 (root-cause and owner decision 2026-11-11: a crate that `include!`s a file from its `OUT_DIR` embeds that target's path in its metadata, so copies from two targets share a store key but not an SVH, and choosing per name-meta mixed builds; three seeded klynt lanes failed with E0463 and the fixture with E0460) — was: ~~WHEN the store already holds a store key THE SYSTEM SHALL clone and compress no file for it again.~~
- **SEED-2.13** WHEN seed fills a unit THE SYSTEM SHALL record for its store key the size, `filled_at`, `last_live_at`, and the registered repo that used it, adding that repo when the key is already held, and setting `last_live_at` to the time of this scan.
- **SEED-2.14** WHEN the store has no `CACHEDIR.TAG` and a source target is held under its locks THE SYSTEM SHALL clone that target's `CACHEDIR.TAG` into the store, once.
- **SEED-2.15** WHEN seed fills from a source target THE SYSTEM SHALL write a toolchain-table row only when the `-vV` text in that target's `.rustc_info.json` equals the text `resolve_toolchain` runs for the source checkout's directory and every unit in the target holds one toolchain value.
- **SEED-2.16** IF a source target holds more than one toolchain value THEN THE SYSTEM SHALL write no toolchain-table row from it, and report one finding naming the target.
- **SEED-2.17** IF a source target's `.rustc_info.json` is missing or malformed THEN THE SYSTEM SHALL write no toolchain-table row from it and report one finding naming the target.
- **SEED-2.18** WHEN two toolchains print the same `-vV` text and carry the same toolchain value THE SYSTEM SHALL use one toolchain-table row for both.
- **SEED-2.19** THE SYSTEM SHALL modify no file in a source target while filling, other than the lock taken on its existing lock files, and SHALL leave each source file's bytes and modification time as they were.
- **SEED-2.20** IF a write fails because the disk is full THEN THE SYSTEM SHALL stop, report one finding with the error, and exit with code 1.
- **SEED-2.21** THE SYSTEM SHALL stop filling at no byte level, and SHALL evict nothing from the store.
- ~~**SEED-2.22**~~ superseded by SEED-2.28 (root-cause and owner decision 2026-11-11: rustc rejects an rmeta that keeps the compressed-file flag, so every clone from a compressed store was expanded into a private copy, about 4.28 GiB per klynt lane against GOAL-3) — was: ~~IF compressing a file of an entry fails for a reason other than the file being empty or not compressible enough THEN THE SYSTEM SHALL leave that entry unavailable and report one finding naming the entry and the error.~~
- ~~**SEED-2.23**~~ superseded by SEED-2.28 (root-cause and owner decision 2026-11-11: rustc rejects an rmeta that keeps the compressed-file flag, so every clone from a compressed store was expanded into a private copy, about 4.28 GiB per klynt lane against GOAL-3) — was: ~~WHEN Applesauce reports a file of an entry as empty or not compressible enough THE SYSTEM SHALL leave the file uncompressed and count its compression as finished.~~
- **SEED-2.24** IF a live registry unit has a dependency edge that resolves to no unit in the target THEN THE SYSTEM SHALL fill nothing for that unit and report it in one finding per target.
- **SEED-2.25** (fix, 2026-11-10) WHEN a source target's `.rustc_info.json` is missing THE SYSTEM SHALL name in the SEED-2.17 finding the two causes Cargo has for leaving it out: a rustc wrapper set to the empty string (`RUSTC_WRAPPER=` or `RUSTC_WORKSPACE_WRAPPER=`) and `CARGO_CACHE_RUSTC_INFO=0`.
- **SEED-2.26** (fix, 2026-11-10) WHEN seed's fill finds a target it does not read because its owner is unresolved THE SYSTEM SHALL report one finding of kind `source.unresolved` whose path is that target, and, for a cache-root target no checkout of the repository owns, SHALL say so instead of advising `cargo hive status`.
- **SEED-2.27** (guard, 2026-11-10) WHEN seed's checkout or target walk hits a directory it cannot read THE SYSTEM SHALL CONTINUE TO report it as a `walk.unreadable` finding.
- **SEED-2.28** (fix, 2026-11-11) THE SYSTEM SHALL compress no file in the store, so a file cloned into a checkout shares its blocks with the store file (GOAL-3). An entry an older hive left compressed is still placed as SEED-1.5 says.
- **SEED-2.29** (fix, 2026-11-11) THE SYSTEM SHALL make an entry available to seed only after every file in it has been cloned into the entry's temporary directory and that directory has been renamed into place, and SHALL never clone into a checkout from an entry still being filled.
- **SEED-2.30** (fix, 2026-11-11) WHEN the store already holds a store key THE SYSTEM SHALL hold the unit under an existing variant of that key only when every file of the unit other than its `.d` dep-info and its `root-output` file has the same bytes as the variant's file, and SHALL otherwise store the unit as a new variant of the key. Same bytes are known without reading when both files report `SHARES_ALL` with the same clone id.
- **SEED-2.31** (fix, 2026-11-11) WHEN seed fills from a source target THE SYSTEM SHALL record, for that target, its checkout, its toolchain value, the newest modification time of its units' fingerprint hash files, and the store key and variant of each live registry unit it holds, replacing what was recorded for that target before.
- **SEED-2.32** (guard, 2026-11-11) WHEN two source targets hold a unit whose files have the same bytes THE SYSTEM SHALL CONTINUE TO keep one store entry for it.

## 3. Clone only the units this repo used, on this toolchain

**Story:** As a developer or agent, I want seed to choose units from the store for my checkout's repo and toolchain, so that my checkout gets units Cargo can use and no other repo's units.

- **SEED-3.1** WHEN seed chooses units THE SYSTEM SHALL call `resolve_toolchain` on the checkout's directory and look up the whole `-vV` text in the toolchain table.
- **SEED-3.2** WHEN the toolchain table has a row for that text THE SYSTEM SHALL consider only entries with that row's toolchain value.
- **SEED-3.3** IF the toolchain table has no row for that text THEN THE SYSTEM SHALL clone 0 units, report one finding naming the release, and use no other toolchain value in its place.
- **SEED-3.4** THE SYSTEM SHALL consider only entries that the checkout's registered repo has used, and SHALL clone no entry that only another repo has used.
- ~~**SEED-3.5**~~ superseded by SEED-3.9 and SEED-3.10 (root-cause and owner decision 2026-11-11: a crate that `include!`s a file from its `OUT_DIR` embeds that target's path in its metadata, so copies from two targets share a store key but not an SVH, and choosing per name-meta mixed builds; three seeded klynt lanes failed with E0463 and the fixture with E0460) — was: ~~WHEN several entries share a name-meta THE SYSTEM SHALL clone only the one with the newest `last_live_at`.~~
- **SEED-3.6** WHEN a chosen entry has a dependency edge to a registry unit outside the chosen set THE SYSTEM SHALL drop that entry from the set, and repeat until no chosen entry has such an edge.
- **SEED-3.7** THE SYSTEM SHALL keep out of the drop rule of SEED-3.6 every dependency that is not a registry unit, including workspace crates.
- **SEED-3.8** IF the store has no `CACHEDIR.TAG` THEN THE SYSTEM SHALL clone 0 units and report one finding.
- **SEED-3.9** (fix, 2026-11-11) WHEN seed chooses units THE SYSTEM SHALL take them from one source target only: among the targets recorded by SEED-2.31 for the checkout's repo and toolchain value, the first whose checkout's `Cargo.lock` has the same bytes as the seeded checkout's, and otherwise the one whose fingerprint files are newest; and SHALL clone only that target's recorded units, each at its recorded variant.
- **SEED-3.10** (fix, 2026-11-11) WHEN no target is recorded for the checkout's repo and toolchain value THE SYSTEM SHALL clone 0 units and report one finding saying so.
- **SEED-3.11** (guard, 2026-11-11) WHEN units are taken from one source target THE SYSTEM SHALL CONTINUE TO apply the drop rule of SEED-3.6 and SEED-3.7 to them.

## 4. See that the seeded checkouts add about zero bytes

**Story:** As a developer or agent, I want seed's receipt to count the bytes it added by what is really shared, so that I can trust that seeding k checkouts costs about nothing.

- **SEED-4.1** WHEN seed has cloned a file THE SYSTEM SHALL read its sharing attributes, and count the file's bytes as shared only when it reads `SHARES_ALL` and the same clone id as its store file.
- **SEED-4.2** WHEN seed finishes THE SYSTEM SHALL report the bytes of the cloned files that SEED-4.1 did not count as shared, and SHALL take that number from no `private_bytes` value.
- **SEED-4.3** IF the eviction-accounting capability has not passed for the checkout's release THEN THE SYSTEM SHALL omit the number of SEED-4.2 and record the omission in `unmeasured` with the reason.
- **SEED-4.4** WHEN seed finishes THE SYSTEM SHALL report the bytes the store occupies on disk, counted from allocated blocks and taken from no `private_bytes` value.

## 5. Keep a plan over a seed from claiming shared bytes

**Story:** As a developer or agent, I want `gc --plan` on a seeded target not to report shared bytes as freed, so that the bytes a plan promises are bytes the volume would regain.

- **SEED-5.1** WHEN GC measures a compressed file with the eviction-accounting capability on and the file reads `SHARES_ALL` THE SYSTEM SHALL count its bytes as still shared and not as freed.

## 6. Read how a seed went after the first build

**Story:** As a developer or agent, I want `cargo hive status` to report what became of the seeded units in a target, so that I can see from a receipt that the first build reused them.

- **SEED-6.1** WHEN seed clones into a target THE SYSTEM SHALL write through the state module a seed record holding the target and, for each cloned unit, its name-meta and the modification time of its stamp.
- **SEED-6.2** WHEN `status` reports a target that has a seed record THE SYSTEM SHALL report four counts for that target: seeded units still fresh, seeded units not needed, seeded units recompiled, and seeded units with files missing.
- **SEED-6.3** THE SYSTEM SHALL count a seeded unit as still fresh when it is reached over the fingerprint graph from a unit compiled since the seed, and its stamp's modification time equals the recorded one.
- **SEED-6.4** THE SYSTEM SHALL treat a unit in the target as compiled since the seed when it is not in the seed record or its stamp's modification time differs from the recorded one.
- **SEED-6.5** THE SYSTEM SHALL count a seeded unit as recompiled when its stamp's modification time differs from the recorded one.
- **SEED-6.6** THE SYSTEM SHALL count a seeded unit as not needed when its files are present, its stamp equals the recorded one, and no unit compiled since the seed reaches it.
- ~~**SEED-6.7**~~ superseded by SEED-6.11 (design review, 2026-11-02: holding every file's path made `seeds.json` too large, and a build never removes files inside `build/<dir>/`) — was: ~~THE SYSTEM SHALL count a seeded unit as having files missing when its `.fingerprint/` directory or any file of its entry is gone from the target.~~
- **SEED-6.8** IF a target has no seed record, or the record cannot be read THEN THE SYSTEM SHALL report none of the four counts for that target and guess none of them.
- **SEED-6.9** THE SYSTEM SHALL compute the four counts when `status` runs, reporting them as of that moment, and SHALL store no snapshot of the first build.
- **SEED-6.10** THE SYSTEM SHALL put none of the four counts in seed's own receipt.
- **SEED-6.11** THE SYSTEM SHALL count a seeded unit as having files missing when its `.fingerprint/` directory or its `build/<dir>/` directory is gone, or `deps/` holds fewer of its files than the seed cloned.
- **SEED-6.12** IF the target directory is not the one the record was written for THEN THE SYSTEM SHALL treat the target as having no seed record.

## 7. Read what seed did from its receipt

**Story:** As a developer or agent, I want one receipt from seed that separates the fill from the clone, so that I can tell what happened without reading the store.

- **SEED-7.1** WHEN seed finishes THE SYSTEM SHALL report separate counts for the fill part (entries added, entries already held, source targets read, source targets skipped) and the clone part (units chosen, units cloned, units dropped by SEED-3.6, units not cloned by SEED-1.7).
- **SEED-7.2** WHEN seed finishes THE SYSTEM SHALL report how many cloned units have a stamp older than the GC window, or no stamp, at the end of the seed.
- **SEED-7.3** THE SYSTEM SHALL put no duration, and no estimate of time saved, in seed's receipt.
- **SEED-7.4** WHEN seed runs THE SYSTEM SHALL report the toolchain release of the checkout and the checkout's path in the receipt.
- **SEED-7.5** WHEN seed is cancelled THE SYSTEM SHALL stop before the next clone, report what it had done, and exit with code 130.

## 8. Teach agents to seed

**Story:** As an agent, I want the hive skill to tell me when and how to run `cargo hive seed`, so that I seed a new lane without being told by a person.

- **SEED-8.1** THE SYSTEM SHALL document `seed` in `plugin/skills/hive/SKILL.md`, inside its existing headings: what it does, that it is for a new secondary checkout before its first build, that an existing target gets nothing, and the `status` counts to read afterward.
- **SEED-8.2** THE SYSTEM SHALL add `seed` to the commands the skill's `Version first` section says not to run when the hive version does not match.
- **SEED-8.3** THE SYSTEM SHALL update `tests/skill.rs`, including its exhaustive match over the CLI's commands and its shipped-command list, so that the test covers `seed`, and keep the skill text `skills/hive/eval.json` quotes in step with the skill.

## 9. Quality attributes

**Section-kind:** nfr

**Story:** As a stakeholder, I want measurable quality targets for this feature, so that how-well is not left implicit.

- **Performance:** None — no latency target; hive reports only deterministic counters (ARCH-10, GOAL-4), and time is measured only by `cargo hive bench`. The first seed compresses the registry part and every seed scans; the owner accepted that cost, and it reopens the decision if `cargo hive bench` or the first klynt run shows the scan is significant against the build saved, or if concurrent seeds make builds in the source target wait.
- ~~**SEED-9.1**~~ superseded by SEED-9.8 (design review, 2026-11-02: leaving out one file would publish a partial unit as complete) — was: ~~THE SYSTEM SHALL clone into the store only regular files and directories found without following a symbolic link, and SHALL report and leave out any other entry.~~
- **Security:** **SEED-9.2** THE SYSTEM SHALL treat each relative path in a store entry as untrusted, and clone it only when its destination lies inside the checkout's target directory — verified by a test that offers an entry path containing `..`.
- **Reliability:** **SEED-9.3** WHEN seed is stopped at any point during a fill THE SYSTEM SHALL leave no entry available that has an uncompressed or missing file, and a later seed SHALL be able to fill that key — verified by a test that stops a fill after each step of one entry.
- **Reliability:** **SEED-9.4** THE SYSTEM SHALL write the store's metadata, the toolchain table and the seed record only through the state module, atomically, and SHALL hold the state lock across no clone and no compression — verified by a test that fails the write and finds no partial record, and a test that takes the state lock from another process during a fill.
- **Reliability:** **SEED-9.5** WHEN `cargo build` runs at the same time as seed into a new target THE SYSTEM SHALL overwrite no file — verified at the `clone_file` boundary by a test where the destination exists. Whether the race can corrupt a build is not proven: the only evidence is 4 trials using `cp -c` (`.skills/SEED/decisions.md`), and a corrupted file or a recompiling second build reopens the decision.
- **Reliability:** **SEED-9.6** WHEN seed runs on klynt in a new linked worktree whose target does not exist THE SYSTEM SHALL meet SEED-1.9 and SEED-4.1 — verified by a recorded manual run that also records the four counts of SEED-6.2 and, beside them, the comparison with spike 7's 466 needed and 31 compiled. Neither the comparison nor Cargo's printed fresh count is an expected value for those counts.
- **Reliability:** **SEED-9.7** THE SYSTEM SHALL bound every directory walk and file read of fill, seed and the `status` counts, and SHALL report hitting a bound as an error — verified by a test with a low bound.
- **Security:** **SEED-9.8** THE SYSTEM SHALL clone into the store only regular files and directories found without following a symbolic link, and SHALL report a unit holding anything else and fill nothing for that unit — verified by a test whose source unit holds a symbolic link to a file outside the target.
- **Accessibility:** None — headless CLI.

## 10. Existing behavior this feature must not change

Files and surfaces this change touches:
- `src/cli.rs`: a new `seed` command, and `status` gains counts for a target with a seed record. Other commands and exit codes are at risk. Guarded below.
- `src/receipt.rs`: seed's receipt is added; existing receipts are at risk. Guarded below.
- `src/state.rs`: gains the store metadata, the toolchain table and the seed record; existing files and keys are at risk. Guarded below.
- `src/doctor.rs`: `resolve_toolchain` must also give the whole `-vV` text; `release`, capability checks and `require` are at risk. Guarded below.
- `src/fingerprint.rs`: `Unit` keeps the toolchain value; the format signature is at risk. Guarded below.
- `src/liveness.rs`: called by fill and by the `status` counts, its signature and verdicts unchanged; `embedded_hashes` and `unit_timestamp` may become visible to other modules. Guarded below.
- `src/gc/ledger.rs`: SEED-5.1 changes one case; every other case is at risk. Guarded below.
- `src/plan_tier/locks.rs`: `acquire` is called by fill; plan and apply use it. Guarded below.
- `src/cargo_target.rs`, `src/discover.rs`: `identify` and `resolve` are called; `status` and ownership are at risk. Guarded below.
- `src/platform/mod.rs`, `src/platform/apfs.rs`, and the other `impl Fs`: a directory-creating method and a rename may be added; existing methods are at risk. Guarded below.
- `plugin/skills/hive/SKILL.md`, `skills/hive/eval.json`, `tests/skill.rs`: documented commands and tests. Guarded below.
- `Cargo.toml`: a new dependency needs the owner's approval (`docs/standards/code.md`). Guarded below.
- `CONTEXT.md`, `README.md`: documentation only; no behavior to guard.

- **SEED-10.1** (guard) WHEN `cargo hive gc`, `gc --plan`, `apply`, `repo add`, `repo list`, `doctor` or `explain` runs THE SYSTEM SHALL CONTINUE TO print the receipt it printed before this feature, with the same schema version, count names and exit codes, apart from the byte values SEED-5.1 changes. (2026-11-09: STOREGC adds store counts to bare `gc`; target-deletion counts are unchanged, STOREGC-10.1.)
- **SEED-10.2** (guard) WHEN `cargo hive status` reports a target with no seed record THE SYSTEM SHALL CONTINUE TO report it with the same count names and values as before this feature.
- **SEED-10.3** (guard) WHEN any hive command runs once hive's state is open, seed included, THE SYSTEM SHALL CONTINUE TO satisfy CORE-5.1 to CORE-5.7, and to use exit code 0 for success, 1 for a refusal or failure, 2 for an unsupported OS and for argument errors, and 130 for a cancel.
- **SEED-10.4** (guard) WHEN `liveness()` runs on any target, a seeded target before its first build included THE SYSTEM SHALL CONTINUE TO return the verdicts of LIVE stories 1 and 2, with its signature unchanged, roots dated by the stamp inside the GC window, and not-current units dated as LIVE dates them.
- ~~**SEED-10.5**~~ superseded by SEED-10.15 (design, 2026-11-02: `locks::acquire` returns `Skipped(Missing)` for a profile with `.fingerprint/` and no lock file, so `apply` skips a target no build has locked; measured) — was: ~~(guard) WHEN `gc --plan` meets a seeded target before its first build THE SYSTEM SHALL CONTINUE TO list the dead units `liveness()` returns, seeded units with stale or absent stamps included, and an `apply` of that plan SHALL CONTINUE TO delete them.~~
- **SEED-10.6** (guard) WHEN GC decides what to delete THE SYSTEM SHALL CONTINUE TO use `embedded_hashes` for a unit's files, `.rcgu.o` included.
- **SEED-10.7** (guard) WHEN the ledger measures an uncompressed file, or a compressed file that does not read `SHARES_ALL` THE SYSTEM SHALL CONTINUE TO count its bytes as it did before this feature.
- **SEED-10.8** (guard) WHEN `locks::acquire` is called by plan or apply THE SYSTEM SHALL CONTINUE TO return the same results for the same target, including `Skipped(Missing)`.
- **SEED-10.9** (guard) WHEN `cargo_target::identify` reads a directory THE SYSTEM SHALL CONTINUE TO read only that directory, with no list of seeded paths from state, and to require the tag line and a profile dir.
- **SEED-10.10** (guard) WHEN the capability gate runs THE SYSTEM SHALL CONTINUE TO compare the recorded release with the release it is given, and the doctor checks SHALL CONTINUE TO map to the same capabilities.
- **SEED-10.11** (guard) WHEN the fingerprint reader meets a file outside the known format signature THE SYSTEM SHALL CONTINUE TO report that profile as unsupported, and keeping the toolchain value on `Unit` SHALL change no accepted shape.
- **SEED-10.12** (guard) WHEN hive computes ownership or the target directory THE SYSTEM SHALL CONTINUE TO run no cargo subcommand but `cargo metadata --no-deps --format-version 1 --locked --offline` with the scrubbed environment, and to compile and replay nothing.
- **SEED-10.13** (guard) WHEN the skill tests run THE SYSTEM SHALL CONTINUE TO require that the skill body stays within its line budget and that every command line in it parses with the real CLI.
- **SEED-10.14** (guard) WHEN this feature is built THE SYSTEM SHALL CONTINUE TO depend on nothing beyond its existing crates unless the owner approves one.
- **SEED-10.15** (guard) WHEN `gc --plan` meets a seeded target before its first build THE SYSTEM SHALL CONTINUE TO list the dead units `liveness()` returns, seeded units with stamps outside the window or absent included, and an `apply` of that plan SHALL CONTINUE TO skip that target with reason `lock_missing` while no build has created its lock files.

## Out of Scope

- Seeding a protected checkout, or anything hive cannot classify.
- Seeding into a target path that exists: adding missing units, matching against an existing target's fingerprints, resuming an interrupted seed.
- A try-lock on the destination target, and handling a build that starts while seed clones, beyond `clone_file` not overwriting.
- A `fill` command, or any fill that does not run inside seed.
- `set_mtime` on a stamp, and `write_file` of the tag.
- Adding any protection of seed files from an `apply` between seed and the first build. `apply` already skips such a target while it has no lock files (SEED-10.15); hive adds nothing for a target whose lock files exist.
- Evicting, capping or stopping the store at a byte level, and any store GC: ROAD-10. Until then the store only grows, stopping means deleting it by hand, and the feature is not for daily use. (2026-11-09: bare `gc` bounds the store; SEED still does not evict, STOREGC / ROAD-10.)
- Time claims, and `cargo hive bench`: ROAD-11.
- Workspace, git and path units, and a registry other than the sources Cargo keeps under its registry source directory.
- Real copies into the store, and a store on a different volume from the source or the checkout.
- A checkout whose `Cargo.toml` is not at its root, and more than one workspace in a checkout.
- The new nightly build-dir layout (ADR-0001, vision): reported, never acted on.
- Debugging a dependency through loose `.o` files in a seeded worktree: untried, accepted.
- A daemon, a rustc wrapper, a cargo shim or a hook (ARCH-1).
- `status` calling a target in use an orphan candidate when the metadata cache hits (`.skills/SEED/decisions.md`): a separate root-cause investigation.

## Open Questions

None. Three items are left for `design-solution`, none of them a requirement: receipt field names, the hash of the `-vV` text and where the toolchain table lives, and when a clone id is written to the store's metadata.
