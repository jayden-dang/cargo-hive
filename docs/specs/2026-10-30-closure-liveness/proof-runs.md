# Closure liveness: proof runs on a real target

Status: run 2 passed (2026-10-31, cargo-hive's own target). Run 1 on klynt was vacuous.

## Command (klynt, nothing building in it)

```
HIVE_PROVE_TARGET=~/Developer/klynt/klynt/target \
HIVE_PROVE_CHECKOUT=~/Developer/klynt/klynt \
HIVE_PROVE_CMDS='[["build","--offline"],["check","--all-targets","--offline"],["test","--no-run","--offline"]]' \
cargo test --release --test liveness_prove -- --ignored --nocapture
```

The replay is compile-only (2026-10-31): it accepts only `build`, `check`, `doc`, and `test` or `bench` with `--no-run`, by their long names, and no argument after `--`; any other command is refused, so no test or binary of the checkout runs.

## Results

### Run 1 — 2026-10-30 22:32 to 2026-10-31 02:15 (+07), 13,361 s (3 h 42 min), klynt `a58d9cda5`

Commands: `build`, `check --all-targets`, `test --no-run`, each with `--offline`. Nothing but the
owner's dev servers was running in the checkout; eight `cargo watch` processes with cwd
`klynt/backend` were still alive during the run. They did not touch the target (no
"original changed" failure), the checkout stayed clean and its `Cargo.lock` kept its mtime.

**Outcome: not proved. The test failed with "every window was vacuous, so nothing was proved".**

| Window | Outcome | Dead units | Dead files (measured on the original, before any clone) | Incremental + profile-root files (not collected) |
|---|---|---|---|---|
| 6 h | Vacuous: baseline replay rebuilt units | 1,598 | 25.06 GB freed, 11.35 GB still shared | 74.57 GB freed, 13.17 GB still shared |
| 1 d | Vacuous: baseline replay rebuilt units | 1,357 | 20.66 GB freed, 8.50 GB still shared | (same) |
| 7 d | Vacuous: liveness is Undecided (169 reasons) | 0 | 0 | (same) |

What the run shows:
- **The baseline guard fired.** Replaying the three commands on a clone with nothing deleted already
  rebuilt hundreds of units (workspace crates such as `ai`, `ai_quota_service`, …). Those commands
  therefore do not match what was last built in this target, and any rebuild after a deletion could
  not be told apart from them. No deletion was tried (rebuilt: 0 for both windows).
- **The 7-day window is Undecided on klynt**: all 169 reasons are the same, "unit is mid-rebuild
  or failed, invoked inside the window" (not-current units inside the window).
- The measured byte figures are valid for planning: they were taken on the original before any clone existed.

No claim of "zero live units rebuilt" is made from this run.

### Run 2 — 2026-10-31 07:44 to 07:48 (+07), 257 s, cargo-hive's own target at `c1dc6d5`

Target: `/Users/jayden/Developer/experiments/cargo-zip/target` (1.7 GB: `debug`, `release`,
`flycheck0`, `x86_64-unknown-linux-gnu`). Commands: `build` and `check --all-targets`, each with
`--offline` (the proof added `--locked` and `--message-format=json`). Nothing else was building in
the checkout. After the run the checkout was clean, its `Cargo.lock` kept its mtime and no clone
remained.

Why this target and not klynt or mailgate: the owner is coding in klynt, and mailgate has no target
and 526 of its 1,004 registry crates are not in the offline cache.

**Outcome: test passed** (the new warm-up before the baseline, LIVE-4.18 to 4.22, in force).

| Phase / window | Outcome | Rebuilt | Rebuilt live | Dead units | Dead files (original, before any clone) |
|---|---|---|---|---|---|
| Warm-up | both commands exit 0 | 3 (`cargo-hive` itself, 3 units) | n/a | n/a | n/a |
| Baseline | both commands exit 0 | 0 | n/a | n/a | n/a |
| 6 h | Passed | 152 | **0** | 294 | 0.98 GB freed, 0.23 GB still shared |
| 1 d | Passed | 0 | **0** | 90 | 0.14 GB freed, 3.0 MB still shared |
| 7 d | Vacuous: no dead unit lists a file to delete | 0 | 0 | 0 | 0 |

Incremental and profile-root files, not collected by this feature: 0.51 GB freed, 0.23 GB still shared.

What the run shows:
- **The warm-up fixed the failure of run 1.** The baseline rebuilt 0 units, so a rebuild after a
  deletion can be blamed on the deletion.
- **No live unit rebuilt in either Passed window.** In the 6 h window 152 units rebuilt, all of them
  dead by the window's rule: the commands needed units that no command had compiled in the last 6 h.
  That is the window gap (GOAL-1 as reworded), and the report shows it directly.
- **The 7 d window was vacuous** because no unit of this target was old enough to be dead: it proves
  nothing about a 7-day window.

Limits: one run, one project, one toolchain (rustc 1.97.1), and only the replayed commands
(`build`, `check --all-targets`; no `test`, `bench`, `doc`). A passed window covers only those.
