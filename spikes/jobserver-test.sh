#!/bin/bash
# Spike 3: two concurrent clean builds in two worktrees, with or without one
# shared GNU-make fifo jobserver. Samples the number of live rustc processes.
# Usage: jobserver-test.sh none|TOKENS
set -u
S=$(dirname "$0"); mode=$1
for w in wt-e wt-f; do rm -rf /private/tmp/hive/$w/backend/target; done
holder=
if [[ $mode != none ]]; then
  fifo=/tmp/hive/jobserver.fifo; rm -f $fifo; mkfifo $fifo
  # Holder keeps the fifo open (so tokens survive) and preloads them.
  python3 -I -c "import os,time,sys; fd=os.open(sys.argv[1], os.O_RDWR); os.write(fd, b'+'*int(sys.argv[2])); time.sleep(10**6)" $fifo $mode &
  holder=$!
  export MAKEFLAGS="-j --jobserver-auth=fifo:$fifo" CARGO_MAKEFLAGS="-j --jobserver-auth=fifo:$fifo"
fi
t() { python3 -c 'import time;print(time.time())'; }
s=$(t)
$S/hb.sh wt-e & p1=$!
$S/hb.sh wt-f & p2=$!
max=0; sum=0; n=0
while kill -0 $p1 2>/dev/null || kill -0 $p2 2>/dev/null; do
  c=$(pgrep -f '^/Users/jayden/.rustup/toolchains/.*/rustc .*private/tmp/hive' | wc -l | tr -d ' ')
  ((c > max)) && max=$c; sum=$((sum + c)); n=$((n + 1)); sleep 0.25
done
wait $p1 $p2
e=$(t)
echo "mode=$mode: both done in $(echo "$e-$s" | bc)s; rustc processes max=$max avg=$(echo "scale=1;$sum/$n" | bc)"
grep -ihE 'jobserver.*(warn|error|fail)' /tmp/hive/last-wt-e.log | head -2
[[ -n $holder ]] && kill $holder 2>/dev/null
exit 0
