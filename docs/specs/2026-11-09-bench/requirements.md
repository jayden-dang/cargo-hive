# Requirements: Bench (measured cold-versus-seeded first builds)

Feature code: BENCH
Status: Draft
Date: 2026-11-09

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

Roadmap item: ROAD-11 (MILE-3, `docs/roadmap/INDEX.md`). Tracker issue #6, which counts as claimed with no
assignee (operator, 2026-11-09).

Sources:
- The approved close package and its cards, by absolute path:
  `/Users/jayden/Developer/experiments/cargo-zip/.skills/_pending-bench/auto/close-package.md` (r2) and
  `.../auto/cards/01`–`05`. The ledger is `.../auto/ledger.md`.
- Locks this feature builds on: GOAL-4 (`docs/product/vision.md:35`), ARCH-10 (`docs/architecture/INDEX.md:36`),
  the MILE-3 outcome and the ROAD-11 line (`docs/roadmap/INDEX.md:120,125`), and the SEED spec
  (`docs/specs/2026-11-02-store-seed/`).

**How labels read.** No value here is the operator's own choice unless it says so. `[Weigh Wn]` marks a value
bench-interpret picked under the charter (ledger line Wn). `[Owner-reserved]` marks the sampling method, which
`docs/specs/2026-10-25-foundation/requirements.md:159` reserves for the project owner (forbid-guess); it is
carried as Weigh until the operator confirms it. `[Proposed Fn]` marks a value bench-work put to
bench-interpret with this file; after review every such value is Weigh. An unlabeled criterion follows from a Lock, from the code, or from an
earlier unlabeled criterion.

Terms (`CONTEXT.md`): bench sample, cold sample, seeded sample, prime, secondary checkout, seed, fill, store,
receipt. A **run** below means one `cargo hive bench` invocation. An **arm** is the set of its cold samples
or the set of its seeded samples. **n** is the number of samples in one arm.

## 1. Measure cold against seeded

**Story:** As a developer or agent, I want to run `cargo hive bench` on fresh secondary checkouts I made, so
that one receipt tells me, on this machine, how long a new lane's first build takes cold, how long seeding it
takes, and how long its first build takes after the seed.

- **BENCH-1.1** WHEN `cargo hive bench --cold <checkout>… --seeded <checkout>…` passes every check of story 2
  THE SYSTEM SHALL run one prime before any sample: a fill of the store from the repo's owned targets, with
  no destination checkout. [Owner-reserved, Weigh W4]
- **BENCH-1.2** THE SYSTEM SHALL run the samples in the fixed order cold, seeded, seeded, cold, repeated
  (cold_1, seeded_1, seeded_2, cold_2, cold_3, seeded_3, …), taking each arm's checkouts in the order the
  caller gave them. [Owner-reserved, Weigh W4]
- **BENCH-1.3** WHEN a cold sample runs THE SYSTEM SHALL run the timed build (story 3) in that checkout with
  no seed, and report the build's milliseconds.
- **BENCH-1.4** WHEN a seeded sample runs THE SYSTEM SHALL seed that checkout as `cargo hive seed` does (fill,
  then clone), then run the timed build in it, and report four milliseconds: the whole seed, its fill part,
  its clone part, and the build. The fill part runs from the start of the owned-target walk to the end of the
  fill. The clone part runs from there to the end of the seed record's write, covering the store tag check,
  the store read, the choice of units, the clones and the record. So the whole seed equals fill plus clone.
  [Weigh W1, W8; part boundaries Weigh W14]
- **BENCH-1.5** THE SYSTEM SHALL report each sample as one finding of kind `bench.sample` carrying its
  checkout path, its arm, its 1-based position in the run, and its measured integers. A seeded sample also
  carries `fill_targets_read`.
- **BENCH-1.6** THE SYSTEM SHALL record the 1-minute load average immediately before and immediately after
  each sample, as an integer in hundredths, and SHALL refuse or skip no sample because of load.
  [Weigh W10]
