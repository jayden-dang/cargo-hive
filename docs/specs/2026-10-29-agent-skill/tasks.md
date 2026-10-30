# Tasks: Agent skill (the hive skill, shipped as a plugin)

> **For agentic workers:** after plan approval, pick one execute skill:
> - `build-in-waves`: a continuous, dependency-aware scheduler;
> - `build-by-story`: human-gated story review units;
> - `build-inline`: the controller implements, with no implementer subagents.
>
> The chosen skill writes `Execution-mode:`. Steps use checkbox (`- [ ]`) syntax for tracking.

Feature code: SKILL
Status: Approved
Date: 2026-10-29
Execution-mode: continuous
Max-concurrency: auto
Requirements: ./requirements.md
Design: ./design.md

**Goal:** Ship the hive agent skill as plugin `cargo-hive` from this repo's own marketplace. It teaches agents the MILE-1 commands with `--json`, and is kept in step with the binary by `tests/skill.rs` and by recorded RED/GREEN agent runs.

**Architecture:**
- **Three manifests** (`.claude-plugin/marketplace.json`, `.claude-plugin/plugin.json`, `.codex-plugin/plugin.json`) let Claude Code and Codex install the plugin.
- **`skills/hive/SKILL.md`** is the whole interface an agent reads.
- **`tests/skill.rs`** parses every command line in the skill through the real `cargo_hive::cli::Cli`, and checks versions, frontmatter, the body budget and `eval.json`.
- **`skills/hive/TESTS.md`** records the agent and install runs.
- No file under `src/` changes.

**Tech Stack:** Rust 2021, rustc 1.97.1 on macOS 27. The tests use existing dependencies only: `clap`, `serde_json`. No new dependency. The agent runs use the `skill-tester` (Sonnet) and `skill-tester-haiku` agents; installs use Claude Code `/plugin` and the `codex plugin` CLI.

## Gate: EXPLAIN on `main`

Task 3 parses the skill's `explain` line and matches `Command::Explain`. Task 4 writes the skill's `explain` line, and Task 5 runs agents against a binary that has `explain`. **Do not start Task 3 until EXPLAIN (ROAD-3, `.worktrees/explain`) is merged to `main`.** Tasks 1 and 2 do not depend on it. EXPLAIN also edits `README.md`; Task 6 edits only its install and uninstall paragraphs, so rebase Task 6 onto `main` after EXPLAIN merges.

## Global Constraints

The canonical sources (`docs/standards/` is the SSOT). Read them; do not restate them. Hashes were taken 2026-10-29, and a mismatch at execute time means the source changed after planning.

| Source | sha256 (first 16) |
|---|---|
| `docs/standards/INDEX.md` | `80cd41328c0f7652` |
| `docs/standards/code.md` (no new deps, no requirement IDs in code or comments) | `afdcc8b89136f573` |
| `docs/standards/errors-logging.md` (exit codes the skill teaches) | `48f5baac3b52a552` |
| `docs/standards/testing.md` (merge gate; scratch-only fixtures) | `de677bebcc504edd` |
| `docs/architecture/INDEX.md`: this feature relies on ARCH-2, 8, 10, 12 | `8cb275c512d9ab3c` |
| `docs/agents/project.md` (verify commands) | `6e20a5107273bb75` |
| `docs/specs/2026-10-29-agent-skill/design.md` (Decisions 1–8) | `28bc8edf4a4e57f9` |
| `docs/specs/2026-10-29-agent-skill/requirements.md` | `97b2016485ca9cd5` |

Merge gate (testing standard). Every task must leave all four commands green:
- `cargo fmt --check`, after `cargo fmt`.
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- `cargo check --target x86_64-unknown-linux-gnu` (only the existing `entry_kind` warning in `src/platform/mod.rs` is allowed)

Known pre-existing flake, outside this feature: `tests/status_targets.rs::an_interrupted_status_waiting_on_the_state_lock_exits_130_without_writing_the_snapshot` fails intermittently under full-suite load. If it fails, rerun and report it; do not fix it here.

