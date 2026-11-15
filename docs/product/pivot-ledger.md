# Pivot ledger: warm agent worktrees (MILE-5)

Date: 2026-11-15
Status: Confirmed 2026-11-15 — every row confirmed by the owner

**New intent.** Agents work in warm worktrees kept at stable paths: a worktree is switched between
commits instead of recreated. A gc scoped to one checkout keeps it fast, removing dead units and the
debug objects (`*.rcgu.o`) that no linked binary references. Roadmap: MILE-5, ROAD-16 then ROAD-17,
approved 2026-11-15. Evidence: `docs/roadmap/milestones/MILE-5.notes.md`.

**Unchanged.** ARCH-1 stays: no wrapper, hook, Cargo config or background process. Every checkout
still builds at its own path with stock Cargo.

Disposition values: Keep, Adapt, Retire, Deprecate, Freeze, Carve out, Accept debt, Unknown.

## Contradicted, or needing a decision

| # | Item | Source | Collision | Proposed | Entrenchment | Confirmed |
|---|---|---|---|---|---|---|
| 1 | **GOAL-6** | `docs/product/vision.md` | "deletes no file of a unit that is live". Stale debug objects carry a live unit's hash, but no binary references them, and deleting them rebuilt nothing in the spike. | **Retire**: superseded by a new GOAL-9 that keeps GOAL-6's guarantee (no live unit rebuilds because of a deletion; freed bytes never exceed what the volume regained) and allows removing a live unit's debug objects that no linked binary references | Cited by closed MILE-1 and MILE-2, by open MILE-5, and by the shipped LIVE, GCPLAN and STOREGC requirements | Confirmed: Retire, new GOAL-9 ("Retire, new GOAL-9") |
| 2 | Non-goal **"Slot mode"** and Deferred **"Slot mode, until `-p` and `--workspace` resolve the same feature set"** | vision Non-goals, Scope boundaries | ROAD-17 is slot mode | **Retire** both lines. In the spike, one slot ran `-p gateways` and `--workspace` every task, and each shape kept its own units warm | Vision text only; nothing shipped depends on it | Confirmed: Retire both ("Retire both") |
| 3 | Non-goal **"Editing repo files"** and **ARCH-2** ("never write a repo file") | vision Non-goals, `docs/architecture/INDEX.md` | The handoff runs `reset --hard`, `clean`, `switch` and an mtime touch in the slot | **Unknown**: depends on whether hive or the skill runs the handoff (ROAD-17 Ask me). If the skill runs it: Keep both. If hive runs it: Adapt both, so that git operations in a slot hive leased are allowed | ARCH-2 is cited by every shipped deletion and seed path; a change needs an ADR | Confirmed: **Keep** both, the skill runs the handoff ("The skill runs it") |
| 4 | **ARCH-18** ("never removes a fingerprint dir on its own and never touches `build/`") | `docs/architecture/INDEX.md` | The spike's gc also removed dead units' `.fingerprint` and `build/` entries | **Keep**: ROAD-16 deletes only dead units' `deps/` files and unreferenced debug objects. `deps/` size drove the slowdown; `build/` did not | Every shipped deletion path cites it | Confirmed as written ("Confirm all as written") |
| 5 | **ARCH-17** ("never compile or replay a build to decide liveness") | `docs/architecture/INDEX.md` | The spike took its live set from no-op `cargo --message-format json` runs | **Keep**: ROAD-16 must judge liveness from files on disk, i.e. the fingerprint graph, the debug map and #20's fingerprint-read signal | Shipped LIVE and GCPLAN depend on it | Confirmed as written ("Confirm all as written") |
| 6 | **GCPLAN-1.8 / 1.10** (GC window in whole days from config; no per-run window) | `docs/specs/2026-11-01-gc-plan-apply/requirements.md` | A window anchored at a handoff would reverse 1.10 | **Keep** for now: ROAD-16's unit liveness is an open Ask me. The debug-object class needs no window | Shipped in v0.1.0 | Confirmed as written ("Confirm all as written") |
| 7 | Feature **LIVE** (closure liveness) | `docs/specs/2026-10-30-closure-liveness/` | Governed by GOAL-6 | **Adapt**: repoint its goal citation to GOAL-9 when row 1 lands; behavior unchanged | Shipped in v0.1.0 | Confirmed as written ("Confirm all as written") |
| 8 | Feature **GCPLAN** | `docs/specs/2026-11-01-gc-plan-apply/` | Cites GOAL-6 | **Adapt**: repoint citation only | Shipped in v0.1.0 | Confirmed as written ("Confirm all as written") |
| 9 | Feature **STOREGC** | `docs/specs/2026-11-09-store-gc/` | Cites GOAL-6 | **Adapt**: repoint citation only | Shipped in v0.1.0 | Confirmed as written ("Confirm all as written") |
| 10 | Milestones **MILE-1, MILE-2, MILE-5** goal citations | `docs/roadmap/INDEX.md` | Cite GOAL-6 | **Adapt**: a `plan-milestones` pass repoints them to GOAL-9 after the vision update; the closed milestones' outcomes are unchanged | MILE-1 and MILE-2 are Closed | Confirmed as written ("Confirm all as written") |

