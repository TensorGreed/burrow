#!/usr/bin/env bash
# Adversarial self-test for tools/check-qpdf-trapped.py's parser fixtures.
#
# The checker verifies its four declaration parsers against fixtures on every run. That gate
# is only observable when a parser is wrong, so deleting it leaves every normal run green --
# the same survivor a mutation sweep found for every other checker here.
#
# So this breaks a parser in a COPY of the tool and asserts the copy refuses, NAMING the
# reason. Two details are load-bearing and were both learned the hard way (see CLAUDE.md):
#
#   * the copy sits beside the original, because the tool resolves REPO from its own
#     location -- a copy in /tmp exits non-zero with "not found", and an exit-code-only
#     assertion reports that as a pass;
#   * every fixture asserts the mutation applied, because a sed that matched nothing is
#     indistinguishable from a gate that works.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
tool="$here/check-qpdf-trapped.py"

pass=0
fail=0

breaks_it() {
  local name="$1" find="$2" replace="$3" expect="$4"
  local copy="$here/.qpdf-trapped-fixture.py"
  sed "s|$find|$replace|" "$tool" >"$copy"
  if cmp -s "$copy" "$tool"; then
    echo "  FAIL $name: the mutation did not apply, so this case would prove nothing"
    fail=$((fail + 1))
    rm -f "$copy"
    return
  fi
  local got
  if got="$(python3 "$copy" 2>&1)"; then
    echo "  FAIL $name: the tool ran anyway"
    fail=$((fail + 1))
  elif grep -qF "$expect" <<<"$got"; then
    echo "  ok   $name"
    pass=$((pass + 1))
  else
    echo "  FAIL $name: it failed, but not for the stated reason"
    sed 's/^/        /' <<<"$got" | head -4
    fail=$((fail + 1))
  fi
  rm -f "$copy"
  rm -rf "$here/__pycache__"
}

echo "check-qpdf-trapped.py: the parser-fixture gate must exist"

# THE CASE THIS FILE EXISTS FOR. `engines/build-wasm.sh` parses the same ffi.rs and got these
# fixtures in PR #40; this tool did not, and its per-source floor of 10 would pass with 15 of
# 16 functions parsed. A tightened regex NARROWS the set required to be trapped, silently.
breaks_it "a tightened ffi.rs parser is caught" \
  'pub\\(super\\) fn (qpdf\[a-z_0-9\]\*)\\s\*\\(' \
  'pub\\(super\\) fn (qpdf[a-z_0-9]*)\\s+\\(' \
  "does not find an ordinary declaration"

breaks_it "a loosened ffi.rs parser is caught" \
  'r"\^\\s\*pub\\(super\\) fn (qpdf\[a-z_0-9\]\*)\\s\*\\("' \
  'r"(qpdf[a-z_0-9]*)\\s*\\("' \
  "matches a commented-out declaration"

breaks_it "a silent JS bridge parser is caught" \
  'r"\\\._(qpdf\[a-z_0-9\]\*)\\s\*\\("' \
  'r"\\._ZZZ(qpdf[a-z_0-9]*)\\s*\\("' \
  "does not find an ordinary bridge call"

# The direction that would bless a call able to abort the process.
breaks_it "a trapped-set parser that blesses everything is caught" \
  'if re.search(r"\\btrap_errors\\s\*\\(", body):' \
  'if True:' \
  "reports an UNTRAPPED function as trapped"

# AND THE GATE ITSELF. Deleting it alone changes nothing observable -- the parsers still
# work, so the tool still passes. The case has to break a parser AND delete the gate, which
# is what a real regression would look like: somebody "fixes" a failing fixture by removing
# the check instead of the cause.
gate_removal_is_caught() {
  local copy="$here/.qpdf-trapped-gate-fixture.py"
  sed -e 's|r"\^\\s\*pub\\(super\\) fn (qpdf\[a-z_0-9\]\*)\\s\*\\("|r"ZZZNOMATCH"|' \
      -e 's|    if fixture_problems:|    if False and fixture_problems:|' \
      "$tool" >"$copy"
  local applied=0
  grep -q 'ZZZNOMATCH' "$copy" && grep -q 'if False and fixture_problems' "$copy" && applied=1
  if [ "$applied" != 1 ]; then
    echo "  FAIL deleting the fixture gate is caught: one of the two mutations did not apply"
    fail=$((fail + 1))
    rm -f "$copy"
    return
  fi
  local got
  if got="$(python3 "$copy" 2>&1)"; then
    echo "  FAIL deleting the fixture gate is caught: a broken parser ran anyway"
    fail=$((fail + 1))
  else
    # It must fail -- but NOT via the fixture gate, which is deleted. The point of the case
    # is that something else has to notice, and it does: the per-source floor.
    if grep -qE "parsed only|Traceback" <<<"$got"; then
      echo "  ok   with the gate gone, the per-source floor still catches a dead parser"
      pass=$((pass + 1))
    else
      echo "  FAIL deleting the fixture gate is caught: it failed for an unrecognised reason"
      sed 's/^/        /' <<<"$got" | head -4
      fail=$((fail + 1))
    fi
  fi
  rm -f "$copy"
  rm -rf "$here/__pycache__"
}
gate_removal_is_caught

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED — $fail case(s) failed, $pass passed" >&2
  exit 1
fi
echo "OK — $pass adversarial case(s) all behaved as required"
