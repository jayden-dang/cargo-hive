---
name: hive
description: Reclaim disk space from Rust build output safely. Use when a target/ directory is large, a disk is filling up, you need to reclaim Rust build space, or the same crate is built in many copies across checkouts and worktrees. Use this instead of cargo clean or deleting target/.
metadata:
  hive-version: "0.1.0"
---

## Command form

Run hive as `cargo hive` followed by a subcommand. Where the harness cannot call a Cargo subcommand, run `cargo-hive` with the same arguments. No `hive` binary exists. Always pass `--json` so the result is one receipt you can read. Run `--help` on a subcommand for flags not listed here.

## Version first

Before the first write, learn the binary's version. Take `hive_version` from any receipt already printed. If none exists, ask the binary for its version: it prints the binary name followed by its version. Compare major and minor with this skill's `metadata.hive-version`.

- Same major and minor: carry on.
- If different:
  - `doctor`, `repo list`, `status` and `explain` may still run. Tell the user both versions and how to update: `/plugin marketplace update cargo-hive` for the plugin, or reinstall the binary.
  - Do not run `gc` or `repo add`. Ask the user to update first.

## The model

- A checkout is protected (a registered main checkout, or anything hive cannot classify) or secondary (a linked worktree inside its own main, or a checkout under a secondary root, by default `/private/tmp` and `$TMPDIR`). Orphan target dirs are not checkouts: they are target dirs no checkout owns.
- Actions come in two tiers. The auto tier is safe to run unasked on protected and secondary checkouts alike; in this version it is all `gc` does: it deletes superseded incremental sessions and leftover rustc temp entries, each under its owner's lock. The plan tier (closure deletion and whole orphan target dirs) needs the owner's approval on protected checkouts and orphan dirs, and is not available yet. Reclaim only through hive, never by other means.
- A result marked `needs-human` is the owner's to resolve. Report it; do not work around it.
- Never run the owner's approval command. It is named here in prose only; an agent does not issue it.
- The repos a command acts on come from its arguments. Never infer them from the current directory.

## Which command

| Question | Command |
|---|---|
| Is hive usable here? | `doctor` |
| What is registered? | `repo list` |
| How big is it, what is orphaned? | `status` |
| How do I free space? | `gc` with `--dry-run` first, then `gc` |
| Why are there so many copies of a crate? | `explain` |

`status`, `gc` and `explain` act on registered repos, so run `repo add` first. Leaving out the repo arguments means every registered repo (for `gc`, every target hive knows). Pass explicit paths unless the user asked for everything.

## Command lines

```
cargo hive doctor --json
cargo hive repo add /path/to/repo --json
cargo hive repo list --json
cargo hive status /path/to/repo --json
cargo hive gc /path/to/repo --dry-run --json
cargo hive gc /path/to/repo --json
cargo hive explain /path/to/repo --top 20 --json
```

## Reading a receipt

With `--json`, hive prints exactly one JSON object on stdout and its prose on stderr. Read the object and report its counters as measured. Never estimate bytes or time yourself, and say so when a field says a size was not measured.

Exit codes: 0 success; 1 refusal or failure (read the receipt, or stderr if no receipt was printed); 2 unsupported operating system; 130 cancelled.

## Do not

When hive covers the request, do not run `cargo clean`, and do not delete a target dir, an `incremental/` dir, or `rustc*` or `rmeta*` entries yourself. Use `gc` (after `--dry-run`) instead.
