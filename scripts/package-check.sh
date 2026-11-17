#!/usr/bin/env bash
# Packaged-build check (release step 3): every member must publish only the allowed files, and
# `cargo publish --workspace --dry-run` must build from the packaged sources, so a file missing
# from a package fails here instead of after the release.
#
# usage: scripts/package-check.sh
# exit:  0 packages are clean and the dry-run built every member; 1 otherwise
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

allowed='^(src/.+|Cargo\.toml|Cargo\.toml\.orig|\.cargo_vcs_info\.json|Cargo\.lock|README\.md|LICENSE-MIT|LICENSE-APACHE)$'

listing="$(cargo package --workspace --list --allow-dirty)"
bad="$(grep -Ev "$allowed" <<<"$listing" || true)"
if [[ -n "$bad" ]]; then
  echo "package check: paths outside the allowed set:" >&2
  echo "$bad" >&2
  exit 1
fi
echo "package check: every listed path is allowed ($(wc -l <<<"$listing" | tr -d ' ') entries)"

# Never the repo's own target/ (FIX-1).
scratch="$(mktemp -d "${RUNNER_TEMP:-/private/tmp}/hive-package-verify.XXXXXX")"
trap 'rm -rf "$scratch"' EXIT
CARGO_TARGET_DIR="$scratch/target" cargo publish --workspace --dry-run --allow-dirty
