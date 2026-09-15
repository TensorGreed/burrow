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

# REFUSE IF A PREVIOUS RUN'S MUTATION IS STILL IN THE TREE.
#
# This rewrites tracked source and restores it from a backup under an EXIT trap. The trap
# survives INT and TERM (measured), so the only way a mutation outlives a run is SIGKILL, an
# OOM kill, or a failing `cp` inside the restore. What survives then COMPILES AND RUNS --- that
# is the whole point of the new mutation shape --- and a split built from it carries content
# from every page it excluded. So the residue is checked for by name rather than left for
# someone to notice.
#
# NOT a `git diff --quiet` on these files: they are ordinarily modified on a working branch,
# and refusing then would make the tool unusable exactly when it is wanted. The check is for
# the marker, which nothing but this script writes.
for f in "$web_file" "$native_file"; do
  if grep -q 'let _ = (&graph' "$repo/$f"; then
    echo "REFUSED — $f still carries a planted mutation from an earlier run." >&2
    echo "  A previous run was killed before its restore. That tree COMPILES and prunes" >&2
    echo "  NOTHING, so it must not be built on. Recover with:" >&2
    echo "    git checkout -- $f      # if the file is otherwise unmodified" >&2
    echo "  or restore the prune_output call by hand from the surrounding comment." >&2
    exit 1
  fi
done

pass=0
fail=0

# Replace the `prune_output` call with one that STILL READS ITS ARGUMENTS and prunes nothing.
#
# The first version substituted a bare `Ok(())?`, which left `source.deadline` unread -- and
# CI compiles with `RUSTFLAGS: -D warnings`, so the mutated tree failed to BUILD with "field
# `deadline` is never read" instead of failing the conformance case. The suite went red for
# the wrong reason, which is the one outcome a mutation test must not accept. `require_red`
# caught it, because it matches on the expected text rather than on the exit status alone;
# this is the fix it asked for.
#
# A deleted FILE would also fail to compile and prove nothing about the corpus. What is wanted
# is a tree that builds, runs, and does not prune.
plant() {
  local file="$1"
  python3 - "$file" <<'PY'
import sys

path = sys.argv[1]
source = open(path, encoding="utf-8").read()
# The call spans several lines on the native side and one on the web side, so the span is
# found rather than assumed: from `crate::prune::prune_output(` to the `?;` that ends it.
start = source.index("crate::prune::prune_output(")
end = source.index("?;", start) + 2
call = source[start:end]
assert "prune_output" in call, call
# The argument list, verbatim, bound and dropped. Every name the real call reads is still
# read, so `-D dead-code` has nothing to say, and nothing is pruned.
args = call[call.index("(") + 1 : call.rindex(")")]
assert args.strip(), f"no arguments found in {call!r}"
mutated = source[:start] + "{ let _ = (" + args + "); }" + source[end:]
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

# THE ENGINES ARE NEEDED BY BOTH HALVES NOW. The web half runs with `--features
# native-engines` (see below for why), and `build.rs` FAILS rather than skipping when that
# feature is on and `engines/vendor` is absent. The guard used to sit between the two halves
# and cover only the native one, so on a clean checkout the web half reported
# "red, but nothing mentioned ..." -- the tool that exists to tell "red for the right reason"
# from "red for the wrong reason", giving the wrong reason for its own missing prerequisite.
# Found by security review.
if [ ! -d "$repo/engines/vendor" ]; then
  echo "SKIP  both paths: engines/vendor is absent (engines/fetch.sh && engines/build-native.sh)"
  echo "      Nothing was measured. This is not a pass."
  exit 0
fi

echo "=== the web path"
plant "$web_file"
# `--features native-engines`, and it is load-bearing rather than incidental: without it the
# NATIVE caller of `crate::prune` is compiled out, so removing the web one leaves the whole
# shared module unreachable and the tree fails to build with 31 dead-code errors under CI's
# `-D warnings`. That is red for the wrong reason, which `require_red` refuses. With the
# feature on, the module keeps its other caller and only the web BEHAVIOUR changes --- which
# is the thing being measured.
require_red \
  "web/extract.rs without its prune" \
  "the_web_split_refuses_a_layered_document_and_releases_its_handles" \
  cargo test -p burrow-engines --features native-engines --lib web::tests
restore

echo
echo "=== the native path"
{
  plant "$native_file"
  require_red \
    "qpdf/extract.rs without its prune" \
    "split-refuses-a-layered-document" \
    cargo test -p burrow-ops --features native-engines --test conformance \
      every_fixture_produces_the_outcome_the_corpus_records
  restore
}

echo
echo "------------------------------------------------------------"
echo "$pass passed, $fail failed"
[ "$fail" -eq 0 ]
