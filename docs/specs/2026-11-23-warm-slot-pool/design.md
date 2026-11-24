# Design: Warm slot pool

Feature code: SLOTS
Status: Approved
Approved: 2026-11-23 13:39 +07 by Jayden, who replied "approve" on the spec sheet at `99fcdb1faeadd60fcb3dd86047127e27d03c8a40` (recorded in `.skills/_pending-warm_slot_pool/auto/operator-reply.md`).
Date: 2026-11-23
Requirements: ./requirements.md

## Context

A fresh seeded worktree reuses registry units but still builds workspace crates
at its own path. The slot mode approved for MILE-5 keeps a secondary Git worktree
at one fixed path. CHECKOUTGC already deletes dead units and unreferenced debug
objects under Cargo's locks. Its skips do not establish readiness. The current
CLI and state file types have no pool or lease protocol.

The finished handoff has two separate proofs. The filesystem and Git checks
demonstrate an authorized clean checkout at the requested commit. The supported
build profile, targeted invalidation and causal fixture establish whether its
retained output can be delivered. Git status alone misses temporary and restored
inputs documented in SPIKES.md §4. The agent guide owns mutating Git and mtime
steps. Hive reads the checkout, changes its own state, and runs checkout-scoped GC.
For the retained consumer-anchor step, it may also lend a verified descriptor
to the guide's fixed timestamp action and launch that action with a deadline.

The design scan covers the lock and state writer, scope, receipt, Git, and GC
paths. Fresh subgraph retrieval had OWNS for 15 of 16 codes with no active OBS.
CHECKOUTGC and UPGRADE share the CLI path, and UPGRADE owns the state primitive.
The code-level reuse below comes from `crates/state/src/lib.rs`, `crates/cargo/src/git.rs`,
`crates/cargo/src/discover.rs`, `crates/gc/src/checkout.rs` and the current agent guide.
The architecture index, ADR-0004 and project code, error and testing standards
apply. No optional approved trust, operations, architecture-shape or codebase
narrative files were present.

## Decisions

1. **Lock.** A dirty return stays held and preserves work and outputs. The next
   agent receives only a verified secondary pool slot. The guide owns Git changes and authorizes anchor touches. Hive may launch
   the fixed descriptor-only anchor action described below. Cleanup stays scoped
   to that checkout.
2. **Agent-decided.** Use an additive layout-tagged slot record under the state
   root, stable per-slot operation locks and a checked generation. Registry or
   receipt extensions as lease storage would alter shipped formats and cannot
   model a lease without new replay rules. A memory-only coordinator would
   introduce a service and would lose its holds on restart. ADR-0004 already
   establishes the additive layout convention. This Draft introduces no new ADR.
3. **Agent-decided.** Choose bounded supported build profiles and unconditional
   invalidation of their candidate consumer set. Generic Cargo freshness misses
   the restored-input case. Starting fresh every time gives up stable-path
   workspace reuse. Off-profile requests get the existing fallback.
4. **Agent-decided.** Allow explicit, quiescence-based recovery, with no expiry
   or forced takeover. A missing acknowledgement is a queryable outcome, not
   additional authority to repeat a mutating step.

## Architecture

### Slot identity and clean return inspection

Satisfies: SLOTS-1.1, SLOTS-1.5, SLOTS-1.9, SLOTS-2.1, SLOTS-2.2, SLOTS-2.3, SLOTS-2.5
Reuse: rung 2 — extend `Git`'s read-only invocations, `discover::checkouts`, C1 classification and platform no-follow inspection.
Respects: ARCH-2, ARCH-4, ARCH-7, ARCH-8, ARCH-13, ARCH-15
Surface: Frozen — existing `status`, seed and GC callers of Git and C1, plus their CLI and docs, retain their existing methods and results. The slot inspection is separate.
Interface: Given the canonical registered main, pool identity and slot index, return verified checkout identity plus clean, dirty or uncertain.
Depth: n/a — extends Git and discovery readers.
Locality: Extend `crates/cargo/src/git.rs` for extra read-only Git facts and implement slot policy in `crates/cargo-hive/src/slot/inspect.rs`. Leave C1 and normal discovery unchanged.

**Agent-decided provisioning.** An explicit initialization takes a registered
main, a durable secondary root already accepted by C1, and the one supported
profile. Require the root and descendants to be canonical, same-owner directories
with no path aliases, and disjoint from registered main or owner worktree roots.
It establishes two paths `<root>/hive-slots/<pool-id>/slot-1` and `slot-2`.
The pool ID is a random 128-bit value encoded as lowercase hex. Initial slots
may be unprovisioned. First reservation authorizes only a path verified absent;
the skill uses `git -C <main> worktree add --detach <literal-path> <verified-oid>`
without force. Before the first ready result record canonical path and filesystem
identity, Git admin directory identity and common Git directory identity. Cleanup
after an ambiguous first provision is manual and non-destructive. An existing
directory without the durable identity is held rather than adopted.

