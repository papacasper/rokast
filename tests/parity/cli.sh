#!/usr/bin/env bash
# The Rust `rokast` against the Python cli.py, command by command: same stdout, stderr and exit code.
#   tests/parity/cli.sh          commands that need no hardware (CI)
#   tests/parity/cli.sh --live   also list, doctor, doctor --report, selftest (this laptop: TV on the LAN, X11)
set -u
cd "$(dirname "$0")/../.." || exit 1
root=$PWD
cargo build -q --release -p rokast || exit 1
rust="$root/target/release/rokast"
live=0; [ "${1:-}" = --live ] && live=1

tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
# own config + cache, so the user's settings and the daily update check don't change the answer
export XDG_CONFIG_HOME="$tmp/config" XDG_CACHE_HOME="$tmp/cache"
mkdir -p "$XDG_CONFIG_HOME/rokast" "$XDG_CACHE_HOME/rokast"
cp "${ROKAST_PARITY_CONF:-/dev/null}" "$XDG_CONFIG_HOME/rokast/rokast.conf" 2>/dev/null || :

# what differs between two runs of the same program: the selftest temp file, frame counts, ffmpeg's own chatter
mask() { sed -E 's#rokast-selftest-[A-Za-z0-9_]+\.ts#rokast-selftest-X.ts#g; s#[0-9]+ frames#N frames#g' "$1"; }

fail=0 n=0
same() { # same <what> <args...>
  local what=$1; shift
  n=$((n + 1))
  python3 -I src/miracast/cli.py "$@" >"$tmp/py.out" 2>"$tmp/py.err"; local pc=$?
  ROKAST_ROOT="$root" "$rust" "$@" >"$tmp/rs.out" 2>"$tmp/rs.err"; local rc=$?
  local streams="out"; [ "$what" = all ] && streams="out err"
  for s in $streams; do
    if ! diff <(mask "$tmp/py.$s") <(mask "$tmp/rs.$s") >"$tmp/diff"; then
      echo "DIFF std$s: rokast $*"; sed 's/^/    /' "$tmp/diff"; fail=1
    fi
  done
  if [ "$pc" != "$rc" ]; then echo "DIFF exit: rokast $* (python $pc, rust $rc)"; fail=1; fi
}

fresh_cache() { # fresh_cache <latest tag>: a day-fresh update cache, so neither side asks GitHub
  printf '{"at": %s, "latest": "%s", "url": "https://github.com/papacasper/rokast/releases/tag/%s"}' \
    "$(date +%s)" "$1" "$1" >"$XDG_CACHE_HOME/rokast/update.json"
}

same all help
same all bogus
same all name
same all config
same all version
fresh_cache v0.3.0;  same all version --check;  same all version --check --json
fresh_cache v99.0.0; same all version --check;  same all version --check --json
CHECK_UPDATES=0 same all version --check
CHECK_UPDATES=0 same all version --check --json
NAME="  Den TV  " same all name
same all monitors
same all monitors --json
CAPTURE=x11 DISPLAY=:nope same all monitors --json
same all selftest --seconds 0
same all selftest --bogus
if [ "$live" = 1 ]; then
  same out list
  same out list --json
  same out doctor
  same out selftest --seconds 2
  same out doctor --report
fi
[ "$fail" = 0 ] && echo "cli parity: $n commands match" || exit 1
