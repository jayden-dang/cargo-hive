# MILE-5 notes

Evidence (local spike records, not tracked in git):
`.skills/verify/2026-11-14-warm-slots/PLAN.md` and `.skills/verify/2026-11-14-pool-vs-wrapper/PLAN.md`.
The #17 summary is in issue #17's comment of 2026-11-14.

Measured on klynt (one Mac, rustc 1.97.1, gc emulated in a script, one sample per row):

- A worktree kept at a stable path and switched between commits rebuilt only the changed crates and
  their dependents.
- Without gc it slowed down task by task. About 10k stale debug objects (`*.rcgu.o`) pile up in
  `deps/` per test build, and a new test binary starts 2× slower in that directory.
- With gc at every handoff (by the units the task used, plus debug objects no binary references), the
  median task time was 108 s and edits took 42 s over 6 tasks; inside a task the wait grew about 4 s per
  build. With the debug-object gc also after every build, it was 85 s and 29 s, flat, not counting that
  gc's 7–8 s per build. A seeded new worktree took 308 s.
- The debug-object class needs no window: in one run it removed 115,248 objects against 1,542 unit
  entries, and the 4 extra recompiles per test build came from the unit gc's incomplete live set.
- With three agents at once, the median makespan was 443 s, against 732 s for seeded new worktrees.

Blockers, in full:

- Vision lists "Slot mode" as a non-goal (deferred "until `-p` and `--workspace` resolve the same
  feature set"). In the spike, one slot ran both `-p gateways` and `--workspace`
  commands every task, and each shape kept its own units warm: after a switch, each recompiled only
  the changed crates and their dependents. `/define-project` decides whether that clears the deferral.
- GOAL-6 says hive deletes no file of a live unit. Stale debug objects carry the live unit's hash but
  no binary references them. Deleting them rebuilt nothing in the spike. GOAL-6's wording has to
  cover them before ROAD-16 deletes them.
- Who runs the handoff's `reset --hard`, `clean`, `switch` and mtime touch is open. hive touches only
  target dirs today, and the vision lists "Editing repo files" as a non-goal. `reset --hard` also
  discards whatever the returning agent left.
- A handoff-anchored window for unit gc meets issue #20: a unit used without recompiling leaves no
  `invoked.timestamp`. The spike's gc took the live set from the task's own build output, which a
  CLI-only hive cannot see. Example 4 on the ROAD-16 card is the behavior that must hold.
