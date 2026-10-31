# Design: Closure liveness (live and dead units of a Cargo target dir, proven on a copy)

Feature code: LIVE
Status: Approved
Date: 2026-10-30
Requirements: ./requirements.md

## Context

Status already reads everything liveness needs except time. `discover::targets`
(`src/discover.rs:506`) builds one `TargetDir` per Cargo target dir, with a `ProfileReport` per
profile dir, and each supported report keeps its `UnitGraph` (`src/fingerprint.rs:103-121`). A
`Unit` carries its fingerprint dir name, kind, fingerprint (`hash_hex`, the hash-file content) and
its dependencies' fingerprints as `deps: Vec<u64>` (`:68-86`). Three things are missing:
- Edges are resolved per profile dir only, ambiguous matches push no edge, and unresolved ones are
  only counted (`:465-481`). Yet under `--target` a bin's proc-macro and build-script dependencies
  live in the host profile dir (`.skills/LIVE/fresh-build-trace.md`).
- Nothing reads a unit's `invoked.timestamp` mtime, except `explain::invoked_mtime` for
  not-current units (`src/explain.rs:880`).
- Nothing maps a unit to its `deps/` and `examples/` entries by name. explain's `mode_index`
  (`:343`) keeps only build and check flags per hash.

The binding constraint is that status, explain and gc must not change (LIVE-5.1 to LIVE-5.6), and
that status already pays for one full read of every fingerprint (the CORE-6.1 budget). That rules
out changing `read_profile` to resolve edges across profile dirs or to stat timestamps: its
per-profile `EdgeCounts` are what status prints (`src/cli.rs:660-684`), and every stat added there
is paid on every status run. Liveness instead works from the `TargetDir` the reader already
produced. It builds its own target-wide index from `Unit.deps` and `le_hex`, and does its own reads
(timestamps, two directory listings per profile dir) only when someone asks for liveness.

The second constraint is that ROAD-5 deletes nothing and adds no command (LIVE-1.8, LIVE-5.5). In
this feature its only caller is test code: CI fixtures, and an ignored proof test that replays real
commands against a clone. The proof needs the per-inode byte accounting gc uses (LIVE-4.9), which
is private today (`src/gc.rs:6`; `Ledger` is `pub(crate)`). One small public measuring function is
the narrowest way to reach it from `tests/`.

## Decisions

1. **Liveness is a pure, read-only function over an already-read `TargetDir`.** Its signature is
   `liveness(fs, &TargetDir, now, window) -> Liveness`. Timestamps and listings go through `Fs`
   (`mtime`, `read_dir_bounded`). The one JSON re-read in Decision 4 goes through `read_capped`
   on a std `File`, as `read_profile` already does. Nothing writes, so ROAD-6 can call it again
   under the owner's lock at apply time (ARCH-20).
2. **Edges are resolved target-wide, inside liveness.** One index maps fingerprint hex to every
   `(profile, unit)` holding it, across all supported profile dirs. Each `Unit.deps` entry is
   looked up through `le_hex`: one match is an edge, several are an edge to each (LIVE-1.4), and
   none is an unresolved edge. `read_profile` and its per-profile `EdgeCounts` stay as they are
   (LIVE-5.1).
3. **Any doubt makes the whole target dir Undecided, and every reason is collected before
   returning.** Liveness runs every check (LIVE-2.1 to LIVE-2.3, LIVE-2.8 to LIVE-2.11), including
   the `deps/` and `examples/` listings, before it chooses between Decided and Undecided, so one
   reason never hides another (LIVE-2.6). The printed list is capped at 20 reasons, and the total
   count is always kept.
4. **The dependency name for an unresolved edge is read lazily.** `Unit` is left unchanged: a new
   field would break struct literals in fingerprint's and explain's tests, and status would pay
   for it on every run. Only for an unresolved edge does liveness re-read that one unit's JSON
   through `fingerprint::dep_name`. If the name cannot be read, the reason gives the fingerprint
   instead (LIVE-2.3).
