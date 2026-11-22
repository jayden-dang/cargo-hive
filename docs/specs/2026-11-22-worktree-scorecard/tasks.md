# Tasks: Worktree scorecard

> **For agentic workers:** after plan approval, pick one execute skill:
> - `build` (wave mode): a continuous, dependency-aware scheduler;
> - `build` (story mode): human-gated story review units;
> - `build` (inline mode): the controller implements, with no implementer subagents.
>
> The chosen skill writes `Execution-mode:`. Steps use checkbox (`- [ ]`) syntax for tracking.

Feature code: SCORECARD
Status: Approved
Approved: 2026-11-22 12:46 by the operator (Jayden), "approve" on the spec sheet for #16 at feat/worktree-scorecard 226a925, relayed by the conductor (/Users/jayden/Developer/experiments/cargo-zip/.skills/_pending-worktree-scorecard/auto/operator-reply.md). The reply approves the sheet as a whole; the Open Questions O1 and O2 stay open as listed, and Task 9 still waits for the operator's own go-ahead.
Date: 2026-11-22
Execution-mode: continuous
Max-concurrency: auto
Requirements: ./requirements.md
Design: ./design.md

**Completion:** Tasks 1–8 and 10 land the scorecard and its steps. #16 is complete only after Task 9, the operator-gated first run, so a landing before Task 9 leaves #16 open (ledger S3).

**Goal:** Land `docs/scorecard.md`. It records an agent worktree's cost on the measured workspace,
cold and seeded, in ten rows with open target slots, and its copy-paste Rerun steps replace the
numbers in place.

**Architecture:** One markdown file holds a Values table, a Provenance table, notes per row, and a
Rerun section of zsh blocks. The blocks drive the shipped `cargo hive bench`, `cargo hive seed`
and `cargo hive gc --checkout`, plus by-hand cargo builds in bench's environment, and reduce
their JSON with `/usr/bin/jq`. Nothing in `crates/`, `plugin/` or `scripts/` changes.

**Tech Stack:** Markdown; zsh (with `zsh/datetime` and `zsh/mathfunc`); macOS `/usr/bin/jq`
1.7.1, `/bin/df`, `sysctl`; git worktrees; the shipped `cargo hive` CLI. No test framework: every
check is a fixed command run by hand, and its output goes to evidence (Weigh W1).

## Global Constraints

- Standards: `docs/standards/INDEX.md` (sha256 80cd41328c0f…). In particular:
  - `docs/standards/code.md` (62576fe2b798…): "Measured claims", "Dependencies", "Requirement IDs";
  - `docs/standards/testing.md` (50a53d5f3c86…): the merge gate, and the document tier that
    stays unwidened.
- Architecture spine: `docs/architecture/INDEX.md` (af3a1ac71995…). Every task respects:
  - ARCH-10: durations in hive's reports come only from bench; the doc labels R6 as by hand;
  - ARCH-11: bytes counted by what the volume lost;
  - ARCH-5: seed only secondary checkouts;
  - ARCH-13: the child environment scrub, mirrored by `hive_env_cargo`;
  - ARCH-1: nothing installed in the build path.
- Commands: verify commands and merge gate in `docs/agents/project.md` "verify commands"
  (`scripts/gate.sh`). The evidence convention is `docs/agents/verify.md` "Evidence"
  (`.skills/verify/<date>-<feature>/`, gitignored).
- Feature-wide rules:
  - No file under `crates/`, `plugin/` or `scripts/` changes (SCORECARD-11.3).
  - No requirement ID appears in `docs/scorecard.md` (code.md "Requirement IDs" keeps IDs in the
    spec).
  - No absolute path, user name or host name appears in `docs/scorecard.md` (SCORECARD-10.1).
  - No target value is written by anyone but the operator (O1).
  - The measured workspace is not named, and nothing measured from it is committed, until O2 is
    answered (SCORECARD-9.3).
  - Dry runs use a scratch `HIVE_HOME` and paths under `/private/tmp` only. Every build is
    `--offline`, so nothing is fetched into `~/.cargo`.
