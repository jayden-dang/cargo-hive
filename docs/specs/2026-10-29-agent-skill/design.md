# Design: Agent skill (the hive skill, shipped as a plugin)

Feature code: SKILL
Status: Approved
Date: 2026-10-29
Requirements: ./requirements.md

## Context

Nothing in the repo tells an agent how to use hive today. The CLI is self-describing only through clap's `--help` (`src/cli.rs:23-77`). Every `--json` run prints one receipt on stdout carrying `hive_version` from `CARGO_PKG_VERSION` (`src/receipt.rs:77`), and `cargo-hive --version` prints `cargo-hive 0.1.0`. Exit codes follow `docs/standards/errors-logging.md:17`: 0 ok, 1 refusal or failure, 2 unsupported OS, 130 cancel. The model an agent must work under lives only in `docs/product/vision.md` and the ARCH spine: protected versus secondary checkouts, auto versus plan tier, `needs-human`, no approval by agents (ARCH-12), and scope from arguments (ARCH-8).

The binding constraint is ARCH-2, together with CORE-6.3: hive writes only to its state root, to doctor's temp dir, and, by deleting or cloning, to Cargo target dirs. A binary that writes `~/.claude/skills/` would break that rule. The frame-change therefore rejected embedding the skill and adding `cargo hive skill install` (option A). It chose the pattern the owner's `jayden-dang/skills` repo already runs: the repo is its own plugin marketplace, and each harness's plugin manager installs, updates and removes the skill. That leaves a second constraint. The skill and the binary now arrive through two channels (`/plugin update` and `cargo install`), so drift has to be caught twice. At build time `tests/skill.rs` checks that versions are equal and that every command line parses with the real parser. At run time the skill tells the agent to compare `hive_version` and refuse writes on a major.minor mismatch.

Fresh retrieval (load-subgraph `fsubr-1.2`, seeds SKILL, `skill`, `plugin`, `marketplace`, `--json`, `try_parse_from`, `CARGO_PKG_VERSION`, `approve`; paths `src/cli.rs`, `tests/skill.rs`, `README.md`, `skills/hive/SKILL.md`). It returned no path overlap with CORE, AUTOGC or EXPLAIN, because SKILL creates only new files. CORE, AUTOGC and EXPLAIN are term-only neighbours: CORE owns the parser and `CARGO_PKG_VERSION`, and AUTOGC and EXPLAIN own commands the skill documents. `README.md` is owned by all three, and this feature edits only its install section. `owns_coverage` 3/4: SKILL has no `tasks.md` yet. Observations: none.

This feature relies on ARCH-2, ARCH-8, ARCH-10 and ARCH-12, and changes none of them.

## Decisions

