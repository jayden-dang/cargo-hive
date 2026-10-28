# Requirements: Foundation (registry, discovery, classification, target identification, fingerprint reader, receipts, doctor)

Feature code: CORE
Status: In-progress
Date: 2026-10-25

<!--
Rules:
- Feature code: 2-12 chars, A-Z0-9, starts with a letter, unique repo-wide.
  Register it in docs/specs/INDEX.md before use.
- Every acceptance criterion gets a hierarchical ID: <CODE>-<story>.<criterion>.
- Criteria use EARS phrasing:
    WHEN <event/condition> THE SYSTEM SHALL <behavior>          (event-driven)
    WHILE <state> THE SYSTEM SHALL <behavior>                   (state-driven)
    IF <unwanted condition> THEN THE SYSTEM SHALL <behavior>    (unwanted behavior)
    WHERE <feature is included> THE SYSTEM SHALL <behavior>     (optional feature)
    THE SYSTEM SHALL <behavior>                                 (ubiquitous)
- Guard requirements protect existing behavior this feature touches:
    WHEN <condition> THE SYSTEM SHALL CONTINUE TO <existing behavior>
- IDs are immutable once Status is Approved. Retire a requirement by striking it
  through (~~**CODE-N.M**~~ reason) — never renumber.
-->

Roadmap item: ROAD-1 (MILE-1, docs/roadmap/INDEX.md). Sources: `.skills/CORE/close-package.md` (confirmed 2026-10-25), `CONTEXT.md` (terms used here as defined there), `SPIKES.md` §1–15.

## 1. Check a machine before trusting hive on it

**Story:** As a developer who just installed cargo-hive on a Mac, I want to run `cargo hive doctor`, so that I see which hive capabilities actually work on this machine before any of them is used.

- **CORE-1.1** WHEN the user runs `cargo hive doctor` THE SYSTEM SHALL run each of these capability checks inside a newly created temporary directory: (a) an APFS clone of a file shares all its blocks, measured as zero private bytes for both files; (b) a single-link file compressed with the embedded Applesauce library reads back byte-for-byte identical with its mtime unchanged, and a file with more than one hardlink is left uncompressed with its inode, content, and mtime unchanged; (c) a cloned file carries the `SHARES_ALL` flag and the same clone id as its source, and a whole-file rewrite of the clone gives it a new clone id; (d) a non-blocking lock probe reports a lock held by another process as held without waiting; (e) device-owner authentication reports as available or unavailable without showing any dialog; (f) the fingerprint files that the active Rust toolchain writes for a throwaway crate built inside that temporary directory match a fingerprint format hive supports. The active Rust toolchain is the one rustup selects with the working directory at the hive state root.
- **CORE-1.2** WHEN `cargo hive doctor` finishes THE SYSTEM SHALL report every check as `pass`, `fail`, or `unavailable`, together with the measured value it was judged on (for example, the private byte count after the clone).
- **CORE-1.3** WHEN a doctor check's verdict is not pass THE SYSTEM SHALL record in hive's state under `~/.hive/` that the capabilities mapped to that check are disabled on this machine, using this map: (a) clone sharing → seeding; (b) compression round-trip → store compression; (c) sharing flags and clone id → store eviction accounting and private-byte receipts; (d) lock probe → every deletion; (e) owner authentication → approval; (f) fingerprint format → fingerprint graph reading, every deletion, and seeding.
- **CORE-1.4** IF a command needs a capability recorded as disabled THEN THE SYSTEM SHALL refuse that command and name the failing doctor check.
- ~~**CORE-1.5**~~ superseded by CORE-1.8 and CORE-1.9 (design review F10: no process can clean up on SIGKILL) — was: ~~WHEN `cargo hive doctor` exits, whether its checks passed or failed, THE SYSTEM SHALL have removed the temporary directory it created.~~
- **CORE-1.6** IF the operating system is not macOS THEN THE SYSTEM SHALL exit with a non-zero status and a message stating that cargo-hive supports macOS only.
- **CORE-1.7** IF the macOS version is older than 27 THEN THE SYSTEM SHALL report the version as below the declared floor and still run every check, so that support on that machine follows the check results.
- **CORE-1.8** WHEN `cargo hive doctor` exits normally, after a failed check, after a panic, or on SIGINT or SIGTERM, THE SYSTEM SHALL have removed the temporary directory it created.
- **CORE-1.9** WHEN `cargo hive doctor` starts THE SYSTEM SHALL remove any temporary directory left by an earlier doctor run whose process no longer exists.
- **CORE-1.10** IF a command needs a capability and the recorded doctor results are missing or were recorded for a different Rust toolchain release than the active one THEN THE SYSTEM SHALL refuse that command and ask the user to run `cargo hive doctor`. The active toolchain is the one rustup selects with the working directory at the hive state root.
- **CORE-1.11** (fix, 2026-10-28) WHEN a hive command runs with `RUSTUP_TOOLCHAIN` set and `RUSTUP_TOOLCHAIN_SOURCE` either absent or naming a directory selection (`toolchain-file`, `path-override`) THE SYSTEM SHALL remove both variables from the `rustup` and `cargo` processes it starts, so that neither the active toolchain nor `cargo metadata` follows a toolchain the caller's directory selected.
- **CORE-1.12** (guard, 2026-10-28) WHEN a hive command runs with `RUSTUP_TOOLCHAIN_SOURCE` set to `cli`, `env` or `default` THE SYSTEM SHALL CONTINUE TO pass `RUSTUP_TOOLCHAIN` to the `rustup` and `cargo` processes it starts.

