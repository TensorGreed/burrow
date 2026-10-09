#!/usr/bin/env bash
# Adversarial self-test for tools/check-afm-provenance.sh (ADR 0030). Each case plants one defect
# in a COPY of the directory and record and requires the refusal BY NAME.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
check="$here/check-afm-provenance.sh"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

EXPECTED_CASES=6
pass=0
fail=0
ok() { echo "  ok   $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL $1" >&2; fail=$((fail + 1)); }

fresh() {
  rm -rf "$work/afm" "$work/record.toml"
  cp -r "$repo/third_party/adobe-core14-afm" "$work/afm"
  cp "$repo/third_party/adobe-core14-afm.toml" "$work/record.toml"
}
# Runs the check on the copy, requiring exit $2 and $3 in its output.
expect() {
  local name="$1" status_wanted="$2" needle="$3"
  set +e
  out="$(AFM_DIR="$work/afm" AFM_RECORD="$work/record.toml" "$check" 2>&1)"
  status=$?
  set -e
  if [ "$status" -eq "$status_wanted" ] && grep -qF -- "$needle" <<<"$out"; then
    ok "$name"
  else
    bad "$name (exit $status, wanted $status_wanted and: $needle)"
    sed 's/^/         /' <<<"$out" >&2
  fi
}

echo "check-afm-provenance.sh:"

fresh
expect "the committed files pass, and the count is reported" 0 "15 of 15 recorded file(s) examined"

fresh
printf 'X' >>"$work/afm/Helvetica.afm"
expect "an edited AFM is refused by name" 1 "Helvetica.afm differs from the published file"

fresh
rm "$work/afm/MustRead.html"
expect "the AFMs without their notice are refused" 1 "MustRead.html is recorded and missing"

fresh
cp "$work/afm/Courier.afm" "$work/afm/Extra.afm"
expect "a file added to the archive is refused by name" 1 "Extra.afm is in "

fresh
python3 - "$work/record.toml" <<'PY'
import sys
p = sys.argv[1]; s = open(p).read()
assert 'notice = "MustRead.html"' in s
open(p, "w").write(s.replace('notice = "MustRead.html"', 'notice = "ReadMe.txt"'))
PY
expect "a record whose notice is not among its files is refused" 1 "could ship without it"

fresh
python3 - "$work/record.toml" <<'PY'
import sys, re
p = sys.argv[1]; s = open(p).read()
t = re.sub(r'\[files\].*', '[files]\n', s, flags=re.S)
assert t != s
open(p, "w").write(t)
PY
expect "a record of no files is refused, not passed as nothing to check" 1 "records no files"

echo
echo "$pass passed, $fail failed"
if [ "$((pass + fail))" -ne "$EXPECTED_CASES" ]; then
  echo "FAILED -- $((pass + fail)) case(s) ran, $EXPECTED_CASES expected." >&2
  exit 1
fi
[ "$fail" -eq 0 ]
