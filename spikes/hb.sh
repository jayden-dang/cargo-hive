#!/bin/bash
# hb.sh WORKTREE [cargo args...]: build in /private/tmp/hive/WORKTREE via slot-wrap.sh,
# print wall time and compiled-unit count.
w=$1; shift
export HIVE_SLOTS="$(echo /private/tmp/hive/wt-{a,b,c,d,e,f})"
cd /private/tmp/hive/$w/backend || exit 1
s=$(python3 -c 'import time;print(time.time())')
n=$(SQLX_OFFLINE=true CARGO_TARGET_DIR=/private/tmp/hive/$w/backend/target RUSTC_WRAPPER=$(dirname "$0")/${HIVE_WRAPPER:-slot-wrap.sh} \
    cargo +1.97.1 build --locked --bin server "$@" 2>&1 | tee /tmp/hive/last-$w.log | grep -cE '^\s+Compiling')
e=$(python3 -c 'import time;print(time.time())')
printf '%-6s %6.1fs  %3d units compiled\n' "$w" "$(echo "$e-$s"|bc)" "$n"
