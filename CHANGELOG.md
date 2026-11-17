# Changelog

## Unreleased

### Cargo unit dirs beside the flat layout

- Rewriting a state file stamps layout 2. cargo-hive 0.1.2 and earlier refuse a rewritten file; downgrade is unsupported.
- `cargo hive doctor` records `unit_layout` when check (f) passes.
- A capabilities file with no `unit_layout` and fingerprint reading still enabled is treated as flat until doctor runs again.

## 0.1.2 — 2026-11-16

### Upgrading keeps hive's state (UPGRADE)

- Every state file hive writes now records its layout. A newer hive reads each file an earlier release wrote, or refuses it by name (UPGRADE-1.1, UPGRADE-2.1).
- A state file that exists but cannot be read stops the command with exit 1. The same holds for one written by a newer hive. The finding names the file and the command that recovers it, and the file is left untouched. It is never read as empty (UPGRADE-3.1, UPGRADE-3.2, UPGRADE-3.8).
- A missing `store.db` no longer makes bare `gc` empty the store (UPGRADE-4.1).
- `seed` never replaces a seed record it cannot read (UPGRADE-3.4).
- **One-way step:** once 0.1.2 writes `registry.json`, `capabilities.json` or `snapshot.json`, cargo-hive 0.1.0 and 0.1.1 can no longer read that file. An ordinary `status` or `gc` writes `snapshot.json`. Downgrading is not supported (UPGRADE-5.8).

### Clean one checkout with `gc --checkout` (CHECKOUTGC)

- `cargo hive gc --checkout <path>` cleans one registered checkout, a worktree or the main checkout. In one run, under the target's Cargo locks, it deletes:
  - that checkout's dead units;
  - the debug objects (`deps/*.rcgu.o`) that no linked binary's debug map references.

  It touches nothing outside that checkout (CHECKOUTGC-1.1, CHECKOUTGC-2.1, CHECKOUTGC-3.1).
- Targets that are shared, lie outside the checkout, or are held by a build are left alone and named in the receipt (CHECKOUTGC-3.2, CHECKOUTGC-3.7, CHECKOUTGC-4.2).
- On a copy of an aged worktree, one run took 5.2 s, deleted 29,048 of 32,231 debug objects, and caused no recompile.
- The hive skill tells an agent to run it before every test build in a reused worktree (CHECKOUTGC-5.5).

### Faster checkout discovery (#31)

- Listing a repository's checkouts no longer starts git for a repository under a secondary root whose object store holds none of the repository's root commits (CORE-3.9). `gc --checkout` lists checkouts once (CHECKOUTGC-5.6).
- With about 1,400 unrelated repositories in the temporary directories, `status --no-sizes` went from 84 s to about 1.4 s, and `gc --checkout` from 151 s to about 2–3 s.
- Discovery lists the same checkouts as before, with one exception. A repository git cannot answer for, such as one with no commit yet, is no longer listed as an unclassifiable protected checkout (CORE-3.10).

### Recorded as shipped

- The workspace split (SPLIT) and the single hive skill guide (SKILLPASS) shipped in 0.1.1. Their specs are marked shipped with this release.

### Misc

- Hive's own tests use a scratch secondary root and no longer walk the machine's temporary directories.
- The skill leaves a target a build holds (`gc.skipped`) for the next run. Only shared and outside targets fall back to repo-wide `gc --plan` then `apply`.
