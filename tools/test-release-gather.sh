#!/usr/bin/env bash
# Smoke test for the fact-GATHER in `.github/workflows/deploy.yml`'s publish job.
#
# `tools/test-check-release-preconditions.sh` feeds JSON fixtures to the DECISION. That leaves the
# shell that PRODUCES those facts -- the git plumbing in the publish job -- untested, and a review
# found a typo there (`origin/FETCH_HEAD`, not a ref) that made `merged` always false and the whole
# feature non-functional, fail-closed, in nobody's test. This runs the decisive git commands the
# way the workflow runs them, against a throwaway repo, and pins deploy.yml to the fixed spelling.
#
# It needs only git, so it runs in the checker jobs with no vendor tree.

set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
deploy="$repo/.github/workflows/deploy.yml"
pass=0
fail=0

check() {  # check <description> <condition-result 0/1-as-exit>
  if "$@"; then :; fi
}
ok()   { echo "  ok   $1"; pass=$((pass + 1)); }
bad()  { echo "  FAIL $1"; fail=$((fail + 1)); }

echo "release-gather smoke test:"

# --- Part 1: the git commands behave as the gather assumes, on a built repo -------------------
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

origin="$work/origin.git"
clone="$work/clone"
git init -q --bare "$origin"
git init -q "$clone"
cd "$clone"
git config user.email t@t && git config user.name t && git config commit.gpgsign false

# main: C0 -> C1 (C1 is the tip). An off-main commit X branches from C0 and is never merged.
git commit -q --allow-empty -m C0; C0=$(git rev-parse HEAD)
git commit -q --allow-empty -m C1; C1=$(git rev-parse HEAD)
git branch -q -M main
git checkout -q -b feature "$C0"
git commit -q --allow-empty -m X; X=$(git rev-parse HEAD)
git checkout -q main
git remote add origin "$origin"
git push -q origin main

# Annotated tag on an ancestor (C0); lightweight tag on the tip (C1).
git tag -a vA -m "annotated" "$C0"
git tag vL "$C1"

# Reproduce the gather's ref resolution exactly: fetch main, then merge-base against FETCH_HEAD.
git fetch --no-tags -q origin main

merged() { git merge-base --is-ancestor "$1" FETCH_HEAD && echo true || echo false; }
[ "$(merged "$C0")" = true ]  && ok "an ancestor of main's tip is merged" || bad "C0 should be merged (the origin/FETCH_HEAD bug lands here)"
[ "$(merged "$C1")" = true ]  && ok "main's tip itself is merged"          || bad "C1 (tip) should be merged"
[ "$(merged "$X")"  = false ] && ok "an off-main commit is not merged"     || bad "X should NOT be merged"

# Annotated vs lightweight, as the gather reads it.
[ "$(git cat-file -t refs/tags/vA)" = tag ]    && ok "an annotated tag reads as a tag object" || bad "vA should be an annotated tag object"
[ "$(git cat-file -t refs/tags/vL)" = commit ] && ok "a lightweight tag reads as a commit"    || bad "vL should read as a commit (lightweight)"

cd "$repo"

# --- Part 2: deploy.yml uses the fixed spelling, not the broken one ----------------------------
if grep -qE 'merge-base --is-ancestor "\$sha" FETCH_HEAD' "$deploy"; then
  ok "deploy.yml resolves main's tip as FETCH_HEAD"
else
  bad "deploy.yml does not use 'merge-base --is-ancestor \"\$sha\" FETCH_HEAD'"
fi
# The broken spelling as a COMMAND (a prose mention of it in a comment is fine and expected).
if grep -qE 'is-ancestor[^#]*origin/FETCH_HEAD' "$deploy"; then
  bad "deploy.yml still runs merge-base against origin/FETCH_HEAD, which is not a ref (merged always false)"
else
  ok "deploy.yml does not run merge-base against the invalid origin/FETCH_HEAD"
fi

if [ "$fail" -gt 0 ]; then
  echo "FAILED -- $fail case(s) failed, $pass passed"
  exit 1
fi
echo "OK -- $pass gather case(s) all behaved as required"