Before any effect recheck C1, the registry identity, Git common/admin
identities, checkout device/inode and exact path. Compare existing HEAD to
the recorded prior clean return HEAD or establish that the provision path is
absent. Resolve the requested commit object independently in the registered
repo. The post-switch and pre-publication checks require HEAD to equal that
requested commit. No path prefix alone grants pool ownership. Re-registration,
a moved/replaced directory, symlink, or nested conflicting checkout invalidates
that proof. A returned detached commit still belongs to the old developer who
created it. The guide tells holders to deliberately keep any intended commit
reachable before return and never prunes or updates refs on their behalf.

Use the existing tracked status check without optional locks or fsmonitor.
Detect staged, unstaged, unmerged and incomplete Git operations read-only.
Inspect untracked and ignored entries with a bounded no-follow walk, excluding
only positively identified reserved Git metadata and exact directory identities
for profile-verified, exclusive generated Cargo roots. A tracked path, declared
or negative input probe, nested checkout, symlink or an unknown root is never
hidden by the exception. Slot scope requires complete Git and owner inventories.
The slot-only reader detects one byte past its accepted output cap and refuses
truncation, malformed trailing records or incomplete NUL termination even if
the retained prefix parses. Cap-sized complete input is allowed. Do not change
the existing public discovery path. The first profile excludes `node_modules` from retained
generated roots. Dirty inspection precedes every checkout/output effect. Return
with material, uncertainty, exceeded bounds, active build or possible writer
does not change checkout contents, index, mtimes or output roots.

### Durable lease state and transition fences

Satisfies: SLOTS-1.3, SLOTS-2.4, SLOTS-4.1, SLOTS-4.2, SLOTS-4.3, SLOTS-4.4, SLOTS-4.5, SLOTS-4.6, SLOTS-4.7, SLOTS-5.4
Reuse: rung 2 — `State::locked_or_cancelled` and bounded atomic state writes, platform flock and no-follow identity. Add slot-specific durability reconciliation and inherited descriptor handling.
Respects: ARCH-2, ARCH-8, ARCH-15, ARCH-16, ARCH-19
Surface: Frozen — registry, snapshots, capabilities, plans, store, seed and existing receipts readers keep their old formats and locking semantics. Slot-specific I/O uses the existing state lock, a private slot record and its independent authority witness.
Interface: Under a checked slot identity and caller credential, a command either records one fenced transition, returns an earlier outcome, or refuses.
Depth: n/a — extends the state transaction and platform lock primitives.
Locality: Extend `crates/state/src/lib.rs` with the private bounded writer and named slot recovery, add `crates/state/src/slots.rs` for slot records, extend `crates/platform/src/flock.rs` for inherited descriptors, and keep workflow transitions in `crates/cargo-hive/src/slot/mod.rs`.

**Agent-decided format.** `$HIVE_HOME/slots.json` is an owner-owned regular
file with mode 0600. Layout `2` is the first slot layout and agrees with the
current `Layout::NEWEST`. It is explicitly required even though some older,
unrelated artifacts support layout-less records. Store one strict JSON object
whose first key is `layout`, followed by a checked positive `revision` and a
`pools` array. Every field below is required unless its tagged variant omits it.
Only a field written as nullable may hold JSON null. Objects reject unknown
fields. Strings and arrays have explicit limits within the 1 MiB envelope.
The transition validator also checks cross-references, independently of parsing.

Under the existing state lock, keep an independent owner-private durable
witness alongside `slots.json` for the authoritative slot record. Bind its
exact revision and contents before any transition receipt or writer permit
grants authority. A valid-looking record by itself cannot replace the witnessed
record. On disagreement or uncertain publication, accept only the exact
independently witnessed successor after bounded re-read and durability checks.
Hold unresolved authority without changing source or output.

| JSON record | Fields and constraints |
|---|---|
| Pool | `pool_id` (128-bit lowercase hex), `main` (canonical absolute path), `common_git_id` (canonical path and device/inode), `root` (canonical secondary root), one `profile`, and exactly two `slots`. Pool ID, canonical main/Git identity and root-qualified paths cannot collide. Profile binds this pool's repo. |
| Slot | `number` (1 or 2), `path` (exact implied relative name), `lock_id` (device/inode of the private lock), `checkout_id` (nullable checked checkout/admin identity), `last_head` (nullable verified prior HEAD), `generation` (checked u64, starts at 0), `phase`, `producer`, `outcomes`. No slot or lock identity may appear twice. Unprovisioned vacant/reserved records have null checkout and HEAD. A leased record requires both. |
| Phase | Internally tagged by `kind`: `vacant` without active lease, `reserved` with `lease`, `preparing` with `lease` and `attempt`, `leased` with `lease` and `ready_commit`, `held` with `reason` and retained prior active phase or vacant provenance. Held is not assignable and cannot contain another held phase. |
| Lease / attempt | A lease has `requested_commit` (verified full Git object ID), `credential` (32-byte OS random, 64 lowercase hex digits) and `reservation_op`. An attempt has checked `attempt_id`, `begin_op`, `runner` and `stage` (permit issued or GC in progress). A runner binds local boot session, dedicated parent process identity and unique runner nonce. The attempt belongs to that slot's current lease generation. |
| Producer | Tagged by `kind`: `never_built`, `seed_only`, `pending`, `complete`, `tainted` or `unknown` as specified under bounded build provenance. |
| Outcome | Containing pool/slot identity, `generation`, `operation` (128-bit public ID), `kind`, nullable `attempt_id` for operations before preparation, exact sanitized request fields, original credential binding and typed outcome. Unique per slot and generation/attempt/operation. Historical results never provide current authority. |

