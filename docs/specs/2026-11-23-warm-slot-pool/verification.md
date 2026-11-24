# Verify a real warm handoff

This runbook covers SLOTS-5.1 and SLOTS-5.2 for the real macOS Klynt
workspace. The performance thresholds are Draft acceptance criteria.

## Prepare the proof area

1. Start from a clean, exact Hive candidate revision. Build a release-mode
   `cargo-hive` and use the guide packaged with that revision. Use an already
   installed Rust pin, a disposable proof area and private Hive, Cargo and
   Rustup homes as described in the [build plan](tasks.md). Resolve both Cargo
   output roles under that area. Record the candidate commit, binary and guide
   digests, reported versions, OS, architecture, filesystem, Rust, Cargo,
   Clippy and Nextest versions, and warm profile digest.
2. Obtain permission to read the real source and a reviewed profile covering the
   actual commands. Copy the authorized local source and dependency material into
   the proof area. The source stays read-only. Verify both commit and tree
   identities and that the second endpoint has exactly the agreed routine Rust
   edit. Confirm identical Cargo.lock and dependency inputs, including tracked
   SQLx cache data. Use new independent secondary checkouts for the seed arm.
3. Record exact commands, features, target and build roots, and relevant
   nonsecret environment. The Klynt case uses offline Cargo and SQLx and the
   compile-only Nextest mode with the pinned listing behavior. Keep database
   setup, runtime suites and service startup outside the proof. Save any tool
   behavior and file coverage needed to qualify the reviewed profile.

Use a new dated directory under `.skills/SLOTS/proof/`, which is ignored by
Git. Keep a local manifest of environment, candidate digests, endpoints, effective
profile and tool identities. Capture raw command and unit-event logs, Hive
receipts, independent worktree/root identities, sample results and void reasons.
Filter lease credentials and other secrets from all recorded data. Store timing
in integer milliseconds beside the receipts using a monotonic clock.

## Qualify before timing

Drive the installed guide against the disposable real workspace. Use the
following exact commands for qualification.

```text
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo nextest run --workspace --all-features --no-run --locked --user-config-file none
cargo nextest run -p gateways --no-run --locked --user-config-file none
```

1. Obtain a clean initial ready checkout. Execute all three recipes under the
   guarded guide and preserve their observed completion. Compare the outputs
   with the same commands against an independent clean reference.
2. Return the clean checkout using the guide. Re-lease it at the edited commit
   and verify its stable absolute path, the actual scoped GC evidence for both
   output roles, the consumer invalidation and a successful ready handoff.
   Record evidence that the affected workspace unit ran again and an unrelated
   previously built workspace unit stayed fresh.
3. Exercise the separate seed path on an independently provisioned clean
   secondary at the same endpoint. Confirm that its two output roles resolve
   to its own roots and that seed placed eligible registry units only. Verify
   ownership and distinctness from the pool before the first build.

A refused or incomplete required case ends qualification. Record its exact
outcome, leave the profile unsupported, and reopen acceptance with evidence.

## Run five matched pairs

Predeclare the first-command recipe and edit, the evidence expected from actual
GC and registry seed, and the unit events proving rebuild and reuse. Record the
fixed inputs and command environment before each arm. Use the same host, candidate,
tools, commit pair, recipe, feature flags, cache material and nonsecret environment
throughout. Keep the separate arm outputs in comparable conditions.

1. Alternate the order of attempted pairs, starting with warm then fresh.
   Accumulate five valid pairs. Each warm attempt starts with the preceding
   successful returned lease on the same physical slot. Use that successive
   lease through reserve, checkout, validation, invalidation, real scoped GC,
   ready and the first command.
2. Start the fresh clock before provisioning a separate secondary checkout.
   Include checkout setup, the full ordinary registry-only seed and the same
   first command. Stop each arm's end-to-end clock after successful first-command
   completion. Capture intermediate warm preparation and GC times within that
   total.
