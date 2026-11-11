# Design: Store and seed (a compressed store of registry units, cloned into a new secondary checkout)

Feature code: SEED
Status: Approved
Date: 2026-11-02
Requirements: ./requirements.md

## Context

Hive reads target dirs and deletes from them; it has never written into one. Discovery (`discover::checkouts`, `discover::targets`), the fingerprint reader, closure liveness and the owner locks all exist, and `doctor` already reports `Capability::Seeding` and `StoreCompression`, which nothing consumes (`src/doctor.rs:38`). What does not exist: a store, any code that maps a unit to everything Cargo needs to find it fresh, any reader of `.rustc_info.json`, a way from a checkout path to its registered repo, and a reachability walk outside the private `mark_live` (`.skills/SEED/design-scan.md`).

The binding constraint is that the slice must leave Cargo believing the units are fresh, while hive never compiles (ARCH-17), never installs anything in the build path (ARCH-1) and holds Cargo's own flocks as briefly as it can (ARCH-19). Cargo decides freshness; hive can only place the right files with the right mtimes. So every choice below is judged by one measured question: does a real first build recompile a unit hive cloned. This rules out spike 7's method, a target-shaped store cloned whole: it cannot be repo-scoped, cannot gather units across lanes, and would carry workspace units. The owner chose per-unit entries (`.skills/SEED/decisions.md`, locks 5 and 8).

The second constraint is that the store outlives every source. Entries are independent trees, published atomically and never modified after, so an interrupted fill or a concurrent seed cannot expose half an entry, and the source can be rebuilt or deleted afterward (SPIKES.md §12).

Retrieval (schema 1.1, recipe fsubr-1.2, derived by script from `passes.md`, 2026-11-02): OWNS coverage 6/7, with SEED itself the one without `tasks.md`; observations none. Neighbors by shared paths: CORE (9), GCPLAN (9, including `src/plan_tier/locks.rs`), AUTOGC (5, including `src/gc/ledger.rs`), EXPLAIN (4), LIVE (2, `src/liveness.rs`, `src/fingerprint.rs`), SKILL (2, `plugin/skills/hive/SKILL.md`, `tests/skill.rs`). Advisory; the reuse below comes from the scan digest. An independent review is at `.skills/SEED/design-review.md`; this version carries its fixes.

ARCH invariants this design relies on: ARCH-1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 14, 15, 16, 17, 19.

## Decisions

Decisions 1 to 13 of `.skills/SEED/decisions.md` are locked by the owner and not restated. These settle what they left to design, and what designing found.

