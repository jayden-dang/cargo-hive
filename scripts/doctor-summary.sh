#!/usr/bin/env bash
# Builds cargo-hive and prints `hive doctor --json` as Markdown on stdout (CI appends it to the
# job summary). Reports capabilities; never fails because one is unavailable.
#
# usage: scripts/doctor-summary.sh >> "$GITHUB_STEP_SUMMARY"
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

scratch="$(mktemp -d "${RUNNER_TEMP:-/private/tmp}/hive-doctor.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT

cargo build -p cargo-hive >&2
export HIVE_HOME="$scratch/home"

status=0
json="$(target/debug/cargo-hive hive doctor --json 2>"$scratch/stderr")" || status=$?

echo "### cargo hive doctor"
echo
echo "Exit status: \`$status\` (an unavailable capability does not fail the job)"
echo
echo '```json'
echo "$json"
echo '```'
if [[ -s "$scratch/stderr" ]]; then
  echo
  echo "stderr:"
  echo '```'
  cat "$scratch/stderr"
  echo '```'
fi