Commits: each commit step is `cargo fmt`, then `git add -A && git commit -m "<subject>"`, on the feature branch the execute skill creates. No `Co-Authored-By` line.

House rules for this plan:
1. No file under `src/` changes (design: "Existing CLI (unchanged)"). `git diff main -- src/` must be empty at every commit.
2. No requirement ID appears in test names, test messages, or the skill's files. `TESTS.md` headings name scenarios, not IDs.
3. Agent and install runs use scratch dirs under `/private/tmp/hive-skill-*` and a scratch `HIVE_HOME`. The developer's real `~/.hive` must be unchanged by every run.
4. Repo files are located from `env!("CARGO_MANIFEST_DIR")` in `tests/skill.rs`.
5. The skill's body never contains `cargo hive ` or `cargo-hive ` (with a trailing space) except in a command line that parses (Task 3). To describe output such as the version line, use prose without a code span ("prints the binary name followed by its version").

## File Structure

| Path | Change | Responsibility |
|---|---|---|
| `plugin/skills/hive/SKILL.md` (was `skills/hive/SKILL.md` until Task 7) | create | the skill: frontmatter (`name`, `description`, `metadata.hive-version`) and a body of at most 150 lines |
| `.claude-plugin/marketplace.json` | create | marketplace `cargo-hive`, one plugin entry with `source: "./plugin"` (`"./"` until Task 7), no `version`, no `skills` |
| `plugin/.claude-plugin/plugin.json` (was `.claude-plugin/plugin.json` until Task 7) | create | plugin `cargo-hive`, `version`, `"skills": ["./skills/hive"]` |
| `plugin/.codex-plugin/plugin.json` (was `.codex-plugin/plugin.json` until Task 7) | create | plugin `cargo-hive`, `version`, `"skills": "./skills/"`, `interface` block |
| `tests/skill.rs` | create | the drift tests: versions, manifest shapes, frontmatter, command lines, MILE-1 coverage, body budget, `eval.json` fields |
| `skills/hive/TESTS.md` | create | fixture recipe; RED and GREEN agent runs; Claude Code and Codex install runs |
| `skills/hive/eval.json` | create | `behavior`, `trigger` and `contract` entries, each `derived_from` a `TESTS.md` heading |
| `README.md` | modify | install section gains the two plugin-install pairs; uninstall notes `/plugin uninstall cargo-hive` |
| `docs/agents/project.md` | modify (whole-branch review fix, owner-approved at land) | release step 2 also bumps the version in both `plugin.json` files and the skill's `metadata.hive-version` |

Retrieval (load-subgraph `fsubr-1.2`): `blast_radius` over these paths finds one owned path, `README.md`, which CORE, AUTOGC and EXPLAIN all own. No other mapped path has an owner. `cluster(SKILL)` has the focus alone: SKILL has no `tasks.md` OWNS before this file, so no member overlaps. `owns_coverage` is 3/4.

---

### Task 1: The skill's frontmatter, held to the spec

The skill file exists with its final frontmatter and a placeholder body of one line, and the frontmatter reader and test that later tasks extend are in place.

**Files:**
- Create: `skills/hive/SKILL.md`, `tests/skill.rs`
- Test: `tests/skill.rs`

**Reuse:** rung 4 — the Agent Skills format (frontmatter plus a markdown body), read natively by both harnesses; rung 7 — the frontmatter subset reader in `tests/skill.rs` (design Decision 6)

**Interfaces:**
- Consumes: none
- Produces (private to `tests/skill.rs`):
  - `fn repo_file(rel: &str) -> String`, which reads a file under `env!("CARGO_MANIFEST_DIR")`.
  - `struct Frontmatter { keys: Vec<(String, String)>, metadata: Vec<(String, String)> }`.
  - `fn split_skill(text: &str) -> Result<(Frontmatter, String), String>`, which returns the frontmatter and the body, or names the first unsupported line.
  - `fn get<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str>`.

  The skill's frontmatter is fixed:
  - `name: hive`
  - `description:` on one line. It names the triggers: a large `target/` dir, a disk filling up, reclaiming Rust build space, a crate with many copies. It says the skill is used instead of `cargo clean` or deleting `target/`.
  - `metadata:`, followed by `  hive-version: "0.1.0"`

