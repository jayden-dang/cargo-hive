# Tasks: Explain (why a crate has N copies, feature divergence, failed units)

> **For agentic workers:** after plan approval, pick one execute skill:
> - `build-in-waves`: a continuous, dependency-aware scheduler;
> - `build-by-story`: human-gated story review units;
> - `build-inline`: the controller implements, with no implementer subagents.
>
> The chosen skill writes `Execution-mode:`. Steps use checkbox (`- [ ]`) syntax for tracking.

Feature code: EXPLAIN
Status: Approved
Date: 2026-10-29
Execution-mode: continuous
Max-concurrency: auto
Requirements: ./requirements.md
Design: ./design.md

**Goal:** Ship `cargo hive explain [<repo>…] [-n|--top <N>] [--json]`, which reports, as unit counts, why a crate has N copies in a target dir, where one crate builds with different feature sets in one mode and target kind, and which units failed their last compile, with doctor check (g) gating the failed-unit report.

**Architecture:**
- **The reader** (`src/fingerprint.rs`) keeps the copy-group fields from its one read, lists not-current units, and owns the summary-line matcher.
- **`src/explain.rs`** is a pure `analyse` over in-memory profile inputs, plus a small `io` part that lists `deps/` and reads `output-*`.
- **The CLI** `explain` follows `status`'s path. It uses a repository selection and a scope shared with `status` and `gc`.

**Tech Stack:** Rust 2021, rustc 1.97.1 on macOS 27 (APFS). Existing dependencies only: `serde`, `serde_json`, `clap`, `libc`. No new dependency.

## Gate: the receipt amendment

Tasks 5 to 9 emit findings with measured values. They need the CORE/AUTOGC receipt amendment,
which this plan does not contain. It adds `Finding.measured: BTreeMap<String, Measured>`, moves
status's and gc's per-entity numbers into `measured`, adds status's `not_current` key, and sets the
receipt schema to 2. It is planned and merged separately (`amend-feature`). **Do not start
Task 5 until that amendment is on `main`.** Tasks 1 to 4 do not depend on it.

## Global Constraints

The canonical sources (`docs/standards/` is the SSOT). Read them; do not restate them. Hashes were taken 2026-10-29, and a mismatch at execute time means the source changed after planning.

| Source | sha256 (first 16) |
|---|---|
| `docs/standards/INDEX.md` | `80cd41328c0f7652` |
| `docs/standards/code.md` (bounds and deadlines, `// SAFETY:`, no new deps, no requirement IDs in code or comments, `//!` module comments) | `afdcc8b89136f573` |
| `docs/standards/errors-logging.md` (typed error enums, refusals are data, exit codes, no `process::exit` outside `main.rs`) | `48f5baac3b52a552` |
| `docs/standards/testing.md` (merge gate; boundary tests; scratch-only fixtures) | `de677bebcc504edd` |
| `docs/architecture/INDEX.md`: this feature relies on ARCH-1, 2, 4, 8, 9, 10, 13, 14, 16, 17 | `8cb275c512d9ab3c` |
| `docs/specs/2026-10-29-explain/design.md` (Decisions 1–13 and the owner decisions) | `20e98a02cd0ad4bf` |
| `docs/specs/2026-10-29-explain/requirements.md` | `29078f2d237dd679` |
| `.skills/EXPLAIN/close-package.md` | `dfd0c62bc2dd95f3` |

Merge gate (testing standard). Every task must leave all four commands green:
- `cargo fmt --check`, after `cargo fmt`.
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- `cargo check --target x86_64-unknown-linux-gnu` (only the existing `entry_kind` warning in `src/platform/mod.rs` is allowed)

Known pre-existing flake, outside this feature: `tests/status_targets.rs::an_interrupted_status_waiting_on_the_state_lock_exits_130_without_writing_the_snapshot` fails intermittently under full-suite load, on `main` too. If it fails, rerun the suite and report it; do not fix it here.

Commits: each commit step is `cargo fmt`, then `git add -A && git commit -m "<subject>"`, on the feature branch the execute skill creates. No `Co-Authored-By` line.

House rules for this plan:
1. `explain` writes only under `$HIVE_HOME`/`~/.hive`: its receipt and the merged snapshot (ARCH-2). It never builds, replays or deletes (ARCH-17).
2. No requirement ID appears in source, comments or test names.
3. Tests use `/private/tmp/hive-it-*` scratch dirs only.
4. `status` and `gc` output must not change in any task (EXPLAIN-7.1, 7.3); their existing tests are the guard.

## File Structure

