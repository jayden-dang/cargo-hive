# Assessments: MILE-3 — Warm worktrees from a shared store

<!--
Outcome truth for one milestone, written only by `assess-milestone` to
docs/roadmap/assessments/<MILE-N>.md. One file per milestone. Optional — a project
without a roadmap layer never has one.

This file records whether the milestone DELIVERED. docs/roadmap/INDEX.md keeps recording
what was INTENDED, and is written only by `plan-milestones`. Neither writes into the other.

Structural rules — AUTHORITATIVE. `assess-milestone` validates against this list before it
writes, and `plan-milestones` validates against it before it records a close:

  A1  every `## Assessment <N>` ordinal is unique and blocks appear in ascending order
  A2  `Committed baseline` and `Candidate closing revision` are full 40-hex SHAs
  A3  exactly one `### Agent assessment` and one `### Human disposition` per block
  A4  `Current:` holds exactly one of Pending, Deferred, Accepted, Overridden
  A5  a terminal `Current:` is accompanied by a `Close decision:` of Close or Hold
  A6  every block after the first carries `Supersedes:` naming the prior ordinal
  A7  every block earlier than the last is byte-identical to when it was written

Append-only at the assessment-event level. A further block is appended ONLY when the
requested closing revision differs from the recorded candidate, or when material evidence
changed. A human disposition arriving later is neither — it completes the block already
recorded, by appending to that block's dated History.

Every heading below is a REQUIRED slot — fill it or write `None`.
-->

## Assessment 1

**Supersedes:** None — first assessment of this milestone
**Committed baseline:** b09606d854017ee69c4c88d0a4df8c40ca218dc3
**Candidate closing revision:** f6ea6b65e5609cf011e294eb5abd644e01435313
**Roadmap revision assessed:** b09606d854017ee69c4c88d0a4df8c40ca218dc3 (working tree: clean)
**Assessed:** 2026-11-17

### Agent assessment

**Outcome verdict:** not achieved
**Evidence:** Every member is bound and Shipped (docs/specs/catalog/hive.md):
- ROAD-8 → SEED: v0.1.0.
- ROAD-10 → STOREGC: v0.1.0.
- ROAD-11 → BENCH: v0.1.0.
- ROAD-13 → SKILLPASS: in the v0.1.1 code, stamped Shipped in v0.1.2.

A new secondary checkout is seeded from a hive-owned store by `cargo hive seed`. On klynt (2026-11-11, after fix e5f6a8e):
- the first build recompiled 0 registry units and reused 596 fresh;
- one seed cost about 2 MiB of private bytes;
- `cargo hive bench` measured cold 86.9 s against seed+build 73.6 s (−15%).

**One clause is not delivered: the store is not compressed.** The owner decided on 2026-11-11, after a root cause, to drop compression: rustc rejects an rmeta that keeps the compressed-file flag, so every clone from a compressed store failed (SEED-2.9, SEED-2.10, SEED-2.22, SEED-2.23 superseded by SEED-2.28 to SEED-2.30). The outcome sentence still says "compressed".
**Goal coverage:**
- GOAL-2 advanced: 0 seeded registry units recompiled on klynt.
- GOAL-3 advanced: about 2 MiB private per seed, through APFS clones.
- GOAL-4 advanced: bench durations are measured integer milliseconds; every other count is measured.
- GOAL-7 advanced: agents seed secondary checkouts through the skill (SKILLPASS) with no human step.
**Deferrals:** ROAD-9 seed-secondary-checkouts → merged into ROAD-8 within MILE-3 (2026-11-02: a filled store gives nothing until something clones from it). Honest, with a destination.
**Plan accuracy:** +0 added · 0 moved out · 0 deferred · 0 days. MILE-3 was committed on 2026-11-17, after all its members shipped, so baseline and candidate are one day of merges apart and these figures carry no information about the plan.
**Findings:**
- (1) The outcome sentence promises a "compressed" store that the owner deliberately dropped on 2026-11-11 → plan-milestones (state the reading in the close).
- (2) Project docs still describe the store as compressed (#21, open) → frame-change.
**Rationale:**
- Each measurable clause holds on klynt with numbers: zero recompiles, about zero private bytes, and a measured bench.
- The verdict is negative only because of the word "compressed": a delivery choice, reversed for a proven cause, that the outcome sentence was never amended to drop.
- No further member work can change that. Closing with this verdict records what was delivered.

### Human disposition

**Current:** Accepted
**Close decision:** Close
**History:**
- 2026-11-17 Pending
- 2026-11-17 Accepted / Close — "Accept + Close (Recommended)"
