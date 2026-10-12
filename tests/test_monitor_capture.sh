#!/usr/bin/env bash
# MONITOR= captures only that monitor: a 3840x1080 Xvfb screen painted red (left) / blue (right), a stub xrandr
# listing it as two monitors (Xvfb ignores `xrandr --setmonitor`), then `rokast selftest --out` per monitor and the
# centre pixel of the first frame. Needs Xvfb (Arch: xorg-server-xvfb), ffmpeg, ImageMagick; skips without them.
# Never touches your real display. Not run by CI (no ffmpeg there); run it after changing capture code.
set -euo pipefail
repo="$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)"
for c in Xvfb ffmpeg display; do
    command -v "$c" >/dev/null || { echo "test_monitor_capture: skipped ($c missing)"; exit 0; }
done
# ImageMagick 7 is `magick`; a bare `convert` on PATH may be something else entirely
if command -v magick >/dev/null; then im=(magick)
elif convert -version 2>/dev/null | grep -q ImageMagick; then im=(convert)
else echo "test_monitor_capture: skipped (ImageMagick missing)"; exit 0; fi
t="$(mktemp -d)"; xp=""
trap '[ -n "$xp" ] && kill "$xp" 2>/dev/null; rm -rf "$t"' EXIT
mkdir -p "$t/bin"
printf '#!/bin/sh\nprintf "Monitors: 2\\n 0: +*LEFT 1920/508x1080/286+0+0  screen\\n 1: +RIGHT 1920/508x1080/286+1920+0  screen\\n"\n' > "$t/bin/xrandr"
chmod +x "$t/bin/xrandr"
"${im[@]}" -size 1920x1080 xc:red xc:blue +append "$t/split.png"
d=":$((90 + RANDOM % 9))"
Xvfb "$d" -ac -noreset -screen 0 3840x1080x24 >/dev/null 2>&1 & xp=$!   # -noreset: keep the root picture
for _ in $(seq 20); do DISPLAY="$d" import -window root -crop 1x1+0+0 txt:- >/dev/null 2>&1 && break; sleep 0.5; done
DISPLAY="$d" display -window root "$t/split.png" 2>/dev/null || true   # exits 1 even when it painted
DISPLAY="$d" import -window root -crop 1x1+3000+500 txt:- 2>/dev/null | grep -qw blue \
    || { echo "test_monitor_capture: FAIL could not paint Xvfb $d"; exit 1; }

fails=0
for case in "RIGHT 0 0 254" "LEFT 254 0 0" ":primary 254 0 0"; do
    read -r m r g b <<< "$case"; [ "$m" = :primary ] && m=""
    PATH="$t/bin:$PATH" DISPLAY="$d" MONITOR="$m" AUDIO=0 ENCODER=libx264 ROKAST_CONFIG="$t/none.conf" \
        "$repo/scripts/rokast" selftest --seconds 2 --out "$t/m.ts" > "$t/log" 2>&1 || { cat "$t/log"; fails=$((fails + 1)); continue; }
    # 2x2 crop: a 1x1 crop of yuv420p has a zero-size chroma plane
    read -r R G B < <(ffmpeg -v error -i "$t/m.ts" -frames:v 1 -vf crop=2:2:960:540 -f rawvideo -pix_fmt rgb24 - | od -An -tu1 -N3)
    if [ $((R - r)) -ge -8 ] && [ $((R - r)) -le 8 ] && [ $((G - g)) -le 8 ] && [ $((B - b)) -ge -8 ] && [ $((B - b)) -le 8 ]; then
        echo "ok    MONITOR=${m:-<primary>} centre $R $G $B"
    else
        echo "FAIL  MONITOR=${m:-<primary>} centre $R $G $B, expected about $r $g $b"; fails=$((fails + 1))
    fi
done
if [ "$fails" != 0 ]; then echo "test_monitor_capture: $fails failed"; exit 1; fi
echo "test_monitor_capture: all ok"
