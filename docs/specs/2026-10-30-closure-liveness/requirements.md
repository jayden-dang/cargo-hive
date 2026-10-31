# Requirements: Closure liveness (live and dead units of a Cargo target dir, proven on a copy)

Feature code: LIVE
Status: In-progress
Date: 2026-10-30

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

Roadmap item: ROAD-5 (MILE-2, `docs/roadmap/INDEX.md`).

Sources:
- `.skills/LIVE/close-package.md` (confirmed 2026-10-30), `.skills/LIVE/knowns.md`,
  `.skills/LIVE/scan.md`.
- `.skills/LIVE/req-review.md` (code-claim review, 2026-10-30).
- `.skills/LIVE/fresh-build-trace.md`: what fully fresh commands write, cross-profile edges under
  `--target`, artifact names per unit, and the D5 deletion premise (measured 2026-10-30).
- `SPIKES.md` §8, §9; `.skills/CORE/close-package.md` (A1, D3, D4, D5); ADR-0001.

Terms used as `CONTEXT.md` defines them: Cargo target dir, Profile dir, Unit, Roots, Closure
liveness, Dead unit, Undecided, Vacuous, Fingerprint format signature. Further terms used below:
- **Current unit:** a unit counted by CORE-4.7 in a supported profile dir.
- **Not-current unit:** a unit directory counted by CORE-4.16.
- **Unit hash:** the 16 lowercase hex characters after the last `-` of a unit's directory name
  under `.fingerprint/` (for example `4360aef667e9273e` in `dep-4360aef667e9273e`). It names the
  unit's files; the hash file's content is the unit's fingerprint, a different value.
- **Hash-keyed entry:** an entry of a profile dir's `deps/` or `examples/` whose name contains
  `-<unit hash>` followed by `.` or by the end of the name (for example
  `libdep-4360aef667e9273e.rlib`, `app-51eb62a7ca1398b8`, `app-51eb62a7ca1398b8.d`,
  `app-51eb62a7ca1398b8.<cgu>.rcgu.o`, `utoipa-92572aff23f22a19.utoipa.<hash>-cgu.02.rcgu.o`).
- **Never-collected kind:** a unit whose target kind (EXPLAIN terms) is `build-script`,
  `run-build-script`, `doc-lib`, `doc-bin` or `other`. Build-script units write to `build/`, doc
  units to `target/doc/`, and an `other` unit's outputs are unknown, so none has files a deletion
  could pair with its fingerprint dir.
- **Unit timestamp:** the `invoked.timestamp` file Cargo rewrites when it starts building a unit.
  For a `run-build-script` unit it is `<profile dir>/build/<unit directory name>/invoked.timestamp`
  (its fingerprint dir has none); for every other unit it is the one in the unit's directory under
  `.fingerprint/`. Reading it is the only access liveness makes to `build/`.
- **Window:** a duration; a unit is inside the window when its unit timestamp's modification time
  is at or after `now − window`, or later than `now`.
- **Liveness result:** for one Cargo target dir, either *Decided* (live units and dead units) or
  *Undecided* (one or more reasons).

## 1. Tell which units of a target dir are live

**Story:** As the owner, I want hive to compute, for one Cargo target dir, which units are live and
which are dead, so that a later plan can delete dead units without touching a unit reachable over
the fingerprint graph from a unit whose timestamp falls inside the window.

- **LIVE-1.1** WHEN liveness is computed for a Cargo target dir with a window and a current time
  THE SYSTEM SHALL treat as roots exactly the current units, in every profile dir of that target
  dir, that are inside the window.
- ~~**LIVE-1.2**~~ superseded by LIVE-1.11 and LIVE-2.12 (2026-10-31, whole-branch review): a
  timestamp that exists but cannot be read is doubt, not absence, and doubt must not let a recently
  built unit be listed as dead.
- **LIVE-1.11** IF the unit timestamp file of a current unit does not exist THEN THE SYSTEM SHALL
  not treat that unit as a root.
- **LIVE-1.3** THE SYSTEM SHALL mark live every current unit reachable from a root over fingerprint
  `deps` edges, each edge resolved by matching the dependency's u64, as little-endian hex, against
  the hash files of the current units of every profile dir of the same Cargo target dir.
- **LIVE-1.4** WHEN an edge matches the hash files of more than one current unit THE SYSTEM SHALL
  treat every matching unit as reached through that edge.
- **LIVE-1.5** WHEN the liveness result is Decided THE SYSTEM SHALL report as dead every current
  unit that is not live and is not of a never-collected kind.
