#!/usr/bin/env bash
# Adversarial self-test for tools/check-release-artifact.sh.
#
# This is the real command publish and the build job both run against a real artifact, so its
# refusals are tested against real files on disk -- not a JSON fixture standing in for the command,
# which is exactly how the previous stamp check shipped a defect that refused every release and
# was caught by no test. Each case builds a tiny dist, breaks one thing, and requires a refusal
# naming its reason. The baseline passes, so a refusal below means something.

set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
checker="$here/check-release-artifact.sh"
SHA=0123456789abcdef0123456789abcdef01234567
pass=0
fail=0

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

# Build a minimal, complete artifact stamped for $SHA (no trailing newline).
make_dist() {
  local d="$1" sha="$2"
  rm -rf "$d"; mkdir -p "$d"
  echo "<!doctype html>" > "$d/index.html"
  printf 'x' > "$d/_headers"
  printf '%s' "$sha" > "$d/release-sha.txt"
}

# expect <pass|fail> <label> <needle> -- runs the checker on $work/dist for $SHA
expect() {
  local want=$1 label=$2 needle=$3 out status=0
  out="$("$checker" "$work/dist" "$SHA" 2>&1)" || status=$?
  if [ "$want" = pass ]; then
    if [ "$status" -eq 0 ]; then echo "  ok   $label"; pass=$((pass + 1));
    else echo "  FAIL $label: expected pass, got $status"; sed 's/^/       /' <<<"$out"; fail=$((fail + 1)); fi
  else
    if [ "$status" -eq 0 ]; then echo "  FAIL $label: expected refusal, passed"; fail=$((fail + 1));
    elif grep -qF -- "$needle" <<<"$out"; then echo "  ok   $label"; pass=$((pass + 1));
    else echo "  FAIL $label: refused, but not for '$needle'"; sed 's/^/       /' <<<"$out"; fail=$((fail + 1)); fi
  fi
}

echo "check-release-artifact self-test:"

make_dist "$work/dist" "$SHA"
expect pass "a complete artifact stamped for the sha passes" ""

# THE ORIGINAL DEFECT, reproduced: a dist with no stamp file. The previous check pointed at a path
# nothing stamped, so this was its permanent state -- and it must go red, which is what makes the
# build job's self-check on main catch it the day it breaks.
make_dist "$work/dist" "$SHA"; rm -f "$work/dist/release-sha.txt"
expect fail "a missing release-sha.txt is refused (the original always-fail bug)" "release-sha.txt is absent"

make_dist "$work/dist" "ffffffffffffffffffffffffffffffffffffffff"
expect fail "a mismatched release-sha.txt is refused" "not this commit's build"

make_dist "$work/dist" "$SHA"; printf '%s\n' "$SHA" > "$work/dist/release-sha.txt"
expect fail "a trailing newline is refused" "trimming whitespace"

make_dist "$work/dist" "$SHA"; printf ' %s ' "$SHA" > "$work/dist/release-sha.txt"
expect fail "surrounding whitespace is refused" "trimming whitespace"

make_dist "$work/dist" "$SHA"; rm -f "$work/dist/"*/index.html "$work/dist/index.html"
expect fail "an artifact with no pages is refused as incomplete" "no index.html"

make_dist "$work/dist" "$SHA"; rm -f "$work/dist/_headers"
expect fail "a missing _headers is refused" "_headers"

make_dist "$work/dist" "$SHA"; rm -rf "$work/dist/release-sha.txt"; mkdir "$work/dist/release-sha.txt"
expect fail "a release-sha.txt that is a directory is refused" "not a regular file"

# A malformed expectation (caller bug) must refuse, not compare loosely.
make_dist "$work/dist" "$SHA"
if out="$("$checker" "$work/dist" "not-a-sha" 2>&1)"; then
  echo "  FAIL a non-hex expected sha is refused: passed"; fail=$((fail + 1))
elif grep -qF "not lowercase hex" <<<"$out"; then
  echo "  ok   a non-hex expected sha is refused"; pass=$((pass + 1))
else
  echo "  FAIL a non-hex expected sha is refused: wrong reason"; sed 's/^/       /' <<<"$out"; fail=$((fail + 1))
fi

if [ "$fail" -gt 0 ]; then
  echo "FAILED -- $fail case(s) failed, $pass passed"
  exit 1
fi
echo "OK -- $pass artifact case(s) all behaved as required"
