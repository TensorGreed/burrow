#!/usr/bin/env bash
#
# Build the redaction corpus and check it, before anything measures against it.
#
# WHY THIS IS A SCRIPT AND NOT TWO LINES IN ci.yml
#
# `tests/redaction/generated/` is gitignored -- it is regenerated rather than stored, which
# `corpus/manifest.toml` says and which is the right call for 39 derived files. What did not
# exist was anything that regenerated it. Neither generator, and not
# `tools/check-redaction-corpus.py` either, appeared in any CI job.
#
# That was invisible for as long as nothing read the directory. `core/burrow-engines/tests/
# redaction_corpus.rs` reads it, so on a clean checkout the sweep panicked on a missing path --
# and it passed on the machine where the files happened to be left over. A code review found
# it by checking out the commit somewhere else, which is the whole argument for the worktree
# rule.
#
# A gate belongs in a script (CLAUDE.md, *Working agreements*): `tools/check-*.sh` is a name
# both CI and `tools/ci-local.py` can invoke, and that is what gives the parity table something
# to track. Two lines inline would be a gate betting the extractor has a pattern for its shape.
#
# IT TAKES TWO GENERATORS, AND ONLY ONE WAS DOCUMENTED
#
# `make-redaction-fixtures.py` writes the 24 survival channels. `make-evasion-fixtures.py`
# writes the evasion and near-miss documents (15 then; the manifest has the count now). Nothing said so: the security review ran the
# first, got 28 files against the sweep's floor of 40, and had to find the second by reading
# the tools directory. Both are named here so the count is reproducible from one command.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="$root/tests/redaction/generated"

# CLEANED FIRST, and that is not tidiness. `tests/redaction/generated/` is gitignored and
# regenerated, so a stale file is invisible to git and indistinguishable from a live one — and
# the count below is taken from the directory. Without this, a fixture REMOVED from a generator
# leaves its last output sitting there, the count still matches, and the gate passes over a
# document nothing writes any more.
#
# Measured: deleting an entry from `make-evasion-fixtures.py`'s `CASES` left this script green.
# It is the mutation the derived count exists to catch, and cleaning is what makes it catchable.
rm -f "$out"/*.pdf

python3 "$root/tools/make-redaction-fixtures.py" "$out" >/dev/null
python3 "$root/tools/make-evasion-fixtures.py" "$out" >/dev/null

generated=$(find "$out" -maxdepth 1 -name '*.pdf' | wc -l | tr -d ' ')
committed=$(find "$root/tests/redaction/fixtures" -maxdepth 1 -name '*.pdf' | wc -l | tr -d ' ')

# THE EXPECTED COUNT, DERIVED FROM THE MANIFEST rather than typed here.
#
# It was two integers in this file. They had to be edited by hand twice in one batch — 39 -> 42
# when the `/ActualText` channel gained fixtures, and again for the nested one — and a count
# somebody bumps without looking is worse than no count: it reads as a measurement while
# asserting whatever the last person typed. That is the rotting-list shape CLAUDE.md names, and
# the same batch saw it three more times (`DISPUTED` against the calibration's font list, and
# the integration-suite list twice).
#
# `tests/redaction/manifest.toml` is the right source because it is not a restatement of the
# file listing: it assigns every fixture the verdict ADR 0029 owes it, so a file that exists and
# is not declared there is a document with no assigned verdict — a defect in its own right, and
# one `check-redaction-corpus.py` now refuses. That refusal is what makes deriving sound; a
# count derived from an incomplete declaration would quietly fall to match it.
# `sort -u` BEFORE COUNTING, because `grep -c` counts LINES. A `file =` line duplicated in the
# manifest would inflate the expectation, and the derived gate would then agree with itself over
# a corpus missing a fixture. The `>= 20` floor below catches a collapsed derivation, not an
# inflated one.
expected_generated=$(grep '^file = "generated/' "$root/tests/redaction/manifest.toml" |
    sort -u | wc -l | tr -d ' ')
expected_committed=$(grep '^file = "fixtures/' "$root/tests/redaction/manifest.toml" |
    sort -u | wc -l | tr -d ' ')

# AND THE DERIVATION ITSELF IS GATED. `grep -c` returning zero is how a moved manifest, a
# renamed key or a changed quoting style would present, and zero expected against zero found
# would agree with itself and print OK over an empty corpus.
if [ "$expected_generated" -lt 20 ] || [ "$expected_committed" -lt 2 ]; then
    echo "redaction corpus: the manifest yielded $expected_generated generated and \
$expected_committed committed declarations, which is not that manifest -- the derivation is \
broken, not the corpus" >&2
    exit 1
fi

if [ "$generated" -ne "$expected_generated" ] || [ "$committed" -ne "$expected_committed" ]; then
    echo "redaction corpus: $generated generated (manifest declares $expected_generated), \
$committed committed (manifest declares $expected_committed)" >&2
    echo "  a generator that stopped writing one, or a fixture written without a verdict in \
tests/redaction/manifest.toml. Neither is something to pass over." >&2
    exit 1
fi

python3 "$root/tools/check-redaction-corpus.py"

echo "redaction corpus: $generated generated + $committed committed = $((generated + committed)) documents"