## 2. Register a repository

**Story:** As a developer, I want to run `cargo hive repo add <path>` on my repository's main checkout, so that hive starts serving that repository.

- **CORE-2.1** WHEN the user runs `cargo hive repo add <path>` and `<path>` is inside the main (non-linked) checkout of a git repository that has a `Cargo.toml` at most 2 directory levels below the checkout root, THE SYSTEM SHALL record the main checkout's realpath and the set of the repository's root commits in hive's registry.
- **CORE-2.2** IF `<path>` is inside a linked worktree (its absolute `git rev-parse --git-dir` differs from its absolute `--git-common-dir`) THEN THE SYSTEM SHALL refuse the registration and name the main checkout's path to register instead.
- **CORE-2.3** IF `<path>` is not inside a git checkout, or that checkout has no `Cargo.toml` at most 2 directory levels below its root, THEN THE SYSTEM SHALL refuse the registration and state which of the two is missing.
- **CORE-2.4** WHEN the user registers a main checkout that is already registered THE SYSTEM SHALL leave the registry unchanged and report that the repository is already registered.
- **CORE-2.5** WHEN the user runs `cargo hive repo list` THE SYSTEM SHALL list every registered repository with its main checkout path and its root commits.
- **CORE-2.6** WHEN any hive command runs on a machine with no hive state THE SYSTEM SHALL create its state directory `~/.hive/` and proceed with the built-in defaults, requiring no configuration file.
- **CORE-2.7** IF `<path>` is inside a git submodule checkout (git reports a superproject working tree) THEN THE SYSTEM SHALL refuse the registration and name the superproject's path.
- **CORE-2.8** IF the repository has no commits yet THEN THE SYSTEM SHALL refuse the registration and state that it has no commits.
- **CORE-2.9** IF the repository is a shallow clone THEN THE SYSTEM SHALL refuse the registration and state that its root commits cannot be determined.

## 3. See every checkout of a repository and how hive classifies it

**Story:** As a developer, I want `cargo hive status <repo>` to list every checkout of a registered repository as protected or secondary, so that I know exactly where hive is allowed to act before it acts.

