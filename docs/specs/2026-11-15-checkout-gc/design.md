# Design: Checkout GC

Feature code: CHECKOUTGC
Status: Approved
Approved: 2026-11-16, by the operator (Jayden), on the spec sheet at a16e5e0: "approve" (.skills/_pending-checkout-gc/auto/operator-reply.md)
Date: 2026-11-15
Requirements: ./requirements.md

## Context

An agent that keeps a worktree at a stable path and switches it between tasks slows down task by task. The main cause is loose debug objects (`*.rcgu.o`) left in `deps/` under macOS's unpacked split-debuginfo, about 10k per workspace test build, plus dead units (MILE-5.notes.md:11-18). Hive already has closure liveness (`gc::liveness::liveness`), per-target Cargo try-locks (`locks::acquire`), entry guards (`plan_tier::guard`) and per-inode byte accounting (`ledger::remove_counted`). But dead units are deleted only through a stored plan (`gc --plan`, then `apply`), and only per checkout class. Nothing reads a binary's debug map, and liveness never lists an entry that carries a kept unit's hash (LIVE-3.5), so a live unit's stale objects are unreachable today.

The binding constraint is that one run deletes directly on one named checkout and touches nothing else (ROAD-16.md:3-5, 19-21), under the same safety rules the plan tier proves: re-read under the held locks, try the locks without waiting, check each entry's identity before deleting it (ARCH-19, ARCH-20 in spirit). That rules out two alternatives:
- **Reusing `apply::execute` with an unstored plan.** `execute` starts by refusing on the stored plan's window and identity fence (crates/gc/src/plan_tier/apply.rs:384-392), so using it without storing would fork a contract GCPLAN guards.
- **Reusing `Scope` / `TargetDir.owner` for "this checkout only".** `Owner` names one checkout, and `Scope::contains` matches any target under a checkout's path, so a main would claim `<main>/.worktrees/*/target` (crates/cargo/src/discover.rs:32-57, 664).

The new path therefore composes the shipped pieces in a new `gc::checkout` module, and computes which checkouts resolve to each target from the per-workspace `cargo metadata` results that discovery already holds.

The second constraint is reading debug maps with no child process (ARCH-17), no new dependency (docs/standards/code.md:16), and no `cfg(target_os)` outside the platform crate (ARCH-15). So the Mach-O reader is a pure byte parser in `crates/gc`, fed through a new ranged read on the platform `Fs`. That keeps it compiling for `x86_64-unknown-linux-gnu`, with no persisted cache (charter hard stop).

Deleting a loose object cannot trigger a rebuild. Cargo's fingerprint excludes debug-info outputs and never lists `.o` files (research note §6, read from source). The spike measured it: 115,248 objects were removed and no recompile came from them (MILE-5.notes.md:17-18). The objects only feed the debugger's debug map, and lldb skips a mismatched object without failing.

Fresh retrieval (load-subgraph, neighbors of CHECKOUTGC, terms and scan paths; owns_coverage 12/13, since CHECKOUTGC has no `tasks.md` yet; observations none) names these neighbors:
- **GCPLAN** (term: `remove_counted`, `check_entry`), whose guard and ledger pieces this design reuses.
- **SEED** (term: `locks::acquire`, `rcgu.o`), whose guards SEED-10.6 and 10.8 bind.
- **LIVE** (term: `rcgu.o`, `dead unit`).
- **SPLIT** and **UPGRADE** (path: `crates/cargo-hive/src/cli.rs`, `crates/cargo/src/discover.rs`, `crates/platform/src/lib.rs`).

Every `Reuse:` line below rests on the scan digest (`.skills/_pending-checkout-gc/auto/design-scan.md`), not on the envelope alone.

## Decisions

1. **Surface:** `cargo hive gc --checkout <PATH> [--json]`, a flag on `Gc` that conflicts with `repos`, `--plan` and `--dry-run`. The receipt command is `"gc --checkout"`, schema 2 (Fork 2; the cli.rs:1274 precedent).
2. **One run, two classes, one hold of the locks per target:**
   - acquire;
   - re-read the profiles fresh;
   - liveness, then delete the dead units;
   - list and read the linked binaries;
   - delete the unreferenced objects;
   - release.

   No auto tier and no store eviction (Fork 2 A).