Canonical serializers preserve the layout key first and sort pools and slots
deterministically. Nullable checkout identity from an interrupted initial
provision is held. Only an explicitly verified new creation can set the
initial identity. Normal return/ready recomputes the persisted HEAD binding.

Reject wrong layout, duplicate pool/slot/operation identities, unsupported path
forms, invalid phases/references and malformed or over-limit input. Impose a
1 MiB file bound and at most 32 unacknowledged transition outcomes per slot.
A full outcome list refuses new reservations with exact retry guidance. An absent record is unconfigured only when bounded reads find no slot lock
namespace and no candidate slot directories at the explicitly selected or known
registered pool roots. Existing lock or path evidence with no record is
orphaned/held with possible prior work. Never infer idle, adopt, delete or
reinitialize those paths. The operator may restore a trustworthy durable
record only with independent proof of its exact last authoritative pool ID,
revision, slot generations, attempts and replay/ack state, plus revalidated
identity and quiescence. An older copy, matching paths or successful reboot
cannot by itself rule out superseding operations. Where the last epoch cannot
be proved, explicitly quarantine the orphaned namespace while preserving its
files. An explicit fresh initialization after orphan quarantine requires a
non-overlapping root and previously unused pool ID. This lost-record route
does not replace a valid existing pool record. An unresolved or unreadable
namespace prevents initializing there. Malformed, unreadable or newer metadata
names `slots.json` and instructs inspection or use of a compatible reader; it
never proposes deleting the file. Old binaries
continue their current commands and do not migrate or read this new file.

| State | Entry and permitted effects | Next state |
|---|---|---|
| Vacant | A provisioned clean return, or never used. Reserve picks a verified eligible slot and binds one caller credential and operation ID under the state lock. | Reserved |
| Reserved | A successful `begin` irrevocably grants one attempt permission under that slot's operation lock. Only this runner may provision or switch. | Preparing |
| Preparing | Recheck paths and outputs. Skill runs Git and invalidation with the same operation lock. `ready` rechecks evidence, runs scoped GC once for the completing attempt, then publishes ready under this attempt. No build for the next holder begins in this state. | Leased or remains held |
| Leased | The agent uses supported commands. A clean, quiescent credentialed return records finalized producer evidence and releases the slot for the next validated preparation. Dirty or uncertain work and tainted/unknown provenance stay leased or visibly held without checkout or target changes. | Vacant, or held on uncertainty |
| Held | Failure or interrupted effect leaves the durable reservation, lease, or attempt in place. The operator may inspect and explicitly recover after establishing quiescence, resolving prior work, and proving identity. | Vacant for subsequent full prepared validation if proven, else remains held |

**Agent-decided operation exclusion.** Create stable, private lock files under
`$HIVE_HOME/slot-locks/<pool-id>/slot-<n>.lock` with identities bound to the
pool; never unlink/recreate them to clear a hold. The guide's dedicated
handoff process opens the known lock without truncation and passes its open
descriptor to `begin`. The platform verifies the descriptor refers to that
same owner-only regular inode, takes a nonblocking exclusive flock, and closes
only the CLI-side reference on command exit without explicitly unlocking the
inherited open-file instance. The same dedicated guide process remains parent of its CLI calls. Before
issuing the permit, record the boot session, parent instance identity and
unique runner nonce under the current generation and attempt. Use that parent's
private control input for the nonce. An independent open of the same inode is
insufficient. Later calls must match the recorded runner and be able to
nonblockingly acquire the exclusive lock on the inherited open file
description. A different open blocks while the runner owns the lock. Recheck
generation, attempt and scope immediately before each authorized mutating step;
the helper retains that descriptor until Git/mtime children exit and final
ready returns. Each foreground writer retains an inherited reference where
supported and is synchronously awaited. If the holder dies or inheritance is
unproved, the durable attempt remains blocked even if the descriptor closes.
Other state/effect commands take the same per-slot lock or refuse.
Where both locks are needed, take the slot lock before the short state lock.
Reserve selects under the state lock without taking or waiting on a slot lock.
GC and external effects run after the state lock is released.

The guide receives a mutation permit only in the first successfully
acknowledged `begin` call for that attempt. A commit-before-permit failure is
still preparing. The next call or lookup reports permit issuance in history
and cannot start Git or a new writer. Even the original credentialed agent
needs to clear the old attempt after quiescence. Use a separate new attempt
after recovery, never regrant the old one.

