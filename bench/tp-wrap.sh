#!/bin/bash
# RUSTC_WRAPPER: add -Ztime-passes for the persistence lib only.
rustc=$1; shift
for a in "$@"; do [[ $a == persistence ]] && { exec "$rustc" "$@" -Ztime-passes 2>>/tmp/klynt-bench/tp-incr.txt; }; done
exec "$rustc" "$@"
