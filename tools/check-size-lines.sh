#!/usr/bin/env bash
# The size-lines artifact is complete: it holds every base line `apps/web/size-budget.json` names,
# by name, and nothing else (owner (92), 2026-10-30; DECISIONS rule 18's x86-records change, PR-A).
#
#   tools/check-size-lines.sh --out <file>    measure apps/web/dist, write <file>, then check it
#   tools/check-size-lines.sh --file <file>   check a file that already exists (the self-test's door)
#
# Why it is a check and not a step that writes a file: a size-lines file that lacks a line reads
# exactly like one that has it until PR-B's compare looks for the line a month later. "8 of 9" is
# printed with its expectation on every run, and the run refuses on any difference.
#
# It reads the PRODUCTION `apps/web/dist` (`pnpm build`), never the harness build: the figures are
# the build that ships. See tools/size-lines.mjs for the format.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"

usage() {
  echo "usage: tools/check-size-lines.sh --out <file> | --file <file>" >&2
  exit 2
}

[ "$#" -eq 2 ] || usage
case "$1" in
  --out)
    out="$2"
    dist="${BURROW_SIZE_LINES_DIST:-$repo/apps/web/dist}"
    if [ ! -d "$dist" ]; then
      echo "check-size-lines: REFUSED -- no build at $dist; run \`pnpm build\` in apps/web." >&2
      exit 1
    fi
    node "$here/size-lines.mjs" write "$dist" "$out"
    node "$here/size-lines.mjs" check "$out"
    ;;
  --file)
    node "$here/size-lines.mjs" check "$2"
    ;;
  *) usage ;;
esac
