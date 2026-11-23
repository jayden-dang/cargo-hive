# Tasks: Warm slot pool

Feature code: SLOTS
Status: Draft
Date: 2026-11-23
Execution-mode: unset
Requirements: ./requirements.md
Design: ./design.md

**Goal:** Let a coding agent acquire and return a stable worktree without losing unfinished work or passing stale output to the next holder.

**Architecture:** The CLI persists scoped, replay-safe lease transitions through the state module. An exclusive guide attempt owns Git and mtime actions, and hive verifies the profile, checkout identities and scoped GC before delivering readiness.

**Tech Stack:** Rust 2024 workspace, existing clap and serde, macOS platform primitives, local Git, stock Cargo, shell agent guide, and existing Rust and subprocess test harnesses. No new runtime dependency.

## Global Constraints

- Engineering standards: `docs/standards/` at Git tree `bebbc3bcd899525d5e282e8eb5b232cf09c866f4`. Architecture invariants: `docs/architecture/INDEX.md` at blob `9ff98627d8a8a3af6546b6e3a325893dc33a6c1a`.
- Gate and workspace dependency order: `docs/agents/project.md` at blob `6d9bfd95c5a035e330924906eb30b646eaca1ab8`. CLI acceptance and evidence conventions: `docs/agents/verify.md` at blob `28aac8f11b42104ddd452dd82173527d5a355c91`. No approved optional codebase-map or ownership docs were present. Team band is Solo.
- Run future proof with isolated fixture repos, worktrees, targets, HIVE_HOME and writable Cargo caches in task scratch. Existing project testing exceptions apply only to the cases they name. For the real-workspace comparison, the operator supplies a supported profile and read-only source/commit inputs with no default path. Measure on a separate disposable copy and leave the owner checkout, config, live Hive state and real ~/.cargo untouched. Run future Cargo gates with scratch `CARGO_HOME` and `CARGO_TARGET_DIR` supplied
  outside the gate script, which provisions only scratch Hive state. Keep the
  fixture's Cargo roots separately scoped and scrub the outer target overrides
  from child fixture invocations. Use the installed `1.97.1` and `1.100.0`
  toolchains and the documented whole gate at implementation close, with no
  toolchain installation as a side effect. No requirement IDs in test source or test titles.
- The Draft approves no execution. Every checkbox below describes future work. The current branch keeps existing state/CLI compatibility, owner-reserved source actions, and the guide's same-account trust boundary.

## File Structure

