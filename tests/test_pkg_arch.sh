#!/usr/bin/env bash
# tests/test_pkg_arch.sh [rokast|rokast-git] [outdir]
# Build packaging/aur/<pkg>/PKGBUILD in a clean archlinux:base-devel container (the clean-chroot check), run its
# check(), namcap, install it, then check /usr/bin/rokast. rokast: source = this working tree as the release archive;
# rokast-git: source = this repo's committed HEAD. Copies the built package to [outdir]. Needs docker; not run by CI.
# shellcheck disable=SC2016  # check strings are single-quoted on purpose: eval expands them
set -euo pipefail
repo="$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)"
pkg="${1:-rokast}"; out="${2:-}"
command -v docker >/dev/null || { echo "test_pkg_arch: skipped (docker missing)"; exit 0; }
t="$(mktemp -d)"; trap 'rm -rf "$t"' EXIT
mkdir -p "$t/pkg" "$t/out"
cp "$repo/packaging/aur/$pkg/PKGBUILD" "$t/pkg/"
if [ "$pkg" = rokast ]; then
    v="$(sed -n 's/^pkgver=//p' "$t/pkg/PKGBUILD")"
    mkdir -p "$t/a/rokast-$v"
    git -C "$repo" ls-files -z -co --exclude-standard | (cd "$repo" && xargs -0 cp --parents -a -t "$t/a/rokast-$v" --)
    echo "$v" > "$t/a/rokast-$v/VERSION"   # as the tagged release will have it
    tar -C "$t/a" -czf "$t/pkg/rokast-$v.tar.gz" "rokast-$v"   # same name as the source entry: makepkg uses it as is
    sum="$(sha256sum "$t/pkg/rokast-$v.tar.gz" | cut -d' ' -f1)"
    sed -i "0,/^sha256sums=('[^']*'/s//sha256sums=('$sum'/" "$t/pkg/PKGBUILD"   # SKIP or the release's sum
else
    sed -i 's|git+https://github.com/papacasper/rokast.git|git+file:///repo|' "$t/pkg/PKGBUILD"
    v="$(cat "$repo/VERSION")"
fi

docker run --rm -v "$t:/t" -v "$repo:/repo:ro" archlinux:base-devel bash -euc '
    pacman -Syu --noconfirm --needed namcap git >/dev/null
    useradd -m b && cp -r /t/pkg /home/b/pkg && chown -R b /home/b/pkg && cd /home/b/pkg
    git config --system --add safe.directory "*"
    # shellcheck disable=SC1091
    deps=$(bash -c "source PKGBUILD; echo \${depends[*]} \${makedepends[*]}")
    # shellcheck disable=SC2086
    pacman -S --noconfirm --needed $deps >/dev/null
    su b -c "makepkg --noconfirm" > /t/makepkg.log 2>&1 || { tail -40 /t/makepkg.log; exit 1; }
    p=$(ls ./*.pkg.tar.zst | grep -v debug)
    { namcap PKGBUILD; namcap "$p"; } > /t/namcap.log 2>&1 || true
    pacman -U --noconfirm "$p" >/dev/null
    cp "$p" /t/out/
    {
        echo "which=$(readlink -f "$(command -v rokast)")"
        echo "version=$(rokast version)"
        rokast doctor 2>&1 || true
        echo "missing=$(ldd /usr/lib/rokast/wpa_supplicant /usr/lib/rokast/ui/target/release/rokast-ui | grep -c "not found")"
        sed -n "s/^Exec=//p" /usr/share/applications/io.github.papacasper.rokast.desktop
        ls /usr/share/icons/hicolor/scalable/apps/
    } > /t/run.log 2>&1
'
fails=0
check() { if eval "$2"; then echo "ok    $1"; else echo "FAIL  $1   [$2]"; fails=$((fails + 1)); fi; }
check "package built ($(cd "$t/out" && ls))" '[ -n "$(ls "$t/out")" ]'
check "makepkg ran check() (python + cargo tests)" 'grep -q "Starting check()" "$t/makepkg.log" && grep -q "test result: ok" "$t/makepkg.log"'
# namcap false positives: our sibling modules (cli.py puts src/miracast on sys.path), the optional gi (Wayland, in
# optdepends), tools run as subprocesses (it only sees linked libraries), the dynamic loader itself, and bash (in
# base, so not listed; namcap warns whether or not it is).
grep -E " (E|W): " "$t/namcap.log" | grep -vE "Referenced python module '(wfd|wayland|paths|capture|selftest|p2p|update|joinlog|gi(\.repository\.(GLib|Gio))?)'|Dependency included, but may not be needed \('(ffmpeg|iw|dhcpcd|wpa_supplicant|sudo)'\)|Unused shared library '/usr/lib64/ld-linux-x86-64.so.2'|Referenced library 'bash' is an uninstalled dependency" > "$t/namcap.real" || true
check "namcap: no errors, no warnings beyond the known false positives" '[ ! -s "$t/namcap.real" ]'
check "rokast on PATH is the packaged one" 'grep -qx "which=/usr/lib/rokast/scripts/rokast" "$t/run.log"'
check "version $v" 'grep -qx "version=$v" "$t/run.log"'
check "doctor: packaged supplicant found, stamp ok" 'grep -qx "ok    patched wpa_supplicant ok: /usr/lib/rokast/wpa_supplicant" "$t/run.log"'
check "doctor: tools from depends" '[ "$(grep -cxE "ok    (ffmpeg|wpa_cli|dhcpcd|iw) installed" "$t/run.log")" = 4 ]'
check "no missing libraries" 'grep -qx "missing=0" "$t/run.log"'
check "menu entry runs the packaged app" 'grep -qx "/usr/lib/rokast/ui/target/release/rokast-ui" "$t/run.log"'
check "icon installed" 'grep -qx "io.github.papacasper.rokast.svg" "$t/run.log"'
if [ "$fails" != 0 ]; then cat "$t/namcap.real" "$t/run.log"; echo "test_pkg_arch: $fails failed"; exit 1; fi
if [ -n "$out" ]; then mkdir -p "$out" && cp "$t"/out/* "$out/"; fi
echo "test_pkg_arch: all ok ($pkg)"
