# Design: Cargo 1.100 unit dirs

Feature code: BUILDDIR
Status: Draft
Date: 2026-11-17
Requirements: ./requirements.md

## Context

Hive reads Cargo's internal fingerprint files to decide what a target holds (ADR-0001), and
accepts only the exact shape it knows (ARCH-9). Every module that reads, deletes, fills or places
a unit builds its own paths from a unit's flat `dir_name`: `.fingerprint/<dir_name>`,
`deps/*-<hash>.*`, `build/<dir_name>`. About 88 non-test lines read `dir_name` across core, gc,
seed and the CLI, and about 20 more join `.fingerprint` or `deps` literally. Only `core::target` knows the 1.100 shape, and only to
identify a target. Everything after identification fails closed today, because `read_profile`
reports a profile with no `.fingerprint/` as unsupported (CORE-4.14). That makes 0.1.2 safe on
1.100 and useless there.

The binding constraint is that an unported consumer must keep failing closed. The danger lies in
the step between "unsupported" and "supported". Once `read_profile` returns unit-dir units as
Supported, any consumer that still joins `.fingerprint/<dir_name>` silently reads the wrong
path. For example, `seed::record::unit_missing` would read every seeded unit as gone, and
`gc::liveness::assemble` would list a fingerprint dir that does not exist. Each consumer would
act on wrong data instead of refusing. So the layout has to travel in the type that every
consumer already takes, and paths have to come from one place.

That constraint rules out the cheapest alternative, a second reader that rewrites unit-dir units
into flat-looking `dir_name`s so the rest of hive keeps working. Rejected options:

- **Flat-looking `dir_name`s.** These would point consumers at paths that do not exist, which is
  exactly the silent wrong action described above.
- **A separate 1.100 code path per command.** This would duplicate liveness, plan and seed logic
  that does not depend on where the files live.
- **Refusing mixed profile dirs.** The operator declined it, because a 1.99 build followed by a
  1.100 build in one target is the normal upgrade path.

Fresh retrieval (load-subgraph, 2026-11-17) names the shipped neighbours this design must guard
or amend: CHECKOUTGC, UPGRADE, SPLIT, LIVE, CORE, SEED, GCPLAN, EXPLAIN (and AUTOGC, BENCH by
surface). Invariants relied on:

- ARCH-9 (exact signature, now per layout);
- ARCH-17 (liveness from files on disk);
- ARCH-19 (locks tried without waiting);
- ARCH-20 (apply rechecks);
- ARCH-3 (never replace an artifact);
- ARCH-16 (state through `state`);
- ARCH-14 (gate on doctor).

ARCH-18 is restated as ARCH-22 (Decision 7).

## Decisions

1. **Unit location in the type.** `Unit` and `liveness::UnitRef` drop `dir_name: String` for
   `loc: UnitLoc { layout: UnitLayout, name_meta: String }`. `UnitLayout` is `Flat | UnitDir`,
   deriving `Ord`, `Hash`, `Serialize` and `Deserialize`. `NotCurrentUnit` keeps `dir`, its
   fingerprint dir, and gains `loc`.
   - **Name-meta.** `name_meta` is `<package>-<16 hex>` in both layouts. Flat dirs are already
     named by the package: `.fingerprint/probe-<hash>` holds the example `ex` of package `probe`,
     probed 2026-11-17. For a unit-dir unit hive builds it from `<package dir>-<unit dir>`. So
     `unit_hash`, `package_name`, crate-name stripping and store name-metas keep their meaning.
   - **Coverage.** The rename makes every `Unit`/`UnitRef` reader a compile error. Two kinds of
     reader escape it, and both are listed under Surface and caught by Decision 2's grep:
     `nc.dir.file_name()` at `gc/liveness.rs:145` and `seed/source.rs:263`, and the literal joins.
   - **Persisted name.** `SeededUnit` keeps its persisted serde name `dir_name` for the name-meta.
2. **One home for paths.** A new `cargo_hive_core::unit_path` module is the only code that joins
   `.fingerprint`, `deps`, `build`, `fingerprint`, `out` or `run` for a unit. Its functions are:
   - `fingerprint_dir`, `hash_file`, `stamp`;
   - `unit_dir`, which is `None` for flat;
   - `outputs`, which is `deps/` for flat and `out/` for unit-dir;
   - `flat_deps_dir`, for profile-wide listings.

   Task gate: `grep` finds no literal `".fingerprint"`, `"deps"`, `"out"` or `"run"` join outside
   it and the test helpers.
