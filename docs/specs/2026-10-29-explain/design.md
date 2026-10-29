# Design: Explain (why a crate has N copies, feature divergence, failed units)

Feature code: EXPLAIN
Status: Approved
Date: 2026-10-29
Requirements: ./requirements.md

## Context

Status already does almost all the reading explain needs. `discover::targets`
(`src/discover.rs:478`) finds every Cargo target dir of the scope, and runs
`fingerprint::read_profile` on each profile dir, one thread per target (`:531-545`). Each
supported `ProfileReport` keeps its full `UnitGraph`. `read_unit` already parses and type-checks
all 11 fingerprint keys (`check_types`, `src/fingerprint.rs:103-136`), but it keeps only the unit
dir name, the kind prefix, the hash and the deps. It drops `features`, `declared_features`,
`profile`, `target`, `path` and the full hash-file name, which are exactly what a copy group is
made of. Not-current units (CORE-4.16) are counted at two places (`:265`, `:282`), not listed. gc
already owns the target scope explain must share (`gc::Scope`, private `in_scope`,
`src/gc/run.rs:22-83`).

The binding constraint is the time budget, together with the guard on status. Status takes 1.6
to 2.2 s on klynt under load against its own 2 s limit (CORE-6.1), and explain may add at most
2 s on top (EXPLAIN-6.1). Status's output must not change (EXPLAIN-7.1). This rules out a second
pass that re-reads and re-parses all 2,731 fingerprint JSON files for explain: it would cost about
0.46 s on klynt (measured in Python) and duplicate the parser. The reader instead keeps what
explain needs from its one read, in a compact form, since status pays for whatever the reader
keeps. Explain adds only what the reader cannot know: a `deps/` listing of the profile dirs that
hold a multi-copy group (263,549 entries on klynt, about 0.18 s) and the `output-*` files of
not-current units (23 files, about 0.07 s).

The second constraint is receipt size. Measured with a prototype of the approved shapes on klynt
(`target/debug`, 2,731 units, 676 multi-copy groups, 139 feature divergences), the default receipt
is 16 KB at N = 10 for both lists, 25 KB at 20 and 10, 35 KB at 20 and 20, and 559 KB with
everything shown. Both lists are cut run-wide, so the size does not grow with the number of
worktrees.

This feature builds on the CORE/AUTOGC receipt amendment, which is not built yet. Today
`Finding` has no `measured` field and the receipt schema is 1 (`src/receipt.rs:9, 33-37`). That
amendment adds `Finding.measured: BTreeMap<String, Measured>` and schema 2, and must merge before
any EXPLAIN task that emits a finding.

## Decisions

1. **The reader keeps the copy-group fields from its one read, in a compact form.** `Unit` gains
   `stem`, `features` (an `Arc<str>` shared by every unit of the profile with the same string),
   `coverage` (computed at read time from `features` and `declared_features`, which is then
   dropped), `profile`, `target` and `path`. No second parse. `declared_features` can be several
   KB per unit, so it is not kept.
2. **The reader lists not-current units, instead of only counting them.** `ProfileReport` gains a
   list. `not_current` keeps its value: it is the list's length, filled at the same two places.
   The `unsupported` closure (`:205`) returns an empty list. The reader does not open `output-*`.
3. **Mode is computed in explain**, from one `std::fs::read_dir` of `deps/`, only for the profile
   dirs that hold at least one multi-copy group. Status never lists `deps/`.
4. **The summary-line matcher lives in `fingerprint`**, so doctor and explain share it without a
   dependency cycle. It matches exactly the requirement's definition: level `"error"`, code null,
   message prefix `aborting due to`. It adds no span check, because the requirement names none.
   Each line is parsed into a struct whose three fields are all optional and which ignores unknown
   fields. A line of valid JSON that is not a diagnostic, such as future-incompat or unused-externs,
   therefore parses and simply does not match. Only invalid JSON counts as "did not parse"
   (EXPLAIN-3.5). `output-*` is read with the reader's existing 4 MiB cap.
