# Design: Auto-tier GC (superseded incremental sessions and rustc temp entries)

Feature code: AUTOGC
Status: Approved
Date: 2026-10-28
Requirements: ./requirements.md

## Context

CORE (on `main`, 84c74e4 after its realign) already does three things:
- It finds every Cargo target dir of the registered repos. For each one it reports an owner (`Owned` / `OrphanCandidate` / `Unresolved`), whether the dir is readable, and every profile dir's fingerprint format (`src/discover.rs:478`, `src/cargo_target.rs:355`).
- It gates capabilities on doctor results for the active toolchain (`src/doctor.rs:254`). `Capability::Deletion` exists, disabled by check (d) or check (f), but no command requires it yet.
- Its platform layer can clone, read sharing attributes and hold a lock in a child process (`__hold-lock`, `src/platform/apfs.rs:161, 261`). It cannot hold a lock in hive's own process, cannot take an fcntl read lock, and cannot read a link count.

Nothing in hive deletes user files today (`.skills/AUTOGC/design-scan.md`).

The binding constraint is that the owners of these files keep using them while `gc` runs. rustc reads, copies from and finalizes sessions in other processes, and Cargo stages temp entries during a build. A deletion is safe only if all of these hold:
- the owner's lock is tried without waiting (ARCH-19);
- it is held in hive's process from before the first unlink until after the last;
- the state that justified the deletion is **re-checked under that lock**.

