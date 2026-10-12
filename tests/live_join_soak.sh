#!/usr/bin/env bash
# tests/live_join_soak.sh [N] [IDLE_MIN] [TV]   (defaults: 10 runs, 15 min, TARGET from rokast.conf)
# Cold-start join check against a real TV: N times, let the TV sit on its home screen IDLE_MIN minutes, then
# `rokast start`, wait for streaming, stop it, and check the teardown (only the Wi-Fi interface left). Prints which
# attempt joined each run. Passes when every run streamed and every join was on attempt 1. Needs the TV on and
# rokast set up on this machine; not run by CI. Uses this checkout (scripts/rokast), not the installed copy.
# Failed attempts are kept as evidence in ~/.cache/rokast/join-failures/ (src/miracast/joinlog.py).
set -uo pipefail
repo="$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)"
n="${1:-10}"; idle="${2:-15}"; tv=("${@:3}")
log="$(mktemp -d)"; pid=""
trap '[ -n "$pid" ] && kill -TERM "$pid" 2>/dev/null; echo "logs: $log"' EXIT
wifi="$(sudo ls /run/wpa_supplicant | sed -n 's/^p2p-dev-//p' | head -1)"
[ -n "$wifi" ] || { echo "live_join_soak: no p2p-dev interface"; exit 1; }
ok=0
for i in $(seq "$n"); do
    [ "$i" = 1 ] || { echo "run $i: idle ${idle} min"; sleep "$((idle * 60))"; }
    f="$log/run-$i.log"
    "$repo/scripts/rokast" start "${tv[@]}" > "$f" 2>&1 & pid=$!   # scripts/rokast execs python: $! is the CLI
    for _ in $(seq 180); do grep -qE '^status: (streaming|error|stopped)' "$f" && break; sleep 1; done
    kill -TERM "$pid" 2>/dev/null; wait "$pid" 2>/dev/null; pid=""
    sleep 2
    joined="$(sed -n 's/^p2p: joined on attempt \([0-9]*\/[0-9]*\).*/\1/p' "$f")"
    streamed=no; grep -q '^status: streaming' "$f" && streamed=yes
    clean=yes; iw dev | grep -q "Interface p2p-$wifi-" && clean=no
    echo "run $i: joined ${joined:-never} streamed $streamed teardown-clean $clean $(grep -m1 '^status: error' "$f")"
    [ "$streamed" = yes ] && [ "${joined%%/*}" = 1 ] && [ "$clean" = yes ] && ok=$((ok + 1))
done
echo "live_join_soak: $ok/$n first-try joins that streamed and tore down cleanly (idle ${idle} min)"
[ "$ok" = "$n" ]