1. **`store.db` is JSON, through `Tx`.** A flat file in the state root, written whole with `write_bounded`, as `plans.json` is. The name stays because ARCH-16 and the roadmap use it; the format is JSON because SQLite would be a new dependency, which nothing here requires. Rows hold no file lists: an entry directory is immutable, so the directory is its own list. A row is about 400 to 700 bytes in pretty JSON (an estimate, from about 15 fields and a few dependency hashes), so the 32 MiB cap holds about 50,000 rows, about 80 full klynt sets of 637 units. Over the cap is a reported failure, and SEED-9.7 covers it; deleting `store/` clears it, as decision 6 prunes the rows.
2. **The toolchain table lives in `store.db`, keyed by the whole `-vV` text.** No hash: the text is short, the requirement compares whole text, and no hash crate is installed.
3. **No clone id is stored in this slice.** SEED-4.1 reads both clone ids at seed time and needs no record. Only ROAD-10's eviction accounting could need one, and it can add a field then. So "when clone_id is written" is answered: not yet.
4. **The seed record is its own file, `seeds.json`,** read by `status` with a new bounded read that takes no lock. A unit is about 200 bytes, a klynt target about 130 KB, so the 16 MiB cap holds about 120 lanes. A write drops records whose target path is gone, then the oldest records until it fits. A dropped record means `status` reports none of the four counts for that target, which SEED-6.8 allows.
5. **Layout.** `~/.hive/store/<toolchain value>/<name-meta>-<fingerprint hash>/<paths relative to the target dir>`, plus `~/.hive/store/CACHEDIR.TAG`. Work in progress sits in `~/.hive/store/.tmp/<pid>-<key>/`; publishing is `rename` to the final path. A `.tmp` entry older than one day is removed at the start of a fill. The relative paths include the profile dir, so the entry records where each file belongs.
6. **A published directory is complete and is never touched again.** (2026-11-09, STOREGC: never modified; store eviction may remove a published directory whole.) Write order is clone, compress, rename, row. An entry is available when its row and its directory both exist, and `store::read` filters on that, one `entry_stat` per row, so `choose` stays pure. A directory with no row, left by a crash or a cancel, is adopted: the next fill for that key merges its row without cloning. A row whose directory is gone is pruned at the next merge, so deleting `store/` by hand is the whole of "delete the store". The loser of a rename race discards its copy and still merges the row.
7. **Lock window.** Compute live units and new keys with no lock. If a source target has no new keys and the store has its tag, take no lock on it at all: this is the common case for later seeds. Otherwise `locks::acquire`, re-read only the hash files of the new units, and for each whose hash is unchanged list its files and clone them, all under the lock. Drop the locks, then compress. Listing under the lock is what keeps one entry from mixing two compilations; liveness is not recomputed, since storing a unit that just became dead is harmless.
8. **Clones are per file, never per directory.** Per-file cloning is what allows the no-follow check before each file (SEED-9.8), the path check (SEED-9.2), the existing-file count (SEED-1.7) and sharing checks per file (SEED-4.1). A directory matching a unit's hash in `deps/`, such as a `.dSYM` bundle, is walked with the same bound.
9. **Clone order inside a unit:** `deps/`, then `build/`, then `.fingerprint/`. A unit whose seed is interrupted then looks absent to Cargo rather than present. The race trials saw no difference between orders (`.skills/SEED/decisions.md`), so this is reasoning, not a measurement.
10. **Registry rule.** A unit's `.d` file is at `deps/<stem>-<hash>.d` or `build/<dir>/*-<hash>.d`. Its first input, read bounded and with backslash-escaped spaces undone, must lie under `<CARGO_HOME>/registry/src/`, with `CARGO_HOME` from the environment, else `$HOME/.cargo`. A unit with no `.d` (a run-build-script unit) needs at least one edge, every one resolved to a registry unit. First-input and any-path gave the same answer on 88 and 581 units (`.skills/SEED/decisions.md`); first input is the tighter reading.
11. **Dependency keys.** A row's `registry_deps` holds fingerprint hashes, `le_hex(dep.fingerprint)`, not name-metas. A dependent built against fingerprint F1 must be dropped when the chosen entry for that name-meta has F2, because Cargo would rebuild it. An edge that resolves to nothing means the unit is not stored (SEED-2.24); that covers a dependency only a not-current node kept live.
12. **Seed fills even when it will not clone.** Path exists, no toolchain row and an unresolved target all still fill first (SEED-1.2). Refusals (SEED-1.10, 1.11, 1.15, 1.17) fill nothing. So `seed` on an existing worktree warms the store for later lanes without a second command.
13. **Field names, receipt schema 2.** Seed uses the existing builder; `plan_tier()` is not called. Counts: `fill_entries_added`, `fill_entries_held`, `fill_targets_read`, `fill_targets_skipped` (with `Reason::count_name` beside it), `clone_units_chosen`, `clone_units_cloned`, `clone_units_dropped`, `clone_units_existing`, `clone_units_failed`, `clone_units_outside_window`, `clone_bytes_not_shared`, `store_bytes`. Status, per target: `reused_fresh:<path>`, `seeded_unneeded:<path>`, `seeded_recompiled:<path>`, `seeded_missing:<path>`.
14. **`Fs` gains one required method, `create_dir`.** Writes into a target go through `Fs` today, and a required method makes each of the nine `impl Fs` state its behavior, where a default would let the four wrappers silently skip `inner`. The store's own `rename`, `create_dir` and `remove_dir_all` use `std::fs` directly: the store is inside the state root, and `state.rs` already does the same (`src/state.rs:359`).
15. **Reachability is a small new walk, not an extraction.** `mark_live` is fused with window roots and not-current nodes and carries LIVE's proof, and SEED-10.4 keeps it unchanged. The `status` walk needs only `hash_hex`, `deps` and `le_hex`, over all profiles of one target, about 20 lines.
16. **Compression outcome gains a variant.** `Apfs::compress` turns every Applesauce skip into `Failed` strings (`src/platform/apfs.rs:140`), but "empty" and "not compressible enough" are normal for an entry's small files (measured). A new `CompressOutcome::Incompressible`, set from the typed callbacks in `Watch` (`src/platform/apfs.rs:350-374`), carries them. A source file already compressed, as in a seeded target, is treated as finished and is not compressed again. Doctor's check b compares with `==` and is unaffected.
17. **A full disk during compression.** `Failed` carries no errno. After any `Failed`, `store` reads `Fs::volume(...).available_bytes`; less than the size of the file being compressed is SEED-2.20, anything else is SEED-2.22.
18. **Concurrent first seeds get little, and that is accepted under lock 7.** Lanes seeded together try-lock one source; the first fills, and the others are `Skipped(Held)` and find an empty or partial store. With nothing to clone they create nothing (SEED-1.18), so they build cold, and the receipt names `targets_skipped_held`. The skill says to seed one lane first when starting several. Waiting for another seed's entries is not designed; it can be added if a real run shows the loss matters.

