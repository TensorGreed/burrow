#!/usr/bin/env bash
# Test that the known-crashes ledger reports what it owns and refuses what it does not.
#
# The case that justifies the whole design is case 4: the SAME frame under a different verdict is
# a different defect. #62's page-tree use-after-free and #119's stack overflow both go through
# `pushInheritedAttributesToPageInternal` -- measured, from two reproductions in this repository
# -- so a ledger keyed on the frame alone would have silenced #119 the moment #62 was entered,
# which is the exact failure this mechanism exists to prevent.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
work="$(mktemp -d)"
mutant="$here/check-known-crashes.MUTANT.py"
trap 'rm -rf "$work" "$mutant"' EXIT

EXPECTED_CASES=12
pass=0
fail=0
ok() { echo "  ok   $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL $1" >&2; fail=$((fail + 1)); }

report() {
  printf 'ERROR: AddressSanitizer: %s on address 0x1\n    #0 0x1 in %s(QPDFObjectHandle)\n' \
    "$1" "$2" > "$work/report.log"
}

expect() {
  local name="$1" want="$2" needle="$3"
  local out status=0
  out="$(python3 "${4:-$here/check-known-crashes.py}" --ledger "$work/ledger.toml" \
    --log "$work/report.log" 2>&1)" || status=$?
  if [ "$status" -ne "$want" ]; then
    bad "$name (exit $status, wanted $want)"
    sed 's/^/         /' <<<"$out" >&2
  elif ! grep -qF "$needle" <<<"$out"; then
    bad "$name: right exit, wrong reason"
    echo "       expected: $needle" >&2
    sed 's/^/         /' <<<"$out" >&2
  else
    ok "$name"
  fi
}

# A LEDGER OF ITS OWN, for the same reason `PROBE_LEDGER` exists in the tool: the day #62 is
# fixed, `--check-issues` will correctly demand its entries be deleted -- and a suite asserting
# against those entries would go red in CI and block the very cleanup the mechanism requires.
# The fixture keeps the SHAPE that matters: two issues sharing a frame under different verdicts.
cat > "$work/ledger.toml" <<'LEDGER'
[[known]]
issue = 1
verdict = "heap-use-after-free"
frame = "Shared::Frame"
note = "a fixture, not a real defect"

[[known]]
issue = 2
verdict = "stack-overflow"
frame = "Shared::Frame"
note = "the same frame, a different verdict -- the case the pair exists for"

[[known]]
issue = 3
verdict = "heap-use-after-free"
frame = "Distinct::Frame"
note = "a fixture with a frame of its own"
LEDGER

echo "check-known-crashes.py:"

# 1-3. The three defects this repository actually owns, by their real (verdict, frame) pairs.
report "heap-use-after-free" "Distinct::Frame"
expect "a crash with its own frame resolves to its own issue" 0 "#3"

report "heap-use-after-free" "Shared::Frame"
expect "a shared frame under one verdict resolves to that verdict's issue" 0 "#1"

report "stack-overflow" "Shared::Frame"
expect "THE SAME FRAME under another verdict resolves elsewhere" 0 "#2"

# 4. THE LIVE LEDGER IS VALID, which is a different question from how matching behaves. This is
#    the only case touching the real file, and it asserts only that it loads and validates -- so
#    deleting #62's entries when #62 closes cannot make this suite red.
if python3 "$here/check-known-crashes.py" --check >/dev/null 2>&1; then
  ok "the committed ledger loads and every entry validates"
else
  bad "the committed ledger loads and every entry validates"
fi

# 5. A NEW DEFECT FAILS. The entire point: an unowned crash is what this nightly is for.
report "heap-buffer-overflow" "Nobody::FiledThis"
expect "an unowned crash is a new finding and fails" 1 "UNMATCHED"

# 6. A NEW DEFECT IN AN OWNED FUNCTION STILL FAILS, if the verdict differs. SEGV is uppercase,
#    which an earlier verdict pattern silently dropped -- and it is how a stack overflow surfaces
#    when ASan's own detector does not engage, i.e. #119's class.
report "SEGV" "Shared::Frame"
expect "a new verdict in an owned function still fails" 1 "UNMATCHED"

# 7. A LOG THAT IS NOT A CRASH REPORT is the caller's problem, and says so rather than guessing.
printf 'error[E0432]: unresolved import\nerror: could not compile\n' > "$work/report.log"
expect "a build failure is refused as not-a-crash-report" 1 "not a crash report"

