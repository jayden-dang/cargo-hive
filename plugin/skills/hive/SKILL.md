---
name: hive
description: Reclaim disk space from Rust build output safely, seed new worktrees or lanes before their first build, and time cold against seeded builds. Use when a target/ directory is large, a disk is filling up, you need to reclaim Rust build space, the same crate is built in many copies across checkouts and worktrees, or you start several lanes. Use this instead of cargo clean or deleting target/.
metadata:
  hive-version: "0.1.0"
---

## Command form

Run hive as `cargo hive` followed by a subcommand. Where the harness cannot call a Cargo subcommand, run `cargo-hive` with the same arguments. No `hive` binary exists. Always pass `--json` so the result is one receipt you can read. Run `--help` on a subcommand for flags not listed here.

## Version first

Before the first write, learn the binary's version. Take `hive_version` from any receipt already printed. If none exists, run `cargo hive --version`; it prints the binary name followed by its version. If the binary is missing or the command fails, stop and tell the user to install hive. Compare major and minor with this skill's `metadata.hive-version`.

- Same major and minor: carry on.
- If different:
  - `doctor`, `repo list`, `status` and `explain` may still run. Tell the user both versions and how to update: `/plugin marketplace update cargo-hive` for the plugin, or reinstall the binary.
  - Do not run `gc`, `apply`, `repo add`, `repo remove`, `seed` or `bench`. Ask the user to update first.

## Free disk

The repos a command acts on come from its arguments. Never infer them from the current directory. `status`, `gc` and `explain` act on registered repos, so run `repo add` first. Leaving out the repo arguments means every registered repo; for `gc` it means every target hive knows, including orphan candidates and targets under a cache root. Pass explicit paths unless the user asked for everything. A path inside a registered repo, or a linked worktree of one, makes `gc` act on that whole repo, with a note.

To free disk on the checkouts the user named, run `gc --dry-run` with those checkouts as arguments, then `gc` with the same arguments. The auto tier is safe to run unasked on every checkout, protected or secondary (both defined under Look before acting). `gc` without `--plan` deletes superseded incremental sessions (with their lock files) and leftover rustc temp entries, taking each owner's lock without waiting where one exists and skipping anything held.

`gc` with repo arguments leaves the store alone and notes `store.skipped_scoped`. Bare `gc` (no repo) also bounds the store, evicting by age, toolchain and the 40 GB cap; `gc --dry-run` previews that.

Report freed bytes from the `freed_bytes` count of the real `gc` run's receipt, not the dry run's, and read it from the whole output, never from output you cut short. Report `deleted_bytes_still_shared` apart. When the receipt lists `freed_bytes` under `unmeasured`, say the size was not measured. Report the store's freed bytes from a bare `gc` as `store_freed_bytes`.

When hive covers the request, reclaim only through hive: do not run `cargo clean`, and do not delete a target dir, an `incremental/` dir, or `rustc*` or `rmeta*` entries yourself, and do not delete `~/.hive/store/` by hand.

```
cargo hive repo add /path/to/repo --json
cargo hive gc /path/to/lane-a /path/to/lane-b --dry-run --json
cargo hive gc /path/to/lane-a /path/to/lane-b --json
```

Only when the user asked for everything:

```
cargo hive gc --json
```

## Start new lanes

Use `seed` to fill a new secondary checkout before its first build. It takes one registered checkout and does not mean every repo. It clones registry units from the store into that new checkout, taking them from the record of one checkout that has already built: the one whose `Cargo.lock` matches, else the most recently built, so every cloned unit comes from one build. A checkout that already has a target gets nothing, so do not seed it.

When starting several lanes, seed one lane first, build it, then seed the other lanes. Run bare `gc` after the batch of lanes finishes, and whenever `seed`'s `store_bytes` passes 40 GB. Read the seed receipt, and the `status` counts afterward, under Reading a receipt.

```
cargo hive seed /path/to/checkout --json
```

## Delete what gc keeps

The plan tier is closure deletion and whole orphan targets. These go through `gc --plan`, then `apply`, not by hand. `gc --plan` returns plan ids per class and `undecided` targets; `apply` applies one plan.

An agent applies plans of every class itself: `secondary`, `protected` and `orphan`, with no owner step. An `orphan` plan removes the whole target dir, with anything in it no build recreates, so name those targets when you report. After `refused`, plan again and never reuse an id the binary reported as `already_consumed`. `partial` is not an error; check its reason.

```
cargo hive gc --plan /path/to/repo --json
cargo hive apply plan-id --json
```

## Look before acting

A checkout is protected (a registered main checkout, or anything hive cannot classify) or secondary (a linked worktree inside its own main, or a checkout under a secondary root, by default `/private/tmp` and `$TMPDIR`). A Cargo target dir that no known checkout owns is not a checkout: `status` reports it as an orphan candidate.

