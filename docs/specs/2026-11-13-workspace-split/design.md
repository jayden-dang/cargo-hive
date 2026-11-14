# Design: Workspace split

Feature code: SPLIT
Status: Approved
Approved: 2026-11-13, by the operator (Jayden), reply relayed by the conductor: "approve"
Date: 2026-11-13
Requirements: ./requirements.md

<!--
Labels: (agent-decided) marks a choice ROAD-15 delegates ("where each module goes; which data structures change
and how, when results stay equal; test layout", ROAD-15.md:23-24), made in the unattended run and open to the
operator's review. Crate names are working names (the operator approves them before publish, ROAD-15.md:25).
Evidence digests (local, git-ignored): .skills/_pending-workspace-split/auto/{scan.md, design-scan.md,
research-build-rule.md, research-publish-workspace.md}.
-->

## Context

hive is one package, `cargo-hive` 0.1.0 (Cargo.toml:2-16): a library `cargo_hive` with 18 top-level modules
(src/lib.rs:1-19) and a three-line binary that calls `cargo_hive::cli::main_with` (src/main.rs:2). 22
integration-test binaries and 336 unit tests all link that one library, so any edit rebuilds everything. Two
dependency cycles run through it: `cargo_target` ↔ `doctor` (cargo_target.rs:574 calls
`doctor::drop_directory_toolchain`; doctor uses the spawn runner in cargo_target) and
{bench, cli, plan_tier, seed} (plan_tier/apply.rs:11 → `cli::cross_check_notes`; bench/check.rs:7 →
`cli::locate_checkout`). Raw OS calls sit outside `platform`: `libc::flock` in state.rs:166/192/212, `sigaction`
and `kill` in doctor.rs:181-233, `kill(-pid)` in cargo_target.rs:237, errno constants in seed, and
`cfg(target_os)` in explain.rs:675.

The binding constraint is Lock L2 (ROAD-15.md:8): an edit to how gc decides a dead unit rebuilds the gc crate and
the binary, not the seed crate. seed does not just share helpers with gc. It calls the closure-liveness verdict
itself (seed/source.rs:183, :327 match `liveness::liveness` on `Decided { live }` / `Undecided { reasons }`),
together with gc's accounting (`Ledger`, `remove_counted`, `measure`, `Footprint`; seed/evict.rs:758, :777) and
caps (`CRATE_DIR_CAP`, `DEPS_CAP`). The obvious layering, which design-scan.md §1.1 proposed and which puts
liveness in a shared core crate, is therefore ruled out: seed would depend on the dead-unit algorithm, and every
edit to it would rebuild seed. Instead the liveness *result types* go to core, the *algorithm* stays in the gc
crate, and seed receives the algorithm as a function value that the binary passes in (Decision 4).

The second constraint is L4/L6 (ROAD-15.md:12, :17): the installed binary, its receipts and its state files must
not change. The design therefore follows approach 1 (approaches.md, W5): untangle inside the one crate first,
then extract crates bottom-up as pure moves, so moves and behaviour-preserving changes never share a commit, and
the unchanged suite plus the SPLIT-4.1 liveness comparison prove the results equal.

## Decisions

1. **Layers and members** (agent-decided, working names): `cargo-hive-platform` → `cargo-hive-core` →
   `cargo-hive-state` → `cargo-hive-cargo` (the cargo adapter; its role word is open for the operator) →
   features `cargo-hive-doctor`, `cargo-hive-seed`, `cargo-hive-gc` → binary `cargo-hive`. A virtual root
   manifest with members under `crates/<role>/`. Every edge points to a strictly lower layer, and no feature
   depends on another feature (SPLIT-1.2, 1.3).
2. **OS boundary** (agent-decided, close-package D1/D2, W1/W3): outside platform there is no `unsafe`, no `libc`,
   no non-test `cfg(target_os)` and no `std::process::Command`. Enforcement: a crate-root
   `#![forbid(unsafe_code, reason = "only the platform crate may touch the OS (ARCH-15)")]`; a root `clippy.toml`
   with `disallowed-types`/`disallowed-methods` reasons (platform opts out with its own `[lints.clippy]` allows and
   does not inherit `[workspace.lints]`, since Cargo rejects combining the two); and a
   guard test. Probes: research-build-rule.md.
3. **Spawning**: platform owns the mechanism. It keeps today's bounded runner (deadline, pipe drain,
   process-group kill) and a plain unbounded `output`, because git's `run`/`has_commits` (git.rs:115, :176) have
   no deadline in 0.1.0 and must keep that. The cargo adapter owns every child's environment rule in one module,
   `env`. That covers `cargo metadata`, `git`, `rustup which`, doctor's probe build and bench's timed build, so
   ARCH-13 has one owner (W1 amendment). Each rule keeps the 0.1.0 environment exactly.
4. **Liveness inversion** (agent-decided, forced by L2): `Liveness`, `UnitRef`, `DeadUnit`, `Uncertain`, and
   `LivenessFn = fn(&dyn Fs, &TargetDir, SystemTime, Duration) -> Liveness`, live in core. The algorithm
   `liveness()` lives in `cargo-hive-gc`. seed's context carries a `LivenessFn`, which the binary fills with
   `cargo_hive_gc::liveness`. This is ADR-0003 (Proposed, docs/adr/0003-seed-receives-liveness-from-the-binary.md).
5. **Explain lives in the gc crate** (agent-decided): its only non-core edge is `DEPS_CAP` (explain.rs:11, moving
   to core), and it explains copies in a target dir, which is gc's domain. Its `host_fs`/`DirListFs` move into
   platform.
