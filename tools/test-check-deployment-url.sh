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

# ---------------------------------------------------------------------------------------
# THE CUSTOM-DOMAIN SHAPE. `BURROW_SITE` is the custom domain and wrangler's alias is on the
# PROJECT's pages.dev host, so the two are different hosts and the third argument is what
# reconciles them. Without these cases the third argument could do nothing at all and every
# case above would still pass -- it is a new branch, so it gets its own positives AND its own
# near-misses.
CUSTOM="https://notonlypdf.com"
PROJECT_HOST="burrow-f2s.pages.dev"

accepts_custom() {
  local why="$1" url="$2"
  if "$checker" "$url" "$CUSTOM" "$PROJECT_HOST" >/dev/null 2>&1; then
    echo "  ok   accepts  ${url:-<empty>}"
    pass=$((pass + 1))
  else
    echo "  FAIL rejects  ${url:-<empty>}  ($why)"
    fail=$((fail + 1))
  fi
}

# THE MESSAGE IS CHECKED, NOT ONLY THE EXIT STATUS, and the difference was measured: security
# review deleted the ENTIRE project-host validation block and this suite stayed green, because
# the three "a host, not a URL" cases were being refused by the fallthrough at the end of the
# script rather than by the rule they name. A case that passes for the wrong reason is a case
# that stops binding its rule the moment the rule is removed.
#
# It also prints the HOST, because that is what varies in those three cases -- three identical
# `ok rejects <same url>` lines say nothing about what was examined.
rejects_custom() {
  local why="$1" url="$2" host="${3:-$PROJECT_HOST}" expect="${4:-}"
  local out status=0
  out="$("$checker" "$url" "$CUSTOM" "$host" 2>&1)" || status=$?
  if [ "$status" -eq 0 ]; then
    echo "  FAIL ACCEPTS  ${url:-<empty>} (host ${host:-<empty>})  ($why)"
    fail=$((fail + 1))
    return
  fi
  if [ -n "$expect" ] && ! grep -qF "$expect" <<<"$out"; then
    echo "  FAIL ${url:-<empty>} (host ${host:-<empty>}): refused, but not for the stated reason"
    echo "        wanted: $expect"
    sed 's/^/        /' <<<"$out" | tail -2
    fail=$((fail + 1))
    return
  fi
  echo "  ok   rejects  ${url:-<empty>} (host ${host:-<empty>})"
  pass=$((pass + 1))
}

echo
echo "  custom domain: alias on the project host, site on another host ---------------"

accepts_custom "the production alias, which is the ONLY thing a correct custom-domain deploy \
reports -- and which the two-argument form could never accept" \
  "https://f55a5097.burrow-f2s.pages.dev"
accepts_custom "the project host itself" "https://burrow-f2s.pages.dev"

# THE LABEL RULE MUST STILL HOLD against the new base, not just against the old one.
rejects_custom "a lookalike of the PROJECT host, now that the project host is the base" \
  "https://evil-burrow-f2s.pages.dev"
rejects_custom "another project's alias, which is the thing this check exists to catch" \
  "https://f55a5097.other-project.pages.dev"
rejects_custom "two labels in front of the project host" \
  "https://a.b.burrow-f2s.pages.dev"

# AND THE SITE ORIGIN IS NO LONGER A BASE. With a custom domain wrangler cannot report it, so
# accepting it would mean the base was silently still the old one -- the exact confusion this
# argument was added to end.
rejects_custom "the custom domain itself, which wrangler never reports and which would mean \
the project host was being ignored" "https://notonlypdf.com"

# THE THIRD ARGUMENT IS A HOST, NOT A URL. Passing an origin here is the obvious slip, and
# silently stripping it would make `https://x` and `x` the same argument in a security check.
rejects_custom "a URL where a host belongs" "https://f55a5097.burrow-f2s.pages.dev" \
  "https://burrow-f2s.pages.dev" "is a HOST, not a URL"
rejects_custom "a host carrying a path" "https://f55a5097.burrow-f2s.pages.dev" \
  "burrow-f2s.pages.dev/x" "must carry no path"
rejects_custom "a single label, which cannot be a deployment host" \
  "https://f55a5097.burrow-f2s.pages.dev" "localhost" "must be a <project>.pages.dev name"

# THE ZONE ITSELF. `pages.dev` is a valid-looking hostname and accepting it would make every
# Cloudflare Pages project in the world "an alias of ours" -- the base carries the whole
# comparison once it is supplied, so a base that is too short silently widens the check.
rejects_custom "the pages.dev ZONE as the base, which would accept any project's alias" \
  "https://f55a5097.someone-elses-project.pages.dev" "pages.dev" \
  "must be a <project>.pages.dev name"
rejects_custom "an empty first label, which matches the suffix and is not a project" \
  "https://f55a5097.burrow-f2s.pages.dev" ".pages.dev" "empty first label"
rejects_custom "a host that is not on pages.dev at all" \
  "https://f55a5097.burrow-f2s.pages.dev" "evil.example" \
  "must be a <project>.pages.dev name"
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
