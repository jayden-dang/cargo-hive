# Requirements: Plan-tier GC (stored plans of dead-unit files and orphan targets, applied after a recheck)

Feature code: GCPLAN
Status: Shipped
Date: 2026-11-01

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

Roadmap item: ROAD-6 (MILE-2, `docs/roadmap/INDEX.md`), tracker issue #3.

Sources:
- `.skills/GCPLAN/close-package.md` (confirmed by the owner 2026-11-01), `.skills/GCPLAN/scan.md`.
- Owner amendment 2026-11-02: plans of every class apply with no human step (ADR-0002).
- `.skills/research/2026-11-01-cargo-build-locks.md`: which locks Cargo holds during a build.
- Locks this feature builds on: ARCH-4, 8, 9, 11, 13, 14, 16, 17, 18, 19, 20; F9 (ownership resolved fresh);
  the liveness verdicts of LIVE (`docs/specs/2026-10-30-closure-liveness/`); the freed-byte rules of AUTOGC-5.

Terms (defined in `CONTEXT.md`): plan tier, protected checkout, secondary checkout, orphan candidate, dead unit,
Undecided, receipt. This file adds three:
- **Plan:** a one-use record hive stores, naming the paths one `apply` may delete in the targets of one class.
- **Class:** `secondary`, `protected` or `orphan`; the class of a target is the class of its owner checkout,
  `protected` when a protected checkout also owns it, and `orphan` when no checkout owns it.
- **Outcome:** the typed result of one `apply`: `applied`, `partial` or `refused`.

Slices: the closure slice (stories 1 to 4, 6, 7) comes first; the orphan slice (story 5) follows and shares the plan
store, ids, revalidation, receipt and skill with it.

## 1. Preview what plan-tier GC would delete, as plans

**Story:** As a developer or agent, I want `cargo hive gc --plan` to store plans of the dead-unit files in the targets of the repos I name, so that I see what can be reclaimed before anything is deleted.

- **GCPLAN-1.1** WHEN `cargo hive gc --plan` runs THE SYSTEM SHALL create, modify and delete no file in any checkout, any Cargo target dir or `~/.cargo`, writing only to hive's own state.
- **GCPLAN-1.2** WHEN a Cargo target dir in scope is `Decided` and `liveness()` lists at least one path to delete in it THE SYSTEM SHALL include it in a plan, listing each such path as `liveness()` returns it.
- **GCPLAN-1.3** THE SYSTEM SHALL write one plan per class that has at least one target, give each plan its own id, and put each target in exactly one plan.
- **GCPLAN-1.4** WHEN a target is owned by both a protected checkout and a secondary checkout THE SYSTEM SHALL put it in the `protected` plan.
- **GCPLAN-1.5** WHEN a target is `Undecided` THE SYSTEM SHALL put it in no plan and list it in the receipt's `undecided` group with the target path and, for each reason, what the reason is and its `clears_when`.
- **GCPLAN-1.6** WHEN a target is `Decided` and `liveness()` lists no path to delete in it, including when a dead unit owns no artifact, THE SYSTEM SHALL put it in no plan and list it in the receipt's `nothing_to_do` group.
- **GCPLAN-1.7** THE SYSTEM SHALL record in each plan its id, its class, the effective GC window, the registered repos it was made for, and for each target its path and owner checkout, and for each path to delete its entry kind and its device and inode numbers.
- **GCPLAN-1.8** THE SYSTEM SHALL use as the effective GC window the value in hive's configuration, and 7 days when none is set.
- **GCPLAN-1.9** IF the configured GC window is not a positive whole number of days, or is above the largest value the design allows THEN THE SYSTEM SHALL create no plan and report the refusal with exit code 1.
- **GCPLAN-1.10** THE SYSTEM SHALL offer no command-line option that sets the GC window for one run.
- **GCPLAN-1.11** WHEN `gc --plan` finishes THE SYSTEM SHALL report for each plan its id, class, targets, the number of paths, and the bytes it would free and the bytes still held elsewhere, measured without deleting by the rules of AUTOGC-5.1, AUTOGC-5.2 and AUTOGC-5.5.
- **GCPLAN-1.12** IF the eviction-accounting capability has not passed for the active toolchain THEN THE SYSTEM SHALL still create the plans, omit the byte values of GCPLAN-1.11, and record each omission in `unmeasured` with the reason.
- **GCPLAN-1.13** IF the fingerprint-reading capability has not passed THEN THE SYSTEM SHALL create no plan and report the refusal with exit code 1. `gc --plan` needs no other capability, since it deletes nothing.
- **GCPLAN-1.14** IF a plan cannot be written THEN THE SYSTEM SHALL leave no partial plan visible to `apply`, report the failure, and exit with code 1.

