#!/usr/bin/env bash
# rokast-core gives the answers the Python version gave: tests/parity/cases.json against expected.jsonl (frozen from
# the Python version before it was removed, docs/plan-rust.md R6). Add a case: append it to both files.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
cargo run -q --locked --manifest-path "$here/../../Cargo.toml" -p rokast-core --example parity -- \
    "$here/cases.json" "$here/expected.jsonl"
