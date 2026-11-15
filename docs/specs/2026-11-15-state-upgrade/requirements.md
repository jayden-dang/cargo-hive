# Requirements: State upgrade (a newer hive reads, or clearly refuses, state an earlier release wrote)

Feature code: UPGRADE
Status: Draft
Date: 2026-11-15

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

Delivers ROAD-14 (docs/roadmap/roads/ROAD-14.md) for issue #13. Source of every decision is the confirmed close
package (`.skills/_pending-state-upgrade/auto/close-package.md`, D1–D20). A value marked *(agent-decided)* was
delegated by ROAD-14.md:22-23 or taken by the run's fork order. A value marked *(Weigh)* was picked by the
reviewing session. Neither is the operator's own choice.

Terms: **state file** and **layout** as defined in `CONTEXT.md`. The six JSON state files are `registry.json`,
`capabilities.json`, `snapshot.json`, `plans.json`, `store.db` and `seeds.json`. "A release" means a published
cargo-hive version. 0.1.0 and 0.1.1 write the same shapes (`git diff v0.1.1 HEAD -- crates` is empty, and
0.1.0 → 0.1.1 changed no persisted type).

## 1. Every state file says which layout wrote it

**Story:** As a user who will upgrade hive later, I want each state file hive writes to record its layout, so
that I can open any of them and see the layout number a later release will read it by.

- **UPGRADE-1.1** WHEN hive writes any of the six JSON state files THE SYSTEM SHALL write `"layout"` as the
  first key of the file's top-level object, holding the layout number of the shape it writes. In this release
  that number is 1 for every file *(agent-decided: an in-file key, ROAD-14.md:23)*.
- **UPGRADE-1.2** WHEN hive reads one of the six JSON state files that has no `layout` key THE SYSTEM SHALL
  read it as layout 1 *(agent-decided)*.
- **UPGRADE-1.3** WHEN hive prints `--json` output or appends a receipt line THE SYSTEM SHALL add no `layout`
  key to it. A receipt line's existing `schema` value is its layout record *(Weigh)*.
- **UPGRADE-1.4** WHEN hive reads or writes `config.json` THE SYSTEM SHALL neither require nor write a
  `layout` key in it. The user writes that file, and hive never does *(agent-decided)*.
- **UPGRADE-1.5** WHILE `store.db` records layout 1 THE SYSTEM SHALL find each store entry at
  `store/<toolchain value>/<name-meta>-<hash>` for variant 0 and `…-<hash>~<variant>` otherwise. store.db's
  layout number covers this path scheme *(Weigh)*.

## 2. State an earlier release wrote reads as before

**Story:** As a user upgrading from 0.1.0, I want `cargo hive repo list`, `status`, `gc` and `seed` to see the
repos, plans, store entries and seed records my old hive wrote, so that nothing is forgotten or filled again.

- **UPGRADE-2.1** WHEN hive reads a state file that 0.1.0 or 0.1.1 wrote THE SYSTEM SHALL read the same field
  values that release wrote, with no refusal and no reset *(agent-decided: every 0.1.x layout is read as is,
  ROAD-14.md:22)*.
- **UPGRADE-2.2** THE SYSTEM SHALL write a state file only from a command that writes that file before this
  feature. No command opens a file only to add its layout record *(agent-decided)*.
- **UPGRADE-2.3** WHEN a command writes a JSON state file that 0.1.0 wrote, and the command changes nothing in
  its content, THE SYSTEM SHALL write the 0.1.0 bytes with one line, `  "layout": 1,`, inserted after the
  opening `{` line *(Weigh)*.
- **UPGRADE-2.4** WHEN hive reads a JSON state file that records a layout and writes it back with no change to
  its content THE SYSTEM SHALL write identical bytes.
- **UPGRADE-2.5** WHEN hive appends a receipt line to a `receipts/<date>.jsonl` file that 0.1.0 wrote THE
  SYSTEM SHALL keep every earlier line byte-identical *(Weigh)*.
- **UPGRADE-2.6** THE SYSTEM SHALL parse every receipt line 0.1.0 wrote as a receipt at the schema the line
  records (2 or 3) *(Weigh)*.
- **UPGRADE-2.7** (guard) WHEN hive looks for a state file THE SYSTEM SHALL CONTINUE TO look under the file
  name and state-root path 0.1.0 used. No release renames or moves a state file without also reading it at
  the old name and path *(Weigh)*.
