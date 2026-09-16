#!/usr/bin/env bash
# Adversarial self-test for tools/check-integration-suites.sh.
#
# That gate exists because a suite can stop running silently. So each of its three rules is
# broken here in turn and required to refuse, NAMING the reason -- an exit-code-only assertion
# would pass on a checker that failed for an unrelated reason, which this repository has been
# caught by before.
#
# The copies live BESIDE the original, in tools/, not in a temp directory: a copy elsewhere
# resolves `$repo` to the wrong place and exits non-zero for the wrong reason, which an
# exit-code assertion reports as a pass.
#
# Usage: tools/test-check-integration-suites.sh

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
checker="$here/check-integration-suites.sh"
copy="$here/.integration-suites-fixture.sh"
planted=""

cleanup() {
  trap - EXIT INT TERM HUP
  rm -f "$copy"
  [ -n "$planted" ] && rm -f "$planted"
  return 0
}
# EXIT IS NOT ENOUGH. Measured on bash 5: an EXIT trap runs on INT and TERM but NOT on HUP --
# exit 129, body never executed. A run in a terminal that goes away would leave a planted
# `.rs` under `core/burrow-ops/tests/`, which is a file `git add -A` would pick up. Security
# review; `.gitignore` covers the same case from the other side.
trap cleanup EXIT INT TERM HUP

pass=0
fail=0

# `name <expected text> <command...>`
expect_refusal() {
  local name="$1" expect="$2"
  shift 2
  local out status=0
  out="$("$@" 2>&1)" || status=$?
  if [ "$status" -eq 0 ]; then
    echo "  FAIL $name: it did not refuse"
    fail=$((fail + 1))
    return
  fi
  if ! grep -qF "$expect" <<<"$out"; then
    echo "  FAIL $name: it refused, but not for the stated reason"
    echo "        wanted: $expect"
    sed 's/^/        /' <<<"$out" | tail -4
    fail=$((fail + 1))
    return
  fi
  echo "  ok   $name"
  pass=$((pass + 1))
}

# THE BASELINE. Without it every case below could be passing against a checker that refuses
# everything, which is the same non-check as one that refuses nothing.
if "$checker" --list-only >/dev/null 2>&1; then
  echo "  ok   the real suite list is consistent with the tree"
  pass=$((pass + 1))
else
  echo "  FAIL the real suite list is already inconsistent, so no case below means anything"
  "$checker" --list-only 2>&1 | tail -6 | sed 's/^/        /'
  exit 1
fi

# --- Rule 1: a suite on disk that nobody named -----------------------------------------------
#
# The miss that took PR #76 red: `optimistic_counts.rs` arrived and was named nowhere.
planted="$repo/core/burrow-ops/tests/zz_planted_by_self_test.rs"
cat >"$planted" <<'RSEOF'
//! Planted by tools/test-check-integration-suites.sh; removed on the way out.
#[test]
fn planted_by_test_check_integration_suites() {}
RSEOF
[ -f "$planted" ] || { echo "  FAIL the plant did not apply"; exit 1; }
expect_refusal "a suite on disk that nothing names is refused, by name" \
  "zz_planted_by_self_test" \
  "$checker" --list-only
rm -f "$planted"
planted=""

# --- Rule 2: a named suite with no file on disk ----------------------------------------------
#
# The direction the inline version did NOT check. A renamed file left a name pointing at
# nothing, and it surfaced as cargo's "no test target named X" rather than as this gate.
# ANCHORED ON THE ASSIGNMENT, NOT ON WHICHEVER SUITE HAPPENS TO BE FIRST. It was
# `^ops_suites="conformance `, and adding `compress` to the front of an alphabetical list
# stopped it matching -- so the mutation silently did nothing. The `cmp -s` guard below caught
# that and reported it, which is the whole reason it is there: a mutation that does not apply
# is indistinguishable from a defence that holds. Fixing the anchor rather than the list,
# because the list will keep growing and the next suite would break it again.
sed 's/^ops_suites="/ops_suites="zz_phantom_suite /' \
  "$checker" >"$copy"
chmod +x "$copy"
if cmp -s "$checker" "$copy"; then
  echo "  FAIL the phantom mutation did not apply, so this case measured nothing"
  fail=$((fail + 1))
else
  # MATCHED ON THE OWNING RULE'S OWN WORDS, not on the suite name. The name appeared in two
  # possible messages, so the case reported "ok" while exercising a different rule from the one
  # it claimed -- and the rule it claimed was unreachable. Code review found it; the unreachable
  # rule is gone and this case now names the check that actually fires.
  expect_refusal "a named suite with no file on disk is refused by the per-suite check" \
    "names the suite 'zz_phantom_suite', which is not on disk" \
    "$copy" --list-only
fi
rm -f "$copy"

# --- Rule 3: a suite that compiles to zero tests ---------------------------------------------
#
# This is `native-engines` not taking effect, and it is the quietest of the three: the suite
# exists, it is named, and it runs nothing. Planted as an EMPTY test file, with a copy of the
# checker narrowed to just that suite so one small binary is compiled rather than sixteen.
planted="$repo/core/burrow-ops/tests/zz_empty_by_self_test.rs"
# `//!`, NOT `//`. The crate lints deny `missing_docs`, and `ci-local.py` applies ci.yml's
# `RUSTFLAGS: -D warnings` to every local job -- so a plain comment made this fixture fail to
# COMPILE, and the case then refused for the wrong reason while still exiting non-zero. The
# full sweep caught it; running this suite on its own does not, because only the sweep carries
# CI's lint flags. That is the whole argument for `workflow_env()`, arriving on this file.
cat >"$planted" <<'RSEOF'
//! Deliberately carries no #[test]. tools/test-check-integration-suites.sh plants this.
RSEOF
sed -e 's/^engines_suites=.*/engines_suites=""/' \
    -e 's/^ops_suites=.*/ops_suites="zz_empty_by_self_test"/' \
    "$checker" >"$copy"
chmod +x "$copy"
if cmp -s "$checker" "$copy"; then
  echo "  FAIL the zero-test mutation did not apply, so this case measured nothing"
  fail=$((fail + 1))
else
  expect_refusal "a suite that discovers no tests is refused, by name" \
    "discovered no tests" \
    "$copy"
fi
rm -f "$copy" "$planted"
planted=""

# --- The count comparison itself --------------------------------------------------------------
#
# Both list checks can pass while the totals disagree only if one of them is broken, so this
# breaks the comparison's input directly and requires the count line to catch it.
sed 's/^named_count=.*/named_count=999/' "$checker" >"$copy"
chmod +x "$copy"
if cmp -s "$checker" "$copy"; then
  echo "  FAIL the count mutation did not apply, so this case measured nothing"
  fail=$((fail + 1))
else
  expect_refusal "a disagreeing suite count is refused even when both lists match" \
    "neither direction reported it" \
    "$copy" --list-only
fi
rm -f "$copy"

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED — $fail case(s) failed, $pass passed" >&2
  exit 1
fi
echo "OK — $pass adversarial case(s) all behaved as required"
