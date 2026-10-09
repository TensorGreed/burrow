#!/usr/bin/env bash
# tools/check-standard14-table.sh refuses a committed table the AFMs and PDFium do not produce.
#
# The shape it exists for is a PDFium bump that moves a bundled width: the measurement then
# disagrees with the committed table. Planting the disagreement on the COMMITTED side -- one width
# changed in place -- is the same diff, and needs no second PDFium. The file is restored byte for
# byte, and the restore asserted, on every path.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
table="$repo/core/burrow-engines/src/pdfsyntax/standard14_table.rs"
work="$(mktemp -d)"
cp "$table" "$work/original.rs"
trap 'cp "$work/original.rs" "$table"; rm -rf "$work"' EXIT

EXPECTED_CASES=2
pass=0
fail=0
ok() { echo "  ok   $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL $1" >&2; fail=$((fail + 1)); }

echo "check-standard14-table.sh:"

set +e
out="$("$here/check-standard14-table.sh" 2>&1)"
status=$?
set -e
if [ "$status" -eq 0 ] && grep -qF "names agreeing per style: Courier" <<<"$out"; then
  ok "the committed table passes, and the agreement per style is reported"
else
  bad "the committed table passes (exit $status)"
  sed 's/^/         /' <<<"$out" >&2
fi

# ONE WIDTH MOVED, as a PDFium bump would move it: the first `556,` in the WIDTHS block becomes 557.
python3 - "$table" <<'PY'
import sys
p = sys.argv[1]; s = open(p).read()
at = s.index("pub const WIDTHS")
hit = s.index(" 556,", at)
mutated = s[:hit] + " 557," + s[hit + len(" 556,"):]
assert mutated != s
open(p, "w").write(mutated)
PY
if cmp -s "$table" "$work/original.rs"; then
  bad "the planted width did not apply, so this case measured nothing"
else
  set +e
  out="$("$here/check-standard14-table.sh" 2>&1)"
  status=$?
  set -e
  if [ "$status" -ne 0 ] && grep -qF "is not what the AFMs and the pinned PDFium produce" <<<"$out"; then
    ok "a width the measurement does not produce is refused"
  else
    bad "a width the measurement does not produce is refused (exit $status)"
    sed 's/^/         /' <<<"$out" >&2
  fi
fi
cp "$work/original.rs" "$table"
cmp -s "$table" "$work/original.rs" || { echo "FAILED -- the table was not restored" >&2; exit 1; }

echo
echo "$pass passed, $fail failed"
if [ "$((pass + fail))" -ne "$EXPECTED_CASES" ]; then
  echo "FAILED -- $((pass + fail)) case(s) ran, $EXPECTED_CASES expected." >&2
  exit 1
fi
[ "$fail" -eq 0 ]