| File | Responsibility |
|---|---|
| `crates/state/src/lib.rs` | Extend bounded private state writes and named recovery without changing old readers. |
| `crates/state/src/slots.rs` | New strict layout 2 record, checked revisions, transition/outcome I/O with fault tests. |
| `crates/platform/src/flock.rs` | Checked borrowed descriptor lock without explicit unlock while the guide keeps its reference. |
| `crates/platform/src/lib.rs` | Export slot-only platform support. |
| `crates/platform/src/slot.rs` | New local process instance, boot session and entropy helpers. |
| `crates/platform/tests/slot_fence.rs` | New real macOS multi-process fence and child-lifetime proof. |
| `crates/cargo/src/git.rs` | Slot-only bounded complete read-only Git identity, worktree and cleanliness facts. |
| `crates/cargo-hive/src/lib.rs` | Declare the new private slot module beside CLI and bench. |
| `crates/cargo-hive/src/cli.rs` | Add slot dispatch and extract typed GC and seed paths from standalone emission. |
| `crates/cargo-hive/tests/cli_tree.rs` | Freeze existing command help and allow only the intentional additive slot branch. |
| `crates/cargo-hive/tests/fixtures/current/cli-tree.txt` | Update only the current command tree golden for the new slot branch. |
| `crates/cargo-hive/src/slot/mod.rs` | New slot dispatch, identity-scoped transition/replay and operator recovery. |
| `crates/cargo-hive/src/slot/inspect.rs` | New positive slot identity, dirty-return, and candidate root checks. |
| `crates/cargo-hive/src/slot/profile.rs` | New effective producer, certified graph/consumer and mtime evidence checks. |
| `crates/cargo-hive/src/slot/result.rs` | New sanitized JSON projection and existing Receipt audit projection. |
| `crates/gc/src/checkout.rs` | Expose positive root-lock/completion evidence beside the existing report. |
| `crates/cargo-hive/tests/slots.rs` | New real CLI slot, persistence, scope, recovery and fault interleavings. |
| `crates/cargo-hive/tests/slot_inputs.rs` | New generated throwaway fixtures for causal Cargo and build reuse proofs. |
| `crates/cargo-hive/tests/checkout_gc.rs` | Guard existing scoped GC output and add the stricter slot-run root evidence cases. |
| `crates/cargo-hive/tests/upgrade.rs` | Verify new slot records and appended existing-format audit data coexist with old fixtures. |
| `crates/cargo-hive/tests/skill.rs` | Keep shipped command/example cross-checks and exercise new guide journey. |
| `crates/cargo-hive/tests/skill/names.rs` | Add checked slot command and result identifiers to existing guide assertions. |
| `plugin/skills/hive/SKILL.md` | Document slot use, refused return, held recovery and scoped seeded fallback. |
| `plugin/skills/hive/scripts/slot-handoff.sh` | New guide-owned fenced Git/mtime/foreground Cargo procedure. |
| `README.md` | Document optional pool use beside the existing workflow. |
| `docs/specs/2026-11-23-warm-slot-pool/verification.md` | New implementation-time runbook and proof procedure for real pair measurements. |

Retrieval at the file-map boundary was advisory. It found OWNS paths for 15
of 16 registered codes and no active observations. The SLOTS cluster had only
its focus before its task Files blocks existed. Blast-radius paths
`crates/gc/src/checkout.rs` and `crates/cargo-hive/src/cli.rs` overlap
CHECKOUTGC, and `crates/state/src/lib.rs` overlaps UPGRADE. Each existing
reader keeps its contract.

---

### Task 1: Prove the platform fence before granting any writer permission

**Files:**
- Modify: `crates/platform/src/flock.rs`
- Modify: `crates/platform/src/lib.rs`
- Create: `crates/platform/src/slot.rs`
- Create: `crates/platform/tests/slot_fence.rs`

**Reuse:** rung 2 — `State::locked_or_cancelled` and bounded atomic state writes, platform flock and no-follow identity. Add slot-specific durability reconciliation and inherited descriptor handling.

**Interfaces:**
- Consumes: existing platform flock conventions, pinned libc and no-follow file identity.
- Produces (proposed): `platform::flock::try_lock_inherited(fd: BorrowedFd<'_>, expected: LockFileIdentity) -> io::Result<InheritedLock>`. The result is `Acquired` or `Held`; errors refuse, and no guard drops with an explicit unlock.
- Produces (proposed): `platform::slot::parent_instance() -> io::Result<ProcessInstance>` (boot session, PID and start identity) and `platform::slot::random_slot_bytes() -> io::Result<[u8; 32]>`. This task uses only the platform portion of the design's reuse line.

**Depends-on:** none.

- [ ] Write a real macOS subprocess fixture for an inherited open description, a separate open of the same inode, a paused runner, and a killed parent with a live child that can still write. Test both inherited and intentionally stripped descriptors, with uncertain custody in the latter case.
- [ ] Run `cargo test -p cargo-hive-platform --test slot_fence`. Observe absence of the new fence before relying on it.
- [ ] Implement nonblocking borrowed-lock ownership, close without explicit unlock, no-follow identity and bounded process/entropy operations. A missed child or unproved process identity must remain uncertain.
- [ ] Re-run the subprocess fixture and the platform tests. Show that separate opens cannot govern writes while an old description holds the guard, and that a surviving or uncertain child cannot justify takeover. Block dependent writes if the mechanism fails on the admitted macOS platform.
- [ ] Commit the proved platform primitive as `feat(platform): fence warm slot writer processes`.

