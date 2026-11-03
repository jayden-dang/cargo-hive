# Testing standards

Status: Approved
Date: 2026-10-28

## Purpose and boundary

How cargo-hive is tested and what must pass before a merge. Covers the merge gate, where tests live, how they are written, and what they may touch on the machine running them.

## Merge gate

Every merge needs all four commands to exit 0:

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo test`
- `cargo check --target x86_64-unknown-linux-gnu`, which proves the non-macOS stub still compiles

## Strategy

| Layer | Purpose | When required |
|---|---|---|
| unit | A module's rules and limits, in `#[cfg(test)] mod tests` beside the code | Every rule gets table-driven cases; every limit gets tests at the exact boundary and one past it |
| integration | Behaviour seen through the `cargo-hive` binary, run as a subprocess with `HIVE_HOME` set to a scratch dir | No trigger rule ratified; these tests live in `tests/*.rs` and share `tests/common` |
| document | Repo files that ship beside the binary (the agent skill, plugin manifests, eval records) agree with the crate: versions, spec limits, and every command line parsing with the public `cargo_hive::cli::Cli` parser in-process | When such a file is added or changed; these tests live in `tests/skill.rs`, read files from `CARGO_MANIFEST_DIR`, touch nothing on the machine, and need neither `HIVE_HOME` nor `tests/common` |

## Test-first rules

None — the 2026-10-28 interview ratified no rule beyond the `test-first` skill, which governs.

## Mocks and boundaries

- Tests never write under the real `~/.cargo`, `~/.rustup`, `~/.cache`, or any repo outside their scratch dir.
- Integration fixtures live in `/private/tmp/hive-it-*` and are removed on drop.
- `HIVE_HOME` exists for tests only.
- Exception (approved 2026-10-28 for AUTOGC-5.4 and AUTOGC-6.4): a test may create an APFS disk image with `hdiutil`, under these conditions:
  - the image lives in its `/private/tmp/hive-it-*` scratch dir;
  - it is attached with `-nobrowse` and `-mountpoint` inside that dir, never under `/Volumes`;
  - it contains `.metadata_never_index`;
  - it is detached on drop;
  - stale `hive-it-*` images and mounts are swept at test start.
- Exception (approved 2026-10-31 for LIVE-4.18 to LIVE-4.28; amended 2026-10-31 for LIVE-8.1 to LIVE-8.7): the `#[ignore]` test in `tests/liveness_prove.rs` may run compile-only `cargo` commands in a checkout the owner names through `HIVE_PROVE_CHECKOUT`, under these conditions:
  - it never runs by default, and it has no default target or checkout;
  - only `build`, `check`, `doc`, `test --no-run` and `bench --no-run` are replayed, so no test or binary of the checkout is executed; the checkout's own `build.rs` and proc-macros run as they do in the owner's own `cargo build`;
  - every replay uses `--locked` and has `CARGO_TARGET_DIR` and `CARGO_BUILD_BUILD_DIR` set to a clone inside the test's `/private/tmp/hive-it-*` scratch dir;
  - the original target dir is locked only while the test decides liveness and takes one APFS snapshot clone of it; every later step works on clones, and the test itself deletes or modifies nothing in the original (it removes the snapshot once the warm-up clone exists); the end-of-run checks below catch only the breaches they name, not every possible change to the original;
  - at the end, every file the test listed for deletion must still exist in the original, the `Cargo.lock` of `HIVE_PROVE_CHECKOUT` must be unchanged, and every `compiler-artifact` a replay reports must lie inside the clone it replays into; any breach fails the test.
- Exception (approved 2026-11-03): the `#[ignore]` test in `tests/seed.rs` may seed and build a checkout the owner names through `HIVE_SEED_CHECKOUT`, under these conditions:
  - it never runs by default, and it has no default checkout;
  - the test creates and removes its own git worktree inside that checkout;
  - `CARGO_TARGET_DIR` stays unset;
  - `HIVE_HOME` is a scratch directory under `/private/tmp/hive-it-*`;
  - the build is `cargo build --offline --locked`;
  - the test does not write `~/.rustup` or `~/.cache`. Cargo's default home stays in use, because that is where the checkout's registry lives. The only change under `~/.cargo` the test may leave is the mtime of `~/.cargo/.global-cache`, which may also move the mtime of the `~/.cargo` directory. Every other direct child of `~/.cargo` is unchanged.

## Not a substitute for acceptance

Unit conventions here do not replace `validate-feature` / e2e acceptance.