- Team: Solo (docs/agents/project.md `## Team`); no assignees.

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `docs/scorecard.md` | Create | The scorecard: Values table, Provenance table, notes per row, what a run touches, Rerun blocks (preflight, `bench_row`, `resolve_toolchain`, `hive_env_cargo`, `pipeline`, `summarize`, Record) |
| `docs/specs/catalog/hive.md` | Modify | The SCORECARD row's Status: Draft → In-progress at build start, Implemented once Task 8's dry run passes |
| `docs/roadmap/lanes.md` | Regenerate | Written only by `refresh-lanes` after each Status stamp. On this branch it prints `not on main` and writes nothing. The Delivered-by line in `docs/roadmap/INDEX.md` belongs to landing (`land-branch`), not to this plan |
| `docs/specs/2026-11-22-worktree-scorecard/tasks.md` | Modify | Checkboxes ticked during build |

Evidence, outside git: `.skills/verify/<date>-scorecard/` (runs), `.skills/verify/scorecard.env`
(workspace values), and `/private/tmp/sc-dry/` (dry-run clone, scratch `HIVE_HOME`, worktrees).

Retrieval for this map (load-subgraph):
- blast_radius: `docs/specs/catalog/hive.md` is owned by AUTOGC and SEED, both Shipped. `CONTEXT.md`
  is owned by CORE, SEED, CHECKOUTGC, GCPLAN and BUILDDIR, none in flight. `docs/scorecard.md` has
  no owner.
- cluster(SCORECARD): the focus only.
- owns_coverage is 14 of 15 before this file.

Dry-run harness, used in Tasks 3–8. Write it once to `/private/tmp/sc-dry/dry.env`:

```zsh
export REPO=<this repo's root> DRY=/private/tmp/sc-dry
export HIVE_HOME=$DRY/hive TMPDIR=$DRY/tmp
[[ $HIVE_HOME == /private/tmp/* && $TMPDIR == /private/tmp/* ]] || { print -u2 "dry run refused: HIVE_HOME/TMPDIR not under /private/tmp"; exit 1; }
mkdir -p $HIVE_HOME $TMPDIR
export PATH=$REPO/target/debug:$PATH
[[ $(command -v cargo-hive) == $REPO/target/debug/cargo-hive ]] || { print -u2 "dry run refused: cargo-hive is not the dev build"; exit 1; }
export MAIN=$DRY/main
```

Before the first check, run `cargo build -p cargo-hive` in `$REPO` and `git clone "$REPO" "$DRY/main"`.
The dev build is needed because installed 0.1.2 fails doctor check (f) on Cargo 1.100 (plan-review
F8). Every check below runs as `zsh -ec 'source /private/tmp/sc-dry/dry.env; source $DRY/rerun.zsh; …'`,
one shell per check, so a missing or wrong harness fails closed before any `cargo hive` command.
`$DRY/rerun.zsh` holds every zsh block under `## Rerun`, extracted fresh for each check:
`awk '/^## Rerun/,0' docs/scorecard.md | awk '/^```zsh/{f=1;next}/^```/{f=0}f' > $DRY/rerun.zsh`.
A check whose step needs a block also requires `[ -s $DRY/rerun.zsh ]` and `whence <function>` to
succeed (plan-review F3, P1).

---

### Task 1: The scorecard's tables, cells and notes

**Files:**
- Create: `docs/scorecard.md`
- Test: none (inspection commands below)

**Reuse:** rung 2 — the README "Measured results" table convention (README.md:78-99) and the dated-evidence rule (docs/standards/code.md:20); the table itself is new text

**Interfaces:**
- Consumes: none
- Produces:
  - row IDs `R1`…`R10`;
  - Values columns `Row | Metric | Source | Cold n | Cold median | Seeded n | Seeded median |
    Target (cold) | Target (seeded) | Target set`;
  - Provenance columns `Row | Measured on | Hive commit | Toolchain | Machine | Evidence`;
  - the cell vocabulary `not yet measured`, `open — operator`, `R rebuilt over k non-vacuous
    samples; m vacuous`, `all vacuous`, `s stale of n`;
  - the section headings `## Values`, `## Provenance`, `## Notes per row`,
    `## What a run touches`, `## Rerun`.

**Depends-on:** none