6. **Integration tests stay in `cargo-hive`** (agent-decided test layout): `tests/common/mod.rs:98` and `:413`
   use `env!("CARGO_BIN_EXE_cargo-hive")`, and `common::prove` imports plan_tier and gc (design-scan.md §4).
   Moving the few liveness-only test files would split `tests/common` for little gain. Unit tests move with
   their modules, so `cargo test -p <member>` runs a member's unit tests (SPLIT-1.5). The exception is every seed
   unit test whose call path reaches the liveness algorithm, through `source::holds` (seed/source.rs:163) or
   `source::live_registry_units` (seed/source.rs:310, :327, reached from `fill` at seed/fill.rs:136). These
   include the tests in seed/source.rs's test module, the fill tests that build a `Ctx` (seed/fill.rs:986-1232)
   and the seed/mod.rs tests that build one (seed/mod.rs:539-580). They move to
   `crates/cargo-hive/tests/seed_liveness.rs` with assertions unchanged.
   Seed unit tests that never reach liveness pass a `LivenessFn` stub that panics if
   called. A test that does reach liveness therefore fails loudly instead of passing on a fake verdict, which also
   finds any test the list above missed. seed has no dev-dependency on gc, and `cargo test -p cargo-hive-seed`
   never builds gc.
   The unsafe test helpers in seed (`set_var`/`remove_var` at seed/source.rs:829, :837; `libc::openat`,
   `unlinkat` and `flock` at seed/store.rs:1215-1237, :1534) move into `platform::testing`, because a crate-root
   `forbid(unsafe_code)` also covers `cfg(test)` code. Integration-test files in `crates/cargo-hive/tests/` are
   separate crates with no forbid attribute. Each carries a file-level
   `#![allow(clippy::disallowed_types, clippy::disallowed_methods, reason = "test fixtures drive real cargo and git")]`
   (89 `Command::new` sites in 20 files), and they use `libc` as a dev-dependency of `cargo-hive`.
7. **Versions** (L7 + agent-decided pin): one `[workspace.package] version`. Inter-member deps are
   `{ path = "../<role>", version = "=X.Y.Z" }` through `[workspace.dependencies]`. The first release after the
   split is proposed as 0.2.0, a 0.x minor bump, because the `cargo_hive::<module>` library paths change (out of
   scope to keep, requirements Out of Scope). The number shows in `--version` and receipts, so both the number and
   the path drop are the operator's (Ask me; requirements Open Questions). Nothing is built on the number. If the operator wants the old paths
   kept, the fallback is a re-export in `crates/cargo-hive/src/lib.rs` (`pub use cargo_hive_seed as seed;` and so on
   for every 0.1.0 module). That changes no other section.
8. **Publishing** (research-publish-workspace.md): `CARGO_TARGET_DIR=target/package-verify cargo publish
   --workspace --dry-run`, then the same command without `--dry-run`. Cargo 1.97.1 verifies each member against a
   temporary local registry and uploads in dependency order. A single-member dry-run fails while its deps are
   unpublished, so none is used. Every member's package carries the workspace `Cargo.lock` by default, even
   under an `include` list that does not name it.
9. **Data-structure rework** (agent-decided set, close-package D7, L12):
   - `UnitId(u32)` for unit indexes;
   - `UnitHash(u64)` for unit hashes, parsed from and printed as 16 lowercase hex;
   - `UnitKind`, an enum of the nine `KINDS` names (fingerprint.rs:31), including `Other` = `"other"`;
   - a CSR graph per profile (`offsets: Vec<u32>`, `targets: Vec<UnitId>`) in place of `Vec<(usize, usize)>`;
   - the hash index keyed by `UnitHash` in place of `HashMap<&str, Vec<usize>>`;
   - a `BitSet` for the closure walk's live marks in place of `Vec<bool>`.

   The walk keeps its lazy target-wide edge resolution, so `Uncertain` reasons stay as in 0.1.0 (tasks.md
   Task 25). A target-wide CSR for the walk is an Open Question for the operator (requirements).

   `UnitGraph` is never serialized to output (`ProfileReport.graph` is `#[serde(skip)]`, fingerprint.rs:130-131).
   `Unit`'s serialized fields keep their 0.1.0 form through `serialize_with`/`Display` on the new types, and
   `UnitHash` round-trips through `{:016x}` (SPLIT-4.6). The 0.1.0 capture comes from the v0.1.0 export
   (`git archive v0.1.0`, below). Its binary runs a fixed set of fixture commands under a `HIVE_HOME` in
   /private/tmp and saves the receipts, the state files and every `--json` envelope there. SPLIT-4.3 compares
   the new binary's output from the same fixtures against that capture. This work happens last, inside
   core and gc, after the extractions.
10. **Edition 2024**: `cargo fix --edition` on the single crate before the workspace exists. It enables the
    `rust-2024-compatibility` lint group, which includes `if-let-rescope` and `tail-expr-drop-order` (rustc 1.97.1
    `rustc -W help`). Those lints flag exactly a guard whose drop point moves. The SPLIT-5.2 evidence is the
    recorded output of that step: no `if-let-rescope` or `tail-expr-drop-order` warning left unresolved, which
    covers every `State::locked` caller mechanically. The named-site inspection (design-scan.md §6: no guard in an
    `if let` scrutinee or a tail expression) and the existing lock tests are the second check.