- **UPGRADE-2.8** WHEN hive writes any state file or prints any `--json` shape from the same inputs as 0.1.0
  THE SYSTEM SHALL produce output equal to 0.1.0's in field names, values and number and string forms, compared
  as normalized JSON values (keys sorted, the normalized keys of capture.rs replaced), apart from:
  - the `hive_version` value;
  - values derived from the clock;
  - in state files only, the `layout` line of UPGRADE-2.3.

  This is checked against output captured from 0.1.0. The byte-exact claim for state files is UPGRADE-2.3's. It
  supersedes SPLIT-4.3
  (docs/specs/2026-11-13-workspace-split/requirements.md:129-132). It withdraws SPLIT-4.3's key-order claim for
  the capture comparison, and the run's report names this *(agent-decided)*.
- **UPGRADE-2.9** WHEN `seed` runs after an upgrade on a state root 0.1.0 wrote, for a checkout whose source
  target 0.1.0 filled and that is unchanged since that fill, THE SYSTEM SHALL clone the store's units for it and add no store entry. The seed
  receipt's `fill_entries_added` count is 0 (ROAD-14.md:8-9; the count is at crates/seed/src/lib.rs:515).

## 3. A state file that cannot be read stops hive instead of reading as empty

**Story:** As a user whose state file is damaged, or was written by a newer hive, I want the command to stop and
name that file, so that my repos, plans and store entries are not dropped behind my back.

- **UPGRADE-3.1** IF a command reads a JSON state file that exists and cannot be parsed THEN THE SYSTEM SHALL
  end the command with exit 1 and one receipt whose finding names the file's full path and the parse error.
  The exceptions are the narrower, named stops in UPGRADE-3.4 to UPGRADE-3.7 and UPGRADE-3.10.
- **UPGRADE-3.2** IF a JSON state file records a layout number above the newest this hive reads THEN THE SYSTEM
  SHALL stop on it exactly where UPGRADE-3.1 stops on an unparseable file, including its narrower stops
  (UPGRADE-3.4 to UPGRADE-3.7, UPGRADE-3.10). The message names the file's full path, the layout the file
  records and the newest layout this hive reads *(agent-decided)*.
- **UPGRADE-3.3** IF hive refuses or stops on a state file under UPGRADE-3.1, UPGRADE-3.2, UPGRADE-3.4, UPGRADE-3.7 or
  UPGRADE-3.10 THEN THE SYSTEM SHALL leave that file's bytes unchanged (ROAD-14.md:18). The one writer that
  never reads its file is outside this: `cargo hive doctor` SHALL CONTINUE TO rewrite `capabilities.json`
  whatever it holds, since that is the fix UPGRADE-3.8 names (crates/cargo-hive/src/cli.rs:459-461)
  *(agent-decided)*.
