#!/usr/bin/env bash
# The merge gate: the single source of the four commands every merge needs (docs/standards/testing.md).
# CI (.github/workflows/ci.yml) and humans both run this file, so they cannot drift.
#
# usage: scripts/gate.sh
# exit:  0 every command passed; otherwise the exit code of the first command that failed
# env:   HIVE_HOME is set to a fresh scratch dir under $RUNNER_TEMP (else /private/tmp), never ~/.hive
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

# The tests build fixtures with Cargo's defaults and need its incremental sessions (gc's auto
# tier deletes superseded ones). CI caches such as Swatinem/rust-cache export CARGO_INCREMENTAL=0.
unset CARGO_INCREMENTAL CARGO_BUILD_INCREMENTAL

scratch_root="${RUNNER_TEMP:-/private/tmp}"
HIVE_HOME="$(mktemp -d "$scratch_root/hive-gate.XXXXXX")"
export HIVE_HOME
trap 'rm -rf "$HIVE_HOME"' EXIT

run() {
  echo "::group::$*"
  echo "+ $*"
  "$@"
  echo "::endgroup::"
}

run cargo fmt --check
run cargo clippy --workspace --all-targets -- -D warnings
run cargo test --workspace
run cargo check --workspace --target x86_64-unknown-linux-gnu

echo "gate: all four commands passed"
