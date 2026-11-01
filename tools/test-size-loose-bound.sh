#!/usr/bin/env bash
# Mutation gate for the loose off-architecture size check (DECISIONS rule 18, owner (92)(Q3); review
# r1 of PR-B part 1, nit 3, and CLAUDE.md's Definition of done: a new check's probes are themselves
# tested).
#
# The check is `looseBound` / `driftFindings` in apps/web/src/size-budget-drift.ts, probed by the
# "base lines at 0%, loose off-arch, held bundles at 2%" block of apps/web/src/size-budget.test.ts.
# Nothing else fails if someone weakens those probes, so this does it on purpose: six mutants (L1-L6), each
# a COPY of size-budget-drift.ts with one rule broken, run against a COPY of the test file that
# imports the mutant, both written BESIDE the originals in apps/web/src (so every relative import
# resolves; a copy elsewhere resolves its paths wrongly). Each mutant must
#   1. apply (the text to replace is asserted present and the copy asserted different), and
#   2. be CAUGHT, NAMING the probe that is meant to catch it: the run must fail AND its output must
#      name that probe. A failure with another name means the plant is not exercising this rule.
# The unmutated copy runs first and must pass, so a red mutant run means the mutation and not the
# harness. The probes run under apps/web/vitest.size-probes.config.ts: no Astro builds.
#
# Usage: tools/test-size-loose-bound.sh   (needs apps/web/node_modules: `pnpm install`)
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
web="$(cd "$here/../apps/web" && pwd)"
drift="$web/src/size-budget-drift.ts"
suite="$web/src/size-budget.test.ts"
mdrift="$web/src/size-budget-drift.mutant.ts"
msuite="$web/src/size-budget.mutant.test.ts"
log="$(mktemp)"
cleanup() { trap - EXIT INT TERM HUP; rm -f "$mdrift" "$msuite" "$log"; return 0; }
trap cleanup EXIT INT TERM HUP

pass=0
fail=0
ok() { echo "  ok   $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL $1"; fail=$((fail + 1)); }

command -v node >/dev/null || { echo "REFUSED: node is not on PATH" >&2; exit 1; }
[ -d "$web/node_modules" ] || { echo "REFUSED: apps/web/node_modules is missing; run pnpm install" >&2; exit 1; }

# <old> <new> -> writes the mutant drift module and the suite copy that imports it; asserts it applied.
write_pair() {
  python3 -I - "$drift" "$suite" "$mdrift" "$msuite" "$1" "$2" <<'PY'
import sys
drift, suite, mdrift, msuite, old, new = sys.argv[1:]
d = open(drift).read()
if old != "" and old not in d:
    sys.exit("MUTATION DID NOT APPLY: " + old)
m = d.replace(old, new, 1) if old != "" else d
if old != "" and m == d:
    sys.exit("MUTATION CHANGED NOTHING")
s = open(suite).read()
imp = '"./size-budget-drift.js"'
if imp not in s:
    sys.exit("the suite does not import " + imp)
open(mdrift, "w").write(m)
open(msuite, "w").write(s.replace(imp, '"./size-budget-drift.mutant.js"'))
PY
}
run_suite() { # -> status; output in $log
  (cd "$web" && npx vitest run --config vitest.size-probes.config.ts src/size-budget.mutant.test.ts \
    -t "base lines at 0%" >"$log" 2>&1)
}

echo "the unmutated copy passes (so a red mutant below is the mutation's doing)"
if write_pair "" ""; then
  status=0; run_suite || status=$?
  if [ "$status" -eq 0 ]; then ok "unmutated copy: passes ($(grep -E 'Tests ' "$log" | head -1 | sed 's/^ *//'))"; else bad "unmutated copy failed (status $status)"; sed 's/^/        /' "$log" | tail -12; fi
else bad "could not write the unmutated copy"; fi

echo
echo "mutants (each must apply, fail the run, and fail NAMING its probe)"
mutant() { # <name> <old> <new> <probe name it must be caught by>
  local name="$1" old="$2" new="$3" probe="$4" status=0
  if ! write_pair "$old" "$new"; then bad "$name: the mutation did not apply"; return; fi
  run_suite || status=$?
  if [ "$status" -eq 0 ]; then bad "$name: SURVIVED (the suite passed)"; return; fi
  if ! grep -qF -- "$probe" "$log"; then
    bad "$name: failed, but not at the probe '$probe'"; grep -E '^ *(FAIL|×)' "$log" | head -4 | sed 's/^/        /'; return
  fi
  ok "$name: caught at '$probe'"
}
FLOOR="holds a small line to the 64-byte floor"
EDGE="passes a base line at exactly 1% off the recording's architecture, and fails one byte past"
MAX='Math.max(recordedBrotli * LOOSE_FRACTION, LOOSE_FLOOR_BYTES)'
GATE='moved > looseBound(record.measured_brotli)'
mutant "L1 the loose bound is 0" "$MAX" "0" "$EDGE"
mutant "L2 the loose check never fires" "$GATE" "false" "$EDGE"
mutant "L3 the 64 B floor is dropped" "$MAX" "recordedBrotli * LOOSE_FRACTION" "$FLOOR"
mutant "L4 the fraction is 2%, not 1%" "LOOSE_FRACTION = 0.01" "LOOSE_FRACTION = 0.02" "$EDGE"
mutant "L5 the floor is 128 B, not 64" "LOOSE_FLOOR_BYTES = 64" "LOOSE_FLOOR_BYTES = 128" "$FLOOR"
mutant "L6 the bound is inclusive of one byte more (>= for >)" "$GATE" "moved >= looseBound(record.measured_brotli)" "$EDGE"
echo
if [ "$fail" -ne 0 ]; then echo "FAILED — $fail case(s) failed, $pass passed" >&2; exit 1; fi
echo "OK — $pass case(s) all behaved as required"