_Requirements: SLOTS-5.4_

---

### Task 2: Establish two private, unoccupied slots

**Files:**
- Modify: `crates/state/src/lib.rs`
- Create: `crates/state/src/slots.rs`
- Modify: `crates/cargo-hive/src/lib.rs`
- Modify: `crates/cargo-hive/src/cli.rs`
- Modify: `crates/cargo-hive/tests/cli_tree.rs`
- Modify: `crates/cargo-hive/tests/fixtures/current/cli-tree.txt`
- Modify: `crates/cargo-hive/tests/skill.rs`
- Create: `crates/cargo-hive/src/slot/mod.rs`
- Create: `crates/cargo-hive/src/slot/result.rs`
- Create: `crates/cargo-hive/tests/slots.rs`
- Modify: `crates/cargo-hive/tests/upgrade.rs`

**Reuse:** rung 2 — `State::locked_or_cancelled` and bounded atomic state writes, platform flock and no-follow identity. Add slot-specific durability reconciliation and inherited descriptor handling.

**Interfaces:**
- Consumes: registered repo identity, existing `State`/`Tx` file I/O and receipt representation, and Task 1 platform identity/randomness.
- Produces (proposed): `SlotsFile`, `PoolProfile` and typed records in `state::slots`. `Tx::read_slots() -> Result<Option<SlotsFile>, StateError>` and `Tx::commit_slots(previous: Option<u64>, next: &SlotsFile) -> Result<(), StateError>` supply bounded read and durable checked publish.
- Produces (proposed): `Command::Slot { command: SlotCommand }` and `slot::run(state: &State, command: SlotCommand) -> i32` via private crate-root `mod slot`. `SlotResult::audit_receipt(&self) -> Receipt` provides a sanitized existing-format audit event separately from slot stdout.
- Scope: layout 2, profile schema 1, max file size 1,048,576 bytes. Define the final additive command parser tree here. An operation whose later task has not wired a validated dispatch remains non-mutating and unavailable.

**Depends-on:** Task 1.

- [ ] Write CLI and state fixtures for exactly two stable unused paths after restart, private metadata and temporary-file modes, symlinks and unsafe pre-existing state, malformed/newer layouts, duplicate/reused identities, wrong schema and the exact file cap plus one byte.
- [ ] Add cases for an absent fresh record versus old lock/path evidence with a missing record, and for old state files and old-format audit lines remaining readable.
- [ ] Run `cargo test -p cargo-hive --test slots --test upgrade --test cli_tree --test skill` and `cargo test -p cargo-hive-state`. Observe missing behavior and keep prior compatibility tests present.
- [ ] Add strict bounded layout parsing, owner-only no-follow writes, initialization and named fail-closed recovery. Keep orphans held. Parse the added Slot case in the skill harness and scope the historical CLI comparison to the additive slot branch. Regenerate only the current fixture via the existing `HIVE_WRITE_CLI_TREE` seam. Preserve the archived fixture and old-state readers.
- [ ] Repeat the tests. Inspect private file modes, both allocated names and the persisted result after a separate process opens state.
- [ ] Commit as `feat(slot): initialize bounded private pool state`.

_Requirements: SLOTS-1.1, SLOTS-4.6, SLOTS-4.7, SLOTS-4.8, SLOTS-4.9_

---

### Task 3: Reserve one slot through competing or retried requests

**Files:**
- Modify: `crates/state/src/slots.rs`
- Modify: `crates/cargo-hive/src/slot/mod.rs`
- Modify: `crates/cargo-hive/src/slot/result.rs`
- Modify: `crates/cargo-hive/tests/slots.rs`

**Reuse:** rung 2 — reuse clap and existing JSON receipt conventions. Keep the slot result projection independent of the existing emitter.

