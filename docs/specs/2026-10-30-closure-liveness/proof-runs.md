# Closure liveness: proof runs on a real target

Status: pending owner run. No results are recorded yet.

## Command (klynt, nothing building in it)

```
HIVE_PROVE_TARGET=~/Developer/klynt/klynt/target \
HIVE_PROVE_CHECKOUT=~/Developer/klynt/klynt \
HIVE_PROVE_CMDS='[["build","--offline"],["check","--all-targets","--offline"],["test","--no-run","--offline"]]' \
cargo test --release --test liveness_prove -- --ignored --nocapture
```

The replay accepts only `build`, `check`, `test`, `doc`, `bench` and `run` by their long names; any other command is refused.

## Results

Date, commands, each window's report and elapsed time: pending owner run.
