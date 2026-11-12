# Tasks: Skill pass (one hive skill guide that covers every command)

> **For agentic workers:** after plan approval, pick one execute skill —
> `build-in-waves` (continuous dependency-aware scheduler), `build-by-story` (human-gated story review
> units), or `build-inline` (controller implements, no implementer subagents).
> The chosen skill writes `Execution-mode:`. Steps use checkbox (`- [ ]`) syntax
> for tracking.

Feature code: SKILLPASS
Status: Draft
Date: 2026-11-12
Execution-mode: unset
Max-concurrency: auto
Requirements: ./requirements.md
Design: ./design.md

**Goal:** Rewrite the hive skill as one guide organised by what the user wants done, hold it and the README to the binary's real flags, names and config keys by test, and record before-and-after agent runs on Sonnet and Haiku that show the rewrite is followed.

**Architecture:**
- `plugin/skills/hive/SKILL.md` is rewritten in place under the goal headings of design Decision 2.
- `tests/skill.rs` gains a name check in a private module `tests/skill/names.rs`, made of a classifier, typed rules and a src literal lexer (design Decisions 3-5), plus a README pass (Decision 7).
- A new integration test `tests/config_keys.rs` checks the README's config keys against the binary (Decision 6).
- Agent runs on a scratch fixture are recorded in `skills/hive/TESTS.md` and `skills/hive/eval.json`, with every RED run made before the rewrite (author-skills, Lock L10).

**Tech Stack:** Rust 2021, `cargo test` (integration tests in `tests/`), clap 4 (`derive`, `CommandFactory`), serde_json; agents `skill-tester` (Sonnet) and `skill-tester-haiku` (Haiku) through the Agent tool; zsh for the fixture script.

## Global Constraints

Canonical sources, referenced by every task and not copied:

- `docs/standards/INDEX.md` (sha256 `80cd41328c0f…`), with its domain files:
  - `docs/standards/testing.md` (`6659c2301761…`): the merge gate; the document-test class, where `tests/skill.rs` touches nothing on the machine; the integration class with a scratch `HIVE_HOME` under `/private/tmp/hive-it-*`.
  - `docs/standards/code.md` (`afdcc8b89136…`): no requirement IDs in source, comments or test names; a `//!` comment on every module.
- Commands: `docs/agents/project.md` § verify commands.
  - Task gate: `cargo test --test skill`, or `cargo test --test config_keys`.
  - Close: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`, `cargo check --all-targets` and `cargo check --target x86_64-unknown-linux-gnu`, or "non-macOS compile check not run: target not installed".
- Architecture spine: `docs/architecture/INDEX.md` (`d290fdb59a37…`). This plan relies on ARCH-2 (no new write path), ARCH-8 (scope from arguments) and ARCH-10 (counters measured; durations only from bench).
- Run rules from the operator's charter (`.skills/_pending-hive-skill-pass/auto/charter.md`):
  - commit in the worktree, never push;
  - no file under `src/` changes;
  - no version bump;
  - agents run only on the scratch fixture under a temporary directory;
  - dropping one of today's instructions, or raising the 150-line budget, stops the task and goes to the operator.
- Team: Solo (docs/agents/project.md `## Team`). No reviewer roster is invented.

## File Structure

| Path | Change | Responsibility |
|---|---|---|
| `plugin/skills/hive/SKILL.md` | Modify | The guide: frontmatter plus a goal-organised body, at most 150 lines |
| `tests/skill.rs` | Modify | Document tests: existing guards with pins updated; the skill and README name checks; the README pass |
| `tests/skill/names.rs` | Create | Private module behind `#[path = "skill/names.rs"] mod names;`: inline span finder, classifier, typed rules, src literal lexer |
| `tests/config_keys.rs` | Create | Integration test: each README config key is accepted by the binary and has its effect; an unknown key is refused |
| `README.md` | Modify | Quick start in the skill's goal order; the Commands table with real command lines and long flags; the two recoveries |
| `skills/hive/TESTS.md` | Modify | Fixture (2026-11-12), predictions, RED and GREEN runs per scenario and model, trigger test, instruction map, rename demonstration |
| `skills/hive/eval.json` | Modify | `behavior` entries for new scenarios; entries 8-11 re-pointed; contract entries 6-7 re-quoted |
| `docs/specs/2026-10-29-agent-skill/requirements.md` | Modify | Strike the superseded clauses of SKILL-2.4 and SKILL-4.2 |
| `docs/specs/2026-11-02-store-seed/requirements.md` | Modify | Strike SEED-8.1's "inside its existing headings" |
| `docs/specs/2026-11-09-store-gc/requirements.md` | Modify | Strike STOREGC-10.11's seven-headings clause |
| `docs/specs/2026-11-09-bench/requirements.md` | Modify | Strike the heading clauses of BENCH-8.1 and BENCH-9.4 |

Outside the repo, under the run directory and not committed: `.skills/_pending-hive-skill-pass/auto/fixture.sh`, which builds the scratch fixture (design Decision 9).

Retrieval (fsubr-1.2; rerun after the stage review over all 11 mapped paths, against the tree with this file present; output in `.skills/_pending-hive-skill-pass/auto/plan-retrieval.txt`):
- owns_coverage is 10/10; observations [] (no active overlay).
- `blast_radius`:
  - `plugin/skills/hive/SKILL.md` and `tests/skill.rs` → CORE, GCPLAN, SEED, SKILL, STOREGC;
  - `README.md` → AUTOGC, CORE, EXPLAIN, SEED, SKILL, STOREGC;
  - `skills/hive/TESTS.md` and `eval.json` → GCPLAN, SEED, SKILL;
  - `docs/specs/2026-11-02-store-seed/requirements.md` → STOREGC, whose tasks edited it;
  - the other three requirement files and both new test files → no owner but SKILLPASS.
- `cluster(SKILLPASS)`: SKILLPASS, then SEED (5 shared paths), SKILL (5), GCPLAN (4), STOREGC (4), CORE (3), AUTOGC (1), EXPLAIN (1). `members_truncated` false.

Every neighbour is Shipped on `main` (54a9e1e; main has since moved to e2c2130, a roadmap-only commit), so no other plan collides on these files.

---

