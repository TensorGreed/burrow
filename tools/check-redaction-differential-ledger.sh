#!/usr/bin/env bash
# The browser differential's ledger, reconciled from outside the spec (#137). The reasoning is in
# check-redaction-differential-ledger.mjs; this is its name as a gate, which is what lets CI and
# tools/ci-local.py track it.
#
# Usage: tools/check-redaction-differential-ledger.sh [--results <dir>] [--spec <basename>]
#          [--projects a,b] [--helper <path>] [--config <path>]

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# TYPE STRIPPING, so the check imports the spec's own `reconcile` rather than a copy of it: a
# second implementation of the judgement is a second place for it to be wrong.
exec node --experimental-strip-types --no-warnings "$here/check-redaction-differential-ledger.mjs" "$@"
