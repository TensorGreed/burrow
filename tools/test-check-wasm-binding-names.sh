#!/usr/bin/env bash
# Adversarial self-test for tools/check-wasm-binding-names.py.
#
# The checker compares hand-written worker declarations against the generated binding. This
# plants the failure it exists for -- and the two ways it could stop being a check at all --
# in a COPY beside the original, and requires a refusal naming the reason.
#
# THE COPY LIVES BESIDE THE ORIGINAL, not in a temp directory: the checker resolves its paths
# from `__file__`, so a copy anywhere else exits non-zero for the wrong reason and an
# exit-code-only assertion reports that as a pass.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
original="$here/check-wasm-binding-names.py"
copy="$here/.check-wasm-binding-names.selftest.py"
declared="$repo/apps/web/src/worker/globals-render.d.ts"
dbackup="$(mktemp)"

pass=0
fail=0

cleanup() {
  rm -f "$copy"
  [ -f "$dbackup" ] && cp "$dbackup" "$declared" && rm -f "$dbackup"
}
trap cleanup EXIT

cp "$declared" "$dbackup"

# --- The baseline. Without it every case below is measured against a broken checker. --------
if out="$(python3 "$original" 2>&1)"; then
  echo "  ok   the checker itself passes on the real tree"
  pass=$((pass + 1))
else
  echo "  FAIL the checker does not pass unmutated, so no case below means anything"
  sed 's/^/        /' <<<"$out" | tail -5
  exit 1
fi

# --- Case 1: THE DEFECT ITSELF, replanted. --------------------------------------------------
#
# A member the worker declares and the binding does not expose. This is exactly what
# `pixelLength` was: reading it is `undefined`, `undefined > 0` is false, and the strip posts
# zero bytes for every page while reporting success.
python3 - "$declared" <<'PYEOF'
import pathlib, sys
path = pathlib.Path(sys.argv[1])
text = path.read_text()
old = "  readonly pixelLength: number;"
assert old in text, "globals-render.d.ts no longer declares pixelLength"
path.write_text(text.replace(old, "  readonly pixel_length: number;", 1))
PYEOF
if cmp -s "$declared" "$dbackup"; then
  echo "  FAIL the snake_case mutation did not apply, so this case measured nothing"
  fail=$((fail + 1))
else
  status=0
  out="$(python3 "$original" 2>&1)" || status=$?
  if [ "$status" -ne 0 ] && grep -q "pixel_length" <<<"$out" \
     && grep -q "which the binding does not expose" <<<"$out"; then
    echo "  ok   a declared member the binding does not expose is refused, by name"
    pass=$((pass + 1))
  else
    echo "  FAIL a snake_case declaration was not refused (status $status)"
    sed 's/^/        /' <<<"$out" | tail -4
    fail=$((fail + 1))
  fi
fi
cp "$dbackup" "$declared"

# --- Case 2: a checker that compares nothing ------------------------------------------------
#
# The shape the root CLAUDE.md names: a check that silently examines nothing reads as coverage.
# With no interface matched the checker must refuse rather than print OK over zero comparisons.
sed 's/^PAIRS = \[/PAIRS = [] or [/' "$original" > "$copy"
python3 - "$copy" <<'PYEOF'
import pathlib, sys
path = pathlib.Path(sys.argv[1])
text = path.read_text()
old = 'return {m.group(1) for line in body.splitlines() if (m := MEMBER.match(line))}'
assert old in text, "the member extractor has moved"
path.write_text(text.replace(old, "return None", 1))
PYEOF
status=0
out="$(python3 "$copy" 2>&1)" || status=$?
if [ "$status" -ne 0 ] && grep -q "examined nothing" <<<"$out"; then
  echo "  ok   a checker that compares no interface refuses, naming the reason"
  pass=$((pass + 1))
else
  echo "  FAIL a checker comparing nothing was not refused (status $status)"
  sed 's/^/        /' <<<"$out" | tail -4
  fail=$((fail + 1))
fi
rm -f "$copy"

# --- Case 3: a comparison that is symmetric ---------------------------------------------------
#
# The opposite failure, and not the same bug. This rule is deliberately ONE-WAY: a name the
# worker declares and the binding lacks is the silent-undefined defect and must fail; a name the
# binding has and the worker ignores is the worker choosing not to use part of its own binding,
# which is normal -- `WebLimits` has six such members today.
#
# A symmetric comparison fails on a correct tree, and a checker that fails on a correct tree is
# one somebody switches off. That is the same outcome as one that matches nothing, reached the
# other way, which is why both probes are here.
python3 - "$original" "$copy" <<'PYEOF'
import pathlib, sys
src, dst = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
text = src.read_text()
old = "            missing = sorted(hand - real)"
assert old in text, "the one-way comparison has moved"
dst.write_text(text.replace(old, "            missing = sorted(hand ^ real)", 1))
PYEOF
if cmp -s "$copy" "$original"; then
  echo "  FAIL the symmetry mutation did not apply, so this case measured nothing"
  fail=$((fail + 1))
else
  status=0
  out="$(python3 "$copy" 2>&1)" || status=$?
  if [ "$status" -ne 0 ] && grep -q "max_input_bytes" <<<"$out"; then
    echo "  ok   a symmetric comparison refuses on the real tree, so the rule stays one-way"
    pass=$((pass + 1))
  else
    echo "  FAIL a symmetric comparison still reported OK (status $status)"
    sed 's/^/        /' <<<"$out" | tail -4
    fail=$((fail + 1))
  fi
fi
rm -f "$copy"

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED — $fail case(s) failed, $pass passed" >&2
  exit 1
fi
echo "OK — $pass adversarial case(s) all behaved as required"
