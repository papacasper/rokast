#!/usr/bin/env bash
# SCREEN=extend against the real display, no TV: add a 1920x1080 screen right of the primary one, check xrandr and
# GDK see it there and x11grab captures it; SIGTERM -> monitors, every port's debugfs `force` and XFCE's /Notify
# match the snapshot from before; SIGKILL -> doctor FAILs, doctor --fix restores. Needs sudo. Not run by CI.
set -euo pipefail
repo="$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)"
export DISPLAY="${DISPLAY:-:0}"
cargo build -q --manifest-path "$repo/Cargo.toml" -p rokast-core --example extend_hold
cargo build -q --manifest-path "$repo/Cargo.toml" -p rokast
hold="$repo/target/debug/examples/extend_hold"
snap() {
    xrandr --listmonitors
    for f in /sys/class/drm/card*-*/status; do
        c="${f%/status}"; c="${c##*/}"; n="${c#card}"
        sudo cat "/sys/kernel/debug/dri/${n%%-*}/${n#*-}/force" 2>/dev/null | sed "s|^|$c force |" || true
    done
    xfconf-query -c displays -p /Notify 2>/dev/null || true
}
t="$(mktemp -d)"; hp=""
trap '[ -n "$hp" ] && kill "$hp" 2>/dev/null; rm -rf "$t"' EXIT
snap > "$t/before"
same() { snap > "$t/now"; diff "$t/before" "$t/now" || { echo "test_extend_live: FAIL after $1"; exit 1; }; }
wait_for() { for _ in $(seq 100); do grep -q "$1" "$2" && return 0; sleep 0.1; done; cat "$2"; return 1; }

"$hold" 1920 1080 right > "$t/out" 2>&1 & hp=$!
wait_for '^extended' "$t/out"
cat "$t/out"
read -r _ name geo kind < "$t/out"
[ "$kind" = full ] || echo "test_extend_live: note: $kind"
[ "$(xrandr --listmonitors | head -1)" = "Monitors: 2" ] || { echo "test_extend_live: FAIL not 2 monitors"; exit 1; }
pw="$(xrandr --listmonitors | awk 'NR>1 && $2 ~ /\*/ {split($3,a,"[/x+]"); print a[1] + a[5]; exit}')"
[ "$geo" = "1920x1080+${pw}+0" ] || { echo "test_extend_live: FAIL at $geo, want 1920x1080+${pw}+0"; exit 1; }
if [ "$kind" = full ]; then
    n="$(python3 -I -c 'import gi; gi.require_version("Gdk","3.0"); from gi.repository import Gdk; print(Gdk.Display.get_default().get_n_monitors())')"
    [ "$n" = 2 ] || { echo "test_extend_live: FAIL GDK sees $n monitors"; exit 1; }
fi
ffmpeg -loglevel error -f x11grab -video_size 1920x1080 -i "${DISPLAY}+${pw},0" -frames:v 1 -y "$t/grab.png"
[ -s "$t/grab.png" ] || { echo "test_extend_live: FAIL grab"; exit 1; }
[ -s "${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/rokast/extend.json" ] || { echo "test_extend_live: FAIL no extend.json"; exit 1; }
kill -TERM "$hp"; wait "$hp"; hp=""
grep -q '^restored' "$t/out" || { cat "$t/out"; echo "test_extend_live: FAIL not restored"; exit 1; }
same SIGTERM
echo "test_extend_live: SIGTERM ok ($name $kind)"

"$hold" 1920 1080 right > "$t/out" 2>&1 & hp=$!
wait_for '^extended' "$t/out"
kill -KILL "$hp"; wait "$hp" 2>/dev/null || true; hp=""
[ "$(xrandr --listmonitors | head -1)" = "Monitors: 2" ] || { echo "test_extend_live: FAIL SIGKILL should leave 2"; exit 1; }
if "$repo/target/debug/rokast" doctor > "$t/doc" 2>&1 || ! grep -q '^FAIL  TV screen' "$t/doc"; then
    cat "$t/doc"; echo "test_extend_live: FAIL doctor did not report the leftover"; exit 1
fi
"$repo/target/debug/rokast" doctor --fix > "$t/doc" 2>&1 || true
grep -q '^ok    TV screen a crashed session left behind: removed' "$t/doc" || { cat "$t/doc"; echo "test_extend_live: FAIL --fix"; exit 1; }
same 'doctor --fix'
echo "test_extend_live: ok (screen added right of the laptop, GDK + grab, SIGTERM restore, SIGKILL -> doctor --fix restore)"
