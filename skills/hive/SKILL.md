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
- Different: `doctor`, `repo list`, `status` and `explain` may still run. Tell the user both versions and how to update: `/plugin marketplace update cargo-hive` for the plugin, or reinstall the binary.
- Different: do not run `gc` or `repo add`. Ask the user to update first.

## The model

- Checkouts are protected (the main checkout, and orphan target dirs with no checkout) or secondary (extra worktrees, such as an agent lane). Reclaiming from protected ones needs the owner's approval; secondary ones an agent may reclaim itself.
- Actions come in two tiers. The auto tier is safe to run unasked; in this version it is all `gc` does: it deletes superseded incremental sessions and leftover rustc temp entries, each under its owner's lock. The plan tier (larger reclaims that need approval) is not available yet.
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

Exit codes: 0 success; 1 refusal or failure (the receipt says why); 2 unsupported operating system; 130 cancelled.

## Do not

When hive covers the request, do not run `cargo clean`, and do not delete a target dir, an `incremental/` dir, or `rustc*` or `rmeta*` entries yourself. Use `gc` (after `--dry-run`) instead.
