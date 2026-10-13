#!/usr/bin/env bash
# Every Python test_* has a Rust fn test_* of the same name, or is listed below with the phase that ports it.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
# name  phase   (ported with the code they drive)
deferred=$(cat <<'LIST'
test_allowed_hosts R5
test_auto_update_decision R5
test_good_tarball R5
test_no_ssh_keygen_refuses_before_downloading R5
test_unsafe_tarballs_are_refused R5
test_missing_signature_or_key R5
test_one_changed_byte_is_rejected R5
test_shipped_signers_file_is_valid R5
test_sign_and_verify R5
test_unlisted_key_fails_to_sign_and_verify R5
LIST
)
py=$(grep -ohE 'def test_\w+' tests/test_*.py | cut -c5- | sort -u)
rs=$(grep -rohE 'fn test_\w+' src/rokast-core src/rokast | cut -c4- | sort -u)
missing=0
for t in $py; do
    grep -qx "$t" <<<"$rs" && continue
    phase=$(awk -v t="$t" '$1 == t {print $2}' <<<"$deferred")
    if [ -n "$phase" ]; then echo "deferred to $phase: $t"; else echo "MISSING in Rust: $t"; missing=$((missing + 1)); fi
done
stale=$(awk '{print $1}' <<<"$deferred" | while read -r t; do grep -qx "$t" <<<"$rs" && echo "$t"; done || true)
[ -z "$stale" ] || { echo "ported now, drop from the deferred list: $stale"; missing=$((missing + 1)); }
echo "coverage: $(wc -l <<<"$py") python tests, $missing problems"
[ "$missing" -eq 0 ]
