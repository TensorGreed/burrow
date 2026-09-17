#!/usr/bin/env bash
# Adversarial self-test for tools/check-pdfium-is-render-only.sh.
#
# That checker's base rules are ABSENCE rules, which is the shape that passes on an empty
# directory, a mistyped path, or a build that never ran. Since ADR 0026 it also has PARTITION
# controls, which fail the opposite way: a build staging no PDFium at all would satisfy every
# absence rule and serve a thumbnail strip that cannot render. Both kinds are broken in turn
# against a copy of a real build and required to refuse, NAMING the reason — an exit-code-only
# assertion would pass on a checker that failed for an unrelated reason.
#
# The fixtures are copies of `apps/web/dist`, never the build itself: a self-test that edits
# the thing being shipped is one interrupted run away from putting a planted file on main.
#
# Usage: tools/test-check-pdfium-is-render-only.sh

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
checker="$here/check-pdfium-is-render-only.sh"
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
  "a PDFium C entry point is called from code outside the render bundle" \
  "$checker" "$work"

# --- Rule 2: a PDFium bridge global ----------------------------------------------------------
fixture
worker="$(find "$work/engines" -name 'burrow-worker.*.js' | head -1)"
printf '\nself.__burrow_pdfium_copy_in = () => 0;\n' >>"$worker"
expect_refusal "a PDFium bridge global in the bundle is refused" \
  "a PDFium bridge global is defined or imported outside the render bundle" \
  "$checker" "$work"

# --- Rule 3: the staged manifest naming a PDFium artifact ------------------------------------
fixture
worker="$(find "$work/engines" -name 'burrow-worker.*.js' | head -1)"
printf '\n// pdfiumWasm\n' >>"$worker"
expect_refusal "a PDFium entry in the engine manifest is refused" \
  "a PDFium artifact is named in a manifest outside the render bundle" \
  "$checker" "$work"

# --- Rule 4: the artifact count --------------------------------------------------------------
#
# ONE PDFium artifact, not zero and not two. It used to be "none at all"; ADR 0026 made a
# staged PDFium the expected state, so what is checkable is the count rather than the absence.
fixture
# THE COUNT STAYS 4, so it is the PDFium count and nothing else that can refuse: the render
# bundle's Rust module is REPLACED by a second PDFium rather than a fifth file being added.
cp "$work/engines/pdfium."*.wasm "$work/engines/pdfium.0123456789abcdef.wasm"
rm -f "$work/engines/burrow_wasm_render_bg."*.wasm
expect_refusal "a second staged PDFium artifact is refused" \
  "expected exactly 1 PDFium artifact staged" \
  "$checker" "$work"

# --- Rule 5: the wasm module count -----------------------------------------------------------
fixture
rm -f "$work/engines/qpdf."*.wasm
expect_refusal "a missing engine module is refused, not passed as one fewer thing to object to" \
  "expected exactly 4 wasm modules" \
  "$checker" "$work"

# --- Rule 6: the policy that governs fetching them --------------------------------------------
#
# The CSP must name exactly what is staged. A FIFTH wasm URL is the shape of a policy that
# outlived an artifact, or one somebody widened by hand -- and it is checked rather than the
# old "no pdfium anywhere in the policy", which ADR 0026 made false.
fixture
sed -i 's|connect-src |connect-src http://localhost:4321/engines/zzz.0123456789abcdef.wasm |' \
  "$work/_headers"
expect_refusal "a CSP naming a wasm URL the build does not stage is refused" \
  "connect-src names 5 wasm URLs" \
  "$checker" "$work"

# --- Rule 6b: and it must still name exactly one PDFium URL -----------------------------------
fixture
sed -i 's|/engines/pdfium\.[0-9a-f]*\.wasm|/engines/zzz.0123456789abcdef.wasm|g' "$work/_headers"
expect_refusal "a CSP that no longer permits the PDFium URL is refused" \
  "connect-src names 0 PDFium URLs" \
  "$checker" "$work"

