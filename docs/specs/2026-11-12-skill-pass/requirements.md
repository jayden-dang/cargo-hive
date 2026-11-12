# Requirements: Skill pass (one hive skill guide that covers every command)

Feature code: SKILLPASS
Status: Draft
Date: 2026-11-12

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

Roadmap item: ROAD-13 (`docs/roadmap/INDEX.md:126`, card `docs/roadmap/roads/ROAD-13.md`). Tracker: #10.

Sources: the frame-change close package approved by hive-skill-pass-interpret on 2026-11-12, round 3 (`.skills/_pending-hive-skill-pass/auto/close-package.md`, rows 1-15). Also the scan digest (`scan.md`, same directory), the knowns (`knowns.md`) and the fork cards with interpret's picks W1-W3 (`ledger.md`). Labels in square brackets give each value's class under the run's charter: **Lock** values are the operator's (card and roadmap); **Weigh** values were derived by a session from code or a Lock; **agent-decided** values were chosen where nothing else decided. No Weigh or agent-decided value here is the operator's own choice.

Terms:
- "The skill" is `plugin/skills/hive/SKILL.md`, and "the body" is its text after the closing `---` of the frontmatter.
- "The README" is `README.md`, and "the skill test" is `tests/skill.rs`.
- "Today's instructions" are the 51 numbered instructions the skill gives at commit 23defc9, as listed in `scan.md` §5. This feature copies that list into `skills/hive/TESTS.md` (SKILLPASS-5.3).
- "A cited name" is any single-word inline code span in the body or the README that names something the binary defines: a subcommand, a long flag, a bench build shape, a plan outcome, a plan class, a receipt count key, a finding kind or reason, or a config key.

Files this feature touches:
- `plugin/skills/hive/SKILL.md` — rewritten. Behaviour to guard: SKILLPASS-9.1 to 9.4.
- `tests/skill.rs` — gains the name check and the README pass; its heading and phrase pins change. Behaviour to guard: SKILLPASS-9.1 to 9.5.
- `tests/config_keys.rs` — new integration test (SKILLPASS-8.4); no behaviour to guard.
- `README.md` — Quick start and Commands rewritten. Behaviour to guard: SKILLPASS-9.6.
- `skills/hive/TESTS.md` and `skills/hive/eval.json` — new runs and headings added. Behaviour to guard: SKILLPASS-9.7.
- `docs/specs/catalog/hive.md` — one new card row; no behaviour to guard.
- `docs/specs/2026-10-29-agent-skill/requirements.md`, `docs/specs/2026-11-02-store-seed/requirements.md`, `docs/specs/2026-11-09-store-gc/requirements.md`, `docs/specs/2026-11-09-bench/requirements.md` — strike-through supersede edits (see Supersedes); no behaviour to guard.
- No file under `src/` changes.

## 1. Free disk across several lanes

**Story:** As an agent asked to "free disk across my lanes", I want the skill to tell me to preview and then collect on the checkouts the user named, so that I report the bytes hive freed from its receipt instead of deleting build output myself.