5. **A value explain cannot measure is omitted and named in `unmeasured`** (CORE-5.5, which
   EXPLAIN-7.4 keeps in force). "Measured keys exactly" in EXPLAIN-1.2, 1.9, 1.10 and 3.1 names the
   full key set. Cases:
   - `failed_units` while failed-unit reporting is off (EXPLAIN-4.3) is omitted, never written as 0.
   - The three `mode_*` keys of a group whose `deps/` listing failed are omitted.
   - `attempted_at` of a failed unit with no `invoked.timestamp` is omitted.
   - A not-current unit whose `output-*` file is unreadable or over 4 MiB is counted in
     `compiling_or_unknown_units`, because explain cannot tell, and gets an `unmeasured` entry.
6. **Check (g) is a separate doctor check** (owner, 2026-10-29). It reuses check (f)'s
   `PROBE_CRATE` (`src/doctor.rs:583`), `probe_command` (`:592`) and
   `cargo_target::output_with_deadline` (`src/cargo_target.rs:189`), plus one injected type error.
   The verdict:
   - **pass:** cargo exits non-zero and at least one `output-*` under the probe target holds a
     summary line.
   - **fail:** cargo exits non-zero and no `output-*` holds one. This disables failed-unit
     reporting.
   - **unavailable:** anything else (cargo succeeds, cannot start, or hits the deadline). This
     reports the check as unavailable and disables nothing, so a cargo problem that has nothing to
     do with compiling cannot switch off failed-unit reporting.
7. **A capabilities record written before (g) existed reads as "(g) not run".** A new
   `failed_marker_check: u32` field (constant `FAILED_MARKER_CHECK_VERSION = 1`) gets a serde
   default of 0, as `lock_check` does (`src/doctor.rs:122-124`; load test at `:1331`). `require`
   returns `Ok` for any capability absent from `disabled` (`:284-289`), so explain asks
   `doctor::failed_unit_reporting`, which returns not-run while the field is 0. It never refuses
   the command (EXPLAIN-4.3).