- **BENCH-1.7** THE SYSTEM SHALL report the prime apart from the samples, with its milliseconds, the entries it
  added and the targets it read, and SHALL include the prime in no summary. [Owner-reserved, Weigh W4]
- **BENCH-1.8** WHEN every sample of both arms has finished THE SYSTEM SHALL report, in the receipt's
  `counts`, the median, p90, min, max and spread of four series: cold build milliseconds, seeded build
  milliseconds, seeded seed milliseconds, and seeded seed-plus-build milliseconds. [Weigh W1]
- **BENCH-1.9** THE SYSTEM SHALL compute the median as the middle value at odd n, and at even n as the floor
  of the mean of the two middle values. It SHALL compute p90 as the value at 1-based rank ceil(9n/10), and
  spread as max minus min, all in integer arithmetic. [Weigh W7, W10]
- **BENCH-1.10** THE SYSTEM SHALL measure every reported millisecond with a monotonic clock read on this
  machine around a step the run itself performed: a timed Cargo child, or an in-process prime, seed, fill or
  clone. [Weigh W10]
- **BENCH-1.11** THE SYSTEM SHALL write the run's results as one receipt of the existing schema 2 with
  `command` `bench`. Every measured time and count is an integer in `counts` or in a finding's `measured`.
  Text such as the command line or a variable's value goes in a finding's `measured` text or its detail. Unrun
  steps go in the existing `unmeasured` list. The receipt gets no new top-level key. No key name it adds contains `seconds`, `duration`, `elapsed`,
  `saved` or `estimate`.
- **BENCH-1.12** WHEN a run finishes every sample THE SYSTEM SHALL exit with code 0.

## 2. Refuse an unfair bench before anything builds

**Story:** As a developer or agent, I want bench to refuse a set of checkouts that cannot give a fair
comparison, so that I never read a number from a sample that was not cold, not fresh, or not the same code.

- **BENCH-2.1** IF any checkout given is missing, is not a checkout of a registered repo, or is a protected
  checkout THEN THE SYSTEM SHALL refuse the run.
- **BENCH-2.2** IF the checkouts given belong to more than one registered repo THEN THE SYSTEM SHALL refuse
  the run.
- **BENCH-2.3** IF the checkouts given are not all at one HEAD commit THEN THE SYSTEM SHALL refuse the run.
  [Weigh W2]
- **BENCH-2.4** IF any checkout given has uncommitted changes to tracked files THEN THE SYSTEM SHALL refuse
  the run. [Weigh W13]
- **BENCH-2.10** THE SYSTEM SHALL read each checkout's HEAD and its uncommitted state with git's optional locks
  off (`git --no-optional-locks`), so the check never rewrites the checkout's index, a repo file under ARCH-2.
  By default `git status` refreshes the index and writes it out (Context7 `/git/htmldocs`, git-status
  "BACKGROUND REFRESH"). [Weigh W13]