No ADR: none of these is hard to reverse (Compat obligation is None, and the store can be rebuilt by refilling). Glossary owed: `CONTEXT.md` Store and Fill are corrected; the seven terms in `requirements.md` are added when this design is approved.

## Architecture

Vocabulary for every section below: **module**, **interface**, **implementation**,
**seam** (public surface where behavior is observable and substitutable). Prefer
deep modules — an interface much simpler than what it hides.

Layout: a new module `src/seed/`, a sibling of `src/plan_tier/`: `mod.rs` (`run`, `Outcome`, the flow), `source.rs`, `store.rs`, `fill.rs`, `choose.rs`, `place.rs`, `record.rs`. Its public surface is `run` and `record::counts`; `cli.rs` does loading, gating, receipt and `emit`, as it does for plans. In code the two numbers are `toolchain_value` and `fingerprint_hash`, since `rustc`, `toolchain` and `fingerprint` already mean other things.

```
cargo hive seed <checkout>
  cli: load -> find repo -> class -> resolve_toolchain(checkout) -> require(...)
  seed::run
    fill::fill      for each source target of the repo
                      live registry units, no lock -> new keys?
                      locks::acquire -> re-read hashes -> list and clone into store/.tmp -> drop locks
                      compress -> rename -> merge rows under the state lock
    store::read     rows whose directory exists
    choose::choose  this repo, this toolchain -> newest per name-meta -> drop open edges
    place::place    empty? nothing. Target path absent? create dirs, clone tag, clone units, read sharing
    record::write   seeds.json under the state lock
  cli: receipt, emit
```

### Command and flow (`src/cli.rs`, `src/seed/mod.rs`)

Satisfies: SEED-1.1, SEED-1.2, SEED-1.10, SEED-1.11, SEED-1.14, SEED-1.15, SEED-1.17, SEED-7.1, SEED-7.3, SEED-7.4, SEED-7.5, SEED-10.1, SEED-10.3, SEED-10.12
Reuse: rung 2 — the shape of `plan_cmd` and `apply_cmd` (`src/cli.rs:1045`, `:941`), `plan_tier::Ctx` and `plan_tier::checkout_walk`, `doctor::require`, `ReceiptBuilder`, `emit`, `doctor::CANCEL`
Respects: ARCH-5, ARCH-7, ARCH-8, ARCH-10, ARCH-13, ARCH-14
Surface: `Command` enum in `cli.rs`: `tests/skill.rs` `parse()` exhaustive match — replace (adds `Parsed::Seed`). `Toolchain` gains a field: see "Fingerprint and toolchain facts".
Interface: `Command::Seed { checkout: PathBuf, out: OutputArgs }`. `seed::run(ctx: &Ctx, repo: &Repo, checkout: &Checkout, toolchain: &Toolchain, cancel: &(dyn Fn() -> bool + Sync)) -> Result<Outcome, Error>`. `Outcome { counts, findings, unmeasured, notes, cancelled: bool }`. `Error { State(StateError), DiskFull(String) }`.
Depth: if `seed::run` vanished, callers would need only a repo, a checkout and a toolchain to rebuild a call to it; the five steps, their order and lock discipline stay inside.
Locality: new `src/seed/`; `cli.rs` extends with one command; leave `plan_tier`.

`cli.rs` canonicalizes the argument, loops `reg.repos`, calls `discover::checkouts` for each and matches `row.path == real && !row.missing`. These runs call git, not cargo. No match is SEED-1.11, tested before any class check, because `classify` would call an unregistered clone below a secondary root `Secondary`. A match with `Class::Protected` is SEED-1.10. Then `resolve_toolchain(checkout)`, whose error is SEED-1.15, and `doctor::require` for `Seeding`, `StoreCompression` and `Deletion` against that release (SEED-1.17). `Deletion` is required because check d, which proves hive's try-lock excludes Cargo, vouches only for it. The cwd is the checkout path given as an argument, so the caller's directory still selects nothing (ARCH-8, ARCH-13). `account_bytes` comes from `EvictionAccounting`, as for plans. A `Ctx` is built with `repos = [repo]` and every `mains`. Exit codes: 0, 1 for a refusal or an `Error`, 130 when `Outcome.cancelled`. Cancel is polled at every step boundary of fill and before each unit of place.