**Interfaces:**
- Consumes: Task 2 private pool state, normalized pool/slot/generation key, typed prior outcomes; Task 1 private OS entropy.
- Produces (proposed): `SlotRequestKey` (pool, slot, generation, optional attempt, kind and operation ID), `SlotOperationInput` (private credential channel), and typed `SlotOutcome` (current or historical).
- Produces: `slot key`, `reserve`, `outcome` and `ack` on Task 2's parser. Ready dispatch stays unavailable pending the Task 7 proof.

**Depends-on:** Task 2.

- [ ] Test competing subprocesses for the sole available slot, full pool, a reused operation ID with conflicting parameters, and exhaustion at the 32-outcome cap and the 33rd unacknowledged operation.
- [ ] Add tests for an old reserve against a replacement pool with the same repo, slot number and initial generation, and for no credential disclosure in argv, stdout result, diagnostics, status or audit. Use the private credential descriptor to deliver the key.
- [ ] Inject state failures before rename, after rename before directory sync, and after durable commit with lost response. In a new process, read the authoritative generation and replay the identical reserve. Assert no second reserve or rollback. Inject an audit append failure after commit and prove it neither releases nor repeats allocation.
- [ ] Run `cargo test -p cargo-hive --test slots` and `cargo test -p cargo-hive-state`. Observe missing allocation/reconciliation and duplicate protection.
- [ ] Add current namespace/generation checks, one reservation transaction, replay lookup and ack compaction. Re-persist or sync a matching visible revision before acknowledging it.
- [ ] Repeat both suites and re-read the authoritative slot record after each failure. Commit as `feat(slot): reserve and replay one scoped lease`.

_Requirements: SLOTS-1.3, SLOTS-1.4, SLOTS-4.2, SLOTS-4.3, SLOTS-4.8, SLOTS-5.3, SLOTS-5.4_

---

### Task 4: Refuse a dirty or uncertain return without checkout effects

**Files:**
- Modify: `crates/cargo/src/git.rs`
- Create: `crates/cargo-hive/src/slot/inspect.rs`
- Modify: `crates/cargo-hive/src/slot/mod.rs`
- Modify: `crates/state/src/slots.rs`
- Modify: `crates/cargo-hive/tests/slots.rs`

**Reuse:** rung 2 — extend `Git`'s read-only invocations, `discover::checkouts`, C1 classification and platform no-follow inspection.

**Interfaces:**
- Consumes: Task 3 current scope/lease/operation checks and the existing read-only Git settings, C1 and no-follow readers.
- Produces (proposed): `Git::worktrees_complete(&self, max: usize) -> Result<Vec<WorktreeRecord>, GitError>`, through a slot-only cap+1 reader. Existing `Git::worktrees` is intentionally unchanged.
- Produces (proposed): `slot::inspect::inspect_checkout(scope: &SlotScope, expected: &ExpectedCheckout) -> Result<CheckoutProof, SlotRefusal>` with checked path/HEAD and clean/dirty/uncertain inventory. `slot return` consumes those facts. Start with a valid leased fixture here, and verify the normal full journey in Task 7.

**Depends-on:** Task 3.

- [ ] Write subprocess cases for staged and unstaged tracked edits, unmerged state, ignored/untracked `.env` and `.sqlx`, disguised material in retained roots and active writers. Assert bytes, index, mtimes, outputs and original hold are unchanged on refusal.
- [ ] Exercise main, owner lane, replacement inode, symlink and overlapping roots. Prove the slot reader accepts complete NUL records at the exact output cap and refuses a plausible complete-looking retained prefix at cap+1, missing terminal NUL and incomplete owner inventories.
- [ ] Check deliberate resolution, clean return, lost reply and identical replay from a new process. Re-read the retired generation and confirm old discovery/classification behavior.
- [ ] Run `cargo test -p cargo-hive --test slots` and `cargo test -p cargo-hive-cargo`. Observe missing slot-only inspection and clean-return checks.
- [ ] Implement the checked, read-only preflight and transition. Confine root exceptions to positively verified generated roots and perform no cleanup on refusal.
- [ ] Repeat the tests and inspect refused checkout and state. Commit as `feat(slot): preserve changes on refused returns`.