- **BENCH-2.5** IF the target directory Cargo resolves for any checkout given already exists, or its build
  directory differs from its target directory THEN THE SYSTEM SHALL refuse the run. [Weigh W2; the build
  directory clause follows from seed's own `target.split` refusal, src/seed/place.rs:206, added at design]
- **BENCH-2.6** IF the two arms differ in count, either arm has fewer than 2 checkouts, or one checkout is
  given more than once THEN THE SYSTEM SHALL refuse the run. [Weigh W10]
- **BENCH-2.7** IF doctor has not passed Seeding, StoreCompression and Deletion for the toolchain release the
  checkouts resolve THEN THE SYSTEM SHALL refuse the run, as `seed` refuses (`src/cli.rs:1496-1506`).
- **BENCH-2.8** IF the toolchain cannot be resolved for a checkout given, or the checkouts resolve toolchains
  whose whole `rustc -vV` text differs THEN THE SYSTEM SHALL refuse the run. Seed matches the store on that
  text (`choose::choose`, src/seed/mod.rs:221). [Weigh W16; compared on the whole text since design]
- **BENCH-2.11** IF two checkouts given resolve the same target directory THEN THE SYSTEM SHALL refuse the
  run, because the first build would create it and every later seeded sample would then clone nothing
  (src/seed/place.rs:218-232). [Weigh W17]
- **BENCH-2.12** IF the workspace root that Cargo resolves for a checkout given has no `Cargo.lock` THEN THE
  SYSTEM SHALL refuse the run, because every timed build runs `--locked`, and Cargo exits with an error when
  the lock file is missing (Cargo docs, `--locked`, via Context7 `/rust-lang/cargo`). [Weigh W20. bench-interpret measured that `cargo metadata --no-deps --locked` passes with no lock
  file, so no existing step catches it.]
- **BENCH-2.9** WHEN the run is refused THE SYSTEM SHALL run no prime, seed or build, report one `refusal`
  finding per failing checkout naming its reason, and exit with code 1.

## 3. Choose the build that is timed

**Story:** As an agent, I want to time the build command my lane will actually run first, so that the
seeded number answers my lane's question.

- **BENCH-3.1** WHERE `--build-cmd` is not given THE SYSTEM SHALL time `cargo build --offline --locked` at the
  checkout's root.
- **BENCH-3.2** WHEN `--build-cmd` is `build`, `check-all`, `clippy-all`, `test-no-run` or
  `check-lib=<package>` THE SYSTEM SHALL time, respectively, `cargo build`, `cargo check --all-targets`,
  `cargo clippy --all-targets`, `cargo test --no-run` or `cargo check -p <package> --lib`, each with
  `--offline --locked` added, at the checkout's root. [Weigh W5]
- **BENCH-3.3** IF `--build-cmd` names anything else THEN THE SYSTEM SHALL reject the arguments before any
  prime, seed or build, with a non-zero exit.
- **BENCH-3.4** THE SYSTEM SHALL report the exact Cargo command line it timed in the receipt.

## 4. See whether the seed served the build

**Story:** As a developer or agent, I want each seeded sample to show how many seeded units its build reused,
so that a slow seeded number caused by a command the store cannot serve is not mistaken for seeding failing.

- **BENCH-4.1** WHEN a seeded sample's build finishes THE SYSTEM SHALL record in that sample's finding the
  four counts SEED-6.2 defines for its target (seeded units still fresh, not needed, recompiled, files
  missing), computed as `status` computes them (SEED-6.3 to SEED-6.6, SEED-6.9, SEED-6.11). [Weigh W5]
- **BENCH-4.2** WHEN the seed of a seeded sample clones 0 units with a finding (for example
  `toolchain.missing` or `units.empty`) THE SYSTEM SHALL still run the build, report the sample, and carry
  the seed's finding in the receipt.
- **BENCH-4.3** IF a seeded sample's target has no seed record that `status` would read (the seed cloned 0
  units, or SEED-6.8 or SEED-6.12 applies) THEN THE SYSTEM SHALL report none of the four counts for that
  sample, guess none of them, and list them in the receipt's `unmeasured` with the reason.

## 5. Build every sample in the same environment, with no compiler wrapper

**Story:** As a developer, I want bench to time a plain rustc build in one environment for both arms, so that a
cache wrapper such as sccache cannot pass off a cache read as a cold build.

- **BENCH-5.1** THE SYSTEM SHALL build one environment map per run and give it to every timed build of that
  run. The prime and the seed keep the environment scrubs they use today (BENCH-9.6). [Weigh W6]