- **UPGRADE-3.4** IF `seed` finds `seeds.json` present and unparseable when it writes its seed record THEN THE
  SYSTEM SHALL write no seed record and leave `seeds.json` untouched. It reports the existing `record.failed`
  finding, whose detail carries `seeds.json`'s full path and the parse error. Before this feature it replaced
  the file (crates/seed/src/record.rs:62-65; the finding is at crates/seed/src/lib.rs:481-488). `seed` then
  exits 0, as it does on `record.failed` today (crates/seed/src/lib.rs:140-146). *(agent-decided: ticket:13
  "hive stops" is read as the record write stopping, not the command, because refusing before the clone would
  block seed until a hand removal. The run's report names this reading.)*
- **UPGRADE-3.5** (guard) WHEN `status` meets a `seeds.json` it cannot parse THE SYSTEM SHALL CONTINUE TO:
  - add a note carrying the file's full path and the error;
  - report none of the four seed counts (SEED-6.8);
  - keep its exit code *(Weigh)*.
- **UPGRADE-3.6** (guard) WHEN `bench` meets a `seeds.json` it cannot parse THE SYSTEM SHALL CONTINUE TO mark
  the field that needs it unmeasured, with the error as the reason *(Weigh)*.
- **UPGRADE-3.7** IF `bench`'s read of `store.db` for its nothing-to-seed check fails THEN THE SYSTEM SHALL
  fail the run with a finding whose path is `store.db` and whose detail is the read error. Before this feature
  that failure counted as "no stored unit lists the repo" and reported `bench.nothing_to_seed` with its build
  advice (crates/cargo-hive/src/bench/mod.rs:292-313). An unparseable `store.db` met earlier, in bench's prime
  fill, SHALL CONTINUE TO fail the run as it does today (bench/mod.rs:238; fill.rs:91). The nothing-to-seed
  read meets a bad `store.db` only when the prime fill did not read it (fill.rs:77-85) or it changed during
  the run *(Weigh)*.
- **UPGRADE-3.8** IF `capabilities.json` exists and cannot be parsed THEN THE SYSTEM SHALL name
  `cargo hive doctor` in the refusal as the command that rewrites it *(agent-decided)*.
- **UPGRADE-3.9** (guard) WHEN a state file does not exist THE SYSTEM SHALL CONTINUE TO read it as empty. A
  first run with no hive state SHALL CONTINUE TO proceed with the built-in defaults and no configuration
  (CORE-2.6) *(Weigh)*.
- **UPGRADE-3.10** (guard) WHEN bare `cargo hive gc` or `gc --dry-run` cannot parse `store.db` THE SYSTEM SHALL
  CONTINUE TO:
  - evict no store entry;
  - report a finding whose path is `store.db` and whose detail carries the error (`store.write_failed`,
    crates/seed/src/evict.rs:78-84);
  - run the target tier and keep its exit code.

  *(agent-decided: the store tier stops and names the file; fork step 4 leaves the shipped behavior.)*

## 4. A lost store index does not empty the store

**Story:** As a user whose `store.db` has gone missing while the store still holds entries, I want bare
`cargo hive gc` to leave every entry and tell me how to rebuild the index, so that I do not lose the store and
fill it again from scratch.

- **UPGRADE-4.1** IF `store.db` does not exist while `store/<toolchain value>/` holds at least one entry
  directory THEN bare `cargo hive gc` and `gc --dry-run` SHALL evict, and preview as evicted, no store entry.
  Each reports one finding whose path is `store.db`
  and whose detail says to build a checkout of the repository with the current toolchain and then run
  `cargo hive seed` *(Weigh)*.
- **UPGRADE-4.2** (guard) WHEN UPGRADE-4.1 holds THE SYSTEM SHALL CONTINUE TO run bare `gc`'s target tier as
  before.
- **UPGRADE-4.3** (guard) WHEN `seed` fills a store whose `store.db` does not exist THE SYSTEM SHALL CONTINUE TO
  adopt each rowless entry directory whose bytes equal the live unit's, publish a new variant where they
  differ, and write `store.db` (STOREGC-10.4; crates/seed/src/store.rs:274-291).

## 5. Fixtures from each release test the upgrade

**Story:** As the maintainer cutting a release, I want `cargo test --workspace` to load the state every earlier
release wrote, so that a release that cannot read old state fails its gate before it ships.

- **UPGRADE-5.1** THE SYSTEM SHALL carry, under `crates/cargo-hive/tests/fixtures/v0.1.0/`, a state root
  written by a cargo-hive binary built from tag `v0.1.0` (commit fb8fabb) in an isolated `HIVE_HOME` under a
  temporary directory. It holds:
  - each of the six JSON state files, with at least one plan and one seed record *(Weigh)*;
  - a `receipts/` day file;
  - the `store/` tree `store.db` indexes *(Weigh)*;
  - a file naming the tag and the commit it was built from *(Weigh)*.

  It SHALL carry the same under `fixtures/v0.1.1/`, from a binary built from tag `v0.1.1`. No file in either set
  holds the generating user's home path or login *(agent-decided)*.
- **UPGRADE-5.2** WHEN `cargo test --workspace` runs THE SYSTEM SHALL check UPGRADE-2.1, UPGRADE-2.3 and
  UPGRADE-2.4 on each JSON state file of every committed fixture set.
- **UPGRADE-5.3** WHEN `cargo test --workspace` runs THE SYSTEM SHALL check UPGRADE-2.5 and UPGRADE-2.6 on the
  receipts file of every committed fixture set, in a test apart from UPGRADE-5.2's *(Weigh)*.
- **UPGRADE-5.4** WHEN `cargo test --workspace` runs THE SYSTEM SHALL check UPGRADE-3.1 to UPGRADE-3.4 and
  UPGRADE-3.10 on an unparseable copy and on a layout-2 copy of each JSON state file, placed in a test home.
  Each copy is driven through a command that reads it and asserted with that command's own stop:
  - exit 1 for the UPGRADE-3.1 cases;
  - exit 0 with `record.failed` for `seeds.json` through `seed`;
  - the store tier's finding for `store.db` through bare `gc`.

  It SHALL also check UPGRADE-4.1 on a test store with `store.db` removed.
- **UPGRADE-5.5** THE release steps in `docs/agents/project.md` SHALL include a step that does the following,
  and keeps every earlier fixture set:
  - build the release's binary from its tag;
  - write `crates/cargo-hive/tests/fixtures/v<release>/` with it, as UPGRADE-5.1 describes for 0.1.0;
  - commit it.
- **UPGRADE-5.6** THE release step that runs the capture test SHALL take `HIVE_V010_BIN` from a binary built
  from tag `v0.1.0`, not from `/private/tmp/hive-0.1.0`. Today that path is at docs/agents/project.md:158 *(Weigh)*.
- **UPGRADE-5.7** WHEN the capture test runs with a 0.1.0 binary THE SYSTEM SHALL also run `seed` with the new
  binary on a state root 0.1.0 wrote and filled, and check UPGRADE-2.9 on its receipt.

## 6. Quality attributes

**Section-kind:** nfr

**Story:** As a stakeholder, I want measurable quality targets for this feature, so that how-well is not left implicit.

- **Performance:** None. No standing target applies (no docs/product/metrics.md and no docs/ops/). The layout
  key is one more field in files hive already reads whole.
- **Security:** None. No trust boundary is crossed (no docs/security/). Hive still writes only under its state
  root, which UPGRADE-6.2 guards.
- **Reliability:** **UPGRADE-6.1** (guard) WHEN hive writes a state file THE SYSTEM SHALL CONTINUE TO write it
  through the `state` module, atomically (temp file, fsync, rename, directory fsync) and under its one lock
  (ARCH-16 for the files it lists; SEED-9.4 for `seeds.json`). This is verified by the existing state-module tests and a crash-free read-back in UPGRADE-5.2.
  No docs/ops/reliability.md exists, so the target comes from the ARCH-16 lock.
- **Reliability:** **UPGRADE-6.2** WHEN any test or release step makes a fixture THE SYSTEM SHALL
  write only inside a temporary `HIVE_HOME` and the repository's fixture directory, never `~/.hive`. The source
  is the run's charter hard stop; ARCH-2 bounds only the product's own writes. This is verified by the fixture generator taking its home only from a temporary directory.
- **Accessibility:** None. Hive is a headless CLI, and its refusal text is plain text.

## 7. Touched files and the behavior at risk

Every file this feature changes, with the guard that protects it:

- `crates/state/src/lib.rs`: the read path (layout key, missing → empty, refusal text) and the atomic write.
  Guarded by UPGRADE-3.9 and UPGRADE-6.1.
- `crates/cargo/src/registry.rs`, `crates/doctor/src/lib.rs`, `crates/cargo/src/discover.rs`,
  `crates/gc/src/plan_tier/record.rs`, `crates/seed/src/store.rs`, `crates/seed/src/record.rs`: the persisted
  types gain the layout key.
  - Field names, order and values are at risk. Guarded by UPGRADE-2.1, UPGRADE-2.8 and **UPGRADE-7.1**.
  - `deny_unknown_fields` still refuses other unknown keys. Guarded by **UPGRADE-7.2**.
- `crates/seed/src/record.rs`, `crates/seed/src/lib.rs`: the seed-record write changes from replace to refuse
  (UPGRADE-3.4), surfacing through the existing `record.failed` finding. Its other
  pruning is guarded by **UPGRADE-7.3**.
- `crates/seed/src/evict.rs`: store eviction gains the UPGRADE-4.1 case. Eviction otherwise is guarded by
  **UPGRADE-7.4**, and its stop on an unparseable store.db by **UPGRADE-3.10**.
- `crates/cargo-hive/src/bench/mod.rs`: the store.db read changes (UPGRADE-3.7). bench's other findings and
  receipt are guarded by **UPGRADE-7.5**.
- `crates/cargo-hive/src/cli.rs`: refusal text reaches receipts through `emit_failure`. Guarded by
  **UPGRADE-7.6**.
- `crates/cargo-hive/tests/capture.rs`: gains the layout line in its comparison and a `seed` run (UPGRADE-5.7).
  no behavior to guard beyond UPGRADE-2.8.
- `crates/cargo-hive/tests/fixtures/v0.1.0/`: new files. no behavior to guard.
- `docs/agents/project.md` (release steps): gains UPGRADE-5.5 and UPGRADE-5.6. Steps 1–6 are guarded by
  **UPGRADE-7.7**.
- `docs/specs/2026-11-13-workspace-split/requirements.md`: SPLIT-4.3 is struck through as superseded by
  UPGRADE-2.8. no behavior to guard.

Guards:

- **UPGRADE-7.1** (guard) WHEN hive writes `store.db`, `seeds.json` or `plans.json` THE SYSTEM SHALL CONTINUE TO
  use the same fields as before this feature, with `layout` as the only addition (STOREGC-10.5).
- **UPGRADE-7.2** (guard) IF `registry.json`, `capabilities.json`, `snapshot.json` or `config.json` holds a key
  this hive does not define, other than `layout` in the first three, THEN THE SYSTEM SHALL CONTINUE TO refuse it
  as malformed.
- **UPGRADE-7.3** (guard) WHEN `seed` writes a seed record into a readable `seeds.json` THE SYSTEM SHALL
  CONTINUE TO drop records whose target is gone and drop the oldest records to fit its size cap.
- **UPGRADE-7.4** (guard) WHEN `store.db` exists, or the store holds no entry directory, THE SYSTEM SHALL
  CONTINUE TO evict store entries by age, toolchain and cap exactly as before this feature (STOREGC-1.x).
- **UPGRADE-7.5** (guard) WHEN `bench` reads a readable `store.db` THE SYSTEM SHALL CONTINUE TO report
  `bench.nothing_to_seed` with its build advice when no stored unit lists the repo, and the same receipt shape
  as before.
- **UPGRADE-7.6** (guard) WHEN any command refuses THE SYSTEM SHALL CONTINUE TO exit 1 and still print and log
  one receipt with its `refusal` or `error` finding (CORE-5.x). Receipt `schema` values stay 2, and 3 for plan
  and apply (GCPLAN-6.1, GCPLAN-9.2).
- **UPGRADE-7.7** (guard) WHEN a release is cut THE SYSTEM SHALL CONTINUE TO run release steps 1–6 as written
  (gate, version, package check, tag, publish, smoke). Step 6 changes only in where `HIVE_V010_BIN` comes from
  (UPGRADE-5.6).

## Out of Scope

- Downgrading to an older hive. After this release writes `registry.json`, `capabilities.json` or
  `snapshot.json`, 0.1.x refuses it through `deny_unknown_fields`. 0.1.x still reads `plans.json`, `store.db`
  and `seeds.json`, and drops `layout` on its next write. That is named in the run's report and not decided here (ticket:20).
- Cargo's and rustc's file formats, which `cargo hive doctor` gates (ticket:19).
- Telling a file the user deleted apart from a first run. A missing file reads as empty, apart from the
  store-eviction case in UPGRADE-4.1.
- A root manifest of state files, versioned file names, and converting files on open.
- A command that repairs, rebuilds or deletes a state file. The only fix for an unparseable file is today's
  "fix or remove it by hand". Whether a user should ever be told to remove hive's state is the operator's call
  (ROAD-14.md:24); see Open Questions.
- Refusing an older layout this hive no longer reads. No layout older than 1 exists. A later release that drops
  a layout owns that refusal and its fixing command (ticket:7).
- State for ROAD-16 and ROAD-17.
- Teaching the hive skill the new findings. The skill pass follows the author-skills workflow and is not
  required by ROAD-14.

## Open Questions

- Whether this release may ship "fix or remove it by hand" (crates/state/src/lib.rs:44-49) as the only fix in
  the refusal for an unparseable registry.json, snapshot.json, plans.json, store.db or seeds.json. Telling a
  user to remove a state file may fall under ROAD-14's Ask me (ROAD-14.md:24). — owner: operator — date: before
  `Status: Approved` — forbid-guess: the build keeps today's text unchanged and builds nothing on it.
