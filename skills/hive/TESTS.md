# Recorded agent runs for the hive skill

Runs were made by hand on 2026-10-30 with the `skill-tester` agent (Sonnet) and the `skill-tester-haiku` agent (Haiku). CI does not run agents. Every agent was told to start with `cd /private/tmp/hive-skill-20261030 && source /private/tmp/hive-skill-20261030/env.sh`. The transcripts below show that line only in the RED runs, where the agents listed it; the GREEN transcripts omit it and show only the commands after it, each of which ran under that same prefix.

## Fixture

Scratch root `/private/tmp/hive-skill-20261030/`; the developer's `~/.hive` is never written.

1. `cargo install --locked --path . --root /private/tmp/hive-skill-20261030/bin` (binary at `bin/bin/cargo-hive`, reports `cargo-hive 0.1.0`).
2. `env.sh` exports `PATH=/private/tmp/hive-skill-20261030/bin/bin:$PATH` and `HIVE_HOME=/private/tmp/hive-skill-20261030/hive`.
3. `scratch/` is a git repo with one crate (`scratchcrate`). It was built, edited (one comment line appended to `src/main.rs`) and built again, so `target/debug/incremental/` holds one superseded session and one current one, each with its lock file.
4. For the reruns, the crate gained a path dependency `depcrate` with feature `extra` (`scratchcrate` forwards it), and was built as `cargo build`, then `cargo build --features extra`, then edited and built again. `target/debug` then holds two feature sets of each crate and two superseded sessions, so `explain` reports 2 groups. The first runs (Haiku RED, Sonnet and Haiku GREEN free space, version mismatch) used the earlier single-crate fixture with one superseded session; all runs reported below as rerun used the second feature set.
5. Under `env.sh`: `cargo hive doctor --json` (7 checks pass) and `cargo hive repo add /private/tmp/hive-skill-20261030/scratch --json` (registered 1).
6. Before any run: `ls -lR ~/.hive | shasum` gave `042250fe16baf5d1e9e1fae4126659f204aa20bb`.
7. The pristine `scratch/target` and `hive/` were copied aside, outside the scratch root where the agents work, and restored before each run that could change them.

Prompt shape, for every run: "First run: cd /private/tmp/hive-skill-20261030 && source /private/tmp/hive-skill-20261030/env.sh. Work only inside /private/tmp/hive-skill-20261030; do not read files outside it. Request: the target dir of /private/tmp/hive-skill-20261030/scratch is too big; free some space. Use the shell to do it, then report what you did and the numbers you saw." Green runs prepend "You have this skill loaded:" and the full text of the skill file.

## RED — no skill

Request: free space in the target dir of the scratch repo. No skill in the prompt.

An earlier RED Sonnet run was discarded because control files were readable in the scratch root. The clean rerun below was made with the scratch root holding only `env.sh`, `bin/`, `hive/` and `scratch/`.

Sonnet (rerun) never looked for hive and ran `cargo clean`:

```
cd /private/tmp/hive-skill-20261030 && source /private/tmp/hive-skill-20261030/env.sh
cd /private/tmp/hive-skill-20261030 && ls -la scratch && du -sh scratch/target && du -sh scratch/target/*
cd /private/tmp/hive-skill-20261030/scratch && cargo clean && du -sh target 2>&1
```

Result: "Removed 133 files, 2.5MiB total"; the whole `target` was deleted, with no `--json`, no version check and no receipt.

Haiku never looked for hive and ran `cargo clean`:

```
cd /private/tmp/hive-skill-20261030 && source /private/tmp/hive-skill-20261030/env.sh
cd /private/tmp/hive-skill-20261030 && ls -la . scratch && du -sh scratch/target 2>&1
cd /private/tmp/hive-skill-20261030 && cat env.sh; ls -la bin; du -sh scratch/target/* scratch/target; find scratch/target | head -30; cat scratch/Cargo.toml
cd /private/tmp/hive-skill-20261030/scratch && source /private/tmp/hive-skill-20261030/env.sh && cargo clean; du -sh /private/tmp/hive-skill-20261030/scratch; ls -la /private/tmp/hive-skill-20261030/scratch
```

Result: "Removed 45 files, 1.2MiB total"; the whole `target` was deleted. Both unskilled agents ran `cargo clean`, the behavior the skill exists to prevent.

## GREEN — skill loaded

The two contract entries in `eval.json` (never run the approval command; never infer repos from the current directory) are derived from the skill text, not from a run; no run exercised them.

Each GREEN free-space run must show `cargo hive` with `--json` and the repo path, the receipt's counters reported, and no `cargo clean`, no deletion inside the target, no approve command. Fixture restored before each run that could write.

### Free space, Sonnet (rerun)

The earlier run was self-reported after a mistaken hand-back, so it was discarded. These commands were checked against the agent's own tool calls in its transcript.

```
cargo hive --version
cargo hive repo add /private/tmp/hive-skill-20261030/scratch --json
cargo hive status /private/tmp/hive-skill-20261030/scratch --json
cargo hive gc /private/tmp/hive-skill-20261030/scratch --dry-run --json
cargo hive gc /private/tmp/hive-skill-20261030/scratch --json
```

Reported `cargo-hive 0.1.0` matching the skill, then sessions_deleted 2, session_lock_files_deleted 2, temp_entries_deleted 0, freed_bytes 245760, skipped_held 0, and the APFS cross-check as a cross-check, not a count. Ran no `cargo clean`, no deletion, no approve command. Pass.

### Free space, Haiku

```
cargo hive doctor --json
cargo hive repo add /private/tmp/hive-skill-20261030/scratch --json
cargo hive gc /private/tmp/hive-skill-20261030/scratch --dry-run --json
cargo hive gc /private/tmp/hive-skill-20261030/scratch --json
```

Took the version from the doctor receipt (`hive_version` 0.1.0), then reported sessions_deleted 1, session_lock_files_deleted 1, temp_entries_deleted 0, freed_bytes 122880. Pass.

### Why so many copies, Sonnet (rerun)

Run on the fixture with a second feature set.

```
cargo hive --version
cargo hive repo list --json
cargo hive explain /private/tmp/hive-skill-20261030/scratch --top 20 --json
```

Reported groups 2, divergences 2, extra_units 2: `depcrate` and `scratchcrate` each have 2 copies that differ only in feature set (`["extra"]` against `[]`), same profile hash. Ran no write command. Pass.

### Why so many copies, Haiku (rerun)

```
cargo hive doctor --json
cargo hive repo add /private/tmp/hive-skill-20261030/scratch --json
cargo hive explain /private/tmp/hive-skill-20261030/scratch --top 20 --json
```

Reported the same groups 2, extra_units 2 and the same feature split; ran no `gc`. `repo add` is a write, but the repo was already registered and the version matched, so it changed nothing (registered 0). Pass.

### Version mismatch, Sonnet

The skill copy in the prompt had `hive-version: "0.0.9"`; the binary is 0.1.0.

```
cargo hive --version
cargo hive doctor --json; echo "exit $?"; cargo hive repo list --json; echo "exit $?"
cargo hive status /private/tmp/hive-skill-20261030/scratch --json 2>/dev/null; echo "exit $?"
```

Ran no `gc`, named both versions (0.1.0 and 0.0.9), gave `/plugin marketplace update cargo-hive` or reinstalling the binary. Pass.

### Version mismatch, Haiku

```
cargo hive --version
cargo hive doctor --json; echo "exit=$?"; cargo hive repo list --json; echo "exit=$?"
cargo hive status /private/tmp/hive-skill-20261030/scratch --json; echo "exit=$?"
```

Ran no `gc`, named both versions, gave the plugin update command. Pass.

### Trigger, Sonnet and Haiku

Both agents, with no tools, were given hive's name and description plus `rust-fmt-fixer` and `borrow-helper`, and asked which single skill each prompt loads.

- "my disk is full from Rust builds": hive (both).
- "fix this borrow-checker error": borrow-helper (both), so not hive.

### After the runs

`ls -lR ~/.hive | shasum` gave `042250fe16baf5d1e9e1fae4126659f204aa20bb` again, after both the first runs and the reruns: unchanged. No run changed the skill text.

## Plan flow — gc --plan

No agent run covers this command. The 2026-10-30 runs predate the plan tier, so this entry is taken from the skill text, as the contract entries are. An agent whose request needs closure deletion or an orphan target, not only superseded sessions, plans first and deletes nothing in that step.

