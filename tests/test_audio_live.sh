#!/usr/bin/env bash
# Sound routing against the real sound server (PipeWire/PulseAudio), no TV: the audio_hold example routes like a
# playing session until SIGTERM. Quiet test tones (-45 dB) from paplay; checks where each stream plays, that the
# TV source carries sound, and that everything is put back after stop, SIGTERM and a crash (SIGKILL + recover).
# Needs pactl, paplay, parecord, sox; skips without them or without a sound server. Not run by CI.
set -euo pipefail
repo="$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)"
for c in pactl paplay parecord sox python3; do
    command -v "$c" >/dev/null || { echo "test_audio_live: skipped ($c missing)"; exit 0; }
done
pactl info >/dev/null 2>&1 || { echo "test_audio_live: skipped (no sound server)"; exit 0; }
cargo build -q --manifest-path "$repo/Cargo.toml" -p rokast-core --example audio_hold
hold_bin="$repo/target/debug/examples/audio_hold"
t="$(mktemp -d)"; pids=()
cleanup() { for p in "${pids[@]}"; do kill "$p" 2>/dev/null || true; done; wait 2>/dev/null || true; rm -rf "$t"; }
trap cleanup EXIT
sox -n -r 48000 -c 2 "$t/tone.wav" synth 60 sine 440 gain -45
fails=0
fail() { echo "test_audio_live: FAIL $*"; fails=$((fails + 1)); }

# sink name each test stream (application.name rokast-test-*) plays on, as "app=sink" lines
streams() {
    pactl -f json list sink-inputs | python3 -I -c '
import json, subprocess, sys
sinks = {s["index"]: s["name"] for s in json.loads(subprocess.run(["pactl","-f","json","list","sinks"],capture_output=True,text=True).stdout)}
for i in json.load(sys.stdin):
    a = i["properties"].get("application.name", "")
    if a.startswith("rokast-test-"):
        k = i["sink"]
    print(a + "=" + str(sinks.get(k, k)))' | sort
}
snapshot() { { echo "default=$(pactl get-default-sink)"; pactl list short sinks | awk '$2=="rokast"{print "rokast sink present"}'; streams; } }
play() { paplay --client-name="$1" "$t/tone.wav" & pids+=($!); }
wait_for() { for _ in $(seq 50); do eval "$1" && return 0; sleep 0.1; done; return 1; }
start_hold() { "$hold_bin" "$1" > "$t/hold.out" 2>&1 & hold=$!; wait_for 'grep -q "^source:\|rror\|no such" "$t/hold.out"'; }
stop_hold() { kill -"${1:-TERM}" "$hold" 2>/dev/null || true; wait "$hold" 2>/dev/null || true; }
loud() { # $1 = pulse source: RMS of 1 s recorded from it is above silence
    # raw, not wav: a killed parecord never writes the wav header's length
    { parecord --device="$1" --raw --format=s16le --channels=2 --rate=48000 2>/dev/null || true; } \
        | head -c 192000 > "$t/rec.raw"
    sox -t raw -r 48000 -e signed -b 16 -c 2 "$t/rec.raw" -n stat 2>&1 | awk '/RMS +amplitude/ {exit !($3 > 0.0005)}'
}
speakers="$(pactl get-default-sink)"
[ "$speakers" != rokast ] || { echo "test_audio_live: FAIL default output is already rokast (run rokast doctor --fix)"; exit 1; }

# 1. AUDIO=tv: everything to the TV, laptop silent; then SIGTERM restores
play rokast-test-a; wait_for '[ -n "$(streams)" ]'
before="$(snapshot)"
start_hold tv
grep -qx "source: rokast.monitor" "$t/hold.out" || fail "tv: $(cat "$t/hold.out")"
wait_for '[ "$(streams)" = "rokast-test-a=rokast" ]' || fail "tv: stream not moved: $(streams)"
[ "$(pactl get-default-sink)" = rokast ] || fail "tv: default output not rokast"
loud rokast.monitor || fail "tv: rokast.monitor is silent"
play rokast-test-b   # a new stream follows the default
wait_for 'streams | grep -qx "rokast-test-b=rokast"' || fail "tv: new stream not on rokast: $(streams)"
stop_hold TERM
grep -qx restored "$t/hold.out" || fail "tv: no 'restored' after SIGTERM"
kill "${pids[1]}"; wait "${pids[1]}" 2>/dev/null || true
[ "$(snapshot)" = "$before" ] || fail "tv: not restored after SIGTERM: $(snapshot) vs $before"

# 2. crash: SIGKILL leaves routing behind, the next start recovers it
start_hold tv; wait_for '[ "$(streams)" = "rokast-test-a=rokast" ]' || fail "crash: stream not moved"
stop_hold KILL
[ "$(pactl get-default-sink)" = rokast ] || fail "crash: expected leftover routing"
start_hold off
grep -qx recovered "$t/hold.out" || fail "crash: not recovered: $(cat "$t/hold.out")"
grep -qx "source: none" "$t/hold.out" || fail "crash: off should record nothing"
stop_hold TERM
[ "$(snapshot)" = "$before" ] || fail "crash: not restored: $(snapshot) vs $before"

# 3. AUDIO=app:rokast-test-a: only that app moves, a new stream of it moves within 1 s, the other stays
play rokast-test-b; wait_for 'streams | grep -q rokast-test-b'
before="$(snapshot)"
start_hold app:rokast-test-a
wait_for 'streams | grep -qx "rokast-test-a=rokast"' || fail "app: a not moved: $(streams)"
streams | grep -qx "rokast-test-b=$speakers" || fail "app: b moved: $(streams)"
[ "$(pactl get-default-sink)" = "$speakers" ] || fail "app: default output changed"
kill "${pids[0]}"; wait "${pids[0]}" 2>/dev/null || true
t0=$(date +%s%N); play rokast-test-a
if wait_for '[ "$(streams | grep -c "rokast-test-a=rokast")" = 1 ]'; then
    ms=$(( ($(date +%s%N) - t0) / 1000000 )); [ "$ms" -le 1000 ] || fail "app: new stream moved after ${ms} ms"
else fail "app: new stream not moved: $(streams)"; fi
loud rokast.monitor || fail "app: rokast.monitor is silent"
stop_hold TERM
[ "$(snapshot)" = "$before" ] || fail "app: not restored: $(snapshot) vs $before"

# 4. AUDIO=device:<speakers>: records that output's monitor, moves nothing
start_hold "device:$speakers"
grep -qx "source: $speakers.monitor" "$t/hold.out" || fail "device: $(cat "$t/hold.out")"
[ "$(snapshot)" = "$before" ] || fail "device: something moved: $(snapshot)"
stop_hold TERM
start_hold device:nope; grep -q "no such output" "$t/hold.out" || fail "device: bad sink accepted"
stop_hold TERM

[ "$fails" = 0 ] && echo "test_audio_live: ok (tv, SIGTERM, crash+recover, app, device)" || exit 1
