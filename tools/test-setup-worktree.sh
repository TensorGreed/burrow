#!/usr/bin/env bash
# Self-test for tools/setup-worktree.sh. Each case plants what the script must refuse, or checks
# what it must leave behind, and names the reason it expects.
#
# AGAINST A THROWAWAY "MAIN CHECKOUT", never the real one: a local clone of this repository, given
# a copy of the real engines with any link out of them removed. So the cases do not depend on the
# real main checkout's state (on 2026-10-15 its engines held a stray absolute link, which the
# script now refuses -- correctly, and it would fail every case below), and nothing here can write
# into the real one.
#
# Needs a main checkout with engines/vendor to copy from, so it is a LOCAL test, not a CI or
# `ci-local` job: CI's runner has one checkout, a fetch-only engines tree in some jobs and none in
# others, and this copies about 900 MB per run.
#
# Usage: tools/test-setup-worktree.sh
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
script="$here/setup-worktree.sh"
common="$(git -C "$here" rev-parse --path-format=absolute --git-common-dir)"
real="$(cd "$common/.." && pwd -P)"
work="$(mktemp -d)"
fake="$work/main"
tree="$work/tree"
cleanup() {
  git -C "$fake" worktree remove --force "$tree" >/dev/null 2>&1 || true
  rm -rf "$work"
}
trap cleanup EXIT
expected=11
pass=0
fail=0
ok() { echo "  ok   $1"; pass=$((pass + 1)); }
bad() { echo "  FAIL $1"; fail=$((fail + 1)); }

