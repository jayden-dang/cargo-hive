# cargo-hive

Cargo build artifacts across agent worktrees on macOS, with measured receipts.

It registers repositories, lists their checkouts and Cargo target dirs, and says how
hive may treat each one. `cargo hive gc` deletes superseded rustc incremental sessions
and leftover rustc temp dirs, each under its owner's lock. It compresses and seeds
nothing yet.

## Install from scratch

Requirements: macOS, a Rust toolchain (rustup), and the Xcode Command Line Tools
(`xcode-select --install`), which also build the C parts of the embedded Applesauce
library.

```sh
cargo install --path .
cargo hive doctor          # checks what works on this machine; run it again after a toolchain change
cargo hive repo add ~/code/my-repo
cargo hive status          # every registered repo; add --json for one machine-readable receipt
```

Optional: install the agent skill, so Claude Code and Codex learn to use `cargo hive` instead of
`cargo clean`.

In Claude Code, inside a session:

```
/plugin marketplace add jayden-dang/cargo-hive
/plugin install cargo-hive@cargo-hive
```

In Codex, from a shell:

```sh
codex plugin marketplace add jayden-dang/cargo-hive
codex plugin add cargo-hive@cargo-hive
```

## Commands

| Command | What it does |
|---|---|
| `cargo hive doctor` | Runs checks (a)–(g) in a temporary directory and records which capabilities are enabled for the active toolchain |
| `cargo hive repo add <path>` | Registers the main checkout containing `<path>` |
| `cargo hive repo list` | Lists registered repositories and their root commits |
| `cargo hive status [<repo>…] [--no-sizes]` | Lists checkouts (protected, secondary, missing) and Cargo target dirs (owner or orphan candidate, size, units and edges) |
| `cargo hive gc [<repo>…] [--dry-run]` | Deletes superseded incremental sessions and `deps/rustc*`/`rmeta*` temp dirs. Freed bytes are counted per inode and reported as totals. `--dry-run` deletes nothing |
| `cargo hive explain [<repo>…] [-n N]` | Reports copies of one crate, feature splits, and units whose last compile failed. `-n` defaults to 10; `-n 0` shows every group |

A failure of check (g) disables failed-unit reporting only.

## explain

`cargo hive explain` reports copies of one crate, feature splits, and units whose last compile failed. It never claims bytes, durations, or a tool name, and it never says that a unit always fails or that a copy is an older build. `-n` (`--top`) defaults to 10 groups; `-n 0` shows every group. Naming a failed unit needs doctor check (g); without it the count stays unmeasured and no unit is named.

Every command accepts `--json`. It then prints exactly one receipt on stdout, and every
receipt is also appended to `~/.hive/receipts/<date>.jsonl`. Receipts carry counts and
measured values only, never durations or estimated time saved.

## gc

`cargo hive gc` deletes superseded incremental sessions and leftover rustc temp dirs
inside Cargo target dirs:

- a session that has a lock file is deleted under that lock; a session with no lock file is deleted without one;
- temp entries are deleted under `.cargo-build-lock`;
- held entries are skipped;
- output kept with `-C save-temps` matches the temp-entry rule and is reclaimed;
- targets off APFS are skipped.

## What hive never does

- It installs no Cargo configuration, environment variable, rustc wrapper, cargo shim or
  hook. `cargo` and rust-analyzer build exactly as without hive.
- It writes only under `~/.hive/` and doctor's temporary directory. Outside those, it
  only deletes, through `gc`, inside Cargo target dirs.
- It reads Cargo's internal `.fingerprint/` files only when their exact format is known
  (docs/adr/0001). Anything else is reported as unsupported.

## Configuration

None is required. `~/.hive/config.json` may set `secondary_roots`, which defaults to
`/private/tmp` and `$TMPDIR`, and `cache_roots`, which defaults to
`~/.cache/cargo-target`. A key you set replaces its default list.

`HIVE_HOME` relocates the state directory. It exists for tests only.

## Uninstall

```sh
cargo uninstall cargo-hive && rm -rf ~/.hive
```

To remove the agent skill, run `/plugin uninstall cargo-hive` in Claude Code, or
`codex plugin remove cargo-hive` in Codex.
