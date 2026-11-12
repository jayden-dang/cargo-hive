# Assessments: MILE-2 — Planned reclaim

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
**Committed baseline:** cdf1d6132dd7249c3a509741d9371009300ba809
**Candidate closing revision:** cdf1d6132dd7249c3a509741d9371009300ba809
**Roadmap revision assessed:** cdf1d6132dd7249c3a509741d9371009300ba809 (working tree: clean)
**Assessed:** 2026-11-12

### Agent assessment

**Outcome verdict:** achieved
**Evidence:** Both members are bound and Shipped in v0.1.0: ROAD-5 → LIVE, ROAD-6 → GCPLAN (docs/specs/catalog/hive.md). A user or agent runs `cargo hive gc --plan`, which writes one plan per checkout class (secondary, protected, orphan) of the dead units that closure liveness finds, plus whole orphan target dirs, and deletes nothing; `cargo hive apply <id>` then applies one plan with no owner step. Apply takes every Cargo lock of the target without waiting and skips a held target (plan_tier `a_lock_held_elsewhere_skips_the_target_for_every_lock_file_name`, `a_held_lock_skips_the_orphan_removal_and_deletes_nothing`), and rechecks liveness before deleting, refusing a plan a command made live again (`a_planned_unit_that_a_command_made_live_again_refuses_the_whole_plan`). An agent applies a protected plan itself (`an_agent_applies_a_protected_plan_itself_and_the_plan_is_consumed`), and the hive skill teaches plans of every class with no owner step. On klynt (2026-11-11) an orphan plan removed the old `~/.cache/cargo-target/klynt`: hive reported 20,342,845,440 B freed and the volume regained 20,484,304,896 B.
**Goal coverage:** GOAL-6 advanced — liveness from the fingerprint graph with Undecided failing closed, proof runs 2–4 passing (docs/specs/2026-10-30-closure-liveness/proof-runs.md), and reported freed bytes not above the volume's gain on the klynt orphan apply. GOAL-7 advanced — agents apply secondary, protected and orphan plans with no human step (GCPLAN, ADR-0002, the hive skill).
**Deferrals:** ROAD-7 owner-approval → no milestone (2026-11-02, a target-dir deletion costs at most a rebuild, so the owner step added work without protection; ADR-0002) — honest as a drop with its reason, but it sits in the `Deferred:` slot with no destination milestone.
**Plan accuracy:** +0 added · 0 moved out · 0 deferred · 0 days (MILE-2 was committed in the candidate revision itself, after both members shipped, so baseline and candidate are the same commit and these figures carry no information about the plan)
**Findings:** (1) On klynt, closure deletion of dead units inside a live target was not exercised end to end: `gc --plan` found no dead units there (every unit was inside the 7-day window), so only the orphan plan ran; closure deletion rests on the plan_tier tests and the LIVE proof runs → plan-milestones (state the reading in the close). (2) ROAD-7 is a drop recorded in the `Deferred:` slot without a destination milestone → plan-milestones. (3) The klynt run hit a P2: a registered repo whose directory was deleted made `apply` refuse with `walk.unreadable`; `repo remove --missing` shipped in v0.1.0 as the way out → none, already fixed.
**Rationale:** Each clause of the outcome maps to a Shipped member with tests behind it: preview as a plan (GCPLAN `gc --plan`), closure-based garbage (LIVE), orphan target dirs (GCPLAN orphan plans, exercised on klynt), applied by an agent on any checkout with no owner step (GCPLAN, ADR-0002, the skill), rechecked under locks before deleting (plan_tier lock and liveness-recheck tests). The verdict rests on tests and one real orphan apply; closure deletion on a real target with dead units has not been run since the LIVE proof runs.

### Human disposition

**Current:** Accepted
**Close decision:** Close
**History:**
- 2026-11-12 Pending
- 2026-11-12 Accepted / Close
