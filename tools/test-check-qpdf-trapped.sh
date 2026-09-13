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
  'pub\\(super\\) fn (qpdf\[A-Za-z_0-9\]\*)\\s\*\\(' \
  'pub\\(super\\) fn (qpdf[A-Za-z_0-9]*)\\s+\\(' \
  "does not find an ordinary declaration"

breaks_it "a loosened ffi.rs parser is caught" \
  'r"\^\\s\*pub\\(super\\) fn (qpdf\[A-Za-z_0-9\]\*)\\s\*\\("' \
  'r"(qpdf[A-Za-z_0-9]*)\\s*\\("' \
  "matches a commented-out declaration"

breaks_it "a silent JS bridge parser is caught" \
  'r"\\\._(qpdf\[A-Za-z_0-9\]\*)\\s\*\\("' \
  'r"\\._ZZZ(qpdf[A-Za-z_0-9]*)\\s*\\("' \
  "does not find an ordinary bridge call"

# The direction that would bless a call able to abort the process.
breaks_it "a trapped-set parser that blesses everything is caught" \
  'if re.search(r"\\btrap_errors\\s\*\\(", body):' \
  'if True:' \
  "reports an UNTRAPPED function as trapped"

# THE WRAPPER PROOF, which is the one place this tool trusts a function other than the one it
# is judging. Following a helper is what makes the 58 `qpdf_oh_*` functions callable at all,
# so a proof that admits the wrong helper blesses 58 calls able to abort the process. Two
# directions, because they fail differently: admitting a helper that never traps, and
# admitting one that traps and then calls its callback again outside the trap.
breaks_it "a wrapper proof that skips the trap requirement is caught" \
  'if not traps:' \
  'if not traps and False:' \
  "proves a helper that never calls trap_errors"

breaks_it "a wrapper proof blind to a callback used outside the trap is caught" \
  'if takes_data:' \
  'if False:' \
  "calls its qpdf_data callback OUTSIDE the trap"

# RULE 2, which had a rule in the tool, a sentence in the ADR, and no probe and no mutation
# until code review counted them. Mutating it changed nothing observable, which is the same
# shape as a pattern that matches nothing.
breaks_it "a wrapper proof blind to an early exit before the trap is caught" \
  'if befores:' \
  'if False and befores:' \
  "early exit that skips the trap"

# AND THE CALLER'"'"'S OWN BODY. A helper'"'"'s proof says nothing about a caller that throws on
# the way to it. This was a real hole: the route walk matched the helper name ANYWHERE in the
# body, so a function doing untrapped work and then forwarding was listed as safe to call.
# Security review measured 20 of 71 indirect entries carrying such code at qpdf 12.4.1.
breaks_it "a route walk that ignores what the caller does before forwarding is caught" \
  'if forwards_purely_to(body, helper):' \
  'if re.search(rf"\\b{re.escape(helper)}\\s*\[<(\]", body):' \
  "after doing untrapped work of its own"

# AND THE HELPER'"'"'S PROOF MUST BE LOAD-BEARING, not decorative. If `trapped_routes` reported
# a function as trapped whichever helper it went through, the proofs above could all pass and
# mean nothing. The mutation probe inside the tool asserts that removing `trap_errors` from
# the helper turns its callers red; this asserts that probe is not itself inert.
breaks_it "a route walk that follows any helper at all is caught" \
  'for helper in wrappers:' \
  'for helper in list(wrappers) + ["helper_that_does_not_trap"]:' \
  "is reported as trapped"

# THE FINGERPRINT GATE, which is what stops a restructured helper being re-proved silently at
# a version bump. Its own probes run on every invocation; this is the case that proves those
# probes are load-bearing rather than decorative.
breaks_it "a fingerprint gate blind to a changed body is caught" \
  'elif recorded\[name\]\[1\] != sha:' \
  'elif False:' \
  "does not notice a helper body changing"