5. **A unit's files are matched by `-<unit hash>` followed by `.` or the end of the name.** This
   covers klynt's 3,650 `crate-<h>.crate.<h2>-cgu.NN.rcgu.o` objects (1.76 GB), which a "tail
   after the last `-`" rule misses (`.skills/LIVE/design-review.md` item 8). An entry that also
   carries the unit hash of a non-dead unit of the profile is never listed (LIVE-3.5). The unit
   hash rule (16 lowercase hex after the last `-`) moves to `fingerprint::unit_hash`, reusing
   `is_hash`, and explain's `directory_suffix` calls it. explain's `mode_index` keeps its own
   tail rule, so its output is unchanged (LIVE-5.3).
6. **Never-collected kinds are ordinary graph nodes that never become dead.** They can be roots and
   are traversed (LIVE-1.10), but they are filtered out when dead units are listed (LIVE-1.5,
   LIVE-3.3). A `run-build-script` unit's timestamp, current or not current, is
   `<profile>/build/<dir_name>/invoked.timestamp`. This is a read, never a touch (ARCH-18). One
   `in_window(mtime, now, window)` helper, with `checked_sub` so a huge window cannot underflow,
   serves every window check.
7. **ARCH-18 holds as written.** A dead unit with no hash-keyed entry lists no file, so its
   fingerprint dir stays (LIVE-3.6, replacing LIVE-3.4). The leftover is a few KB of JSON per unit.
8. **gc exposes one measuring function, built on the code it already runs.** `Runner::scan` moves to
   a module-level `scan(fs, root, account_bytes)` that stays directory-only, so gc's calls and its
   test-recorded operation order are unchanged. The new public `gc::measure(fs, target, paths,
   account_bytes) -> Footprint` lstats each top-level path: a file goes straight to the `Ledger`,
   a directory goes through `scan`. `measure` dedups its paths and drops any path nested under
   another, so `Ledger`'s per-path link counting stays correct. `Ledger` stays `pub(crate)`. The
   name `Footprint` avoids `receipt::Measured` (`src/receipt.rs:13`).
9. **The proof procedure is test code in a shared module, and every byte is measured before the
   first clone exists.** `tests/common/prove.rs` holds `prove(...) -> ProveReport`;
   `tests/liveness_prove.rs` (`#[ignore]`) calls it with `HIVE_PROVE_*`, and `tests/liveness.rs`
   calls it on a fixture crate. A clone makes the original's private bytes read as 0 while it
   exists (`design-review.md` item 2), so every window is computed and measured first. Each clone
   is then made, used and removed one at a time.
10. **The clone is APFS `cp -c -R -p`, preflighted and checked, not trusted.** `Fs::volume` must
    report `apfs`, or the test fails before copying (LIVE-4.16): on another file system `cp -c`
    silently does a full copy. The proof holds every profile's `.cargo-build-lock` for its whole
    run, so a background `cargo watch` cannot change the original (LIVE-4.15). It warms the
    target up first: one replay in a clone of the original builds whatever the owner's commands
    need but the target never had, and that warmed-up clone becomes the source of every later
    clone (LIVE-4.18, LIVE-4.21). A baseline replay on a fresh clone of it must then rebuild
    nothing, so an environment difference is never blamed on a deletion (LIVE-4.19). Each window
    clone's unit timestamps are compared with the warmed-up clone's (LIVE-4.20). The first klynt run
    showed why: a baseline on the original rebuilt hundreds of units the commands had never built,
    and the run took 3 h 42 min before every window came out Vacuous.
    Cargo's freshness does not depend on inodes, and `-p` kept every mtime at nanosecond
    resolution on klynt, measured in the review.
11. **No new dependency.** Mtimes are planted with std `File::set_modified` (already used in
    `tests/explain.rs`), `HIVE_PROVE_CMDS` is parsed with `serde_json` (already a dependency), and
    the clone uses the system `cp`.

