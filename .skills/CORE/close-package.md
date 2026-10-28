# cargo-hive: interview close package (confirmed 2026-10-25)

Source: the frame-change interview, with clarify-decisions nested and coverage gate OFF (no written posture). Evidence: `RESEARCH.md`, `SPIKES.md` §1–15 plus the evidence correction, and `evidence/build-concurrency.py`. Confirmed by the user ("yes, approve").

## Problem lock

- **Observed.** Every new agent worktree pays a cold first build: median 37 s, p90 119 s across 69 new checkouts in 27 days. Each one also holds tens of GB of build output. Nothing reclaims superseded state: about 60 GB of old incremental sessions across checkouts, and 41.7 GB of the 69 GB in klynt's main target was unreachable from the keep-set.
- **Desired.** Running N agent worktrees on one Mac costs roughly one checkout's build time and disk, proven by real counters.
- **Non-goals:**
  - changes to rustc or Cargo;
  - "relink, don't rebuild";
  - distributed or remote caching;
  - CI and Linux;
  - a jobserver;
  - slot mode;
  - editing repo files;
  - replacing artifacts in place;
  - Claude Code hooks;
  - time claims outside `hive bench`;
  - claiming that main is protected from agents;
  - build-on-host / jayden-host.

## Decisions

| Radius | Topic | Decision |
|---|---|---|
| architecture | v1 core | Worktree seeding plus disk (clone, compress, GC). No jobserver. No resident daemon |
| architecture | Placement | Foreign mode everywhere: no slot pool, no `WorktreeCreate` hook. Slot mode waits until `-p` and `--workspace` resolve the same feature set |
| architecture | Wrapper / shim | No rustc wrapper (spike 12) and no `cargo` shim on PATH |
| data | Seed source | A hive-owned store, filled when `hive seed` runs, from registry units in live checkouts. Clonefile, not hardlinks. Runs outside Cargo. Crates with build scripts are kept (spike 7) |
| data | Store key (D1) | `(rustc, "<name>-<metadata hash>", fingerprint hash)`. At seed time, each name gets the variant with the newest `last_live_at` |
| data | Store GC | Each unit records `size`, `filled_at`, `last_live_at`; `last_live_at` comes from closure scans. Evict after 30 days not live anywhere. Drop a rustc generation that no checkout uses. The GB cap counts bytes eviction would actually free (D7). Never evict live units; if the live set exceeds the cap, report and stop |
| data | GC liveness | GC never invokes cargo and never replays commands. Roots are units whose `invoked.timestamp` falls inside the window. Closure follows Cargo's fingerprint JSON `deps`, each u64 matched to a unit's hash file as little-endian hex (spike 9) |
| data | Deletion rule | Delete `deps/` artifacts of dead units, plus the fingerprint dir with the same hash. Never delete fingerprint dirs on their own. Never touch `build/` |
| UX flow | GC tiers | **Auto tier:** `rustc*`/`rmeta*` temp entries, and superseded incremental sessions per the rustc rule (D6), always after a try-lock on the owner's lock. **Plan tier:** closure deletion and whole orphan target dirs |
| auth/security | Who may apply | Agents may apply plans that name a secondary checkout. Plans for protected checkouts or orphan dirs need the human. Apply recomputes liveness, try-locks and format-checks, and refuses stale plans. Scope never comes from cwd |
| auth/security | Approval | `apply` on a protected plan never opens a dialog; it returns `needs-human` with the plan id. `hive approve <id>` opens `.deviceOwnerAuthentication` only when there is a TTY and no agent-CLI ancestor (matched on argv; configurable list). The dialog expires after 60 s. One approval per plan, no reuse. MCP has no approve tool. This is a guard on hive's own path only |
| data | Classification (C1) | Rules in order: (1) a registered main is always protected. (2) A linked worktree whose realpath is inside its own main is secondary. (3) A checkout whose realpath is under a `secondary_root` (default `/private/tmp`, `$TMPDIR`) is secondary. (4) Everything else is protected |
| data | Discovery | `hive repo add <main>`. Checkouts are main, plus `git worktree list`, plus a depth-2 scan of temp roots matched by root commit. Target dirs are found by `CACHEDIR.TAG` whose comment reads "created by cargo" **and** that has a profile dir containing `.fingerprint/`. Search inside known checkouts and under cache roots (default `~/.cache/cargo-target`, depth 2). A target that isn't the workspace's configured target is an orphan candidate (plan tier). No unbounded walks |
| architecture | Receipts | Deterministic counters only. `reused_fresh` = seeded units reachable in the closure of the first build's compiled units whose `invoked.timestamp` is unchanged. Seeded units that nothing needed are counted separately. Bytes are summed per file (D7); a `statfs` delta may appear only as a labelled cross-check. Seconds come only from `hive bench` (runs, median, p90, spread, load average) |
| architecture | Write boundary | On protected checkouts hive only deletes (auto tier and approved plans) and reads (store fill, scans). It never replaces artifacts (spike 14) and never edits repo files. `hive explain` reports feature divergence, the always-failing rust-analyzer clippy command, orphans, and emptied protected targets. It never suggests seeding a protected checkout. Fixes belong to the per-repo build-optimization skill |
| auth/security | `cargo clean` guard | No hook in v1. Reopen if `explain` sees main emptied more than once a month |
| architecture | Platform | macOS in general, with a platform layer (`Fs`, `Approve`) whose only implementations are APFS and LocalAuthentication. The proof machine is macOS 27 |