## 2. Apply a secondary plan

**Story:** As an agent, I want `cargo hive apply <id>` to delete the dead-unit files of a secondary plan, so that I reclaim a worktree's build output without asking anyone.

- **GCPLAN-2.1** WHEN `apply` is given the id of a stored `secondary` plan THE SYSTEM SHALL resolve the owner and class of each target in it afresh, over every registered repo, without reading the snapshot cache.
- **GCPLAN-2.2** WHEN `apply` processes a target THE SYSTEM SHALL try, without waiting, an exclusive lock on each `.cargo-build-lock` and each `.cargo-lock` present in every profile and triple directory of the target, and on each `.cargo-artifact-lock` present where the plan lists a path under `examples/`.
- **GCPLAN-2.3** WHEN a lock named in GCPLAN-2.2 is held, cannot be opened, cannot be locked on its volume, or is missing from a profile directory that has a `.fingerprint` directory THEN THE SYSTEM SHALL skip the whole target, delete nothing in it, and count the skip under a name for its reason: held, cannot be opened, cannot be locked on its volume, or missing.
- **GCPLAN-2.4** WHEN `apply` has acquired the locks of a target THE SYSTEM SHALL check that each lock still names the same entry as the path it opened, and skip the target if one does not.
- **GCPLAN-2.5** THE SYSTEM SHALL re-read the fingerprints of every profile of a target and recompute its liveness, with the plan's window and the current time, only while every lock of GCPLAN-2.2 for that target is held.
- **GCPLAN-2.6** WHEN GCPLAN-2.1 to GCPLAN-2.5 and every check of story 3 pass for all targets of the plan THE SYSTEM SHALL delete each path the plan lists for it by its entry kind, without following symbolic links.
- **GCPLAN-2.7** THE SYSTEM SHALL delete a unit's fingerprint directory only together with the artifacts the plan lists for that unit.
- **GCPLAN-2.8** THE SYSTEM SHALL delete no path that the plan does not list.
- **GCPLAN-2.9** WHEN `apply` finishes with every target of the plan applied THE SYSTEM SHALL report outcome `applied`; otherwise, when a target was skipped, a deletion failed, or the run was cancelled, outcome `partial` with a reason, including when no target was applied.
- **GCPLAN-2.10** WHEN `apply` deletes files THE SYSTEM SHALL report, per target and in total, the paths deleted, `freed_bytes` and `deleted_bytes_still_shared` by the rules of AUTOGC-5.1 and AUTOGC-5.2, and the cross-check notes of AUTOGC-5.4.
- **GCPLAN-2.11** WHEN `apply` is given a `secondary` plan THE SYSTEM SHALL mark the plan consumed before it deletes the first path, so the same id is never applied twice.
- **GCPLAN-2.12** IF a profile or triple directory exists in a target when `apply` re-reads it that was not in the set it locked THEN THE SYSTEM SHALL skip the target and delete nothing in it.
- **GCPLAN-2.13** WHEN `apply` deletes the files of one unit THE SYSTEM SHALL delete its dep-info `.d` files first, then its other artifacts, then its fingerprint directory, or the fingerprint directory first when the plan lists no `.d` file for the unit, so that after any prefix of the deletions Cargo sees the unit as stale or as fully present.

## 3. Refuse a plan that is no longer true

**Story:** As an agent or developer, I want `apply` to refuse a plan whose world has changed, so that nothing live is deleted because a plan went stale.

