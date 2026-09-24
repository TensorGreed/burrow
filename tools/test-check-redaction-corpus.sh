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

# --- Rule 7: the placement COUNT half of the floor ------------------------------------------
#
# Case 5 above removes a fixture's only placement, which trips `len(silent_fixtures)` -- so the
# count half was never reached. A code review deleted the count comparison outright and all
# seven cases still printed ok, then dropped one of `producer-writer`'s FOUR placements (leaving
# three, so the fixture is not silent) and the checker exited 0.
cp "$manifest" "$backup"
python3 - "$manifest" <<'PYEOF'
import sys, pathlib
p = pathlib.Path(sys.argv[1]); s = p.read_text()
start = s.index('file = "fixtures/producer-writer.pdf"')
block = s.index("  [[fixture.placement]]", start)
nxt = s.index("  [[fixture.placement]]", block + 10)
p.write_text(s[:block] + s[nxt:])
PYEOF
if cmp -s "$backup" "$manifest"; then
  echo "  FAIL the placement-count mutation did not apply, so this case measured nothing"
  fail=$((fail + 1))
else
  expect_refusal "one placement removed from a fixture that keeps others is refused" \
    "placement(s)" \
    python3 "$checker"
fi
mv -f "$backup" "$manifest"

# --- Rule 8: completeness compares FULL PATHS, not basenames ---------------------------------
#
# Case 6 passes on the separate "resolves outside the corpus" rule, so the basename -> full-path
# hardening had no case of its own. A declaration naming a real basename under the WRONG
# directory satisfies a basename comparison and must not satisfy this one.
cp "$manifest" "$backup"
python3 - "$manifest" <<'PYEOF'
import sys, pathlib
p = pathlib.Path(sys.argv[1]); s = p.read_text()
p.write_text(s.replace('file = "generated/01-plain-tj.pdf"', 'file = "fixtures/01-plain-tj.pdf"', 1))
PYEOF
if cmp -s "$backup" "$manifest"; then
  echo "  FAIL the wrong-directory mutation did not apply, so this case measured nothing"
  fail=$((fail + 1))
else
  expect_refusal "a declaration naming the right basename under the wrong directory is refused" \
    "generated/01-plain-tj.pdf" \
    python3 "$checker"
fi
mv -f "$backup" "$manifest"

# --- The "after" half (#176): one planted case per rule, each run against ONE fixture --------
#
# Each case edits one fixture's block of the manifest, runs `--after` filtered to that fixture,
# and requires a refusal naming the rule. `after_case <name> <fixture> <old> <new> <wanted>`;
# the edit is asserted to have applied, inside that fixture's block and nowhere else.
#
# An empty <old> APPENDS <new> to the end of the block -- a new placement, which has no anchor.
plant() {
  local fixture="$1" old="$2" new="$3"
  python3 - "$manifest" "$fixture" "$old" "$new" <<'PYEOF'
import pathlib, sys
path, fixture, old, new = pathlib.Path(sys.argv[1]), *sys.argv[2:5]
text = path.read_text()
start = text.index(f'name = "{fixture}"\n')
end = text.find("\n[[fixture]]", start)
# THE LAST FIXTURE HAS NO SUCCESSOR, and `find` returns -1: slicing to it dropped the file's final
# character, which the first `after_case_before` did (#176's review).
end = len(text) if end < 0 else end
block = text[start:end]
if old == "":
    block = block.rstrip("\n") + "\n\n" + new + "\n"
else:
    assert block.count(old) == 1, f"{old!r} occurs {block.count(old)} times in {fixture}'s block"
    block = block.replace(old, new, 1)
updated = text[:start] + block + text[end:]
assert updated != text, "the edit changed nothing"
path.write_text(updated)
PYEOF
}

after_case() {
  local name="$1" fixture="$2" old="$3" new="$4" wanted="$5"
  cp "$manifest" "$backup"
  if ! plant "$fixture" "$old" "$new"; then
    echo "  FAIL $name: the mutation did not apply, so this case measured nothing"
    fail=$((fail + 1))
    mv -f "$backup" "$manifest"
    return
  fi
  BURROW_AFTER_ONLY="$fixture" expect_refusal "$name" "$wanted" python3 "$checker" --after
  mv -f "$backup" "$manifest"
}

# The baseline for every case below: the real manifest's after-half holds.
if BURROW_AFTER_ONLY="evade-image-in-form,plain-tj,cid-without-tounicode,17-incremental-update" python3 "$checker" --after >/dev/null 2>&1; then
  echo "  ok   the after-half holds on the real manifest for the fixtures the cases plant into"
  pass=$((pass + 1))
else
  echo "  FAIL the after-half fails before any mutation"
  fail=$((fail + 1))
fi

after_case "an owed refusal with its marker removed is refused: redacted where the manifest says refused" \
  evade-image-in-form $'  owed_by = 125' '' "redacted where the manifest says refused"
after_case "an owed marker on a document that refuses now is stale" \
  evade-oc-outside-the-region $'expect_after = "refused"' $'expect_after = "refused"\n  owed_by = 125' \
  "outlived the work it waited for"
after_case "a refusal where the manifest expects a redaction is refused" \
  type3-glyph $'  owed_by = 131' '' "where the manifest expects it redacted"
after_case "an owed handling that is handled now is stale" \
  plain-tj $'expect_after = "gone"' $'expect_after = "gone"\n  owed_by = 131' "drop the marker"
