#!/usr/bin/env bash
# tests/test_pkg_deb.sh <rokast_X.Y.Z_amd64.deb>
# Install the .deb (packaging/deb/build-deb) in a clean ubuntu:22.04 (or $ROKAST_DEB_IMAGE) container with apt
# resolving its Depends, then check /usr/bin/rokast. doctor's Wi-Fi checks fail in a container (no wpa_supplicant
# running); everything up to them must pass. Run by CI after building the .deb; needs docker.
# shellcheck disable=SC2016  # check strings are single-quoted on purpose: eval expands them
set -euo pipefail
deb="$(readlink -f "${1:?usage: tests/test_pkg_deb.sh <deb>}")"
command -v docker >/dev/null || { echo "test_pkg_deb: skipped (docker missing)"; exit 0; }
v="$(basename "$deb" | cut -d_ -f2)"
image="${ROKAST_DEB_IMAGE:-ubuntu:22.04}"   # also tried: ubuntu:24.04, debian:12
t="$(mktemp -d)"; trap 'rm -rf "$t"' EXIT
docker run --rm -v "$deb:/rokast.deb:ro" "$image" bash -c '
    export DEBIAN_FRONTEND=noninteractive
    apt-get update -qq && apt-get install -y -qq -o Debug::pkgDepCache::AutoInstall=1 /rokast.deb > /tmp/apt.log 2>&1 ||
        { tail -20 /tmp/apt.log; exit 1; }
    echo "python=$(command -v python3 python 2>/dev/null | head -1)"   # before anything else is installed
    grep -i "python" /tmp/apt.log | grep -i "as \(Depends\|Recommends\|PreDepends\)" | sed "s/^ */pulled: /" | head -20
    # Ubuntu 22.04 itself brings Python (networkd-dispatcher): rokast must not be among what needs it
    apt-get remove -s python3-minimal > /tmp/rm.log 2>&1 || true
    echo "python_removal_listed=$(grep -c "^Remv python3-minimal " /tmp/rm.log)"
    echo "python_needed_by_rokast=$(grep -c "^Remv rokast " /tmp/rm.log)"
    [ "$(grep -c "^Remv python3" /tmp/rm.log)" -gt 0 ] || sed "s/^/removal: /" /tmp/rm.log | head -15
    echo "python_files=$(grep -rIli python /usr/lib/rokast | wc -l)"
    echo "which=$(readlink -f "$(command -v rokast)")"
    echo "version=$(rokast version)"
    echo "kind=$(CHECK_UPDATES=0 rokast version --check --json | sed -n "s/.*\"kind\": \"\([a-z]*\)\".*/\1/p")"
    test -s /usr/lib/rokast/config/release-signers && echo signers=ok
    rokast doctor 2>&1
    echo "doctor_exit=$?"
    echo "missing=$(ldd /usr/lib/rokast/wpa_supplicant /usr/lib/rokast/ui/target/release/rokast-ui | grep -c "not found")"
    sed -n "s/^Exec=//p" /usr/share/applications/io.github.papacasper.rokast.desktop
' > "$t/run.log" 2>&1 || true
fails=0
check() { if eval "$2"; then echo "ok    $1"; else echo "FAIL  $1   [$2]"; fails=$((fails + 1)); fi; }
check "apt installs it with its Depends" 'grep -q "^which=" "$t/run.log"'
grep -E "^(python_|removal: )" "$t/run.log" || true
check "rokast needs no Python (removing it would not remove rokast)" \
    'grep -q "^python_removal_listed=[1-9]" "$t/run.log" && grep -qx "python_needed_by_rokast=0" "$t/run.log"'
check "no installed rokast file mentions Python" 'grep -qx "python_files=0" "$t/run.log"'
grep "^python=/" "$t/run.log" && grep "^pulled: " "$t/run.log" | head -3   # what else brought Python in
check "rokast on PATH is the packaged one" 'grep -qx "which=/usr/lib/rokast/scripts/rokast" "$t/run.log"'
check "version $v" 'grep -qx "version=$v" "$t/run.log"'
check "install kind is deb (rokast update hands it to the package manager)" 'grep -qx "kind=deb" "$t/run.log"'
check "release signers shipped" 'grep -qx "signers=ok" "$t/run.log"'
check "doctor runs to the end (no traceback)" 'grep -q "^doctor_exit=" "$t/run.log" && ! grep -q Traceback "$t/run.log"'
check "doctor: tools from Depends" '[ "$(grep -cxE "ok    (ffmpeg|wpa_cli|dhcpcd|iw) installed" "$t/run.log")" = 4 ]'
check "doctor: packaged supplicant found, stamp ok" 'grep -qx "ok    patched wpa_supplicant ok: /usr/lib/rokast/wpa_supplicant" "$t/run.log"'
check "no missing libraries" 'grep -qx "missing=0" "$t/run.log"'
check "menu entry runs the packaged app" 'grep -qx "/usr/lib/rokast/ui/target/release/rokast-ui" "$t/run.log"'
if [ "$fails" != 0 ]; then cat "$t/run.log"; echo "test_pkg_deb: $fails failed"; exit 1; fi
echo "test_pkg_deb: all ok ($(basename "$deb") on $image)"
