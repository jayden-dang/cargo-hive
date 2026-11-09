# Design: Store GC (bare `cargo hive gc` keeps hive's store bounded)

Feature code: STOREGC
Status: Draft — revised after the independent review (`.skills/_pending-store-gc/auto/design-review.md`, findings 1–10 applied)
Date: 2026-11-09
Requirements: ./requirements.md

## Context

The store exists and only grows. `seed` fills it from live targets and clones out of it (`src/seed/mod.rs:186-241`). Only fill moves an entry's `last_live_at` (`src/seed/fill.rs:406-407,519-520`). Nothing deletes a published entry; the one store deletion is fill's one-day sweep of `store/.tmp/` (`src/seed/store.rs:63-128`). Bare `gc` already walks every target and checkout hive knows (`src/cli.rs:862-864`), but it never calls `liveness` and never reads the store. The pieces eviction needs mostly exist:
- closure liveness (`src/liveness.rs:210`);
- the unit-to-store-key code in `src/seed/source.rs:122-171`;
- the freed-byte ledger and `gc::measure`, which accepts a directory root (`src/gc/run.rs:393`);
- `remove_counted` (`src/gc/account.rs:43`);
- the state lock (`src/state.rs:151-178`).

The digests are `.skills/_pending-store-gc/auto/scan.md` and `design-scan.md`.

The binding constraint is concurrency without a daemon. Seed reads `store.db` with no lock (`src/seed/mod.rs:220`) and clones file by file (`src/seed/place.rs:302-379`). Fill renames a finished entry into place and only later merges its row under the state lock (`src/seed/store.rs:332-347`, `src/seed/fill.rs:469`). Eviction is the first writer that deletes published entries, so it must never run inside either window (STOREGC-6.1). The state lock cannot be that exclusion, because SEED-9.4 forbids holding it across a clone or a compression. That rules out the cheapest option, "evict under the state lock". It also rules out "no lock, seed tolerates a vanished entry": a unit that loses its entry mid-clone surfaces as a `unit.refused` finding (`src/seed/place.rs:348-371`), which STOREGC-9.4 forbids, and STOREGC-6.1 says eviction does not remove an entry a seed is using.

The second constraint is the persisted-format hard stop of this run: no new field in `store.db`, no config key, no plan class. The spike `.skills/_pending-store-gc/auto/spike-k4.md` shows that none is needed for bytes. A compressed store file reads `shares_all` only while a live clone shares it, so `Ledger::file` (`src/gc/ledger.rs:65-71`) already counts freeable bytes correctly with no clone id. The reference that shaped the trigger and order is Cargo's own cache GC (`.skills/_pending-store-gc/auto/research-store-gc.md`): it cleans inside a command already doing work, by age of last use, and by size, oldest first.

Retrieval (schema 1.1, recipe fsubr-1.2, fresh 2026-11-09, seeds STOREGC plus requirement terms plus 14 candidate paths): OWNS coverage 7/8; STOREGC is the one without `tasks.md`. Observations: none. Neighbors by shared paths:
- SEED: `src/seed/store.rs`, `place.rs`, `fill.rs`, `src/gc/ledger.rs`, `src/liveness.rs`, `src/state.rs`, `src/cli.rs`, `src/doctor.rs`, the skill.
- AUTOGC: `src/gc/run.rs`, `ledger.rs`, `cli.rs`, `doctor.rs`, README.
- GCPLAN: `src/plan_tier/mod.rs`, `src/state.rs`, the skill.
- LIVE: `src/liveness.rs`, `src/gc/run.rs`.
- CORE: `src/state.rs`, `src/discover.rs`, `src/doctor.rs`.

The retrieval is advisory; every `Reuse:` below comes from the scan digests.

ARCH invariants this design relies on: ARCH-2, ARCH-4, ARCH-8, ARCH-9, ARCH-10, ARCH-11, ARCH-13, ARCH-14, ARCH-15, ARCH-16, ARCH-17, ARCH-18. ARCH-19's try-without-waiting shape is borrowed for the store lock (decision 2); the store is not a Cargo target dir, so ARCH-19 itself does not bind it.

## Decisions

The close package and ledger (`.skills/_pending-store-gc/auto/`) hold the frame's decisions; they are not restated. These are what design settled.