_Requirements: SLOTS-1.5, SLOTS-1.9, SLOTS-2.1, SLOTS-2.2, SLOTS-2.3, SLOTS-2.4, SLOTS-2.5_

---

### Task 5: Admit only proved producer history for a guarded checkout

**Files:**
- Create: `crates/cargo-hive/src/slot/profile.rs`
- Modify: `crates/cargo-hive/src/slot/mod.rs`
- Modify: `crates/state/src/slots.rs`
- Modify: `crates/cargo-hive/tests/slots.rs`

**Reuse:** rung 2 — use the hive guide's command workflow, existing Git facts, Cargo metadata and fingerprint readers. The new eligibility check uses these readers without changing normal Cargo execution.

**Interfaces:**
- Consumes: Task 1 physical writer fence, Task 2 profile schema and Task 4 verified checkout/root identity.
- Produces (proposed): `profile::check_invocation(profile: &PoolProfile, invocation: &InvocationInput, checkout: &CheckoutProof) -> Result<InvocationEvidence, SlotRefusal>`, and `profile::eligible_producer(profile: &PoolProfile, record: &ProducerRecord, checkout: &CheckoutProof) -> Result<ProducerProof, SlotRefusal>`.
- Produces: `slot producer` start/finish transitions keyed by the current lease and guarded invocation. `SeedEvidence` represents positively empty first roots plus typed initial registry seed completion for Task 7.

**Depends-on:** Task 4.

- [ ] Write table-driven CLI and state cases for wrong profile revision, uncovered input/consumer/anchor, unexpected actual root/layout/argv or env, missing graph input, and changed fixed manifests or configuration.
- [ ] Reproduce pending start with lost completion, tainted/unknown roots followed by a successful admitted build, and an old generation trying to record a producer. None may regain eligible status.
- [ ] Exercise producer start and finish through real CLI subprocesses. Denial must precede any writer. A covered foreground exit permits completion; losing a parent while its child can write does not. Exercise inherited and stripped descriptor custody. Only proof of empty roots or a typed registry-only result admits initial evidence.
- [ ] Run `cargo test -p cargo-hive --test slots` and `cargo test -p cargo-hive-state` for the red state and command assertions.
- [ ] Implement bounded profile coverage and producer transitions, bind completion to fenced foreground child exit, and keep taint sticky until output is isolated on a distinct checkout.
- [ ] Re-run tests and commit as `feat(slot): bind retained output to verified producers`.

_Requirements: SLOTS-3.2, SLOTS-3.3, SLOTS-3.11, SLOTS-5.4_

---

### Task 6: Prove exactly which roots checkout GC completed

**Files:**
- Modify: `crates/cargo-hive/src/cli.rs`
- Modify: `crates/gc/src/checkout.rs`
- Modify: `crates/cargo-hive/tests/checkout_gc.rs`

**Reuse:** rung 2 — reuse `metadata::resolve`, target ownership, and `gc::checkout::run` through the existing checkout-GC driver after extracting its typed operation result from emission. Continue to use shipped `seed`.

**Interfaces:**
- Consumes: existing CLI/GC operation, both independently resolved output-root identities and scoped ownership facts from the design.
- Consumes (existing): `checkout::run(input: &CheckoutInput<'_>, cancel: &dyn Fn() -> bool) -> CheckoutReport`, and CLI `checkout_cmd(state: &State, checkout: &Path, out: &OutputArgs) -> i32`.
- Produces (proposed): `checkout::run_with_evidence(input: &CheckoutInput<'_>, expected: &[RequiredRoot], cancel: &dyn Fn() -> bool) -> EvidencedCheckoutReport`, with root path, role and device/inode proof; `cli::checkout_gc_for_slot(state: &State, proof: &CheckoutProof, cancel: &dyn Fn() -> bool) -> Result<SlotGcReport, SlotRefusal>` brings in checkout/target scan faults. Exercise crate-private orchestration with in-module tests and preserve the existing public standalone seam.

**Depends-on:** Task 4.

