#!/usr/bin/env bash
# tests/test_pkg_rpm.sh <rokast-X.Y.Z-1.x86_64.rpm>
# Install the .rpm (packaging/rpm/build-rpm) in a clean fedora:43 (or $ROKAST_RPM_IMAGE) container with dnf resolving
# its Requires (so ffmpeg-free, not RPM Fusion's ffmpeg), check /usr/bin/rokast, then run `rokast selftest` on an Xvfb
# screen: proves the encoder auto-detect finds one in ffmpeg-free (no libx264) and that it encodes. doctor's Wi-Fi
# checks fail in a container; everything up to them must pass. Run by CI after building the .rpm; needs docker.
# shellcheck disable=SC2016  # check strings are single-quoted on purpose: eval expands them
set -euo pipefail
rpm="$(readlink -f "${1:?usage: tests/test_pkg_rpm.sh <rpm>}")"
command -v docker >/dev/null || { echo "test_pkg_rpm: skipped (docker missing)"; exit 0; }
v="$(basename "$rpm" | cut -d- -f2)"
image="${ROKAST_RPM_IMAGE:-fedora:43}"
t="$(mktemp -d)"; trap 'rm -rf "$t"' EXIT
docker run --rm -v "$rpm:/rokast.rpm:ro" "$image" bash -c '
    dnf install -y -q /rokast.rpm > /tmp/dnf.log 2>&1 || { tail -20 /tmp/dnf.log; exit 1; }
    echo "python=$(command -v python3 python 2>/dev/null | head -1)"   # before anything else is installed
    echo "which=$(readlink -f "$(command -v rokast)")"
    echo "version=$(rokast version)"
    echo "kind=$(CHECK_UPDATES=0 rokast version --check --json | sed -n "s/.*\"kind\": \"\([a-z]*\)\".*/\1/p")"
    test -s /usr/lib/rokast/config/release-signers && echo signers=ok
    rokast doctor 2>&1
    echo "doctor_exit=$?"
    echo "missing=$(ldd /usr/lib/rokast/wpa_supplicant /usr/lib/rokast/ui/target/release/rokast-ui | grep -c "not found")"
    sed -n "s/^Exec=//p" /usr/share/applications/io.github.papacasper.rokast.desktop
    echo "ffmpeg_pkg=$(rpm -qf "$(command -v ffmpeg)" --qf "%{NAME}")"
    dnf install -y -q xorg-x11-server-Xvfb > /tmp/dnf2.log 2>&1 || { tail -20 /tmp/dnf2.log; exit 1; }
    Xvfb :9 -screen 0 1280x720x24 >/dev/null 2>&1 & sleep 2
    mkdir -p ~/.config/rokast && printf "AUDIO=0\nRES=720p30\n" > ~/.config/rokast/rokast.conf
    DISPLAY=:9 rokast selftest --seconds 3 2>&1
    echo "selftest_exit=$?"
' > "$t/run.log" 2>&1 || true
fails=0
check() { if eval "$2"; then echo "ok    $1"; else echo "FAIL  $1   [$2]"; fails=$((fails + 1)); fi; }
check "dnf installs it with its Requires" 'grep -q "^which=" "$t/run.log"'
check "no Python installed with it (none in its Requires, direct or indirect)" 'grep -qx "python=" "$t/run.log"'
check "rokast on PATH is the packaged one" 'grep -qx "which=/usr/lib/rokast/scripts/rokast" "$t/run.log"'
check "version $v" 'grep -qx "version=$v" "$t/run.log"'
check "install kind is rpm (rokast update hands it to the package manager)" 'grep -qx "kind=rpm" "$t/run.log"'
check "release signers shipped" 'grep -qx "signers=ok" "$t/run.log"'
check "doctor runs to the end (no traceback)" 'grep -q "^doctor_exit=" "$t/run.log" && ! grep -q Traceback "$t/run.log"'
check "doctor: tools from Requires" '[ "$(grep -cxE "ok    (ffmpeg|wpa_cli|dhcpcd|iw) installed" "$t/run.log")" = 4 ]'
check "doctor: packaged supplicant found, stamp ok" 'grep -qx "ok    patched wpa_supplicant ok: /usr/lib/rokast/wpa_supplicant" "$t/run.log"'
check "doctor: an encoder works with Fedora's ffmpeg" 'grep -qE "^ok    encoder " "$t/run.log"'
check "no missing libraries" 'grep -qx "missing=0" "$t/run.log"'
check "menu entry runs the packaged app" 'grep -qx "/usr/lib/rokast/ui/target/release/rokast-ui" "$t/run.log"'
check "selftest encodes on Xvfb" 'grep -qx "selftest_exit=0" "$t/run.log"'
grep -E "^(ffmpeg_pkg=|ok    encoder|note  )" "$t/run.log" || true
if [ "$fails" != 0 ]; then cat "$t/run.log"; echo "test_pkg_rpm: $fails failed"; exit 1; fi
echo "test_pkg_rpm: all ok ($(basename "$rpm") on $image)"
