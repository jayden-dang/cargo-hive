# ROAD-13 agent-skill-pass

**Outcome:** An agent in any registered repo reads the hive skill once and drives every
hive command correctly through the CLI with `--json`, from one guide organised by what
the user wants done, with no stale, duplicated or contradicting instruction left from the
per-item additions; a person reading the README finds the same commands described the same way.

**Examples:**
1. In klynt, asked "free disk across my lanes", a Sonnet agent runs `gc --dry-run` then
   `gc` on the named checkouts, and reports the bytes freed from the receipt.
2. Starting three build-in-waves lanes, the agent seeds one lane, builds it, then seeds
   the other two, and runs bare `gc` after the batch finishes.
3. `seed` returns `source.none`: the agent builds the main checkout first instead of
   retrying; `apply` refuses with `walk.unreadable`: it runs `repo remove --missing`.
4. A flag or receipt field the skill names is renamed in the binary: the skill test fails.

**Must keep:** the version check first; the 150-line body budget; the plugin and
hive-version match with the crate.
**Off limits:** new commands, flags or CLI behaviour.
**Decided:** the skill is written through the author-skills workflow; RED/GREEN agent
evals are rerun with Sonnet and Haiku, as ROAD-12 did; the README is rewritten to match.
**Delegated:** section order, wording, which examples the guide and README show.
**Ask me:** dropping an instruction the skill gives today; raising the line budget.
**UI:** none
