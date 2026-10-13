#!/usr/bin/env bash
# Every Python test_* has a Rust fn test_* of the same name, or is listed below with the phase that ports it.
set -euo pipefail
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
# name  phase   (ported with the code they drive)
deferred=$(cat <<'LIST'
test_all_attempts_fail_keeps_every_attempt R3
test_first_try_join_keeps_nothing R3
test_join_after_a_retry_is_kept_too R3
test_probes_for_the_tv_by_name R3
test_tv_not_visible_is_kept_with_its_error R3
test_full_handshake_and_play R3
test_failed_join_is_retried_once R3
test_failure_after_streaming_is_not_retried R3
test_first_round_works R3
test_gives_up_after_the_second_round R3
test_no_rtsp_is_retried R3
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
