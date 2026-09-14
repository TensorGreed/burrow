#!/usr/bin/env bash
#
# `tools/check-subsetting-gate.sh` refuses when its own rules are broken, naming the reason.
#
# The gate's whole value is the "1 passed" match: `cargo test -- --exact <name>` on a test that is
# `#[ignore]`d, renamed or deleted exits **0** and reports `0 passed`. A gate that trusted the exit
# code would report success over a suite in which nothing ran -- which is the state the gate
# replaced, arriving through the gate itself.
#
# So each rule is broken in a COPY and required to refuse. The copy lives beside the original, for
# the reason `CLAUDE.md` records: one in a temp directory resolves its own paths wrongly and exits
# non-zero for the wrong reason, which an exit-code-only assertion reports as a pass.
set -uo pipefail

cd "$(dirname "$0")/.."
original="tools/check-subsetting-gate.sh"
copy="tools/.check-subsetting-gate.undertest.sh"
trap 'rm -f "$copy"' EXIT

pass=0
fail=0

# `name <sed-old> <sed-new> <expected text>` -- mutate the copy, run it, require a refusal that
# says why. The mutation is ASSERTED TO HAVE APPLIED first: a `replace` that matched nothing
# leaves the copy correct, the copy passes, and the case reports a working defence over a
# mutation that never happened.
mutate_and_check() {
  local name="$1" old="$2" new="$3" expect="$4"
  python3 - "$original" "$copy" "$old" "$new" <<'PY'
import sys
src, dst, old, new = sys.argv[1:5]
text = open(src).read()
assert old in text, f"the mutation target is not in {src}: {old!r}"
open(dst, "w").write(text.replace(old, new, 1))
PY
  if [ $? -ne 0 ]; then
    echo "  FAIL $name: the mutation did not apply, so this case would prove nothing"
    fail=$((fail + 1))
    return
  fi
  chmod +x "$copy"
  local out
  out=$("$copy" 2>&1)
  if [ $? -eq 0 ]; then
    echo "  FAIL $name: the checker accepted it"
    fail=$((fail + 1))
  elif grep -qF "$expect" <<<"$out"; then
    echo "  ok   $name"
    pass=$((pass + 1))
  else
    echo "  FAIL $name: it refused, but not for the stated reason ($expect)"
    sed 's/^/        /' <<<"$out" | tail -6
    fail=$((fail + 1))
  fi
}

# The unmutated gate must succeed, or every case below measures a broken baseline.
if "$original" >/dev/null 2>&1; then
  echo "  ok   the gate itself passes"
  pass=$((pass + 1))
else
  echo "  FAIL the gate does not pass, so no case below means anything"
  fail=$((fail + 1))
fi

# --- Case 1: a test that does not run ------------------------------------------------------
#
# The shape `#[ignore]` produces, simulated by naming a test that is not there. `--exact` on it
# exits 0, so only the "1 passed" match can catch it.
mutate_and_check "a gate test that does not run is caught" \
  'split_no_leak::the_scan_can_still_see_every_channel_it_is_the_gate_for' \
  'split_no_leak::a_test_that_does_not_exist' \
  "did not run"

# --- Case 2: and the "1 passed" match is WHAT catches it -------------------------------------
#
# The near-miss for case 1. Without this, case 1 would pass just as happily if the exit code were
# doing the work -- and the whole argument for this gate is that the exit code is not.
python3 - "$original" "$copy" <<'PY'
import sys
src, dst = sys.argv[1:3]
text = open(src).read()
old_name = "split_no_leak::the_scan_can_still_see_every_channel_it_is_the_gate_for"
old_grep = 'if ! grep -q "1 passed" "$log"; then'
assert old_name in text and old_grep in text, "the two mutation targets must both be present"
text = text.replace(old_name, "split_no_leak::a_test_that_does_not_exist", 1)
text = text.replace(old_grep, 'if false; then', 1)
open(dst, "w").write(text)
PY
chmod +x "$copy"
if out=$("$copy" 2>&1); then
  echo "  ok   without the \"1 passed\" match, the absent test is NOT caught (so that line is the check)"
  pass=$((pass + 1))
else
  if grep -q "did not run" <<<"$out"; then
    echo "  FAIL the \"1 passed\" match was removed and the absent test was still caught -- something else is doing the work, and case 1 proves nothing about that line"
  else
    echo "  FAIL the mutated copy failed for an unrelated reason"
    sed 's/^/        /' <<<"$out" | tail -6
  fi
  fail=$((fail + 1))
fi

# --- Case 3: the expected count ---------------------------------------------------------------
#
# A loop over an emptied list runs nothing and exits 0, which reads as success.
mutate_and_check "a gate list that lost a test is caught" \
  'expected=4' \
  'expected=5' \
  "expected 5"

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED — $fail case(s) failed, $pass passed" >&2
  exit 1
fi
echo "OK — $pass case(s) all behaved as required"