# --- Rule 7: the base bundle may not NAME a PDFium URL ----------------------------------------
#
# The rule that decides the claim rather than merely being consistent with it: a worker fetches
# only what its own generated manifest names. Planted as a manifest entry rather than as a bare
# string, so it is this rule and not the `pdfiumWasm` needle that catches it.
fixture
python3 - "$work" <<'PLANT'
import pathlib, sys, re
dist = pathlib.Path(sys.argv[1])
base = next(p for p in (dist / "engines").glob("burrow-worker.*.js"))
text = base.read_text()
text = text.replace('"control": {', '"zzz": {\n    "url": "http://localhost:4321/engines/pdfium.0123456789abcdef.wasm",\n    "integrity": "sha384-x",\n    "bytes": 1\n  },\n  "control": {', 1)
base.write_text(text)
PLANT
expect_refusal "a base bundle whose manifest names a PDFium URL is refused" \
  "the base bundle's own engine manifest names a PDFium URL" \
  "$checker" "$work"

# --- Rule 8a: the render bundle must be able to fetch one --------------------------------------
#
# The other direction, and the one an absence-only checker cannot see: a build that staged
# PDFium and then forgot to name it would pass every rule above and render nothing.
fixture
sed -i 's|"url": "http://localhost:4321/engines/pdfium\.[0-9a-f]*\.wasm"|"url": "http://localhost:4321/engines/zzz.wasm"|' \
  "$(find "$work/engines" -name 'burrow-render-worker.*.js' | head -1)"
expect_refusal "a render bundle whose manifest names no PDFium URL is refused" \
  "the render bundle's engine manifest names no PDFium URL" \
  "$checker" "$work"

# --- Rule 9: the partition is not vacuous -----------------------------------------------------
#
# Gut the render bundle. Every absence rule over the base set still holds -- there is no PDFium
# outside the render bundle in this fixture either -- so without the partition controls the
# checker would report OK over a build whose renderer is empty.
fixture
: >"$(find "$work/engines" -name 'burrow-render-worker.*.js' | head -1)"
expect_refusal "an empty render bundle is refused rather than satisfying every absence rule" \
  "the partition is vacuous" \
  "$checker" "$work"

# --- Rule 10: exactly one render bundle -------------------------------------------------------
#
# The partition rests on there being one file PDFium is allowed in. Two would mean the base set
# is missing one of them, which is a scan that quietly examines less than it says.
fixture
cp "$(find "$work/engines" -name 'burrow-render-worker.*.js' | head -1)" \
  "$work/engines/burrow-render-worker.0123456789abcdef.js"
expect_refusal "a second render bundle is refused rather than silently leaving one unscanned" \
  "expected exactly 1 render bundle" \
  "$checker" "$work"

# --- Rule 11: a PDFium module staged under a name that is not "pdfium" ------------------------
#
# Rules 4 and 5 are both about the FILENAME, so a module renamed on the way in passes both and
# keeps the count at 4. This is the content rule, and it is the one that would catch a staging
# script that hashed the wrong source. Appending to a copy keeps the name and the count intact,
# so nothing but the content rule can be what refuses.
fixture
printf 'FPDF_GetPageCount' >>"$(find "$work/engines" -name 'qpdf.*.wasm' | head -1)"
expect_refusal "a wasm module exporting PDFium symbols under another name is refused" \
  "exports PDFium symbols under a non-PDFium name" \
  "$checker" "$work"

# --- Rule 12: and the PDFium module must actually be PDFium -----------------------------------
#
# The same rule from the other side. A staging script that copied the wrong source to the right
# name keeps every count intact, and the only thing that can notice is the content.
fixture
cp "$(find "$work/engines" -name 'qpdf.*.wasm' | head -1)" \
  "$(find "$work/engines" -name 'pdfium.*.wasm' | head -1)"
expect_refusal "a non-PDFium module staged under the PDFium name is refused" \
  "the staged PDFium module exports no PDFium symbols" \
  "$checker" "$work"

# --- Rule 12a: the base Rust module may not import a PDFium bridge global -------------------
#
# THE HALF ADR 0026 CALLS STRUCTURAL, and nothing scanned it until code review measured the
# needle. Neither Rust module contains `FPDF_` -- they reach PDFium through the bridge -- and
# the base set is text-only, so a `pkg/` built with the wrong features shipped a module
# importing globals that do not exist, and only a browser instantiation failure said so.
#
# Planted by swapping the two Rust modules' CONTENTS, which is exactly what a stale or
# mis-ordered `--out-dir` produces, and keeps every name and count intact.
fixture
base_rust="$(find "$work/engines" -name 'burrow_wasm_bg.*.wasm' | head -1)"
render_rust="$(find "$work/engines" -name 'burrow_wasm_render_bg.*.wasm' | head -1)"
cp "$render_rust" "$base_rust"
# THE MESSAGE IS THE PRESENCE RULE'S, not the absence rule's, because a swap makes both true
# and presence is checked first. That is the right order -- "this is not the module you meant"
# is a better first sentence than "this module has an extra import" -- and the absence rule is
# not going untested for it: it has its own probe gate below, planted by APPENDING the import
# so only one direction can fire.
expect_refusal "a base Rust module that is not the base build is refused" \
  "the base Rust module imports no qpdf bridge global" \
  "$checker" "$work"

