#!/usr/bin/env bash
# The Rust `rokast`, command by command, against the answers the Python version gave (tests/parity/cli/, frozen
# before Python was removed, docs/plan-rust.md R6): same stdout, stderr and exit code, for commands that need no TV.
# Each runs in a fixed environment (no display, own config and cache) so the answer is the same on any machine.
#   tests/parity/cli.sh            compare (CI)
#   tests/parity/cli.sh --record   rewrite tests/parity/cli/ from the current binary (only for a deliberate change)
# ROKAST_PARITY_PROG overrides the program (the first recording ran the Python cli.py).
set -u
cd "$(dirname "$0")/../.." || exit 1
root=$PWD
want="$root/tests/parity/cli"
record=0; [ "${1:-}" = --record ] && record=1
if [ -n "${ROKAST_PARITY_PROG:-}" ]; then
  read -r -a prog <<<"$ROKAST_PARITY_PROG"
else
  cargo build -q --release --locked -p rokast || exit 1
  prog=("$root/target/release/rokast")
fi
version=$(cat VERSION)

tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
mkdir -p "$tmp/home" "$tmp/config/rokast" "$tmp/cache/rokast" "$tmp/run"
# what differs between machines and runs: the temp dir, this version, the selftest temp file, frame counts
mask() { sed -E "s#$tmp#@TMP@#g; s#$version#@VERSION@#g; "'s#rokast-selftest-[A-Za-z0-9_]+\.ts#rokast-selftest-X.ts#g; s#[0-9]+ frames#N frames#g' "$1"; }

fail=0 n=0
same() { # same [VAR=value...] -- <args...>
  local envs=()
  while [ "$1" != -- ]; do envs+=("$1"); shift; done; shift
  n=$((n + 1))
  local id; id=$(printf '%02d' "$n")
  env -i PATH="/usr/bin:/bin" HOME="$tmp/home" LANG=C.UTF-8 XDG_CONFIG_HOME="$tmp/config" XDG_CACHE_HOME="$tmp/cache" \
      XDG_RUNTIME_DIR="$tmp/run" ROKAST_ROOT="$root" DISPLAY=:rokast-none "${envs[@]}" "${prog[@]}" "$@" \
      >"$tmp/out" 2>"$tmp/err"
  echo $? >"$tmp/code"
  mask "$tmp/out" >"$tmp/out.m"; mask "$tmp/err" >"$tmp/err.m"
  if [ "$record" = 1 ]; then
    mkdir -p "$want"
    { printf '%s ' "${envs[@]}" rokast; printf '%s ' "$@"; echo; } | sed 's/ $//' >"$want/$id.cmd"
    cp "$tmp/out.m" "$want/$id.out"; cp "$tmp/err.m" "$want/$id.err"; cp "$tmp/code" "$want/$id.code"
    return
  fi
  for s in out err code; do
    local got="$tmp/$s.m"; [ "$s" = code ] && got="$tmp/code"
    if ! diff "$want/$id.$s" "$got" >"$tmp/diff"; then
      echo "DIFF $s: $(cat "$want/$id.cmd")"; sed 's/^/    /' "$tmp/diff"; fail=1
    fi
  done
}

fresh_cache() { # fresh_cache <latest tag>: a day-fresh update cache, so nothing asks GitHub
  printf '{"at": %s, "latest": "%s", "url": "https://github.com/papacasper/rokast/releases/tag/%s"}' \
    "$(date +%s)" "$1" "$1" >"$tmp/cache/rokast/update.json"
}

same -- help
same -- bogus
same -- config
same -- version
fresh_cache "v$version"; same -- version --check;  same -- version --check --json
fresh_cache v99.0.0; same -- version --check;  same -- version --check --json
same CHECK_UPDATES=0 -- version --check
same CHECK_UPDATES=0 -- version --check --json
same "NAME=  Den TV  " -- name
same -- monitors
same -- monitors --json
same CAPTURE=x11 DISPLAY=:nope -- monitors --json
same -- selftest --seconds 0
same -- selftest --bogus
same -- update --rollback
if [ "$record" = 1 ]; then echo "cli parity: recorded $n commands in $want"; exit 0; fi
[ "$fail" = 0 ] && echo "cli parity: $n commands match" || exit 1
