#!/usr/bin/env bash
# Adversarial self-test for tools/check-engine-licences.py's parser fixtures.
#
# The checker verifies `split_expression` and `resolve_audited_original` against fixtures on
# every run. Both gates are only observable when a parser is wrong, so they are exercised
# here by breaking one in a COPY and asserting the copy refuses, NAMING the reason.
#
# Two details, both learned the hard way (CLAUDE.md): the copy sits beside the original,
# because the tool resolves REPO from its own location and a copy in /tmp fails with "not
# found" -- which an exit-code-only assertion reports as a pass; and every fixture asserts
# the mutation applied, because a sed that matched nothing looks exactly like a working gate.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
tool="$here/check-engine-licences.py"

pass=0
fail=0

breaks_it() {
  local name="$1" find="$2" replace="$3" expect="$4"
  local copy="$here/.engine-licences-fixture.py"
  sed "s|$find|$replace|" "$tool" >"$copy"
  if cmp -s "$copy" "$tool"; then
    echo "  FAIL $name: the mutation did not apply, so this case would prove nothing"
    fail=$((fail + 1))
    rm -f "$copy"
    return
  fi
  local got
  if got="$(python3 "$copy" 2>&1)"; then
    echo "  FAIL $name: the tool ran anyway"
    fail=$((fail + 1))
  elif grep -qF "$expect" <<<"$got"; then
    echo "  ok   $name"
    pass=$((pass + 1))
  else
    echo "  FAIL $name: it failed, but not for the stated reason"
    sed 's/^/        /' <<<"$got" | head -4
    fail=$((fail + 1))
  fi
  rm -f "$copy"
  rm -rf "$here/__pycache__"
}

echo "check-engine-licences.py: the parser-fixture gate must exist"

# A split that returns the whole expression as one token sends an unsplit string to the
# allowlist check. Loud, but it means no constituent was checked.
breaks_it "a split that collapses to one token is caught" \
  '    parts = expr.replace("(", " ").replace(")", " ").split()' \
  '    parts = [expr]' \
  "split_expression"

# THE DANGEROUS DIRECTION: a split that silently drops a constituent passes a licence nobody
# compared against ADR 0008's allowlist.
breaks_it "a split that drops a constituent is caught" \
  '        if tok in {"AND", "OR"}:' \
  '        if tok in {"AND", "OR"} and len(out) < 1:' \
  "split_expression"

# The architecture wildcard. Removing it is what made the licence-text drift comparison run
# on 4 of 15 components in CI while printing identical output.
breaks_it "removing the architecture wildcard is caught" \
  '    for prefix in sorted((REPO / "engines" / "vendor").glob("native-\*")):' \
  '    for prefix in []:' \
  "does not resolve a vendor path naming an architecture"

# A resolver that invents paths would compare a committed text against the wrong original.
breaks_it "a resolver that returns a nonexistent path is caught" \
  '        return direct if direct.is_file() else None' \
  '        return direct' \
  "invented a path"

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED — $fail case(s) failed, $pass passed" >&2
  exit 1
fi
echo "OK — $pass adversarial case(s) all behaved as required"
