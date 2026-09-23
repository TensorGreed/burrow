#!/usr/bin/env bash
# Adversarial self-test for tools/check-redaction-corpus.py.
#
# WHY IT EXISTS
#
# The checker grew three rules in one commit -- manifest/disk completeness, the
# `no_placements_because` escape hatch, and the inertness sweep over the canary-free control --
# and a code review pointed out that a new check with no negative test is a check nobody has
# shown can fail. One of the three was already wrong when it shipped: a manifest entry naming a
# file that does not exist was reported as a note and exited 0, while the commit message and the
# ADR both said the checker "refuses that in both directions".
#
# THE COPY LIVES BESIDE THE ORIGINAL, per CLAUDE.md. A copy in a temp directory resolves
# `MANIFEST` and `REPO` from its own location, fails for that reason, and an exit-code-only
# assertion reports it as a pass.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
cd "$repo"

checker="$here/check-redaction-corpus.py"
manifest="$repo/tests/redaction/manifest.toml"
backup="$repo/tests/redaction/.manifest-self-test-backup.toml"
planted=""

cleanup() {
  [ -f "$backup" ] && mv -f "$backup" "$manifest"
  [ -n "$planted" ] && rm -f "$planted"
  return 0
}
trap cleanup EXIT

pass=0
fail=0

# Assert the checker refuses, AND names the reason. An exit code alone cannot tell a refusal
# from a crash, and this checker has plenty of ways to crash.
expect_refusal() {
  local name="$1" wanted="$2"
  shift 2
  local out status
  set +e
  out=$("$@" 2>&1)
  status=$?
  set -e
  if [ "$status" -eq 0 ]; then
    echo "  FAIL $name: it did not refuse"
    fail=$((fail + 1))
  elif printf '%s' "$out" | grep -q -- "$wanted"; then
    echo "  ok   $name"
    pass=$((pass + 1))
  else
    echo "  FAIL $name: it refused, but not for the stated reason"
    echo "        wanted: $wanted"
    printf '        %s\n' "$(printf '%s' "$out" | tail -3)"
    fail=$((fail + 1))
  fi
}

echo "check-redaction-corpus self-test:"

# The corpus has to exist before any of this means anything.
bash "$here/check-redaction-corpus.sh" >/dev/null 2>&1

# --- The baseline, so a refusal below is the mutation and not the tree ------------------------
if python3 "$checker" >/dev/null 2>&1; then
  echo "  ok   the real corpus and manifest agree"
  pass=$((pass + 1))
else
  echo "  FAIL the real corpus and manifest agree: it refused before any mutation"
  fail=$((fail + 1))
fi

# --- Rule 1: a fixture on disk that the manifest does not declare ------------------------------
planted="$repo/tests/redaction/generated/zz-undeclared-by-self-test.pdf"
cp "$repo/tests/redaction/generated/01-plain-tj.pdf" "$planted"
[ -f "$planted" ] || { echo "  FAIL the plant did not apply"; exit 1; }
expect_refusal "a fixture on disk with no verdict declared is refused, by name" \
  "zz-undeclared-by-self-test" \
  python3 "$checker"
rm -f "$planted"
planted=""

# --- Rule 2: a manifest entry naming a file that is not there ----------------------------------
#
# The direction that was NOT enforced: it printed `not generated: …` and exited 0, so every
# placement on that entry was skipped in silence under an `OK`.
cp "$manifest" "$backup"
cat >>"$manifest" <<'TOMLEOF'

[[fixture]]
name = "zz-phantom-by-self-test"
file = "generated/zz-phantom-by-self-test.pdf"
kind = "hand-built"
pages = 1

  [[fixture.placement]]
  channel = "a file that does not exist"
  canary = "BURROW-SECRET-PHANTOM"
  verdict = "handle"
  adr = "§1"
  witness_before = "raw-utf16-hex"
  witness_observes = "canary"
  expect_after = "gone"
TOMLEOF
if cmp -s "$backup" "$manifest"; then
  echo "  FAIL the phantom mutation did not apply, so this case measured nothing"
  fail=$((fail + 1))
