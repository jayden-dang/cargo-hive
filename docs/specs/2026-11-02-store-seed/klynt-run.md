# Klynt seed run

Date: 2026-11-03

The run passed. `cargo build --offline --locked` exited 0 and `cloned_recompiled` was empty. The home comparison was `Ok`. The worktree and the scratch home were removed.

## Command

```
HIVE_SEED_CHECKOUT=/Users/jayden/Developer/klynt/klynt cargo test --test seed --offline -- --ignored seeding_a_new_worktree_of_a_real_repo_recompiles_no_cloned_unit
```

Exit code: 0. The test ran 141.59s. Seed 61s, build 71s, status 2s.

Hive: `cfb97e297a43b6428e18dd2167841848c90878d7`. Checkout: `/Users/jayden/Developer/klynt/klynt` at `5b5a8a2219eb0c60430c6829ab4307cc6465a295`. The test added a detached worktree at `.worktrees/hive-seed-52989` and removed it. `git worktree list` in klynt does not name it.

## Seed receipt

Receipt sha256: `310b8a279728cce6946c17cdcb3376e4604f9a0ad07330f78e724ab495c675be` (`target/klynt-run/seed-receipt.json`, not committed).

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

## Status counts, beside spike 7

Status exited 0. The four counts are for `.worktrees/hive-seed-52989/target`:

| Count | Value |
|---|---|
| reused_fresh | 570 |
| seeded_unneeded | 809 |
| seeded_recompiled | 0 |
| seeded_missing | 0 |

Spike 7, beside those numbers and not as a pass mark: 466 needed and 31 compiled. Cargo's 520 fresh registry artifacts are a different count from `reused_fresh`.

## Home

`~/.cargo/.global-cache` stayed at mtime `1793675342995573416`, which the check may allow to move along with the `~/.cargo` directory, and the allowed `p10k-` cache files `p10k-dump-jayden.zsh`, `p10k-dump-jayden.zsh.zwc`, `p10k-instant-prompt-jayden.zsh`, `p10k-instant-prompt-jayden.zsh.zwc`, and `p10k-jayden` did not change.

## Commands after the seed

Date: 2026-11-03. Hive `78e1f3620c52b5cfda183e51f395b9d52b9cf6cd`. Checkout `/Users/jayden/Developer/klynt/klynt` at `0bdae577810ed2634874443a27a636fa03ee4e83`. The same ignored test, after `cargo build --offline --locked` exited 0, ran four more commands in that worktree. Each used `--message-format=json-render-diagnostics` so `cloned_recompiled` is read from compiler-artifact lines. `CARGO_TARGET_DIR` stayed unset. The library package was the first workspace member with `src/lib.rs`, `base`.

| Command | Exit | cloned_recompiled |
|---|---|---|
| `cargo check --all-targets --offline --locked` | 0 | empty |
| `cargo clippy --all-targets --offline --locked` | 0 | empty |
| `cargo test --offline --locked --no-run` | 0 | empty |
| `cargo check -p base --lib --offline --locked` | 0 | empty |

The count files `target/klynt-run/cargo-check-counts.json`, `cargo-clippy-counts.json`, `cargo-test-counts.json`, and `cargo-check-lib-counts.json` are the same 29 bytes, sha256 `da0cbaadf9fc46a9690ae16b48f95b6e419cbd8e608becbf6e61056db0cecd6b`. `summary.json` sha256 `02acdad458923080d27325cf3e57fb2c88e5272f093d09ceef99e167437aa1c8`. The test finished in 424.23s. Seed 116s, build 100s, status 3s. The home comparison passed. The worktree `.worktrees/hive-seed-71383` was removed.
