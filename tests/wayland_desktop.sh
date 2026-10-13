#!/usr/bin/env bash
# tests/wayland_desktop.sh gnome|kde
# `rokast selftest` with CAPTURE=wayland on a headless GNOME (gnome-shell --headless, xdg-desktop-portal-gnome) or
# KDE (kwin_wayland --virtual, xdg-desktop-portal-kde) session: the real portal ScreenCast + PipeWire + GStreamer
# path. The share dialog is answered over accessibility (tests/portal_click.py). Runs as root inside a throwaway
# fedora container (.github/workflows/wayland.yml installs the packages and builds the Rust rokast); never run it on
# your own desktop. ROKAST_NO_PY=1: the selftest must be the Rust binary's (Python is only the dialog clicker).
set -uo pipefail
desk="${1:?usage: tests/wayland_desktop.sh gnome|kde}"
repo="$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)"
export XDG_RUNTIME_DIR=/tmp/xdg-run HOME=/tmp/home XDG_SESSION_TYPE=wayland WAYLAND_DISPLAY=wayland-rokast
export GTK_A11Y=atspi QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1 QT_ACCESSIBILITY=1 NO_AT_BRIDGE=0
mkdir -p "$HOME/.config/rokast"; mkdir -m 700 "$XDG_RUNTIME_DIR"
printf 'CAPTURE=wayland\nAUDIO=0\nRES=720p30\nENCODER=auto\n' > "$HOME/.config/rokast/rokast.conf"
case "$desk" in
    gnome) export XDG_CURRENT_DESKTOP=GNOME
           compositor=(gnome-shell --headless --wayland-display "$WAYLAND_DISPLAY" --virtual-monitor 1280x720 --no-x11) ;;
    kde)   export XDG_CURRENT_DESKTOP=KDE KDE_FULL_SESSION=true
           # KWin screencasts only with OpenGL compositing; with no GPU that is Mesa's llvmpipe (not zink, not QPainter)
           export KWIN_COMPOSE=O2 LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe
           compositor=(kwin_wayland --virtual --width 1280 --height 720 --socket "$WAYLAND_DISPLAY" --no-lockscreen) ;;
    *) echo "unknown desktop $desk"; exit 2 ;;
esac

# gnome-shell needs logind on the system bus: a mock one (python-dbusmock's template, as GNOME's own CI does)
mkdir -p /run/dbus; [ -S /run/dbus/system_bus_socket ] || dbus-daemon --system --fork
python3 -m dbusmock --system --template logind > /tmp/dbusmock.log 2>&1 &
export QT_LOGGING_RULES="xdp*.debug=true;kwin_*.debug=false"

# shellcheck disable=SC2016  # expanded by the inner bash
exec dbus-run-session -- bash -c '
    log=/tmp/wayland-test; mkdir -p "$log"
    dbus-update-activation-environment --all
    /usr/libexec/at-spi-bus-launcher --launch-immediately > "$log/atspi.log" 2>&1 &
    pipewire > "$log/pipewire.log" 2>&1 &
    sleep 1; wireplumber > "$log/wireplumber.log" 2>&1 &
    "$@" > "$log/compositor.log" 2>&1 &
    for _ in $(seq 60); do [ -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ] && break; sleep 1; done
    [ -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ] || { echo "compositor did not start"; tail -40 "$log/compositor.log"; exit 1; }
    sleep 3
    portal="/usr/libexec/xdg-desktop-portal-'"$desk"'"   # started here, not by D-Bus, so its log is ours
    QT_FORCE_STDERR_LOGGING=1 QT_LOGGING_RULES="*.debug=true;qt.*.debug=false" "$portal" > "$log/portal-backend.log" 2>&1 &
    sleep 2
    python3 -I "$0/tests/portal_click.py" 90 > "$log/click.log" 2>&1 & click=$!
    ROKAST_NO_PY=1 "$0/scripts/rokast" selftest --seconds 3 > "$log/selftest.log" 2>&1; rc=$?
    kill "$click" 2>/dev/null; wait "$click" 2>/dev/null
    for f in selftest click compositor portal-backend; do echo "== $f"; tail -60 "$log/$f.log"; done
    for f in /tmp/wayland-test/*.log; do grep -il "error\|fail" "$f" >/dev/null && { echo "== $f (errors)"; grep -i "error\|fail" "$f" | tail -15; }; done
    echo "wayland_desktop: selftest exit $rc on '"$desk"'"
    exit "$rc"
' "$repo" "${compositor[@]}"