3. **Ownership from resolutions:** `discover::targets` gains a `workspaces` field carrying every per-workspace resolution result. A target belongs to the checkout only when three things hold:
   - the checkout's own resolutions name it;
   - it lies inside the checkout's directory and not inside a checkout nested in it;
   - no other live checkout's resolutions name it.

   Any failed resolution in the repo blocks every deletion, with exit 1 (CHECKOUTGC-3.5 and 3.7, agent-decided).
4. **The reader is pure and lives in `crates/gc`** (`gc::debug_map`), over bytes from a new `Fs::read_at`. No new crate, since a 9th member would add a publish, and no dependency.
5. **Linked binaries are found by header, not by name.** Every regular file, or symlink to one, in the scanned directories gets a 32-byte header read. The one exception is names ending in `.o`, which are objects Cargo never links; that skips the ~10^5 `*.rcgu.o` files, which dominate `deps/`. Dep-info, rlib, rmeta and stamp files cost one small read each, a few thousand per profile (design review F10).
6. **Object matching is by file name** across every loose N_OSO entry of the profile dir's binaries. `<archive>(<member>)` entries are ignored (CHECKOUTGC-2.3, 2.4).
7. **Fail closed per profile dir** for the object class (2.7–2.9), independent of liveness's verdict (2.6).
8. **The CLI tree fixture:**
   - `tests/fixtures/v0.1.0/cli-tree.txt` stays as written.
   - A new `tests/fixtures/current/cli-tree.txt` holds today's tree.
   - `cli_tree.rs` asserts the current tree equals it, and that its diff from v0.1.0 touches only the `gc` block and the root command list's `gc` line (fixture line 11), which carries gc's help text (CHECKOUTGC-6.6, agent-decided).
9. **No ADR.** Every choice above can be reversed by a later release without migrating anything, since no state format changes. GOAL-9 already covers the new deletion class.

## Architecture

Vocabulary for every section below: **module**, **interface**, **implementation**,
**seam** (public surface where behavior is observable and substitutable). Prefer
deep modules — an interface much simpler than what it hides.

Data flow of one run:

```
cli::checkout_cmd(path)
  ├─ find_checkout(reg, cfg, path) ─────────────► (Repo, Checkout) | refusal 3.3
  ├─ window_days, doctor::require ──────────────► refusal 1.3 / 1.9
  ├─ discover::checkouts(repo) → discover::targets(all live checkouts of repo)
  │      └─ TargetScan { targets, workspaces (new), … }
  ├─ gc::checkout::run(&CheckoutInput, cancel) ─► CheckoutReport
  │      ├─ select_targets(scan, checkout) ────► mine | shared(3.2) | blocked(3.5)
  │      └─ per mine target, under locks::acquire(fs, t, true):
  │            fresh profiles → liveness → delete dead units (unit order)
  │            → debug_map::read per linked binary → delete unreferenced *.rcgu.o
  ├─ merge_snapshot under state lock (as gc_cmd)
  └─ emit(Receipt "gc --checkout", schema 2)
```

### Ownership resolutions in discovery

Satisfies: CHECKOUTGC-3.1, CHECKOUTGC-3.2, CHECKOUTGC-3.5
Reuse: rung 2 — extends `discover::targets` (crates/cargo/src/discover.rs:530-660) and its private `resolve_workspaces` (:425), whose results it already computes and drops
Respects: ARCH-8, ARCH-17 (ownership only from `cargo metadata --no-deps --locked --offline`, metadata.rs:25-36)
Surface:
- `TargetScan` gets a new `pub workspaces: Vec<WorkspaceResolution>` field, constructed only at discover.rs:652. Every caller reads fields by name, and none destructures the struct. So every caller, found by `rg 'targets\(' crates`, is **replace** (no edit needed; the existing fields keep their values):
  - cli.rs `gc_cmd`, `status`, `seed` and `bench`;
  - `plan_tier::build::make`, `plan_tier::orphan`, and `apply.rs:600`;
  - `seed/source.rs`;
  - the tests.
- The existing `owner` and `failure` computations: **replace**, unchanged. CHECKOUTGC-6.1 guards them.
Interface: `pub struct WorkspaceResolution { pub dir: PathBuf, pub checkout: PathBuf, pub result: Result<Resolved, String> }`. Beside it, `pub fn resolvers_of(scan: &TargetScan, target: &Path) -> Vec<PathBuf>` gives the distinct checkouts whose canonicalized `target_directory` or `build_directory` equals the canonicalized `target`. `pub fn failed_workspaces(scan: &TargetScan) -> Vec<(PathBuf, String)>` returns every failure, not only the first.
Depth: n/a — extends `discover::targets`.
Locality: extend `discover.rs`. Neighbor impact: leave `protected_owner`, `owner_checkout` and `Scope`.