- [x] **Step 1: Write the failing check.** `test -f docs/scorecard.md && [ "$(grep -cE '^\| R([1-9]|10) \|' docs/scorecard.md)" = 20 ] && [ "$(grep -o 'open — operator' docs/scorecard.md | wc -l | tr -d ' ')" = 20 ] && grep -q '^## Targets' docs/scorecard.md && grep -q 'zsh on macOS' docs/scorecard.md && ! grep -nE '/Users/|/private/|SCORECARD-|\$HOME' docs/scorecard.md`. Run it and expect a non-zero exit, because the file is absent.
- [x] **Step 2: Write the file.** Write the header paragraph, then the Values and Provenance tables, with one line per R1–R10 in each and the Metric names from SCORECARD-1.1. Fill the cells:
  - Every measured value cell reads `not yet measured`, and every target slot reads `open — operator`.
  - Target set is empty.
  - Source cells: R1–R5 name `cold_build_ms_median` / `seeded_total_ms_median`; R6 reads `by hand (outside bench)`; R7 reads `by hand (outside bench): free space`; R8 reads `by hand (outside bench): cargo -v after gc --checkout`; R9 reads `by hand (outside bench): cargo -v after probe`; R10 reads `listed`.
  - R10's n and median cells read `—`.
  - Add `## Targets`: the operator sets a target by editing that row's Target cell and its Target set date in a plain commit, and a rerun never touches either.
  - State once, above `## Rerun`, that the Rerun blocks are zsh on macOS.
  - Add `## Notes per row` and `## What a run touches` as the design section "Values and Provenance tables" and "Rerun preflight" lay them out. The R6, R7 and R8 notes go in here; the R9 note is added in Task 7.
- [x] **Step 3: Run the check.** Run Step 1's command and expect exit 0. Also run `grep -cE '^\| R([1-9]|10) \|' docs/scorecard.md` and expect 20 (header rows do not match).
- [x] **Step 4: Commit.** `git add docs/scorecard.md && git commit -m "docs(scorecard): add the worktree scorecard's tables with open target slots"`

_Requirements: SCORECARD-1.1, SCORECARD-1.2, SCORECARD-1.3, SCORECARD-1.4, SCORECARD-1.5, SCORECARD-1.6, SCORECARD-1.7, SCORECARD-1.8, SCORECARD-1.9, SCORECARD-10.1_

---

### Task 2: The setup-footprint row

**Files:**
- Modify: `docs/scorecard.md` (the R10 line of the Values table, and one Rerun step)

**Reuse:** rung 2 — README.md install/uninstall (README.md:101-115, :272, :324) and ARCH-1/ARCH-2

**Interfaces:**
- Consumes: the `R10` line and `## Rerun` heading from Task 1
- Produces: R10's Seeded cell list, and Rerun step "Re-check the setup footprint"

**Depends-on:** Task 1

- [ ] **Step 1: Write the failing check.** `grep -E '^\| R10 \|' docs/scorecard.md | grep -q 'cargo install cargo-hive' && grep -E '^\| R10 \|' docs/scorecard.md | grep -q 'Cargo config files 0' && grep -q 'Re-check the setup footprint' docs/scorecard.md`. Run it and expect a non-zero exit.
- [ ] **Step 2: Write the row.** R10 Seeded holds the list from design "Setup footprint R10": the three commands; `Cargo config files 0, environment variables 0, rustc wrappers 0, cargo shims 0`; `files outside the state root: 0, beyond clones into target dirs`. R10 Cold reads `nothing`. Under `## Rerun`, add the step "Re-check the setup footprint": compare the list with README's install section, and fix a changed line in the rerun's own commit.
- [ ] **Step 3: Run the check.** Run Step 1's command and expect exit 0. Also read README.md:101-115 and confirm each listed command appears there.
- [ ] **Step 4: Commit.** `git commit -am "docs(scorecard): list the setup footprint hive needs before its numbers apply"`

_Requirements: SCORECARD-7.1_

---

### Task 3: Rerun preflight and cleanup trap

**Files:**
- Modify: `docs/scorecard.md` (`## Rerun` → `### 0. Preflight`)

**Reuse:** rung 2 — the shipped CLI (`cargo hive doctor`, `cargo hive repo add`, `cargo hive bench --help`; crates/cargo-hive/src/cli.rs:75-90)

**Interfaces:**
- Consumes: `## Rerun` heading (Task 1)
- Produces:
  - the env file `.skills/verify/scorecard.env`, with keys `MAIN`, `P`, `FILE`, `PROBE`,
    `SCORECARD_N` (default 4), `WORKSPACE_ENV`, `UNTRACKED_COPY`;
  - the shell variables `RUN` (`.skills/verify/$(date +%F)-scorecard`) and `WTS` (an array of
    worktree paths the trap removes);
  - the function `cleanup_worktrees`, set by `trap cleanup_worktrees EXIT INT`.

**Depends-on:** Task 1

