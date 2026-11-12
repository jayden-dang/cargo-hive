# Design: Skill pass (one hive skill guide that covers every command)

Feature code: SKILLPASS
Status: Draft
Date: 2026-11-12
Requirements: ./requirements.md

Sources: requirements.md (approved by hive-skill-pass-interpret at 695bdc8, round 3); the design scan `.skills/_pending-hive-skill-pass/auto/design-scan.md`; the frame scan `scan.md`; a fresh load-subgraph retrieval (fsubr-1.2) of SKILLPASS with terms skill, SKILL.md, README, receipt, flag, eval, TESTS.md, source.none, walk.unreadable, store.tag, lanes and rename. Its owns_coverage is 9 with OWNS out of 10 registered (SKILLPASS has no tasks.md yet). It found no path neighbours, because SKILLPASS owns no Files yet, and nine term-only neighbours led by SEED, CORE, BENCH, STOREGC and SKILL. Its observations band is empty. Labels in square brackets follow the requirements: Lock, Weigh or agent-decided.

## Context

The hive skill (`plugin/skills/hive/SKILL.md`) is 78 body lines under seven fixed headings: Command form, Version first, The model, Which command, Command lines, Reading a receipt, Do not. Each roadmap item after ROAD-12 appended a paragraph inside those headings, and several later specs pinned the headings in place (SEED-8.1, BENCH-8.1, STOREGC-10.11, BENCH-9.4). The guard file `tests/skill.rs` (853 lines) holds the shape. It parses every `cargo hive` command line with the real clap `Cli`, matches versions, holds the 150-line budget, pins seven headings and about 30 phrases, and checks `eval.json` against `TESTS.md`. It reads nothing in `src/` apart from `Cli`, so a renamed receipt name passes it (design-scan §1, scan.md §3).

The binding constraint is that `src/` stays unchanged (Lock L9; requirements Out of Scope). Together with the absent dev-dependencies (Cargo.toml has no `[dev-dependencies]`, design-scan §2), this rules out the obvious alternative: a `pub mod names` of constants, or a `syn` parse of `src/`, as the bridge between the skill's words and the binary's. The bridge must instead be built from what `src/` already exposes publicly (`Cli`, `BuildCmd`, `Outcome`, `class_label`, `ReceiptBuilder`) plus a small hand lexer over `src/**/*.rs` string literals. Adopting `syn` as a dev-dependency would be a new dependency, which is the operator's call. Nothing here needs it.

A second constraint comes from `docs/standards/testing.md` (Strategy table): document tests in `tests/skill.rs` "touch nothing on the machine, and need neither `HIVE_HOME` nor `tests/common`". That rules out SKILLPASS-8.4's original mechanism (open a temp state root with `State::open_at` from `tests/skill.rs`). The config-key check therefore moves to an integration test that runs the binary with a scratch `HIVE_HOME`, the class the standard gives that job. Requirement 8.4 and 12.2 are corrected to match; see Decisions 6 and Upstream sync-back.

