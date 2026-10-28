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

## Test-first rules

None — the 2026-10-28 interview ratified no rule beyond the `test-first` skill, which governs.

## Mocks and boundaries

- Tests never write under the real `~/.cargo`, `~/.rustup`, `~/.cache`, or any repo outside their scratch dir.
- Integration fixtures live in `/private/tmp/hive-it-*` and are removed on drop.
- `HIVE_HOME` exists for tests only.

## Not a substitute for acceptance

Unit conventions here do not replace `validate-feature` / e2e acceptance.