- [ ] **Step 1: Write the failing check.** Write `$DRY/scorecard.env` with `MAIN=$DRY/main`, extract `$DRY/rerun.zsh`, then run `zsh -ec 'source /private/tmp/sc-dry/dry.env; [ -s $DRY/rerun.zsh ]; source $DRY/rerun.zsh; whence preflight; preflight'`. Expect a failure, because the block is absent and `rerun.zsh` is empty.
- [ ] **Step 2: Write the block.** Write it as design "Rerun preflight" lays out:
  - the block defines `preflight`;
  - source the env file;
  - resolve `TC` in `$MAIN` (as `resolve_toolchain` does), then run `RUSTUP_TOOLCHAIN=$TC cargo hive doctor --json`, so doctor records the measured workspace's toolchain (P2);
  - record `command -v cargo-hive` and `cargo hive --version` in `$RUN/hive.txt`;
  - if doctor fails, or the toolchain is unsupported, stop before any measurement. Write the failing checks to `$RUN/preflight-fail.txt` and change no cell (B3);
  - `$MAIN` is registered (`cargo hive repo add "$MAIN"` when not);
  - the `cargo hive bench --help` greps for `--cold`, `--seeded`, `--build-cmd`, `--json`;
  - the `clippy` component is installed;
  - the `secondary_roots` check, reading `${HIVE_HOME:-~/.hive}/config.json`, the state root hive itself reads (crates/state/src/lib.rs:184-189);
  - the main's tracked tree is clean;
  - the env.rs drift guard (repo path, or `$CARGO_HOME/registry/src/*/cargo-hive-cargo-<hive_version>/src/env.rs`);
  - `RUN` setup with `sysctl -n vm.loadavg > $RUN/load-start.txt`;
  - the `cleanup_worktrees` trap (`git -C $MAIN worktree remove --force` for each of `$WTS`, then `git -C $MAIN worktree prune`).

  Every quoted command line is written exactly as it runs, with placeholders only.
- [ ] **Step 3: Run the check.** Run Step 1's command and expect exit 0, with `$RUN/load-start.txt` present.
  - Then rename `--build-cmd` to `--build-cmdx` in a scratch copy of the block and rerun it. Expect it to stop with a message naming the flag.
  - Then run the block with `WTS=($DRY/x)` after `git -C $DRY/main worktree add --detach $DRY/x HEAD`, and send INT. Expect `git -C $DRY/main worktree list` to show only the main.
- [ ] **Step 4: Commit.** `git commit -am "docs(scorecard): add the rerun preflight and worktree cleanup trap"`

_Requirements: SCORECARD-8.1, SCORECARD-8.2, SCORECARD-8.3, SCORECARD-8.7_

---

### Task 4: Bench rows R1–R5

**Files:**
- Modify: `docs/scorecard.md` (`### 1. Bench rows`)

**Reuse:** rung 2 — `cargo hive bench --build-cmd <shape> --json` as shipped (bench/command.rs:27-34; receipt keys bench/mod.rs:578-599)

**Interfaces:**
- Consumes: `RUN`, `WTS`, `cleanup_worktrees`, env keys `MAIN`, `P`, `SCORECARD_N` (Task 3)
- Produces: the function `bench_row <row> <shape>`, which writes `$RUN/<row>.json`, or
  `$RUN/<row>.refusal.json` on a non-zero exit, and prints
  `<row> cold=<ms> seeded=<ms> seed=<ms> build=<ms> toolchain=<release>` or
  `<row> kept previous values`. It also documents the main build with plain `cargo`
  (`RUSTC_WRAPPER`/`RUSTC_WORKSPACE_WRAPPER` unset), and the `.rustc_info.json` check after
  R1's main build.

**Depends-on:** Task 3

- [ ] **Step 1: Write the failing check.** Extract `$DRY/rerun.zsh`. Then, in one harness shell, run `preflight` and `bench_row R1 build` on the dry-run clone with `SCORECARD_N=2`. Expect a failure, because the section is absent.
- [ ] **Step 2: Write the block.** Write it as design "Bench rows R1–R5" lays out:
  - the main build per shape with plain `cargo`;
  - after R1, `test -f "$(cargo metadata --format-version 1 --no-deps --manifest-path $MAIN/Cargo.toml | jq -r .target_directory)/.rustc_info.json"`;
  - `git -C $MAIN worktree add --detach` for `SCORECARD_N` cold and `SCORECARD_N` seeded under `$TMPDIR/scorecard/<row>/`, each added to `WTS`, with `UNTRACKED_COPY` files copied in;
  - `cargo hive bench --cold … --seeded … --build-cmd <shape> --json`;
  - the jq extraction of `.counts.cold_build_ms_median`, `.counts.seeded_total_ms_median`, `.counts.seeded_seed_ms_median`, `.counts.seeded_build_ms_median` and `.toolchain`;
  - the non-zero-exit path;
  - the worktree removal.

  Rows run in the order of design Decision 10: R1 here, R2–R5 after the pipeline. R5 uses `check-lib=$P`.