- **SKILLPASS-1.1** THE SYSTEM SHALL tell the agent in the skill, in one section about freeing disk, to run `gc --dry-run` with the checkouts the user named as arguments, then `gc` with the same arguments, and to report freed bytes from the receipt's `freed_bytes` count, with `deleted_bytes_still_shared` reported apart. When the receipt lists `freed_bytes` under `unmeasured`, the agent shall say the size was not measured, and the store's freed bytes from a bare `gc` are reported from `store_freed_bytes`. [Lock L4; count names Weigh, src/cli.rs:893-910,1102-1105, src/seed/evict.rs]
- **SKILLPASS-1.2** THE SYSTEM SHALL state in that section that a path inside a registered repo, or a linked worktree of one, makes `gc` act on that whole repo with a note. It shall also state that `gc` with repo arguments leaves the store alone and notes `store.skipped_scoped`, while bare `gc` also evicts from the store by age, toolchain and the 40 GB cap. Each fact is stated once in the body. [today's instructions 14, 25, 26; Weigh from L3, close-package row 7]

## 2. Start several lanes from one seeded build

**Story:** As an agent starting three build-in-waves lanes, I want the skill to tell me the order to seed, build and collect in, so that the second and third lanes are seeded from a build and the store is bounded after the batch.

- **SKILLPASS-2.1** WHEN the user starts several new secondary checkouts THE SYSTEM SHALL tell the agent in the skill to seed one lane, build it, and only then seed the remaining lanes. [Lock L5]
- **SKILLPASS-2.2** WHEN a batch of lanes finishes THE SYSTEM SHALL tell the agent in the skill to run bare `gc`, and also to run it whenever `seed`'s `store_bytes` passes 40 GB. [Lock L5; today's instruction 34]
- **SKILLPASS-2.3** THE SYSTEM SHALL state in the skill, once each:
  - what `seed` copies, and from which source (the checkout whose `Cargo.lock` matches, else the most recently built, so every unit comes from one build);
  - that `seed` takes one registered checkout;
  - that a checkout which already has a target gets nothing;
  - that the seed receipt keeps its fill counts apart from its clone counts and holds no duration;
  - which counts `status` reports afterwards (`reused_fresh`, `seeded_unneeded`, `seeded_recompiled`, `seeded_missing`), with `reused_fresh` not being Cargo's printed fresh count.

  [today's instructions 27, 30, 31, 41, 42; SEED-8.1's content]

## 3. Recover from a finding that names a recovery

**Story:** As an agent whose receipt carries a finding that names a recovery (`source.none`, `store.tag`, `toolchain.missing`, `walk.unreadable`), I want the skill to give me that recovery, so that I take it instead of retrying the same command.

- **SKILLPASS-3.1** WHEN `seed`'s receipt carries the finding `source.none` THE SYSTEM SHALL tell the agent in the skill that no source target of the repo has a record for the current toolchain (no checkout of it has built yet, or none has with this toolchain), and to build the main checkout and then run `seed` again, not to retry `seed` first. [Lock L6; meaning Weigh from code, src/seed/mod.rs:284-288, close-package row 9]
- **SKILLPASS-3.2** WHEN a receipt from `status`, `gc`, `gc --plan`, `explain` or `apply` carries `walk.unreadable` for a path that is a registered repo's main checkout that no longer exists THE SYSTEM SHALL tell the agent in the skill to run `repo remove --missing`, which unregisters every such repo, and then to rerun the command. [Lock L6; wider sources Weigh from code, src/cli.rs:610,995, src/plan_tier/apply.rs:223, close-package row 10; `explain` added from the req-review drive, `req-review.md` item 4]
- **SKILLPASS-3.3** WHEN `seed`'s receipt carries `store.tag` because the store's tag is missing (the store has not been filled yet), or `toolchain.missing` (the store has no row for this toolchain), THE SYSTEM SHALL tell the agent in the skill to take the same recovery as SKILLPASS-3.1: build the main checkout with the current toolchain, then run `seed` again. When `store.tag` says the tag could not be read, the skill SHALL tell the agent to report the finding's detail to the user instead, since a build does not fix it. [agent-decided: an added instruction, since a first seed on a new machine meets `store.tag` before it can meet `source.none`; the causes are Weigh from code: missing tag src/seed/mod.rs:262-268, unreadable tag 269-273, `toolchain.missing` 290-302; requirements-review.md finding 3]

## 4. Read every receipt by its real names

**Story:** As an agent reading a hive receipt, I want the skill to name each count I report exactly as the binary prints it, so that I quote the right key from the JSON.

- **SKILLPASS-4.1** THE SYSTEM SHALL state in the skill that receipt counts sit in the receipt's `counts` object, and that counts per target or per profile carry a suffix after the name, such as `:` and a path (`reused_fresh:/path/to/target`). [Weigh from code, src/cli.rs:862,893-910, close-package row 12; `req-review.md` item 6]
- **SKILLPASS-4.2** THE SYSTEM SHALL write every receipt name in the body in full, with no fragment that stands for several names (no `_live` for `store_entries_held_live`). [agent-decided wording, W1, close-package row 2]
- **SKILLPASS-4.3** THE SYSTEM SHALL name in the skill bench's per-sample seed counts `seeded_fresh`, `seeded_unneeded`, `seeded_recompiled` and `seeded_missing`, and say that bench's `seeded_fresh` and status's `reused_fresh` are different names. [Weigh from code, src/bench/mod.rs:215-218, close-package row 12]

