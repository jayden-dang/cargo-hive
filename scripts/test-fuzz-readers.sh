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
calls=$(wc -l < "$FAKE_CALLS")
if [[ $FAKE_PHASE == replay || $calls -eq 2 ]]; then
  printf 'retained input' > "${artifact}synthetic-failure"
  exit 42
fi
echo 'stat::number_of_executed_units: 1'
echo 'stat::peak_rss_mb: 1'
exit 0
SH
chmod +x "$scratch/cargo-fuzz"

for phase in replay mutation; do
  export FAKE_PHASE=$phase FAKE_CALLS="$scratch/$phase-calls"
  expect_failure 42 env CARGO_FUZZ="$scratch/cargo-fuzz" FUZZ_REPORT_DIR="$scratch/$phase" \
    "$runner" 1 fingerprint
  [[ -s "$scratch/$phase/fingerprint/artifacts/synthetic-failure" ]]
  grep -q "$phase.*42" "$scratch/$phase/fingerprint/status.tsv"
  if [[ $phase == replay ]]; then [[ $(wc -l < "$FAKE_CALLS") -eq 1 ]]; fi
done
echo 'runner controls: passed'
