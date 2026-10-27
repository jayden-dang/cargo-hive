#!/bin/bash
# Incremental rebuild benchmark: append a private fn to a file, rebuild the
# server binary, report wall time, restore the file. Usage: incr.sh TARGET_DIR [extra cargo args]
set -u
export SQLX_OFFLINE=true CARGO_TARGET_DIR="$1"; shift
cd /tmp/klynt-bench/src/backend || exit 1
cargo ${TC:-} build --locked --bin server "$@" -q 2>/dev/null   # warm
for f in src/main.rs:server \
         crates/gateways/src/lib.rs:gateways \
         crates/infra/persistence/src/health.rs:persistence \
         crates/shared/domain/src/lib.rs:domain; do
  path=crates/server/${f%%:*}; [[ $f == crates/* ]] && path=${f%%:*}
  name=${f##*:}
  cp "$path" /tmp/klynt-bench/orig.rs
  printf '\n#[allow(dead_code)]\nfn __bench_probe_%s() -> u32 { %s }\n' "$RANDOM" "$RANDOM" >> "$path"
  s=$(date +%s.%N 2>/dev/null || python3 -c 'import time;print(time.time())')
  s=$(python3 -c 'import time;print(time.time())')
  cargo ${TC:-} build --locked --bin server "$@" -q 2>/dev/null
  e=$(python3 -c 'import time;print(time.time())')
  printf '%-12s %6.1fs\n' "$name" "$(echo "$e - $s" | bc)"
  cp /tmp/klynt-bench/orig.rs "$path"
  cargo ${TC:-} build --locked --bin server "$@" -q 2>/dev/null
done
