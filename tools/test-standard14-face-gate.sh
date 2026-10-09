#!/usr/bin/env bash
# The standard-14 face gate refuses a face it was not measured on (#290, the owner's condition 1).
#
# tests/standard14_measure.rs::pdfium_draws_each_standard_fourteen_name_from_its_bundled_face asks
# PDFium which face it actually loaded. This SUBSTITUTES one -- a generated block face whose family
# name is "Helvetica", in a directory PDFium searches as a user font path -- and requires the gate
# to go red naming it. The control runs the same gate with no substitute and requires it green, so
# the red cannot be a gate that fails for every reason.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

EXPECTED_CASES=2
pass=0
fail=0
ok() { echo "  ok   $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL $1" >&2; fail=$((fail + 1)); }

gate() {
  (cd "$repo" && cargo test --quiet -p burrow-engines --features native-engines \
    --test standard14_measure pdfium_draws_each_standard_fourteen_name_from_its_bundled_face \
    -- --exact 2>&1)
}

echo "standard-14 face gate:"

set +e
out="$(gate)"
status=$?
set -e
if [ "$status" -eq 0 ]; then
  ok "the bundled faces pass the gate (control)"
else
  bad "the bundled faces pass the gate (control), exit $status"
  sed 's/^/         /' <<<"$out" >&2
fi

mkdir "$work/fonts"
python3 - "$repo/tools" "$work/fonts/Helvetica.ttf" <<'PY'
import sys
sys.path.insert(0, sys.argv[1])
import blockfont
font, _ = blockfont.build("ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789- ", family="Helvetica")
open(sys.argv[2], "wb").write(font)
PY
if [ ! -s "$work/fonts/Helvetica.ttf" ]; then
  bad "the substitute face was not built, so the mutation measured nothing"
else
  set +e
  out="$(BURROW_PDFIUM_FONT_PATHS="$work/fonts" gate)"
  status=$?
  set -e
  if [ "$status" -ne 0 ] && grep -qF 'Type1 /Helvetica: PDFium loaded "Helvetica"' <<<"$out"; then
    ok "a substituted Helvetica face is refused, naming the face PDFium loaded"
  else
    bad "a substituted Helvetica face is refused by name (exit $status)"
    sed 's/^/         /' <<<"$out" >&2
  fi
fi

echo
echo "$pass passed, $fail failed"
if [ "$((pass + fail))" -ne "$EXPECTED_CASES" ]; then
  echo "FAILED -- $((pass + fail)) case(s) ran, $EXPECTED_CASES expected." >&2
  exit 1
fi
[ "$fail" -eq 0 ]
