# Closure liveness: proof runs on a real target

Status: runs 2 and 3 passed (2026-10-31, cargo-hive's own target). Run 1 on klynt was vacuous.

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

### Run 3 — 2026-10-31 09:38 to 09:39 (+07), 45 s, after the whole-branch review fixes, the compile-only replay and the polish (`live` at `cd82e92`)

Same target and checkout as run 2 (`c1dc6d5`), commands `build`, `check --all-targets` and
`test --no-run`, each with `--offline`; the proof added `--locked` and `--message-format=json`. The
target had not been built since run 2. The checkout stayed clean, no clone remained.

**Outcome: test passed.**

| Phase / window | Outcome | Rebuilt | Rebuilt live | Dead units | Dead files (original, before any clone) |
|---|---|---|---|---|---|
| Warm-up | 3 commands exit 0 | 3 (`cargo-hive`, 3 units) | n/a | n/a | n/a |
| Baseline | 3 commands exit 0 | 0 | n/a | n/a | n/a |
| 6 h | Passed | 166 | **0** | 294 | 0.98 GB freed, 0.23 GB still shared |
| 1 d | Passed | 17 | **0** | 133 | 0.41 GB freed, 0.07 GB still shared |
| 7 d | Vacuous: no dead unit lists a file to delete | 0 | 0 | 0 | 0 |

Why the 1-day figures differ from run 2 (90 dead units, 0.14 GB, 0 rebuilt): about 19 hours passed
between the runs, so more units fell outside the 1-day window, and the added `test --no-run` needs
units the other two commands do not. The 17 units that rebuilt were all dead by the window's rule,
the window gap again; none was live.

Limits are those of run 2, plus this: `test --no-run` compiles test binaries but, by design, no test
or binary of the checkout ran (LIVE-4.28).

## Measurements — 2026-10-31 (read-only, after the merge)

Not a proof run: nothing was deleted or replayed. A throwaway crate in `/private/tmp` (since
removed, not in the repo) depended on `cargo_hive` by path, built each target with
`cargo_target::identify` and `fingerprint::read_profile`, and called `liveness()` three times per
window, taking the best time. The machine was under heavy outside load (load average about 28), so
the times are pessimistic. The targets belong to live work and drift from minute to minute.

| | cargo-hive `target` (2.0 GB) | klynt `target` (113 GB) |
|---|---|---|
| Profiles | 3, none unsupported | 1, none unsupported |
| Current units / not-current units | 396 / 0 | 3,152 / 199 (3,179 / 172 minutes earlier) |
| `read_profile` for the whole target (what `status` already pays) | 0.36 s | 0.60 s warm, 4.46 s cold |
| `liveness()`, best of 3 | 0.023 s (6 h, 1 d), 0.001 s (7 d) | 1.2 s (6 h), 0.7 s (1 d), 0.05 s (7 d) |
| 6 h and 1 d windows | Decided: 256 live, 106 dead, 387 files | Undecided, 195 reasons: 30 `NotCurrentInWindow`, 165 `Unresolved` |
| 7 d window | Decided: 396 live, 0 dead | Undecided, 809 reasons: 199 `NotCurrentInWindow`, 610 `Unresolved` |

What these numbers say:
- **Cost is not the problem.** On 3,000 units `liveness()` adds about one second to the 0.6 to 4.5
  seconds `status` already spends reading fingerprints. Before the polish commit that replaced the
  per-dead-unit scan with one pass, `liveness()` was never timed; its equivalence to the old code was
  checked on 3 million generated names, not benchmarked.
- **At the default 7-day window nothing is reclaimable on either real target today.** cargo-hive has
  no unit older than 7 days; klynt is Undecided.
- **On an actively developed workspace Undecided is the normal result.** klynt was Decided at the 6 h
  window in run 1 (1,598 dead units) and is Undecided now, because units are mid-rebuild or failed
  inside the window. The `Unresolved` reasons are most likely the same cause (a dependency unit with
  no hash to match), which was not verified.
- **The dead-unit count of cargo-hive fell from 294 (run 3) to 106** because the target was rebuilt in
  between by the full test run on `main`.
- **Most bytes are outside this feature.** On klynt, `incremental/` plus the files directly under the
  profile dir held 74.57 GB (66% of the target) in run 1, against 25.06 GB for the dead units' files
  at the 6 h window. This feature does not collect them (requirements, Out of Scope).

Open for ROAD-6 and later: decide a target dir partly, not all or nothing; reconsider `incremental/`;
and measure a 7-day window on a quiet target that is old enough.

