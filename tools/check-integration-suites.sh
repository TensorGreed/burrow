#!/usr/bin/env bash
# Every integration suite on disk is named here, and every suite named here discovers tests.
#
# WHY THIS IS A SCRIPT AND NOT A `run:` BLOCK, which is the whole reason it moved.
#
# It used to live inline in ci.yml's `test` job. Its body is `cargo test -p "$crate"
# --all-features --test "$suite" -- --list`, and `tools/ci-local.py`'s extractor maps that to
# the token `cargo:test` -- which the local `test` job (`cargo test --workspace --all-features`)
# already covers. So the parity check reported FULL COVERAGE over a gate with no local
# counterpart, because `cargo test --workspace` does not enumerate suite names at all.
#
# Measured: PR #76 added `core/burrow-ops/tests/optimistic_counts.rs`, `tools/ci-local.py`
# passed clean, and CI went red on this gate. That is the SEVENTH miss of the class that runner
# exists for, and the second from this exact cause -- #54's subsetting gate was the sixth, and
# CLAUDE.md already drew the conclusion: **a gate belongs in a script**, because `tools/*.sh` is
# a name both CI and the local runner can invoke. A gate written inline is a gate betting that
# the extractor happens to have a pattern for its shape.
#
# THE LIST IS DERIVED FROM DISK, AND THAT IS A CHANGE. It used to be two hand-written strings
# compared against `core/*/tests/*.rs` in both directions, which made adding a suite a two-step
# edit -- and the second step is the one people miss. It was missed here: `standard14_calibration`
# and `oracle_canonicalisation` were added, CI named neither, and would have run neither. This
# gate caught it, so the comparison worked; but a list that has to be edited alongside the thing
# it mirrors is the rotting-list shape CLAUDE.md already names twice, and the same batch saw it
# rot twice more -- `DISPUTED` against `FONTS` in the standard-14 calibration, and the fixture
# count in `check-redaction-corpus.sh`.
#
# Deriving does not weaken the gate, because the goal was never "the list agrees with disk" --
# it was "CI runs every suite on disk". Derivation satisfies that **by construction**, and disk
# is the right source: cargo compiles every `core/*/tests/*.rs` as a test binary, so the files
# ARE the set of suites, and no second opinion about them can be more correct.
#
# WHAT IT CHECKS:
#
#   1. Every suite discovers at least one test. A suite that compiles to zero tests is the
#      `native-engines` feature not taking effect, which is silent otherwise. **This is the
#      substantive rule**, and it is untouched by deriving the list.
#   2. Each crate has suites at all. A glob that matched nothing would otherwise make this
#      script pass over an empty set, which is the failure deriving newly makes possible and
#      which the old hand-written list could not have -- so it is gated explicitly.
#   3. Every suite is run under the crate it lives in, which the directory says.
#
# Shared helpers live in `core/*/tests/support/` and are excluded by `-maxdepth 1`. A future
# FLAT helper -- `core/burrow-ops/tests/testsupport.rs` -- would be demanded as a named suite
# and then refused for discovering no tests, with the misleading message about native-engines.
# Whoever adds that file will read this paragraph; move it under `support/`.
#
# Adversarial self-test: tools/test-check-integration-suites.sh. A checker with no negative
# test is a checker nobody has shown can fail.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
cd "$repo"

# PER CRATE, because the suites live in two of them since M1 PR B2: `conformance` moved to
# burrow-ops when the corpus grew an OPERATION, and `merge` was born there. Naming a suite
# under the wrong crate is an immediate hard failure -- "no test target named `conformance` in
# `burrow-engines`" -- which is how the inline version found that out.
# DERIVED, not written out. `-maxdepth 1` excludes `core/*/tests/support/`; `sed` rather than
# `find -printf`, which is GNU-only and this repository has an iOS milestone.
# `|| true` ON THE PIPELINE, so a missing directory reaches the floor check below instead of
# killing the script. Without it, `set -euo pipefail` turned a moved `tests/` directory into a
# silent exit 1 with no message at all -- the failure mode this file exists to prevent, produced
# by the check meant to prevent it. Measured while writing the self-test case for it.
suites_in() {
  { find "core/$1/tests" -maxdepth 1 -name '*.rs' 2>/dev/null |
      sed -e 's#.*/##' -e 's/\.rs$//' | LC_ALL=C sort -u | tr '\n' ' '; } || true
}
engines_suites="$(suites_in burrow-engines)"
ops_suites="$(suites_in burrow-ops)"