## Architecture

Vocabulary for every section below: **module**, **interface**, **implementation**, **seam** (public surface where
behavior is observable and substitutable). Prefer deep modules — an interface much simpler than what it hides.

### Workspace manifest and layer guard

Satisfies: SPLIT-1.1, SPLIT-1.2, SPLIT-1.3, SPLIT-1.5, SPLIT-3.3, SPLIT-3.4, SPLIT-3.6, SPLIT-5.1
Reuse: rung 4 — Cargo workspaces (`[workspace]`, `[workspace.package]`, `[workspace.dependencies]`,
`[workspace.lints]`, `resolver = "3"`); the guard test reuses `cargo metadata --format-version 1`, already parsed
with `serde_json` in src/cargo_target.rs.
Respects: ARCH-1 (no Cargo config or env is installed for users; this is hive's own manifest)
Surface:
- `Cargo.toml` (root): replace. It becomes the virtual workspace; its `[package]` fields move to
  `crates/cargo-hive/Cargo.toml`.
- `Cargo.lock`: replace. It is regenerated with the same third-party versions (`cargo update --workspace`
  only adds members).
- docs/agents/project.md (release step 2, "set `version` in `Cargo.toml`"): replace. The step now names
  `[workspace.package]`.
- tests/skill.rs:259-261 (`env!("CARGO_PKG_VERSION")` against the plugin files): replace. Its value is unchanged,
  because every member inherits the one version.
Interface: members `crates/{platform,core,state,cargo,doctor,seed,gc,cargo-hive}`, each with
`version.workspace = true`, `edition.workspace = true`, `[lints] workspace = true` (every member except
platform, which declares its own lints), and its own `include`, README
and license files. Guard test `crates/cargo-hive/tests/workspace.rs` asserts, from `cargo metadata`:
- every member's `edition == "2024"`;
- each member depends only on members of a lower layer (a fixed `LAYERS` table in the test), and no feature
  member depends on another;
- every inter-member dep requirement is `=<workspace version>`;
- only `cargo-hive` has a `bin` target.
Depth: n/a — extends Cargo's workspace model. The guard test's deletion test: callers would still need only the
layer table and the four checks.
Locality: extract — the root manifest splits into nine manifests. No source module changes here.

### `cargo-hive-platform`

Satisfies: SPLIT-2.1 (owner of the barred constructs), SPLIT-3.8, SPLIT-2.7
Reuse: rung 2 — src/platform/{mod,apfs}.rs stay as they are (`Fs` trait mod.rs:295, `Apfs` apfs.rs:47, lock
self-exec apfs.rs:172-245, `current()` mod.rs:444-455). These items move in unchanged:
- the spawn runner `output_with_deadline`, `run_bounded`, `Capture`, `Ran`, `ChildOutputError`, `kill_pgid`
  (cargo_target.rs:217-550, through `finish_readers`);
- signals `CANCEL`, `on_signal`, `install_signal_handlers` and `kill(pid, 0)` (doctor.rs:179-233);
- the three `flock` loops (state.rs:164-215);
- `utc_date`/`utc_timestamp` (state.rs);
- `host_fs`/`DirListFs` (explain.rs:674-764).

The errno checks become `io::ErrorKind` tests (rung 3, std); `Hooked` (mod.rs:344) becomes
`#[doc(hidden)] pub mod testing`.
Respects: ARCH-15 (all macOS-specific code stays here), ARCH-19 (lock tried without waiting: `flock_*` keeps
`LOCK_NB` + poll)
Surface:
- seed sites that match `libc::ENOSPC/EXDEV/EEXIST/ENOTEMPTY/ENOTDIR` (seed/place.rs:610; seed/fill.rs:506,
  :509, :750; seed/store.rs:766-767; seed/mod.rs:96): replace with `io::ErrorKind::{StorageFull,
  CrossesDevices, AlreadyExists, DirectoryNotEmpty, NotADirectory}`.
- doctor/cli callers of `install_signal_handlers`, `CANCEL`, `process_cancel`: replace (path change only).
- state's three lock methods: replace. They call `platform::flock::{exclusive_or_cancelled, shared_or_cancelled,
  try_exclusive}`.
- receipt.rs:5/:128 and explain.rs:17/:551 `utc_timestamp`: replace with `platform::clock::utc_timestamp`.
- tests/platform_locks.rs, gc_disk_image.rs, seed_fill.rs, store_gc.rs (`Apfs::new(env!("CARGO_BIN_EXE_cargo-hive")
  .into())`): replace (path only). The binary stays the holder (SPLIT-3.8).
Interface:
- `Fs`, `Apfs`, `Platform`, `current()`, `ensure_supported()`, `MACOS_ONLY_MESSAGE`;
- `proc::{Spec, output_with_deadline, run_bounded, output, Capture, Ran, ChildOutputError}`, where `Spec` is
  program, args, cwd, `env_clear: bool`, `env_set`, `env_remove`, `stdin_null`, `own_process_group`. Doctor's
  probe build (doctor.rs:751) and bench's timed build (bench/command.rs:143) use `env_clear` plus an allowlist;
- `signals::{install, cancelled}`, `alive(pid)`;
- `flock::{exclusive_or_cancelled(&File, cancel) -> Result<FlockGuard, FlockError>,
  shared_or_cancelled(…) -> Result<FlockGuard, FlockError>, try_exclusive(&File) -> io::Result<Option<FlockGuard>>}`,
  where `FlockError` is `Cancelled | Io(io::Error)` and `FlockGuard` unlocks on drop. This keeps `LOCK_NB`, the
  poll and the cancel semantics of state.rs:164-215;
- `testing` (`#[doc(hidden)]`): `Hooked`, `set_env`/`remove_env`, and the raw `openat`/`unlinkat`/`flock`
  helpers that seed's store tests need;