## 5. One guide, organised by what the user wants done

**Story:** As an agent in any registered repo, I want to read the hive skill once and find every command under the job I was asked to do, with each instruction in one place, so that nothing I read is stale, repeated or contradicted elsewhere in the skill.

- **SKILLPASS-5.1** THE SYSTEM SHALL organise the body under headings named for what the user wants done, with the version check as the first section after the command form. The heading names and order are agent-decided. [Lock L3, L8; headings Weigh from L3/L16, close-package row 6; names and order agent-decided, card Delegated]
- **SKILLPASS-5.2** THE SYSTEM SHALL give in the skill a command line with `--json` for every subcommand the CLI ships, apart from hidden ones: `doctor`, `repo add`, `repo list`, `repo remove`, `status`, `gc` (with and without `--dry-run`, and with `--plan`), `apply`, `explain`, `seed` and `bench`. [Lock L1, L3; supersedes SKILL-2.4's "exactly the MILE-1 commands", Weigh from L3]
- **SKILLPASS-5.3** THE SYSTEM SHALL keep each of today's instructions in the body, stated in ordinary prose at exactly one place. `skills/hive/TESTS.md` records the map from each numbered instruction to its line in the new body. Instructions 28 and 32 change as SKILLPASS-3.1 and 3.2 say, instruction 19 changes as SKILLPASS-11.1 says, and instruction 10 changes as SKILLPASS-6.1 says. [Lock L3 "no … duplicated instruction"; Ask me: none dropped]
- **SKILLPASS-5.4** THE SYSTEM SHALL keep the frontmatter `description` within SKILL-2.1's rules (it names a large `target/` dir, a disk filling up, reclaiming Rust build space and a crate built in many copies, and says to use it instead of `cargo clean` or deleting `target/`). The description shall add seeding new worktrees or lanes before their first build and timing cold against seeded builds. [agent-decided wording, close-package row 14]

## 6. Hold every write when the versions differ

**Story:** As an agent whose installed hive's major and minor version differ from the skill's, I want the skill to hold every command that writes, `repo remove` included, so that I ask the user to update before a command the skill may describe wrongly changes anything.

- **SKILLPASS-6.1** IF the binary's major or minor version differs from `metadata.hive-version` THEN THE SYSTEM SHALL tell the agent in the skill not to run `gc`, `apply`, `repo add`, `repo remove`, `seed` or `bench`, and to ask the user to update first. `doctor`, `repo list`, `status` and `explain` may still run. [today's instructions 8-10; `repo remove` agent-decided, close-package row 11; it writes the registry, src/cli.rs:356-357, src/registry.rs:211-212]

## 7. A renamed name makes the skill test fail

**Story:** As the developer maintaining hive, I want `cargo test --test skill` to fail and name the token when the binary renames a flag or receipt name the skill cites, so that a release never ships a skill naming something the binary no longer has.