Supported producer starts use the same lock and parent-instance/nonce rule
for a foreground Cargo invocation. Persist pending before launch. Persist
completion only after the guide's synchronous child wait and evidence
checks succeed. A lost parent, uncertain descendant, detached child or
failed completion leaves the durable producer pending/unknown. A lock or
PID becoming absent does not clear it. For a known foreground runner,
verify all recorded children actually exited and cannot resume before
recovery. If that evidence was lost, require a verified later boot session
on the same local host and a fresh target-lock check before the operator may
recover. The local macOS boot-session identity was readable during shaping.
Inherited guard lifetime, parent identity, and causal child-exit proof must
be exercised on the admitted platform; none was implemented or tested here.
Recovery acquires the slot lock, checks and resolves prior work in scope, and
records a new attempt fence before permitting effects. Lost or tainted producer
evidence still blocks warm reuse after process quiescence.
The same-UID cooperative boundary is deliberate. Hive cannot stop a user or
tool from touching a path outside that protocol.

**Agent-decided replay.** Reserve, ready and return use caller-held unique
operation IDs, one credential and exact request parameters. The durable key binds pool ID, slot number, generation, attempt
where applicable, operation kind and operation ID. Validate that supplied pool
ID matches the registered repository's current namespace before checking old
outcomes, allocating or recovering. Check a matching
unacknowledged outcome before allocating, preparing or releasing. Reuse of the
ID with different parameters in the retained generation/attempt refuses.
A live exact replay may return a usable ready path only if the current slot is
still leased to that credential in that generation and identity/HEAD match.
For a newer generation, or the old generation already returned or recovered,
lookup returns historical completion and `current: false`, without path for use
or a renewed permit. This includes ready-reply loss followed by return and
re-lease, and applies only to an unresolved original operation. The installed
guide's private confirmation compacts only an already received result. After compaction,
the declared replay window ends and an old exact request is stale, not a new
operation. Bind every mutation to pool/slot/generation and attempt where
applicable, even if its old operation ID is no longer tracked. A new reserve
names exactly one currently vacant slot, pool ID and observed generation, so an
old request cannot choose another slot or match a new pool for the same repo
with identical slot number and generation. There is no public historical archive or caller-managed acknowledgement
workflow. The exact unresolved operation remains queryable by its current
credential or original replay input through the guide's private channel. Duplicate `begin`, `ready` or producer start for an already
active operation must report current status without replaying effects. Generation/revision exhaustion
refuses without wrapping. A caller with a lost key asks the operator for
manual recovery, never an automatic free.

A transaction first persists its intent or resulting state with the durable
write primitive. Publish no permit or ready result until durability is
confirmed. On a write error, reread while holding the state lock since the
atomic writer can fail after renaming but before syncing the parent
(`crates/state/src/lib.rs` lines 527–552). If the current revision contains the
same operation, successfully re-persist that exact revision or sync its file
and directory under the lock before returning its outcome. If the unchanged
prior revision is present, do not run outside effects until a fresh exact
retry commits. An unknown revision, mismatching file or unresolved sync
refuses and permits no downstream effect. A restart follows the same rule
before acknowledging a persisted outcome. No stale in-memory state is written
over a visible new revision. An interrupted GC whose result did not commit
leaves the slot held. Recovery requires a fresh fence and a fresh scoped
GC decision after validating the actual filesystem, even if some prior
deletions completed, before publishing readiness. Tests distinguish pre-rename
failure, visible post-rename error, durable write with lost reply, an old reserve
against a new pool with the same repo/slot/generation, stale
follow-up, two open descriptions of the same inode, paused runner, killed
parent with a surviving writer, and superseded attempts.

### Preparation and bounded build provenance

Satisfies: SLOTS-1.6, SLOTS-3.1, SLOTS-3.2, SLOTS-3.3, SLOTS-3.4, SLOTS-3.5, SLOTS-3.8, SLOTS-3.9, SLOTS-3.11
Reuse: rung 2 — use the hive guide's command workflow, existing Git facts, Cargo metadata and fingerprint readers. The new eligibility check uses these readers without changing normal Cargo execution.
Respects: ARCH-1, ARCH-2, ARCH-3, ARCH-8, ARCH-9, ARCH-13, ARCH-14, ARCH-17, ARCH-22
Surface: Frozen — the installed guide's current seed, GC and build guidance remains valid. Slot mode adds a narrowly scoped path with explicit refusal cases.
Interface: Given an active attempt and current profile, either prepare the chosen slot under the fence and return verification evidence, or report why it stays unavailable.
Depth: n/a — extends the installed guide and existing readers.
Locality: Extend `plugin/skills/hive/SKILL.md` with the fenced guide procedure and add its `scripts/slot-handoff.sh` orchestration. Add profile/evidence policy under `crates/cargo-hive/src/slot/profile.rs`.