# AND THE GATE ITSELF. Deleting it alone changes nothing observable -- the parsers still
# work, so the tool still passes. The case has to break a parser AND delete the gate, which
# is what a real regression would look like: somebody "fixes" a failing fixture by removing
# the check instead of the cause.
gate_removal_is_caught() {
  local copy="$here/.qpdf-trapped-gate-fixture.py"
  sed -e 's|r"\^\\s\*pub\\(super\\) fn (qpdf\[A-Za-z_0-9\]\*)\\s\*\\("|r"ZZZNOMATCH"|' \
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
    # is that something ELSE has to notice, and two things now do.
    #
    # It used to be the per-source floor. Since M1 PR B the independent token walk gets
    # there first and says more: it names every function the dead pattern stopped seeing,
    # rather than reporting a count below a threshold. Either is a pass here -- the case is
    # about the gate not being the only thing standing, not about which backstop wins.
    if grep -qE "parsed only|token walk|Traceback" <<<"$got"; then
      echo "  ok   with the gate gone, a backstop still catches a dead parser"
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

# THE CASE FOR THE SECOND ROUTE, and it has to defeat the fixtures to prove anything.
#
# The fixtures now know about capitals, so narrowing the character class alone is caught by
# them -- which is good, and says nothing about whether the token walk works. Fixtures can
# only cover a class of name somebody thought of; the walk is what covers the class nobody
# has. So this case narrows the pattern AND deletes the two capital-letter fixtures, leaving
# the walk as the only thing that can notice. If it ever stops noticing, this fails.
only_the_token_walk_is_left() {
  local copy="$here/.qpdf-trapped-crosscheck-fixture.py"
  sed -e 's|r"\^\\s\*pub\\(super\\) fn (qpdf\[A-Za-z_0-9\]\*)\\s\*\\("|r"^\\s*pub\\(super\\) fn (qpdf[a-z_0-9]*)\\s*\\("|' \
      -e '/a name with a capital letter, which this parser used to miss entirely/d' \
      -e '/pub(super) fn qpdf_set_deterministic_ID(", "qpdf_set_deterministic_ID"/d' \
      -e '/a capital letter after an underscore, not at the end/d' \
      -e '/pub(super) fn qpdf_set_static_aes_IV(", "qpdf_set_static_aes_IV"/d' \
      "$tool" >"$copy"
  local applied=0
  grep -q 'qpdf\[a-z_0-9\]' "$copy" \
    && ! grep -q 'qpdf_set_deterministic_ID", "qpdf_set_deterministic_ID' "$copy" \
    && applied=1
  if [ "$applied" != 1 ]; then
    echo "  FAIL only the token walk is left: the mutations did not apply, so this proves nothing"
    fail=$((fail + 1))
    rm -f "$copy"
    return
  fi
  local got
  if got="$(python3 "$copy" 2>&1)"; then
    echo "  FAIL only the token walk is left: a pattern blind to capitals ran anyway"
    fail=$((fail + 1))
  elif grep -qF "token walk" <<<"$got" && grep -qF "qpdf_set_deterministic_ID" <<<"$got"; then
    echo "  ok   with the capital-letter fixtures gone, the token walk still names the missed function"
    pass=$((pass + 1))
  else
    echo "  FAIL only the token walk is left: it failed, but not via the token walk"
    sed 's/^/        /' <<<"$got" | head -4
    fail=$((fail + 1))
  fi
  rm -f "$copy"
  rm -rf "$here/__pycache__"
}
only_the_token_walk_is_left

# AND THE WALK MUST NOT INVENT FUNCTIONS. A cross-check that reports names nobody can find
# is worse than none: every run fails and the failure teaches people to ignore it. The
# first version of `externs_by_token` did exactly this -- it read `fn_like_name(` as a
# declaration because it checked the character before `fn` and not the one after.
breaks_it "a token walk that matches inside a word is caught" \
  'if before.isalnum() or before == "_" or after.isalnum() or after == "_":' \
  'if before.isalnum() or before == "_":' \
  "treats \`fn_like_name(\` as a declaration"

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED — $fail case(s) failed, $pass passed" >&2
  exit 1
fi
echo "OK — $pass adversarial case(s) all behaved as required"