- [ ] Add failing tests for missing expected per-root evidence, row present but skipped, shared/outside/nested roots, held lock, scan fault, unsupported/undecided root and cancellation. Include replaced inode, omitted role, coincident versus split target/build roots, both GC undecided findings and proved initial absence.
- [ ] Run `cargo test -p cargo-hive --test checkout_gc` and observe the missing positive-verdict seam. Keep the standalone receipt and exit tests as guards.
- [ ] Extract the typed runner from emission. Refuse strict discovery/ownership faults before GC effects. Check actual root identity under the acquired Cargo lock before traversal/deletion. Return typed completion for each expected root and carry incomplete verdicts without changing standalone snapshots, emission or exits.
- [ ] Re-run the scoped integration tests and `cargo test -p cargo-hive-gc`. Compare the unchanged standalone cases. Assert no mutation of skipped, unsafe or out-of-scope roots. A valid earlier scoped effect may persist when a later root fails; the strict verdict must be incomplete, with no readiness eligibility.
- [ ] Commit as `refactor(gc): expose positive scoped cleanup evidence`.

_Requirements: SLOTS-3.6, SLOTS-3.7, SLOTS-3.10_

---

### Task 7: Deliver a ready slot at the requested commit

**Files:**
- Modify: `crates/cargo-hive/src/cli.rs`
- Modify: `crates/cargo-hive/tests/skill.rs`
- Modify: `crates/cargo-hive/src/slot/mod.rs`
- Modify: `crates/cargo-hive/src/slot/inspect.rs`
- Modify: `crates/cargo-hive/src/slot/profile.rs`
- Modify: `crates/state/src/slots.rs`
- Create: `plugin/skills/hive/scripts/slot-handoff.sh`
- Modify: `crates/cargo-hive/tests/slots.rs`
- Create: `crates/cargo-hive/tests/slot_inputs.rs`

**Reuse:** rung 2 — use the hive guide's command workflow, existing Git facts, Cargo metadata and fingerprint readers. The new eligibility check uses these readers without changing normal Cargo execution.

**Interfaces:**
- Consumes: Task 1 physical fence, Task 5 verified producer/anchor evidence, Task 6 typed checkout GC and prior witnessed clean HEAD.
- Consumes (existing): `cargo_hive_seed::run` and its `Result<cargo_hive_seed::Outcome, cargo_hive_seed::Error>`, before exit/receipt projection.
- Produces (proposed): `cli::initial_seed_for_slot(state: &State, proof: &CheckoutProof, cancel: &dyn Fn() -> bool) -> Result<SeedEvidence, SlotRefusal>`, binding typed seed outcome and verified first-root proof to the current guard.
- Produces: `slot begin`, `step-check` and `ready` keyed by `SlotRequestKey`. The guide holds the physical fence, directly runs Git, mtime and foreground Cargo steps and passes its private runner nonce and evidence under the guard.

**Depends-on:** Task 5, Task 6.

- [ ] Test full reserve to first ready on proved empty roots and under optional typed registry-only seed from that same new empty-root fence. Inconclusive/no-op, partial or cancelled seed cannot attest seed-only output. Guard ordinary seed's contract. Test the actual guide's foreground producer lifecycle and child custody under the inherited guard.
- [ ] Generate isolated Cargo fixtures for each admitted layout. For restored `.sqlx` including original mtime, and a newly created then removed `.env` read by a previously absent lookup, establish changed output and an unprotected warm result that really remains stale.
- [ ] Test handoff against the independent clean reference. Require the affected consumer to rebuild or rerun and an unrelated workspace unit to stay fresh. Separately change one crate at the next commit and check the stable absolute path and unrelated unit reuse. Treat unsupported or noncausal controls as failures for that profile.
- [ ] Add the next-commit, lost permit, lost ready reply and post-rename ready fault cases. Include prior HEAD mismatch, wrong runner, concurrent ready, late inventory/GC skips and interrupted effects. Show actual scoped deletion completes before good ready. A zero standalone exit with an advisory skip must still give incomplete slot handoff.
- [ ] Run `cargo test -p cargo-hive --test slots --test slot_inputs --test skill` under installed `RUSTUP_TOOLCHAIN=1.97.1` (flat) and `RUSTUP_TOOLCHAIN=1.100.0` (unit-dir). Use existing `tests/common` scratch helpers. Capture independent fixture values and build events; do not enable a profile without causal lock and Cargo proofs.
- [ ] Implement fenced stages, non-forcing Git/mtime, initial seed/empty verification, typed GC and durable ready. Add the precise helper inventory to the existing skill package guard in this same task. Re-run both layouts, the CLI skill test, the clean/stale/restored controls and state re-read in a new process; verify identical ready replay without a second effect.
- [ ] Commit as `feat(slot): deliver verified warm handoffs`.