**Agent-decided support.** Start with one reviewed repo profile per pool. Use
the klynt-like fixture with a known `persistence` consumer of `.sqlx` and
`.env`, and a demonstrably independent workspace unit. The approved Draft
intent is to certify the actual klynt profile before advertising that workflow.
Use an operator-supplied read-only klynt source and profile, with no default
path, and make a scratch copy. Exercise the actual workspace lint, compile-only
test build and package-scoped `gateways` workflow with the operator-reviewed
flags and a routine tracked Rust edit that leaves dependency inputs fixed.
Check the next commit and unrelated unit on the real admitted flow.
Do not bake guesses about the private checkout or its command layout into the
binary. A short fixed-shape profile submitted at pool initialization records
a revision, exact registered identity, admitted graph and toolchain/layout
identities, fixed Cargo command/feature shapes, effective nonsecret input
environment, both output-root constraints, and an explicit input-to-consumer
and anchor map with proof evidence. No profile editor, external observer or
new user config is needed. Existing checkout directives remain in effect. The profile's graph input
inventory binds the reviewed commands to the exact workspace manifests,
lockfile, local dependencies, feature/options and relevant project/ancestor
configuration, including absent paths. Profile review records why these
fixed inputs bound consumer discovery. Hive compares the inventory and
canonical path/object identities for both endpoints before effects, and again
when validating the requested commit and producer. The existing
`metadata::Resolved` covers paths only; it supplies no graph proof. An actual
changeset outside the profile's bounded edit set or any unproved graph input
refuses support. The guide supplies the actual fixed stock Cargo command and
its nonsecret effective input environment. It refuses unreviewed CLI, env,
configuration or wrapper overrides before launch; both interpreted roots
must match independent, uncached discovery with the supported profile inputs.

In the cooperative guide, record each supported producing invocation against
the current ready generation and validated input/profile facts before it can
write, and record its completion. A known off-profile start marks tainted. A missing
or interrupted start/completion record marks unknown for warm readiness,
including after a reboot. Multiple completed invocations carry one generation
and accumulate only allowed command shapes and root identities. This uses
explicit guide checkpoints and ordinary foreground Cargo commands, with no
installed build wrapper, hook or override. Before first use the ready result
sets `use_policy: "guide_guarded_cargo_only"`, which the guide explains in
plain language. The holder must report any bypass rather than present its
output as supported. A detected or reported bypass can leave the slot held
indefinitely. No observation of arbitrary same-account processes is promised.
A missing/incomplete invocation, possible background writer, untracked
profile change, known out-of-profile run or skipped producer evidence marks
the output unproved. The final status or file hash is not a substitute.
The review checks transient source/manifests that can change which resources
are read. Ordinary edits are admitted only where the profile bounds their
possible inputs and consumer coverage. An admitted profile enumerates or
proves exclusion of every direct and transitive consumer of allowed mutable
inputs, including absent or ancestor probes, environment values, file creation,
deletion, rename and edits restored with their mtimes. A build that can read a
retained generated root as input makes that root ineligible for the exception.
An undeclared or unreviewable producer or consumer leaves the slot unavailable
for warm reuse.

**Agent-decided profile record (schema 1).** One fixed, reviewed profile is
submitted at initialization. Required fields use the strict shapes below.

| Field | Representation and checks |
|---|---|
| `schema`, `id`, `revision`, `common_git_id` | Exact schema 1, bounded ASCII profile ID, positive u64 revision, and canonical pool Git identity. Every producer binds the profile ID and revision. |
| `graph_inputs` | Unique exact locators, relative to the checkout or from a finite set of explicit canonical ancestor/approved external paths. Each expectation is tagged `present` with exact regular-file bytes identity (verified Git object or reviewed fixed bytes), or `absent` with positive nonexistence evidence. No symlink following or implicit resolution scope. |
| `invocations` | Unique IDs each naming Cargo/rustc identity, layout, literal argv including profile/features/target, workspace-relative cwd, explicit nonsecret effective environment expectations (present value, absent, or allowed mutable input), and both output roles and expected in-slot root paths, which may coincide. Unknown overrides are refused. |
| `mutable_inputs` | Unique IDs with exact location or finite reviewed directory membership rule, named nonsecret environment input where applicable, allowed presence/absence changes, and invocation IDs. Each maps all potentially affected direct/transitive consumer IDs or gives a reviewed exclusion. Include additions, deletion, rename, and restored snapshots. |
| `consumers`, `proofs` | Unique consumer IDs identify the workspace package, unit role and invocation/layout, with tracked regular in-slot anchor path and expected Git object at each admitted endpoint. A unique proof ID refers to the dated causal fixture evidence for each admitted invocation/layout, including an unaffected fresh unit. Validate every referenced ID and refuse incomplete coverage. |

Every retained output epoch binds the pool, checkout identity and both proposed
or observed root identities. The allowed serialized producer tags carry the
following evidence. Empty and seed-only evidence is specific to a freshly
provisioned checkout and can never be inferred from arbitrary existing output.

| Tag | Required payload and transition |
|---|---|
| `never_built` | Output epoch, positively new checkout, and both root proofs (absent or verified empty at canonical path). Unclassified existing output is ineligible. |
| `seed_only` | Same epoch and generation, certified profile ID/revision, prior empty-root proof, final roots, and the typed successful registry-only seed result within initialization. No workspace producer ran. An inconclusive seed cannot confer this tag. A verified empty initial slot may instead be used without seed. |
| `pending` | Output epoch, current producing generation, profile ID and revision, actual bounded invocation ID and graph/environment evidence, both root expectations, distinct invocation operation, runner identity and prior eligible evidence. Persist before launching the sole guarded writer. |
| `complete` | Same epoch and generation, matching profile ID and revision, verified identities for both roots, and a bounded set of scoped completed invocation records with graph/effective environment evidence and verified foreground child completion. Only the immediate next reservation can inherit it. |
| `tainted` or `unknown` | Epoch, last generation, last root identities (each nullable when uncertain) and scoped reason. Tainted records an off-profile write. Unknown records lost pending writer custody, unproved invoked command or unaccounted structure. Neither transitions to eligible for these retained outputs. |