- `clock::{utc_timestamp, utc_date}`, `os_version()`, `host_fs()`.
Depth: `proc::Spec` is new (rung 7). Deletion test: callers would still need only "program, args, cwd and which
variables to clear, set or remove", which is the struct itself. That is small, and it hides `Command`, the
process group, the deadline thread and the pipe drain. The rest extends src/platform.
Locality: extend — platform gains five small modules. state, doctor, cargo_target, explain and seed lose their OS
code.

The mapping test sits beside it: `io::Error::from_raw_os_error(libc::X).kind()` equals the kind seed now
matches, for each of the five errnos. On rustc 1.97.1 / macOS SDK errno.h it gives 28→StorageFull,
18→CrossesDevices, 17→AlreadyExists, 66→DirectoryNotEmpty, 20→NotADirectory (probe `/private/tmp/ws-split-errno`).
A seed unit test checks that a non-EXDEV clone error still takes the non-EXDEV branch (design-notes.md).

### `cargo-hive-core`

Satisfies: SPLIT-4.3, SPLIT-4.4, SPLIT-4.5, SPLIT-4.6
Reuse: rung 2 — these move whole:
- fingerprint.rs, classify.rs, size.rs, receipt.rs;
- the target-dir types and identification from cargo_target.rs (`TAG_LINE` :17, `Layout` :26, `identify` :120,
  `identify_bounded` :129, `realpath_lossy` :189 (made `pub`), `Owner` :626, `TargetDir` :639);
- the liveness *result types* (liveness.rs:31-60), `embedded_hashes`, `unit_timestamp`;
- gc accounting: gc/ledger.rs, gc/account.rs, and `Scanned`, `Footprint`, `measure`, `record`, `scan`,
  `CRATE_DIR_CAP`, `DEPS_CAP` from gc/run.rs:20-456;
- plan_tier/locks.rs.
Respects: ARCH-9 (fingerprint format signature stays in fingerprint.rs), ARCH-10, ARCH-11 (ledger counting
unchanged)
Surface:
- `Unit.kind: String`, `hash_hex: String`, `UnitGraph.edges` (fingerprint.rs:78-120): replace in place. Readers
  are liveness, explain, seed/record, seed/source, plan_tier/build, and tests/seed.rs, tests/liveness.rs,
  tests/explain.rs (grep `hash_hex|\.kind\b|\.edges`). `UnitGraph` is never emitted. Every printed or persisted
  form that contains unit fields is frozen and is held by the SPLIT-4.3 comparison against the v0.1.0 capture
  (Decision 9).
- `receipt::Receipt` and records (receipt.rs:88-108): frozen. They move unchanged; the receipt keys are read by
  the plugin skill (plugin/skills/hive/SKILL.md) and by agents.
- `pub(crate)` items that now cross crates (`INVOKED_TIMESTAMP`, `read_capped`, `embedded_hashes`, `Ledger`,
  `Tally`, `remove_counted`, `RemoveFailed`, `entry_kind`): replace. They become `pub`, with `#[doc(hidden)]`
  where only hive's own crates use them.
Interface:
- `fingerprint::{read_profile, Unit, UnitId, UnitHash, UnitKind, UnitGraph, ProfileReport, Format}`;
- `target::{identify, identify_bounded, TargetDir, Owner, Layout}`;
- `liveness::{Liveness, UnitRef, DeadUnit, Uncertain, LivenessFn, embedded_hashes, unit_timestamp}`;
- `ledger::{Ledger, Tally, measure, Footprint, remove_counted, RemoveFailed}`, `caps::{CRATE_DIR_CAP, DEPS_CAP}`;
- `receipt::{Receipt, Scope, Measured, CheckRecord, Finding, Unmeasured, Outcome, PlanRecord, ReasonRecord,
  UndecidedRecord, SCHEMA}`; `classify::{classify, Class, CheckoutKind}`; `size::{tree_size, MAX_DEPTH}`;
  `locks::{acquire, acquire_capped, Acquired, Locked, Reason}`.
Depth: the moved modules extend existing code. These new parts are rung 7:
- `UnitId`, `UnitHash`, `UnitKind`: small value types. Callers keep knowing "an id, a hash that prints as 16 hex,
  a kind that prints as one of nine names".
- the CSR graph: callers keep `successors(id)` and `edge_count()`; `BitSet` keeps `insert`/`contains`.
- `LivenessFn`: callers keep one function signature.

Each interface is a line long and hides parsing, layout or the walk. Editing `embedded_hashes` or
`unit_timestamp` (fingerprint file naming, not the dead-unit decision) rebuilds seed, which L2 allows.
Locality: extract — the moves out of cargo_target.rs, liveness.rs and gc/run.rs split those files. fingerprint.rs
changes in place for the rework (Decision 9).

### `cargo-hive-state`

