# Design: Worktree scorecard

Feature code: SCORECARD
Status: Draft
Date: 2026-11-22
Requirements: ./requirements.md

## Context

Hive's only cost figure today is one bench receipt from 2026-11-11, published in README.md:80-92
with two samples per arm and no hive commit. `cargo hive bench` already times first builds the
way a scorecard needs: five compile-only shapes, fresh caller-made worktrees, ABBA order, and
integer-ms medians in its receipt (crates/cargo-hive/src/bench/command.rs:27-34,
bench/mod.rs:578-599). `cargo hive seed` reports a clone's private bytes
(crates/seed/src/lib.rs:396-460). `cargo hive gc --checkout` reports what it deleted
(crates/cargo-hive/src/cli.rs:939-946). No doc gathers these into rows with targets, and no
doc says how to measure them again. The touched surface and receipt shapes are mapped in
`.skills/_pending-worktree-scorecard/auto/design-scan.md` and `scan.md`, which sit outside
the repo.

The binding constraint is that this item measures and changes nothing hive does (ROAD-19.md
Off limits; #16 Out of scope "Changing bench's own rules or receipts"). Bench times exactly one
first build per fresh checkout and refuses a checkout whose target dir exists
(bench/check.rs:154-162). That leaves R6, the next build after an edit, to be timed by hand,
which the operator allowed on 2026-11-22. It also rules out the obvious alternative: extending
bench with an edit-and-rebuild sample, or adding a `scorecard` command that runs everything and
writes the table. A second constraint is Weigh W1: a doc only, with no test or driver script,
because a gated test would widen the document test tier (docs/standards/testing.md:25).

So the design is one markdown file with two parts. The first is a table of rows. The second is
a Rerun section of copy-paste zsh blocks that drive the shipped commands and extract their
counts with `/usr/bin/jq`, which macOS ships, so no dependency is added. Two items stay open:
O1, the target values, and O2, whether the doc may name the measured workspace. The design
keeps every workspace-specific name in a local, uncommitted file until O2 is answered.

Fresh retrieval (load-subgraph, schema 1.1, SCORECARD seed) found these neighbours: BENCH,
SEED and CHECKOUTGC (the commands the steps drive), BUILDDIR (toolchain layout), and LIVE (the
7-day window behind R8 vacuity). owns_coverage is 14 of 15, because SCORECARD has no tasks.md
yet. There are no observations.

## Decisions

1. **One file, two tables.** `docs/scorecard.md` holds a **Values** table and a **Provenance**
   table, both keyed by row ID R1–R10. A single table with ~15 columns is unreadable in a
   terminal. Splitting at the row ID keeps L4: one row per ID in each table.
2. **Every workspace-specific value lives in a local env file.** The steps read the main
   checkout path, `P`, `FILE`, the probe file, and any variable the workspace needs to build
   offline from `.skills/verify/scorecard.env` (gitignored, `.gitignore:2`). The doc states only
   the property each value must have. This leaves O2 open (agent-decided, ledger A3). When the
   operator answers O2 "yes", the values may move into the doc.
3. **One crate, `P`, serves R5, R6 and R9.** `P` is the workspace crate whose proc macro reads
   `.sqlx/*.json` (or `.env`) without declaring it, so the three rows measure the same crate
   (agent-decided, ledger A14).
4. **The R6 edit is fixed.** Append with `printf '\n/// Scorecard edit.\n#[allow(dead_code, unreachable_pub)]\npub const SCORECARD_EDIT: u32 = 1;\n' >> "$FILE"`.
   The leading newline, the doc comment and the allow keep it compiling under `deny(missing_docs)`
   and `unreachable_pub`. `FILE` is an `.rs` file in `P`. Restore it with `git checkout -- "$FILE"`. A public item changes
   `P`'s metadata, so its dependents rebuild as they would after an agent's real edit
   (agent-decided, ledger A15).
5. **The R9 probe change is fixed.** Flip the first `"nullable"` entry of the probe file's
   `describe` section (`false`↔`true`), which changes the type the macro emits. Then rebuild, and
   restore with `git checkout --`. The sample is decided by SCORECARD-6.2:
   - a control rebuild just before the change must exit 0 with no rustc invocation;
   - after the change, a rerun that recompiles `P` is not stale;
   - exit 0 with `Fresh P v` is stale;
   - anything else is void.

   Agent-decided, ledger A16; design review finding 2. Where the workspace has no `.sqlx`, the `.env` fallback changes
   one variable the macro reads.
6. **By-hand medians use bench's rule.** Odd n takes the middle value. Even n takes
   `floor((a+b)/2)` of the two middle values (bench/stats.rs:18-24), so a by-hand median and a
   bench median are computed the same way.
7. **Provenance cells are read, never typed:**
   - Toolchain is the receipt's top-level `.toolchain`.
   - The hive commit is `git rev-list -n1 v<hive_version>` for a crates.io install, or the
     source checkout's `git rev-parse HEAD` for `cargo install --path`.
   - Machine is `sysctl -n hw.model machdep.cpu.brand_string hw.ncpu`.
8. **No ADR.** No decision here is hard to reverse. The one surprising rule, a labelled by-hand
   duration in a doc, is the operator's own amendment of ROAD-19 (e0318de). It is not an
   architecture choice, and the vision stays unedited (charter).
9. **The main is built with plain `cargo`, never through `hive_env_cargo`.** An empty
   `RUSTC_WRAPPER` makes Cargo skip `.rustc_info.json`, and fill learns its toolchain row from
   that file (bench/mod.rs:149-171; seed/fill.rs:893). Built that way, the main gives
   `source.none` and nothing gets seeded. The main build therefore runs `cargo` with
   `RUSTC_WRAPPER` and `RUSTC_WORKSPACE_WRAPPER` unset, not empty, in the main checkout, and
   preflight checks that `.rustc_info.json` exists in the main's target dir (design review
   finding 1).