# 8. AN UNRESOLVABLE REPORT IS NOT A FINDING. With no frames nothing can match, and calling that
#    a new defect announces a known one as new. Measured as a real defect in the first version.
printf 'ERROR: AddressSanitizer: heap-use-after-free on address 0x1\n    #0 0x55 (/nope/x+0x12)\n' \
  > "$work/report.log"
expect "a report whose frames cannot be resolved says so, rather than claiming a new defect" \
  1 "UNCLASSIFIED"

# 9. A MULTI-WORD VERDICT. `deadly signal` was captured as `deadly`, so an entry spelling it
#    correctly could never have matched.
cat > "$work/phrase.toml" <<'LEDGER'
[[known]]
issue = 7
verdict = "deadly-signal"
frame = "Some::Frame"
note = "a libFuzzer verdict that is two words"
LEDGER
printf 'ERROR: libFuzzer: deadly signal\n    #0 0x1 in Some::Frame(Thing)\n' > "$work/report.log"
if out="$(python3 "$here/check-known-crashes.py" --ledger "$work/phrase.toml" --log "$work/report.log" 2>&1)" \
   && grep -qF "#7" <<<"$out"; then
  ok "a two-word verdict is normalised and matches its entry"
else
  bad "a two-word verdict is normalised and matches its entry"
  sed 's/^/         /' <<<"${out:-}" >&2
fi

# 10. AN ENTRY THE CLASSIFIER COULD NEVER RESOLVE IS REFUSED, not accepted in silence.
cat > "$work/bad.toml" <<'LEDGER'
[[known]]
issue = 7
verdict = "deadly signal"
frame = "Some::Frame"
note = "a space, so this can never match a normalised verdict"
LEDGER
if ! python3 "$here/check-known-crashes.py" --ledger "$work/bad.toml" --check >/dev/null 2>&1; then
  ok "a verdict that can never match is refused when the ledger is validated"
else
  bad "a verdict that can never match is refused when the ledger is validated"
fi

# 11. AN ENTRY WITH NO REASON IS REFUSED. An unexplained entry is a mute button.
cat > "$work/noreason.toml" <<'LEDGER'
[[known]]
issue = 7
verdict = "stack-overflow"
frame = "Some::Frame"
LEDGER
if ! python3 "$here/check-known-crashes.py" --ledger "$work/noreason.toml" --check >/dev/null 2>&1; then
  ok "an entry with no written reason is refused"
else
  bad "an entry with no written reason is refused"
fi

# 12. THE META-TEST. Break the pair in a copy beside the original -- match on frame alone -- and
#    require case 3 to resolve to the WRONG issue, which is what the pair prevents.
python3 - "$here/check-known-crashes.py" "$mutant" <<'PY'
import sys
from pathlib import Path
source = Path(sys.argv[1]).read_text(encoding="utf-8")
old = 'matches = [e for e in entries if e["verdict"] == verdict and e["frame"] in frames]'
assert old in source, "the matching rule moved; update this meta-test rather than deleting it"
mutated = source.replace(old, 'matches = [e for e in entries if e["frame"] in frames]', 1)
assert mutated != source, "the mutation did not apply"
Path(sys.argv[2]).write_text(mutated, encoding="utf-8")
PY
report "stack-overflow" "QPDF::Doc::Pages::pushInheritedAttributesToPageInternal"
set +e
out="$(python3 "$mutant" --log "$work/report.log" 2>&1)"
status=$?
set -e
# The mutant never reaches classification: its PROBE GATE refuses first, naming the two fixtures
# that a frame-only match conflates. That is the gate doing its job, and it is the stronger
# result -- the rule is protected before any real log is looked at. The assertion is therefore
# "refuses, naming the conflation", not "mis-attributes".
if [ "$status" -ne 0 ] && grep -qF "DIFFERENT defect: matched" <<<"$out"; then
  ok "the probe gate refuses a frame-only match, naming the two defects it would conflate"
else
  bad "the probe gate refuses a frame-only match, naming the conflation (exit $status)"
  sed 's/^/         /' <<<"$out" >&2
fi

echo
echo "$pass passed, $fail failed"
if [ "$((pass + fail))" -ne "$EXPECTED_CASES" ]; then
  echo "FAILED -- $((pass + fail)) case(s) ran, $EXPECTED_CASES expected." >&2
  exit 1
fi
[ "$fail" -eq 0 ] || exit 1