Satisfies: SPLIT-4.2
Reuse: rung 2 — src/state.rs moves whole, minus its `flock` loops and `utc_*` functions (now in platform).
Respects: ARCH-16 (metadata files only through state, atomically, under its one lock)
Surface: every `State`/`Tx`/`StoreLock` caller (registry, discover, plan_tier/record, seed, cli): replace
(path change only). The state files are frozen: no name, layout or meaning changes (ROAD-14 owns that). The
discharge is to build around them, held by SPLIT-4.2/4.3 tests against a 0.1.0-written state root.
Interface: `State::{open, locked, locked_or_cancelled, store_shared_or_cancelled, try_store_exclusive}`, `Tx`,
`StoreLock`, `Config`, `StateError`.
Depth: n/a — extends src/state.rs.
Locality: extract — one file. The lock primitive is delegated to platform.

### `cargo-hive-cargo` (cargo adapter)

Satisfies: SPLIT-2.5 (environment policy), SPLIT-2.6
Reuse: rung 2 — these move whole:
- git.rs, registry.rs, discover.rs;
- `Resolved` (cargo_target.rs:208), `resolve`, `caller_overrides`, `TARGET_VARS`, `METADATA_DEADLINE` from
  cargo_target.rs (:208, :553-620);
- the toolchain family from doctor.rs (`Toolchain`, `resolve_toolchain*`, `drop_directory_toolchain`,
  `caller_toolchain_kept*`, :355-397 and helpers);
- `Ctx`, `checkout_walk`, `split_checkout_faults`, `faults_that_can_hide_an_owner` from plan_tier/mod.rs:18-68.

New: the `env` module (rung 7, the minimum that gives ARCH-13 one owner), holding every child's environment rule
as it is in 0.1.0:
- `env::metadata(&mut Spec, parent)`: cargo_target.rs:564-574;
- `env::git(&mut Spec, parent)`: git.rs:70-74;
- `env::rustup_which(&mut Spec, parent, ResolveEnv)`: doctor.rs:428, :514;
- `env::probe_build(&mut Spec, parent, cargo_home, rustup_home, toolchain_name)`: doctor.rs:748-760;
- `env::timed_build(&mut Spec, parent, cargo, rustc, toolchain_name)`: bench/command.rs:84-143.

Here `parent` is `&[(OsString, OsString)]` (production passes `std::env::vars_os()`). `env` owns `ResolveEnv` and
takes plain data, never `Toolchain`, so `toolchain` depends on `env` and not the reverse (tasks.md Task 5).
Respects: ARCH-13, ARCH-17 (ownership only from `cargo metadata --no-deps --locked --offline`, flags unchanged)
Surface:
- cargo_target.rs:565-574 (metadata env) and git.rs:70-74 (git env): replace. They call `env::metadata` /
  `env::git`, and the resulting environment is equal (SPLIT-2.6).
- doctor.rs:428, :514, :748 and bench/command.rs:113-114, :142: replace. They call the matching `env::*` function,
  and the resulting environment is unchanged (design-scan.md §2.1). The same env-diff test covers each.
- seed/evict.rs:12, :572 and seed/fill.rs:868 (`doctor::resolve_toolchain*`): replace with the adapter paths.
  This removes the seed → doctor edge.
- seed and gc callers of `plan_tier::Ctx` / walk fns: replace (path).
Interface: `discover::{discover, Scope, Checkout, …}`, `registry::*`, `git::Git`, `metadata::resolve`,
`toolchain::{Toolchain, resolve_toolchain_until, …}`, `Ctx`, `env::{metadata, git, rustup_which, probe_build,
timed_build}`.
Depth (for `env`): if it vanished, each caller would need to know which variables its kind of child must not
inherit and which it must be given. Today that is five scattered rule sets, two of them allowlists. The interface
is "name the kind of child", which is small, and it hides every variable list.
Locality: extract — cargo_target.rs and doctor.rs split. The {cargo_target, doctor} cycle disappears, because
`resolve` and `drop_directory_toolchain` land in the same crate.

### `cargo-hive-doctor`

Satisfies: none — infrastructure (the doctor member; its no-feature-dependency property is SPLIT-1.3, owned by the workspace guard)
Reuse: rung 2 — doctor.rs minus the toolchain family (now in the adapter) and the signal/kill code (now in
platform). It depends on neither gc nor seed (design-scan.md §1.4).
Surface: cli callers of `doctor::run`, `Capabilities`, `require`, `toolchain_failure_exit`: replace (path).
`capabilities.json` is frozen (SPLIT-4.2).
Interface: `run`, `Capabilities`, `Capability`, `require`, `DoctorEnv`, `toolchain_failure_exit`.
Depth: n/a — extends doctor.rs.
Locality: extract.

### `cargo-hive-seed`

