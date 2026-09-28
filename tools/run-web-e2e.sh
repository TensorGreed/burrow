#!/usr/bin/env bash
# The web end-to-end run as CI's `web` job makes it: the Playwright suite, then the checks that read
# what that run wrote, in the same order, with nothing in between.
#
# It exists for tools/ci-local.py. CI runs these as separate steps of one job, so a check reading
# the run's `test-results/` cannot be reached without the run. Locally they were separate jobs, and
# `--changed` could select the checker alone: it then judged whatever an earlier e2e run had left,
# from another commit or another branch, against this commit's golden file (code review, #137).
# One script is one job, so they are selected together or not at all.
#
# Usage: tools/run-web-e2e.sh

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"

# Spelled `cd apps/web && pnpm e2e` from the repository root: that is the command
# tools/ci-local.py's `paths_as` names for this script, and it checks the script still runs it.
(
  cd "$repo"
  cd apps/web && pnpm e2e
)
"$here/check-redaction-differential-ledger.sh"