3. Measure the same routine edit in each arm with a separate clock. Apply the
   same reviewed change, keep dependency inputs fixed, and use the same command
   and flags. Capture affected-unit rebuild and unaffected-unit freshness. Resolve
   the known measurement edit in each disposable checkout and verify cleanliness
   before returning the warm lease through the guide for the next handoff.
4. Void the pair if either arm fails, if exact command, toolchain, checkout,
   root, source or fixture identities diverge, if actual scoped GC is missing or
   incomplete, if the fresh seed is ineligible or unproved, or if a required unit
   observation is missing. Keep every void and the evidence for its reason.
   A slower valid sample remains in the result.

For each pair record the attempt number, arm order, raw end-to-end and edit
milliseconds, command and receipt paths, and rebuilt and reused units. Report
the median and min–max for both durations in each arm, the valid-pair win
count, and the warm-to-fresh edit median ratio. Pass SLOTS-5.1 when at least four
of five valid pairs have a strictly faster warm total. Pass SLOTS-5.2 when the
warm edit median is at most 110% of the fresh seeded edit median. Require
observed unaffected-unit reuse as part of real-workspace acceptance.

At authorized implementation close, run `scripts/gate.sh` with both installed
pins, `1.97.1` and `1.100.0`, using the documented scratch isolation. Keep
both outputs with the exact final revision.

## November 24, 2026 qualification

Klynt remains unsupported. The disposable source pins were
`205ad3b575db5337bf310d1e7fb92c21f5e1bd47` and
`8df6be1e30357cc562c8c4279a043237833302f6`. Bounded read-only Git probes
observed that each endpoint has 9,222 tracked files in 1,033,703 bytes of
NUL-delimited inventory. The size-bearing inventory reports 69,785,685 and
69,785,704 total blob bytes, with a largest blob of 2,100,599 bytes at both
endpoints. The second endpoint changes only
`crates/infra/persistence/src/lib.rs`. The Cargo.lock blob and the 1,531
tracked SQLx cache entries are identical at both endpoints. Both contain the
same checked-in Nextest config.

The verifier reviewed at `72b060bd9c23a92cccefc12722bb96f1f7f98b51`
uses a 64 KiB Git inventory cap. Its fixed and mutable
profile lists are each limited to 256 entries. Both endpoints exceed those
bounds. The default-harness check also refuses the checked-in Nextest config.
The actual workspace has local test and bench target kinds outside the admitted
grammar, and the current recipe environment has no entry for the required
`SQLX_OFFLINE=true`. The real dependency and tool closure still needs positive
custody evidence. Saved build material includes Cargo-managed generated inputs
such as `serde_core`'s included `OUT_DIR/private.rs` and unit-local native
archives. Allowing such inputs would require revising the
[design's retained-root boundary](design.md).

The qualification evidence is a source observation checked against the current
verifier predicates. It includes no observed Hive admission receipt. The saved
isolated 1.97.1 lint and compile-only runs used older flag sets and serve as
baseline controls. There are zero valid matched pairs. Both performance
thresholds remain unevaluated. The local evidence index is
`.skills/SLOTS/task-10-real-qualification.md`.

To repeat the source check, use the operator-supplied disposable checkout.
Verify its Git directories and local object store first. Use system Git with
an empty environment except for a system-only `PATH`, `GIT_CONFIG_NOSYSTEM=1`,
`GIT_CONFIG_GLOBAL=/dev/null`, `GIT_ALLOW_PROTOCOL=`, `GIT_NO_LAZY_FETCH=1`
and `GIT_OPTIONAL_LOCKS=0`, plus the `--no-replace-objects` and
`--no-optional-locks` Git flags. Run `ls-tree -r -z --full-tree ENDPOINT` for
each commit with a five-second deadline and a 2 MiB output cap. Count complete
NUL-delimited entries and bytes and verify the result with an empty error
stream. Use `rev-parse --verify` with each endpoint's `^{tree}`, `:Cargo.lock`,
`:.sqlx` and `:.config/nextest.toml`. Check the one edit with
`diff --name-only -z --no-ext-diff --no-renames BASE EDIT --`. Cap these smaller
outputs at 4 KiB. Stop if any object requires fetching or any observation is
incomplete.