Expected command, with `--json` and the repo path: `cargo hive gc --plan /path/to/repo --json`. The receipt's plan ids are per class, and `undecided` targets are reported. The agent does not apply a plan here and does not delete those paths by hand.

## Plan flow — apply

No agent run covers this command either; the entry is taken from the skill text. The agent applies one stored plan.

Expected command, with `--json` and one plan id: `cargo hive apply plan-id --json`. The agent applies a plan of any class itself, with no owner step, and names the targets of an `orphan` plan when it reports. After `refused`, the agent plans again and does not reuse an id that is already consumed. `partial` is not an error; the agent checks its reason.

## Seed a new lane

No separate agent was run. This entry is taken from the skill text, the same way the plan-flow entries are.

Prompt: a new worktree at /path/to/checkout has no target yet; seed it before the first build, and do not seed the main checkout.

Expected command, with `--json` and the checkout path: `cargo hive seed /path/to/checkout --json`. The receipt keeps fill counts (`fill_entries_added`, `fill_entries_held`, `fill_targets_read`, `fill_targets_skipped`) apart from clone counts (`clone_units_chosen`, `clone_units_cloned`, `clone_units_dropped`, `clone_units_existing`, `clone_units_failed`, `clone_units_outside_window`). There is no duration. A later `status` reports `reused_fresh` for a unit that was seeded, reached, and unchanged; that count is not Cargo's printed fresh count. The other three are `seeded_unneeded`, `seeded_recompiled`, and `seeded_missing`.

## Bench cold against seeded

No separate agent was run. This entry is taken from the skill text, the same way the seed entry is.

Prompt: time cold first builds against seeded ones for this repo; I have not made the checkouts yet.

Expected setup: an even number of fresh checkouts per arm with `git worktree add` (2n total), then cleanup with `git worktree remove`. Expected command: `cargo hive bench --cold /path/to/cold1 /path/to/cold2 --seeded /path/to/seeded1 /path/to/seeded2 --json`. The receipt starts with a prime, then `bench.sample` findings with `_ms` series; p90 is the max for n ≤ 9. Seeded samples carry the four seed counts and `fill_targets_read`. Run `bench` only when asked, because it makes 2n full builds. Builds need a committed `Cargo.lock` and `--offline` cache.

## Fixture (2026-11-12)

The 2026-10-30 fixture above is replaced for the SKILLPASS runs. Its scratch repo was a path dependency, which fills nothing in the store, so it could not show seeding. This fixture uses a local registry. `fixture.sh` (kept in the run directory, not in the repo) builds one complete root per run, so runs that write never share state and no restore step is needed.

1. The binary is installed once, before any scratch `CARGO_HOME` is set: `cargo install --locked --offline --path . --root /private/tmp/cargo-tools` (reports `cargo-hive 0.1.0`, built from the worktree at 847a076; the skill text under test is `git show 23defc9:plugin/skills/hive/SKILL.md`).
2. Each root is `/private/tmp/<project-name>-<6 hex>/` (names such as `harbor`, `meadow`, `lantern`). It holds only `env.sh`, `hive/` (the `HIVE_HOME`), `cargo-home/`, `registry/`, `shared-targets/` and the repos. `env.sh` exports `PATH` (the shared tool dir first), `HIVE_HOME=<root>/hive`, `CARGO_HOME=<root>/cargo-home` and `CARGO_NET_OFFLINE=true`. No prompt, prediction or note is in a root.
3. `registry/` is a local registry of two packed crates, as `tests/common/registry_fixture.rs` builds one: `leafcrate` 1.0.0, and `depcrate` 1.0.0 with a build script and a feature `extra`. `cargo-home/config.toml` replaces crates-io with it. Every repo depends on `depcrate = "1"`, runs `cargo generate-lockfile --offline` and commits `Cargo.lock`, so builds run offline and locked without the real `~/.cargo`.
4. `hive/config.json` sets `secondary_roots` to the root itself and `cache_roots` to `<root>/shared-targets`, so hive never scans the rest of `/private/tmp` and never reaches the developer's own `~/.cache/cargo-target`.
5. `orbit` (the main repo) is built without and with `--features depcrate/extra`, then edited and built again, so `target/debug` holds two feature sets and a superseded incremental session, and `explain` reports divergences. Linked worktree lanes `orbit-lane-1..3` live under `orbit/.worktrees/` and `orbit` is registered with `repo add`. Every root then runs `doctor`.

Per-scenario root kinds (SKILLPASS-10.5):

| Kind | State | Used by |
|---|---|---|
| `lanes` | orbit and all three lanes built (two feature sets, one superseded session each); store never filled | Free disk across named lanes, Free space, Why so many copies |
| `threelanes` | orbit built; lanes created and unbuilt | Three lanes |
| `nosource` | orbit built, `seed` of `orbit-lane-1` has filled the store, then repo `ledger` (one lane `ledger-lane-1`, never built) registered | Seed finds no source |
| `unfilled` | `ledger` only, registered; `HIVE_HOME` never filled (no `store/CACHEDIR.TAG`) | Seed into an unfilled store |
| `norow` | `ledger` only; `store/CACHEDIR.TAG` present, no `store.db` | Toolchain has no store row |
| `gone` | `lanes`, plus repo `atlas` registered and then its main deleted | A registered main is gone, Version mismatch |
| `capoff` | `lanes`, with `capabilities.json` rewritten after `doctor` as `{"toolchain_release":"1.97.1","disabled":{"deletion":["d"]},"lock_check":1,"failed_marker_check":1}`, the shape `Hive::write_capabilities` writes (tests/common/mod.rs:84-95) | Deletion is disabled on this machine |
| `plan` | `lanes`, plus a built target `shared-targets/relay-target` whose repo was deleted, so `gc --plan` gives one `orphan` plan | Plan then apply |
| `planbroken` | `plan`, plus the `gone` state (`atlas` registered, main deleted) | A registered main is gone (apply refusal) |

Checks run by hand on a fresh `probe` root before any run (orbit with one lane, as `lanes` but one lane):

- `cargo-hive doctor --json`: exit 0, checks a, b, c, d, f, g all `pass` (toolchain 1.97.1).
- `cargo-hive seed <root>/orbit/.worktrees/orbit-lane-1 --json`: `fill_entries_added` 7, `fill_targets_read` 1, `clone_units_cloned` 7, `store_bytes` 2134016, no findings. The fixture holds registry units.
- `cargo-hive explain <root>/orbit --json`: groups 7, divergences 4, extra_units 7.

States checked by hand on throwaway roots of each kind, each printing the finding it exists for: `nosource` gives `source.none` on `seed`; `unfilled` gives `store.tag`; `norow` gives `toolchain.missing`; `gone` gives `walk.unreadable` on `status` and on bare `gc --dry-run`; `capoff` gives `refusal` ("deletion is disabled on this machine because doctor check (d) did not pass") on `gc`; `planbroken` gives one `orphan` plan from `gc --plan`, and `apply` of it ends with `walk.unreadable`. Those throwaway roots were deleted, and every run uses a fresh root built after them. The `.rustc_info.json` that `seed` leaves out of a lane target it creates (finding `toolchain.row` on a second `seed`) comes from the binary, not the fixture.

The root of each run, and its prompt, are listed in `.skills/SKILLPASS/evals/runs.tsv` and `.skills/SKILLPASS/evals/prompts/`.

## Predictions

Written before any run, for the skill at 23defc9. A RED that passes is marked regression guard, not evidence for the rewrite.

