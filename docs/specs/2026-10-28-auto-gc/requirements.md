# Requirements: Auto-tier GC (superseded incremental sessions and rustc temp entries)

Feature code: AUTOGC
Status: Approved
Date: 2026-10-28

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

Roadmap item: ROAD-2 (MILE-1, `docs/roadmap/INDEX.md`).

Sources:
- `.skills/AUTOGC/close-package.md` (confirmed 2026-10-28).
- `.skills/CORE/close-package.md` (locks A1–A6, D5, D6 and C1 stay in force; D7 is amended for live targets in story 5).
- `SPIKES.md` §10 and §16.
- The rustc 1.97.1 sources (`compiler/rustc_incremental/src/persist/fs.rs` at 8bab26f4f) and Cargo 1.97.1 sources (`src/cargo/core/compiler/layout.rs` at c980f4866).

Terms used as `CONTEXT.md` defines them: Auto tier, Cargo target dir, Profile dir, Build directory, Receipt, Protected/Secondary checkout. Further terms used below:
- **Session:** `s-<timestamp>-<random>-working` while rustc uses it, renamed to `s-<timestamp>-<random>-<svh>` once finalized.
- **Session lock file:** `s-<timestamp>-<random>.lock`, beside the session. rustc creates and locks it before it creates the `-working` directory (`fs.rs` 249–255).
- **Name timestamp:** the second dash field of those names: microseconds since the Unix epoch, written in base 36. A name timestamp later than the current time is never "at least 10 seconds before the current time", so such an item is never old enough.
- **Matching:** a session and a session lock file match when their names share the first three dash-separated fields, `s-<timestamp>-<random>`. rustc pairs them the same way (`fs.rs` 653–662).
- **Temp entry:** a *directory*, not a symbolic link, directly in `deps/`, whose name is `rustc` or `rmeta` followed only by ASCII letters and digits. These are the `tempfile` directories rustc stages output in (`link.rs` 107–112, `mfs.rs` 43–47). A real artifact can never match, because its name always contains `-`, `_` or `.` (for example `rustc_hash-<hash>.d`).

## 1. Check that hive's deletion lock works on this machine

**Story:** As a developer on a new Mac, I want `cargo hive doctor` to prove that the lock hive will delete under is refused whenever rustc or Cargo holds a lock, so that `cargo hive gc` is only enabled where its safety mechanism works.

- **AUTOGC-1.1** WHEN `cargo hive doctor` runs check (d) THE SYSTEM SHALL measure, for each of three holder kinds held by another process on a file (an exclusive `flock`, an `fcntl` read lock, and an `fcntl` write lock), two things:
  - whether a non-blocking exclusive `flock` taken on a read-only descriptor of that file is refused without waiting;
  - whether the same attempt succeeds after the holder has exited.
- **AUTOGC-1.2** WHEN `cargo hive doctor` runs check (d) THE SYSTEM SHALL also measure the reverse: while hive holds that exclusive `flock`, another process's non-blocking `fcntl` read-lock attempt and write-lock attempt on the same file are both refused without waiting.
- **AUTOGC-1.3** WHEN check (d) finishes THE SYSTEM SHALL report it as `pass` only if every refusal and every success measured by AUTOGC-1.1 and AUTOGC-1.2 occurred. Otherwise it reports `fail`, with each measured value in the receipt.

## 2. Reclaim superseded sessions and leftover rustc temp entries

**Story:** As a developer, I want to run `cargo hive gc` and have the superseded incremental sessions and leftover rustc temp entries in my targets deleted, so that disk comes back without any build being broken or rebuilt.