`checkout` is the deepest live checkout containing `dir` (the same rule as `owner_checkout`, :509). `Resolved` paths are already realpaths (metadata.rs:80-86), so `/tmp` and `/private/tmp` spellings match. The target side is canonicalized too. A cancel can truncate `resolve_workspaces`, so callers check `cancel()` right after `targets()` and trust nothing when it fired.

### Ranged read on the platform `Fs`

Satisfies: CHECKOUTGC-2.10 (no child process: a read, not `nm`), CHECKOUTGC-7.1 (reads only header, symbol table and string table)
Reuse: rung 3 — `std::os::unix::fs::FileExt::read_at` inside `Apfs`. The trait default goes through the existing `Fs::read`.
Respects: ARCH-15 (the only OS access stays in `crates/platform`)
Surface:
- The new trait method `fn read_at(&self, path: &Path, offset: u64, len: usize) -> io::Result<Vec<u8>>` has a default body that slices `self.read(path)`. So all 12 `impl Fs` are **replace** with no edit:
  - `Apfs`, `DirListFs`, `Refuse`, `Hooked`;
  - doctor `FakeFs`, gc liveness `FakeFs`, explain `FakeFs`;
  - run.rs `Rec`, build.rs `FailStat`;
  - tests `Volumes`, `NamedShare`, `Recording`.

  Delegating wrappers return the same bytes because the default calls their own delegating `read`, at the cost of reading the whole file. Only `Apfs` (a positioned read) and `Recording` (tests/common/recording_fs.rs:234: logs the call and offers a `fail_read_at(path)` hook) override it. So "only tables are read" is shown on `Apfs` and through `Recording`, not through the other wrappers.
Interface: `read_at(path, offset, len)` returns up to `len` bytes from `offset`, fewer at end of file, an empty vector past it. Like `read`, it follows a symlink. That is safe because it only reads: every deletion goes through `check_entry` (lstat).
Depth: n/a — extends `Fs`.
Locality: extend `crates/platform/src/lib.rs` (the trait) and `apfs.rs`. Neighbor impact: leave other methods.

### Debug-map reader (`gc::debug_map`)

