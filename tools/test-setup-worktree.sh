#!/usr/bin/env bash
# Self-test for tools/setup-worktree.sh. Each case plants what the script must refuse, or checks
# what it must leave behind, and names the reason it expects.
#
# Needs a main checkout with engines/vendor (it copies it into a throwaway worktree), so it is a
# local test: CI's runner has one checkout and no second tree to set up.
#
# Usage: tools/test-setup-worktree.sh
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/setup-worktree.sh"
common="$(git -C "$here" rev-parse --path-format=absolute --git-common-dir)"
main="$(cd "$common/.." && pwd -P)"
work="$(mktemp -d)"
tree="$work/tree"
cleanup() {
  git -C "$main" worktree remove --force "$tree" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT
pass=0
fail=0
ok() { echo "  ok   $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL $1"; fail=$((fail + 1)); }

# 1. THE MAIN CHECKOUT IS REFUSED, by name: setting it up would overwrite its own engines.
status=0; out="$("$script" "$main" --no-build 2>&1)" || status=$?
if [ "$status" -eq 2 ] && grep -q "is the main checkout" <<<"$out"; then ok "the main checkout is refused"
else bad "the main checkout was not refused by name (status $status): $out"; fi

# FROM THE COMMIT THIS SCRIPT IS ON, not the main checkout's HEAD: the cases below test this
# branch's script and `.gitignore`, and the main checkout may have anything checked out.
git -C "$main" worktree add -q --detach "$tree" "$(git -C "$here" rev-parse HEAD)"

# 2. A DIRECTORY INSIDE A WORKTREE, not its top, is refused.
status=0; out="$("$script" "$tree/tools" --no-build 2>&1)" || status=$?
if [ "$status" -eq 2 ] && grep -q "is not the top of a worktree" <<<"$out"; then ok "a subdirectory is refused"
else bad "a subdirectory was not refused by name (status $status): $out"; fi

# 3. A LINKED vendor tree is replaced by a copy: the link is the hazard this script exists to
# remove. Plant one, then set up.
ln -s "$main/engines/vendor" "$tree/engines/vendor"
status=0; out="$("$script" "$tree" --no-build 2>&1)" || status=$?
if [ "$status" -eq 0 ] && [ -d "$tree/engines/vendor" ] && [ ! -L "$tree/engines/vendor" ] \
   && grep -q "replacing it with a copy" <<<"$out"; then ok "a linked engines/vendor is replaced by a copy"
else bad "the planted link was not replaced (status $status): $out"; fi

# 3b. THE CORPUS LINK IS IGNORED. `corpus/files/` with a trailing slash matched only a directory,
# so the link showed as untracked, one `git add` from being committed.
if [ -L "$tree/corpus/files" ] || [ ! -e "$main/corpus/files" ]; then
  untracked="$(git -C "$tree" status --porcelain --ignored=no -- corpus/files)"
  if [ -z "$untracked" ]; then ok "the corpus link is ignored, not untracked"
  else bad "the corpus link shows in git status: $untracked"; fi
else
  bad "setup did not link corpus/files although the main checkout has one"
fi

# 4. NOTHING WRITTEN IN THE WORKTREE'S ENGINES REACHES THE MAIN CHECKOUT'S.
marker="setup-worktree-test-$$"
touch "$tree/engines/vendor/$marker"
if [ ! -e "$main/engines/vendor/$marker" ]; then ok "a write into the copy does not reach the main checkout"
else rm -f "$main/engines/vendor/$marker"; bad "a write into the worktree's engines appeared in the main checkout's"; fi
rm -f "$tree/engines/vendor/$marker"

# 5. THE COPIED ENGINE STAMP IS CURRENT for an identical tree...
if (cd "$tree" && python3 -B tools/build-stamp.py check engines-wasm >/dev/null 2>&1); then
  ok "the copied engine stamp reads current against an identical tree"
else bad "the copied engine stamp did not read current against an identical tree"; fi

# 6. ...AND REFUSES, naming the input, once this tree's pins differ from the ones it was built from.
echo "# planted by test-setup-worktree.sh" >> "$tree/engines/pins.sh"
if cmp -s "$tree/engines/pins.sh" "$main/engines/pins.sh"; then
  bad "the pins mutation did not apply, so case 6 measured nothing"
else
  status=0; out="$(cd "$tree" && python3 -B tools/build-stamp.py check engines-wasm 2>&1)" || status=$?
  if [ "$status" -ne 0 ] && grep -q "pins.sh" <<<"$out"; then ok "a stamp built from another tree's pins refuses, naming pins.sh"
  else bad "a stamp from another tree's pins was not refused by name (status $status): $out"; fi
fi

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED — $fail case(s) failed, $pass passed" >&2
  exit 1
fi
echo "OK — $pass case(s) of 7 behaved as required"