1. **Store eviction is `src/seed/evict.rs`, a sibling of `store.rs`.** Eviction needs store internals that are private to `seed`: `entry_dir`, `same_key`, row merging. Living inside `seed` keeps them private. `cli.rs` calls one function, as it calls `seed::run`.
2. **The exclusion is one store lock, `<state root>/store.lock`, created and taken through `state`.**
   - `seed::run` holds it shared from just before `fill::fill` (`src/seed/mod.rs:194`) until `place` returns. That covers fill's `.tmp` sweep (`src/seed/fill.rs:83`), its publish-to-merge window, seed's `store::read` (`src/seed/mod.rs:220`), the clone-out, and the `store_bytes` walk. It takes the lock as `locked_or_cancelled` takes the state lock: `LOCK_SH | LOCK_NB`, polled every 10 ms, cancellable.
   - Bare `gc` tries it exclusive once, with `LOCK_EX | LOCK_NB`, and holds it through the commit and removal phases (decision 3). Held means no entry is evicted this run, with a `store.busy` finding and the `store_entries_held_busy` count.
   - One lock for the whole store, not one per entry. Per-entry locks would cost one file per entry for a window that seed holds across hundreds of entries anyway. So while any seed runs, gc evicts nothing and the cap is not enforced that run; STOREGC-6.1 and STOREGC-3.2 are reworded to say so (Upstream sync-back).
   - It is a lock file, not metadata: it holds no content, so it is no persisted format. `store.db` is still written only under the state lock (ARCH-16).
   - SH and EX|NB flocks are atomic and the two locks are always taken in one order (store lock, then state lock), so there is no cycle (review finding 4, verified).