### Task 1: Fixture, predictions and RED runs with today's skill

**Files:**
- Modify: `skills/hive/TESTS.md`
- Test: `skills/hive/TESTS.md` (the recorded runs are the test)

**Reuse:** rung 2 — `skills/hive/TESTS.md`'s fixture recipe (:5-17) and run format, `skills/hive/eval.json`'s entry shape; rung 4 — the `skill-tester` (Sonnet) and `skill-tester-haiku` agents; rung 7 — the fixture script (Decision 9)

**Interfaces:**
- Consumes: today's skill text, from `git show 23defc9:plugin/skills/hive/SKILL.md`.
- Produces:
  - TESTS.md headings `## Fixture (2026-11-12)` and `## Predictions`;
  - one `## <scenario>` heading per scenario: `## Free disk across named lanes`, `## Three lanes`, `## Seed finds no source`, `## Seed into an unfilled store`, `## Toolchain has no store row`, `## A registered main is gone`, `## Free space (2026-11-12)`, `## Why so many copies (2026-11-12)`, `## Version mismatch (2026-11-12)`, `## Plan then apply (2026-11-12)`, `## Bench cold against seeded (2026-11-12)`;
  - under each, `### RED (skill at 23defc9), Sonnet` and `### RED (skill at 23defc9), Haiku`;
  - under the free-space scenario, `### RED (no skill), Sonnet` and `### RED (no skill), Haiku`.

**Depends-on:** none

- [ ] **Step 1: Build the fixture.**
  - Write `fixture.sh` under the run directory. It makes a fresh random root under `/private/tmp` with project-shaped names only (for example `orbit/`, `ledger/`, no test vocabulary). It installs the worktree's binary with `cargo install --locked --path . --root <root>/bin`, and writes `env.sh` setting `PATH`, `HIVE_HOME=<root>/hive` and `CARGO_HOME=<root>/cargo-home`.
  - Registry units come from a local registry, because only registry units go in the store (CONTEXT.md, Registry unit) and a path dependency fills nothing. The script mirrors `tests/common/registry_fixture.rs` (lines 86-135): it packs `leafcrate` and `depcrate` (with a build script, plus a feature `extra` for this fixture) into `<root>/registry`, writes their index, and writes `<root>/cargo-home/config.toml` with `[source.crates-io] replace-with = "fixture"` and `[source.fixture] local-registry = "<root>/registry"`. Every repo depends on `depcrate = "1"` and runs `cargo generate-lockfile --offline` and commits `Cargo.lock`, so builds, agents' recovery builds and bench run `--offline --locked` without the network or the real `~/.cargo`.
  - The fixture holds every state in SKILLPASS-10.5:
    - repo `orbit` depending on the registry `depcrate`, built both without and with `--features depcrate/extra`, so `explain` reports two groups (L17) and the first `seed` fills the store with registry units;
    - three linked worktree lanes under `orbit/.worktrees/`;
    - repo `ledger`, registered after `orbit` has filled the store and never built;
    - a second `HIVE_HOME` that has never been filled;
    - repo `atlas`, registered and then its main deleted;
    - an even bench pair per arm.
  - For `toolchain.missing`, set `"toolchains": {}` in a copied `store.db`, keep `store/CACHEDIR.TAG`, and run `seed` once to see whether the finding appears. Record the attempt and its receipt under `## Toolchain has no store row`.
  - In a third scratch `HIVE_HOME`, run `cargo-hive doctor --json` first, so that capabilities are recorded (without them `gc` refuses at src/cli.rs lines 974-979, before the window code). Then drive `cargo-hive gc --json` with no repo registered and `gc_window_days: 0`, and record whether the receipt carries `store.window_invalid` (U5, needed by Task 11; a static trace with ready capabilities says it reaches src/cli.rs line 1046).
  - Record the fixture as `## Fixture (2026-11-12)`. Copy the pristine state aside, and restore it before each run that can write.

  Run: `source <root>/env.sh && cargo-hive doctor --json` — expect exit 0, every check passing. Then `cargo-hive seed <orbit lane> --json` — expect `fill_entries_added` above 0. If it is 0, the fixture holds no registry unit: fix the fixture before any run.
- [ ] **Step 2: Write predictions before any run.** Under `## Predictions`, predict each scenario's RED with today's skill:
  - E3b `walk.unreadable` fails: its request is a `status` and `gc --dry-run` of the registered repos while `atlas`'s main is gone, so the finding comes from `status`/`gc` (src/cli.rs lines 610 and 995), and SKILL.md line 44 at 23defc9 tells the agent to run `repo remove --missing` only for `apply` of an orphan plan. An `apply`-refusal variant of E3b is also run, and is marked a regression guard if its RED passes;
  - E2 fails, because there is no "build it, then seed the rest";
  - E3a `source.none` fails, because no action is given;
  - E3c `store.tag` fails, because the skill does not name it;
  - version mismatch fails on the unregister ask, because `repo remove` is in neither list at lines 18-19;
  - E1 is unknown;
  - the other regressions pass.

  A RED that passes is marked regression guard.
- [ ] **Step 3: Run every RED.** For each scenario and model, dispatch `skill-tester` and `skill-tester-haiku`. Use a fresh context per run, the prompt shape at TESTS.md line 17, "You have this skill loaded:" followed by today's skill text, and the scenario's request. E3c uses the unfilled `HIVE_HOME`, and the version-mismatch run uses the skill text with `metadata.hive-version` edited to `0.0.9` in the prompt only. Quote each agent's commands verbatim under its `### RED` heading, scored from the transcript, never from the agent's own report.
- [ ] **Step 4: Run the no-skill free-space RED** on both models on the new fixture, and record it under `## Free space (2026-11-12)`.
- [ ] **Step 5: Check the gate.** Each instruction that SKILLPASS-10.8 says is added or changed has a recorded failing RED, or a recorded attempt that shows why not (`toolchain.missing`). A changed instruction whose RED passed stops this task: report it as "nothing to fix" for that instruction (author-skills) before any text is written. Today's routing row at SKILL.md line 34 at 23defc9 ("deleted or should go → `repo remove --missing`") may make E3b's widened RED pass. If it does, report it to the operator as a conflict between author-skills and the approved SKILLPASS-3.2, not as a dropped instruction. [tasks-review round 2]

  Run: `cargo test --test skill` — expect: pass (no text changed; the new headings do not disturb `eval_entries_carry_their_evidence`).
