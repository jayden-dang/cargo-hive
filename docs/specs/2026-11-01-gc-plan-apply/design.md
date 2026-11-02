# Design: Plan-tier GC (stored plans of dead-unit files and orphan targets, applied after a recheck)

Feature code: GCPLAN
Status: Approved
Date: 2026-11-01
Requirements: ./requirements.md

## Context

Today `cargo hive gc` is the auto tier only: it deletes superseded incremental sessions and leftover rustc temp entries, one lock per owner, and nothing else (`src/cli.rs:726-895`, `src/gc/run.rs:68`). LIVE (`src/liveness.rs:210`) already decides which units of a Cargo target dir are dead and lists their files, but no command calls it: ROAD-5 deletes nothing and writes nothing. CONTEXT.md fixes the plan-tier shape as `cargo hive gc --plan` then `apply`, and agents apply plans of every class themselves, with no owner step (ADR-0002).

The binding constraint is time. A plan is made at one moment and applied at another, and a build can run in between, so "dead" is a fact about the past. ARCH-20 therefore requires a recheck at apply, and ARCH-19 forbids deleting while any process holds the lock that guards the entry. Cargo's locks are whole-build and per profile, but liveness is a closure over the whole target dir (CONTEXT.md, Closure liveness). So the design locks every profile of the target before it recomputes anything, and compares the recomputed dead set against the stored plan as an upper bound. The alternative this rules out is an `apply` that re-derives its own deletion list ("scope only, no stored list"): it would delete things no `gc --plan` receipt ever listed.

Second constraint: agents and the owner share one user account, so hive's state files can be edited by an agent. Hive does not claim protection against that (vision non-goal), but it does not make it easier: the class and owner stored in a plan are never trusted, they are compared with a fresh resolution (F9), and every path in a plan is treated as untrusted input and checked against its target dir before it is removed.

Retrieval (manual approximation of `load-subgraph`, since the fresh package was not produced by its passes): neighbors by shared paths in `src/cli.rs`, `src/receipt.rs`, `src/state.rs`, `src/gc/`, `src/platform/` are LIVE, AUTOGC, CORE, EXPLAIN and SKILL; `owns_coverage` was counted by hand as 5 of 5 `tasks.md` files naming files, not by the skill, so treat "no other owner of plan/apply" as unverified beyond those five specs. Scan digests: `.skills/GCPLAN/scan.md`, `.skills/GCPLAN/design-scan.md`. Research: `.skills/research/2026-11-01-cargo-build-locks.md`.

## Decisions

