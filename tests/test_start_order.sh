#!/usr/bin/env bash
# start measures the screen only after undoing what a crashed session left behind (a FIT=match mode, a second
# screen): measured first, a leftover 1920x1080 gave a cropped picture on the TV (2026-10-13).
set -euo pipefail
f="$(dirname "$(readlink -f "$0")")/../src/rokast/src/start.rs"
line() { grep -n "$1" "$f" | head -1 | cut -d: -f1; }
geo="$(line 'capture::monitor_geometry(')"
for r in 'fit::recover(' 'extend::recover('; do
    [ "$(line "$r")" -lt "$geo" ] || { echo "test_start_order: FAIL $r must come before monitor_geometry"; exit 1; }
done
echo "test_start_order: ok"
