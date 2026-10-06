#!/usr/bin/env bash
# The artifact-side release precondition: this dist is the build of a specific commit, and it is
# complete and readable.
#
#     tools/check-release-artifact.sh <dist> <expected-sha>
#
# THE SAME COMMAND IN TWO PLACES, deliberately. `deploy.yml`'s build job runs it on a main push
# against the artifact it just built (expected = `$GITHUB_SHA`), and the publish job runs it on a
# tag against the artifact it downloaded by run id (expected = the tagged sha). Because it is one
# script, not two copies, a precondition that can never pass fails on `main` the day it breaks --
# which is how the previous version's defect reached a release: it checked `apps/web/dist` for a
# `build-stamp` that nothing ever wrote there, so it refused every tag, and no test ran the real
# command (a fixture stood in for it). The stamp is now a literal SHA this script both writes the
# expectation for and reads back.
#
# It needs only coreutils, so it runs in the checker jobs and in the build job alike.

set -uo pipefail

dist="${1:-}"
expected="${2:-}"

fail() { echo "refuse: $*" >&2; exit 1; }

[ -n "$dist" ]     || fail "no dist directory given (arg 1)"
[ -n "$expected" ] || fail "no expected sha given (arg 2)"
# The expectation comes from the workflow (GITHUB_SHA or the tagged sha); a malformed one is a
# caller bug, not an artifact problem, but it must still refuse rather than compare loosely.
case "$expected" in
  *[!0-9a-f]* | "") fail "the expected sha '$expected' is not lowercase hex" ;;
esac
[ "${#expected}" -eq 40 ] || fail "the expected sha '$expected' is not 40 characters"

[ -d "$dist" ] || fail "no build at '$dist' -- nothing to check, which is not a pass"
# Complete and readable: a build with no pages is not a deployable artifact. (The deep
# origin/manifest checks live in check-deployable-build.sh, which both callers also run; this is
# the cheap completeness gate that travels with the SHA stamp.)
[ -n "$(find "$dist" -name 'index.html' -type f 2>/dev/null | head -1)" ] \
  || fail "'$dist' contains no index.html -- the artifact is empty or incomplete"
[ -r "$dist/_headers" ] || fail "'$dist/_headers' is missing or unreadable -- the response-header policy is not in the artifact"

stamp="$dist/release-sha.txt"
[ -e "$stamp" ]        || fail "release-sha.txt is absent from the artifact -- it cannot be shown to be this commit's build (this is the shape of the defect fixed in the SHA-stamp PR: a precondition checking a file nothing writes)"
[ -f "$stamp" ]        || fail "release-sha.txt is not a regular file"
[ -r "$stamp" ]        || fail "release-sha.txt is not readable"

# EXACT bytes. The build writes the bare 40-char sha with no trailing newline; anything else --
# a mismatch, a trailing newline, surrounding whitespace -- means the bytes are not what this
# release expects, so it refuses rather than normalising and hoping.
# `$(cat)` strips trailing newlines, which would hide exactly the trailing-newline case this must
# refuse; the `printf x` / `%x` trick preserves every byte.
actual="$(cat "$stamp"; printf x)"; actual="${actual%x}"
if [ "$actual" != "$expected" ]; then
  # Name the likely cause without leaking a long diff.
  if [ "$(printf '%s' "$actual" | tr -d '[:space:]')" = "$expected" ]; then
    fail "release-sha.txt matches the tagged commit only after trimming whitespace; it must be the bare 40-char sha with no trailing newline"
  fi
  fail "release-sha.txt is '${actual}', not the expected '${expected}' -- the bytes are not this commit's build"
fi

echo "OK -- release-sha.txt == $expected, and the artifact has pages and _headers"
