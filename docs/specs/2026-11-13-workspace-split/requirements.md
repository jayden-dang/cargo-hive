# Requirements: Workspace split

Feature code: SPLIT
Status: Draft
Date: 2026-11-13
Roadmap item: ROAD-15 (docs/roadmap/roads/ROAD-15.md), ticket #26

<!--
Rules:
- Every acceptance criterion gets a hierarchical ID: SPLIT-<story>.<criterion>.
- EARS phrasing; guards use SHALL CONTINUE TO.
- IDs are immutable once Status is Approved. Retire by strikethrough, never renumber.

Labels used below:
- (agent-decided) — a choice ROAD-15 delegates ("where each module goes; which data structures change and
  how, when results stay equal; test layout", ROAD-15.md:23-24) or a reading the operator has not given;
  chosen during the unattended frame-change run, recorded in its ledger, and open to the operator's review.
- (working name — operator approves before publish) — crate names are the operator's ("Ask me",
  ROAD-15.md:25); nothing is published until the operator approves them.
- "the system" is the cargo-hive workspace: its crates, its gate, its tests and its release steps.
- "the gate" is the verify table in docs/agents/project.md (`cargo check --all-targets`, the non-macOS
  `cargo check`, `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`), run over the
  whole workspace.
- "0.1.0" is the cargo-hive 0.1.0 release (tag v0.1.0, crates.io `cargo-hive = "0.1.0"`).
-->

## 1. Change one part, build only what it touches

**Story:** As a contributor, I want to edit how gc decides a dead unit and see only the gc crate and the binary rebuild, so that I build and test only what I touched.

- **SPLIT-1.1** THE SYSTEM SHALL be a Cargo workspace whose members are the library crates
  `cargo-hive-platform`, `cargo-hive-core`, `cargo-hive-state`, a cargo-adapter crate, `cargo-hive-doctor`,
  `cargo-hive-seed` and `cargo-hive-gc`, plus the binary crate `cargo-hive` (working names — operator approves
  before publish; the role set and the adapter's role word are agent-decided, and design may merge or split
  roles).
- **SPLIT-1.2** THE SYSTEM SHALL assign every member to one layer of the order platform → core → state →
  cargo adapter → features (doctor, seed, gc) → binary, and every dependency between members SHALL point from a
  higher layer to a strictly lower one (agent-decided layer order), checked by a test that reads
  `cargo metadata`.
- **SPLIT-1.3** THE SYSTEM SHALL have no dependency between any two feature crates (doctor, seed, gc)
  (agent-decided), checked by the same test.
- **SPLIT-1.4** WHEN a source file of the gc crate changes and `cargo build -v` runs at the workspace root, THE
  SYSTEM SHALL compile the gc crate and `cargo-hive` and SHALL report the seed crate `Fresh`
  (ROAD-15.md:8).
- **SPLIT-1.5** WHEN `cargo test -p <member>` runs for a library member, THE SYSTEM SHALL compile only that
  member, the members below it and their dependencies, and SHALL run that member's unit tests and its own
  integration tests (agent-decided test layout).

## 2. An OS call outside the platform crate fails with the rule's name

**Story:** As an agent, I want an OS call I add to the seed crate to fail the gate with a message naming the rule, so that I learn the boundary without reading the architecture doc.

- **SPLIT-2.1** THE SYSTEM SHALL confine `unsafe` code, `libc` as a normal (non-dev) dependency,
  `cfg(target_os = …)` and `std::process::Command`, each outside test code, to the platform crate; `std::fs` stays allowed in every member
  (agent-decided reach of "touch the OS").
- **SPLIT-2.2** IF a non-platform member contains an `unsafe` block THEN `cargo build` SHALL fail with an error
  whose text contains "only the platform crate may touch the OS" (agent-decided mechanism: a crate-root
  `#![forbid(unsafe_code, reason = …)]`).