**Depends-on:** none

- [x] **Step 1: Write the failing test** `skill_frontmatter_follows_the_spec`. It covers these rules:
  - `name` equals the directory name `hive`, and is at most 64 characters of `[a-z0-9]` with single hyphens;
  - `description` is at most 1024 characters and contains `target` and `cargo clean`;
  - any `compatibility` is at most 500 characters;
  - keys come only from `name`, `description`, `license`, `compatibility` and `metadata`, so `allowed-tools` fails;
  - a line outside the subset fails, naming the line.

  Add a unit-style table inside the test that feeds `split_skill` crafted frontmatters, one per rule broken, and asserts each error names its rule. Run `cargo test --test skill`; expect a failure because `skills/hive/SKILL.md` does not exist.
- [x] **Step 2: Implement.** Write `skills/hive/SKILL.md` with the frontmatter above and a placeholder one-line body. Implement `repo_file`, `split_skill` and `get`. The subset is: the file opens with `---`; top-level `key: value` lines; one `metadata:` line followed by two-space-indented `key: "value"` lines; a closing `---`. Run `cargo test --test skill`; expect pass.
- [x] **Step 3: Verify.** Run the merge gate. `git diff main -- src/` is empty.
- [x] **Step 4: Commit** `feat(skill): add the hive skill's frontmatter and a test that holds it to the Agent Skills spec`.

_Requirements: SKILL-2.1, SKILL-4.3, SKILL-6.2_

---

### Task 2: Plugin manifests whose versions follow the crate

Claude Code and Codex can load the plugin, and a version skew between any manifest, the skill and `Cargo.toml` fails `cargo test`.

**Files:**
- Create: `.claude-plugin/marketplace.json`, `.claude-plugin/plugin.json`, `.codex-plugin/plugin.json`
- Modify: `tests/skill.rs`
- Test: `tests/skill.rs`

**Reuse:** rung 4 — the Claude Code and Codex plugin systems, laid out as in the owner's `jayden-dang/skills` (`.claude-plugin/marketplace.json`, `.claude-plugin/plugin.json`, `.codex-plugin/plugin.json`); rung 4 — `env!("CARGO_PKG_VERSION")`; rung 5 — `serde_json` (already a dependency)

**Interfaces:**
- Consumes: `repo_file`, `split_skill`, `get` (Task 1)
- Produces:
  - `.claude-plugin/marketplace.json`: `{"name": "cargo-hive", "owner": {"name": "Jayden Dang"}, "description": …, "plugins": [{"name": "cargo-hive", "source": "./", "description": …}]}`. The entry has no `version` and no `skills`.
  - `.claude-plugin/plugin.json`: `{"name": "cargo-hive", "version": "0.1.0", "description": …, "author": {"name": "Jayden Dang"}, "license": "MIT OR Apache-2.0", "skills": ["./skills/hive"]}`.
  - `.codex-plugin/plugin.json`: `{"name": "cargo-hive", "version": "0.1.0", "description": …, "author": {"name": "Jayden Dang", "url": "https://github.com/jayden-dang"}, "homepage": "https://github.com/jayden-dang/cargo-hive", "repository": "https://github.com/jayden-dang/cargo-hive", "license": "MIT OR Apache-2.0", "skills": "./skills/", "interface": {"displayName": "cargo-hive", "shortDescription": …, "developerName": "Jayden Dang", "category": "Developer Tools"}}`.

**Depends-on:** Task 1

