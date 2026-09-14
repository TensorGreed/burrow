#!/usr/bin/env bash
# Adversarial self-test for tools/seed-fuzz-corpus.py.
#
# The seeder is not only a seeder: it decides what every fuzz target gets to see, and three of
# its rules are the difference between a corpus of real documents and a corpus of truncated
# ones. A truncated seed fails nothing -- it produces a green run over documents the parser
# rejects at the first byte, which is the exact state this whole change exists to end.
#
# So each rule is broken in a COPY and required to refuse, naming its reason. Exit status
# alone would let the copy fail for an unrelated reason and still look correct; this repository
# has been caught by that before, which is why the copy sits beside the original rather than in
# a temp directory -- the script resolves the repository root from its own location.
#
# Usage: tools/test-seed-fuzz-corpus.sh

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
original="$here/seed-fuzz-corpus.py"
copy="$here/.seed-fuzz-probe-fixture.py"
trap 'rm -f "$copy"' EXIT

pass=0
fail=0

# `name <old> <new> <expected text>` -- mutate the copy, run it, require a refusal that says why.
mutate_and_check() {
  local name="$1" old="$2" new="$3" expect="$4"

  python3 - "$original" "$copy" "$old" "$new" <<'PYEOF'
import pathlib, sys
src, dst, old, new = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
text = pathlib.Path(src).read_text()
if old not in text:
    sys.exit(f"the mutation target is not in the seeder: {old!r}")
pathlib.Path(dst).write_text(text.replace(old, new, 1))
PYEOF

  # THE MUTATION MUST HAVE APPLIED. A replace that matched nothing leaves an identical file,
  # the case passes, and the green reads as "the gate works" while nothing was mutated at all.
  if cmp -s "$copy" "$original"; then
    echo "  FAIL $name: the mutation did not apply, so this case would prove nothing"
    fail=$((fail + 1))
    return
  fi

  local out status
  set +e
  out="$(python3 "$copy" --check 2>&1)"
  status=$?
  set -e

  if [ "$status" -eq 0 ]; then
    echo "  FAIL $name: the broken seeder ran to completion and reported OK"
    fail=$((fail + 1))
  elif grep -qF "$expect" <<<"$out"; then
    echo "  ok   $name"
    pass=$((pass + 1))
  else
    echo "  FAIL $name: it refused, but not for the stated reason ($expect)"
    sed 's/^/        /' <<<"$out" | tail -4
    fail=$((fail + 1))
  fi
}

# The unmutated seeder must succeed, or every case below is measuring a broken baseline.
if out="$(python3 "$original" --check 2>&1)"; then
  echo "  ok   the seeder itself succeeds on --check"
  pass=$((pass + 1))
else
  echo "  FAIL the seeder does not succeed on --check, so no case below means anything"
  sed 's/^/        /' <<<"$out" | tail -5
  exit 1
fi

# --- Case 1: a seed whose document does not survive its prefix ---------------------------
#
# One byte short is a PDF without its `%`. It parses as nothing, teaches libFuzzer nothing,
# and looks exactly like a seeded corpus from the outside.
mutate_and_check "a truncated seed is refused" \
  'seed = bytes([5] * prefix) + body' \
  'seed = bytes([5] * prefix) + body[1:]' \
  "does not survive"

# --- Case 2: a merge seed that splits somewhere other than the boundary -------------------
#
# The target splits the buffer in two and merges the halves. If the split does not land on the
# document boundary, it is merging two fragments -- which only ever exercises the refusal path,
# and is how the foreign-object copier went unfuzzed.
mutate_and_check "a merge seed that splits mid-document is refused" \
  'if offset % total == len(first):' \
  'if offset % total >= 0:' \
  "two half-documents"

# --- Case 3: a prefix table that has drifted from the targets -----------------------------
#
# Each target carves its parameters off the front of the buffer. If a target changes how many
# bytes it takes and the table does not follow, every seed for it is silently truncated and
# the run still reports clean.
mutate_and_check "a stale prefix table is refused" \
  '"rotate": 2,' \
  '"rotate": 1,' \
  "either the prefix changed or this table is stale"

# --- Case 4: nothing to seed from is a failure, not a pass --------------------------------
mutate_and_check "having no fixtures to seed from is refused" \
  'found.extend(sorted(p for p in directory.iterdir() if p.is_file() and p.suffix != ".md"))' \
  'found.extend([])' \
  "no fixtures found"

# --- Case 5: a merge corpus with no real pairing is a failure -----------------------------
#
# Without this, a change that made every pairing fail would leave the merge corpus empty and
# the seeder cheerfully reporting success over six other targets.
mutate_and_check "a merge corpus with no document pairs is refused" \
  'for padding in range(0, 4096):' \
  'for padding in range(0, 0):' \
  "never see a two-document merge"

# --- Case 6: a carving that produces empty spans -----------------------------------------
#
# `pdfsyntax_names` and `pdfsyntax_dict_keys` are seeded by CARVING spans out of the fixtures
# rather than copying them, because neither target's input is a document. A carving bug that
# produced empty spans would fill a corpus directory with zero-byte files, and the run would
# report a healthy span count over a corpus that is nothing at all.
mutate_and_check "a carving that produces empty spans is refused" \
  'spans.append(body[start:end])' \
  'spans.append(b"")' \
  "an empty span is not a seed"

# --- Case 7: a carving that quietly stops contributing ------------------------------------
#
# The gate is the derivable count, not non-zero: every fixture with a closing delimiter must
# contribute at least one span. Without it, a carver that worked on two fixtures out of
# eighteen would report 8 spans and look like a seeded corpus. This is the shape the root
# CLAUDE.md calls "4 of 15 reads exactly like success" -- here the numerator is checked
# against a denominator the fixtures themselves supply.
mutate_and_check "a carving that skips most fixtures is refused" \
  'start = body.find(b"<<", at)' \
  'start = body.find(b"<<ONLYINNOFIXTURE", at)' \
  "carved nothing from"

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED — $fail case(s) failed, $pass passed" >&2
  exit 1
fi
echo "OK — $pass adversarial case(s) all behaved as required"