_Requirements: SLOTS-1.2, SLOTS-1.6, SLOTS-1.7, SLOTS-2.4, SLOTS-3.1, SLOTS-3.4, SLOTS-3.5, SLOTS-3.6, SLOTS-3.7, SLOTS-3.8, SLOTS-3.9, SLOTS-3.11, SLOTS-4.3, SLOTS-5.4_

---

### Task 8: Recover an abandoned slot without replaying old authority

**Files:**
- Modify: `crates/cargo-hive/src/slot/mod.rs`
- Modify: `crates/state/src/slots.rs`
- Modify: `crates/cargo-hive/tests/slots.rs`

**Reuse:** rung 2 — `State::locked_or_cancelled` and bounded atomic state writes, platform flock and no-follow identity. Add slot-specific durability reconciliation and inherited descriptor handling.

**Interfaces:**
- Consumes: Task 7 active preparation/ready states and lease results, Task 1 process evidence and Task 4 checkout cleanliness and ownership checks.
- Produces (proposed): `RecoveryInput` naming the current `SlotRequestKey` and explicit quiescence evidence, and `RecoveryOutcome` (held, or eligible for new fenced preparation). `slot recover` and historical result lookup share credentialed scope checks. Unresolved path or producer evidence is never made ready.

**Depends-on:** Task 7.

- [ ] Write tests for occupied locks and a live/paused/surviving prior writer, lost key, old process identity, wrong pool and stale generation. Refuse actionable discovery while uncertainty remains.
- [ ] Test a lost ready response followed by clean return and a later lease. Historical lookup cannot return an actionable path or permit. Exercise explicit ack, replay after compaction, prepare/ready reconciliation with malformed or unreadable state and authoritative re-read by a new process.
- [ ] Interrupt a GC run after a partial effect and before ready persists. Require a new valid fence and fresh scoped GC proof. Exercise proven foreground completion or a verified later boot after lost custody, plus a positive abandoned-slot recovery through ordinary ready. A state rollback or orphan record lacking the exact epoch must preserve work and producer evidence while refusing.
- [ ] Run `cargo test -p cargo-hive --test slots` for the new red cases.
- [ ] Implement quiescence-qualified recovery and the state-preserving unknown/tainted hold. The operator resolves work outside the command, and the command never forces, deletes, archives or stashes it.
- [ ] Re-run tests and inspect held/ready/status under faults. Pass one known slot key through normal, refused and interrupted begin/producer/ready/return/recovery calls. Check argv, environment, public stdout/status, stderr and existing-schema on-disk audit for no disclosure, keeping private file-descriptor delivery separate. Commit as `feat(slot): recover held slots through a new fence`.

_Requirements: SLOTS-4.1, SLOTS-4.2, SLOTS-4.3, SLOTS-4.4, SLOTS-4.5, SLOTS-4.6, SLOTS-4.9, SLOTS-5.3, SLOTS-5.4_

---

### Task 9: Guide take, refused return and a distinct seeded fallback

**Files:**
- Modify: `plugin/skills/hive/SKILL.md`
- Modify: `plugin/skills/hive/scripts/slot-handoff.sh`
- Modify: `README.md`
- Modify: `crates/cargo-hive/tests/skill.rs`
- Modify: `crates/cargo-hive/tests/skill/names.rs`
- Modify: `crates/cargo-hive/tests/slots.rs`