8. **Scope is gc's scope, moved to `discover`.** Explain requires only the fingerprint reading
   capability, never Deletion (`src/cli.rs:711` stays gc's).
9. **The repository selection is extracted, `all_repos` included.** The selection block
   (`src/cli.rs:425-447`, `:719-741`), the checkouts loop and `all_repos` (`:498`, `:758`) are
   duplicated between status and gc. `all_repos` decides target ownership and so scope, so explain
   must compute it the same way (EXPLAIN-5.1).
10. **Explain follows status's cancel order.** Signal handlers are installed before `targets`.
    `targets` sees cancel only while resolving workspaces (`src/discover.rs:409`), so explain checks
    `cancelled()` right after it returns (`src/cli.rs:504`), before merging the snapshot, and exits
    130. `explain::run` checks it again between profile dirs and before each `deps/` listing.
11. **Explain merges the snapshot** after that check, as status and gc do (`discover::merge_snapshot`,
    ARCH-16). It writes nothing else besides its receipt.
12. **ADR-0001 is extended, not replaced.** One sentence records that hive also reads `output-*`, a
    Cargo-internal file of rustc JSON diagnostics whose format the rustc book does not promise to
    keep stable, gated per toolchain by check (g), with a field dropped on any line it cannot parse.
13. **Requirements sync-back (2026-10-29).** Measuring the approved shapes broke two premises.
    EXPLAIN-1.3, 1.6, 1.7, 2.1 and 2.2 are struck and replaced by EXPLAIN-1.9, 1.10, 1.11, 2.5 and
    2.6: divergences are ranked and cut with counters, and an empty `features` string gets its own
    coverage value. Cargo writes that empty string for run-build-script units through
    `Fingerprint::new()` (`fingerprint/mod.rs:1028-1035, 1697` at `c980f4866`).

Owner decisions (proposed here, approved by the owner on 2026-10-29):

- **Default counts.** Proposed: one shared default of 10 for copy groups and divergences, 16 KB on
  klynt. The reason is that an agent reading `--json` through a shell tool may have its output cut
  around 30,000 characters. That is recalled as Claude Code's Bash default and is **unverified**.
  The runner-up is 20 and 10 (25 KB).
- **Flag spelling.** Proposed: `-n, --top <N>`, where `0` is the show-all flag of EXPLAIN-1.5,
  2.6 and 5.5. This follows `cargo bloat -n` (default 20, 0 shows all, which the owner confirmed in
  its source). One flag covers both lists.
- **Finding kind names.** Proposed: `copies.group`, `copies.features`, `copies.divergence`,
  `unit.failed`, `copies.totals`.
- **Coverage value for an empty string.** Proposed: `not_recorded`.

Respects: ARCH-1, ARCH-2, ARCH-4, ARCH-8, ARCH-9, ARCH-10, ARCH-13, ARCH-14, ARCH-16, ARCH-17.

## Architecture

Vocabulary for every section below: **module**, **interface**, **implementation**,
**seam** (public surface where behavior is observable and substitutable). Prefer
deep modules — an interface much simpler than what it hides.

Data flow (module dependencies point one way: `cli` → `explain` → `discover`, `cargo_target`,
`fingerprint`; `doctor` → `fingerprint`):

```
cli explain ─ select_repos ─ discover::targets ─ cancelled? ─ merge_snapshot ─ Scope::contains
                                   │                                               │
                          fingerprint::read_profile                          explain::run
                          (Unit + not-current list)          deps/ listing, output-* reads → analyse (pure)
```

### fingerprint (extend)

Satisfies: EXPLAIN-7.1
Reuse: 2 — extends `fingerprint::{Unit, ProfileReport, read_profile, read_unit, never_compiled, read_capped}` (`src/fingerprint.rs:47-76, 103-300`)
Respects: ARCH-9, ARCH-17
Surface:

| Reader | Disposition |
|---|---|
| `cli::report_targets` (`src/cli.rs:569-649`): reads `units`, `format`, `edges`, `path` only | replace — no edit. Neither `Unit` nor `ProfileReport` is serialized into any receipt, and these four values are unchanged (EXPLAIN-7.1) |
| `gc::run` (`src/gc/run.rs:131`): reads `format` | replace — no edit; same value |
| `doctor::check_format` (`src/doctor.rs:689-697`): reads `units`, `edges` | replace — no edit; same values |
| The `ProfileReport` literal in `src/gc/run.rs:728-735` and the reader's tests | replace — they add the new fields |
| `read_unit`'s signature (`:153`) | replace — it returns the stem, `features` and `coverage` too, and the emptied-hash case returns them in place of `None` |

Interface:
```
pub struct Unit { dir_name, kind, hash_hex, deps,                    // existing
                  pub stem: String, pub features: Arc<str>, pub coverage: Coverage,
                  pub profile: u64, pub target: u64, pub path: u64 }
pub enum Coverage { NotRecorded, None, AllDeclared, Partial }
pub struct NotCurrentUnit { pub dir: PathBuf, pub stem: Option<String>,
                            pub features: Option<Arc<str>>, pub outputs: Vec<String> }
pub struct ProfileReport { …, pub not_current_units: Vec<NotCurrentUnit> }   // not_current = len
pub fn is_summary_line(line: &str) -> Option<bool>      // None = not JSON
pub fn error_code(line: &str) -> Option<String>         // non-null code.code of an "error" line
pub(crate) fn read_capped(reader: &mut impl Read) -> Result<Vec<u8>, String>
```
Depth: n/a — extends the reader.
Locality: extend — `src/fingerprint.rs`. `stem` is `None` for the never-compiled shape.

### scope (extract)

Satisfies: EXPLAIN-5.1, EXPLAIN-7.3
Reuse: 2 — moves `gc::Scope` and `in_scope` (`src/gc/run.rs:22-83`) and the scope construction in `gc_cmd` (`src/cli.rs:774-783`)
Respects: ARCH-8
Surface:

| Reader | Disposition |
|---|---|
| `gc::run` calling `in_scope` (`src/gc/run.rs:75`) and the `gc::Scope` uses in `src/gc.rs` and gc's tests (about 30) | replace — they use `discover::Scope` and `Scope::contains` |
| `cli::gc_cmd` building the scope (`src/cli.rs:774-783`) | replace — calls `Scope::from_paths` |
| `tests/gc.rs:196` `scope_follows_the_arguments` | replace — no edit; it guards EXPLAIN-7.3 |

Interface:
```
pub enum Scope { All, Checkouts(Vec<PathBuf>) }
impl Scope { pub fn from_paths(args_empty: bool, checkouts: impl Iterator<Item = PathBuf>) -> Scope;
             pub fn contains(&self, t: &TargetDir) -> bool }
```
Depth: n/a — a move of existing code.
Locality: extract — into `src/discover.rs`, which already depends on `cargo_target` for `TargetDir`
and `Owner`. It takes paths, not `Checkout`, so no new dependency direction appears.

### cli repository selection (extract)

Satisfies: EXPLAIN-5.2
Reuse: 2 — extracts the selection block, checkouts loop and `all_repos` duplicated in `status` (`src/cli.rs:425-447, ~480-501`) and `gc_cmd` (`:719-741, ~744-761`)
Surface:

| Reader | Disposition |
|---|---|
| `cli::status` (`src/cli.rs:391-567`) | replace — calls the helper; refusal findings, checkouts and exit code 1 unchanged (EXPLAIN-7.1) |
| `cli::gc_cmd` | replace — same (EXPLAIN-7.3) |

Interface:
```
struct Selection { repos: Vec<registry::Repo>, checkouts: Vec<Checkout>, all_repos: bool, refused: bool }
fn select(reg: &Registry, args: &[PathBuf], cfg: &Config, b: &mut ReceiptBuilder) -> Selection
```
Depth: n/a — a move of existing code.
Locality: extract — a private function in `src/cli.rs`.

### doctor (extend)

Satisfies: EXPLAIN-4.1, EXPLAIN-4.2, EXPLAIN-7.2
Reuse: 2 — extends `doctor::{CheckId, Capability, capabilities_for, Capabilities, probe_command, PROBE_CRATE}` (`src/doctor.rs:25-72, 583-719`), `cargo_target::output_with_deadline` (`src/cargo_target.rs:189`), `platform::Availability` (`src/platform/mod.rs:315`), and the `lock_check` precedent (`src/doctor.rs:122-124, 269-274, 1331`)
Respects: ARCH-13, ARCH-14
Surface:

| Reader | Disposition |
|---|---|
| `capabilities.json` records written before (g) (`Capabilities` is `deny_unknown_fields`) | replace — `failed_marker_check` has a serde default, so old records load and read as "(g) not run" |
| The `Capabilities { lock_check: 0 }` literals (`src/doctor.rs:1248, 1266, 1311`) | replace — they add the new field |
| `tests/doctor.rs:77` `EXPECTED_IDS: [&str; 6]` and the unit tests enumerating checks (`src/doctor.rs:~1010, 1038, 1143, 1194-1199`) | replace — include (g) |
| `tests/common/mod.rs` `Hive::write_ready_capabilities` | replace — records (g) as passed |
| `README.md` "(a)–(f)" | replace — lists (g) |

Interface:
```
CheckId::G                                  // "g"; a fail disables FailedUnitReporting only
Capability::FailedUnitReporting
pub struct Capabilities { …, #[serde(default)] pub failed_marker_check: u32 }
pub fn failed_unit_reporting(caps: &Capabilities) -> Availability   // Unavailable("…not run"|"…failed")
fn judge_failed_marker(exit_ok: bool, outputs: &[String]) -> Verdict // pure: pass | fail | unavailable
```
Depth: n/a — extends doctor.
Locality: extend — `src/doctor.rs`. Check (g) runs in a fresh subdirectory of doctor's temp dir.
Its process half collects the exit status and the texts of every `output-*` under the probe
target. `judge_failed_marker` decides the verdict without I/O, using `fingerprint::is_summary_line`.
Checks (a)–(f) and their map entries are untouched.

### explain (new module `src/explain.rs`)

Satisfies: EXPLAIN-1.1, EXPLAIN-1.2, EXPLAIN-1.4, EXPLAIN-1.5, EXPLAIN-1.8, EXPLAIN-1.9, EXPLAIN-1.10, EXPLAIN-1.11, EXPLAIN-2.3, EXPLAIN-2.4, EXPLAIN-2.5, EXPLAIN-2.6, EXPLAIN-3.1, EXPLAIN-3.2, EXPLAIN-3.3, EXPLAIN-3.4, EXPLAIN-3.5, EXPLAIN-3.6, EXPLAIN-4.3, EXPLAIN-5.3, EXPLAIN-6.1
Reuse: none — new code (rung 7). Nothing in the codebase or its dependencies groups fingerprints into copies. It composes `fingerprint::{Unit, Coverage, NotCurrentUnit, is_summary_line, error_code, read_capped}`, `cargo_target::TargetDir`, `state::utc_timestamp` (`src/state.rs:286`), `receipt::{Finding, Measured}` and `serde_json` (rung 5).
Respects: ARCH-2, ARCH-4, ARCH-9, ARCH-10, ARCH-17
Interface:
```
pub struct Limits { pub top: Option<usize> }            // None = show all
pub enum FailedUnits { Report, Off(String) }            // explain's own; cli maps doctor's Availability
pub struct Cancelled;
pub fn run(targets: &[&TargetDir], limits: Limits, failed: FailedUnits,
           cancelled: &dyn Fn() -> bool) -> Result<Explanation, Cancelled>
pub struct Explanation { pub findings: Vec<Finding>, pub counts: Vec<(&'static str, u64)>,
                         pub unmeasured: Vec<(String, String)>, pub human: String }
```
Depth: deletion test — without this module, callers would have to rebuild everything it hides:
copy-group keys, mode from `deps/`, coverage labels, ranking and cutting, the deduplication of
feature-set findings, summary-line and error-code reading, the unmeasured rules of Decision 5, and
the fixed divergence detail text. What they keep knowing is only "in-scope targets, a top count
and whether failed units may be named, in; findings, counts, unmeasured entries and text, out".
Locality: new `src/explain.rs` with two private parts: `io`, which lists `deps/` for profile dirs
that hold a multi-copy group and reads `output-*` through `read_capped`, checking `cancelled`
between profile dirs; and `analyse`, a pure function from profile inputs to an `Explanation`.
`pub mod explain;` in `src/lib.rs`. `discover`, `size` and `gc` are not edited by this section.

Ranking and cutting: copy groups and divergences are ranked separately, run-wide, by copies
descending, then profile dir path, crate name, hash-file name (and mode for divergences). The same
`top` applies to both. Totals per target dir always count every group and divergence. Each
feature-set finding is emitted once per (group, mode, features), whichever list asked for it first.

EXPLAIN-4.3 is delivered here: explain obeys `FailedUnits::Off`. The cli section decides when it
is `Off`, from doctor's `failed_unit_reporting`. EXPLAIN-5.1's scope is applied by the cli
section through `Scope::contains` before `run` sees the targets.

### cli `explain` (extend)

Satisfies: EXPLAIN-5.4, EXPLAIN-5.5, EXPLAIN-6.2, EXPLAIN-7.4, EXPLAIN-7.5
Reuse: 2 — mirrors `cli::status` (`src/cli.rs:391-567`), `emit` (`:185`), `discover::{targets, merge_snapshot}`, `doctor::{require, failed_unit_reporting, install_signal_handlers, CANCEL}`; 5 — `clap` derive
Respects: ARCH-1, ARCH-8, ARCH-10, ARCH-14, ARCH-16, ARCH-17
Surface:

| Reader | Disposition |
|---|---|
| `Command` enum and dispatch (`src/cli.rs:23-66, 120-140`) | replace — adds `Explain { repos, top, out }` |
| `README.md:25-31` commands table | replace — adds the `explain` row |
| `docs/adr/0001-read-cargo-internal-fingerprints.md` | replace — adds Decision 12's sentence |

Interface:
```
cargo hive explain [<repo>…] [-n|--top <N>] [--json]     // N = 0 shows all
```
Depth: n/a — a thin adapter, like `status`.
Locality: extend — `src/cli.rs`, `README.md`, the ADR. Order:
1. require `FingerprintReading`;
2. open state and install signal handlers;
3. `select`;
4. `discover::targets`;
5. if `cancelled()`, emit and exit 130;
6. merge the snapshot;
7. keep the targets `Scope::contains` accepts;
8. `explain::run`, with `FailedUnits` from `failed_unit_reporting`;
9. `emit`: exit 130 when cancelled, 1 when an argument was refused, else 0.

## Seams for testing

| Seam | Kind | Covers |
|---|---|---|
| `fingerprint::read_profile`, `is_summary_line`, `error_code` (existing CORE seam): new `Unit` fields, `Coverage`, `not_current_units` | unit | EXPLAIN-7.1 |
| `explain::analyse` (new) over in-memory profile inputs: units, `deps/` names and `output-*` texts | unit | EXPLAIN-1.1, EXPLAIN-1.2, EXPLAIN-1.4, EXPLAIN-1.5, EXPLAIN-1.8, EXPLAIN-1.9, EXPLAIN-1.10, EXPLAIN-1.11, EXPLAIN-2.3, EXPLAIN-2.4, EXPLAIN-2.5, EXPLAIN-2.6, EXPLAIN-3.1, EXPLAIN-3.2, EXPLAIN-3.3, EXPLAIN-3.4, EXPLAIN-3.5, EXPLAIN-3.6, EXPLAIN-4.3, EXPLAIN-5.3 |
| `doctor::run`, `require` and `failed_unit_reporting` with fake `Fs`, and `judge_failed_marker` (existing CORE seam) | unit | EXPLAIN-4.2, EXPLAIN-4.3, EXPLAIN-7.2 |
| The `cargo-hive` binary with `HIVE_HOME=<tmp>` and fixture repos built with real cargo, including a failing-build helper beside `Hive::build_crate` (existing CORE seam) | integration | EXPLAIN-3.1, EXPLAIN-4.1, EXPLAIN-4.3, EXPLAIN-5.1, EXPLAIN-5.2, EXPLAIN-5.4, EXPLAIN-5.5, EXPLAIN-6.2, EXPLAIN-7.1, EXPLAIN-7.3, EXPLAIN-7.4, EXPLAIN-7.5 |
| `tests/guards.rs` `#[ignore]` timing tests on klynt: explain's, and CORE-6.1's re-run because the reader keeps more | integration (manual) | EXPLAIN-6.1, EXPLAIN-7.1 |

One new seam: `explain::analyse`. `judge_failed_marker` is a pure function inside the existing
doctor seam.

## Coverage check

Every live ID in requirements.md appears in exactly one `Satisfies:` line:

| Module | IDs |
|---|---|
| fingerprint | 7.1 |
| scope | 5.1, 7.3 |
| cli repository selection | 5.2 |
| doctor | 4.1, 4.2, 7.2 |
| explain | 1.1, 1.2, 1.4, 1.5, 1.8, 1.9, 1.10, 1.11, 2.3, 2.4, 2.5, 2.6, 3.1–3.6, 4.3, 5.3, 6.1 |
| cli `explain` | 5.4, 5.5, 6.2, 7.4, 7.5 |

Cross-module: EXPLAIN-4.3 needs doctor's `failed_unit_reporting` and the cli mapping; EXPLAIN-5.1
needs the cli to apply `Scope::contains`; EXPLAIN-6.1 is the cost of the reader, the cli and
explain together. Each is listed under the module that owns its observable behavior.

Deliberately unmapped: ~~EXPLAIN-1.3~~, ~~EXPLAIN-1.6~~, ~~EXPLAIN-1.7~~, ~~EXPLAIN-2.1~~ and ~~EXPLAIN-2.2~~, superseded by EXPLAIN-1.9, 1.10, 1.11, 2.5 and 2.6.
