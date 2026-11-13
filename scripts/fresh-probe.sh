#!/usr/bin/env bash
# Edit-then-build freshness probe, run after packaging (release steps 3 and 6).
#
# Appends a comment to one lib source file, runs `cargo build -v`, puts the file back, and
# says whether Cargo recompiled the lib. A packaging verify build that shares this target
# dir can leave the lib's fingerprint pointing at target/package/<name>-<ver>/src, so later
# edits under src/ are ignored and builds report Fresh (docs/specs/fixes.md, FIX-1).
#
# usage: scripts/fresh-probe.sh [source-file] [package]   (defaults: src/size.rs cargo-hive)
# exit:  0 recompiled (GREEN), 1 reported Fresh (RED), 2 anything else
set -euo pipefail
file="${1:-src/size.rs}"
pkg="${2:-cargo-hive}"
backup="$(mktemp)"
cp -p "$file" "$backup"
# Restore without -p: a fresh mtime makes the next build recompile the original source,
# instead of keeping the probe's build as current.
trap 'cp "$backup" "$file"; rm -f "$backup"' EXIT
echo "// fresh-probe $(date +%s)" >> "$file"
out="$(cargo build -v 2>&1)" || { echo "$out" | tail -5; exit 2; }
if grep -qE "Compiling $pkg v" <<<"$out"; then echo "GREEN: $pkg recompiled after an edit to $file"; exit 0; fi
if grep -qE "Fresh $pkg v" <<<"$out"; then echo "RED: $pkg reported Fresh after an edit to $file"; exit 1; fi
echo "$out" | tail -5; exit 2