3. **One reader, two layouts.** `read_profile` reads `.fingerprint/` when present and every package
   dir under `build/` when present, in one pass. It builds one graph over both, and the
   format-signature and not-current rules are shared code. `ProfileReport` gains
   `layouts: BTreeSet<UnitLayout>` and `partial_unit_dirs: usize`. A unit-dir unit and a flat unit
   with the same unit hash in one profile stay `DuplicateUnitHash`, which fails closed.
4. **A dead unit-dir unit is planned as ordered entries.** The entries are each `.d` file in `out/`,
   then `out/`, `run/`, `fingerprint/`, and finally the unit dir. `ledger::remove_counted` removes
   dir entries recursively.
   - **Absent outputs.** An `out/` or `run/` that does not exist is left out of the list. NotFound
     means absence, not `ListingFailed`. So a unit an earlier run cut short after `out/` is finished
     on the next run; its outputs are already gone, which ARCH-22 allows.
   - **Groups.** A group of entries inside one unit dir is recognised by `unit_path::unit_dir_of`.
     In `unit_order` and `removal_order` it keeps its plan order, and it is never skipped for having
     no `deps/` files.
   - **Failure.** In `gc --checkout` and in `apply`, the group stops at its first failed or changed
     entry, so `fingerprint/` is never removed after an output removal failed.

   This gives BUILDDIR-3.3's order and the identity fence on every entry (dev/ino), and it puts no
   per-object rows in `plans.json`. Flat entries are unchanged (BUILDDIR-3.7). Today flat apply
   carries on with a unit's later entries after a failed one. That is recorded as debt and not
   changed here.
5. **One global state layout bump, to 2** (ADR-0004 keeps one counter). Every file with a new field
   reads the field as `#[serde(default)]`, so layout-1 files read unchanged:
   - `Capabilities.unit_layout: Option<UnitLayout>`, where absent with (f) passed means Flat
     (BUILDDIR-2.5);
   - `store::Row.unit_layout`, `StoreKey.unit_layout` and `SourceUnitRef.unit_layout`, defaulting
     to Flat;
   - `SeededUnit.unit_layout`, defaulting to Flat.

   A new committed `fixtures/v0.1.3-pre` is not needed. The existing v0.1.0 to v0.1.2 fixtures are
   the read side, and the first-write expectation becomes `"layout": 2`.
6. **Unit-dir store entries are named `u.<name_meta>-<fp hash>[~n]`** under
   `store/<toolchain value>/`. Package names never contain `.`, so the prefix cannot collide with
   a flat entry, and `parse_entry_key` reads the layout from it.
7. **ARCH-18 is retired for ARCH-22**:

   > A deletion inside a Cargo target dir removes a unit's fingerprint only in the same run as all
   > of that unit's outputs and after them. In the flat layout these are its hash-keyed `deps/` and
   > `examples/` files, never anything under `build/`. In the unit-dir layout they are its whole
   > `build/<package>/<hash>/` dir. Build-script units are never deleted, and only a plan that
   > deletes a whole orphan target removes the target entire.

   ADR-0005 records the reader decision (the beside-ADR-0001 note "already differs on nightly"
   becomes the stable case).
8. **CI runs the gate on both toolchains.** `ci.yml` gets a matrix `gate (1.97.1)` /
   `gate (1.100.0)`, both required on `main`, plus the non-blocking newest-stable job. Real-cargo
   fixture tests read the layout they got from `read_profile` and assert per layout. A synthetic
   twin builder in `tests/common/` writes each test's units in either layout.
9. **Sync-back to requirements (needs re-approval):**
   - BUILDDIR-1.8 named "the receipt's profile entry", but receipts hold only per-profile counts
     (`cli.rs:838-858`). It is reworded to the counts `units_flat:<profile>` and
     `units_unit_dir:<profile>`. The human-line wording is kept.
   - BUILDDIR-3.6 named checkout gc's counts for every command, but `apply` keeps `paths_deleted`
     (`apply.rs:1133`). It is reworded to "the counts that command keeps for a flat unit", and
     `gc --checkout`'s `unit_files_deleted` counts every file in the unit dir (`Removed.files`),
     not one per entry.

