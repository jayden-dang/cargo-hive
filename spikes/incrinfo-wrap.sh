#!/bin/bash
# Chain to remap-wrap.sh, adding -Zincremental-info for the crate in $INFO_CRATE.
for a in "$@"; do [[ $a == "$INFO_CRATE" ]] && { RUSTC_BOOTSTRAP=1 exec "$(dirname "$0")/remap-wrap.sh" "$@" -Zincremental-info 2>>/tmp/hive/incrinfo.txt; }; done
exec "$(dirname "$0")/remap-wrap.sh" "$@"
