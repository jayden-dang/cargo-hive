# Assessments: MILE-4 — Upgrades keep hive's state

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
**Committed baseline:** 71120a10b9e041d72abcc2a3f3994ca29c962584
**Candidate closing revision:** d24cd2a1bf443612b59b4baa60d94f494fba23ed
**Roadmap revision assessed:** f086da731c2d64f926464df9d0403a7fb2cb5c74 (working tree: clean)
**Assessed:** 2026-11-15

### Agent assessment

**Outcome verdict:** achieved
**Evidence:** Both members are bound (docs/specs/catalog/hive.md): ROAD-14 → UPGRADE (Implemented, PR #28 merged 2026-11-15 as 1770d2e, 41/41 task steps checked) and ROAD-15 → SPLIT (PR #27 merged 2026-11-14, published as v0.1.1; 110/110 task steps checked, spec Status still In-progress). On main, every state file hive writes (registry, capabilities, snapshot, plans, receipts, `store.db`, seeds) records its layout. Committed fixture homes written by the v0.1.0 and v0.1.1 binaries read back with the values each release wrote (`each_state_file_reads_the_values_its_release_wrote`, `appending_a_receipt_keeps_every_earlier_line_and_each_parses`). A damaged file, or one from a newer hive, stops the command, names the file and its recovery, and is left untouched (`status_stops_on_each_damaged_state_file_and_leaves_it_untouched`, `gc_plan_stops_on_a_damaged_plans_file_and_leaves_it_untouched`, the refusal matrix). Bare gc no longer empties the store when `store.db` is missing (tests/store_gc.rs). Run fresh at the candidate: `cargo test -p cargo-hive --test upgrade --test store_gc`, 48 + 9 passed, 0 failed.
**Goal coverage:** GOAL-8 advanced — UPGRADE reads each earlier file or refuses it by name with its fixing command, and never treats an unreadable file as empty; SPLIT kept 0.1.0's behaviour (capture comparison against 0.1.0 at release) and added the v0.1.1 fixture set.
**Deferrals:** None
**Plan accuracy:** +1 added · 0 moved out · 0 deferred · 2 days (ROAD-15 joined after the commitment)
**Findings:** (1) No release carries UPGRADE yet, so no user can upgrade to it today; neither member's spec reads Shipped, so closing MILE-4 now would raise R9 until a release stamps them → plan-milestones (hold the close until the release, or record the close knowing R9 fires). (2) SPLIT's spec still reads In-progress although all tasks are checked and it shipped in v0.1.1 → realign-spec (outside this skill's routing table; named for the operator). (3) ADR-0003 (ROAD-15) and ADR-0004 (ROAD-14) are still Proposed → define-domain. (4) Issue #26 (SPLIT) is still open on the tracker although PR #27 merged → none (tracker upkeep).
**Rationale:** Each clause of the outcome maps to UPGRADE with tests behind it: keeps repos, plans, receipts and store entries an earlier release wrote (fixture suite over v0.1.0 and v0.1.1 homes), or stops with the file named and the command that fixes it (refusal matrix, UPGRADE-3.8), and never starts empty (store_gc missing-index test, seed refusing to replace an unreadable record). The verdict is about the code on main; the outcome reaches users only when a release ships it, which finding 1 records.

### Human disposition

**Current:** Accepted
**Close decision:** Hold
**History:**
- 2026-11-15 Pending
- 2026-11-15 Accepted / Hold — "Accept, hold for release"