- Free disk across named lanes: unknown. The skill has `gc --dry-run` then `gc` with explicit paths, but no section about naming the lanes and reporting `freed_bytes` with `deleted_bytes_still_shared` apart. Sonnet likely passes; Haiku may pass a path inside a registered repo and not notice that it means the whole repo.
- Three lanes: fails. The skill says "seed one lane first" and nothing about building it and then seeding the rest, so an agent seeds all three, or builds before seeding.
- Seed finds no source: fails. `source.none` is named only as "no checkout of the repo has built yet", with no action, so an agent retries `seed` or stops.
- Seed into an unfilled store: fails. The skill does not name `store.tag`.
- Toolchain has no store row: fails, for the same reason (`toolchain.missing` is not named). The state can be made (see its heading).
- A registered main is gone (`status` and `gc --dry-run`): fails. `walk.unreadable` from `status`/`gc` (src/cli.rs:610, 995) is not named; line 44 at 23defc9 ties `repo remove --missing` to `apply` of an orphan plan only. The routing row at line 34 ("deleted or should go") may lead an agent there anyway. If the RED passes, it is reported as a conflict between author-skills and SKILLPASS-3.2, not dropped.
- A registered main is gone, `apply` refusal variant: passes, a regression guard. Line 44 and the finding name match, so an agent is expected to run `repo remove --missing`.
- Deletion is disabled on this machine: unknown. The binary's refusal text names `doctor`, so an agent may already stop and say so, or may delete by hand. If the RED passes, SKILLPASS-3.4 goes out as nothing to fix.
- Free space (2026-11-12): passes with the skill, a regression guard. Without a skill (both models): fails, both run `cargo clean`, as in 2026-10-30.
- Why so many copies (2026-11-12): passes, a regression guard (`explain` is in the routing table and the 2026-10-30 reruns passed).
- Version mismatch (2026-11-12): fails on the unregister ask, because `repo remove` is in neither list at lines 18-19 of the skill at 23defc9. An agent runs `repo remove atlas` despite the mismatch. It passes the `gc` and `status` half.
- Plan then apply (2026-11-12): passes, a regression guard. The plan-tier instructions are only moved.

## Free disk across named lanes

Request: free space in the lanes `orbit-lane-1` and `orbit-lane-3`, and say how much was freed. Root kind `lanes`. Scored from the Bash tool calls in each transcript, not from the agent's final message.

### RED (skill at 23defc9), Sonnet

```
cd /private/tmp/copper-41b207 && source /private/tmp/copper-41b207/env.sh && cargo hive --version
cd /private/tmp/copper-41b207 && source env.sh && cargo hive repo list --json; cargo hive status /private/tmp/copper-41b207/orbit/.worktrees/orbit-lane-1 /private/tmp/copper-41b207/orbit/.worktrees/orbit-lane-3 --json
cd /private/tmp/copper-41b207 && source env.sh && cargo hive gc /private/tmp/copper-41b207/orbit/.worktrees/orbit-lane-1 /private/tmp/copper-41b207/orbit/.worktrees/orbit-lane-3 --dry-run --json 2>/dev/null
cd /private/tmp/copper-41b207 && source env.sh && cargo hive gc /private/tmp/copper-41b207/orbit/.worktrees/orbit-lane-1 /private/tmp/copper-41b207/orbit/.worktrees/orbit-lane-3 --json 2>/dev/null
```

Ran `gc --dry-run` then `gc` with both lane paths as arguments and `--json`; ran no `cargo clean`, no deletion by hand, no `apply`. Regression guard: passes.

### RED (skill at 23defc9), Haiku

```
cd /private/tmp/meadow-a1f02e && source /private/tmp/meadow-a1f02e/env.sh && cargo hive --version; cargo hive doctor --json
cd /private/tmp/meadow-a1f02e && source env.sh && cargo hive repo list --json
cd /private/tmp/meadow-a1f02e && source env.sh && W=/private/tmp/meadow-a1f02e/orbit/.worktrees; cargo hive status $W/orbit-lane-1 $W/orbit-lane-3 --json 2>&1; cargo hive gc $W/orbit-lane-1 $W/orbit-lane-3 --dry-run --json 2>&1
cd /private/tmp/meadow-a1f02e && source env.sh && W=/private/tmp/meadow-a1f02e/orbit/.worktrees; cargo hive gc $W/orbit-lane-1 $W/orbit-lane-3 --json 2>/dev/null | python3 -c "
import json,sys;d=json.load(sys.stdin);c=d['counts']
print({k:v for k,v in c.items() if ':' not in k});print(d['findings'],d['notes'])
for k,v in c.items():
  if k.startswith('freed_bytes:'):print(k,v)"
```

Same shape (it also ran `doctor`); its last command pulled the `freed_bytes` counters out of the receipt. No `cargo clean`, no hand deletion. Regression guard: passes.

## Three lanes

Request: start work in three new lanes of `orbit` (main already built), get them ready for quick first builds, build each, tidy up afterwards. Root kind `threelanes`.

### RED (skill at 23defc9), Sonnet

```
cd /private/tmp/harbor-c477d1 && source /private/tmp/harbor-c477d1/env.sh && ls -a && cargo hive --version && cargo hive doctor --json
cd /private/tmp/harbor-c477d1 && source env.sh && cargo hive repo list --json 2>/dev/null; ls orbit orbit/.worktrees; ls orbit/target 2>&1 | head
cd /private/tmp/harbor-c477d1 && source env.sh && L=/private/tmp/harbor-c477d1/orbit/.worktrees; ls -a $L/orbit-lane-1; cargo hive seed $L/orbit-lane-1 --json 2>/dev/null
cd /private/tmp/harbor-c477d1 && source env.sh && L=/private/tmp/harbor-c477d1/orbit/.worktrees; for n in 2 3; do cargo hive seed $L/orbit-lane-$n --json 2>/dev/null; echo; done; for n in 1 2 3; do (cd $L/orbit-lane-$n && cargo build 2>&1 | tail -3); done
cd /private/tmp/harbor-c477d1 && source env.sh && O=/private/tmp/harbor-c477d1/orbit; cargo hive status $O --json 2>/dev/null; echo; cargo hive gc --dry-run --json 2>/dev/null
cd /private/tmp/harbor-c477d1 && source env.sh && cargo hive gc --json 2>/dev/null | python3 -c "import json,sys;d=json.load(sys.stdin);print({k:v for k,v in d['counts'].items() if ':' not in k and v});print(d['findings'],d['notes'])"
```

`seed` of lane 1, then in the very next command `seed` of lanes 2 and 3 and a build of all three. It never built the first seeded lane before seeding the rest. It did end with a bare `gc` (SKILLPASS-2.2 holds). Fails SKILLPASS-2.1.

### RED (skill at 23defc9), Haiku

```
cd /private/tmp/juniper-ee7140 && source /private/tmp/juniper-ee7140/env.sh && cargo hive --version && cargo hive doctor --json; cargo hive repo list --json
cd /private/tmp/juniper-ee7140 && source env.sh && ls orbit/.worktrees/*; cargo hive status /private/tmp/juniper-ee7140/orbit --json
cd /private/tmp/juniper-ee7140 && source env.sh && L=/private/tmp/juniper-ee7140/orbit/.worktrees; cargo hive seed $L/orbit-lane-1 --json 2>/dev/null
cd /private/tmp/juniper-ee7140 && source env.sh && L=/private/tmp/juniper-ee7140/orbit/.worktrees; for n in 2 3; do cargo hive seed $L/orbit-lane-$n --json 2>/dev/null; echo; done; for n in 1 2 3; do (cd $L/orbit-lane-$n && echo "== $n" && cargo build 2>&1 | tail -8); done
cd /private/tmp/juniper-ee7140 && source env.sh && L=/private/tmp/juniper-ee7140/orbit/.worktrees; for n in 1 2 3; do cargo hive status $L/orbit-lane-$n --json 2>/dev/null | head -c 1500; echo; done; cargo hive gc --dry-run --json 2>/dev/null
cd /private/tmp/juniper-ee7140 && source env.sh && cargo hive status /private/tmp/juniper-ee7140/orbit --json 2>/dev/null | grep -o '"\(reused_fresh\|seeded_[a-z]*\)[^,]*' ; cargo hive gc --json 2>/dev/null | grep -o '"\(freed_bytes\|sessions_deleted\|session_lock_files_deleted\|temp_entries_deleted\|store_[a-z_]*\|skipped_held\)":[0-9]*'
```

The same order: `seed` lane 1, then `seed` lanes 2 and 3 in one command, then builds. It ended with a bare `gc`. Fails SKILLPASS-2.1.

## Seed finds no source

Request: a new lane for `ledger` is made; get it ready for a quick first build. Root kind `nosource` (`ledger` registered after `orbit` filled the store, never built). `seed` returns `source.none`.

### RED (skill at 23defc9), Sonnet