- **LIVE-1.6** WHEN the caller gives no window THE SYSTEM SHALL use a window of 7 days.
- **LIVE-1.7** THE SYSTEM SHALL compute liveness without running `cargo`, `rustc` or any other
  build command.
- **LIVE-1.8** THE SYSTEM SHALL compute liveness without creating, modifying or deleting any file
  or directory.
- **LIVE-1.9** WHEN liveness is computed twice for an unchanged Cargo target dir with the same
  window and current time THE SYSTEM SHALL return the same liveness result.
- **LIVE-1.10** THE SYSTEM SHALL treat a unit of a never-collected kind like any other current unit
  when choosing roots and following edges.

## 2. Refuse to decide when the graph cannot prove reachability

**Story:** As the owner, I want hive to return Undecided, with the reasons and what clears each
one, for a target dir whose fingerprint graph cannot prove which units are needed, so that no unit
is called dead on incomplete evidence.

- **LIVE-2.1** IF a profile dir of the Cargo target dir is unsupported (CORE-4.14) THEN THE SYSTEM
  SHALL return Undecided for the whole target dir with a reason naming that profile dir.
- **LIVE-2.2** IF a not-current unit of the Cargo target dir is inside the window THEN THE SYSTEM
  SHALL return Undecided for the whole target dir with a reason naming that unit directory.
- **LIVE-2.3** IF an edge from a live unit matches no current unit in any profile dir of the Cargo
  target dir THEN THE SYSTEM SHALL return Undecided for the whole target dir with a reason naming
  the unit directory and the dependency's name, or the dependency's fingerprint when its name
  cannot be read.
- ~~**LIVE-2.4**~~ superseded by LIVE-2.13 (2026-10-31): the unreadable case moved to LIVE-2.12.
- **LIVE-2.13** WHEN a not-current unit is outside the window, or its unit timestamp file does not
  exist, THE SYSTEM SHALL leave it out of the liveness computation and out of every dead unit's
  file list.
- **LIVE-2.5** WHEN the liveness result is Undecided THE SYSTEM SHALL report no dead units.
- **LIVE-2.6** WHEN the liveness result is Undecided THE SYSTEM SHALL report every reason that
  applies, not only the first found.
- **LIVE-2.7** THE SYSTEM SHALL state, with each Undecided reason, the condition that clears it:
  the profile dir becoming supported (LIVE-2.1), the unit compiling or its unit timestamp leaving
  the window (LIVE-2.2), the depending unit no longer being live (LIVE-2.3), the directory
  becoming listable (LIVE-2.8), the unit directory being renamed or removed by Cargo (LIVE-2.9,
  LIVE-2.10), or the target dir becoming readable (LIVE-2.11).
- **LIVE-2.8** IF the `deps/` or `examples/` directory of a profile dir holding a dead unit exists
  but cannot be listed in full THEN THE SYSTEM SHALL return Undecided for the whole target dir with
  a reason naming that directory.
- **LIVE-2.9** IF the directory name of a current unit has no unit hash THEN THE SYSTEM SHALL
  return Undecided for the whole target dir with a reason naming that unit directory.
- **LIVE-2.10** IF two current units of one profile dir have the same unit hash THEN THE SYSTEM
  SHALL return Undecided for the whole target dir with a reason naming both unit directories.
- **LIVE-2.11** IF the Cargo target dir could not be read when it was identified THEN THE SYSTEM
  SHALL return Undecided with a reason naming the target dir and why.
- **LIVE-2.12** IF the unit timestamp file of a current or not-current unit exists but its
  modification time cannot be read, or a unit directory's name is not valid UTF-8, THEN THE SYSTEM
  SHALL return Undecided for the whole target dir with a reason naming that file or directory.

## 3. List the files of each dead unit

**Story:** As the owner, I want hive to list, for each dead unit, exactly the files a deletion may
remove, so that the later plan deletes artifacts with their fingerprint dir and nothing else.

- **LIVE-3.1** WHEN the liveness result is Decided THE SYSTEM SHALL list, for each dead unit that
  has at least one hash-keyed entry, the unit's directory under `.fingerprint/` and every
  hash-keyed entry in the `deps/` and `examples/` directories of the unit's own profile dir.
- **LIVE-3.2** THE SYSTEM SHALL never list a path under `build/` or `incremental/`, a direct child
  file of a profile dir, or a path outside the dead unit's own profile dir.
