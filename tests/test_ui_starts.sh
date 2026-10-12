#!/usr/bin/env bash
# tests/test_ui_starts.sh [rokast-ui]: the app opens its window and keeps running, on a headless X server (Xvfb) with
# its own D-Bus session and a stub rokast (offers an update, finds no TVs). Catches start-up panics that unit tests
# can't, like v0.3.0's (a GTK call before GTK was initialised). Needs xvfb-run and dbus-run-session. Run by CI.
set -euo pipefail
repo="$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)"
ui="$(readlink -f "${1:-$repo/ui/target/debug/rokast-ui}")"
[ -x "$ui" ] || { echo "test_ui_starts: build the UI first ($ui)" >&2; exit 1; }
for c in xvfb-run dbus-run-session; do command -v "$c" >/dev/null || { echo "test_ui_starts: needs $c" >&2; exit 1; }; done
t="$(mktemp -d)"; trap 'rm -rf "$t"' EXIT
cat > "$t/rokast" <<'EOF'
#!/bin/sh
case "$1 $2" in
    "version --check") echo '{"newer": true, "latest": "9.9.9", "url": "https://example.invalid/", "packaged": false, "kind": "user"}' ;;
    "update --auto") echo "update: skipped AUTO_UPDATE is off" ;;
    list*|scan*) echo '[]' ;;
esac
EOF
chmod +x "$t/rokast"
mkdir -p "$t/rt"; chmod 700 "$t/rt"
st=0
ROKAST="$t/rokast" XDG_CONFIG_HOME="$t/cfg" XDG_RUNTIME_DIR="$t/rt" GDK_BACKEND=x11 \
    timeout 6 xvfb-run -a dbus-run-session -- "$ui" > "$t/log" 2>&1 || st=$?
if [ "$st" = 124 ] && ! grep -q panicked "$t/log"; then
    echo "test_ui_starts: ok (still running after 6 s)"
else
    cat "$t/log"; echo "test_ui_starts: FAIL (exit $st)"; exit 1
fi
