#!/usr/bin/env bash
# `rokast update` end to end in a throwaway HOME against a local fake GitHub (python http.server): signed release
# tarballs built from this working tree with a throwaway key, installed with scripts/install. Checks the update, the
# rollback, and that every bad download or bad moment changes nothing. No docker, no network. Run by CI.
# Needs `cargo build --release -p rokast`.
# Last, the upgrade across the switch: a Python-only install (v0.4.0) updates itself to a Rust release.
# shellcheck disable=SC2016  # check strings are single-quoted on purpose: eval expands them
set -euo pipefail
repo="$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)"
t="$(mktemp -d)"
export HOME="$t/home" PATH="/usr/bin:/bin"
unset XDG_DATA_HOME XDG_CONFIG_HOME XDG_CACHE_HOME XDG_RUNTIME_DIR CHECK_UPDATES
mkdir -p "$HOME" "$t/srv"
cli="$repo/target/release/rokast"
[ -x "$cli" ] || { echo "test_self_update: build it first: cargo build --release --locked -p rokast" >&2; exit 1; }
export base="$HOME/.local/share/rokast" dl="$HOME/.cache/rokast/download"
fails=0
check() { if eval "$2"; then echo "ok    $1"; else echo "FAIL  $1   [$2]"; cat "$t/out"; fails=$((fails + 1)); fi; }

ssh-keygen -q -t ed25519 -N "" -C test -f "$t/key"
ssh-keygen -q -t ed25519 -N "" -C other -f "$t/other"
echo "rokast-release namespaces=\"rokast-release\" $(cut -d' ' -f1,2 "$t/key.pub")" > "$t/signers"

port="$(python3 -I -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1])')"
python3 -I -m http.server -b 127.0.0.1 -d "$t/srv" "$port" >/dev/null 2>&1 & srv=$!
trap 'kill $srv 2>/dev/null; rm -rf "$t"' EXIT
url="http://127.0.0.1:$port"

tree() {   # tree <version>: a release folder like the tarball's (tracked files, prebuilt rokast, stub UI, test signers)
    local d="$t/build/rokast-$1"
    mkdir -p "$d"
    git -C "$repo" ls-files -z -co --exclude-standard | (cd "$repo" && xargs -0 cp --parents -a -t "$d" --)
    echo "$1" > "$d/VERSION"
    cp "$t/signers" "$d/config/release-signers"
    mkdir -p "$d/ui/target/release"
    printf '#!/bin/sh\necho stub-ui %s\n' "$1" > "$d/ui/target/release/rokast-ui"; chmod +x "$d/ui/target/release/rokast-ui"
    install -Dm755 "$cli" "$d/target/release/rokast"
    echo "$d"
}

publish() {   # publish <scenario> <version> [prerelease]: tarball + SHA256SUMS + .sig + latest.json under srv/<scenario>
    local s="$t/srv/$1" v="$2" tb="rokast-$2-x86_64-linux.tar.gz"
    mkdir -p "$s"
    [ -d "$t/build/rokast-$v" ] || tree "$v" > /dev/null
    tar -C "$t/build" -czf "$s/$tb" "rokast-$v"
    (cd "$s" && sha256sum "$tb" > SHA256SUMS)
    ROKAST_SIGNING_KEY="$(cat "$t/key")" ROKAST_SIGNERS="$t/signers" "$repo/scripts/sign-release" "$s/SHA256SUMS" > /dev/null
    python3 -I - "$s" "$v" "$url/$1" "${3:-false}" <<'EOF'
import json, os, sys
s, v, u, pre = sys.argv[1:]
names = [n for n in sorted(os.listdir(s)) if n != "latest.json"]
json.dump({"tag_name": "v" + v, "html_url": u, "draft": False, "prerelease": pre == "true",
           "assets": [{"name": n, "browser_download_url": f"{u}/{n}"} for n in names]}, open(os.path.join(s, "latest.json"), "w"))
EOF
}

upd() {   # upd <scenario> [args...]: the installed rokast's `update`; exit code in $st
    if ROKAST_UPDATE_URL="$url/$1/latest.json" "$HOME/.local/bin/rokast" update "${@:2}" > "$t/out" 2>&1; then st=0; else st=$?; fi
    export st
}
ver() { "$HOME/.local/bin/rokast" version; }
unchanged() { [ "$(ver)" = "$1" ] && [ ! -e "$dl" ]; }