# --- Rule 12b: and the render one may not import a qpdf bridge global ------------------------
fixture
base_rust="$(find "$work/engines" -name 'burrow_wasm_bg.*.wasm' | head -1)"
render_rust="$(find "$work/engines" -name 'burrow_wasm_render_bg.*.wasm' | head -1)"
cp "$base_rust" "$render_rust"
expect_refusal "a render Rust module built with the wrong features is refused" \
  "the render Rust module imports no PDFium bridge global" \
  "$checker" "$work"

# --- Rule 13: the build variant, which is a precondition and not a rule about PDFium ------------
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

# --- And a build with nothing but the render bundle in it -------------------------------------
#
# The shape absence rules are worst at. It was `rm -rf dist/*` until ADR 0026 put the partition
# check first: an empty directory now refuses for having no render bundle, which is true and is
# a different rule. Leaving it that way would have made the base-set emptiness rule unreachable,
# and an unreachable rule reads as coverage. So the fixture keeps the render bundle and removes
# everything else, which is the one arrangement that reaches it.
fixture
find "$work" -mindepth 1 -not -name 'burrow-render-worker.*.js' -not -path "$work/engines" -delete 2>/dev/null || true
expect_refusal "a build with no base files is refused rather than passing every absence rule" \
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

# `0,/.../s//.../` REPLACES ONLY THE FIRST MATCH, and that is load-bearing here: `NEEDLES` and
# `RENDER_PRESENT` hold the same three literals -- the base set may not contain them and the
# render bundle must -- so a global mutation guts both rules at once and the copy refuses for
# the other one. Measured: three probe gates reported "the defect is still caught" when what
# had happened is that the partition control caught it. NEEDLES is first in the file.
probe_gate "the FPDF_ needle" \
  "0,/^  'FPDF_'$/s//  'ZZZ_NO_SUCH_SYMBOL_'/" \
  'printf "\nself.__x = () => FPDF_GetPageCount(0);\n" >>"$(find "$work/engines" -name "burrow-worker.*.js" | head -1)"'

probe_gate "the bridge-global needle" \
  "0,/^  '__burrow_pdfium_'$/s//  '__ZZZ_no_such_global_'/" \
  'printf "\nself.__burrow_pdfium_copy_in = () => 0;\n" >>"$(find "$work/engines" -name "burrow-worker.*.js" | head -1)"'

probe_gate "the manifest needle" \
  "0,/^  'pdfiumWasm'$/s//  'zzzNoSuchKey'/" \
  'printf "\n// pdfiumWasm\n" >>"$(find "$work/engines" -name "burrow-worker.*.js" | head -1)"'

probe_gate "the positive control" \
  "s/^  'createQpdfModule('$/  ''/;s/^  'self.__burrow_qpdf_'$/  ''/" \
  ': >"$(find "$work/engines" -name "burrow-worker.*.js" | head -1)"'

# GUT THE COMPARISON, NOT THE `find`. Blanking the pattern makes the count 0, which fails the
# same `-eq 1` for the opposite reason -- a mutation that changes why a rule fires is not a
# mutation that removes it.
probe_gate "the PDFium artifact count" \
  's/\[ "$staged_pdfium" -eq 1 \]/[ "$staged_pdfium" -ge 0 ]/' \
  'cp "$work/engines/pdfium."*.wasm "$work/engines/pdfium.0123456789abcdef.wasm" && rm -f "$work/engines/burrow_wasm_render_bg."*.wasm'

probe_gate "the base bundle's manifest rule" \
  's@^! grep -qE .\"url\".*base_bundle\" ||@true ||@' \
  'sed -i "s|\"control\": {|\"zzz\": {\\n    \"url\": \"http://localhost:4321/engines/pdfium.0123456789abcdef.wasm\",\\n    \"integrity\": \"sha384-x\",\\n    \"bytes\": 1\\n  },\\n  \"control\": {|" "$(find "$work/engines" -name "burrow-worker.*.js" | head -1)"'