- **SKILLPASS-7.1** IF a cited name in the body that starts `--` is not a long flag of some subcommand in the built command `cargo_hive::cli::Cli::command()` (after `.build()`, which adds `--help` and `--version`) THEN THE SYSTEM SHALL fail the skill test, naming the span and its body line. [Lock L7; W1]
- **SKILLPASS-7.2** IF the skill test cannot match a cited name in the body against the binary THEN it SHALL fail, naming the span and its body line. Each kind is checked against its source:
  - a subcommand against the subcommands of `Cli::command()`;
  - a bench build shape through `cargo_hive::bench::command::BuildCmd::from_str`;
  - a plan outcome against the serialised variants of `cargo_hive::receipt::Outcome`;
  - a plan class through `cargo_hive::plan_tier::build::class_label`;
  - a receipt envelope key (`hive_version`, `counts`) against the top-level keys of a serialised `cargo_hive::receipt::ReceiptBuilder` receipt;
  - a frontmatter key path (`metadata.hive-version`) against the skill's own frontmatter;
  - a config key (`secondary_roots`, `cache_roots`, `gc_window_days`) by the check in SKILLPASS-8.4;
  - any other span containing `_` or `.` against the string literals in `src/` outside each file's `#[cfg(test)] mod tests`. The span matches a literal it equals. It also matches a literal with a trailing `{…}` or `:{…}` placeholder once that placeholder is dropped (`"sessions_deleted{suffix}"`, `"reused_fresh:{path}"`). It also matches a literal with a leading `{…}` placeholder when that placeholder is filled by another literal (`"{series}_median"` with `"cold_build_ms"`, src/bench/mod.rs:572-590).

  [Lock L7; W1; config-key and placeholder forms from `req-review.md` item 2, src/state.rs:243-249, src/cli.rs:893-906; envelope, frontmatter, leading-placeholder and cfg(test) rules added at design, design.md Decisions 3-4, agent-decided]
A rename that leaves an exact copy of the old literal elsewhere in `src/` passes these checks; the integration tests that assert real receipt names are what catch it (W1 residual).

- **SKILLPASS-7.3** IF a single-word inline code span in the body is none of the kinds in SKILLPASS-7.1 and 7.2, and not a path, a glob, a placeholder or a command of another tool by the skill test's fixed skip rules, THEN THE SYSTEM SHALL fail the skill test, naming the span, rather than passing it unchecked. [W1]

## 8. The README describes each command as the skill does

**Story:** As a person reading the README, I want each hive command described with the same flags and recoveries as the agent skill, so that the README and the skill never disagree on how a command is called.