- [x] **Step 1: Write the failing test** `manifest_versions_match_the_crate`. It reads the three manifests with `serde_json` and checks:
  - Versions: both `plugin.json` `version` values and the skill's `metadata.hive-version` equal `env!("CARGO_PKG_VERSION")`, and the marketplace entry has no `version`.
  - Marketplace shape: the marketplace `name` is `cargo-hive`; it has exactly one plugin, named `cargo-hive`, with `source` `"./"` and no `skills`.
  - Plugin shapes: the Claude `skills` is exactly `["./skills/hive"]`, and the Codex `skills` is `"./skills/"`.
  - Messages: each failure message names the file and the field.

  Run `cargo test --test skill`; expect a failure because `.claude-plugin/marketplace.json` is missing.
- [x] **Step 2: Implement.** Write the three files exactly as in Interfaces. Run `cargo test --test skill`; expect pass. Then change `.codex-plugin/plugin.json` `version` to `0.0.9`, run, and expect a failure naming that file. Restore it.
- [x] **Step 3: Check that Claude Code loads it.** Run `claude --plugin-dir <repo root> plugin details cargo-hive`; expect `Skills (1)  hive`. Paste the output into the task report.
- [x] **Step 4: Verify.** Run the merge gate. `git diff main -- src/` is empty.
- [x] **Step 5: Commit** `feat(skill): publish the hive skill as plugin cargo-hive from this repo's marketplace`.