- **GCPLAN-3.1** IF a path the plan lists is no longer in the dead set recomputed under GCPLAN-2.5 THEN THE SYSTEM SHALL refuse the whole plan, delete nothing, and report the target and the first such path.
- **GCPLAN-3.2** IF a target in the plan is `Undecided` when recomputed THEN THE SYSTEM SHALL refuse the whole plan, delete nothing, and report the target's reasons.
- **GCPLAN-3.3** IF a path the plan lists is missing, or has a different entry kind, device or inode number than the plan recorded THEN THE SYSTEM SHALL refuse the whole plan, delete nothing, and report the path.
- **GCPLAN-3.4** WHEN a unit not in the plan has become dead since the plan was made THE SYSTEM SHALL leave its files alone and not treat the plan as stale for that reason.
- **GCPLAN-3.5** IF the owner checkout of a target differs from the plan, or its class differs from the plan, or ownership cannot be resolved THEN THE SYSTEM SHALL refuse the whole plan, delete nothing, and report the reason as `owner_changed`, `class_changed` or `ownership_unresolved`.
- **GCPLAN-3.6** IF a profile's fingerprint files no longer match a known format signature THEN THE SYSTEM SHALL refuse the whole plan and delete nothing.
- **GCPLAN-3.7** IF the id names no stored plan, or names a consumed plan THEN THE SYSTEM SHALL refuse with a distinct reason for each of the two cases and delete nothing.
- **GCPLAN-3.8** THE SYSTEM SHALL complete every check of this story for all targets of the plan before it deletes any path of any target.
- **GCPLAN-3.9** WHEN `apply` refuses a plan after it marked the plan consumed THE SYSTEM SHALL keep the plan consumed.
- **GCPLAN-3.10** IF the fingerprint-reading capability or the deletion capability has not passed THEN THE SYSTEM SHALL refuse every plan, delete nothing, leave the plan unconsumed, and exit with code 1.
- **GCPLAN-3.11** (fix, 2026-11-09) WHEN `apply` walks a secondary root for checkouts and meets a directory that hive's user cannot enter THE SYSTEM SHALL report it as `walk.unreadable` and go on, for unit plans and orphan removals alike, since no build that user runs can work inside it.
- **GCPLAN-3.12** (guard, 2026-11-09) IF any other fault occurs while `apply` walks for checkouts or targets, a directory that can be entered but not listed and an unreadable directory inside a checkout included, THEN THE SYSTEM SHALL CONTINUE TO refuse the whole plan with reason `walk.unreadable` and delete nothing.

## 4. Apply protected plans

**Story:** As an agent, I want `apply` to delete a protected plan itself, so that main's dead units are reclaimed with no human in the loop.

- ~~**GCPLAN-4.1**~~ superseded by GCPLAN-4.6 (owner amendment 2026-11-02: plans apply with no human step, ADR-0002) — was: ~~WHEN `apply` is given a plan of class `protected` or `orphan` THE SYSTEM SHALL delete nothing, return outcome `needs-human`, and report the plan's id, class, targets and planned bytes so the owner can see what would be approved.~~
- **GCPLAN-4.6** WHEN `apply` is given a plan of class `protected` THE SYSTEM SHALL delete its paths by the same locks, checks, deletion and receipt as story 2 and story 3, with no owner step.
- **GCPLAN-4.2** THE SYSTEM SHALL NOT open an owner-authentication dialog from `apply`.
- ~~**GCPLAN-4.3**~~ retired, no `needs-human` outcome exists (owner amendment 2026-11-02: plans apply with no human step, ADR-0002) — was: ~~WHEN `apply` returns `needs-human` THE SYSTEM SHALL leave the plan unconsumed.~~
- ~~**GCPLAN-4.4**~~ retired, `apply` deletes for these classes without an argument or environment variable (owner amendment 2026-11-02: plans apply with no human step, ADR-0002) — was: ~~THE SYSTEM SHALL give `apply` no argument or environment variable that makes it delete for a `protected` or `orphan` plan.~~
- ~~**GCPLAN-4.5**~~ superseded by GCPLAN-4.6 (owner amendment 2026-11-02: plans apply with no human step, ADR-0002) — was: ~~THE SYSTEM SHALL provide an internal entry point that deletes the paths of a `protected` plan by the same locks, checks, deletion and receipt as story 2 and story 3, and that runs only when given an approval value that no command-line path can construct; ROAD-7 supplies that value after the owner approves.~~

## 5. Plan orphan target dirs

**Story:** As a developer, I want `cargo hive gc --plan` to propose removing a whole target dir that no checkout owns, so that an agent can reclaim the space of a deleted repo.

