#!/usr/bin/env bash
# Adversarial self-test for tools/check-redaction-not-in-base.sh.
#
# That checker is made of ABSENCE rules, and until #191 builds a redaction module no shipped
# artifact contains a needle. So its positives come from here, in two kinds:
#
#   REAL. The base Rust module is BUILT with `--cfg burrow_redaction_probe`, which compiles one
#   export (`__burrow_redaction_probe` in bindings/burrow-wasm) reaching five of redaction's
#   components. The checker must refuse it. This tests the premise the whole check rests on --
#   that code reachable from an export keeps its literals through the real compiler and real LTO --
#   which appending bytes to a file cannot. Which needles it covers is pinned in REAL below, so a
#   component that stops leaving its literal in the module fails here by name.
#
#   PLANTED. Every needle, including the three whose code is native-only until #191, is written
#   into a copy of each shipped Rust module in turn, and the checker must refuse NAMING it.
#   Near-misses sit beside them, so a rule that matched everything fails here too.
#
# The fixtures are copies of `apps/web/dist`, never the build itself. A harness build is accepted
# as the SOURCE of a copy -- the Rust modules are the same in both -- and each copy has its harness
# routes removed so the checker's production precondition holds; one case keeps them.
#
# Usage: tools/test-check-redaction-not-in-base.sh

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
checker="$here/check-redaction-not-in-base.sh"
real="$repo/apps/web/dist"
probe_target="$repo/target/redaction-probe"
workroot=""
copy=""

cleanup() {
  trap - EXIT INT TERM HUP
  [ -n "${workroot:-}" ] && rm -rf "$workroot"
  [ -n "${copy:-}" ] && rm -f "$copy"
  return 0
}
trap cleanup EXIT INT TERM HUP

pass=0
fail=0

if [ ! -d "$real" ]; then
  echo "  SKIP-REFUSED: no build at apps/web/dist. This suite needs one to copy; run" >&2
  echo "                \`pnpm build\` in apps/web. Refusing rather than passing vacuously." >&2
  exit 1
fi

workroot="$(mktemp -d)"

# A fresh production-shaped copy of the real build, at $work, with $base and $render its modules.
fresh() {
  work="$workroot/dist"
  rm -rf "$work"
  cp -r "$real" "$work"
  rm -rf "$work/harness" "$work/host"
  base="$(find "$work/engines" -maxdepth 1 -name 'burrow_wasm_bg.*.wasm' | head -1)"
  render="$(find "$work/engines" -maxdepth 1 -name 'burrow_wasm_render_bg.*.wasm' | head -1)"
}

expect_refusal() {
  local name="$1" wanted="$2"
  shift 2
  local out status
  set +e
  out="$("$@" 2>&1)"
  status=$?
  set -e
  if [ "$status" -eq 0 ]; then
    echo "  FAIL $name: it did not refuse"
    fail=$((fail + 1))
  elif printf '%s' "$out" | grep -qF -- "$wanted"; then
    echo "  ok   $name"
    pass=$((pass + 1))
  else
    echo "  FAIL $name: it refused, but not for the stated reason"
    echo "        wanted: $wanted"
    printf '        %s\n' "$(printf '%s' "$out" | tail -2)"
    fail=$((fail + 1))
  fi
}

expect_pass() {
  local name="$1"
  shift
  if "$@" >/dev/null 2>&1; then
    echo "  ok   $name"
    pass=$((pass + 1))
  else
    echo "  FAIL $name: it refused"
    "$@" 2>&1 | tail -2 | sed 's/^/        /'
    fail=$((fail + 1))
  fi
}

# Append $2 to file $1 and assert the file changed -- a plant that did not apply measures nothing.
plant() {
  local file="$1" text="$2" before
  before="$(wc -c <"$file")"
  printf '%s' "$text" >>"$file"
  [ "$(wc -c <"$file")" -gt "$before" ] || {
    echo "  FAIL the plant into ${file##*/} did not apply"
    fail=$((fail + 1))
    return 1
  }
}