after_case "an owed marker never excuses a canary still witnessed after redaction" \
  cid-without-tounicode $'expect_after = "present"' $'expect_after = "gone"\n  owed_by = 131' \
  "still finds the canary after redaction"
after_case "a gone placement whose canary survives is refused" \
  cid-without-tounicode $'expect_after = "present"' $'expect_after = "gone"' \
  "still finds the canary after redaction"
after_case "a disclosed placement that is no longer found is refused" \
  plain-tj $'expect_after = "gone"' $'expect_after = "present"' "manifest says is disclosed"
after_case "a producer fixture with no region is refused" \
  producer-writer $'region = [174, 96, 154, 13]' '' "must declare \`region\`"
after_case "a document that must refuse and also promises a removal contradicts itself" \
  evade-oc-outside-the-region $'  [[fixture.placement]]' \
  $'  [[fixture.placement]]\n  channel = "planted"\n  canary = "BURROW-EVADE-OC-ELSEWHERE"\n  verdict = "handle"\n  adr = "§1"\n  witness_before = "raw-file"\n  witness_observes = "canary"\n  expect_after = "gone"\n\n  [[fixture.placement]]' \
  "contradicts itself"
# THE RAW-FILE BRANCH, which no case reached: it reads the output as written AND expanded, and a
# kept line is re-emitted as ASCII hex, so this also needs the hex spelling to see it.
after_case "a raw-file placement whose canary survives is refused" \
  17-incremental-update '' \
  $'  [[fixture.placement]]\n  channel = "planted: the kept line"\n  canary = "KEEP-THIS-LINE"\n  verdict = "handle"\n  adr = "§1"\n  witness_before = "raw-file"\n  witness_observes = "canary"\n  expect_after = "gone"' \
  "still finds the canary after redaction"
# PARTIAL REMOVAL IS NOT GONE: a region clearing only the start of the canary left the rest, and
# the whole-string witness read that as removed.
after_case "a region that removes only part of the canary is refused" \
  plain-tj $'kind = "hand-built"' $'kind = "hand-built"\nregion = [30, 68, 100, 44]' \
  "part of the canary survives"
# AND THE WITNESS THAT MAKES 06 JUDGEABLE: without it, the font's alphabet -- kept by design --
# reads as the canary still there.
# AND ON 06, where the first fragment rule could see nothing: PDFium's text and the bytes are glyph
# ids there, so a region that left `ECRET-06` drawn scored gone (both reviews of round two).
after_case "a region that removes only part of channel 06's canary is refused" \
  cid-without-tounicode $'kind = "hand-built"' $'kind = "hand-built"\nregion = [30, 68, 100, 44]' \
  "part of the canary survives"
after_case "channel 06 without its cid-codes witness cannot be judged gone" \
  cid-without-tounicode $'  witness_after = "cid-codes"\n' '' \
  "still finds the canary after redaction"

# And the field validation, on the ordinary run.
after_case_before() {
  local name="$1" fixture="$2" old="$3" new="$4" wanted="$5"
  cp "$manifest" "$backup"
  if ! plant "$fixture" "$old" "$new"; then
    echo "  FAIL $name: the mutation did not apply, so this case measured nothing"
    fail=$((fail + 1))
    mv -f "$backup" "$manifest"
    return
  fi
  expect_refusal "$name" "$wanted" python3 "$checker"
  mv -f "$backup" "$manifest"
}
after_case_before "an expect_after outside gone, refused and present is refused" \
  plain-tj $'expect_after = "gone"' $'expect_after = "vanished"' "must be gone, refused or present"
after_case_before "an owed_by that is not an issue number is refused" \
  evade-image-in-form $'owed_by = 125' $'owed_by = "soon"' "must name one of the issues"
after_case_before "an owed marker added is a change to the pinned count" \
  evade-oc-outside-the-region $'expect_after = "refused"' $'expect_after = "refused"\n  owed_by = 125' \
  "owed markers per issue are"
after_case_before "after_unwitnessed is no longer an exemption" \
  plain-tj $'expect_after = "gone"' $'expect_after = "gone"\n  after_unwitnessed = "planted"' \
  "is not a field"
after_case_before "a witness_after that cannot see the canary before the run is refused" \
  plain-tj $'expect_after = "gone"' $'expect_after = "gone"\n  witness_after = "cid-codes"' \
  "BEFORE the run"

# AND THE LEAK PIN, which only a full run can reach: a copy of the checker, beside the original,
# whose pinned set has lost one name -- so a real leak reads as a new one.
copy="$here/.redaction-corpus-leaks.py"
python3 - "$checker" "$copy" <<'PYEOF'
import sys
text = open(sys.argv[1]).read()
line = '    "acroform-field",\n'
assert text.count(line) == 1, "the pinned leak set is not where this expects it"
open(sys.argv[2], "w").write(text.replace(line, "", 1))
PYEOF
if cmp -s "$checker" "$copy"; then
  echo "  FAIL the leak-count mutation did not apply, so this case measured nothing"
  fail=$((fail + 1))
else
  expect_refusal "a change in how many owed placements leak is refused" \
    "newly disclosing \['acroform-field'\]" \
    env -u BURROW_AFTER_ONLY python3 "$copy" --after
fi
rm -f "$copy"

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED — $fail case(s) failed, $pass passed" >&2
  exit 1
fi
echo "OK — $pass adversarial case(s) all behaved as required"
