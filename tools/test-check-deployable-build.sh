#!/usr/bin/env bash
# Adversarial self-test for tools/check-deployable-build.sh.
#
# That checker is the last gate before an upload, and every rule in it is "this string is the
# expected origin" -- a shape that passes on an empty directory and on a build that never ran.
# So each rule is broken in turn against a COPY of a real build and required to refuse, naming
# the reason.
#
# The fixtures are copies of `apps/web/dist`, never the build itself: a self-test that edits
# the thing being uploaded is one interrupted run away from deploying a planted file.
#
# Usage: tools/test-check-deployable-build.sh

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
checker="$here/check-deployable-build.sh"
real="$repo/apps/web/dist"
workroot=""
work=""

cleanup() {
  trap - EXIT INT TERM HUP
  [ -n "$workroot" ] && rm -rf "$workroot"
  rm -f "$here/.deployable-probe-fixture.sh"
  return 0
}
trap cleanup EXIT INT TERM HUP

pass=0
fail=0

if [ ! -d "$real" ]; then
  echo "  SKIP-REFUSED: no build at apps/web/dist. This suite needs one to copy; run" >&2
  echo "                \`pnpm build\` in apps/web. Refusing rather than passing vacuously." >&2
  exit 1
fi

# THE BUILD'S OWN ORIGIN, read from it rather than assumed. The suite has to work whether the
# fixture was built for localhost or for a real host, and hardcoding either would make this
# file pass or fail for a reason that has nothing to do with the checker.
built_for="$(grep -o '<meta name="burrow-built-for" content="[^"]*"' "$real/index.html" |
  head -1 | sed 's/.*content="//;s/"$//')"
[ -n "$built_for" ] || {
  echo "  SKIP-REFUSED: apps/web/dist carries no burrow-built-for stamp to test against." >&2
  exit 1
}

# A DEPLOYABLE origin to run the cases under. The checker refuses localhost outright, which is
# its own case below; every other case needs an origin it will accept, so the fixture is
# rewritten to one.
DEPLOY_ORIGIN="https://burrow.test"

workroot="$(mktemp -d)"
work="$workroot/dist"

# A fresh copy of the real build, rewritten to `$DEPLOY_ORIGIN`, so each case starts from
# something that PASSES.
fixture() {
  rm -rf "$work"
  cp -r "$real" "$work"
  # A HARNESS BUILD IS A USABLE FIXTURE ONCE IT STOPS BEING ONE. `pnpm e2e` leaves a harness
  # build in `apps/web/dist`, and the checker refuses those by design -- so the baseline case
  # failed with "the rewritten build does not pass", which reads as an accusation against the
  # checker rather than as "your dist is the wrong variant". The harness rule has its own two
  # cases below, which plant these directories deliberately.
  rm -rf "$work/harness" "$work/host"
  grep -rlF "$built_for" "$work" 2>/dev/null | while IFS= read -r file; do
    sed -i "s|${built_for//|/\\|}|$DEPLOY_ORIGIN|g" "$file"
  done
}

expect_refusal() {
  local name="$1" expect="$2"
  shift 2
  local out status=0
  out="$("$@" 2>&1)" || status=$?
  if [ "$status" -eq 0 ]; then
    echo "  FAIL $name: it did not refuse"
    fail=$((fail + 1))
    return
  fi
  if ! grep -qF "$expect" <<<"$out"; then
    echo "  FAIL $name: it refused, but not for the stated reason"
    echo "        wanted: $expect"
    sed 's/^/        /' <<<"$out" | tail -3
    fail=$((fail + 1))
    return
  fi
  echo "  ok   $name"
  pass=$((pass + 1))
}

# THE BASELINE. Without it every case below could be passing against a checker that refuses
# everything, which is the same non-check as one that refuses nothing.
fixture
if "$checker" "$DEPLOY_ORIGIN" "$work" >/dev/null 2>&1; then
  echo "  ok   a rewritten build passes, so a refusal below means something"
  pass=$((pass + 1))
else
  echo "  FAIL the rewritten build does not pass, so no case below means anything"
  "$checker" "$DEPLOY_ORIGIN" "$work" 2>&1 | tail -4 | sed 's/^/        /'
  exit 1
fi

# --- THE CASE THIS CHECKER EXISTS FOR: the right build, the wrong origin --------------------
fixture
expect_refusal "a build for another origin is refused, which is the whole point" \
  "is stamped for $DEPLOY_ORIGIN, not https://somewhere.else" \
  "$checker" "https://somewhere.else" "$work"

