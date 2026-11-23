#!/usr/bin/env bash
# Checks runner refusals and failure retention through the executable boundary.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
runner="$PWD/scripts/fuzz-readers.sh"
if [[ ! -x "$runner" ]]; then
  echo 'FAIL: bounded reader runner is not implemented' >&2
  exit 1
fi
scratch=$(mktemp -d "${TMPDIR:-/private/tmp}/reader-runner-test.XXXXXX")
scratch=$(cd "$scratch" && pwd -P)
export FAKE_ROOT=$scratch
trap 'rm -rf "$scratch"' EXIT

expect_failure() {
  local expected=$1
  shift
  local actual=0
  "$@" > "$scratch/output" 2>&1 || actual=$?
  if [[ $actual != "$expected" ]]; then
    cat "$scratch/output" >&2
    echo "FAIL: expected exit $expected, got $actual" >&2
    exit 1
  fi
}

expect_failure 2 "$runner" 0
expect_failure 2 "$runner" not-seconds
expect_failure 2 "$runner" 9999999999999999999999999999
expect_failure 2 "$runner" 1 ../unknown

cat > "$scratch/git" <<'SH'
#!/usr/bin/env bash
case "$1" in
  rev-parse) echo '0000000000000000000000000000000000000001' ;;
  status) ;;
  *) exit 2 ;;
esac
SH
chmod +x "$scratch/git"
export PATH="$scratch:$PATH"

cat > "$scratch/cargo-fuzz" <<'SH'
#!/usr/bin/env bash
set -eu
if [[ ${1:-} == --version ]]; then
  echo 'cargo-fuzz 0.13.2'
  exit 0
fi
if [[ ${2:-} == list ]]; then
  cat fuzz/targets.txt
  exit 0
fi
printf 'called\n' >> "$FAKE_CALLS"
artifact=
for arg in "$@"; do
  case "$arg" in -artifact_prefix=*) artifact=${arg#-artifact_prefix=} ;; esac
done
[[ -n $artifact && -d $artifact ]] || exit 2
artifact=$(cd "$artifact" && pwd -P)/
case "$artifact" in
  "$FAKE_ROOT"/*/) ;;
  *) echo 'fixture artifact path is outside scratch' >&2; exit 2 ;;
esac
calls=$(wc -l < "$FAKE_CALLS")
if [[ $FAKE_PHASE == timeout && $calls -eq 1 ]]; then
  sleep 8 &
  echo "$!" > "$FAKE_CHILD"
  wait "$!"
  exit 0
fi
if [[ $FAKE_PHASE == replay || $calls -eq 2 ]]; then
  printf 'retained input' > "${artifact}synthetic-failure"
  exit 42
fi
echo 'stat::number_of_executed_units: 1'
echo 'stat::peak_rss_mb: 1'
exit 0
SH
chmod +x "$scratch/cargo-fuzz"

export FAKE_PHASE=replay FAKE_CALLS="$scratch/missing-artifact-calls"
expect_failure 2 /bin/bash -c 'cd "$1"; exec "$2" fuzz run' fixture "$scratch" "$scratch/cargo-fuzz"
[[ ! -e $scratch/synthetic-failure ]]
mkdir -p "$scratch/allowed" "$scratch/outside"
ln -s "$scratch/outside" "$scratch/allowed/link"
for artifact in "$scratch/allowed/../outside/" "$scratch/allowed/link/"; do
  expect_failure 2 env FAKE_ROOT="$scratch/allowed" "$scratch/cargo-fuzz" fuzz run "-artifact_prefix=$artifact"
  [[ ! -e $scratch/outside/synthetic-failure ]]
done

for phase in replay mutation; do
  export FAKE_PHASE=$phase FAKE_CALLS="$scratch/$phase-calls"
  expect_failure 42 env CARGO_FUZZ="$scratch/cargo-fuzz" FUZZ_REPORT_DIR="$scratch/$phase" \
    "$runner" 1 fingerprint
  [[ -s "$scratch/$phase/fingerprint/artifacts/synthetic-failure" ]]
  grep -q "$phase.*42" "$scratch/$phase/fingerprint/status.tsv"
  if [[ $phase == replay ]]; then [[ $(wc -l < "$FAKE_CALLS") -eq 1 ]]; fi
done
export FAKE_PHASE=timeout FAKE_CALLS="$scratch/timeout-calls" FAKE_CHILD="$scratch/timeout-child"
expect_failure 124 env CARGO_FUZZ="$scratch/cargo-fuzz" FUZZ_RUN_TIMEOUT=3 \
  FUZZ_REPORT_DIR="$scratch/timeout" "$runner" 1 fingerprint
grep -q 'replay.*124' "$scratch/timeout/fingerprint/status.tsv"
[[ -s $FAKE_CHILD ]]
if kill -0 "$(cat "$FAKE_CHILD")" 2>/dev/null; then
  echo 'FAIL: phase deadline left its child running' >&2
  exit 1
fi
export FAKE_CALLS="$scratch/cancel-calls" FAKE_CHILD="$scratch/cancel-child"
env CARGO_FUZZ="$scratch/cargo-fuzz" FUZZ_REPORT_DIR="$scratch/cancel" \
  "$runner" 1 fingerprint > "$scratch/cancel-output" 2>&1 &
runner_pid=$!
for ((attempt=0; attempt<200; attempt++)); do
  [[ -s $FAKE_CHILD ]] && break
  sleep 0.05
done
if [[ ! -s $FAKE_CHILD ]]; then
  kill -TERM "$runner_pid" 2>/dev/null || true
  wait "$runner_pid" 2>/dev/null || true
  echo 'FAIL: cancellation fixture did not reach its child' >&2
  exit 1
fi
kill -TERM "$runner_pid"
actual=0
wait "$runner_pid" || actual=$?
[[ $actual -eq 130 ]]
grep -q 'replay.*130' "$scratch/cancel/fingerprint/status.tsv"
if kill -0 "$(cat "$FAKE_CHILD")" 2>/dev/null; then
  echo 'FAIL: cancellation left its child running' >&2
  exit 1
fi
mkdir -p "$scratch/startup/scripts"
ln -s "$PWD/fuzz" "$scratch/startup/fuzz"
awk '
  $0 == "campaign_pid=$!" {
    print "echo \"$!\" > \"$FAKE_STARTUP_GROUP\""
    print "kill -TERM \"$$\""
  }
  { print }
' "$runner" > "$scratch/startup/scripts/fuzz-readers.sh"
chmod +x "$scratch/startup/scripts/fuzz-readers.sh"
actual=0
env CARGO_FUZZ="$scratch/cargo-fuzz" FUZZ_REPORT_DIR="$scratch/startup-report" \
  FAKE_STARTUP_GROUP="$scratch/startup-group" \
  "$scratch/startup/scripts/fuzz-readers.sh" 1 fingerprint > "$scratch/startup-output" 2>&1 || actual=$?
startup_group=$(cat "$scratch/startup-group")
left_running=0
if kill -0 -- "-$startup_group" 2>/dev/null; then
  left_running=1
  kill -TERM -- "-$startup_group" 2>/dev/null || true
  sleep 1
  kill -KILL -- "-$startup_group" 2>/dev/null || true
fi
if [[ $actual -ne 130 || $left_running -ne 0 ]]; then
  echo "FAIL: startup cancellation returned $actual, surviving group $left_running" >&2
  exit 1
fi
echo 'runner controls: passed'
