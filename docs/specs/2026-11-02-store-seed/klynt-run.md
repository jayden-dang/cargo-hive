# Klynt seed run

Date: 2026-11-03

One run. The summary is `target/klynt-run/summary.json` (sha256 `02acdad458923080d27325cf3e57fb2c88e5272f093d09ceef99e167437aa1c8`). Seed 116s, build 100s, status 3s. The test finished in 424.23s and exited 0. `cargo build --offline --locked` exited 0 and `cloned_recompiled` was empty. The home comparison passed. The worktree and the scratch home were removed.

## Command

```
HIVE_SEED_CHECKOUT=/Users/jayden/Developer/klynt/klynt cargo test --test seed --offline -- --ignored seeding_a_new_worktree_of_a_real_repo_recompiles_no_cloned_unit
```

Hive: `78e1f3620c52b5cfda183e51f395b9d52b9cf6cd`. Checkout: `/Users/jayden/Developer/klynt/klynt` at `0bdae577810ed2634874443a27a636fa03ee4e83`. The test added a detached worktree at `.worktrees/hive-seed-71383` and removed it. `git worktree list` in klynt does not name it.

## Seed receipt

Receipt sha256: `83e408053e21fdc6f65e1047c889b2e4b33ea6f14df97a9de7eadaac65ae9d10` (`target/klynt-run/seed-receipt.json`, not committed).

| Count | Value |
|---|---|
| fill_entries_added | 1490 |
| fill_entries_held | 1924 |
| fill_targets_read | 3 |
| fill_targets_skipped | 1 |
| targets_skipped_undecided | 1 |
| clone_units_chosen | 1437 |
| clone_units_cloned | 1379 |
| clone_units_dropped | 58 |
| clone_units_existing | 0 |
| clone_units_failed | 0 |
| clone_units_outside_window | 0 |
| clone_bytes_not_shared | 2576793600 |
| store_bytes | 1290018816 |

`unmeasured` is empty. One finding: `source.skipped` because liveness was undecided on `feat-pinwin-lane1/target`.

## Cargo

`cargo build --offline --locked` exited 0. Stderr has no line that starts with `error`. `target/klynt-run/cargo-counts.json` sha256 `2e0ce1c60975a997e9154c511ae95b3adb8cac9b9f3e409f86f9b331d28b81eb`.

| Count | Value |
|---|---|
| registry_fresh | 520 |
| registry_recompiled | 16 |
| other_fresh | 0 |
| other_recompiled | 32 |
| cloned_recompiled | empty |

The 16 registry artifacts that were not fresh did not match a cloned unit hash. `other_recompiled` 32 is the workspace. Neither Cargo's fresh count nor these 16 is the pass mark. The pass mark is an empty `cloned_recompiled` list.

## Commands after the build

In the same worktree, with the same env scrub and with `CARGO_TARGET_DIR` unset. Each used `--message-format=json-render-diagnostics`. The library package was the first workspace member with `src/lib.rs`, `base`. The summary keys are `cargo-check`, `cargo-clippy`, `cargo-test`, and `cargo-check-lib`. Each `cloned_recompiled` list is empty.

| Command | Exit | cloned_recompiled |
|---|---|---|
| `cargo check --all-targets --offline --locked` | 0 | empty |
| `cargo clippy --all-targets --offline --locked` | 0 | empty |
| `cargo test --offline --locked --no-run` | 0 | empty |
| `cargo check -p base --lib --offline --locked` | 0 | empty |

The count files `target/klynt-run/cargo-check-counts.json`, `cargo-clippy-counts.json`, `cargo-test-counts.json`, and `cargo-check-lib-counts.json` are the same 29 bytes, sha256 `da0cbaadf9fc46a9690ae16b48f95b6e419cbd8e608becbf6e61056db0cecd6b`.

## Status counts, beside spike 7

Status exited 0. The four counts are for `.worktrees/hive-seed-71383/target`:

| Count | Value |
|---|---|
| reused_fresh | 1115 |
| seeded_unneeded | 263 |
| seeded_recompiled | 1 |
| seeded_missing | 0 |

`seeded_recompiled` 1 is what status reported. It is not a reason to change the empty `cloned_recompiled` lists.

Spike 7, beside those numbers and not as a pass mark: 466 needed and 31 compiled. Cargo's 520 fresh registry artifacts are a different count from `reused_fresh`.
