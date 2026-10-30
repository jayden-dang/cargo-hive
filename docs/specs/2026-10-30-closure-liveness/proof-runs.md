# Closure liveness: proof runs on a real target

Status: pending owner run. No results are recorded yet.

## Command (klynt, nothing building in it)

```
HIVE_PROVE_TARGET=~/Developer/klynt/klynt/target \
HIVE_PROVE_CHECKOUT=~/Developer/klynt/klynt \
HIVE_PROVE_CMDS='[["build","--offline"],["check","--all-targets","--offline"],["test","--no-run","--offline"]]' \
cargo test --release --test liveness_prove -- --ignored --nocapture
```

The replay accepts only `build`, `check`, `test`, `doc`, `bench` and `run` by their long names; any other command is refused.

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