Architecture invariants this feature relies on: ARCH-8 (scope from arguments: the guide keeps that rule and every command line carries path arguments); ARCH-10 (counters measured, durations only from bench: the guide's receipt section keeps that rule); ARCH-2 (no new write path: the binary is unchanged and the new integration test writes only its scratch `HIVE_HOME`).

## Decisions

1. **One spec, no ADR.** Every choice below can be undone in `tests/skill.rs` or the skill text alone, so none meets the ADR gate (hard to reverse AND surprising AND a real trade-off). [agent-decided]
2. **Body outline, by user goal** (SKILLPASS-5.1; heading names and order are agent-decided under the card's Delegated, and the evals may move them): `## Command form`, `## Version first`, `## Free disk`, `## Start new lanes`, `## Delete what gc keeps`, `## Look before acting` (doctor, repo, status, explain), `## Time cold against seeded builds`, `## When a command refuses`, `## Reading a receipt`. Each of today's 51 instructions gets one home under one of these headings. A ban (no `cargo clean`, no hand deletion of the store) lives under the goal it guards, not in a trailing `Do not` list. The duplication sweep (SKILLPASS-5.3) then has one candidate home per rule. Planned size: about 120 of 150 lines. [agent-decided]
3. **Name check is a classifier plus typed rules plus a template match** (SKILLPASS-7.1 to 7.3; W1). Each single-word inline span outside fences is classified once, in a fixed order, and the first matching class decides:
   1. skip: a path, glob or placeholder (contains `/`, `~`, `$`, `*`, `<`, or ends in a file extension), a version or number (digits and dots), a frontmatter key path (`metadata.` prefix, checked against `split_skill`'s metadata keys);
   2. long flag (`--x`): the long flags of `Cli::command().build()` over all subcommands, `--help` and `--version` included;
   3. short flag (`-x`): skill and README both fail on any hive short flag (SKILLPASS-8.3), and other tools' flags only reach here inside a multi-word span, which is not checked;
   4. subcommand: the non-hidden subcommand names of the built command, plus `repo`'s;
   5. bench shape: `BuildCmd::from_str`, with a `=<…>` placeholder replaced by `x` first;
   6. plan outcome: `serde_json::from_value::<Outcome>`;
   7. plan class: the three `class_label` values, reached by an exhaustive match over `PlanClass`;
   8. receipt envelope key: the top-level keys of `serde_json::to_value(ReceiptBuilder::new("x", Scope::default()).finish())`, which reaches `hive_version`, `counts`, `findings` and `unmeasured` (src/receipt.rs:89-126, 116, 206);
   9. config key: a span equal to a key in the README's `## Configuration` table (checked by Decision 6);
   10. name with `_` or `.`: the template match in Decision 4;
   11. skip: a command of another tool listed by name (`cargo`, `rustc`, `git`, `rustup`), and the product words `hive` and `cargo-hive`;
   12. anything else fails, naming the span and its line.

   The skill must therefore write a plain English word in prose, not in backticks. [W1; classes 1, 8 and 11 agent-decided, design-scan §5]
4. **Template match against src literals.** A hand lexer (no new dependency) collects every string literal in `src/**/*.rs`. It reads `"…"` with escapes and `r#"…"#` raw strings, skips `//` and `/* */` comments, and reads `'x'` and `'\…'` char literals so that a `'"'` does not open a string. Lifetimes are left alone. For each file it stops at the first `#[cfg(test)]` line that opens the file's `mod tests`, so that test fixtures in `src/` do not vouch for a name. A literal with `{…}` holes is a template. A span matches a literal when it equals it. A span matches a template under either of two rules:
   - The template minus a trailing `{…}` or `:{…}` equals the span. This covers `"sessions_deleted{suffix}"` (src/cli.rs:893-906) and `"reused_fresh:{path}"` (src/cli.rs:862-865).
   - The template starts with a `{…}` hole, and the span is some placeholder-free literal followed by the rest of the template. This covers `format!("{series}_median")` with `"cold_build_ms"` (src/bench/mod.rs:572-590).

   [W1; the cfg(test) cut and the leading-hole rule agent-decided, design-scan §3]
5. **Spans are found by one new helper.** `inline_spans(text) -> Vec<(usize, String)>` walks lines with the same fence toggle as `scan` (tests/skill.rs:371-401) and returns each backtick span outside fences with its 1-based line number. It is used for the skill body and for README.md. An odd backtick count on a line fails the test with that line rather than silently flipping the parity (design-scan §8). [agent-decided]
6. **Config keys are checked by the binary in an integration test** (SKILLPASS-8.4, corrected). A new `tests/config_keys.rs` uses `tests/common`'s `Hive::new()` and `Hive::config(json)` (tests/common/mod.rs:52, 61). It reads the first-column code spans of README's `## Configuration` table and writes each key alone into the scratch `config.json` with a value from a table in the test (`secondary_roots: ["/private/tmp"]`, `cache_roots: ["/private/tmp"]`, `gc_window_days: 7`). It then runs `cargo-hive status --json`, which reads the config (src/cli.rs:633), and requires exit 0. A key the binary renamed is refused by `deny_unknown_fields` (src/state.rs:243-249). A key the README names with no value in the test's table fails with "no test value for <key>", so the two cannot drift silently. The test also runs one unknown key and expects a refusal, which proves the refusal path is live. [Weigh, the standard's document-test boundary; agent-decided mechanism]
7. **README pass reuses the skill helpers** with three differences (SKILLPASS-8.2, 8.3; W3):
   - no `--json` rule;
   - `.md` links allowed;
   - a whole command line must start a fenced line or a code span. The Uninstall line `cargo uninstall cargo-hive && rm -rf ~/.hive` is then not a stray, because `cargo-hive` there is an argument to `cargo uninstall`. The README pass counts a mention only where the prefix opens the line or span (design-scan §5).

   It also requires one README command line per shipped subcommand form, which is how BENCH-8.3 gets a test. [W3; the Uninstall rule agent-decided]
8. **Pins move from phrases to meaning.** The heading list pin (tests/skill.rs:756-770) becomes "the first heading is `## Command form`, the second is `## Version first`" (Lock L8). The ~30 phrase pins in `the_skill_teaches_seeding_a_new_lane`, `the_skill_teaches_benching_cold_against_seeded` and `the_skill_teaches_the_plan_flow_for_every_class` are kept where the rewritten sentence still carries the phrase. Where it does not, the pin is replaced by the full receipt name or the recovery the requirement names: `already consumed` becomes `already_consumed`, and pins are added for `source.none`, `store.tag`, `toolchain.missing`, `walk.unreadable` and `repo remove --missing`. No pin is dropped without its replacement. [Weigh from SKILLPASS-5.1, 1.3, 3.x; agent-decided form]
9. **Evals run from a fixture script under the run directory, not from the repo.** The fixture is built by a shell script kept with the run records. Every agent-visible path is project-shaped (for example `/private/tmp/<random>/orbit/`), with no test vocabulary, per author-skills' sanitising rule. The RED arm pastes `git show 23defc9:plugin/skills/hive/SKILL.md`, and the GREEN arm pastes the worktree file, never the installed plugin copy. `toolchain.missing` is first made by deleting the `toolchains` key from the scratch `HIVE_HOME/store.db` (design-scan §6, src/seed/store.rs:72-77), and the attempt is recorded (SKILLPASS-10.8). [W2; agent-decided mechanism]

## Architecture

Vocabulary for every section below: **module**, **interface**, **implementation**,
**seam** (public surface where behavior is observable and substitutable). Prefer
deep modules — an interface much simpler than what it hides.

### The skill body

Satisfies: SKILLPASS-1.1, SKILLPASS-1.2, SKILLPASS-2.1, SKILLPASS-2.2, SKILLPASS-2.3, SKILLPASS-3.1, SKILLPASS-3.2, SKILLPASS-3.3, SKILLPASS-4.1, SKILLPASS-4.2, SKILLPASS-4.3, SKILLPASS-5.1, SKILLPASS-5.2, SKILLPASS-5.3, SKILLPASS-5.4, SKILLPASS-6.1, SKILLPASS-11.1
Reuse: rung 2 — the existing `plugin/skills/hive/SKILL.md`, its frontmatter and its 51 instructions (scan.md §5), rewritten in place under Decision 2's headings
Respects: ARCH-8, ARCH-10
Surface:
- `tests/skill.rs` heading pin (:756-770) and phrase pins (:543-600, :688-704) — replace (Decision 8, same change).
- `skills/hive/eval.json` contract entries 6 and 7, which quote S:25 and S:26 verbatim — replace (SKILLPASS-10.7, same change).
- Installed plugin copies under `~/.claude/plugins/cache/cargo-hive/cargo-hive/0.1.0/` and Codex's cache — frozen: the change keeps `metadata.hive-version` at `0.1.0`, so it does not reach them. Shipping it to them is gated on the operator's version-bump decision (requirements Open Questions).
- Agents reading the skill from the marketplace (`main`) — replace on merge; no compat path, since the Compat obligation is None.
- `docs/specs/2026-10-29-agent-skill/design.md:75-81`, which describes the old body layout — frozen as a record of what ROAD-12 shipped; the new layout's home is this design.

Interface: one markdown file whose frontmatter keeps `name`, `description` and `metadata.hive-version`, and whose body is about 120 lines under nine goal headings, command form and version first.
Depth: n/a — extends the existing skill.
Locality: `plugin/skills/hive/SKILL.md` only; neighbours `tests/skill.rs` (extend, pins) and `skills/hive/eval.json` (extend, quotes).

Section plan, each instruction in one place:
- **Command form**: instructions 1-3.
- **Version first**: 4-10, with `repo remove` added to the held list (SKILLPASS-6.1).
- **Free disk**: 12-14, 16, 23-26 and 47. The scope paragraph says lanes named means their whole repo, and that the store is left alone. Freed bytes are reported from `freed_bytes`, with `deleted_bytes_still_shared` apart and the store's `store_freed_bytes` from a bare `gc` (SKILLPASS-1.1, 1.2).
- **Start new lanes**: 27, 30, 31, 33 and 34. The order is seed one lane, build it, seed the rest, then bare `gc` after the batch (SKILLPASS-2.1 to 2.3).
- **Delete what gc keeps**: 15 and 17-20, written `already_consumed`, plus 48 (SKILLPASS-11.1).
- **Look before acting**: 11 and 22, recast as routing, plus 29 for `explain --top` and the `status` counts afterwards.
- **Time cold against seeded builds**: 35-37 and 50.
- **When a command refuses**:
  - `source.none` (32, changed), `store.tag` and `toolchain.missing` (new): build the main checkout, then seed again; an unreadable tag is reported to the user;
  - `walk.unreadable` (28, widened) and `store.busy` (44): rerun the command after the fix;
  - `store.held_over_cap` (45): retire the targets it names;
  - exit codes (40).

  (SKILLPASS-3.1 to 3.3.)
- **Reading a receipt**: 39 and 41-43, with every name written in full (SKILLPASS-4.1 to 4.3), and 46.
- The command-line block is split so that each goal section carries its own `--json` lines. Together they cover every shipped subcommand (SKILLPASS-5.2). Instruction 38's block is thereby moved, not dropped.
- The description: instruction 51, plus seeding and benching (SKILLPASS-5.4).

### Name check (tests/skill.rs)

Satisfies: SKILLPASS-7.1, SKILLPASS-7.2, SKILLPASS-7.3
Reuse: rung 2 — `split_skill`, `scan`'s fence toggle and `repo_file` in `tests/skill.rs`; rung 5 — `clap::CommandFactory` (clap with `derive` is already a dependency, Cargo.toml:19) and `serde_json`; rung 2 — public lib items `cargo_hive::cli::Cli`, `cargo_hive::bench::command::BuildCmd`, `cargo_hive::receipt::{Outcome, ReceiptBuilder, Scope}`, `cargo_hive::plan_tier::build::{class_label, PlanClass}`; rung 7 — the literal lexer and the classifier (no dependency in the project lexes Rust, and adopting `syn` is the operator's call).
Interface: `fn cited_name_failures(text: &str, origin: &str) -> Vec<String>`. Each entry reads `"<origin>:<line>: `<span>` is <reason>"`, and an empty list means pass. Two tests call it: one on the skill body, one on README.md (as `README.md`).
Depth: if this module vanished, callers would still need to know only that a cited span must name something the binary defines, and which public item defines each kind (Decision 3's table). The lexer, the template match and the skip rules stay hidden.
Locality: new private helpers in `tests/skill.rs` (extend); `src/` leave.

Flow: `inline_spans` (Decision 5) → classify (Decision 3) → check against the class's source. `src_literals()` lexes `src/**/*.rs` once per test run with `std::fs::read_dir` recursion (rung 3). Each failure names the span, so an agent fixing a red sees the renamed token (SKILLPASS-7.1, 7.2). An unclassifiable span fails with "is not a hive name the binary defines; write it as prose or add a skip rule" (SKILLPASS-7.3).

### README pass (tests/skill.rs)

Satisfies: SKILLPASS-8.2, SKILLPASS-8.3
Reuse: rung 2 — `scan`/`command_words`/`parse` (tests/skill.rs:357-456) and the name check above
Interface: `#[test] fn readme_describes_the_commands_as_the_skill_does()`: README command lines parse, each shipped subcommand form has one, no short hive flag, no stray, and cited names pass `cited_name_failures`.
Depth: n/a — extends the existing command-line check.
Locality: `tests/skill.rs` (extend). The two README strings at :569-576 stay (SKILLPASS-9.6).

### README text

Satisfies: SKILLPASS-8.1, SKILLPASS-8.5
Reuse: rung 2 — README.md's existing Quick start (:119-151) and Commands table (:153-169), rewritten in place
Surface:
- `tests/skill.rs:569-576`, which asserts a `cargo hive seed` line and the bare-gc sentence — kept unchanged (no edit needed; SKILLPASS-9.6).
- GitHub readers of `main`'s README and the crates.io page of v0.1.0 — frozen: the published v0.1.0 package carries its own README, and this change reaches crates.io only at the next release.

Interface: Quick start in the skill's goal order, and a Commands table with one row per command form whose code span is a real command line (`cargo hive explain /path/to/repo --top 0`), with optional parts in prose. The two recoveries appear as in the skill.
Depth: n/a — extends README.md.
Locality: README.md Quick start and Commands only. Other sections are leave, apart from a line that contradicts the skill (requirements Out of Scope).

### Config-key check (tests/config_keys.rs)

Satisfies: SKILLPASS-8.4
Reuse: rung 2 — `tests/common`'s `Hive::new`, `Hive::config` and `Hive::json` (tests/common/mod.rs:52, 61, 167); rung 4 — the binary's own `deny_unknown_fields` config read (src/state.rs:243-249, 286), reached through `status --json`
Respects: ARCH-2
Interface: `#[test] fn every_config_key_the_readme_names_is_accepted()`, plus `#[test] fn an_unknown_config_key_is_refused()`.
Depth: if it vanished, callers would need to know only that each README config key, set alone, must be accepted by the binary.
Locality: new file `tests/config_keys.rs` (rung 7 by file, rung 2 by content); `tests/common` leave.

### Guards kept in tests/skill.rs

Satisfies: SKILLPASS-9.1, SKILLPASS-9.2, SKILLPASS-9.3, SKILLPASS-9.4, SKILLPASS-9.5, SKILLPASS-9.6, SKILLPASS-12.1, SKILLPASS-12.2
Reuse: rung 2 — the existing tests `skill_body_fits_the_budget`, `manifest_versions_match_the_crate`, `skill_command_lines_parse_with_the_cli`, `skill_documents_every_shipped_command`, `skill_frontmatter_follows_the_spec` and `eval_entries_carry_their_evidence`, kept. Only their heading and phrase pins change (Decision 8).
Surface: the pinned phrases are the readers of the old text — replace, as in the skill-body section.
Interface: unchanged test names and checks.
Depth: n/a — extends the existing tests.
Locality: `tests/skill.rs` (extend). The duplicate 150-line assertion in `the_skill_teaches_benching_cold_against_seeded` (:602) is left; the budget's one home is `skill_body_fits_the_budget`, and the second assertion is a red-flag echo, not a second rule.

### Eval records and fixture

Satisfies: SKILLPASS-9.7, SKILLPASS-10.1, SKILLPASS-10.2, SKILLPASS-10.3, SKILLPASS-10.4, SKILLPASS-10.5, SKILLPASS-10.6, SKILLPASS-10.7, SKILLPASS-10.8
Reuse: rung 2 — `skills/hive/TESTS.md`'s fixture recipe (:5-17) and run format, `skills/hive/eval.json`'s entry shape; rung 4 — the `skill-tester` (Sonnet) and `skill-tester-haiku` agents; rung 7 — the fixture script (Decision 9)
Surface:
- `eval.json` entries 1-11 — kept. 8-11 are re-pointed from text-derived sections to their new run headings; 6-7 get their quotes updated.
- TESTS.md headings that `derived_from` names (TESTS.md:5-268) — kept, with new `## ` headings added for each new scenario.

Interface:
- TESTS.md gains `## Fixture (2026-11-12)`.
- It gains `## Predictions`, written before any run.
- For each scenario it gains `## <scenario>` with `### RED (skill at 23defc9), <model>` and `### GREEN (rewrite), <model>` runs that quote the agent's commands verbatim.
- It also gains `## Trigger (2026-11-12)` and `## Instruction map`: today's 51 instructions against the body lines that hold them (SKILLPASS-5.3).

Depth: if the fixture script vanished, a reader would need to know only the fixture's states (SKILLPASS-10.5's list), which TESTS.md records.
Locality: `skills/hive/` (extend). The fixture script lives in the run directory under `.skills/`, not in the repo.

Run order (author-skills: RED before writing): predictions, then every RED, then the rewrite, then every GREEN, then the trigger test. The no-skill free-space RED runs alongside the REDs (SKILLPASS-10.2).

### Supersede edits (infrastructure)

Satisfies: none — infrastructure. It carries out requirements.md's Supersedes table once the requirements are Approved.
Reuse: rung 2 — the strike-through convention of `docs/specs/2026-10-29-agent-skill/requirements.md:37`
Interface: each listed clause is wrapped as `~~clause~~ (superseded by SKILLPASS-N.M)` in its own file.
Depth: n/a.
Locality: four requirement files under `docs/specs/` (extend); no code.

## Seams for testing

| Seam | Kind | Covers |
|---|---|---|
| `tests/skill.rs` reading the skill and README from `CARGO_MANIFEST_DIR`, with the public lib items named above | integration (document class) | SKILLPASS-4.2, 5.1, 5.2, 6.1 (held-list pin), 7.1, 7.2, 7.3, 8.2, 8.3, 9.1, 9.2, 9.3, 9.4, 9.5, 9.6, 12.1, 12.2 |
| `tests/config_keys.rs` running the `cargo-hive` binary with a scratch `HIVE_HOME` via `tests/common` | integration | SKILLPASS-8.4 |
| Agent runs recorded in `skills/hive/TESTS.md` (skill-tester Sonnet and Haiku), with `eval.json` entries | e2e (manual) | SKILLPASS-1.1, 1.2, 2.1, 2.2, 2.3, 3.1, 3.2, 3.3, 4.1, 4.3, 5.3, 5.4, 6.1, 9.7, 10.1, 10.2, 10.3, 10.4, 10.5, 10.6, 10.7, 10.8, 11.1 |
| Reading the README against the skill, recorded in TESTS.md's instruction map | manual | SKILLPASS-8.1, 8.5 |

A temporary rename per class (Decision 3's classes 2-10), made in a scratch copy of the worktree and reverted, shows the document-class seam failing on each. This is the close package's done signal, recorded in TESTS.md, not a committed test.

## Coverage check

Walked top to bottom; every requirement ID appears in exactly one `Satisfies:` line:

- Skill body: 1.1-1.2, 2.1-2.3, 3.1-3.3, 4.1-4.3, 5.1-5.4, 6.1, 11.1.
- Name check: 7.1-7.3.
- README pass: 8.2-8.3.
- README text: 8.1, 8.5.
- Config keys: 8.4.
- Guards: 9.1-9.6, 12.1-12.2.
- Evals: 9.7, 10.1-10.8.

No ID is unmapped. The Supersedes table has no ID of its own and maps to the infrastructure section.

## Upstream sync-back

Designing showed that two approved requirement texts named a mechanism that breaks `docs/standards/testing.md`, and that one rule missed a name form found in the code. Corrected in requirements.md, to be re-approved:

- **SKILLPASS-8.4** named `State::open_at` and `config()` from the skill test. The standard forbids a document test touching the machine, so 8.4 now says the binary refuses the key when it is set in a scratch `HIVE_HOME`, checked by an integration test.
- **SKILLPASS-12.2** said the skill test's temporary state root lives under a temp directory. It now says the config-key integration test's `HIVE_HOME` is a scratch directory under `/private/tmp/hive-it-*`, and that `tests/skill.rs` touches nothing.
- **SKILLPASS-7.2** listed only trailing placeholders. It now also accepts a template whose leading `{…}` is filled by another literal (bench series names, src/bench/mod.rs:572-590). It adds the receipt-envelope-key kind (`hive_version`), checked through `ReceiptBuilder`, and the frontmatter-key kind (`metadata.hive-version`), checked against the skill's own frontmatter.
