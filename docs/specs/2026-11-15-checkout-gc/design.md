# Design: Checkout GC

Feature code: CHECKOUTGC
Status: Draft
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
3. **Ownership from resolutions:** `discover::targets` gains a `workspaces` field carrying every per-workspace resolution result. A target belongs to the checkout when the checkout's own resolutions name it and no other live checkout's do. Any failed resolution in the repo blocks every deletion (CHECKOUTGC-3.5, agent-decided).
4. **The reader is pure and lives in `crates/gc`** (`gc::debug_map`), over bytes from a new `Fs::read_at`. No new crate, since a 9th member would add a publish, and no dependency.
5. **Linked binaries are found by header, not by name, after a deny-list of names that are never linked:** `*.rcgu.o`, `*.o`, `*.d`, `*.rlib`, `*.rmeta`, `*.a`, `*.json`, `*.timestamp`. Every other regular file in the scanned directories gets an 8-byte header read. The deny-list only removes names Cargo uses for non-linked outputs, so a renamed binary is still read.
6. **Object matching is by file name** across every loose N_OSO entry of the profile dir's binaries. `<archive>(<member>)` entries are ignored (CHECKOUTGC-2.3, 2.4).
7. **Fail closed per profile dir** for the object class (2.7–2.9), independent of liveness's verdict (2.6).
8. **The CLI tree fixture:**
   - `tests/fixtures/v0.1.0/cli-tree.txt` stays as written.
   - A new `tests/fixtures/current/cli-tree.txt` holds today's tree.
   - `cli_tree.rs` asserts the current tree equals it, and that its diff from v0.1.0 lies only inside the `gc` block (CHECKOUTGC-6.6, agent-decided).
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
- `TargetScan` gets a new `pub workspaces: Vec<WorkspaceResolution>` field, constructed only at discover.rs:652. Every caller reads fields by name, and none destructures the struct, so all 8 callers are **replace** (no edit needed; the existing fields keep their values). These are cli.rs `gc_cmd`, `status`, `explain`, `plan`, `seed`, `bench`, plus `plan_tier::build::make` and tests.
- The existing `owner` and `failure` computations: **replace**, unchanged. CHECKOUTGC-6.1 guards them.
Interface: `pub struct WorkspaceResolution { pub dir: PathBuf, pub checkout: PathBuf, pub result: Result<Resolved, String> }`. Beside it, `pub fn resolvers_of(scan: &TargetScan, target: &Path) -> Vec<PathBuf>` gives the distinct checkouts whose canonicalized `target_directory` or `build_directory` equals the canonicalized `target`. `pub fn failed_workspaces(scan: &TargetScan) -> Vec<(PathBuf, String)>` returns every failure, not only the first.
Depth: n/a — extends `discover::targets`.
Locality: extend `discover.rs`. Neighbor impact: leave `protected_owner`, `owner_checkout` and `Scope`.

`checkout` is the deepest live checkout containing `dir` (the same rule as `owner_checkout`, :509). The comparison canonicalizes both sides, because `Resolved` paths are raw (:604-607). A cancel can truncate `resolve_workspaces`, so callers check `cancel()` right after `targets()` and trust nothing when it fired.

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

  Delegating wrappers keep real behavior because the default calls their own delegating `read`. `Apfs` overrides with a positioned read. `Recording` (tests/common/recording_fs.rs:234) also overrides it, to log the call and offer a `fail_read_at(path)` hook.
Interface: `read_at(path, offset, len)` returns up to `len` bytes from `offset`, fewer at end of file, an empty vector past it. It follows no symlink: it opens with `O_NOFOLLOW`, as `try_lock_exclusive` does (lib.rs:319-321).
Depth: n/a — extends `Fs`.
Locality: extend `crates/platform/src/lib.rs` (the trait) and `apfs.rs`. Neighbor impact: leave other methods.

### Debug-map reader (`gc::debug_map`)

