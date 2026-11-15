# Design: State upgrade

Feature code: UPGRADE
Status: Draft
Date: 2026-11-15
Requirements: ./requirements.md

## Context

Hive keeps six JSON state files under its state root. Every read and write goes through the `cargo-hive-state`
crate:
- four generic methods, `read`, `read_bounded`, `write` and `write_bounded`, on `State` and `Tx`;
- private `encode` (`to_vec_pretty` plus `\n`), `read_json`/`read_json_limited` (NotFound → `T::default()`) and
  `atomic_write` (crates/state/src/lib.rs:360-431).

The files record no layout, and four of the six top-level types use `deny_unknown_fields` (registry, capabilities,
snapshot; config does too). No persisted shape changed from 0.1.0 to HEAD, so this release needs no converter.
It needs a layout record that 0.1.0 files acquire on their next ordinary write. It also needs three code paths
changed:
- `seeds.json` is replaced on a parse error (crates/seed/src/record.rs:62-65);
- bench counts an unreadable `store.db` as "nothing to seed" (crates/cargo-hive/src/bench/mod.rs:292);
- store eviction deletes every unheld entry when `store.db` is missing (crates/seed/src/evict.rs:79, :127-137).

Finally, it needs committed fixtures from the 0.1.0 binary.

The binding constraint is serde itself. `#[serde(flatten)]` "is not supported in combination with structs that
use `deny_unknown_fields`. Neither the outer nor inner flattened struct should use that attribute" (serde.rs
attr-flatten and container-attrs, fetched via Context7). That rules out a generic `Layered<T>` envelope in the
state crate. A `serde_json::Value` round trip is also out: the workspace's serde_json has no `preserve_order`, so
a `Value` re-serialization sorts keys and breaks UPGRADE-2.3/2.4 (design-scan §3). So each persisted type declares
the field itself. The state crate owns the field's type and the check, and every type keeps its own strict parse.

Fresh retrieval (load-subgraph, recipe fsubr-1.2) gives these neighbours:
- OWNS coverage 11/12, because UPGRADE has no tasks.md yet; `observations: []`.
- SPLIT has path_overlap on all five seed paths (`crates/state/src/lib.rs`, `crates/seed/src/evict.rs`,
  `crates/seed/src/record.rs`, `crates/cargo-hive/src/bench/mod.rs`, `crates/cargo-hive/tests/capture.rs`).
- CORE, SEED, STOREGC and GCPLAN match by term only.

None of these offers a layout mechanism to reuse, so the reuse below comes from the scan.

The spine invariants this relies on are ARCH-16 (state files written only through `state`, atomically, under one
lock) and ARCH-2 (writes stay inside the state root).

## Decisions

1. **The layout record is a first field on each type.** Each of `Registry`, `Capabilities`, `Snapshot`,
   `PlansFile`, `StoreFile` and `SeedsFile` gets a first field `#[serde(default)] pub layout:
   cargo_hive_state::Layout`. `Layout`'s `Default` is layout 1, so a 0.1.0 file without the key reads as 1, and
   the derived `Default` of each type writes 1 on a first run. ADR-0004.
2. **Peek, then parse.** `read_json` and `read_json_limited` first parse the bytes into a private
   `Peek { #[serde(default)] layout: Option<u64> }`, which ignores every other key. If the peek parses and
   `layout` is above `Layout::NEWEST` (1), the read returns a new `StateError::NewerLayout { file, found, newest }`
   and never parses `T`.
   - A peek that does not parse falls through to the normal parse, which reports `Malformed` as today.
   - `layout: 0` is `Malformed`.
   - config.json passes through the same peek. A config carrying `layout` is still refused by its own
     `deny_unknown_fields` (UPGRADE-7.2).
3. **One newest-layout constant for now.** `Layout::NEWEST` serves all six files in this release, since every
   file is at 1. The first release that moves one file to layout 2 splits it per file. *Reopen trigger:* any
   persisted type changes shape.
4. **Refusals carry a fix.** `StateError::Malformed` gains `fix: Fix` (`ByHand` | `Run(&'static str)`), with
   `ByHand` as the default. `ByHand` keeps today's text, "fix or remove it by hand". `doctor::load(state)` reads
   capabilities.json and sets `Run("cargo hive doctor")`, so the text reads "…; run `cargo hive doctor` to rewrite
   it". Doctor gains a direct, downward dependency on `cargo-hive-state`.