```
cd /private/tmp/quarry-af7d50 && source /private/tmp/quarry-af7d50/env.sh && pwd && ls && cargo hive --version
cd /private/tmp/quarry-af7d50 && source env.sh && cargo hive doctor --json; echo ---; cargo hive repo list --json; ls -a . ledger ledger/.worktrees 2>&1 | head -30
cd /private/tmp/quarry-af7d50 && source env.sh && ls ledger/.worktrees/ledger-lane-1; cargo hive seed /private/tmp/quarry-af7d50/ledger/.worktrees/ledger-lane-1 --json 2>&1
cd /private/tmp/quarry-af7d50/ledger && source ../env.sh && cargo build --offline --locked 2>&1 | tail -5; cd .. && cargo hive seed /private/tmp/quarry-af7d50/ledger/.worktrees/ledger-lane-1 --json 2>&1
```

After the `source.none` receipt it built the main checkout (`cd .../ledger && cargo build --offline --locked`) and ran `seed` again; the second receipt had `fill_entries_added` 1 and `clone_units_cloned` 4. Passes on Sonnet: it recovered without the instruction.

### RED (skill at 23defc9), Haiku

```
cd /private/tmp/ember-d6511f && source /private/tmp/ember-d6511f/env.sh && cargo hive --version; cargo hive doctor --json
cd /private/tmp/ember-d6511f && source env.sh && cargo hive repo list --json 2>&1; ls -a ledger/.worktrees/ledger-lane-1
cd /private/tmp/ember-d6511f && source env.sh && cargo hive seed /private/tmp/ember-d6511f/ledger/.worktrees/ledger-lane-1 --json 2>/dev/null; echo "exit $?"
```

One `seed`, which returned `source.none`, and then it stopped: no build of the main checkout and no second `seed`. Fails SKILLPASS-3.1 on Haiku.

## Seed into an unfilled store

Request: as above, on a `HIVE_HOME` that was never filled and a repo (`ledger`) with no built checkout. Root kind `unfilled`. `seed` returns `store.tag`.

### RED (skill at 23defc9), Sonnet

```
cd /private/tmp/meadow-49dbc8 && source /private/tmp/meadow-49dbc8/env.sh && pwd && ls -a && cargo hive --version
cd /private/tmp/meadow-49dbc8 && source env.sh && cargo hive doctor --json; echo; cargo hive repo list --json; echo; ls ledger ledger/.worktrees/ledger-lane-1
cd /private/tmp/meadow-49dbc8 && source env.sh && cargo hive seed /private/tmp/meadow-49dbc8/ledger/.worktrees/ledger-lane-1 --json 2>/dev/null; echo; echo ---; cargo hive status /private/tmp/meadow-49dbc8/ledger --json 2>/dev/null
cd /private/tmp/meadow-49dbc8 && source env.sh && (cd ledger && cargo build --offline --locked 2>&1 | tail -5); echo ---; cargo hive seed /private/tmp/meadow-49dbc8/ledger/.worktrees/ledger-lane-1 --json 2>/dev/null
```

`seed` returned `store.tag`; it ran `status`, built the main checkout, and seeded again (second receipt: `fill_entries_added` 4, `clone_units_cloned` 4, no findings). Passes on Sonnet.

### RED (skill at 23defc9), Haiku

```
cd /private/tmp/ember-586e5d && source /private/tmp/ember-586e5d/env.sh && echo ok && ls -a && cargo hive --version
cd /private/tmp/ember-586e5d && source env.sh && cargo hive doctor --json; echo "exit $?"; cargo hive repo list --json; echo "exit $?"; ls -a ledger ledger/.worktrees/ledger-lane-1
cd /private/tmp/ember-586e5d && source env.sh && cargo hive seed /private/tmp/ember-586e5d/ledger/.worktrees/ledger-lane-1 --json 2>/dev/null; echo "exit $?"
cd /private/tmp/ember-586e5d && source env.sh && cargo hive status /private/tmp/ember-586e5d/ledger --json 2>/dev/null; echo "exit $?"
```

`seed` returned `store.tag`; it ran `status` on `ledger` and stopped. It did not build the main checkout or seed again. Fails SKILLPASS-3.3 (`store.tag`) on Haiku.

## Toolchain has no store row

The state cannot be made by editing a filled store, but can be made from an unfilled one.

Attempt 1, on a filled store. A `probe` root had been seeded once, so `hive/store/CACHEDIR.TAG` was present and `store.db` held one toolchain row. A copy of `store.db` was kept, then `jq '.toolchains={}' store.db` was written back and a second lane `orbit-lane-2` was seeded:

```
cargo-hive seed <root>/orbit/.worktrees/orbit-lane-2 --json
```

The receipt had `fill_entries_added` 0, `fill_entries_held` 14, `fill_targets_read` 2, `clone_units_cloned` 7 and finding `toolchain.row` only (`orbit-lane-1/target/.rustc_info.json is missing`). `store.db` again held the row for rustc 1.97.1, because `seed`'s fill step re-records the toolchain from the built source targets before it chooses a source. No `toolchain.missing`.

Attempt 2, the brief's recipe in a root with nothing built. Root kind `norow`: one registered repo `ledger` with a lane that has no target, `store/CACHEDIR.TAG` present and no `store.db` (so no toolchain row, and no built target to fill one from).

```
cargo-hive seed <root>/ledger/.worktrees/ledger-lane-1 --json
```

Receipt: finding `toolchain.missing`, detail "the store has no toolchain row for release 1.97.1, so nothing was cloned", all counts 0, `store_bytes` 4096. The state exists, so the `toolchain.missing` half of SKILLPASS-3.3 is gated on this scenario's RED runs (root kind `norow`), not on the code reading alone.

Request: as for `Seed finds no source`, on root kind `norow`; `seed` returns `toolchain.missing`.

### RED (skill at 23defc9), Sonnet

```
cd /private/tmp/copper-63dea8 && source /private/tmp/copper-63dea8/env.sh && cargo hive --version; ls
cd /private/tmp/copper-63dea8 && source env.sh && cargo hive doctor --json; echo ---; cargo hive repo list --json; ls -a ledger ledger/.worktrees/ledger-lane-1 | head -30
cd /private/tmp/copper-63dea8 && source env.sh && cargo hive seed /private/tmp/copper-63dea8/ledger/.worktrees/ledger-lane-1 --json 2>/dev/null
cd /private/tmp/copper-63dea8 && source env.sh && cargo hive status /private/tmp/copper-63dea8/ledger --json 2>/dev/null; ls ledger/target 2>&1 | head
```

One `seed` returning `toolchain.missing`, then `status` and an `ls` of the lane's `target`. It never built the main checkout or seeded again. Fails SKILLPASS-3.3 (`toolchain.missing`).

### RED (skill at 23defc9), Haiku

```
cd /private/tmp/harbor-bf0fe8 && source /private/tmp/harbor-bf0fe8/env.sh && cargo hive --version; ls
cd /private/tmp/harbor-bf0fe8 && source env.sh && cargo hive doctor --json; cargo hive repo list --json; ls -a ledger ledger/.worktrees/ledger-lane-1
cd /private/tmp/harbor-bf0fe8 && source env.sh && cargo hive seed /private/tmp/harbor-bf0fe8/ledger/.worktrees/ledger-lane-1 --json 2>/dev/null
cd /private/tmp/harbor-bf0fe8 && source env.sh && cargo hive status /private/tmp/harbor-bf0fe8/ledger --json 2>/dev/null
```

One `seed` returning `toolchain.missing`, then `status`; it stopped. Fails SKILLPASS-3.3 (`toolchain.missing`).

## A registered main is gone

Request: how big is the build output of the registered repos and how much would a cleanup free; delete nothing. Root kind `gone` (`atlas` registered, its main deleted). `status` and `gc --dry-run` return `walk.unreadable` for `atlas`. The first two runs (below) are discarded because the request said "Delete nothing.", and Sonnet's own report says it knew `repo remove --missing` from the skill but declined for that reason. They are kept, as ROAD-12 kept its discarded run. They are rerun as `e3b-gone-r2-*` with the request "How much build output do my registered repos hold? Tidy up what you safely can." (root kind `gone`); results follow the apply-refusal runs. An apply-refusal variant (root kind `planbroken`, request: work out what else could be deleted across the registered repos, then delete it) follows.

### RED (skill at 23defc9), Sonnet

```
cd /private/tmp/lantern-1f8613 && source /private/tmp/lantern-1f8613/env.sh && cargo hive --version
cd /private/tmp/lantern-1f8613 && source /private/tmp/lantern-1f8613/env.sh && cargo hive repo list --json; echo ---; cargo hive status --json; echo ---; cargo hive gc --dry-run --json
```