Satisfies: CHECKOUTGC-2.3, CHECKOUTGC-2.4, CHECKOUTGC-2.7 (parse failures and bounds)
Reuse: none — new code (rung 7). There is no Mach-O reader in the workspace or `Cargo.lock`, a dependency needs the owner (docs/standards/code.md:16), and `nm` is a process (ARCH-17).
Respects: ARCH-15 (pure bytes; the read is the caller's `Fs`), ARCH-17
Interface:
```
pub enum Header { NotMachO, Object, Dsym, Linked }            // from the first 8 bytes
pub fn sniff(first8: &[u8]) -> Header
pub struct DebugMap { pub loose: BTreeSet<String>, pub archive_entries: u64 }
pub fn read(fs: &dyn Fs, path: &Path) -> Result<DebugMap, MapError>
pub enum MapError { Read(io::Error), Truncated(&'static str), Bound(&'static str, u64), UnknownSlice(u32), Unsupported(&'static str) }
```
Depth: if this module vanished, callers would still need to know only "give me the set of loose object file names this binary's debug map names, or why not". Everything else is hidden: header and fat-header layout, load-command walking, `nlist_64` decoding, string-table bounds and archive-entry classification.
Locality: new file `crates/gc/src/debug_map.rs` with inline unit tests on hand-built byte images. Neighbor impact: leave.

Rules, with each layout fact cited in the research note `.skills/research/2026-11-15-macho-n-oso.md` (main checkout; V = verified against Apple's `loader.h`, `nlist.h`, `stab.h`, `fat.h` and LLVM `MachO.h`):
- **Thin 64-bit little-endian** (`0xfeedfacf`) is parsed.
  - `filetype` `MH_OBJECT` → `Object`, `MH_DSYM` → `Dsym`.
  - Every other file type → `Linked`. That covers executable, dylib and bundle, and keeps more for the rare kinds.
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
- CHECKOUTGC-1.1, 1.2, 1.4, 1.5, 1.6, 1.11 (library side);
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
pub struct CheckoutInput<'a> { fs: &'a dyn Fs, scan: &'a TargetScan, checkout: &'a Path,
    now: SystemTime, window: Duration, account_bytes: bool }
pub fn run(input: &CheckoutInput, cancel: &dyn Fn() -> bool) -> CheckoutReport
pub struct CheckoutReport { per_target: Vec<(PathBuf, Counts)>, totals: Counts,
    findings: Vec<Finding>, unmeasured: Vec<(PathBuf, String)>, unmeasured_more: u64,
    cancelled: bool, failed: bool }
pub struct Counts { units_deleted, unit_files_deleted, objects_deleted, objects_referenced,
    binaries_read: u64, bytes: Option<Bytes>, skips: BTreeMap<&'static str, u64> }
```
Depth: if this module vanished, callers would still need to know only "run checkout gc over this scan for this checkout and get counts, findings and two flags". Hidden from them:
- target selection;
- lock lifetime;
- the fresh re-read;
- deletion order;
- fail-closed rules;
- byte accounting.

Locality: new file `crates/gc/src/checkout.rs` with unit tests on a `Recording`-style fake. Neighbor impact: leave `plan_tier`, `gc::run` and `liveness`.

Per run:
1. **Select the targets.** Collect `failed_workspaces(scan)`. When any exists: one `ownership.unresolved` finding per dir, whose detail names how it clears (CHECKOUTGC-3.5), and no target is touched. Otherwise, for each target in `scan.targets` whose `resolvers_of` contains the checkout:
   - when it also holds another checkout → a `gc.shared_target` finding (detail: the other checkouts), counted `targets_shared`, and nothing in it is touched;
   - otherwise it is the checkout's own.
2. **For each own target, in path order.**
   - **Acquire.** `locks::acquire(fs, path, true)`.
     - `Skipped(reason)` → a `gc.skipped` finding with the path and the reason text, plus `skips[reason.count_name()] += 1` (4.2).
     - `Failed(why)` → a `gc.target_failed` finding, and `failed = true` (4.3).
   - **Re-read under the locks.**
     - `identify_bounded(fs, path, CRATE_DIR_CAP)` must equal `Locked.profiles`; a mismatch counts as the `ProfileSet` skip.
     - `read_profile` runs for each profile.
     - Build a fresh `TargetDir`, with the owner taken from the scan.
   - **Units.** Run `liveness(fs, &fresh, now, window)`.
     - `Undecided` → one `gc.undecided` finding per reason: `"<reason> — clears when <clears_when>"` (1.5).
     - `Decided` → for each dead unit, in unit order, stat each entry. Then `check_entry` with `Expect::Entry { kind, dev, ino }` right before `remove_counted(fs, ledger, path, entry, account_bytes, false)`.
     - Only regular files, symlinks inside `deps/` or `examples/`, and the unit's fingerprint dir are accepted (7.2). `check_entry` refuses anything outside the target or of kind `Other`.
     - A failed entry stops that unit, gives a `gc.delete_failed` finding and sets `failed = true`.
     - `cancel()` is checked before each unit.
   - **Objects.** For each profile dir in the Fingerprint layout (layout per `identify_bounded`; any other layout gives a `gc.objects_undecided` finding, 2.9), list these directories:
     - `deps/` with `read_dir_bounded(…, DEPS_CAP)`;
     - the profile dir itself, `examples/` (DEPS_CAP) and `build/` with each `build/<dir>/` (CRATE_DIR_CAP).

     A listing error gives a `gc.objects_undecided` finding (2.9). Then:
     - **Sniff.** Read 8 bytes of every regular file whose name is not on the deny-list (Decision 5) and sniff it. `Linked` files get `debug_map::read`. A `MapError` gives a `gc.objects_undecided` finding naming the file (2.7).
     - **No debug map.** Binaries ≥ 1, loose objects ≥ 1, and zero N_OSO entries in total → a `gc.objects_undecided` finding (2.8).
     - **Delete.** Otherwise, each `deps/*.rcgu.o` regular file whose name is not in the union of `loose` is deleted with `check_entry` plus `remove_counted`. The others count `objects_referenced`. `cancel()` is checked before each deletion.
   - **Release.** Drop `Locked` after the last deletion in the target (4.4).
3. **Finish.** `ledger.finish()` gives the per-target bytes. When `account_bytes` is false, `bytes` is `None`, and the CLI marks the two byte names unmeasured (1.10).

A run killed part-way has stored nothing but the deletions themselves, so the next run re-derives everything (7.3).

### CLI mode, receipt and checkout lookup

Satisfies: CHECKOUTGC-1.3, 1.7, 1.8, 1.9, 1.10, 1.12, 3.3, 3.4, 3.6
Reuse: rung 2:
- the `gc_cmd` flow (cli.rs:914-1130): signals, state open, `doctor::require`, `discover::targets`, volume cross-check notes (`apply::cross_check_notes`), `put_counts`, `merge_snapshot` under `locked_or_cancelled`, `emit`;
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
   - otherwise: `"not a registered repository; run cargo hive repo add first"` (3.3).

   A main is accepted (3.4).
4. **Receipt scope.** `Receipt::new("gc --checkout", Scope { repos: [repo.main], checkouts: [checkout.path] })` (1.7).
5. **Discover.** `discover::checkouts(repo)`, then `discover::targets(live checkouts of that repo only, cfg, false, snapshot, cancel)`. A cancel gives the `error` finding and exit 130 before any deletion. Walk faults become `walk.unreadable` findings.
6. **Run.** `gc::checkout::run` brackets with the volume cross-check, as gc does. Then:
   - `put_counts` for totals and `:<target>` keys;
   - the five `targets_skipped_*` names, plus `targets_shared`;
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
- **When to count.** Before a test build, count the entries with `ls <target>/debug/deps | wc -l`, or the `deps/` of its profile dir.
- **When to run.** When the count is above 20,000, run `cargo hive gc --checkout <worktree> --json`.
- **How to read the receipt.** `objects_deleted`, `units_deleted`, `freed_bytes` and `targets_skipped_held`, and the `gc.skipped`, `gc.shared_target`, `ownership.unresolved` and `gc.objects_undecided` findings, each with what to do.

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
| `gc::debug_map::{sniff, read}` over hand-built byte images, via a fake `Fs` that serves bytes | unit | CHECKOUTGC-2.3, 2.4, 2.7 (each bound at exact and one past) |
| `gc::checkout::run` over a scratch target with `Recording` (fail_remove, symlink_after_stat, replace_after_lock, fail_read_at) | unit / library | CHECKOUTGC-1.1, 1.4, 1.5, 1.6, 2.1, 2.2, 2.5, 2.6, 2.8, 2.9, 4.1, 4.3, 4.4, 7.2, 7.3 |
| `discover::{resolvers_of, failed_workspaces}` over a `TargetScan` from two checkouts sharing a `CARGO_TARGET_DIR`, one with a broken manifest | library | CHECKOUTGC-3.1, 3.2, 3.5 |
| `Fs::read_at` on `Apfs` (offsets, EOF, symlink refused) | unit (platform) | CHECKOUTGC-2.10, 7.1 (only tables read) |
| `cargo hive gc --checkout` through the binary (`Hive::ready`, `RegistryRepo`, real `cargo test --no-run` builds, `hold_lock_child`, `backdate_all`) | integration | CHECKOUTGC-1.2, 1.3, 1.7, 1.8, 1.9, 1.10, 1.11, 1.12, 2.11, 3.3, 3.4, 3.6, 4.2 |
| Rebuild proof: after `gc --checkout`, rerun the fixture's cargo commands and assert no unit compiled inside the window recompiles (the cargo JSON `fresh` flags) | integration | CHECKOUTGC-1.4 |
| `tests/skill.rs` (+ eval.json / TESTS.md) | integration | CHECKOUTGC-5.1, 5.2, 5.3, 5.4 |
| Existing suites listed under "Existing behavior guards", `tests/cli_tree.rs`, Linux `cargo check` | integration | CHECKOUTGC-6.1–6.7 |
| klynt proof on a copy of an aged slot (scratch `HIVE_HOME`, `/private/tmp`), timed outside hive | manual proof | CHECKOUTGC-7.1; Open Questions (Example 1 figures, incremental share) |

New seams: one, `gc::checkout::run`. The `debug_map` and `read_at` seams are the new modules' own public surface.

## Coverage check

Every ID in requirements.md appears in exactly one `Satisfies:` line:

| Section | IDs |
|---|---|
| Ownership resolutions in discovery | 3.1, 3.2, 3.5 |
| Ranged read | 2.10, 7.1 |
| Debug-map reader | 2.3, 2.4, 2.7 |
| Checkout GC run | 1.1, 1.2, 1.4, 1.5, 1.6, 1.11, 2.1, 2.2, 2.5, 2.6, 2.8, 2.9, 2.11, 4.1, 4.2, 4.3, 4.4, 7.2, 7.3 |
| CLI mode | 1.3, 1.7, 1.8, 1.9, 1.10, 1.12, 3.3, 3.4, 3.6 |
| Skill and README | 5.1, 5.2, 5.3, 5.4 |
| Existing behavior guards | 6.1–6.7 |

1.11 (cancel) appears under the run (library side) and is completed by the CLI's exit handling; its primary line is the run. Accessibility is `None` in the requirements and is not mapped.
