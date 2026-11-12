# cargo-hive

**Manage Rust build artifacts across many worktrees on macOS: reclaim dead bytes safely,
seed new checkouts with already-built dependencies, and report only what was measured.**

When several coding agents (Claude Code, Codex, or anything else) work on one Rust
project in parallel Git worktrees, every worktree grows its own `target/`. Each new
checkout recompiles the same registry dependencies from scratch, and old incremental
sessions pile up until the disk is full. The usual fix, `cargo clean`, throws away the
useful state along with the dead state.

cargo-hive works next to Cargo. It doesn't replace it. It finds the build output that no
build can reach anymore and deletes it under Cargo's and rustc's own locks. It can also
clone registry dependencies that are already built into a fresh worktree before that
worktree's first build. It installs no wrapper, shim, hook or Cargo config, so
`cargo build` and rust-analyzer behave exactly as they would without it.

> **Status:** early (`0.1.0`), macOS only, not yet published on crates.io. Every
> capability is gated on `cargo hive doctor` passing for your toolchain.

## Contents

- [Why](#why)
- [What it does and doesn't do](#what-it-does-and-doesnt-do)
- [Measured results](#measured-results)
- [Install](#install)
- [Quick start](#quick-start)
- [Commands](#commands)
- [How it works](#how-it-works)
- [Safety guarantees](#safety-guarantees)
- [Using it from coding agents](#using-it-from-coding-agents)
- [Configuration](#configuration)
- [Uninstall](#uninstall)
- [Project docs and contributing](#project-docs-and-contributing)
- [License](#license)

## Why

These numbers come from the author's machine, where agents work in many worktrees of one
large Rust workspace ([`SPIKES.md`](SPIKES.md), [`RESEARCH.md`](RESEARCH.md)):

- Over 27 days, 69 new checkouts each paid a cold first build: median 37 s, p90 119 s.
- In one main `target/`, 41.7 GB was unreachable from any command still being run,
  against 31.4 GB that was live.
- About 60 GB of superseded incremental sessions sat across checkouts, with nothing
  reclaiming them.

Cargo has no default GC policy for target dirs yet
([cargo#13136](https://github.com/rust-lang/cargo/issues/13136)). cargo-hive fills that
gap for the multi-worktree case. Its receipts record the data such a policy would need.

```
Without hive                              With hive

main/  target/  grows, never pruned       main/  target/  dead sessions and units reclaimed
wt-a/  target/  every dep built cold      wt-a/  target/  registry deps cloned in first;
wt-b/  target/  every dep built cold      wt-b/  target/  only workspace crates compile
...    stale incremental sessions         ~/.hive/store/  one copy of each registry unit,
       pile up                                            shared by APFS clones
```

Each worktree still has its own `target/`. Hive doesn't turn them into one shared build
directory.

## What it does and doesn't do

| Problem | Handled by hive? |
|---|---|
| Show where build output lives, how big it is, and which checkout owns it | Yes: `status` |
| Delete superseded incremental sessions and leftover rustc temp dirs | Yes: `gc` |
| Delete dead units, and whole orphan target dirs that no checkout owns | Yes: `gc --plan`, then `apply` |
| Seed registry dependencies into a fresh worktree before its first build | Yes: `seed` |
| Explain why a crate is built in many copies (feature splits, failed units) | Yes: `explain` |
| Time cold against seeded first builds on your machine | Yes: `bench` |
| Make `rustc` itself faster (type checking, LLVM, linking) | No |
| Skip rebuilds of workspace crates after a change | No. Workspace crates still compile cold in a seeded checkout |
| Share every build output between agents automatically | No |
| Distributed or remote caching | No |
| Linux, CI, or non-APFS volumes | No |

Hive manages the lifecycle of build artifacts. It doesn't optimize the compiler. If your
project takes minutes to rebuild after you touch a foundational crate, hive won't change
that.

## Measured results

The table comes from one `cargo hive bench` receipt from 2026-11-11, with two fresh
checkouts per arm. The repo is the author's workspace, which had 597 packages, 42 of them
workspace crates, when [`SPIKES.md`](SPIKES.md) was written. The machine was an M5 Pro
with 18 cores, running macOS 27.0.1 and Rust 1.97.1.

| | Cold first build | Seeded (seed + first build) |
|---|---|---|
| Median wall time | 86,943 ms | 73,551 ms (medians: 4,893 ms seed, 68,657 ms build) |
| Registry units recompiled | all | 0 of 596 seeded |

In three more seeded worktrees that day, each `seed` receipt reported
`clone_bytes_not_shared: 0` for 596 cloned units: every seeded file shared its blocks
with the store.

An earlier run, on a build from before the last seed fix, tested `gc` in a worktree where
builds had failed and been interrupted. It deleted 31 superseded sessions (1,223 MiB).
The next build compiled the same 28 crates as a control run that skipped the `gc`.

This is one repo on one machine, with only two samples per arm. It promises nothing about
yours. Run `cargo hive bench` to measure your own.

## Install

Requirements:

- macOS on an APFS volume. The declared floor is macOS 27; older versions work where
  `cargo hive doctor` passes
- a Rust toolchain (rustup)
- the Xcode Command Line Tools (`xcode-select --install`), which also build the C parts
  of the embedded [Applesauce](https://crates.io/crates/applesauce) library

```sh
git clone https://github.com/jayden-dang/cargo-hive
cd cargo-hive
cargo install --path .
cargo hive doctor    # checks what works on this machine; run it again after a toolchain change
```

`cargo-hive <sub>` is the direct form of `cargo hive <sub>`.

## Quick start

```sh
# 1. Register a repo by any path inside its main checkout
cargo hive repo add ~/code/my-repo

# 2. See every checkout and target dir, with sizes and owners
cargo hive status

# 3. Find out why some crates are built in many copies
cargo hive explain

# 4. Reclaim superseded incremental sessions and rustc temp dirs (preview first)
cargo hive gc --dry-run
cargo hive gc

# 5. Plan deeper cleanup (dead units, orphan targets), review it, then apply it
cargo hive gc --plan
cargo hive apply <id>

# 6. Seed a new worktree before its first build
git -C ~/code/my-repo worktree add .worktrees/wt-a
cargo hive seed ~/code/my-repo/.worktrees/wt-a
cargo build --manifest-path ~/code/my-repo/.worktrees/wt-a/Cargo.toml
```

`seed` only fills a *secondary* checkout: a linked worktree that sits inside its main
checkout, as above, or a checkout under `/private/tmp` or `$TMPDIR`. A sibling worktree
such as `~/code/my-repo-wt-a` counts as protected, and `seed` refuses it. To change that,
list its parent in `secondary_roots`, together with any default roots you want to keep
(see [Configuration](#configuration)).

Every command accepts `--json`. With it, a command prints exactly one receipt on stdout.

## Commands

| Command | What it does |
|---|---|
| `cargo hive doctor` | Runs checks (a)–(d), (f), and (g) in a temporary directory and records which capabilities are enabled for the active toolchain. There is no check (e). A failure of check (g) disables failed-unit reporting only |
| `cargo hive repo add <path>` | Registers the main checkout containing `<path>` |
| `cargo hive repo list` | Lists registered repositories and their root commits |
| `cargo hive repo remove [<path>…] [--missing]` | Unregisters repositories. `--missing` also removes every repository whose main checkout no longer exists |
| `cargo hive status [<repo>…] [--no-sizes]` | Lists checkouts (protected, secondary, missing) and Cargo target dirs (owner or orphan candidate, size, units and edges) |
| `cargo hive gc [<repo>…] [--dry-run]` | Deletes superseded incremental sessions and `deps/rustc*`/`rmeta*` temp dirs. Freed bytes are counted per inode and reported as totals. `--dry-run` deletes nothing. With no repo argument it also evicts from the store |
| `cargo hive gc --plan [<repo>…]` | Writes a plan and deletes nothing |
| `cargo hive apply <id>` | Deletes the plan's paths for every class |
| `cargo hive explain [<repo>…] [-n N]` | Reports copies of one crate, feature splits, and units whose last compile failed. `-n` defaults to 10; `-n 0` shows every group |
| `cargo hive seed <checkout>` | Copies registry output into a new secondary checkout before its first build. A checkout that already has a target gets nothing. Bare `cargo hive gc` keeps the store under 40 GB |
| `cargo hive bench --cold <checkout>… --seeded <checkout>… [--build-cmd <cmd>]` | Times cold and seeded first builds of fresh secondary checkouts and reports integer milliseconds in a receipt. `--build-cmd` sets the compile-only cargo command (default `build`) |

Run `cargo hive <command> --help` for every flag.

### Receipts

Every receipt is also appended to `~/.hive/receipts/<date>.jsonl`. Receipts carry counts
and measured values only, never an estimate of time saved. Durations appear only in
`cargo hive bench`, as integer milliseconds. You can feed them to dashboards or agent
automation.

### explain

`cargo hive explain` reports copies of one crate, feature splits, and units whose last
compile failed. It never claims bytes, durations, or a tool name. It never says that a
unit always fails, or that a copy is an older build. `-n` (`--top`) defaults to 10 groups;
`-n 0` shows every group. Naming a failed unit needs doctor check (g). Without it, the
count stays unmeasured and no unit is named.

### gc

`cargo hive gc` deletes superseded incremental sessions and leftover rustc temp dirs
inside Cargo target dirs:

- a session that has a lock file is deleted under that lock; a session with no lock file is deleted without one;
- temp entries are deleted under `.cargo-build-lock`;
- held entries are skipped;
- output kept with `-C save-temps` matches the temp-entry rule and is reclaimed;
- targets off APFS are skipped.

Bare `cargo hive gc`, with no repo argument, also evicts store entries that are older
than 30 days, or that belong to a toolchain no checkout uses anymore, and it keeps the
store under its 40 GB budget. That budget covers hive's store only. Your `target/` dirs
are not counted in it.

`cargo hive gc --plan` goes further. It plans to delete units that are no longer live,
plus whole orphan target dirs, and writes the plan without deleting anything.
`cargo hive apply <id>` applies one plan. Before deleting, it rechecks liveness, locks
and the fingerprint format, and it refuses a stale plan.

## How it works

- **Checkouts.** Hive classifies each checkout. A *protected* checkout is a registered
  main checkout, or anything hive cannot classify. A *secondary* checkout is a linked
  worktree of a registered main, or a checkout under a secondary root (by default
  `/private/tmp` and `$TMPDIR`). A target dir that no known checkout owns is reported
  as an *orphan candidate*.
- **Liveness.** Hive reads Cargo's fingerprint graph from disk. A unit is live when a
  unit compiled inside the GC window (7 days by default) reaches it through that graph.
  Hive never compiles or replays a build to decide what is live. It reads the
  fingerprint files only when their exact format is known
  ([ADR-0001](docs/adr/0001-read-cargo-internal-fingerprints.md)), and reports any
  other format as unsupported.
- **Store and seed.** `seed` first copies the live registry units of a registered repo's
  targets into a store under `~/.hive/`. Then it clones the units for the new checkout
  out of the store with APFS copy-on-write clones, so the seeded files share disk blocks
  with the store. Every unit it places comes from one source target, so all of them come
  from one consistent build. Only registry dependencies are seeded. Your workspace crates
  compile normally on the first build.
- **Locks.** Where the Cargo or rustc entry being deleted has a lock, hive tries that
  lock without waiting. A held entry is skipped, never waited on.

The design is recorded as numbered invariants in
[`docs/architecture/INDEX.md`](docs/architecture/INDEX.md).

## Safety guarantees

- It installs no Cargo configuration, environment variable, rustc wrapper, cargo shim or
  hook. `cargo` and rust-analyzer build exactly as without hive.
- It writes under `~/.hive/` and doctor's temporary directory, and `seed` also
  places files into a new Cargo target dir. Apart from that placement, it deletes
  only inside Cargo target dirs, through `gc` and `apply`, and inside its own store
  under `~/.hive/`, through bare `gc`. It never writes a repo file or `~/.cargo`.
- It never replaces a build artifact in place. Cargo can't detect a swapped file, but it
  always notices a deleted one.
- It never deletes a file of a unit that is live when the deletion is decided. A unit
  hive collected rebuilds the next time a command needs it.
- Reported freed bytes never exceed what the volume regained. Bytes still held by a
  surviving hardlink or clone are counted separately, never as freed.
- It takes the repos a command acts on from that command's arguments or from a plan,
  never from the current directory.

## Using it from coding agents

Agents drive hive through the CLI with `--json`. An optional skill teaches Claude Code
and Codex to use `cargo hive` instead of `cargo clean` or deleting `target/`. Agents
apply plans of every class themselves, with no human approval step
([ADR-0002](docs/adr/0002-plans-apply-with-no-human-step.md)).

In Claude Code, inside a session:

```
/plugin marketplace add jayden-dang/cargo-hive
/plugin install cargo-hive@cargo-hive
```

In Codex, from a shell:

```sh
codex plugin marketplace add jayden-dang/cargo-hive
codex plugin add cargo-hive@cargo-hive
```

## Configuration

None is required. `~/.hive/config.json` may set:

| Key | Default | Meaning |
|---|---|---|
| `secondary_roots` | `/private/tmp`, `$TMPDIR` | Checkouts under these roots count as secondary |
| `cache_roots` | `~/.cache/cargo-target` | Where shared `CARGO_TARGET_DIR`s are looked for |
| `gc_window_days` | `7` | A whole number of days from 1 to 365. A unit compiled within the window is live. `gc --plan` records the window, and `apply` uses that recorded window |

A key you set replaces its default list. `HIVE_HOME` relocates the state directory. It
exists for tests only.

## Uninstall

```sh
cargo uninstall cargo-hive && rm -rf ~/.hive
```

To remove the agent skill, run `/plugin uninstall cargo-hive` in Claude Code, or
`codex plugin remove cargo-hive` in Codex.

## Project docs and contributing

- [`docs/product/vision.md`](docs/product/vision.md): problem, users, goals, non-goals
- [`docs/architecture/INDEX.md`](docs/architecture/INDEX.md): the invariants every
  change must keep
- [`docs/adr/`](docs/adr/): decision records
- [`docs/roadmap/INDEX.md`](docs/roadmap/INDEX.md): milestones
- [`SPIKES.md`](SPIKES.md): the experiments the design rests on
- [`docs/standards/`](docs/standards/): code, error and testing standards

Before you open a pull request, run:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo check --all-targets
cargo test
```

The integration tests in `tests/` drive the real binary and need macOS.

## License

Licensed under either of MIT or Apache-2.0, at your option.