5. **seed's record write propagates a parse error** (`record.rs` `write_capped`) instead of replacing the file.
   The existing `record.failed` finding carries it, and seed exits 0 (UPGRADE-3.4, agent-decided).
6. **Bench fails on an unreadable store.db.** bench's nothing-to-seed check `match`es the store read. `Err` calls
   `fail_run` with kind `bench.failed` (the kind bench already uses for prime failures), path `store.db`, and the
   error as detail *(agent-decided: no new kind)*.
7. **Eviction skips when store.db is missing.** `evict::run`, right after `inventory` (evict.rs:79), checks
   whether `store.db` is absent while `inv.rowless` is non-empty. If so, it evicts nothing, counts every eviction
   as 0 (same count names), and pushes one finding:
   - kind `store.index_missing` *(agent-decided)*;
   - path `<root>/store.db`;
   - detail "… is missing while the store holds N entries, so no store entry was evicted; build a checkout of the
     repository with the current toolchain, then run `cargo hive seed` to rebuild it".

   This covers bare gc and `--dry-run` alike. The check runs before the store lock (:333). If `store.db` appears
   in between, eviction skips, which is the safe direction.
8. **Committed fixtures.** They live at `crates/cargo-hive/tests/fixtures/v<release>/` as `home/` (a whole state
   root) plus `PROVENANCE` (tag, commit, rustc -vV). An `#[ignore]` generator test, `write_fixture_home` in
   `capture.rs`, makes them. It runs a fixed scenario with the binary in `HIVE_FIXTURE_BIN` and copies the home
   to `HIVE_FIXTURE_OUT`:
   - doctor, repo add, build, a backdated superseded unit so `gc --plan` stores a plan, a secondary worktree, and
     seed;
   - all inside `/private/tmp/hive-it-*`.

   The release steps build that binary from the release tag (`git archive`) in a temporary directory.
9. **The fixture suite is a new integration test file**, `crates/cargo-hive/tests/upgrade.rs`. It walks every
   `fixtures/v*/home/` and is the only new seam. capture.rs strips the top-level `layout` key of JSON state files
   (never receipts) in its normalized comparison (UPGRADE-2.8).
10. **SPLIT-4.3 is struck through as superseded by UPGRADE-2.8**, in the workspace-split requirements, with this
    feature's first task.

## Architecture

Vocabulary for every section below: **module**, **interface**, **implementation**,
**seam** (public surface where behavior is observable and substitutable). Prefer
deep modules — an interface much simpler than what it hides.

### A. State crate: `Layout`, the layout check, and refusal text

Satisfies: UPGRADE-1.2, UPGRADE-2.2, UPGRADE-2.4, UPGRADE-3.2, UPGRADE-3.3, UPGRADE-3.9, UPGRADE-6.1
Reuse: rung 2, which extends `read_json` / `read_json_limited` / `StateError` in crates/state/src/lib.rs:20-70,
:369-403. The atomic write is unchanged.
Respects: ARCH-16
Surface:
- `StateError::Malformed` gains a field.
  - Its `Display` callers: every `e.to_string()` path into `emit_failure` (cli.rs:293) and `record.failed`
    (seed lib.rs:481-488). Disposition `replace`: the text is unchanged for `ByHand`.
  - Its constructors are inside the state crate only (design-scan §4). Disposition `replace`.
  - The pattern matches `matches!(err, StateError::Malformed { .. })` are at gc/plan_tier/record.rs:345, :600.
    Disposition `replace`: they compile unchanged.
- `read`/`read_bounded` on both `State` and `Tx` now return `NewerLayout` for a layout above 1. Every caller
  propagates `StateError` with `?` or `to_string()` (design-scan §1). Disposition `replace`: no caller matches
  exhaustively.
Interface:
```
pub struct Layout(u32);            // serde transparent; Default = Layout::NEWEST
impl Layout { pub const NEWEST: Layout = Layout(1); }
pub enum Fix { ByHand, Run(&'static str) }
StateError::Malformed { file, detail, fix: Fix }
StateError::NewerLayout { file: PathBuf, found: u64, newest: u32 }
impl StateError { pub fn with_fix(self, command: &'static str) -> StateError }
```
Depth: n/a, extends the state crate's read path.
Locality: extend crates/state/src/lib.rs only. Neighbour crates leave.

The `NewerLayout` display text is: "`<path>` records layout N, but this hive reads layouts up to 1; install a
newer cargo-hive (`cargo install cargo-hive`) to read it". The file is never written on any refusal path, because
read returns before a `Tx` writes (UPGRADE-3.3). No read writes a file (UPGRADE-2.2): the read functions only call
`fs::read` or `File::open`.