10. **Run order is R1, then the R6–R9 pipeline, then R2–R5.** The store and the main's target
   keep every shape built so far, so a later seed sees more units. Running the pipeline right
   after R1 keeps its seeds to `build` units. R2–R5 are noted as measured after earlier shapes
   (design review finding 8).
11. **A run cleans up after itself.** One `trap` on EXIT/INT removes every worktree the run made
   (`git worktree remove --force`), then runs `git worktree prune`. Worktrees are made with
   `git worktree add --detach <wt> HEAD` from a main whose tracked tree is clean (bench refuses
   otherwise, check.rs:117-170).

Spine invariants this feature relies on:
- **ARCH-10:** hive's own reports carry only bench durations; the doc labels its one exception.
- **ARCH-11:** private bytes are bytes the volume lost, never clone-shared bytes.
- **ARCH-5:** the steps seed only secondary checkouts.
- **ARCH-13:** the by-hand environment mirrors the scrub hive applies to its children.
- **ARCH-1:** R10 counts what hive installs: nothing in the build path.

## Architecture

There are no code modules. Each section below is one area of `docs/scorecard.md`, or the spec
bookkeeping around it.

### Values and Provenance tables

Satisfies: SCORECARD-1.1, SCORECARD-1.2, SCORECARD-1.3, SCORECARD-1.4, SCORECARD-1.5, SCORECARD-1.6, SCORECARD-1.7, SCORECARD-1.8, SCORECARD-1.9, SCORECARD-10.1
Reuse: rung 2 — the README "Measured results" table convention (README.md:78-99) and the dated-evidence rule (docs/standards/code.md:20); the table itself is new text
Respects: ARCH-10
Interface: a reader sees, per row ID, the metric, its Source, and for each arm the n and median, plus Target (cold), Target (seeded) and Target set. The Provenance table gives, per row ID, Measured on, Hive commit, Toolchain, Machine and Evidence
Depth: if the file vanished, a reader would need only the ten row names and the column list to rebuild it; every value is regenerated by the Rerun section
Locality: new file `docs/scorecard.md`; leave README.md and the MILE assessments untouched