# Install 0.9.0 with a supplicant whose stamp matches the patches, so the update has nothing to build.
a="$(tree 0.9.0)"
"$a/scripts/install" --no-supplicant > "$t/out" 2>&1
mkdir -p "$base/supplicant" "$HOME/.config/rokast"
printf '#!/bin/sh\n' > "$base/supplicant/wpa_supplicant"; chmod +x "$base/supplicant/wpa_supplicant"
cat "$repo/patches/hostap-2.11-roku.patch" "$repo/patches/hostap.config" | sha256sum | cut -d' ' -f1 > "$base/supplicant/patch.sha256"
echo "TARGET=Den" > "$HOME/.config/rokast/rokast.conf"
check "installed 0.9.0" '[ "$(ver)" = 0.9.0 ]'

publish good 0.9.1
upd good --check
check "--check: says 0.9.1 is available, downloads nothing" '[ "$st" = 0 ] && grep -qx "update: available 0.9.1" "$t/out" && unchanged 0.9.0'

publish same 0.9.0
upd same --yes
check "same version: up to date" '[ "$st" = 0 ] && grep -qx "update: uptodate 0.9.0" "$t/out" && unchanged 0.9.0'

publish pre 0.9.1 true
upd pre --yes
check "prerelease: refused" '[ "$st" = 1 ] && grep -q "not a published version" "$t/out" && unchanged 0.9.0'

publish badtar 0.9.1; printf x >> "$t/srv/badtar/rokast-0.9.1-x86_64-linux.tar.gz"
upd badtar --yes
check "tampered tarball: refused" '[ "$st" = 1 ] && grep -q "does not match SHA256SUMS" "$t/out" && unchanged 0.9.0'

publish badsums 0.9.1; echo "0000000000000000000000000000000000000000000000000000000000000000  extra.tar.gz" >> "$t/srv/badsums/SHA256SUMS"   # always a change (sed s/^./0/ was a no-op when the hash began with 0)
upd badsums --yes
check "tampered SHA256SUMS: refused" '[ "$st" = 1 ] && grep -q "not a valid rokast release signature" "$t/out" && unchanged 0.9.0'

publish nosig 0.9.1; rm "$t/srv/nosig/SHA256SUMS.sig"
python3 -I -c 'import json, sys; p = sys.argv[1]; d = json.load(open(p)); d["assets"] = [x for x in d["assets"] if x["name"] != "SHA256SUMS.sig"]; json.dump(d, open(p, "w"))' "$t/srv/nosig/latest.json"
upd nosig --yes
check "unsigned release: refused" '[ "$st" = 1 ] && grep -q "has no SHA256SUMS.sig" "$t/out" && unchanged 0.9.0'

publish wrongkey 0.9.1; rm "$t/srv/wrongkey/SHA256SUMS.sig"
ssh-keygen -q -Y sign -f "$t/other" -n rokast-release "$t/srv/wrongkey/SHA256SUMS"
upd wrongkey --yes
check "signed by another key: refused" '[ "$st" = 1 ] && grep -q "not a valid rokast release signature" "$t/out" && unchanged 0.9.0'

upd good    # no --yes and no terminal
check "no --yes without a terminal: refused" '[ "$st" = 1 ] && grep -q "not confirmed" "$t/out" && unchanged 0.9.0'

mkdir -p "$HOME/.cache/rokast"   # a running mirror session holds this lock (update::lock_file, no XDG_RUNTIME_DIR)
( exec 9>"$HOME/.cache/rokast/session.lock"; flock 9; echo held; exec sleep 60 ) > "$t/lock" & locker=$!
for _ in $(seq 50); do grep -q held "$t/lock" && break; sleep 0.1; done
upd good --yes
kill $locker; wait $locker 2>/dev/null || true
check "while mirroring: refused" '[ "$st" = 1 ] && grep -q "stop mirroring first" "$t/out" && unchanged 0.9.0'

if ROKAST_UPDATE_URL="http://127.0.0.1:9/latest.json" "$HOME/.local/bin/rokast" update --yes > "$t/out" 2>&1; then st=0; else st=$?; fi
check "server down: clear error" '[ "$st" = 1 ] && grep -q "could not reach" "$t/out" && unchanged 0.9.0'