echo "check-redaction-not-in-base self-test:"

# --- the needle list, from the checker, cross-checked ---------------------------------------------
# FROM `--print-needles`, NOT PARSED: a code review added a double-quoted needle and the old sed,
# which read single-quoted lines only, silently probed four of five. The count is also compared with
# the lines of the array itself, so a needle the print mode somehow dropped is caught too.
mapfile -t needles < <("$checker" --print-needles)
declared="$(sed -n '/^NEEDLES=(/,/^)/p' "$checker" | grep -cE "^  ['\"]")"
if [ "${#needles[@]}" -ne "$declared" ] || [ "$declared" -lt 8 ]; then
  echo "  FAIL --print-needles gave ${#needles[@]} needle(s) and the array declares $declared"
  exit 1
fi
echo "  $declared needle(s), read from the checker"

# --- the baseline -----------------------------------------------------------------------------
fresh
expect_pass "a copy of the real build passes" "$checker" "$work"

# --- REAL: a base module built with the probe export ---------------------------------------------
# The needles whose code compiles for wasm today, and which the probe reaches. The other three --
# `pdf redaction [`, `pdf resources [`, `redact: the region is not cleared` -- are spelled only in
# code behind the native engines until #191, so no wasm build can contain them yet.
REAL=(
  'pdf geometry ['
  'content-stream edit'
  'a /ToUnicode'
  'a string token that begins with neither'
  'pdf region ['
)
if ! command -v cargo >/dev/null || ! rustup target list --installed 2>/dev/null | grep -qx wasm32-unknown-unknown; then
  echo "  FAIL the probe build needs cargo and the wasm32-unknown-unknown target; refusing rather" >&2
  echo "       than reporting the planted cases alone as if they were the whole suite" >&2
  exit 1
fi
# ITS OWN TARGET DIRECTORY, because RUSTFLAGS is part of cargo's fingerprint: sharing `target/`
# would rebuild every wasm artifact the next ordinary build asks for.
if ! RUSTFLAGS="--cfg burrow_redaction_probe" CARGO_TARGET_DIR="$probe_target" \
  cargo build -q -p burrow-wasm --target wasm32-unknown-unknown --release >"$workroot/probe.log" 2>&1; then
  echo "  FAIL the probe module did not build:"
  tail -5 "$workroot/probe.log" | sed 's/^/        /'
  exit 1
fi
probe="$probe_target/wasm32-unknown-unknown/release/burrow_wasm.wasm"
grep -qaF '__burrow_redaction_probe' "$probe" || {
  echo "  FAIL the probe module carries no __burrow_redaction_probe export, so the cfg did not apply"
  exit 1
}
for needle in "${needles[@]}"; do
  expected=no
  for real_needle in "${REAL[@]}"; do [ "$needle" = "$real_needle" ] && expected=yes; done
  present=no
  grep -qaF -- "$needle" "$probe" && present=yes
  if [ "$expected" = "$present" ]; then
    echo "  ok   the probe module $([ "$present" = yes ] && echo contains || echo lacks) '$needle', as pinned"
    pass=$((pass + 1))
  else
    echo "  FAIL the probe module $([ "$present" = yes ] && echo contains || echo lacks) '$needle', \
and REAL says it should$([ "$expected" = yes ] || echo "n't") -- update REAL deliberately"
    fail=$((fail + 1))
  fi
done
fresh
cp "$probe" "$base"
expect_refusal "a base module BUILT to reach redaction is refused by the real check" \
  "from a shipped module ('pdf geometry [' in" "$checker" "$work"

# --- PLANTED: every needle, into each shipped module ---------------------------------------------
for needle in "${needles[@]}"; do
  for target in base render; do
    fresh
    file="${!target}"
    plant "$file" "${needle} planted" || continue
    expect_refusal "'$needle' planted in the $target module is refused, by name" \
      "('$needle' in" "$checker" "$work"
  done
done

# --- near-misses, one per needle, which must not refuse --------------------------------------------
# `pdf name [` belongs in the base module (split's pruning); the rest are each needle with one
# character changed, so a rule loosened to a prefix or a case-insensitive match fails here.
for near in 'pdf name [planted]' 'pdf geometry' 'content stream edit' 'a ToUnicode' \
  'a string token that begins with either' 'pdf regions [x]' 'pdf redactions' 'pdf resources' \
  'redact: the region is cleared'; do
  fresh
  plant "$base" "$near" || continue
  expect_pass "the near-miss '$near' is not refused" "$checker" "$work"
done

# --- the positive controls ------------------------------------------------------------------------
for pair in 'base __burrow_qpdf_copy_in' 'render __burrow_pdfium_copy_in'; do
  target="${pair%% *}"
  control="${pair#* }"
  fresh
  file="${!target}"
  python3 - "$file" "$control" <<'PYEOF'
import sys
path, control = sys.argv[1], sys.argv[2].encode()
data = open(path, "rb").read()
swapped = data.replace(control, b"x" * len(control))
assert swapped != data, f"the control {control!r} is not in {path}"
open(path, "wb").write(swapped)
PYEOF
  expect_refusal "a $target module without its engine control is refused rather than passed" \
    "positive control '$control' is missing" "$checker" "$work"
done

# --- the preconditions ----------------------------------------------------------------------------
for variant in harness host; do
  fresh
  mkdir "$work/$variant"
  expect_refusal "a build carrying $variant/ is refused as not the production payload" \
    "this is a HARNESS build" "$checker" "$work"
done

fresh
cp "$base" "${base%.wasm}.extra.wasm"
expect_refusal "a third Rust module is refused rather than scanned silently" \
  "expected exactly 2 non-redaction Rust modules" "$checker" "$work"

fresh
rm -f "$render"
expect_refusal "a build missing the render module is refused" \
  "expected exactly 2 non-redaction Rust modules" "$checker" "$work"

fresh
plant "$base" "__burrow_redaction_probe" &&
  expect_refusal "the probe export's name in a shipped module is refused" \
    "the redaction probe export shipped" "$checker" "$work"

# A REDACTION MODULE IS NOT SCANNED, and must not be: it is the one place the needles belong.
fresh
cp "$probe" "$work/engines/burrow_wasm_redact_bg.0000000000000000.wasm"
expect_pass "a redaction module beside the other two is not scanned" "$checker" "$work"

# --- the checker's own consistency, in copies beside the original ----------------------------------
# BESIDE, per CLAUDE.md: a copy in a temp directory resolves its repository root wrongly and fails
# for that reason instead.
stale_copy() {
  local from="$1" to="$2" name="$3" wanted="$4"
  copy="$here/.check-redaction-not-in-base.copy.sh"
  python3 - "$checker" "$copy" "$from" "$to" <<'PYEOF'
import sys
text = open(sys.argv[1]).read()
old, new = sys.argv[3], sys.argv[4]
assert text.count(old) == 1, f"{old!r} is not where this expects it"
open(sys.argv[2], "w").write(text.replace(old, new, 1))
PYEOF
  chmod +x "$copy"
  fresh
  expect_refusal "$name" "$wanted" "$copy" "$work"
  rm -f "$copy"
  copy=""
}
stale_copy "  'pdf region ['"$'\n' "  'pdf regoin ['"$'\n' \
  "a needle no source line spells is refused as unable to match anything" \
  "the needle 'pdf regoin [' is not spelled"
stale_copy "  'the glyph geometry walk is reachable'"$'\n' "" \
  "a needle without its meaning is refused" \
  "each needle needs its meaning"

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED -- $fail case(s) failed, $pass passed" >&2
  exit 1
fi
echo "OK -- $pass adversarial case(s) all behaved as required"
