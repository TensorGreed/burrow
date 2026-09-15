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
#
# ONLY MEANINGFUL WHERE A NATIVE VENDOR TREE EXISTS. Without one there is nothing for the
# wildcard to fall back TO, so the checker's own fixture skips the vendor cases and removing
# the glob changes nothing -- the case would fail for the wrong reason. Skipped loudly rather
# than silently: a skip nobody sees is how a check stops covering something.
if [ -z "$(ls -d "$here/../engines/vendor/native-"* 2>/dev/null)" ]; then
  echo "  SKIP removing the architecture wildcard is caught"
  echo "       no engines/vendor/native-* tree here; this case runs in CI's \`test\` job"
else
breaks_it "removing the architecture wildcard is caught" \
  '    for prefix in sorted((REPO / "engines" / "vendor").glob("native-\*")):' \
  '    for prefix in []:' \
  "does not resolve a vendor path naming an architecture"
fi

# A resolver that invents paths would compare a committed text against the wrong original.
breaks_it "a resolver that returns a nonexistent path is caught" \
  '        return direct if direct.is_file() else None' \
  '        return direct' \
  "invented a path"

# --- the artifact-id cross-check, probed by PLANTING IN A COPY OF THE MANIFEST ---------------
#
# Three rules, three mistakes, and each is probed against its own defect rather than by
# mutating this script: the checker takes an optional manifest path precisely so the fixture is
# a copy and the committed `engines/licenses.toml` is never edited by a test.
#
# WHY THESE RULES EXIST. The checker did not read `artifacts` or `linked_in` at all until this
# change. Spike 0004 called removing PDFium from the web "bounded work" because
# "check-engine-licences.py enforces the consistency" -- it did not, and the bounding came from
# `grep` over twenty-five lines across fourteen components.
manifest="$here/../engines/licenses.toml"
planted="$here/.engine-licences-manifest-fixture.toml"

plants_it() {
  local name="$1" python_edit="$2" expect="$3"
  MANIFEST_SRC="$manifest" PLANTED="$planted" python3 -c "$python_edit" || {
    echo "  FAIL $name: the plant did not apply, so this case would prove nothing"
    fail=$((fail + 1))
    rm -f "$planted"
    return
  }
  if cmp -s "$planted" "$manifest"; then
    echo "  FAIL $name: the plant produced an identical file"
    fail=$((fail + 1))
    rm -f "$planted"
    return
  fi
  local got
  if got="$(python3 "$tool" "$planted" 2>&1)"; then
    echo "  FAIL $name: the checker accepted it"
    fail=$((fail + 1))
  elif grep -qF "$expect" <<<"$got"; then
    echo "  ok   $name"
    pass=$((pass + 1))
  else
    echo "  FAIL $name: it refused, but not for the stated reason"
    echo "        wanted: $expect"
    sed 's/^/        /' <<<"$got" | tail -3
    fail=$((fail + 1))
  fi
  rm -f "$planted"
}

# THE BASELINE. Without it the three cases below could all be passing against a checker that
# refuses every manifest it is handed, which is the same non-check as one that refuses none.
cp "$manifest" "$planted"
if python3 "$tool" "$planted" >/dev/null 2>&1; then
  echo "  ok   an unmodified copy of the manifest passes, so a refusal below means something"
  pass=$((pass + 1))
else
  echo "  FAIL an unmodified copy of the manifest does not pass; no case below means anything"
  python3 "$tool" "$planted" 2>&1 | tail -3 | sed 's/^/        /'
  fail=$((fail + 1))
fi
rm -f "$planted"

plants_it "a component naming an artifact nothing declares is caught" '
import os, re
src = open(os.environ["MANIFEST_SRC"]).read()
old = re.search(r"^artifacts = \[[^\]]*\]", src, re.M)
assert old, "no `artifacts = [...]` line to plant into"
new = old.group(0)[:-1] + ", \"no-such-artifact\"]"
open(os.environ["PLANTED"], "w").write(src.replace(old.group(0), new, 1))
' "but no [[artifact]] block declares it"

plants_it "an artifact nothing references is caught" '
import os
src = open(os.environ["MANIFEST_SRC"]).read()
assert "[[artifact]]" in src
open(os.environ["PLANTED"], "w").write(
    src + "\n[[artifact]]\nid = \"orphan-artifact\"\npath = \"engines/vendor/none\"\n"
)
' "is declared but no component names it"

plants_it "a linked_in that is not in that component's artifacts is caught" '
import os, re
Q = chr(34)  # a bare double quote; this source is a shell single-quoted argument
src = open(os.environ["MANIFEST_SRC"]).read()
declared = re.findall("^id = " + Q + "([^" + Q + "]+)" + Q, src, re.M)
assert declared, "no [[artifact]] ids to choose a stray from"

# A COMPONENT BLOCK, not a lone line. The stray id has to be one this component does not list
# in `artifacts`, and it has to be an id that EXISTS -- otherwise the dangling rule catches it
# and the case measures the wrong rule.
blocks = src.split("[[component]]")
for index, block in enumerate(blocks[1:], start=1):
    linked = re.search(r"^linked_in = (\[[^\]]*\])", block, re.M)
    arts = re.search(r"^artifacts = (\[[^\]]*\])", block, re.M)
    if not linked or not arts:
        continue
    stray = next((i for i in declared if Q + i + Q not in arts.group(1)), None)
    if stray is None:
        continue
    inner = linked.group(1)[1:-1].strip()
    prefix = inner + ", " if inner else ""
    blocks[index] = block.replace(
        linked.group(0), "linked_in = [" + prefix + Q + stray + Q + "]", 1
    )
    break
else:
    raise AssertionError("no component with both artifacts and linked_in to plant into")
open(os.environ["PLANTED"], "w").write("[[component]]".join(blocks))
' "without listing it in artifacts"

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED — $fail case(s) failed, $pass passed" >&2
  exit 1
fi
echo "OK — $pass adversarial case(s) all behaved as required"
