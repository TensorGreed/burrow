#!/usr/bin/env bash
# Adversarial self-test for .claude/hooks/refuse-force-push-to-main.py.
#
# A PreToolUse hook that never fires is indistinguishable from one that is not installed,
# and this one fails OPEN by design (see its docstring) -- so a bug in it is silent twice
# over. Hence a table with both halves: every command that must be REFUSED, and every
# near-miss that must be ALLOWED.
#
# The allow half is the one that matters most. A hook that refuses everything would pass a
# refuse-only suite and make the repository unusable, and the four spellings that prompted
# this hook were found by someone reading the list rather than by anything running.
#
# Usage: .claude/hooks/test-refuse-force-push-to-main.py

set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
hook="$here/refuse-force-push-to-main.py"

pass=0
fail=0

run_hook() {
  printf '{"tool_name":"Bash","tool_input":{"command":%s}}' "$(python3 -c '
import json, sys
print(json.dumps(sys.argv[1]))' "$1")" | "$hook" 2>/dev/null
}

refuses() {
  local why="$1" cmd="$2"
  run_hook "$cmd"
  if [ "$?" -eq 2 ]; then
    echo "  ok   REFUSED  $cmd"
    pass=$((pass + 1))
  else
    echo "  FAIL allowed  $cmd  ($why)"
    fail=$((fail + 1))
  fi
}

allows() {
  local why="$1" cmd="$2"
  run_hook "$cmd"
  if [ "$?" -eq 0 ]; then
    echo "  ok   allowed  $cmd"
    pass=$((pass + 1))
  else
    echo "  FAIL REFUSED  $cmd  ($why)"
    fail=$((fail + 1))
  fi
}

echo "refuse-force-push-to-main: the property, not the spelling"
echo
echo "  must refuse --------------------------------------------------------------"

# THE FOUR THE DENY LIST MISSED. Each is named because each is why this hook exists.
refuses "HEAD:main -- the deny list matched only a literal 'main'" \
  "git push --force origin HEAD:main"
refuses "the fully-qualified ref is the same destination" \
  "git push --force origin refs/heads/main"
refuses "a flag AFTER the refspec is the same command" \
  "git push origin main --force"
refuses "a remote that is not origin or upstream is still a remote" \
  "git push --force fork main"

# The spellings the deny list already covered, kept here so the hook is a superset of it
# rather than a different set.
refuses "the bare form pushes the CURRENT branch, which may be main" "git push --force"
refuses "the bare short form" "git push -f"
refuses "a lease is still a force" "git push --force-with-lease"
refuses "a remote with no refspec still pushes the current branch" "git push --force origin"
refuses "the +refspec spelling of --force" "git push origin +main"
refuses "the short flag with an explicit target" "git push -f origin main"
refuses "a lease with an explicit target" "git push --force-with-lease origin main"
refuses "a lease carrying an expected value is still a lease" \
  "git push --force-with-lease=main origin main"
refuses "src:dst writes to dst" "git push --force origin feature:main"
refuses "--set-upstream does not change what is pushed" "git push -u --force origin main"
refuses "git -C elsewhere is still git push" "git -C /tmp/repo push --force origin main"
refuses "a compound command contains a force push" "true && git push --force origin main"
refuses "so does one behind a semicolon" "echo hi; git push -f origin refs/heads/main"

# --- THE SIX A SECURITY REVIEW WALKED PAST, each reproduced ------------------------------
#
# Every one of these was ALLOWED by the first version of this hook. Two of them walked past
# the deny list as well, so the pair documented as "layered rather than duplicated" had both
# layers open on the same input.
refuses "a value-taking global option: `-c` left its VALUE where `push` was expected" \
  "git -c core.pager=cat push --force origin main"
refuses "and --git-dir, and the deny list does not match a command starting `git --git-dir`" \
  "git --git-dir .git push --force origin main"
refuses "and --work-tree" \
  "git --work-tree . push -f origin main"
refuses "and --namespace" \
  "git --namespace ns push --force origin main"
refuses "and --config-env" \
  "git --config-env=user.email=E push --force origin main"
refuses "a NEWLINE is a command separator; multi-line is the ordinary shape, not an exotic one" \
  "echo hi
git push --force origin main"
refuses "a line CONTINUATION is not a separator: splitting on newlines alone loses the flag" \
  "git push \\
  --force origin main"
refuses "git's own DWIM: HEAD:heads/main resolves to refs/heads/main" \
  "git push --force origin HEAD:heads/main"
refuses "an empty source is a DELETION, and carries no force flag to notice" \
  "git push origin :main"
refuses "--mirror force-updates and deletes every ref, main included, naming no refspec" \
  "git push --mirror origin"
refuses "--delete by flag, which the deny list already treated as in scope" \
  "git push --delete origin main"
refuses "an absolute path is still git" \
  "/usr/bin/git push -f origin main"
refuses "HEAD names the CURRENT branch, which may be main -- the bare form's hazard, named" \
  "git push --force origin HEAD"
refuses "and @ is git's own shorthand for HEAD" \
  "git push -f origin @"

echo
echo "  must allow ---------------------------------------------------------------"

# THE NEAR-MISSES. A rule that matches everything fails everything.
allows "a different branch is the normal case this hook must not touch" \
  "git push --force-with-lease origin m1-deploy-origin"
allows "a branch whose name merely starts with main" "git push --force origin main-backup"
allows "and one that contains it" "git push --force origin maintenance"
allows "a non-force push to main is ordinary" "git push origin main"
allows "a plain push" "git push"
allows "a pull is not a push" "git pull --rebase origin main"
allows "another tool entirely" "git status --short"
allows "a word that merely contains the flag" "rg -- --force origin main"
allows "deleting a remote branch is a different rule, and the deny list has it" \
  "git push --delete origin some-branch"
allows "src:dst to a different destination" "git push --force origin main:staging"

# THE WIDER PARSER MUST NOT OVER-REFUSE. It now looks for a `push` TOKEN anywhere after `git`
# rather than skipping options positionally, and it treats deletion as in scope. Both widen
# what is examined, so what must still be allowed is asserted rather than assumed -- a hook
# that refused these would make ordinary work impossible and would pass a refuse-only suite.
allows "a value-taking global on an ordinary branch" \
  "git -c core.pager=cat push --force-with-lease origin feature"
allows "the word push in another command entirely" "echo push --force origin main"
allows "deleting a branch that is not main" "git push origin :some-branch"
allows "a multi-line command with no force push in it" \
  "git add -p
git commit -m wip"
allows "heads/ on another branch" "git push --force origin HEAD:heads/feature"
allows "HEAD with an EXPLICIT destination is somebody's ordinary work" \
  "git push --force origin HEAD:feature"
allows "deleting a branch that is not main, by refspec" "git push origin :feature"

echo
echo "examined ${pass} case(s) plus ${fail} failure(s) across refuse and allow halves"
if [ "$fail" -ne 0 ]; then
  echo "FAILED — $fail case(s) behaved wrongly, $pass correct" >&2
  exit 1
fi
echo "OK — $pass case(s) all behaved as required"
