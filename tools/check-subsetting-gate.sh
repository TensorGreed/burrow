#!/usr/bin/env bash
#
# ADR 0019 §2's rule is met, and the tests that say so actually ran.
#
# WHY THIS IS A SCRIPT AND NOT A `run:` BLOCK
#
# It was a `run:` block, and `tools/ci-local.py` reported full parity while not replicating it.
# The parity extractor maps a `cargo test …` line to a token the main test job already covers, so
# a gate written as a bare `cargo test` invocation is invisible to it however specific the gate
# is. This branch's first push went red on exactly that step after a green local run -- the fifth
# time a local sweep has missed a CI gate, and the first where the local runner had *said* it
# covered everything.
#
# A check that lives in a script is one both can invoke by name, which is what gives the parity
# table something to track. Same shape as every other `tools/check-*.sh`.
#
# WHAT IT GUARDS
#
# Until #54 landed, the two subsetting tests were `#[ignore]`d BECAUSE THEY FAILED, and CI
# required them to fail -- without which "the gap is visible in the suite" was a claim about a doc
# comment, since `cargo test` never runs an ignored test.
#
# #54 closed the gap, so the gate inverts rather than disappearing. The hazard it covered has not
# gone away; it has changed shape. Put `#[ignore]` back on either test, or delete one, and
# `cargo test --workspace` stays green while nothing checks the property this whole change exists
# for. So the gate is now "it ran, and it passed" -- and the second half is what makes the first
# real: `--exact` on a name that is ignored, misspelled or gone exits **0** and reports
# `0 passed`, so the exit code alone measures nothing.
set -uo pipefail

cd "$(dirname "$0")/.."

# The four tests, and each one's job. Two assert the property; two are the controls that fail if
# the scan goes blind, which is the state that otherwise looks identical to success.
gates="
split_no_leak::no_output_carries_anything_from_a_page_it_excluded_even_when_objects_are_shared
subset_closure::every_surviving_object_belongs_to_a_page_the_output_contains
split_no_leak::the_scan_can_still_see_every_channel_it_is_the_gate_for
subset_closure::the_closure_scan_can_still_find_a_trespasser_that_is_really_there
"

expected=4
ran=0
log=$(mktemp)
trap 'rm -f "$log"' EXIT

echo "checking that ADR 0019 §2's rule is met, and measured rather than asserted"
for suite_test in $gates; do
  suite=${suite_test%%::*}
  name=${suite_test##*::}
  printf '  %-72s ' "$suite::$name"
  if ! cargo test -p burrow-ops --all-features --test "$suite" -- --exact "$name" \
       >"$log" 2>&1; then
    echo "FAILED"
    echo "" >&2
    echo "check-subsetting-gate: $suite::$name failed. ADR 0019 §2's rule -- an output carries" >&2
    echo "  nothing derived from the pages it excluded -- is the one thing split may not ship" >&2
    echo "  without. See issue #54 and the ADR's 2026-09-14 amendment." >&2
    tail -30 "$log" >&2
    exit 1
  fi
  # IT RAN. This is the line that does the work; the exit code above does not.
  if ! grep -q "1 passed" "$log"; then
    echo "DID NOT RUN"
    echo "" >&2
    echo "check-subsetting-gate: $suite::$name did not run -- it is #[ignore]d, renamed or" >&2
    echo "  deleted. An assertion that does not execute is a comment containing assert!, which" >&2
    echo "  is the exact state this gate existed to prevent while the rule was unmet." >&2
    tail -30 "$log" >&2
    exit 1
  fi
  ran=$((ran + 1))
  echo "ran and passed"
done

# GATED ON THE EXPECTED COUNT, not on non-zero. A loop over an emptied list runs nothing and
# exits 0, which reads as success.
if [ "$ran" -ne "$expected" ]; then
  echo "check-subsetting-gate: ran $ran gate test(s), expected $expected." >&2
  exit 1
fi
echo "OK -- $ran of $expected gate test(s) ran and passed; the rule is measured, not asserted"
