# Requirements: Worktree scorecard (what an agent worktree costs, cold and seeded)

Feature code: SCORECARD
Status: Implemented
Approved: 2026-11-22 12:46 by the operator (Jayden), "approve" on the spec sheet for #16 at feat/worktree-scorecard 226a925, relayed by the conductor (/Users/jayden/Developer/experiments/cargo-zip/.skills/_pending-worktree-scorecard/auto/operator-reply.md). The reply approves the sheet as a whole; the Open Questions O1 and O2 stay open as listed, and Task 9 still waits for the operator's own go-ahead.
Re-approved: 2026-11-23 by the operator (Jayden), with one bare "đồng ý" to a four-item plan: the realign changes listed in the PR #38 description; the 8.7 and Task 11 fixes; the new requirement 8.11 and Task 12 with the closure of O4; and the goal-backed targets in docs/scorecard.md. The same plan included the ARCH-10 and vision clarification (commit 706e2c0). It was one reply to that plan, on 2026-11-23.
Date: 2026-11-22
Realigned: 2026-11-23, to what was built (docs/scorecard.md at 99186de). O1 and O2 are answered on ticket #16 (2026-11-22); see Open Questions.

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

Roadmap item: ROAD-19 (MILE-5, `docs/roadmap/INDEX.md`). Tracker issue #16, assigned to
jayden-dang.

Sources:
- Issue #16 (Problem, Outcome, Acceptance, Out of scope) and `docs/roadmap/roads/ROAD-19.md`.
  The Must keep line was amended by the operator on 2026-11-22 (e0318de).
- The close package, approved by worktree-scorecard-interpret in round 2:
  `/Users/jayden/Developer/experiments/cargo-zip/.skills/_pending-worktree-scorecard/auto/close-package.md`.
  The decisions D1–D19 are cited below as "close package Dn".

Where a value in this file is a pick rather than a settled line, it carries its label:
**agent-decided** (a choice delegated to the working session) or **Weigh** (a pick by the
reviewing session). Neither is the operator's choice.

"The scorecard" below is the file `docs/scorecard.md`. "The steps" are its Rerun section.
"A run" is one pass through the steps on one machine and one hive commit. "The measured
repo" is the large private Rust workspace the scorecard measures. The owner answered O2 on
2026-11-22: the scorecard may name it and hold its measured numbers, and paths, user names and
host names stay out.

## 1. One scorecard lists every cost with its open target

**Story:** As the owner, I want one file listing every cost an agent's worktree pays, cold and
hive-seeded, each beside an open target, so that I can open `docs/scorecard.md` and set the
target for any row.

- **SCORECARD-1.1** THE SYSTEM SHALL keep exactly one scorecard, `docs/scorecard.md`, holding
  exactly one row for each of these ten metrics:
  - R1 first-command time for `build`
  - R2 first-command time for `check --all-targets`
  - R3 first-command time for `clippy --all-targets`
  - R4 first-command time for `test --no-run`
  - R5 first-command time for `check -p <package> --lib`
  - R6 the next build after a one-file edit in a workspace crate
  - R7 the private bytes each new worktree adds
  - R8 the units rebuilt because of a hive deletion
  - R9 stale results on a probe that changes a file a proc macro reads without declaring it
  - R10 the setup footprint

  Location agent-decided (close package D2). The row IDs R1–R10 are agent-decided (close
  package D3, renamed from SC-N so they cannot be read as requirement IDs).
- **SCORECARD-1.2** THE SYSTEM SHALL give every row a cold-arm cell and a seeded-arm cell. The
  cold arm is a checkout hive did not seed; the seeded arm is a checkout `cargo hive seed`
  seeded just before its first build.
- **SCORECARD-1.3** THE SYSTEM SHALL give every row a Source cell. It names the bench receipt
  count, or the hive receipt count, its value comes from, or reads "by hand (outside bench)",
  or, for R10, "listed" (Weigh W1).
- **SCORECARD-1.4** THE SYSTEM SHALL show each measured row's sample count and median for each
  arm on its own. Measured rows are R1–R9. The count is the number of valid samples: a sample
  voided under SCORECARD-3.5, 4.4 or 6.2 is not counted. A vacuous R8 sample
  (SCORECARD-5.2) is valid and is counted.