### Source reading (`src/seed/source.rs`)

Satisfies: SEED-2.2, SEED-2.3, SEED-2.4, SEED-2.24, SEED-9.7
Reuse: rung 2 — `plan_tier::checkout_walk`, `discover::targets` with `Snapshot::default()` (`src/plan_tier/build.rs:213`), `liveness::liveness`, `fingerprint::le_hex`, `liveness::embedded_hashes` (made `pub(crate)`), `Fs::read_dir_bounded`, `Fs::entry_stat`, `gc::DEPS_CAP`, `gc::CRATE_DIR_CAP`; the classifier is rung 7, as no code reads dep-info
Respects: ARCH-9, ARCH-13, ARCH-17
Surface: `liveness::embedded_hashes` private to `pub(crate)`: its one caller `claimants` — leave.
Interface: `live_registry_units(ctx, target: &TargetDir, now, window) -> Result<Vec<SourceUnit>, Skip>`, where `SourceUnit { rel_profile, dir_name, kind, fingerprint_hash, toolchain_value, registry_deps }`; `unit_files(fs, profile, unit, deps_listing) -> Result<Vec<RelPath>, Refused>`.
Depth: callers keep knowing only that a target yields units and a unit yields files; dep-info parsing, edge inference, no-`.d` units, the `.rcgu.o` exclusion and each bound stay hidden.
Locality: new file; `liveness.rs` extends by one word.

Only `Owner::Owned` targets of this repo's checkouts are read, with `all_repos = false`, which still finds a cache-root target this repo owns. A walk fault is a finding and does not stop the fill: unlike `apply`, nothing here deletes, so a hidden owner cannot do harm. `Undecided` or an unsupported profile is SEED-2.4. Live units with an empty hash file are filtered (SEED-2.3), since live includes not-current nodes. `deps/` is listed with `DEPS_CAP`, as `liveness` does, and unit directories with `CRATE_DIR_CAP`. A non-UTF-8 name in `deps/` skips the target; one under `build/<dir>/` refuses that unit. Each `.d` read stops at 1 MiB. `.rustc_info.json` is not read here.

### Fill (`src/seed/fill.rs`)

Satisfies: SEED-2.1, SEED-2.5, SEED-2.6, SEED-2.7, SEED-2.8, SEED-2.14, SEED-2.15, SEED-2.16, SEED-2.17, SEED-2.18, SEED-2.19, SEED-9.8, SEED-10.8
Reuse: rung 2 — `locks::acquire` and `Locked` unchanged, dropped before compressing (`src/plan_tier/locks.rs:43`), `Fs::clone_file`, `Fs::entry_stat`
Respects: ARCH-2, ARCH-4, ARCH-6, ARCH-19
Interface: `fill(ctx, repo, targets, cancel) -> FillReport { entries_added, entries_held, targets_read, targets_skipped, findings, cancelled }`.
Depth: callers know only that afterward the store holds this repo's live units; the lock window, the hash re-check, the no-follow walk, the tag and the toolchain row stay inside.
Locality: new file; leave `plan_tier`.

For each target: units from `source`. A key with a row or a directory needs no clone; its repo and `last_live_at` are merged. If new keys remain, or the store has no tag, `locks::acquire(fs, target, false)`. `Skipped` or `Failed` is SEED-2.6; a seeded target no build has touched lands here as `lock_missing`, which is correct. Under the lock: re-read each new unit's hash file; for each unchanged one, list its files, `entry_stat` each, and on any non-regular entry refuse the whole unit (SEED-9.8), else `clone_file` each into `.tmp`. `EXDEV` stops that target with one finding (SEED-2.8). Clone the tag once if the store has none. Drop the locks. Then compress, publish and merge through `store`, outside every lock, merging on the cancel path too.

The toolchain row: read `.rustc_info.json` from the target, take the one `outputs` entry whose `stdout` has a `release:` line (none or several is malformed, SEED-2.17; its key is not stable), and compare its text with `resolve_toolchain(source checkout).version_text`. A row needs equal text and one toolchain value across all units of the target; `.rustc_info.json` was measured byte-identical to `rustc -vV`. A target whose dead units carry an older toolchain writes no row until they are collected, so the first row for a new toolchain comes from a fresh target. Identical text and value is one map key, which is SEED-2.18.

### Store (`src/seed/store.rs`)

