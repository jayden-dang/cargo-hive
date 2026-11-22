# Worktree scorecard

What an agent worktree costs, cold and seeded, on the measured workspace. A cold checkout is one hive did not seed. A seeded checkout is one that `cargo hive seed` seeded just before its first build. Every durable value below is an integer: durations in milliseconds, sizes in bytes. A cell reading `not yet measured` has no run behind it yet.

## Values

| Row | Metric | Source | Cold n | Cold median | Seeded n | Seeded median | Target (cold) | Target (seeded) | Target set |
|---|---|---|---|---|---|---|---|---|---|
| R1 | First-command time for `build` (ms) | `cold_build_ms_median` / `seeded_total_ms_median` | not yet measured | not yet measured | not yet measured | not yet measured | open — operator | open — operator |  |
| R2 | First-command time for `check --all-targets` (ms) | `cold_build_ms_median` / `seeded_total_ms_median` | not yet measured | not yet measured | not yet measured | not yet measured | open — operator | open — operator |  |
| R3 | First-command time for `clippy --all-targets` (ms) | `cold_build_ms_median` / `seeded_total_ms_median` | not yet measured | not yet measured | not yet measured | not yet measured | open — operator | open — operator |  |
| R4 | First-command time for `test --no-run` (ms) | `cold_build_ms_median` / `seeded_total_ms_median` | not yet measured | not yet measured | not yet measured | not yet measured | open — operator | open — operator |  |
| R5 | First-command time for `check -p <package> --lib` (ms) | `cold_build_ms_median` / `seeded_total_ms_median` | not yet measured | not yet measured | not yet measured | not yet measured | open — operator | open — operator |  |
| R6 | Next build after a one-file edit in a workspace crate (ms) | by hand (outside bench) | not yet measured | not yet measured | not yet measured | not yet measured | open — operator | open — operator |  |
| R7 | Private bytes each new worktree adds (bytes) | by hand (outside bench): free space | not yet measured | not yet measured | not yet measured | not yet measured | open — operator | open — operator |  |
| R8 | Units rebuilt because of a hive deletion | by hand (outside bench): cargo -v after gc --checkout | not yet measured | not yet measured | not yet measured | not yet measured | open — operator | open — operator |  |
| R9 | Stale results on a probe that changes a file a proc macro reads without declaring it | by hand (outside bench): cargo -v after probe | not yet measured | not yet measured | not yet measured | not yet measured | open — operator | open — operator |  |
| R10 | Setup footprint | listed | — | nothing | — | `cargo install cargo-hive`, `cargo hive doctor`, `cargo hive repo add <main>`; Cargo config files 0, environment variables 0, rustc wrappers 0, cargo shims 0; files outside the state root: 0, beyond clones into target dirs | open — operator | open — operator |  |

R8 cells read `R rebuilt over k non-vacuous samples; m vacuous`, or `all vacuous`. R9 cells read `s stale of n`.

## Provenance

| Row | Measured on | Hive commit | Toolchain | Machine | Evidence |
|---|---|---|---|---|---|
| R1 | not yet measured | not yet measured | not yet measured | not yet measured | not yet measured |
| R2 | not yet measured | not yet measured | not yet measured | not yet measured | not yet measured |
| R3 | not yet measured | not yet measured | not yet measured | not yet measured | not yet measured |
| R4 | not yet measured | not yet measured | not yet measured | not yet measured | not yet measured |
| R5 | not yet measured | not yet measured | not yet measured | not yet measured | not yet measured |
| R6 | not yet measured | not yet measured | not yet measured | not yet measured | not yet measured |
| R7 | not yet measured | not yet measured | not yet measured | not yet measured | not yet measured |
| R8 | not yet measured | not yet measured | not yet measured | not yet measured | not yet measured |
| R9 | not yet measured | not yet measured | not yet measured | not yet measured | not yet measured |
| R10 | not yet measured | not yet measured | not yet measured | not yet measured | not yet measured |

## Targets

The operator sets a target by editing that row's Target cell and its Target set date in a plain commit. A rerun never touches either.

## Notes per row

- R1 to R5: bench times one first build per fresh checkout. Rows measured later in a run see a store and a main target dir that earlier shapes already filled, so R2 to R5 are noted as measured after earlier shapes.
- R6: by hand (outside bench). The environment matches bench's: the same variable scrub that hive applies to the builds it runs. Only the cargo exec is timed, after a fixed one-file edit to a workspace crate. The median follows bench's rule: the middle value for odd n, and the floor of the mean of the two middle values for even n.
- R7: the bytes the volume lost, read as the drop in free space from before a worktree's first build to after it, never bytes that clones share. The reading also carries the seed's store writes, any Cargo source unpacks, and any other writer on the same APFS container, so the quiet-machine rule and the median over four samples are the only guards.
- R8: a sample is vacuous when the deletion removed no unit files and no objects, because then nothing could be rebuilt because of it. A vacuous sample is valid and counted. A sample is void when the deletion or the rebuild fails.

## What a run touches

- The real hive state root: its capabilities file (doctor), the store, the store database and receipts, from bench's priming, `seed` and `gc`.
- The main checkout's target dir.
- The Cargo home: every build is offline, so nothing is downloaded, but Cargo may unpack cached crate files into its registry source dir.
- The main checkout's worktree metadata under its git dir.
- A scratch dir named `scorecard` under the temp dir.

Nothing else is touched. Untracked files the workspace needs to build, such as an `.env`, are copied into each worktree from the local env file's list.

The Rerun blocks are zsh on macOS. Linux is out of scope.

## Rerun

### Re-check the setup footprint

Compare R10's Seeded cell with the install section of the README: each command a user runs, and the safety guarantees about what hive installs and writes. The README's source install (`cargo install --path crates/cargo-hive`) is the source form of `cargo install cargo-hive`. If a line changed, fix it in this rerun's own commit.