- **SPLIT-2.3** IF a non-platform member's code names `std::process::Command` THEN `cargo clippy --all-targets
  -- -D warnings` SHALL fail with an error whose text contains "only the platform crate may touch the OS",
  except inside test code that carries a test-scoped allow (agent-decided mechanism: clippy
  `disallowed-types` / `disallowed-methods` with a `reason`).
- **SPLIT-2.4** IF a non-platform member declares `libc` as a normal dependency, contains `cfg(target_os = …)` outside test
  code, or lacks the crate-root forbid attribute of SPLIT-2.2, THEN `cargo test` SHALL fail with a message that
  contains "only the platform crate may touch the OS" and names the member (agent-decided mechanism: a guard
  test).
- **SPLIT-2.5** THE SYSTEM SHALL spawn every child process through the platform crate's spawn functions, each
  site keeping its 0.1.0 deadline, pipe drain and process-group kill or the lack of them (git's `run` and
  `has_commits`, src/git.rs:115, :176, have no deadline in 0.1.0), and SHALL build the environment of every child process
  (`cargo`, `git`, `rustup`) through one module above the platform crate (agent-decided placement), with each
  child's environment as SPLIT-2.6 keeps it.
- **SPLIT-2.6** (guard) WHEN hive spawns a child process, THE SYSTEM SHALL CONTINUE TO give it the environment
  0.1.0 gives it (ARCH-13): `cargo metadata` without `CARGO_TARGET_DIR`, `CARGO_BUILD_TARGET_DIR`,
  `CARGO_BUILD_BUILD_DIR` and every `GIT_*` variable (src/cargo_target.rs:565-573); `git` without every `GIT_*`
  variable (src/git.rs:70-74); `RUSTUP_TOOLCHAIN` and `RUSTUP_TOOLCHAIN_SOURCE` removed unless the source is
  `cli` or `env` (src/doctor.rs:355-374, applied at src/cargo_target.rs:574); and the other child `cargo` runs
  with the filtering they have in 0.1.0 (src/doctor.rs:748, src/bench/command.rs:113-114, :142).
- **SPLIT-2.7** (guard) IF a filesystem operation fails with an error that 0.1.0 matches by raw errno or by
  `io::ErrorKind`, THEN THE SYSTEM SHALL CONTINUE TO take the branch and print the message 0.1.0 does for it:
  `ENOSPC` / `StorageFull` on directory create and clone (src/seed/place.rs:610, src/seed/fill.rs:509, :750),
  `EXDEV` on clone (src/seed/fill.rs:506), `EEXIST` / `AlreadyExists` and `ENOTEMPTY` / `DirectoryNotEmpty` on
  the store's publish rename (src/seed/store.rs:766-767), and `ENOTDIR` / `NotADirectory` in the directory-listing
  message (src/seed/mod.rs:94-98); any other error SHALL CONTINUE TO take the branch it takes in 0.1.0.

## 3. Installing from the workspace gives the same commands as 0.1.0

**Story:** As a hive user, I want the `cargo-hive` installed from this workspace (`cargo install --path <binary member>`, the README's install-from-a-clone command) to give me the same commands, flags, receipts and exit codes as an installed 0.1.0, so that my scripts and agents keep working once the release ships.

- **SPLIT-3.1** (guard) THE SYSTEM SHALL CONTINUE TO install one binary, `cargo-hive`, whose command tree,
  flags, argument defaults and help text equal 0.1.0's apart from the version number, the hidden `__hold-lock` and `__try-lock` subcommands
  included (src/cli.rs:12-130), checked against a dump of 0.1.0's clap tree.
- **SPLIT-3.2** (guard) WHEN any command runs with `--json` on the same inputs as 0.1.0, THE SYSTEM SHALL
  CONTINUE TO print the same receipt and envelope keys, value types and exit code as 0.1.0 (src/receipt.rs:88-108).
- **SPLIT-3.3** THE SYSTEM SHALL give every member crate and the plugin (`plugin/.claude-plugin/plugin.json`,
  `plugin/.codex-plugin/plugin.json`, `plugin/skills/hive/SKILL.md` `metadata.hive-version`) one shared
  version, set once in `[workspace.package]`.
- **SPLIT-3.4** THE SYSTEM SHALL declare every dependency between members with both `path` and an exact
  `version = "=X.Y.Z"` equal to the shared version, checked by a test, so that `cargo install` without
  `--locked` cannot resolve a member at another version (agent-decided).
- **SPLIT-3.5** WHEN hive writes a receipt, THE SYSTEM SHALL record as `hive_version` the same version that
  `cargo-hive --version` prints.
- **SPLIT-3.6** THE SYSTEM SHALL give no member except `cargo-hive` a binary target, checked by a test, so that
  no `cargo hive-<role>` subcommand is installed (derived from ROAD-15.md:19, "new commands" off limits; the
  test is agent-decided).
- **SPLIT-3.7** (guard) WHEN `cargo hive` runs on a system other than macOS, THE SYSTEM SHALL CONTINUE TO print
  the macOS-only message and exit 2 (src/cli.rs:147-149), and `cargo check --target x86_64-unknown-linux-gnu`
  SHALL CONTINUE TO pass for the whole workspace.
- **SPLIT-3.8** (guard) WHEN a hive command needs a lock holder, THE SYSTEM SHALL CONTINUE TO run the installed
  `cargo-hive` executable with `__hold-lock` / `__try-lock` (src/platform/mod.rs:446, src/platform/apfs.rs:173,
  :239).
- **SPLIT-3.9** WHEN README.md's install-from-a-clone command runs, THE SYSTEM SHALL install the same
  `cargo-hive` binary, with the same commands, flags, receipts and exit codes, as `cargo install cargo-hive`
  of the same version would.

## 4. Liveness and state come out the same as 0.1.0

**Story:** As the maintainer, I want liveness on klynt's main target to find the same live and dead units after the rework as 0.1.0 does, so that I can trust the split and the data-structure rework changed no result.

- **SPLIT-4.1** WHEN the read-only liveness report runs on klynt's main target with `HIVE_HOME` under
  `/private/tmp`, THE SYSTEM SHALL report the same set of live units and the same set of dead units as 0.1.0
  reports when the two run back to back on that target with no build in between and are given the same `now` and
  GC window, and neither run SHALL write anything inside klynt (ROAD-15.md:14;
  which command serves as the report is agent-decided in design).
- **SPLIT-4.2** (guard) WHEN hive opens a state root written by 0.1.0, THE SYSTEM SHALL CONTINUE TO read
  `registry.json`, `snapshot.json`, `capabilities.json` (which also holds the toolchain data), `config.json`,
  `plans.json`, `store.db`, `seeds.json`, `receipts/` and the `store/` unit directory, and use the `.lock` and
  `store.lock` lock files (src/state.rs:156, :188, :208), without refusing, resetting or rewriting any of them.
- **SPLIT-4.3** (guard) WHEN hive writes any state file or prints any `--json` shape from the same inputs as
  0.1.0, THE SYSTEM SHALL CONTINUE TO produce bytes equal to 0.1.0's (field names, order, number and string
  forms) apart from the `hive_version` value and values derived from the clock, checked against output captured
  from 0.1.0.
- **SPLIT-4.4** THE SYSTEM SHALL represent unit indexes and unit hashes as distinct newtypes, a unit's kind as
  an enum, and the fingerprint graph as a compressed adjacency (CSR) with a bitset closure, in place of 0.1.0's
  `usize` indexes, `String` `hash_hex` and `kind`, `Vec<(usize, usize)>` edges and `HashMap<&str, Vec<usize>>`
  index (src/fingerprint.rs:78-120, :520) (agent-decided rework set).
- **SPLIT-4.5** (guard) IF a unit's name stem matches none of 0.1.0's nine kind names, THEN THE SYSTEM SHALL
  CONTINUE TO class it as kind `other` and print `other`, as `kind_of` does (src/fingerprint.rs:146-152); the
  kind enum carries a variant for it (agent-decided).
- **SPLIT-4.6** (guard) WHEN a unit hash is serialized or printed, THE SYSTEM SHALL CONTINUE TO write it as the
  same 16 lowercase hex characters, leading zeros kept (src/fingerprint.rs:82).

## 5. Every crate uses edition 2024

**Story:** As a contributor, I want `cargo metadata` to show edition 2024 for every member while the whole gate stays green, so that new code in any crate follows the current language rules.

- **SPLIT-5.1** THE SYSTEM SHALL build every member with edition 2024 and the workspace with `resolver = "3"`.
- **SPLIT-5.2** (guard) WHEN code that holds a lock guard, a store lock or a temp dir runs, THE SYSTEM SHALL
  CONTINUE TO release it at the same point as 0.1.0 wherever the guard is created in an `if let` scrutinee or a
  block's tail expression, the two places edition 2024 changes drop order; the inspection covers at least
  src/doctor.rs:693, :695, :708, src/plan_tier/locks.rs:61, src/gc/run.rs:173, :318, src/seed/mod.rs:218,
  src/seed/evict.rs:330 and every `State::locked` caller.

## 6. The release steps publish every crate

**Story:** As the maintainer, I want to run the release steps up to the dry-run and see every member package and verify in dependency order, so that the real publish leaves `cargo install cargo-hive` resolving every member at the release's version.

- **SPLIT-6.1** THE SYSTEM SHALL list in docs/agents/project.md release steps every member in an order where
  each member comes after every member it depends on, with each verify build and publish run under
  `CARGO_TARGET_DIR=target/package-verify` (extends FIX-1.1).
- **SPLIT-6.2** WHEN `cargo package --list` runs for a member, THE SYSTEM SHALL list only that member's `src/`
  files, its `Cargo.toml`, a README and the two license files, plus `Cargo.lock` for `cargo-hive` (as 0.1.0's
  package has it, Cargo.toml:8, so `cargo install --locked cargo-hive` keeps a pinned dependency set), and no file
  under `tests/`; a library member's package also carries `Cargo.lock`, which Cargo 1.97.1 adds by default even
  under an `include` list that omits it (probe: .skills/_pending-workspace-split/auto/research-publish-workspace.md).
- **SPLIT-6.3** (guard) WHEN a release step has packaged a member, THE SYSTEM SHALL CONTINUE TO recompile
  that member when a file under its `src/` changes, as `scripts/fresh-probe.sh` checks for every member by
  printing `GREEN` (extends FIX-1.2).
- **SPLIT-6.4** IF any member's crate name has not been approved by the operator, THEN THE SYSTEM's release
  steps SHALL stop before the first `cargo publish`.

## 7. The documented commands work on the workspace

**Story:** As a contributor or agent, I want to run the verify table and the one-crate task gate from docs/agents/project.md at the workspace root exactly as written and see each pass, so that following the docs never fails.

- **SPLIT-7.1** WHEN any command in docs/agents/project.md (verify table, single-test commands, task gate, run
  locally) or docs/agents/verify.md runs at the workspace root as written, THE SYSTEM SHALL run it to success on a
  green tree.
- **SPLIT-7.2** THE SYSTEM SHALL state the SPLIT-2.1 rule in docs/standards/code.md and the test layout of
  SPLIT-1.5 in docs/standards/testing.md.
- ~~**SPLIT-7.3**~~ moved to SPLIT-3.9 (same act as story 3's install).
- **SPLIT-7.4** (guard) WHEN the hive skill or README cites a flag, subcommand, envelope key or config key that no
  member's source defines, THE SYSTEM SHALL CONTINUE TO fail a document test (tests/skill/names.rs:265), now
  sweeping every member's `src/`.
- **SPLIT-7.5** (guard) WHEN any shipped file's version differs from the crate version, THE SYSTEM SHALL CONTINUE
  TO fail the version test (tests/skill.rs:259-261).

## 8. Quality attributes

**Section-kind:** nfr

**Story:** As a stakeholder, I want measurable quality targets for this feature, so that how-well is not left implicit.

- **Performance:** **SPLIT-8.1** WHEN a one-line edit is made to a gc-crate source file, THE SYSTEM SHALL finish
  the following `cargo build` (dev profile, warm) with a median time no greater than the median for the same edit to the same
  code in the 0.1.0 single crate, 5 runs each on the proof machine, recorded with dated evidence (docs/standards/code.md
  "Measured claims") (agent-decided target: ROAD-15 promises building only what an edit touches, not a faster
  build, and no standing product metric or SLO exists — docs/product/metrics.md and docs/ops/ are absent). The
  0.1.0 baseline is captured in the same pre-step-1 session as the SPLIT-4.1 klynt baseline. IF the median is
  greater, THEN the result goes to the operator as an open question with both medians; the target is not
  relaxed silently.
  **SPLIT-8.2** (guard) WHEN `cargo hive status <repo> --no-sizes` runs on the proof machine against klynt, THE
  SYSTEM SHALL CONTINUE TO meet CORE-6.1 as written (median of 5 warm runs and one cold-cache run, each within
  2 s), and `explain` SHALL CONTINUE TO stay within two seconds of `status` (tests/guards.rs:375); both checked by
  `cargo test --test guards -- --ignored` or its moved equivalent.
- **Security:** None — no new trust boundary: no network, credential or permission surface changes, and the OS
  boundary is a code-structure rule covered by story 2. No docs/security/ exists.
- **Reliability:** None as a new target — the reliability promises this road touches are guards: locks released
  where 0.1.0 releases them (SPLIT-5.2), 0.1.0 state read unchanged (SPLIT-4.2), ARCH-13 env hygiene
  (SPLIT-2.6). No docs/ops/reliability.md exists.
- **Accessibility:** None — headless CLI; output is unchanged by SPLIT-3.2.

## Files touched and their guards

Every file under `src/` and `tests/` moves into a member crate (ROAD-15 surfaces, docs/roadmap/INDEX.md:141).

| Files | Existing behavior at risk | Guard |
|---|---|---|
| src/cli.rs, src/main.rs, src/lib.rs | command tree, flags, exit codes, hidden lock subcommands, macOS-only exit | SPLIT-3.1, 3.2, 3.7, 3.8 |
| src/receipt.rs | receipt fields, `hive_version` | SPLIT-3.2, 3.5, 4.3 |
| src/platform/mod.rs, src/platform/apfs.rs | Fs behaviour, lock self-exec, non-macOS stub | SPLIT-3.7, 3.8, 5.2 |
| src/state.rs, src/registry.rs | state root, the one lock, state files | SPLIT-4.2, 4.3, 5.2 |
| src/fingerprint.rs, src/liveness.rs, src/classify.rs, src/size.rs | fingerprint format, unit graph, liveness verdicts, hash print form | SPLIT-4.1, 4.3, 4.5, 4.6 |
| src/cargo_target.rs, src/git.rs, src/discover.rs | child env hygiene, target-dir identification, snapshot | SPLIT-2.6, 4.2, 4.3 |
| src/doctor.rs | capability checks, signal cancel, stale temp sweep | SPLIT-3.2, 4.2, 5.2 |
| src/explain.rs | explain output and its non-macOS fallback | SPLIT-3.2, 3.7 |
| src/gc.rs, src/gc/*.rs, src/plan_tier/*.rs | auto tier, plans, apply outcomes, locks | SPLIT-3.2, 4.1, 4.3, 5.2 |
| src/seed/*.rs | store, fill, place, evict, errno branches | SPLIT-2.7, 3.2, 4.2, 4.3, 5.2 |
| src/bench/*.rs | bench receipt | SPLIT-3.2 |
| tests/*.rs, tests/common/*.rs | the suite's assertions | every existing assertion kept unweakened (Out of Scope: no test is relaxed) |
| tests/skill.rs, tests/skill/names.rs | version and document tests | SPLIT-7.4, 7.5 |
| tests/liveness.rs (`the_package_ships_no_test_file`) | package holds no test file; today it parses the root `Cargo.toml` `include` list (tests/liveness.rs:279-286), so it is rewritten per member | SPLIT-6.2 |
| tests/guards.rs (liveness spawns no process via `include_str!("../src/liveness.rs")`, :437; CORE-6.1 and explain timing) | no process in liveness (a hard-coded path that moves); status and explain timing | SPLIT-2.1, 8.2 |
| Cargo.toml, Cargo.lock | one package's manifest, `include` | SPLIT-1.1, 3.3, 6.2 |
| scripts/fresh-probe.sh | FIX-1.2 probe | SPLIT-6.3 |
| docs/agents/project.md, docs/agents/verify.md, docs/standards/code.md, docs/standards/testing.md, README.md | documented commands | SPLIT-3.9, 7.1, 7.2 |
| plugin/.claude-plugin/plugin.json, plugin/.codex-plugin/plugin.json, plugin/skills/hive/SKILL.md | version | SPLIT-3.3, 7.5 |
| .claude-plugin/marketplace.json | no behavior to guard (no version field; only read by tests/skill.rs) | — |

## Out of Scope

- New commands, flags, receipt fields or behaviour of any kind (ROAD-15.md:19).
- Linux support; the non-macOS build stays a compile-only stub (ROAD-15.md:18-19).
- Any change to a state file's layout, name or meaning (ROAD-14, issue #13).
- Publishing to crates.io in this run; the release steps are written, not run past the dry-run.
- Keeping 0.1.0's `cargo_hive::<module>` Rust paths as an API: `cargo-hive` keeps a library target only for
  what its own tests need (agent-decided; ROAD-15.md:17 lists the contract, and no doc names a Rust API).
- Edits to docs/architecture/INDEX.md (ARCH-15, ARCH-16 wording) and docs/product/vision.md; proposed text goes
  to the operator's `/define-project`.
- A build script in any member; any new third-party dependency.
- Relaxing, deleting or rewording an existing test's assertion to make the split pass.
- Speed work beyond SPLIT-4.4's rework set.

## Open Questions

- Crate names (SPLIT-1.1 working names) — operator — before the first publish — forbid-guess.
- ~~How a member's `cargo publish --dry-run` verifies while the members it depends on are not yet on crates.io~~
  — resolved in design (Decision 8): `cargo publish --workspace --dry-run` verifies every member against a
  temporary local registry and uploads in dependency order (probe, research-publish-workspace.md).