1. **Plans are stored records, applied once.** A plan lives in hive's state as one manifest file, `plans.json`, holding every plan and a tombstone for each consumed one. One manifest needs no new `Tx` method, rewrites atomically under the state lock (ARCH-16), and makes apply-once a single compare-and-rewrite. Consuming a plan strips its path list so the file shrinks.
2. **Consume point.** A plan is consumed the moment `apply` passes its gates, before ownership is resolved. An unknown id and a capability refusal consume nothing. Every later outcome (refused, applied, partial, cancelled) leaves the plan consumed. This is the owner's "apply has started" rule.
3. **Apply is two phases.** Ownership is resolved first (it reads no file of the target dir). Phase A, for all targets in sorted path order: take every lock, check that the profile set read under the locks equals the locked set, re-read fingerprints, recompute liveness, compare with the plan. Phase B only if no target refused: delete. Locks are held from Phase A to the end of Phase B. Try-locks never wait, so there is no deadlock to order against, and a build that starts after the first lock was taken is refused by its own lock when it tries Cargo's exclusive lock on a profile we hold, or is seen by a lock we then fail to take.
4. **Stale means one of five things:** a planned path is no longer in the recomputed dead set; the target is `Undecided`; a planned path is missing or its (kind, device, inode) differ; the owner or class differs, or ownership is unresolved; a profile's format is unsupported. Newly dead units are ignored (GCPLAN-3.4).
5. **Fresh ownership at both plan and apply, over every registered repo.** Both call `discover::targets` with an empty `Snapshot`, discard it, and resolve the checkouts of all registered repos whatever repos the command names; the named repos only filter which targets are planned or applied. This is what makes the orphan rule of CORE-4.5 hold for a scoped run. The cache is never read or written by plan tier. Cost: one offline `cargo metadata` per workspace per call.
6. **Lock set.** For each profile and triple directory that `cargo_target::identify` returns for the target dir: `.cargo-build-lock` and `.cargo-lock` where present, plus `.cargo-artifact-lock` where a unit plan lists a path under `examples/`. An orphan removal takes `.cargo-artifact-lock` whenever that file is present. A target whose build directory is split from its target directory is `Undecided` in LIVE, so it is never planned and its build directory needs no lock here. A profile directory with a `.fingerprint` directory and none of those files is skipped as "lock missing". After locking, the held file's `(dev, ino)` is compared with `entry_stat` of the path; a mismatch, a missing path, or a lock that reports no identity skips the target (fail closed).
7. **No authorization** (amended 2026-11-02, ADR-0002). `apply` takes an id and no authorization value. A plan's class only selects the deletion path: unit by unit for `secondary` and `protected`, whole directory for `orphan`.
8. **Outcomes.** `applied` (every target applied), `partial` (a target was skipped, including every target with reason `all_targets_skipped`; a deletion failed; or the run was cancelled), `refused`. Exit: 0 for `applied` and for `partial` with neither a deletion failure nor a cancel; 1 for `refused`, a failure, and `partial` because a deletion failed; 130 for a cancel (outcome `partial`, reason `cancelled`). An unlogged receipt still forces 1.
9. **Receipt v3 is additive.** A receipt gains optional `plans`, `undecided` and `nothing_to_do` keys, present only when the command is `gc --plan` or `apply`, which also sets `schema` to 3. Existing commands keep emitting the 12 keys and `schema` 2.
10. **Window.** `gc_window_days` in `config.json`, a whole number from 1 to 365, default 7; the plan records it and `apply` uses it with a fresh `now`. `State::config()` carries the raw optional value and validates nothing, so `status`, `gc` and `explain` never fail on a key they ignore; `plan_tier` validates it when `gc --plan` starts (GCPLAN-1.9).
11. **Identity fence** is (entry kind, device, inode) from `Fs::entry_stat`; no mtime, because a build that touches a file's mtime but not its identity is not a reason to refuse.
12. **Delete by kind through one guard, in a safe order.** A path is removed only after its canonical parent is shown to lie inside the target dir and its lstat matches what the caller expects; directories go through `remove_tree`, files and symlinks through `remove_file`, nothing follows a link. Per unit the order is: dep-info `.d` files, other artifacts, fingerprint directory; with no `.d` listed, the fingerprint directory goes first. In Cargo 0.98.0 a missing dep-info file makes the unit stale (`find_stale_item` returns `StaleItem::MissingFile`, `src/cargo/core/compiler/fingerprint/mod.rs`), so deleting the `.d` or the fingerprint first makes every prefix stale. The reverse order was rejected on the review's finding that a unit with its rlib gone but its `.d` and fingerprint present looks fresh and would then fail to link (E0463); that half is not verified in source and is what GCPLAN-8.5 tests. Cancellation is polled between units only. Paths listed by two dead units are kept once, under the first.
13. **Housekeeping.** At the next `gc --plan`, plans and tombstones older than 30 days are dropped, and tombstones beyond the newest 256 are dropped oldest first. A live (unconsumed) plan is never dropped by the cap.
14. **Orphan removal is reached from `apply`** (amended 2026-11-02, ADR-0002): an `orphan` plan calls `plan_tier::orphan::remove`, and every other class runs `execute`.
15. **Smaller choices:** `--plan` conflicts with `--dry-run` in clap, because a plan never deletes; an orphan candidate owned under the caller's environment is still planned and carries a receipt note (GCPLAN-5.5), confirmed by the owner on approving this design (2026-11-01); Cargo's generation is not gated in `doctor` (the owner's lock rule); module is `plan_tier`.
16. **ADR-0002 records dropping the owner step** (amended 2026-11-02); nothing else here is hard to reverse. Documentation owed: CONTEXT.md gains Plan, Plan class and Outcome; the "a missing lock file needs no lock" lines of `docs/specs/2026-10-30-closure-liveness/design.md` and the `TryLock::Gone => {}` arm of the proof harness (`tests/common/prove.rs`, ~line 1002) are corrected through `realign-spec`; the exit-code table in `docs/standards/errors-logging.md` is unchanged.

ARCH invariants this design relies on: ARCH-4, 7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 18, 19, 20.

## Architecture

Vocabulary for every section below: **module**, **interface**, **implementation**,
**seam** (public surface where behavior is observable and substitutable). Prefer
deep modules — an interface much simpler than what it hides.

Layout: a new module `src/plan_tier/` (named so as not to be confused with `src/gc/plan.rs`, the auto tier's action planner) with `mod.rs`, `record.rs`, `build.rs`, `apply.rs`, `orphan.rs`, `guard.rs`. Its public surface is `make`, `apply` and the record types the receipt needs.

### Plan records and store (`src/plan_tier/record.rs`)

Satisfies: GCPLAN-1.7, GCPLAN-1.14, GCPLAN-2.11, GCPLAN-3.7, GCPLAN-3.9, GCPLAN-8.4
Reuse: rung 2 — `state::Tx::{read,write}` under `State::locked_or_cancelled` (`src/state.rs:110-150`); new types are rung 7 because `Liveness`, `DeadUnit` and `Uncertain` derive no serde.
Respects: ARCH-16
Interface: `Store::insert(plans)`, `Store::get(id) -> Found | Unknown`, `Store::consume(id) -> Taken(Plan) | Consumed | Unknown`, `Store::prune(now)` (decision 13); `Plan { id, class, window_days, made_at, scope_repos, planned: Option<Bytes>, targets, state }`, `PlannedTarget { path, owner, planned: Option<Bytes>, entries | orphan }`, `PlannedEntry { path, unit, kind, dev, ino }` (`unit` is the profile directory relative to the target dir, then `/`, then the unit directory name, so units of two triples never collide), `PlanId`. `plans.json` is read and written only here.
Depth: if this module vanished, callers would have to know the file name, the tombstone rule, the id format and the compare-and-rewrite; the interface is four verbs.
Locality: new file; `src/state.rs` `extend` only by exporting the manifest file name (one constant).

`plans.json` is `{ "plans": [ ... ] }`, `Default` for first read. `consume` runs inside one `locked_or_cancelled`: read, find the live plan, replace its `targets` with an empty list and set `state = Consumed { at }`, write, and hand back the removed `Plan`. Two concurrent applies of one id both take the state lock; the second reads a consumed record. An id is 16 lowercase hex characters derived from the clock, the process id and a counter, and checked against the manifest for collision before insertion. A failed write leaves the old file, because `atomic_write` renames last (`src/state.rs:270`). The planned bytes are stored with the plan as `gc --plan` measured them.

### Plan building (`src/plan_tier/build.rs`)

Satisfies: GCPLAN-1.1, GCPLAN-1.2, GCPLAN-1.3, GCPLAN-1.4, GCPLAN-1.5, GCPLAN-1.6, GCPLAN-1.11, GCPLAN-1.12, GCPLAN-1.13, GCPLAN-5.1, GCPLAN-5.2, GCPLAN-5.3, GCPLAN-5.4, GCPLAN-5.5, GCPLAN-5.6
Reuse: rung 2 — `discover::checkouts` and `discover::targets(.., &mut Snapshot::default(), ..)` (`src/discover.rs:120`, `:506`), `liveness::liveness` (`src/liveness.rs:210`), `gc::measure` for planned bytes (`src/gc/run.rs:393`), `Fs::entry_stat` for the identity of each path; the class-of-target lookup and the plan builder are new, rung 7.
Respects: ARCH-4, ARCH-7, ARCH-8, ARCH-17, ARCH-18
Interface: `make(ctx: &Ctx, scope_repos: &[PathBuf], cancel: &(dyn Fn() -> bool + Sync)) -> Made { plans, undecided, nothing_to_do, unmeasured, walk_faults }`. `Ctx` carries `state: &State`, `fs`, `Config`, `now`, `account_bytes`, every registered `Repo` and the registered mains (what `select` and `live_checkouts` already produce in `gc_cmd`).
Depth: if this module vanished, callers would have to know how to turn ownership, class and a liveness verdict into plan groups; they keep a single call.
Locality: new file; reads `discover` and `liveness` unchanged (`leave`); the class lookup is a private function of this module, because `Class` and `Owner` are already public and `discover.rs` need not depend on plan types.

For each target the class is: the class of its owner checkout when `Owner::Owned`, `Protected` when the owner checkout is absent from the checkout list (ARCH-7) or a protected checkout also owns it (the `protected_owner` rule), `Orphan` for `Owner::OrphanCandidate`, and no plan for `Owner::Unresolved` (reported as a finding). Ownership is resolved over the checkouts of every registered repo with `all_repos = true`, and `scope_repos` then filters which targets are considered, so a target in repo A's tree that repo B owns is never an orphan when only A is named (CORE-4.5). `liveness()` runs on the `TargetDir` that `discover::targets` produced. A `Decided` target whose dead units list no path goes to `nothing_to_do`; `Undecided` to `undecided` with each `Uncertain` and its `clears_when()`. Entries copy the paths of `DeadUnit.files` in order, dropping a path already listed by an earlier unit, and add `entry_stat` identity and the unit name. Planned bytes come from `gc::measure`, run only when the `EvictionAccounting` capability is on; otherwise `unmeasured` records the reason. An orphan target gets one entry for the target dir itself and records its profile and triple directories (read with `read_dir_bounded`) for the relist at removal.

### Applying a plan (`src/plan_tier/apply.rs`)

Satisfies: GCPLAN-2.1, GCPLAN-2.5, GCPLAN-2.6, GCPLAN-2.7, GCPLAN-2.8, GCPLAN-2.9, GCPLAN-2.10, GCPLAN-2.13, GCPLAN-3.1, GCPLAN-3.2, GCPLAN-3.3, GCPLAN-3.4, GCPLAN-3.5, GCPLAN-3.6, GCPLAN-3.8, GCPLAN-4.6, GCPLAN-6.8, GCPLAN-8.2, GCPLAN-8.3, GCPLAN-8.5
Reuse: rung 2 — `Fs::try_lock_exclusive` and `TryLock` (`src/platform/mod.rs`), `cargo_target::identify` and `fingerprint::read_profile` to rebuild a `TargetDir` under the locks (`src/cargo_target.rs`, `src/fingerprint.rs:396`), `liveness::liveness`, `discover::targets` with an empty snapshot for ownership, `gc::Ledger` and the volume cross-check note (extracted from `src/cli.rs:808-833`). New: the two-phase driver, rung 7.
Respects: ARCH-9, ARCH-11, ARCH-13, ARCH-17, ARCH-18, ARCH-19, ARCH-20
Surface: the byte-accounting helper of `gc::run` (`remove_measured`, `scan`, `record`, `src/gc/run.rs:356-490`) — `replace`: extracted to a crate-visible `gc::account::remove_counted(fs, ledger, target, path, account_bytes, dry_run) -> Result<Removed, RemoveFailed>`, `RemoveFailed` being `Scan(String)` or `Remove(io::Error)` so the auto tier keeps its own skip and `unmeasured` wording that `Runner` and `apply` both call. A directory root goes through `scan` then `remove_tree`; a file or symlink root is recorded from one `entry_stat` and removed with `remove_file`, because `scan` rejects a file root and would skip every `deps/*.rlib`. `Ledger` and `Tally` (`pub(crate)` in a private module, `src/gc/ledger.rs:25-35`) are re-exported from `gc` as `pub(crate)`. Auto-tier counts, findings and receipts stay identical (GCPLAN-9.1). The volume cross-check block in `gc_cmd` (`src/cli.rs:801-833`) — `replace`: becomes `cross_check_notes(before, after, containers)`; its caller keeps the same text. Callers of `Runner::remove_measured` (`src/gc/run.rs:259,297,345`) — `replace`.
Interface: `apply(ctx, id, cancel) -> Applied { outcome, reason, per_target, counts, bytes, notes, unmeasured }`; and the core it calls, `execute(ctx, plan, resolved: &[ResolvedTarget], cancel)`, public and `#[doc(hidden)]`, which takes a plan whose gates, consume and ownership comparison are already done. `execute` is the seam the deletion trial uses on a scratch clone, with a plan built from the clone's `TargetDir` by `build::plan_for_target`.
Depth: if this module vanished, callers would have to know the lock set, the five staleness checks, the consume point, the delete order and the accounting; they keep one call and one result.
Locality: new file; the extractions in `src/gc/run.rs` and `src/cli.rs` are `extract`, behavior-preserving.

Steps, in order:
1. `Store::get`: unknown id → `refused(unknown_plan)`.
2. `Store::consume`: a consumed id → `refused(already_consumed)`; otherwise the plan is taken and stays consumed from here on.
3. Resolve ownership afresh over every registered repo. For each target: owner or class different from the plan, or `Unresolved`, → `owner_changed`, `class_changed` or `ownership_unresolved`.
4. For each target, sorted by path: `locks::acquire(fs, target, lists_examples)`; a `Skipped` reason skips the target and drops what it took. The lock set, the identity compare and the re-identify rule live in that module (decision 6).
5. For each locked target: `read_profile` every profile, build a `TargetDir`, run `liveness` with the plan's window and a fresh `now`. `Undecided`, an unsupported format, a planned path that is not in the dead set, or a planned path whose `entry_stat` kind, device or inode differs or is missing → refuse the whole plan, release every lock, delete nothing.
6. Phase B, per target, per unit in plan order: delete in the order of decision 12, and delete the fingerprint directory only if the plan holds at least one artifact entry for that unit (GCPLAN-2.7). Each deletion goes through `guard::delete_checked` and `remove_counted` into one `Ledger` shared by the whole plan. If the guard reports `Changed`, `Missing` or `Outside` here, or a removal fails, Phase A's locks and checks were evaded by an outside actor: stop that target at the end of its current unit, record a `plan.delete_failed` finding with the path and reason, keep going with the other targets, and report `partial` with exit 1. Cancellation is polled between units; a cancel returns `partial` with reason `cancelled`, exit 130, and the plan stays consumed.
7. Outcome: every target applied → `applied`; otherwise `partial` with the reason (skipped, `all_targets_skipped`, delete failed, cancelled). Counts: `paths_deleted`, `freed_bytes`, `deleted_bytes_still_shared`, `targets_applied`, `targets_skipped_held`, `targets_skipped_lock_unopenable`, `targets_skipped_lock_unsupported`, `targets_skipped_lock_missing`. The cross-check note is added once per APFS container deleted on.

The liveness recompute takes the shape LIVE already proved: the proof harness holds the same profile locks while it decides (`docs/specs/2026-10-30-closure-liveness/design.md`, proof section), so step 5 is the production form of that sequence, with the missing-lock rule corrected.

### Lock acquisition for one target (`src/plan_tier/locks.rs`)

Satisfies: GCPLAN-2.2, GCPLAN-2.3, GCPLAN-2.12
Reuse: rung 2 — `Fs::try_lock_exclusive` and `TryLock`, `Fs::entry_stat`, `cargo_target::identify` (`src/cargo_target.rs:100`); the candidate-file rule and the skip reasons are new, rung 7.
Respects: ARCH-19
Interface: `acquire(fs, target: &Path, lists_examples: bool) -> Acquired`, with `Acquired::Locked(Locked)` (holds every `HeldLock` and the profile directories) or `Acquired::Skipped(Reason)`, `Reason` being `Held`, `Unopenable`, `Unlockable`, `Missing` or `ProfileSet`. Dropping `Locked` releases every lock.
Depth: if this module vanished, callers would have to know the three lock file names, which ones to try for which layout, the identity compare and the re-identify rule; they keep one call and five reasons.
Locality: new file; `apply.rs` and `orphan.rs` call it; nothing else changes (`leave`).

For each profile and triple directory `identify` returns, it tries `.cargo-build-lock` and `.cargo-lock` where present, plus `.cargo-artifact-lock` when `lists_examples`. A directory with a `.fingerprint` child and none of those files gives `Missing`. After every lock is held it compares each lock's identity with `entry_stat` of its path (a `None` identity, a missing path or a mismatch gives `Missing`), then calls `identify` again and gives `ProfileSet` if the set of directories changed. The `Reason` names map to the receipt counts `targets_skipped_held`, `targets_skipped_lock_unopenable`, `targets_skipped_lock_unsupported`, `targets_skipped_lock_missing` and `targets_skipped_profile_set`.

### Lock identity (`src/platform/mod.rs`, `src/platform/apfs.rs`)

Satisfies: GCPLAN-2.4
Reuse: rung 2 — extends `HeldLock` and `Apfs::try_lock_exclusive` (`src/platform/apfs.rs:187-226`) rather than adding a required `Fs` method.
Respects: ARCH-15, ARCH-19
Surface: readers of `HeldLock::new`: `src/platform/apfs.rs:225` — `replace` (calls the new constructor with the opened file's `(dev, ino)`); `src/doctor.rs:1096`, `src/gc/run.rs:647` — `compat` (unchanged call to `HeldLock::new`, identity `None`); those two serve the doctor probe and the auto tier, which never read the identity, and plan tier fails closed on `None`, so the `None` case cannot silently weaken an apply.
Interface: `HeldLock::with_identity(dev, ino, release)` and `HeldLock::identity() -> Option<(u64, u64)>`; the caller compares it with `entry_stat(path)` once, right after the lock is taken, and treats `None`, a missing path and a mismatch as "skip the target". Tests with a fake `Fs` that want a target applied supply an identity through `with_identity`.
Depth: n/a — extends `HeldLock`.
Locality: `src/platform/mod.rs` and `src/platform/apfs.rs` only; no other `Fs` implementation changes, so none of the 7 implementations listed in the scan needs an edit.

### Path guard and delete by kind (`src/plan_tier/guard.rs`)

Satisfies: GCPLAN-8.1
Reuse: rung 2 — the pattern of `delete_in_clone` in `tests/common/prove.rs:900-935` (canonical parent under root, lstat, then `remove_tree` for a directory or `remove_file` otherwise), moved into `src`.
Respects: ARCH-18
Surface: `tests/common/prove.rs` `delete_in_clone` (`:900`) and its callers `:1174` and `tests/liveness.rs:1891` — `replace`: `delete_in_clone` calls `guard::delete_checked` with the clone root as `root` and `Expect::Any` (clone inodes differ from any planned inode), treating `Missing` as it does today.
Interface: `check_entry(fs, root, path, expect) -> Result<EntryStat, Deleted>` (canonical parent inside `root`, lstat, identity compare; mutates nothing) and `delete_checked(fs, root, path, expect) -> Deleted` (`check_entry` then remove by kind), with `Deleted` being `Removed`, `Missing`, `Changed` or `Outside` and `Expect::Entry { kind, dev, ino }` or `Expect::Any`. `apply` and the orphan removal call `check_entry` and then `remove_counted`, so a path is never removed twice; a whole-tree removal checks the target dir against its parent as `root`.
Depth: if this module vanished, callers would have to know canonicalization, the lstat compare and the remove-by-kind rule; they keep one call.
Locality: new file; `tests/common/prove.rs` is edited (`extend`) to call it.

### Orphan target removal (`src/plan_tier/orphan.rs`)

Satisfies: GCPLAN-5.8, GCPLAN-5.9, GCPLAN-5.10, GCPLAN-5.11
Reuse: rung 2 — the lock helpers and Ledger of the apply module, `identify` for the relist, `discover::targets` for fresh ownership; new orchestration is rung 7.
Respects: ARCH-17, ARCH-18, ARCH-19
Interface: `remove(ctx, plan_target, cancel) -> Removed | Refused(reason) | Skipped(reason)`.
Depth: if this module vanished, callers would have to know the whole-tree lock set, the relist-after-lock rule and the residual `cargo clean` race; they keep one call.
Locality: new file; `apply.rs` calls it for an `orphan` plan.

Takes the lock of every profile and triple directory in the target dir and in its build directory, then lists them again with `read_dir_bounded`: a directory not in the plan, a target dir whose (kind, device, inode) changed, or a target that fresh ownership no longer marks as an orphan candidate refuses the removal. `cargo clean` takes no lock, so a removal can still race with it (research §4); the identity check and the relist narrow the window, they do not close it, and the residual risk is accepted. The removal is one `remove_counted` of the target dir; if `measure` reaches its entry or depth cap, the bytes go to `unmeasured` with the reason.

### Window configuration (`src/state.rs`)

Satisfies: GCPLAN-1.8, GCPLAN-1.9
Reuse: rung 2 — `ConfigFile` and `Config` (`src/state.rs:156-205`), which already read `config.json` and refuse unknown keys.
Surface: readers of `Config` / `State::config()` — `src/cli.rs:473`, `:745`, `:906` and `src/state.rs:413,427` — `replace`: they receive a `Config` with one more optional field and ignore it; `config.json` files with only `secondary_roots` / `cache_roots` — `frozen`: still valid, the new key is optional.
Interface: `Config::gc_window_days -> Option<u64>`, unvalidated; `plan_tier` applies the default of 7 and the 1 to 365 bound, and `make` returns a refusal for anything else.
Depth: n/a — extends `Config`.
Locality: `src/state.rs` (`extend`); no other module.

### Receipt schema 3 (`src/receipt.rs`)

Satisfies: GCPLAN-6.1
Reuse: rung 2 — `Receipt` and its builder (`src/receipt.rs:48-157`).
Respects: ARCH-10
Surface: every reader of the receipt shape — `tests/receipts.rs:7-33` (12 sorted keys, `schema == 2`), the unit tests at `src/receipt.rs:201,213` and `:228`, `cli::emit` (`src/cli.rs:196`), the receipt log lines parsed back under `Deserialize` — `frozen`: the 12 keys and `schema` 2 are unchanged for every command except `gc --plan` and `apply`; the three new keys carry `#[serde(default, skip_serializing_if = "Option::is_none")]`, so v2 output and old log lines are byte-identical.
Interface: `ReceiptBuilder::plan_tier(body: PlanTierBody)` sets `schema` to 3 and fills `plans`, `undecided`, `nothing_to_do`; `PlanRecord { id, class, outcome: Option<Outcome>, reason, targets, paths, counts }`.
Depth: n/a — extends `ReceiptBuilder`.
Locality: `src/receipt.rs` (`extend`); field names avoid the words the no-time test rejects (`saved`, `duration`, `seconds`, `elapsed`, `estimate`, `src/receipt.rs:226`).

### CLI commands (`src/cli.rs`)

Satisfies: GCPLAN-1.10, GCPLAN-3.10, GCPLAN-4.2, GCPLAN-6.2, GCPLAN-6.4, GCPLAN-6.5, GCPLAN-6.6, GCPLAN-6.7
Reuse: rung 2 — `select`, `live_checkouts`, `emit`, `emit_failure`, `OutputArgs`, `doctor::require`, `install_signal_handlers` and the load closure of `gc_cmd` (`src/cli.rs:726-770`).
Respects: ARCH-8, ARCH-14
Surface: `Command::Gc` readers — `src/cli.rs:143` dispatch and `tests/skill.rs:418,442,555,577,578` — `replace`: `Gc` gains `plan: bool` (conflicts with nothing), and `tests/skill.rs` `Parsed` and `parse` map `Gc { plan }` and the new `Apply { id }`; `main_with` exit-code table — `frozen` for 0, 1, 2, and 130. `gc` without `--plan` takes the old path unchanged.
Interface: `cargo hive gc --plan [repos…] [--json]` and `cargo hive apply <id> [--json]`. No window option. `apply` takes one id.
Depth: n/a — extends the `Command` enum and `main_with` dispatch.
Locality: `src/cli.rs` (`extend`: two functions `plan_cmd`, `apply_cmd`, and the extracted `cross_check_notes`); `gc_cmd` (`leave`).

`--plan` conflicts with `--dry-run` in clap; `gc --plan` requires `FingerprintReading`; `apply` requires `FingerprintReading` and `Deletion` (as `gc_cmd` does, `src/cli.rs:763`) and refuses with exit 1 before it consumes anything. `gc --plan` exits 0 whether it made plans or found nothing. The skipped-target counts of step 7 keep the names listed there, so they never collide with the AUTOGC-5.3 names for sessions and profile directories.

### Skill and eval (`plugin/skills/hive/SKILL.md`, `skills/hive/eval.json`, `skills/hive/TESTS.md`)

Satisfies: GCPLAN-7.1, GCPLAN-7.3, GCPLAN-7.4, GCPLAN-7.5, GCPLAN-7.6
Reuse: rung 2 — the existing skill, its eval entries and the drift tests of `tests/skill.rs`.
Surface: `plugin/skills/hive/SKILL.md` line stating the plan tier "is not available yet" and its "Exit codes" sentence — `replace`; `tests/skill.rs` `skill_documents_every_shipped_command` hard-coded `want` list (`:572`) — `replace`: gains `gc --plan` and `apply`; `skills/hive/eval.json` entries — `compat` with no change to the existing ones; new entries carry `derived_from` headings added to `skills/hive/TESTS.md`.
Interface: two fenced command lines with `--json` and a path word, the every-class rule, and the plan-again rule after `refused`.
Depth: n/a — extends the skill.
Locality: the skill stays within its 150-line body budget; ROAD-13 rewrites it as a whole later.

## Seams for testing

The harness runs the real binary (`env!("CARGO_BIN_EXE_cargo-hive")`, `Hive::ready()`, `Scratch`) and, at library level, `cargo_fixture::{fixture, decide_at, read_target}` with the real `Apfs`; lock-before-delete order is asserted with a new recording `Fs` wrapper in `tests/common` (the existing `Rec` and `Volumes` fakes are private to their modules and are not reused).

| Seam | Kind | Covers |
|---|---|---|
| `cargo hive gc --plan` / `apply` through the binary (new tests in `tests/plan_apply.rs`, reusing `Hive`) | integration | GCPLAN-1.1 to 1.6, 1.8 to 1.13, 2.1, 2.6 to 2.11, 3.1 to 3.8, 3.10, 4.2, 4.6, 5.1 to 5.6, 6.1, 6.2, 6.4 to 6.7, 8.3 |
| `plan_tier::{make, apply, execute}` and `plan_tier::locks::acquire` with a recording `Fs` wrapper (new, the one new seam) | integration | GCPLAN-2.2 to 2.5, 2.12, 2.13, 4.6, 5.8 to 5.11, 8.1, 8.3, 8.5 |
| `plan_tier::record::Store` (unit tests in the module) | unit | GCPLAN-1.7, 1.14, 2.11, 3.7, 3.9, 8.4 |
| `HeldLock` identity with `Apfs` (`tests/platform_locks.rs`) | integration | GCPLAN-2.4 |
| `State::config()` (unit tests in `src/state.rs`) | unit | GCPLAN-1.8, 1.9 |
| Receipt shape (`tests/receipts.rs`, unit tests in `src/receipt.rs`) | integration / unit | GCPLAN-6.1, 9.2, 9.3 |
| Deletion trial through `plan_tier::execute` on a scratch clone (`tests/common/prove.rs`, ignored proof test) | integration | GCPLAN-8.2, 9.5 |
| Skill drift tests (`tests/skill.rs`) | integration | GCPLAN-7.1, 7.3 to 7.6, 9.9 |
| Existing `status`, `doctor`, `explain`, `gc` tests | integration | GCPLAN-9.1, 9.2, 9.6 to 9.8, 9.10, 9.11 |

## Coverage check

Every ID of requirements.md appears in exactly one `Satisfies:` line above, with these deliberate exceptions:
- GCPLAN-9.1 to 9.11 (guards) have no `Satisfies:` line: they are held by leaving the guarded code unchanged, by the `Surface:` rows above, and by the existing tests named in the last rows of the seam table.
- Story 8's Performance and Accessibility lines are `None` and carry no ID.
