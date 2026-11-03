# Klynt seed run

Date: 2026-11-03

The run passed. `cargo build --offline --locked` exited 0 and `cloned_recompiled` was empty. The worktree and the scratch home were removed.

## Command

```
HIVE_SEED_CHECKOUT=/Users/jayden/Developer/klynt/klynt cargo test --test seed --offline -- --ignored seeding_a_new_worktree_of_a_real_repo_recompiles_no_cloned_unit
```

Exit code: 0. The test ran 148.95s. Seed 63s, build 77s, status 2s.

Hive: `772a6742c8fa63a09277c95c90dfac9033e232b2`. Checkout: `/Users/jayden/Developer/klynt/klynt` at `7e63d129a4364d15313d5141c660d62b39f6f73f`. The test added a detached worktree at `.worktrees/hive-seed-53741` and removed it. `git worktree list` in klynt does not name it.

## Seed receipt

Receipt sha256: `619f4e92b26ee6b43adafec26808a285dd49c6fbe10a9e4a107800870de96a46` (`target/klynt-run/seed-receipt.json`, not committed).

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

`cargo build --offline --locked` exited 0. Stderr has no `error` line. `target/klynt-run/cargo-counts.json` sha256 `2e0ce1c60975a997e9154c511ae95b3adb8cac9b9f3e409f86f9b331d28b81eb`.

| Count | Value |
|---|---|
| registry_fresh | 520 |
| registry_recompiled | 16 |
| other_fresh | 0 |
| other_recompiled | 32 |
| cloned_recompiled | empty |

The 16 registry artifacts that were not fresh did not match a cloned unit hash: `libwebp-sys` 0.9.6, `webp` 0.3.1, `aws-lc-rs` 1.18.0, `rustls-webpki` 0.103.15, `rustls-platform-verifier` 0.7.0, `rustls` 0.23.45, `tokio-rustls` 0.26.4, `hyper-rustls` 0.27.9, `sqlx-core` 0.9.0, `reqwest` 0.13.4, `opentelemetry-http` 0.32.0, `opentelemetry-otlp` 0.32.0, `sqlx-postgres` 0.9.0, `sqlx` 0.9.0, `jsonschema` 0.54.0, `rig-core` 0.42.0. `other_recompiled` 32 is the workspace. Neither Cargo's fresh count nor these 16 is the pass mark. The pass mark is an empty `cloned_recompiled` list.

## Status counts, beside spike 7

Status exited 0. The four counts are for `.worktrees/hive-seed-53741/target`:

| Count | Value |
|---|---|
| reused_fresh | 570 |
| seeded_unneeded | 809 |
| seeded_recompiled | 0 |
| seeded_missing | 0 |

Spike 7, beside those numbers and not as a pass mark: 466 needed and 31 compiled. Cargo's 520 fresh registry artifacts are a different count from `reused_fresh`.

## Locks 5, 7, and 13

None of the three reopen triggers is shown by this run.

- Lock 5. 53 name-metas have more than one fingerprint (`summary.json`). The cloned units did not recompile, and `seeded_recompiled` is 0. `clone_units_dropped` is 58 on this one seed, not a non-zero dropped count across many seeds.
- Lock 7. Fill and clone took 63s and the build 77s. Status took 2s. That is not a scan cost that swamps the build saved. No concurrent build was seen waiting on a source lock.
- Lock 13. `seeded_recompiled` 0 agrees with the empty `cloned_recompiled` list. Status was not slower because of this branch. The disk counts and Cargo's artifact counts are different definitions; they are recorded above, not scored against each other.