- **SCORECARD-1.5** THE SYSTEM SHALL record, for each measured row, the date it was measured,
  the hive commit, the toolchain, the machine and the evidence path. The toolchain is the
  `rustc -vV` release line. The machine is its model, chip and core count.
- **SCORECARD-1.6** THE SYSTEM SHALL give every row a Target (cold) slot, a Target (seeded) slot
  and a Target set date. The two-slot layout is agent-decided (close package D4), and what fills it is O1 (close package D5): two slots keep open
  whether the operator sets one target per row or one per arm.
- **SCORECARD-1.7** WHILE the operator has not set a row's target THE SYSTEM SHALL show that slot
  as "open — operator", with an empty Target set date. Only the operator fills a target slot.
- **SCORECARD-1.8** WHILE no run has measured a row THE SYSTEM SHALL show its value cells as "not
  yet measured".
- **SCORECARD-1.9** THE SYSTEM SHALL write every duration as integer milliseconds.

## 2. The five first-command rows come from bench

**Story:** As the owner, I want to fill a first-command row from one `cargo hive bench` run, so
that I can run the steps for one shape and copy that receipt's medians into its row.

- **SCORECARD-2.1** WHEN a run measures R1–R5 THE SYSTEM SHALL take each row from its own
  `cargo hive bench --build-cmd <shape> --json` run, with `<shape>` being `build`, `check-all`,
  `clippy-all`, `test-no-run` or `check-lib=$P` in that order. Each run uses 4 cold and 4 seeded
  worktrees made fresh with `git worktree add` at the main checkout's HEAD, after the main
  checkout has built that same shape. n = 4 per arm is agent-decided: it meets ≥3 (#16
  Acceptance 3; close package D8).
- **SCORECARD-2.2** WHEN a bench run for R1–R5 completes THE SYSTEM SHALL record:
  - the receipt's `cold_build_ms_median` as the cold value;
  - `seeded_total_ms_median` as the seeded value;
  - `seeded_seed_ms_median` and `seeded_build_ms_median` beside the seeded value.

  The seeded value as seed plus build is a default from #16 Problem ("Of the 73.6 s, 4.9 s is
  the seed"); close package D7.
- **SCORECARD-2.3** THE SYSTEM SHALL fix the package `$P` that R5 checks in the steps, and keep
  it the same from run to run.
- **SCORECARD-2.4** IF a bench run exits non-zero THEN THE SYSTEM SHALL keep that row's previous
  values and metadata unchanged, and the steps SHALL say to record the refusal or failure
  finding in the run's evidence.

## 3. The next build after an edit is timed by hand

**Story:** As the owner, I want the next build after a one-file edit timed by hand in each arm,
so that I can read what an agent waits for on its second build.

- **SCORECARD-3.1** THE SYSTEM SHALL measure R6–R9 on their own worktrees, 4 cold and 4 seeded,
  made fresh with `git worktree add` and never the worktrees a bench run used (agent-decided;
  close package D10).
- **SCORECARD-3.2** WHEN a pipeline worktree's first build has finished THE SYSTEM SHALL apply
  the one edit the steps fix, to the file `$FILE` in workspace crate `$P`. It then runs the
  `cargo` command line the `build` bench receipt records in its `bench.command` finding
  (`cargo build --offline --locked`), and times it in zsh with `zsh/datetime` `$EPOCHREALTIME`,
  rounded to integer milliseconds (agent-decided; close package D11).
- **SCORECARD-3.3** THE SYSTEM SHALL show R6's Source as "by hand (outside bench)" (ROAD-19.md
  Must keep, as amended 2026-11-22), with a note that its environment matches bench's
  (SCORECARD-3.4) but bench did not time it.
