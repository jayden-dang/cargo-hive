# ROAD-19 worktree-scorecard

**Outcome:** One scorecard in the repo says what an agent's worktree costs on klynt today, cold
and hive-seeded, and what each number should become. Every later upgrade is judged by whether it
moves a row.

**Examples:**
1. Today's one headline number is a seeded first build of 73.6 s against 86.9 s cold, of which
   4.9 s is the seed and 68.7 s is workspace crates compiling cold. The scorecard puts that row
   beside the other four first-command shapes (`check --all-targets`, `clippy --all-targets`,
   `test --no-run`, `check -p <package> --lib`), each with a baseline, a target and a date.
2. Jayden edits one file in a workspace crate of a seeded worktree: the scorecard shows the next
   build's time, the private bytes the worktree added, and how many units rebuilt because of a
   hive deletion.
3. A probe changes a file a proc macro reads without declaring it (`.sqlx/*.json`, `.env`): the
   scorecard records whether the build came back stale.
4. A later feature ships: someone follows the written steps, reruns the scorecard, and the new
   numbers replace the old ones in place, with no second row.

**Must keep:** bench's own rules and receipts (the scorecard uses bench where it fits and
records the rest by hand); a duration bench can time comes from `cargo hive bench`, as integer
milliseconds (GOAL-4); the one bench cannot time, the next build after an edit, is timed by hand
and labelled outside bench (2026-11-22).
**Off limits:** Linux; changing what hive does (this item measures, it fixes nothing).
**Decided:** none yet beyond the issue's rows (#16, 2026-11-22).
**Delegated:** where the scorecard file lives and its layout; how the by-hand rows are
recorded; the written rerun steps.
**Ask me:** the target value of each row (the issue says each needs one, and only the owner
decides what "good enough for parallel agents" means); whether the scorecard may hold klynt
paths and numbers in a public repo.
**UI:** none