## Architecture

### Unit location and paths (`crates/core`: `unit_path`, `fingerprint`, `liveness`)

Satisfies: BUILDDIR-1.9, BUILDDIR-1.10
Reuse: rung 2, extending `core::liveness::unit_timestamp` and `fingerprint::unit_hash`; the
`UnitLoc` type and `unit_path` module are new (rung 7), because no type carries the layout today.
Respects: ARCH-9
Surface:
- `liveness::unit_timestamp` callers (`seed/place.rs:567`, `gc/liveness.rs:63,163,272`): replace,
  with `unit_path::stamp(profile, &loc, kind)`.
- `Unit.dir_name` / `UnitRef.dir_name` readers (about 88 non-test lines in gc, seed, cli): replace.
- Literal `.fingerprint` / `deps` joins (about 20, including `prove.rs:1040,1284`'s lock predicate
  in the test helpers): replace, by `unit_path` calls.
- Test readers of `dir_name` / `unit_timestamp` (`tests/seed_fill.rs`, `seed_liveness.rs`,
  `seed.rs`) and hard-coded layout tests (`state/src/lib.rs:914`, `discover.rs:1071-1086`):
  replace.
- `Uncertain::Unresolved` Display (`core/liveness.rs:90`): replace, printing the unit's real path.
- Test helpers that read `dir_name` (`tests/common/cargo_fixture.rs`, `prove.rs`, `plan_fixture.rs`):
  replace.
- `Unit`'s `Serialize` impl, which no state file persists (`Snapshot` holds no units): replace, with
  `dir_name` renamed to `name_meta`, plus `unit_layout`.

Interface:
- `UnitLayout { Flat, UnitDir }` (serde `"flat"` / `"unit-dir"`);
- `UnitLoc { layout, name_meta }`;
- `unit_path::{fingerprint_dir, hash_file, stamp, unit_dir, outputs}(profile, &UnitLoc, ...)`;
- `unit_path::{flat_deps_dir, unit_dir_of, is_fingerprint_dir}(path)`.

Depth: if it vanished, callers would need to know both trees: where each file of a unit lives in
each layout, and how to tell that a path lies inside a unit dir. Today that knowledge is copied
into some 108 lines.
Locality: lands in `crates/core/src/unit_path.rs`. Extend `fingerprint.rs` and `liveness.rs`;
every consumer only swaps a join for a call.

### Profile reader (`crates/core/src/fingerprint.rs`, `target.rs`, `locks.rs`)

Satisfies: BUILDDIR-1.1, BUILDDIR-1.2, BUILDDIR-1.3, BUILDDIR-1.4, BUILDDIR-1.5, BUILDDIR-1.6, BUILDDIR-1.7
Reuse: rung 2, extending `read_profile`, `read_unit`, `never_compiled` and `target::profile_layout_with`
(whose package-dir walk exists).
Respects: ARCH-9
Surface:
- Every `read_profile` caller (cli status, gc liveness, checkout, plan build/apply, seed source and
  fill, doctor f, explain): replace. They now receive unit-dir units, through `UnitLoc`.
- `ProfileReport` gains `layouts` and `partial_unit_dirs`: replace (struct literals in tests).
- `target::Layout`, read only by `checkout.rs:455`: replace, by `ProfileReport.layouts`. The enum
  stays only for identification.
- `locks::acquire_capped`'s missing-lock test (`:57`, `.fingerprint` exists): replace. It uses the
  `Layout` that `identify_bounded` already returns for the profile (`:44-50` discards it today), so
  a unit-dir profile with no lock file is skipped as Missing. A real-cargo test holds a 1.100 build
  (a slow `build.rs`) and asserts that hive's try-lock reports Held (BUILDDIR-3.9).
- `discover.rs:635`, status's caller of `read_profile`: replace, since it reads `layouts` and
  `partial_unit_dirs`. `Snapshot.unsupported_profiles` (`discover.rs:678-682`) self-heals once a
  profile is supported: leave.

Interface: `read_profile(profile_dir) -> ProfileReport` is unchanged in signature. How the reader
handles each kind of directory:

- **Flat units.** These are the CORE-4.7/4.16/4.17 rules, read from `fingerprint_dir`.
- **Each `build/<d>`.**
  - It is a package dir when every entry is a 16-hex dir, an empty dir included.
  - Otherwise it is a flat build-script dir when `<d>` ends in `-<16hex>`.
  - Otherwise the profile is unsupported (BUILDDIR-1.5).
- **Each `<hex>` in a package dir.**
  - With no `fingerprint/`, or an empty one, it is counted as a partial unit dir.
  - With `fingerprint/`, it is read with the same per-dir rules as a flat unit dir, so the JSON,
    hash file and not-current checks are one function over a dir path.

Unit hash and `name_meta` come from `<package>-<hex>`.
Depth: n/a — extends `read_profile`.
Locality: extend `fingerprint.rs` (`read_profile` split into `read_flat` + `read_unit_dirs` feeding
one graph build); `target.rs` leave; `locks.rs` extend one predicate.

### Status and doctor (`crates/cargo-hive/src/cli.rs`, `crates/doctor/src/lib.rs`)

Satisfies: BUILDDIR-1.8, BUILDDIR-2.1, BUILDDIR-2.2, BUILDDIR-2.3, BUILDDIR-2.4, BUILDDIR-2.5, BUILDDIR-2.6
Reuse: rung 2, using `report_targets`, `check_format`, `probe_output_texts` and `capabilities_for`.
Respects: ARCH-14, ARCH-16
Surface:
- Status receipt counts (`units:{key}` etc.): compat, since the existing keys stay. New keys are
  `units_flat:{key}`, `units_unit_dir:{key}` and, when non-zero, `partial_unit_dirs:{key}`. The
  human line gains ` (flat)`, ` (unit-dir)` or ` (flat + unit-dir)` after the edges, and
  `, N partial` when there are partial unit dirs.
- A `capabilities.json` that 0.1.2 wrote on 1.100 has check (f) failed, so commands stay refused
  until doctor reruns. The refusal for a failed (f) gains the hint "rerun `cargo hive doctor`: this
  hive reads Cargo 1.100 unit dirs": replace.
- `tests/status_targets.rs` line expectations: replace.
- `Capabilities` (`doctor:121`, `deny_unknown_fields`) gains `unit_layout`: replace. It is
  persisted, at layout 2.
- 0.1.2 reading a layout-2 `capabilities.json`: frozen, and discharged by UPGRADE-3.2 (it stops on
  a newer layout) and UPGRADE-5.8 (downgrade unsupported), which are already published.
- `require()` callers in cli and bench: leave, since gating is unchanged.

Interface:
- Check (f) passes iff the probe profile is Supported, `layouts.len() == 1`, units > 0,
  resolved > 0, unresolved = 0 and ambiguous = 0. It prints `unit_layout=<value>` and
  `capabilities_for` stores it.
- Check (g) collects `output-*` from `fingerprint_dir` of every unit and not-current unit of the
  probe report, and no longer re-lists `.fingerprint/`. A failed 1.100 compile leaves exactly the
  not-current shape in `fingerprint/`: only `output-lib-<name>` and `invoked.timestamp`, with the
  E0308 diagnostic in the output file. This was probed in `/private/var/tmp/gfail` on 2026-11-17.
- `Capabilities::unit_layout() -> Option<UnitLayout>` returns the stored value, else `Flat` when
  check (f) is not disabled, else `None`.

Depth: n/a — extends doctor and `report_targets`.
Locality: extend both. Check (g)'s own listing is replaced by the report's unit list.

### Dead units: liveness, plan and apply (`crates/gc/src/liveness.rs`, `plan_tier/`)

Satisfies: BUILDDIR-3.1, BUILDDIR-3.2, BUILDDIR-3.3, BUILDDIR-3.4, BUILDDIR-3.5, BUILDDIR-3.6, BUILDDIR-3.7, BUILDDIR-3.8, BUILDDIR-3.9
Reuse: rung 2, using `liveness()`, `dead_indices` (kinds unchanged), `assemble`, `plan_for_target`,
`recheck`, `removal_order` and `ledger::remove_counted` (which removes dirs recursively).
Respects: ARCH-17, ARCH-19, ARCH-20, ARCH-22
Surface:
- `DeadUnit.files`, read by `checkout.rs:307` `unit_order`, `plan_tier/build.rs:100` and
  `apply.rs:831` `dead_paths`: replace. For a unit-dir unit, `files` is the Decision 4 list in
  deletion order.