- [ ] **Step 6: Commit.** `git commit -m "test(skill): record RED runs of today's hive skill on a rebuilt fixture"`

_Requirements: SKILLPASS-10.1, SKILLPASS-10.2, SKILLPASS-10.3, SKILLPASS-10.5, SKILLPASS-10.8_

---

### Task 2: Name check against the binary

**Files:**
- Create: `tests/skill/names.rs`
- Modify: `tests/skill.rs`, `plugin/skills/hive/SKILL.md` (file lines 78 and 80 at 23defc9 only: write the `store_entries_held_*` and bench `_ms` names in full)
- Test: `tests/skill.rs`

**Reuse:** rung 2 — `split_skill`, `scan`'s fence toggle and `repo_file` in `tests/skill.rs`; rung 5 — `clap::CommandFactory` (clap with `derive` is already a dependency, Cargo.toml:19) and `serde_json`; rung 2 — public lib items `cargo_hive::cli::Cli`, `cargo_hive::bench::command::BuildCmd`, `cargo_hive::receipt::{Outcome, Receipt, PlanTierBody, Scope}`, `cargo_hive::plan_tier::build::class_label`, `cargo_hive::plan_tier::record::PlanClass`; rung 7 — the literal lexer and the classifier (no dependency in the project lexes Rust, and adopting `syn` is the operator's call)

**Interfaces:**
- Consumes: `repo_file(rel: &str) -> String`, `split_skill(&str) -> Result<(Frontmatter, String), String>` (tests/skill.rs lines 17, 27).
- Produces, in `tests/skill/names.rs`:
  - `pub fn inline_spans(text: &str) -> Result<Vec<(usize, String)>, String>`: 1-based line and span, outside fences; `Err` names a line with an odd backtick count.
  - `pub fn string_literals(src: &str) -> Vec<String>`: the raw text of each string literal outside test modules, per Decision 4.
  - `pub fn cited_name_failures(text: &str, origin: &str) -> Vec<String>`: one `"<origin>:<line>: `<span>` <reason>"` per failure, in Decision 3's order.
  - Internally, one `OnceLock<Context>` holding the src literals, the built `Cli` command, the README's `## Configuration` keys and the skill's metadata keys.

**Depends-on:** Task 1

- [ ] **Step 1: Write the failing tests** in `tests/skill.rs`:

```rust
#[path = "skill/names.rs"]
mod names;

#[test]
fn the_literal_lexer_reads_the_string_forms_src_uses() {
    let src = r##"
fn f<'a>(x: &'a str) { let q = '"'; let b = b"bytes"; // "comment"
    let r = r#"raw "quoted""#; let s = "esc \" aped"; }
#[cfg(all(test, target_os = "macos"))]
mod exit_code_tests { const T: &str = "only_in_tests"; }
const AFTER: &str = "after_tests";
"##;
    assert_eq!(
        names::string_literals(src),
        vec!["bytes", "raw \"quoted\"", "esc \\\" aped", "after_tests"]
    );
}

#[test]
fn the_name_check_places_every_kind_of_span() {
    let cases: &[(&str, bool)] = &[
        ("check-lib=<package>", true), ("check-al", false),
        ("--dry-run", true), ("--dry-rn", false), ("--help", true),
        ("--missing", true), ("--top", true), ("--misssing", false),
        ("-n", false),
        ("seed", true), ("sed", false),
        ("refused", true), ("refusd", false),
        ("orphan", true), ("orphn", false),
        ("hive_version", true), ("undecided", true), ("hive_versio", false),
        ("metadata.hive-version", true), ("metadata.hive-versio", false),
        ("gc_window_days", true),
        ("cold_build_ms_median", true), ("build_ms_median", false),
        ("~/.hive/store/", true), ("0.1.0", true), ("<id>", true),
        ("sessions_deleted", true), ("reused_fresh", true),
        ("source.none", true), ("source.nne", false),
        ("Cargo.lock", true), ("RESEARCH.md", true),
        ("cargo", true), ("hive", true), ("frobnicate", false),
    ];
    for (span, ok) in cases {
        let failures = names::cited_name_failures(&format!("x `{span}` y\n"), "case");
        assert_eq!(failures.is_empty(), *ok, "`{span}`: {failures:?}");
    }
    assert!(names::inline_spans("one ` two\n").is_err());
}

