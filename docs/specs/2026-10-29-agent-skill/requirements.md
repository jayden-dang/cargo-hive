# Requirements: Agent skill (the hive skill, shipped as a plugin)

Feature code: SKILL
Status: Approved
Date: 2026-10-29

<!--
Rules:
- Feature code: 2-12 chars, A-Z0-9, starts with a letter, unique repo-wide.
  Register it in docs/specs/INDEX.md before use.
- Every acceptance criterion gets a hierarchical ID: <CODE>-<story>.<criterion>.
- Criteria use EARS phrasing:
    WHEN <event/condition> THE SYSTEM SHALL <behavior>          (event-driven)
    WHILE <state> THE SYSTEM SHALL <behavior>                   (state-driven)
    IF <unwanted condition> THEN THE SYSTEM SHALL <behavior>    (unwanted behavior)
    WHERE <feature is included> THE SYSTEM SHALL <behavior>     (optional feature)
    THE SYSTEM SHALL <behavior>                                 (ubiquitous)
- Guard requirements protect existing behavior this feature touches:
    WHEN <condition> THE SYSTEM SHALL CONTINUE TO <existing behavior>
- IDs are immutable once Status is Approved. Retire a requirement by striking it
  through (~~**CODE-N.M**~~ reason) — never renumber.
-->

Roadmap item: ROAD-12 (MILE-1, `docs/roadmap/INDEX.md`). ROAD-12 starts after ROAD-3 (EXPLAIN), so the skill describes `explain` as EXPLAIN ships it; until `explain` is in the CLI, SKILL-4.2 is red for its command line.

Sources: the frame-change close package confirmed 2026-10-29 (`.skills/SKILL/close-package.md`), the scan digest (`.skills/SKILL/scan.md`), and the research note `.skills/research/2026-10-29-agent-skill-distribution.md` (Agent Skills spec at agentskills.io, Claude Code and Codex plugin docs). The install pattern follows the owner's `jayden-dang/skills` repo.

"The skill" below is `skills/hive/SKILL.md` in this repo. "The CLI" is the `cargo-hive` binary. A criterion about what the skill tells the agent is verified by reading the skill and by the agent runs recorded in story 5.

Files this feature touches: `.claude-plugin/marketplace.json`, `.claude-plugin/plugin.json`, `.codex-plugin/plugin.json`, `skills/hive/SKILL.md`, `skills/hive/TESTS.md`, `skills/hive/eval.json`, `tests/skill.rs` (all new: no behavior to guard); `README.md` (install section: documentation, no behavior to guard). No file under `src/` changes; the guards in stories 1 and 4 protect the CLI behavior the skill relies on.

## 1. Install the skill through the harness's plugin manager

**Story:** As a developer, I want to install the hive skill with my agent harness's plugin commands, so that my agents can use hive without my copying files by hand.

- **SKILL-1.1** THE SYSTEM SHALL ship a Claude Code marketplace at `.claude-plugin/marketplace.json` named `cargo-hive` that lists exactly one plugin, `cargo-hive`, whose source is the repository root; the entry carries neither a `skills` list nor a `version`, so the plugin manifest is the only place for both.
- **SKILL-1.2** THE SYSTEM SHALL ship plugin manifests `.claude-plugin/plugin.json` (with `"skills": ["./skills/hive"]`) and `.codex-plugin/plugin.json` (with `"skills": "./skills/"`), each naming the plugin `cargo-hive`, so that `hive` is the only skill either harness loads from the plugin.
- **SKILL-1.3** WHEN the developer runs `/plugin marketplace add jayden-dang/cargo-hive` and then `/plugin install cargo-hive@cargo-hive` in Claude Code THE SYSTEM SHALL make the skill invocable as `/cargo-hive:hive`, verified by a manual run recorded in `skills/hive/TESTS.md`.
- **SKILL-1.4** WHEN the developer runs `codex plugin marketplace add jayden-dang/cargo-hive` and then `codex plugin add cargo-hive@cargo-hive` THE SYSTEM SHALL make the hive skill available in Codex, verified by a manual run recorded in `skills/hive/TESTS.md`.
- **SKILL-1.5** (guard) WHEN any `cargo hive` command runs THE SYSTEM SHALL CONTINUE TO write only where its own spec allows it (CORE-6.3 for `repo add`, `status`, and `doctor`; AUTOGC-6.1 for `gc`; EXPLAIN-6.2 for `explain`); this feature adds no command and no write path, so no command writes a skills directory.