3. **Two phases: plan with no lock, commit under the exclusive lock.**
   - **Plan:** scan targets, form held keys, resolve toolchains, read `store.db`, list entry directories and measure them. This is the slow part, and seeds run beside it.
   - **Commit**, under the exclusive store lock: no fill can merge while it is held, so the rows read now are final.
     1. Remove any `store/.tmp/evict-*` left by a killed run.
     2. `commit_eviction` takes the state lock with `State::locked` (not cancellable) before the first rename and holds it through the renames and the write, in one `Tx`. The state lock is then held across renames only, which are metadata operations, never across a clone or a compression (SEED-9.4). Inside it: read `store.db` and recompute from those fresh rows. Drop any candidate whose row's `last_live_at` rose since the plan, or whose rowless directory gained a row.
     3. Rename each remaining candidate to `store/.tmp/evict-<pid>-<seq>`, checking cancel before each.
     4. Write `store.db` in the same `Tx`: drop the evicted rows, raise `last_live_at` of held rows (STOREGC-1.3), and drop the texts of every value with no row and no entry directory left (STOREGC-2.3).
     5. Remove each emptied `store/<value>/`.
   - **Removal**, still under the exclusive store lock: remove the renamed trees with `remove_counted`, then release.
   - Holding the lock through removal is what keeps fill's `.tmp` sweep and seed's `store_bytes` walk off a tree being removed (review finding 1: a rename keeps the directory's mtime, so a 30-day-old entry is sweep-eligible at once).
   - A seed waits for renames, one write and the removals, never for the scan. Accepted trade-off. Reopen if a measured seed waits more than 10 s on `store.lock`.
4. **Kill safety comes from rename-then-remove (STOREGC-9.2).**
   - A rename within one volume is atomic, so an entry path holds either the whole entry or nothing.
   - Killed after a rename and before the write: the row's directory is gone, and `store::read`/`merge` already prune such rows (STOREGC-10.4).
   - Killed during removal: the partial tree sits under `.tmp/`, where fill does not adopt. The next gc's commit step 1 removes it, and fill's one-day sweep may too. Neither runs while another gc holds the lock.
   - The `evict-` prefix never matches fill's `<pid>-<key>` names.
5. **The emptied `store/<value>/` is removed under the exclusive lock.** Fill's `create_dir_all(parent)` then `rename` (`src/seed/store.rs:340-342`) runs while fill holds the shared lock, so the two cannot race.
6. **Freeable bytes come from `gc::measure(fs, entry_dir, &[entry_dir], account_bytes)`, one call per entry.** Entries share no inode (fill clones per file, and `SkippedHardLink` fails a fill), so per-entry sums add up. The spike shows the ledger's rule is right for every store case. No clone id is recorded (SEED design decision 3 stays answered "not yet").
7. **Cap order and tie-break (STOREGC-3.2, Weigh A2 amended by W12).** Candidates sort by `last_live_at` ascending. Within one `last_live_at`, an entry comes before every entry its `registry_deps` name (dependents first: a reverse topological order over the cohort's `registry_deps`, which `Row` already holds), and remaining ties go by (`toolchain_value`, `name_meta`, `fingerprint_hash`) ascending. Ties are the normal case: a gc run gives every held key one `now`, and a fill writes one `now` (`src/seed/fill.rs:406-407,519-520`). An alphabetical cut through a cohort would leave dependents whose dependencies are gone, which `choose` drops (`src/seed/choose.rs`), counted against the cap and usable by no seed. The total compared with the cap is the freeable bytes of every entry remaining after stories 1 and 2, held entries included. Freeable bytes of rowless directories count too, but `.tmp/` does not; its bytes are reported apart (STOREGC-1.8). An entry that frees 0 bytes is skipped by the cap step: evicting it cannot lower the total. `CAP_BYTES = 40_000_000_000` and `MAX_AGE = 30 × 86 400 s` are constants in `cli.rs`, passed to `evict::run` in `Limits` (Weigh W10).
8. **Held keys are formed from each profile's graph, not from `liveness` alone.**
   - `liveness` gives `UnitRef { profile, dir_name }` for Decided targets and nothing for Undecided ones (`src/liveness.rs:30-36`). For a Decided target, the live refs are mapped to keys through the profile graph, as `source::live_registry_units` does (`src/seed/source.rs:122-171`). A live ref with no graph unit, a live not-current node, is held by its name-meta (STOREGC-4.3).
   - Undecided: every unit in the graph gives its key, and every not-current unit gives its name-meta (STOREGC-4.2, 4.3).
   - Unsupported profile: its `.fingerprint/` is listed with `read_dir_bounded(CRATE_DIR_CAP)`, and each directory name is held as a name-meta. With no `.fingerprint/` it holds nothing. A failed listing is STOREGC-4.4's whole-store hold.
9. **Toolchain lookups (STOREGC-2.1, Weigh W9).**
   - Lookup (a) is a new `doctor::resolve_toolchain_own_dir_until(cwd, cancel)`, which removes `RUSTUP_TOOLCHAIN` and `RUSTUP_TOOLCHAIN_SOURCE` unconditionally: the stated CORE-1.12 exception, and allowed by ARCH-13 as a variable "set by hive or kept on purpose". It runs once per non-missing checkout.
   - Lookup (b) is seed's `resolve_toolchain_until` (`src/doctor.rs:443`). When the caller's `RUSTUP_TOOLCHAIN` survives `drop_directory_toolchain`, (b) resolves the same toolchain in every directory, so it runs once. `gc_cmd`'s existing `resolve_toolchain(state.root())` (`src/cli.rs:849`) is that one result and is reused. When it does not survive, (b) equals (a), and the run reuses (a). Whether it survives is one predicate, extracted from `drop_directory_toolchain` so both read it (verified on rustup 1.29.3, review findings 2 and 9).
   - No lookup runs when the store has no entry.
   - Cost: about 87 ms per checkout on klynt (`.skills/_pending-store-gc/auto/measure-u1.md`). S2's 10 s trigger is about 115 checkouts.
10. **Dependents of an evicted entry are not cascaded by the age and toolchain steps (K6 closed).** A live dependent's dependencies are live under closure liveness, so they are held. An unheld dependent is not live, and the age or toolchain rule reaches it with its dependencies, since they share a value and age together. The cap step is different: it can stop inside a cohort, which is why decision 7 orders dependents first. `choose` already drops a unit whose `registry_deps` fall outside the store (SEED-3.7).
11. **Time.** One `SystemTime::now()` at the start of eviction, in whole seconds since the epoch, as `Row` stores it. "More than 30 days before" means `now - last_live_at > MAX_AGE`.
12. **Dry run (STOREGC-5.1).** It runs the plan phase and computes what the commit phase would do with no lock, no rename and no write; the counts are named as in a real run. It takes no store lock, so its counts match the next real run only on an idle store (STOREGC-5.2).
13. **Cancel and failure.**
    - The cancel flag (`doctor::CANCEL`) is polled between plan steps and before each rename. A cancel before commit evicts nothing. `gc`'s exit is then 130 as today.
    - The state lock is taken, not cancellably, before the first rename (decision 3, step 2), so what was renamed is always recorded. A cancel seen before a rename stops renaming; the write still happens, the removal is skipped, and both locks are released. The `evict-` trees stay for the next gc.
    - A failed rename keeps that entry, with a finding.
    - A failed `store.db` write leaves rows whose directories are gone. `store::read` and `merge` prune them at the next read (STOREGC-10.4). It gets a finding.
    - None of these changes `gc`'s exit code (STOREGC-4.5, W5).
14. **`live_checkouts` returns its walk faults** to all three callers, instead of turning them into findings and dropping them. `gc_cmd` passes them to `evict::run` for STOREGC-2.4/4.4; `plan_cmd` and `explain_cmd` make the same findings as today.
15. **Older "same counts" guards.** GCPLAN-9.1 and SEED-10.1 promise bare `gc` the same count names. STOREGC adds store counts by design, so each gets a dated note naming STOREGC. That is documentation upkeep, as the SEED design note is.
16. **Where eviction sits in `gc_cmd`.** After the `after` volume sample and its cross-check notes (`src/cli.rs:903-912`), so store bytes never enter AUTOGC-5.4's note. Not when `report.cancelled` (`src/cli.rs:950`).
17. **Evicted trees stay under `store/.tmp/` (Weigh W14 declined for now).** Moving them outside `store/` would let removal run after the store lock is released, so seeds would not wait for it, but it reopens STOREGC-9.1 for a wait nobody has measured. Decision 3's reopen trigger (a measured seed waiting more than 10 s on `store.lock`) is the gate for W14.
18. **No ADR.** Every choice is reversible under Compat None. The store lock file, the `evict-` prefix and the counts can change in a later slice. The CORE-1.12 exception is recorded in requirement STOREGC-2.1 itself.

## Architecture

Vocabulary for every section below: **module**, **interface**, **implementation**,
**seam** (public surface where behavior is observable and substitutable). Prefer
deep modules — an interface much simpler than what it hides.

```
cargo hive gc            (no repo, no --plan)
  cli::gc_cmd: load -> checkouts (+walk faults) -> discover::targets -> gc::run   [unchanged]
             -> window_days(cfg) ok? -> evict::run(ctx, Limits{CAP_BYTES, MAX_AGE}, dry_run)
  seed::evict::run
    plan (no lock):  held keys over every target  (liveness + graph, holds)
                     toolchains in use            (lookups a, b)
                     store rows + entry dirs      (store::read_all)
                     ages -> toolchain -> cap      (gc::measure per entry)
    commit:          state.try_store_exclusive()  -- held: evict nothing, count busy
                     re-read rows, drop changed candidates
                     rename each to store/.tmp/evict-<pid>-<seq>
                     store::commit_eviction(state, drop rows, raise last_live_at, drop texts)
                     remove emptied store/<value>/ ; release
    after:           remove_counted each renamed tree -> ledger -> counts
  cli: put counts, findings, notes, unmeasured -> emit
cargo hive seed <checkout>
  seed::run: state.store_shared_or_cancelled() held from first store::read to end of place
```

### Store lock (state)

Satisfies: STOREGC-6.1
Reuse: rung 2 — the polling `flock` loop of `State::locked_or_cancelled` (`src/state.rs:151-178`), extended with a shared mode and a single exclusive try
Surface:
- `State::locked_or_cancelled` and its callers: `leave` (unchanged; the store lock is a separate file).
- `<state root>` listing in `tests/guards.rs:233-246`: `leave`. That test asserts the full state-root listing but runs neither gc nor seed, so `store.lock` never appears there.
- Other state-root readers (`state` itself): `leave`.
Interface: `State::store_shared_or_cancelled(cancel) -> Result<StoreLock, StateError>`; `State::try_store_exclusive() -> Result<Option<StoreLock>, StateError>` (`None` when held). Both create `store.lock` if absent; `StoreLock` releases on drop.
Depth: n/a — extends `State`'s lock code.
Locality: extend `src/state.rs`; no neighbor changes.

### Seed holds the store lock

Satisfies: STOREGC-6.2, STOREGC-9.4
Reuse: rung 2 — `seed::run` (`src/seed/mod.rs:186-241`)
Surface:
- `seed::run` callers (`cli::seed_cmd`, `src/cli.rs:1523`; tests/seed.rs, tests/seed_fill.rs): `replace`. The signature is unchanged; a seed may now wait while a gc's commit phase holds the lock exclusive, with cancel honored as SEED-7.5 requires.
- SEED-9.4 ("the state lock is never held across a clone or compression"): `leave`, since the store lock is not the state lock.
Interface: unchanged — `seed::run(...) -> Outcome`.
Depth: n/a — extends `seed::run`.
Locality: extend `src/seed/mod.rs` with one guard taken before `fill::fill` (`src/seed/mod.rs:194`) and dropped after `place`; `fill.rs` and `place.rs` leave, because the guard spans their calls.

### Store access for eviction (store)

Satisfies: STOREGC-1.3, STOREGC-1.5, STOREGC-1.6, STOREGC-2.3
Reuse: rung 2 — `store::read`, `merge`'s pruning, `entry_dir`, `store_dir`, `tmp_dir`, `Tx::write_bounded` (`src/seed/store.rs:43-236`)
Respects: ARCH-16, ARCH-2
Surface:
- `store.db` readers (`store::read` in `seed::run`, `store::merge` in fill; tests that parse `store.db`): `frozen`, by building around it. The format is unchanged: rows and toolchain texts are only removed, and `last_live_at` only rises, as merge already does.
- `entry_dir` (`pub`; readers `fill.rs`, `place.rs`, `seed/mod.rs`) and `same_key` (private): `leave`.
Interface:
- `StoreKey { toolchain_value: u64, name_meta: String, fingerprint_hash: String }`, the key `same_key` already compares.
- `store::inventory(fs, state) -> Inventory { rows: Vec<Row>, rowless: Vec<(StoreKey, PathBuf)>, tmp_bytes: u64 }`: every row with its directory present, every entry directory with no row, and the allocated bytes of `.tmp/`. It lists `store/<u64>/<name-meta>-<hash>/` only, skipping `.tmp/`, `CACHEDIR.TAG` and any name that is not one.
- `store::evict_dir(state, pid, seq) -> PathBuf` (`store/.tmp/evict-<pid>-<seq>`) and `store::rename_out(state, key, to) -> io::Result<()>`.
- `store::commit_eviction(state, plan: &CommitPlan, rename: impl FnMut(&StoreKey) -> io::Result<Option<PathBuf>>) -> Result<Committed, StateError>`, where `Ok(None)` stops renaming (cancel). In one `Tx` it reads the rows, revalidates the plan, calls `rename` for each surviving candidate, writes the rows and texts, and returns what it renamed (decision 3).
Depth: n/a — extends `store`.
Locality: extend `src/seed/store.rs`; `read` and `merge` leave (STOREGC-10.4).

### Held keys (source)

Satisfies: STOREGC-1.2, STOREGC-4.2, STOREGC-4.3
Reuse: rung 2 — the unit-to-key mapping of `source::live_registry_units` (`src/seed/source.rs:122-171`) and `liveness::liveness`; extract the mapping into a helper both callers use
Respects: ARCH-17, ARCH-9
Surface:
- `source::live_registry_units` (called through `owned_targets` by fill, `src/seed/mod.rs:186`): `replace`. It calls the extracted helper, with the same output.
- `liveness()`: `leave` (STOREGC-10.6).
Interface: `source::unit_key(&Unit) -> StoreKey` (the extracted mapping); `source::holds(fs, targets: &[TargetDir], now, window) -> Holds { keys: BTreeSet<StoreKey>, name_metas: BTreeSet<String>, by_reason: counts, whole_store: Vec<Finding>, findings: Vec<Finding> }`.
Depth: n/a — extends `source`.
Locality: extend `src/seed/source.rs` (extract one helper); `fill.rs` leave.

Rules, per target in the AUTOGC-4.1 set plus those `gc` skips for their volume or profile:
- `unreadable` → `whole_store`.
- Each profile:
  - Supported and Decided → live refs mapped to keys; a live ref with no graph unit (a live not-current node) → its name-meta.
  - Supported and Undecided → every graph unit's key, plus every not-current unit's name-meta, plus a finding with `clears_when`.
  - `ListingFailed` → `whole_store`.
  - Unsupported → `read_dir_bounded(<profile>/.fingerprint, CRATE_DIR_CAP)` names as name-metas. A missing `.fingerprint/` holds nothing; any other listing error → `whole_store`.

### Toolchains in use (doctor)

Satisfies: STOREGC-2.1, STOREGC-2.4, STOREGC-2.5
Reuse: rung 2 — `resolve_toolchain_with` and `rustup_which` (`src/doctor.rs:397-472`), parameterized by how the environment is cleaned
Respects: ARCH-13, ARCH-14
Surface:
- `drop_directory_toolchain` readers (`src/doctor.rs:408`, `src/cargo_target.rs:360`): `leave` (STOREGC-10.7).
- `resolve_toolchain`, `resolve_toolchain_until` callers (`src/cli.rs:369, 533, 849, 1012, 1118, 1279, 1474`; `src/seed/fill.rs:740`; `src/doctor.rs:1661`; tests/capability_gate.rs): `leave`. Same results, same environment handling.
- `drop_directory_toolchain` body: `replace`. Its keep/drop predicate is extracted into the pure `caller_toolchain_kept_from(source)` and the env-reading `caller_toolchain_kept()`, which it calls, with the same behavior.
Interface:
- `doctor::resolve_toolchain_own_dir_until(cwd, cancel) -> Result<Toolchain, String>`, lookup (a).
- `doctor::caller_toolchain_kept() -> bool`, which reports whether (b) is one lookup.
Depth: n/a — extends `doctor`.
Locality: extend `src/doctor.rs`; neighbors leave.

### Store eviction (evict)

Satisfies: STOREGC-1.1, STOREGC-1.4, STOREGC-1.7, STOREGC-1.8, STOREGC-2.2, STOREGC-3.1, STOREGC-3.2, STOREGC-3.3, STOREGC-3.4, STOREGC-4.1, STOREGC-4.4, STOREGC-4.5, STOREGC-5.1, STOREGC-5.2, STOREGC-7.1, STOREGC-7.2, STOREGC-7.3, STOREGC-7.4, STOREGC-9.1, STOREGC-9.2, STOREGC-9.3
Reuse: none — new code (rung 7), because no module orders evictions or phases a store commit. Inside it: rung 2 for `gc::measure` (per-entry freeable), `gc::account::remove_counted` with `Ledger` (removal and freed bytes), `split_checkout_faults` and `faults_that_can_hide_an_owner` (`src/plan_tier/mod.rs:50-64`), and `Receipt` builders.
Respects: ARCH-2, ARCH-4, ARCH-8, ARCH-10, ARCH-11, ARCH-15, ARCH-16, ARCH-18
Interface: `seed::evict::run(ctx: &EvictCtx, limits: Limits, dry_run: bool, cancel: &dyn Fn() -> bool) -> EvictOutcome`.
- `EvictCtx { state, fs, targets, checkouts, checkout_faults, scan_faults, cache_roots, window, account_bytes, now, caller_toolchain }`, where `caller_toolchain` is the whole `Toolchain` `gc_cmd` already resolves at `src/cli.rs:849`, lookup (b)'s single result.
- `Limits { cap_bytes, max_age_secs }`.
- `EvictOutcome { counts: Vec<(String, u64)>, findings, notes, unmeasured, cancelled }`.
Depth: if this module vanished, its one caller would still need to know only that eviction takes the scanned targets, the checkouts, a cap and a max age, and returns counts and findings. The phase order, hold rules, sort, lock protocol and commit stay hidden behind that interface. With one caller, the module earns its place as the locality of every STOREGC rule and as the one test seam for the cap (W10).
Locality: new `src/seed/evict.rs`; `seed/mod.rs` gains `pub mod evict`; neighbors leave.

Flow:
1. Holds: `source::holds`. Any fault STOREGC-4.4 names joins `whole_store`.
2. Toolchains: lookups (a) and (b). A failure, a fault `apply` would refuse on, or no resolved checkout blocks drops (STOREGC-2.4, 2.5).
3. `store::inventory`.
4. If `whole_store` is non-empty → no candidates; report the holds (STOREGC-4.4).
5. Otherwise: age candidates, then toolchain candidates (unheld entries of values not in use), then the cap. Without `account_bytes` the cap step is skipped into `unmeasured` (STOREGC-3.4). With it, measure every remaining entry, sum, and evict unheld entries in decision 7's order until the sum is at or under `cap_bytes`. If only held entries remain over it, that is STOREGC-3.3's finding.
6. Commit and remove as decision 3, or in a dry run report the counts only.

Rowless directories go through the same rules with `last_live_at` unknown. They are evicted when unheld (STOREGC-1.6) and are never refreshed.

Count names, receipt schema 2, added to bare `gc` only:
- `store_entries_evicted_age`, `store_entries_evicted_toolchain`, `store_entries_evicted_cap`, `store_entries_evicted_rowless`
- `store_toolchains_dropped`
- `store_entries_held`, plus `store_entries_held_live`, `_undecided`, `_name_meta`, `_busy`, `_whole_store`
- `store_freed_bytes`, `store_bytes_still_shared`, `store_freeable_bytes_remaining`, `store_tmp_bytes`

Finding kinds:
- `store.target_undecided`, `store.target_unreadable`, `store.walk_fault`
- `store.toolchain_unresolved`
- `store.held_over_cap`, `store.busy`, `store.window_invalid`
- `store.rename_failed`, `store.write_failed`

Notes: `store.skipped_scoped` for `gc <repo>` (STOREGC-5.3), and `store.no_checkout_resolved` when STOREGC-2.5 blocks drops. The bytes counts go to `unmeasured` with "doctor check (c) did not pass" when `account_bytes` is false (STOREGC-7.3).

### gc command wiring (cli)

Satisfies: STOREGC-5.3, STOREGC-10.1, STOREGC-10.2
Reuse: rung 2 — `gc_cmd` (`src/cli.rs:816-977`), `live_checkouts` (`src/cli.rs:485`), `state::window_days` (`src/state.rs:216`)
Respects: ARCH-8, ARCH-14
Surface:
- `live_checkouts` callers `gc_cmd` (`src/cli.rs:862`), `plan_cmd` (`:1141`) and `explain_cmd` (`:1301`): `replace`. It returns `(checkouts, faults)`; `plan_cmd` and `explain_cmd` turn the faults into the same findings as today (STOREGC-10.2).
- The bare `gc` receipt readers (the hive skill's "Reading a receipt"; tests/gc.rs, which pins no full count set for `gc` — `tests/gc.rs:655` pins `status` only): `replace`. Counts are only added.
- The `gc <repo>` receipt: `replace`. Only the note of STOREGC-5.3 is added.
- GCPLAN-9.1 and SEED-10.1, guards in other specs that promise bare `gc` the same count names: `replace`. Each gets a dated note (decision 15).
Interface: unchanged CLI — `cargo hive gc [repos…] [--dry-run | --plan] [--json]`.
Depth: n/a — extends `gc_cmd`.
Locality: extend `src/cli.rs`. The eviction call is inserted after the cross-check notes (`src/cli.rs:903-912`) and before `put_counts` (`:916`), only when `repos` is empty, `--plan` is absent and `report.cancelled` is false. `window_days(cfg.gc_window_days)` failing is STOREGC-1.7: the finding is made, store eviction is skipped, and target deletions are kept.

### Skill and README

Satisfies: STOREGC-8.1, STOREGC-8.2, STOREGC-10.11
Reuse: rung 2 — the existing skill rows on `gc` (`plugin/skills/hive/SKILL.md:19,24,35-36,40,53-55,71`) and README lines 10 and 54
Surface:
- `tests/skill.rs:569-572` (it asserts "The store only grows until store GC ships"): `replace`. The assertion moves to the new sentence.
- `skills/hive/eval.json`: `leave`. It has no store sentence (review finding 7).
- SKILL.md has no store text and no bare `gc --json` command line. Both are added, and the new command line must parse (STOREGC-10.11).
Interface: the skill's text; no code.
Depth: n/a — extends the skill.
Locality: extend `plugin/skills/hive/SKILL.md` (71 of 150 lines), `README.md` (line 10 and the seed row at line 54), and `tests/skill.rs`. ROAD-13 rewrites the skill later.

### SEED design note

Satisfies: none — documentation upkeep for the Open Question on SEED design decision 6 and `store_bytes`.
Reuse: rung 2 — `docs/specs/2026-11-02-store-seed/design.md` decisions 5 and 6
Surface:
- SEED design decision 6's sentence "a published directory is complete and is never touched again": `replace`. A dated note adds "until store eviction removes it whole (STOREGC)". The SEED requirements are unchanged.
Interface: none.
Depth: n/a — documentation.
Locality: one dated line in the SEED design; `store_bytes` keeps its meaning (STOREGC-10.13).

## Seams for testing

New seams: one, `seed::evict::run` called as a library function with a small `Limits` (Weigh W10). The rest are existing seams.

| Seam | Kind | Covers |
|---|---|---|
| `cargo hive gc [--dry-run] --json` binary, `Hive` fixture with `RegistryRepo`, a seeded store and `store.db` rows edited to back-date `last_live_at` | integration | STOREGC-1.1, 1.2, 1.3, 1.4, 1.5, 1.6, 1.7, 1.8, 2.1, 2.2, 2.3, 2.4, 2.5, 4.1, 4.2, 4.3, 4.4, 4.5, 5.1, 5.2, 5.3, 7.1, 7.3, 7.4, 9.1 |
| `seed::evict::run` with a small `Limits.cap_bytes` (library) | integration | STOREGC-3.1, 3.2, 3.3, 3.4, 7.2 |
| the test process holds `store.lock` shared through `State::store_shared_or_cancelled` while it runs the `cargo hive gc` binary; `cargo hive gc` and `cargo hive seed` run concurrently in a loop (binary) | integration | STOREGC-6.1, 6.2, 9.4 |
| kill-during-eviction (the pattern of `tests/gc.rs:393`), then `store::read`, a seed and a fill | integration | STOREGC-9.2 |
| APFS disk image (`tests/gc_disk_image.rs` `Image`), with a compressed entry and an incompressible entry shared by a seeded clone | integration | STOREGC-9.3 |
| `doctor::resolve_toolchain_own_dir_until` with a fixture `rust-toolchain.toml` and `RUSTUP_TOOLCHAIN_SOURCE=default` set | unit | STOREGC-2.1 |
| `source::holds` on fixture targets (Decided, Undecided, unsupported, missing `.fingerprint/`, unreadable) | unit | STOREGC-1.2, 4.2, 4.3, 4.4 |
| tests/skill.rs | unit | STOREGC-8.1, 8.2, 10.11 |
| `cargo hive seed` with stale `store/.tmp/evict-*` trees present, then `store_bytes` and the `.tmp` sweep | integration | STOREGC-10.3, 10.13 |
| existing suites: tests/gc.rs, tests/plan_tier.rs, tests/plan_apply.rs, tests/seed.rs, tests/seed_fill.rs, tests/status*.rs, tests/explain.rs, tests/liveness*.rs, tests/doctor.rs, tests/capability_gate.rs | integration | STOREGC-10.1 to 10.13 |

## Coverage check

Requirements walked top to bottom, 2026-11-09.

| Section | Satisfies |
|---|---|
| Store lock | 6.1 |
| Seed holds the store lock | 6.2, 9.4 |
| Store access | 1.3, 1.5, 1.6, 2.3 |
| Held keys | 1.2, 4.2, 4.3 |
| Toolchains in use | 2.1, 2.4, 2.5 |
| Store eviction | 1.1, 1.4, 1.7, 1.8, 2.2, 3.1, 3.2, 3.3, 3.4, 4.1, 4.4, 4.5, 5.1, 5.2, 7.1, 7.2, 7.3, 7.4, 9.1, 9.2, 9.3 |
| gc wiring | 5.3, 10.1, 10.2 |
| Skill and README | 8.1, 8.2, 10.11 |

Guards are not owned by a section and are covered by the existing-suites seam row: 10.3 to 10.10, 10.12 and 10.13. They name behavior that must not move, and the existing suites plus STOREGC-10.x tests check it.

No ID is unmapped.

## Upstream sync-back

Applied to requirements.md and sent for re-approval with this design:
- **STOREGC-9.1** also allows creating `<state root>/store.lock` (decision 2). Seed creates the same file; that is a write in hive's state root, which ARCH-2 allows.
- **STOREGC-6.1** reads "an entry" where the design excludes per store (decision 2). Reworded: while a `seed` holds the store lock, store eviction removes no entry in that run; it tries the lock once without waiting, and counts the run as busy.
- **STOREGC-3.2** is not enforced in a run STOREGC-6.1 skips. Reworded to name that exception.
