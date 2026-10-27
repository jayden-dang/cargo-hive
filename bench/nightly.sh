#!/bin/bash
# Nightly experiments: baseline vs parallel frontend vs cranelift, clean + incremental.
set -u
export SQLX_OFFLINE=true
cd /tmp/klynt-bench/src/backend || exit 1
t() { python3 -c 'import time;print(time.time())'; }

run() { # name target_dir rustflags [extra cargo args...]
  local name=$1 dir=$2 rf=$3; shift 3
  local s=$(t)
  RUSTFLAGS="$rf" CARGO_TARGET_DIR=$dir cargo +nightly build --locked --bin server "$@" -q 2>/tmp/klynt-bench/err-$name.log || { echo "$name FAILED"; tail -5 /tmp/klynt-bench/err-$name.log; return; }
  local e=$(t)
  printf '%-14s clean %6.1fs  size %s\n' "$name" "$(echo "$e - $s" | bc)" "$(du -sh $dir | cut -f1)"
  TC=+nightly RUSTFLAGS="$rf" /tmp/klynt-bench/incr.sh "$dir" 2>&1 | sed "s/^/   $name incr /"
}

run baseline  /tmp/klynt-bench/n-base ""
run threads8  /tmp/klynt-bench/n-thr  "-Zthreads=8"
run cranelift /tmp/klynt-bench/n-clif "-Zcodegen-backend=cranelift"
run clif+thr8 /tmp/klynt-bench/n-both "-Zcodegen-backend=cranelift -Zthreads=8"

echo "== time-passes persistence (baseline dir)"
touch crates/infra/persistence/src/lib.rs
CARGO_TARGET_DIR=/tmp/klynt-bench/n-base CARGO_INCREMENTAL=0 cargo +nightly rustc --locked -p persistence --lib -q -- -Ztime-passes 2>&1 \
  | grep -E '^time:' | sort -t: -k2 -rn | awk '{print}' | head -40 > /tmp/klynt-bench/timepasses.txt
echo done