# AN ARGUMENT, NOT AN ENVIRONMENT VARIABLE. It was `BURROW_SUITES_LIST_ONLY`, and an
# environment variable can be inherited by accident: set it anywhere in CI's environment and
# this gate would skip the "discovers no tests" rule -- the rule the script exists for -- while
# still exiting 0 and printing OK. An argument cannot be inherited. Security review.
list_only=0
for arg in "$@"; do
  case "$arg" in
    --list-only) list_only=1 ;;
    *) echo "::error::unknown argument: $arg" >&2; exit 2 ;;
  esac
done

fail() {
  echo "::error::$*" >&2
  exit 1
}

total_tests=0
total_suites=0

run_suites() {
  crate="$1"
  shift
  for suite in "$@"; do
    if [ ! -f "core/$crate/tests/$suite.rs" ]; then
      fail "$crate names the suite '$suite', which is not on disk at core/$crate/tests/$suite.rs"
    fi
    if [ "$list_only" = "1" ]; then
      total_suites=$((total_suites + 1))
      continue
    fi
    list=$(cargo test -p "$crate" --all-features --test "$suite" -- --list)
    count=$(printf '%s' "$list" | grep -c ': test$' || true)
    [ "$count" -gt 0 ] ||
      fail "$crate's $suite suite discovered no tests. Is native-engines taking effect?"
    echo "  $crate/$suite: $count tests"
    total_tests=$((total_tests + count))
    total_suites=$((total_suites + 1))
  done
}

run_suites burrow-engines $engines_suites
run_suites burrow-ops $ops_suites

# THE SET IS NOW DERIVED, SO THE OLD COMPARISON IS GONE. It compared a hand-written list
# against disk in one direction; with the list coming from disk that comparison can only ever
# agree with itself, and a tautology that prints `OK` is the thing this repository treats as
# worse than no check.
#
# WHAT REPLACES IT IS THE FAILURE DERIVING MAKES POSSIBLE. A hand-written list could not
# silently become empty; a glob can. `find` on a mistyped or moved directory prints nothing and
# exits 0, so every loop below would run zero times and this script would report success over
# an empty sweep -- "0 of 26 reads exactly like success". So each crate must yield suites, and
# the floor is stated per crate rather than over the total, because one crate going empty while
# the other still has twenty would pass any total-based gate.
# `|| true` HERE TOO, and for a subtler reason than the `find`. Under `pipefail` the pipeline's
# status is `grep`'s, and `grep -v` on empty input exits 1 -- so with both crates empty this
# assignment killed the script under `set -e` before the floor check below could name the
# problem. Traced with `bash -x` after the self-test case for it reported a refusal "for the
# wrong reason": the refusal was a silent exit, which is what the case was right to reject.
named=$(printf '%s %s' "$engines_suites" "$ops_suites" | tr ' ' '\n' | grep -v '^$' | LC_ALL=C sort -u || true)
named_count=$(printf '%s\n' "$named" | grep -vc '^$' || true)

engines_count=$(printf '%s' "$engines_suites" | tr ' ' '\n' | grep -vc '^$' || true)
ops_count=$(printf '%s' "$ops_suites" | tr ' ' '\n' | grep -vc '^$' || true)

echo "derived: $(printf '%s' "$named" | tr '\n' ' ')"

[ "$engines_count" -ge 5 ] ||
  fail "found $engines_count suite(s) under core/burrow-engines/tests, which is not that \
directory -- a glob matching nothing would make this whole sweep vacuous"
[ "$ops_count" -ge 5 ] ||
  fail "found $ops_count suite(s) under core/burrow-ops/tests, which is not that directory -- \
a glob matching nothing would make this whole sweep vacuous"

if [ "$list_only" = "1" ]; then
  echo "OK -- $named_count suite(s) derived from disk ($engines_count engines, $ops_count ops; list-only: tests not discovered)"
else
  echo "OK -- $named_count suite(s) derived from disk ($engines_count engines, $ops_count ops), $total_tests tests discovered across $total_suites"
fi