- **GCPLAN-5.1** WHEN a Cargo target dir in scope is an orphan candidate THE SYSTEM SHALL include it in the `orphan` plan as a single whole-directory removal.
- **GCPLAN-5.2** THE SYSTEM SHALL list in an `orphan` plan only the target dir itself, and no path inside it.
- **GCPLAN-5.3** THE SYSTEM SHALL decide that a target dir is an orphan candidate by the rule of CORE-4.5, using ownership resolved afresh over every registered repo whatever repos the command names, and so plan an orphan only when every queried workspace resolved.
- **GCPLAN-5.4** THE SYSTEM SHALL record in an `orphan` plan the target dir's entry kind, device and inode numbers, and the profile and triple directories it holds.
- **GCPLAN-5.5** WHEN an orphan candidate would be owned under the caller's `CARGO_TARGET_DIR`, `CARGO_BUILD_TARGET_DIR` or `CARGO_BUILD_BUILD_DIR` THE SYSTEM SHALL say so against that target in the receipt.
- **GCPLAN-5.6** THE SYSTEM SHALL plan the removal of a whole Cargo target dir only in an `orphan` plan, and never in a `secondary` or `protected` plan (ARCH-18).
- ~~**GCPLAN-5.7**~~ superseded by GCPLAN-5.11 (owner amendment 2026-11-02: plans apply with no human step, ADR-0002) — was: ~~THE SYSTEM SHALL provide an internal entry point that removes the target dir of an `orphan` plan, and that runs only when given the approval value of GCPLAN-4.5.~~
- **GCPLAN-5.11** WHEN `apply` is given a plan of class `orphan` THE SYSTEM SHALL remove each target dir in it by the rules of GCPLAN-5.8 to GCPLAN-5.10, with no owner step.
- **GCPLAN-5.8** WHEN the orphan removal runs THE SYSTEM SHALL try, without waiting, an exclusive lock on each lock file present in every profile and triple directory of the target dir, and skip the removal, deleting nothing, when one is held, cannot be opened, cannot be locked on its volume, or is missing from a profile directory that has a `.fingerprint` directory.
- **GCPLAN-5.9** WHEN the orphan removal holds its locks THE SYSTEM SHALL list the target dir's profile and triple directories again, and IF one is not in the plan, or the target dir's entry kind, device or inode number differs from the plan, or ownership resolved afresh no longer marks it an orphan candidate THEN THE SYSTEM SHALL refuse the removal and delete nothing.
- **GCPLAN-5.10** WHEN the orphan removal proceeds THE SYSTEM SHALL remove the target dir by its entry kind without following symbolic links, and report the bytes freed and held elsewhere by the rules of AUTOGC-5.1, AUTOGC-5.2 and AUTOGC-5.5.

## 6. Read the outcome from the receipt and the exit code

**Story:** As an agent, I want each `gc --plan` and `apply` receipt to say per plan what happened, so that I know whether to continue or plan again without reading prose.

- **GCPLAN-6.1** WHEN `gc --plan` or `apply` runs with `--json` THE SYSTEM SHALL print one receipt object whose schema version is 3 and which holds a list of plans, each with its id, class, outcome (for `apply`), reason, and the counters that apply to it.
- **GCPLAN-6.2** WHEN `apply` returns outcome `applied`, or `partial` with no deletion failure and no cancel, THE SYSTEM SHALL exit with code 0.
- ~~**GCPLAN-6.3**~~ retired, no `needs-human` outcome exists (owner amendment 2026-11-02: plans apply with no human step, ADR-0002) — was: ~~WHEN `apply` returns outcome `needs-human` THE SYSTEM SHALL exit with code 3.~~
- **GCPLAN-6.4** WHEN `apply` returns outcome `refused`, or fails, or returns `partial` because a deletion failed, THE SYSTEM SHALL exit with code 1.
- **GCPLAN-6.5** WHEN `gc --plan` creates plans, or finds nothing to plan, THE SYSTEM SHALL exit with code 0.
- **GCPLAN-6.6** THE SYSTEM SHALL take exactly one plan id per `apply`.
- **GCPLAN-6.7** THE SYSTEM SHALL count each skipped target in the receipt by the reason of GCPLAN-2.3, under count names that do not reuse a name AUTOGC-5.3 defines for sessions or profile directories, and keep those skips out of the `refused` outcome.
- **GCPLAN-6.8** WHEN `apply` is cancelled after it consumed the plan THE SYSTEM SHALL stop between units, report outcome `partial` with reason `cancelled`, keep the plan consumed, and exit with code 130.

## 7. Teach agents the plan tier

**Story:** As an agent, I want the hive skill to teach `gc --plan` and `apply`, so that I follow the plan flow for every class.