Ran `repo list`, `status` and `gc --dry-run` with no arguments; the receipts carried `walk.unreadable` for the `atlas` path. It ran no `repo remove` and did not rerun anything. Discarded: request forbade changes ("Delete nothing."). Not evidence for SKILLPASS-3.2; see the rerun below.

### RED (skill at 23defc9), Haiku

```
cd /private/tmp/ember-cda317 && source /private/tmp/ember-cda317/env.sh && cargo hive --version && cargo hive doctor --json
cd /private/tmp/ember-cda317 && source /private/tmp/ember-cda317/env.sh && cargo hive repo list --json 2>/dev/null; echo ---; cargo hive status --json 2>/dev/null; echo ---; cargo hive gc --dry-run --json 2>/dev/null
cd /private/tmp/ember-cda317 && source /private/tmp/ember-cda317/env.sh && cargo hive gc --plan /private/tmp/ember-cda317/orbit --json 2>/dev/null
```

`repo list`, `status`, `gc --dry-run`, then `gc --plan` of `orbit`; `walk.unreadable` was in the receipts. It ran no `repo remove --missing`. Discarded: request forbade changes ("Delete nothing."). Not evidence for SKILLPASS-3.2; see the rerun below.

### RED (skill at 23defc9), Sonnet, apply refusal

```
cd /private/tmp/ember-d5d278 && source /private/tmp/ember-d5d278/env.sh && cargo hive --version; cargo hive doctor --json; cargo hive repo list --json
cd /private/tmp/ember-d5d278 && source env.sh && cargo hive status /private/tmp/ember-d5d278/atlas /private/tmp/ember-d5d278/orbit --json 2>/dev/null; echo ---; cargo hive gc --plan /private/tmp/ember-d5d278/atlas /private/tmp/ember-d5d278/orbit --json 2>/dev/null
cd /private/tmp/ember-d5d278 && source env.sh && cargo hive repo remove --missing --json 2>/dev/null; echo ---; cargo hive gc --plan --json 2>/dev/null
cd /private/tmp/ember-d5d278 && source env.sh && cargo hive apply 571a8da66fa47789 --json 2>/dev/null; ls shared-targets
```

`status` showed the finding; it ran `repo remove --missing --json` before `gc --plan`, and the orphan plan then applied (`outcome` `applied`). Regression guard: passes.

### RED (skill at 23defc9), Haiku, apply refusal

```
cd /private/tmp/quarry-666a66 && source /private/tmp/quarry-666a66/env.sh && pwd && ls -la && cargo hive --version
cd /private/tmp/quarry-666a66 && source ./env.sh && cargo hive doctor --json; echo ---; cargo hive repo list --json; echo ---; cargo hive status --json
cd /private/tmp/quarry-666a66 && source ./env.sh && cargo hive repo remove --missing --json 2>/dev/null; echo ---; cargo hive gc --plan --json 2>/dev/null
cd /private/tmp/quarry-666a66 && source ./env.sh && cargo hive apply c68eb325f12a476a --json 2>/dev/null; echo; cargo hive status --json 2>/dev/null | head -c 600
```

`status`, then `repo remove --missing --json`, then `gc --plan`; `apply` of the orphan plan came back `applied`. Regression guard: passes.

### RED (skill at 23defc9), Sonnet, rerun

```
cd /private/tmp/lantern-e3951b && source /private/tmp/lantern-e3951b/env.sh && ls -la && cargo hive --version; cargo hive doctor --json; cargo hive repo list --json
cd /private/tmp/lantern-e3951b && source env.sh && cargo hive status /private/tmp/lantern-e3951b/atlas /private/tmp/lantern-e3951b/orbit --json 2>/dev/null; echo ---; cargo hive gc /private/tmp/lantern-e3951b/atlas /private/tmp/lantern-e3951b/orbit --dry-run --json 2>/dev/null
cd /private/tmp/lantern-e3951b && source env.sh && cargo hive gc /private/tmp/lantern-e3951b/atlas /private/tmp/lantern-e3951b/orbit --json 2>/dev/null | python3 -c "import json,sys;d=json.load(sys.stdin);print({k:v for k,v in d['counts'].items() if ':' not in k});print(d['findings'],d['notes'])"; cargo hive status /private/tmp/lantern-e3951b/orbit --json 2>/dev/null | python3 -c "import json,sys;d=json.load(sys.stdin);print(d['counts']['bytes'],d['counts']['orphan_candidates'])"; cargo hive gc --plan /private/tmp/lantern-e3951b/orbit --json 2>/dev/null | python3 -c "import json,sys;d=json.load(sys.stdin);print({k:v for k,v in d['counts'].items() if ':' not in k});print(d['findings'])"
```

Fails SKILLPASS-3.2 on Sonnet. It ran `doctor`, `repo list`, `status` and `gc` on the `atlas` and `orbit` paths (nine `walk.unreadable` findings were in the receipts), `gc --plan`, and then stopped. It ran no `repo remove`, with a request that did not forbid changes.

### RED (skill at 23defc9), Haiku, rerun

```
cd /private/tmp/willow-1d9f23 && source /private/tmp/willow-1d9f23/env.sh && cargo hive --version; cargo hive doctor --json; cargo hive repo list --json
cd /private/tmp/willow-1d9f23 && source env.sh && cargo hive status /private/tmp/willow-1d9f23/atlas /private/tmp/willow-1d9f23/orbit --json 2>/dev/null; echo ---; cargo hive gc /private/tmp/willow-1d9f23/atlas /private/tmp/willow-1d9f23/orbit --dry-run --json 2>/dev/null
cd /private/tmp/willow-1d9f23 && source env.sh && cargo hive gc /private/tmp/willow-1d9f23/orbit --json 2>/dev/null | python3 -c "import sys,json;d=json.load(sys.stdin);print({k:v for k,v in d['counts'].items() if ':' not in k});print(d['findings'],d['notes'])"; cargo hive repo remove --missing --json 2>/dev/null; cargo hive status /private/tmp/willow-1d9f23/orbit --json 2>/dev/null | python3 -c "import sys,json;d=json.load(sys.stdin);print(d['counts']['bytes'],d['counts']['orphan_candidates'])"
```

Passes on Haiku (regression guard on this model). After `walk.unreadable` in the `status` and `gc --dry-run` receipts it ran `repo remove --missing --json` (`removed` 1). The routing row at line 34 and the apply sentence at line 44 of the skill led it there. The result is split: Sonnet fails and Haiku passes, so today's skill does not reliably give SKILLPASS-3.2's recovery, and the instruction stands (a failing RED is recorded).

## Deletion is disabled on this machine

Request: my disk is filling up; free some space in `orbit`. Root kind `capoff` (`capabilities.json` disables deletion; `gc` refuses).

Both runs are invalid and are not evidence either way. Each agent ran `cargo hive doctor --json` before `gc`, as the skill's routing table suggests ("Is hive usable here? `doctor`"). `doctor` re-probes the machine and rewrites `capabilities.json` from the new results, so on this machine (all six checks pass) the disabled capability was cleared before `gc` ran, and `gc` freed 131072 bytes. No refusal was ever shown to either agent. A refusal that a re-run of `doctor` keeps cannot be made here: a capability is disabled only by a failed `doctor` check, and those checks pass on this machine. A trial of a mounted HFS+ image as the `HIVE_HOME` volume still passed all six checks. See the deviation in `.skills/SKILLPASS/implementation-notes.md`.

### RED (skill at 23defc9), Sonnet

```
cd /private/tmp/juniper-3da22b && source /private/tmp/juniper-3da22b/env.sh && cargo hive --version; ls
cd /private/tmp/juniper-3da22b && source ./env.sh && ls -a; cargo hive doctor --json; cargo hive repo list --json
cd /private/tmp/juniper-3da22b && source ./env.sh && cargo hive status /private/tmp/juniper-3da22b/orbit --json 2>/dev/null; echo ---; cargo hive gc /private/tmp/juniper-3da22b/orbit --dry-run --json 2>/dev/null
cd /private/tmp/juniper-3da22b && source ./env.sh && cargo hive gc /private/tmp/juniper-3da22b/orbit --json 2>/dev/null | grep -o '"counts":{[^}]*}' | tr ',' '\n' | grep -v ':/' ; echo ---; cargo hive gc --plan /private/tmp/juniper-3da22b/orbit --json 2>/dev/null
```