Later successful builds, removal of temporary files, and quiescence after a
reboot do not clear tainted or unknown provenance. The guide keeps that slot
held, preserves its files, and offers a separate checkout with independently
proven new output roots. The current slot remains unavailable for the life
of those retained outputs. Status identifies the blocked capacity. If both
paths are held this way, only the separate fresh seeded flow continues work.
A valid existing pool cannot be reinitialized to clear holds or replace one
of its paths in this Draft. A capacity replacement flow would need separate
lifecycle and ownership design. This avoids moving, deleting, replacing or
archiving held work or target contents.

Under the attempt fence, recheck old cleanliness, use non-forcing detached
switch if needed, and recheck the requested commit, worktree and both resolved
output roots. Compute the conservative union of verified consumer anchors for
every allowed possibly changed input, without depending on an end-state diff.
Each anchor is an existing, tracked, in-slot regular file of the actual
consumer, with identity and content preserved. The guide authorizes each mtime
action. The verifier checks the current attempt, lock, identity and content, opens
the regular anchor without following links or waiting on a nonregular file, and
lends that descriptor to the guide's single fixed Bash touch action. The action
has a deadline and accepts no checkout pathname. The normal staged readiness
checks verify its result. Validate a timestamp the admitted Cargo layout will recognize above
the relevant prior fingerprint/invocation boundary. A future stamp,
unsupported fingerprint, resolution ambiguity, unexpected symlink or failed
verification prevents readiness. Never delete or edit fingerprints just to
force a rebuild. Admission on each supported command/layout needs a causal
fixture proving the content change was consumed, restored inputs leave the
unprotected warm control stale, and a protected handoff rebuilds or reruns
the intended compile or build-script unit while preserving an unrelated
workspace unit. Separately exercise the one-changed-crate next-commit case.
If any required touch or control fails, the profile remains unsupported.

### Scoped checkout GC and fresh seed fallback

Satisfies: SLOTS-1.7, SLOTS-3.6, SLOTS-3.7, SLOTS-3.10, SLOTS-3.12
Reuse: rung 2 — reuse `metadata::resolve`, target ownership, and `gc::checkout::run` through the existing checkout-GC driver after extracting its typed operation result from emission. Continue to use shipped `seed`.
Respects: ARCH-3, ARCH-4, ARCH-5, ARCH-7, ARCH-9, ARCH-14, ARCH-17, ARCH-19, ARCH-22
Surface: Frozen — the released `gc --checkout` and `seed` callers, receipts, integration cases and guide examples retain their current selections, errors and exits.
Interface: For the verified attempt's exact checkout, return a scoped cleanup verdict covering every required root. A fallback is a distinct eligible fresh path.
Depth: n/a — extracts the existing checkout-GC operation and composes shipped seed.
Locality: Extract the checkout-scoped driver in `crates/cargo-hive/src/cli.rs` for reuse by `slot`. Keep selection and deletion in `crates/gc/src/checkout.rs` and leave `seed` behavior unchanged.

Call ROAD-16 cleanup inside the current durable preparing attempt, after
content and mtime checks and before ready. Reuse Cargo's own locks through
the typed GC driver. Obtain uncached metadata for both target and build
directories independently of the snapshot. Recheck held/unknown writers, both
root identities, and exact target/checkout ownership. The typed verdict
carries discovery and target-scan walk faults, unresolved ownership, both
expected root identities, positive per-root lock and traversal completion,
and partial or cancelled GC results. The driver binds coverage evidence
to device/inode observed under the GC lock, rather than inferring completed
work from the presence of a per-target row. Each required existing root
must be positively verified and fully handled with no incomplete finding. A demonstrably absent root of a new slot can be
accounted for separately. Missing, skipped, unsupported, held, shared, outside,
nested, unresolved, cancelled or undecided roots block warm readiness even
when the standalone command would exit zero. Readiness never uses the guide's
repo-wide GC fallback. The public standalone command retains its earlier
meaning and emits its existing receipt.

Hive publishes a ready response only when all checks agree on the same
current generation and attempt: the registered pool and actual Git identities,
post-switch requested detached HEAD and clean working tree, exclusive and
unchanged root identities, the certified profile ID/revision and matching eligible producer or verified new
empty/seed-only output, every required mtime advanced with an on-layout invalidation
proof, no active/unknown previous writer, complete scoped GC and a durable
leased transition. Hive checks scope, identity, statuses, metadata, record
fences, mtime/stamp evidence and typed GC. The cooperating runner attests
its actual invocation and foreground-child completion, input-envelope
compliance and guide-owned non-forcing Git/touch actions. Missing or
inconsistent facts are not a warm grant.

