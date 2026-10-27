#!/bin/bash
# Spike 1 RUSTC_WRAPPER: make outputs path-independent across worktrees.
# HIVE_ROOT = worktree root, CARGO_TARGET_DIR = its target dir. Cargo passes
# source paths canonicalized (/private/tmp) but OUT_DIR exactly as configured
# (/tmp), so each prefix is remapped in both spellings. Later rules win, so
# the more specific target-dir rules come last.
rustc=$1; shift
args=()
for p in "$HIVE_ROOT" "$(realpath "$HIVE_ROOT")"; do args+=("--remap-path-prefix=$p=/ws"); done
for p in "$CARGO_TARGET_DIR" "$(realpath "$CARGO_TARGET_DIR")"; do args+=("--remap-path-prefix=$p=/target"); done
# Proc-macro dylibs: ld64 records the output path as the install name and the
# absolute .o paths in the debug map. Neither matters for dlopen-by-path.
name=; is_pm=
for ((i=1; i<=$#; i++)); do
  a=${!i}
  [[ $a == --crate-type ]] && { j=$((i+1)); [[ ${!j} == proc-macro ]] && is_pm=1; }
  [[ $a == --crate-name ]] && { j=$((i+1)); name=${!j}; }
done
if [[ -n $is_pm && $(uname) == Darwin ]]; then
  args+=("-Clink-arg=-Wl,-install_name,@rpath/lib$name.dylib")
  # ld64 honours only the last -oso_prefix; Cargo passes -L/--out-dir in the
  # configured (non-canonical) spelling, so use that one.
  args+=("-Clink-arg=-Wl,-oso_prefix,$CARGO_TARGET_DIR")
fi
exec "$rustc" "$@" "${args[@]}"