Satisfies: SPLIT-1.4 (seed stays Fresh on a gc edit)
Reuse: rung 2 — src/seed/* moves whole. Its edges are rewired downward: liveness result types, caps, ledger and
locks come from core; toolchain, `Ctx` and the walk fns from the adapter.
Respects: ARCH-5, ARCH-19
Surface:
- seed/source.rs:183, :327 (`liveness::liveness(...)`): replace with a call through a `LivenessFn`. `holds()`
  (seed/source.rs:163, `pub(crate)`, called from seed/evict.rs:67) gains a `liveness: LivenessFn` parameter, and
  `EvictCtx` (seed/evict.rs:24) gains a `liveness` field.
- `EvictCtx` construction sites (cli.rs:1050, tests/store_gc.rs four sites): replace, passing
  `cargo_hive_gc::liveness`.
- every seed unit test that reaches `holds` or `live_registry_units` (Decision 6): move to
  `crates/cargo-hive/tests/seed_liveness.rs` with assertions unchanged. `holds`, `live_registry_units` and any
  private helper those tests call become `#[doc(hidden)] pub`.
- production `Ctx` construction sites (cli.rs, bench/mod.rs) and tests/seed*.rs: replace. Each passes
  `liveness: cargo_hive_gc::liveness`. Unit-test `Ctx` builds that stay in seed pass the panicking stub
  (Decision 6).
Interface (the `LivenessFn` seam is agent-decided, Decision 4, ADR-0003 Proposed): unchanged seed entry points (`seed`, `fill`, `evict`, `status counts`). `Ctx` and `EvictCtx` each gain
one field, `liveness: LivenessFn`.
Depth: n/a — extends src/seed.
Locality: extract — seed moves whole. One field is added to `Ctx` (in the adapter). The edge to gc disappears.

### `cargo-hive-gc`

Satisfies: SPLIT-4.1
Reuse: rung 2 — these move whole:
- gc.rs, gc/{plan,run}.rs (minus the accounting moved to core);
- the liveness algorithm from liveness.rs (`liveness()` :210 and its private graph walk);
- plan_tier/{apply,build,guard,orphan,record}.rs;
- explain.rs;
- `cross_check_notes` (cli.rs:915). Moving it here removes the plan_tier → cli edge.
Respects: ARCH-17, ARCH-18, ARCH-20
Surface:
- cli.rs callers of `cross_check_notes`, `plan_tier::apply::exit_code` (cli.rs:1215), `explain::*`: replace
  (path).
- tests/guards.rs:437 `include_str!("../src/liveness.rs")`: replace. The no-process guard reads both liveness
  files, the types (`../../core/src/liveness.rs`) and the algorithm (`../../gc/src/liveness.rs`), with the
  intermediate paths in tasks.md Tasks 11, 16 and 21.
- `plans.json` (plan_tier/record.rs:12): frozen (SPLIT-4.2/4.3).
Interface: `liveness()`, `gc::{auto, plan, Scope}`, `plan_tier::{build, apply, orphan, record}`,
`explain::run`, `cross_check_notes`.
Depth: n/a — extends existing modules.
Locality: extract.

The SPLIT-4.1 liveness report is an `#[ignore]` integration test `crates/gc/tests/liveness_report.rs`. It reads
`HIVE_REPORT_TARGET`, `HIVE_REPORT_NOW` (unix seconds), `HIVE_REPORT_WINDOW_DAYS` and `HIVE_REPORT_OUT` (a path
under /private/tmp). It builds a `TargetDir` with `identify` + `read_profile` (the same calls the 0.1.0 unit tests
make, liveness.rs:744-754), runs `liveness(&*platform::current()?.fs, &target, now, window)`, and writes
`{live: [UnitRef], dead: [UnitRef], undecided: [String]}` sorted as JSON.

The 0.1.0 side is the same file compiled against an export of the v0.1.0 tag (`git archive v0.1.0 | tar -x -C
/private/tmp/hive-0.1.0`, a read-only operation on the repo) with its `cargo_hive::` paths, and built `--offline`.
Both read the target through `Fs` and fingerprint files only: no `cargo metadata`, no process, no write
(liveness.rs:1-3). With the same `now` and window, the two outputs must be byte-equal. A first comparison runs before
step 1. Because klynt's target changes in between, each checkpoint after steps 1, 4 and 5 re-runs the 0.1.0
report and the new one back to back with one fresh `now` (W5a; tasks.md Task 3's checkpoint rule).

### `cargo-hive` (binary)

Satisfies: SPLIT-3.1, SPLIT-3.2, SPLIT-3.5, SPLIT-3.7, SPLIT-3.9
Reuse: rung 2 — cli.rs (minus `cross_check_notes`), bench/*, main.rs; `locate_checkout` and `Refusal` stay
(cli.rs:1579, :1586, used by cli and bench in this crate). lib.rs keeps `pub mod cli; pub mod bench;` for its own
tests (tests/skill.rs:434, tests/skill/names.rs:7-11).
Respects: ARCH-8, ARCH-14
Surface:
- `cargo_hive::<module>` paths outside `cli`/`bench`, read by external library users of 0.1.0: replace. They are
  removed in 0.2.0 (agent-decided, close-package D6), and 0.1.0 stays on crates.io unchanged. This is flagged for
  the operator, with Decision 7's re-export as the fallback.
- tests/*.rs using `cargo_hive::{seed,plan_tier,platform,state,…}` (scan.md §4: 57 + 28 + 20 + … refs): replace.
  They import from the member crates, which `crates/cargo-hive/Cargo.toml` lists as `[dev-dependencies]` where not
  already normal deps.
- `hive_version` (receipt.rs:124, `env!("CARGO_PKG_VERSION")` evaluated in core) and `--version` (cli.rs:15,
  evaluated in cargo-hive): replace, value equal. Both read the one workspace version, held by SPLIT-3.5's test.
Interface: the installed `cargo-hive` command tree (cli.rs:12-130) including hidden `__hold-lock`/`__try-lock`,
and the macOS-only exit (cli.rs:147-149).
The binary wires `cargo_hive_gc::liveness` into seed's `Ctx` (agent-decided, ADR-0003 Proposed).
Depth: n/a — extends cli.rs.
Locality: extract — the package moves to `crates/cargo-hive/`, taking tests/ with it.

The SPLIT-3.1 check is a test that walks `Cli::command()` (clap) and prints every subcommand, argument, flag,
default and help line as text. It is compared with a dump captured once from the v0.1.0 export, with the version
line excluded. SPLIT-3.2's key/type check compares `--json` envelopes from the integration tests' fixtures with a
0.1.0 capture.

### OS-boundary guard

(agent-decided reach and mechanism: Decision 2, W1/W3)

Satisfies: SPLIT-2.2, SPLIT-2.3, SPLIT-2.4
Reuse: rung 4 — rustc `forbid(unsafe_code, reason)`, clippy `disallowed-types`/`disallowed-methods` with
`reason`, and Cargo `[lints]` (probed, research-build-rule.md). The guard test reuses `cargo metadata` and a
plain source scan, the same shape as tests/guards.rs:436-440.
Surface:
- test-only `Command` sites in src (cargo_target.rs:788/803/816, doctor.rs:1673/1797/1800, git.rs:269,
  cli.rs:1948, seed/source.rs:996, seed/store.rs:940/1251, fingerprint.rs:917): replace. Each test module carries
  `#[allow(clippy::disallowed_types, clippy::disallowed_methods, reason = "test fixture")]`.
- the 89 `Command::new` sites in 20 files under tests/: replace. Each file gets a file-level allow (Decision 6).
- unsafe test helpers in seed: move to `platform::testing` (Decision 6).
Interface:
- every non-platform `lib.rs` opens with the forbid attribute;
- root `clippy.toml` lists `std::process::Command` with reason "only the platform crate may touch the OS";
- `crates/cargo-hive/tests/os_boundary.rs` fails with that text and the member's name when a non-platform member:
  - has `libc` in its normal `[dependencies]` (dev-dependencies are test code);
  - has a `cfg(target_os` that is not test code;
  - is missing the forbid attribute.

  "Test code" means an attribute whose own predicate contains `test` (`cfg(all(test, target_os = …))`,
  plan_tier/guard.rs:92, gc/account.rs:156), or any line inside a `#[cfg(test)]` module or item. The scan tracks
  brace depth from the `#[cfg(test)]` line, which also covers a separate `#[cfg(target_os)]` under
  `#[cfg(test)]` (liveness.rs:655-656). Files under `tests/` are test code.
Depth: n/a — configuration plus one test.
Locality: leave — no production module changes beyond the attribute line.

### Edition 2024 migration

Satisfies: SPLIT-5.2
Reuse: rung 4 — `cargo fix --edition` (Cargo).
Surface: RPIT sites that capture more under 2024 (liveness.rs:626, seed/store.rs:1304, gc/run.rs:818): replace
with `use<..>` bounds only if the compiler asks. `set_var`/`remove_var` in tests: already `unsafe`; the arms at
tests/seed_fill.rs:54-55 and tests/common/mod.rs:476-477 are wrapped if the compiler asks.
Interface: none.
Depth: n/a.
Locality: leave — mechanical, in step 2 on the single crate.

### Release steps, fresh-probe and docs

Satisfies: SPLIT-6.1, SPLIT-6.2, SPLIT-6.3, SPLIT-6.4, SPLIT-6.5, SPLIT-6.6, SPLIT-7.1, SPLIT-7.2, SPLIT-7.4, SPLIT-7.5
Reuse: rung 2 — the existing release steps (docs/agents/project.md:130-137), scripts/fresh-probe.sh,
tests/skill.rs, tests/skill/names.rs, tests/liveness.rs:279-293.
Surface:
- docs/agents/project.md:
  - verify table (:79-84): `--workspace` where a command needs it;
  - single-test lines (:90-92): `cargo test -p <member> --test <file>` and `cargo test -p <member> --lib`;
  - task gate (:94-95): "tests of the changed member and the members above it";
  - run locally (:115-124): unchanged `target/debug/cargo-hive`;
  - release steps (:130-137): step 2 sets `[workspace.package] version` and commits `release: vX.Y.Z` before packaging; step 3 runs
    `cargo package --workspace --list --allow-dirty`, then `CARGO_TARGET_DIR=target/package-verify cargo publish --workspace
    --dry-run --allow-dirty`, then fresh-probe per member; step 4 tags and pushes that commit; step 5 first stops unless the line `Crate names approved:` in this file holds a date
    (SPLIT-6.4), then runs `cargo publish --workspace` with no `--dry-run` and no `--allow-dirty`. Steps 1–4 run on working names. The line is dated 2026-11-14.

  All replace.
- scripts/fresh-probe.sh: replace. Its default file `src/size.rs` no longer exists. It takes `<member> <file>`
  pairs and runs once per member.
- tests/skill.rs:15, :25 (`CARGO_MANIFEST_DIR` as repo root): replace with `repo_root()`, the nearest ancestor
  whose `Cargo.toml` has `[workspace]`.
- tests/skill/names.rs:265: replace. It sweeps `crates/*/src` instead of `CARGO_MANIFEST_DIR/src`.
- tests/liveness.rs:279-286 `the_package_ships_no_test_file`: replace. It runs over every member's manifest.
- docs/standards/code.md:15, testing.md:22-25: replace (SPLIT-7.2).
- README.md:113: replace with `cargo install --path crates/cargo-hive`.
Interface: release steps 1–6 as listed. `fresh-probe.sh <member> <file> …`.
Depth: n/a.
Locality: extend.

### Build-time measurement (NFR)

Satisfies: SPLIT-8.2 (SPLIT-8.1 struck; the build-time medians are recorded as evidence only)
Reuse: rung 2 — tests/guards.rs `#[ignore]` timing tests (:329, :375) move unchanged with the binary package.
`bench/*.sh` is not reused (it times hive's own commands, not `cargo build`).
Interface: a recorded procedure in `.skills/` evidence (code.md "Measured claims"):
- touch one line in `crates/gc/src/run.rs`, `cargo build`, and record the time, 5 times;
- baseline: the same line in `src/gc/run.rs` of the v0.1.0 export, built 5 times, in the same session as the
  SPLIT-4.1 baseline.
Depth: n/a.
Locality: leave.

## Open items

- Resuming a partial publish. `cargo publish --workspace` is not atomic, and whether a re-run skips versions
  already on the index is unprobed (research-publish-workspace.md). The release steps say only "stop and report
  which members uploaded". The resume procedure is decided when it is probed at the first real release, not in
  this run.

## Seams for testing

| Seam | Kind | Covers |
|---|---|---|
| `crates/cargo-hive/tests/workspace.rs` (cargo metadata checks) | integration | SPLIT-1.1, 1.2, 1.3, 3.3, 3.4, 3.6, 5.1 |
| `cargo build -v` after touching a gc-crate file (scripted in the same test file, `#[ignore]`, builds into a temp `CARGO_TARGET_DIR`) | integration | SPLIT-1.4, 1.5 |
| `crates/cargo-hive/tests/os_boundary.rs` + a deliberately bad fixture member built in a temp dir | integration | SPLIT-2.1, 2.2, 2.3, 2.4 |
| platform `proc`/`flock`/`signals` unit tests; errno→kind mapping test | unit | SPLIT-2.5, 2.7 |
| adapter `env` unit test: for each of `metadata`, `git`, `rustup_which`, `probe_build`, `timed_build`, the `Spec` built from a fixed parent environment yields the same final child environment (cleared or not, every set and removed variable) as 0.1.0's code yields from that parent | unit | SPLIT-2.6 |
| existing integration suite through `CARGO_BIN_EXE_cargo-hive` (tests/common/mod.rs:98) | integration | SPLIT-3.2, 3.7, 3.8, 4.2, 5.2 |
| clap tree dump vs v0.1.0 capture (`Cli::command()`) | unit | SPLIT-3.1 |
| receipt version test (`hive_version` == `--version`) | integration | SPLIT-3.5 |
| `crates/cargo-hive/tests/seed_liveness.rs` (every seed unit test that reaches `holds` or `live_registry_units`, from seed/source.rs, seed/fill.rs and seed/mod.rs, run with the real `cargo_hive_gc::liveness`) | integration | SPLIT-1.4 (seed keeps working through `LivenessFn`) |
| `crates/gc/tests/liveness_report.rs` (`#[ignore]`, klynt) vs v0.1.0 export | integration (manual, proof machine) | SPLIT-4.1 |
| capture comparison: the new binary vs the v0.1.0 export's binary on the same fixtures (receipts, state files, `--json` envelopes under /private/tmp, Decision 9) | integration | SPLIT-4.3, 4.5 |
| core unit tests: `UnitHash` `{:016x}` round trip, `UnitKind` names incl. `other`, CSR closure equals the 0.1.0 closure on the liveness fixtures | unit | SPLIT-4.4, 4.6 |
| `cargo package --workspace --list`, `cargo publish --workspace --dry-run`, `fresh-probe.sh` | manual (release steps) | SPLIT-6.1, 6.2, 6.3, 6.4 |
| tests/skill.rs, tests/skill/names.rs, tests/liveness.rs manifest test | integration | SPLIT-7.4, 7.5, 6.2 |
| running the docs' commands at the workspace root (task-close check) | manual | SPLIT-7.1, 7.2, 3.9 |
| release step 5 name check (`cargo search` per member) and step 6 crates.io-install comparison | manual (release) | SPLIT-6.5, 6.6 |
| build-time procedure (evidence only); tests/guards.rs `--ignored` | manual (proof machine) | SPLIT-8.1, 8.2 |

## Coverage check

Each ID appears in exactly one Satisfies line:
- workspace 1.1, 1.2, 1.3, 1.5, 3.3, 3.4, 3.6, 5.1;
- platform 2.1, 2.7, 3.8;
- core 4.3, 4.4, 4.5, 4.6;
- state 4.2;
- adapter 2.5, 2.6;
- seed 1.4;
- gc 4.1;
- binary 3.1, 3.2, 3.5, 3.7, 3.9;
- OS-boundary guard 2.2, 2.3, 2.4;
- edition 5.2 (5.1 is under workspace);
- release/docs 6.1–6.6, 7.1, 7.2, 7.4, 7.5;
- NFR 8.2 (~~SPLIT-8.1~~ struck).

SPLIT-2.5's mechanism half is built in platform but listed once, under the adapter. Doctor carries no Satisfies
of its own (infrastructure). ~~SPLIT-7.3~~ is retired (moved to SPLIT-3.9). No ID is unmapped.

Requirements sync-back in this design (re-surfaced with it):
- the K5 Open Question closes, because `cargo publish --workspace` works (Decision 8);
- SPLIT-6.2's library-member `Cargo.lock` branch resolves to "yes";
- SPLIT-4.1 names the fixed `now` and window;
- SPLIT-2.5 no longer says every child is spawned through the *bounded* runner, because git's two calls have no
  deadline in 0.1.0. It now names one adapter module for every child's environment;
- SPLIT-2.1 and SPLIT-2.4 say the `libc` rule applies to normal dependencies, since dev-dependencies are test code
  (design review #12, #13).