## Constraints

**Architecture**
- A1. No resident daemon, no rustc wrapper, no `cargo` shim, no Claude Code hooks.
- A2. Foreign mode for every checkout.
- A3. Seeding targets secondary checkouts only.
- A4. On protected checkouts, delete and read only.
- A5. Receipts carry deterministic counters only. Seconds come only from `hive bench`.
- A6. Operation scope comes from the plan or an argument, never from cwd.

**Data**
- D1. Store key: `(rustc, name-meta, fingerprint)`. Registry units only.
- D2. Compress in the store, then clone out. Never compress a clone.
- D3. Liveness: roots from the `invoked.timestamp` window, plus closure over the fingerprint graph (little-endian hash match).
- D4. Version-check the fingerprint format per toolchain. Refuse GC on an unknown shape.
- D5. Never delete fingerprint dirs on their own. Never touch `build/`.
- D6. Sessions: keep the newest finalized one, keep any `-working` whose lock is held, delete the rest.
- D7. Freed bytes: `st_blocks×512` minus bytes still shared, from `SHARES_ALL`, or from `MAY_SHARE` together with a matching clone id among recorded clones.
- D8. Store fill try-locks the source checkout's target and skips it if the lock is held.

**Auth / security**
- S1. Dialogs come only from `hive approve`, with a TTY and no agent ancestor. `apply` and MCP never open one.
- S2. Agents may apply plans for secondary checkouts only. Apply always revalidates.

**Classification**
- C1. Registered main → protected. Linked inside own main → secondary. Under a `secondary_root` → secondary. Else → protected.

**Install** (any Mac, from scratch)
- I1. No dependency beyond a Rust toolchain. Applesauce is embedded as a crate. LocalAuthentication is reached via Rust objc bindings, with no Swift or Xcode needed at runtime. No afsctool.
- I2. Install with `cargo install cargo-hive`, then `cargo hive doctor`, then `cargo hive repo add <main>`. Register MCP with `claude mcp add hive -- cargo-hive mcp`.
- I3. Works with zero config. Defaults: temp roots, cache root `~/.cache/cargo-target`, GC window 7 days, store budget 40 GB, store max age 30 days.
- I4. `cargo hive doctor` runs the spiked mechanism checks in a temp dir on that machine:
  - clone sharing;
  - Applesauce compress and verify on a single-link file with its mtime kept, and leave hardlinked files untouched (Applesauce skips files with link count > 1; req-review §9);
  - `SHARES_ALL` and clone id;
  - Cargo and rustc try-lock;
  - `canEvaluatePolicy`;
  - the fingerprint format of the active toolchain.

  Features that fail are disabled on that machine. The declared floor is macOS 27; older versions are supported when they pass `doctor`.