## 2. The skill teaches an agent to drive hive safely

**Story:** As an agent with the hive skill loaded, I want to know which hive command fits a disk or target-dir question and how to call it, so that I free space or explain it through hive instead of deleting build output myself.

- **SKILL-2.1** THE SYSTEM SHALL give the skill frontmatter with `name: hive`, a `description` that names when to use it (a large `target/` dir, a disk filling up, reclaiming build space, a crate with many copies) and says it is used instead of `cargo clean` or deleting a target dir, and `metadata.hive-version` set to the crate version; the frontmatter carries no key outside `name`, `description`, `license`, `compatibility`, and `metadata` (the Agent Skills spec set without `allowed-tools`, see SKILL-6.2).
- **SKILL-2.2** THE SYSTEM SHALL open the skill body by stating that the command is `cargo hive <sub>`, or `cargo-hive <sub>` with the same arguments where the harness cannot call a Cargo subcommand, and that no `hive` binary exists.
- **SKILL-2.3** THE SYSTEM SHALL state in the skill the model an agent works under: protected versus secondary checkouts, auto tier versus plan tier, that `needs-human` is the owner's to resolve, that an agent never runs the owner-approval command (it arrives with ROAD-7; the skill names it in prose only, never as a `cargo hive` command line), and that the repos a command acts on come from its arguments and never from the current directory.
- **SKILL-2.4** THE SYSTEM SHALL give in the skill command lines for exactly the MILE-1 commands `doctor`, `repo add`, `repo list`, `status`, `gc` (with and without `--dry-run`), and `explain` with the flags EXPLAIN ships, each with `--json`, and no command line for any other subcommand, hidden ones included.
- **SKILL-2.5** THE SYSTEM SHALL tell the agent in the skill to read the receipt hive prints on stdout and report its counters as measured, never estimating bytes or time itself.
- **SKILL-2.6** THE SYSTEM SHALL tell the agent in the skill not to run `cargo clean` or delete a target dir, an incremental dir, or rustc temp entries itself when hive covers the request.

## 3. The agent notices when the skill and the binary disagree

**Story:** As an agent whose skill and installed hive binary may come from different releases, I want a rule for a version mismatch, so that I keep answering read-only questions but never run a write command the skill may describe wrongly.

- **SKILL-3.1** THE SYSTEM SHALL tell the agent in the skill to take the binary's version from any receipt's `hive_version`, or from `cargo hive --version` (which prints `cargo-hive <version>`) when no receipt has been printed, and to compare its major and minor parts with the skill's `metadata.hive-version`.
- **SKILL-3.2** WHEN the major or minor parts differ THE SYSTEM SHALL tell the agent in the skill that `doctor`, `repo list`, `status`, and `explain` may still run, and that the agent tells the user both versions and how to update the plugin (`/plugin marketplace update cargo-hive`) or the binary.
- **SKILL-3.3** IF the major or minor parts differ THEN THE SYSTEM SHALL tell the agent in the skill not to run `gc` or `repo add`, and to ask the user to update first.

## 4. Tests fail when the skill drifts from the CLI

**Story:** As the developer maintaining hive, I want `cargo test` to fail when the skill or a manifest drifts from the crate, so that a release never ships a skill that names a missing flag or a different version.