If the pool cannot supply warm output, tell the agent to create a separate
secondary worktree at the requested commit and run ordinary fresh seed under
the normal eligibility rules. Verify that it has distinct paths and does not
point at the rejected slot's workspace target/build output. Seed places registry
units only. If independent seed eligibility cannot be established, report the seed
refusal and withhold a warm result. This does not release the rejected slot.

### CLI, confidential credentials and scoped results

Satisfies: SLOTS-1.2, SLOTS-1.4, SLOTS-4.8, SLOTS-4.9, SLOTS-5.3
Reuse: rung 2 — reuse clap and existing JSON receipt conventions. Keep the slot result projection independent of the existing emitter.
Respects: ARCH-2, ARCH-8, ARCH-10, ARCH-13, ARCH-15, ARCH-16
Surface: Frozen — shell and programmatic users, receipt files and readers of existing command results keep the existing command and emitter contracts.
Interface: `cargo hive slot` or direct binary form returns one sanitized JSON result when `--json` is used. Mutation credentials travel separately.
Depth: n/a — extends CLI dispatch and uses a different projection only for new slot commands.
Locality: Extend `crates/cargo-hive/src/cli.rs` dispatch and add `crates/cargo-hive/src/slot/result.rs`. Keep `emit` and `crates/core/src/receipt.rs` frozen.

**Agent-decided command surface.** Public hive help documents `slot init`,
`slot status` and `slot recover`. The installed guide exposes take, guarded
Cargo use and return. It uses the private protocol operations in the table
for cross-process validation. Hide the private operations from public CLI
help and examples. They are not a supported standalone API. Exact unresolved
requests and their automatic confirmation remain available to the guide,
without a public historical query or ack workflow. Repository, pool ID, slot,
expected generation, commit and operation ID are explicit where applicable.

| Command | Scope and output |
|---|---|
| `slot init --repo MAIN --root ROOT --profile FILE --json` | Record the two unused paths and reviewed profile for an existing registered repo. The profile file is read only. |
| `slot status --repo MAIN --json` | Return the current pool ID, slot numbers, generations, available/held stages and next steps, including capacity that cannot be recovered in this version. No key or prior caller's secret. |
| `slot key --out-fd N --json` | Generate the credential with the platform OS entropy primitive; send it only to the caller's private descriptor, with redacted success on stdout. |
| `slot reserve --repo MAIN --pool POOL --slot N --expect GEN --commit OID --op ID --json` | Credential on stdin. Reserve the candidate explicitly or report full/unavailable. A retry of the same request returns its existing outcome. |
| `slot begin --repo MAIN --pool POOL --slot N --generation GEN --op ID --lock-fd N --json` | Credential and unique runner nonce on stdin. Only the current reservation's dedicated parent can receive a first preparation permit under the guide-held lock. Later steps verify the same runner and attempt. |
| `slot step-check --repo MAIN --pool POOL --slot N --generation GEN --attempt ID --lock-fd N --json` | Credential and runner nonce on stdin. Validate the current attempt, physical lock, scope and next guide-owned writer before that foreground action. |
| `slot producer --repo MAIN --pool POOL --slot N --generation GEN --op ID --lock-fd N --json` | Credential, runner nonce, invocation ID and current shape on stdin. Only the ready holder records scoped start and completion under the guide-held lock. |
| `slot ready --repo MAIN --pool POOL --slot N --generation GEN --attempt ID --op ID --lock-fd N --json` | Credential, runner nonce and confined preparation evidence on stdin. Run scoped GC and return ready only for the verified path and commit. |
| `slot return --repo MAIN --pool POOL --slot N --generation GEN --op ID --json` | Credential on stdin. Refuse dirty/uncertain state or return the slot cleanly. |
| `slot outcome/ack --repo MAIN --pool POOL --slot N --generation GEN --op ID [--attempt ID] --json` | Private guide-only protocol commands for credentialed exact unresolved replay and confirmation of delivered results. Include the recorded attempt when one applies. No repeated effects or general history lookup. |
| `slot recover --repo MAIN --pool POOL --slot N --generation GEN --op ID --json` | Operator verifies old processes cannot write and deliberately resolves work. The command acquires the slot lock and revalidates the scope and state. No force/data cleanup flag. A current holder can resume an authenticated interrupted attempt under the same quiescence checks. Abandoned work, a lost key or a lost runner needs explicit repository-operator confirmation and evidence. |

Use fresh operation IDs as public random 128-bit values. Slot keys are 256-bit
OS-random values. Use the repo-pinned libc's macOS entropy primitive through
`crates/platform`, with bounded error handling. Existing time/PID plan IDs
are not keys. The scoped slot reader/writer rejects unsafe metadata perms
and uses owner-only scratch/atomic private state files. The guide passes
secrets via bounded stdin and an owner-only temporary descriptor, with shell
tracing disabled and no credential in argv or environment. Sensitive parse,
debug, child and error paths must redact fields. A sanitized result has a
`schema`, stable `kind`, repo, slot number/phase where applicable, commit,
path only for the deliberate result and a next action for refusal, with no
timings. Use the existing CLI exit conventions: 0 success, 1 operational
refusal/failure, 2 unsupported OS, 130 cancelled. For audit, convert only sanitized fields to the existing `Receipt` representation
with its existing schema, command/scope, counts and findings. Append through
the state receipt writer to its normal date-based JSONL file. The existing
receipt readers still parse every line and no new slot projection appears in
that log. Keep the stdout slot result separate. The authoritative operation
outcome is the slot state transaction. A failed receipt append or interrupted delivery does not release
the lease or justify running the transition again.

