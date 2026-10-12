#!/usr/bin/env bash
# tests/test_pkg_update.sh deb|rpm: `rokast update` on a package install, in a clean ubuntu:22.04 / fedora:43
# container. Builds packages A (0.9.0) and B (0.9.1) from this tree with a throwaway release key, installs A, serves a
# signed B from inside the container, then:
#   as a normal user without pkexec -> refuses to guess: keeps the verified package and prints the sudo command
#   as root                         -> installs B with apt-get/dnf
# Needs docker, ui/target/release/rokast-ui built, and the supplicant build deps (built once, reused). Run by CI.
# shellcheck disable=SC2016  # check strings are single-quoted on purpose: eval expands them
set -euo pipefail
repo="$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)"
fmt="${1:?usage: tests/test_pkg_update.sh deb|rpm}"
command -v docker >/dev/null || { echo "test_pkg_update: skipped (docker missing)"; exit 0; }
case "$fmt" in
    deb) image="${ROKAST_DEB_IMAGE:-ubuntu:22.04}"; a=rokast_0.9.0_amd64.deb; b=rokast_0.9.1_amd64.deb
         inst='export DEBIAN_FRONTEND=noninteractive; apt-get update -qq && apt-get install -y -qq /pkg/'"$a" ;;
    rpm) image="${ROKAST_RPM_IMAGE:-fedora:43}"; a=rokast-0.9.0-1.x86_64.rpm; b=rokast-0.9.1-1.x86_64.rpm
         inst="dnf install -y -q /pkg/$a" ;;
    *) echo "usage: tests/test_pkg_update.sh deb|rpm" >&2; exit 2 ;;
esac
[ -x "$repo/ui/target/release/rokast-ui" ] || { echo "test_pkg_update: build the UI first" >&2; exit 1; }
t="$(mktemp -d)"; trap 'rm -rf "$t"' EXIT
mkdir -p "$t/pkg" "$t/srv"
ssh-keygen -q -t ed25519 -N "" -C test -f "$t/key"
echo "rokast-release namespaces=\"rokast-release\" $(cut -d' ' -f1,2 "$t/key.pub")" > "$t/signers"
"$repo/packaging/supplicant" "$t/sup"

for v in 0.9.0 0.9.1; do
    d="$t/tree-$v"; mkdir -p "$d"
    git -C "$repo" ls-files -z -co --exclude-standard | (cd "$repo" && xargs -0 cp --parents -a -t "$d" --)
    echo "$v" > "$d/VERSION"; cp "$t/signers" "$d/config/release-signers"
    install -Dm755 "$repo/ui/target/release/rokast-ui" "$d/ui/target/release/rokast-ui"
    ROKAST_PREBUILT_SUPPLICANT="$t/sup" "$d/packaging/$fmt/build-$fmt" "$t/pkg" > /dev/null
done
mv "$t/pkg/$b" "$t/srv/"
(cd "$t/srv" && sha256sum "$b" > SHA256SUMS)
ROKAST_SIGNING_KEY="$(cat "$t/key")" ROKAST_SIGNERS="$t/signers" "$repo/scripts/sign-release" "$t/srv/SHA256SUMS" > /dev/null
python3 -I - "$t/srv" <<'EOF'
import json, os, sys
s = sys.argv[1]
json.dump({"tag_name": "v0.9.1", "html_url": "http://127.0.0.1:8000/", "draft": False, "prerelease": False,
           "assets": [{"name": n, "browser_download_url": "http://127.0.0.1:8000/" + n} for n in sorted(os.listdir(s))]},
          open(os.path.join(s, "latest.json"), "w"))
EOF

docker run --rm -v "$t/pkg:/pkg:ro" -v "$t/srv:/srv:ro" "$image" bash -c "
    $inst > /tmp/inst.log 2>&1 || { tail -20 /tmp/inst.log; exit 1; }"'
    echo "before=$(rokast version)"
    python3 -I -m http.server -b 127.0.0.1 -d /srv 8000 > /dev/null 2>&1 &
    for _ in $(seq 50); do python3 -I -c "import urllib.request as u; u.urlopen(\"http://127.0.0.1:8000/latest.json\")" 2>/dev/null && break; sleep 0.2; done
    export ROKAST_UPDATE_URL=http://127.0.0.1:8000/latest.json
    mkdir -p /tmp/u && chown 1000:1000 /tmp/u
    HOME=/tmp/u python3 -I -c "import os, sys; os.setgid(1000); os.setuid(1000); os.execvp(\"rokast\", [\"rokast\", \"update\", \"--yes\"])"
    echo "user_exit=$?"
    ls /tmp/u/.cache/rokast/ | sed "s/^/kept=/"
    rokast update --yes
    echo "root_exit=$?"
    echo "after=$(rokast version)"
' > "$t/run.log" 2>&1 || true

fails=0
check() { if eval "$2"; then echo "ok    $1"; else echo "FAIL  $1   [$2]"; fails=$((fails + 1)); fi; }
check "package A installed" 'grep -qx "before=0.9.0" "$t/run.log"'
check "user without pkexec: refused, prints the command" 'grep -qx "user_exit=1" "$t/run.log" && grep -q "install it yourself: sudo" "$t/run.log"'
check "user without pkexec: verified package kept" 'grep -qx "kept=$b" "$t/run.log"'
check "root: verified and installed" 'grep -qx "update: verifying" "$t/run.log" && grep -qx "update: done 0.9.1" "$t/run.log" && grep -qx "root_exit=0" "$t/run.log"'
check "package B installed" 'grep -qx "after=0.9.1" "$t/run.log"'
if [ "$fails" != 0 ]; then cat "$t/run.log"; echo "test_pkg_update: $fails failed"; exit 1; fi
echo "test_pkg_update: all ok ($fmt on $image)"
