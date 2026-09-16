#!/usr/bin/env bash
# This `dist/` is for THIS origin, and it says so everywhere it has to.
#
# WHY A DEPLOY NEEDS ITS OWN GATE. ADR 0014 §4 makes the origin a BUILD input: `connect-src`
# names the exact content-hashed engine URLs and the worker bundle carries absolute URLs,
# because a `blob:` worker's `self.location` is opaque and a relative `fetch` fails to parse
# before CSP is consulted. So a `dist/` is bound to one origin, and uploading the wrong one
# produces a site that looks perfect and whose every tool is dead.
#
# `apps/web/src/built-for-origin.test.ts` already asserts the six places AGREE WITH EACH
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

  # PRESENCE AND EXCLUSIVITY. The first version checked only that the expected origin appeared,
  # which is not the same claim as the line this script prints at the end. A security review
  # appended ` https://evil.example` to a page's `connect-src` and the gate reported
  # "deployable to https://ok.example and to nowhere else" -- false for that build, on the
  # copy of the policy that is in force on exactly the hosts that ignore `_headers`.
  #
  # The `_headers` rule below already did this, which is what made the asymmetry a defect
  # rather than a decision.
  page_csp="$(grep -o '<meta http-equiv="Content-Security-Policy" content="[^"]*"' "$page" |
    head -1 | sed 's/.*content="//;s/"$//' || true)"
  [ -n "$page_csp" ] || fail "$shown carries no <meta> CSP. ADR 0014 §5 duplicates the policy \
into the markup because a static host may ignore _headers"
  grep -qF "connect-src $expected/" <<<"$page_csp" ||
    fail "$shown's own CSP does not name $expected in connect-src"
  page_stray="$(grep -o 'https\?://[^ ;"]*' <<<"$page_csp" |
    sed 's|\(https\?://[^/]*\).*|\1|' | sort -u | grep -vxF -- "$expected" || true)"
  [ -z "$page_stray" ] ||
    fail "$shown's own CSP names origins other than $expected: $(printf '%s' "$page_stray" | tr '\n' ' ')"
done
echo "  ${#pages[@]} page(s): stamp, canonical and meta CSP name $expected and no other origin"

# --- the header policy ----------------------------------------------------------------------
headers="$dist/_headers"
[ -f "$headers" ] || fail "no _headers in the build"
grep -qF "connect-src $expected/" "$headers" ||
  fail "_headers does not name $expected in connect-src"
# `-vxF`, NOT `-v "^...$"`. The origin was being used as a REGEX, so `.` matched any
# character and `https://burrow.app` accepted a stray `https://burrow-app` -- a registrable
# lookalike satisfying the one rule that did enforce exclusivity. Reproduced by security
# review. `-F` is literal, `-x` is whole-line.
stray="$(grep -o 'https\?://[^ ;"]*' "$headers" | sed 's|\(https\?://[^/]*\).*|\1|' | sort -u |
  grep -vxF -- "$expected" || true)"
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

# --- what a crawler is told ------------------------------------------------------------------
#
# LAST GATE BEFORE THE UPLOAD, and these two are the quietest way to get the origin wrong:
# nothing on the page breaks, and the only symptom is a search engine being asked to index a
# host this build is not for. They matter more since the custom domain, because the same bytes
# are also served permanently on the project's `pages.dev` host with no redirect available --
# so the sitemap and the canonical links are the whole of what stops one site being counted
# as two.
robots="$dist/robots.txt"
[ -f "$robots" ] || fail "no robots.txt in the build, so nothing points a crawler at a sitemap"
sitemap_line="$(grep -iE '^Sitemap:' "$robots" | head -1 | awk '{print $2}')"
[ -n "$sitemap_line" ] || fail "robots.txt names no sitemap"
# THE EXACT FILE THIS GATE THEN VALIDATES, not merely something under the origin. Security
# review measured the gap: `Sitemap: $expected/not-the-one-that-was-checked` passed, so the
# gate validated a document no crawler is directed to. A rule that checks a different file
# from the one it points at is two rules that never meet.
case "$sitemap_line" in
  "$expected"/sitemap.xml) : ;;
  *) fail "robots.txt points at $sitemap_line; this gate validates $expected/sitemap.xml and \
a crawler must be sent to the file that was checked" ;;
esac
grep -qiE '^Disallow:[[:space:]]*/[[:space:]]*$' "$robots" &&
  fail "robots.txt disallows the whole site; this build would never be indexed"