- **LIVE-3.3** THE SYSTEM SHALL never list the directory under `.fingerprint/`, or a hash-keyed
  entry, of a unit that is live, of a never-collected kind, or not current.
- ~~**LIVE-3.4**~~ superseded by LIVE-3.6 (2026-10-30, design review): listing a fingerprint dir
  alone contradicts ARCH-18.
- **LIVE-3.5** THE SYSTEM SHALL never list an entry whose name also carries the unit hash of a unit
  of the same profile dir that is not dead.
- **LIVE-3.6** WHEN a dead unit has no hash-keyed entry THE SYSTEM SHALL list no file for it, so
  its directory under `.fingerprint/` stays.

## 4. Prove on a copy of a real target that no live unit rebuilds

**Story:** As the owner, I want an ignored test that deletes the dead units' files in a clone of a
real Cargo target dir and replays my commands against it, so that I see whether any live unit
rebuilds and how many bytes the deletion would free, before ROAD-6 deletes anything for real.

- **LIVE-4.1** IF the proof test runs without `HIVE_PROVE_TARGET` (a Cargo target dir),
  `HIVE_PROVE_CHECKOUT` (the checkout that builds into it) and `HIVE_PROVE_CMDS` (the commands to
  replay) all set THEN THE SYSTEM SHALL fail the test with a message naming each missing variable.
- **LIVE-4.2** WHEN the proof test runs THE SYSTEM SHALL compute the liveness result and the dead
  units' files on the original Cargo target dir, once for each window of 6 hours, 24 hours and
  7 days, with the same current time for all three.
- ~~**LIVE-4.3**~~ superseded by LIVE-4.21 (2026-10-31, owner amendment after the first klynt run):
  the window clone is now taken from the warmed-up clone, not from the original.
- ~~**LIVE-4.4**~~ superseded by LIVE-4.23 (2026-10-31): the replay also carries `--locked`.
- **LIVE-4.23** WHEN the proof test replays commands (warm-up, baseline or a window) THE SYSTEM
  SHALL run each one as an argument vector, without a shell, in `HIVE_PROVE_CHECKOUT` with both
  `CARGO_TARGET_DIR` and `CARGO_BUILD_BUILD_DIR` set to the clone being replayed, with `--locked`
  (unless `--locked` or `--frozen` is already present) and `--message-format=json` inserted after
  the cargo subcommand (the first element that does not start with `+` or `-`), before any `--`.
- **LIVE-4.5** THE SYSTEM SHALL count as rebuilt every unit, in any profile dir of the clone, whose
  directory under `.fingerprint/` is new after the replay or whose unit timestamp's modification
  time changed during the replay.
- **LIVE-4.6** IF a rebuilt unit was live in that window's liveness result THEN THE SYSTEM SHALL
  fail the test, naming each such unit.
- **LIVE-4.7** IF a window's liveness result is Undecided or has no dead unit, or any replayed
  command exits non-zero or emits no `compiler-artifact` message, THEN THE SYSTEM SHALL report
  that window as Vacuous with the reason, and SHALL NOT count it as passed.
- **LIVE-4.8** IF every window is Vacuous THEN THE SYSTEM SHALL fail the test.
- **LIVE-4.9** WHEN the proof test finishes a window THE SYSTEM SHALL print, for that window, the
  number of dead units, the bytes their deletion would free measured by a dry run on the original
  Cargo target dir before any clone of it exists, with the same per-inode accounting as gc (AUTOGC-5.1, AUTOGC-5.2), the bytes held in
  `incremental/` and in direct child files of each profile dir, the number of rebuilt units, the
  number of rebuilt live units, and each Undecided or Vacuous reason.
- **LIVE-4.10** WHEN the proof test ends, passing or failing, THE SYSTEM SHALL remove every clone
  it created.
- **LIVE-4.11** IF a command in `HIVE_PROVE_CMDS` has no cargo subcommand, or carries
  `--message-format`, `--target-dir` or `--artifact-dir` in any form before a `--`, THEN THE
  SYSTEM SHALL fail the test naming that command.
- ~~**LIVE-4.12**~~ superseded by LIVE-4.20 (2026-10-31): the clone is compared with the warmed-up
  clone it was taken from, because the warm-up legitimately rebuilds units.
- **LIVE-4.13** IF any entry of the original Cargo target dir, or the checkout's `Cargo.lock`, is
  created, removed, or has its modification time or size changed between the start and the end of
  the proof test THEN THE SYSTEM SHALL fail the test naming the first such entries.
