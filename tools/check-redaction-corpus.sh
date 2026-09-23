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
# writes the 15 evasion and near-miss documents. Nothing said so: the security review ran the
# first, got 28 files against the sweep's floor of 40, and had to find the second by reading
# the tools directory. Both are named here so the count is reproducible from one command.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
out="$root/tests/redaction/generated"

python3 "$root/tools/make-redaction-fixtures.py" "$out" >/dev/null
python3 "$root/tools/make-evasion-fixtures.py" "$out" >/dev/null

generated=$(find "$out" -maxdepth 1 -name '*.pdf' | wc -l | tr -d ' ')
committed=$(find "$root/tests/redaction/fixtures" -maxdepth 1 -name '*.pdf' | wc -l | tr -d ' ')

# THE EXPECTED COUNT, not a non-zero gate. A generator that wrote four files prints `OK` just
# as loudly as one that wrote thirty-nine, and "4 of 39" reads exactly like success.
expected_generated=39
expected_committed=4
if [ "$generated" -ne "$expected_generated" ] || [ "$committed" -ne "$expected_committed" ]; then
    echo "redaction corpus: $generated generated (expected $expected_generated), \
$committed committed (expected $expected_committed)" >&2
    echo "  a count that has moved is either a fixture added without updating this gate, or a \
generator that stopped writing one. Neither is something to pass over." >&2
    exit 1
fi

python3 "$root/tools/check-redaction-corpus.py"

echo "redaction corpus: $generated generated + $committed committed = $((generated + committed)) documents"