1. **Distribution.** The skill ships as plugin `cargo-hive` from a marketplace named `cargo-hive` in this repo: `.claude-plugin/` for Claude Code, and `.codex-plugin/` for Codex. Codex also reads the Claude marketplace file, which the requirements review confirmed on codex-cli 0.165.0. No `src/` code changes, and ARCH-2 stays as written. This is not an ADR: Compat obligation is None, and switching back to an installer later would only add code, not undo any.
2. **One home per field.** The `version` and the skills list live only in each `plugin.json`. The marketplace entry carries neither, as the Claude Code docs advise against setting `version` in both places. All of these must equal `Cargo.toml`: `.claude-plugin/plugin.json` `version`, `.codex-plugin/plugin.json` `version`, and the skill's `metadata.hive-version`. The owner's `jayden-dang/skills` repo is the reference for layout only. It sets `version` in both places, and its Claude and Codex manifests have already drifted (1.4.0 vs 1.3.0), which is the failure SKILL-4.1 exists to catch.
3. **Names.** The plugin is `cargo-hive` (the crate), the skill is `hive` (the product), and it is invoked as `/cargo-hive:hive`. The skill's first body line says the command is `cargo hive`.
4. **Mismatch rule Z is text, not code.** The skill tells the agent to compare major.minor of `hive_version` with `metadata.hive-version`. On a mismatch, read commands run with a warning, and `gc` and `repo add` do not run. hive itself enforces nothing here.
5. **The drift test parses through the real parser and matches commands exhaustively.** `tests/skill.rs` calls `cargo_hive::cli::Cli::try_parse_from`. It classifies each parsed command with a `match` on `cargo_hive::cli::Command` that has no wildcard arm. When a later item adds a subcommand (ROAD-6 plan/apply, ROAD-7 approve, ROAD-9 seed), `tests/skill.rs` stops compiling until someone decides whether the skill names it. That is the roadmap rule "each later item extends the skill", enforced by the compiler.
6. **No YAML dependency.** The frontmatter is a file this repo owns, so `tests/skill.rs` reads only a strict subset: top-level `key: value` lines, plus one `metadata:` block of two-space-indented `key: "value"` lines. Anything else fails the test as an unsupported line. Adding `serde_yaml` or a similar crate would be a new dependency, which is the owner's call, and nothing here needs it.
7. **Command lines use literal example values, and names stay bare.** A placeholder in a subcommand or typed-flag position fails to parse (`cargo hive <sub>` gives `InvalidSubcommand`). So every command line spells its subcommand and uses literal values for typed flags, such as `/path/to/repo` and whatever numeric flag EXPLAIN ships with a number. When the skill names the command itself, it writes the bare span `cargo hive` or `cargo-hive`, with nothing after it. The extractor only takes spans that continue past `cargo hive ` (SKILL-4.2), so a bare name is never read as a command line.
8. **Agent runs are made by hand**, with the `skill-tester` (Sonnet) and `skill-tester-haiku` agents, and recorded in `skills/hive/TESTS.md`. `eval.json` follows the owner's `lint-skill-evals.py` fields. CI does not run agents (requirements Out of Scope).

## Architecture

Vocabulary for every section below: **module**, **interface**, **implementation**,
**seam** (public surface where behavior is observable and substitutable). Prefer
deep modules — an interface much simpler than what it hides.

### Plugin manifests

Satisfies: SKILL-1.1, SKILL-1.2
Reuse: rung 4 — the Claude Code and Codex plugin systems, laid out as in the owner's `jayden-dang/skills` (`.claude-plugin/marketplace.json`, `.claude-plugin/plugin.json`, `.codex-plugin/plugin.json`)
Interface: marketplace `cargo-hive` → plugin `cargo-hive` (source `./`) → skill `hive`
Depth: n/a — extends the harnesses' plugin systems; the files are configuration they read
Locality: three new JSON files at the repo root; no existing module touched (leave)

```
.claude-plugin/marketplace.json  {"name": "cargo-hive", "owner": {...},
                                  "plugins": [{"name": "cargo-hive", "source": "./", "description": "..."}]}
.claude-plugin/plugin.json       {"name": "cargo-hive", "version": "0.1.0", "description": "...",
                                  "skills": ["./skills/hive"]}
.codex-plugin/plugin.json        {"name": "cargo-hive", "version": "0.1.0", "description": "...",
                                  "skills": "./skills/", "interface": {"displayName": "cargo-hive", ...}}
```

The marketplace entry has no `version` and no `skills` (Decision 2). The Codex manifest copies the `interface` block the skills repo uses, with only the fields that repo fills.

### The skill (`skills/hive/SKILL.md`)

Satisfies: SKILL-2.1, SKILL-2.2, SKILL-2.3, SKILL-2.4, SKILL-2.5, SKILL-2.6, SKILL-3.1, SKILL-3.2, SKILL-3.3, SKILL-6.1, SKILL-6.2
Reuse: rung 4 — the Agent Skills format (frontmatter plus a markdown body), read natively by both harnesses
Respects: ARCH-2, ARCH-8, ARCH-10, ARCH-12
Interface: frontmatter `name: hive`, `description`, `metadata.hive-version`; the body the agent reads on load
Depth: n/a — content in a platform format. Its job is to be the agent's whole interface to hive, so it carries the model and the command lines, and points to `--help` for the rest.
Locality: one new file (leave)