- **GCPLAN-7.1** THE SYSTEM SHALL document `gc --plan` and `apply` in the hive skill, each as a command line with `--json` that parses with the real CLI.
- ~~**GCPLAN-7.2**~~ superseded by GCPLAN-7.6 (owner amendment 2026-11-02: plans apply with no human step, ADR-0002) — was: ~~THE SYSTEM SHALL state in the skill that exit code 3 and outcome `needs-human` mean the owner must approve, and that an agent applies only `secondary` plans and reports the rest.~~
- **GCPLAN-7.6** THE SYSTEM SHALL state in the skill that an agent applies plans of every class itself, with no owner step, and that an `orphan` plan removes the whole target dir, with anything in it no build recreates.
- **GCPLAN-7.3** THE SYSTEM SHALL state in the skill that an agent plans again after a `refused` outcome and never reuses a consumed plan id.
- **GCPLAN-7.4** THE SYSTEM SHALL carry recorded-run entries for `gc --plan` and `apply` in the skill's eval file, in the form the existing entries use, each derived from a heading in `skills/hive/TESTS.md`.
- **GCPLAN-7.5** THE SYSTEM SHALL replace the skill's statement that the plan tier is not available yet with the plan flow of GCPLAN-7.1 to GCPLAN-7.3.

## 8. Quality attributes

**Section-kind:** nfr

**Story:** As a stakeholder, I want measurable quality targets for this feature, so that how-well is not left implicit.

- **Performance:** None — no standing latency target for plan or apply; hive reports only deterministic counters (ARCH-10), and time is measured only by `cargo hive bench`.
- **Security:** **GCPLAN-8.1** THE SYSTEM SHALL treat every path in a stored plan as untrusted input, and delete a path only when its canonical parent lies inside the plan's target dir — verified by a test at the path guard that offers a path outside the target dir and a path reached through a symbolic link and finds both refused, and by a test that plants such paths in a stored plan and finds `apply` refuse it.
- **Reliability:** **GCPLAN-8.2** WHEN `apply` deletes the dead-unit files of a copy of a real target THE SYSTEM SHALL cause no rebuild of any unit that was live when `apply` decided — verified by a deletion trial built on `tests/common/prove.rs` that deletes through `apply` instead of the trial's own deletion.
- **Reliability:** **GCPLAN-8.3** WHEN `apply` is interrupted after it marked the plan consumed THE SYSTEM SHALL leave the plan unusable for a second `apply`, delete no path outside the plan, and let a new `gc --plan` plan the rest — verified by a test that cancels `apply` between two deletions.
- **Reliability:** **GCPLAN-8.4** THE SYSTEM SHALL write, mark consumed and remove plans only through the state module, atomically — verified by a test that fails the write and finds no partial plan.
- **Reliability:** **GCPLAN-8.5** WHEN `apply` is stopped after any one deletion of a unit THE SYSTEM SHALL leave a target on which `cargo build` succeeds — verified by a test that stops after each prefix of one unit's deletions on a scratch crate and then runs `cargo build`.
- **Accessibility:** None — headless CLI.

## 9. Existing behavior this feature must not change

Files and surfaces this change touches:
- `src/cli.rs`: `gc` gains `--plan` and a new `apply` command is added; `status`, `repo`, `doctor`, `explain`, the hidden `__hold-lock`, `emit` and the exit codes exist. Guarded below.
- `src/receipt.rs`: the receipt schema (2 today) gains a version 3 shape for plan and apply. Guarded below.
- `src/state.rs`: gains storage for plans and a configuration key for the window; existing files and keys are at risk. Guarded below.
- `src/liveness.rs`: called by plan and apply, not changed. Guarded below.
- `src/gc/run.rs`, `src/gc/ledger.rs`: byte accounting and no-follow measuring may be reused or shared; the auto tier is at risk. Guarded below.
- `src/discover.rs`, `src/cargo_target.rs`: ownership must be resolvable afresh without the cache. `status` is at risk. Guarded below.
- `src/platform/mod.rs`, `src/platform/apfs.rs`: lock and removal methods may grow. Guarded below.
- `plugin/skills/hive/SKILL.md`, `skills/hive/eval.json`, `tests/skill.rs`: documented commands and tests. Guarded below.
- `CONTEXT.md`, `docs/specs/2026-10-30-closure-liveness/design.md`: documentation only; no behavior to guard.

