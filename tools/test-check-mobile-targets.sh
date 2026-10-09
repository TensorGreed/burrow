#!/usr/bin/env bash
# Adversarial self-test for tools/check-mobile-targets.sh (#293). Each case requires a refusal or a
# listing BY CONTENT, not by exit code alone. Nothing here runs `cargo check`.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
check="$here/check-mobile-targets.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

EXPECTED_CASES=4
pass=0
fail=0
ok() { echo "  ok   $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL $1" >&2; fail=$((fail + 1)); }

echo "check-mobile-targets.sh:"

# 1. The real ci.yml's matrix is read: exactly the two targets CI checks today.
out="$("$check" --list)"
if [ "$out" = "$(printf 'aarch64-apple-ios\naarch64-linux-android')" ]; then
  ok "the targets are read from ci.yml's mobile matrix"
else
  bad "the targets are read from ci.yml's mobile matrix (got: $out)"
fi

# 2. A target ADDED to the matrix is checked without editing the script.
python3 - "$repo/.github/workflows/ci.yml" "$work/ci.yml" <<'PY'
import sys
s = open(sys.argv[1], encoding="utf-8").read()
anchor = "          - aarch64-linux-android\n"
assert s.count(anchor) == 1, "the mobile matrix moved; update this case"
open(sys.argv[2], "w", encoding="utf-8").write(s.replace(anchor, anchor + "          - x86_64-linux-android\n"))
PY
out="$(CI_YML="$work/ci.yml" "$check" --list)"
if grep -qxF x86_64-linux-android <<<"$out" && [ "$(wc -l <<<"$out")" -eq 3 ]; then
  ok "a target added to the matrix is checked without editing the script"
else
  bad "a target added to the matrix is checked (got: $out)"
fi

# 3. A ci.yml with no mobile matrix is refused, not read as "nothing to check".
python3 - "$repo/.github/workflows/ci.yml" "$work/none.yml" <<'PY'
import sys
s = open(sys.argv[1], encoding="utf-8").read()
assert s.count("\n  mobile:\n") == 1, "the mobile job moved; update this case"
open(sys.argv[2], "w", encoding="utf-8").write(s.replace("\n  mobile:\n", "\n  renamed:\n"))
PY
set +e
out="$(CI_YML="$work/none.yml" "$check" 2>&1)"
status=$?
set -e
if [ "$status" -ne 0 ] && grep -qF "no targets found" <<<"$out"; then
  ok "a workflow with no mobile matrix is refused by name"
else
  bad "a workflow with no mobile matrix is refused by name (exit $status): $out"
fi

# 4. A target that is not installed is refused by name, not skipped.
set +e
out="$("$check" burrow-not-a-real-target 2>&1)"
status=$?
set -e
if [ "$status" -ne 0 ] && grep -qF "target burrow-not-a-real-target is not installed" <<<"$out"; then
  ok "a target that is not installed is refused by name"
else
  bad "a target that is not installed is refused by name (exit $status): $out"
fi

echo
echo "$pass passed, $fail failed"
if [ "$((pass + fail))" -ne "$EXPECTED_CASES" ]; then
  echo "FAILED -- $((pass + fail)) case(s) ran, $EXPECTED_CASES expected." >&2
  exit 1
fi
[ "$fail" -eq 0 ]