- **BENCH-5.2** THE SYSTEM SHALL build that map from the caller's environment, as new behavior that goes beyond
  ARCH-13's floor (today `resolve` scrubs only the target-dir variables and `GIT_*`, src/cargo_target.rs:352-359,
  and the proof replay removes the wrapper variables, tests/common/prove.rs:797-803): without `CARGO_TARGET_DIR`,
  `CARGO_BUILD_TARGET_DIR`, `CARGO_BUILD_BUILD_DIR`, any `GIT_*`, any `CARGO_PKG_*`, `CARGO_MANIFEST_DIR`,
  `OUT_DIR`, `CARGO_BUILD_RUSTC_WRAPPER`, `CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER`, `RUSTC`, `CARGO_BUILD_RUSTC`,
  `RUSTUP_TOOLCHAIN` or `RUSTUP_TOOLCHAIN_SOURCE`; with `RUSTC_WRAPPER` and `RUSTC_WORKSPACE_WRAPPER` set to
  the empty string; and pinned to the toolchain `seed` resolves for the checkout (`doctor::resolve_toolchain`),
  not one the caller's current directory selects (ARCH-13). The pin is `RUSTC` set to the resolved rustc, plus
  `RUSTUP_TOOLCHAIN` set to the resolved name when rustup resolved one, plus `RUSTUP_AUTO_INSTALL=0`.
  [Weigh W6; the RUSTC removal and pin Weigh W18]
- **BENCH-5.3** WHEN the caller's environment sets `RUSTC_WRAPPER`, `RUSTC_WORKSPACE_WRAPPER`,
  `CARGO_BUILD_RUSTC_WRAPPER` or `CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER` to a non-empty value THE SYSTEM SHALL
  report one finding of kind `env.wrapper.overridden` per variable, naming it and its value. [Weigh W6]
- **BENCH-5.4** WHEN the caller's environment sets `RUSTC` or `CARGO_BUILD_RUSTC` THE SYSTEM SHALL report a
  finding of kind `env.rustc.overridden` naming the variable and its value. The timed build uses the pin of
  BENCH-5.2 instead. [Weigh W6, W18]

## 6. Stop a bench

**Story:** As a developer, I want Ctrl-C to stop a bench promptly and keep what it measured, so that a long
run I cancel still tells me what finished.

- **BENCH-6.1** WHEN the run is cancelled by SIGINT or SIGTERM THE SYSTEM SHALL kill the running child's
  process group, start no further step, and exit with code 130. [Weigh W10]
- **BENCH-6.2** WHEN the run is cancelled THE SYSTEM SHALL report the prime and every finished sample as
  story 1 does, and list each sample not finished in the receipt's `unmeasured`, with the reason
  `cancelled`. [Weigh W10]
- **BENCH-6.3** WHEN the run is cancelled or fails THE SYSTEM SHALL report no summary of BENCH-1.8.
  [Weigh W12]
- **BENCH-6.4** THE SYSTEM SHALL remove no checkout and no target, whether a run finishes, fails or is
  cancelled. [Weigh W2]

## 7. A failed step ends the run

**Story:** As a developer or agent, I want a bench whose build or seed fails to stop and say which step
failed, so that a failed build is never averaged in as a time.