- `unit_order` (`checkout.rs:95`) and `removal_order` (`apply.rs:927`): replace with the Decision 4
  exception for unit-dir groups. Both re-sort today and skip a group with no `deps/` files.
- `NotCurrentUnit.dir.file_name()` readers (`gc/liveness.rs:145`, `seed/source.rs:263`): replace,
  with `nc.loc.name_meta`.
- Counts, per the reworded BUILDDIR-3.6:
  - checkout's `unit_files_deleted` adds `Removed.files` for a unit-dir entry: replace;
  - apply's `paths_deleted` counts entries as today: leave.
- `is_fingerprint_dir` (`checkout.rs:609`, `apply.rs:986`): replace, with
  `unit_path::is_fingerprint_dir(path)`, which is true for a `.fingerprint/<d>` or a
  `<hex>/fingerprint`.
- `PlannedEntry.unit` label (`build.rs:462`): replace, with `<rel_profile>/build/<pkg>/<hex>` for
  unit-dir. The shape and fields are unchanged, so no `plans.json` field is added.
- Plans written by 0.1.2 (flat only): frozen. They are read and applied as today (BUILDDIR-3.8),
  and the label scheme for flat is unchanged.
- `lists_examples` (`apply.rs:994`): leave, since a unit-dir plan lists no `examples/` entry
  (BUILDDIR-3.9).

Interface: `liveness(fs, target, now, window) -> Liveness` is unchanged. A dead unit-dir unit's
`files` is

```
[out/*.d (sorted)..., out/ (if present), run/ (if present), fingerprint/, <unit dir>]
```

and nothing else. `list_dead_files` / `claimants` run only over flat units. The unit-dir listing is
`out/` with `.d` names, bounded by `DEPS_CAP`. A NotFound leaves `out/` out of the list; any other
error or a cap hit is `ListingFailed`, which makes the target Undecided.
Depth: n/a — extends liveness.
Locality: extend `assemble`, plus one ordering exception in `unit_order` and `removal_order`.
Apply's recheck works unchanged, because it compares planned paths with the fresh dead set and
dev/ino per entry (BUILDDIR-3.5).

### Checkout gc and debug objects (`crates/gc/src/checkout.rs`)

Satisfies: BUILDDIR-4.1, BUILDDIR-4.2, BUILDDIR-4.3, BUILDDIR-4.4, BUILDDIR-4.5
Reuse: rung 2, using `clean_target`, `clean_profile_objects`, `list_profile`, `push_dir_candidates`
and `debug_map::read` (layout-free).
Respects: ARCH-17, ARCH-19, ARCH-22
Surface:
- The layout refusal at `:455`: replace. A profile whose report is Supported in any layout set is
  cleaned.
- `tests/checkout_gc.rs` expectations on `deps/`: replace with per-layout assertions.

Interface: `list_profile` becomes per layout. It lists `deps/` when present (flat), and walks
`build/<pkg>/<hex>/out/` for every unit dir, partial ones included.

- **The walk.** It lists only that one level: no recursion into sub-dirs, so rustc temp dirs are
  skipped. `*.rcgu.o` regular files there become objects, and other regular files or symlinks become
  binary candidates.
- **Early exits.** Two exits fire on a missing `deps/` today: `list_profile` returns an empty
  listing (about `:620`) and `clean_profile_objects` returns (`:466`). Both now apply only when the
  profile has no unit dir either.
- **Bounds.** Each `out/` listing counts against `DEPS_CAP`, and the `build/` and package-dir
  listings against `CRATE_DIR_CAP`. A failure or cap hit means no object deletion in the profile,
  plus a finding (BUILDDIR-4.4). Dead units are deleted before the object
scan, as today (CHECKOUTGC-2.5).
Depth: n/a — extends `list_profile`.
Locality: extend `list_profile` and remove the layout refusal.

### Temp sweep (`crates/gc/src/gc/run.rs`)