sitemap="$dist/sitemap.xml"
[ -f "$sitemap" ] || fail "no sitemap.xml in the build, though robots.txt promises one"

# THE SET, NOT THE COUNT. A count was the first version of this rule and security review
# measured what it misses: replacing `/credits/` with a second copy of `/` gives seven entries,
# all on the right origin, with a page silently absent -- and the comment claiming the count
# was the defence against "4 of 15" was then false, because a count is satisfiable by
# duplicates. Comparing sorted sets says which route is missing AND which is extra, and cannot
# be satisfied by a repeat.
locs="$(grep -oE '<loc>[^<]*</loc>' "$sitemap" | sed -e 's|<loc>||' -e 's|</loc>||' || true)"
loc_count="$(printf '%s\n' "$locs" | grep -c . || true)"

# A CASE GLOB, NOT A GREP PATTERN, and this file already records why at the `_headers` rule
# above: the origin was being used as a REGEX there, so `.` matched any character and
# `https://burrow.app` accepted a stray `https://burrow-app` -- a registrable lookalike
# satisfying the one rule that enforced exclusivity. Code review caught the identical defect
# re-planted HERE, three blocks below the comment describing it, and measured it: with the
# real origin, `https://notonlypdfXcom` was accepted and both success lines printed. `case`
# does literal prefix matching on a shell pattern, and `$expected` contains no glob
# metacharacter because `build-origin.mjs` produced it with `URL.origin`.
stray_locs=""
while IFS= read -r loc; do
  [ -n "$loc" ] || continue
  case "$loc" in
    "$expected"/*) : ;;
    *) stray_locs="$stray_locs $loc" ;;
  esac
done <<EOF
$locs
EOF
[ -z "$stray_locs" ] || fail "the sitemap lists URLs not under $expected:$stray_locs"

# EVERY BUILT PAGE IS LISTED, AND NOTHING ELSE IS. `pages` holds the index.html paths found by
# scanning the build; a page's URL is its directory with a trailing slash.
want_paths="$(for page in "${pages[@]}"; do
  rel="${page#"$dist"}"
  printf '%s\n' "${rel%index.html}"
done | sort)"
# `|| true` ON EVERY FILTER IN THIS PIPELINE. `set -e` applies inside a command substitution,
# and `grep` returning 1 on no match is not an error here -- an EMPTY sitemap is exactly the
# case this block must REPORT. Without it the script died silently part way through, which the
# self-test caught as "refused, but not for the stated reason": a gate that exits non-zero with
# no message is worse than the defect it was looking for, because the caller sees a failure
# with nothing to act on.
got_paths="$(printf '%s\n' "$locs" | grep -v '^$' | while IFS= read -r loc; do
  printf '%s\n' "${loc#"$expected"}"
done | sort || true)"
missing="$(comm -23 <(printf '%s\n' "$want_paths") <(printf '%s\n' "$got_paths") | tr '\n' ' ')"
extra="$(comm -13 <(printf '%s\n' "$want_paths") <(printf '%s\n' "$got_paths") | tr '\n' ' ')"
[ -z "${missing// /}" ] || fail "the sitemap does not list built page(s): $missing"
[ -z "${extra// /}" ] || fail "the sitemap lists page(s) the build does not contain: $extra"
echo "  robots.txt + sitemap.xml: $loc_count URL(s), exactly the built pages, all under $expected"

echo "OK -- this build is deployable to $expected and to nowhere else."
