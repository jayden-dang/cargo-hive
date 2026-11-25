# Fixes

Requirements for defects no feature spec owns. Each fix pairs the corrective requirement
with a `SHALL CONTINUE TO` guard.

## FIX-1 Packaging leaves the dev build able to see source edits

Found 2026-11-13. Since Cargo 1.91 the verify build of `cargo package` and `cargo publish`
reuses the workspace's target dir (cargo#15783), and the unpacked copy under
`target/package/<name>-<version>/` gets the same unit hash as the workspace package
(cargo#12516). For a manifest with explicit `[lib]` and `[[bin]]` targets, the verify build
overwrote the lib's fingerprint with a dep-info naming the packaged sources, so after the
v0.1.0 release `cargo build` reported `Fresh` and ignored edits under `src/`.
Cause accepted by the owner 2026-11-13 (confirmed for scope: Cargo 1.97.1, macOS, the
v0.1.0 release run). Investigation note: `.skills/research/2026-11-13-cargo-package-verify-fingerprint.md`
(local).

- **FIX-1.1** WHEN a release step runs `cargo publish --dry-run` or `cargo publish` THE RELEASE STEPS SHALL run it with `CARGO_TARGET_DIR` set to a directory other than the repo's `target/` (`target/package-verify`).
- **FIX-1.2** AFTER any release step that packages the crate, THE SYSTEM SHALL CONTINUE TO recompile the lib when a file under `src/` changes, as `scripts/fresh-probe.sh` checks by printing `GREEN`.

Checked by: `scripts/fresh-probe.sh`, run in release steps 3 and 6 (`docs/agents/project.md`).

## FIX-2 A target that something created before Cargo is invisible to hive

Found 2026-11-24 (#42). Hive recognizes a Cargo target by the `CACHEDIR.TAG` in it
(`target::identify`). Cause accepted by the owner 2026-11-25 ("approve, do it") for the
reproduced scope (macOS, Rust 1.97.1 and 1.100.0, cargo-nextest 0.9.144): Cargo writes that tag only
when it creates the target dir itself, and cargo-nextest with JUnit output configured
creates its store dir `target/nextest` before it invokes Cargo, so the tag never appears, and
`status`, `seed` and `gc --checkout` treat the checkout as having no target
(`counts.targets` is 0, no finding).
`mkdir target && cargo build` also leaves no tag on Rust 1.97.1 and 1.100.0. Evidence: the
red/green repro script `/private/tmp/tagbug-scripts/repro.sh <hive-bin> junit|nojunit`
(local). The identity rule stays tag-based; the owner chose to report the case and to tag
what seed creates.

- **FIX-2.1** WHEN `status`, `seed` or `gc --checkout` scans a checkout whose Cargo target location holds a directory with no Cargo tag but with Cargo's layout (a profile dir with `.fingerprint`, or `.rustc_info.json`) THE SYSTEM SHALL report a `target.untagged` finding naming that path and the remedy (Cargo's exact tag bytes, or deleting the directory so Cargo recreates it), and SHALL NOT read, clone from, or delete anything in it.
- **FIX-2.2** WHEN `seed` creates a checkout's target dir THE SYSTEM SHALL leave it carrying Cargo's tag, whether or not it places any unit (the standalone path; a slot's seed output is unchanged), and SHALL NOT write a tag into a target dir that already exists. SEED-1.5 carries the matching exception. A later `seed` treats a target holding only that exact tag as absent and seeds it (owner decision 2026-11-25, SEED-1.4).
- **FIX-2.3** WHEN a target dir carries Cargo's tag THE SYSTEM SHALL CONTINUE TO discover, own, lock, report and clean it exactly as before, with no `target.untagged` finding.

Checked by: `crates/cargo-hive/tests/untagged_target.rs`, and the unit tests of
`is_untagged_cargo_shaped` in `crates/core/src/target.rs`.
