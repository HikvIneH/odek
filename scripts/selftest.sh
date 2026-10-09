#!/bin/bash
# Drive the UI off-screen and save PNG snapshots + memory footprint.
# Usage: scripts/selftest.sh <binary> <project-dir> <out-dir> <query> <file>...
# The binary must be built with: cargo build --release --features selftest
set -euo pipefail
bin=$1 project=$2 out=$3 query=$4
shift 4
files=$(IFS=,; echo "$*")
rm -rf "$out"
mkdir -p "$out"
SELFTEST_DIR="$out" SELFTEST_QUERY="$query" SELFTEST_FILES="$files" "$bin" --viewer "$project" > "$out/log.txt" 2>&1 &
pid=$!
# The test prints "done" then idles 4 s: measure memory in that window.
for _ in $(seq 1 60); do
  if grep -q "SELFTEST pid=" "$out/log.txt" 2>/dev/null; then
    footprint "$pid" > "$out/footprint.txt" 2>&1 || true
    grep -E "Footprint|phys_footprint_peak" "$out/footprint.txt" >> "$out/log.txt" || true
    break
  fi
  sleep 0.5
done
wait "$pid" || true
cat "$out/log.txt"