## High-tweak surface
- The receipt schema and the `reused_fresh` closure definition. Validate against Cargo's own counts from spike 7 (466 needed, 31 compiled).
- Reading Cargo's internal fingerprint format; it can change with any toolchain.
- The `store.db` schema, and closure-scan cost per `hive seed`.
- The `plan` → `apply` → `needs-human` → `approve` flow, over CLI and MCP.

## Done signals
- **S-1.** In a freshly seeded secondary checkout, the first build recompiles **0 seeded units**. Newly compiled registry units (variants the store doesn't hold yet) are counted separately.
- **S-2.** **0 units rebuilt because of a deletion**, and every deletion follows D6. The GB reclaimed is reported in the receipt; it is not a target.
- **S-3.** Every operation, CLI and MCP, returns a receipt of deterministic counters only. On protected checkouts only deletes and reads happen.
- **S-4.** After seeding k secondary checkouts, their registry part adds **≈ 0 private bytes**, counted per D7.

## Boundaries
- **Off limits:** the non-goals above, plus suggesting a seed for a protected checkout.
- **Must keep working:**
  - `cargo` and rust-analyzer behave as without hive on every checkout;
  - existing worktree workflows (`isolate-workspace`, native `.claude/worktrees`, `build-in-waves` and `build-by-story` lane worktrees) need no change;
  - on main, `cargo watch` and rust-analyzer are never made to rebuild by hive, except through approved deletions.

## Spine touch
None. No architecture spine exists.

## Owned unknowns (carried into design)
- **Jobserver:** what signal would bring it back while hive cannot see build durations.
- **Builds hive can't see:** background runs, `cargo watch`, rust-analyzer.
- **Orphans:** attributing `~/.cache` orphan targets to a repo via paths in `.d` files.
- **Measurement and timing:**
  - closure-scan cost;
  - steady-state reclaim at a 7-day window, which needs a target older than the window;
  - how `hive bench` samples cold versus seeded;
  - when the "after first build" receipt is computed.
- **Agent environment:**
  - the MCP server's cwd;
  - TTY behaviour inside Codex;
  - argv of the Node-based agent CLIs.
- **Proposed, not locked:** warn when a secondary root contains git checkouts that belong to no registered repo. A per-checkout "protect" marker for `/tmp` clones is deferred.

## Amendment 2026-10-25 (design-solution, F9)

The lock "GC never invokes cargo" now reads: GC never compiles and never replays a build. Ownership is the one exception: it may run exactly `cargo metadata --no-deps --format-version 1 --locked --offline` with `RUSTUP_AUTO_INSTALL=0`, with `CARGO_TARGET_DIR`, `CARGO_BUILD_TARGET_DIR`, `CARGO_BUILD_BUILD_DIR` and `GIT_*` scrubbed, and no other cargo subcommand. At ROAD-6 apply time ownership is resolved fresh, never from the `snapshot.json` cache.

## Amendment 2026-10-28 (define-project)

The Desired line in the Problem lock is narrowed to the wording in `docs/product/vision.md`: dead bytes are reclaimed with zero deletion-caused rebuilds, and a new secondary checkout's first build recompiles zero seeded registry units at about zero private bytes, all shown in receipts. The original line stays above as confirmed. It overstated both halves:
- **Time.** A deps-only seed in a foreign checkout rebuilt 0 of 434 registry units, yet the first build still took 56.2 s against 77.1 s cold, because the 42 workspace crates compile cold (SPIKES.md §7). N worktrees still pay N workspace compiles.
- **Disk.** Main's live `incremental/` was 17.33 GB (SPIKES.md §8), and Cargo uses incremental compilation only for workspace members and path dependencies, so that state stays private to each checkout. Seeding shares the registry part only.
