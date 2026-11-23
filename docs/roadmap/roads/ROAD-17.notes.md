# ROAD-17 framing notes

## Confirmed on 2026-11-23

Jayden accepted option A: refuse a return while uncommitted work remains, preserve the
worktree, and keep the slot unavailable. The agent must commit or explicitly discard its
work before returning it. Returning a slot alone does not authorize discarding work.
The first version adds no automatic stash, snapshot, or recovery archive.

This records one decision. It does not approve a complete technical design.

## Questions for requirements and design

1. Define which untracked and ignored inputs make a return dirty. Include temporary
   environment files without treating every retained build or dependency directory as work
   to discard. Preserve the existing requirement to ask before removing `node_modules`.
2. Define how to recover a slot after an agent crashes or disappears. Elapsed time alone
   must not be treated as permission to discard the previous agent's uncommitted work.
   The owner and recovery action still need a decision.
3. Define how the handoff detects inputs that were changed, built, and restored before
   return. A clean Git status alone does not establish that cached output is clean.
   The old klynt spike covered selected `.sqlx` and `.env` inputs; broader coverage is unproven.
4. Define what happens after an interrupted cleanup, commit switch, invalidation, or GC.
   Specify when a slot becomes eligible for another agent and how retries are fenced.
5. Define performance acceptance with the real checkout GC and all handoff costs included.
   The old 85 s versus roughly 300 s example came from a local spike with emulated GC;
   it is evidence, not an approved threshold for ROAD-17.

## Existing boundaries and neighbours

- The feature remains within MILE-5's stable-path worktree scope.
- ROAD-16 supplies checkout GC; it does not implement the pool or its leases.
- Seed remains the fallback for a fresh worktree when the pool is full.
- The existing hive agent guide owns the Git and mtime handoff steps. Hive's binary stays
  within ARCH-1 and ARCH-2; existing commands and state upgrade behaviour remain guards.
- ROAD-19 supplies a baseline scorecard. Its current cold/seeded arms do not measure a
  reused slot, so warm-slot measurements need an explicit comparison plan.
- Pool size, naming, and lease representation remain delegated. Recovery and input
  correctness questions above are open questions, not new standing approval gates.
