# Roadmap: cargo-hive

Status: Approved
Date: 2026-10-29

<!--
The program layer: milestone INTENT, above any single feature and below the product
vision. Optional — only multi-milestone projects need it. `plan-milestones` authors it;
`refresh-roadmap-status` derives progress from it without writing anything.

This file owns intent only: outcomes, ordering, membership, dependencies, commitments,
blockers, deferrals, goal dispositions. It never records how far a feature has got —
that lives once, as `Status:` in the feature's own requirements.md, mirrored into its
docs/specs/INDEX.md row. Every heading below is a REQUIRED slot — fill it or write
`None`.

Structural rules — AUTHORITATIVE. Both `plan-milestones` (before its approval gate) and
`refresh-roadmap-status` (as finding R11) validate against this list. A roadmap is structurally
defective when any of these does not hold:

  S1  every MILE-N and ROAD-N is defined exactly once — a definition lives in a
      Members: slot or a Deferred: slot, struck through or not
  S2  every ROAD-N sits under exactly one milestone, counting definitions in
      both slots
  S3  every milestone carries a non-empty Outcome sentence
  S4  no Depends-on names a milestone appearing later in the milestone table
  S5  every Depends-on resolves to exactly one live, non-struck-through MILE-N
  S6  every live GOAL-N in vision.md is cited by a milestone or listed under
      Goal dispositions
  S7  the milestone table and every milestone block parse
  S8  every After: value is `none` or comma-separated ROAD-N / MILE-N, each defined
      once, live, and neither the item itself nor its own milestone; and following
      After edges (an item without After waiting on the member above it) never
      returns to the item it left

ID rules:
- Grammar: **MILE-<n>** and **ROAD-<n>**, flat and repo-wide.
- Stable from first definition. Retire by strikethrough with a reason
  (~~**MILE-3**~~ superseded by MILE-5) — never renumber, never reuse.
- A ROAD-N keeps its ID when it moves between milestones.
- Milestone ORDER is table row order; milestone IDENTITY is the MILE-N. Reordering the
  table never renumbers anything.
- ITEM order is the After: field, written between the slug and Surfaces:
  `- **ROAD-7** <slug> — After: ROAD-5, MILE-2 — Surfaces: ...`. It names what must
  be done before this item starts: a ROAD-N in any milestone, or a MILE-N meaning
  every live member of that milestone. `none` means it can start now. IDs only — a
  reason belongs in Blockers:. Items whose After is done build in parallel. An item
  with no After: field (written before this rule) waits on the member above it, and
  the first member on its milestone's Depends-on. Order is the field, identity is
  the ID, so resequencing renumbers nothing.
- GOAL-N is defined in docs/product/vision.md and only cited here. Feature codes are
  defined in docs/specs/INDEX.md and never written here — a roadmap item is identified
  by its ROAD-N and slug until a feature spec binds to it.
- ROAD-N is a program SLOT (intent + order + Surfaces). A feature CODE is a separate
  delivery unit. Creating a ROAD does not create a feature. Binding is the INDEX
  "Roadmap item" cell (specify-behavior only). At most one live CODE binds a ROAD (R6).
  Same slug language does not make the IDs the same object.
-->

| ID | Milestone | Outcome | Depends-on | Commitment |
|---|---|---|---|---|
| MILE-1 | Safe automatic reclaim | On any Mac, a user installs cargo-hive from scratch, registers a repo, and reclaims superseded incremental sessions and leftover rustc temp entries across all its checkouts, from the CLI or from an agent driving that same CLI through the hive skill, with a receipt showing per-file bytes freed and zero units rebuilt because of a deletion. | none | Committed |
| MILE-2 | Planned reclaim with approval | A user or agent previews closure-based garbage and orphan target dirs as a plan, agents apply it themselves on secondary checkouts, and plans for main or orphan dirs apply only after the owner approves with Touch ID or password through `cargo hive approve`. | MILE-1 | Planned |
| MILE-3 | Warm worktrees from a shared store | A new secondary checkout (a build-in-waves lane, a native Claude worktree) is seeded from a compressed, hive-owned store, so its first build recompiles zero seeded registry units while the seeded copies add about zero private bytes, and `cargo hive bench` gives the measured cold-versus-seeded time. | MILE-2 | Planned |

## MILE-1 — Safe automatic reclaim