- **GCPLAN-9.1** (guard) WHEN `cargo hive gc` runs without `--plan` THE SYSTEM SHALL CONTINUE TO behave as stories 2 to 5 of AUTOGC require, with the same counts, findings, exit codes and receipt schema version. (2026-11-09: STOREGC adds store counts to bare `gc`; target-deletion counts are unchanged, STOREGC-10.1.)
- **GCPLAN-9.2** (guard) WHEN `cargo hive repo add`, `status`, `doctor` or `explain` runs THE SYSTEM SHALL CONTINUE TO print the receipt it printed before this feature, with schema version 2, the same count names and values, and leave every checkout, every Cargo target dir and `~/.cargo` with no file created, modified or deleted.
- **GCPLAN-9.3** (guard) WHEN any hive command runs once hive's state is open, `gc --plan` and `apply` included, THE SYSTEM SHALL CONTINUE TO satisfy CORE-5.1 to CORE-5.7: one JSON receipt on stdout under `--json`, the receipt appended to the receipt log, only counted or measured values, no time saved, unmeasurable values omitted with a reason in `unmeasured`, version, scope and schema in every receipt, scope from arguments and never from the working directory.
- ~~**GCPLAN-9.4**~~ superseded by GCPLAN-9.11 (owner amendment 2026-11-02: plans apply with no human step, ADR-0002) — was: ~~(guard) WHEN a command other than `apply` exits THE SYSTEM SHALL CONTINUE TO use exit code 0 for success, 1 for a refusal or failure, 2 for an unsupported OS and for argument errors, and 130 for a cancel; code 3 is used only by `apply` for `needs-human`.~~
- **GCPLAN-9.11** (guard) WHEN any command exits THE SYSTEM SHALL CONTINUE TO use exit code 0 for success, 1 for a refusal or failure, 2 for an unsupported OS and for argument errors, and 130 for a cancel, and SHALL use no other code.
- **GCPLAN-9.5** (guard) WHEN liveness is computed THE SYSTEM SHALL CONTINUE TO return the verdicts of LIVE stories 1 and 2 for the same target, window and time; plan and apply call it and do not change it.
- **GCPLAN-9.6** (guard) WHEN hive computes ownership THE SYSTEM SHALL CONTINUE TO run no cargo subcommand but `cargo metadata --no-deps --format-version 1 --locked --offline` with the scrubbed environment, and to compile and replay nothing.
- **GCPLAN-9.7** (guard) WHEN hive reads its configuration THE SYSTEM SHALL CONTINUE TO read `secondary_roots` and `cache_roots` as before, and keep refusing unknown keys other than the window key this feature adds.
- **GCPLAN-9.8** (guard) WHEN `cargo hive status` runs THE SYSTEM SHALL CONTINUE TO read ownership through the snapshot cache as it does today.
- **GCPLAN-9.9** (guard) WHEN the skill tests run THE SYSTEM SHALL CONTINUE TO require that the skill body stays within its line budget, that every command line in it parses with the real CLI, and that every shipped command is documented with `--json`.
- **GCPLAN-9.10** (guard) WHEN `apply` or `gc --plan` meets a Cargo target dir with a build-script, run-build-script, doc or other-kind unit, `build/`, or an `incremental/` directory THE SYSTEM SHALL CONTINUE TO leave them alone, as LIVE does.

## Out of Scope

- `cargo hive approve`, an owner-authentication dialog and approval records: dropped with ROAD-7 on 2026-11-02 (ADR-0002).
- Seeding, the store, store GC and `bench`: MILE-3.
- A full rewrite of the hive skill: ROAD-13. This feature only adds `gc --plan` and `apply`.
- A per-run window option, and a per-repo window.
- Replaying or compiling anything to decide liveness or ownership (ARCH-17).
- Deleting `build/`, `incremental/`, never-collected unit kinds, or a lone fingerprint directory (ARCH-18).
- Cargo target dirs whose build directory is split from the target directory: they stay `Undecided` as in LIVE.
- The new nightly build-dir layout and fine-grained locking (ADR-0001).
- Reusing a consumed plan, resuming a plan, and editing a stored plan.
- A claim that protected checkouts are safe from an agent that edits hive's state files (vision non-goal).

## Open Questions

None. The treatment of an orphan candidate owned under the caller's environment (planned, with the receipt note of GCPLAN-5.5) was confirmed by the project owner on 2026-11-01.