else
  expect_refusal "a manifest entry with no file on disk is refused, not noted" \
    "zz-phantom-by-self-test" \
    python3 "$checker"
fi
mv -f "$backup" "$manifest"

# --- Rule 3: a fixture that asserts nothing and does not say why -------------------------------
cp "$manifest" "$backup"
python3 - "$manifest" <<'PYEOF'
import re, sys, pathlib
p = pathlib.Path(sys.argv[1])
s = p.read_text()
# Remove the `no_placements_because` block from the canary-free control, leaving it silent.
start = s.index('name = "00-control-no-canary"')
block = s.index('no_placements_because = """', start)
end = s.index('"""', s.index('"""', block) + 3) + 3
p.write_text(s[:block] + s[end:])
PYEOF
if cmp -s "$backup" "$manifest"; then
  echo "  FAIL the silent-fixture mutation did not apply, so this case measured nothing"
  fail=$((fail + 1))
else
  expect_refusal "a fixture with no placement and no stated reason is refused" \
    "no_placements_because" \
    python3 "$checker"
fi
mv -f "$backup" "$manifest"

# --- Rule 4: a witness that matches everything -------------------------------------------------
#
# The inertness sweep. Without it, a witness returning True unconditionally passes every
# placement in the manifest and fails nothing -- a rule that matches everything is not a check.
copy="$here/.redaction-corpus-fixture.py"
sed 's/^def witness_raw(data: bytes, canary: str) -> bool:/def witness_raw(data: bytes, canary: str) -> bool:\n    return True/' \
  "$checker" >"$copy"
if cmp -s "$checker" "$copy"; then
  echo "  FAIL the always-true mutation did not apply, so this case measured nothing"
  fail=$((fail + 1))
else
  expect_refusal "a witness that matches everything is caught by the canary-free control" \
    "canary-free control" \
    python3 "$copy"
fi
rm -f "$copy"

# --- Rule 5: a placement silenced by `no_placements_because` --------------------------------
#
# A security review replaced `01-plain-tj`'s only placement with one sentence: this checker
# exited 0, the shell gate exited 0 and `redaction_corpus.rs` stayed green. The witness census
# dropped a canary and nothing looked at it.
cp "$manifest" "$backup"
python3 - "$manifest" <<'PYEOF'
import sys, pathlib
p = pathlib.Path(sys.argv[1]); s = p.read_text()
start = s.index('file = "generated/01-plain-tj.pdf"')
block = s.index("  [[fixture.placement]]", start)
nxt = s.index("[[fixture]]", block)
p.write_text(s[:block] + 'no_placements_because = "silenced by the self-test"\n\n' + s[nxt:])
PYEOF
if cmp -s "$backup" "$manifest"; then
  echo "  FAIL the silencing mutation did not apply, so this case measured nothing"
  fail=$((fail + 1))
else
  expect_refusal "a placement removed behind a stated reason is refused" \
    "asserting nothing" \
    python3 "$checker"
fi
mv -f "$backup" "$manifest"

# --- Rule 6: a declaration resolving outside the corpus --------------------------------------
cp "$manifest" "$backup"
python3 - "$manifest" <<'PYEOF'
import sys, pathlib
p = pathlib.Path(sys.argv[1]); s = p.read_text()
p.write_text(s.replace('file = "generated/01-plain-tj.pdf"',
                       'file = "generated/../../../../../../tmp/01-plain-tj.pdf"', 1))
PYEOF
if cmp -s "$backup" "$manifest"; then
  echo "  FAIL the escaping-path mutation did not apply, so this case measured nothing"
  fail=$((fail + 1))
else
  expect_refusal "a declared file resolving outside the corpus is refused" \
    "outside the corpus directory" \
    python3 "$checker"
fi
mv -f "$backup" "$manifest"

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED — $fail case(s) failed, $pass passed" >&2
  exit 1
fi
echo "OK — $pass adversarial case(s) all behaved as required"