- **BENCH-7.1** IF the prime, a seed or a timed build fails (a non-zero Cargo exit, a seed error, or a full
  disk) THEN THE SYSTEM SHALL start no further sample, report the finished samples and one finding naming
  the failed step with its reason (Cargo's exit code for a build), and exit with code 1. [Weigh W12]
- **BENCH-7.3** IF, after the prime, no unit in the store lists the repo's main checkout THEN THE SYSTEM SHALL
  start no sample, report the prime and one finding of kind `bench.nothing_to_seed` saying to build the repo
  with the timed command first, and exit with code 1. Without this stop, seeded_1's own fill would read
  cold_1's fresh target, and the store's first, compressing fill would land inside seeded_1's seed time, which
  the prime exists to keep out (BENCH-1.7). [Weigh W19]
- **BENCH-7.2** IF a timed build runs past a fixed 60-minute deadline THEN THE SYSTEM SHALL kill its process
  group and treat the sample as failed under BENCH-7.1, naming the deadline (`docs/standards/code.md:14`).
  There is no flag for the deadline. [Weigh W15]

## 8. Learn bench from the hive skill

**Story:** As an agent, I want the hive skill to teach me how to run a bench, so that I set up the checkouts,
run it, read it and clean up without reading the source.

- **BENCH-8.1** THE SYSTEM SHALL document `bench` in `plugin/skills/hive/SKILL.md`, inside its existing
  headings: what it measures, making the 2n fresh checkouts with `git worktree add` before the run and
  removing them after (with `git worktree remove`, which also deletes a gitignored target), an even n per
  arm, the five build shapes, that p90 is the max for n ≤ 9, that bench builds `--offline --locked`, so
  the repo needs a committed `Cargo.lock` and every dependency already in Cargo's cache (building the main
  checkout once with the timed command does both, and gives the prime a target to fill from), and that
  `bench` is the only command whose receipt holds times. [Weigh W9 for the even-n advice; the lockfile and
  cache line Weigh W20]
- **BENCH-8.2** THE SYSTEM SHALL tell agents in the skill to run `bench` only when asked, because each run
  makes 2n full builds and their targets. [Weigh W16]
- **BENCH-8.3** THE SYSTEM SHALL list `cargo hive bench` in the README's command table.

## 9. Keep what bench touches working

**Story:** As a developer, I want seed, status and every other receipt to behave as before once bench ships, so
that adding bench breaks nothing I already rely on.

Files this change touches, and what each guards:
- `src/bench/` (new module): no behavior to guard.
- `src/cli.rs`: the new subcommand and dispatch. Guards BENCH-9.1, 9.2, 9.3.
- `src/seed/` (the seed steps bench calls and times): guards BENCH-9.1, 9.2.
- `src/receipt.rs`: the module doc comment is reworded to name bench as the one command with times
  (a comment, no behavior); guard BENCH-9.3 for the receipt shape.
- `src/git.rs`: a HEAD and dirty-tree reader is added; guard BENCH-9.5.
- `src/cargo_target.rs`: gains a bounded child runner for bench's own children. `resolve` and its inline
  scrub (src/cargo_target.rs:352-359) are untouched; bench reads the same `TARGET_VARS` list. Guard
  BENCH-9.6.
- `src/platform/`: the `Fs` trait gains a load-average reading (`libc::getloadavg`), so macOS code stays
  behind it (ARCH-15). No existing method changes; no behavior to guard.
- `plugin/skills/hive/SKILL.md`, `tests/skill.rs`, `skills/hive/eval.json`: guard BENCH-9.4.
- `README.md`: no behavior to guard.
- `CONTEXT.md` (done in commit 089aa7c): no behavior to guard.

- **BENCH-9.1** (guard) WHEN `cargo hive seed` runs THE SYSTEM SHALL CONTINUE TO fill, clone, record, refuse
  and report exactly as the SEED spec requires, with its receipt's counts and findings unchanged.
- **BENCH-9.2** (guard) WHEN `cargo hive status` reports a target with a seed record THE SYSTEM SHALL CONTINUE
  TO report the four counts of SEED-6.2 as SEED-6.3 to SEED-6.9 define them.
- **BENCH-9.3** (guard) WHEN any command other than `bench` writes a receipt THE SYSTEM SHALL CONTINUE TO
  write the same schema version and keys as before, and no receipt SHALL CONTINUE TO have a top-level key
  containing `saved`, `duration`, `seconds`, `elapsed` or `estimate`.
- **BENCH-9.4** (guard) WHEN `tests/skill.rs` runs THE SYSTEM SHALL CONTINUE TO hold the skill body to the
  seven `##` headings in their order and at most 150 lines, parse every command line in it with the real
  CLI, and match its version to the crate's.