### B. The six persisted types carry the field

Satisfies: UPGRADE-1.1, UPGRADE-1.5, UPGRADE-2.1, UPGRADE-2.3, UPGRADE-2.7, UPGRADE-7.1, UPGRADE-7.2
Reuse: rung 2, which extends each type in place:
- `Registry` (crates/cargo/src/registry.rs:14);
- `Capabilities` (crates/doctor/src/lib.rs:120);
- `Snapshot` (crates/cargo/src/discover.rs:265);
- `PlansFile` (crates/gc/src/plan_tier/record.rs:112);
- `StoreFile` (crates/seed/src/store.rs:72);
- `SeedsFile` (crates/seed/src/record.rs:35).
Respects: ARCH-16
Surface:
- Struct literals, all `replace` (add `layout: Layout::NEWEST`, or `..Default::default()`):
  - production: doctor/src/lib.rs:292 and discover.rs:745;
  - tests: cli.rs:1961, record.rs (gc):292, seed record.rs:472, store.rs:1319-1594 (8), choose.rs:202,
    doctor lib.rs:1070-1345 (4), discover.rs:778-904 (7).
- Persisted files on users' disks (0.1.0/0.1.1 homes): `frozen`. They are read as is (absent key → 1) and gain
  the key only on a write the command already makes.
- 0.1.x binaries reading this release's files: `frozen`, worked around rather than changed. Downgrades are off
  limits. registry/capabilities/snapshot are refused by 0.1.x; plans/store.db/seeds are read and lose the key on
  0.1.x's next write.
- capture.rs `assert_homes_equal` (capture.rs:350-362): `replace` (§I strips `layout`).
Interface: each type gains one public field, `layout: Layout`. The file-name constants (`registry::FILE`,
`doctor::CAPABILITIES_FILE`, `SNAPSHOT_FILE`, `record::FILE`, `store::STORE_FILE`, `SEEDS_FILE`) and the store
entry path functions (`entry_dir`, `entry_name`, store.rs:80-122) are unchanged.
Depth: n/a, extends the six types.
Locality: extend the six files, plus `crates/doctor/Cargo.toml` (adds `cargo-hive-state`). No module moves.

The field is declared first, so `to_vec_pretty` writes it as the first key. A 0.1.0 file's next write is the
0.1.0 bytes with `  "layout": 1,` after `{` (UPGRADE-2.3), because every other field keeps its order and form.
`deny_unknown_fields` stays on Registry, Capabilities, Snapshot (and its inner types) and ConfigFile, and
`layout` is now a defined key in the first three (UPGRADE-7.2).

### C. Receipts, config.json and `--json` stay as they are

Satisfies: UPGRADE-1.3, UPGRADE-1.4, UPGRADE-2.5, UPGRADE-2.6
Reuse: rung 1, no new code. `Receipt` (crates/core/src/receipt.rs:85-108), `append_receipt_line` (state
lib.rs:343-357) and `ConfigFile` (state lib.rs:225-231) are untouched. The only stdout JSON is the receipt
(cli.rs:261).
Interface: none new.
Depth: n/a, no module.
Locality: leave.

This section exists so the four IDs are tested (§H, receipts test) rather than built. The existing
`Receipt: Deserialize` (design-scan §2) parses 0.1.0 lines at schema 2 or 3 (UPGRADE-2.6).

### D. Refusals reach the receipt; capabilities names doctor

Satisfies: UPGRADE-3.1, UPGRADE-3.8, UPGRADE-7.6
Reuse: rung 2, which reuses `emit_failure` (cli.rs:293) unchanged and adds `doctor::load`.
Surface: the 7 `state.read::<Capabilities>(doctor::CAPABILITIES_FILE)` calls at cli.rs:632, :940, :1152, :1263,
:1429, :1680, :1782 become `doctor::load(state)` (`replace`). tests/bench.rs:121 keeps its direct read
(`replace` not needed: a test helper).
Interface: `pub fn load(state: &State) -> Result<Capabilities, StateError>` in `cargo-hive-doctor`.
Depth: n/a, a one-line wrapper over `State::read` plus `with_fix`.
Locality: extend crates/doctor/src/lib.rs and cli.rs call sites. emit_failure leaves.