- [ ] **Step 3: Run the check.** Run Step 1's command and expect exit 0. `$RUN/R1.json` has `.counts.cold_build_ms_median` as an integer. The printed `seeded=` equals `jq .counts.seeded_total_ms_median`. `git -C $DRY/main worktree list` shows only the main.
  - Then rerun `bench_row R1 build` with an existing target dir planted in one worktree. Expect `R1 kept previous values` and `$RUN/R1.refusal.json` present.
- [ ] **Step 4: Commit.** `git commit -am "docs(scorecard): fill the first-command rows from one bench run per shape"`

_Requirements: SCORECARD-2.1, SCORECARD-2.2, SCORECARD-2.3, SCORECARD-2.4_

---

### Task 5: The by-hand pipeline through R7 and R6

**Files:**
- Modify: `docs/scorecard.md` (`### 2. By-hand pipeline`; the R6 and R7 notes)

**Reuse:** rung 2 — `cargo hive seed <checkout> --json` (cli.rs:68-73; seed/lib.rs:396-460) and `cargo hive gc --checkout <path> --json` (cli.rs:46-57, 939-946); rung 4 — zsh `zsh/datetime` + `zsh/mathfunc`, `df -k`, `/usr/bin/jq`, `git worktree`

**Interfaces:**
- Consumes: `RUN`, `WTS`, env keys `MAIN`, `P`, `FILE`, `SCORECARD_N`, `UNTRACKED_COPY` (Task 3)
- Produces:
  - the functions `resolve_toolchain` (sets `TC`, `RUSTC_BIN`, `CARGO_BIN`),
    `hive_env_cargo <args…>`, and `pipeline <arm> <i>` steps 1–6;
  - one JSON line per worktree appended to `$RUN/pipe.jsonl`, with keys `arm`, `i`,
    `r7_bytes`, `r7_clone_bytes_not_shared` (or `r7_unmeasured`), `r6_ms`, `void`
    (null or `{rows, reason}`);
  - the alternating run order cold1, seeded1, seeded2, cold2, ….

**Depends-on:** Task 4

- [ ] **Step 1: Write the failing check.** Extract `$DRY/rerun.zsh`. In one harness shell, run `preflight; bench_row R1 build` (Task 4, which builds the main), then `pipeline cold 1; pipeline seeded 1` on the dry-run clone, with `P=cargo-hive-core` and `FILE=crates/core/src/lib.rs`. Expect a failure, because the section is absent.
- [ ] **Step 2: Write the block.** Write it as design "By-hand pipeline R6–R9" steps 1–6 lay out:
  - `resolve_toolchain` in a subshell `cd $WT` with `RUSTUP_TOOLCHAIN` and `RUSTUP_TOOLCHAIN_SOURCE` unset, with the `…/toolchains/<name>/bin/` basename rule;
  - `hive_env_cargo` with the full `env -u` list from design and `RUSTUP_TOOLCHAIN=$TC` only when non-empty;
  - `sync; /bin/df -k` readings;
  - the seed and its void rule (non-zero exit, `clone_units_cloned` 0, `source.none` / `toolchain.missing` / `store.tag` findings);
  - the first build and its void rule;
  - the Decision 4 `printf` edit;
  - the `$EPOCHREALTIME` timing around `$CARGO_BIN` only, then `int(rint(ms))`;
  - the R6-failure void (R6, R8 and R9; R7 stands).

  Add the R6 note "by hand (outside bench); environment matches bench's, but bench did not time it" and the R7 noise note.
