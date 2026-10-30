# Closure liveness: proof runs on a real target

Status: pending owner run. No results are recorded yet.

## Command (klynt, nothing building in it)

```
HIVE_PROVE_TARGET=~/Developer/klynt/klynt/target \
HIVE_PROVE_CHECKOUT=~/Developer/klynt/klynt \
HIVE_PROVE_CMDS='[["build","--offline"],["check","--all-targets","--offline"],["test","--no-run","--offline"]]' \
cargo test --release --test liveness_prove -- --ignored --nocapture
```

Add `["clippy","--all-targets","--offline"]` only if it exits 0 on klynt first.

## Results

Date, commands, each window's report and elapsed time: pending owner run.
