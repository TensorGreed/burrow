#!/usr/bin/env bash
# This `dist/` is for THIS origin, and it says so everywhere it has to.
#
# WHY A DEPLOY NEEDS ITS OWN GATE. ADR 0014 §4 makes the origin a BUILD input: `connect-src`
# names the exact content-hashed engine URLs and the worker bundle carries absolute URLs,
# because a `blob:` worker's `self.location` is opaque and a relative `fetch` fails to parse
# before CSP is consulted. So a `dist/` is bound to one origin, and uploading the wrong one
# produces a site that looks perfect and whose every tool is dead.
#
# `apps/web/src/built-for-origin.test.ts` already asserts the four places AGREE WITH EACH
# OTHER. That is not the same question. A build for `http://localhost:4321` is perfectly
# self-consistent, and it is the one a person gets by running `pnpm build` without thinking
# about it -- which is the build most likely to be sitting in `dist/` at the moment somebody
# decides to deploy. This asks the other question: are they the origin you MEANT?
#
# Usage:
#   tools/check-deployable-build.sh https://burrow.example [dist]
#   BURROW_SITE=https://burrow.example tools/check-deployable-build.sh
#
# Self-test: tools/test-check-deployable-build.sh.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
expected="${1:-${BURROW_SITE:-}}"
dist="${2:-$repo/apps/web/dist}"

fail() {
  echo "::error::$*" >&2
  exit 1
}

[ -n "$expected" ] ||
  fail "no origin given -- pass it as an argument or set BURROW_SITE. This check is about \
whether the build is for the origin you MEANT, so it cannot guess one"
[ -d "$dist" ] || fail "no build at $dist -- run \`pnpm build\` in apps/web first"

# Normalised the same way `tools/build-origin.mjs` normalises it, so a trailing slash in an
# argument is not reported as a mismatch nobody can see.
expected="${expected%/}"

case "$expected" in
  http://localhost*|http://127.0.0.1*|https://localhost*)
    fail "refusing to call $expected a deployable origin. That is the development default \
(tools/build-origin.mjs), and a build carrying it is the one \`pnpm build\` produces when \
nobody passed BURROW_SITE -- which is exactly the build most likely to be sitting in dist/ \
when somebody decides to deploy"
    ;;
esac

pages=()
while IFS= read -r page; do pages+=("$page"); done < <(find "$dist" -name 'index.html' | sort)
[ "${#pages[@]}" -gt 0 ] || fail "no pages under $dist -- nothing to check, which is not a pass"

echo "check-deployable-build: ${#pages[@]} page(s) under ${dist#"$repo"/}, expecting $expected"

# --- every page's own stamp, canonical link and CSP -----------------------------------------
#
# Three rules rather than one, because they are three different generators: the stamp comes
# from `Astro.site`, the canonical link from `Astro.site` too but through a `new URL(...)`, and
# the CSP from `stage-web-engines.mjs`. A single grep over the file would pass if any one of
# them carried the right string.
for page in "${pages[@]}"; do
  shown="${page#"$dist"/}"

  # `|| true` ON EVERY EXTRACTION. `grep` exits 1 when it finds nothing, and under
  # `set -euo pipefail` an assignment from such a pipeline kills the script BEFORE the
  # `[ -n ... ] || fail` below can report why. Measured by this checker's own self-test: a
  # page with the stamp deleted exited non-zero having printed nothing at all, which reads as
  # a crash rather than as the refusal it is.
  stamp="$(grep -o '<meta name="burrow-built-for" content="[^"]*"' "$page" | head -1 |
    sed 's/.*content="//;s/"$//' || true)"
  [ -n "$stamp" ] || fail "$shown carries no burrow-built-for stamp, so its own origin guard \
has nothing to compare against at run time"
  [ "$stamp" = "$expected" ] || fail "$shown is stamped for $stamp, not $expected"

  canonical="$(grep -o '<link rel="canonical" href="[^"]*"' "$page" | head -1 |
    sed 's/.*href="//;s/"$//' || true)"
  [ -n "$canonical" ] || fail "$shown has no canonical link"
  case "$canonical" in
    "$expected"/*) : ;;
    *) fail "$shown is canonical at $canonical, not under $expected" ;;
  esac

  grep -qF "connect-src $expected/" "$page" ||
    fail "$shown's own CSP does not name $expected in connect-src. ADR 0014 §5 duplicates the \
policy into the markup because a static host may ignore _headers, so this is the policy in \
force on exactly the hosts that need it most"
done
echo "  ${#pages[@]} page(s): stamp, canonical and meta CSP all name $expected"

# --- the header policy ----------------------------------------------------------------------
headers="$dist/_headers"
[ -f "$headers" ] || fail "no _headers in the build"
grep -qF "connect-src $expected/" "$headers" ||
  fail "_headers does not name $expected in connect-src"
stray="$(grep -o 'https\?://[^ ;"]*' "$headers" | sed 's|\(https\?://[^/]*\).*|\1|' | sort -u |
  grep -v "^$expected$" || true)"
[ -z "$stray" ] || fail "_headers names origins other than $expected: $(echo "$stray" | tr '\n' ' ')"
echo "  _headers names $expected and no other origin"

# --- the worker bundle's absolute URLs -------------------------------------------------------
bundle="$(find "$dist/engines" -maxdepth 1 -name 'burrow-worker.*.js' | head -1)"
[ -n "$bundle" ] || fail "no worker bundle in $dist/engines"
bundle_origins="$(grep -o '"url": "https\?://[^"]*"' "$bundle" |
  sed 's|"url": "\(https\?://[^/]*\)/.*|\1|' | sort -u || true)"
[ -n "$bundle_origins" ] || fail "the worker bundle names no absolute engine URLs, which would \
mean the generated manifest is missing -- a relative fetch cannot work from a blob: worker"
[ "$bundle_origins" = "$expected" ] ||
  fail "the worker bundle fetches from: $(echo "$bundle_origins" | tr '\n' ' ')-- expected $expected"
grep -qF "\"probeOrigin\": \"$expected\"" "$bundle" ||
  fail "the worker bundle's probeOrigin is not $expected"
echo "  worker bundle fetches only from $expected"

# --- and the harness must not be here --------------------------------------------------------
#
# Not an origin rule, and it is here because this is the last gate before an upload. `/harness`
# exposes `window.burrowHarness`, which drives a real file through the real worker.
for forbidden in harness host; do
  [ ! -e "$dist/$forbidden" ] ||
    fail "$dist/$forbidden exists, so this is a HARNESS build. Rebuild without BURROW_HARNESS"
done
echo "  no harness route and no /host/ directory"

echo "OK -- this build is deployable to $expected and to nowhere else."