Satisfies: SEED-2.9, SEED-2.10, SEED-2.12, SEED-2.13, SEED-2.20, SEED-2.21, SEED-2.22, SEED-2.23, SEED-9.3, SEED-9.4
Reuse: rung 2 — `Tx::read_bounded`, `Tx::write_bounded`, `State::locked_or_cancelled`, the read-merge-write of `plan_tier/record.rs`, `Fs::compress`, `Fs::volume`; `std::fs::rename` (rung 3) for publishing
Respects: ARCH-6, ARCH-16
Surface: `CompressOutcome` gains `Incompressible`: `doctor.rs` check b — leave (it compares with `==`), `Display` in `platform/mod.rs` — replace, `Apfs::compress` and `Watch` — replace. Test doubles construct only existing variants — leave.
Interface: `read(state) -> StoreFile` (available rows only), `compress_and_publish(fs, tmp, key, cancel) -> Published`, `merge(state, rows, table_rows, cancel)`. `StoreFile { units: Vec<Row>, toolchains: BTreeMap<String, u64> }`. `Row { toolchain_value, name_meta, fingerprint_hash, kind, rel_profile, size, filled_at, last_live_at, repos: Vec<PathBuf>, registry_deps: Vec<String> }`.
Depth: callers keep knowing only that a row from `read` is a complete, compressed entry; the temp directory, the rename, adoption, pruning and the cap stay hidden.
Locality: new file; `platform` extends by one enum variant.

The state lock is held only for the merge, never across a clone or a compression (SEED-9.4). `compress_and_publish` walks the entry and calls `Fs::compress` per file. `Incompressible`, and a file already compressed, count as finished (SEED-2.23). `SkippedHardLink` is a failure, since a clone just made has `nlink` 1. Compressing before the rename is what makes SEED-2.9's "no checkout shares it" true by construction: a checkout can only clone from a published entry, and a published entry is never compressed again. A rename that finds the destination present, `EEXIST` or `ENOTEMPTY`, discards the copy and counts the key held. `merge` drops rows whose directory is gone before it writes, and the cap applies after that.

### Choosing units (`src/seed/choose.rs`)

Satisfies: SEED-3.1, SEED-3.2, SEED-3.3, SEED-3.4, SEED-3.5, SEED-3.6, SEED-3.7, SEED-3.8
Reuse: none — new code (rung 7): no closure-drop exists, and this is a pure function of rows.
Interface: `choose(store: &StoreFile, repo: &Path, version_text: &str) -> Choice`, where `Choice` is `Units { rows, dropped }` or `NoRow { release }`.
Depth: if this vanished, callers would need only the store, the repo and the toolchain text, and would rebuild filter, newest-per-name and a fixpoint: the interface is three values against four rules.
Locality: new file, no I/O; leave everything else.

`registry_deps` was recorded at fill from edges resolved to registry units, so non-registry dependencies are absent by construction (SEED-3.7). A tie on `last_live_at` takes the larger fingerprint hash, so the result is deterministic. `last_live_at` is global across repos, so another repo's fill can reorder this repo's newest; with 0 of 637 name-metas holding two fingerprints on klynt, this is accepted. `seed::run` checks `store/CACHEDIR.TAG` before calling `place`, for SEED-3.8.

### Placing units (`src/seed/place.rs`)

Satisfies: SEED-1.3, SEED-1.4, SEED-1.5, SEED-1.6, SEED-1.7, SEED-1.8, SEED-1.9, SEED-1.12, SEED-1.16, SEED-1.18, SEED-1.19, SEED-4.1, SEED-4.2, SEED-4.3, SEED-4.4, SEED-7.2, SEED-9.2, SEED-9.5, SEED-10.9
Reuse: rung 2 — `cargo_target::resolve`, `Fs::clone_file`, `Fs::sharing`, `Fs::entry_stat`, `size::allocated_bytes` for the store's size, `liveness::unit_timestamp`, the window from `Config`
Respects: ARCH-2, ARCH-3, ARCH-5, ARCH-6, ARCH-10, ARCH-11, ARCH-15
Surface: `Fs` gains required `create_dir`: `Apfs` — replace; `explain::DirListFs` — replace (`Unsupported`, as its other methods); wrappers `FailStat` (`plan_tier/build.rs:637`), `Volumes` (`tests/liveness.rs:968`), `Recording` (`tests/common/recording_fs.rs:189`), `liveness::FakeFs` (`liveness.rs:1570`) — replace (forward); doubles `doctor::FakeFs`, `explain::FakeFs`, `gc/run.rs` `Rec` — replace (trivial).
Interface: `place(ctx, checkout, units: &[Row], cancel) -> PlaceReport { placed: Vec<SeededUnit>, cloned, existing, failed, outside_window, bytes_not_shared, findings, unmeasured, cancelled }`.
Depth: callers keep knowing only that an absent target gets a seed and a present one gets a finding; the order, the path check and the sharing measurement stay inside.
Locality: new file; `platform` extends by one method.