- **CORE-3.1** WHEN the user runs `cargo hive status <repo>` THE SYSTEM SHALL list as that repository's checkouts: its registered main checkout, every worktree reported by `git worktree list` for it, and every primary git checkout (its git directory equals its common directory) found at most 2 directory levels below a secondary root whose root commits intersect the repository's root commits.
- **CORE-3.2** THE SYSTEM SHALL classify each checkout by applying these rules in order and stopping at the first match: (1) a registered main checkout is protected; (2) a linked worktree whose realpath is inside its own main checkout's directory is secondary; (3) a checkout whose realpath is under a secondary root is secondary; (4) every other checkout is protected.
- **CORE-3.3** THE SYSTEM SHALL resolve every path to its realpath before applying any classification rule.
- **CORE-3.4** IF hive cannot determine a checkout's realpath or git metadata THEN THE SYSTEM SHALL classify that checkout as protected and state the reason in the status output.
- **CORE-3.5** THE SYSTEM SHALL use `/private/tmp` and the realpath of `$TMPDIR` as the secondary roots when no configuration sets them.
- **CORE-3.6** WHERE hive's configuration file sets `secondary_roots` THE SYSTEM SHALL use exactly that list in place of the default secondary roots.
- **CORE-3.7** WHEN `git worktree list` reports a worktree whose directory does not exist on disk, whether or not git marks it `prunable` (a locked worktree is not marked), THE SYSTEM SHALL list it as missing and exclude it from every other status result.
- **CORE-3.8** WHEN the user runs `cargo hive status` with no repository argument THE SYSTEM SHALL report every registered repository, regardless of the current working directory.

## 4. See every Cargo target dir, its owner, and whether hive can read it

**Story:** As a developer, I want `cargo hive status <repo>` to list every Cargo target dir of the repository with its size, its owning checkout or an orphan-candidate mark, and whether its fingerprints are readable, so that I see where the disk goes and which dirs later reclaim steps may touch.

