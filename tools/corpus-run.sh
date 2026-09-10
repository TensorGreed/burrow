#!/usr/bin/env bash
# corpus-run — see tools/README.md
#
# Not implemented yet. Lands in M1, when there is an operation to exercise.
# Deliberately fails loudly rather than silently succeeding: a regression harness that
# reports success without running anything is worse than no harness.
set -euo pipefail
echo "corpus-run: not implemented yet (M1). See tools/README.md and docs/ROADMAP.md." >&2
exit 64