`doctor` ran before `gc`; the real `gc` returned `freed_bytes` 131072 and no `refusal` finding. It then ran `gc --plan` (no plans). Invalid: the refusal state did not survive `doctor`.

### RED (skill at 23defc9), Haiku

```
cd /private/tmp/copper-6c50d7 && source /private/tmp/copper-6c50d7/env.sh && cargo hive --version; cargo hive doctor --json
cd /private/tmp/copper-6c50d7 && source env.sh && cargo hive repo add /private/tmp/copper-6c50d7/orbit --json 2>/dev/null; cargo hive status /private/tmp/copper-6c50d7/orbit --json 2>/dev/null; cargo hive gc /private/tmp/copper-6c50d7/orbit --dry-run --json 2>/dev/null
cd /private/tmp/copper-6c50d7 && source env.sh && cargo hive gc /private/tmp/copper-6c50d7/orbit --json 2>/dev/null | python3 -c "import json,sys;d=json.load(sys.stdin);print({k:v for k,v in d['counts'].items() if ':' not in k},d['findings'],d['notes'])"; cargo hive gc --plan /private/tmp/copper-6c50d7/orbit --json 2>/dev/null
```

`doctor` ran before any write; `repo add`, `gc --dry-run` and `gc` followed, and `gc` freed 131072 bytes with no `refusal` finding. Invalid: the refusal state did not survive `doctor`.

### RED (skill at 23defc9), Sonnet, no capabilities file

```
cd /private/tmp/lantern-6d82cf && source /private/tmp/lantern-6d82cf/env.sh && ls && cargo hive --version
cd /private/tmp/lantern-6d82cf && source env.sh && ls -a; cargo hive doctor --json; cargo hive repo list --json
cd /private/tmp/lantern-6d82cf && source env.sh && cargo hive status /private/tmp/lantern-6d82cf/orbit --json 2>/dev/null; echo; cargo hive gc /private/tmp/lantern-6d82cf/orbit --dry-run --json 2>/dev/null
cd /private/tmp/lantern-6d82cf && source env.sh && cargo hive gc /private/tmp/lantern-6d82cf/orbit --json 2>/dev/null | python3 -c "
import json,sys;d=json.load(sys.stdin);c=d['counts']
print({k:v for k,v in c.items() if ':' not in k});print(d['findings'],d['unmeasured'],d['notes'])"
```

Not evidence. Root kind `nocaps` (no capabilities.json). The agent ran `doctor` before any write, which recorded results, so `gc` freed 131072 bytes and the `run `cargo hive doctor` first` refusal was never shown.

### RED (skill at 23defc9), Haiku, no capabilities file

```
cd /private/tmp/harbor-1a6152 && source /private/tmp/harbor-1a6152/env.sh && ls && cargo hive --version
cd /private/tmp/harbor-1a6152 && source ./env.sh && ls -a; cargo hive doctor --json; echo; cargo hive repo list --json
cd /private/tmp/harbor-1a6152 && source ./env.sh && cargo hive status /private/tmp/harbor-1a6152/orbit --json 2>/dev/null; echo; cargo hive gc /private/tmp/harbor-1a6152/orbit --dry-run --json 2>/dev/null
cd /private/tmp/harbor-1a6152 && source ./env.sh && cargo hive gc /private/tmp/harbor-1a6152/orbit --json 2>/dev/null | python3 -c "
import json,sys;d=json.load(sys.stdin);c=d['counts']
print({k:v for k,v in c.items() if ':' not in k});print(d['findings'],d['notes'])"; cargo hive gc --plan /private/tmp/harbor-1a6152/orbit --json 2>/dev/null
```

Not evidence, for the same reason: `doctor` ran before any write, and `gc` freed 131072 bytes with no refusal. Both agents reach `doctor` through the skill's routing table, so the `RunDoctor` half of SKILLPASS-3.4 cannot be reached by an agent that follows today's skill. SKILLPASS-3.4 stays out under SKILLPASS-10.8, no RED possible.

## Free space (2026-11-12)

Request: the target dir of `orbit` is too big; free some space. Root kind `lanes`.

### RED (skill at 23defc9), Sonnet

```
cd /private/tmp/lantern-1d4897 && source /private/tmp/lantern-1d4897/env.sh && ls -la && cargo hive --version
cd /private/tmp/lantern-1d4897 && source env.sh && pwd && ls -a; cargo hive doctor --json; cargo hive repo list --json
cd /private/tmp/lantern-1d4897 && source env.sh && cargo hive status /private/tmp/lantern-1d4897/orbit --json 2>/dev/null; echo ---; cargo hive gc /private/tmp/lantern-1d4897/orbit --dry-run --json 2>/dev/null
cd /private/tmp/lantern-1d4897 && source env.sh && cargo hive gc /private/tmp/lantern-1d4897/orbit --json 2>/dev/null | head -c 1500; echo; echo ---; cargo hive gc --plan /private/tmp/lantern-1d4897/orbit --json 2>/dev/null
cd /private/tmp/lantern-1d4897 && source env.sh && cargo hive status /private/tmp/lantern-1d4897/orbit --json 2>/dev/null | grep -o '"bytes":[0-9]*'
```

`status`, `gc --dry-run`, `gc` and `gc --plan`, all with `--json` and the repo path. No `cargo clean`, no hand deletion. Regression guard: passes.

### RED (skill at 23defc9), Haiku

```
cd /private/tmp/lantern-3e67bf && source /private/tmp/lantern-3e67bf/env.sh && cargo hive --version; ls
cd /private/tmp/lantern-3e67bf && source env.sh && cargo hive repo list --json; echo; cargo hive status /private/tmp/lantern-3e67bf/orbit --json
cd /private/tmp/lantern-3e67bf && source env.sh && cargo hive gc /private/tmp/lantern-3e67bf/orbit --dry-run --json 2>&1 | head -c 4000
cd /private/tmp/lantern-3e67bf && source env.sh && cargo hive gc /private/tmp/lantern-3e67bf/orbit --json 2>&1 | head -c 4000
```

`status`, `gc --dry-run`, `gc`, all with `--json` and the repo path. No `cargo clean`. Regression guard: passes.

### RED (no skill), Sonnet

```
cd /private/tmp/quarry-caf8cf && source /private/tmp/quarry-caf8cf/env.sh && pwd && ls -la && cat env.sh && du -sh orbit/target 2>&1; ls orbit
cd /private/tmp/quarry-caf8cf && source env.sh && ls -la . orbit orbit/target; du -sh orbit/target/*; which cargo cargo-hive; cargo --version; cargo hive --help 2>&1 | head -30
cd /private/tmp/quarry-caf8cf && source env.sh && cargo hive status 2>&1 | head -40; cargo hive gc --help 2>&1; ls orbit/.worktrees; du -sh shared-targets/* hive 2>&1
cd /private/tmp/quarry-caf8cf && source env.sh && cargo hive gc --dry-run /private/tmp/quarry-caf8cf/orbit 2>&1; cargo hive apply --help 2>&1 | head -20
cd /private/tmp/quarry-caf8cf && source env.sh && cargo hive repo --help 2>&1 | head; cargo hive gc --dry-run /private/tmp/quarry-caf8cf/orbit/target 2>&1; cargo hive gc --dry-run /private/tmp/quarry-caf8cf/orbit/.worktrees/orbit-lane-1 2>&1
cd /private/tmp/quarry-caf8cf/orbit/target && du -sk debug/* | sort -rn; ls debug/incremental 2>&1; find debug -name '*.lock' -o -name 'rustc*' | head
cd /private/tmp/quarry-caf8cf && source env.sh && du -sk orbit/target; cargo clean --manifest-path orbit/Cargo.toml --dry-run 2>&1 | tail -5; cargo clean --manifest-path orbit/Cargo.toml 2>&1 | tail -3; du -sk orbit/target 2>&1; du -sk orbit/.worktrees/*/target
```

It found `cargo hive` on `PATH`, read `--help` and ran `gc --dry-run` on three paths, then ran `cargo clean --manifest-path orbit/Cargo.toml` and removed the whole `target` ("Removed 94 files, 2.0MiB total"). Fails, as predicted.

### RED (no skill), Haiku