An unparseable or newer-layout file ends every command that reads it through `?` into `emit_failure`. That gives
exit 1 and an `error` finding whose detail is the full message, which contains the full path (UPGRADE-3.1/3.2).
The narrower stops are §E–§G.

### E. seed's record write refuses instead of replacing

Satisfies: UPGRADE-3.4, UPGRADE-3.5, UPGRADE-7.3
Reuse: rung 2, which extends `record::write_capped` (crates/seed/src/record.rs:54-72). It reuses
`record_placed`'s `record.failed` path (seed lib.rs:476-488) and status's existing note (cli.rs:753-758).
Surface:
- `write_capped`'s `Ok(Some(note))` replace-note case disappears. Readers:
  - `record_placed` (lib.rs:482): `replace`, its `Ok(Some)` arm still compiles for the remaining notes, if any.
    plan-tasks checks whether any note remains and deletes the arm and the `replaced` helper if none does.
  - The unit test at record.rs:~608-618, which asserts the replacement: `replace` with a test asserting refusal
    and unchanged bytes.
Interface: unchanged signature, `Result<Option<String>, StateError>`. A parse error is now `Err`.
Depth: n/a, extends record.rs.
Locality: extend record.rs (and possibly lib.rs). status leaves.

Pruning (`retain target_present`, `drop_oldest_until`) is unchanged for a readable file (UPGRADE-7.3).

### F. bench names an unreadable store.db

Satisfies: UPGRADE-3.6, UPGRADE-3.7, UPGRADE-7.5
Reuse: rung 2, which extends bench/mod.rs:292-313 with the existing `fail_run` and `Finding`.
Surface: bench's receipt on an unreadable `store.db` after the prime changes from `bench.nothing_to_seed` to
`bench.failed` with a path (`replace`; the existing bench tests keep `nothing_to_seed` for a readable store that
lists no unit, UPGRADE-7.5). The seeds.json unmeasured path (:190-195) leaves.
Interface: none new.
Depth: n/a, extends bench.
Locality: extend crates/cargo-hive/src/bench/mod.rs only.

### G. Store eviction leaves the store when store.db is missing

Satisfies: UPGRADE-3.10, UPGRADE-4.1, UPGRADE-4.2, UPGRADE-4.3, UPGRADE-7.4
Reuse: rung 2, which extends `evict::run` (crates/seed/src/evict.rs:79) with `inventory`'s own `rowless` list and
the existing early-return-with-zero-counts shape (evict.rs:~245-253).
Respects: ARCH-16
Surface: bare `gc` and `gc --dry-run` receipts on a home with store.db missing and entries present gain the
`store.index_missing` finding and report zero eviction counts (`replace`; there are no external readers of this
case). The unparseable-store.db path (`store.write_failed`, evict.rs:78-84) leaves (UPGRADE-3.10). Fill
(`merge_with_source`, store.rs:386) leaves (UPGRADE-4.3).
Interface: none new. The check is a private `fn index_missing(state, inv) -> bool` inside evict.rs.
Depth: n/a, extends evict.
Locality: extend crates/seed/src/evict.rs only. The target tier in cli.rs leaves (UPGRADE-4.2).

### H. Fixtures and the fixture suite

Satisfies: UPGRADE-5.1, UPGRADE-5.2, UPGRADE-5.3, UPGRADE-5.4, UPGRADE-6.2
Reuse:
- rung 2: `tests/common` (the `Hive` scratch-home helper, `store_fixture`) and capture.rs's scratch repo,
  registry and scenario steps (capture.rs:14-19, :395-454);
- rung 7: the new `tests/upgrade.rs`, which no existing test file does: none loads committed state.
Interface:
- `write_fixture_home` (`#[ignore]`, env `HIVE_FIXTURE_BIN`, `HIVE_FIXTURE_OUT`) writes
  `fixtures/v<release>/{home/, PROVENANCE}`.
- `tests/upgrade.rs` walks `fixtures/v*/home/`. Per JSON state file:
  - parse with its crate type and compare its field values to the fixture's `Value` (2.1);
  - write once through `State::write` into a scratch copy and compare bytes (2.3);
  - write again and compare (2.4).
- A separate test appends a receipt line and checks the old bytes and the parse (2.5/2.6).
- Refusal tests (5.4):
  - each file unparseable, and each file at `"layout": 2`, driven through a command that reads it: `status`
    for registry, capabilities and snapshot; `gc --plan` for plans; `seed` for store.db and seeds.json;
  - each asserts exit, finding text and unchanged bytes;
  - the store.db-missing case uses `common::store_fixture` with bare `gc` and `--dry-run`.