| Question | Command |
|---|---|
| Is hive usable here? | `doctor` |
| What is registered? | `repo list` |
| A registered repo was deleted or should go | `repo remove <path>`, or `repo remove --missing` |
| How big is it, what is orphaned? | `status` |
| How do I free space? | `gc --dry-run`, then `gc` |
| What can be deleted later? | `gc --plan` |
| Why are there so many copies of a crate? | `explain` |
| A new secondary checkout, before its first build | `seed` |
| Cold against seeded build times | `bench` |

`explain` shows 10 groups by default; `--top 0` shows all of them, as in `cargo hive explain /path/to/repo --top 0 --json`.

```
cargo hive doctor --json
cargo hive repo list --json
cargo hive repo remove --missing --json
cargo hive status /path/to/repo --json
cargo hive explain /path/to/repo --top 20 --json
```

## Time cold against seeded builds

`bench` times cold and seeded first builds on fresh secondary checkouts. Run `bench` only when asked, because it makes 2n full builds. Make an even number of fresh checkouts per arm with `git worktree add` (2n total), then remove them after with `git worktree remove` (which also deletes a gitignored target). Builds run `--offline --locked`, so the repo needs a committed Cargo.lock and its dependencies in Cargo's cache; building the main checkout once with the same command does both and fills the store. `--build-cmd` picks one of five shapes: `build`, `check-all`, `clippy-all`, `test-no-run`, or `check-lib=<package>`.

```
cargo hive bench --cold /path/to/cold1 /path/to/cold2 --seeded /path/to/seeded1 /path/to/seeded2 --json
```

## When a command refuses

- `source.none` means the repo has no source target record for the current toolchain: no checkout of it has built yet, or none has with this toolchain. Build the main checkout, then run `seed` again; do not retry `seed` first.
- `store.tag` with a missing tag (the store has not been filled yet) and `toolchain.missing` (the store has no row for this toolchain) take the same recovery: build the main checkout with the current toolchain, then run `seed` again. When `store.tag` says the tag could not be read, report the finding's detail to the user, since a build does not fix it.
- A registered repo whose main checkout is gone makes `status`, `gc`, `gc --plan`, `explain` and `apply` report `walk.unreadable` for its path. Run `cargo hive repo remove --missing --json`, which unregisters every such repo, then rerun the command.
- Finding `store.busy` means run `gc` again when no seed runs.
- `store.held_over_cap` names the targets to retire.

Exit codes: 0 success; 1 refusal or failure (read the receipt, or stderr if no receipt was printed); 2 unsupported operating system; 130 cancelled.

## Reading a receipt

With `--json`, hive prints exactly one JSON object on stdout and its prose on stderr. Read the object and report its counters as measured. Never estimate bytes or time yourself, and do not fill in a size the receipt lists under `unmeasured`. Times come only from `bench`. Counts sit in the receipt's `counts` object. A count per target or per profile carries a suffix after its name, a colon and a path (`reused_fresh:/path/to/target`).

A seed receipt keeps the fill counts (`fill_entries_added`, `fill_entries_held`, `fill_targets_read`, `fill_targets_skipped`) apart from the clone counts (`clone_units_chosen`, `clone_units_cloned`, `clone_units_dropped`, `clone_units_existing`, `clone_units_failed`, `clone_units_outside_window`). It carries no duration. Afterward, `status` reports `reused_fresh` (seeded, reached, and unchanged, not Cargo's printed fresh count), `seeded_unneeded`, `seeded_recompiled`, and `seeded_missing`.

A bare `gc` receipt reports store counts `store_entries_evicted_age`, `store_entries_evicted_toolchain`, `store_entries_evicted_cap`, `store_entries_evicted_rowless`, `store_toolchains_dropped`, `store_entries_held`, `store_entries_held_live`, `store_entries_held_undecided`, `store_entries_held_name_meta`, `store_entries_held_busy`, `store_entries_held_whole_store`, `store_freed_bytes`, `store_bytes_still_shared`, `store_freeable_bytes_remaining`, and `store_tmp_bytes`.

A bench receipt starts with a prime, then one `bench.sample` finding per sample with its time series. Counts give `cold_build_ms_median`, `cold_build_ms_p90`, `cold_build_ms_min`, `cold_build_ms_max`, `cold_build_ms_spread`, `seeded_build_ms_median`, `seeded_build_ms_p90`, `seeded_build_ms_min`, `seeded_build_ms_max`, `seeded_build_ms_spread`, `seeded_seed_ms_median`, `seeded_seed_ms_p90`, `seeded_seed_ms_min`, `seeded_seed_ms_max`, `seeded_seed_ms_spread`, `seeded_total_ms_median`, `seeded_total_ms_p90`, `seeded_total_ms_min`, `seeded_total_ms_max` and `seeded_total_ms_spread`; p90 is the max for n ≤ 9. Each seeded sample also carries `seeded_fresh`, `seeded_unneeded`, `seeded_recompiled`, `seeded_missing` and `fill_targets_read`; bench's `seeded_fresh` and `status`'s `reused_fresh` are different names. Seed and fill times rise through a run as finished samples add targets to scan, shown by `fill_targets_read`.