- **AUTOGC-2.1** WHEN the user runs `cargo hive gc` THE SYSTEM SHALL, in each crate directory under `incremental/` of each profile directory in scope, delete every finalized session that has a lock file and whose name timestamp is older than that of the newest such session in the same crate directory. The newest finalized session with a lock file is kept. Sessions without a lock file are handled by AUTOGC-2.7.
- **AUTOGC-2.2** WHEN gc deletes a session that has a lock file, or deletes a session lock file, THE SYSTEM SHALL hold a non-blocking exclusive `flock` on a read-only descriptor of that lock file throughout. The descriptor is opened without creating the file. The lock is taken before the deletion starts. The session directory is removed first and then its lock file, and the lock is released only after both are gone.
- **AUTOGC-2.3** IF a session's lock cannot be acquired without waiting THEN THE SYSTEM SHALL leave that session and its lock file untouched and count the session as skipped because held.
- **AUTOGC-2.4** IF a session's lock file exists but cannot be opened (for example, permission denied) THEN THE SYSTEM SHALL leave that session and its lock file untouched and count the session as skipped because its lock file could not be opened. That count is separate from the held count.
- **AUTOGC-2.5** WHEN gc finds a `-working` session THE SYSTEM SHALL delete it only if its name timestamp is at least 10 seconds before the current time and its lock is acquired as AUTOGC-2.2 requires.
- **AUTOGC-2.6** WHEN gc finds a session lock file with no matching session directory THE SYSTEM SHALL delete that lock file only if its name timestamp is at least 10 seconds before the current time and its lock is acquired as AUTOGC-2.2 requires. rustc 1.97.1 itself never removes these files: its GC tries to, but its timestamp parser rejects names ending in `.lock` (`fs.rs` 569–579, 672), so they accumulate.
- **AUTOGC-2.7** WHEN gc finds a session directory with no matching lock file THE SYSTEM SHALL delete that directory without taking a lock, as rustc's own GC does (`fs.rs` 700–708). rustc cannot be using such a directory, because it locks before it creates one, and it cannot copy from one, because copying needs that lock (`fs.rs` 387–400). This holds even when the directory is newer than the session AUTOGC-2.1 keeps.
- **AUTOGC-2.8** IF a name under a crate directory does not parse as a session or session lock file with a base-36 name timestamp THEN THE SYSTEM SHALL leave it untouched.
- **AUTOGC-2.9** WHEN gc processes a profile directory THE SYSTEM SHALL delete its temp entries only while holding a non-blocking exclusive `flock` on a read-only descriptor, opened without creating the file, of `.cargo-build-lock` in that build-directory profile dir. The lock is taken before the entries are listed and released after the last of them is removed.
- **AUTOGC-2.10** IF that `.cargo-build-lock` cannot be locked without waiting THEN THE SYSTEM SHALL leave every temp entry of that profile directory untouched and count the profile directory as skipped because held.
- **AUTOGC-2.11** IF that `.cargo-build-lock` does not exist or cannot be opened THEN THE SYSTEM SHALL leave every temp entry of that profile directory untouched and add a finding naming the profile directory and the reason.
- **AUTOGC-2.12** THE SYSTEM SHALL NOT delete, create or modify any path in a target other than those AUTOGC-2.1, 2.2, 2.5, 2.6, 2.7 and 2.9 name. That excludes the following:
  - `.fingerprint/` and `build/`;
  - any `deps/` entry that is not a temp entry;
  - artifacts outside `deps/`;
  - any lock file other than a session lock file.
- **AUTOGC-2.13** THE SYSTEM SHALL NOT follow a symbolic link while deleting, so nothing outside the session directory or temp entry being removed is reached through a link.
- **AUTOGC-2.14** IF a path gc is about to delete no longer exists (for example, a `-working` session finalized and renamed between listing and deletion) THEN THE SYSTEM SHALL treat it as neither an error nor a deletion, and count nothing for it.
- **AUTOGC-2.15** IF the recorded doctor results do not allow deletion for the active toolchain THEN THE SYSTEM SHALL refuse `gc` before touching any target, naming the failing doctor check or asking the user to run `cargo hive doctor`. This applies as CORE-1.4 and CORE-1.10 define, with the capability disabled by check (d) or check (f).

## 3. Preview what gc would reclaim

**Story:** As a developer or agent, I want `cargo hive gc --dry-run` to show exactly what `gc` would delete and free right now, so that I can decide before anything is deleted.

