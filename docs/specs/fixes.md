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
