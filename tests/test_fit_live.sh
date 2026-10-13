#!/usr/bin/env bash
# FIT=match against the real display (Xvfb has no modes to switch), no TV: switch the primary output to 1920x1080,
# check it, SIGTERM -> the original mode and rate are back; SIGKILL -> doctor FAILs, doctor --fix restores.
# Skips unless the primary output is not already 1920x1080 and has that mode. Not run by CI.
set -euo pipefail
repo="$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)"
export DISPLAY="${DISPLAY:-:0}"
cargo build -q --manifest-path "$repo/Cargo.toml" -p rokast-core --example fit_hold
cargo build -q --manifest-path "$repo/Cargo.toml" -p rokast
hold="$repo/target/debug/examples/fit_hold"
current() { xrandr --query | awk '/ connected primary/{p=1;next} /^[^ ]/{p=0} p && /\*/{for(i=2;i<=NF;i++) if($i ~ /\*/){r=$i; gsub(/[*+]/,"",r); print $1, r; exit}}'; }
size() { xrandr --listmonitors | awk 'NR>1 && $2 ~ /\*/ {split($3,a,"[/x+]"); print a[1] "x" a[3]; exit}'; }
before="$(current)"
case "$before" in 1920x1080\ *) echo "test_fit_live: skip (already 1920x1080)"; exit 0 ;; esac
xrandr --query | awk '/ connected primary/{p=1;next} /^[^ ]/{p=0} p' | grep -q '^ *1920x1080 ' \
    || { echo "test_fit_live: skip (no 1920x1080 mode)"; exit 0; }
t="$(mktemp -d)"; hp=""
out="$(xrandr --listmonitors | awk 'NR>1 && $2 ~ /\*/ {print $4; exit}')"
undo() { [ "$(current)" = "$before" ] || xrandr --output "$out" --mode "${before% *}" --rate "${before#* }"; }
trap '[ -n "$hp" ] && kill "$hp" 2>/dev/null; rm -rf "$t"; undo' EXIT
echo "test_fit_live: before: $out $before"

wait_for() { for _ in $(seq 50); do grep -q "$1" "$2" && return 0; sleep 0.1; done; cat "$2"; return 1; }
"$hold" 1920 1080 > "$t/out" 2>&1 & hp=$!
wait_for '^switched' "$t/out"
[ "$(size)" = 1920x1080 ] || { echo "test_fit_live: FAIL size $(size)"; exit 1; }
[ -s "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/rokast/fit.json" ] || { echo "test_fit_live: FAIL no fit.json"; exit 1; }
kill -TERM "$hp"; wait "$hp"; hp=""
grep -q '^restored' "$t/out" || { cat "$t/out"; echo "test_fit_live: FAIL not restored"; exit 1; }
[ "$(current)" = "$before" ] || { echo "test_fit_live: FAIL after SIGTERM: $(current)"; exit 1; }
echo "test_fit_live: SIGTERM ok"

"$hold" 1920 1080 > "$t/out" 2>&1 & hp=$!
wait_for '^switched' "$t/out"
kill -KILL "$hp"; wait "$hp" 2>/dev/null || true; hp=""
[ "$(size)" = 1920x1080 ] || { echo "test_fit_live: FAIL SIGKILL should leave 1920x1080"; exit 1; }
if "$repo/target/debug/rokast" doctor > "$t/doc" 2>&1 || ! grep -q '^FAIL  screen mode' "$t/doc"; then
    cat "$t/doc"; echo "test_fit_live: FAIL doctor did not report the leftover"; exit 1
fi
"$repo/target/debug/rokast" doctor --fix > "$t/doc" 2>&1 || true
grep -q '^ok    screen mode a crashed session left behind: put back' "$t/doc" || { cat "$t/doc"; echo "test_fit_live: FAIL --fix"; exit 1; }
[ "$(current)" = "$before" ] || { echo "test_fit_live: FAIL after --fix: $(current)"; exit 1; }
echo "test_fit_live: ok (switch, SIGTERM restore, SIGKILL -> doctor --fix restore)"