- **AUTOGC-3.1** WHEN the user runs `cargo hive gc --dry-run` THE SYSTEM SHALL return a receipt with the count names of AUTOGC-5.3, as a real run would report them. Every lock is tried, held only while its entry is measured, and released before the next entry. On a target where no build runs between the dry run and a real run, the counts are the same as the real run's. The test is on an idle fixture.
- **AUTOGC-3.2** WHILE gc runs with `--dry-run` THE SYSTEM SHALL create, modify or delete no file outside hive's state directory.
- **AUTOGC-3.3** THE SYSTEM SHALL record the receipt's command as `gc --dry-run` for a dry run and as `gc` for a real run.
- **AUTOGC-3.4** THE SYSTEM SHALL run `gc`, with or without `--dry-run`, without reading any input or asking for confirmation.

## 4. Clean the targets of one repository, or of all of them

**Story:** As a developer, I want `cargo hive gc <repo>` to clean only that repository's targets, and bare `cargo hive gc` to clean every target hive knows, so that a scoped run never touches another repository's builds.

- **AUTOGC-4.1** WHEN the user runs `cargo hive gc` with no repository argument THE SYSTEM SHALL process every target that `cargo hive status` with no argument would report and not report as unreadable, whatever its owner state: owned, orphan candidate, or unresolved, including targets under cache roots. Within those targets it processes only profile directories that status does not report as unsupported.
- **AUTOGC-4.2** WHEN the user runs `cargo hive gc <repo>…` THE SYSTEM SHALL process exactly the targets that are owned by a checkout of a named repository, plus the targets whose directory lies inside one of those checkouts, whatever their owner state. Other targets, including cache-root targets no named repository owns, are left untouched.
- **AUTOGC-4.3** IF a target in scope is reported unreadable, or one of its profile directories is reported unsupported (including the newer build-dir layout), THEN THE SYSTEM SHALL leave that target or profile directory untouched and add a finding naming it and the reason.
- **AUTOGC-4.4** IF a repository argument does not resolve to the main checkout of a registered repository THEN THE SYSTEM SHALL refuse that argument with a finding and still process the other arguments. The command then exits with status 1, as `status` does.
- **AUTOGC-4.5** IF a target in scope is not on an APFS volume THEN THE SYSTEM SHALL leave it untouched and add a finding naming it and its filesystem type. The lock proof (doctor check (d), spike 16) covers local APFS only.

## 5. See exactly how many bytes gc freed

**Story:** As a developer, I want the gc receipt to state the bytes actually freed, and separately the bytes deleted that are still held elsewhere, so that the reclaim claim is never inflated.

This story amends D7 of `.skills/CORE/close-package.md` for live targets only. D7 stays as written for the store (ROAD-8/10).

- **AUTOGC-5.1** WHEN gc deletes files THE SYSTEM SHALL count each deleted inode once in `freed_bytes`, as follows:
  - 0, if a hard link to it remains outside the set of paths this run deletes;
  - its private size from the APFS sharing attributes, if it carries a clone-sharing flag and is not compressed;
  - otherwise, `st_blocks × 512`.
- **AUTOGC-5.2** WHEN gc deletes files THE SYSTEM SHALL count in `deleted_bytes_still_shared`, for each deleted inode whose attributes were read, its `st_blocks × 512` minus what AUTOGC-5.1 counted as freed for it.
- **AUTOGC-5.3** WHEN gc finishes THE SYSTEM SHALL report these receipt counts, each as a total under its bare name and per processed target as `<name>:<target path>` (following the `units:<path>` precedent of `status`):

  | Count name | Meaning |
  |---|---|
  | `sessions_deleted` | Sessions removed |
  | `session_lock_files_deleted` | Session lock files removed |
  | `temp_entries_deleted` | Temp entries removed |
  | `skipped_held` | Sessions and profile directories skipped because their lock was held |
  | `skipped_lock_unopenable` | Sessions skipped because their lock file could not be opened |
  | `freed_bytes` | Per AUTOGC-5.1 |
  | `deleted_bytes_still_shared` | Per AUTOGC-5.2 |
- **AUTOGC-5.4** WHEN a real (not dry-run) gc finishes THE SYSTEM SHALL add one note per APFS container it deleted on, labelled as a cross-check and not a count. Volumes in one container share free space, so one note per container avoids counting twice. The note gives:
  - a mount point in that container;
  - the signed change in available bytes between gc's start and end. The design picks the `statfs` fields.
