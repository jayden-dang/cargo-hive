#!/bin/bash
# Spike 4: chain to slot-wrap.sh, injecting the open() tracer into rustc for $TRACE_CRATE.
for ((i=1; i<=$#; i++)); do
  [[ ${!i} == --crate-name ]] && { j=$((i+1)); [[ ${!j} == "$TRACE_CRATE" ]] && export HIVE_INJECT=$(dirname "$0")/libtrace_open.dylib HIVE_TRACE_FILE=/tmp/hive/trace-$TRACE_CRATE.txt; }
done
exec "$(dirname "$0")/slot-wrap.sh" "$@"
