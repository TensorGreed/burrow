#!/usr/bin/env bash
# Adversarial self-test for tools/check-no-pdfium-on-the-web.sh.
#
# That checker is entirely made of ABSENCE rules, which is the shape that passes on an empty
# directory, a mistyped path, or a build that never ran. So every rule is broken in turn
# against a copy of a real build and required to refuse, NAMING the reason — an exit-code-only
# assertion would pass on a checker that failed for an unrelated reason.
#
# The fixtures are copies of `apps/web/dist`, never the build itself: a self-test that edits
# the thing being shipped is one interrupted run away from putting a planted file on main.
#
# Usage: tools/test-check-no-pdfium-on-the-web.sh

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
checker="$here/check-no-pdfium-on-the-web.sh"
real="$repo/apps/web/dist"
work=""
workroot=""

cleanup() {
  trap - EXIT INT TERM HUP
  # The mktemp PARENT, not just `$work` inside it. Removing only the child left one empty
  # directory in /tmp per run; ten had accumulated before a review counted them.
  [ -n "${workroot:-}" ] && rm -rf "$workroot"
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

# `name <expected text> <command...>`
expect_refusal() {
  local name="$1" expect="$2"
  shift 2
  local out status=0
  out="$("$@" 2>&1)" || status=$?
  if [ "$status" -eq 0 ]; then
    echo "  FAIL $name: it did not refuse"
    fail=$((fail + 1))
    return
  fi
  if ! grep -qF "$expect" <<<"$out"; then
    echo "  FAIL $name: it refused, but not for the stated reason"
    echo "        wanted: $expect"
    sed 's/^/        /' <<<"$out" | tail -3
    fail=$((fail + 1))
    return
  fi
  echo "  ok   $name"
  pass=$((pass + 1))
}

# A fresh copy of the real build, so each case starts from something that PASSES.
fixture() {
  rm -rf "$work"
  cp -r "$real" "$work"
}

workroot="$(mktemp -d)"
work="$workroot/dist"

# THE BASELINE. Without it every case below could be passing against a checker that refuses
# everything, which is the same non-check as one that refuses nothing.
fixture
if "$checker" "$work" >/dev/null 2>&1; then
  echo "  ok   the real build passes, so a refusal below means something"
  pass=$((pass + 1))
else
  echo "  FAIL the real build does not pass, so no case below means anything"
  "$checker" "$work" 2>&1 | tail -4 | sed 's/^/        /'
  exit 1
fi

# --- Rule 1: a PDFium C entry point called from shipped code ---------------------------------
fixture
worker="$(find "$work/engines" -name 'burrow-worker.*.js' | head -1)"
printf '\nself.__x = () => FPDF_GetPageCount(0);\n' >>"$worker"
expect_refusal "a PDFium C entry point in the bundle is refused" \
  "a PDFium C entry point is called from shipped code" \
  "$checker" "$work"

# --- Rule 2: a PDFium bridge global ----------------------------------------------------------
fixture
worker="$(find "$work/engines" -name 'burrow-worker.*.js' | head -1)"
printf '\nself.__burrow_pdfium_copy_in = () => 0;\n' >>"$worker"
expect_refusal "a PDFium bridge global in the bundle is refused" \
  "a PDFium bridge global is defined or imported" \
  "$checker" "$work"

# --- Rule 3: the staged manifest naming a PDFium artifact ------------------------------------
fixture
worker="$(find "$work/engines" -name 'burrow-worker.*.js' | head -1)"
printf '\n// pdfiumWasm\n' >>"$worker"
expect_refusal "a PDFium entry in the engine manifest is refused" \
  "the staged engine manifest still names a PDFium artifact" \
  "$checker" "$work"

# --- Rule 4: the artifact itself ------------------------------------------------------------
#
# The one rule the text scan cannot catch: a `.wasm` is binary, so it is checked by name.
fixture
cp "$work/engines/qpdf."*.wasm "$work/engines/pdfium.0123456789abcdef.wasm"
expect_refusal "a staged PDFium artifact is refused" \
  "a PDFium artifact is staged" \
  "$checker" "$work"

# --- Rule 5: the wasm module count -----------------------------------------------------------
fixture
rm -f "$work/engines/qpdf."*.wasm
expect_refusal "a missing engine module is refused, not passed as one fewer thing to object to" \
  "expected exactly 2 wasm modules" \
  "$checker" "$work"

# --- Rule 6: the policy that governs fetching them --------------------------------------------
fixture
sed -i 's|connect-src |connect-src http://localhost:4321/engines/pdfium.0123456789abcdef.wasm |' \
  "$work/_headers"
expect_refusal "a CSP that still permits a PDFium URL is refused" \
  "the generated CSP still names a PDFium URL" \
  "$checker" "$work"

# --- Rule 7: a PDFium module staged under a name that is not "pdfium" -------------------------
#
# Rules 4 and 5 are both about the FILENAME, so a module renamed on the way in passes both and
# keeps the count at 2. This is the content rule, and it is the one that would catch a staging
# script that hashed the wrong source. Appending to a copy keeps the name and the count intact,
# so nothing but the content rule can be what refuses.
fixture
printf 'FPDF_GetPageCount' >>"$(find "$work/engines" -name 'qpdf.*.wasm' | head -1)"
expect_refusal "a wasm module exporting PDFium symbols under another name is refused" \
  "exports PDFium symbols under a non-PDFium name" \
  "$checker" "$work"

# --- Rule 8: the build variant, which is a precondition and not a rule about PDFium ------------
#
# Found by the security review, measured on a real tree: `dist/host/harness-driver.js` carries a
# comment naming `__burrow_pdfium_copy_in` while explaining that the poison hook moved OFF it, so
# after any `pnpm e2e` this checker refused, naming a PDFium symbol. The comment is right; the
# scan was looking at the wrong artifact. Both directories are asserted, because deleting one and
# keeping the other is the shape a future `harnessGating()` change could take.
for variant in harness host; do
  fixture
  mkdir -p "$work/$variant"
  expect_refusal "a harness build is refused for being one ($variant/), not for naming PDFium" \
    "this is a HARNESS build" \
    "$checker" "$work"
done

# --- The positive control, which is what stops every rule above passing vacuously -------------
#
# Break the thing that MUST be present. Every absence rule still holds — there is no PDFium in
# an empty build either — so a checker without this control would report OK over nothing.
fixture
worker="$(find "$work/engines" -name 'burrow-worker.*.js' | head -1)"
: >"$worker"
expect_refusal "a build with the engine gutted is refused by the positive control" \
  "positive control" \
  "$checker" "$work"

# --- And an empty directory, which is the shape absence rules are worst at -------------------
fixture
rm -rf "${work:?}/"*
expect_refusal "an empty build is refused rather than passing every absence rule" \
  "no scannable files" \
  "$checker" "$work"

# --- A path that does not exist ---------------------------------------------------------------
expect_refusal "a missing build directory is refused" \
  "no build at" \
  "$checker" "$work/does-not-exist"

# --- THE PROBE GATE: each rule is the thing that catches its own defect ------------------------
#
# Every case above shows that the checker refuses a planted defect. None of them shows WHICH
# rule refused, and that is the gap the convention closes: a checker whose rules all matched
# everything would pass all ten, and so would one where a second rule happened to cover the
# first's case. This gate breaks ONE rule at a time in a copy of the checker, replants that
# rule's defect, and requires the gutted copy to PASS -- which is only true if that rule was
# the thing doing the work.
#
# The shape differs from the sibling suites deliberately. Their checkers carry an in-run probe
# table, so a gutted rule makes the checker REFUSE with "does not match its own fixture". This
# checker is made of absence rules: gutting one makes it more permissive, never noisier. So the
# assertion is inverted -- a gutted rule must let its own defect through -- and the failure this
# catches is a rule that was never load-bearing.
#
# The copy sits BESIDE the original, because the script resolves `repo` from its own location;
# a copy in a temp directory exits non-zero for the wrong reason, which an exit-code-only
# assertion reports as a pass.
copy="$here/.probe-gate-fixture.sh"
gutted_cleanup() { rm -f "$copy"; }
trap 'gutted_cleanup; cleanup' EXIT INT TERM HUP

# `name <sed expression that guts one rule> <shell to plant that rule's defect>`
probe_gate() {
  local name="$1" mutation="$2" plant="$3"
  sed "$mutation" "$checker" >"$copy"
  chmod +x "$copy"
  if cmp -s "$copy" "$checker"; then
    echo "  FAIL probe-gate/$name: the mutation did not apply, so this case would prove nothing"
    fail=$((fail + 1))
    return
  fi
  fixture
  eval "$plant"
  # The ORIGINAL must refuse -- otherwise the defect was not planted and the pass below is
  # vacuous. This is the assert-the-mutation-applied rule, one level up.
  if "$checker" "$work" >/dev/null 2>&1; then
    echo "  FAIL probe-gate/$name: the unmutated checker did not refuse, so nothing was planted"
    fail=$((fail + 1))
    return
  fi
  if "$copy" "$work" >/dev/null 2>&1; then
    echo "  ok   probe-gate/$name: that rule alone is what catches it"
    pass=$((pass + 1))
  else
    echo "  FAIL probe-gate/$name: the defect is still caught with the rule removed, so the"
    echo "        rule is not what the case above was measuring"
    fail=$((fail + 1))
  fi
}

probe_gate "the FPDF_ needle" \
  "s/^  'FPDF_'$/  'ZZZ_NO_SUCH_SYMBOL_'/" \
  'printf "\nself.__x = () => FPDF_GetPageCount(0);\n" >>"$(find "$work/engines" -name "burrow-worker.*.js" | head -1)"'

probe_gate "the bridge-global needle" \
  "s/^  '__burrow_pdfium_'$/  '__ZZZ_no_such_global_'/" \
  'printf "\nself.__burrow_pdfium_copy_in = () => 0;\n" >>"$(find "$work/engines" -name "burrow-worker.*.js" | head -1)"'

probe_gate "the manifest needle" \
  "s/^  'pdfiumWasm'$/  'zzzNoSuchKey'/" \
  'printf "\n// pdfiumWasm\n" >>"$(find "$work/engines" -name "burrow-worker.*.js" | head -1)"'

probe_gate "the positive control" \
  "s/^  'createQpdfModule'$/  ''/;s/^  '__burrow_qpdf_copy_in'$/  ''/" \
  ': >"$(find "$work/engines" -name "burrow-worker.*.js" | head -1)"'

probe_gate "the staged-artifact rule" \
  "s|-name 'pdfium\*'|-name 'zzz-no-such-artifact*'|" \
  'cp "$work/engines/burrow_wasm_bg."*.wasm "$work/engines/pdfium.0123456789abcdef.wasm" && rm -f "$work/engines/burrow_wasm_bg."*.wasm'

probe_gate "the wasm-content rule" \
  "s/! grep -qaF 'FPDF_'/! grep -qaF 'ZZZ_NO_SUCH_SYMBOL_'/" \
  'printf "FPDF_GetPageCount" >>"$(find "$work/engines" -name "qpdf.*.wasm" | head -1)"'

# THE PLANT HERE CARRIES NO PDFIUM TEXT, and the first attempt at this case is why the
# distinction is worth stating. It planted a `host/harness-driver.js` naming
# `__burrow_pdfium_copy_in` --- a faithful copy of the real false positive --- and the gutted
# copy still refused, because the bridge-global needle caught the text. That is correct
# behaviour and a useless probe: it measured the needle, not the precondition. An EMPTY
# `host/` isolates the precondition, which is the thing being probed.
probe_gate "the build-variant precondition" \
  's/for variant in harness host; do/for variant in zzz-no-such-dir; do/' \
  'mkdir -p "$work/host"'

gutted_cleanup

echo
if [ "$fail" -ne 0 ]; then
  echo "FAILED — $fail case(s) failed, $pass passed" >&2
  exit 1
fi
echo "OK — $pass adversarial case(s) all behaved as required"
