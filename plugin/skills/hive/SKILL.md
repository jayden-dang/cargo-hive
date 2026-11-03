---
name: hive
description: Reclaim disk space from Rust build output safely. Use when a target/ directory is large, a disk is filling up, you need to reclaim Rust build space, or the same crate is built in many copies across checkouts and worktrees. Use this instead of cargo clean or deleting target/.
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
  - Do not run `gc`, `apply`, `repo add` or `seed`. Ask the user to update first.

## The model

- A checkout is protected (a registered main checkout, or anything hive cannot classify) or secondary (a linked worktree inside its own main, or a checkout under a secondary root, by default `/private/tmp` and `$TMPDIR`). A Cargo target dir that no known checkout owns is not a checkout: `status` reports it as an orphan candidate.
- Actions come in two tiers. The auto tier is safe to run unasked on protected and secondary checkouts alike. `gc` without `--plan` does that tier: it deletes superseded incremental sessions (with their lock files) and leftover rustc temp entries, taking each owner's lock without waiting where one exists and skipping anything held. The plan tier is closure deletion and whole orphan targets. `gc --plan` returns plan ids per class and `undecided` targets; `apply` applies one plan. When hive covers the request, reclaim only through hive, never by other means.
- An agent applies plans of every class itself: `secondary`, `protected` and `orphan`, with no owner step. An `orphan` plan removes the whole target dir, with anything in it no build recreates, so name those targets when you report. After `refused`, plan again and never reuse a consumed id (already consumed). `partial` is not an error; check its reason.
- The repos a command acts on come from its arguments. Never infer them from the current directory.

## Which command

| Question | Command |
|---|---|
| Is hive usable here? | `doctor` |
| What is registered? | `repo list` |
| How big is it, what is orphaned? | `status` |
| How do I free space? | `gc` with `--dry-run` first, then `gc` |
| What can be deleted later? | `gc --plan`, then `apply` one plan |
| Why are there so many copies of a crate? | `explain` |
| How do I seed a new secondary checkout before its first build? | `seed` |

`status`, `gc` and `explain` act on registered repos, so run `repo add` first. Leaving out the repo arguments means every registered repo; for `gc` it means every target hive knows, including orphan candidates and targets under a cache root. Pass explicit paths unless the user asked for everything. `seed` takes one registered checkout and does not mean every repo.

`explain` shows 10 groups by default; `--top 0` shows all of them, as in `cargo hive explain /path/to/repo --top 0 --json`.

`seed` copies registry output from checkouts that have already built into that new checkout. A checkout that already has a target gets nothing. Seed one lane first when starting several.

## Command lines

```
cargo hive doctor --json
cargo hive repo add /path/to/repo --json
cargo hive repo list --json
cargo hive status /path/to/repo --json
cargo hive gc /path/to/repo --dry-run --json
cargo hive gc /path/to/repo --json
cargo hive gc --plan /path/to/repo --json
cargo hive apply plan-id --json
cargo hive explain /path/to/repo --top 20 --json
cargo hive seed /path/to/checkout --json
```

## Reading a receipt

With `--json`, hive prints exactly one JSON object on stdout and its prose on stderr. Read the object and report its counters as measured. Never estimate bytes or time yourself, and say so when a field says a size was not measured.

Exit codes: 0 success; 1 refusal or failure (read the receipt, or stderr if no receipt was printed); 2 unsupported operating system; 130 cancelled.

A seed receipt keeps the fill counts (`fill_entries_added`, `fill_entries_held`, `fill_targets_read`, `fill_targets_skipped`) apart from the clone counts (`clone_units_chosen`, `clone_units_cloned`, `clone_units_dropped`, `clone_units_existing`, `clone_units_failed`, `clone_units_outside_window`). It carries no duration. Afterward, `status` reports `reused_fresh` (seeded, reached, and unchanged, not Cargo's printed fresh count), `seeded_unneeded`, `seeded_recompiled`, and `seeded_missing`.

## Do not

When hive covers the request, do not run `cargo clean`, and do not delete a target dir, an `incremental/` dir, or `rustc*` or `rmeta*` entries yourself. Use `gc` (after `--dry-run`) for the auto tier. Closure deletion and orphan targets go through `gc --plan`, then `apply`, not by hand. Do not seed a checkout that already has a target, since it gets nothing. When starting several lanes, seed one lane first.