### Installed guide and measurement

Satisfies: SLOTS-1.8, SLOTS-2.6, SLOTS-5.1, SLOTS-5.2
Reuse: rung 2 — extend `plugin/skills/hive/SKILL.md` and reuse the scorecard's matched controls and existing guide/CLI cross-checks.
Respects: ARCH-1, ARCH-2, ARCH-8, ARCH-10
Surface: Frozen — existing guide actions, README samples and scorecard targets remain intact. New sections document slot-specific outcomes and a separate evidence runbook.
Interface: The guide drives take, use, return and recovery. A record of paired real commands shows correctness and the Draft performance criteria.
Depth: n/a — extends the guide and scorecard sampling approach.
Locality: Extend the hive guide and `README.md`. Add a focused runbook in the SLOTS spec directory during implementation, with separate measurement evidence.

Keep the agent journey short: choose warm for the reviewed profile, reserve,
prepare, use only after ready, resolve work, return. Document held remedies,
credential recovery, prohibited tracked edits and full-pool fallback.
The take result and guide both state the guarded Cargo procedure before use,
including the lost-capacity effect of unrecorded work. Keep the checkpoints
and receipt confirmation private to the guide. Public operator inspection and
recovery explain when held warm capacity cannot be restored. The operator
can stop requesting slots and keep the existing checkout flow. No
auto retirement changes a worktree.

**Agent-decided private helper compatibility.** Before any slot-helper
reservation, retry, recovery or effect, resolve its hive binary and compare
the full version reported by that same binary with the installed guide's
`metadata.hive-version`. Use the existing read-only version check. Recheck
whenever starting or resuming a helper invocation. A missing, unreadable,
unparseable or nonidentical version, including a patch mismatch, refuses with
the update action before any slot reservation, phase change, Git step, mtime
touch or GC. The bail-out preserves any existing hold. Non-slot guide use
keeps its existing same-major/minor policy. The bundled metadata remains the
single version source. See `plugin/skills/hive/SKILL.md`, Version first.

Use five valid matched pairs as agent-decided Draft acceptance. Compare
complete warm reservation/handoff plus first command to fresh worktree
provisioning plus seed plus the same first command. Record switch/check/GC
and preparation times within the total, with the same host, profile, commit,
toolchain, arguments and edit. Run the warm pairs across successive leases,
alternate which arm runs first, keep each sample and its void reason, and
show raw times, medians, spread, units actually rebuilt and unrelated units
reused. Warm total is strictly faster in at least four valid pairs. The
warm edit median is at most 110% of the fresh seeded median. Timings measured
by hand appear as integer milliseconds outside hive's receipt and name the
machine/toolchain. These targets are Draft, separate from approved Scorecard
targets. Compare against the real release-mode checkout GC implementation,
never the historical emulation.

## Seams for testing

| Seam | Kind | Covers |
|---|---|---|
| Real `cargo hive slot` and existing public CLI on scratch state/repos | Integration | SLOTS-1.1, SLOTS-1.2, SLOTS-1.3, SLOTS-1.4, SLOTS-1.5, SLOTS-1.6, SLOTS-1.7, SLOTS-1.9, SLOTS-2.1, SLOTS-2.2, SLOTS-2.3, SLOTS-2.4, SLOTS-2.5, SLOTS-3.1, SLOTS-3.2, SLOTS-3.3, SLOTS-3.4, SLOTS-3.5, SLOTS-3.6, SLOTS-3.7, SLOTS-3.10, SLOTS-3.11, SLOTS-3.12, SLOTS-4.1, SLOTS-4.2, SLOTS-4.3, SLOTS-4.4, SLOTS-4.5, SLOTS-4.6, SLOTS-4.7, SLOTS-4.8, SLOTS-4.9, SLOTS-5.3, SLOTS-5.4 |
| Injected slot publication fault at the existing transaction/I/O boundary, plus real subprocesses | Unit/integration | SLOTS-4.1, SLOTS-4.2, SLOTS-4.3, SLOTS-4.6, SLOTS-5.4 |
| Installed hive guide and real stock Cargo, with independent clean and unprotected stale controls on each admitted layout | Integration/acceptance | SLOTS-1.8, SLOTS-2.6, SLOTS-3.8, SLOTS-3.9 |
| Paired real release-mode handoff and seed on supported proof machine | Manual evidence | SLOTS-5.1, SLOTS-5.2 |

## Coverage check

Each of the 40 behavioral and quality IDs appears on one Satisfies line and
at one or more verification seams. No browser-rendered surface is delivered.
All mechanisms and numbers describe planned proofs and implementation. No
profile, filesystem fence, live state change or performance result was
exercised during shaping.