No ADR: Decisions 2, 3 and 5 are cheap to reverse inside one module. The choices that are hard to
reverse (no wrapper, GOAL-1's wording) are recorded in ARCH-1 and the confirmed close package.

**Sync-back to requirements (re-surfaced for approval):**
- **Rewritten:** LIVE-2.3 (fingerprint fallback), LIVE-2.7 (clear conditions), LIVE-3.1 (only
  dead units with entries), LIVE-4.3 (APFS clone), LIVE-4.4 (insert after the subcommand),
  LIVE-4.9 (measured before any clone), LIVE-4.11 (also `--target-dir`, `--artifact-dir`, no
  subcommand), LIVE-4.13 (plus the checkout's `Cargo.lock`), and the *Hash-keyed entry* term.
- **New:** LIVE-2.8 (listing failure), LIVE-2.9 (no unit hash), LIVE-2.10 (duplicate unit hash),
  LIVE-2.11 (unreadable target dir), LIVE-3.5 (an entry carrying another unit's hash),
  LIVE-3.6 (no artifacts, no files), LIVE-4.15 (hold the build locks), LIVE-4.16 (APFS
  preflight), LIVE-4.17 (baseline replay; superseded 2026-10-31 by LIVE-4.18 to LIVE-4.22).
- **Retired:** LIVE-3.4, superseded by LIVE-3.6 because it contradicts ARCH-18.

## Architecture

Vocabulary for every section below: **module**, **interface**, **implementation**,
**seam** (public surface where behavior is observable and substitutable). Prefer
deep modules — an interface much simpler than what it hides.

### Liveness (`src/liveness.rs`, new)

Satisfies: LIVE-1.1, LIVE-1.2, LIVE-1.3, LIVE-1.4, LIVE-1.5, LIVE-1.6, LIVE-1.7, LIVE-1.8, LIVE-1.9, LIVE-1.10, LIVE-2.1, LIVE-2.2, LIVE-2.3, LIVE-2.4, LIVE-2.5, LIVE-2.6, LIVE-2.7, LIVE-2.8, LIVE-2.9, LIVE-2.10, LIVE-2.11, LIVE-3.1, LIVE-3.2, LIVE-3.3, LIVE-3.5, LIVE-3.6
Reuse: none — new code (rung 7). Nothing computes reachability or lists files per unit. The inputs come from rung 2: `TargetDir`/`ProfileReport`/`Unit`/`NotCurrentUnit`, `fingerprint::{le_hex, kind_of, unit_hash, dep_name}`, `Fs::{mtime, read_dir_bounded}`, and `gc::DEPS_CAP` as the listing cap.
Respects: ARCH-1, ARCH-9, ARCH-15, ARCH-16, ARCH-17, ARCH-18, ARCH-20
Interface:
```
pub const DEFAULT_WINDOW: Duration;                       // 7 days
pub fn liveness(fs: &dyn Fs, target: &TargetDir, now: SystemTime, window: Duration) -> Liveness;
pub enum Liveness { Decided { live: Vec<UnitRef>, dead: Vec<DeadUnit> },
                    Undecided { reasons: Vec<Uncertain> } }
pub struct UnitRef { pub profile: PathBuf, pub dir_name: String }
pub struct DeadUnit { pub unit: UnitRef, pub files: Vec<PathBuf> }   // empty, or fingerprint dir + entries
pub enum Uncertain { ProfileUnsupported { profile, why }, NotCurrentInWindow { dir },
                     Unresolved { unit: UnitRef, dependency: String }, ListingFailed { dir, why },
                     NoUnitHash { dir }, DuplicateUnitHash { a, b }, TargetUnreadable { target, why } }
impl Uncertain { pub fn clears_when(&self) -> &'static str }
```
Depth: callers know a target dir, a time and a window go in, and either live/dead units with file lists or a set of reasons come out. The index, root selection, traversal, kind filter, name matching and every check stay hidden.
Locality: all edits for these IDs land in the new module, plus one `pub mod liveness;` line in `src/lib.rs` after `git` (leave). Neighbors `fingerprint` (extend, next section), `explain`, `discover`, `cli`: leave.

Flow for one `TargetDir`. Every step adds to one reason list, and nothing returns early:
1. `target.unreadable` is Some → `TargetUnreadable` (LIVE-2.11). Each Unsupported profile →
   `ProfileUnsupported` (LIVE-2.1).
2. Supported profiles: index every current unit, `hash_hex → Vec<UnitId>`. A unit whose
   `unit_hash(dir_name)` is None → `NoUnitHash` (LIVE-2.9). Two units of one profile with the same
   unit hash → `DuplicateUnitHash` (LIVE-2.10).
3. Each not-current unit: its timestamp path is `build/<dir>/` when `kind_of(stem)` is
   `run-build-script`, else the fingerprint dir. Inside the window → `NotCurrentInWindow`
   (LIVE-2.2). Outside, or unreadable → ignored (LIVE-2.4).
4. Roots (LIVE-1.1, LIVE-1.2): current units whose timestamp (same path rule) is in the window.
   Missing or unreadable means not a root.
5. Traverse breadth-first from the roots over `le_hex(dep)` lookups (LIVE-1.3, LIVE-1.4). A lookup
   with no match from a live unit → `Unresolved`, with the name from `dep_name` or the
   fingerprint hex (LIVE-2.3).
6. Dead candidates: current units that are not live, have a unit hash, and are not of a
   never-collected kind (LIVE-1.5, LIVE-3.3). For each profile holding one, list `deps/` and
   `examples/` with `read_dir_bounded(_, DEPS_CAP)`. NotFound means empty. Any other error, or a
   listing over the cap → `ListingFailed` (LIVE-2.8).
7. Any reason → `Undecided { reasons }` sorted by (variant order, path), all of them (LIVE-2.5 to
   LIVE-2.7).
8. Otherwise each dead unit's entries are those whose name contains `-<h>` followed by `.` or the
   end, minus any entry that also contains `-<h'>` of a non-dead unit of that profile (LIVE-3.5).
   No entries → `files` empty (LIVE-3.6). Otherwise `files` = `.fingerprint/<dir>` plus the
   entries sorted by name (LIVE-3.1). Every path is joined onto the unit's own profile dir from its
   own two listings, so LIVE-3.2 holds by scope. LIVE-3.3 and LIVE-3.5 hold by the filters above
   and are tested, including the duplicate-tail and no-dash names.
9. `live` and `dead` are sorted by (profile, dir name), and entry groups use `BTreeMap`, so two
   calls on an unchanged target dir compare equal (LIVE-1.9).

### Fingerprint helpers (`src/fingerprint.rs`, extend)

Satisfies: infrastructure — the unit-hash and dependency-name rules Liveness consumes (no requirement ID of its own)
Reuse: rung 2 — extends `fingerprint`: `is_hash` (:140) for the 16-hex test, `read_capped` (:182) for the JSON re-read, and the `deps` entry shape `[u64, string, bool, u64]` checked at :171-176.
Respects: ARCH-9
Surface:
- `explain::directory_suffix` (src/explain.rs:315; called at :309 and :332) — **replace**: its body becomes `fingerprint::unit_hash`, the same 16-lowercase-hex rule.
- `explain::is_lower_hex_16` (src/explain.rs:618) — **leave**: it stays used by explain's other parsers. Merging the two copies of the hex test is not needed for this feature.
- `explain::directory_crate` (:302) and `failed_crate` (:602) — **leave**: they split the crate part, not the hash, and their output is unchanged (LIVE-5.3).
- `explain::mode_index` (:343-349) — **leave**: it keeps its tail rule. Liveness's broader match is its own.
- `read_profile`, `Unit`, `ProfileReport`, `EdgeCounts` and status's use of them (src/cli.rs:660-684) — untouched. Liveness only reads them.
Interface:
```
pub fn unit_hash(dir_name: &str) -> Option<&str>;      // tail after last '-', 16 lowercase hex
pub fn dep_name(fingerprint_dir: &Path, stem: &str, dep: u64) -> Option<String>;
```
Depth: n/a — extends `fingerprint`.
Locality: two small functions in `fingerprint` (extend). `explain::directory_suffix` becomes a one-line call (extract). Nothing else changes.

### gc byte measurement (`src/gc/run.rs`, `src/gc.rs`, extend)

Satisfies: infrastructure — the dry-run byte measurement the Proof procedure consumes (dead units' files, `incremental/` and profile-root files, with gc's per-inode accounting, AUTOGC-5.1/5.2; no requirement ID of its own)
Reuse: rung 2 — extends `gc`: `Ledger` (src/gc/ledger.rs:27-80), and `Runner::scan` (src/gc/run.rs:388) extracted to a module-level, directory-only `scan(fs, root, account_bytes)`; `measure` handles file roots itself.
Respects: ARCH-11
Surface:
- `Runner::remove_measured` (src/gc/run.rs:356-386; the only caller of `scan`, at :357; itself called at :259, :297, :345) — **replace**: it calls the module-level `scan` and keeps the same counts, receipt and skips (LIVE-5.4). The extracted `scan` performs the same `Fs` calls in the same order, so the recorded-operation tests in run.rs stay green. Its guard is `tests/gc.rs` plus the run.rs unit tests.
- `gc` public re-exports (src/gc.rs:11-13) — extended with `measure` and `Footprint`. Existing names are unchanged.
Interface:
```
pub struct Footprint { pub bytes: Bytes, pub unmeasured: u64 }
pub fn measure(fs: &dyn Fs, target: &Path, paths: &[PathBuf], account_bytes: bool) -> Footprint;
```
Depth: n/a — extends `gc`.
Locality: `run.rs` (extract `scan`, add `measure`) and `gc.rs` (one re-export line). `ledger.rs`: leave.

### Proof procedure (`tests/common/prove.rs`, `tests/liveness_prove.rs`, new test code)

Satisfies: LIVE-4.1, LIVE-4.2, LIVE-4.3, LIVE-4.4, LIVE-4.5, LIVE-4.6, LIVE-4.7, LIVE-4.8, LIVE-4.9, LIVE-4.10, LIVE-4.11, LIVE-4.12, LIVE-4.13, LIVE-4.14, LIVE-4.15, LIVE-4.16, LIVE-4.17
Reuse: none — new code (rung 7) for the procedure. It reuses `liveness::liveness`, `gc::measure`, `cargo_target::identify` + `fingerprint::read_profile` (it builds the `TargetDir` by hand, since no function does that for one path), `platform::current()` for `Fs` (`volume`, `try_lock_exclusive`), `serde_json`, the system `cp`, and the tree-walk pattern of `tests/guards.rs`.
Respects: ARCH-17 (it runs cargo only to verify, after liveness has decided, never as an input to it)
Interface:
```
pub struct ProveInput { target: PathBuf, checkout: PathBuf, cmds: Vec<Vec<String>>,
                        windows: Vec<Duration>, now: SystemTime }
pub fn prove(input: &ProveInput) -> Result<ProveReport, String>;  // Err = test-level failure
pub fn prove_with(fs: &dyn Fs, scratch: &Path, input: &ProveInput) -> Result<ProveReport, String>;
pub struct ProveReport { baseline_rebuilt: Vec<String>, windows: Vec<WindowReport>,
                         original_changed: Vec<PathBuf> }
pub enum WindowOutcome { Passed, Vacuous(String), RebuiltLive(Vec<String>) }
pub struct WindowReport { window, outcome, dead_units, dead_bytes: Footprint, unmapped: Footprint,
                          rebuilt: usize, rebuilt_live: usize, reasons: Vec<String>,
                          commands: Vec<(Vec<String>, i32)> }
```
Depth: a caller states the target, checkout, commands and windows, and gets a per-window verdict with its numbers. Locking, the original snapshot, measurement, cloning, the mtime check, deletion, baseline and replay, rebuild detection and cleanup stay hidden.
Locality: new test files only. `tests/common/mod.rs` gains `pub mod prove;` (extend). Existing helpers: leave.

Order of one run:
1. **Validate (LIVE-4.1, LIVE-4.11).** `tests/liveness_prove.rs` reads `HIVE_PROVE_TARGET`,
   `HIVE_PROVE_CHECKOUT` and `HIVE_PROVE_CMDS`, a JSON array of argument arrays such as
   `[["build","--offline"],["test","--no-run","--offline"]]`. The test fails, naming what is
   wrong, when:
   - a variable is missing;
   - a vector has no subcommand (no element that starts with neither `+` nor `-`);
   - a vector carries `--message-format`, `--target-dir` or `--artifact-dir` (bare, or with `=`)
     before a `--`.
2. **Quiesce and preflight (LIVE-4.15, LIVE-4.16).** Build the `TargetDir`, try-lock
   `.cargo-build-lock` in every profile dir and hold the locks to the end (a missing lock file
   needs no lock). A lock held elsewhere fails the test. `Fs::volume` of the target must report
   `apfs`, and the scratch dir must sit on the same volume (`mount` equal), or the test fails.
3. **Snapshot the original (LIVE-4.13).** Walk the target dir (path, mtime, size) and stat the
   checkout's `Cargo.lock`. The review measured 3.4 s for 545k entries.
4. **Decide and measure every window before any clone (LIVE-4.2, LIVE-4.9).** For 6 h, 24 h and
   7 d with one `now`: compute liveness, then `gc::measure` over the dead units' files and,
   separately, over every profile's `incremental/` and direct child files.
5. **Warm-up and baseline (LIVE-4.18, LIVE-4.19, LIVE-4.22).**
   - Warm-up: clone the original with `cp -c -R -p` into a warmed-up clone and replay the commands
     there once. A non-zero exit or no `compiler-artifact` line makes every window Vacuous. Keep
     the warmed-up clone until the end, and record its unit timestamps.
   - Baseline: clone the warmed-up clone, replay once more, and record the rebuilt units, then
     remove that clone. If any unit rebuilt, every window is Vacuous, naming them.
   - Print the warm-up's and the baseline's rebuilt counts, argv, exit status and elapsed time.
6. **Each window with dead units (LIVE-4.4 to LIVE-4.7, LIVE-4.20, LIVE-4.21).**
   - Clone the warmed-up clone, then compare every unit timestamp in the clone with the
     warmed-up clone's. Any difference makes the window Vacuous.
   - Delete exactly the listed files, rebased onto the clone.
   - Record the clone's unit timestamps and fingerprint dirs, after the deletion and before the
     replay.
   - Replay each vector with `--message-format=json` inserted after the subcommand, in
     `current_dir(checkout)`, with the three target-dir variables removed and then
     `CARGO_TARGET_DIR` and `CARGO_BUILD_BUILD_DIR` set to the clone. Record argv and exit status
     (LIVE-4.14).
   - A new fingerprint dir, or a changed timestamp (fingerprint dir or `build/` for
     run-build-script), counts as rebuilt. A rebuilt unit whose (profile rebased, dir name) is in
     the window's `live` makes the outcome `RebuiltLive`.
   - Non-zero exit, or no `compiler-artifact` line → Vacuous.
   - Remove the clone before the next window.
7. **Finish (LIVE-4.8, LIVE-4.10, LIVE-4.13).** Re-walk the original and compare: a change fails
   the test. Release the locks. All windows Vacuous fails the test. Clones live in a `Scratch`, so
   a panic still removes them. Each window prints its report block (run with
   `--ignored --nocapture`), stating that a Passed window covers only the replayed commands.

Notes: the proof needs an old-layout target, because a build dir split from the target dir is
Unsupported and therefore Undecided. On klynt, one clone took 84 s and its removal 30 s. The
dominant cost is the warm-up compile, which is paid once; later clones are cheap because APFS
clones share blocks, and the original is never replayed into.

### Fixture coverage (`tests/liveness.rs`, new test code)

Satisfies: infrastructure — the CI seam that exercises Liveness and the proof procedure on real Cargo output (IDs are carried by the sections above; the Seams table maps them)
Reuse: rung 2 — `tests/common` (`Scratch`, the `Hive::build_crate`/`check_crate` command pattern), and std `File::set_modified` for backdating. The `tag_target`/`write_unit` pattern from tests/explain.rs is copied in for synthetic dirs, with deps entries and timestamps added.
Interface: test functions only.
Depth: n/a — test code.
Locality: one new file (leave everything else).

Cases:
- Real cargo: a `--target` host-triple build with a proc-macro and a build script, then
  backdating. The host-dir proc-macro is live through the triple-dir bin.
- Real cargo: the backdated, unreached check variant of one lib is dead with its `lib*-<h>.rmeta`
  and `.d`. An example's `examples/<stem>-<h>*` entries are listed. A build-script unit is never
  dead. A run-build-script timestamp in `build/` makes it a root.
- Real cargo: the proof procedure on a fixture crate gives one Passed window and one Vacuous
  (empty dead set), and its clones are removed.
- Synthetic, for the remaining rules:
  - not-current units inside and outside the window, including a run-build-script one;
  - ambiguous edges, and unresolved edges from live and from dead units;
  - an Unsupported profile, and an unreadable target;
  - kind `other`, a non-hex tail, and duplicate tails;
  - a no-dash entry, a cgu-object entry, and an entry carrying a live unit's hash;
  - a dead unit with no entries;
  - a shuffled listing, for determinism.

## Seams for testing

New seams: `liveness::liveness` and `prove` (the test-side procedure). `gc::measure` and the
`fingerprint` helpers are tested in-module. Guards use existing seams.

| Seam | Kind | Covers |
|---|---|---|
| `liveness::liveness` on synthetic fingerprint dirs (`src/liveness.rs` tests) | unit | LIVE-1.2, LIVE-1.4, LIVE-1.5, LIVE-1.6, LIVE-1.8, LIVE-1.9, LIVE-1.10, LIVE-2.1, LIVE-2.2, LIVE-2.3, LIVE-2.4, LIVE-2.5, LIVE-2.6, LIVE-2.7, LIVE-2.8, LIVE-2.9, LIVE-2.10, LIVE-2.11, LIVE-3.2, LIVE-3.3, LIVE-3.5, LIVE-3.6 |
| `liveness::liveness` on real Cargo builds (`tests/liveness.rs`) | integration | LIVE-1.1, LIVE-1.3, LIVE-3.1 |
| Source guard: `src/liveness.rs` uses no process-spawning API (`tests/guards.rs`, existing) | integration | LIVE-1.7 |
| `fingerprint::{unit_hash, dep_name}` (`src/fingerprint.rs` tests) | unit | LIVE-2.3, LIVE-2.9 (name rules) |
| `gc::measure` and the file-root `scan` (`src/gc/run.rs` tests with the existing `Rec` fake) | unit | LIVE-4.9 (byte accounting) |
| `prove` / `prove_with` and its pure helpers on a fixture crate (`tests/liveness.rs`) | integration | LIVE-4.2, LIVE-4.3, LIVE-4.4, LIVE-4.5, LIVE-4.6, LIVE-4.7, LIVE-4.8, LIVE-4.9, LIVE-4.10, LIVE-4.11, LIVE-4.12, LIVE-4.13, LIVE-4.14, LIVE-4.15, LIVE-4.16, LIVE-4.17 |
| `tests/liveness_prove.rs` (`missing_vars` in CI; the `#[ignore]` env-driven run) | integration | LIVE-4.1 |
| `read_profile` per-profile edge counts and not-current handling (`src/fingerprint.rs` tests at :598, :848-1019, existing) | unit | LIVE-5.1, LIVE-5.2 |
| `cargo hive status` snapshot keys (`tests/liveness.rs`, new assertion) | integration | LIVE-5.6 |
| `cargo hive explain` (`tests/explain.rs`, existing) | integration | LIVE-5.3 |
| `cargo hive gc` (`tests/gc.rs`, existing) | integration | LIVE-5.4 |
| CLI flags (`tests/skill.rs` command/flag check, existing) plus a `--help` snapshot in `tests/liveness.rs` | integration | LIVE-5.5 |
| `cargo package --list` in `tests/liveness.rs` | integration | LIVE-5.7 |

## Coverage check

Every live requirement ID appears in exactly one `Satisfies:` line:
- LIVE-1.1 to LIVE-1.10, LIVE-2.1 to LIVE-2.11, LIVE-3.1 to LIVE-3.3, LIVE-3.5 and LIVE-3.6 →
  Liveness.
- LIVE-4.1 to LIVE-4.17 → Proof procedure.
- Deliberately unmapped:
  - LIVE-3.4 is retired (struck through).
  - LIVE-5.1 to LIVE-5.7 are guards, held by the `Surface:` dispositions (fingerprint, gc) and the
    existing seams in the table.
  - LIVE-6 (NFR) has no IDed criteria; all four attributes are `None` with reasons.
