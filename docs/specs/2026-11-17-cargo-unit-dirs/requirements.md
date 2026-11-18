# Requirements: Cargo 1.100 unit dirs

Feature code: BUILDDIR
Status: Implemented
Approved: 2026-11-17, by the operator (Jayden), in session: "xác nhận" (each story confirmed as one demoable act); BUILDDIR-1.8 and 3.6 rewordings approved 2026-11-17 with the design: "duyet"
Date: 2026-11-17
Roadmap item: — (issue #33, a bug outside the roadmap's items)

<!--
Rules:
- Every acceptance criterion gets a hierarchical ID: BUILDDIR-<story>.<criterion>.
- EARS phrasing; guards use SHALL CONTINUE TO.
- IDs are immutable once Status is Approved. Retire by striking through; never renumber.
- A criterion marked "(agent-decided)" states a value the operator delegated or did not
  choose. It is not the operator's own choice.
-->

Operator decisions, 2026-11-17, in this session: a dead unit in the new layout is deleted as its
whole unit dir, build-script units never; a profile dir holding both layouts is read with both;
every command gets the new layout in one feature and one release.

Terms are those of `CONTEXT.md`: Cargo target dir, profile dir, build directory, unit, unit hash,
not-current unit, stamp, dead unit, debug object, linked binary, store, entry, fill, seed,
registry unit, plan, receipt, state file. In this file:

- **Flat layout**: what Cargo 1.99 and older write. Units are dirs under `<profile>/.fingerprint/`
  named `<name>-<unit hash>`, their outputs are hash-keyed files in `<profile>/deps/` and
  `<profile>/examples/`, and build scripts have `<profile>/build/<name>-<unit hash>/`.
- **Unit-dir layout**: what Cargo 1.100 writes (cargo issue 15010). Each unit is one dir
  `<profile>/build/<package>/<unit hash>/`, the **unit dir**. Its `fingerprint/` holds the same
  files a flat `.fingerprint/<dir>/` holds, its `out/` holds every output (rlib, rmeta, `.d`,
  binaries, `*.rcgu.o`), and a run-build-script unit also has `run/` (`invoked.timestamp`,
  `root-output`, `stdout`, `stderr`). There is no `.fingerprint/` and no `deps/`.
- **Mixed profile dir**: a profile dir holding units in both layouts, which building with 1.99
  and then 1.100 in one target produces (reproduced 2026-11-17).
- **A package dir** is a dir directly under `<profile>/build/` all of whose entries, possibly
  none, are dirs named by 16 lowercase hex. A **partial unit dir** is such a 16-hex dir with no
  `fingerprint/` (one was seen holding only an empty `out/`).
- **Unit layout**: which of the two layouts a unit, a store entry or a seeded unit is in, `flat` or
  `unit-dir`. It is not the state-file **Layout** of `CONTEXT.md`.

## 1. See a Cargo 1.100 target in status

**Story:** As a developer on Rust 1.100, I want `cargo hive status` to count the units of my
targets and their edges, so that I can see hive reads them instead of calling them unsupported.

- **BUILDDIR-1.1** WHEN hive reads a profile dir THE SYSTEM SHALL read as a unit each unit dir
  whose `fingerprint/` holds exactly one fingerprint JSON in the fingerprint format signature
  beside a non-empty hash file of the same stem, and SHALL take its unit hash from the unit dir's
  name and its package from the package dir's name (the dir names give the unit's name-meta; the
  edge key is BUILDDIR-1.11's).
- **BUILDDIR-1.2** WHEN a profile dir is a mixed profile dir THE SYSTEM SHALL read the units of
  both layouts, resolving edges over all of them and across every profile dir of the target as for
  flat units.
- **BUILDDIR-1.3** WHEN a unit dir's `fingerprint/` holds only `invoked.timestamp` and `output-*`
  files, or holds an empty hash file beside a fingerprint JSON in the format signature, THE SYSTEM
  SHALL count it as a not-current unit, as CORE-4.16 does for a flat unit dir.
- **BUILDDIR-1.4** (agent-decided) WHEN a profile dir holds a partial unit dir THE SYSTEM SHALL
  count it apart from the units, treat it as neither a unit nor a reason for unsupported, and never
  delete or fill from it.
- **BUILDDIR-1.5** IF a unit dir's `fingerprint/` deviates from the format signature in any other
  way, or `build/` holds an entry that is neither a package dir nor a flat build-script dir, THEN THE
  SYSTEM SHALL mark the whole profile dir unsupported (ARCH-9).
- **BUILDDIR-1.6** (agent-decided) THE SYSTEM SHALL read a dir directly under `build/` as a package
  dir when it meets the package-dir definition above, empty ones included, and otherwise as a flat
  build-script dir only when its name ends in `-<16 lowercase hex>`.
- **BUILDDIR-1.7** THE SYSTEM SHALL no longer mark a profile dir unsupported for having no
  `.fingerprint/` when it holds a unit dir (CORE-4.14 is superseded by this criterion and 1.5). A dir
  with neither stays unidentified as a profile dir, as CORE-4.12 has it.
- **BUILDDIR-1.8** WHEN status reports a profile dir THE SYSTEM SHALL name the unit layouts its units
  are in, in the human line (`flat`, `unit-dir` or `flat + unit-dir` after the edge counts) and in the
  receipt as the counts `units_flat:<profile>` and `units_unit_dir:<profile>`, the number of units
  in each layout, a layout with no units getting no key (agent-decided: the names; reworded
  2026-11-17 in design, because a receipt holds per-profile counts, not profile entries).
- **BUILDDIR-1.9** WHEN hive dates a unit-dir unit for liveness THE SYSTEM SHALL read its stamp at
  `run/invoked.timestamp` for a run-build-script unit and at `fingerprint/invoked.timestamp`
  otherwise, and the hash file inside `fingerprint/`.
- **BUILDDIR-1.10** (guard) WHEN a profile dir holds only flat units THE SYSTEM SHALL CONTINUE TO
  read, count and resolve them as 0.1.2 does (CORE-4.7, CORE-4.8, CORE-4.16, CORE-4.17).
- **BUILDDIR-1.11** WHEN hive resolves a dependency edge to a unit-dir unit THE SYSTEM SHALL match
  the edge to the unit's hash file content, as it does for a flat unit, and SHALL NOT require the
  unit dir's name to equal it. Cargo 1.100 writes a different hex as the dir name and in the hash
  file (reproduced 2026-11-18 on rustc 1.100.0: dir `5c0d43421562aa29`, hash file
  `c33088e25b0468c9`, edge `c968045be28830c3`); the first design took the dir name as the edge key,
  so on a real 1.100 target every edge stayed unresolved (issue #33). Guard: BUILDDIR-1.10.

## 2. Doctor passes on Cargo 1.100 and names the layout

**Story:** As a developer on Rust 1.100, I want `cargo hive doctor` to pass its fingerprint and
failed-unit checks and tell me which layout it found, so that deletion and seeding turn on for my
toolchain.

- **BUILDDIR-2.1** WHEN doctor runs check (f) THE SYSTEM SHALL read its probe target's `debug/` in
  whichever layout the active toolchain wrote, and pass under the conditions it uses today: at least
  one unit, at least one resolved edge, none unresolved, none ambiguous.
- **BUILDDIR-2.2** WHEN check (f) passes THE SYSTEM SHALL print among its measures `unit_layout` with
  the value `flat` or `unit-dir`, the one unit layout the probe build wrote, and record that value in
  `capabilities.json` for the active toolchain.
- **BUILDDIR-2.3** IF the probe build wrote units in both layouts or in neither THEN THE SYSTEM SHALL
  fail check (f), naming the reason.
- **BUILDDIR-2.4** WHEN doctor runs check (g) on a unit-dir probe THE SYSTEM SHALL read the
  `output-*` files in each unit dir's `fingerprint/`, with the pass rule it uses today.
- **BUILDDIR-2.5** WHEN hive reads a `capabilities.json` written by 0.1.2, which records no unit
  layout, THE SYSTEM SHALL honour its results and read the unit layout as `flat`, since 0.1.2 passes
  check (f) only on flat profiles (CORE-4.14).
- **BUILDDIR-2.6** WHEN doctor writes `capabilities.json` with the unit layout THE SYSTEM SHALL write
  it at a new state-file layout, so that 0.1.2 stops on it as a file from a newer hive
  (UPGRADE-3.2, UPGRADE-5.8).

## 3. Collect dead units in the unit-dir layout with a plan

**Story:** As a developer on Rust 1.100, I want `cargo hive gc --plan` and `apply` to delete my
targets' dead units, so that old builds' units stop filling my disk.

- **BUILDDIR-3.1** WHEN liveness is Decided for a target THE SYSTEM SHALL list, for each dead
  unit-dir unit, its whole unit dir as the unit's one deletable entry, and SHALL list nothing else
  under `build/` for it.
- **BUILDDIR-3.2** THE SYSTEM SHALL treat as dead units only lib, bin, test, example and bench units,
  in either layout. A build-script, run-build-script, doc-lib, doc-bin or other-kind unit, a
  not-current unit and a partial unit dir are never deleted.
- **BUILDDIR-3.3** (agent-decided: the order) WHEN hive deletes a unit dir THE SYSTEM SHALL delete
  the `.d` files in its `out/` first, then the rest of `out/`, then `run/`, then `fingerprint/`, then
  the unit dir. So if the deletion stops part way, through a failed removal or a cancel, every file
  of the unit's `fingerprint/` that is gone has every file of its `out/` and `run/` gone before it.
- **BUILDDIR-3.4** THE SYSTEM SHALL never delete a package dir, a flat build-script dir, or any file
  under `build/` that belongs to no dead unit-dir unit; only an orphan plan removes a whole target.
- **BUILDDIR-3.5** WHEN `apply` runs a plan holding a unit dir THE SYSTEM SHALL delete it only if, at
  apply time, the unit is still dead under liveness, the target's locks are taken, its profile dir's
  format is supported and the unit dir's device and inode equal those the plan recorded. Any other
  outcome refuses that unit as stale, as a stale flat entry is refused (GCPLAN).
- **BUILDDIR-3.6** WHEN a dead unit dir is deleted THE SYSTEM SHALL count it with the counts that
  command keeps for a flat unit (`gc --checkout`: units deleted, and unit files deleted counting
  every file in the unit dir; `apply`: paths deleted), and its bytes per inode in freed bytes or in
  deleted bytes a surviving link or clone still holds (reworded 2026-11-17 in design: `apply` keeps
  no unit counts).
- **BUILDDIR-3.7** (guard) WHEN a plan or apply handles flat units THE SYSTEM SHALL CONTINUE TO
  delete only their hash-keyed `deps/` and `examples/` files with their fingerprint dirs, and nothing
  under `build/` (ARCH-18 for the flat layout).
- **BUILDDIR-3.8** (guard) WHEN `apply` reads a plan written by 0.1.2 THE SYSTEM SHALL CONTINUE TO
  apply or refuse it as 0.1.2 would.
- **BUILDDIR-3.9** WHEN hive deletes unit dirs from a profile dir THE SYSTEM SHALL hold the locks it
  holds for a flat deletion there: `.cargo-build-lock` and `.cargo-lock` when present. A unit-dir
  deletion lists no `examples/` entry, so it takes no `.cargo-artifact-lock`.

## 4. Clean one checkout in the unit-dir layout

**Story:** As an agent in a reused worktree on Rust 1.100, I want `cargo hive gc --checkout <path>`
to delete that worktree's dead units and unreferenced debug objects, so that the next test build
starts as fast as it does on the flat layout.

- **BUILDDIR-4.1** WHEN the command runs on a target with unit-dir units THE SYSTEM SHALL delete its
  dead unit dirs as story 3 lists and orders them, in the same run, under the target's Cargo locks.
- **BUILDDIR-4.2** WHEN the command runs THE SYSTEM SHALL treat as debug objects of a profile dir,
  beside those of CHECKOUTGC-2.1, the `*.rcgu.o` regular files directly in each unit dir's `out/`,
  and SHALL delete those whose file name no loose N_OSO entry of any linked binary in that profile
  dir names.
- **BUILDDIR-4.3** THE SYSTEM SHALL count as linked binaries of a profile dir, beside those of
  CHECKOUTGC-2.2, the Mach-O files directly in each unit dir's `out/`, test and build-script binaries
  included.
- **BUILDDIR-4.4** IF listing a package dir or a unit dir's `out/` fails or goes past the listing
  bound THEN THE SYSTEM SHALL delete no debug object in that profile dir and report a finding naming
  it and the reason, as CHECKOUTGC-2.9 does for `deps/`.
- **BUILDDIR-4.5** (guard) WHEN the command runs on flat units THE SYSTEM SHALL CONTINUE TO behave
  as CHECKOUTGC stories 1 to 4 state, including CHECKOUTGC-2.7, 2.8 and 2.9.

## 5. Sweep rustc temp entries in unit dirs

**Story:** As a developer on Rust 1.100, I want bare `cargo hive gc` to delete the temp dirs a killed
rustc left in my unit dirs, so that crashed builds do not leave space behind.

- **BUILDDIR-5.1** (agent-decided) WHEN gc processes a profile dir THE SYSTEM SHALL treat as temp
  entries, beside those directly in `deps/`, the directories directly in a unit dir's `out/` that
  match the temp-entry rule of AUTOGC (a name of `rustc` or `rmeta` followed only by ASCII letters and
  digits, not a symlink), and SHALL delete them under the lock AUTOGC-2.9 names.
- **BUILDDIR-5.2** (guard) WHEN gc deletes superseded incremental sessions THE SYSTEM SHALL CONTINUE
  TO handle `incremental/` as it does today, whatever layout the profile's units are in.

## 6. Seed a fresh checkout on Cargo 1.100

**Story:** As a developer on Rust 1.100, I want `cargo hive seed` to fill the store from my repo and
clone registry units into a new worktree, so that its first build skips them as it does on 1.99.

- **BUILDDIR-6.1** WHEN fill takes a live registry unit in the unit-dir layout THE SYSTEM SHALL store
  as its entry every file of its unit dir, except the `*.rcgu.o` files in `out/`, each under its
  path relative to the target.
- **BUILDDIR-6.2** WHEN fill decides whether a unit-dir unit is a registry unit THE SYSTEM SHALL read
  the `.d` dep-info in its `out/`, with the rule fill uses for a flat unit's `deps/` or `build/<dir>/`
  `.d`.
- **BUILDDIR-6.3** WHEN seed places entries into a checkout THE SYSTEM SHALL place only entries whose
  unit layout is the one `capabilities.json` records for the checkout's active toolchain, so that no
  flat entry is ever cloned into a target its Cargo writes as unit dirs, nor the reverse.
- **BUILDDIR-6.4** THE SYSTEM SHALL keep a unit's unit layout in its store key and entry path, so that
  a flat and a unit-dir entry of the same toolchain value, name-meta and fingerprint hash are two
  entries.
- **BUILDDIR-6.5** WHEN store gc decides which entries a target holds THE SYSTEM SHALL count the
  unit-dir units of that target, and the unit dirs of an unsupported profile, as holding their keys,
  as it does for flat units.
- **BUILDDIR-6.6** (guard) WHEN hive reads a `store.db` or `seeds.json` written by 0.1.2 THE SYSTEM
  SHALL CONTINUE TO read every entry and seeded unit in it as flat, and seed flat checkouts from it as
  0.1.2 does.
- **BUILDDIR-6.7** WHEN hive writes `store.db` or `seeds.json` with unit layouts THE SYSTEM SHALL write
  them at a new state-file layout, and SHALL read the committed v0.1.0, v0.1.1 and v0.1.2 fixtures as
  flat (UPGRADE-1.1, UPGRADE-2.1). This amends UPGRADE-7.1, which allowed `layout` as the only new
  field.
- **BUILDDIR-6.8** WHEN seed records what it placed THE SYSTEM SHALL record each seeded unit's unit
  layout, and status SHALL count a seeded unit-dir unit as present or gone by its unit dir.

## 7. Explain a Cargo 1.100 target

**Story:** As a developer on Rust 1.100, I want `cargo hive explain <crate>` to list the crate's
copies, feature splits and failed units, so that I can see why it is built more than once.

- **BUILDDIR-7.1** WHEN explain reads a unit-dir unit THE SYSTEM SHALL take its crate from its package
  dir and its outputs from its `out/`, and report it in copy groups, divergence and failed units as
  it reports a flat unit.
- **BUILDDIR-7.2** (guard) WHEN explain reads flat units THE SYSTEM SHALL CONTINUE TO report them as
  0.1.2 does.

## 8. Bench on Cargo 1.100

**Story:** As a developer on Rust 1.100, I want `cargo hive bench` to time cold and seeded first
builds, so that I can measure what seeding saves on my toolchain.

- **BUILDDIR-8.1** WHEN bench runs with an active toolchain whose check (f) recorded `unit-dir` THE
  SYSTEM SHALL prime, seed and time samples as it does on flat, with seeded samples placing unit-dir
  entries.
- **BUILDDIR-8.2** (guard) WHEN a checkout's build directory differs from its target directory THE
  SYSTEM SHALL CONTINUE TO refuse it in bench as today.

## 9. Quality attributes

**Section-kind:** nfr

**Story:** As a stakeholder, I want measurable quality targets for this feature, so that how-well is
not left implicit.

- **Performance:** **BUILDDIR-9.1** (agent-decided) WHEN the proof runs `cargo hive gc --checkout` on a
  copy of a klynt worktree built with Cargo 1.100 THE SYSTEM SHALL have its wall time recorded against
  CHECKOUTGC-7.1's 15 s target, under that criterion's proof conditions (scratch `HIVE_HOME`, the
  copy outside the secondary roots). This is a measured target, not a pass/fail bound; a run past it
  reopens the read design. Timed outside hive (ARCH-10).
- **Security:** None — the feature adds no trust boundary: the same local files under the same locks,
  read without child processes beyond those hive already starts.
- **Reliability:** **BUILDDIR-9.2** After `gc --checkout` and after `apply` on that klynt copy, a
  rerun of the same cargo test build SHALL recompile no unit because of the gc (0 gc-caused
  recompiles, as CHECKOUTGC-1.4), and after seed a first build SHALL reuse every placed unit.
- **Reliability:** **BUILDDIR-9.3** The full test suite SHALL pass under rustc 1.97.1 (flat) and
  under rustc 1.100.0 (unit-dir). Verified by the CI run on the merge commit and by `main`'s branch
  protection listing both gate jobs as required checks.
- **Accessibility:** None — headless CLI; output stays the existing text and JSON receipts.

## Files the change touches, and their guards

- `crates/core/src/fingerprint.rs`, `target.rs`, `liveness.rs`, `locks.rs`: BUILDDIR-1.10.
- `crates/gc/src/liveness.rs`, `gc/run.rs`, `plan_tier/build.rs`, `apply.rs`, `guard.rs`:
  BUILDDIR-3.7, 3.8, 5.2.
- `crates/gc/src/checkout.rs`: BUILDDIR-4.5. `crates/gc/src/debug_map.rs`: no behavior to guard
  (layout-free; it reads one file).
- `crates/gc/src/explain.rs`: BUILDDIR-7.2.
- `crates/seed/src/*.rs`, `crates/state/src/lib.rs`: BUILDDIR-6.6, 6.7.
- `crates/doctor/src/lib.rs`: BUILDDIR-2.5.
- `crates/cargo-hive/tests/fixtures/`: no behavior to guard; the v0.1.0 to v0.1.2 fixtures are read,
  never rewritten (UPGRADE).
- `crates/cargo-hive/src/bench/`: BUILDDIR-8.2. `crates/cargo-hive/src/cli.rs`: BUILDDIR-1.10
  (status lines of flat profiles keep their text apart from the layout name of 1.8).
- `.github/workflows/ci.yml`, `scripts/gate.sh`: BUILDDIR-9.3 replaces the 1.97.1-only required gate;
  the non-blocking latest-stable job stays.
- `docs/architecture/INDEX.md` (ARCH-18 restated for both layouts), `docs/adr/` (a new ADR beside
  ADR-0001), `CONTEXT.md` (unit, unit hash, stamp, dead unit, debug object, linked binary, entry):
  no behavior to guard.

## Out of Scope

- Cargo layouts other than the flat one and the 1.100 unit-dir one; any later Cargo change is
  refused as unsupported, as today.
- Converting a target, a store entry or a seed record from one layout to the other.
- Seeding a checkout whose toolchain doctor has not checked.
- Deleting the flat units Cargo 1.100 leaves behind in a mixed profile dir by any rule other than
  closure liveness (they are judged like any flat unit).
- Linux; build directories outside the target in bench (BUILDDIR-8.2).
- Changing which cargo the test fixtures run beyond choosing the toolchain for the CI matrix.

## Open Questions

None.
