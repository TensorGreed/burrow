#!/usr/bin/env bash
# Mutation test: each split path is shown to REACH the shared pruning policy.
#
# `split`'s pruning policy is written once (`core/burrow-engines/src/prune/`) and called from
# two places -- `qpdf/extract.rs` on the native path and `web/extract.rs` on the web one. That
# is deliberate and argued there, and it trades something away: the differential corpus
# compares typed outcomes and page counts, so with one policy it can no longer catch the two
# paths pruning DIFFERENTLY, because there is only one pruning.
#
# What is left is one path never reaching the policy at all -- a `prune_output` call dropped
# from one side while the other keeps it. That IS observable, because the optional-content
# refusal lives inside the policy: a path that skips pruning does not refuse a layered
# document, it succeeds. So this plants exactly that deletion on each side in turn and
# requires the suite that covers that side to turn red.
#
# It asserts the mutation applied before running anything. A `sed` that matched nothing is
# indistinguishable from a defence that holds, and this repository has been caught by that
# twice.
#
# Usage: tools/test-prune-is-reached.sh

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
cd "$repo"

web_file="core/burrow-engines/src/web/extract.rs"
native_file="core/burrow-engines/src/qpdf/extract.rs"

# The originals, restored on EVERY exit including a failed assertion or a Ctrl-C. A mutation
# tool that can leave the tree mutated is a tool that will, once.
backup="$(mktemp -d)"
cp "$web_file" "$backup/web.rs"
cp "$native_file" "$backup/native.rs"
restore() {
  cp "$backup/web.rs" "$web_file"
  cp "$backup/native.rs" "$native_file"
}
trap 'restore; rm -rf "$backup"' EXIT

pass=0
fail=0

# Comment out every line of the `prune_output` call on one side, leaving the rest of the
# extractor intact. A deleted call is the mutation; a deleted FILE would fail to compile and
# prove nothing about the corpus.
plant() {
  local file="$1"
  python3 - "$file" <<'PY'
import re
import sys

path = sys.argv[1]
source = open(path, encoding="utf-8").read()
# The call spans several lines on the native side and one on the web side, so the span is
# found rather than assumed: from `crate::prune::prune_output(` to the `?;` that ends it.
start = source.index("crate::prune::prune_output(")
end = source.index("?;", start) + 2
call = source[start:end]
assert "prune_output" in call, call
mutated = source[:start] + "Ok::<(), burrow_types::Error>(())?;" + source[end:]
assert mutated != source, "the mutation did not apply"
open(path, "w", encoding="utf-8").write(mutated)
print(f"planted: removed the prune_output call from {path}")
PY
}

# Run a suite and require it to FAIL, naming what the failure has to mention. An exit status
# alone would accept a compile error or an unrelated red as proof.
require_red() {
  local name="$1" expect_text="$2"
  shift 2
  local out status
  set +e
  out="$("$@" 2>&1)"
  status=$?
  set -e
  if [ "$status" -eq 0 ]; then
    echo "FAIL  $name: the suite passed with the prune deleted"
    fail=$((fail + 1))
    return
  fi
  if ! grep -qF "$expect_text" <<<"$out"; then
    echo "FAIL  $name: red, but nothing mentioned '$expect_text'"
    echo "$out" | tail -40
    fail=$((fail + 1))
    return
  fi
  echo "PASS  $name: red, naming '$expect_text'"
  pass=$((pass + 1))
}

echo "=== the web path"
plant "$web_file"
require_red \
  "web/extract.rs without its prune" \
  "the_web_split_refuses_a_layered_document_and_releases_its_handles" \
  cargo test -p burrow-engines --lib web::tests
restore

echo
echo "=== the native path"
if [ -d "$repo/engines/vendor" ]; then
  plant "$native_file"
  require_red \
    "qpdf/extract.rs without its prune" \
    "split-refuses-a-layered-document" \
    cargo test -p burrow-ops --features native-engines --test conformance \
      every_fixture_produces_the_outcome_the_corpus_records
  restore
else
  echo "SKIP  the native path: engines/vendor is absent (engines/fetch.sh && engines/build-native.sh)"
fi

echo
echo "------------------------------------------------------------"
echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
