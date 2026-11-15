# ROAD-17 warm-slot-pool

**Outcome:** An agent asks hive for a worktree for a repo and a commit and gets a warm one at a
stable path, then gives it back; the next agent gets that worktree cleaned, switched and still
fast.

**Examples:**
1. Agent A takes slot 2 of klynt at the dev tip, works, and returns it. Agent B takes slot 2 at
   the next commit, which changes one crate: lint, test build and `-p gateways` take about 85 s,
   against about 300 s in a new seeded worktree.
2. Agent A left a changed `.sqlx` file and a `.env` file behind: B's first build rebuilds the
   crates that read `.sqlx`, and B finds no `.env`.
3. Two agents ask at the same moment: they never get the same slot.
4. Every slot is taken: hive says so and the agent falls back to a new seeded worktree.

**Must keep:** the main checkout and the owner's lanes are never handed out or switched; seed
keeps working for new worktrees.
**Off limits:** hooks into agent harnesses (ROAD-18, deferred); wrappers.
**Decided:** keep the worktree and switch commits (way b from #17); nothing from the previous
agent's uncommitted work reaches the next agent, in files or in build output; the handoff runs
ROAD-16's gc; the skill, not hive, runs the handoff's git steps and mtime touch (2026-11-15).
**Delegated:** pool size, slot naming, the lease format.
**Ask me:** what happens to work the returning agent left uncommitted; anything that edits tracked files
beyond their mtimes, or removes `node_modules`.
**UI:** none