#[test]
fn the_skill_cites_only_names_the_binary_defines() {
    let failures = names::cited_name_failures(&skill_body(), "SKILL.md body");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
```

Run: `cargo test --test skill` — expect: a compile error, because `tests/skill/names.rs` does not exist.
- [ ] **Step 2: Implement `tests/skill/names.rs`.** Open it with a `//!` comment giving its contract. Implement:
  - the span finder per Decision 5;
  - the lexer per Decision 4: strings, raw strings, byte strings, `//` comments and char literals; literal text kept raw, with escapes not decoded; literals inside an attribute `#[…]` dropped; brace counting to drop the block of a `mod` that follows an attribute run mentioning `test`; a `read_dir` walk of `src/` only, without following symlinks;
  - the classifier per Decision 3's fifteen-step order, each class checked against the public item it names;
  - the trailing-placeholder match `{…}` / `:{…}` and the bench series rule.

  Run: `cargo test --test skill the_literal_lexer the_name_check_places` — expect: pass. Run: `cargo test --test skill the_skill_cites_only` — expect: FAIL naming `_live`, `_undecided`, `_name_meta`, `_busy`, `_whole_store` and `_ms` (design-scan §5).
- [ ] **Step 3: Write those names in full** on the two body lines (`store_entries_held_live`, `store_entries_held_undecided`, `store_entries_held_name_meta`, `store_entries_held_busy`, `store_entries_held_whole_store`; the bench series as `cold_build_ms_median` … written out). If the line budget forces a cut, stop and report: that is the operator's call.

  Run: `cargo test --test skill` — expect: pass, with the body still at most 150 lines.
- [ ] **Step 4: Time it.** Run: `cargo test --test skill`, and record its test time in the commit body. Expect: no agent run, no fixture build, and nothing written outside `target/` (SKILLPASS-12.1, 12.2). The whole `cargo test` stays about a minute at close (docs/agents/project.md verify commands).
- [ ] **Step 5: Commit.** `git commit -m "test(skill): fail when the skill cites a name the binary does not define"`

_Requirements: SKILLPASS-7.1, SKILLPASS-7.2, SKILLPASS-7.3, SKILLPASS-4.2, SKILLPASS-12.1, SKILLPASS-12.2_

---

### Task 3: Restructure the body by user goal, one home per instruction

**Files:**
- Modify: `plugin/skills/hive/SKILL.md`, `tests/skill.rs`, `skills/hive/eval.json`
- Test: `tests/skill.rs`

**Reuse:** rung 2 — the existing `plugin/skills/hive/SKILL.md`, its frontmatter and its 51 instructions (scan.md §5), rewritten in place under Decision 2's headings

**Interfaces:**
- Consumes: `names::cited_name_failures` (Task 2).
- Produces: the body headings, in order: `## Command form`, `## Version first`, `## Free disk`, `## Start new lanes`, `## Delete what gc keeps`, `## Look before acting`, `## Time cold against seeded builds`, `## When a command refuses`, `## Reading a receipt`. Tasks 4-9 fill these sections.
- Produces, in `tests/skill.rs`: one const per goal heading (`FREE_DISK`, `START_LANES`, `PLAN_APPLY`, `LOOK`, `BENCH`, `REFUSES`, `RECEIPT`, plus `VERSION_FIRST`). Tasks 4-9 read sections through these consts, so a heading renamed in Task 12's REFACTOR loop changes one line.

**Depends-on:** Task 2

- [ ] **Step 1: Change the heading pin first.** In `skill_body_fits_the_budget` (tests/skill.rs lines 756-770), replace the exact seven-heading list with two checks: the first heading is `## Command form` and the second is `## Version first` (Decision 8, Lock L8). Then check that every goal-heading const exists in the body. The other headings' names and order stay free for the evals to move (SKILLPASS-5.1). Keep `## Version first` found by name at lines 538-546.

  Run: `cargo test --test skill skill_body_fits_the_budget` — expect: FAIL, headings differ.
- [ ] **Step 2: Move each instruction to its one home.**
  - Move each of today's 51 instructions (scan.md §5) into the section the design's section plan names, as it reads today. Merge every duplicate pair listed in scan.md §4 into its home, and drop the trailing `## Do not` paragraph once each of its bans sits under the goal it guards.
  - Split the command-line block so each section carries its own `--json` lines, covering every shipped subcommand (SKILLPASS-5.2).
  - Update the description per SKILLPASS-5.4, keeping `target` and `cargo clean` (tests/skill.rs lines 122-126).
  - Update eval.json contract entries 6 and 7 to quote the moved sentences verbatim (SKILLPASS-10.7).
- [ ] **Step 3: Check the guards.** Run: `cargo test --test skill` — expect: pass. That covers the budget of at most 150 lines with no `.md`; the version match; every command line parsing with `--json`; no stray mention; the frontmatter rules and banned words; the eval.json shape and quotes; the README strings; the name check. A phrase pin that no longer finds its words fails here. Keep the pin when the moved sentence still carries the phrase, and otherwise replace it with the full name the requirement names (Decision 8). Never delete a pin without its replacement.
- [ ] **Step 4: Sweep for duplicates.** Grep the body for each of today's instructions. Each appears once in ordinary prose (SKILLPASS-5.3). List the map in the commit body as a draft for Task 12's `## Instruction map`.
- [ ] **Step 5: Commit.** `git commit -m "docs(skill): organise the hive skill by what the user wants done"`

_Requirements: SKILLPASS-5.1, SKILLPASS-5.2, SKILLPASS-5.3, SKILLPASS-5.4, SKILLPASS-9.1, SKILLPASS-9.2, SKILLPASS-9.3, SKILLPASS-9.4, SKILLPASS-9.5, SKILLPASS-9.6_

---

### Task 4: Free disk across named lanes

**Files:**
- Modify: `plugin/skills/hive/SKILL.md` (`## Free disk`), `tests/skill.rs`
- Test: `tests/skill.rs`

**Reuse:** rung 2 — the existing `plugin/skills/hive/SKILL.md`, its frontmatter and its 51 instructions (scan.md §5), rewritten in place under Decision 2's headings

**Interfaces:**
- Consumes: the `## Free disk` heading (Task 3), and a helper `fn section(body: &str, heading: &str) -> String`, added here to `tests/skill.rs` and used by Tasks 5-9.
- Produces: `fn section`.

**Depends-on:** Task 3

- [ ] **Step 1: Write the failing test.**

```rust
fn section(body: &str, heading: &str) -> String {
    let start = body.find(&format!("\n{heading}\n")).unwrap_or_else(|| panic!("no {heading}"));
    let rest = &body[start + heading.len() + 2..];
    rest[..rest.find("\n## ").unwrap_or(rest.len())].to_string()
}

#[test]
fn the_skill_frees_disk_on_the_checkouts_the_user_named() {
    let free = section(&skill_body(), FREE_DISK);
    for phrase in ["--dry-run", "`freed_bytes`", "`deleted_bytes_still_shared`",
                   "`store_freed_bytes`", "`unmeasured`", "whole repo", "`store.skipped_scoped`"] {
        assert!(free.contains(phrase), "## Free disk lacks {phrase}");
    }
}
```

Run: `cargo test --test skill the_skill_frees_disk` — expect: FAIL, missing `freed_bytes`.
- [ ] **Step 2: Write the section.** Write SKILLPASS-1.1 and 1.2 into `## Free disk`, with a command-line pair `cargo hive gc /path/to/lane-a /path/to/lane-b --dry-run --json` and the same line without `--dry-run`.

  Run: `cargo test --test skill` — expect: pass.
- [ ] **Step 3: Commit.** `git commit -m "docs(skill): free disk on the named lanes and report freed bytes from the receipt"`

_Requirements: SKILLPASS-1.1, SKILLPASS-1.2_

---

### Task 5: Start several lanes from one seeded build

**Files:**
- Modify: `plugin/skills/hive/SKILL.md` (`## Start new lanes`), `tests/skill.rs`
- Test: `tests/skill.rs`

**Reuse:** rung 2 — the existing `plugin/skills/hive/SKILL.md`, its frontmatter and its 51 instructions (scan.md §5), rewritten in place under Decision 2's headings

**Interfaces:**
- Consumes: `fn section` (Task 4).
- Produces: none.

**Depends-on:** Task 4

- [ ] **Step 1: Write the failing test.**

```rust
#[test]
fn the_skill_seeds_one_lane_builds_it_then_seeds_the_rest() {
    let lanes = section(&skill_body(), START_LANES);
    let seed_one = lanes.find("seed one lane").expect("seed one lane");
    let build = lanes[seed_one..].find("build it").expect("build it");
    assert!(lanes[seed_one + build..].contains("seed the other"), "order: seed one, build, seed the rest");
    for phrase in ["bare `gc`", "`store_bytes`", "gets nothing", "one build"] {
        assert!(lanes.contains(phrase), "## Start new lanes lacks {phrase}");
    }
}
```

Run: `cargo test --test skill the_skill_seeds_one_lane` — expect: FAIL, no "build it".
- [ ] **Step 2: Write the section.** Write SKILLPASS-2.1 to 2.3 into `## Start new lanes`. The `status` counts read after a seed (instruction 42) stay in `## Reading a receipt`, their one home (Task 7). The seed pins in `the_skill_teaches_seeding_a_new_lane` keep reading the whole body.

  Run: `cargo test --test skill` — expect: pass.
- [ ] **Step 3: Commit.** `git commit -m "docs(skill): seed one lane, build it, then seed the rest and bound the store"`

_Requirements: SKILLPASS-2.1, SKILLPASS-2.2, SKILLPASS-2.3_

---

### Task 6: Recoveries for findings that name one

**Files:**
- Modify: `plugin/skills/hive/SKILL.md` (`## When a command refuses`), `tests/skill.rs`
- Test: `tests/skill.rs`

**Reuse:** rung 2 — the existing `plugin/skills/hive/SKILL.md`, its frontmatter and its 51 instructions (scan.md §5), rewritten in place under Decision 2's headings

**Interfaces:**
- Consumes: `fn section` (Task 4).
- Produces: none.

**Depends-on:** Task 5 (same files: `plugin/skills/hive/SKILL.md`, `tests/skill.rs`)

- [ ] **Step 1: Write the failing test.**

```rust
#[test]
fn the_skill_gives_the_recovery_each_refusal_names() {
    let refuses = section(&skill_body(), REFUSES);
    for phrase in ["`source.none`", "`store.tag`", "`toolchain.missing`", "build the main checkout",
                   "could not be read", "`walk.unreadable`", "`explain`",
                   "cargo hive repo remove --missing --json"] {
        assert!(refuses.contains(phrase), "## When a command refuses lacks {phrase}");
    }
    let none = refuses.find("`source.none`").expect("source.none");
    let build = refuses[none..].find("build the main checkout").expect("build the main checkout after source.none");
    assert!(refuses[none + build..].contains("`seed` again"), "after source.none: build the main checkout, then seed again");
}
```

Run: `cargo test --test skill the_skill_gives_the_recovery` — expect: FAIL, no `store.tag`.
- [ ] **Step 2: Write the section.** Write SKILLPASS-3.1 to 3.3 into `## When a command refuses`: the meaning of `source.none`, its build-then-seed recovery, the tag-missing and tag-unreadable cases of `store.tag`, `toolchain.missing`, and `walk.unreadable` from `status`, `gc`, `gc --plan`, `explain` or `apply` for a gone main.

  Run: `cargo test --test skill` — expect: pass, with the name check green on `store.tag` and `toolchain.missing` (src/seed/mod.rs literals).
- [ ] **Step 3: Commit.** `git commit -m "docs(skill): give the recovery for source.none, store.tag, toolchain.missing and walk.unreadable"`

_Requirements: SKILLPASS-3.1, SKILLPASS-3.2, SKILLPASS-3.3_

---

### Task 7: Receipt names as the binary prints them

**Files:**
- Modify: `plugin/skills/hive/SKILL.md` (`## Reading a receipt`), `tests/skill.rs`
- Test: `tests/skill.rs`

**Reuse:** rung 2 — the existing `plugin/skills/hive/SKILL.md`, its frontmatter and its 51 instructions (scan.md §5), rewritten in place under Decision 2's headings

**Interfaces:**
- Consumes: `fn section` (Task 4); `names::cited_name_failures` (Task 2).
- Produces: none.

**Depends-on:** Task 6 (same files)

- [ ] **Step 1: Write the failing test.**

```rust
#[test]
fn the_skill_names_receipt_counts_as_the_binary_keys_them() {
    let receipt = section(&skill_body(), RECEIPT);
    for phrase in ["`counts`", "`reused_fresh:/path/to/target`", "`reused_fresh`",
                   "not Cargo's printed fresh count", "`seeded_fresh`",
                   "`seeded_unneeded`", "`seeded_recompiled`", "`seeded_missing`"] {
        assert!(receipt.contains(phrase), "## Reading a receipt lacks {phrase}");
    }
}
```

Run: `cargo test --test skill the_skill_names_receipt_counts` — expect: FAIL, no `seeded_fresh`.
- [ ] **Step 2: Write the section.** Write SKILLPASS-4.1 and 4.3 into `## Reading a receipt`, with every name in full (SKILLPASS-4.2). The span `reused_fresh:/path/to/target` contains `/`, so the name check skips it as a path, and the bare name is cited apart.

  Run: `cargo test --test skill` — expect: pass.
- [ ] **Step 3: Commit.** `git commit -m "docs(skill): name receipt counts as the binary keys them, bench's seeded_fresh included"`

_Requirements: SKILLPASS-4.1, SKILLPASS-4.2, SKILLPASS-4.3_

---

### Task 8: Hold every write on a version mismatch

**Files:**
- Modify: `plugin/skills/hive/SKILL.md` (`## Version first`), `tests/skill.rs`
- Test: `tests/skill.rs`

**Reuse:** rung 2 — the existing `plugin/skills/hive/SKILL.md`, its frontmatter and its 51 instructions (scan.md §5), rewritten in place under Decision 2's headings

**Interfaces:**
- Consumes: `fn section` (Task 4).
- Produces: none.

**Depends-on:** Task 7 (same files)

- [ ] **Step 1: Write the failing test.** Extend `the_skill_teaches_seeding_a_new_lane`'s `## Version first` check (tests/skill.rs lines 538-546), or add:

```rust
#[test]
fn a_version_mismatch_holds_every_write() {
    let version = section(&skill_body(), VERSION_FIRST);
    let held = &version[version.find("Do not run").expect("Do not run")..];
    for cmd in ["`gc`", "`apply`", "`repo add`", "`repo remove`", "`seed`", "`bench`"] {
        assert!(held.contains(cmd), "the held list lacks {cmd}");
    }
}
```

Run: `cargo test --test skill a_version_mismatch_holds` — expect: FAIL, no `repo remove`.
- [ ] **Step 2: Add `repo remove`** to the held list (SKILLPASS-6.1).

  Run: `cargo test --test skill` — expect: pass.
- [ ] **Step 3: Commit.** `git commit -m "docs(skill): hold repo remove with the other writes on a version mismatch"`

_Requirements: SKILLPASS-6.1_

---

### Task 9: Plan, then apply, whatever the class

**Files:**
- Modify: `plugin/skills/hive/SKILL.md` (`## Delete what gc keeps`), `tests/skill.rs`
- Test: `tests/skill.rs`

**Reuse:** rung 2 — the existing `plugin/skills/hive/SKILL.md`, its frontmatter and its 51 instructions (scan.md §5), rewritten in place under Decision 2's headings

**Interfaces:**
- Consumes: `fn section` (Task 4).
- Produces: none.

**Depends-on:** Task 8 (same files)

- [ ] **Step 1: Write the failing test.** Change `the_skill_teaches_the_plan_flow_for_every_class` (tests/skill.rs lines 685-704) to require `already_consumed` in place of `already consumed`, keep its banned words, and read the phrases from `section(&skill_body(), PLAN_APPLY)`.

  Run: `cargo test --test skill the_skill_teaches_the_plan_flow` — expect: FAIL, `already_consumed` absent.
- [ ] **Step 2: Write SKILLPASS-11.1 into the section** with `already_consumed` (src/plan_tier/apply.rs lines 105 and 125).

  Run: `cargo test --test skill` — expect: pass.
- [ ] **Step 3: Commit.** `git commit -m "docs(skill): plan and apply every class, and name already_consumed as the binary does"`

_Requirements: SKILLPASS-11.1_

---

### Task 10: README described as the skill describes it

**Files:**
- Modify: `README.md` (Quick start, lines 119-151; Commands, lines 153-169), `tests/skill.rs`
- Test: `tests/skill.rs`

**Reuse:** rung 2 — `scan`/`command_words`/`parse` (tests/skill.rs:357-456) and the name check above

**Interfaces:**
- Consumes: `names::inline_spans`, `names::cited_name_failures` (Task 2); `parse(&[String]) -> Result<Parsed, String>` (tests/skill.rs line 426).
- Produces: `#[test] fn readme_describes_the_commands_as_the_skill_does()`.

**Depends-on:** Task 9 (same `tests/skill.rs`; consumes Task 2's `names` and Task 6's recovery wording)

- [ ] **Step 1: Write the failing test.**

```rust
#[test]
fn readme_describes_the_commands_as_the_skill_does() {
    let readme = repo_file("README.md");
    let mut failures = names::cited_name_failures(&readme, "README.md");
    let mut seen = Vec::new();
    for (line, words) in names::readme_command_lines(&readme) {
        if words.iter().any(|w| w.starts_with('-') && !w.starts_with("--") && w.len() == 2) {
            failures.push(format!("README.md:{line}: short flag in `{}`", words.join(" ")));
        }
        match parse(&words) {
            Ok(p) => seen.push(p),
            Err(e) => failures.push(format!("README.md:{line}: {e}")),
        }
    }
    for want in [Parsed::Doctor, Parsed::RepoAdd, Parsed::RepoList, Parsed::RepoRemove, Parsed::Status,
                 Parsed::Gc { dry_run: false, plan: false }, Parsed::Gc { dry_run: false, plan: true },
                 Parsed::Apply, Parsed::Explain, Parsed::Seed, Parsed::Bench] {
        if !seen.contains(&want) { failures.push(format!("README.md has no command line for {want:?}")); }
    }
    for recovery in ["`source.none`", "`walk.unreadable`", "repo remove --missing"] {
        if !readme.contains(recovery) { failures.push(format!("README.md lacks {recovery}")); }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
```

  `names::readme_command_lines(&str) -> Vec<(usize, Vec<String>)>` is added to `tests/skill/names.rs` per Decision 7. It reads mentions only in spans and fenced lines, takes a command line only where the prefix opens its span or line, and replaces `<word>` placeholders with `x`. It does not return a synopsis line, one whose subcommand word is a placeholder (README.md lines 117 and 169). It checks such a line's flags itself, as long hive flags, and adds any failure to `cited_name_failures`'s output for the README.

  Run: `cargo test --test skill readme_describes` — expect: a compile error until `names::readme_command_lines` exists. Once it does, expect FAIL naming `-n` (single-word spans at README.md lines 165 and 182) and the bracketed synopsis rows (lines 160-167). `-n 0` at line 183 sits in a multi-word span, so it is caught only as a short flag on a parsed command line.
- [ ] **Step 2: Rewrite the README.**
  - Quick start goes in the skill's goal order.
  - The Commands table gets one row per command form, each a real command line with long flags (`cargo hive explain /path/to/repo --top 0`), and optional parts in the row's text.
  - Add the `source.none` and `walk.unreadable` recoveries as SKILLPASS-3.1 and 3.2 give them.
  - Keep the two strings at tests/skill.rs lines 569-576, and keep a `cargo hive bench` row.

  Run: `cargo test --test skill` — expect: pass.
- [ ] **Step 3: Commit.** `git commit -m "docs(readme): describe each command as the hive skill does, with long flags and real command lines"`

_Requirements: SKILLPASS-8.1, SKILLPASS-8.2, SKILLPASS-8.3, SKILLPASS-8.5_

---

### Task 11: Config keys checked against the binary

**Files:**
- Create: `tests/config_keys.rs`
- Test: `tests/config_keys.rs`

**Reuse:** rung 2 — `tests/common`'s `Hive::ready`, `Hive::config` and `Hive::json` (tests/common/mod.rs:73, 61, 167); rung 4 — the binary's own `deny_unknown_fields` config read (src/state.rs:243-249, 286), reached through `status --json`

**Interfaces:**
- Consumes: `common::Hive` (`ready() -> Hive`, `config(&self, json: &str)`, `json(&self, args) -> (Output, serde_json::Value)`, `run(&self, args) -> Output`, `home.path: PathBuf`).
- Produces: `#[test] fn every_config_key_the_readme_names_is_accepted()`, `#[test] fn an_unknown_config_key_is_refused()`.

**Depends-on:** Task 1 (U5 result)

- [ ] **Step 1: Write the failing test.**

```rust
//! Every config key the README names is accepted by the binary and has its effect.
#![cfg(target_os = "macos")]
mod common;
use common::Hive;

fn configuration_keys(readme: &str) -> Vec<String> {
    let start = readme.find("\n## Configuration\n").expect("README has ## Configuration");
    let section = &readme[start..];
    let section = &section[..section[1..].find("\n## ").map_or(section.len(), |i| i + 1)];
    section.lines()
        .filter_map(|l| l.strip_prefix("| `").and_then(|r| r.split('`').next()))
        .map(str::to_string)
        .collect()
}

