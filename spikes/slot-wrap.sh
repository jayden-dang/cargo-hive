#!/bin/bash
# Spike 2 RUSTC_WRAPPER: identical rustc command lines in every worktree.
# rustc tracks --remap-path-prefix for incremental reuse, so per-worktree
# prefixes make a cloned incremental cache useless. Instead every invocation
# remaps ALL slots in $HIVE_SLOTS (fixed, canonical worktree roots), so the
# argument list never depends on which worktree is building. Roots first,
# target dirs last: rustc applies the last matching rule.
#
# rustc also tracks its working directory for incremental reuse, and Cargo runs
# it from the workspace root. So relative path arguments are made absolute and
# rustc runs from a fixed directory. --remap-path-scope=object keeps real paths
# in diagnostics (clickable errors) and remaps only what lands in artifacts.
rustc=$1; shift
abs=()
for a in "$@"; do
  if [[ $a == *.rs && $a != /* && -f $a ]]; then abs+=("$PWD/$a"); else abs+=("$a"); fi
done
set -- "${abs[@]}"
cd "${HIVE_NEUTRAL_CWD:-/}" || exit 1
args=(${HIVE_REMAP_SCOPE:+--remap-path-scope=$HIVE_REMAP_SCOPE})
# Workspace root -> "" reproduces stock Cargo's relative names (crates/x/src/lib.rs)
# in diagnostics, file!() and panics; the repo root -> "../" covers files outside
# the workspace (include_str! of ../contracts). The target dir sits under the
# workspace root, so it needs no rule of its own.
for r in $HIVE_SLOTS; do args+=("--remap-path-prefix=$r/=../"); done
for r in $HIVE_SLOTS; do args+=("--remap-path-prefix=$r/backend/="); done
name=; is_pm=
for ((i=1; i<=$#; i++)); do
  a=${!i}
  [[ $a == --crate-type ]] && { j=$((i+1)); [[ ${!j} == proc-macro ]] && is_pm=1; }
  [[ $a == --crate-name ]] && { j=$((i+1)); name=${!j}; }
done
if [[ -n $is_pm && $(uname) == Darwin ]]; then
  args+=("-Clink-arg=-Wl,-install_name,@rpath/lib$name.dylib" "-Clink-arg=-Wl,-oso_prefix,$CARGO_TARGET_DIR")
fi
# Audit mode, invisible to Cargo's fingerprints. rustc tracks unstable-feature
# availability, so sessions to be audited must also be created with HIVE_BOOTSTRAP=1.
[[ -n ${HIVE_BOOTSTRAP:-} ]] && export RUSTC_BOOTSTRAP=1
[[ -n ${HIVE_EXTRA:-} ]] && args+=($HIVE_EXTRA)
if [[ -n ${HIVE_LOG_ARGS:-} && $name == "$HIVE_LOG_ARGS" ]]; then
  tag=$(echo "$CARGO_TARGET_DIR" | md5 | cut -c1-6)
  printf '%s\n' "$rustc" "$@" "${args[@]}" > "/tmp/hive/args-$name-$tag.txt"
  env -0 > "/tmp/hive/env-$name-$tag.bin"
fi
# Diagnostics for one crate. stderr is tee'd, not swallowed: Cargo reads rustc's
# JSON artifact notifications from it to drive pipelining.
if [[ -n ${INFO_CRATE:-} && $name == "$INFO_CRATE" ]]; then
  RUSTC_BOOTSTRAP=1 "$rustc" "$@" "${args[@]}" -Zincremental-info -Ztime-passes 2> >(tee -a /tmp/hive/incrinfo.txt >&2)
  exit $?
fi
# DYLD_* is stripped when exec'ing SIP-protected binaries such as /bin/bash, so
# an injection request travels as HIVE_INJECT and becomes DYLD_INSERT_LIBRARIES
# only for the final exec of rustc.
[[ -n ${HIVE_INJECT:-} ]] && export DYLD_INSERT_LIBRARIES=$HIVE_INJECT
exec "$rustc" "$@" "${args[@]}"