An empty set returns at once, creating nothing (SEED-1.18). `resolve` runs at the checkout's root; a root with no `Cargo.toml`, an error, or `build_directory != target_directory` is SEED-1.12. `Fs::create_dir(target_directory)` is the check: `EEXIST`, for an existing path or a dangling symlink, is SEED-1.4 with no window between test and create, and two racing seeds resolve here (probed). `ENOENT` is SEED-1.19, since creating a missing parent would write outside the target (ARCH-2). Inner directories are made one component at a time, tolerating `EEXIST`. Then the tag, then units in the order of decision 9, each walked from its entry directory with `CRATE_DIR_CAP`, in the store. Each relative path is checked to hold only normal components before it is joined (SEED-9.2). `EEXIST` from `clone_file` counts as existing (SEED-1.7); other errors follow SEED-1.16; `ENOSPC` follows SEED-2.20. Mtimes come with the clone. A placed file that still carries the compressed-file flag is replaced, in the same directory, by a new regular file of the same logical bytes. That file keeps the clone's mode and gets the clone's mtime back, before sharing is read. The store entry stays compressed.

Each cloned file reads `Fs::sharing` for itself and for its store file: shared only with `shares_all` and equal `clone_id`; otherwise its `st_blocks * 512` go to `clone_bytes_not_shared`. `private_bytes` is not read. `store_bytes` is `size::allocated_bytes(store root)`, and a walk error from a racing `.tmp` is an `unmeasured` row. SEED-1.8 follows from cloning a tag `identify` already accepted, plus at least one profile dir. SEED-1.9 is a property of the whole slice, proved at its seam.

### Seed record and status counts (`src/seed/record.rs`, `src/cli.rs` `report_targets`)

Satisfies: SEED-6.1, SEED-6.2, SEED-6.3, SEED-6.4, SEED-6.5, SEED-6.6, SEED-6.8, SEED-6.9, SEED-6.10, SEED-6.11, SEED-6.12, SEED-10.2
Reuse: rung 2 — `Tx::write_bounded`, `TargetDir.profiles[].graph` already in memory in `status`, `fingerprint::le_hex`, `liveness::unit_timestamp`, the `bytes:<path>` count precedent (`src/cli.rs:609`); the walk is rung 7 (decision 15)
Respects: ARCH-10, ARCH-16
Surface: `report_targets` (`src/cli.rs:654`) gains a record argument; its one caller (`src/cli.rs:627`) — replace. Existing counts — leave, four more per target only when a record exists. `State` gains `read_bounded`: no existing reader changes. `tests/status_targets.rs`, `tests/receipts.rs` — leave (no record, same output).
Interface: `record::write(state, target, identity, units)`, `record::counts(target: &TargetDir, record: &SeedRecord) -> Option<SeedCounts>`. `SeedRecord { identity: (u64, u64), units: Vec<SeededUnit> }`. `SeededUnit { rel_profile, dir_name, stamp: Option<(RelPath, u64 /* ns */)>, has_build: bool, deps_files: u32 }`.
Depth: `status` keeps knowing only "a record gives four numbers"; what counts as compiled, the walk, the identity check and the exclusive ordering stay inside.
Locality: new file; `cli.rs` extends one loop.

`identity` is the target directory's device and inode; a mismatch is SEED-6.12, which covers a deleted and rebuilt target. Records are matched by (profile, directory name). Each unit lands in one count, tested in this order: missing (SEED-6.11), where `deps/` is listed once per profile and files per hash are counted with `embedded_hashes`, leaving out `.rcgu.o`; recompiled, when the stamp differs, including none at seed and some now; else fresh when reached and unneeded when not. Starts are units in the graph not in the record, and recorded units whose stamp differs. Edges follow `le_hex(dep.fingerprint)` against `hash_hex` over all profiles of the target. The counts read no ownership, so the `orphan_candidate` bug in `status` cannot change them. A missing file means no record; an unreadable or oversized file gives no counts and one note. The added stats are one per seeded unit plus one listing per profile.

### Ledger (`src/gc/ledger.rs`)

Satisfies: SEED-5.1, SEED-10.7, SEED-10.15
Reuse: rung 2 — one condition at `src/gc/ledger.rs:66`
Respects: ARCH-11, ARCH-18
Surface: `Ledger::file` result for a compressed file with `shares_all`: `gc::measure` and `plan_tier/build.rs` planned bytes — leave (smaller, truer numbers); `plan_tier/apply.rs` and `orphan.rs` — leave; `gc::run` — leave; bytes in plans already stored in `plans.json` — frozen, the design builds around them: they were measured before compressed clones existed, and a re-plan recomputes them. Test `compressed: private size is not trusted` (`ledger.rs:191`) — leave, it sets `shares_all: false`.
Interface: unchanged.
Depth: n/a — extends `Ledger`.
Locality: one line, one new table case.