Satisfies: CHECKOUTGC-2.3, CHECKOUTGC-2.4, CHECKOUTGC-2.7 (parse failures and bounds)
Reuse: none — new code (rung 7). There is no Mach-O reader in the workspace or `Cargo.lock`, a dependency needs the owner (docs/standards/code.md:16), and `nm` is a process (ARCH-17).
Respects: ARCH-15 (pure bytes; the read is the caller's `Fs`), ARCH-17
Interface:
```
pub struct DebugMap { pub loose: BTreeSet<String>, pub archive_entries: u64 }
/// Ok(None): not a linked binary (no Mach-O magic, an object file, or a dSYM).
pub fn read(fs: &dyn Fs, path: &Path) -> Result<Option<DebugMap>, MapError>
pub enum MapError { Read(io::Error), Truncated(&'static str), Bound(&'static str, u64), UnknownSlice(u32), Unsupported(&'static str) }
```
Depth: if this module vanished, callers would still need to know only "is this file a linked binary, and if so, which loose object file names does its debug map name, or why can't you tell". Everything else is hidden: header and fat-header layout, load-command walking, `nlist_64` decoding, string-table bounds and archive-entry classification.
Locality: new file `crates/gc/src/debug_map.rs` with inline unit tests on hand-built byte images. Neighbor impact: leave.

Rules, with each layout fact cited in the research note `.skills/research/2026-11-15-macho-n-oso.md` (main checkout; V = verified against Apple's `loader.h`, `nlist.h`, `stab.h`, `fat.h` and LLVM `MachO.h`):
- **Magic.** The first 4 bytes decide. A file shorter than 4 bytes, or with no Mach-O or fat magic, is `Ok(None)`. A file with a Mach-O magic but shorter than its 32-byte header is `Truncated`, so the profile fails closed.
- **Thin 64-bit little-endian** (`0xfeedfacf`) is parsed; `filetype` is at offset 12.
  - `filetype` `MH_OBJECT` or `MH_DSYM` → `Ok(None)`.
  - Every other file type is a linked binary. That covers executable, dylib and bundle, and keeps more for the rare kinds.
  - A 32-bit or big-endian thin header is a Mach-O the reader cannot parse → `Unsupported`, so the profile fails closed.
- **Fat** (`0xcafebabe` with 20-byte `fat_arch`, `0xcafebabf` with 32-byte `fat_arch_64`, big-endian) is parsed.
  - `nfat_arch` must lie in 1..=`FAT_ARCH_CAP` (8). Otherwise → `Unsupported`, which also covers a Java class file that shares the magic.
  - Each slice is parsed as thin, with offsets relative to the slice.
  - A slice that is not thin 64-bit LE → `UnknownSlice`.
- **Bounds and tables.**
  - `sizeofcmds` ≤ `MACHO_CMDS_CAP` (4 MiB), and each `cmdsize` must be a multiple of 8, at least 8, and stay inside `sizeofcmds`.
  - The first `LC_SYMTAB` (0x2) gives the symbol table: `nsyms × 16` ≤ `MACHO_SYMTAB_CAP` (256 MiB), and `strsize` ≤ `MACHO_STRTAB_CAP` (512 MiB).
  - Each bound is a new const in `crates/core/src/caps.rs`, and hitting one is `Bound`, an error and never a skip (code.md "Bounds").
  - No `LC_SYMTAB` gives an empty map, a binary with no debug map.
- **Entries.** An entry with `n_type == 0x66` (N_OSO) takes its NUL-terminated string at `n_strx`; an `n_strx` past `strsize` → `Truncated`.
  - A string that ends in `)` and contains `(` counts as an archive entry.
  - Every other string is loose, and its file name (the text after the last `/`) goes into `loose`.
  - Strings are compared as bytes and never split on spaces.
- **Reads.** Every read goes through `fs.read_at`:
  - the header (32 bytes);
  - the load commands;
  - the symbol table;
  - the string table.

  One binary's tables are held at a time.

### Checkout GC run (`gc::checkout`)

Satisfies:
- CHECKOUTGC-1.1, 1.2, 1.4, 1.5, 1.6, 1.11 (library side), 3.7;
- CHECKOUTGC-2.1, 2.2, 2.5, 2.6, 2.8, 2.9, 2.11;
- CHECKOUTGC-4.1, 4.2, 4.3, 4.4;
- CHECKOUTGC-7.2, 7.3.

Reuse: rung 2:
- `locks::acquire(fs, target, true)` (crates/core/src/locks.rs:43);
- `target::identify_bounded` and `fingerprint::read_profile` (the fresh read, as `recheck`, apply.rs:719-800, does);
- `liveness()` (crates/gc/src/liveness.rs:77);
- `guard::check_entry` with `Expect::Entry` (guard.rs:24);
- `ledger::{Ledger, remove_counted}` (crates/core/src/ledger/account.rs:44).

The per-unit deletion order follows apply's `removal_order` rule (apply.rs:927: `.d` files first, then the rest, the fingerprint dir last, or first when the unit has no `.d`), rewritten over paths because apply's helper works on `PlannedEntry`.
Respects: ARCH-2, ARCH-3, ARCH-4, ARCH-9, ARCH-11, ARCH-18, ARCH-19
Surface: `crates/gc/src/lib.rs` gets one `pub mod checkout; pub mod debug_map;` line. No existing reader changes, and plan, apply and auto tier are untouched (CHECKOUTGC-6.1).
Interface:
```
pub struct CheckoutInput<'a> { pub fs: &'a dyn Fs, pub scan: &'a TargetScan, pub checkout: &'a Path,
    pub nested: &'a [PathBuf], pub now: SystemTime, pub window: Duration, pub account_bytes: bool }
pub fn run(input: &CheckoutInput, cancel: &dyn Fn() -> bool) -> CheckoutReport
pub struct CheckoutReport { pub per_target: Vec<(PathBuf, Counts)>, pub totals: Counts,
    pub findings: Vec<Finding>, pub unmeasured: Vec<(PathBuf, String)>, pub unmeasured_more: u64,
    pub cancelled: bool, pub failed: bool }
pub struct Counts { pub units_deleted: u64, pub unit_files_deleted: u64, pub objects_deleted: u64,
    pub objects_referenced: u64, pub binaries_read: u64, pub targets_shared: u64, pub targets_outside: u64,
    pub bytes: Option<Bytes>, pub skips: BTreeMap<&'static str, u64> }
```
Depth: if this module vanished, callers would still need to know only "run checkout gc over this scan for this checkout and get counts, findings and two flags". Hidden from them:
- target selection;
- lock lifetime;
- the fresh re-read;
- deletion order;
- fail-closed rules;
- byte accounting.

Locality: new file `crates/gc/src/checkout.rs`. Its inline unit tests use a local fake `Fs`, like gc's `liveness.rs:1448` `FakeFs`. `Recording` lives in `crates/cargo-hive/tests/common`, so the Recording-based tests are library tests there (`LibCtx`, plan_fixture.rs:227). Neighbor impact: leave `plan_tier`, `gc::run` and `liveness`.

Per run:
1. **Select the targets** (pure, before any lock).
   - **Failed resolutions.** Collect `failed_workspaces(scan)`. When any exists, emit one `ownership.unresolved` finding per dir, whose detail says how it clears. Set `failed = true` (exit 1) and touch no target (3.5).
   - **Otherwise,** look at each target in `scan.targets` whose `resolvers_of` contains the checkout:
     - **another checkout also resolves to it** → a `gc.shared_target` finding (detail: the other checkouts), counted in `targets_shared`; nothing in it is touched (3.2);
     - **it does not lie inside the checkout's directory, or lies inside a live checkout nested in it** (a main's `.worktrees/*`) → a `gc.outside_checkout` finding, counted in `targets_outside`; nothing is touched (3.7);
     - **otherwise** it is the checkout's own.
2. **For each own target, in path order.** Bind the whole `Locked` value to a local that lives until the target's last deletion. Destructuring it would drop the `HeldLock`s early (4.4).
   - **Acquire.** `locks::acquire(fs, path, true)`.
     - `Skipped(reason)` → a `gc.skipped` finding with the path and the reason text, plus `skips[reason.count_name()] += 1` (4.2).
     - `Failed(why)` → a `gc.target_failed` finding, and `failed = true` (4.3).
   - **Re-read under the locks.**
     - `identify_bounded(fs, path, CRATE_DIR_CAP)` must equal `Locked.profiles`; a mismatch counts as the `ProfileSet` skip.
     - `read_profile` runs for each profile.
     - Build a fresh `TargetDir`, with the owner taken from the scan.
   - **Units.** Run `liveness(fs, &fresh, now, window)`.
     - `Undecided` → one `gc.undecided` finding per reason: `"<reason> — clears when <clears_when>"` (1.5).
     - `Decided` → group the dead units' entries by apply's rule (apply.rs:927-960):
       - dep-info (`.d`) first, then the other hash-keyed entries, the fingerprint dir last;
       - a unit with no `.d` puts its fingerprint dir first;
       - a unit with only a fingerprint dir is skipped (ARCH-18).

       That order keeps a stopped run from leaving a unit Cargo would call fresh with a file missing (7.3).
     - Before a unit's first deletion, `entry_stat` every entry of the unit once. Each deletion is then `check_entry(fs, target, entry, Expect::Entry { kind, dev, ino })` from that stat, followed by `remove_counted(fs, ledger, target, entry, account_bytes, false)`, so an entry swapped after the stat is refused.
     - Every kind but `Other` is accepted, as `apply::delete_entry` does. That covers files, symlinks, the fingerprint dir, and a `.dSYM` dir under packed split-debuginfo (7.2).
     - A failed entry stops that unit, gives a `gc.delete_failed` finding and sets `failed = true`.
     - `cancel()` is checked before each unit.
   - **Objects**, for each profile dir:
     - **Layout check.** Skip the profile with a `gc.objects_undecided` finding (2.6, 2.9; ARCH-9) when `identify_bounded` gives a layout other than Fingerprint, or its `ProfileReport.format` is `Unsupported`.
     - **Listing.** List these directories:
       - `deps/` with `read_dir_bounded(…, DEPS_CAP)`;
       - the profile dir itself and `examples/` (DEPS_CAP);
       - `build/` and each `build/<dir>/` (CRATE_DIR_CAP), direct children only.

       A missing `examples/` or `build/` reads as empty; a missing `deps/` means nothing to do. Any other listing error gives a `gc.objects_undecided` finding (2.9). The listing also stats each `deps/*.rcgu.o` regular file once, recording `(dev, ino)`.
     - **Read.** `debug_map::read` runs on every regular file, or symlink to one, whose name does not end in `.o` (Decision 5).
       - `Ok(Some(map))` counts toward `binaries_read`.
       - A `MapError` gives a `gc.objects_undecided` finding naming the file (2.7).
     - **No debug map.** Binaries ≥ 1, loose objects ≥ 1, and zero N_OSO entries in total → a `gc.objects_undecided` finding (2.8).
     - **Delete.** Otherwise, each listed `deps/*.rcgu.o` whose name is not in the union of `loose` is deleted, with `check_entry(…, Expect::Entry { kind: File, dev, ino })` from the listing stat, then `remove_counted`. The others count `objects_referenced`. `cancel()` is checked before each deletion.
   - **Release.** Drop `Locked` after the last deletion in the target (4.4).
3. **Finish.** `ledger.finish()` gives the per-target bytes. When `account_bytes` is false, `bytes` is `None`, and the CLI marks the two byte names unmeasured (1.10).

A stopped run stored nothing but the deletions themselves, and deleted in the order above, so the next run re-derives everything from disk (7.3). A file of a unit whose fingerprint dir already went is not listed again, the same limit the plan tier has.

**Freed bytes.** A `deps/*.rcgu.o` is usually hardlinked (nlink 2) to its copy under `incremental/`, so the ledger counts most object bytes as `deleted_bytes_still_shared`, not `freed_bytes` (design review probe, cargo 1.97.1). The speed win comes from the entry count in `deps/`, not from the bytes, and the receipt says so through the two counts (ARCH-11).

### CLI mode, receipt and checkout lookup

Satisfies: CHECKOUTGC-1.3, 1.7, 1.8, 1.9, 1.10, 1.12, 3.3, 3.4, 3.6
Reuse: rung 2:
- the `gc_cmd` flow (cli.rs:914-1130): signals, state open, `doctor::require`, `discover::targets`, volume cross-check notes (`apply::cross_check_notes`), the `ignored_vars` notes (cli.rs:886), `merge_snapshot` under `locked_or_cancelled`, `emit`. Counts are written by a new sibling `put_checkout_counts(b, suffix, &checkout::Counts)`, since `put_counts` takes the auto tier's `gc::Counts`;
- `cargo_hive_state::window_days`, refused like `gc --plan` (build.rs:191);
- the first half of `locate_checkout` (cli.rs:1569-1608), extracted.

Respects: ARCH-8, ARCH-10, ARCH-14, ARCH-16, ARCH-1 (an explicit command; nothing installed)
Surface:
- `Command::Gc` gains `#[arg(long, value_name = "PATH", conflicts_with_all = ["repos", "plan", "dry_run"])] checkout: Option<PathBuf>`, and its doc comment is reworded. The readers:
  - the dispatch at cli.rs:192 — **replace**;
  - tests/skill.rs `parse()` (:476), which binds `Gc { dry_run, plan, .. }` — **replace**: `Parsed::Gc` gains `checkout: bool`, so the new line is told apart (5.4);
  - tests/cli_tree.rs and its fixture — **replace** (Decision 8).
- `locate_checkout` becomes `find_checkout(reg, cfg, path) -> Result<(Repo, Checkout), Refusal>`, which holds its first half, followed by its own protected refusal, toolchain and capabilities. Its one caller, seed (`seed_cmd`, cli.rs:1672), is **replace**: seed keeps the same refusals, guarded by the existing seed tests.
- The new receipt string `"gc --checkout"`: no reader matches it, since `emit_failure`'s schema-3 branch (cli.rs:296) matches only `gc --plan` and `apply`. `tests/receipts.rs` and `tests/upgrade.rs` read receipts by field, not command — **replace**, no edit.

Interface: `fn checkout_cmd(state: &State, path: &Path, out: &OutputArgs) -> i32`.
Depth: n/a — extends the `gc` command's flow.
Locality: extend `crates/cargo-hive/src/cli.rs`. Neighbor impact: leave `gc_cmd`, `plan_cmd`, `apply_cmd`, `select`, `owning_repo` and `live_checkouts`, none of which this mode calls.

Flow:
1. **Window.** `window_days(cfg.gc_window_days)`; an `Err` is a `refusal`, exit 1 (1.3).
2. **Capabilities.** `doctor::require` for FingerprintReading and Deletion; a failure is a `refusal`, exit 1 (1.9).
3. **Find the checkout.** `find_checkout(path)`; an `Err` is a `refusal` naming the path, exit 1. The detail depends on the path:
   - inside a registered repo but not a checkout root (`owning_repo` used only to word it): `"<path> is not a checkout of <main>; name the checkout's root directory"`;
   - otherwise: `find_checkout` keeps `locate_checkout`'s existing text (cli.rs:1612-1618), which names `cargo hive repo add` (3.3).

   A main is accepted (3.4).
4. **Receipt scope.** `Receipt::new("gc --checkout", Scope { repos: [repo.main], checkouts: [checkout.path] })` (1.7).
5. **Discover.** `discover::checkouts(repo)`, then `discover::targets(live checkouts of that repo only, cfg, false, snapshot, cancel)`. The live checkouts nested inside the named one are passed as `nested` (3.7). A cancel gives the `error` finding and exit 130 before any deletion. Walk faults become `walk.unreadable` findings.
6. **Run.** `gc::checkout::run` brackets with the volume cross-check, as gc does. Then:
   - `put_checkout_counts` for totals and `:<target>` keys;
   - the five `targets_skipped_*` names, plus `targets_shared` and `targets_outside`;
   - an `ignored_vars` note for each ignored target variable, as gc_cmd does;
   - `freed_bytes` and `deleted_bytes_still_shared` marked unmeasured when EvictionAccounting is disabled (1.10);
   - findings copied in;
   - `snapshot.json` merged under the state lock (3.6).
7. **Exit** (1.11, 1.12):
   - `cancelled` → the `error` finding, then `emit(…, 130)`; `emit` records the receipt unless the cancel lands during the append;
   - `failed` → `emit(…, 1)`;
   - otherwise `emit(…, 0)`.

   `emit` already returns 1 for a receipt it cannot record.

### Skill and README

Satisfies: CHECKOUTGC-5.1, CHECKOUTGC-5.2, CHECKOUTGC-5.3, CHECKOUTGC-5.4
Reuse: rung 2 — the SKILLPASS guide structure (`plugin/skills/hive/SKILL.md`, its "Free disk" section and Reading table) and the README's per-command sections. It is checked by the existing `tests/skill.rs` checks (:539, :657, :778, :899, :1125, :1131, :949).
Surface:
- `plugin/skills/hive/SKILL.md`: a new short section, "Keep a reused worktree fast", plus a Reading-table row and a line under "Free disk" restating that `gc <path>` covers the whole repo. The readers:
  - agents (the shipped plugin) — **replace**;
  - `skill.rs` checks — **replace** (5.4).

  It holds within the 150-line budget (118 used).
- `skills/hive/eval.json` and `TESTS.md`: new entries for the trigger and the receipt — **replace**.
- `README.md`: a `gc --checkout` subsection — **replace**. `readme_command_lines_and_names_match_the_binary` (skill.rs:1131) checks it.

Interface: the skill text tells an agent working in a reused worktree what to do:
- **When to count.** Before a test build, count the entries with `ls -f <the checkout's real target dir>/debug/deps | wc -l`, or the `deps/` of its profile dir.
- **When to run.** When the count is above 20,000, run `cargo hive gc --checkout <worktree> --json`.
- **How to read the receipt.** `objects_deleted`, `units_deleted`, `freed_bytes` and `targets_skipped_held`, and the `gc.skipped`, `gc.shared_target`, `gc.outside_checkout`, `ownership.unresolved` and `gc.objects_undecided` findings, each with what to do. For `gc.outside_checkout`, the agent learns that nothing was deleted in that target, and that repo-wide `gc --plan` then `apply` can still collect its dead units, though not its debug objects.

Depth: n/a — extends the shipped skill.
Locality: extend `plugin/skills/hive/SKILL.md`, `skills/hive/{eval.json,TESTS.md}` and `README.md`. The text goes through the `author-skills` workflow, with Sonnet and Haiku evals, at build time. Neighbor impact: leave the other skill sections.

### Existing behavior guards

Satisfies: CHECKOUTGC-6.1, 6.2, 6.3, 6.4, 6.5, 6.6, 6.7
Reuse: rung 2 — the existing tests that pin each guard:
- `tests/gc.rs`, `plan_apply.rs` and `plan_tier.rs` (6.1);
- the GCPLAN-9.2 tests (6.2);
- `liveness*.rs` and `seed.rs:1520` (6.3);
- `locks.rs` unit tests (6.4);
- `seed` record tests (record.rs:813-817, 906; 6.5);
- `cli_tree.rs` (6.6);
- the Linux `cargo check` in the merge gate (6.7).

Interface: none new. The guards hold because every edit above is additive or a visibility-preserving extraction.
Depth: n/a — extends the existing suites.
Locality: tests only. Neighbor impact: leave.

## Seams for testing

| Seam | Kind | Covers |
|---|---|---|
| `gc::debug_map::read` over hand-built byte images (thin, fat, fat64, 32-bit, truncated at each table, every bound at exact and one past, archive and loose entries, a path with spaces), via a fake `Fs` that serves bytes | unit | CHECKOUTGC-2.3, 2.4, 2.7 |
| `gc::checkout::run` through the library from `crates/cargo-hive/tests` with `Recording` (fail_remove, symlink_after_stat, replace_after_lock, fail_read_at, a stop after each position in a unit), plus inline unit tests with a local fake `Fs` | library / unit | CHECKOUTGC-1.1, 1.4, 1.5, 1.6, 2.1, 2.2, 2.5, 2.6, 2.8, 2.9, 4.1, 4.3, 4.4, 7.2, 7.3 |
| `discover::{resolvers_of, failed_workspaces}` over a `TargetScan` from two checkouts sharing a target via `.cargo/config.toml` `build.target-dir` (hive scrubs `CARGO_TARGET_DIR`, env.rs:7-11), a main with a worktree nested under it, and one checkout with a broken manifest | library | CHECKOUTGC-3.1, 3.2, 3.5, 3.7 |
| `Fs::read_at` on `Apfs` (offsets, EOF, past EOF) | unit (platform) | CHECKOUTGC-2.10, 7.1 (only tables read) |
| `cargo hive gc --checkout` through the binary (`Hive::ready`, `RegistryRepo`, real `cargo test --no-run` builds, `hold_lock_child`, `backdate_all`) | integration | CHECKOUTGC-1.2, 1.3, 1.7, 1.8, 1.9, 1.10, 1.11, 1.12, 2.11, 3.3, 3.4, 3.6, 4.2 |
| Rebuild proof: after `gc --checkout`, rerun the fixture's cargo commands and assert no unit compiled inside the window recompiles (the cargo JSON `fresh` flags) | integration | CHECKOUTGC-1.4 |
| `tests/skill.rs` (+ eval.json / TESTS.md) | integration | CHECKOUTGC-5.1, 5.2, 5.3, 5.4 |
| Existing suites listed under "Existing behavior guards", `tests/cli_tree.rs`, Linux `cargo check` | integration | CHECKOUTGC-6.1–6.7 |
| klynt proof on a copy of an aged slot (scratch `HIVE_HOME`, `/private/tmp`), timed outside hive, with a cold and a warm discovery snapshot. `cargo metadata` calls are part of the 15 s, and the snapshot caches them (discover.rs:265) | manual proof | CHECKOUTGC-7.1; Open Questions (Example 1 figures, incremental share) |

New seams: one, `gc::checkout::run`. The `debug_map` and `read_at` seams are the new modules' own public surface.

## Coverage check

Every ID in requirements.md appears in exactly one `Satisfies:` line:

| Section | IDs |
|---|---|
| Ownership resolutions in discovery | 3.1, 3.2, 3.5 |
| Ranged read | 2.10, 7.1 |
| Debug-map reader | 2.3, 2.4, 2.7 |
| Checkout GC run | 1.1, 1.2, 1.4, 1.5, 1.6, 1.11, 2.1, 2.2, 2.5, 2.6, 2.8, 2.9, 2.11, 3.7, 4.1, 4.2, 4.3, 4.4, 7.2, 7.3 |
| CLI mode | 1.3, 1.7, 1.8, 1.9, 1.10, 1.12, 3.3, 3.4, 3.6 |
| Skill and README | 5.1, 5.2, 5.3, 5.4 |
| Existing behavior guards | 6.1–6.7 |

1.11 (cancel) appears under the run (library side) and is completed by the CLI's exit handling; its primary line is the run. Accessibility is `None` in the requirements and is not mapped.