- **SKILL-4.1** THE SYSTEM SHALL keep the `version` in `.claude-plugin/plugin.json` and in `.codex-plugin/plugin.json`, and the skill's `metadata.hive-version`, equal to the package version in `Cargo.toml`, verified by a test in `tests/skill.rs` that fails on any difference or on a `version` in the marketplace entry.
- **SKILL-4.2** IF a command line in the skill does not parse with the CLI's own argument parser, or names a subcommand outside the set in SKILL-2.4, THEN THE SYSTEM SHALL fail a test in `tests/skill.rs` that names the line. A command line is any line of a fenced code block, or any inline code span, that starts with `cargo hive ` or `cargo-hive `; its arguments are the whitespace-separated words up to the first `|`, `>`, `&&`, or `#`; the test drops the leading `cargo hive` or `cargo-hive` and parses `["cargo-hive", arguments…]`; `--help` and `--version` count as parsing.
- **SKILL-4.3** IF the skill's frontmatter `name` differs from its directory name or breaks the spec's name rule (at most 64 characters, lowercase letters, digits, and single hyphens), its `description` exceeds 1024 characters, a `compatibility` value exceeds 500 characters, or the frontmatter carries a key outside the set in SKILL-2.1 THEN THE SYSTEM SHALL fail a test in `tests/skill.rs` that names the rule broken.
- **SKILL-4.4** (guard) WHEN `tests/skill.rs` parses a skill command line THE SYSTEM SHALL CONTINUE TO accept and reject arguments exactly as the `cargo hive` binary does, because the test calls the public `cargo_hive::cli::Cli` parser rather than a copy of it.

## 5. Recorded agent runs show the skill changes what an agent does

**Story:** As the developer, I want recorded runs of agents with and without the skill on the same request, so that I can show the skill makes an agent use hive correctly.

- **SKILL-5.1** THE SYSTEM SHALL keep in `skills/hive/TESTS.md` a RED run (no skill loaded) and a GREEN run (skill loaded) of the request "the target dir of `<registered repo>` is too big; free some space", each made on a Sonnet-class and a Haiku-class model, with the agent's commands quoted verbatim.
- **SKILL-5.2** WHEN a GREEN run is recorded THE SYSTEM SHALL show in it that the agent ran `cargo hive` with `--json` and the repo path as an argument, reported counters from the receipt, and ran neither `cargo clean`, a deletion inside the target dir, nor any owner-approval command.
- **SKILL-5.3** THE SYSTEM SHALL keep `skills/hive/eval.json` as a non-empty array whose entries each carry `eval_id` (unique positive integer), `eval_name` (kebab-case), `kind` (`behavior`, `trigger`, or `contract`), `derived_from` naming a heading that exists in `skills/hive/TESTS.md`, `prompt`, and non-empty `assertions`, with at least one `behavior` entry and at least one `trigger` entry, verified by a test in `tests/skill.rs`.

## 6. Quality attributes

**Section-kind:** nfr

**Story:** As a stakeholder, I want measurable quality targets for this feature, so that how-well is not left implicit.

- **Performance:** **SKILL-6.1** THE SYSTEM SHALL keep the skill's body (after the frontmatter) at 150 lines or fewer, with no other file the body links to or tells the agent to read, so loading the skill costs an agent little context — verified by a test in `tests/skill.rs`. (Target from the frame-change lock; no `docs/product/metrics.md` exists.)
- **Security:** **SKILL-6.2** THE SYSTEM SHALL ship the skill without an `allowed-tools` key, so installing it grants the agent no tool permission it did not already have — verified by the frontmatter test in `tests/skill.rs`. (No `docs/security/threat-model.md` exists; no TB/THR ID is cited.)
- **Reliability:** None — the feature has no runtime component; drift between the skill and the binary is covered by stories 3 and 4.
- **Accessibility:** None — the skill is text an agent reads; there is no UI.

## Out of Scope

- An MCP server (ROAD-4, dropped 2026-10-29).
- Claude Code hooks, and any periodic or automatic run of hive by the agent.
- A `cargo hive skill` subcommand, embedding the skill in the binary, or any write by hive outside `~/.hive`.
- Installing the skill into a user's repository (`.claude/skills/` in a checkout).
- Teaching Cargo in general (build flags, profiles, features) beyond what a hive command needs.
- The plan, apply, approve, and seed commands: ROAD-6, ROAD-7, and ROAD-9 each extend the skill when they ship; ROAD-13 rereads the whole skill.
- Harnesses beyond Claude Code and Codex (Gemini CLI, Cursor, Copilot, Grok, Kimi) as a verified target; they may work through `.agents/skills` or their own plugin commands, but no criterion checks them.
- Running `eval.json` in CI; the agent runs in story 5 are made by hand.

## Open Questions

None.
