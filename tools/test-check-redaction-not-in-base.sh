#!/usr/bin/env bash
# Adversarial self-test for tools/check-redaction-not-in-base.sh.
#
# That checker is made of ABSENCE rules, which pass on anything that lacks the needle -- and until
# #191 builds a redaction module, no real artifact contains one. So each rule's positive is PLANTED
# here: every needle is written into a copy of a real build's base module and bundle in turn, and
# the checker must refuse NAMING that needle. Near-misses that must NOT refuse sit beside them, so
# a rule that matched everything would fail here too.
#
# The fixtures are copies of `apps/web/dist`, never the build itself. A harness build is accepted
# as the SOURCE of the copy: the Rust module and worker bundle are the same in both, and the copy's
# harness routes are removed so the checker's production precondition holds. A separate case keeps
# them and requires the refusal.
#
# Usage: tools/test-check-redaction-not-in-base.sh

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
checker="$here/check-redaction-not-in-base.sh"
real="$repo/apps/web/dist"
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

# A fresh production-shaped copy of the real build, at $work.
fresh() {
  work="$workroot/dist"
  rm -rf "$work"
  cp -r "$real" "$work"
  rm -rf "$work/harness" "$work/host"
  module="$(find "$work/engines" -maxdepth 1 -name 'burrow_wasm_bg.*.wasm' | head -1)"
  bundle="$(find "$work/engines" -maxdepth 1 -name 'burrow-worker.*.js' | head -1)"
}

# Run the checker (or $2's command) over $work and require a refusal whose output names $wanted.
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

# --- the baseline -----------------------------------------------------------------------------
fresh
expect_pass "a copy of the real build passes" "$checker" "$work"

# --- one planted positive per needle, in each artifact ------------------------------------------
# The needles are read from the checker itself, so a needle added there is probed here without
# anyone remembering to.
mapfile -t needles < <(sed -n '/^NEEDLES=(/,/^)/p' "$checker" | sed -n "s/^  '\(.*\)'$/\1/p")
[ "${#needles[@]}" -ge 4 ] || {
  echo "  FAIL read ${#needles[@]} needle(s) from the checker, expected at least 4"
  exit 1
}
for needle in "${needles[@]}"; do
  for target in module bundle; do
    fresh
    file="${!target}"
    plant "$file" "${needle}planted-rule]: planted" || continue
    expect_refusal "'$needle' planted in the base $target is refused, by name" \
      "'$needle' in" "$checker" "$work"
  done
done

# --- near-misses, which must not refuse ---------------------------------------------------------
# `pdf name [` belongs in the base module (split's pruning); the others are each needle without
# its bracket, or with a different word, so a rule loosened to a prefix match fails here.
for near in 'pdf name [planted]' 'pdf geometry' 'pdf redactions' 'pdf regions [x]'; do
  fresh
  plant "$module" "$near" || continue
  expect_pass "the near-miss '$near' in the base module is not refused" "$checker" "$work"
done

# --- the positive control -----------------------------------------------------------------------
fresh
python3 - "$module" "$bundle" <<'PYEOF'
import sys
for path in sys.argv[1:]:
    data = open(path, "rb").read()
    swapped = data.replace(b"__burrow_qpdf_copy_in", b"__burrow_xxxx_copy_in")
    assert swapped != data, f"the positive control is not in {path}"
    open(path, "wb").write(swapped)
PYEOF
expect_refusal "an artifact without the qpdf control is refused rather than passed" \
  "positive control '__burrow_qpdf_copy_in' is missing" "$checker" "$work"

# --- the preconditions --------------------------------------------------------------------------
fresh
mkdir "$work/harness"
expect_refusal "a harness build is refused as not the production payload" \
  "this is a HARNESS build" "$checker" "$work"

fresh
cp "$module" "${module%.wasm}.extra.wasm"
expect_refusal "two base Rust modules are refused rather than one being picked" \
  "expected exactly 1 base Rust module" "$checker" "$work"

fresh
rm -f "$bundle"
expect_refusal "a build with no base bundle is refused" \
  "expected exactly 1 base worker bundle" "$checker" "$work"

# --- a needle nothing spells any more -----------------------------------------------------------
# A COPY OF THE CHECKER, BESIDE THE ORIGINAL, per CLAUDE.md: one in a temp directory resolves its
# repository root wrongly and fails for that reason instead.
copy="$here/.check-redaction-not-in-base.stale.sh"
python3 - "$checker" "$copy" <<'PYEOF'
import sys
text = open(sys.argv[1]).read()
line = "  'pdf region ['\n"
assert text.count(line) == 1, "the needle to stale is not where this expects it"
open(sys.argv[2], "w").write(text.replace(line, "  'pdf regoin ['\n", 1))
PYEOF
chmod +x "$copy"
fresh
expect_refusal "a needle no source file spells is refused as unable to match anything" \
  "the needle 'pdf regoin [' is not spelled" "$copy" "$work"
rm -f "$copy"
copy=""

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED -- $fail case(s) failed, $pass passed" >&2
  exit 1
fi
echo "OK -- $pass adversarial case(s) all behaved as required"