| Path | Change | Responsibility |
|---|---|---|
| `src/cli.rs` | modify | `select` (repository selection, checkouts, `all_repos`) shared by `status`, `gc`, `explain`; the `explain` command |
| `src/discover.rs` | modify | `Scope` (moved from `gc`), `Scope::from_paths`, `Scope::contains` |
| `src/gc.rs`, `src/gc/run.rs` | modify | use `discover::Scope`; the `ProfileReport` test literal gains the new field |
| `src/fingerprint.rs` | modify | `Unit` copy-group fields, `Coverage`, `NotCurrentUnit`, `ProfileReport.not_current_units`, `is_summary_line`, `error_code`, `read_capped` made `pub(crate)` |
| `src/doctor.rs` | modify | `CheckId::G`, `Capability::FailedUnitReporting`, `Capabilities.failed_marker_check`, check (g), `judge_failed_marker`, `failed_unit_reporting` |
| `src/explain.rs` | create | `run`, `Limits`, `FailedUnits`, `Cancelled`, `Explanation`; private `analyse` (pure) and `io` |
| `src/lib.rs` | modify | `pub mod explain;` |
| `tests/common/mod.rs` | modify | `Hive::build_crate_failing`; `write_ready_capabilities` records (g) |
| `tests/doctor.rs` | modify | `EXPECTED_IDS` includes `"g"`; check (g) on the proof machine |
| `tests/explain.rs` | create | binary-level tests of `explain` |
| `tests/guards.rs` | modify | `explain` in the CORE-6.3 snapshot tests; the `#[ignore]` explain timing test |
| `README.md` | modify | `explain` row and section; check (g) in the doctor text |
| `docs/adr/0001-read-cargo-internal-fingerprints.md` | modify | one sentence on reading `output-*` |

---

### Task 1: One repository selection for status and gc

Prefactor (design "cli repository selection"). `status` and `gc_cmd` carry the same argument matching and `all_repos` computation; gc's checkouts loop is the one `explain` needs. Extract those, unchanged, so `explain` becomes a caller, not a copy. Status's own checkouts loop (`src/cli.rs:450-486`) stays where it is: it interleaves per-repo headers, `walk.unreadable` and `checkout.*` findings and class counts, and keeps missing checkouts in the scope, so moving it would reorder status's findings.

