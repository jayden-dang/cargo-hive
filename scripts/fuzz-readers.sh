#!/usr/bin/env bash
# Replays synthetic seeds and runs bounded compiler-reader campaigns.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
root=$PWD
usage() { echo 'usage: scripts/fuzz-readers.sh <1..86400 seconds> [registered_target]' >&2; exit 2; }
[[ $# -ge 1 && $# -le 2 ]] || usage
seconds=$1
case "$seconds" in ''|*[!0-9]*) usage ;; esac
[[ ${#seconds} -le 5 ]] || usage
seconds=$((10#$seconds))
[[ $seconds -ge 1 && $seconds -le 86400 ]] || usage
selection=${2:-}
targets=()
while IFS= read -r target; do
  [[ -n $target ]] || continue
  case "$target" in *[!a-z_]*|'') echo 'invalid target registry' >&2; exit 2 ;; esac
  if [[ -z $selection || $selection == "$target" ]]; then targets+=("$target"); fi
done < fuzz/targets.txt
[[ ${#targets[@]} -gt 0 ]] || usage

deadline=${FUZZ_RUN_TIMEOUT:-$((600 + ${#targets[@]} * (seconds + 40)))}
case "$deadline" in ''|*[!0-9]*) usage ;; esac
[[ ${#deadline} -le 7 ]] || usage
deadline=$((10#$deadline))
[[ $deadline -ge 1 && $deadline -le 1000000 ]] || usage
if [[ -n ${FUZZ_REPORT_DIR:-} ]]; then
  [[ ! -e $FUZZ_REPORT_DIR && ! -L $FUZZ_REPORT_DIR ]] || { echo 'report directory already exists; choose a new run directory' >&2; exit 2; }
  mkdir -p "$(dirname "$FUZZ_REPORT_DIR")"
  mkdir "$FUZZ_REPORT_DIR"
  out=$(cd "$FUZZ_REPORT_DIR" && pwd -P)
else
  mkdir -p fuzz/reports
  out=$(mktemp -d "$root/fuzz/reports/run.XXXXXX")
fi

run_campaigns() {
toolchain=${FUZZ_TOOLCHAIN:-nightly-2026-11-17}
cargo_fuzz=${CARGO_FUZZ:-cargo-fuzz}
[[ $("$cargo_fuzz" --version) == 'cargo-fuzz 0.13.2' ]] || { echo 'install cargo-fuzz 0.13.2 in an isolated tool root' >&2; exit 2; }
tool_info=$(rustup run "$toolchain" rustc -vV) || { echo 'install nightly-2026-11-17 before running campaigns' >&2; exit 2; }
grep -q '^commit-hash: 5df8c02afdf6b882bc301442cea70f026e3d3974$' <<< "$tool_info" || { echo 'fuzz toolchain commit differs from the tested pin' >&2; exit 2; }
grep -q '^host: .*apple-darwin$' <<< "$tool_info" || { echo 'reader campaigns require macOS' >&2; exit 2; }
export RUSTUP_TOOLCHAIN=$toolchain
export CARGO_TERM_COLOR=never
unset RUSTC RUSTDOC RUSTFLAGS CARGO_ENCODED_RUSTFLAGS RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER
unset CARGO_TARGET_DIR CARGO_BUILD_TARGET CARGO_BUILD_BUILD_DIR CARGO_BUILD_RUSTFLAGS

printf '%s\n' "$tool_info" > "$out/toolchain.txt"
{
  printf 'source_commit\t%s\n' "$(git rev-parse HEAD)"
  printf 'source_tree\t%s\n' "$(git rev-parse HEAD^{tree})"
  printf 'cargo_fuzz\t0.13.2\ntoolchain\t%s\nseconds_per_target\t%s\n' "$toolchain" "$seconds"
  printf 'whole_run_timeout_seconds\t%s\n' "$deadline"
  printf 'started_utc\t%s\n' "$(date -u +%FT%TZ)"
  if [[ -n $(git status --porcelain --untracked-files=all) ]]; then printf 'dirty\ttrue\n'; else printf 'dirty\tfalse\n'; fi
} > "$out/run.tsv"
git status --porcelain --untracked-files=all > "$out/source-status.txt"
"$cargo_fuzz" fuzz list --fuzz-dir "$root/fuzz" > "$out/manifest-targets.txt"
sort fuzz/targets.txt > "$out/registry-sorted.txt"
sort "$out/manifest-targets.txt" > "$out/manifest-sorted.txt"
cmp -s "$out/registry-sorted.txt" "$out/manifest-sorted.txt" || { echo 'target registry and fuzz manifest disagree' >&2; exit 2; }
seed=${FUZZ_SEED:-20261123}
case "$seed" in ''|*[!0-9]*) echo 'FUZZ_SEED must be an unsigned integer' >&2; exit 2 ;; esac
[[ ${#seed} -le 9 ]] || { echo 'FUZZ_SEED exceeds its bound' >&2; exit 2; }

run_phase() {
  local phase=$1
  shift
  local start end rc=0
  start=$(date +%s)
  printf '%s\t%s\t%s\n' "$target" "$phase" "$start" > "$out/active-phase.tsv"
  local command=("$cargo_fuzz" fuzz run --fuzz-dir "$root/fuzz" --target-dir "$root/fuzz/target" "$target" "$@")
  {
    printf 'RUSTUP_TOOLCHAIN=%q ' "$RUSTUP_TOOLCHAIN"
    printf '%q ' "${command[@]}"
    printf '\n'
  } > "$dir/$phase.command"
  "${command[@]}" < /dev/null > "$dir/$phase.log" 2>&1 || rc=$?
  end=$(date +%s)
  printf '%s\t%s\t%s\n' "$phase" "$rc" "$((end-start))" >> "$dir/status.tsv"
  : > "$out/active-phase.tsv"
  if [[ $rc -ne 0 ]]; then
    echo "$target $phase failed ($rc); evidence: $dir" >&2
    if ! find "$dir/artifacts" -type f -print | head -1 | grep -q .; then
      printf '%s\tno input artifact produced; reproduction incomplete\n' "$phase" >> "$dir/status.tsv"
    fi
  fi
  return "$rc"
}

echo "reader campaign evidence: $out"
for target in "${targets[@]}"; do
  dir="$out/$target"
  mkdir -p "$dir/corpus" "$dir/artifacts" "$dir/home"
  export HIVE_HOME="$dir/home"
  seeds=()
  for input in "$root/fuzz/corpus/$target"/*; do
    [[ -f $input && ! -L $input ]] || { echo "missing regular seed for $target" >&2; exit 2; }
    size=$(wc -c < "$input")
    [[ $size -gt 0 && $size -le 65536 ]] || { echo "seed size outside replay bounds for $target" >&2; exit 2; }
    seeds+=("$input")
    cp "$input" "$dir/corpus/"
  done
  flags=(-timeout=5 -rss_limit_mb=2048 -max_len=65536 "-seed=$seed" -print_final_stats=1 "-artifact_prefix=$dir/artifacts/")
  echo "$target: replaying ${#seeds[@]} seed file(s)"
  run_phase replay "${seeds[@]}" -- "${flags[@]}" || exit $?
  echo "$target: running ${seconds}s"
  run_phase mutation "$dir/corpus" -- "${flags[@]}" "-max_total_time=$seconds" || exit $?
  if ! grep -E 'stat::(number_of_executed_units|peak_rss_mb)|cov:' "$dir/mutation.log" > "$dir/statistics.txt"; then
    printf 'engine statistics unavailable; inspect raw phase log\n' > "$dir/statistics.txt"
  fi
  echo "$target: passed bounded campaign"
done
printf 'completed_utc\t%s\n' "$(date -u +%FT%TZ)" >> "$out/run.tsv"
echo 'all registered campaigns completed'
}

stop_group() {
  if kill -0 -- "-$1" 2>/dev/null; then
    kill -TERM -- "-$1" 2>/dev/null || true
    sleep 1
    kill -KILL -- "-$1" 2>/dev/null || true
  fi
}

finish() {
  local status=$1
  trap '' INT TERM
  stop_group "$campaign_pid"
  stop_group "$watchdog_pid"
  wait "$campaign_pid" 2>/dev/null || true
  wait "$watchdog_pid" 2>/dev/null || true
  if [[ $status -eq 124 || $status -eq 130 ]]; then
    printf 'supervisor_exit\t%s\n' "$status" >> "$out/run.tsv"
    if [[ -s $out/active-phase.tsv ]]; then
      local target phase start
      IFS=$'\t' read -r target phase start < "$out/active-phase.tsv"
      printf '%s\t%s\t%s\n' "$phase" "$status" "$(($(date +%s)-start))" >> "$out/$target/status.tsv"
      printf '%s\tno input artifact guaranteed after supervisor termination; reproduction incomplete\n' "$phase" >> "$out/$target/status.tsv"
    fi
  fi
  exit "$status"
}

# Monitor mode gives each owned job its own process group on macOS Bash 3.2.
set -m
(set +m; run_campaigns) &
campaign_pid=$!
(
  set +m
  sleep "$deadline"
  : > "$out/run-timeout"
  kill -TERM -- "-$campaign_pid" 2>/dev/null || true
) &
watchdog_pid=$!
set +m
trap 'finish 130' INT TERM
status=0
wait "$campaign_pid" || status=$?
if [[ -f $out/run-timeout ]]; then
  echo "reader campaign exceeded its ${deadline}s whole-run deadline; evidence: $out" >&2
  status=124
fi
finish "$status"