- **AUTOGC-5.5** IF the link count, block count or sharing attributes of a file gc deletes cannot be read THEN THE SYSTEM SHALL count that file in neither `freed_bytes` nor `deleted_bytes_still_shared`. It adds the file's path to `unmeasured` with the reason.

- **AUTOGC-5.6** IF doctor check (c) has not passed for the active toolchain THEN THE SYSTEM SHALL still delete as stories 2 to 4 require. It omits `freed_bytes` and `deleted_bytes_still_shared` from the counts and records each in `unmeasured` with the reason "doctor check (c) did not pass". It never falls back to counting `st_blocks × 512` as freed.

## 6. Quality attributes

**Section-kind:** nfr

**Story:** As a stakeholder, I want measurable quality targets for this feature, so that how-well is not left implicit.

Consult outcome: no `docs/product/metrics.md`, `docs/ops/reliability.md`, `docs/security/threat-model.md` or `docs/standards/accessibility.md` exists, so the consult is a no-op. The targets below come from the close-package done signals S-2, S-5 and S-6, and from CORE's locks. No system IDs were invented.

- **Performance:** None, because no latency target was locked for gc. The time a lock is held is bounded by AUTOGC-2.2 and AUTOGC-2.9, each lock covering only one session or one profile directory's temp entries.
- **Security:** **AUTOGC-6.1** THE SYSTEM SHALL delete only paths that AUTOGC-2.12 allows, inside targets in scope. This is verified by a test that snapshots the following, before and after `gc`, using a fixture `HOME` and `CARGO_HOME`:
  - a fixture repository and its targets;
  - a sibling registered repository and its targets;
  - the fixture `CARGO_HOME`.

  The test finds every other file present with unchanged content, and no file added. Directory metadata outside the allowed paths is not compared.
- **Reliability:** **AUTOGC-6.2** WHEN `gc` has run on a target THE SYSTEM SHALL have caused no rebuild. This is verified by a test in which `cargo build` with no source change compiles 0 units afterwards. The test also logs, without asserting it, whether the next build after a source edit populated its new session from the kept one: shared inodes with the files the kept session held. That is rustc's behavior (`fs.rs` 387–440), not hive's.
- **Reliability:** **AUTOGC-6.3** WHILE builds run on a target THE SYSTEM SHALL let `gc` run concurrently without causing a build failure or a rustc warning, and without deleting any session whose lock is held. This is verified by a test that runs 12 edit-and-build cycles while `gc` runs in a loop, as spike 16 did. It passes with 0 build failures, 0 rustc warnings and 0 held sessions deleted.
- **Reliability:** **AUTOGC-6.4** THE SYSTEM SHALL keep `freed_bytes` at or below the increase in the volume's available bytes over that run. This is verified on a dedicated, otherwise idle APFS disk image created by the test, whose deleted sessions include:
  - a file hard-linked into `deps/`;
  - a file hard-linked into both `deps/` and the kept session.
- **Reliability:** **AUTOGC-6.5** IF `gc` is killed at any point THEN THE SYSTEM SHALL leave every target in a state where the next `cargo build` with no source change compiles 0 units. At worst an interrupted deletion leaves a partly removed old session, or an orphan lock file, which the next gc removes; the order of AUTOGC-2.2 guarantees this. This is verified by a kill-during-gc test.
- **Accessibility:** None, because this is a headless CLI with JSON output.

## 7. Existing behavior this feature must not change

Files and surfaces this change touches:
- `src/platform/mod.rs`, `src/platform/apfs.rs`: the `Fs` trait gains a held-lock method; `probe_lock` and `hold_lock` already exist. Behavior at risk is guarded below.
- `src/doctor.rs`: check (d) changes; the other checks must not. Guarded below.
- `src/cli.rs`: a new `gc` command; `status`, `repo add`, `repo list`, `doctor` and the hidden `__hold-lock` exist. Guarded below.
- `src/discover.rs`, `src/cargo_target.rs`: target scope may need where each target was found. `status` output is at risk. Guarded below.
- `src/receipt.rs`: no behavior to guard; gc only adds counts and findings by name.
- `src/size.rs`: `status` sizes are at risk if it is shared. Guarded by AUTOGC-7.3.
- `README.md`: no behavior to guard; documentation only.
- Registered repositories' checkouts, their targets, and `~/.cargo`: behavior at risk. Guarded below.