# --- and a development build, which is the one most likely to be sitting in dist/ -----------
fixture
expect_refusal "a localhost origin is refused as not deployable at all" \
  "refusing to call http://localhost:4321 a deployable origin" \
  "$checker" "http://localhost:4321" "$work"

# --- Rule: the run-time stamp ----------------------------------------------------------------
fixture
sed -i 's|<meta name="burrow-built-for"[^>]*>||' "$work/split-pdf/index.html"
expect_refusal "a page with no origin stamp is refused" \
  "carries no burrow-built-for stamp" \
  "$checker" "$DEPLOY_ORIGIN" "$work"

# --- Rule: the canonical link ----------------------------------------------------------------
#
# The regression that prompted all of this: no `site:` in astro.config.mjs meant every page
# shipped `<link rel="canonical" href="http://localhost/...">` while the CSP named the real
# origin. Re-planted exactly.
fixture
sed -i 's|<link rel="canonical" href="[^"]*"|<link rel="canonical" href="http://localhost/split-pdf/"|' \
  "$work/split-pdf/index.html"
expect_refusal "the localhost canonical link, re-planted, is refused" \
  "is canonical at http://localhost/split-pdf/" \
  "$checker" "$DEPLOY_ORIGIN" "$work"

# --- Rule: the page's own CSP ----------------------------------------------------------------
fixture
sed -i "s|connect-src $DEPLOY_ORIGIN/|connect-src https://elsewhere.test/|" \
  "$work/split-pdf/index.html"
expect_refusal "a page whose meta CSP names another origin is refused" \
  "own CSP does not name $DEPLOY_ORIGIN" \
  "$checker" "$DEPLOY_ORIGIN" "$work"

# --- Rule: the page's own CSP naming an EXTRA origin ------------------------------------------
#
# THE GAP A SECURITY REVIEW FOUND, re-planted. The page rule checked only that the expected
# origin was PRESENT, while the `_headers` rule below checked exclusivity -- so appending
# ` https://evil.example` to a page's connect-src passed, and the gate printed "deployable to
# ... and to nowhere else" over a build whose in-markup policy permitted an engine fetch to an
# attacker origin. On the hosts that ignore `_headers`, that markup IS the policy.
#
# There was no case for it here, which is why nothing caught it. The asymmetry between the two
# rules is what made it a defect rather than a decision.
fixture
sed -i "s|; style-src| https://evil.example; style-src|" "$work/index.html"
expect_refusal "a page whose meta CSP names an EXTRA origin is refused, not just a wrong one" \
  "own CSP names origins other than $DEPLOY_ORIGIN" \
  "$checker" "$DEPLOY_ORIGIN" "$work"

fixture
sed -i 's|<meta http-equiv="Content-Security-Policy"[^>]*>||' "$work/index.html"
expect_refusal "a page with no meta CSP at all is refused" \
  "carries no <meta> CSP" \
  "$checker" "$DEPLOY_ORIGIN" "$work"

# --- Rule: a LOOKALIKE origin, which the stray check used to accept -----------------------------
#
# `grep -v "^$expected$"` used the origin as a REGEX, so `.` matched any character and
# `https://burrow.app` accepted a stray `https://burrow-app`. Registrable, and it satisfied the
# one rule that did enforce exclusivity. The fixture origin is `https://burrow.test`, so the
# lookalike here is `https://burrowXtest`.
fixture
sed -i "s|; style-src| https://burrowXtest/x.wasm; style-src|" "$work/_headers"
expect_refusal "a lookalike origin differing only where a regex dot would match is refused" \
  "names origins other than $DEPLOY_ORIGIN" \
  "$checker" "$DEPLOY_ORIGIN" "$work"

# --- Rule: _headers --------------------------------------------------------------------------
fixture
sed -i "s|connect-src $DEPLOY_ORIGIN/|connect-src https://elsewhere.test/|" "$work/_headers"
expect_refusal "a _headers naming another origin is refused" \
  "_headers does not name $DEPLOY_ORIGIN" \
  "$checker" "$DEPLOY_ORIGIN" "$work"

# APPENDED, NOT PREPENDED. The first attempt put the stray origin immediately after
# `connect-src `, which made the very next token a foreign URL -- so the "names $expected"
# rule fired first and this case passed for the wrong reason. The stray goes at the END of
# the directive, where the expected origin is still present and only the EXTRA one is wrong.
fixture
sed -i "s|; style-src| https://extra.test/x.wasm; style-src|" "$work/_headers"
expect_refusal "a _headers naming an EXTRA origin is refused, not just a wrong one" \
  "names origins other than $DEPLOY_ORIGIN" \
  "$checker" "$DEPLOY_ORIGIN" "$work"