- **SCORECARD-3.4** THE SYSTEM SHALL run every by-hand `cargo` command of R6–R9, the pipeline's
  first build included, under the environment bench's `timed_build` builds
  (crates/cargo/src/env.rs:114-156), all of it:
  - the toolchain's own `cargo` binary, not the rustup proxy;
  - an environment cleared and refilled from the caller's, dropping `CARGO_TARGET_DIR`,
    `CARGO_BUILD_TARGET_DIR`, `CARGO_BUILD_BUILD_DIR`, `CARGO_MANIFEST_DIR`, `OUT_DIR`,
    `CARGO_BUILD_RUSTC`, `CARGO_BUILD_RUSTC_WRAPPER`, `CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER`,
    `RUSTC`, `RUSTUP_TOOLCHAIN`, `RUSTUP_TOOLCHAIN_SOURCE`, and every `GIT_*` and
    `CARGO_PKG_*` variable;
  - `RUSTC_WRAPPER` and `RUSTC_WORKSPACE_WRAPPER` set empty, `RUSTUP_AUTO_INSTALL=0`, `RUSTC`
    set to the toolchain's `rustc`, and `RUSTUP_TOOLCHAIN` set to the toolchain the bench
    receipt names.

  The `bench.command` line carries no toolchain and no environment
  (crates/cargo-hive/src/bench/command.rs:62-67). The match is agent-decided; ledger A10,
  req-review.md 3c.

