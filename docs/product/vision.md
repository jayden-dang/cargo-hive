# Product Vision: cargo-hive

Status: Approved
Date: 2026-10-31

<!--
The repo-level north star: WHAT this project is and for whom, above any single
feature. `frame-change` reads it to check a new idea's scope; it never gates.
Every heading is a REQUIRED slot. Source: the define-project interview close
package confirmed 2026-10-28 (.skills/_pending-define-project/close-package.md).
GOAL-N IDs are permanent: never renumbered, never reused; retire one by
striking it through with a reason.
-->

## Problem

**Observed.** Every new agent worktree pays a cold first build: median 37 s, p90 119 s across 69 new checkouts in 27 days (`SPIKES.md`, evidence correction). Each one also holds tens of GB of build output, and nothing reclaims superseded state: about 60 GB of old incremental sessions across checkouts, and 41.7 GB of the 69 GB in klynt's main target was unreachable (`SPIKES.md` §8). Background: about 185 GB of Rust build output across checkouts on the author's machine (`RESEARCH.md` §1).

**Desired.** Dead bytes are reclaimed with zero deletion-caused rebuilds, and a new secondary checkout's first build recompiles zero seeded registry units at about zero private bytes, all shown in receipts. "Warm" covers registry units only: workspace crates still compile cold (`SPIKES.md` §6, §7).

Source: the problem lock in `.skills/CORE/close-package.md` (confirmed 2026-10-25), with its Desired line narrowed to this wording on 2026-10-28.

## Users

- **Developer.** A Rust developer on a Mac who runs coding agents across many worktrees. Registers repos, reads status, and approves plans for protected checkouts.
- **Agent.** A coding agent (Claude Code, Codex, or another CLI) driving hive through the CLI with `--json`, guided by the hive skill. It never gets an owner-authentication dialog, applies plans for secondary checkouts only, and takes its scope from arguments, never from the current directory.

## Goals

Each goal holds on a machine where `cargo hive doctor` passes. None promises klynt's numbers elsewhere.

- ~~**GOAL-1** Every deletion hive makes causes 0 unit rebuilds, and reported freed bytes never exceed what the volume regained.~~ Superseded by GOAL-6 (2026-10-31): a command that stays fully fresh for the whole GC window leaves no trace on disk, so a unit hive collects can rebuild when that command next needs it (`SPIKES.md` §9; the closure-liveness spec, `docs/specs/2026-10-30-closure-liveness/`).
- **GOAL-2** A fresh seeded secondary checkout's first build recompiles 0 seeded units.
- **GOAL-3** Seeding k secondary checkouts adds about 0 private bytes to their registry part.
- **GOAL-4** Every number hive reports, in receipts and in explain findings, is measured on this machine, never estimated; seconds come only from `cargo hive bench`.
- **GOAL-5** An agent reclaims and seeds secondary checkouts with no human in the loop; a plan-tier change on a protected checkout returns `needs-human` until the owner approves it.
- **GOAL-6** Hive deletes no file of a unit that is live when the deletion is decided (reachable over the fingerprint graph from a unit compiled inside the GC window), so no live unit rebuilds because of a deletion; a unit hive collected rebuilds when a command needs it again. Reported freed bytes never exceed what the volume regained.

Receipts are also the data Cargo lacks for a default target-dir GC policy (cargo#13136; `RESEARCH.md` §6).

## Non-goals

- Changes to rustc or Cargo.
- "Relink, don't rebuild".
- Distributed or remote caching.
- CI and Linux.
- A jobserver.
- Slot mode.
- Editing repo files.
- Replacing artifacts in place.
- Claude Code hooks.
- Time claims outside `cargo hive bench`.
- Claiming that main is protected from agents.
- Build-on-host / jayden-host.

## Scope boundaries

**Now.** MILE-1 is committed (ROAD-1 to ROAD-3 and ROAD-12; ROAD-4, the MCP server, dropped 2026-10-29). MILE-2 and MILE-3 are planned (`docs/roadmap/INDEX.md`).

**Deferred.**
- Slot mode, until `-p` and `--workspace` resolve the same feature set.
- A per-checkout "protect" marker.
- Cargo's new build-dir layout: reported, never acted on.

**Hard constraints.**
- macOS only. The declared floor is macOS 27; older versions are supported where `cargo hive doctor` passes.
- Install path: `cargo install cargo-hive`, then `cargo hive doctor`, then `cargo hive repo add <main>`. For agents, install the hive skill as a plugin from this repo's marketplace (`/plugin marketplace add jayden-dang/cargo-hive`, then `/plugin install cargo-hive@cargo-hive`; Codex has the same pair) (ROAD-12); there is no MCP server. `cargo-hive <sub>` is the direct form of `cargo hive <sub>`. Until the crate is published, `cargo install --path .` (README) stands in for the first step.
- Building needs a Rust toolchain and the Xcode Command Line Tools; nothing else at runtime. Applesauce is embedded, LocalAuthentication is reached through objc bindings, and afsctool is never used.
- Zero configuration works. Defaults: secondary roots `/private/tmp` and `$TMPDIR`, cache root `~/.cache/cargo-target`, GC window 7 days, store budget 40 GB, store max age 30 days.
- `cargo hive doctor` runs the mechanism checks on each machine and disables every capability that fails.
- Foreign mode only: every checkout builds at its own path with stock Cargo.
- A toolchain is supported when its fingerprint files match a known format signature; any other is reported as unsupported (ADR-0001).