# THE PLANT LEAVES THE MANIFEST INTACT, and the first attempt at this case is why that matters.
# It gutted the render bundle outright (`: > file`), and the copy still refused -- correctly,
# because a bundle with no manifest also fails the "the render bundle's engine manifest names no
# PDFium URL" rule one section up. Two rules catching one defect is defence in depth and a
# useless probe: it measured whichever rule ran first. Removing only the PDFium SYMBOLS isolates
# the partition control, which is the thing being probed.
probe_gate "the partition controls" \
  's/for marker in "\${RENDER_PRESENT\[@\]}"; do/for marker in; do/' \
  'sed -i "s/FPDF_/ZZZZ_/g; s/__burrow_pdfium_/__zzz_no_such_/g" "$(find "$work/engines" -name "burrow-render-worker.*.js" | head -1)"'

probe_gate "the wasm-content rule" \
  "s/! grep -qaF 'FPDF_'/! grep -qaF 'ZZZ_NO_SUCH_SYMBOL_'/" \
  'printf "FPDF_GetPageCount" >>"$(find "$work/engines" -name "qpdf.*.wasm" | head -1)"'

# THE PLANT HERE CARRIES NO PDFIUM TEXT, and the first attempt at this case is why the
# distinction is worth stating. It planted a `host/harness-driver.js` naming
# `__burrow_pdfium_copy_in` --- a faithful copy of the real false positive --- and the gutted
# copy still refused, because the bridge-global needle caught the text. That is correct
# behaviour and a useless probe: it measured the needle, not the precondition. An EMPTY
# `host/` isolates the precondition, which is the thing being probed.
# APPENDED, NOT SWAPPED. Copying one Rust module over the other trips BOTH directions at once
# -- the base module then has a PDFium import AND has lost its qpdf one -- so neither gate
# could show which rule was doing the work. Appending the offending name leaves the module's
# own import intact, so exactly one rule can fire. (The refusal cases above still swap, because
# swapping is what a stale `--out-dir` actually produces and that is the defect worth planting.)
probe_gate "the base Rust module's import rule" \
  "s/! grep -qaF '__burrow_pdfium_copy_in'/! grep -qaF 'ZZZ_no_such_import'/" \
  'printf "__burrow_pdfium_copy_in" >>"$(find "$work/engines" -name "burrow_wasm_bg.*.wasm" | head -1)"'

probe_gate "the render Rust module's import rule" \
  "s/! grep -qaF '__burrow_qpdf_copy_in'/! grep -qaF 'ZZZ_no_such_import'/" \
  'printf "__burrow_qpdf_copy_in" >>"$(find "$work/engines" -name "burrow_wasm_render_bg.*.wasm" | head -1)"'

# THE FOUR COUNT RULES, which had refusal cases (5, 6, 6b, 10) and no gate showing that THAT
# rule is what refused. Raised by security review: a refusal case says the checker noticed,
# not which part of it noticed, and in a file where several rules read the same directory that
# is a real distinction.
probe_gate "the wasm module count" \
  's/\[ "$wasm_count" -eq 4 \]/[ "$wasm_count" -ge 0 ]/' \
  'rm -f "$work/engines/qpdf."*.wasm'

probe_gate "the connect-src wasm URL count" \
  's/\[ "$connect_wasm" -eq 4 \]/[ "$connect_wasm" -ge 0 ]/' \
  'sed -i "s|connect-src |connect-src http://localhost:4321/engines/zzz.0123456789abcdef.wasm |" "$work/_headers"'

probe_gate "the connect-src PDFium URL count" \
  's/\[ "$connect_pdfium" -eq 1 \]/[ "$connect_pdfium" -ge 0 ]/' \
  'sed -i "s|/engines/pdfium\.[0-9a-f]*\.wasm|/engines/zzz.0123456789abcdef.wasm|g" "$work/_headers"'

probe_gate "the render bundle count" \
  's/\[ "${#render_bundles\[@\]}" -eq 1 \]/[ "${#render_bundles[@]}" -ge 1 ]/' \
  'cp "$(find "$work/engines" -name "burrow-render-worker.*.js" | head -1)" "$work/engines/burrow-render-worker.0123456789abcdef.js"'

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