_Requirements: SKILL-1.6, SKILL-1.7, SKILL-4.5_ (repointed 2026-10-30: this task's SKILL-1.1, 1.2 and 4.1 were superseded when Task 7 moved the plugin into `plugin/`)

---

### Task 3: Every command line in the skill parses with the CLI

**Gate:** EXPLAIN (ROAD-3) merged to `main`; `Command::Explain { repos, top, out }` exists in `src/cli.rs` (EXPLAIN tasks.md, the `explain` command with `-n/--top`).

The skill gains its command-line block, and `cargo test` fails if:
- any command line does not parse;
- a command line names a subcommand outside MILE-1;
- a `cargo hive` mention escapes the extractor;
- a MILE-1 command is missing.

**Files:**
- Modify: `skills/hive/SKILL.md`, `tests/skill.rs`
- Test: `tests/skill.rs`

**Reuse:** rung 2 — `cargo_hive::cli::{Cli, Command, RepoCommand}` (`src/cli.rs:17,23,69`, all `pub`) and `clap::Parser::try_parse_from`, `clap::error::ErrorKind::{DisplayHelp, DisplayVersion}`; rung 7 — command-line extraction (design "Drift test"); `env!("CARGO_MANIFEST_DIR")` locates the skill

**Interfaces:**
- Consumes: `repo_file`, `split_skill` (Task 1)
- Produces (private to `tests/skill.rs`):
  - `struct CommandLine { line_no: usize, text: String, words: Vec<String> }`
  - `fn command_lines(body: &str) -> Vec<CommandLine>`
  - `fn stray_mentions(body: &str) -> Vec<(usize, String)>`
  - `enum Parsed { Info, Doctor, RepoAdd, RepoList, Status, Gc { dry_run: bool }, Explain }`
  - `fn parse(words: &[String]) -> Result<Parsed, String>`

  `parse` feeds `["cargo-hive", words…]` to `Cli::try_parse_from`. `DisplayHelp` and `DisplayVersion` map to `Info`. An `Ok` goes through a `match` on `Command` with no wildcard arm: `HoldLock` and `TryLock` give `Err` naming the line. `Explain` is the only variant EXPLAIN adds, so the match gains exactly one arm.
  - Definitions:
    - Scans cover the skill's **body** only. The frontmatter is checked by Task 1.
    - A **path word** is a word after the subcommand words that does not start with `-` and is not the value of a flag.
    - A line **has `--json`** when its words include `--json`.

**Depends-on:** Task 1, Task 2

- [x] **Step 1: Write the failing tests.**
  - `skill_command_lines_parse_with_the_cli`: every `command_lines` entry parses, and `stray_mentions` is empty. Each failure prints the line number and the text.
  - `skill_documents_every_shipped_command`: the parsed set covers `Doctor`, `RepoAdd`, `RepoList`, `Status`, `Gc { dry_run: true }`, `Gc { dry_run: false }` and `Explain`. Every such line contains `--json`, and the `RepoAdd`, `Status`, `Gc` and `Explain` lines carry at least one path word.
  - A table of extractor cases inside the first test:
    - a fenced line;
    - an inline span;
    - the bare span `` `cargo hive` ``, which is not a command line and not a stray mention;
    - `cargo hive gc /p --json 2>&1 | jq`, cut to `gc /p --json`;
    - `$ cargo hive status`, `HIVE_HOME=/x cargo hive status`, an indented line and a `\` continuation, each a stray mention;
    - `cargo hive __hold-lock a b`, which parses but is rejected;
    - `cargo hive --help`, which is `Info`.

  Run `cargo test --test skill`; expect `skill_documents_every_shipped_command` to fail because no command lines exist yet.
- [x] **Step 2: Implement** `command_lines`, `stray_mentions` and `parse` per design "Drift test" and Decision 7. A command line is a fenced-block line, or an inline code span, that starts with `cargo hive ` or `cargo-hive `. Words are split on whitespace and cut before the first word that starts with `|`, `&&` or `#`, or contains `>`. A stray mention is any other occurrence of `cargo hive ` or `cargo-hive `.
- [x] **Step 3: Add the command-line block** to `skills/hive/SKILL.md`: one fenced block with `doctor`, `repo add`, `repo list`, `status`, `gc --dry-run`, `gc` and `explain` (written `explain /path/to/repo --top 20 --json`), each with `--json`. Lines that take repos use the literal path `/path/to/repo`. Typed flags get literal values. Lines are written plainly: no prompt, no env prefix, no indentation. Run `cargo test --test skill`; expect pass. Then add `--no-such-flag` to one line, expect a failure naming it, and revert.
- [x] **Step 4: Verify.** Run the merge gate. `git diff main -- src/` is empty.
- [x] **Step 5: Commit** `feat(skill): list the MILE-1 command lines and parse each one through the CLI in tests`.

_Requirements: SKILL-2.4, SKILL-4.2, SKILL-4.4_

---

### Task 4: The skill's guidance: form, version rule, model, receipts, limits

The skill body is complete. An agent reading it knows:
- how to call hive;
- when a version mismatch stops writes;
- the protected/secondary and tier model;
- how to read a receipt;
- what never to delete itself.

**Files:**
- Modify: `skills/hive/SKILL.md`, `tests/skill.rs`
- Test: `tests/skill.rs`

**Reuse:** rung 4 — the Agent Skills format (frontmatter plus a markdown body), read natively by both harnesses

**Interfaces:**
- Consumes: `split_skill` (Task 1); `command_lines` and `stray_mentions` (Task 3)
- Produces: the body's sections in design order:
  1. `## Command form`
  2. `## Version first`
  3. `## The model`
  4. `## Which command`
  5. `## Command lines` (from Task 3)
  6. `## Reading a receipt`
  7. `## Do not`

**Depends-on:** Task 3

- [x] **Step 1: Write the failing test** `skill_body_fits_the_budget`. The body:
  - has 1 to 150 lines;
  - contains no markdown link to a local path;
  - has no `.md` path anywhere in it.

  The test also checks that the body's `## ` headings are exactly the seven named in Produces, in that order. This keeps the structure the design fixes, so a later edit cannot silently drop the version rule. Run `cargo test --test skill`; expect a failure on the headings.
- [x] **Step 2: Write the body**, section by section, per design "The skill":
  - **Command form:** the bare spans `cargo hive` and `cargo-hive`; no `hive` binary exists.
  - **Version first:**
    - take `hive_version` from any receipt, or from `cargo hive --version` when no receipt has been printed (it prints the binary name followed by its version, written as prose per house rule 5);
    - compare its major.minor with `metadata.hive-version`;
    - on a mismatch, `doctor`, `repo list`, `status` and `explain` still run, and the agent tells the user both versions and `/plugin marketplace update cargo-hive`;
    - `gc` and `repo add` do not run, and the agent asks the user to update first.
  - **The model:**
    - protected and secondary checkouts;
    - the auto tier (what `gc` does) versus the plan tier (not available yet);
    - `needs-human` is the owner's to resolve;
    - never run the owner-approval command, which is named in prose only;
    - repos come from arguments, never from the current directory.
  - **Which command:** a five-row table.
  - **Reading a receipt:**
    - one JSON object on stdout, prose on stderr;
    - report its counters as measured, never estimated;
    - exit codes 0, 1, 2, 130.
  - **Do not:** run `cargo clean`, or delete a target dir, an `incremental/` dir, or `rustc*`/`rmeta*` entries yourself, when hive covers the request.

  Run `cargo test --test skill`; expect every test to pass, including Task 3's stray-mention check over the new prose.
- [x] **Step 3: Review against the criteria.** Read the body once per criterion listed in this task's footer, and record in the task report the line that meets each one.
- [x] **Step 4: Verify.** Run the merge gate. `git diff main -- src/` is empty.
- [x] **Step 5: Commit** `feat(skill): teach agents the hive model, the version rule and how to read receipts`.

_Requirements: SKILL-2.2, SKILL-2.3, SKILL-2.5, SKILL-2.6, SKILL-3.1, SKILL-3.2, SKILL-3.3, SKILL-6.1_

---

### Task 5: Recorded agent runs and evals

`TESTS.md` shows agents without the skill and with it on the same request, and `eval.json` turns those runs into assertions that `cargo test` holds to their fields.

**Files:**
- Create: `skills/hive/TESTS.md`, `skills/hive/eval.json`
- Modify: `tests/skill.rs`
- Test: `tests/skill.rs`

**Reuse:** rung 2 — the owner's skill-testing practice: a `TESTS.md` RED/GREEN record plus `eval.json`, run by the `skill-tester` and `skill-tester-haiku` agents; rung 5 — `serde_json` for reading `eval.json` in the test

**Interfaces:**
- Consumes:
  - `repo_file` (Task 1);
  - the finished `skills/hive/SKILL.md` (Task 4);
  - `tests/common/mod.rs` `plant_session`, as the model for a superseded incremental session.
- Produces:
  - `TESTS.md` headings, in order:
    - `## Fixture`
    - `## RED — no skill`
    - `## GREEN — skill loaded`
    - `## Install — Claude Code` (filled by Task 6)
    - `## Install — Codex` (filled by Task 6)
  - `eval.json` entries:
    - `behavior`: free space; why so many copies; version mismatch before gc.
    - `trigger`: "my disk is full from Rust builds" should load the skill; "fix this borrow-checker error" should not.
    - `contract`: quotes of the never-approve and never-cwd lines.

**Depends-on:** Task 4

- [x] **Step 1: Write the failing test** `eval_entries_carry_their_evidence`. `eval.json` is a non-empty array. Each entry has:
  - a unique positive `eval_id`;
  - a kebab-case `eval_name`;
  - `kind` in `behavior`, `trigger`, `contract`;
  - `derived_from` naming a `## ` heading that exists in `TESTS.md`;
  - a non-empty `prompt`;
  - non-empty string `assertions`.

  There is at least one `behavior` entry and one `trigger` entry. Run `cargo test --test skill`; expect a failure because `eval.json` is missing.
- [x] **Step 2: Build the fixture** under `/private/tmp/hive-skill-<date>/` and write the recipe in `## Fixture`:
  1. Run `cargo install --locked --path . --root <tmp>/bin`.
  2. Create `<tmp>/env.sh`, which exports `PATH=<tmp>/bin/bin:$PATH` and `HIVE_HOME=<tmp>/hive`.
  3. Create a scratch git repo with one crate. Build it, edit it, and build it again, so `target/debug/incremental/` holds a superseded session.
  4. Run `cargo hive doctor --json` and `cargo hive repo add <scratch repo> --json` under `env.sh`.
  5. Record `ls -lR ~/.hive | shasum` before the runs.
- [x] **Step 3: RED runs.** Dispatch `skill-tester` and `skill-tester-haiku`, each told to `cd <tmp>` and `source <tmp>/env.sh` first, to read nothing outside `<tmp>`, and given the request "the target dir of `<scratch repo>` is too big; free some space". Give them no skill. The cwd and read limits matter: these agents have Read, Grep and Glob, so they could otherwise find `skills/hive/SKILL.md` in this repo. Quote each agent's commands verbatim under `## RED — no skill`.
- [x] **Step 4: GREEN runs.** Repeat the same request with the SKILL.md body in the prompt as a loaded skill, and quote the commands verbatim under `## GREEN — skill loaded`. Each GREEN run must show:
  - `cargo hive` called with `--json` and the repo path;
  - the receipt's counters reported;
  - no `cargo clean`, no deletion inside the target, and no approve command.

  If a GREEN run fails this, fix the skill text (Task 4's file) and rerun. Under the same heading, add one GREEN run per model for each of these scenarios:
  - **Why so many copies.** "why does `<scratch repo>`'s target have so many copies of one crate?" The agent must run `explain` with the path and `--json`.
  - **Version mismatch.** The same free-space request, with the skill's `hive-version` changed to `0.0.9` in the prompt copy. The agent may run read commands, must not run `gc`, and must name both versions and the update command.
  - **Trigger.** The agent is given the skill's `name` and `description` among two decoy skill descriptions, and asked which skill it would load for "my disk is full from Rust builds" and for "fix this borrow-checker error". The answers must be hive, then not hive.

  Then confirm that `ls -lR ~/.hive | shasum` is unchanged.
- [x] **Step 5: Write `eval.json`** from those runs, with `derived_from` naming the headings above. Run `cargo test --test skill`; expect pass.
- [x] **Step 6: Verify.** Run the merge gate. `git diff main -- src/` is empty.
- [x] **Step 7: Commit** `test(skill): record agent runs with and without the hive skill, and evals derived from them`.

_Requirements: SKILL-5.1, SKILL-5.2, SKILL-5.3_

---

### Task 6: Install through each harness, and the README

The plugin is installed through Claude Code's and Codex's own plugin commands, and the runs are recorded. The README tells users how to install and remove it. The CLI is unchanged.

**Files:**
- Modify: `skills/hive/TESTS.md`, `README.md` (install section and the uninstall line near line 68)
- Test: none new; the existing `tests/guards.rs` and `tests/gc.rs`

**Reuse:** rung 2 — the existing `README.md` install section; rung 2 — the existing guard tests: `tests/guards.rs` (CORE-6.3, extended by EXPLAIN-6.2) and `tests/gc.rs` (AUTOGC-6.1)

**Interfaces:**
- Consumes: the manifests (Task 2); `TESTS.md` headings (Task 5)
- Produces: README install steps.
  - Claude Code: `/plugin marketplace add jayden-dang/cargo-hive`, then `/plugin install cargo-hive@cargo-hive`.
  - Codex: `codex plugin marketplace add jayden-dang/cargo-hive`, then `codex plugin add cargo-hive@cargo-hive`.
  - Uninstall: `/plugin uninstall cargo-hive`, next to `rm -rf ~/.hive`.

**Depends-on:** Task 2, Task 5

- [x] **Step 1: Claude Code install from the branch.**
  - **Clone.** `git clone` the feature branch into `<tmp>/clone`. A clean clone holds no `target/`, and a local-path install copies the whole directory into the plugin cache.
  - **Install.** With `CLAUDE_CONFIG_DIR=<tmp>/claude`, so the real `~/.claude` is never written, run `claude plugin marketplace add <tmp>/clone` and then `claude plugin install cargo-hive@cargo-hive`. These are the CLI forms of `/plugin marketplace add` and `/plugin install`.
  - **Record.** Paste `claude plugin details cargo-hive` under `## Install — Claude Code`. It must show `Skills (1)  hive`, which is the skill behind `/cargo-hive:hive`.
- [x] **Step 2: Codex install from the branch.** Set `CODEX_HOME=<tmp>/codex`, run `codex plugin marketplace add <tmp>/clone` and `codex plugin add cargo-hive@cargo-hive`, and confirm the `hive` skill is listed. Record the output under `## Install — Codex`.
- [x] **Step 3: README.** Add the two install pairs after the `cargo install` step, and add the plugin uninstall next to `rm -rf ~/.hive`.
- [x] **Step 4: Verify the CLI is unchanged.** `git diff main -- src/` is empty. `cargo test --test guards --test gc` passes. Run the merge gate.
- [x] **Step 5: Commit** `docs(skill): record plugin installs in Claude Code and Codex, and document them in the README`.
- [ ] **Step 6 (after the branch lands on `main`; the owner runs it in a real session):** repeat Steps 1 and 2 with the GitHub form `jayden-dang/cargo-hive`, including the interactive `/plugin marketplace add` and `/plugin install` in Claude Code, and confirm `/cargo-hive:hive` is offered. Append the result to each install heading in a follow-up commit `docs(skill): record installs from the published marketplace`.

_Requirements: SKILL-1.3, SKILL-1.4, SKILL-1.5_

---

### Task 7: The plugin moves to its own root

Amendment 2026-10-30 (tier 1, owner-approved). Harness plugin caches receive only `plugin/`, not the repository, and a contract eval with an unclosed skill quote fails the test instead of being skipped.

**Files:**
- Move: `.claude-plugin/plugin.json` → `plugin/.claude-plugin/plugin.json`, `.codex-plugin/plugin.json` → `plugin/.codex-plugin/plugin.json`, `skills/hive/SKILL.md` → `plugin/skills/hive/SKILL.md`
- Modify: `.claude-plugin/marketplace.json`, `tests/skill.rs`, `docs/agents/project.md`
- Test: `tests/skill.rs`

**Reuse:** rung 4 — the Claude Code and Codex plugin systems (marketplace `source` relative path)

**Interfaces:**
- Consumes: `repo_file`, `manifest`, `skill_quotes` (Tasks 1-5)
- Produces: `const SKILL`, `const CLAUDE_PLUGIN`, `const CODEX_PLUGIN`; `fn entries_of(rel: &str) -> Vec<String>`; `skill_quotes(&str) -> Result<Vec<&str>, String>`

**Depends-on:** Task 6

- [x] **Step 1: Write the failing tests.** The skill and manifests are read from `plugin/`; the marketplace `source` is `./plugin`; `plugin/`, `plugin/.claude-plugin`, `plugin/.codex-plugin`, `plugin/skills`, `plugin/skills/hive` and `.claude-plugin` hold exactly their expected entries; an unclosed `The skill says: "` quote is an error naming the eval entry. Run `cargo test --test skill`; expect every test to fail because the files are not under `plugin/` yet.
- [x] **Step 2: Move the files** with `git mv`, set `source` to `./plugin`, update the release step in `docs/agents/project.md`. Run `cargo test --test skill`; expect pass. Add a stray file under `plugin/skills/hive/`, expect a failure naming that dir, remove it.
- [x] **Step 3: Install check.** From a clean clone with `CLAUDE_CONFIG_DIR` / `CODEX_HOME` in a scratch dir: `claude plugin marketplace add`, `claude plugin install cargo-hive@cargo-hive`, `claude plugin details cargo-hive` (`Skills (1)  hive`); the plugin cache holds only `.claude-plugin`, `.codex-plugin`, `skills`; the same for Codex. Record under both Install headings in `skills/hive/TESTS.md`.
- [x] **Step 4: Verify and commit** `refactor(skill): ship the plugin from its own plugin/ root` after the merge gate.

_Requirements: SKILL-1.6, SKILL-1.7, SKILL-1.8, SKILL-4.5, SKILL-5.4_
