#!/usr/bin/env bash
# Before pushing: what CI's quick jobs check (fmt, clippy, tests, frozen answers, the app's tests, shellcheck).
# Prints "check: ok" or stops at the first failure.
set -euo pipefail
cd "$(dirname "$(readlink -f "$0")")/.."
cargo fmt --all -- --check
cargo clippy -q --workspace --all-targets --locked -- -D warnings
cargo test -q --workspace --locked
bash tests/parity/run.sh
bash tests/parity/cli.sh
(cd ui && cargo test -q --locked)
for f in scripts/* packaging/stage packaging/supplicant packaging/deb/build-deb packaging/rpm/build-rpm packaging/aur/update; do
    bash -n "$f"
done
shellcheck scripts/* tests/*.sh tests/parity/*.sh packaging/stage packaging/supplicant packaging/deb/build-deb \
    packaging/rpm/build-rpm packaging/aur/update
echo "check: ok"
