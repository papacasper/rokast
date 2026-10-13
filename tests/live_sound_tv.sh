#!/usr/bin/env bash
# The phase 12 TV check: mirror to the real TV with AUDIO=tv, play a soft chime (C-E-G-C) a few times, check the
# player is on the rokast output and the laptop's output has nothing, stop with SIGTERM, check everything is back,
# then play the chime once more on the laptop. The person at the TV confirms what they heard. Not run by CI.
# usage: tests/live_sound_tv.sh ["TV name"] [chimes]
set -euo pipefail
repo="$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)"
tv="${1:-}"; n="${2:-6}"
for c in pactl paplay sox; do command -v "$c" >/dev/null || { echo "live_sound_tv: $c missing"; exit 1; }; done
cargo build -q --release --manifest-path "$repo/Cargo.toml" -p rokast
t="$(mktemp -d)"; rp=""
trap '[ -n "$rp" ] && kill "$rp" 2>/dev/null; rm -rf "$t"' EXIT
i=0
for f in 523.25 659.25 783.99 1046.5; do   # C5 E5 G5 C6, 0.22 s apart, soft attack, long fade
    sox -n -r 48000 -c 2 "$t/n$i.wav" synth 1.6 sine "$f" fade q 0.015 1.6 1.5 gain -16 pad "$(awk "BEGIN{print $i*0.22}")" 0
    i=$((i + 1))
done
sox -m "$t"/n?.wav "$t/dry.wav" && sox "$t/dry.wav" "$t/chime.wav" reverb 40 50 60 gain -n -6 pad 0 0.6

speakers="$(pactl get-default-sink)"
before="$(pactl get-default-sink; pactl list short modules | awk '{print $2}' | sort)"
echo "live_sound_tv: speakers = $speakers; starting the mirror"
AUDIO=tv ROKAST_ROOT="$repo" "$repo/target/release/rokast" start ${tv:+"$tv"} > "$t/start.log" 2>&1 & rp=$!
for _ in $(seq 120); do
    grep -q '^status: streaming' "$t/start.log" && break
    kill -0 "$rp" 2>/dev/null || { cat "$t/start.log"; echo "live_sound_tv: FAIL start exited"; exit 1; }
    sleep 1
done
grep -q '^status: streaming' "$t/start.log" || { tail -20 "$t/start.log"; echo "live_sound_tv: FAIL not streaming after 120 s"; exit 1; }
grep '^audio:' "$t/start.log" || true
fails=0
for k in $(seq "$n"); do
    paplay --client-name=rokast-chime "$t/chime.wav" & pp=$!
    sleep 0.7
    where="$(pactl -f json list sink-inputs | python3 -I -c '
import json, subprocess, sys
sinks = {s["index"]: s["name"] for s in json.loads(subprocess.run(["pactl","-f","json","list","sinks"],capture_output=True,text=True).stdout)}
print(" ".join(sorted({sinks.get(i["sink"], "?") for i in json.load(sys.stdin) if i["properties"].get("application.name") == "rokast-chime"})))')"
    echo "live_sound_tv: chime $k/$n on: $where"
    [ "$where" = rokast ] || fails=$((fails + 1))
    wait "$pp" || true
    sleep 1.5
done
echo "live_sound_tv: stopping (SIGTERM)"
kill -TERM "$rp"; wait "$rp" || true; rp=""
grep -E '^status: stopped|^audio:' "$t/start.log" | tail -3
after="$(pactl get-default-sink; pactl list short modules | awk '{print $2}' | sort)"
[ "$after" = "$before" ] || { echo "live_sound_tv: FAIL not restored"; diff <(echo "$before") <(echo "$after") || true; fails=$((fails + 1)); }
echo "live_sound_tv: one chime on the laptop speakers now ($speakers)"
paplay "$t/chime.wav"
if [ "$fails" != 0 ]; then echo "live_sound_tv: FAIL ($fails)"; exit 1; fi
echo "live_sound_tv: ok ($n chimes on rokast, restored after SIGTERM)"
