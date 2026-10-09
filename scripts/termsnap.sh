#!/bin/bash
# Run the terminal off-screen, follow a script of steps, save PNG + text
# snapshots and memory numbers. Needs: cargo build --release --features selftest
#
#   scripts/termsnap.sh <out-dir> <start-dir> '<steps>'|@steps-file ['<command>']
#
# Steps, one per line:    wait <secs> | keys <text> (\r \e \t escapes)
#                           | resize <cols>x<rows> | snap <name> | mem | quit
# Without a command the pane runs your login shell.
set -euo pipefail
cd "$(dirname "$0")/.."
out=$1 dir=$2 steps=$3 cmd=${4:-}
case $steps in @*) steps=$(cat "${steps#@}");; esac
rm -rf "$out"
mkdir -p "$out"
if [ -n "$cmd" ]; then export ODEK_TERM_CMD="$cmd"; fi
ODEK_TERM_SNAP="$out" ODEK_TERM_STEPS="$steps" ./target/release/odek --term "$dir" > "$out/log.txt" 2>&1 &
pid=$!
# Measure the whole process (what Activity Monitor shows) while it runs.
( while kill -0 "$pid" 2>/dev/null; do
    footprint "$pid" 2>/dev/null | grep -m1 Footprint >> "$out/footprint.txt" || true
    sleep 1
  done ) &
wait "$pid" || true
cat "$out/log.txt"
[ -s "$out/footprint.txt" ] && echo "footprint samples: $(sed -E 's/.*Footprint: //; s/ \(.*//' "$out/footprint.txt" | tr '\n' ' ')"
ls "$out"
