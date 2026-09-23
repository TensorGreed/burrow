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
# WHAT IT CHECKS, in both directions, because one direction is not a check:
#
#   1. Every suite named below discovers at least one test. A suite that compiles to zero
#      tests is the `native-engines` feature not taking effect, which is silent otherwise.
#   2. Every `core/*/tests/*.rs` on disk is named below. A new suite nobody added would be
#      covered by nothing -- the shape of a fuzz target declared and never run.
#   3. Every suite named below exists on disk, checked per suite in `run_suites` before any
#      cargo call. The inline version omitted this: a renamed file left a name pointing at
#      nothing, and it surfaced as `cargo`'s own "no test target named X" rather than as this
#      gate's message -- a worse error at a later moment.
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
engines_suites="parallelism limits properties structure secret_leak prescan glyph_geometry redaction_corpus redaction_defences redaction_disclosure"
ops_suites="compress compress_keeps_everything conformance merge optimistic_counts render reorder reorder_keeps_everything rotate rotate_keeps_everything split split_no_leak subset_closure"

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

# AND THE LIST ITSELF IS CHECKED, in both directions. It is hand-maintained, and this gate
# exists precisely to stop a suite going unrun.
# `LC_ALL=C` on both: `sort` collates by locale and `comm` compares bytes, so a name with a
# capital or a dash could make `comm` report spurious both-only lines. And `sed` rather than
# `find -printf`, which is GNU-only -- this repository has an iOS milestone, so a checker that
# dies on a macOS `find` is a checker somebody will have to fix later.
named=$(printf '%s %s' "$engines_suites" "$ops_suites" | tr ' ' '\n' | grep -v '^$' | LC_ALL=C sort -u)
on_disk=$(find core/*/tests -maxdepth 1 -name '*.rs' | sed -e 's#.*/##' -e 's/\.rs$//' | LC_ALL=C sort -u)

named_count=$(printf '%s\n' "$named" | grep -vc '^$' || true)
disk_count=$(printf '%s\n' "$on_disk" | grep -vc '^$' || true)

echo "named:   $(printf '%s' "$named" | tr '\n' ' ')"
echo "on disk: $(printf '%s' "$on_disk" | tr '\n' ' ')"

missing=$(comm -13 <(printf '%s\n' "$named") <(printf '%s\n' "$on_disk"))
[ -z "$missing" ] ||
  fail "integration suites on disk but not named in this step: $(printf '%s' "$missing" | tr '\n' ' ')"

# NO `comm -23` HERE, DELIBERATELY. The other direction -- a name with no file -- is owned by
# the per-suite existence check in `run_suites`, which fires earlier and with a better message
# naming the crate. A second rule for the same condition was UNREACHABLE, and its self-test
# case passed by matching a string that appears in both messages, so it reported "ok" for a
# rule it never exercised. Code review found it. One condition, one owner.

# THE COUNT IS A TRIPWIRE, and is labelled as one rather than dressed up as a rule: over two
# `sort -u` sets whose one-way difference is already empty, the counts cannot disagree. It is
# here so that a future edit which breaks the comparison above cannot also silently agree with
# itself, and the self-test reaches it by mutating `named_count` directly.
[ "$named_count" -eq "$disk_count" ] ||
  fail "named $named_count suite(s) against $disk_count on disk, and neither direction reported it"

if [ "$list_only" = "1" ]; then
  echo "OK -- $named_count suite(s) named, all present on disk (list-only: tests not discovered)"
else
  echo "OK -- $named_count suite(s) named and on disk, $total_tests tests discovered across $total_suites"
fi