- **SKILLPASS-8.1** THE SYSTEM SHALL order the README's Quick start by the same user goals as the skill, and keep its Commands section as a per-command reference with one row per command form (`repo add`, `repo list`, `repo remove`, `status`, `gc`, `gc --plan`, `apply`, `explain`, `seed`, `bench`, `doctor`), in an agent-decided order. Each row's code span shall be a real command line, with optional arguments described in the row's text. [W3; order agent-decided, card Delegated; Lock L12]
- **SKILLPASS-8.2** IF a `cargo hive` command line in the README does not parse with `Cli`, or a cited name in the README fails the checks of SKILLPASS-7.1 to 7.3, THEN THE SYSTEM SHALL fail the skill test, naming the README line; it SHALL also fail when a shipped subcommand has no command line in the README. The README pass does not require `--json` and does not forbid `.md` links. [W3; the per-subcommand check holds BENCH-8.3, which no test held, `req-review.md` item 9]
- **SKILLPASS-8.3** IF a README `cargo hive` / `cargo-hive` command line, or a README code span naming a flag of `Cli::command()`, uses a short flag THEN THE SYSTEM SHALL fail the skill test, naming the README line, so the README writes only long hive flags (`--top`, not `-n`). Spans of other tools (rustc's `-C save-temps`, README.md:194) stay under SKILLPASS-7.3's skip rules. [W3, src/cli.rs:91; scope narrowed in requirements-review.md finding 1]
- **SKILLPASS-8.4** IF a config key named in the README's `## Configuration` table (`secondary_roots`, `cache_roots`, `gc_window_days`) is refused by the binary when it is set alone in `config.json` under a scratch `HIVE_HOME` THEN THE SYSTEM SHALL fail an integration test, naming the key. [W3, src/state.rs:243-249,286; mechanism corrected at design: `docs/standards/testing.md` keeps document tests from touching the machine, design.md Decision 6]
- **SKILLPASS-8.5** THE SYSTEM SHALL give the same recovery for `source.none` and `walk.unreadable` in the README as SKILLPASS-3.1 and 3.2 give in the skill. [Lock L3, L12]

## 9. Keep what the skill tests already hold

**Story:** As the developer, I want every check the skill test makes today to keep holding after the rewrite, so that the rewrite cannot loosen the version match, the budget or the parse check.

- **SKILLPASS-9.1** (guard) WHEN the skill tests run THE SYSTEM SHALL CONTINUE TO hold the body to at most 150 lines counted after the closing `---`, including blank lines, and to fail on any local markdown link or `.md` path in the body. [Lock L8; SKILL-6.1]
- **SKILLPASS-9.2** (guard) WHEN the skill tests run THE SYSTEM SHALL CONTINUE TO require that `metadata.hive-version`, `plugin/.claude-plugin/plugin.json` and `plugin/.codex-plugin/plugin.json` equal the crate version, and that the marketplace entry carries no version. [Lock L8; SKILL-4.5]
- **SKILLPASS-9.3** (guard) WHEN the skill tests run THE SYSTEM SHALL CONTINUE TO parse every `cargo hive` / `cargo-hive` command line in the body with the real `Cli`, require `--json` on each apart from `--help` and `--version` lines, require a path argument on the `repo add`, `status`, `explain`, `seed` and `bench` lines and a `--version` line in the body, and fail on a stray mention. [SKILL-4.2, SKILL-4.4, GCPLAN-9.9, SEED-10.13; pins listed in `req-review.md` item 8]
- **SKILLPASS-9.4** (guard) WHEN the skill tests run THE SYSTEM SHALL CONTINUE TO hold the frontmatter to SKILL-4.3's rules, with no `allowed-tools` key, and to keep the banned words `approve`, `needs-human`, `exit 3` and `not available yet` out of the body. [SKILL-4.3, SKILL-6.2, GCPLAN-7.5]
- **SKILLPASS-9.5** (guard) WHEN the skill tests run THE SYSTEM SHALL CONTINUE TO hold `skills/hive/eval.json` to SKILL-5.3's shape, with every `derived_from` naming a `## ` heading in `skills/hive/TESTS.md`, and every `contract` quote occurring in the body verbatim (SKILL-5.4). [SKILL-5.3, SKILL-5.4]
- **SKILLPASS-9.6** (guard) WHEN the skill tests run THE SYSTEM SHALL CONTINUE TO find in the README a `cargo hive seed` command line and the sentence "Bare `cargo hive gc` keeps the store under 40 GB". [STOREGC-8.2; BENCH-8.3's bench row is held by SKILLPASS-8.2]
- **SKILLPASS-9.7** (guard) WHEN `skills/hive/TESTS.md` is rewritten THE SYSTEM SHALL CONTINUE TO keep its recorded no-skill RED and skill-loaded GREEN runs of the free-space request on a Sonnet-class and a Haiku-class model, and its install records. [SKILL-5.1, SKILL-1.3, SKILL-1.4, SKILL-1.8]

## 10. Recorded runs show the rewritten skill is followed

**Story:** As the developer, I want recorded runs on Sonnet and Haiku of each card example with the current skill and with the rewrite, so that I can see the rewrite changed what an agent does where it had to and held where it already worked.

- **SKILLPASS-10.1** THE SYSTEM SHALL keep in `skills/hive/TESTS.md` one run per model (Sonnet via `skill-tester`, Haiku via `skill-tester-haiku`) and per arm (RED with the skill at commit 23defc9, GREEN with the rewrite) for each scenario: E1 free disk across named lanes, E2 three lanes, E3a `source.none`, E3b `walk.unreadable` for a gone main, E3c `store.tag` on a first seed into an unfilled store, free space, why so many copies, version mismatch with an added ask to unregister a gone repo, `gc --plan` then `apply`, and `bench`. Each run has its own heading and quotes the agent's commands verbatim. [Lock L11; W2]
- **SKILLPASS-10.2** THE SYSTEM SHALL also keep, for the free-space scenario, a no-skill RED run on each model, rerun on the new fixture. [SKILL-5.1; W2a]
- **SKILLPASS-10.3** THE SYSTEM SHALL write each scenario's predicted RED outcome into `skills/hive/TESTS.md` before its runs, and mark a scenario whose current-skill RED passes as a regression guard, not as evidence for the rewrite. [W2]
- **SKILLPASS-10.4** WHEN a GREEN run is recorded THE SYSTEM SHALL show in it that the agent ran hive with `--json` and its scope as arguments, quoted counts from the receipt, and ran no `cargo clean` and no deletion inside a target dir or `~/.hive/store/`. For E1-E3c it shall also show the behaviour SKILLPASS-1.1, 2.1, 2.2, 3.1, 3.2 and 3.3 describe, and the `gc --plan` then `apply` run the behaviour of SKILLPASS-11.1. [SKILL-5.2; Lock L4-L6]
- **SKILLPASS-10.5** THE SYSTEM SHALL run the evals only on a scratch fixture under a temporary directory with `HIVE_HOME` set there. The fixture shall hold:
  - linked worktree lanes;
  - a registered repo whose main checkout is deleted;
  - a repo with no built checkout, registered after another repo has filled the store, so its `seed` gives `source.none` rather than `store.tag` (`req-review.md` item 1, src/seed/choose.rs:64-107);
  - a fresh `HIVE_HOME` whose store has never been filled, for E3c;
  - a bench pair;
  - a second feature set (`depcrate` with feature `extra`), so `explain` reports groups.

  [charter Out of scope; Lock L17; W2]
- **SKILLPASS-10.6** THE SYSTEM SHALL record in `skills/hive/TESTS.md` a trigger test of the new description on each model, with 8-10 should-fire and 8-10 should-not-fire queries. The should-not-fire queries shall include a `cargo clean` or disk ask in an unregistered repo, a sccache or linker speed-up question, and `git worktree` housekeeping. [W2; author-skills pressure-testing]
- **SKILLPASS-10.7** THE SYSTEM SHALL add to `skills/hive/eval.json` one `behavior` entry per new scenario, each `derived_from` the scenario's heading, and SHALL update contract entries 6 and 7 so that their quotes match the rewritten body. [SKILL-5.3; W2]
- **SKILLPASS-10.8** IF an instruction is added or changed in meaning (SKILLPASS-3.1, 3.2, 3.3, 6.1) THEN THE SYSTEM SHALL have a recorded RED run with the current skill that fails on it before the rewrite is written. For the `toolchain.missing` half of SKILLPASS-3.3, the build first tries to make that state inside the fixture's temp `HIVE_HOME` (the store's tag present, with no row for the current toolchain: src/seed/choose.rs:64-67, tag at `<HIVE_HOME>/store/CACHEDIR.TAG`, src/seed/store.rs:88-89), and records the attempt in `skills/hive/TESTS.md`. Only if the state cannot be made there is it held by the code reading alone (src/seed/mod.rs:290-302), and TESTS.md says so. [requirements-review.md round 2] A move or a merge that keeps the meaning is held by its scenario's RED and GREEN pair. GREEN shall hold on Haiku. [Lock L10; close-package §2]

## 11. Delete what gc keeps: plan, then apply

**Story:** As an agent asked to remove dead units or an orphan target, I want the skill to tell me to plan and then apply one plan myself, whatever its class, so that I delete through hive's recheck instead of by hand.

- **SKILLPASS-11.1** THE SYSTEM SHALL state in the skill, in one section about deleting what `gc` keeps, that closure deletion and whole orphan targets go through `gc --plan` and then `apply` of one plan id. It shall state that the agent applies plans of every class (`secondary`, `protected`, `orphan`) itself with no owner step, that it names the targets an `orphan` plan removes when it reports, and that after a `refused` outcome it plans again and never reuses an id the binary reported as `already_consumed`. [today's instructions 15, 17-20; `already_consumed` spelling Weigh from code, src/plan_tier/apply.rs:105,125, close-package row 8]

## 12. Quality attributes

**Section-kind:** nfr

**Story:** As a stakeholder, I want measurable quality targets for this feature, so that how-well is not left implicit.

- **Performance:** **SKILLPASS-12.1** THE SYSTEM SHALL keep the skill body at 150 lines or fewer (SKILLPASS-9.1), and SHALL keep `cargo test --test skill` free of agent runs, binary builds beyond the crate's own, and Cargo builds of fixtures, so the whole suite stays about a minute. Verified by the test's runtime in the close gate. (Lock L8 for the budget; the suite time from docs/agents/project.md verify commands; no `docs/product/metrics.md` exists, so no metric ID is cited.)
- **Security:** **SKILLPASS-12.2** THE SYSTEM SHALL ship the skill without an `allowed-tools` key (SKILLPASS-9.4), so the rewrite grants no tool permission. `tests/skill.rs` SHALL touch nothing on the machine. The config-key integration test's `HIVE_HOME` SHALL be a scratch directory under `/private/tmp/hive-it-*`, never `~/.hive`. Verified by tests/skill.rs and the scratch helper in `tests/common`. (No `docs/security/threat-model.md` exists; no TB/THR ID is cited; corrected at design, design.md Decision 6.)
- **Reliability:** None — no runtime component; the skill is text and the checks are tests.
- **Accessibility:** None — the skill is text an agent reads, and the README is plain markdown; no UI.

## Supersedes

When this file is Approved, each superseded clause below is struck through in its own file, inline as `~~clause~~ (superseded by SKILLPASS-N.M)`, and the rest of the criterion stays live. SKILL-1.2 (`docs/specs/2026-10-29-agent-skill/requirements.md:37`) struck a whole criterion; striking a single clause is a new form, chosen because each of these criteria still holds a live guard. [agent-decided]

| Criterion | Clause superseded | By |
|---|---|---|
| SKILL-2.4 (`docs/specs/2026-10-29-agent-skill/requirements.md`) | "exactly the MILE-1 commands … and no command line for any other subcommand" | SKILLPASS-5.2 |
| SKILL-4.2 (`docs/specs/2026-10-29-agent-skill/requirements.md`) | "or names a subcommand outside the set in SKILL-2.4" (the set it points at becomes SKILLPASS-5.2's) | SKILLPASS-5.2 |
| SEED-8.1 (`docs/specs/2026-11-02-store-seed/requirements.md:165`) | "inside its existing headings" | SKILLPASS-5.1 |
| BENCH-8.1 (`docs/specs/2026-11-09-bench/requirements.md:226-227`, the clause wraps a line) | "inside its existing headings" | SKILLPASS-5.1 |
| STOREGC-10.11 (`docs/specs/2026-11-09-store-gc/requirements.md:153`) | "that it keeps its seven fixed headings in order" | SKILLPASS-5.1 |
| BENCH-9.4 (`docs/specs/2026-11-09-bench/requirements.md:266-267`, the clause wraps a line) | "to the seven `##` headings in their order" | SKILLPASS-5.1 |

## Out of Scope

- New commands, flags or CLI behaviour, and any edit under `src/` (Lock L9).
- A version bump, or a change to the plugin distribution or the version-match mechanism (charter; Lock L8 keeps the match). Installed plugin caches keyed on 0.1.0 are not refreshed; the evals paste the worktree skill text. [agent-decided, close-package row 13]
- Releasing v0.1.0 and closing any milestone.
- Running agents against klynt or any checkout outside a temporary fixture.
- Dropping any of today's instructions, or raising the 150-line budget (operator only, card Ask me).
- A reference file beside the skill, or a second skill (SKILL-6.1, SKILL-1.7).
- Running `eval.json` in CI.
- README sections outside Quick start and Commands (Why, Measured results, How it works, Safety guarantees, Configuration) beyond correcting a statement that contradicts the skill.

## Open Questions

- U1: whether every cited `_` or `.` name has a whole literal in `src/` (about 25 checked by hand) — hive-skill-pass-work — answered at the first run of the SKILLPASS-7.2 check over the rewritten body, in the build of this feature — forbid-guess: a name that fails red there gets a typed rule, as config keys did, and is never put on a skip list. The design names where typed rules live.
- Version bump for existing installs: v0.1.0 is published (main bc842b1), and the rewritten skill reaches existing installs only through a version bump, which this road does not make (Out of Scope; Lock L8 keeps the match). Bump at the next release? — operator — at the next release (cut-release) — forbid-guess: Still-open, not agent-decided; no session picks it.