# --- Rule: the worker bundle's absolute URLs -------------------------------------------------
fixture
worker="$(find "$work/engines" -name 'burrow-worker.*.js' | head -1)"
python3 - "$worker" "$DEPLOY_ORIGIN" <<'PLANT'
import sys
path, origin = sys.argv[1], sys.argv[2]
text = open(path).read()
needle = '"url": "%s' % origin
assert needle in text, "nothing to plant into"
open(path, "w").write(text.replace(needle, '"url": "https://elsewhere.test', 1))
PLANT
expect_refusal "a worker bundle fetching from another origin is refused" \
  "the worker bundle fetches from" \
  "$checker" "$DEPLOY_ORIGIN" "$work"

fixture
worker="$(find "$work/engines" -name 'burrow-worker.*.js' | head -1)"
sed -i "s|\"probeOrigin\": \"$DEPLOY_ORIGIN\"|\"probeOrigin\": \"https://elsewhere.test\"|" "$worker"
expect_refusal "a worker bundle probing another origin is refused" \
  "probeOrigin is not $DEPLOY_ORIGIN" \
  "$checker" "$DEPLOY_ORIGIN" "$work"

# --- Rule: the harness must not reach a deploy ------------------------------------------------
for variant in harness host; do
  fixture
  mkdir -p "$work/$variant"
  expect_refusal "a harness build is refused before upload ($variant/)" \
    "this is a HARNESS build" \
    "$checker" "$DEPLOY_ORIGIN" "$work"
done

# --- The shapes an absence-style checker is worst at ------------------------------------------
fixture
rm -rf "${work:?}/"*
expect_refusal "a build with no pages is refused rather than passing every rule" \
  "no pages under" \
  "$checker" "$DEPLOY_ORIGIN" "$work"

expect_refusal "a missing build directory is refused" \
  "no build at" \
  "$checker" "$DEPLOY_ORIGIN" "$work/does-not-exist"

fixture
expect_refusal "no origin at all is refused rather than guessed" \
  "no origin given" \
  env -u BURROW_SITE "$checker" "" "$work"

# --- THE PROBE GATE: each rule is the thing that catches its own defect -------------------------
#
# Every case above shows the checker refusing. None shows WHICH rule refused, and a checker
# whose rules all matched everything would pass all of them. This breaks ONE rule in a copy of
# the checker, replants that rule's defect, and requires the gutted copy to PASS -- which is
# only true if that rule was doing the work.
copy="$here/.deployable-probe-fixture.sh"

probe_gate() {
  local name="$1" mutation="$2" plant="$3"
  sed "$mutation" "$checker" >"$copy"
  chmod +x "$copy"
  if cmp -s "$copy" "$checker"; then
    echo "  FAIL probe-gate/$name: the mutation did not apply, so this case would prove nothing"
    fail=$((fail + 1))
    return
  fi
  fixture
  eval "$plant"
  if "$checker" "$DEPLOY_ORIGIN" "$work" >/dev/null 2>&1; then
    echo "  FAIL probe-gate/$name: the unmutated checker did not refuse, so nothing was planted"
    fail=$((fail + 1))
    return
  fi
  if "$copy" "$DEPLOY_ORIGIN" "$work" >/dev/null 2>&1; then
    echo "  ok   probe-gate/$name: that rule alone is what catches it"
    pass=$((pass + 1))
  else
    echo "  FAIL probe-gate/$name: still caught with the rule removed, so the case above was"
    echo "        measuring a different rule"
    fail=$((fail + 1))
  fi
}

probe_gate "the stamp rule" \
  's|^  \[ "\$stamp" = "\$expected" \]|  [ -n "$stamp" ]|' \
  'sed -i "s|content=\"$DEPLOY_ORIGIN\"|content=\"https://elsewhere.test\"|" "$work/split-pdf/index.html"'

probe_gate "the canonical rule" \
  's|^    "\$expected"/\*) : ;;|    *) : ;;|' \
  'sed -i "s|<link rel=\"canonical\" href=\"[^\"]*\"|<link rel=\"canonical\" href=\"http://localhost/x/\"|" "$work/split-pdf/index.html"'

probe_gate "the _headers extra-origin rule" \
  's|^\[ -z "\$stray" \]|[ -n "${stray:-x}" ]|' \
  'sed -i "s|; style-src| https://extra.test/x.wasm; style-src|" "$work/_headers"'

probe_gate "the harness rule" \
  's|^for forbidden in harness host; do|for forbidden in zzz-no-such-dir; do|' \
  'mkdir -p "$work/harness"'

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED — $fail case(s) failed, $pass passed" >&2
  exit 1
fi
echo "OK — $pass adversarial case(s) all behaved as required"