- **SCORECARD-3.5** IF the pipeline's first build, the R6 rebuild, or the `cargo hive gc
  --checkout` run or rerun of SCORECARD-5.1, exits non-zero THEN THE SYSTEM SHALL void that
  worktree's samples for the row that failed and every row after it in the pipeline (design
  review finding 3; agent-decided; ledger A23).

## 4. Private bytes come from the volume

**Story:** As the owner, I want the bytes a new worktree adds to the disk recorded for each arm,
so that I can read how much private build output one more agent costs.

- **SCORECARD-4.1** WHEN a pipeline worktree is measured for R7 THE SYSTEM SHALL record the drop
  in that volume's free space, in bytes from `df -k`, between just after `git worktree add` and
  just after the worktree's first build. For the seeded arm, the drop includes the seed. The
  target-dir scope is a default from the vision Problem; the method is agent-decided (close
  package D12).
- **SCORECARD-4.2** WHEN the seeded arm is measured for R7 THE SYSTEM SHALL record the seed
  receipt's `clone_bytes_not_shared` beside the seeded value. It is labelled as the seed's
  registry part at seed time: it is 0 on a fully shared clone (README.md:91), so it differs from
  the free-space drop by design and never stands in for it.
- **SCORECARD-4.3** IF the seed receipt lists `clone_bytes_not_shared` under `unmeasured` THEN
  THE SYSTEM SHALL record the receipt's reason in its place, not a number.
- **SCORECARD-4.4** IF a seeded pipeline worktree's seed exits non-zero, or its receipt reports
  `clone_units_cloned` of 0 or a `source.none`, `toolchain.missing` or `store.tag` finding, THEN THE SYSTEM SHALL
  void that worktree's samples for R6–R9, because the worktree was not seeded (design review
  finding 4; agent-decided; ledger A23).

## 5. Units rebuilt after a hive deletion are counted

**Story:** As the owner, I want the units a hive deletion made Cargo rebuild counted for each
arm, so that I can see whether cleaning a worktree costs a rebuild.

- **SCORECARD-5.1** WHEN a pipeline worktree's R6 rebuild has finished THE SYSTEM SHALL run
  `cargo hive gc --checkout <worktree> --json`, then rerun the same `cargo` command line. The
  rerun passes `-v`, and the sample's value is the number of rustc invocations Cargo reports
  on that rerun: `Running` lines that carry `--crate-name`, one per unit compiled. A `Compiling`
  line is one per package, not per unit, so it is not used. Agent-decided; close package D13,
  design review finding 5.
- **SCORECARD-5.2** IF that gc receipt's totals `unit_files_deleted` and `objects_deleted` are
  both 0 THEN THE SYSTEM SHALL count the sample as vacuous, not as 0 rebuilt units. Both totals
  are recorded beside each sample (crates/cargo-hive/src/cli.rs:939-946).
- **SCORECARD-5.3** THE SYSTEM SHALL show R8's value per arm as "R rebuilt over k non-vacuous
  samples; m vacuous". When every sample is vacuous, the cell shows "all vacuous", which is a
  legal value a later run replaces in place.

## 6. The undeclared-input probe reports stale builds

**Story:** As the owner, I want a probe that changes a file a proc macro reads without
declaring it, so that I can see whether the rebuild came back stale.

- **SCORECARD-6.1** WHEN a pipeline worktree's R8 rerun has finished THE SYSTEM SHALL change one
  input that a proc macro in the measured repo reads without declaring it. The input is a
  `.sqlx/*.json` query file, or `.env` where there is none, and the change alters what the macro
  would emit. The system then reruns the same `cargo` command line (agent-decided; close package
  D14).
- **SCORECARD-6.2** IF that rerun exits 0 and Cargo reports the crate whose macro reads the
  changed file as `Fresh` (with `-v`) THEN THE SYSTEM SHALL count the sample as stale.
  - A rerun that recompiles that crate is not stale, whatever its exit code.
  - A rerun that exits non-zero without recompiling it is void.
  - A control rebuild just before the probe change must exit 0 with no rustc invocation, or
  the sample is void.

  Design review finding 2; agent-decided; ledger A23.
- **SCORECARD-6.3** THE SYSTEM SHALL show R9's value per arm as "s stale of n", and the steps
  SHALL restore the changed file after each sample.
- **SCORECARD-6.4** THE SYSTEM SHALL state beside R9 that seed clones registry units and not
  workspace crates (`crates/cargo-hive/src/cli.rs:68`; `crates/seed/src/lib.rs:1`; #16
  Problem). The probe therefore measures Cargo's own input tracking in both arms and does not
  single out staleness hive caused.

## 7. The setup footprint is listed

**Story:** As the owner, I want to read in one row everything a user installs or changes before
the numbers apply, so that I can weigh the setup against the savings.

- **SCORECARD-7.1** THE SYSTEM SHALL list in R10's seeded cell:
  - each command a user runs before the seeded numbers apply;
  - the count of Cargo config files, environment variables, rustc wrappers and cargo shims hive
    installs;
  - the count of files hive writes outside its state root.

  R10's cold cell reads "nothing". The default is from README.md install and uninstall and the
  vision Hard constraints; close package D15.

## 8. A rerun replaces the numbers in place

**Story:** As the owner, I want to rerun the scorecard by following its steps after a later
feature ships, so that the new numbers replace the old ones in the same rows.

- **SCORECARD-8.1** THE SYSTEM SHALL open the steps with `cargo hive bench --help` and a check
  that every flag the steps quote appears in its output (Weigh W1).
- **SCORECARD-8.2** IF a flag the steps quote no longer appears in `cargo hive bench --help` THEN
  the steps SHALL say to correct that step in the same commit as the rerun's numbers (Weigh W1).
- **SCORECARD-8.3** THE SYSTEM SHALL quote, in each step that fills a row, the exact command line
  it runs. A placeholder (`$MAIN`, `$P`, `$FILE`, `$WT`) stands for each path or name of the
  measured repo or machine (agent-decided; close package D6).
- **SCORECARD-8.4** WHEN a run records its numbers THE SYSTEM SHALL overwrite the value,
  sample-count, median and metadata cells of the existing rows, and SHALL NOT add a row.
- **SCORECARD-8.5** WHEN a run records its numbers THE SYSTEM SHALL leave every target slot and
  Target set date as it was.
- **SCORECARD-8.6** THE SYSTEM SHALL keep each run's receipts and by-hand logs under
  `.skills/verify/<date>-scorecard/`, outside git, and cite that path in each measured row's
  evidence cell (close package D17; docs/agents/verify.md "Evidence"). The scorecard says the
  evidence is local to the measuring machine: git ignores the directory, no clone can open it, and
  the committed values are copies of that run's `summary.txt`.
- **SCORECARD-8.7** WHEN the steps make worktrees for a run THE SYSTEM SHALL have them removed
  with `git worktree remove` at the end of the run, and on an aborted run as well, followed by
  `git worktree prune`. WHEN the run is interrupted (Ctrl-C) THE SYSTEM SHALL do the same cleanup
  and then stop the run: the
  interrupt trap sets `SCORECARD_INTERRUPTED`, `bench_row` and `pipeline` return exit status 130
  and refuse to start while it is set, and the doc's loops break. `preflight` clears the flag.
  The exit and interrupt traps are separate, and the interrupt trap does not exit an interactive
  shell. A caller that ends a call with `|| true` hides the 130 and must test the flag (`intr`).
  Known gap: an interrupt that lands during `preflight`'s `doctor` call makes `preflight` return 0
  with the flag set and `PREFLIGHT_OK` unset; later steps still refuse (O3 below).
- **SCORECARD-8.8** IF a row has fewer than 3 valid samples in an arm after a run THEN THE SYSTEM
  SHALL keep that row's previous values and metadata, and the run's evidence SHALL say which
  samples were void and why (agent-decided; ledger A23).
- **SCORECARD-8.9** IF `P` is empty or is not letters, digits, `_` and `-` starting with a letter
  or digit, or `FILE` is empty, or `FILE`, `PROBE` or an `UNTRACKED_COPY` entry is absolute or
  has a `..` segment, or `bench_row` is given a row other than R1 to R5, THEN THE SYSTEM SHALL
  refuse before any measurement, and `bench_row` and `pipeline` SHALL refuse to run unless the
  shell's last `preflight` passed (`PREFLIGHT_OK`). Added at realign 2026-11-23; built in
  `check_inputs`, `rel_ok`, `safe_rm` and the `PREFLIGHT_OK` guard (commits 97181bd, 94f9628).
- **SCORECARD-8.10** WHEN `summarize` runs THE SYSTEM SHALL write the end load average to
  `load-end.txt`, and print a warning to stderr, not a failure, when the start load in
  `load-start.txt` is above half the logical core count or the file is missing. Added at realign
  2026-11-23; built in 97181bd.
- **SCORECARD-8.11** WHEN `preflight` finds hive's binary under a debug build directory
  (`target/debug/`) THE SYSTEM SHALL print one warning line to stderr and record the same line in
  `$RUN/hive.txt`, and SHALL continue; WHEN the path matches neither `target/debug/` nor
  `target/release/` it SHALL print and record a different one-line note, and continue. It SHALL
  also record the build profile (`debug`, `release` or `unknown`) in `$RUN/hive.txt`. Added
  2026-11-23 after O4; the cold arm is unaffected, since hive does no work in its timed window.

## 9. The first run replaces today's headline

**Story:** As the owner, I want the first run recorded with at least three samples per arm, so
that I can read R1 as a measured baseline rather than the two-sample headline.

- **SCORECARD-9.1** WHEN the first run is recorded THE SYSTEM SHALL fill R1–R9 from that run. R1
  holds the n = 4 medians in place of the 2026-11-11 headline (86.9 s cold against 73.6 s
  seeded, n = 2, no hive commit), which fails #16 Acceptance 3 (close package D18).
- **SCORECARD-9.2** (guard) WHEN the scorecard is written THE SYSTEM SHALL CONTINUE TO hold
  README.md's "Measured results" and the MILE assessments' numbers unchanged as dated history.
- **SCORECARD-9.3** IF the operator's switch is not set when a run is ready to record THEN THE
  SYSTEM SHALL keep the run's numbers in its evidence only, and commit nothing that names or
  measures the measured repo (agent-decided: the option that leaves the question fully open,
  charter fork order step 4; ledger A12). O2 was answered on 2026-11-22 (yes: numbers and the
  repo's name may be committed). The switch in the doc is the variable `SCORECARD_NUMBERS_PUBLIC`,
  unset by default: `record` writes nothing into the scorecard unless it is `yes`. The operator
  sets it in the local env file.

## 10. Quality attributes

**Section-kind:** nfr

**Story:** As a stakeholder, I want measurable quality targets for this feature, so that how-well is not left implicit.

- **Performance:** None. The scorecard measures performance but has none of its own. Its rows'
  targets are the operator's (O1), and `docs/product/metrics.md` is absent, so there is no
  standing target to cite.
- **Security:** **SCORECARD-10.1** THE SYSTEM SHALL keep out of `docs/scorecard.md` every
  absolute path, user name and host name of the measuring machine. Verification: a `grep` of the
  file for `/Users/`, `/private/`, `$HOME` and the host name finds nothing. No threat model exists
  (`docs/security/` is absent), so this criterion carries no TB or THR ID.
- **Reliability:** None. A doc has no availability or recovery surface. Rerun fidelity is
  story 8.
- **Accessibility:** None. The scorecard is a markdown file read in a terminal or on GitHub, with
  no UI.

## 11. Guarded files

These are the files the change touches:
- `docs/scorecard.md`: new. No behavior to guard.
- `docs/specs/2026-11-22-worktree-scorecard/`: new. No behavior to guard.
- `docs/specs/catalog/hive.md`: one row added.
- `CONTEXT.md`: glossary terms added.

The guards:

- **SCORECARD-11.1** (guard) WHEN the SCORECARD card is added to `docs/specs/catalog/hive.md` THE
  SYSTEM SHALL CONTINUE TO keep every other row of that shard unchanged, with one CODE bound to
  ROAD-19.
- **SCORECARD-11.2** (guard) WHEN terms are added to `CONTEXT.md` THE SYSTEM SHALL CONTINUE TO
  keep every existing term and its definition unchanged.
- **SCORECARD-11.3** (guard) WHEN the scorecard lands THE SYSTEM SHALL CONTINUE TO pass
  `scripts/gate.sh`, with no file under `crates/`, `plugin/` or `scripts/` changed. That also
  leaves `cargo hive bench`'s flags, refusal rules and receipt keys as they ship (ROAD-19.md Must
  keep).

## Out of Scope

- Changing `cargo hive bench`, or any hive command, flag, rule, receipt key or state file.
- Linux, and CI runs of the scorecard.
- Choosing any target value (O1), or committing the measured repo's paths, user names or host
  names (O2 answered 2026-11-22: its name and numbers are allowed, paths are not).
- Fixing any number the scorecard shows.
- Aging a target, or faking invocation times, to make R8 non-vacuous (agent-decided, from
  ROAD-19's "measures only": aging fakes the invocation times hive reads. Reopen if R8 stays
  all vacuous after the first run; ledger S1, A13).
- A test, lint or script that checks the scorecard's shape or drives a run (Weigh W1; reopen
  when someone other than the owner reruns it, or when a rerun commit adds a row).
- Committing receipts or by-hand logs to git.
- Features the scorecard will later judge: ROAD-17 warm-slot-pool and issue #20.

## Open Questions

- O1 Target value of each row, and whether a row takes one target or one per arm. Owner:
  operator. Date: 2026-11-22. Forbid-guess: no session or agent writes a target value.
  **Answered 2026-11-22 on ticket #16, in part:** targets stay `open — operator` until the first
  measured run is recorded; after that the owner sets them, and a rerun never changes them. The
  first run was recorded 2026-11-23, so the owner's targets were still to come then.
  **Answered 2026-11-23 on a plan the operator agreed to:** only goal-backed targets are set now.
  R8 takes `0 rebuilt` in both slots, R9 seeded `no more stale than cold`, and R10 seeded
  `nothing installed or changed`, each dated 2026-11-23; R9 cold and R10 cold stay open. The time
  rows R1 to R6 and R7 stay open, for the reasons now in docs/scorecard.md `## Targets`. Their
  owner sets them later, and a rerun never changes any target.
- O2 Whether the scorecard may name the measured workspace and hold its numbers and paths in
  this public repo. Owner: operator. Date: 2026-11-22. Forbid-guess: until it is
  answered, SCORECARD-9.3 holds. **Answered 2026-11-22 on ticket #16:** the scorecard may hold
  the measured numbers and may name the measured repo, as other files in this repo already do.
  Paths, user names and host names stay out of it. SCORECARD-9.3 now keys on
  `SCORECARD_NUMBERS_PUBLIC`.
- O3 Known gap, found by dry run 3 (2026-11-23), kept open: an interrupt during `preflight`'s
  `doctor` call returns 0 with `SCORECARD_INTERRUPTED` set and `PREFLIGHT_OK` unset. Later
  `bench_row` and `pipeline` calls still refuse, so no wrong number can be measured, but the
  exit status is misleading, because `preflight` has no wrapper like the other two. `summarize`
  and `record` are not interrupt-aware either. Owner: operator (fix, or accept).
- O4 Closed 2026-11-23: the first run used a debug build of hive, so the seeded values of R1 to
  R5 and R7 carried a debug-built seed. The operator chose to record and warn, not stop:
  `preflight` prints and records a one-line warning for a debug build and continues
  (SCORECARD-8.11). The Latest run paragraph still names the build the numbers came from.