Satisfies: BUILDDIR-5.1, BUILDDIR-5.2
Reuse: rung 2, using `Runner::temp_entries`, `is_temp_entry` and `remove_measured`.
Respects: ARCH-19
Interface: under the same `.cargo-build-lock` flock, `temp_entries` lists `deps/` (flat) and each
`build/<pkg>/<hex>/out/` (unit-dir), and applies `is_temp_entry` to each.
- **Bounds.** `build/` and package dirs are bounded by `CRATE_DIR_CAP`, and each `out/` by
  `DEPS_CAP`. A cap hit skips the rest of that profile's sweep, with a note.
- **Sessions.** `sessions` is untouched.
Depth: n/a — extends `temp_entries`.
Locality: extend one function.

### Seed: fill, store, place, record, evict (`crates/seed/src/`, `crates/state/src/lib.rs`)

Satisfies: BUILDDIR-6.1, BUILDDIR-6.2, BUILDDIR-6.3, BUILDDIR-6.4, BUILDDIR-6.5, BUILDDIR-6.6, BUILDDIR-6.7, BUILDDIR-6.8
Reuse: rung 2, using `live_registry_units`, `unit_files`, `dep_class`, `entry_name`/`parse_entry_key`,
`place_unit`, `area`, `unit_missing`, `stamp_present`, `holds` and `hold_unsupported_fingerprint`.
Respects: ARCH-3, ARCH-5, ARCH-16, ARCH-21
Surface:
- `store.db` `Row` / `SourceUnitRef`, and `seeds.json` `SeededUnit`, gain `unit_layout`
  (serde default Flat): replace, at layout 2. 0.1.2 readers are frozen and discharged by
  UPGRADE-3.2/5.8.
- `state::Layout::NEWEST` goes from 1 to 2, which affects all six files: replace. The first-write
  expectations in `tests/upgrade.rs` change, and the v0.1.0 to v0.1.2 fixtures stay read-only.
- UPGRADE-7.1 (only `layout` may be added): its text is retired and its successor is BUILDDIR-6.7.
- Store entry dirs: flat entries are frozen and keep `{name_meta}-{fp}[~n]`. New unit-dir entries
  are `u.{name_meta}-{fp}[~n]`.
- `seed::Ctx` gains `unit_layout: Option<UnitLayout>`: replace.
  - The four `cli.rs` sites that build it pass `Capabilities::unit_layout()`.
  - Bench's direct `run_clone` call (`bench/mod.rs:371`) gets it through the same `Ctx`.
  - `choose` reads it from `Ctx`.
- `parse_entry_key` (`store.rs:610`): replace. Today it would read `u.<…>` as a flat rowless key,
  which evict would then delete. Unparseable entry dirs are still skipped (`store.rs:490`).
- `holds`, which keys `name_metas` without a layout: leave. It can only over-hold, never under-hold.
- `choose` filters rows by `unit_layout`; a `None` layout chooses nothing (`Choice::NoRow`).

Interface:
- **Fill.** For a unit-dir `SourceUnit`, `unit_files` is a recursive walk of `unit_dir`, minus
  `out/*.rcgu.o`. `dep_class` reads every `.d` file in the unit's `out/`, since that dir belongs to
  this one unit. Matching by hash would be wrong: a build-script unit's `out/build_script_build.d`
  carries no hash, so every crate with a `build.rs` would drop out of seeding. The hash file comes
  from `unit_path::hash_file`.
- **Place.** `area()` ranks by layout. Flat keeps deps 0, build 1, `.fingerprint` 2. Unit-dir ranks
  by the path component right after `build/<pkg>/<hex>/`, taken by position rather than by
  searching component names, so a package named `out` cannot misrank: `out/` 0, `run/` 1,
  `fingerprint/` 2. A cut-short placement then never leaves a fingerprint without its outputs.
- **Record.** `SeededUnit` for unit-dir records `deps_files` as the count of `out/` files placed
  and `has_build = false`. `unit_missing` checks `fingerprint_dir` and the `out/` file count.
- **Evict.** `hold_unsupported_fingerprint` also holds `<pkg>-<hex>` for every unit dir of an
  unsupported profile.

A mixed target, whose units carry two toolchain values, is skipped by fill as today
(`toolchain_row` needs one value).
Depth: n/a — extends seed.
Locality: extend `source.rs`, `store.rs`, `place.rs`, `record.rs`, `evict.rs`, `choose.rs` and
`fill.rs`. In `state/src/lib.rs` it is a one-constant change.