## Checked, no collision

| # | Item | Proposed | Note |
|---|---|---|---|
| 11 | GOAL-2, GOAL-3, GOAL-4, GOAL-7, GOAL-8 | **Keep** | The pool extends GOAL-7 (agents with no human in the loop). Any new speed goal is phrased in counts (GOAL-4, ARCH-10) |
| 12 | ARCH-1, ARCH-3 to ARCH-11, ARCH-13 to ARCH-16, ARCH-19, ARCH-20 | **Keep** | Debug-map reading is macOS-specific and stays behind `crates/platform` (ARCH-15). Lease state goes through `state` (ARCH-16) |
| 13 | Hard constraint "Foreign mode only: every checkout builds at its own path with stock Cargo" | **Keep** | A slot is a checkout at its own stable path with stock Cargo |
| 14 | Non-goals: "Claude Code hooks", "A jobserver", "Replacing artifacts in place", "Time claims outside `cargo hive bench`", the rest | **Keep** | ROAD-18 (harness hooks) stays deferred |
| 15 | Features CORE, AUTOGC, EXPLAIN, SEED, BENCH, SPLIT | **Keep** | Unchanged |
| 16 | Features SKILL, SKILLPASS | **Keep** | The skill gains ROAD-16/17 commands through the normal spec cycle |

Rows 11–16: Confirmed as written ("Confirm all as written").

## Next units of work, after confirmation

- `/define-project` (update): rows 1–2. New GOAL-9; strike GOAL-6; drop "Slot mode" and its deferral.
  Row 3 needs no vision change (the skill runs the handoff; ARCH-2 and "Editing repo files" stay).
- ROAD-17's card: move "who runs the handoff" from Ask me to Decided (the skill).
- `plan-milestones`: row 10, then re-approval and `refresh-lanes`.
- `realign-spec` (citation only): rows 7–9.
- ROAD-16 `frame-change`: carries rows 4–6 as constraints.

## Applied (2026-11-15)

- Rows 1–2: `docs/product/vision.md` strikes GOAL-6 with its reason, adds GOAL-9, and drops "Slot mode"
  from the non-goals and its deferral (approved by the owner 2026-11-15).
- Row 3: no vision change; ROAD-17's card records that the skill runs the handoff.
- Rows 7–8: nothing to repoint. LIVE's and GCPLAN's requirements do not cite GOAL-6 by ID.
- Row 9: STOREGC `requirements.md` now cites GOAL-9. Audit-trace before and after: the same 8 pre-existing
  E1 errors (CORE, SEED task cites), 0 warnings.
- Row 10: MILE-1, MILE-2 and MILE-5 cite GOAL-9. MILE-5's Blockers cleared (roadmap re-approved 2026-11-15).
- `docs/roadmap/assessments/MILE-2.md` keeps its GOAL-6 mention as a historical record.
