# Requirements: Warm slot pool

Feature code: SLOTS
Status: In-progress
Approved: 2026-11-23 13:39 +07 by Jayden, who replied "approve" on the spec sheet at `99fcdb1faeadd60fcb3dd86047127e27d03c8a40` (recorded in `.skills/_pending-warm_slot_pool/auto/operator-reply.md`).
Date: 2026-11-23
Roadmap item: ROAD-17 (MILE-5, issue #18)

Sources are the ROAD-17 card, its 2026-11-23 framing notes, and the prior approved
MILE-5 pivot. The user authorized drafting the persisted lease format and the triad.
An **agent-decided** criterion records a Draft implementation or scope choice,
including a delegated choice. All measurements and runtime effects remain unverified.

A **slot** is a secondary linked worktree explicitly provisioned for the pool.
A **lease** is the current agent's exclusive use of that slot. A **ready** result
authorizes use after handoff, whereas a reservation temporarily holds an assigned
slot for preparation. A **reviewed build profile** binds a repository and bounded
Cargo invocation shapes to the producer and consumer evidence in story 3. It
applies to cooperating agents on the repository owner's macOS account. Each
supported Cargo invocation uses the installed guide's guarded procedure. A
known bypass leaves the affected output outside supported reuse, and this
contract does not promise detection of every unreported side build.

## 1. Take a warm worktree

**Story:** As a coding agent, I want to take a pooled worktree at my requested commit, so that I can start work at a stable path with useful build state.

- **SLOTS-1.1** (agent-decided) WHEN the repository operator establishes a pool at an explicitly selected durable secondary root THE SYSTEM SHALL provision capacity for two stable slot paths for that registered repository.
- **SLOTS-1.2** WHEN a supported take completes successfully THE SYSTEM SHALL return one machine-readable ready result naming the registered repository, the assigned path and its verified requested commit.
- **SLOTS-1.3** WHILE a slot is reserved, leased or held for recovery THE SYSTEM SHALL issue no assignment for that slot to a competing request.
- **SLOTS-1.4** IF no slot is available THEN THE SYSTEM SHALL report an actionable pool-unavailable result that directs the agent to a separate fresh seeded checkout.
- **SLOTS-1.5** (agent-decided positive identity check) IF a candidate is main, an owner's existing lane, an unclassified path, or a path with unverified pool or Git identity THEN THE SYSTEM SHALL refuse it before slot-level Git or target changes.
- **SLOTS-1.6** (agent-decided) IF a supported warm handoff cannot be completed THEN THE SYSTEM SHALL withhold a ready warm result and point to a distinct fresh checkout when one can continue independently.
- **SLOTS-1.7** (guard) WHEN the existing seed command receives an otherwise eligible fresh secondary checkout THE SYSTEM SHALL CONTINUE TO operate under the shipped seed contract, including registry-unit-only seeding.
- **SLOTS-1.8** (agent-decided take notice) WHEN an agent takes a slot through the installed hive guide THE SYSTEM SHALL present the supported use contract before the first build. The notice distinguishes reservation, ready, fallback and refusal, tells the holder to run Cargo through the guide, and says an unrecorded build may hold that slot indefinitely.
- **SLOTS-1.9** (guard) WHEN the existing Git discovery and classification path runs for a checkout THE SYSTEM SHALL CONTINUE TO apply the current main, linked secondary, configured-root secondary, and protected fallback rules.

## 2. Return a worktree without losing unfinished work

**Story:** As a lease holder, I want to return a worktree without losing any unfinished work, so that a failed return never makes my work available to another agent.

- **SLOTS-2.1** IF the returning slot has an index or tracked change THEN THE SYSTEM SHALL refuse the return while keeping the current lease held.
- **SLOTS-2.2** (agent-decided) IF the slot contains untracked or ignored material outside verified retained generated roots, including a temporary `.env` or untracked `.sqlx` input THEN THE SYSTEM SHALL refuse the return while keeping the current lease held.
- **SLOTS-2.3** WHILE a dirty or uncertain return is being checked or refused THE SYSTEM SHALL preserve the slot's source bytes, index, mtimes and build state, with no GC, switch, stash, archive or discard.
- **SLOTS-2.4** (agent-decided quiescent recheck and preparation fence) WHEN the current holder deliberately resolves unfinished work and a quiescent recheck verifies a clean return THE SYSTEM SHALL make the slot eligible for the next fenced preparation.
- **SLOTS-2.5** (agent-decided) IF unfinished-work inspection fails or a prior writer may still change the slot THEN THE SYSTEM SHALL keep the slot unavailable and report what remains to be checked.
- **SLOTS-2.6** WHEN an agent receives a refused return THE SYSTEM SHALL document a non-destructive path to inspect and deliberately resolve work, retry or escalate to the repository operator.

## 3. Receive a prepared checkout without stale results

**Story:** As the next lease holder, I want to receive a prepared checkout, so that my build sees my own inputs and can keep unrelated workspace units warm.

- **SLOTS-3.1** (agent-decided attempt fence) WHEN an assigned slot changes commits THE SYSTEM SHALL use the hive guide to switch only that verified slot to the requested commit under its current preparation fence.
- **SLOTS-3.2** (agent-decided) IF the previous producer generation, reviewed profile, effective commands and environment, Cargo layout, or both output-root identities cannot be established THEN THE SYSTEM SHALL withhold a ready warm result.
- **SLOTS-3.3** (agent-decided) IF a supported profile cannot prove coverage or exclusion of every direct and transitive consumer of its permitted mutable inputs, including external ancestor and previously absent probes, creation, deletion, rename, and edits restored with their mtimes THEN THE SYSTEM SHALL withhold a ready warm result.
- **SLOTS-3.4** (agent-decided) WHEN a supported warm handoff prepares the requested commit THE SYSTEM SHALL use the guide to invalidate every possibly affected consumer before ready, irrespective of the final Git diff or current input bytes.
- **SLOTS-3.5** (agent-decided current-attempt verification) IF Git switching, cleanliness validation or invalidation cannot be completed and verified under the current attempt THEN THE SYSTEM SHALL keep the slot unavailable.
- **SLOTS-3.6** WHEN a prepared slot is handed off THE SYSTEM SHALL complete ROAD-16 checkout-scoped GC on that slot before delivery.
- **SLOTS-3.7** (agent-decided) IF a required GC root is held, shared, outside, undecided, unresolved, cancelled or otherwise not confirmed cleanable by the scoped run THEN THE SYSTEM SHALL withhold warm readiness. The guide must not escalate this handoff to repo-wide cleanup.
- **SLOTS-3.8** WHEN a supported fixture hands off after building with a changed `.sqlx` input and a temporary `.env` that the holder deliberately resolved THE SYSTEM SHALL deliver a checkout with neither the prior temporary file nor prior-input build results, as verified by the causal check below.
- **SLOTS-3.9** WHEN the agent takes a supported slot at the next commit in the one-changed-crate fixture THE SYSTEM SHALL retain a build result that Cargo can reuse for an unrelated workspace unit.
- **SLOTS-3.10** (guard) WHEN standalone `gc --checkout` is invoked THE SYSTEM SHALL CONTINUE TO apply its existing ownership, lock, deletion and receipt rules without requiring a slot.
- **SLOTS-3.11** (agent-decided) IF a handoff would require a tracked content edit beyond the verified Git switch or a `node_modules` removal THEN THE SYSTEM SHALL withhold warm readiness and leave those actions to the operator.
- **SLOTS-3.12** (agent-decided) IF a request falls back from a rejected slot THEN THE SYSTEM SHALL keep the fallback independent of the rejected slot's workspace build output.

For SLOTS-3.8, each admitted Cargo layout needs both a restored `.sqlx` case (including
the original mtime) and a removed `.env` case (including a previously absent lookup). In
each case the changed input must produce a different result. After deliberate restoration
or removal, an unprotected warm control must stay stale. The protected handoff must
match an independent clean result and leave an unrelated workspace unit fresh. A ready
receipt alone does not verify either build outcome.

## 4. Recover a stuck slot

**Story:** As the repository operator, I want to recover a stuck slot after confirming the old holder is gone, so that I can restore pool capacity without losing work.

- **SLOTS-4.1** (agent-decided) WHEN a holder or preparation process disappears THE SYSTEM SHALL preserve the last durable slot state until a current holder retries safely or the operator explicitly recovers it.
- **SLOTS-4.2** (agent-decided) IF a request uses a stale generation or attempt THEN THE SYSTEM SHALL refuse it without mutating a current checkout or lease.
- **SLOTS-4.3** (agent-decided) IF an acknowledgement is missing after a committed transition THEN THE SYSTEM SHALL allow the original credentialed caller to recover the transition's outcome without assigning or applying the work twice.
- **SLOTS-4.4** (agent-decided) IF a slot's previous process can still write, its work remains unresolved, or scope/producer provenance cannot be established THEN THE SYSTEM SHALL prevent the recovery command from making that slot ready.
- **SLOTS-4.5** (agent-decided) WHEN the repository operator confirms quiescence and resolves work on an abandoned slot with provable output provenance THE SYSTEM SHALL permit explicit recovery through the same identity, preparation and readiness checks as a normal handoff.
- **SLOTS-4.6** (agent-decided) IF the pool record is unreadable, malformed or newer than supported THEN THE SYSTEM SHALL refuse slot mutations and name the file and recovery action.
- **SLOTS-4.7** (guard) WHEN an existing consumer reads an artifact from a pre-slot hive state root THE SYSTEM SHALL CONTINUE TO read it or report a refusal naming the file under the artifact's established layout rules.
- **SLOTS-4.8** (guard) WHEN a pre-existing CLI command emits a receipt THE SYSTEM SHALL CONTINUE TO honor its existing JSON schema, findings, exit and data-logging contract.
- **SLOTS-4.9** (agent-decided) WHEN the current slot status is requested THE SYSTEM SHALL report held or unavailable states and next steps without revealing another caller's credential. If output provenance cannot be restored for a retained epoch, report that this slot does not regain warm capacity in this version and give the separate seeded fallback.

## 5. Quality attributes

**Section-kind:** nfr

**Story:** As a repository operator, I want measurable evidence of reuse, exclusivity and recovery, so that I can decide whether slot mode is suitable.

- **Performance:** **SLOTS-5.1** (agent-decided Draft acceptance threshold) WHEN a supported warm slot is measured against matched fresh seeded setup and first-command executions THE SYSTEM SHALL be faster end to end in at least four of five valid pairs, including all reservation, checkout, validation, invalidation and real GC time. Verify on the same machine and toolchain with matching work and record raw integer milliseconds, median, spread and voids outside hive receipts.
- **Performance:** **SLOTS-5.2** (agent-decided Draft acceptance threshold) WHEN the same edit is built in both arms THE SYSTEM SHALL have a warm edit median at most 110% of the fresh seeded median over the five valid pairs. Verify using the same edit, command and fixture. Repeat slot handoffs across the measured warm arm.
- **Security:** **SLOTS-5.3** (agent-decided local credential scope) THE SYSTEM SHALL prevent disclosure of slot credentials in argv, persisted receipts, human diagnostics and status output. Verify with a known credential and inspection across normal, refused and interrupted CLI calls. Same-UID hostile processes that bypass the guide are outside this interface's trust boundary.
- **Reliability:** **SLOTS-5.4** (agent-decided) WHEN competing requests, interrupted handoff steps or an ambiguous state write are injected THE SYSTEM SHALL maintain at most one current ready owner of a slot. Verify with subprocess concurrency, crash/restart, stale attempt and commit-before-response cases.
- **Accessibility:** None. Headless command and machine-readable receipts. No person-facing UI is introduced.

Product metrics, operational reliability, security threat model, personas and
accessibility policy files were absent during the NFR consultation. The scorecard
and ARCH-10 ground the proposed measurement method. The two thresholds above
are Draft acceptance values. The initial real-workspace admission (agent-decided
proof scope) uses the operator's read-only klynt source and reviewed commands
on a scratch copy. Exercise the actual workspace lint, compile-only test
build and package-scoped `gateways` workflow, with a routine tracked Rust
edit that leaves dependency inputs fixed. Capture exact invocation and feature
flags supplied for the fixture. A synthetic fixture alone cannot admit klynt.

## Guard surfaces

| Files or surface | Guard |
|---|---|
| `crates/cargo-hive/src/cli.rs` and existing receipt emit/logging | SLOTS-4.8 |
| `crates/cargo/src/git.rs` and `crates/cargo/src/discover.rs` | SLOTS-1.9 |
| `crates/state/src/lib.rs` and old state readers | SLOTS-4.7 |
| `crates/gc/src/checkout.rs`, existing checkout GC CLI and `plugin/skills/hive/SKILL.md` guidance | SLOTS-3.10 |
| `crates/seed/src/lib.rs`, existing seed CLI and guide | SLOTS-1.7 |
| `README.md` and `crates/cargo-hive/tests/skill.rs` | Existing command examples and guide/binary cross-checks preserved by SLOTS-1.7, SLOTS-3.10 and SLOTS-4.8. |
| New slot state/workflow modules, focused slot integration fixtures, profile examples and new evidence runbook | No behavior to guard. |

## Out of Scope

- Linux, harness integration ROAD-18, compiler wrappers, Cargo config and global build hooks.
- Automatic discard, stash, backup or recovery archive of uncommitted work. The operator's separate approval remains required for extra tracked content edits and removing `node_modules`.
- Unbounded reuse of arbitrary undeclared inputs, same-account process isolation, replacement of an existing held pool's capacity, changes to standalone seed or GC behavior, and a new telemetry service.
- Shipping, implementation, building, or migration as part of Draft shaping.

## Open Questions

- Actual profile coverage, Cargo invalidation across the supported layouts and real performance — future ROAD-17 implementer — resolve before approval to advertise warm support — do not guess a consumer anchor, treat history as a passing test, or claim an unmeasured speed.
- A needed tracked content edit or removal of `node_modules` — repository operator — before approving any such additional action — do not automate or infer that consent.
