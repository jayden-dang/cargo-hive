# Assessments: MILE-1 — Safe automatic reclaim

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
**Committed baseline:** 54d066ba2088952a7da6d89307b906ad7c869b4c
**Candidate closing revision:** 0043374faf1952b06f12c88f632427ebbb46c040
**Roadmap revision assessed:** 454b0c9d8cef66341d53a58405ff3920f85b6a81 (working tree: clean)
**Assessed:** 2026-10-30

### Agent assessment

**Outcome verdict:** achieved
**Evidence:** Every member is bound and Implemented: ROAD-1 → CORE, ROAD-2 → AUTOGC, ROAD-3 → EXPLAIN, ROAD-12 → SKILL (docs/specs/catalog/hive.md). A user installs hive from source (`cargo install --path .`, the stand-in the vision names until the crate is published), runs `cargo hive doctor`, registers a repo with `cargo hive repo add` (CORE), and runs `cargo hive gc` across every checkout of that repo, which deletes superseded incremental sessions and leftover rustc temp entries under their owners' locks (AUTOGC stories 2–4). The receipt carries `freed_bytes` counted per deleted inode, each inode once, with still-shared bytes counted apart (AUTOGC-5.1, 5.2), and a test proves a following `cargo build` compiles 0 units (AUTOGC-6.2). The same operations run from an agent through the hive skill, installed as a plugin from this repo's marketplace (SKILL): recorded GREEN runs on Sonnet and Haiku freed space with `cargo hive gc <repo> --json` and reported the receipt's counters, and the owner installed the plugin in their own Claude Code session on 2026-10-30 (skills/hive/TESTS.md).
**Goal coverage:** GOAL-1 advanced — AUTOGC-6.2 (0 rebuilds after gc) and AUTOGC-6.4 (`freed_bytes` never above the volume's gain, on a test-created APFS image). GOAL-4 advanced — receipts and explain findings hold only measured counters (ARCH-10; CORE receipts, AUTOGC-5.x, EXPLAIN measured keys). GOAL-5 advanced in part — an agent reclaims through the skill with no human in the loop (SKILL GREEN runs); seeding and the `needs-human` plan tier belong to MILE-2 and MILE-3.
**Deferrals:** ROAD-4 mcp-server → no milestone (2026-10-29, "hive is a local CLI with no server state … Replaced by ROAD-12") — honest as a drop with its replacement named, but it sits in the `Deferred:` slot with no destination milestone.
**Plan accuracy:** +1 added (ROAD-12) · 0 moved out · 0 deferred (ROAD-4 left Members and was re-listed struck through in this milestone's own `Deferred:` slot, which the fixed rules count as neither) · 2 days 16 hours (baseline 2026-10-27 18:26 +0700 → candidate 2026-10-30 10:38 +0700)
**Findings:** (1) The outcome says "a receipt showing per-file bytes freed"; the receipt shows `freed_bytes` and `deleted_bytes_still_shared` totals computed per deleted inode (AUTOGC-5.1, the D7 per-file accounting ROAD-2 names), not a per-file list → plan-milestones (state the reading in the close, or reword the outcome). (2) "On any Mac … installs cargo-hive from scratch" rests on one machine (the author's macOS 27, every capability gated by `doctor`) and on a source install, since the crate is unpublished (release steps in docs/agents/project.md still need the license files) → plan-milestones. (3) ROAD-4 is a drop recorded in the `Deferred:` slot without a destination milestone → plan-milestones.
**Rationale:** Each clause of the outcome maps to an Implemented member with a test or a recorded run behind it: install and register (CORE), reclaim across checkouts under locks (AUTOGC), measured receipt and zero rebuilds (AUTOGC-5.1, 6.2, 6.4), CLI and agent paths (SKILL). Read "per-file bytes freed" as the per-file freed-byte accounting the roadmap gives ROAD-2 (D7), which AUTOGC delivers as per-inode totals; read literally as a per-file listing, the receipt does not provide it. "Any Mac" and "from scratch" hold as designed (doctor-gated capabilities, source install) but were exercised on one machine without a published crate. None of these is a missing member; they are wording and breadth questions for the close.

### Human disposition

**Current:** Accepted
**Close decision:** Close
**History:**
- 2026-10-30 Pending
- 2026-10-30 Accepted / Close — "accept + close"
