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

# --- Rule 1: a suite on disk is PICKED UP, not refused ---------------------------------------
#
# This case used to plant a suite and require a refusal, because the list was hand-written and
# the gate's job was to notice the list had not been updated. The list is derived from disk now,
# so the correct behaviour for a new suite is the opposite: it is swept, silently and
# immediately. That is the point of deriving, and it is what this case must therefore assert.
#
# The miss it was written for -- PR #76's `optimistic_counts.rs`, named nowhere -- is no longer
# expressible. It is not that the rule got weaker; the condition it detected cannot arise.
planted="$repo/core/burrow-ops/tests/zz_planted_by_self_test.rs"
cat >"$planted" <<'RSEOF'
//! Planted by tools/test-check-integration-suites.sh; removed on the way out.
#[test]
fn planted_by_test_check_integration_suites() {}
RSEOF
[ -f "$planted" ] || { echo "  FAIL the plant did not apply"; exit 1; }
# CAPTURED, THEN MATCHED. `"$checker" … | grep -q` looks right and is not: `grep -q` closes
# the pipe the moment it matches, the checker takes SIGPIPE, and under `set -o pipefail` the
# pipeline's status is that failure -- so a SUCCESSFUL match read as "not picked up". Traced
# with `bash -x` after this case failed while running the same command by hand passed. It is
# CLAUDE.md's "never put a command whose status you need on the left of a pipe", from the
# other end: here the pipe's right-hand side is what breaks the left.
swept=$("$checker" --list-only 2>&1 || true)
case "$swept" in
  *zz_planted_by_self_test*)
    echo "  ok   a suite newly on disk is swept without being named anywhere"
    pass=$((pass + 1))
    ;;
  *)
    echo "  FAIL a suite newly on disk is swept without being named anywhere: not picked up"
    fail=$((fail + 1))
    ;;
esac
rm -f "$planted"
planted=""

# --- Rule 1b: a crate whose suites cannot be found is refused --------------------------------
#
# The failure deriving newly makes possible, and the reason Rule 1 could be relaxed rather than
# dropped. A hand-written list could not silently become empty; a glob can -- `find` on a moved
# directory prints nothing and exits 0, and every loop would then run zero times under an `OK`.
sed 's#find "core/$1/tests"#find "core/$1/tests-moved-away"#' "$checker" >"$copy"
chmod +x "$copy"
if cmp -s "$checker" "$copy"; then
  echo "  FAIL the empty-glob mutation did not apply, so this case measured nothing"
  fail=$((fail + 1))
else
  expect_refusal "a crate whose suite directory yields nothing is refused, not swept as empty" \
    "which is not that directory" \
    "$copy" --list-only
fi
rm -f "$copy"

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
sed 's/^ops_suites="\$(suites_in burrow-ops)"/ops_suites="zz_phantom_suite $(suites_in burrow-ops)"/' \
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
# The per-crate floors are the gate now, so this breaks one of them directly. Without a case
# here the floors would be two numbers nobody had ever seen fire.
sed 's/^engines_count=.*/engines_count=0/' "$checker" >"$copy"
chmod +x "$copy"
if cmp -s "$checker" "$copy"; then
  echo "  FAIL the count mutation did not apply, so this case measured nothing"
  fail=$((fail + 1))
else
  expect_refusal "a crate reporting zero suites is refused rather than swept as empty" \
    "which is not that directory" \
    "$copy" --list-only
fi
rm -f "$copy"

# --- The OPS floor, which nothing reached -----------------------------------------------------
#
# A code review deleted the `ops_count` floor outright and all six cases still printed `ok`.
# Neither case that touches a floor reaches this one: the moved-directory case empties both
# crates and the engines floor fires first, and the `engines_count=0` case names engines. Half
# the replacement for the deleted comparison was a defence nothing failed for.
sed 's/^ops_count=.*/ops_count=0/' "$checker" >"$copy"
chmod +x "$copy"
if cmp -s "$checker" "$copy"; then
  echo "  FAIL the ops-floor mutation did not apply, so this case measured nothing"
  fail=$((fail + 1))
else
  expect_refusal "a zero count for burrow-ops is refused, not only for burrow-engines" \
    "core/burrow-ops/tests" \
    "$copy" --list-only
fi
rm -f "$copy"

# --- A PARTIAL derivation, which the floors cannot see ----------------------------------------
#
# The failure the git comparison was added for. Truncating `suites_in` swept 10 of 26 suites and
# printed `OK -- 10 suite(s) derived from disk`: both floors passed, because both crates still
# had five. "10 of 26" reads exactly like success.
#
# `tail` rather than `head` DELIBERATELY. With `head -5` the planted-suite case happens to catch
# it, because `zz_planted_by_self_test` sorts last and is dropped -- by alphabetical accident,
# not by design. `tail -5` drops the front instead and leaves that case green, so this is the
# shape that measures the git comparison rather than the accident.
sed 's/| tr .\\n. . .; } || true/| tail -5 | tr "\\n" " "; } || true/' "$checker" >"$copy"
if cmp -s "$checker" "$copy"; then
  # The sed above is fragile against reformatting; fall back to a direct pipeline edit.
  sed 's#LC_ALL=C sort -u | tr#LC_ALL=C sort -u | tail -5 | tr#' "$checker" >"$copy"
fi
chmod +x "$copy"
if cmp -s "$checker" "$copy"; then
  echo "  FAIL the truncation mutation did not apply, so this case measured nothing"
  fail=$((fail + 1))
else
  expect_refusal "a partial derivation is refused against git's index, not swept as complete" \
    "the derivation is partial" \
    "$copy" --list-only
fi
rm -f "$copy"

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED — $fail case(s) failed, $pass passed" >&2
  exit 1
fi
echo "OK — $pass adversarial case(s) all behaved as required"