Layout, top to bottom:
- a one-paragraph header ("what an agent worktree costs, cold and seeded, on the measured
  workspace");
- the **Values** table;
- the **Provenance** table;
- **Notes per row**: R6 "by hand (outside bench); environment matches bench's", the R7 seed
  bytes, the R8 vacuity rule, the R9 registry-only seed note;
- **Rerun**.

Cell vocabulary, fixed so a rerun only swaps values:
- `not yet measured`
- `open — operator`
- integer ms with no unit suffix (the column header says `ms`)
- bytes as an integer with the header saying `bytes`
- R8 as `R rebuilt over k non-vacuous samples; m vacuous` or `all vacuous`
- R9 as `s stale of n`

Values-table columns: `Row | Metric | Source | Cold n | Cold median | Seeded n | Seeded median |
Target (cold) | Target (seeded) | Target set`.

R10's n and median cells read `—`, and its value goes in the Seeded median and Cold median cells
as list text: Seeded holds the list, and Cold holds `nothing`.

SCORECARD-10.1 is met by the env-file split in Decision 2. The doc's text never contains
`/Users/`, `/private/`, a user name or a host name. The Rerun blocks refer to `$MAIN` and `$WT`.

### Rerun preflight

Satisfies: SCORECARD-8.1, SCORECARD-8.2, SCORECARD-8.3
Reuse: rung 2 — the shipped CLI (`cargo hive doctor`, `cargo hive repo add`, `cargo hive bench --help`; crates/cargo-hive/src/cli.rs:75-90)
Interface: one block. It sources `.skills/verify/scorecard.env`, then checks `cargo hive doctor --json` passed and that `$MAIN` is registered. It then greps `cargo hive bench --help` for each quoted flag (`--cold`, `--seeded`, `--build-cmd`, `--json`) and stops on the first missing one, naming the step to fix in the same commit
Depth: callers keep knowing only "source the env file, then run preflight"; the flag list is the one thing to update when bench changes
Locality: new text in `docs/scorecard.md` § Rerun; leave bench

The block also:
- confirms `rustup component list --installed` lists `clippy` (R3);
- confirms `$TMPDIR` is a secondary root: `secondary_roots` is absent from
  `~/.hive/config.json`, so the defaults hold, or it lists `$TMPDIR`;
- confirms the main's tracked tree is clean (`git -C $MAIN status --porcelain --untracked-files=no` is empty);
- after R1's main build, checks that `<main target>/.rustc_info.json` exists (Decision 9);
- runs a drift guard: greps the hive source at the measured commit (`crates/cargo/src/env.rs`
  `DROP_EXACT`, `TARGET_VARS`) for each variable `hive_env_cargo` drops, and stops on any
  name the copy lacks;
- sets `RUN=.skills/verify/$(date +%F)-scorecard` (`mkdir -p`);
- asks for a quiet machine, with no other builds running;
- records `sysctl -n vm.loadavg` into `$RUN/load-start.txt`.

Every later block writes its receipts and logs under `$RUN`.

**What a run touches**, written as a note in the doc:
- the real `~/.hive`: store, `store.db` and receipts, from bench's prime, `seed` and `gc`;
- the main checkout's target dir;
- `~/.cargo`, which only reads its cache, since every build is `--offline`;
- `$MAIN/.git/worktrees/`;
- `$TMPDIR/scorecard/`.

Nothing else is touched. Untracked files the workspace needs to build (an `.env`) are copied
into each worktree from the env file's list.

### Bench rows R1–R5

Satisfies: SCORECARD-2.1, SCORECARD-2.2, SCORECARD-2.3, SCORECARD-2.4
Reuse: rung 2 — `cargo hive bench --build-cmd <shape> --json` as shipped (bench/command.rs:27-34; receipt keys bench/mod.rs:578-599)
Respects: ARCH-10, ARCH-5
Interface: one zsh function `bench_row <row> <shape>`, defined in the doc. For each shape it:
- builds the main once with the bench line for that shape, with plain `cargo` (Decision 9), so
  the store holds its units (bench.nothing_to_seed otherwise; bench/mod.rs:326-333);
- runs `git worktree add` 8 times under `$TMPDIR/scorecard/<row>/{cold,seeded}{1..4}`, at
  `$MAIN`'s HEAD;
- runs `cargo hive bench --cold … --seeded … --build-cmd <shape> --json > $RUN/<row>.json`;
- on exit 0, prints the cells with jq: `.counts.cold_build_ms_median`,
  `.counts.seeded_total_ms_median`, `.counts.seeded_seed_ms_median`,
  `.counts.seeded_build_ms_median` and `.toolchain`;
- on non-zero exit, prints `kept previous values`, with the refusal finding copied to
  `$RUN/<row>.refusal.json`;
- runs `git worktree remove --force` for all 8.
Depth: n/a — extends `cargo hive bench`; the doc adds only worktree setup and jq extraction
Locality: new text in `docs/scorecard.md` § Rerun; leave crates/cargo-hive/src/bench/

The R5 shape is `check-lib=$P`; `$P` must be a package name, matching `[A-Za-z0-9_-]+`
(command.rs:34-46). The rows run in the order of Decision 10. Each row's main build comes before
its bench run.

### By-hand pipeline R6–R9

Satisfies: SCORECARD-3.1, SCORECARD-3.2, SCORECARD-3.3, SCORECARD-3.4, SCORECARD-3.5, SCORECARD-4.1, SCORECARD-4.2, SCORECARD-4.3, SCORECARD-4.4, SCORECARD-5.1, SCORECARD-5.2, SCORECARD-5.3, SCORECARD-6.1, SCORECARD-6.2, SCORECARD-6.3, SCORECARD-6.4
Reuse: rung 2 — `cargo hive seed <checkout> --json` (cli.rs:68-73; seed/lib.rs:396-460) and `cargo hive gc --checkout <path> --json` (cli.rs:46-57, 939-946); rung 4 — zsh `zsh/datetime` + `zsh/mathfunc`, `df -k`, `/usr/bin/jq`, `git worktree`
Respects: ARCH-11, ARCH-13, ARCH-5
Interface: two zsh functions defined in the doc.

`hive_env_cargo <args…>` runs cargo under the environment `timed_build` builds
(crates/cargo/src/env.rs:114-156):
- `TC` is the toolchain name, computed in a subshell `cd $WT` with `RUSTUP_TOOLCHAIN` and
  `RUSTUP_TOOLCHAIN_SOURCE` unset, so the worktree's `rust-toolchain.toml` decides (env.rs:14-47).
  It is `basename` of the toolchain dir when `rustup which rustc` lies under
  `…/toolchains/<name>/bin/`, else empty, the linked-toolchain case (toolchain.rs:118-122);
- `env -u CARGO_TARGET_DIR -u CARGO_BUILD_TARGET_DIR -u CARGO_BUILD_BUILD_DIR -u CARGO_MANIFEST_DIR -u OUT_DIR -u CARGO_BUILD_RUSTC -u CARGO_BUILD_RUSTC_WRAPPER -u CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER -u RUSTUP_TOOLCHAIN -u RUSTUP_TOOLCHAIN_SOURCE`, plus each `GIT_*` and `CARGO_PKG_*` variable present;
- `RUSTC_WRAPPER= RUSTC_WORKSPACE_WRAPPER= RUSTUP_AUTO_INSTALL=0 RUSTC=$(rustup which rustc)`,
  plus `RUSTUP_TOOLCHAIN=$TC` only when `$TC` is non-empty (env.rs:152-155);
- the command itself: `$(rustup which cargo) <args…>`.

`pipeline <arm> <i>` runs once per worktree, in this order:
1. `git worktree add` at `$MAIN`'s HEAD, into `$TMPDIR/scorecard/pipe/<arm><i>`.
2. Free-space reading A: `sync; /bin/df -k $WT | awk 'NR==2{print $4}'`. Use `/bin/df`, because
   a coreutils `df` on PATH changes the columns.
3. Seeded arm only: `cargo hive seed $WT --json > $RUN/pipe-<arm><i>-seed.json`, then read
   `.counts.clone_bytes_not_shared`, or its `.unmeasured[]` reason. The worktree is void
   (SCORECARD-4.4) when `.counts.clone_units_cloned` is 0 or a `source.none`,
   `toolchain.missing` or `store.tag` finding appears.
4. First build: `hive_env_cargo build --offline --locked`.
5. Free-space reading B. R7 sample = (A − B) × 1024.
6. Apply the Decision 4 edit, then time `hive_env_cargo build --offline --locked` with
   `$EPOCHREALTIME` → R6 sample, as `int(rint(ms))`. A non-zero exit voids R6–R9 for this
   worktree (SCORECARD-3.5).
7. `cargo hive gc --checkout $WT --json > …-gc.json`, then read `.counts.unit_files_deleted` and
   `.counts.objects_deleted`. Rerun the build with `-v`, keeping stderr, and count the lines
   matching ``Running `.*--crate-name``, one per unit compiled. That gives the R8 sample:
   - vacuous when both totals are 0;
   - void when gc or the rerun exits non-zero.
8. Control rebuild: `-v`, which must exit 0 with no rustc invocation. Then apply the Decision 5
   probe and rebuild with `-v`, deciding the sample by SCORECARD-6.2 (`Fresh $P v` is the
   stale match). Restore the probe file and `FILE`.
9. `git worktree remove --force`.

Each sample is appended as one JSON line to `$RUN/pipe.jsonl`, with `void` and its reason when
voided.

R7 noise is stated in the row note. The free-space drop also carries:
- the seed's store-fill writes to `~/.hive`;
- any `~/.cargo` source unpacks;
- any other writer on the APFS container.

The quiet-machine rule and the median across 4 samples are the only guards.

Depth: callers know only "run `pipeline cold 1..4` and `pipeline seeded 1..4`, then `summarize`". The ordering inside is the part that would have to be rebuilt
Locality: new text in `docs/scorecard.md` § Rerun and § Notes per row (3.3, 6.4); leave crates/cargo, crates/seed, crates/gc

Order inside one worktree is first build → R7 → edit (R6) → gc (R8) → probe (R9). Each step's
input is the state the previous step left, and no step reuses a bench worktree (3.1). The 4
cold and 4 seeded pipelines alternate cold/seeded/seeded/cold, as bench's ABBA order does, so
drift falls on both arms.

### Recording a run

Satisfies: SCORECARD-8.4, SCORECARD-8.5, SCORECARD-8.6, SCORECARD-8.7, SCORECARD-8.8, SCORECARD-9.1, SCORECARD-9.3
Reuse: rung 2 — the evidence convention `.skills/verify/<date>-<feature>/` (docs/agents/verify.md:59-66)
Interface: `summarize` prints one line per row ID, giving the arm's n and median under Decision 6, or the R8/R9 cell text, from `$RUN/*.json` and `$RUN/pipe.jsonl`. It also prints the Provenance cells under Decision 7
Depth: n/a — a jq/awk reduction over files the earlier blocks wrote
Locality: new text in `docs/scorecard.md` § Rerun; edits only existing cells of the two tables

The "Record" step tells the person to:
- overwrite only the value, n, median and Provenance cells of the existing row IDs, never adding
  a row;
- leave `Target (cold)`, `Target (seeded)` and `Target set` exactly as they are;
- write `$RUN`'s relative path into each Evidence cell;
- confirm every worktree from the run is gone in `git worktree list` (the Decision 11 trap
  removes them on abort too);
- keep a row's previous cells when an arm has fewer than 3 valid samples, with the void reasons
  printed by `summarize` (SCORECARD-8.8).

O2 gate (9.3): while O2 is unanswered, the Record step stops at printing. The numbers stay in
`$RUN`, and the commit for a run is made only once O2 is answered. The first run (9.1) replaces
nothing in README.md. The README headline stays history, and R1 holds the new medians.

### Setup footprint R10

Satisfies: SCORECARD-7.1
Reuse: rung 2 — README.md install/uninstall (README.md:101-115, :272, :324) and ARCH-1/ARCH-2
Respects: ARCH-1
Interface: R10's Seeded cell is a fixed list:
- the commands `cargo install cargo-hive`, `cargo hive doctor`, `cargo hive repo add <main>`;
- "Cargo config files 0, environment variables 0, rustc wrappers 0, cargo shims 0" (ARCH-1);
- "files outside the state root: 0, beyond clones into target dirs" (ARCH-2).

The Cold cell reads `nothing`.
Depth: n/a — restates shipped install text as one cell
Locality: new text in `docs/scorecard.md`; leave README.md

The Rerun section re-checks this list against README's install section on each run. A changed
install step is fixed in the same commit, under the same rule as SCORECARD-8.2.

### Spec bookkeeping and guards

Satisfies: SCORECARD-9.2, SCORECARD-11.1, SCORECARD-11.2, SCORECARD-11.3
Reuse: rung 2 — the catalog shard row format (docs/specs/catalog/hive.md), the CONTEXT.md term format, `scripts/gate.sh`
Surface:
- `docs/specs/catalog/hive.md`: readers are `audit-trace`, `load-subgraph` and `refresh-lanes`. The SCORECARD row was added on this branch (commit 86993b9); build stamps only its Status — **replace** (the readers parse the row in this change).
- `docs/roadmap/INDEX.md`, `docs/roadmap/roads/ROAD-19.md`, `docs/roadmap/lanes.md`: readers are `refresh-lanes`, `refresh-roadmap-status` and `plan-milestones`. Build and land stamp In-progress or Delivered-by, then run `refresh-lanes` — **replace**.
- `docs/specs/INDEX.md`: domain router only, which registers no codes, so no edit — **replace** n/a (untouched).
- `CONTEXT.md`: human readers. Two terms appended — **replace**.
- `README.md` "Measured results": human readers — **frozen**. The design builds around it and leaves it untouched (9.2).
- `docs/roadmap/assessments/MILE-*.md`: human readers — **frozen**, untouched (9.2).

Interface: none new
Depth: n/a — extends the catalog and glossary
Locality: `docs/specs/catalog/hive.md` (+1 row), `CONTEXT.md` (+2 terms); leave crates/, plugin/, scripts/

## Seams for testing

No automated test is added (Weigh W1). Each seam below is a check a person or the build stage
runs and records in evidence.

| Seam | Kind | Covers |
|---|---|---|
| `docs/scorecard.md` text, by fixed commands. `grep -c '^| R[0-9]' ` per table = 10. Each `R1`…`R10` appears exactly once per table. The grep of 10.1's patterns finds nothing. Target cells read `open — operator` | manual (inspection) | SCORECARD-1.1–1.9, 10.1, 8.4, 8.5 |
| Dry run of the Rerun blocks with `$MAIN` set to a fresh `git clone` of this repo under `/private/tmp`, a scratch `HIVE_HOME`, and worktrees under `/private/tmp`. No real checkout and no `~/.hive` is touched. Deps must already be cached, since every build is `--offline`. R9 records "probe input absent", since this repo has no `.sqlx` | e2e (manual) | SCORECARD-2.1–2.4, 3.1–3.5, 4.1–4.4, 5.1–5.3, 6.3, 8.1–8.3, 8.6–8.8 |
| First run on the measured workspace, from its main checkout, with evidence in `$RUN` | e2e (manual) | SCORECARD-6.1, 6.2, 6.4, 7.1, 9.1, 9.3 |
| `scripts/gate.sh` green, and `git diff --stat main` lists no path under crates/, plugin/ or scripts/ | integration (existing gate) | SCORECARD-11.3 |
| `audit-trace` over docs/specs, and `git diff main -- README.md docs/roadmap/assessments CONTEXT.md docs/specs/catalog/hive.md` shows only the added row and terms | manual (inspection) | SCORECARD-9.2, 11.1, 11.2 |

## Coverage check

Every ID in requirements.md appears in exactly one Satisfies line:
- Values and Provenance tables: 1.1–1.9, 10.1
- Rerun preflight: 8.1–8.3
- Bench rows: 2.1–2.4
- By-hand pipeline: 3.1–3.5, 4.1–4.4, 5.1–5.3, 6.1–6.4
- Recording a run: 8.4–8.8, 9.1, 9.3
- Setup footprint: 7.1
- Spec bookkeeping: 9.2, 11.1–11.3

No ID is unmapped.

Requirements changed by this design, synced back after design review (design-review.md):
- SCORECARD-1.4: n counts valid samples only.
- 3.5 (new): a failed R6, gc or rerun voids the rest of that worktree's pipeline.
- 4.4 (new): an unseeded seeded worktree is void.
- 5.1: count rustc invocations with `-v`, not `Compiling` lines, which are per package.
- 6.2: stale needs exit 0 and `Fresh`, with a control rebuild first.
- 8.7: clean up on abort too, then `git worktree prune`.
- 8.8 (new): an arm with fewer than 3 valid samples keeps the row's previous cells.