- [ ] **Step 3: Run the check.** Run Step 1's command and expect exit 0. `$RUN/pipe.jsonl` has 2 lines. The `r6_ms` values are integers, and the seeded line's `r7_clone_bytes_not_shared` is an integer. Environment check: with `CARGO_TARGET_DIR=/tmp/x` and `RUSTC_WRAPPER=sccache` exported, and `CARGO_BIN=/usr/bin/env` set for one call, `hive_env_cargo` prints `RUSTC_WRAPPER=`, `RUSTUP_AUTO_INSTALL=0` and `RUSTC=$RUSTC_BIN`, and no `CARGO_TARGET_DIR` line.
  - Then append a syntax error to `FILE` in place of the Decision 4 edit and rerun `pipeline cold 2`. Expect its line to carry `void.rows` = `["R6","R8","R9"]` and `r7_bytes` present.
- [ ] **Step 4: Commit.** `git commit -am "docs(scorecard): time the next build after an edit and read private bytes by hand"`

_Requirements: SCORECARD-3.1, SCORECARD-3.2, SCORECARD-3.3, SCORECARD-3.4, SCORECARD-3.5, SCORECARD-4.1, SCORECARD-4.2, SCORECARD-4.3, SCORECARD-4.4_

---

### Task 6: R8, units rebuilt after `gc --checkout`

**Files:**
- Modify: `docs/scorecard.md` (`pipeline` step 7; the R8 note)

**Reuse:** rung 2 — `cargo hive seed <checkout> --json` (cli.rs:68-73; seed/lib.rs:396-460) and `cargo hive gc --checkout <path> --json` (cli.rs:46-57, 939-946); rung 4 — zsh `zsh/datetime` + `zsh/mathfunc`, `df -k`, `/usr/bin/jq`, `git worktree`