**Reuse:** rung 2 — extend `plugin/skills/hive/SKILL.md` and reuse the scorecard's matched controls and existing guide/CLI cross-checks.

**Interfaces:**
- Consumes: Task 7 guide and slot results and Task 8 retry/recovery actions, plus the existing `seed` command.
- Produces: installed take/use/return guidance, one independent seeded fallback and guard tests for old and new examples.

**Depends-on:** Task 8.

- [ ] Write the guide/CLI parser checks and run a real full-pool and rejected-profile journey, including an attempted fallback alias of a tainted slot. Prove the actual eligible fallback is a distinct independently seeded secondary with separate roots and only registry units, with no copied workspace-unit output.
- [ ] Run `cargo test -p cargo-hive --test skill --test slots` to observe the absent guide and fallback behavior.
- [ ] Document setup and the ordinary reserve/ready/return path, handling of lost acknowledgements, held and unsupported results, and human resolution of unfinished work. Keep internal checkpoint flags out of the usual examples. A seed refusal is explicit.
- [ ] Re-run the tests and walk an actual normal, full and refused journey in scratch using the installed guide. Preserve ordinary seed and checkout GC examples.
- [ ] Commit as `docs(hive): teach scoped slot use and fallbacks`.

_Requirements: SLOTS-1.4, SLOTS-1.7, SLOTS-1.8, SLOTS-2.6, SLOTS-3.10, SLOTS-3.12_

---

### Task 10: Measure a real repeatable warm handoff against seeded work

**Files:**
- Create: `docs/specs/2026-11-23-warm-slot-pool/verification.md`

**Reuse:** rung 2 — extend `plugin/skills/hive/SKILL.md` and reuse the scorecard's matched controls and existing guide/CLI cross-checks.

**Interfaces:**
- Consumes: Task 9 normal guide and seed paths, the release build's real scoped GC, and an operator-supplied read-only workspace/commit source and approved profile, replicated into an isolated proof area with no default source path.
- Produces: reproducible runbook and future dated raw paired results, receipt paths, environment details and explicit pass/fail for the two Draft performance thresholds. Reuse the guide as a caller without modifying it here.

**Depends-on:** Task 9.

- [ ] Write the manual acceptance steps in the runbook. Require five valid matched pairs, alternating arm order, a fixed first command and edit, and successive warm handoffs. Define missing actual GC, non-eligible seed, mismatched fixtures and failed/void samples before measuring.
- [ ] Drive the shipped entry points on a supported real-workspace fixture copy. Use release-mode hive and include distinct fresh worktree plus seed in the baseline. Time every warm reservation and handoff step, not just Cargo. Preserve sample receipts and integer milliseconds outside the hive receipt under the future runbook's gitignored dated proof directory. The real source checkout stays read-only.
- [ ] Verify at least four warm end-to-end wins, the warm edit median at most 110% of fresh seeded median, and actual unaffected-unit reuse. Report spread, environment and voids. If a required case fails, keep the profile unsupported and reopen the proposed acceptance instead of advertising proven speed.
- [ ] At authorized implementation close, run the documented gate under installed `RUSTUP_TOOLCHAIN=1.97.1` and `RUSTUP_TOOLCHAIN=1.100.0` with the Global Constraints scratch isolation and retain the outputs. Do not extend testing outside the repository's scratch/supported proof rules.
- [ ] Commit the reproducible runbook and evidence reference as `docs(slot): record warm handoff acceptance procedure`.

_Requirements: SLOTS-5.1, SLOTS-5.2_

---

## Coverage and scope of this plan

All four behavioral stories and the NFR section are covered. New proof
commands are future work, and none was run during Draft shaping. Runtime
permission for existing checkouts, live migration, publication, and execution
mode stays outside this plan's Draft status. Issue #18 remains the sole tracker
issue. This plan is not published to it.