fn notes(receipt: &serde_json::Value) -> Vec<String> {
    receipt["notes"].as_array().unwrap().iter().map(|n| n.as_str().unwrap().to_string()).collect()
}

fn finding_kinds(receipt: &serde_json::Value) -> Vec<String> {
    receipt["findings"].as_array().unwrap().iter().map(|f| f["kind"].as_str().unwrap().to_string()).collect()
}

#[test]
fn every_config_key_the_readme_names_is_accepted() {
    let readme = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md")).unwrap();
    let keys = configuration_keys(&readme);
    assert!(!keys.is_empty(), "README's Configuration table names no key");
    for key in keys {
        let hive = Hive::ready();
        let dir = hive.home.path.join("root-for-test");
        std::fs::create_dir_all(&dir).unwrap();
        let dir = dir.display().to_string();
        match key.as_str() {
            "secondary_roots" | "cache_roots" => {
                hive.config(&format!(r#"{{"{key}": ["{dir}"]}}"#));
                let (out, receipt) = hive.json(&["status"]);
                assert!(out.status.success(), "{key}: status failed");
                assert!(!finding_kinds(&receipt).contains(&"refusal".to_string()), "{key}: refused");
                assert!(notes(&receipt).iter().any(|n| n.starts_with(&format!("{key}:")) && n.contains(&dir)),
                        "{key}: no note naming {dir}: {:?}", notes(&receipt));
            }
            "gc_window_days" => {
                hive.config(r#"{"gc_window_days": 0}"#);
                let (_, bad) = hive.json(&["gc"]);
                assert!(finding_kinds(&bad).contains(&"store.window_invalid".to_string()), "window 0 not read");
                hive.config(r#"{"gc_window_days": 7}"#);
                let (out, good) = hive.json(&["gc"]);
                assert!(out.status.success(), "window 7: gc failed");
                assert!(!finding_kinds(&good).contains(&"store.window_invalid".to_string()), "window 7 refused");
            }
            other => panic!("no test value for {other}"),
        }
    }
}

#[test]
fn an_unknown_config_key_is_refused() {
    let hive = Hive::ready();
    hive.config(r#"{"secondary_rootz": ["/nowhere"]}"#);
    let out = hive.run(&["status", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("secondary_rootz"));
}
```

  Run: `cargo test --test config_keys` — expect: compile, and fail on any assertion whose premise U5 or the note format disproves (the notes come from src/cli.rs lines 649-650).
- [ ] **Step 2: Make it pass without touching `src/`.**
  - U5: if Task 1 recorded that bare `gc` with no registered repo returns before src/cli.rs line 1044, register one empty repo first with `common::register` (tests/common/mod.rs line 270) on an `init_repo` checkout (line 280). This is design Decision 6's fallback.
  - If the note text or the refusal channel differs from the assertions (with `--json` the error may sit in the stdout receipt, not on stderr), correct the assertion to what the binary prints, citing the line. The corrected assertion must still name the unknown key, wherever the binary prints it. If an assertion can't be met without a `src/` change, stop and report.

  Run: `cargo test --test config_keys` — expect: pass.
- [ ] **Step 3: Show it can fail.** In a scratch copy of the README, rename `gc_window_days` to `gc_window_dayz` in the Configuration table and run the test against it by setting the read path in a temporary local edit. Expect: FAIL, "no test value for gc_window_dayz". Then rename `"secondary_rootz"` back to a valid key in the unknown-key test and expect FAIL on its exit-code assertion. Revert both edits, and record the two failure lines in the commit body.
- [ ] **Step 4: Commit.** `git commit -m "test(config): check every config key the README names against the binary"`

_Requirements: SKILLPASS-8.4, SKILLPASS-12.2_

---

### Task 12: GREEN runs, trigger test, instruction map and eval entries

**Files:**
- Modify: `skills/hive/TESTS.md`, `skills/hive/eval.json`, `plugin/skills/hive/SKILL.md` (only for REFACTOR fixes that a GREEN run shows are needed)
- Test: `skills/hive/TESTS.md`, `tests/skill.rs`

**Reuse:** rung 2 — `skills/hive/TESTS.md`'s fixture recipe (:5-17) and run format, `skills/hive/eval.json`'s entry shape; rung 4 — the `skill-tester` (Sonnet) and `skill-tester-haiku` agents; rung 7 — the fixture script (Decision 9)

**Interfaces:**
- Consumes: the scenario headings and fixture (Task 1); the rewritten body (Tasks 3-9).
- Produces:
  - `### GREEN (rewrite), Sonnet` and `### GREEN (rewrite), Haiku` under each scenario;
  - `## Trigger (2026-11-12)`;
  - `## Instruction map`;
  - eval.json `behavior` entries with `derived_from` set to each new `## ` heading; entries 8-11 re-pointed to `## Plan then apply (2026-11-12)`, `## Three lanes` and `## Bench cold against seeded (2026-11-12)`.

**Depends-on:** Task 10, Task 11

- [ ] **Step 1: Run every GREEN.** Rerun each Task 1 scenario on both models, with the worktree skill text pasted (never the installed plugin), on the restored fixture. Score each from its transcript against SKILLPASS-10.4: `--json`, scope as arguments, counts quoted from the receipt, no `cargo clean`, no hand deletion, and each scenario's locked behaviour (E1-E3c, the `repo remove` hold, plan then apply).
- [ ] **Step 2: REFACTOR loop.**
  - A GREEN that fails on Haiku gets its rationalisation quoted verbatim, a counter added in the skill (within 150 lines), and a rerun. Stop when a run shows no new rationalisation.
  - Every text change reruns `cargo test --test skill`.
  - A fix that needs a dropped instruction or more than 150 lines stops here for the operator.
- [ ] **Step 3: Run the trigger test.** Write 8-10 should-fire and 8-10 should-not-fire queries (SKILLPASS-10.6). The near-misses are: a `cargo clean` or disk ask in an unregistered repo, sccache or linker speed-up, and `git worktree` housekeeping. Run each fresh on both models with the hive description among neighbours, and record which skill each picks.
- [ ] **Step 4: Read the README against the skill.** For each command, check that the README and the final body give the same flags and recoveries (SKILLPASS-8.1, 8.5, the design's manual seam). Record each difference found and fixed under `## Instruction map`.
- [ ] **Step 5: Write the instruction map.** Map each of today's 51 instructions to its line in the final body, marking 10, 19, 28 and 32 as changed (SKILLPASS-5.3).
- [ ] **Step 6: Add the eval entries.** Add `behavior` entries per new scenario. Re-point 8 (plan before closure deletion) and 9 (apply one plan) to `## Plan then apply (2026-11-12)`, 10 (seed a new lane) to `## Three lanes`, and 11 (bench) to `## Bench cold against seeded (2026-11-12)`. Keep the SKILL-5.3 shape.

  Run: `cargo test --test skill eval_entries_carry_their_evidence` — expect: pass.
- [ ] **Step 7: Commit.** `git commit -m "test(skill): record GREEN runs of the rewritten hive skill on Sonnet and Haiku"`

_Requirements: SKILLPASS-10.1, SKILLPASS-10.4, SKILLPASS-10.6, SKILLPASS-10.7, SKILLPASS-10.8, SKILLPASS-9.7, SKILLPASS-5.3, SKILLPASS-5.4_

---

### Task 13: Show a rename fails, once per class

**Files:**
- Modify: `skills/hive/TESTS.md` (`## Rename demonstration`)
- Test: `tests/skill.rs`, `tests/config_keys.rs`

**Reuse:** rung 2 — the tests from Tasks 2, 10 and 11, run as they are

**Interfaces:**
- Consumes: `cargo test --test skill`, `cargo test --test config_keys`.
- Produces: `## Rename demonstration` in TESTS.md.

**Depends-on:** Task 12

- [ ] **Step 1: Run each rename.** In a scratch copy of the worktree under `/private/tmp`, apply one rename per class, run the gate, record the failure line verbatim, and discard the copy. Each rename changes only the printed name, never a Rust identifier, so `tests/skill.rs`'s exhaustive `parse` match (line 446) still compiles: a flag through `#[arg(long = …)]`, a subcommand through `#[command(name = …)]`, an outcome through `#[serde(rename = …)]`, and literals by editing the string. The classes:
  - a long flag (`--dry-run` in src/cli.rs);
  - a subcommand;
  - a bench shape (`check-all`);
  - an outcome variant (`Partial`'s serde name);
  - a class label (`orphan`);
  - an envelope key (`hive_version`);
  - a bench series literal (`cold_build_ms`);
  - a count literal with one non-test literal in src (`fill_targets_skipped`, src/seed/mod.rs line 513). `fill_entries_held` would not fail, because src/cli.rs line 1764 holds a second copy: that is design Decision 4's residual;
  - a finding kind (`source.none`);
  - a config key (`gc_window_days`, failing `tests/config_keys.rs`);
  - a short flag written into README (`-n`).
- [ ] **Step 2: Confirm the worktree is unchanged.** Run: `git status --porcelain` in the worktree — expect: only `skills/hive/TESTS.md` changed.
- [ ] **Step 3: Commit.** `git commit -m "test(skill): record that a rename of each cited kind fails the tests"`

_Requirements: SKILLPASS-7.1, SKILLPASS-7.2, SKILLPASS-8.3, SKILLPASS-8.4_

---

### Task 14: Strike the superseded clauses

**Files:**
- Modify: `docs/specs/2026-10-29-agent-skill/requirements.md`, `docs/specs/2026-11-02-store-seed/requirements.md`, `docs/specs/2026-11-09-store-gc/requirements.md`, `docs/specs/2026-11-09-bench/requirements.md`
- Test: the `audit-trace` skill (docs-only)

**Reuse:** rung 2 — the strike-through convention of `docs/specs/2026-10-29-agent-skill/requirements.md:37`

**Interfaces:**
- Consumes: requirements.md § Supersedes.
- Produces: inline `~~clause~~ (superseded by SKILLPASS-N.M)` edits.

**Depends-on:** none. It runs only once requirements.md is `Status: Approved` by the operator.

- [ ] **Step 1: Strike each clause.** Wrap each clause in the Supersedes table in its own file, leaving the rest of each criterion live: SKILL-2.4 and SKILL-4.2 → SKILLPASS-5.2; SEED-8.1, BENCH-8.1, STOREGC-10.11 and BENCH-9.4 → SKILLPASS-5.1.
- [ ] **Step 2: Check the trace.** REQUIRED SUB-SKILL: use `audit-trace`. Expect: no unknown citation, and no live criterion that contradicts SKILLPASS.
- [ ] **Step 3: Commit.** `git commit -m "docs(specs): strike the skill clauses SKILLPASS supersedes"`

_Requirements: SKILLPASS-5.1, SKILLPASS-5.2_