The last point is the lesson of the design review (`.skills/AUTOGC/design-review.md` #1):
- rustc releases a session's lock at finalization, right after renaming `-working` to its final name (`rustc_session/src/session.rs:467`, `persist/fs.rs:~355-362`).
- The lock file's name is shared by both names.
- So a plan made from a listing can be stale by the time its lock is granted.

This rules out the probe CORE's design sketched, "probe the way production will: `flock(LOCK_SH|LOCK_NB)`" (CORE `design.md:66-69`). A probe that releases before the delete is a TOCTOU race. A shared probe is also not refused by an fcntl read holder, which the plan review confirmed on macOS.

Spike 16 (`SPIKES.md` §16) chose the primitive. On macOS, an exclusive non-blocking `flock` on a read-only descriptor:
- is refused by every holder kind rustc and Cargo use;
- itself blocks rustc's fcntl attempts;
- survives a second descriptor on the same file.

The session rules mirror rustc 1.97.1's own GC (`compiler/rustc_incremental/src/persist/fs.rs` at 8bab26f4f, 611–881). The difference is that hive runs them from outside, for crates that are not being compiled.

The second constraint is honesty of the byte count (ARCH-11). rustc hard-links session files into `deps/` and into the next session. On klynt today, the superseded sessions hold 8.90 GB raw, but only 7.63 GB is freed by deleting them. So freed bytes are counted per inode, against the whole set of paths the run deletes.

## Decisions

1. **One lock primitive.**
   - The lock file is opened with `O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC`, never created. `fstat` must show a regular file.
   - The lock is `flock(LOCK_EX | LOCK_NB)`, held by a guard in hive's process.
   - Sessions use their `s-*.lock` file. Temp entries use `<build dir>/<profile>/.cargo-build-lock`, which Cargo takes exclusively for the whole build. Under `-Zfine-grain-locking` it is taken shared, which `LOCK_EX` still refuses (`src/cargo/core/compiler/layout.rs` at c980f4866, 247–281).
   - Errors are kept apart:
     - `EWOULDBLOCK` is *held*.
     - A missing file is *gone*.
     - A failed open or a non-regular file is *unopenable*.
     - Any other `flock` error (`ENOTSUP`, `ENOLCK`) is *unlockable*.
   - Changing the primitive later means editing one platform function, so it does not clear the ADR gate.
2. **Re-check under the lock, then delete dir before lock file.** For `DeleteLocked{dir, lock}`:
   1. take the guard on `lock`;
   2. `symlink_metadata(dir)`;
   3. if `dir` is gone, stop: the session finalized or was removed meanwhile. The lock file now belongs to whatever replaced the dir, so it is **left untouched** and nothing is counted (AUTOGC-2.14);
   4. otherwise scan for the ledger, `remove_tree(dir)`, then `remove_file(lock)`, then drop the guard.

   While hive holds the guard no rustc can hold that lock, and only the lock's holder renames the session, so the session cannot change name between steps 2 and 4.

   For `DeleteOrphanLock{lock}`: take the guard, re-list the crate dir, and remove the lock file only if no entry shares its three-field key.

   SIGKILL at any step leaves one of three states: nothing changed; a partly removed *older* session with its lock file; or an orphan lock file. rustc chooses its copy source as the newest finalized dir by name timestamp, with no lock-file test (`fs.rs:516-555`). Its own GC always keeps the newest candidate (`fs.rs:719-760, 867-881`). So the kept session `K` survives every interruption, and a partly removed older session is never promoted while `K` exists. Only something other than rustc or hive removing `K` (`cargo clean` removes everything) could promote it.

   The dir goes before the lock file, so hive never manufactures a lockless finalized session.
3. **Lockless dirs are re-checked, not trusted.** A single `read_dir` can miss a lock file that rustc created before the dir, because listing order is not creation order. Immediately before deleting a `DeleteLockless{dir}`, the runner `symlink_metadata`s the matching lock path. If it now exists, the dir is left alone this run. This needs no requirement change: AUTOGC-2.7 still deletes a dir that has no lock file.
4. **Pure planner, effectful runner, effects through `Fs`.**
   - Deciding what to delete in a crate dir is a pure function of the listing and `now`.
   - The runner performs every effect through `Fs`: lock, stat, list, `remove_tree`, `remove_file` and `volume_free`. A recording fake can then pin the order "lock → re-check → scan → dir → lock file → drop", inject `NotFound`, and stop mid-run.
   - `Apfs::remove_tree` is `std::fs::remove_dir_all`. It never follows a symlink, and it unlinks a symlink root (std at 8bab26f4f, `library/std/src/sys/fs/unix.rs:2526-2577, 2614-2630, 2679-2689`).
5. **No-follow everywhere.**
   - Listings use `DirEntry::file_type()` and `symlink_metadata`.
   - A temp entry must be a real directory.
   - `sharing` gains `FSOPT_NOFOLLOW`, which affects only the last component. `FSOPT_NOFOLLOW_ANY` is deliberately not used, because doctor's temp dir lives under the `/var` symlink.
6. **Scope "inside a checkout" is a realpath prefix test, in the runner's input.** Under arguments, a target is in scope if either:
   - `owner` is `Owned{checkout}` with `checkout` among the selected checkouts; or
   - its path starts, component-wise, with a selected checkout's realpath.

   `Checkout.path` is already a realpath (`discover.rs:102-110`). Without arguments, every target that is not unreadable is in scope. `TargetDir` gains no field, so `status` cannot change.
7. **Only APFS targets.** The lock proof (check (d), spike 16) covers local APFS, and cross-type lock conflict is a macOS/APFS behaviour. A target whose `statfs.f_fstypename` is not `apfs` is skipped with a finding. This needs **new requirement AUTOGC-4.5**.
8. **Per-inode ledger.**
   - Under each entry's guard, before removal, the runner scans what it is about to remove. It keeps regular files only, with no-follow, at a depth of at most `size::MAX_DEPTH` (64). Each file contributes `(dev, ino, nlink, blocks, compressed, sharing)`.
   - Per `(dev, ino)` the ledger keeps the **`nlink` seen at first sight** and the number of links seen in deleted paths.
   - At the end, an inode is freed iff `seen ≥ first_nlink`. A dry run then agrees with a real run: in a real run `nlink` drops between two sessions, in a dry run it does not.
   - A freed inode counts its private size if it has a sharing flag and is not compressed, and `st_blocks × 512` otherwise.
   - Per-target byte counts attribute each inode to the target holding its last-seen link, so per-target values sum to the totals (AUTOGC-5.3).
   - Concurrent rustc activity can only lower `nlink`, so the ledger under-counts and never over-counts.
   - Hitting the depth bound leaves that entry undeleted, with a `gc.skipped` finding.
   - Any stat or `sharing` error on a file goes to `unmeasured`. The list is capped at 100 paths plus a count of the rest.
9. **Byte accounting is gated on check (c).** The private-size branch is a "private-byte receipt", which CORE-1.3 ties to check (c) through `Capability::EvictionAccounting` (`doctor.rs:61, 78-79`). ARCH-14 therefore applies. When (c) has not passed, flags cannot be trusted for any file. What `gc` does then is **owner decision O1**.
10. **Deletion requires the new lock proof.** `Capabilities` gains `lock_check: u32` with `#[serde(default)]`, set to 1 by the new check (d). For Deletion, `require` treats `lock_check < 1` as "doctor results are missing" under CORE-1.10 (`RunDoctor`), so AUTOGC-2.15 needs no new refusal kind. Without this, results from the old shared-probe check would enable Deletion (review #2).
11. **The receipt reuses `ReceiptBuilder` unchanged.**
    - Count names follow AUTOGC-5.3, as `<name>` and `<name>:<target>`.
    - Skips are `gc.skipped` findings with the path and reason.
    - Cross-checks are `note`s, because `Measured` has no signed type.
12. **Containers for the cross-check.**
    - The key is the leading `disk<N>` of `statfs.f_mntfromname`, after any `…@` prefix: regex `^(?:.*@)?/dev/disk([0-9]+)`. On this Mac `/` is `/dev/disk3s1s1` and Data is `/dev/disk3s5`; both give `disk3` (review #14).
    - A name that does not match is keyed by its mount point.
    - Available bytes are `f_bavail × f_bsize`.
    - The note says a volume quota or reserve can cap `f_bavail`.
13. **Bounds.** These four numbers are set by this design, not owner locks; boundary tests pin each one. They stay until a boundary test or klynt shows a need to change them.
    - `__try-lock` and `__hold-lock` children have a 5 s deadline, then are killed and reaped; `hold_lock` gets the same treatment.
    - Crate-dir listings are capped at 100,000 entries and `deps/` listings at 1,000,000. Hitting a cap skips that profile with a finding.
    - Ledger scans use `MAX_DEPTH`.
    - `remove_tree` itself is not bounded, and the design records that as an exception to the bounds rule: it only ever runs on an entry that the ledger scan just walked within those bounds, under its lock.
14. **Cancellation mirrors `status`, and is not a requirement.**
    - Handlers (`doctor::install_signal_handlers`) are installed **before the first walk**.
    - `doctor::CANCEL` is checked after `discover::targets` and between entries, never inside a locked deletion.
    - On cancel, `gc` emits the receipt of what it did with a finding `error` "gc was cancelled", skips the snapshot merge, and exits 130, as `status` does (`src/cli.rs:427-502`).
    - A second Ctrl-C does not interrupt a running `remove_tree`.
15. **Doctor check (d) measures the production primitive.**
    - The four old keys are replaced by the AUTOGC-1.1 and AUTOGC-1.2 matrix.
    - `Fs::probe_lock` is removed.
    - `HeldLock::new(release)` follows the `LockHolder::new(closure)` precedent (`platform/mod.rs:124-148`), so the doctor fake can model "held" without a descriptor.
16. **CORE docs are realigned through `realign-spec`, not edited in place.** CORE `design.md:66-69` and `:178` and requirement CORE-1.1(d) describe the old probe. They are corrected by a `realign-spec` pass on CORE, run in this feature's branch. The README's "reads only" lines (5, 39) are edited with `gc`.

### Owner decisions (decided 2026-10-28)

- **O1, check (c) not passed: degrade** (AUTOGC-5.6).
  - `gc` still deletes, since safety comes from locks.
  - `freed_bytes` and `deleted_bytes_still_shared` go to `unmeasured` with "doctor check (c) did not pass". They never fall back to `st_blocks`.
  - Check (a) is not added to the gate, because CORE-1.3 maps it only to seeding. Whether it should also gate private-byte receipts goes to a later `realign-spec` pass on CORE.
- **O2, why an owner is unresolved: nothing extra.** `status <repo>` already gives the reason (`discover.rs:566`).
- **O3, scope: from the cache, as `status`.** The cache is stamped by the mtimes of `Cargo.toml`, `Cargo.lock`, `rust-toolchain` and every `.cargo/config`, so changing a build-dir invalidates it. F9 binds ROAD-6 only.
- **O4, splitting still-shared bytes by cause: not in v1.** The clone-pair under-count stays a known limitation.
- **O5, the disk-image seam: allowed.** `docs/standards/testing.md` gains the exception. The image lives under `/private/tmp/hive-it-*`, is attached with `-nobrowse` and `-mountpoint` inside that dir (never under `/Volumes`), contains `.metadata_never_index`, is detached on drop, and stale images are swept at test start.

Still open, out of this design:
- Whether `status` should list every checkout that resolves to a target.
- The newer build-dir layout, which stays report-only (AUTOGC-4.3). The review found that `incremental/` and `.cargo-build-lock` stay at `build_dest` in that layout too (`layout.rs:262-279, 326`), but the layout is unstable.

## Architecture

Vocabulary for every section below: **module**, **interface**, **implementation**,
**seam** (public surface where behavior is observable and substitutable). Prefer
deep modules — an interface much simpler than what it hides.

```
cli::gc ─► install_signal_handlers ─► doctor::require(FingerprintReading, Deletion[lock_check ≥ 1])
   ├─► discover::checkouts, discover::targets          (status path; snapshot merged after)
   ├─► Fs::volume_free per container (before)
   ├─► gc::run(&dyn Fs, targets, Scope, RunOpts, cancel)
   │      ├─ scope + APFS + readable/supported filter ─► skips
   │      ├─ plan::crate_dir (pure)  ─► Action list
   │      ├─ Fs::try_lock_exclusive ─► re-check under guard ─► ledger scan ─► Fs::remove_tree / remove_file
   │      └─ ledger::finish ─► Counts per target and total
   └─► Fs::volume_free (after) ─► cross-check notes ─► receipt ─► merge_snapshot ─► emit
doctor::check (d) ─► Fs::hold_lock(Flock|Fcntl|FcntlRead) ─► Fs::try_lock_exclusive
                 └─► guard held ─► Fs::try_lock_elsewhere(Fcntl|FcntlRead)
```

### platform (extend)

Satisfies: none — infrastructure for doctor check (d), the gc runner and cli `gc`.
Reuse: 2 — extends `platform::{Fs, LockKind, LockHolder, hold_lock_forever}` and `apfs::Apfs` (`src/platform/mod.rs:95-185`, `apfs.rs:89-261`); 3 — `std::fs::{remove_dir_all, remove_file, read_dir, symlink_metadata}`; 5 — `libc` (`open` with `O_NOFOLLOW|O_NONBLOCK|O_CLOEXEC`, `fstat`, `flock`, `fcntl`, `statfs`, `getattrlist` with `FSOPT_NOFOLLOW`). No new dependency.
Respects: ARCH-15, ARCH-19

Surface:

| Reader | Disposition |
|---|---|
| `doctor::check_lock_probe` (`doctor.rs:521-524`) | replace — it uses `try_lock_exclusive` and `try_lock_elsewhere` |
| `FakeFs` and `Knobs` in `doctor.rs` tests (`:755-884`; `probe_sees_locks` at 761, 774, 874; the cases at ~1011-1013 and ~1179) | replace — `probe_sees_locks` becomes per-kind refusal knobs, and the new methods are added |
| `tests/platform_locks.rs:15-33, 44, 56` | replace — it moves to `try_lock_exclusive`/`try_lock_elsewhere`, adds the `fcntl-read` kind, and adds a `__try-lock` never-creates-state test |
| `tests/status_targets.rs:582` (`__hold-lock flock`) | replace — the spelling is unchanged and must still work |
| `src/platform/apfs.rs` unit tests (`sharing` at 330/333/338, `hold_lock_forever` at 357) | replace — `FSOPT_NOFOLLOW` gives the same result on regular files, and they gain the deadline |
| doctor checks (a)/(c) calling `sharing` (`doctor.rs:409, 491, 493`) | replace — same result for files hive created (AUTOGC-7.4) |
| `cli.rs:120` usage text "expected flock or fcntl" | replace — it lists `fcntl-read` |

Interface:
- `LockKind::{Flock, Fcntl, FcntlRead}`, with `as_arg` giving `flock`, `fcntl` and `fcntl-read`.
- `HeldLock::new(release)`.
- `Fs::try_lock_exclusive(&Path) -> io::Result<TryLock::{Acquired(HeldLock), Held, Gone, Unopenable(io::Error), Unlockable(io::Error)}>`.
- `Fs::try_lock_elsewhere(LockKind, &Path) -> io::Result<LockState>`. It spawns a child running `__try-lock <kind> <path>`, with a 5 s deadline.
- `Fs::entry_stat(&Path) -> io::Result<EntryStat{dev, ino, nlink, blocks, kind: File|Dir|Symlink|Other, compressed}>`.
- `Fs::read_dir_bounded(&Path, cap) -> io::Result<Vec<(String, EntryKind)>>`.
- `Fs::remove_tree(&Path)` and `Fs::remove_file(&Path)`.
- `Fs::volume(&Path) -> io::Result<VolumeInfo{fs_type, container, mount, available_bytes}>`.
- `sharing`, now no-follow.

Depth: n/a — extends `platform::Fs` / `apfs::Apfs`.
Locality: extend — `src/platform/{mod,apfs}.rs` and the hidden `__try-lock` in `src/cli.rs`, handled before `State::open` like `__hold-lock`. New `unsafe` sites (`open`, `fstat`, `flock`, `fcntl`, `statfs`) each carry `// SAFETY:` (code standard). `HeldLock` has no macOS-only field, so the non-macOS stub compiles (merge gate).

### doctor (extend)

Satisfies: AUTOGC-1.1, AUTOGC-1.2, AUTOGC-1.3, AUTOGC-7.4
Reuse: 2 — extends `doctor::{check_lock_probe, Capabilities, capabilities_for, require}` and `FakeFs` (`doctor.rs:115-121, 254-281, 514-542, ~777`)
Respects: ARCH-14, ARCH-19

Surface:

| Reader | Disposition |
|---|---|
| Check (d)'s measured keys in the doctor receipt | replace — the eight new keys; no test asserts the old names |
| `capabilities.json` and `Capabilities` (`deny_unknown_fields`) | replace — adds `lock_check: u32` with `#[serde(default)]`, so old files still parse and get 0 |
| `tests/capability_gate.rs` (builds `capabilities.json`) | replace — its fixtures need no edit (the default is 0); a new case asserts that Deletion is refused with `lock_check` 0 |
| `tests/common::Hive::write_ready_capabilities` | replace — it writes `lock_check: 1` |

Interface:
- Measured keys, all `Bool`:
  - `<h>_holder_refuses_try` and `<h>_holder_free_after_exit` for each `<h>` in `flock`, `fcntl_read` and `fcntl_write`;
  - `held_flock_refuses_fcntl_read` and `held_flock_refuses_fcntl_write`.
- `pass` ⇔ all eight are true.
- `capabilities_for` writes `lock_check: 1`.
- `require(Deletion)` treats `lock_check < 1` as missing results (`RunDoctor`, CORE-1.10).

Depth: n/a — extends doctor.
Locality: extend — `src/doctor.rs`. Checks (a), (b), (c), (e) and (f) and the CORE-1.3 map are untouched (AUTOGC-7.4).

### gc (new module `src/gc.rs`)

Satisfies: AUTOGC-2.1, AUTOGC-2.2, AUTOGC-2.3, AUTOGC-2.4, AUTOGC-2.5, AUTOGC-2.6, AUTOGC-2.7, AUTOGC-2.8, AUTOGC-2.9, AUTOGC-2.10, AUTOGC-2.11, AUTOGC-2.12, AUTOGC-2.13, AUTOGC-2.14, AUTOGC-3.1, AUTOGC-3.2, AUTOGC-4.1, AUTOGC-4.2, AUTOGC-4.3, AUTOGC-4.5, AUTOGC-5.1, AUTOGC-5.2, AUTOGC-5.3, AUTOGC-5.5, AUTOGC-5.6, AUTOGC-6.1, AUTOGC-6.2, AUTOGC-6.3, AUTOGC-6.4, AUTOGC-6.5
Reuse: none — new code (rung 7). No library models rustc's session naming or an inode ledger. It composes `platform::Fs`, `cargo_target::{TargetDir, Owner}`, `fingerprint::Format` and `size::MAX_DEPTH`.
Respects: ARCH-2, ARCH-3, ARCH-4, ARCH-9, ARCH-11, ARCH-18, ARCH-19

Interface:
- `run(fs: &dyn Fs, targets: &[TargetDir], scope: Scope::{All, Checkouts(Vec<PathBuf>)}, opts: RunOpts{dry_run, now, account_bytes}, cancel: &dyn Fn() -> bool) -> GcReport`
- `GcReport{per_target: Vec<(PathBuf, Counts)>, totals: Counts, skipped: Vec<Skip{path, reason}>, unmeasured: Vec<(PathBuf, String)>, unmeasured_more: u64, cancelled: bool}`
- `Counts{sessions_deleted, session_lock_files_deleted, temp_entries_deleted, skipped_held, skipped_lock_unopenable, bytes: Option<Bytes>}`. When `bytes` is present it supplies both receipt counts `freed_bytes` and `deleted_bytes_still_shared`; when absent, both are omitted.
- `is_temp_entry(name, EntryKind) -> bool`
- `MIN_AGE` (10 s)

Depth: deletion test — callers would still need to know "give it targets, a scope and a dry-run flag, and get counts, skips and unmeasured paths, with nothing outside the allowed entries touched". These stay hidden:
- the base-36 name parser and the three-field key pairing;
- newest-finalized-by-timestamp, the 10 s rule and malformed-name handling;
- the re-check-under-guard steps and the dir-before-lock-file order;
- the lockless-dir re-stat;
- the APFS, readable and supported filters and the scope prefix test;
- the bounded listings;
- the ledger's first-`nlink`, attribution and private-size rules;
- the dry-run branch.

Locality: new `src/gc.rs` with private submodules `plan` (pure), `ledger`, `run`, plus `pub mod gc;` in `src/lib.rs`. `discover`, `cargo_target` and `size` are left unchanged.

Flow per target:
1. **Filter** (4.1–4.3, 4.5): scope, then `Fs::volume().fs_type == "apfs"`, then readable. Each profile must be `Format::Supported`.
2. Per crate dir under `incremental/`: `plan::crate_dir(listing, now)` → actions (2.1, 2.5–2.8). Execute each one per Decisions 2 and 3:
   - `Held` → `skipped_held`.
   - `Unopenable` → `skipped_lock_unopenable`.
   - `Unlockable` → a `gc.skipped` finding with the errno.
   - `Gone` → nothing.
3. **Temp entries:** `try_lock_exclusive(.cargo-build-lock)`.
   - `Held` → `skipped_held`.
   - `Gone`/`Unopenable`/`Unlockable` → `gc.skipped` (2.11).
   - Acquired → bounded-list `deps/`, and for each `is_temp_entry`: ledger-scan, then `remove_tree`. Drop the guard after the last one.
   - A missing `incremental/` or `deps/` (an artifact-only dir) is simply nothing to do.
4. **Dry run:** every step runs the same way, including the ledger scan **while the guard is held**. It removes nothing and drops each guard right after the scan (3.1, 3.2).
5. `cancel()` is checked between entries.

### cli `gc` (extend)

Satisfies: AUTOGC-2.15, AUTOGC-3.3, AUTOGC-3.4, AUTOGC-4.4, AUTOGC-5.4, AUTOGC-7.1, AUTOGC-7.2, AUTOGC-7.3, AUTOGC-7.5
Reuse: 2 — mirrors `cli::status` (`src/cli.rs:329-504`), `emit`, `discover::merge_snapshot` (`discover.rs:653`) and `doctor::{require, install_signal_handlers, CANCEL}`; 5 — `clap` derive
Respects: ARCH-1, ARCH-8, ARCH-10, ARCH-13, ARCH-14, ARCH-16, ARCH-17

Surface:

| Reader | Disposition |
|---|---|
| `Command` enum and dispatch (`cli.rs:23-50, 101-115`) | replace — adds `Gc{repos, dry_run, out}` and the hidden `TryLock{kind, path}` |
| `README.md`: the line 5 "reads only … deletes nothing yet", the line 39 "writes only under `~/.hive/`", the command table | replace — states that `gc` deletes superseded sessions and temp entries (`-C save-temps` output included) under locks, and adds a row and a section |
| CORE `design.md:66-69, 178`, CORE-1.1(d) | replace — through `realign-spec` on CORE (Decision 16) |

Interface:
- `cargo hive gc [<repo>…] [--dry-run] [--json]`.
- Exit codes: 0 done; 1 for any refusal (gate, unknown argument); 130 when cancelled.

Depth: n/a — a thin adapter, like `status`.
Locality: extend — `src/cli.rs`, `README.md`. `status` itself is not refactored, so its output cannot drift (AUTOGC-7.3).

Flow:
1. Install handlers.
2. Load the registry, config, capabilities and snapshot.
3. `require(FingerprintReading)`, then `require(Deletion)` with `lock_check ≥ 1`. A refusal becomes a `refusal` finding with exit 1, before any target is touched (2.15).
4. Select the repos, as in `status`: unknown arguments become refusals, the others still run, and the exit code is 1 (4.4).
5. Discover checkouts, then targets (O3). If cancelled at this point: the cancel path.
6. Read the volumes, before.
7. `gc::run`. `account_bytes` is false when check (c) has not passed (O1).
8. Read the volumes, after (real runs only).
9. Build the receipt:
   - command `gc` or `gc --dry-run`;
   - scope from the arguments;
   - the counts;
   - skips;
   - `unmeasured`;
   - one note per container.
10. Merge the snapshot, then `emit`.

No input is read (3.4).

## Seams for testing

| Seam | Kind | Covers |
|---|---|---|
| `gc::plan::crate_dir` and the name parser, table-driven:<br>- the 10 s boundary at exactly 10 s and one microsecond under;<br>- equal timestamps;<br>- future timestamps;<br>- `+`, uppercase and `u64`-overflow names;<br>- lockless finalized and lockless `-working` dirs;<br>- orphan locks;<br>- `is_temp_entry` versus `rustc_hash-<hash>.d`, `rustcfoo-1.dSYM` and symlink kinds | unit | AUTOGC-2.1, 2.5, 2.6, 2.7, 2.8, 2.12 |
| `gc::run` against a **recording `Fs` fake**:<br>- pins the order lock → re-check → scan → dir → lock file → drop;<br>- injects `Gone` after the lock (the finalize race: the lock file must survive);<br>- injects a lock file that appears before a lockless delete (left alone);<br>- injects `Held`, `Unopenable` and `Unlockable`;<br>- in a dry run, records no removal;<br>- a non-APFS volume;<br>- scope `Checkouts` versus `All`;<br>- a stop between entries;<br>- `account_bytes` false, so the byte counts land in `unmeasured`;<br>- a crate-dir listing of exactly 100,000 entries (processed) and of 100,001 (profile skipped with a finding);<br>- a `deps/` listing of exactly 1,000,000 and of 1,000,001 | unit | AUTOGC-2.2, 2.3, 2.4, 2.9, 2.10, 2.11, 2.13, 2.14, 3.1, 3.2, 4.1, 4.2, 4.3, 4.5, 5.6, 6.1 |
| `gc::ledger` with synthetic stats:<br>- a surviving link;<br>- two old sessions sharing an inode, in real order versus dry order (same result);<br>- a clone flag;<br>- compressed;<br>- an unreadable file;<br>- a cross-target link (attribution sums to the totals);<br>- the `unmeasured` cap | unit | AUTOGC-5.1, 5.2, 5.3, 5.5 |
| `doctor::run` check (d) with `FakeFs` per-kind knobs; `require(Deletion)` with `lock_check` 0 and 1 | unit | AUTOGC-1.3, 7.4 |
| `apfs::Apfs` across processes (`tests/platform_locks.rs`):<br>- `try_lock_exclusive` versus `__hold-lock flock`, `fcntl` and `fcntl-read`;<br>- `try_lock_elsewhere` while a guard is held;<br>- a FIFO or a symlink as the lock path gives `Unopenable`;<br>- the 5 s deadline on a child that never answers | integration | AUTOGC-1.1, 1.2 |
| The `cargo-hive` binary with `HIVE_HOME`, on real-cargo fixtures (`Hive::ready`, `build_crate`):<br>- two-session crate dirs;<br>- a `-working` session held through `__hold-lock fcntl`;<br>- a lockless dir;<br>- an orphan lock;<br>- a `deps/rustc_hash-*.d` look-alike;<br>- a held build lock;<br>- unreadable and unsupported targets;<br>- two repos sharing a target;<br>- an unknown argument;<br>- the doctor gate without `lock_check`;<br>- `--dry-run` versus a real run on an idle fixture;<br>- 12 build cycles with a concurrent gc loop;<br>- after `gc`, `cargo build` compiles 0 units;<br>- repeated SIGKILL at random offsets, then a second `gc`: the kept session's file set and inodes are unchanged, the second `gc` converges, and an edited build succeeds | integration | AUTOGC-2.15, 3.3, 3.4, 4.4, 6.2, 6.3, 6.5, 7.1, 7.2, 7.3, 7.5 |
| `cargo hive doctor` through the binary (`tests/doctor.rs`, machine-gated): the eight keys are reported, and the check passes on this Mac | integration | AUTOGC-1.1, 1.2, 1.3 |
| **New (needs O5):** a test-created APFS disk image:<br>- `hdiutil create -fs APFS`, then `attach -nobrowse` under `/private/tmp/hive-it-*`;<br>- `.metadata_never_index`, `sync` before sampling;<br>- detached on drop and swept when stale;<br>- holds a fixture target;<br>- asserts `freed_bytes` ≤ the measured increase plus a stated slack for system writes, and the per-container note | integration | AUTOGC-5.4, 6.4 |

## Coverage check

Every live ID in `requirements.md` appears in exactly one `Satisfies:` line:

| Section | IDs |
|---|---|
| platform | none — infrastructure |
| doctor | 1.1, 1.2, 1.3, 7.4 |
| gc | 2.1–2.14, 3.1, 3.2, 4.1–4.3, 4.5, 5.1–5.3, 5.5, 5.6, 6.1–6.5 |
| cli `gc` | 2.15, 3.3, 3.4, 4.4, 5.4, 7.1, 7.2, 7.3, 7.5 |

The guards 7.1–7.3 sit with the cli, which hosts `repo add`, `status` and `doctor`. This feature leaves those paths unchanged, and the binary seam proves it.

Requirement changes from this design (upstream sync-back, approved by the owner 2026-10-28):
- **AUTOGC-4.5 (new):** a target not on APFS is left untouched, with a finding (Decision 7).
- **AUTOGC-5.6 (new):** the degrade branch of O1.
- **AUTOGC-3.1 (amended):** "Every lock is tried, held only while its entry is measured, and released before the next entry." The dry run measures under the same guard as a real run, so first-sight `nlink` is read at the same point in both modes (Decision 8). G-U1 in the close package was amended to match.

Answered during review (no longer open):
- `remove_dir_all` never follows a symlink (std `unix.rs` lines above).
- `f_mntfromname` on this Mac (Decision 12).
- The rename race (Decision 2).