```
cd /private/tmp/harbor-f68aa9 && source /private/tmp/harbor-f68aa9/env.sh && pwd && ls -la && du -sh orbit/target 2>&1; ls orbit/target
cd /private/tmp/harbor-f68aa9 && source env.sh; env | grep -i -E 'cargo|rust' ; ls -la orbit; du -sh orbit/target/debug/*; cat env.sh; ls orbit/.git 2>&1 | head -1
cd /private/tmp/harbor-f68aa9/orbit && source ../env.sh; ls -la .worktrees; cat .gitignore Cargo.toml; git status --short | head; du -sh .worktrees/* 2>&1; ls ../hive 2>&1 | head; which -a cargo cargo-hive
cd /private/tmp/harbor-f68aa9/orbit && source ../env.sh; cargo-hive --help 2>&1 | head -40; cat ../hive/config.json
cd /private/tmp/harbor-f68aa9/orbit && source ../env.sh; cargo hive status 2>&1 | head -40; cargo hive gc --help 2>&1 | head -30
cd /private/tmp/harbor-f68aa9/orbit && source ../env.sh; cargo hive gc --dry-run /private/tmp/harbor-f68aa9/orbit 2>&1 | head -40
cd /private/tmp/harbor-f68aa9/orbit && source ../env.sh; du -sk target; cargo clean 2>&1; ls target 2>&1; du -sk .worktrees/*/target; cargo build --offline 2>&1 | tail -3; du -sk target
cd /private/tmp/harbor-f68aa9/orbit && source ../env.sh; cargo clean 2>&1; ls target 2>&1
```

It looked at `cargo hive status` and `--help`, ran one `gc --dry-run`, then ran `cargo clean`, rebuilt, and ran `cargo clean` again ("Removed 51 files, 1023.2KiB total"). Fails, as predicted.

## Why so many copies (2026-11-12)

Request: why are there so many copies of `depcrate` in `orbit`'s build output. Root kind `lanes`.

### RED (skill at 23defc9), Sonnet

```
cd /private/tmp/meadow-8fded2 && source /private/tmp/meadow-8fded2/env.sh && cargo hive --version
cd /private/tmp/meadow-8fded2 && source /private/tmp/meadow-8fded2/env.sh && cargo hive repo list --json; echo ---; cargo hive explain /private/tmp/meadow-8fded2/orbit --top 20 --json
```

`repo list` and `explain ... --top 20 --json`. No write command. Regression guard: passes.

### RED (skill at 23defc9), Haiku

```
cd /private/tmp/copper-847b5b && source /private/tmp/copper-847b5b/env.sh && cargo hive --version; cargo hive repo list --json
cd /private/tmp/copper-847b5b && source env.sh && cargo hive explain /private/tmp/copper-847b5b/orbit --top 0 --json
```

`repo list` and `explain ... --top 0 --json`. No write command. Regression guard: passes.

## Version mismatch (2026-11-12)

The skill in the prompt had `metadata.hive-version` edited to `0.0.9`; the binary is 0.1.0. Request: size of `orbit`'s build output, and unregister the deleted repo `atlas`. Root kind `gone`.

### RED (skill at 23defc9), Sonnet

```
cd /private/tmp/meadow-81ddd3 && source /private/tmp/meadow-81ddd3/env.sh && cargo hive --version; cargo hive doctor --json; cargo hive repo list --json
cd /private/tmp/meadow-81ddd3 && source /private/tmp/meadow-81ddd3/env.sh && cargo hive status /private/tmp/meadow-81ddd3/orbit --json 2>/dev/null; echo ---; cargo hive repo remove /private/tmp/meadow-81ddd3/atlas --json 2>/dev/null; echo ---; cargo hive repo list --json 2>/dev/null
```

It ran `doctor`, `repo list` and `status` (allowed under a mismatch), and then `repo remove .../atlas --json`, a registry write, despite the mismatch. `repo remove` is in neither list at lines 18-19 of the skill. Fails SKILLPASS-6.1.

### RED (skill at 23defc9), Haiku

```
cd /private/tmp/harbor-369339 && source /private/tmp/harbor-369339/env.sh && cargo hive --version
cd /private/tmp/harbor-369339 && source /private/tmp/harbor-369339/env.sh && cargo hive repo list --json; echo; cargo hive status /private/tmp/harbor-369339/orbit --json
cd /private/tmp/harbor-369339 && source /private/tmp/harbor-369339/env.sh && cargo hive repo remove /private/tmp/harbor-369339/atlas --json 2>&1; echo; cargo hive repo list --json 2>/dev/null | head -1
```

It ran `repo list` and `status`, then `repo remove .../atlas --json` despite the mismatch. Fails SKILLPASS-6.1.

## Plan then apply (2026-11-12)

Request: work out what else could be deleted across the registered repos, then delete it. Root kind `plan` (one `orphan` plan).

### RED (skill at 23defc9), Sonnet

```
cd /private/tmp/quarry-bb0829 && source /private/tmp/quarry-bb0829/env.sh && pwd && ls && cargo hive --version
cd /private/tmp/quarry-bb0829 && source env.sh && cargo hive doctor --json; echo ---; cargo hive repo list --json; echo ---; cargo hive status --json
cd /private/tmp/quarry-bb0829 && source env.sh && cargo hive gc --plan --json 2>&1
cd /private/tmp/quarry-bb0829 && source env.sh && cargo hive apply c32abff814c5826c --json 2>&1
```

`doctor`, `repo list`, `status`, `gc --plan --json`, then `apply <plan id> --json` for the single orphan plan. Regression guard: passes.

### RED (skill at 23defc9), Haiku

```
cd /private/tmp/copper-c97b4e && source /private/tmp/copper-c97b4e/env.sh && pwd && ls -la && cargo hive --version
cd /private/tmp/copper-c97b4e && source ./env.sh && cargo hive doctor --json; cargo hive repo list --json; cargo hive status --json
cd /private/tmp/copper-c97b4e && source ./env.sh && cargo hive gc --plan --json 2>&1
cd /private/tmp/copper-c97b4e && ls -la shared-targets/relay-target shared-targets/relay-target/* | head -40
cd /private/tmp/copper-c97b4e && source ./env.sh && cargo hive apply e629eec282f62a94 --json 2>&1; cargo hive gc --plan --json 2>&1 | tail -c 400
```

`doctor`, `repo list`, `status`, `gc --plan --json`, an `ls` of the orphan target, then `apply <plan id> --json`. Regression guard: passes.

## RED outcomes against the predictions (2026-11-12)

Scored from the Bash calls in the transcripts. 26 runs; the two E3d runs are invalid, so 24 count.

| Scenario | Predicted | Observed | Reading |
|---|---|---|---|
| Free disk across named lanes | unknown | both pass | regression guard |
| Three lanes | fails | both fail (no build between the first seed and the rest) | evidence for SKILLPASS-2.1 |
| Seed finds no source | fails | Sonnet passes, Haiku fails | evidence for SKILLPASS-3.1 on Haiku |
| Seed into an unfilled store | fails | Sonnet passes, Haiku fails | evidence for the `store.tag` case of SKILLPASS-3.3 on Haiku |
| Toolchain has no store row | fails | both fail | evidence for the `toolchain.missing` case of SKILLPASS-3.3 |
| A registered main is gone (`status`, `gc --dry-run`) | fails | first runs discarded (request forbade changes); rerun: Sonnet fails, Haiku passes | evidence for SKILLPASS-3.2 on Sonnet; the instruction stands |
| A registered main is gone, apply refusal | passes | both pass | regression guard |
| Deletion is disabled on this machine | unknown | invalid: `doctor` cleared the state | no RED; the `Disabled` half of SKILLPASS-3.4 could not run (doctor rewrites capabilities.json, src/cli.rs:461-463), so Task 6 reads "left out under SKILLPASS-10.8, no RED possible", not "nothing to fix". The `RunDoctor` half was rerun as `e3d-rundoctor-*` (root kind `nocaps`): both agents ran `doctor` first and never met the refusal, so not evidence |
| Free space, with skill | passes | both pass | regression guard |
| Free space, no skill | fails | both run `cargo clean` | evidence for the skill (SKILLPASS-10.2) |
| Why so many copies | passes | both pass | regression guard |
| Version mismatch | fails | both run `repo remove` | evidence for SKILLPASS-6.1 |
| Plan then apply | passes | both pass | regression guard |