Frontmatter (the whole of it):

```
---
name: hive
description: <triggers: a large target/ dir, a disk filling up, reclaiming Rust build space, a crate with many copies; used instead of cargo clean or deleting target/>
metadata:
  hive-version: "0.1.0"
---
```

There is no `allowed-tools` key (SKILL-6.2). The body is at most 150 lines (SKILL-6.1), in this order:

1. **Command form** (SKILL-2.2): run hive as `cargo hive` followed by a subcommand, or as `cargo-hive` with the same arguments; no `hive` binary exists. Both names appear as bare spans (Decision 7).
2. **Version first** (SKILL-3.1–3.3): take `hive_version` from any receipt, or from `cargo hive --version` when no receipt has been printed. The command appears as an inline span; a missing binary or failing command means stop and tell the user to install hive. That command prints the binary name followed by its version; the skill says so in prose, not in a code span, which the extractor would read as a command line. Compare major.minor with `metadata.hive-version`. On a mismatch, `doctor`, `repo list`, `status` and `explain` may still run: tell the user both versions and `/plugin marketplace update cargo-hive`. `gc` and `repo add` do not run; ask the user to update first.
3. **The model** (SKILL-2.3): protected and secondary checkouts; the auto tier (all `gc` does in MILE-1) versus the plan tier (not yet available); `needs-human` is the owner's to resolve; never run the owner-approval command, named in prose only; repos come from arguments, never from the current directory.
4. **Which command for which question**: a short table. "Is hive usable here" → `doctor`. "What is registered" → `repo list`. "How big, what is orphaned" → `status`. "Free space" → `gc --dry-run`, then `gc`. "Why so many copies" → `explain`.
5. **Command lines** (SKILL-2.4): one fenced block with `doctor`, `repo add`, `repo list`, `status`, `gc --dry-run`, `gc` and `explain` (with EXPLAIN's shipped flags, literal values per Decision 7), each with `--json`; the commands that take repos carry a literal path. Lines are written plainly: no `$ ` prompt, no environment prefix, no indentation, no `\` continuation (the test rejects them, see below).
6. **Reading a receipt** (SKILL-2.5): one JSON object on stdout, prose on stderr; report its counters as measured; exit codes 0, 1, 2 and 130.
7. **Do not** (SKILL-2.6): `cargo clean`, or deleting a target dir, an `incremental/` dir or `rustc*`/`rmeta*` entries yourself, when hive covers the request.

### Drift test (`tests/skill.rs`)

Satisfies: SKILL-4.1, SKILL-4.2, SKILL-4.3, SKILL-4.4, SKILL-5.3
Reuse: rung 2 — `cargo_hive::cli::{Cli, Command, RepoCommand}` (`src/cli.rs:17,23,69`, all `pub`) for parsing. Rung 4 — `env!("CARGO_PKG_VERSION")`, which Cargo sets for integration tests. Rung 5 — `serde_json` (already a dependency) for the manifests and `eval.json`. Rung 7 — two small readers, the frontmatter subset (Decision 6) and command-line extraction, because nothing installed does either.
Respects: ARCH-8 (the skill's command lines carry path arguments; the test parses them as the binary would)
Interface: `cargo test --test skill`. Each failure message names the file, the rule and the offending line or value.
Depth: if this file vanished, a maintainer rebuilding it would need only four rules: versions equal `CARGO_PKG_VERSION`; frontmatter keys and limits; the command-line grammar of SKILL-4.2 with the allowed subcommand set; the `eval.json` fields. The parser itself is reused, not reimplemented.
Locality: one new test file; reads `src/cli.rs` types without changing them (leave). Repo files are located from `env!("CARGO_MANIFEST_DIR")`.

Tests, one rule each:

- `manifest_versions_match_the_crate` (SKILL-4.1): the two `plugin.json` `version` values and `metadata.hive-version` equal `CARGO_PKG_VERSION`, and the marketplace entry has no `version`. The same test also checks the shapes of SKILL-1.1 and SKILL-1.2: the names, `source: "./"`, the skills paths, and no `skills` in the marketplace entry. Added at the whole-branch review: the three manifest `description` strings are equal, and `skills/` holds exactly one entry, `hive`, so the Codex `./skills/` path loads only the hive skill.
- `skill_command_lines_parse_with_the_cli` (SKILL-4.2, SKILL-4.4): extract command lines from fenced blocks and inline spans. A command line starts with `cargo hive ` or `cargo-hive `. Its words are split on whitespace and cut before the first word that starts with `|`, `&&` or `#`, or contains `>`, which covers `2>&1`. Any other occurrence of `cargo hive ` or `cargo-hive ` in the skill's body also fails the test (the frontmatter is checked separately), naming the line. That covers prose outside code, a `$ ` prompt, an environment prefix, an indented line and a `\` continuation, so no mention escapes the parse. Each is parsed as `["cargo-hive", words…]` through `Cli::try_parse_from`. An `Err` whose kind is `DisplayHelp` or `DisplayVersion` counts as parsed (the parsed form is `Info`). Each `Ok` goes through an exhaustive `match` on `Command` (Decision 5): `Doctor`, `Repo { Add | List }`, `Status`, `Gc` and `Explain` are allowed; `HoldLock` and `TryLock` fail the line. Failure messages print the line text, since `Cli` and `Command` derive no `Debug`.
- `skill_documents_every_shipped_command` (SKILL-2.4): the accepted lines together cover `doctor`, `repo add`, `repo list`, `status`, `gc` with `--dry-run`, `gc` without it, and `explain`; every one of them carries `--json`; `repo add`, `status`, `gc` and `explain` lines carry at least one path argument.
- `skill_frontmatter_follows_the_spec` (SKILL-4.3, plus the SKILL-2.1 key set and SKILL-6.2): `name` equals the directory and is at most 64 characters of `[a-z0-9]` with single hyphens; `description` is at most 1024 characters, and mentions `target` and `cargo clean` (the structural half of SKILL-2.1; whether it triggers at the right moment is the `trigger` evals' job); any `compatibility` is at most 500; keys come only from `name`, `description`, `license`, `compatibility` and `metadata`.
- `skill_body_fits_the_budget` (SKILL-6.1): the body is at most 150 lines and names no other file to read: no markdown link to a local path, and no `.md` path anywhere in the body. Pointing the agent at a file another way (prose naming a path without `.md`) is left to review.
- `eval_entries_carry_their_evidence` (SKILL-5.3): `eval.json` is a non-empty array. Each entry carries `eval_id` (unique, positive), `eval_name` (kebab-case), `kind` (`behavior`, `trigger` or `contract`), `derived_from` naming an existing `TESTS.md` heading, `prompt`, and non-empty `assertions`. There is at least one `behavior` entry and at least one `trigger` entry. Each `contract` entry's assertions quote skill text as `The skill says: "…"`, and every quote must be a substring of the skill body.

Until EXPLAIN merges, `Command` has no `Explain` variant. The skill's `explain` line then fails `skill_command_lines_parse_with_the_cli`, as the requirements note. The test is written after ROAD-3 lands (ROAD-12 `After: ROAD-3`).

### Agent runs (`skills/hive/TESTS.md`, `skills/hive/eval.json`)

Satisfies: SKILL-1.3, SKILL-1.4, SKILL-5.1, SKILL-5.2
Reuse: rung 2 — the owner's skill-testing practice: a `TESTS.md` RED/GREEN record plus `eval.json`, run by the `skill-tester` and `skill-tester-haiku` agents
Interface: two files a reader opens; `eval.json` is also read by `tests/skill.rs`
Depth: n/a — evidence records, not code
Locality: two new files beside the skill (leave)

`TESTS.md` holds these headings:

- `## RED — no skill`: Sonnet and Haiku runs, without the skill, of "the target dir of `<repo>` is too big; free some space" against a registered fixture repo with a large target.
- `## GREEN — skill loaded`: the same request with the SKILL.md body in context. The agent's commands are quoted verbatim, and the run is checked against SKILL-5.2.
- `## Install — Claude Code` and `## Install — Codex`: the commands run and what the harness listed afterwards (SKILL-1.3, SKILL-1.4). For Claude Code that includes `claude plugin details cargo-hive`, which must list `Skills (1) hive` and shows the skill's always-on token cost.

The fixture and the runner's environment, set up before each run and never taught by the skill:

- **Binary on PATH.** `cargo install --path . --root <tmp>/bin`, with `<tmp>/bin/bin` prepended to the agent's `PATH`. `cargo-hive` is not installed on this machine, and `cargo run` is not the form the skill teaches.
- **Scratch state.** `HIVE_HOME=<tmp>/hive` exported in the agent's shell. README calls `HIVE_HOME` test-only, so the skill never mentions it.
- **A target with something to free.** A scratch git repo with one crate, built, edited and built again so `target/debug/incremental/` holds a superseded session (`src/gc/plan.rs:158` frees nothing otherwise). `tests/common/mod.rs` `plant_session` is the model for that state.
- **Ready hive.** The runner runs `cargo hive doctor --json` and `cargo hive repo add <scratch repo> --json` under the scratch `HIVE_HOME` before the agent starts.
- **Afterwards.** The runner records that `~/.hive` is unchanged: same file list and sizes as before the run.

`eval.json` holds:

- `behavior` entries for "free space", "why so many copies" and "version mismatch before gc";
- `trigger` entries: "my disk is full from Rust builds" should load the skill, "fix this borrow-checker error" should not;
- `contract` entries quoting the skill's never-approve and never-cwd lines.

Each entry's `derived_from` names its `TESTS.md` heading.

### README install section

Satisfies: none — documentation
Reuse: rung 2 — the existing `README.md` install section
Surface: `README.md` install steps (readers: humans): replace. Add the two plugin-install pairs after `cargo install`, and note that uninstalling hive (`rm -rf ~/.hive`, `README.md:68`) leaves the plugin to be removed with `/plugin uninstall cargo-hive`.
Interface: n/a
Depth: n/a — extends README
Locality: README install and uninstall paragraphs (extend)

### Existing CLI (unchanged)

Satisfies: SKILL-1.5
Reuse: rung 2 — the existing guard tests: `tests/guards.rs` (CORE-6.3, extended by EXPLAIN-6.2) and `tests/gc.rs` (AUTOGC-6.1)
Respects: ARCH-2
Interface: n/a — no `src/` file changes
Depth: n/a
Locality: none (leave)

## Seams for testing

| Seam | Kind | Covers |
|---|---|---|
| `tests/skill.rs` reading the repo files and `cargo_hive::cli::Cli` | integration | SKILL-1.1, SKILL-1.2, SKILL-2.1, SKILL-2.4, SKILL-4.1, SKILL-4.2, SKILL-4.3, SKILL-4.4, SKILL-5.3, SKILL-6.1, SKILL-6.2 |
| Agent runs recorded in `skills/hive/TESTS.md` (skill-tester Sonnet and Haiku), plus `eval.json` `trigger` and `contract` entries | e2e (manual) | SKILL-2.1 (when-to-use wording), SKILL-2.2, SKILL-2.3, SKILL-2.5, SKILL-2.6, SKILL-3.1, SKILL-3.2, SKILL-3.3, SKILL-5.1, SKILL-5.2 |
| Harness plugin install, recorded in `skills/hive/TESTS.md` | e2e (manual) | SKILL-1.3, SKILL-1.4 |
| Existing `tests/guards.rs` and `tests/gc.rs` | integration (existing) | SKILL-1.5 |

## Coverage check

Every ID in requirements.md appears in exactly one `Satisfies:` line:

- Plugin manifests: 1.1, 1.2
- The skill: 2.1–2.6, 3.1–3.3, 6.1, 6.2
- Drift test: 4.1–4.4, 5.3
- Agent runs: 1.3, 1.4, 5.1, 5.2
- Existing CLI: 1.5

That is 23 IDs, none unmapped.