- ~~**CORE-4.1**~~ superseded by CORE-4.12 (design scan: Cargo's newer build-dir layout has no `.fingerprint/`) — was: ~~THE SYSTEM SHALL treat a directory as a Cargo target dir only when its `CACHEDIR.TAG` file contains the line `# This file is a cache directory tag created by cargo.` and the directory contains a profile directory that has a `.fingerprint/` subdirectory.~~
- **CORE-4.2** WHEN status runs THE SYSTEM SHALL look for Cargo target dirs at most 3 directory levels below each known checkout and at most 2 levels below each cache root, never descending into `node_modules` or into a directory already identified as a Cargo target dir.
- **CORE-4.3** THE SYSTEM SHALL use `~/.cache/cargo-target` as the cache root when no configuration sets cache roots.
- ~~**CORE-4.4**~~ superseded by CORE-4.13 (design scan: fingerprints live in `build_directory`, which can differ from `target_directory`) — was: ~~THE SYSTEM SHALL mark a Cargo target dir as owned by a checkout when its realpath equals the target directory Cargo resolves for that checkout's workspace.~~
- **CORE-4.5** THE SYSTEM SHALL mark a Cargo target dir that no checkout owns as an orphan candidate only when every registered repo is in the run and every queried workspace resolved; otherwise that target is unresolved.
- **CORE-4.6** WHEN status reports a Cargo target dir THE SYSTEM SHALL report its allocated size as the sum of `st_blocks × 512` over its files, counting each inode once.
- **CORE-4.7** WHEN status reports a Cargo target dir THE SYSTEM SHALL report, for each profile directory, the number of units (one unit per directory under `.fingerprint/` holding exactly one fingerprint JSON file) and whether their fingerprint format is supported.
- **CORE-4.8** WHEN hive reads a profile directory whose fingerprint format is supported THE SYSTEM SHALL resolve each `deps` entry of each unit's fingerprint JSON to the unit whose hash file (named `<kind>-<target>`, for example `lib-serde`, `build-script-build-script-build`, `run-build-script-build-script-build`, `test-it`; never `output-*`, `dep-*`, or `invoked.timestamp`) holds that entry's final u64 written as the lowercase hex of its 8 bytes in little-endian order, and report the counts of resolved, unresolved, and ambiguous edges.
- ~~**CORE-4.9**~~ superseded by CORE-4.14 (design scan: a newer-layout profile dir must be reported unsupported, not missed) — was: ~~IF a profile directory's fingerprint format is not supported THEN THE SYSTEM SHALL mark that profile directory as unsupported in status and in hive's state, and SHALL NOT report unit edges for it.~~
- **CORE-4.10** IF a Cargo target dir is unreadable (for example, permission denied) THEN THE SYSTEM SHALL report that dir as unreadable with the reason and continue with the others.
- **CORE-4.11** WHERE `cargo hive status` is given `--no-sizes` THE SYSTEM SHALL skip the size accounting of CORE-4.6 and mark sizes as not measured in the receipt.
- **CORE-4.12** THE SYSTEM SHALL treat a directory as a Cargo target dir only when its `CACHEDIR.TAG` file contains the line `# This file is a cache directory tag created by cargo.` and the directory contains a profile directory that has either a `.fingerprint/` subdirectory or a `build/<package>/<16 lowercase hex>/fingerprint/` subdirectory.
- **CORE-4.13** THE SYSTEM SHALL mark a Cargo target dir as owned by a checkout when its realpath equals either the target directory or the build directory that Cargo resolves for that checkout's workspace.
- **CORE-4.14** IF a profile directory has no `.fingerprint/` subdirectory, or its fingerprint format is not supported, THEN THE SYSTEM SHALL mark that profile directory as unsupported in status and in hive's state, and SHALL NOT report unit edges for it.
- **CORE-4.15** WHEN the build directory Cargo resolves for a known checkout's workspace lies outside every search root THE SYSTEM SHALL still report it as a Cargo target dir owned by that checkout.

## 5. Get a machine-readable receipt from every command

**Story:** As an agent driving hive, I want every `cargo hive` command to return one JSON receipt with `--json`, so that I can act on exact counts instead of parsing prose.

- **CORE-5.1** WHEN any `cargo hive` command runs with `--json` THE SYSTEM SHALL print exactly one JSON receipt object to stdout and nothing else to stdout.
- **CORE-5.2** WHEN any `cargo hive` command completes THE SYSTEM SHALL append its receipt as one JSON line to a receipts log under `~/.hive/`.
- **CORE-5.3** THE SYSTEM SHALL put in a receipt only values that hive counted or measured: byte counts summed per file, unit counts, file counts, edge counts, and check results.
- **CORE-5.4** THE SYSTEM SHALL NOT put in any receipt a field that states or estimates build time saved.
- **CORE-5.5** IF a receipt value cannot be measured THEN THE SYSTEM SHALL omit that value and state why in the receipt.
- **CORE-5.6** THE SYSTEM SHALL include in every receipt the hive version, the macOS version, the receipt schema version, and the repositories and checkouts the command acted on.
- **CORE-5.7** THE SYSTEM SHALL take the scope of every command from its arguments and never from the current working directory.

## 6. Quality attributes

**Section-kind:** nfr

**Story:** As a stakeholder, I want measurable quality targets for this feature, so that how-well is not left implicit.

Consult outcome: no `docs/product/metrics.md`, `docs/ops/reliability.md`, `docs/security/threat-model.md`, or `docs/standards/accessibility.md` exists (greenfield repo). The targets below come from frame-change locks (no fabricated IDs).

- **Performance:** **CORE-6.1** WHEN `cargo hive status <repo> --no-sizes` runs on the proof machine against klynt (`~/Developer/klynt/klynt`) THE SYSTEM SHALL finish discovery, classification, and target identification within 2 s both for the median of 5 runs with a warm cache and for one run starting from an empty `snapshot.json` (cold cache), verified by timing those runs (lock: "worst case under about 2 s" from the discovery card; measured parts 39 ms + 53 ms in SPIKES.md §13, and 127–148 ms per `cargo metadata` call).
- **Performance:** **CORE-6.2** THE SYSTEM SHALL bound every directory walk by an explicit depth limit and SHALL NOT recurse with an unbounded pattern, verified by a test fixture whose deep and `node_modules` trees must not be entered.
- **Security:** **CORE-6.3** THE SYSTEM SHALL create, modify, or delete files only under `~/.hive/` and inside doctor's own temporary directory, verified by a test that snapshots a fixture checkout and its target dir before and after `repo add`, `status`, and `doctor` and finds them byte-for-byte unchanged.
- **Security:** **CORE-6.4** WHEN doctor checks device-owner authentication THE SYSTEM SHALL only query its availability and SHALL NOT present an authentication dialog, verified by running doctor under a test harness with no GUI session and observing that it completes without blocking.
- **Reliability:** **CORE-6.5** IF a hive command is interrupted while writing its registry, state, or receipts log THEN THE SYSTEM SHALL leave each of those files either in its previous complete form or in its new complete form, verified by a kill-during-write test that parses every file afterwards.
- **Accessibility:** None — headless CLI and JSON output; no UI a person perceives beyond terminal text.

## 7. Existing behavior this feature must not change

Files and surfaces this change touches:
- New source tree for the `cargo-hive` crate (`src/`, `Cargo.toml`): no behavior to guard (new).
- This repo's existing docs and scripts (`RESEARCH.md`, `SPIKES.md`, `CONTEXT.md`, `bench/`, `spikes/`, `evidence/`, `docs/roadmap/`): no behavior to guard (not read or written by the program).
- Registered repositories' checkouts and their Cargo target dirs (read by `repo add` and `status`): existing behavior at risk, guarded below.
- `~/.cargo` and the Rust toolchains (read for the active toolchain's identity): existing behavior at risk, guarded below.

- **CORE-7.1** (guard) WHEN `cargo hive repo add`, `cargo hive status`, or `cargo hive doctor` runs THE SYSTEM SHALL CONTINUE TO leave every registered checkout, every Cargo target dir, and `~/.cargo` with no file created, modified, or deleted.
- **CORE-7.2** (guard) WHEN hive is installed and a repository is registered THE SYSTEM SHALL CONTINUE TO let `cargo` and rust-analyzer build every checkout exactly as without hive, with no Cargo configuration, environment variable, wrapper, or hook added by hive.

## Out of Scope

- Deleting anything in a checkout or target dir: the auto tier is ROAD-2; plan, apply and orphan deletion are ROAD-6.
- Closure liveness (window roots and graph traversal for deletion decisions): ROAD-5. This feature only reads and resolves the fingerprint graph (CORE-4.8).
- The store, filling, compression of real artifacts, and seeding: ROAD-8 and ROAD-9. Doctor compresses only its own temporary test file.
- Showing an owner-authentication dialog and `cargo hive approve`: ROAD-7. Doctor only queries availability.
- The MCP server: ROAD-4. Findings and `cargo hive explain`: ROAD-3. `cargo hive bench`: ROAD-11.
- Unregistering a repository through a command. Editing the registry file by hand is the v1 path.
- Attributing an orphan candidate under a cache root to a specific repository.
- Any platform other than macOS. A jobserver. Rustc wrappers, `cargo` shims, or Claude Code hooks (close-package A1).
- Editing any file of a registered repository.

## Open Questions

Owned unknowns carried from the close package (`.skills/CORE/close-package.md`):

- ~~How CORE-4.4 obtains the target directory Cargo resolves~~ — answered by design.md Decision 4: run `cargo metadata --no-deps` with `CARGO_TARGET_DIR`, `CARGO_BUILD_TARGET_DIR` and `CARGO_BUILD_BUILD_DIR` scrubbed, and note any ignored variables in the receipt. It never compiles, and GC liveness stays disk-only.
- Attributing `~/.cache` orphan targets to a repo via paths in `.d` files — project owner — 2026-10-25 — forbid-guess (cấm đoán).
- What signal would bring the jobserver back while hive cannot see build durations — project owner — 2026-10-25 — forbid-guess (cấm đoán).
- Builds hive can't see (background runs, `cargo watch`, rust-analyzer) — project owner — 2026-10-25 — forbid-guess (cấm đoán).
- Closure-scan cost and steady-state reclaim at a 7-day window — project owner — 2026-10-25 — forbid-guess (cấm đoán); ROAD-5.
- How `hive bench` samples cold versus seeded, and when the "after first build" receipt is computed — project owner — 2026-10-25 — forbid-guess (cấm đoán); ROAD-9 and ROAD-11.
- The MCP server's cwd — project owner — 2026-10-25 — forbid-guess (cấm đoán); ROAD-4.
- TTY behaviour inside Codex, and the argv of Node-based agent CLIs — project owner — 2026-10-25 — forbid-guess (cấm đoán); ROAD-7.
- Whether to warn when a secondary root contains git checkouts of no registered repo — project owner — 2026-10-25 — forbid-guess (cấm đoán).