- **AUTOGC-7.1** (guard) WHEN `cargo hive repo add`, `cargo hive status` or `cargo hive doctor` runs THE SYSTEM SHALL CONTINUE TO leave every registered checkout, every Cargo target dir and `~/.cargo` with no file created, modified or deleted (CORE-7.1).
- **AUTOGC-7.2** (guard) WHEN hive is installed and repositories are registered THE SYSTEM SHALL CONTINUE TO let `cargo` and rust-analyzer build every checkout exactly as without hive, with no Cargo configuration, environment variable, wrapper or hook added (CORE-7.2).
- **AUTOGC-7.3** (guard) WHEN `cargo hive status` runs THE SYSTEM SHALL CONTINUE TO report the same checkouts, targets, owner per target, findings, count names and values as before this feature.
- **AUTOGC-7.4** (guard) WHEN `cargo hive doctor` runs THE SYSTEM SHALL CONTINUE TO run checks (a), (b), (c), (e) and (f) with the same pass rules and measured values, and to disable capabilities by the CORE-1.3 map.
- **AUTOGC-7.5** (guard) WHEN any hive command runs once hive's state is open, `gc` included, THE SYSTEM SHALL CONTINUE TO satisfy CORE-5.1 through CORE-5.7, which require that the command:
  - prints exactly one JSON receipt on stdout under `--json` (5.1);
  - appends its receipt to the receipt log (5.2);
  - puts only counted or measured values in it (5.3);
  - never states or estimates time saved (5.4);
  - omits any value it cannot measure and says why in `unmeasured` (5.5);
  - includes the hive version, macOS version, schema version and scope (5.6);
  - takes its scope from arguments and never from the working directory (5.7).

  `gc`'s refusals (AUTOGC-2.15, AUTOGC-4.4) are receipts emitted through that same path. Argument-parse errors, a state root that cannot be opened, and the hidden `__hold-lock` exit without a receipt, as they do today.

## Out of Scope

- Closure or keep-set deletion: `deps/` artifacts, fingerprint dirs and dead units (ROAD-5/6). Deleting whole orphan target dirs (ROAD-6).
- Anything in `deps/` other than the `rustc*` and `rmeta*` temp entries; `.fingerprint/`; `build/`.
- The newer build-dir layout, which is reported only (AUTOGC-4.3).
- Linux lock semantics; v1 is macOS only.
- A daemon, a scheduler, launchd jobs or hooks (A1); `gc` runs only when a person or agent invokes it.
- Prompting on a TTY (`hive approve` is ROAD-7).
- The MCP tool for gc (ROAD-4).
- Changing how `status` shows owners. Listing every checkout that resolves to a target is a separate CORE change.
- Compressing, seeding or the store (ROAD-8 to 10), including D7's store accounting.
- Keeping `-C save-temps` output: rustc keeps those directories on purpose, and they match the temp-entry rule, so gc reclaims them. The README for `gc` says so.

## Open Questions

Owned unknowns from `.skills/AUTOGC/close-package.md`. These are proposed but not locked; the design must put each one to the owner rather than pick it.

- Where `incremental/` and the locks live in the newer build-dir layout — project owner — 2026-10-28 — forbid-guess (cấm đoán).
- Whether `status` should list every checkout that resolves to a target — project owner — 2026-10-28 — forbid-guess (cấm đoán); a separate CORE change.
- When check (c) fails, whether gc skips the private-size branch of AUTOGC-5.1 and counts shared-flag files in `deleted_bytes_still_shared`, or refuses to run — project owner — 2026-10-28 — forbid-guess (cấm đoán).
- Whether gc's receipt says why a target's owner is unresolved (a metadata failure, or scope) — project owner — 2026-10-28 — forbid-guess (cấm đoán).
- Whether `gc` computes its scope from the `snapshot.json` metadata cache or resolves ownership fresh; F9 binds only ROAD-6 — project owner — 2026-10-28 — forbid-guess (cấm đoán).
- Whether `deleted_bytes_still_shared` is split by cause (hard link or clone) — project owner — 2026-10-28 — forbid-guess (cấm đoán).