**Files:**
- Modify: `src/cli.rs` (argument matching in `status` around lines 425-447 and `gc_cmd` around lines 719-741; `all_repos` at 498 and 758; gc's checkouts loop at 744-757)
- Test: `tests/status_targets.rs`, `tests/gc.rs` (existing, unchanged)

**Reuse:** 2 — extracts the selection block, checkouts loop and `all_repos` duplicated in `status` (`src/cli.rs:425-447, ~480-501`) and `gc_cmd` (`:719-741, ~744-761`)

**Interfaces:**
- Consumes: `registry::{Registry, Repo}`, `discover::{Checkout, checkouts}`, `config::Config`, `ReceiptBuilder`
- Produces (private to `src/cli.rs`):
  - `struct Selection { repos: Vec<registry::Repo>, all_repos: bool, refused: bool }` and `fn select(reg: &Registry, args: &[PathBuf], b: &mut ReceiptBuilder) -> Selection`, used by `status`, `gc_cmd` and `explain`.
  - `fn live_checkouts(repos: &[registry::Repo], cfg: &Config, b: &mut ReceiptBuilder) -> Vec<discover::Checkout>`: gc's loop as it is (walk faults as findings, missing checkouts left out), used by `gc_cmd` and `explain`.

**Depends-on:** none

- [ ] **Step 1: Pin today's behavior.** Run `cargo test --test status_targets --test gc --test status_checkouts` and record the pass count. Then save `status --json` and `gc --dry-run --json` receipts of a two-repo fixture with one unregistered argument (`/private/tmp/hive-it-select/before-*.json`); they are the byte-level guard for this refactor, beside the tests.
- [ ] **Step 2: Extract `select` and `live_checkouts`.** Keep the refusal finding text, its kind and the order of checks byte-for-byte. `status` and `gc_cmd` call `select` and read `refused` to choose exit code 1; `gc_cmd` calls `live_checkouts`.
- [ ] **Step 3: Verify.** Rerun the Step 1 tests; expect the same pass count. Rerun the two receipts and diff them with the saved ones, ignoring only `ts`; expect no difference. Run the merge gate.
- [ ] **Step 4: Commit** `refactor(cli): share repository selection between status and gc`.

_Requirements: EXPLAIN-5.2, EXPLAIN-7.1, EXPLAIN-7.3_

---

### Task 2: gc's scope moves to discover

Prefactor (design "scope"). `explain` must use exactly gc's target scope without requiring Deletion.

**Files:**
- Modify: `src/discover.rs`, `src/gc/run.rs` (lines 22-83), `src/gc.rs`, `src/cli.rs` (scope construction around lines 774-783)
- Test: `src/discover.rs` (`#[cfg(test)]`); `tests/gc.rs` (existing `scope_follows_the_arguments`, unchanged)

**Reuse:** 2 — moves `gc::Scope` and `in_scope` (`src/gc/run.rs:22-83`) and the scope construction in `gc_cmd` (`src/cli.rs:774-783`)

**Interfaces:**
- Consumes: `cargo_target::{TargetDir, Owner}`
- Produces: `pub enum discover::Scope { All, Checkouts(Vec<PathBuf>) }`, `Scope::from_paths(args_empty: bool, checkouts: impl Iterator<Item = PathBuf>) -> Scope`, `Scope::contains(&self, t: &TargetDir) -> bool`

**Depends-on:** none

- [ ] **Step 1: Write the failing tests** in `src/discover.rs`: `a_scope_without_arguments_contains_every_target`; `a_checkout_scope_contains_targets_owned_by_or_inside_its_checkouts` (owned by a listed checkout → true; path inside a listed checkout but owned elsewhere → true; orphan under a cache root → false). Run `cargo test --lib discover::`; expect a compile error: `Scope` is not in `discover`.
- [ ] **Step 2: Move the code.** `Scope` and the body of `in_scope` become `discover::Scope` and `Scope::contains`; `gc_cmd`'s construction becomes `Scope::from_paths(args.is_empty(), checkouts.iter().filter(|c| !c.missing).map(|c| c.path.clone()))`. `gc` imports `discover::Scope`. No behavior change.
- [ ] **Step 3: Verify.** `cargo test --lib discover::` passes; `cargo test --test gc` passes unchanged. Run the merge gate.
- [ ] **Step 4: Commit** `refactor(gc): move target scope into discover so other commands can share it`.

_Requirements: EXPLAIN-5.1, EXPLAIN-7.3_

---

### Task 3: The reader keeps copy-group fields and lists not-current units

Design "fingerprint" and Decisions 1, 2, 4.

**Files:**
- Modify: `src/fingerprint.rs`, `src/gc/run.rs` (the `ProfileReport` literal around lines 728-735)
- Test: `src/fingerprint.rs` (`#[cfg(test)]`)

**Reuse:** 2 — extends `fingerprint::{Unit, ProfileReport, read_profile, read_unit, never_compiled, read_capped}` (`src/fingerprint.rs:47-76, 103-300`)

**Interfaces:**
- Consumes: none new
- Produces:
  - `Unit` gains `pub stem: String`, `pub features: Arc<str>`, `pub coverage: Coverage`, `pub profile: u64`, `pub target: u64`, `pub path: u64`. `Unit`, `UnitGraph` and `ProfileReport` derive `Serialize` (`src/fingerprint.rs:46, 61, 67`) and serde's `rc` feature is off, so every new field carries `#[serde(skip)]`.
  - `pub enum Coverage { NotRecorded, None, AllDeclared, Partial, Unreadable }` with `fn as_str(&self) -> Option<&'static str>` returning `not_recorded`, `none`, `all_declared`, `partial`, and `None` for `Unreadable` (a `features` or `declared_features` string that is not a JSON array of strings; the unit still reads, so status is unchanged, and explain omits `coverage` with an `unmeasured` entry).
  - `pub struct NotCurrentUnit { pub dir: PathBuf, pub stem: Option<String>, pub features: Option<Arc<str>>, pub outputs: Vec<String> }`.
  - `ProfileReport` gains `#[serde(skip)] pub not_current_units: Vec<NotCurrentUnit>`; `not_current == not_current_units.len()` always.
  - `pub fn is_summary_line(line: &str) -> Option<bool>` (`None` when the line is not JSON) and `pub fn error_code(line: &str) -> Option<String>` (non-null `code.code` of a line whose `level` is `"error"`).
  - `read_capped` and `kind_of` become `pub(crate)`; Task 7 reads a never-compiled unit's kind from its `output-<kind>-<name>` file name with `kind_of`.

**Depends-on:** none

- [ ] **Step 1: Write the failing tests** in `src/fingerprint.rs`:
  - `a_unit_keeps_its_stem_features_coverage_and_hashes`: a new helper `unit_json_with(deps, features, declared)` beside `unit_json` (whose 8 call sites stay unchanged); a unit with `"[\"testkit\"]"`/`"[\"testkit\"]"` reads `AllDeclared`, `"[]"` reads `None`, `"[\"a\"]"` of `"[\"a\",\"b\"]"` reads `Partial`, `""`/`""` reads `NotRecorded`, `"not json"` reads `Unreadable` and the profile stays supported; `stem == "lib-q"`, and `profile`, `target`, `path` equal the JSON values (2, 1, 3).
  - `units_with_the_same_features_share_one_string`: two units with `"[]"` have `Arc::ptr_eq` features.
  - `not_current_units_are_listed_with_what_explain_needs`: the never-compiled fixture lists `stem: None, features: None, outputs: ["output-lib-failed"]`; the emptied-hash fixture lists `stem: Some("lib-dep")` and `features: Some("[]")`; `not_current` equals the list length; an unsupported profile has an empty list.
  - `the_summary_line_is_recognized_anywhere_and_only_in_its_exact_form`: lines from `.skills/research/2026-10-29-cargo-output-message-cache.md` — `{"$message_type":"diagnostic","message":"aborting due to 1 previous error","code":null,"level":"error","spans":[]}` → `Some(true)`; same with `"code":{"code":"E0308"}` → `Some(false)`; a `failure-note` → `Some(false)`; `{"$message_type":"future_incompat"}` → `Some(false)`; `not json` → `None`. `error_code` returns `Some("E0308")`, `Some("dead_code")`, and `None` for the summary line.

  Run `cargo test --lib fingerprint::`; expect compile errors on the missing fields and functions.
- [ ] **Step 2: Implement.** `read_unit` returns the stem, the interned `features` (a per-profile `HashMap<String, Arc<str>>`) and `coverage` computed from `features` and `declared_features` (a JSON string array each; `""` → `NotRecorded`), then drops `declared_features`. Both not-current sites push a `NotCurrentUnit` from the names already in hand. The line matchers parse into a struct whose `level`, `code` and `message` are all `Option` and ignore unknown fields.
- [ ] **Step 3: Verify.** `cargo test --lib fingerprint::` passes; `cargo test --test status_targets` passes unchanged (status output identical). Run the merge gate.
- [ ] **Step 4: Check status's time.** Run `cargo test --release --test guards -- --ignored status_on_klynt` and record cold and warm times beside the pre-change numbers in the task report (`.skills/EXPLAIN/`); a cold run over 2 s that `main` passes on the same load is a stop-and-report.
- [ ] **Step 5: Commit** `feat(fingerprint): keep what explain needs from the one read, and list not-current units`.

_Requirements: EXPLAIN-7.1_

---

### Task 4: Doctor check (g) proves failed compiles can be recognized

Design "doctor" and Decisions 6, 7.

**Files:**
- Modify: `src/doctor.rs`, `tests/common/mod.rs`, `tests/doctor.rs` (`EXPECTED_IDS` at line 77), `README.md` (doctor text), `docs/adr/0001-read-cargo-internal-fingerprints.md`
- Test: `src/doctor.rs` (`#[cfg(test)]`), `tests/doctor.rs`

**Reuse:** 2 — extends `doctor::{CheckId, Capability, capabilities_for, Capabilities, probe_command, PROBE_CRATE}` (`src/doctor.rs:25-72, 583-719`), `cargo_target::output_with_deadline` (`src/cargo_target.rs:189`), `platform::Availability` (`src/platform/mod.rs:315`), and the `lock_check` precedent (`src/doctor.rs:122-124, 269-274, 1331`)

**Interfaces:**
- Consumes: `fingerprint::is_summary_line` (Task 3)
- Produces:
  - `CheckId::G` (letter `"g"`), `Capability::FailedUnitReporting`; a `fail` verdict of (g) disables only `FailedUnitReporting`.
  - `Capabilities` gains `#[serde(default)] pub failed_marker_check: u32` and `pub const FAILED_MARKER_CHECK_VERSION: u32 = 1`.
  - `pub fn failed_unit_reporting(caps: &Capabilities) -> Availability`: `Available` when `failed_marker_check >= 1` and `FailedUnitReporting` is not disabled; `Unavailable("doctor check (g) has not run; run cargo hive doctor")` when the field is 0; `Unavailable("doctor check (g) failed")` when disabled.
  - `fn judge_failed_marker(exit_ok: bool, outputs: &[String]) -> Verdict` (pure): pass when `!exit_ok` and any line of any output is a summary line; fail when `!exit_ok` and none is; unavailable when `exit_ok`. A probe that cannot start or hits the deadline is unavailable too.
  - `capabilities_for` (`src/doctor.rs:293-306`) disables on every non-pass verdict today. For (g) only, `unavailable` disables nothing and leaves `failed_marker_check` at 0, so `failed_unit_reporting` reads "has not run" and explain counts without naming. `failed_marker_check` is set to `FAILED_MARKER_CHECK_VERSION` only when (g)'s verdict is pass or fail. Every other check keeps today's rule.
  - `tests/common/mod.rs`: `Hive::build_crate_failing(&self, dir: &Path)` runs the same command as `build_crate` and asserts cargo **fails**; `write_ready_capabilities` writes `"failed_marker_check": 1`.
  - Check (g)'s measured values: `exit_ok` (bool), `outputs` (count of `output-*` files read), `summary_lines` (count).

**Depends-on:** Task 3

- [ ] **Step 1: Write the failing unit tests** in `src/doctor.rs`: `judge_failed_marker` table (`(false, [summary])` → pass; `(false, ["warning only"])` → fail; `(true, [])` → unavailable; `(false, [])` → fail); `an_unavailable_check_g_disables_nothing_and_leaves_g_not_run` (`capabilities_for` with (g) unavailable → no capability disabled, `failed_marker_check == 0`; with (g) fail → only `FailedUnitReporting` disabled, marker 1; with pass → marker 1); `a_capabilities_record_without_check_g_loads_and_reads_as_not_run` (the `lock_check` load-test shape, `:1331`); `a_failed_check_g_disables_only_failed_unit_reporting` (`capabilities_for` with (g) fail leaves `FingerprintReading`, `Deletion`, `Seeding` enabled). Update the three `Capabilities { lock_check: 0 }` literals (`:1248, 1266, 1311`) and the check enumerations (`~1010, 1038, 1143, 1194-1199`). Run `cargo test --lib doctor::`; expect compile errors.
- [ ] **Step 2: Implement** `CheckId::G`, the capability, the field, `failed_unit_reporting`, `judge_failed_marker`, and the check: write `PROBE_CRATE` into a fresh `g/` subdirectory, appending `pub fn hive_probe_failure() -> u32 { "x" }` to its `src/lib.rs`, of doctor's temp dir, build it with `probe_command` and `output_with_deadline`, read every `output-*` under its `target/debug/.fingerprint/` with `read_capped`, and judge.
- [ ] **Step 3: Integration.** `tests/doctor.rs`: `EXPECTED_IDS` becomes 7 entries with `"g"`; add `check_g_passes_on_the_proof_machine` (doctor receipt has check `g` with verdict `pass`). Run `cargo test --test doctor`; expect pass.
- [ ] **Step 4: Docs.** README doctor text lists (g) and what it disables. ADR-0001 gains the sentence of design Decision 12. Run the merge gate.
- [ ] **Step 5: Commit** `feat(doctor): check (g) proves a failed compile leaves rustc's summary line`.

_Requirements: EXPLAIN-4.1, EXPLAIN-4.2, EXPLAIN-7.2_

---

### Task 5: Copy groups, feature sets and totals (pure analysis)

**Gate:** the receipt amendment is on `main`. Design "explain", ranking and cutting.

**Files:**
- Create: `src/explain.rs`
- Modify: `src/lib.rs`
- Test: `src/explain.rs` (`#[cfg(test)]`)

**Reuse:** none — new code (rung 7). Nothing in the codebase or its dependencies groups fingerprints into copies. It composes `fingerprint::{Unit, Coverage, NotCurrentUnit, is_summary_line, error_code, read_capped}`, `cargo_target::TargetDir`, `state::utc_timestamp` (`src/state.rs:286`), `receipt::{Finding, Measured}` and `serde_json` (rung 5).

**Interfaces:**
- Consumes: `Unit` fields and `Coverage` (Task 3); `Finding { kind, path, detail, measured }` and `Measured::{Int, Text}` (receipt amendment)
- Produces (private to `src/explain.rs`, used by Tasks 6–8):
  - `struct ProfileInput { target_dir: PathBuf, profile_dir: PathBuf, units: Vec<Unit>, deps: Option<Vec<String>>, not_current: Vec<NotCurrentInput> }` (`deps: None` = listing failed or over its cap)
  - `struct NotCurrentInput { unit: NotCurrentUnit, kind_from_output: Option<String>, outputs: Result<Vec<String>, String>, attempted_at: Option<SystemTime> }` and `pub enum FailedUnits { Report, Off(String) }`, declared here so `analyse` has its final signature; Task 5's tests pass `not_current: vec![]` and `FailedUnits::Report`, and Task 7 adds the rules that read them
  - `fn analyse(inputs: &[ProfileInput], limits: Limits, failed: &FailedUnits) -> Explanation`
  - `pub struct Limits { pub top: Option<usize> }` (`None` = show all; default `Some(10)`)
  - `pub struct Explanation { pub findings: Vec<Finding>, pub counts: Vec<(&'static str, u64)>, pub unmeasured: Vec<(String, String)>, pub human: String }`
  - Finding kinds `copies.group`, `copies.features`, `copies.totals`; mode from deps names, matched by the unit directory's `-<16 hex>` hash suffix (crate dirs use `-` where `deps/` files use `_`): `build` if any entry ending `-<hash>` or `-<hash>.<ext>` is `.rlib`, `.dylib` or has no extension, else `check` if one is `.rmeta`, else `unknown`

**Depends-on:** Task 3, the receipt amendment

- [ ] **Step 1: Write the failing tests** in `src/explain.rs` over hand-built `ProfileInput`s:
  - `copies_are_grouped_by_crate_stem_target_and_path`: three `lib-persistence` units sharing target/path → one group of 3; a fourth with another `path` → not in it.
  - `a_group_reports_its_counts_under_fixed_keys`: features `["testkit"]`×2 with profile 2 and `[]`×1 with profile 5, deps names giving 2 build and 1 check → `copies.group` measured exactly `{crate:"persistence", target_kind:"lib", copies:3, feature_sets:2, profile_hashes:2, mode_build:2, mode_check:1, mode_unknown:0, copies_sharing_features_and_profile:1}`.
  - `feature_sets_are_reported_per_mode_with_coverage`: one `copies.features` per (mode, features) with `coverage` `all_declared`/`none`/`not_recorded` and `units`.
  - `the_most_copied_groups_are_listed_first_and_cut_at_the_limit`: 12 groups across two profile dirs with ties → `top: Some(10)` lists 10 in the order copies desc, profile path, crate, stem; `top: None` lists 12.
  - `every_target_gets_totals_even_with_nothing_listed`: a target whose groups are all cut still has `copies.totals` with `groups`, `extra_units`, `groups_shown: 0`; run-wide counts `groups`, `extra_units`, `groups_shown`.
  - `names_never_appear_in_keys`: every measured key and count name is in the fixed vocabulary.
  - `a_failed_deps_listing_omits_modes_and_says_why`: `deps: None` for a listed group's profile → no `mode_*` keys and one `unmeasured` entry naming the profile.

  Run `cargo test --lib explain::`; expect compile errors (module missing).
- [ ] **Step 2: Implement** `analyse` for groups, feature sets, ranking, cutting and totals, and `pub mod explain;`.
- [ ] **Step 3: Verify** `cargo test --lib explain::` passes. Run the merge gate.
- [ ] **Step 4: Commit** `feat(explain): group copies of a crate and report what tells them apart`.

_Requirements: EXPLAIN-1.1, EXPLAIN-1.2, EXPLAIN-1.4, EXPLAIN-1.5, EXPLAIN-1.8, EXPLAIN-1.9, EXPLAIN-1.10, EXPLAIN-1.11_

---

### Task 6: Feature divergence within one mode and target kind

**Gate:** the receipt amendment is on `main`.

**Files:**
- Modify: `src/explain.rs`
- Test: `src/explain.rs` (`#[cfg(test)]`)

**Reuse:** none — new code (rung 7). Nothing in the codebase or its dependencies groups fingerprints into copies. It composes `fingerprint::{Unit, Coverage, NotCurrentUnit, is_summary_line, error_code, read_capped}`, `cargo_target::TargetDir`, `state::utc_timestamp` (`src/state.rs:286`), `receipt::{Finding, Measured}` and `serde_json` (rung 5).

**Interfaces:**
- Consumes: `analyse`, `ProfileInput`, `Limits`, `Explanation` (Task 5)
- Produces: finding kind `copies.divergence` with measured keys exactly `crate`, `target_kind`, `mode`, `feature_sets`; `copies.totals` and run-wide counts gain `divergences`, `divergences_shown`; the fixed detail `DIVERGENCE_DETAIL: &str` = "Feature sets in one mode can differ because invocations pass different feature flags, or because dev-dependency feature unification differs between build and test."

**Depends-on:** Task 5

- [ ] **Step 1: Write the failing tests:**
  - `a_crate_with_two_feature_sets_in_one_mode_diverges`: check-mode units with `["testkit"]` and `[]` → one `copies.divergence` (`mode:"check"`, `feature_sets:2`) plus their `copies.features`; build-mode units all `[]` → no divergence for build.
  - `divergences_are_ranked_and_cut_like_groups`: 12 divergences, `top: Some(10)` → 10 listed by copies desc then profile, crate, stem, mode; totals and counts carry `divergences: 12, divergences_shown: 10`; `top: None` → 12.
  - `a_feature_set_finding_is_reported_once`: a group both listed and divergent yields each (mode, features) finding once.
  - `the_divergence_detail_names_both_sources_and_no_tool`: every `copies.divergence` detail equals `DIVERGENCE_DETAIL`.

  Run `cargo test --lib explain::`; expect failures (no divergence findings).
- [ ] **Step 2: Implement** divergence detection, ranking, cutting and the counters.
- [ ] **Step 3: Verify** `cargo test --lib explain::` passes. Run the merge gate.
- [ ] **Step 4: Commit** `feat(explain): report crates built with different feature sets in one mode`.

_Requirements: EXPLAIN-2.3, EXPLAIN-2.4, EXPLAIN-2.5, EXPLAIN-2.6_

---

### Task 7: Units whose last compile failed

**Gate:** the receipt amendment is on `main`.

**Files:**
- Modify: `src/explain.rs`
- Test: `src/explain.rs` (`#[cfg(test)]`)

**Reuse:** none — new code (rung 7). Nothing in the codebase or its dependencies groups fingerprints into copies. It composes `fingerprint::{Unit, Coverage, NotCurrentUnit, is_summary_line, error_code, read_capped}`, `cargo_target::TargetDir`, `state::utc_timestamp` (`src/state.rs:286`), `receipt::{Finding, Measured}` and `serde_json` (rung 5).

**Interfaces:**
- Consumes: `NotCurrentUnit`, `is_summary_line`, `error_code` (Task 3); `analyse`, `ProfileInput` (Task 5); `state::utc_timestamp`
- Produces:
  - `struct NotCurrentInput { unit: NotCurrentUnit, kind_from_output: Option<String>, outputs: Result<Vec<String>, String>, attempted_at: Option<SystemTime> }`
  - `pub enum FailedUnits { Report, Off(String) }`
  - finding kind `unit.failed` with measured keys `crate`, `target_kind`, `features`, `attempted_at`, `error_codes`; `copies.totals` gains `failed_units`, `compiling_or_unknown_units`

**Depends-on:** Task 5

- [ ] **Step 1: Write the failing tests:**
  - `a_summary_line_anywhere_makes_a_unit_failed`: outputs `[E0308 line, summary, failure-note]` → one `unit.failed` with `error_codes: "E0308"`, `attempted_at` from `utc_timestamp`, path = unit dir.
  - `error_codes_are_distinct_non_null_and_in_first_seen_order`: `[dead_code, E0308, dead_code, summary]` → `"dead_code,E0308"`; only the summary → `""`.
  - `a_never_compiled_unit_names_its_kind_from_the_output_file_and_omits_features`: `stem: None` → `crate` from dir name, `target_kind` from `output-lib-x`, no `features` key, an `unmeasured` entry.
  - `a_unit_without_a_summary_line_is_only_counted`: warnings only, or no outputs → no finding, `compiling_or_unknown_units: 1`.
  - `an_unparsable_line_is_skipped_and_reported`: `[not json, summary]` → failed, `unmeasured` names the unit dir.
  - `an_unreadable_output_or_missing_stamp_is_handled`: `outputs: Err(..)` → counted unknown with `unmeasured`; `attempted_at: None` → key omitted with `unmeasured`.
  - `failed_units_are_not_named_while_reporting_is_off`: `FailedUnits::Off("doctor check (g) has not run; …")` → no `unit.failed`, every not-current unit in `compiling_or_unknown_units`, no `failed_units` key, `unmeasured` carries the reason.
  - `a_failed_unit_is_described_only_by_its_last_compile`: the `unit.failed` detail is the fixed text "Its last compile failed." for every finding.

  Run `cargo test --lib explain::`; expect failures.
- [ ] **Step 2: Implement** the failed-unit rules in `analyse`.
- [ ] **Step 3: Verify** `cargo test --lib explain::` passes. Run the merge gate.
- [ ] **Step 4: Commit** `feat(explain): report units whose last compile failed, with rustc's codes`.

_Requirements: EXPLAIN-3.1, EXPLAIN-3.2, EXPLAIN-3.3, EXPLAIN-3.4, EXPLAIN-3.5, EXPLAIN-3.6, EXPLAIN-4.3_

---

### Task 8: `cargo hive explain` end to end

**Gate:** the receipt amendment is on `main`. Design "cli `explain`" and Decisions 3, 8, 10, 11.

**Files:**
- Modify: `src/cli.rs`, `src/explain.rs` (the `io` part and `run`), `tests/common/mod.rs`, `tests/status_targets.rs`, `tests/gc.rs`, `tests/status_checkouts.rs` (they import the moved `register`), `README.md`
- Create: `tests/explain.rs`
- Test: `tests/explain.rs`

**Reuse:** 2 — mirrors `cli::status` (`src/cli.rs:391-567`), `emit` (`:185`), `discover::{targets, merge_snapshot}`, `doctor::{require, failed_unit_reporting, install_signal_handlers, CANCEL}`; 5 — `clap` derive

**Interfaces:**
- Consumes: `select`, `Selection` (Task 1); `discover::Scope` (Task 2); `ProfileReport.not_current_units` (Task 3); `doctor::failed_unit_reporting` (Task 4); `analyse`, `Limits`, `FailedUnits`, `Explanation` (Tasks 5–7)
- Produces:
  - `Command::Explain { repos: Vec<PathBuf>, top: usize /* -n, --top, default 10, 0 = all */, out: OutputArgs }`
  - `pub fn explain::run(targets: &[&TargetDir], limits: Limits, failed: FailedUnits, cancelled: &dyn Fn() -> bool) -> Result<Explanation, Cancelled>`; `pub struct Cancelled;`
  - `io`: lists `deps/` only for profile dirs holding a multi-copy group, through `Fs::read_dir_bounded` with `gc::run::DEPS_CAP` (`src/gc/run.rs:19`, made `pub(crate)`); over the cap or on an error the profile gets `deps: None`. Reads each not-current unit's `output-*` with `read_capped` (4 MiB) and its `invoked.timestamp` mtime; checks `cancelled` between profile dirs.
  - Unreadable targets and unsupported profile dirs (EXPLAIN-5.3) are reported with status's existing finding kinds and texts from `report_targets` (`target.unreadable`, `profile.unsupported`).
  - `tests/common/mod.rs`: `pub fn register(hive: &Hive, repo: &Path)` (moved from `tests/status_targets.rs:12`, `tests/gc.rs:21`, `tests/status_checkouts.rs:25`, which then import it) and `Hive::check_crate(&self, dir: &Path)` (`cargo check --offline --quiet`, same env as `build_crate`).

**Depends-on:** Task 1, Task 2, Task 4, Task 6, Task 7

- [ ] **Step 1: Write the failing integration tests** in `tests/explain.rs` (fixture repos under `/private/tmp/hive-it-*`, `HIVE_HOME` per test, `write_ready_capabilities`):
  - `explain_lists_copies_of_a_crate_built_two_ways`: `build_crate`, then `check_crate` → `copies.group` for its lib with `mode_build:1, mode_check:1`.
  - `explain_names_a_failed_unit_with_its_code`: `build_crate_failing` on a crate with a type error → one `unit.failed` with `error_codes:"E0308"`, `failed_units:1`.
  - `explain_without_check_g_counts_but_does_not_name`: capabilities without `failed_marker_check` → no `unit.failed`, `unmeasured` says (g) has not run, exit 0.
  - `explain_scope_follows_the_arguments`: two registered repos, `explain <a>` reports only a's targets (mirror `tests/gc.rs:196`).
  - `explain_refuses_an_unregistered_argument_and_continues`: one good, one unregistered → refusal finding, the good repo's findings, exit 1.
  - `explain_refuses_without_fingerprint_reading`: `FingerprintReading` disabled → refusal naming check (f).
  - `explain_does_not_need_deletion`: `Deletion` disabled → runs, exit 0.
  - `an_unsupported_profile_is_named_and_skipped`: a profile with a malformed JSON → `profile.unsupported` naming it, no groups from it; `an_unreadable_target_is_named_and_skipped`: a target dir with mode 000 → `target.unreadable`, others still explained.
  - `explain_cancelled_on_sigint_exits_130_and_leaves_the_snapshot` and `explain_merges_the_snapshot` (templates: `tests/status_targets.rs:492` and `:561`).
  - `deps_listing_and_output_reads_are_bounded`: an `output-*` of exactly 4 MiB is read and one byte over is reported in `unmeasured`; a `deps/` over the cap omits the `mode_*` keys with an `unmeasured` entry (unit test in `src/explain.rs` with a fake `Fs` and a small cap).
  - `the_json_receipt_is_the_logged_receipt`: stdout of `explain -n 0 --json` equals the last line of `receipts/<date>.jsonl`.
  - `top_zero_shows_every_group`: a synthetic target (a `CACHEDIR.TAG`, a `debug/.fingerprint/` with 12 crates × 2 units written by the reader test helpers' format, and an empty `deps/`) under the fixture repo → `-n 0` lists 12 groups, the default lists 10.

  Run `cargo test --test explain`; expect failure: `explain` is not a command.
- [ ] **Step 2: Implement** `io`, `run`, the command in the order of the design (require `FingerprintReading`; state and signal handlers; `select`; `targets`; `cancelled()` → exit 130 before the snapshot merge; merge; `Scope::contains`; `run` with `FailedUnits` from `failed_unit_reporting`; `emit` with 130/1/0), and the human rendering: one line per listed group, divergence and failed unit, then the totals.
- [ ] **Step 3: Verify** `cargo test --test explain` passes. Run the merge gate.
- [ ] **Step 4: Docs.** README commands table gains `cargo hive explain [<repo>…] [-n N]`, plus a short section on what it reports and never claims.
- [ ] **Step 5: Commit** `feat(cli): add cargo hive explain`.

_Requirements: EXPLAIN-5.1, EXPLAIN-5.2, EXPLAIN-5.3, EXPLAIN-5.4, EXPLAIN-5.5, EXPLAIN-7.4_

---

### Task 9: Explain writes nothing outside hive's state, within its time budget

**Gate:** the receipt amendment is on `main`.

**Files:**
- Modify: `tests/guards.rs`
- Test: `tests/guards.rs`

**Reuse:** 2 — extends `tests/guards.rs`: the CORE-6.3 snapshot tests (lines ~82-158), `cargo_home_marks` (59-80) and the CORE-6.1 timing test (284-329)

**Interfaces:**
- Consumes: `cargo hive explain` (Task 8); `Hive::build_crate_failing` (Task 4)
- Produces: none

**Depends-on:** Task 8

- [ ] **Step 1: Write the tests** in `tests/guards.rs`:
  - Add `explain` to the command lists of the CORE-6.3 snapshot tests (`repo_add_status_and_doctor_leave_…` at lines ~82-126 and the scratch `CARGO_HOME` test at ~128-158), with the fixture target also holding a failed unit built by `build_crate_failing` before the snapshot: the checkout and target dir are byte-for-byte unchanged, `~/.cargo` shows the same marks, the scratch `CARGO_HOME` is byte-for-byte unchanged.
  - `explain_installs_nothing_in_the_build_path`: after `explain`, the checkout holds no `.cargo/` directory it did not have, and `~/.cargo` shows the same `config`, `config.toml`, `env`, `env.fish` and `bin/` marks (`cargo_home_marks`, `tests/guards.rs:59-80`).
  - `#[ignore]` `explain_on_klynt_stays_within_two_seconds_of_status`: same structure as the CORE-6.1 test (`:284-329`): cold (delete `snapshot.json` before each of the two commands) and warm median of 5 for both `status <klynt> --no-sizes --json` and `explain <klynt> --json`; assert the explain minus status difference is at most 2 s in each case.

  Run `cargo test --test guards`. These guards describe behavior Task 8 already has, so they pass on first run; prove each can fail by temporarily making `explain` write a file into the fixture target (revert after), and expect the snapshot test to fail.
- [ ] **Step 2: Run the timing test** `cargo test --release --test guards -- --ignored explain_on_klynt` on the proof machine and record cold and warm numbers in the task report.
- [ ] **Step 3: Verify.** Run the merge gate.
- [ ] **Step 4: Commit** `test(explain): guard that explain writes nothing outside hive's state, and time it on klynt`.

_Requirements: EXPLAIN-6.1, EXPLAIN-6.2, EXPLAIN-7.5_