# THE FAKE MAIN CHECKOUT, at the commit this script is on.
git clone -q "$real" "$fake"
git -C "$fake" checkout -q --detach "$(git -C "$here" rev-parse HEAD)"
cp -a "$real/engines/vendor" "$fake/engines/vendor"
find "$fake/engines/vendor" -type l | while IFS= read -r link; do
  case "$(readlink -f "$link")" in "$fake/engines/vendor" | "$fake/engines/vendor"/*) ;; *) rm "$link" ;; esac
done
mkdir -p "$fake/corpus/files"
printf 'corpus\n' > "$fake/corpus/files/one.pdf"

# 1. THE MAIN CHECKOUT IS REFUSED, by name: setting it up would overwrite its own engines.
status=0; out="$("$script" "$fake" --no-build 2>&1)" || status=$?
if [ "$status" -eq 2 ] && grep -q "is the main checkout" <<<"$out"; then ok "the main checkout is refused"
else bad "the main checkout was not refused by name (status $status): $out"; fi

git -C "$fake" worktree add -q --detach "$tree" HEAD

# 2. A DIRECTORY INSIDE A WORKTREE, not its top, is refused.
status=0; out="$("$script" "$tree/tools" --no-build 2>&1)" || status=$?
if [ "$status" -eq 2 ] && grep -q "is not the top of a worktree" <<<"$out"; then ok "a subdirectory is refused"
else bad "a subdirectory was not refused by name (status $status): $out"; fi

# 3. A LINKED vendor tree is replaced by a copy: the link is the hazard this script exists to remove.
ln -s "$fake/engines/vendor" "$tree/engines/vendor"
status=0; out="$("$script" "$tree" --no-build 2>&1)" || status=$?
if [ "$status" -eq 0 ] && [ -d "$tree/engines/vendor" ] && [ ! -L "$tree/engines/vendor" ] \
   && grep -q "replacing it with a copy" <<<"$out" && grep -q "no inode shared" <<<"$out"; then
  ok "a linked engines/vendor is replaced by a verified copy"
else bad "the planted link was not replaced (status $status): $out"; fi

# 4. THE CORPUS LINK IS IGNORED. `corpus/files/` with a trailing slash matched only a directory.
untracked="$(git -C "$tree" status --porcelain --ignored=no -- corpus/files)"
if [ -L "$tree/corpus/files" ] && [ -z "$untracked" ]; then ok "the corpus link is made, and ignored"
else bad "the corpus link is missing or shows in git status: '$untracked'"; fi

# 5. A COPIED FILE REWRITTEN IN PLACE LEAVES THE MAIN CHECKOUT'S UNCHANGED -- which a hardlink farm
# would not: the rewrite goes through the shared inode.
# `-print -quit`, not `| head -1`: under pipefail the SIGPIPE `find` takes from `head` ends the run.
victim="$(find "$tree/engines/vendor" -type f -name '*.toml' -size -64k -print -quit)"
[ -n "$victim" ] || victim="$(find "$tree/engines/vendor" -type f -size -64k -print -quit)"
original="$fake/engines/vendor/${victim#"$tree/engines/vendor/"}"
before="$(sha256sum "$original" | cut -d' ' -f1)"
printf 'rewritten in place by test-setup-worktree.sh\n' 1<>"$victim"
if cmp -s "$victim" "$original"; then
  bad "the rewrite did not apply to the copy, so case 5 measured nothing"
elif [ "$(sha256sum "$original" | cut -d' ' -f1)" = "$before" ]; then
  ok "a file rewritten in place in the copy leaves the main checkout's unchanged"
else bad "rewriting the copy's file changed the main checkout's"; fi
cp -a "$original" "$victim"

# 6. NO REGULAR FILE SHARES AN INODE WITH THE MAIN CHECKOUT'S, read directly with `stat`.
shared="$(comm -12 <(find "$tree/engines/vendor" -type f -exec stat -c '%d:%i' {} + | sort -u) \
                   <(find "$fake/engines/vendor" -type f -exec stat -c '%d:%i' {} + | sort -u) | wc -l)"
if [ "$shared" -eq 0 ]; then ok "no file in the copy shares an inode with the main checkout"
else bad "$shared file(s) in the copy share an inode with the main checkout"; fi

# 7. A HARDLINK FARM (`cp -al`) IS REFUSED, by name.
rm -rf "$tree/engines/vendor"; cp -al "$fake/engines/vendor" "$tree/engines/vendor"
status=0; out="$("$script" "$tree" --no-build 2>&1)" || status=$?
if [ "$status" -eq 2 ] && grep -q "share an inode" <<<"$out"; then ok "a hardlink farm is refused"
else bad "a cp -al hardlink farm was not refused (status $status): $out"; fi
rm -rf "$tree/engines/vendor"; "$script" "$tree" --no-build >/dev/null

# 8. A SYMLINK OUT OF THE COPY IS REFUSED, naming it -- the shape `cp -a` carried over from the real
# main checkout on 2026-10-15.
ln -s "$fake/engines/vendor" "$tree/engines/vendor/vendor"
status=0; out="$("$script" "$tree" --no-build 2>&1)" || status=$?
if [ "$status" -eq 2 ] && grep -q "resolve OUTSIDE the copy" <<<"$out" && grep -q "engines/vendor/vendor ->" <<<"$out"; then
  ok "a symlink out of the copy is refused, naming it"
else bad "a symlink out of the copy was not refused by name (status $status): $out"; fi
rm "$tree/engines/vendor/vendor"

# 9. A DANGLING CORPUS LINK IS NAMED, not followed into an error nobody can read.
rm "$tree/corpus/files"; ln -s "$work/nowhere" "$tree/corpus/files"
status=0; out="$("$script" "$tree" --no-build 2>&1)" || status=$?
if [ "$status" -eq 2 ] && grep -q "which does not exist" <<<"$out"; then ok "a dangling corpus link is refused by name"
else bad "a dangling corpus link was not refused by name (status $status): $out"; fi
rm "$tree/corpus/files"; "$script" "$tree" --no-build >/dev/null

# 10. THE COPIED ENGINE STAMP IS CURRENT for an identical tree...
if (cd "$tree" && python3 -B tools/build-stamp.py check engines-wasm >/dev/null 2>&1); then
  ok "the copied engine stamp reads current against an identical tree"
else bad "the copied engine stamp did not read current against an identical tree"; fi

# 11. ...AND REFUSES, naming the input, once this tree's pins differ from the ones it was built from.
echo "# planted by test-setup-worktree.sh" >> "$tree/engines/pins.sh"
if cmp -s "$tree/engines/pins.sh" "$fake/engines/pins.sh"; then
  bad "the pins mutation did not apply, so case 11 measured nothing"
else
  status=0; out="$(cd "$tree" && python3 -B tools/build-stamp.py check engines-wasm 2>&1)" || status=$?
  if [ "$status" -ne 0 ] && grep -q "pins.sh" <<<"$out"; then ok "a stamp built from another tree's pins refuses, naming pins.sh"
  else bad "a stamp from another tree's pins was not refused by name (status $status): $out"; fi
fi

echo
if [ "$fail" -ne 0 ] || [ "$pass" -ne "$expected" ]; then
  echo "FAILED — $pass of $expected case(s) passed, $fail failed" >&2
  exit 1
fi
echo "OK — $pass case(s) of $expected behaved as required"
