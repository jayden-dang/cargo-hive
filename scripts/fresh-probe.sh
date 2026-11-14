#!/usr/bin/env bash
# Edit-then-build freshness probe, run after packaging (release steps 3 and 6).
#
# Appends a comment to one lib source file, runs `cargo build -v`, puts the file back, and
# says whether Cargo recompiled the lib. A packaging verify build that shares this target
# dir can leave the lib's fingerprint pointing at target/package/<name>-<ver>/src, so later
# edits under src/ are ignored and builds report Fresh (docs/specs/fixes.md, FIX-1).
#
# usage: scripts/fresh-probe.sh [<source-file> <package>]...
#        (default: crates/cargo-hive/src/lib.rs cargo-hive)
# exit:  0 every pair recompiled (GREEN), 1 any reported Fresh (RED), 2 anything else
set -euo pipefail

if (($# == 0)); then
  set -- crates/cargo-hive/src/lib.rs cargo-hive
fi
if (($# % 2 != 0)); then
  echo "usage: $0 [<source-file> <package>]..." >&2
  exit 2
fi

probe_one() {
  local file="$1" pkg="$2"
  local backup out
  backup="$(mktemp)"
  cp -p "$file" "$backup"
  # Restore without -p: a fresh mtime makes the next build recompile the original source,
  # instead of keeping the probe's build as current.
  trap 'cp "$backup" "$file"; rm -f "$backup"' RETURN
  echo "// fresh-probe $(date +%s)" >> "$file"
  out="$(cargo build -v 2>&1)" || { echo "$out" | tail -5; return 2; }
  if grep -qE "Compiling $pkg v" <<<"$out"; then
    echo "GREEN: $pkg recompiled after an edit to $file"
    return 0
  fi
  if grep -qE "Fresh $pkg v" <<<"$out"; then
    echo "RED: $pkg reported Fresh after an edit to $file"
    return 1
  fi
  echo "$out" | tail -5
  return 2
}

status=0
while (($# > 0)); do
  file="$1"
  pkg="$2"
  shift 2
  set +e
  probe_one "$file" "$pkg"
  rc=$?
  set -e
  if ((rc != 0)); then
    status=$rc
    break
  fi
done
exit "$status"
