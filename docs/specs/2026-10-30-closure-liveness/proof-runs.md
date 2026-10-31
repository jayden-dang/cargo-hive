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
  no hash to match), which was not verified here; the next section verifies it.
- **The dead-unit count of cargo-hive fell from 294 (run 3) to 106** because the target was rebuilt in
  between by the full test run on `main`.
- **Most bytes are outside this feature.** On klynt, `incremental/` plus the files directly under the
  profile dir held 74.57 GB (66% of the target) in run 1, against 25.06 GB for the dead units' files
  at the 6 h window. This feature does not collect them (requirements, Out of Scope).

Open for ROAD-6 and later: decide a target dir partly, not all or nothing; reconsider `incremental/`;
and measure a 7-day window on a quiet target that is old enough.

## Unresolved edges on klynt — 2026-10-31 (read-only)

Not a proof run: nothing was deleted or replayed. A throwaway probe crate (source kept outside git
in `.skills/LIVE/probe-unresolved/`, the crate itself removed from `/private/tmp`) read klynt's
target between 10:50 and 11:10 (+07) while klynt was building: `cargo watch -x 'run --bin server'`
ran the whole time, and an agent ran `cargo nextest run -p persistence`. Counts drift between
snapshots for that reason.

For every `Unresolved` reason, the probe reread the dependent unit's JSON, took the edge's package
id, crate name and fingerprint, and looked for unit dirs of that package in the same profile.

| Snapshot | 6 h and 1 d | 7 d | Edges whose package has a not-current dir |
|---|---|---|---|
| first | 120 edges from 5 bins (all roots) | 349 edges from 20 units | all |
| a few minutes later | 10 edges | 60 edges | all |

No edge pointed at a package with no unit dir, and none at a package whose dirs were all current.

What this establishes:
- **`Unresolved` has the same cause as `NotCurrentInWindow`.** A dependent records the fingerprint
  its dependency had when the dependent was built. While Cargo rebuilds that dependency, or after
  the rebuild fails, the dependency's hash file is empty, so nothing carries that fingerprint.
- **Direct evidence for one edge.** The bin `emit_native_config` (`gateways-035d4a632163c5b9`, built
  10:30) depends on `observability` with fingerprint `e5f9dbae472a3fef`, unresolved while
  `observability-fd0041f2fbb1deec` was rebuilding. One minute later that dir's hash file read
  `e5f9dbae472a3fef`: the same dir, rebuilt to the same fingerprint, and the edge resolved.
- **A failed build keeps edges unresolved.** Watched for 5 minutes, the 10 remaining edges stayed
  unresolved: `persistence-754f90deb43e28e1` had failed (19 errors in its `output-lib-persistence`)
  where the agent was editing `persistence`.
- **Not-current dirs last for days.** About 224 dirs were not current; 2 had error output. 138 of
  them carried the same invocation minute, 2026-10-29 10:39, two days earlier. They clear only when
  Cargo rebuilds that exact unit or they age out of the window. Why Cargo leaves a whole batch
  behind was not checked in Cargo's source.
- **klynt is never quiet.** Waiting for a quiet moment is not a way to reach Decided there.
- **The 7-day window reclaims nothing on klynt even if Decided**: every unit, current or not, was
  invoked inside the last 7 days.

A simulated rule, measured in the same probe on one snapshot (not implemented, not proven by a
deletion trial): a not-current dir invoked inside the window is a root and its last JSON's `deps`
are followed; an edge that matches no fingerprint keeps the not-current dirs of that package in the
same profile (a current dir carrying that fingerprint would have matched). For comparison, the
same with every dir of the package kept:

| Window | Today | Not-current dirs of the package kept | Every dir of the package kept |
|---|---|---|---|
| 6 h | Undecided | 1,129 dead units, 137,577 files, 38.89 GB allocated | 126 dead units, 2.73 GB |
| 1 d | Undecided | 1,107 dead units, 38.54 GB allocated | 106 dead units, 2.42 GB |
| 3 d | Undecided | 37 dead units, 2.85 GB allocated | 16 dead units, 0.39 GB |
| 7 d | Undecided | 0 | 0 |

Allocated bytes count clone-shared blocks in full; run 1 reported 25.06 GB of which 11.35 GB was
shared, so the volume would regain noticeably less. In this snapshot no unresolved edge lacked a
not-current dir of its package; a rule still has to say what happens then.
