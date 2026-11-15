# Requirements: Checkout GC

Feature code: CHECKOUTGC
Status: Draft
Date: 2026-11-15
Roadmap item: ROAD-16 (issue #29)

<!--
Rules:
- Every acceptance criterion gets a hierarchical ID: CHECKOUTGC-<story>.<criterion>.
- EARS phrasing; guards use SHALL CONTINUE TO.
- IDs are immutable once Status is Approved. Retire by striking through; never renumber.
- A criterion marked "(agent-decided)" states a value the operator delegated or did not
  choose; the operator reads it on the spec sheet. It is not the operator's own choice.
-->

Terms are those of `CONTEXT.md`: checkout, protected checkout, Cargo target dir, profile dir,
unit, dead unit, roots, closure liveness, Undecided, debug object, linked binary, receipt.
In this file:

- **The command** is `cargo hive gc --checkout <PATH>`, with or without `--json`.
- **The checkout** is the registered checkout `<PATH>` names.
- **Its targets** are the Cargo target dirs (a build directory included) that `cargo metadata`
  resolves for the checkout's workspaces and that no other live checkout of its repo resolves to.
- **A loose N_OSO entry** is a debug-map entry of a linked binary that names a file path. An
  entry of the form `<archive>(<member>)` names a member inside an archive and is not loose.

## 1. Delete one checkout's dead units in one command

**Story:** As an agent working in a reused worktree, I want to run one command on that worktree and have its dead units deleted then and there, so that its target keeps only the units live builds still use.

- **CHECKOUTGC-1.1** WHEN the command runs and a target's liveness is Decided THE SYSTEM SHALL delete, in that same run and with no stored plan, every entry `liveness()` lists for that target's dead units: their hash-keyed `deps/` and `examples/` entries and their fingerprint dirs.
- **CHECKOUTGC-1.2** THE SYSTEM SHALL decide dead units with the configured GC window, 7 days when none is set, and SHALL offer no option that sets the window for one run.
- **CHECKOUTGC-1.3** IF the configured GC window is outside 1 to 365 days THEN THE SYSTEM SHALL refuse with exit code 1 and delete nothing, as `gc --plan` and `apply` do.
- **CHECKOUTGC-1.4** (agent-decided: the window-bounded reading of "no unit the last task used rebuilds") WHEN a cargo command that compiled, inside the GC window, a unit of the checkout runs again after the command THE SYSTEM SHALL have deleted no file of any unit that is live under closure liveness, so that cargo recompiles none of them because of the gc.
- **CHECKOUTGC-1.5** IF a target's liveness is Undecided THEN THE SYSTEM SHALL delete no unit file in that target, and SHALL report one finding per reason naming the target, with the reason and its clears-when text in the finding's detail (agent-decided: the finding kind is fixed in design).
- **CHECKOUTGC-1.6** THE SYSTEM SHALL delete a dead unit's fingerprint dir only in the same run as that unit's hash-keyed files, and SHALL delete nothing under a profile dir's `build/`.
- **CHECKOUTGC-1.7** WHEN the command finishes THE SYSTEM SHALL print and record one receipt with schema 2, command `gc --checkout`, scope `repos` holding the checkout's repo and scope `checkouts` holding only the checkout. Its counts, per target as `<name>:<target path>` keys and in total, cover units deleted, unit files deleted, freed bytes and deleted bytes a surviving link or clone still holds, with no duration (agent-decided: the count names are fixed in design).
- **CHECKOUTGC-1.8** IF `--checkout` is given together with a repo argument, `--plan` or `--dry-run` THEN THE SYSTEM SHALL refuse it as an argument error with exit code 2, delete nothing, and write no receipt (agent-decided: no dry run in this mode).
- **CHECKOUTGC-1.9** IF the fingerprint-reading or deletion capability is not passed for the active toolchain THEN THE SYSTEM SHALL refuse with exit code 1 and delete nothing, as bare `gc` does.
- **CHECKOUTGC-1.10** IF the eviction-accounting capability is not passed THEN THE SYSTEM SHALL still delete, and SHALL report `freed_bytes` and `deleted_bytes_still_shared` as unmeasured, as bare `gc` does.
- **CHECKOUTGC-1.11** WHEN the command is cancelled THE SYSTEM SHALL start no further deletion, release every lock it took, print a receipt counting what it already deleted, record it unless the cancel arrives while it is being recorded, and exit with code 130.
- **CHECKOUTGC-1.12** WHEN the command finishes without a refusal, a cancel, a failed deletion or a receipt it could not record THE SYSTEM SHALL exit with code 0, whether or not a target was skipped. Each of those four exits with code 1, apart from the cancel's 130.

## 2. Delete the debug objects no linked binary references

**Story:** As an agent working in a reused worktree, I want the same command to delete the debug objects no linked binary in that worktree's target references, so that the next new test binary starts as fast as it would in a fresh target.

- **CHECKOUTGC-2.1** WHEN the command runs THE SYSTEM SHALL delete each `*.rcgu.o` file directly in a profile dir's `deps/` of its targets whose file name is the file name of no loose N_OSO entry of any linked binary in that profile dir. It SHALL delete no other file in this class.
- **CHECKOUTGC-2.2** (agent-decided) THE SYSTEM SHALL count as a linked binary of a profile dir every regular file whose header is a thin or fat Mach-O executable, dylib or bundle, whether or not it was relinked recently. It covers files in the profile dir itself, its `deps/`, its `examples/` and each `build/<dir>/`.
- **CHECKOUTGC-2.3** (agent-decided) THE SYSTEM SHALL match a loose N_OSO entry to a debug object by file name alone, whatever directory the entry names, so a target that was moved, cloned or seeded keeps every object any of its binaries names.
- **CHECKOUTGC-2.4** (agent-decided) THE SYSTEM SHALL treat an `<archive>(<member>)` N_OSO entry as naming no debug object.
- **CHECKOUTGC-2.5** (agent-decided) WHEN dead units of a target are deleted in the same run THE SYSTEM SHALL read that target's linked binaries only after those deletions.
- **CHECKOUTGC-2.6** (agent-decided) WHEN a target's liveness is Undecided THE SYSTEM SHALL still delete that target's unreferenced debug objects.
- **CHECKOUTGC-2.7** (agent-decided) IF a file in a profile dir has a Mach-O header but cannot be read or parsed, is larger than the read bound, or is a fat binary with a slice it cannot read THEN THE SYSTEM SHALL delete no debug object in that profile dir, and SHALL report a finding naming the file and the reason.
- **CHECKOUTGC-2.8** (agent-decided) IF a profile dir holds linked binaries and loose `*.rcgu.o` files, but no linked binary in it yields any N_OSO entry THEN THE SYSTEM SHALL delete no debug object in that profile dir, and SHALL report a finding naming the profile dir.
- **CHECKOUTGC-2.9** (agent-decided) IF a profile dir is not in the fingerprint layout, or listing its `deps/` or a directory it reads for linked binaries goes past the listing bound THEN THE SYSTEM SHALL delete no debug object in that profile dir, and SHALL report a finding naming it and the reason.
- **CHECKOUTGC-2.10** (agent-decided) THE SYSTEM SHALL read linked binaries without starting a child process.
- **CHECKOUTGC-2.11** WHEN the command finishes THE SYSTEM SHALL count, in the receipt and per target, the debug objects deleted, the debug objects kept because a linked binary names them, and the linked binaries read. Freed bytes are measured per inode as in story 1 (agent-decided: the count names are fixed in design).

## 3. Touch only the named checkout

**Story:** As the owner building in a repo's main checkout, I want an agent's checkout gc on its own worktree to leave my target and every other worktree's target untouched, so that my builds never pay for another checkout's cleanup.

- **CHECKOUTGC-3.1** WHEN the command runs THE SYSTEM SHALL resolve, with `cargo metadata`, the target and build directories of every live checkout of the checkout's repo, and SHALL act only on directories the checkout resolves to that no other of those checkouts resolves to.
- **CHECKOUTGC-3.2** IF a target the checkout resolves to is also resolved to by another checkout of the repo THEN THE SYSTEM SHALL delete nothing in that target, and SHALL report a finding naming the target and the other checkout.
- **CHECKOUTGC-3.3** IF `<PATH>` does not canonicalize to exactly a non-missing checkout of a registered repo (a directory inside a checkout is not one) THEN THE SYSTEM SHALL refuse with a finding naming the path, delete nothing, and exit with code 1. When no registered repo contains the path, the finding names `cargo hive repo add`.
- **CHECKOUTGC-3.4** WHEN `<PATH>` is a repo's registered main checkout THE SYSTEM SHALL act on the main checkout's own targets, under 3.1 and 3.2, and on no other checkout of that repo.
- **CHECKOUTGC-3.5** (agent-decided) IF `cargo metadata` fails for any workspace of any live checkout of the repo THEN THE SYSTEM SHALL delete nothing in any target, because a shared target cannot be ruled out, and SHALL report a finding naming each workspace dir whose metadata failed.
- **CHECKOUTGC-3.6** WHEN the command runs THE SYSTEM SHALL leave unchanged every file outside its targets, apart from the one receipt line it appends to today's receipts file and the discovery snapshot (`snapshot.json`) it refreshes as the other `gc` modes do, both written through `state`. This covers other checkouts' targets, the store, the registry, plans and every other state file.

## 4. Skip a target a build holds

**Story:** As an agent, I want the command to leave a target alone while a build holds it, and tell me so, so that a running build never loses a file under it.

- **CHECKOUTGC-4.1** WHEN the command reaches a target THE SYSTEM SHALL try, without waiting, every Cargo lock present in each of that target's profile dirs (`.cargo-build-lock`, `.cargo-lock` and `.cargo-artifact-lock`), before it reads liveness or a linked binary there.
- **CHECKOUTGC-4.2** IF any of a target's locks is held, cannot be opened or locked, is missing from a profile dir that has `.fingerprint/`, or the profile set changes while the locks are taken THEN THE SYSTEM SHALL delete nothing in that target. It SHALL report a `gc.skipped` finding naming the target and the reason, and SHALL count the target under the `targets_skipped_*` name `apply` uses for that reason (agent-decided: target-level names, as in `apply`).
- **CHECKOUTGC-4.3** IF reading a target's profile dirs fails while its locks are being taken THEN THE SYSTEM SHALL delete nothing in that target, report a finding naming it and the reason, and exit with code 1.
- **CHECKOUTGC-4.4** WHILE it reads liveness, reads linked binaries and deletes in a target THE SYSTEM SHALL hold every lock it took for that target, releasing them only after the last deletion there.

## 5. The hive skill says when to run it

**Story:** As an agent that reads the hive skill, I want it to tell me when to run checkout gc on my worktree and how to read its receipt, so that I run it before my test builds slow down without being asked.

- **CHECKOUTGC-5.1** (agent-decided) THE SYSTEM SHALL ship a hive skill that tells an agent working in a reused worktree to run `cargo hive gc --checkout <that worktree> --json` before a test build whenever that checkout's profile `deps/` holds more than 20,000 entries. It SHALL say how the agent counts those entries.
- **CHECKOUTGC-5.2** THE SYSTEM SHALL ship the README describing `gc --checkout` the same way, with every command line and receipt name it cites existing in the binary.
- **CHECKOUTGC-5.3** THE SYSTEM SHALL ship a hive skill that names this mode's counts and findings, held-target skips included, and that states that `gc <path>` still covers the whole repo.
- **CHECKOUTGC-5.4** (guard) WHEN `tests/skill.rs` runs THE SYSTEM SHALL CONTINUE TO pass these checks: that every `cargo hive` line in the skill parses with the CLI, that every shipped command is documented, and that the "Free disk" section keeps its pinned phrases. The new command line is among the lines checked, and `parse()` tells it apart from bare `gc`.

## 6. Existing commands keep working

**Story:** As a developer who already runs hive's repo-wide commands, I want `gc`, `gc --plan`, `apply` and the other commands to behave exactly as before, so that adding checkout gc costs me nothing I rely on.

- **CHECKOUTGC-6.1** (guard) WHEN `cargo hive gc`, `gc <path>`, `gc --dry-run`, `gc --plan` or `apply` runs THE SYSTEM SHALL CONTINUE TO satisfy SEED-10.1: the same deletions, and the same receipt with the same schema version, count names and exit codes. A path argument SHALL CONTINUE TO cover its whole repo, with the same note when the path is not the registered main.
- **CHECKOUTGC-6.2** (guard) WHEN `cargo hive repo add`, `status`, `doctor` or `explain` runs THE SYSTEM SHALL CONTINUE TO satisfy GCPLAN-9.2.
- **CHECKOUTGC-6.3** (guard) WHEN `liveness()` runs THE SYSTEM SHALL CONTINUE TO return the verdicts of LIVE stories 1 and 2, with its signature unchanged (SEED-10.4). The dead-unit class SHALL CONTINUE TO use `embedded_hashes` for a unit's files, `.rcgu.o` included (SEED-10.6). The debug-object class decides by debug-map reference, not by a unit's files, and leaves SEED-10.6 as it is.
- **CHECKOUTGC-6.4** (guard) WHEN `locks::acquire` is called THE SYSTEM SHALL CONTINUE TO return the same results for the same target, `Skipped(Missing)` included (SEED-10.8).
- **CHECKOUTGC-6.5** (guard) WHEN a seed record is counted THE SYSTEM SHALL CONTINUE TO leave `*.rcgu.o` files out of its counts, so that deleting a debug object changes no seed count.
- **CHECKOUTGC-6.6** (guard) WHEN the CLI command tree is dumped THE SYSTEM SHALL CONTINUE TO print, for every command other than `gc`, exactly the lines of the v0.1.0 fixture. The regenerated fixture SHALL differ from v0.1.0 only by the `--checkout` argument and the reworded `gc` help text.
- **CHECKOUTGC-6.7** (guard) WHEN the workspace is checked for `x86_64-unknown-linux-gnu` THE SYSTEM SHALL CONTINUE TO compile, and every hive command SHALL CONTINUE TO exit with code 2 on an unsupported OS.

Files this change touches, and what each must not break:

| File | Guard |
|---|---|
| `crates/cargo-hive/src/cli.rs` (`Gc` args, dispatch, receipt) | 6.1, 6.2, 6.6 |
| `crates/cargo-hive/tests/fixtures/v0.1.0/cli-tree.txt`, `crates/cargo-hive/tests/cli_tree.rs` | 6.6 |
| `crates/cargo-hive/tests/skill.rs` (a `Parsed` variant for the new mode) | 5.4 |
| `skills/hive/eval.json`, `skills/hive/TESTS.md` (skill evals, pinned by `skill.rs`) | 5.4 |
| `plugin/skills/hive/SKILL.md` | 5.4 |
| `crates/gc/src/` (new one-checkout path and debug-object class beside `plan_tier`) | 6.1, 6.3 |
| `crates/core/src/locks.rs` (reused) | 6.4 |
| `crates/cargo/src/discover.rs` (ownership reused) | 6.1 (`gc --plan` scope, owner resolution) |
| `crates/platform/src/` and its test doubles (bounded read for binaries, if design adds it) | 6.7; every existing `Fs` method keeps its behavior (covered by 6.1–6.4) |
| New Mach-O reader module (crate chosen in design) | no behavior to guard |
| `README.md` | 5.2; its command lines are parsed by `skill.rs` (5.4) |
| `CONTEXT.md` | documentation only; no behavior to guard |

## 7. Quality attributes

**Section-kind:** nfr

**Story:** As a stakeholder, I want measurable quality targets for this feature, so that how-well is not left implicit.

- **Performance:** **CHECKOUTGC-7.1** (agent-decided) WHEN the command runs on a copy of an aged klynt slot THE SYSTEM SHALL finish within 15 s of wall time. Verified by timing the proof run outside hive, since hive reports no durations (ARCH-10). The spike's object gc, using a child process per binary, took 7–8 s per build (MILE-5.notes.md:15-16). No standing metric doc exists (`docs/product/metrics.md` absent).
- **Security:** **CHECKOUTGC-7.2** THE SYSTEM SHALL delete only entries `liveness()` lists and loose `*.rcgu.o` files whose paths lie inside one of its targets. Each one SHALL be re-checked by kind, device and inode immediately before it is deleted, and SHALL be followed out of a target by no symlink. Verified by tests with a symlink and a swapped entry. No threat model exists (`docs/security/` absent).
- **Reliability:** **CHECKOUTGC-7.3** WHEN a run stops part-way through, cancelled or killed THE SYSTEM SHALL leave no state file to repair, and the next run SHALL complete the deletion with the same rules. Verified by a test that cancels mid-run and runs again. No reliability doc exists (`docs/ops/` absent).
- **Accessibility:** None. The command is a headless CLI with plain-text and `--json` output and no UI.

## Out of Scope

- A tighter or handoff-anchored window for dead units, and the fingerprint-read liveness signal of #20.
- **Known limit** (agent-decided reading of Must keep, CHECKOUTGC-1.4): a leaf unit (a test binary or bin) last compiled before the GC window, which no root reaches, is a dead unit even if the last task ran it fresh. It recompiles once. The repo-wide plan tier has the same limit, and the fix is #20.
- The auto tier (incremental sessions, rustc temp entries) and store eviction in this mode. They stay with bare and repo `gc`.
- A dry run, a stored plan or any preview for this mode.
- A persisted cache of what each binary references.
- Hooks, wrappers, background processes and Cargo config (ARCH-1). Hive running gc by itself.
- Deleting anything outside the checkout's own targets, a shared target included.
- Linux behavior. The command compiles there and refuses as every command does.
- The warm-slot pool and leases (ROAD-17, #18) and harness integration (ROAD-18, #19).
- Any change to repo-wide `gc`, `gc --plan` or `apply`.
- Packed or off split-debuginfo targets beyond reporting nothing to delete.

## Open Questions

- Example 1's figures (121k → 4.2k files in `deps/`; 60 s → 28 s for the next edit's test build) are the klynt proof's evidence target, not a requirement. Their source is not in the spike records. The proof records the measured values.
- Unmeasured: the share of an aged slot's slowdown that comes from superseded incremental sessions. Reopen the exclusion of the auto tier if, after the command, the next build stays slower than fresh by more than the deleted objects and units explain. This is an open check for the proof, not a requirement.