**Outcome:** On any Mac, a user installs cargo-hive from scratch, registers a repo, and reclaims superseded incremental sessions and leftover rustc temp entries across all its checkouts, from the CLI or from an agent driving that same CLI through the hive skill, with a receipt showing per-file bytes freed and zero units rebuilt because of a deletion.
**Goals:** GOAL-1, GOAL-4, GOAL-5
**Members:**
- **ROAD-1** foundation-registry-discovery-receipts — After: none — Surfaces: None — the crate and module layout is set in design-solution. Expected components: repo registry, checkout discovery and C1 classification, Cargo target identification (`CACHEDIR.TAG` plus profile/.fingerprint), fingerprint reader with per-toolchain format check, receipt model, `doctor` capability checks, platform layer (`Fs`, `Approve`).
- **ROAD-2** auto-tier-gc — After: ROAD-1 — Surfaces: None — the module layout is set in design-solution. Expected components: owner try-locks (Cargo build-dir lock, rustc session `.lock`), D6 session rule, `rustc*`/`rmeta*` temp sweep, per-file freed-byte accounting (D7).
- **ROAD-3** explain-findings — After: ROAD-1 — Surfaces: None — the module layout is set in design-solution. Expected components: read-only findings on why a crate has N copies in a target dir (variant groups by feature set, mode and profile hash; feature divergence within one mode and target kind; failed units from the rustc summary line). Orphan candidates are left to `status`, which already reports them, and "emptied protected targets" is dropped, never having been defined (2026-10-29, ROAD-3 frame-change).
- **ROAD-12** agent-skill — After: ROAD-2, ROAD-3 — Surfaces: None — the module layout is set in design-solution. Expected components: a hive agent skill (`SKILL.md`) kept in this repo and embedded in the binary, so it always matches that binary's flags and receipt schema; `cargo hive skill install` writing it into the agent's skills dir (Claude Code: `~/.claude/skills/hive/`); a `doctor` check that flags an installed skill from a different version. Agents drive every MILE-1 operation through the CLI with `--json`, taking scope from arguments.
**Depends-on:** none
**Commitment:** Committed 2026-10-25
**Closed:** None
**Deferred:**
- ~~**ROAD-4**~~ mcp-server — dropped 2026-10-29: hive is a local CLI with no server state, so an MCP server would only wrap the receipts `--json` already prints, and its tool schemas would cost tokens in every agent session. Replaced by ROAD-12, the CLI plus a skill, as `gh` does.
**Blockers:** None

## MILE-2 — Planned reclaim with approval

**Outcome:** A user or agent previews closure-based garbage and orphan target dirs as a plan, agents apply it themselves on secondary checkouts, and plans for main or orphan dirs apply only after the owner approves with Touch ID or password through `cargo hive approve`.
**Goals:** GOAL-1, GOAL-5
**Members:**
- **ROAD-5** closure-liveness — After: MILE-1 — Surfaces: None — the module layout is set in design-solution. Expected components: `invoked.timestamp` window roots, fingerprint-graph closure (little-endian match), D5 deletion rule.
- **ROAD-6** gc-plan-apply — After: ROAD-5 — Surfaces: None — the module layout is set in design-solution. Expected components: plan files, apply-time revalidation (liveness, try-locks, format), secondary-versus-protected scope (S2), orphan-target plans, CLI plan and apply whose JSON receipt carries `needs-human` and whose exit code is distinct for it, the hive skill extended to cover them.
- **ROAD-7** owner-approval — After: ROAD-6 — Surfaces: None — the module layout is set in design-solution. Expected components: `cargo hive approve`, the TTY and argv-ancestry checks, `.deviceOwnerAuthentication` through objc bindings, 60 s expiry.
**Depends-on:** MILE-1
**Commitment:** Planned
**Closed:** None
**Deferred:** None
**Blockers:** None

## MILE-3 — Warm worktrees from a shared store

**Outcome:** A new secondary checkout (a build-in-waves lane, a native Claude worktree) is seeded from a compressed, hive-owned store, so its first build recompiles zero seeded registry units while the seeded copies add about zero private bytes, and `cargo hive bench` gives the measured cold-versus-seeded time.
**Goals:** GOAL-2, GOAL-3, GOAL-4, GOAL-5
**Members:**
- **ROAD-8** store-fill-compress — After: MILE-2 — Surfaces: None — the module layout is set in design-solution. Expected components: D8 try-locked fill, D1 key, embedded Applesauce compression in the store, `store.db` (`size`, `filled_at`, `last_live_at`).
- **ROAD-9** seed-secondary-checkouts — After: ROAD-8 — Surfaces: None — the module layout is set in design-solution. Expected components: clonefile out (D2), variant pick by newest `last_live_at`, `reused_fresh` receipts, a CLI `seed` command, the hive skill extended to cover it.
- **ROAD-10** store-gc — After: ROAD-8 — Surfaces: None — the module layout is set in design-solution. Expected components: 30-day not-live eviction, rustc generation drop, cap by freeable bytes (D7), never evicting live units.
- **ROAD-11** bench — After: ROAD-9 — Surfaces: None — the module layout is set in design-solution. Expected components: repeated cold-versus-seeded runs reporting median, p90, spread and load average.
**Depends-on:** MILE-2
**Commitment:** Planned
**Closed:** None
**Deferred:** None
**Blockers:** None

## Goal dispositions

Every live `GOAL-N` in `docs/product/vision.md` that no milestone cites belongs here, so
that a goal is never silently dropped (S6).

None. Every live goal, GOAL-1 to GOAL-5, is cited by at least one milestone.

| Goal | Disposition | Date | Reason |
|---|---|---|---|
