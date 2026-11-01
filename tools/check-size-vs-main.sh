#!/usr/bin/env bash
# Every base line of this build against main's latest CI artifact, at 0% (DECISIONS rule 18, owner
# (85)(3), (92); PR-B).
#
#   tools/check-size-vs-main.sh --build <size-lines.json> --main <dir>
#
# `--build` is what tools/check-size-lines.sh --out wrote for THIS build; `--main` is what
# tools/fetch-main-size-lines.sh --out wrote for main's latest artifact. A line that moved against
# main must be re-recorded in this tree's apps/web/size-budget.json, and the run says how many lines
# it examined of how many it expected, and which run the artifact came from. See
# tools/size-compare.mjs for the rule, what "re-recorded" means, and the witness.
#
# It is a gate in a script so ci.yml and tools/ci-local.py can both name it; the self-test is
# tools/test-check-size-vs-main.sh. The fetch is CI-only (there is no main artifact on a developer
# machine); this half is what runs anywhere, on any two files.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [ "$#" -ne 4 ] || [ "$1" != "--build" ] || [ "$3" != "--main" ]; then
  echo "usage: tools/check-size-vs-main.sh --build <size-lines.json> --main <dir>" >&2
  exit 2
fi
exec node "$here/size-compare.mjs" compare "$2" "$4"
