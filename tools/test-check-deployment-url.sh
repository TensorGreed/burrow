#!/usr/bin/env bash
# Adversarial self-test for tools/check-deployment-url.sh.
#
# THE ACCEPT HALF IS NOT OPTIONAL HERE, and it is the half that failed in production. The rule
# this replaces asserted `reported == $BURROW_SITE`, which no correct production deploy can
# satisfy -- wrangler always prints a per-deployment alias. It failed a run whose upload had
# succeeded, reporting a live site as a red deploy. A rule that refuses everything passes a
# refuse-only suite.
#
# THE REJECT HALF IS WHERE THE STRING FORM KEEPS BITING. `evil-burrow-f2s.pages.dev` ends with
# the same characters as `burrow-f2s.pages.dev`, so a suffix match accepts it. That is the
# fourth deny-or-match rule in this repository where the string form was the defect, so the
# lookalike is a named case rather than an afterthought.
#
# Usage: tools/test-check-deployment-url.sh

set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
checker="$here/check-deployment-url.sh"
ORIGIN="https://burrow-f2s.pages.dev"

pass=0
fail=0

accepts() {
  local why="$1" url="$2"
  if "$checker" "$url" "$ORIGIN" >/dev/null 2>&1; then
    echo "  ok   accepts  ${url:-<empty>}"
    pass=$((pass + 1))
  else
    echo "  FAIL rejects  ${url:-<empty>}  ($why)"
    fail=$((fail + 1))
  fi
}

rejects() {
  local why="$1" url="$2"
  if "$checker" "$url" "$ORIGIN" >/dev/null 2>&1; then
    echo "  FAIL ACCEPTS  ${url:-<empty>}  ($why)"
    fail=$((fail + 1))
  else
    echo "  ok   rejects  ${url:-<empty>}"
    pass=$((pass + 1))
  fi
}

echo "check-deployment-url: a deployment OF $ORIGIN, compared by label"
echo
echo "  must accept ---------------------------------------------------------------"

# THE CASE THAT FAILED IN PRODUCTION. wrangler prints this for a production deploy, and the
# previous rule -- string equality against the origin -- could never accept it.
accepts "wrangler's per-deployment alias, which it prints for production deploys too" \
  "https://f55a5097.burrow-f2s.pages.dev"
accepts "an alias whose label is not hex" "https://some-branch.burrow-f2s.pages.dev"
accepts "the production origin itself, in case wrangler ever prints it" "$ORIGIN"

echo
echo "  must reject ---------------------------------------------------------------"

# THE LOOKALIKE. It ends with the same characters as the expected host, so a suffix match
# accepts it. A label comparison does not: stripping to the first dot gives `pages.dev`.
rejects "a lookalike that ends with the same CHARACTERS but is a different label" \
  "https://evil-burrow-f2s.pages.dev"
rejects "the same trick with a hyphen elsewhere" "https://xburrow-f2s.pages.dev"
rejects "the expected host as a PREFIX of somebody else's domain" \
  "https://burrow-f2s.pages.dev.evil.example"
rejects "two labels in front is a different project's alias shape, not ours" \
  "https://a.b.burrow-f2s.pages.dev"
rejects "another project on the same host suffix" "https://other-project.pages.dev"
rejects "an alias of another project" "https://f55a5097.other-project.pages.dev"
rejects "http, because SRI and the CSP assume a secure context" \
  "http://burrow-f2s.pages.dev"
rejects "nothing reported at all, which is what a failed upload looks like" ""
rejects "a bare host with no scheme" "burrow-f2s.pages.dev"

echo
echo "  and the expected origin is required -------------------------------------"
if "$checker" "https://f55a5097.burrow-f2s.pages.dev" "" >/dev/null 2>&1; then
  echo "  FAIL it guessed an expected origin instead of refusing"
  fail=$((fail + 1))
else
  echo "  ok   refuses when given no expected origin"
  pass=$((pass + 1))
fi

# --- THE PROBE GATE: the comparison is a LABEL comparison, and a suffix match would pass the
# accept half while failing the reject half. Break it in a copy beside the original and require
# the lookalike to get through -- which is what shows the label logic is what refuses it.
copy="$here/.deployment-url-probe-fixture.sh"
sed 's|rest="${host#\*\.}"|rest="${host#*-}"|' "$checker" >"$copy"
chmod +x "$copy"
if cmp -s "$copy" "$checker"; then
  echo "  FAIL probe-gate: the mutation did not apply, so this case would prove nothing"
  fail=$((fail + 1))
elif "$copy" "https://evil-burrow-f2s.pages.dev" "$ORIGIN" >/dev/null 2>&1; then
  echo "  ok   probe-gate: splitting on a character rather than a label lets the lookalike in,"
  echo "       so the label split is what refuses it"
  pass=$((pass + 1))
else
  echo "  FAIL probe-gate: the lookalike is refused even with the label split broken, so these"
  echo "        cases are measuring a different rule"
  fail=$((fail + 1))
fi
rm -f "$copy"

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED — $fail case(s) failed, $pass passed" >&2
  exit 1
fi
echo "OK — $pass case(s) all behaved as required"