if ROKAST_UPDATE_URL="$url/good/latest.json" "$repo/scripts/rokast" update --yes > "$t/out" 2>&1; then st=0; else st=$?; fi
check "git clone: told to git pull" '[ "$st" = 1 ] && grep -q "git pull" "$t/out" && [ ! -e "$dl" ]'

upd good --auto
check "--auto with AUTO_UPDATE off: skipped, silent success" '[ "$st" = 0 ] && grep -qx "update: skipped AUTO_UPDATE is off" "$t/out" && unchanged 0.9.0'

echo "AUTO_UPDATE=1" >> "$HOME/.config/rokast/rokast.conf"
upd good --auto
check "update (--auto, AUTO_UPDATE=1): exit 0, says done" '[ "$st" = 0 ] && grep -qx "update: done 0.9.1" "$t/out"'
check "update: current 0.9.1, previous 0.9.0" '[ "$(ver)" = 0.9.1 ] && [ "$(basename "$(readlink "$base/previous")")" = 0.9.0 ]'
check "update: settings kept, download removed" 'grep -qx "TARGET=Den" "$HOME/.config/rokast/rokast.conf" && [ ! -e "$dl" ]'
sed -i '/AUTO_UPDATE/d' "$HOME/.config/rokast/rokast.conf"
check "update: menu entry runs the new app" '[ "$("$base/current/ui/target/release/rokast-ui")" = "stub-ui 0.9.1" ]'

upd good --rollback
check "rollback: back to 0.9.0" '[ "$st" = 0 ] && [ "$(ver)" = 0.9.0 ]'

b="$(tree 0.9.2)"; printf '#!/bin/sh\necho broken\n' > "$b/scripts/rokast"
publish broken 0.9.2
upd broken --yes
check "new version does not run: rolled back" '[ "$st" = 1 ] && grep -q "rolled back" "$t/out" && [ "$(ver)" = 0.9.0 ] && [ ! -e "$dl" ]'

# Across the switch, in a fresh HOME: install the last Python-only release (v0.4.0; until it is tagged, main stands
# in for it: it ships no rokast binary, so it runs Python), publish a Rust release, update with the old Python updater,
# then roll back with the Rust one.
old_ref="$(git -C "$repo" rev-parse -q --verify v0.4.0 || git -C "$repo" rev-parse -q --verify origin/main || echo main)"
export HOME="$t/home2" base="$t/home2/.local/share/rokast" dl="$t/home2/.cache/rokast/download"
mkdir -p "$HOME" "$t/build/rokast-0.4.0" "$base/supplicant"
git -C "$repo" archive "$old_ref" | tar -x -C "$t/build/rokast-0.4.0"
echo 0.4.0 > "$t/build/rokast-0.4.0/VERSION"
cp "$t/signers" "$t/build/rokast-0.4.0/config/release-signers"
printf '#!/bin/sh\n' > "$base/supplicant/wpa_supplicant"; chmod +x "$base/supplicant/wpa_supplicant"
cat "$repo/patches/hostap-2.11-roku.patch" "$repo/patches/hostap.config" | sha256sum | cut -d' ' -f1 > "$base/supplicant/patch.sha256"
"$t/build/rokast-0.4.0/scripts/install" --no-supplicant > "$t/out" 2>&1
check "switch: Python 0.4.0 installed" '[ "$(ver)" = 0.4.0 ] && [ ! -e "$base/current/target/release/rokast" ]'
publish switch 0.5.0
upd switch --yes
check "switch: the Python updater installs the Rust 0.5.0" '[ "$st" = 0 ] && grep -qx "update: done 0.5.0" "$t/out" && [ "$(ver)" = 0.5.0 ]'
check "switch: the installed rokast is the Rust binary" '[ "$("$base/current/target/release/rokast" version)" = 0.5.0 ]'
upd switch --check
check "switch: the Rust updater says up to date" '[ "$st" = 0 ] && grep -qx "update: uptodate 0.5.0" "$t/out"'
upd switch --rollback
check "switch: rollback returns to 0.4.0" '[ "$st" = 0 ] && [ "$(ver)" = 0.4.0 ]'

if [ "$fails" != 0 ]; then echo "test_self_update: $fails failed"; exit 1; fi
echo "test_self_update: all ok"
