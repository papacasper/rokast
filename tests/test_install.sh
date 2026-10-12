#!/usr/bin/env bash
# scripts/install end to end in a throwaway HOME: upgrade from a v0.1.0-style folder install, prune, rollback,
# uninstall. Uses fake release folders with a stub desktop-app binary (no cargo, no network). Run by CI.
# shellcheck disable=SC2016  # check strings are single-quoted on purpose: eval expands them
set -euo pipefail
repo="$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)"
t="$(mktemp -d)"; trap 'rm -rf "$t"' EXIT
export HOME="$t/home"
unset XDG_DATA_HOME XDG_CONFIG_HOME XDG_CACHE_HOME
mkdir -p "$HOME"
export base="$HOME/.local/share/rokast" desktop="$HOME/.local/share/applications/io.github.papacasper.rokast.desktop"
fails=0
check() { if eval "$2"; then echo "ok    $1"; else echo "FAIL  $1   [$2]"; fails=$((fails + 1)); fi; }

release() {   # release <version>: a tarball-like folder (no .git) with a stub prebuilt UI
    local d="$t/rokast-$1"
    mkdir -p "$d"
    git -C "$repo" ls-files -z -co --exclude-standard | (cd "$repo" && xargs -0 cp --parents -a -t "$d" --)
    echo "$1" > "$d/VERSION"
    mkdir -p "$d/ui/target/release"
    printf '#!/bin/sh\necho stub-ui %s\n' "$1" > "$d/ui/target/release/rokast-ui"; chmod +x "$d/ui/target/release/rokast-ui"
    echo "$d"
}
inst() { PATH="/usr/bin:/bin" "$1/scripts/install" "${@:2}" > "$t/out" 2>&1 || { cat "$t/out"; return 1; }; }
ver() { "$HOME/.local/bin/rokast" version; }

# A v0.1.0-style install: run from its own folder, config inside it, menu entry + PATH link pointing into it.
old="$(release 0.1.0)"
echo "TARGET=Den" > "$old/config/rokast.conf"
mkdir -p "$old/vendor/hostap-src/.git"   # v0.1.0 built the supplicant in-tree
mkdir -p "$HOME/.local/bin" "$(dirname "$desktop")"
ln -s "$old/scripts/rokast" "$HOME/.local/bin/rokast"
sed "s|@BIN@|$old/ui/target/release/rokast-ui|" "$old/assets/rokast.desktop" > "$desktop"

r2="$(release 0.2.0)"; inst "$r2" --no-supplicant
check "upgrade: version 0.2.0" '[ "$(ver)" = 0.2.0 ]'
check "upgrade: settings moved out of the old folder" 'grep -qx "TARGET=Den" "$HOME/.config/rokast/rokast.conf" && [ -f "$old/config/rokast.conf.moved" ] && [ ! -e "$old/config/rokast.conf" ]'
check "upgrade: CLI sees the moved settings" '[ "$("$HOME/.local/bin/rokast" config 2>/dev/null)" = "$HOME/.config/rokast/rokast.conf" ]'
check "menu entry points at current/" 'grep -qx "Exec=$base/current/ui/target/release/rokast-ui" "$desktop"'
check "menu entry target runs" '[ "$("$base/current/ui/target/release/rokast-ui")" = "stub-ui 0.2.0" ]'
check "upgrade: old hostap checkout moved to the build cache" '[ -d "$HOME/.cache/rokast/hostap-src/.git" ] && [ ! -e "$old/vendor/hostap-src" ]'
check "no local config copied into the install" '[ ! -e "$base/versions/0.2.0/config/rokast.conf" ]'

r3="$(release 0.3.0)"; inst "$r3" --no-supplicant
r4="$(release 0.4.0)"; inst "$r4" --no-supplicant
check "keeps current + previous only" '[ "$(ls "$base/versions")" = "$(printf "0.3.0\n0.4.0")" ]'
check "version 0.4.0" '[ "$(ver)" = 0.4.0 ]'

inst "$r4" --rollback
check "rollback -> 0.3.0" '[ "$(ver)" = 0.3.0 ] && [ "$("$base/current/ui/target/release/rokast-ui")" = "stub-ui 0.3.0" ]'
inst "$r4" --rollback
check "rollback again -> 0.4.0" '[ "$(ver)" = 0.4.0 ]'

inst "$r4" --no-supplicant
check "reinstall same version is fine" '[ "$(ver)" = 0.4.0 ] && [ -L "$base/previous" ]'

inst "$r4" --uninstall
check "uninstall removes install, link, menu entry" '[ ! -e "$base" ] && [ ! -e "$HOME/.local/bin/rokast" ] && [ ! -e "$desktop" ]'
check "uninstall keeps settings" '[ -f "$HOME/.config/rokast/rokast.conf" ]'

if [ "$fails" != 0 ]; then echo "test_install: $fails failed"; exit 1; fi
echo "test_install: all ok"