A compressed `SHARES_ALL` file now records a private size of 0 by the flag, not from `private_bytes`, so `freed` is 0 and every block is `still_shared`. The other cases keep the existing condition. With accounting off the ledger gets `None` and behaves as it does today, which is why SEED-5.1 names the capability. SEED-10.15 needs no code: `apply`'s `Skipped(Missing)` already does it (measured).

### Fingerprint and toolchain facts (`src/fingerprint.rs`, `src/doctor.rs`)

Satisfies: SEED-10.4, SEED-10.6, SEED-10.10, SEED-10.11
Reuse: rung 2 — extends `Unit` and `Toolchain`
Respects: ARCH-9, ARCH-14
Surface: `Unit` gains `#[serde(skip)] toolchain_value: u64`: `read_unit` (`fingerprint.rs:285`) and the helper in `explain.rs:947` — replace. `Toolchain` gains `#[serde(skip)] version_text: String`: `resolve_toolchain` (`doctor.rs:420`) — replace; its six callers in `cli.rs`, `DoctorEnv` and two in `tests/` — leave (same `release`). `liveness()` and `mark_live` — leave, unchanged.
Interface: two fields.
Depth: n/a — extends `Unit`, `Toolchain`.
Locality: two fields; `check_types` still checks the same 11 keys.

### Skill (`plugin/skills/hive/SKILL.md`, `tests/skill.rs`, `skills/hive/eval.json`, `skills/hive/TESTS.md`)

Satisfies: SEED-8.1, SEED-8.2, SEED-8.3, SEED-10.13
Reuse: rung 2 — the seven existing headings and the existing test machinery
Respects: ARCH-8
Surface: `Which command` table, `Command lines`, `Version first` bullet (`SKILL.md:19`), `Reading a receipt`, `Do not` — replace. `tests/skill.rs` `Parsed`, `parse()`, `want` list and path-word rule — replace. `eval.json` contract quotes — replace where the quoted text moves. A new eval needs a `TESTS.md` heading first (`derived_from`), so it needs a recorded run.
Interface: `cargo hive seed /path/to/checkout --json` on one command line.
Depth: n/a — extends the skill.
Locality: within the 150-line budget (65 today); no new heading. The skill also says to seed one lane before starting several (decision 18).

### Klynt acceptance (no code)

Satisfies: SEED-9.6
Reuse: rung 2 — the shape of the ignored tests in `tests/guards.rs:328`
Interface: an `#[ignore]` test that seeds a new klynt worktree, builds, and checks SEED-1.9 and SEED-4.1, plus a dated record of the four counts beside the spike 7 comparison in the spec directory.
Depth: n/a — a recorded run.
Locality: `tests/`, and `docs/specs/2026-11-02-store-seed/`.

## Seams for testing

| Seam | Kind | Covers |
|---|---|---|
| `seed::choose::choose` | unit | SEED-3.1 to 3.7 |
| `seed::source` on fixture dirs with hand-written `.d` files | unit | SEED-2.3, 2.4, 2.24, 9.7, Registry unit, Entry |
| `seed::store` with a scratch state root, hand-built directory states for a crash, cancel polled after each step | unit | SEED-2.9, 2.10, 2.12, 2.13, 2.20, 2.21, 2.22, 2.23, 9.3, 9.4 |
| `seed::record::counts` over a hand-built `TargetDir` | unit | SEED-6.2 to 6.6, 6.8, 6.9, 6.11, 6.12 |
| `gc::ledger` table in `ledger.rs` | unit | SEED-5.1, 10.7 |
| `fingerprint`, `doctor` existing unit tests, plus one each | unit | SEED-10.10, 10.11 |
| `seed::run` through `Recording` (`tests/common/recording_fs.rs`) | integration | SEED-1.5, 1.7, 1.16, 2.5, 2.6, 2.7, 2.8, 2.14, 2.19, 3.8, 4.1, 4.3, 9.2, 9.5, 9.8, 10.8 |
| `cargo-hive seed` binary, `Hive` scratch home (`tests/common/mod.rs`) | integration | SEED-1.1, 1.2, 1.3, 1.4, 1.6, 1.8, 1.10, 1.11, 1.12, 1.14, 1.15, 1.17, 1.18, 1.19, 2.1, 2.2, 2.15, 2.16, 2.17, 2.18, 4.2, 4.4, 6.1, 6.10, 7.1 to 7.5, 10.1, 10.2, 10.3, 10.9, 10.12 |
| New fixture: a `local-registry` crate with a build script under a scratch `CARGO_HOME`, built with real `cargo` | integration | SEED-1.9, 10.4, 10.6, 10.15, 5.1 end to end |
| `tests/skill.rs` | integration | SEED-8.1, 8.2, 8.3, 10.13 |
| Ignored klynt test | manual | SEED-9.6 |