- **LIVE-4.14** WHEN the proof test prints a window's report THE SYSTEM SHALL include each replayed
  command's exact argument vector and exit status.
- **LIVE-4.15** WHILE the proof test runs THE SYSTEM SHALL hold, taken without waiting, the build
  lock (`.cargo-build-lock`) of every profile dir of the original Cargo target dir; IF one is held
  by another process THEN THE SYSTEM SHALL fail the test naming that profile dir.
- **LIVE-4.16** IF the volume holding the original Cargo target dir cannot clone files THEN THE
  SYSTEM SHALL fail the test before copying anything.
- ~~**LIVE-4.17**~~ superseded by LIVE-4.18 and LIVE-4.19 (2026-10-31): a baseline on a target the
  commands had never built rebuilt hundreds of units and made every window Vacuous (first klynt run).
- **LIVE-4.18** WHEN the proof test starts replaying THE SYSTEM SHALL first replay the commands
  once in a clone of the original Cargo target dir, with nothing deleted, as a warm-up, and SHALL
  keep that warmed-up clone as the source of every later clone; IF a warm-up command exits non-zero
  or emits no `compiler-artifact` message THEN THE SYSTEM SHALL report every window as Vacuous with
  the reason.
- **LIVE-4.19** WHEN the warm-up has finished THE SYSTEM SHALL replay the commands once more in a
  fresh clone of the warmed-up clone, with nothing deleted, as a baseline; IF any unit rebuilds in
  that baseline THEN THE SYSTEM SHALL report every window as Vacuous, naming the rebuilt units.
- **LIVE-4.20** IF, before a window's replay, any unit timestamp's modification time in the window's
  clone differs from the same file's in the warmed-up clone it was taken from THEN THE SYSTEM SHALL
  report that window as Vacuous with the reason.
- **LIVE-4.21** WHEN a window's liveness result is Decided with at least one dead unit THE SYSTEM
  SHALL delete exactly that window's listed files in a fresh APFS clone of the warmed-up clone, and
  SHALL NOT delete or modify anything in the original or in the warmed-up clone.
- **LIVE-4.22** WHEN the proof test prints its report THE SYSTEM SHALL include, for the warm-up and
  for the baseline, the number of units that rebuilt, each command's argument vector and exit status,
  and the elapsed time.
- ~~**LIVE-4.24**~~ superseded by LIVE-4.28 (2026-10-31, owner decision after the whole-branch review):
  the replay is compile-only, so no test or binary of the checkout runs.
- **LIVE-4.25** WHEN the proof test replays commands THE SYSTEM SHALL remove from the child's
  environment the target-dir, build-dir, artifact-dir, rustc-wrapper, `CARGO_UNSTABLE_*` and
  `CARGO_ALIAS_*` variables, and IF the checkout's Cargo configuration (the checkout, its ancestors
  and `$CARGO_HOME`, or `$HOME/.cargo`) contains `include`, `artifact-dir`, `out-dir`, or an
  `[alias]` entry named like an allowed command, THEN THE SYSTEM SHALL fail the test naming that file.
- **LIVE-4.26** IF a listing, a tree walk, a config file, a child's output or a child's run time
  reaches its bound (or a read fails) THEN THE SYSTEM SHALL fail the test naming it, and SHALL NOT
  continue with a partial result.
- **LIVE-4.27** IF the warm-up or the baseline did not run THEN THE SYSTEM SHALL print "not run" for
  it in the report, not a count of zero.
- **LIVE-4.28** IF a command in `HIVE_PROVE_CMDS` is not one of `build`, `check`, `doc`,
  `test --no-run` or `bench --no-run` (long names only; `test` or `bench` without `--no-run`, and
  `run`, are refused), or has any flag other than `+toolchain`, `-q`, `-v`, `--locked`, `--frozen`,
  `--offline` or `--color <value>` before the subcommand, or has an argument after `--`, THEN THE
  SYSTEM SHALL fail the test naming that command.

## 5. Existing commands keep working

**Story:** As the owner, I want `status`, `explain`, `gc`, `doctor` and hive's state files to behave
exactly as before after this feature lands, so that adding liveness changes nothing I already use.

Files this feature touches, and what each must keep doing:
- `src/fingerprint.rs` — read by status and explain: LIVE-5.1, LIVE-5.2.
- `src/explain.rs` — if its `deps/` hash index is shared with liveness: LIVE-5.3.
- `src/gc/` (`ledger.rs`, `run.rs`, `gc.rs`) — if the per-inode accounting is reused by the proof
  (today `mod ledger` is private, `src/gc.rs:6`): LIVE-5.4.