### Explain (`crates/gc/src/explain.rs`)

Satisfies: BUILDDIR-7.1, BUILDDIR-7.2
Reuse: rung 2, using `directory_crate` (works on `name_meta`), `mode_index`, `read_not_current` and
`profile_inputs`.
Interface: for unit-dir units, `profile_inputs` lists each unit's `out/` names into the same mode
index that `deps/` feeds, so an rlib means build and an rmeta alone means check. Failed units read
`output-*` from `NotCurrentUnit.dir`, which already is the fingerprint dir.
Depth: n/a — extends explain.
Locality: extend `profile_inputs`.

### Bench (`crates/cargo-hive/src/bench/`)

Satisfies: BUILDDIR-8.1, BUILDDIR-8.2
Reuse: rung 1. No bench code changes. Bench primes, seeds and counts through seed and
`record::counts`, which the seed section ports. Its build-dir check (`check.rs:164`) stays.
Interface: unchanged.
Depth: n/a — no new code.
Locality: leave; tests only.

### Spine, CI and proof (infrastructure)

Satisfies: BUILDDIR-9.1, BUILDDIR-9.2, BUILDDIR-9.3
Reuse: rung 2. The CI workflow and `scripts/gate.sh` come from #22. The klynt proof follows the
CHECKOUTGC Task 11 procedure (`.skills/verify/2026-11-15-checkout-gc/PLAN.md`).
Respects: ARCH-22
Interface:
- `ci.yml` has gate jobs `gate (1.97.1)` and `gate (1.100.0)`, both set as required checks on
  `main`.
- `docs/architecture/INDEX.md` has ARCH-18 struck and ARCH-22 added.
- `docs/adr/0005-read-cargo-unit-dirs-beside-flat.md` is added.
- `CONTEXT.md` changes:
  - Unit, unit hash, stamp, dead unit, debug object, linked binary and entry cover both layouts;
  - Flat layout, Unit-dir layout and Unit layout are added.
- The proof:
  1. copy a klynt worktree and build it with 1.100;
  2. run `gc --checkout` (timed), then rerun the test build (0 gc-caused recompiles);
  3. `seed` a second copy and build it.

Depth: n/a — infrastructure.
Locality: docs, CI, `.skills/verify/2026-11-17-cargo-unit-dirs/`.

## Seams for testing

| Seam | Kind | Covers |
|---|---|---|
| `fingerprint::read_profile` on synthetic profile dirs (flat, unit-dir, mixed, partial, deviant) | unit | BUILDDIR-1.1–1.7, 1.10 |
| `unit_path` functions | unit | BUILDDIR-1.9 |
| `cargo hive status --json` on a synthetic and a real-cargo target | integration | BUILDDIR-1.8, 1.10 |
| `doctor` check fns (`check_format`, check (g)) and `capabilities_for` / `Capabilities::unit_layout` | unit + integration | BUILDDIR-2.1–2.6 |
| `gc::liveness::liveness` on synthetic targets | unit | BUILDDIR-3.1, 3.2 |
| `gc --plan` / `apply` CLI on synthetic and real-cargo targets, with a failing `Fs` for 3.3 | integration | BUILDDIR-3.3–3.9 |
| `gc --checkout` CLI | integration | BUILDDIR-4.1–4.5 |
| bare `gc` CLI (temp entries, sessions) | integration | BUILDDIR-5.1, 5.2 |
| `seed` / fill / place / `status` seed counts CLI, store and record fns | integration + unit | BUILDDIR-6.1–6.8 |
| `tests/upgrade.rs` fixtures | integration | BUILDDIR-2.5, 2.6, 6.6, 6.7 |
| `explain` CLI | integration | BUILDDIR-7.1, 7.2 |
| `bench` CLI | integration | BUILDDIR-8.1, 8.2 |
| klynt proof run (manual, recorded) and CI checks | e2e | BUILDDIR-9.1–9.3 |

No new seam: each row is an existing seam the current tests already use.

## Coverage check

Every BUILDDIR ID from 1.1 to 9.3 appears in exactly one `Satisfies:` line above. None is unmapped.
BUILDDIR-1.8 is reworded per Decision 9 and waits for re-approval.
