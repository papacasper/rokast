#!/usr/bin/env bash
# Python and Rust give the same answers: runs tests/parity/cases.json through src/miracast and rokast-core.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
out=$(mktemp -d)
trap 'rm -rf "$out"' EXIT
python3 -I "$here/py_side.py" > "$out/py"
cargo run -q --locked --manifest-path "$here/../../Cargo.toml" -p rokast-core --example parity -- "$here/cases.json" > "$out/rs"
python3 -I "$here/compare.py" "$here/cases.json" "$out/py" "$out/rs"
