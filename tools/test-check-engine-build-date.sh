#!/usr/bin/env bash
# Adversarial self-test for tools/check-engine-build-date.sh.
#
# FIXTURE cases build a complete fake repository (the checker resolves the repo from its own
# location) and require the REAL checker to refuse naming the reason, one per rule, each with a
# near-miss twin that must pass. A GATE case breaks a rule in a COPY of the checker placed beside
# the original -- a copy elsewhere would resolve its paths wrongly and exit non-zero for the wrong
# reason -- asserts the mutation applied, and requires the copy's probe to refuse by name.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/.." && pwd)"
tool="$here/check-engine-build-date.sh"
work="$(mktemp -d)"
gate="$here/.check-engine-build-date-gate-copy.sh"
trap 'rm -rf "$work"; rm -f "$gate"' EXIT
pass=0; fail=0
# The pin, read the way the checker reads it, so a libjpeg-turbo bump that moves build_date does
# not fail this test with a message that does not point at the pin.
# shellcheck source=engines/pins.sh
source "$repo/engines/pins.sh"
PIN_DATE="$(pins_get libjpeg-turbo.build_date)" || { echo "cannot read libjpeg-turbo.build_date" >&2; exit 1; }
PIN="$(date -u -d "$PIN_DATE" +%Y%m%d)" || { echo "bad build_date $PIN_DATE" >&2; exit 1; }
ok()  { pass=$((pass+1)); echo "  ok   $1"; }
bad() { fail=$((fail+1)); echo "  FAIL $1" >&2; }

# make_repo <dir> <qpdf-extra-bytes> <pdfium-extra-bytes> : a fake repo with the real checker.
make_repo() {
  local d="$1"
  mkdir -p "$d/tools" "$d/engines/vendor/wasm/lib"
  cp "$tool" "$d/tools/"; cp "$repo/engines/pins.sh" "$repo/engines/pins.toml" "$d/engines/"
  printf 'MZ\0libjpeg-turbo version 3.2.0 (build %s)\0%b' "$PIN" "$2" > "$d/engines/vendor/wasm/lib/qpdf.wasm"
  printf 'MZ\0%b' "$3" > "$d/engines/vendor/wasm/lib/pdfium.wasm"
  printf '!<arch>\nlibjpeg-turbo version 3.2.0 (build %s)\0' "$PIN" > "$d/engines/vendor/wasm/lib/libjpeg.a"
}
# expect <name> <dir> <want-exit> <needle-in-output>
expect() {
  local out rc
  out="$("$2/tools/check-engine-build-date.sh" 2>&1)"; rc=$?
  if { [ "$3" = 0 ] && [ $rc -eq 0 ]; } || { [ "$3" = 1 ] && [ $rc -ne 0 ]; }; then
    if grep -qF -- "$4" <<<"$out"; then ok "$1"; else bad "$1 (exit ok, output lacks: $4)"; echo "$out" >&2; fi
  else bad "$1 (exit $rc, wanted $3)"; echo "$out" >&2; fi
}

make_repo "$work/clean" '' ''
expect "clean fixtures pass and say what they examined" "$work/clean" 0 "examined 3 artifact(s)"
expect "the stamp is reported against the pin" "$work/clean" 0 "(pinned $PIN)"

make_repo "$work/stamp" '' ''
printf 'libjpeg-turbo version 3.2.0 (build 20261102)\0' >> "$work/stamp/engines/vendor/wasm/lib/qpdf.wasm"
expect "a build stamp other than the pin is refused, by file" "$work/stamp" 1 "qpdf.wasm: build-stamp (build 20261102), pinned is $PIN"

make_repo "$work/macro" '' 'x Nov  2 2026 y'
expect "a __DATE__ shape is refused" "$work/macro" 1 "pdfium.wasm: date-macro"
make_repo "$work/macro-near" '' '%%CreationDate: Tue Oct 31 01:34:22 2006\0D:20210207211528Z'
expect "near-miss: a weekday CreationDate and a PDF-spec example date pass" "$work/macro-near" 0 "check-engine-build-date: OK"

make_repo "$work/on" 'built on a laptop' ''
expect "'built on' is refused" "$work/on" 1 "qpdf.wasm: built-on"
make_repo "$work/on-near" 'built-in table, rebuilt once' ''
expect "near-miss: 'built-in' passes" "$work/on-near" 0 "check-engine-build-date: OK"

make_repo "$work/nostamp" '' ''
printf 'MZ\0no stamp here' > "$work/nostamp/engines/vendor/wasm/lib/qpdf.wasm"
expect "no stamp seen in qpdf.wasm is refused (it examined nothing)" "$work/nostamp" 1 "no libjpeg-turbo build stamp seen"

make_repo "$work/gone" '' ''
rm "$work/gone/engines/vendor/wasm/lib/pdfium.wasm"
expect "a missing required artifact is refused, by name" "$work/gone" 1 "required artifact(s) missing: engines/vendor/wasm/lib/pdfium.wasm"

make_repo "$work/bind" '' ''
out="$("$work/bind/tools/check-engine-build-date.sh" --require-bindings 2>&1)"; rc=$?
if [ $rc -ne 0 ] && grep -qF "pkg/burrow_wasm_bg.wasm" <<<"$out"; then ok "--require-bindings refuses when a binding is absent"; else bad "--require-bindings"; fi

# GATE: neuter the build-stamp rule in a copy beside the original; its probe must refuse.
cp "$tool" "$gate"
python3 -I - "$gate" <<'PY' || { bad "gate mutation did not apply"; exit 1; }
import sys
p = sys.argv[1]; s = open(p).read()
old = 'STAMP = re.compile(rb"\\(build (\\d{8})\\)")'
assert old in s, "mutation target absent"
open(p, "w").write(s.replace(old, 'STAMP = re.compile(rb"\\(never (\\d{8})\\)")'))
assert open(p).read() != s
PY
out="$("$gate" 2>&1)"; rc=$?
if [ $rc -ne 0 ] && grep -qF "probe: rule 'build-stamp' does not match its own positive fixture" <<<"$out"; then
  ok "a rule that matches nothing is refused by its probe, naming the rule"
else bad "gate: neutered rule not refused for the right reason (exit $rc)"; echo "$out" >&2; fi

echo "check-engine-build-date self-test: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