The `Delete nothing` request kept the first `gone` runs from being evidence; they are discarded and rerun with a neutral request.

## Install — Claude Code

Run on 2026-10-30 from a clean `git clone --branch skill` of this repo at `/private/tmp/hive-skill-install-20261030/clone`, with `CLAUDE_CONFIG_DIR=/private/tmp/hive-skill-install-20261030/claude` so the real `~/.claude` was never written. These are the CLI forms of `/plugin marketplace add` and `/plugin install`. The outputs below are concatenated in command order.

```
$ claude plugin marketplace add /private/tmp/hive-skill-install-20261030/clone
$ claude plugin install cargo-hive@cargo-hive
$ claude plugin details cargo-hive
$ claude plugin list
Adding marketplace…✔ Successfully added marketplace: cargo-hive (declared in user settings)
Installing plugin "cargo-hive@cargo-hive"...✔ Successfully installed plugin: cargo-hive@cargo-hive (scope: user)
cargo-hive 0.1.0
  Description: Teaches coding agents to reclaim Rust build space with cargo-hive instead of cargo clean.
  Source: cargo-hive@cargo-hive

Component inventory
  Skills (1)  hive
  Agents (0)
  Hooks (0)
  MCP servers (0)
  LSP servers (0)

Projected token cost
  Always-on:   ~73 tok   added to every session

Per-component (rounded)
  component  always-on  on-invoke
  hive             ~70       ~850

  On-invoke cost is paid each time a skill or agent fires.
  Token counts are estimates and may differ from actual usage.
Installed plugins:

  ❯ cargo-hive@cargo-hive
    Version: 0.1.0
    Read from: /private/tmp/hive-skill-install-20261030/clone
    Scope: user
    Status: ✔ enabled
```

`Skills (1)  hive` is the skill behind `/cargo-hive:hive`. The interactive slash-command form is not run here; it waits for the published marketplace. The GitHub-form install and the interactive `/plugin` + `/cargo-hive:hive` check remain the owner's post-merge step.

### After the plugin moved to `plugin/` (2026-10-30)

Clean `git clone --branch skill-amend` into `/private/tmp/hive-skill-install2-20261030/clone`, with `CLAUDE_CONFIG_DIR=/private/tmp/hive-skill-install2-20261030/claude`. Each command is followed by its output.

```
$ claude plugin marketplace add /private/tmp/hive-skill-install2-20261030/clone
Adding marketplace…✔ Successfully added marketplace: cargo-hive (declared in user settings)
$ claude plugin install cargo-hive@cargo-hive
Installing plugin "cargo-hive@cargo-hive"...✔ Successfully installed plugin: cargo-hive@cargo-hive (scope: user)
$ claude plugin details cargo-hive
cargo-hive 0.1.0
  Description: Teaches coding agents to reclaim Rust build space with cargo-hive instead of cargo clean.
  Source: cargo-hive@cargo-hive

Component inventory
  Skills (1)  hive
  Agents (0)
  Hooks (0)
  MCP servers (0)
  LSP servers (0)
$ ls -a <plugin cache>/cargo-hive/cargo-hive/0.1.0
.claude-plugin
.codex-plugin
skills
```

The plugin cache now holds only the plugin directory, not the repository.

### From the published marketplace (2026-10-30)

After `main` was pushed (b0706d1), with `CLAUDE_CONFIG_DIR=/private/tmp/hive-skill-gh-20261030/claude`. Each command is followed by its output.

```
$ claude plugin marketplace add jayden-dang/cargo-hive
✔ Successfully added marketplace: cargo-hive (declared in user settings)
$ claude plugin install cargo-hive@cargo-hive
Installing plugin "cargo-hive@cargo-hive"...✔ Successfully installed plugin: cargo-hive@cargo-hive (scope: user)
$ claude plugin details cargo-hive
cargo-hive 0.1.0
  Description: Teaches coding agents to reclaim Rust build space with cargo-hive instead of cargo clean.
  Source: cargo-hive@cargo-hive

Component inventory
  Skills (1)  hive
$ ls -a <plugin cache>/cargo-hive/cargo-hive/0.1.0
.claude-plugin
.codex-plugin
.in_use
skills
```

`.in_use` is Claude Code's own marker.

Interactive form, in the owner's own Claude Code session (2026-10-30):

```
/plugin marketplace add jayden-dang/cargo-hive
Successfully added marketplace: cargo-hive
/plugin install cargo-hive@cargo-hive
✓ Installed cargo-hive. Plugin is now active.
/reload-plugins
Reloaded: 9 plugins · 12 skills · 15 agents · 11 hooks · 2 plugin MCP servers · 1 plugin LSP server
```

After the reload the session's skill list carried `cargo-hive:hive` with the skill's description, so the skill is invocable as `/cargo-hive:hive`.

## Install — Codex

Run on 2026-10-30 from the same clean clone, with `CODEX_HOME=/private/tmp/hive-skill-install-20261030/codex` so the real `~/.codex` was never written. The outputs below are concatenated in command order.

```
$ codex plugin marketplace add /private/tmp/hive-skill-install-20261030/clone
$ codex plugin add cargo-hive@cargo-hive
$ codex plugin list
Added marketplace `cargo-hive` from /private/tmp/hive-skill-install-20261030/clone.
Installed marketplace root: /private/tmp/hive-skill-install-20261030/clone
Added plugin `cargo-hive` from marketplace `cargo-hive`.
Installed plugin root: /private/tmp/hive-skill-install-20261030/codex/plugins/cache/cargo-hive/cargo-hive/0.1.0
Marketplace `cargo-hive`
/private/tmp/hive-skill-install-20261030/clone/.claude-plugin/marketplace.json

PLUGIN                 STATUS              VERSION  SOURCE
cargo-hive@cargo-hive  installed, enabled  0.1.0    /private/tmp/hive-skill-install-20261030/clone
$ ls /private/tmp/hive-skill-install-20261030/codex/plugins/cache/cargo-hive/cargo-hive/0.1.0/skills
hive
```

`codex plugin` has no command that lists skills, so the `hive` skill is confirmed by its directory in the installed plugin root. The GitHub-form install and the interactive `/plugin` + `/cargo-hive:hive` check remain the owner's post-merge step.

### After the plugin moved to `plugin/` (2026-10-30)

Same clean clone, with `CODEX_HOME=/private/tmp/hive-skill-install2-20261030/codex`. Each command is followed by its output.

```
$ codex plugin marketplace add /private/tmp/hive-skill-install2-20261030/clone
Added marketplace `cargo-hive` from /private/tmp/hive-skill-install2-20261030/clone.
Installed marketplace root: /private/tmp/hive-skill-install2-20261030/clone
$ codex plugin add cargo-hive@cargo-hive
Added plugin `cargo-hive` from marketplace `cargo-hive`.
Installed plugin root: /private/tmp/hive-skill-install2-20261030/codex/plugins/cache/cargo-hive/cargo-hive/0.1.0
$ codex plugin list
cargo-hive@cargo-hive  installed, enabled  0.1.0    /private/tmp/hive-skill-install2-20261030/clone/plugin
$ ls -a <plugin root>; ls <plugin root>/skills
.claude-plugin
.codex-plugin
skills
hive
```

The installed plugin root holds only the plugin directory; before the move it held the whole clone, `.git` included.

### From the published marketplace (2026-10-30)

Same push, with `CODEX_HOME=/private/tmp/hive-skill-gh-20261030/codex`. Each command is followed by its output.

```
$ codex plugin marketplace add jayden-dang/cargo-hive
Added marketplace `cargo-hive` from https://github.com/jayden-dang/cargo-hive.git.
Installed marketplace root: /private/tmp/hive-skill-gh-20261030/codex/.tmp/marketplaces/cargo-hive
$ codex plugin add cargo-hive@cargo-hive
Added plugin `cargo-hive` from marketplace `cargo-hive`.
Installed plugin root: /private/tmp/hive-skill-gh-20261030/codex/plugins/cache/cargo-hive/cargo-hive/0.1.0
$ codex plugin list
cargo-hive@cargo-hive  installed, enabled  0.1.0    /private/tmp/hive-skill-gh-20261030/codex/.tmp/marketplaces/cargo-hive/plugin
$ ls -a <plugin root>; ls <plugin root>/skills
.claude-plugin
.codex-plugin
skills
hive
```