- **BENCH-9.5** (guard) WHEN discovery classifies checkouts THE SYSTEM SHALL CONTINUE TO return the classes
  and root-commit matches it returns today.
- **BENCH-9.6** (guard) WHEN `cargo metadata` runs for ownership THE SYSTEM SHALL CONTINUE TO run with the
  same arguments, environment scrub and deadline as today.
- **BENCH-9.7** (guard) WHEN a fresh state root is used by any command THE SYSTEM SHALL CONTINUE TO hold only
  the state files it holds today; bench adds no state file of its own.

## 10. Quality attributes

**Section-kind:** nfr

**Story:** As a stakeholder, I want measurable quality targets for this feature, so that how-well is not left implicit.

- **Performance:** None. Bench's own cost is the 2n builds it exists to time. No latency target is set for
  the steps between them. The prime is reported so a reader sees its cost (BENCH-1.7).
- **Security:** **BENCH-10.1** THE SYSTEM SHALL write no repo file and nothing under `~/.cargo` itself, create
  and remove no checkout, and remove no target. The only process it spawns that writes outside a target-dir
  clone is the timed Cargo build, run with `--offline --locked` (ARCH-2 under the reading in the close
  package's D3, `[Weigh W3]`, an interpretation of the spine for the operator to confirm; ARCH-18). It runs
  no test, benchmark or binary target of the checkout. Story 3's shapes only compile, though the checkout's
  own build scripts and proc-macros run as they do in the owner's own build (docs/standards/testing.md:44).
  Cargo's own writes under `~/.cargo` (such as `.global-cache`) are Cargo's, under the same D3 reading. Verified by a test
  that runs bench on fixture checkouts with a scratch `CARGO_HOME` and `HIVE_HOME`, as the fixture helpers
  do (tests/common/cargo_fixture.rs), and checks that the fixture repo's tracked files are unchanged and that
  every checkout and target still exists afterwards, and that each checkout's git index is byte-identical
  before and after the refusal checks.
- **Reliability:** **BENCH-10.2** Every child process bench's own code spawns (the timed builds and the git
  HEAD and dirty readers) has an explicit deadline and bounded captured output, and every file read bench
  adds has a bounded size (`docs/standards/code.md:14`). The git calls that discovery already makes
  through `Git::run` (src/git.rs:66) have no deadline today. That existing gap is out of scope here; see
  Open Questions. Hitting one is a failed step
  under BENCH-7.1, never a silent skip. Verified by a test that injects a deadline shorter than a fixture
  build through a test seam, not a flag.
- **Accessibility:** None. A headless CLI whose output is JSON or one line of text, as every other command.

## Out of Scope

- Creating, cloning or removing checkouts or targets from hive code; the caller does both.
- A bench history file, a new receipt field, a float slot, a schema bump, or any new state file.
- Comparisons across machines, trend lines, estimated time saved, and seed versus sccache.
- Any build command outside story 3's five shapes, and free Cargo arguments or features.
- A load threshold, or refusing or retrying a sample because of load.
- Several repos in one run.
- Store eviction and the store cap (ROAD-10); rewriting the skill as one guide (ROAD-13).
- A standalone fill command (SEED decision 7, `.skills/SEED/decisions.md:12` in the main checkout).

## Open Questions

- How `cargo hive bench` samples cold versus seeded (BENCH-1.1, 1.2, 1.7 carry a Weigh pick) — project owner —
  2026-10-25 — forbid-guess (cấm đoán). Raised at `docs/specs/2026-10-25-foundation/requirements.md:159`. The
  operator confirms or replaces the pick at the spec sheet.
- `Git::run` (src/git.rs:66), used by discovery for every command, spawns git with no deadline, against
  `docs/standards/code.md:14`. bench inherits that through `discover::checkouts`. Found at design review,
  2026-11-09; it is outside ROAD-11 and needs its own fix. Owner: the project owner — 2026-11-09 — not
  guessed here.