- `src/cli.rs` — LIVE-5.5.
- `src/discover.rs` (`Snapshot`) and `snapshot.json` — LIVE-5.6.
- `Cargo.toml` — LIVE-5.7.
- `src/lib.rs` (a new module line), `src/liveness.rs` (new), `tests/liveness.rs` (new),
  `tests/liveness_prove.rs` (new), `tests/common/mod.rs` (new helpers): no behavior to guard.

- **LIVE-5.1** (guard) WHEN status reports a profile dir's unit edges THE SYSTEM SHALL CONTINUE TO
  count them resolved, unresolved and ambiguous within that profile dir alone (CORE-4.8).
- **LIVE-5.2** (guard) WHEN a profile dir holds a not-current unit THE SYSTEM SHALL CONTINUE TO
  report that profile dir as supported in status, leaving the unit out of its unit and edge counts
  (CORE-4.16).
- **LIVE-5.3** (guard) WHEN `cargo hive explain` runs THE SYSTEM SHALL CONTINUE TO report the same
  copy groups, modes, feature divergence and failed units as before this feature.
- **LIVE-5.4** (guard) WHEN `cargo hive gc` runs THE SYSTEM SHALL CONTINUE TO delete only
  superseded incremental sessions with their session lock files and `rustc*`/`rmeta*` temp
  entries, with the same receipt.
- **LIVE-5.5** (guard) THE SYSTEM SHALL CONTINUE TO expose the same subcommands and flags as before
  this feature, adding none.
- **LIVE-5.6** (guard) WHEN status, gc or explain writes hive's state THE SYSTEM SHALL CONTINUE TO
  write `snapshot.json` with only its `metadata` and `unsupported_profiles` fields.
- **LIVE-5.7** (guard) WHEN the crate is packaged THE SYSTEM SHALL CONTINUE TO include only the
  paths listed in `Cargo.toml`'s `include`, so no file under `tests/` ships.

## 6. Quality attributes

**Section-kind:** nfr

**Story:** As a stakeholder, I want measurable quality targets for this feature, so that how-well is not left implicit.

- **Performance:** None — no standing target. Closure-scan cost at a 7-day window is an owned open
  question (below); LIVE-4.9's proof run is where it gets measured, and the number is recorded,
  not guessed.
- **Security:** None — liveness reads the owner's own target dirs and crosses no trust boundary;
  the proof deletes only inside its own clone (LIVE-4.21).
- **Reliability:** None — liveness writes nothing (LIVE-1.8). Deleting under the owner's lock with
  liveness rechecked at apply time is ROAD-6's (ARCH-19, ARCH-20).
- **Accessibility:** None — no user interface; liveness has no CLI surface in this feature.

## Out of Scope

- Deleting anything outside the proof's clone: plan files, apply, apply-time recheck, and the
  user-facing window override are ROAD-6; owner approval is ROAD-7.
- Any CLI subcommand, flag, receipt field or snapshot field for liveness, including a hidden proof
  command.
- Closing the window gap: a command that stays fully fresh for the whole window leaves no disk trace
  for `check`, `clippy` or `test` (`fresh-build-trace.md`), and A1/ARCH-1 forbid a wrapper, daemon,
  shim or hook. Its units may be collected and rebuild when needed again (GOAL-1 rewording).
- A heuristic to decide around a not-current unit or an unresolved edge (for example, protecting
  the newest sibling variant).
- Mapping `incremental/` dirs to units, and deleting profile-root uplifted copies or their `.d`.
- `build/`, and units of a never-collected kind.
- Store liveness (`last_live_at`, ROAD-8, ROAD-10).

## Open Questions

- Closure-scan cost and steady-state reclaim at a 7-day window — project owner — 2026-10-25 —
  forbid-guess (cấm đoán); carried from foundation `requirements.md:154`. Partly answered on
  2026-10-31 (`proof-runs.md`): on cargo-hive's own 1.7 GB target the 6 h and 1 d windows were
  measured (294 and 133 dead units, 0.98 and 0.41 GB freed, 0 live units rebuilt) and the 7 d window
  was vacuous because no unit was old enough; on klynt the 7 d window is Undecided (169 not-current
  units inside it) and no deletion trial completed there. Still open: reclaim and liveness cost at a
  7-day window on a large, active workspace.