Depth: if `upgrade.rs` vanished, a rebuild needs only the fixture layout (`v*/home/` and the six file-name
constants) and the three checks per file.
Locality: new crates/cargo-hive/tests/upgrade.rs and fixtures/v0.1.0/; extend capture.rs with the generator.
Everything else leaves.

The fixture home is copied into a `/private/tmp/hive-it-*` scratch dir before any command runs, so committed
files are never written (UPGRADE-6.2; testing standard "Integration fixtures live in /private/tmp/hive-it-*").
The generator takes its home only from such a dir and writes outside it only to `HIVE_FIXTURE_OUT`.

### I. Capture test, release steps and the SPLIT amendment

Satisfies: UPGRADE-2.8, UPGRADE-2.9, UPGRADE-5.5, UPGRADE-5.6, UPGRADE-5.7, UPGRADE-7.7
Reuse: rung 2, which extends capture.rs (`read_normalized_file`, `state_written_by_0_1_0_reads_unchanged`) and
the release steps in docs/agents/project.md:120-170.
Surface:
- `read_normalized_file` gains the strip of a top-level `layout` key in JSON state files (`replace`).
- Release steps:
  - step 6's `HIVE_V010_BIN=/private/tmp/hive-0.1.0/...` becomes a tag-built path (`replace`, UPGRADE-5.6);
  - a new step 7, "Fixtures", builds the release binary from its tag (`git archive v<X>` into
    `/private/tmp/hive-fixture-<X>/`) and runs `write_fixture_home` (UPGRADE-5.5).
- SPLIT-4.3 text: `replace` (struck through, pointing at UPGRADE-2.8).
Interface: none new beyond the generator in §H.
Depth: n/a, extends existing tests and docs.
Locality: extend capture.rs, project.md, workspace-split requirements.md. Steps 1–5 leave.

`state_written_by_0_1_0_reads_unchanged` gains a `seed` run with the new binary on the 0.1.0-filled home and
asserts `fill_entries_added == 0` (UPGRADE-2.9/5.7).

## Seams for testing

| Seam | Kind | Covers |
|---|---|---|
| `cargo-hive-state` unit tests (`read`/`write` on a scratch root) | unit | UPGRADE-1.2, 2.2, 2.4, 3.2, 3.3, 3.9, 6.1 |
| Unit tests beside each persisted type (registry, doctor, discover, plan record, store, seed record) | unit | UPGRADE-1.1, 1.5, 7.1, 7.2 |
| `crates/cargo-hive/tests/upgrade.rs`, new and the only new seam | integration | UPGRADE-1.3, 1.4, 2.1, 2.3, 2.5, 2.6, 2.7, 3.1, 3.8, 5.1, 5.2, 5.3, 5.4, 6.2 |
| `crates/seed/src/record.rs` unit tests, and `tests/status_seed.rs` | unit / integration | UPGRADE-3.4, 3.5, 7.3 |
| `tests/bench.rs` | integration | UPGRADE-3.6, 3.7, 7.5 |
| `tests/store_gc.rs` (with `common::store_fixture`) | integration | UPGRADE-3.10, 4.1, 4.2, 4.3, 7.4 |
| `tests/receipts.rs` | integration | UPGRADE-7.6 |
| `tests/capture.rs` (`#[ignore]`, release step 6) | integration | UPGRADE-2.8, 2.9, 5.7 |
| `docs/agents/project.md` release steps, checked by reading the diff at review | document | UPGRADE-5.5, 5.6, 7.7 |

## Coverage check

Every ID in requirements.md appears in exactly one `Satisfies:` line:
- A: 1.2, 2.2, 2.4, 3.2, 3.3, 3.9, 6.1
- B: 1.1, 1.5, 2.1, 2.3, 2.7, 7.1, 7.2
- C: 1.3, 1.4, 2.5, 2.6
- D: 3.1, 3.8, 7.6
- E: 3.4, 3.5, 7.3
- F: 3.6, 3.7, 7.5
- G: 3.10, 4.1, 4.2, 4.3, 7.4
- H: 5.1, 5.2, 5.3, 5.4, 6.2
- I: 2.8, 2.9, 5.5, 5.6, 5.7, 7.7

That is 43 IDs, with none unmapped. There is no UI design section: hive is a headless CLI, and no Satisfies ID is
delivered through a browser surface (Step 2b predicate false).
