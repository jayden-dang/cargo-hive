# ROAD-17 warm-slot-pool

**Outcome:** An agent asks hive for a worktree for a repo and a commit and gets a warm one at a
stable path, then gives it back after resolving its uncommitted work; the next agent gets that
worktree cleaned, switched and still fast. A refused return preserves the work and keeps the slot unavailable.

**Examples:**
1. Agent A takes slot 2 of klynt at the dev tip, works, and returns it. Agent B takes slot 2 at
   the next commit, which changes one crate, and reuses the unchanged crates' build state.
2. Agent A tries to return slot 2 with an uncommitted `.sqlx` edit: the return is refused,
   the file is unchanged, and Agent B cannot receive slot 2. A must commit or explicitly discard its work first.
3. Agent A built with a changed `.sqlx` file and a temporary `.env`, then explicitly discarded both:
   Agent B receives neither the temporary file nor stale build results from those inputs.
4. Agents A and B ask at the same moment: they never get the same slot.
5. Every slot is taken: hive says so and Agent B falls back to a new seeded worktree.

**Must keep:** the main checkout and the owner's lanes are never handed out or switched; seed
keeps working for new worktrees; a refused return preserves the slot's files and build state.
**Off limits:** hooks into agent harnesses (ROAD-18, deferred); wrappers; automatic discard or backup of uncommitted work.
**Decided:** keep the worktree and switch commits (way b from #17); nothing from the previous
agent's uncommitted work reaches the next agent, in files or in build output; the handoff runs
ROAD-16's gc; the skill, not hive, runs the handoff's git steps and mtime touch (2026-11-15).
A dirty return is refused and the slot is not reassigned until the agent commits or explicitly
discards its work; no automatic stash or recovery archive (confirmed 2026-11-23).
**Delegated:** pool size, slot naming, the lease format.
**Ask me:** anything that edits tracked files beyond their mtimes, or removes `node_modules`.
**UI:** none