**Interfaces:**
- Consumes: `pipeline`, `hive_env_cargo`, `$RUN/pipe.jsonl` (Task 5)
- Produces: `pipe.jsonl` keys `r8_units` (count of stderr lines matching ``Running `.*--crate-name``),
  `r8_unit_files_deleted`, `r8_objects_deleted` and `r8_vacuous` (true when both are 0); the
  void rule for a non-zero gc or rerun

**Depends-on:** Task 5

- [ ] **Step 1: Write the failing check.** Run `pipeline cold 1` on the dry-run clone, then `jq -e 'has("r8_units") and has("r8_vacuous")' <(tail -1 $RUN/pipe.jsonl)`. Expect a non-zero exit.
- [ ] **Step 2: Write step 7.** Run `cargo hive gc --checkout $WT --json > $RUN/pipe-<arm><i>-gc.json` and read `.counts.unit_files_deleted` and `.counts.objects_deleted` (unsuffixed totals). Run `hive_env_cargo build --offline --locked -v 2> …-r8.log` and count with `grep -c 'Running `.*--crate-name'`. Mark the sample vacuous or void as design step 7 says. Add the R8 note with its cell form.
- [ ] **Step 3: Run the check.** Run Step 1's command and expect exit 0. The line's `r8_unit_files_deleted` equals `jq .counts.unit_files_deleted` on that worktree's saved `-gc.json`, and `r8_units` equals `grep -c 'Running `.*--crate-name'` on its saved `-r8.log` (plan-review F4). On a scratch crate, `printf '\n// e\n' >> src/lib.rs` followed by the counter on `cargo build -v` prints 1, matching the 2026-11-22 check.
- [ ] **Step 4: Commit.** `git commit -am "docs(scorecard): count units rebuilt after a checkout gc, with vacuous samples kept apart"`

_Requirements: SCORECARD-5.1, SCORECARD-5.2, SCORECARD-5.3_

---

### Task 7: R9, the undeclared-input probe

**Files:**
- Modify: `docs/scorecard.md` (`pipeline` step 8; the R9 note)

**Reuse:** rung 2 — `cargo hive seed <checkout> --json` (cli.rs:68-73; seed/lib.rs:396-460) and `cargo hive gc --checkout <path> --json` (cli.rs:46-57, 939-946); rung 4 — zsh `zsh/datetime` + `zsh/mathfunc`, `df -k`, `/usr/bin/jq`, `git worktree`

**Interfaces:**
- Consumes: `pipeline`, `hive_env_cargo`, env keys `P`, `PROBE` (Task 5)
- Produces:
  - the function `r9_classify <exit> <log>`, which prints `stale`, `fresh-recompiled` or `void`
    (stale = exit 0 and a `Fresh $P v` line; recompiled = a ``Running `.*--crate-name ${P//-/_} `` line, the trailing space being the right boundary
    line; otherwise void);
  - `pipe.jsonl` key `r9`;
  - the R9 note that seed clones registry units and not workspace crates.

**Depends-on:** Task 6

- [ ] **Step 1: Write the failing check.** Write three canned logs into `$DRY/r9/`:
  - `stale.log`: `       Fresh p v0.1.0 (/x)`, exit 0;
  - `recompiled.log`: ``     Running `rustc --crate-name p src/lib.rs` ``, exit 1;
  - `void.log`: `error: failed to get ...`, exit 101;
  - `prefix.log`: ``     Running `rustc --crate-name p_core src/lib.rs` ``, exit 0, which must classify `void` for `P=p` and not `fresh-recompiled`.

  Run `P=p r9_classify 0 $DRY/r9/stale.log`, and the same for the other three. Expect a failure, because the function is absent.
- [ ] **Step 2: Write step 8.** Write the control rebuild (`-v`, which must exit 0 with no `--crate-name` line, else void), then the Decision 5 probe change on `$PROBE` (flip the first `"nullable"` with `jq`, or the `.env` fallback). Rebuild with `-v`, classify with `r9_classify`, then restore with `git checkout -- "$PROBE" "$FILE"`. Add the R9 note. Where `PROBE` is unset, the step skips the probe, records `r9: "probe input absent"`, and restores only `git checkout -- "$FILE"` (plan-review F5).
- [ ] **Step 3: Run the check.** The four classify calls print `stale`, `fresh-recompiled`, `void` and `void`. Run `pipeline cold 1` on the dry-run clone, with `PROBE` unset. Expect `r9 == "probe input absent"` and `git -C <wt> status --porcelain` empty before removal.
- [ ] **Step 4: Commit.** `git commit -am "docs(scorecard): probe a proc macro's undeclared input for stale rebuilds"`

_Requirements: SCORECARD-6.1, SCORECARD-6.2, SCORECARD-6.3, SCORECARD-6.4_

---

### Task 8: Summarize and record a run; the full dry run

**Files:**
- Modify: `docs/scorecard.md` (`### 3. Summarize` and `### 4. Record`)
- Modify: `docs/specs/catalog/hive.md` (the SCORECARD row's Status: In-progress, then Implemented after this task's check)

**Reuse:** rung 2 — the evidence convention `.skills/verify/<date>-<feature>/` (docs/agents/verify.md:59-66)

**Interfaces:**
- Consumes: `$RUN/R1…R5.json`, `$RUN/pipe.jsonl`, `bench_row`, `pipeline`, `r9_classify` (Tasks 4–7)
- Produces:
  - the function `summarize`. It prints one line per row ID, either
    `<row> cold n=<k> median=<v> | seeded n=<k> median=<v>` or `<row> kept previous values
    (<arm>: <k> valid of <n>; void: <reasons>)`, plus the Provenance line
    `measured=<date> commit=<sha> toolchain=<release> machine=<model, chip, cores> evidence=<RUN>`;
  - the Record steps.

**Depends-on:** Task 7

- [ ] **Step 1: Write the failing check.** On a fresh `$DRY`, run the whole Rerun section in order with `SCORECARD_N=3`: preflight, `bench_row R1 build`, the pipeline ×6, `bench_row R2 check-all` … `bench_row R5 check-lib=cargo-hive-core`, then `summarize > $DRY/summary.txt`. Expect a failure, because `summarize` is absent.
- [ ] **Step 2: Write both sections.**
  - `summarize`: the median rule of design Decision 6 (odd n takes the middle; even n takes `floor((a+b)/2)`), and the Provenance cells of Decision 7 (`git rev-list -n1 v<hive_version>`, or the source `git rev-parse HEAD`; `sysctl -n hw.model machdep.cpu.brand_string hw.ncpu`).
  - Record:
    - overwrite only the value, n, median and Provenance cells;
    - never touch the Target slots or Target set;
    - write `$RUN`'s relative path into the Evidence cells;
    - an arm with fewer than 3 valid samples keeps the row's previous cells;
    - confirm `git worktree list`;
    - while O2 is unanswered, stop after printing and commit nothing that names or measures the workspace.
- [ ] **Step 3: Run the check.** Run Step 1's command and expect exit 0.
  - `$DRY/summary.txt` has exactly 10 lines starting `R1`…`R10`.
  - R1's cold median equals `jq .counts.cold_build_ms_median $RUN/R1.json`.
  - A by-hand median of `[5,1,4,2]` gives 3.
  - Apply the Record step to a scratch copy of `docs/scorecard.md`. `diff` against the original changes only lines matching `^\| R([1-9]|10) \|` and leaves every `open — operator` cell. `grep -o 'open — operator' | wc -l` is still 20.
  - After the run, `git -C $DRY/main worktree list` shows only the main, and `ls ~/.hive` is unchanged, since the run used the scratch `HIVE_HOME`.
  - `$DRY/summary.txt` also gives min, max and spread per arm for every measured row, as evidence only (B5).
  - Then set the catalog Status to Implemented. Implemented means the doc and its steps work; it does not complete #16, which needs Task 9's first measured run. Landing this branch must not close #16, overriding the default landing action in `docs/agents/project.md` ("closes the tickets it completed"). Run `refresh-lanes`. Record its printed word; on this branch it is expected to be `not on main` (plan-review F6).
- [ ] **Step 4: Commit.** `git commit -am "docs(scorecard): summarize and record a run in place; dry run passes"` (the scratch copy and `$DRY` are not committed)

_Requirements: SCORECARD-8.4, SCORECARD-8.5, SCORECARD-8.6, SCORECARD-8.8, SCORECARD-9.3_

---

### Task 9: First run on the measured workspace (operator-gated)

**Files:**
- Modify: `docs/scorecard.md` (cells only, and only once O2 is answered)

**Reuse:** rung 2 — the evidence convention `.skills/verify/<date>-<feature>/` (docs/agents/verify.md:59-66)

**Interfaces:**
- Consumes: the whole Rerun section, and `.skills/verify/scorecard.env` filled with the workspace's `MAIN`, `P`, `FILE`, `PROBE`, `WORKSPACE_ENV` and `UNTRACKED_COPY` by the operator or with the operator's say-so
- Produces: `.skills/verify/<date>-scorecard/` with every receipt and `summary.txt`; the R1–R9 cells once O2 allows

**Depends-on:** Task 8

- [ ] **Step 1: Gate.** Do not start until the operator says to run it: it is hours of builds on the owner's machine, and it writes the real `~/.hive`, including `capabilities.json` (design "What a run touches"). The operator also decides first how `~/.hive/capabilities.json` is restored after the 2026-11-22 incident. Record the go-ahead and the O2 answer in `$RUN/gate.txt`.
- [ ] **Step 2: Run.** Run the Rerun section in order with `SCORECARD_N=4`, then `summarize > $RUN/summary.txt`. Check:
  - `summary.txt` has 10 row lines;
  - R1's seeded median equals `jq .counts.seeded_total_ms_median $RUN/R1.json`;
  - R9's line reads `s stale of n` with n ≥ 3 per arm, or `kept previous values`, with its void reasons.
- [ ] **Step 3: Record.** If O2 allows, apply Record to `docs/scorecard.md` and confirm `diff` touches only `| R` lines and no target cell. If O2 is unanswered, leave the doc unchanged and note "numbers held in evidence (O2)" in the build report.
- [ ] **Step 4: Commit,** only if Step 3 changed the doc: `git commit -am "docs(scorecard): record the first measured run"`

_Requirements: SCORECARD-9.1, SCORECARD-6.1, SCORECARD-6.2, SCORECARD-9.3_

---

### Task 10: Guards

**Files:**
- Test: none (inspection commands below); no file modified

**Reuse:** rung 2 — the catalog shard row format (docs/specs/catalog/hive.md), the CONTEXT.md term format, `scripts/gate.sh`

**Interfaces:**
- Consumes: the branch's whole diff against `main`
- Produces: the guard evidence in `.skills/verify/<date>-scorecard/guards.txt`

**Depends-on:** Task 8

- [ ] **Step 1: Run the gate.** Run `scripts/gate.sh` and expect exit 0.
- [ ] **Step 2: Check the diff.**
  - `git diff --stat main -- crates plugin scripts` prints nothing.
  - `git diff main -- README.md docs/roadmap/assessments` prints nothing.
  - `git diff main -- docs/specs/catalog/hive.md` shows only the SCORECARD line added.
  - `git diff main -- CONTEXT.md` shows only added lines (`grep -c '^-[^-]'` = 0).
- [ ] **Step 3: Run audit-trace** over `docs/specs` and expect no finding for SCORECARD.
- [ ] **Step 4: Save the outputs** to `guards.txt`. No commit, since no repo file changes.

_Requirements: SCORECARD-11.1, SCORECARD-11.2, SCORECARD-11.3, SCORECARD-9.2_