New seams: the local-registry fixture, additions to `tests/common` (`Recording` logs every mutating call and each unlock, and can fail or plant a chosen clone; `Hive::hold_state_lock` holds `$HIVE_HOME/.lock` for SEED-9.4; `LibCtx::open`; `Hive::write_capabilities`), and one `#[cfg(test)]` double `Hooked` in `src/platform/mod.rs`, a tenth `impl Fs`. The fixture was probed in design review: a `local-registry` crate unpacks under `<CARGO_HOME>/registry/src/`, its `.d` first input is that path, its build script gives the three-unit shape, and a clone in a second project printed `Fresh`. A plain path dependency would be weaker, since Cargo tracks it by mtime and a registry package by version. On macOS the tar needs `COPYFILE_DISABLE=1`.

## Coverage check

Every ID in `requirements.md` appears in exactly one `Satisfies:` line above, except struck IDs SEED-1.13, SEED-2.11, SEED-6.7, SEED-9.1 and SEED-10.5, superseded by SEED-1.17, SEED-2.22, SEED-6.11, SEED-9.8 and SEED-10.15. SEED-10.14 (no new dependency) is met by decisions 1 and 2, and is left deliberately unmapped, since no module implements a dependency's absence; the plan's gate checks `Cargo.toml`.

## Design note, 2026-11-11: one source build per seed, and an uncompressed store

Root-cause on klynt (`.skills/verify/2026-11-11-klynt-sim/report.md`) and the owner's decisions of 2026-11-11 change three things. They supersede decision 2 (compress in the store) and the per-name-meta choice of decision 13 (SEED-3.5).

- **No compression (SEED-2.28, SEED-2.29, SEED-1.20).** rustc reports E0463 for an rmeta that keeps `UF_COMPRESSED`, so `place` expanded every clone of a compressed store file into a private copy. `store::publish_entry` now only checks each file (regular, one link) and renames the entry into place. A clone shares its blocks with the store file, which shares them with its source target. `place` still expands an entry an older hive compressed.
- **Variants (SEED-2.30, SEED-2.32).** A crate that `include!`s a file from `OUT_DIR` embeds that target's path in its metadata, so two targets' copies share a store key but not an SVH. `Row`/`StoreKey` gain `variant`. The entry directory is `<name-meta>-<hash>` for variant 0, as before, and `<name-meta>-<hash>~<n>` after that. `fill::holding_variant` probes the key's variants on disk and holds the unit under the first whose files have the unit's bytes (`store::same_unit_bytes`; `.d` and `root-output` are not compared; a shared clone id short-cuts the read), else gives it the next variant. `publish_entry` re-checks on a rename collision and moves to the next variant when the bytes differ, so two racing fills cannot share a variant with different bytes. No hashing crate is added: bytes are compared.
- **Source records and one-source choice (SEED-2.31, SEED-3.9 to 3.11).** `StoreFile.sources` maps each source target to its repo, checkout, the newest nanosecond mtime of its units' fingerprint hash files, and the key and variant of each unit it held at its last fill. Records of targets that no longer exist are dropped at each merge. `choose::source_order` ranks this repo's records: a checkout whose `Cargo.lock` matches the seeded checkout's first, then newest, then more units, then path. `choose::choose` clones only the first ranked record's units at their variants, then applies the drop rule. No record for the repo is `source.none`; a record whose units are all gone is `units.empty`.

Eviction is unchanged in shape. A live name-meta still holds every variant, through `TargetHolds::name_metas`.
- **One mtime for a placed target (SEED-1.21).** Identical bytes from two builds share a variant, but carry the first filler's mtimes. A proc-macro dylib's bytes can differ between builds while its SVH does not, so it becomes a new variant with a newer time while its dependents stay held with the older one, and Cargo rebuilds them as `StaleDependency` (87 seeded units per klynt lane in the second run). `place::align_mtimes` sets every placed file to the newest mtime among them, after the window is judged on the store stamps, and rewrites the placed units' stamps to that time for the seed record.
