#!/usr/bin/env bash
# No part of PDFium reaches the web build.
#
# Spike 0004 took PDFium out of the web payload: 79.7% of everything a person downloaded
# before their first operation could run, reachable from ONE function (`page_count`), which
# qpdf answers through `StructureEngine::check`. The measured saving is 2,390,698 -> 462,146
# brotli, an 80.7% reduction. (`apps/web/size-budget.json` is the record and has moved a
# little since, by comment text in the worker bundle; the figure here is the removal.)
#
# WHY THIS IS A CHECK AND NOT AN INSPECTION. The removal touched eleven files across Rust, the
# worker bundle, the staging script and the tests, and every one of them fails differently:
# a stale `ENGINE_FILES` entry is a 404 at start-up, a leftover wasm-bindgen import is an
# instantiation error in the browser and nowhere else, and a forgotten `pdfium.js` in the
# bundle is 164 KB nobody notices because the page still works. "I looked and it is gone" does
# not distinguish those from each other, and none of them is visible in a green test run.
#
# WHAT IT EXAMINES, reported on every run rather than implied: every shipped file under
# `apps/web/dist`, which is what a person actually downloads -- and it refuses outright if
# that directory is a HARNESS build, because then it is not what a person downloads. The
# variant is a stated, checked precondition rather than an assumption; see below.
#
# A POSITIVE CONTROL IS THE HALF THAT MAKES IT A CHECK. Every rule below is "this string is
# absent", and a rule like that passes on an empty directory, a mistyped path, or a build that
# never ran. So the same scan requires qpdf's markers to be PRESENT: if it cannot find the
# engine that IS supposed to ship, it has not been looking at a build.
#
# Adversarial self-test: tools/test-check-no-pdfium-on-the-web.sh.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
dist="${1:-$repo/apps/web/dist}"

fail() {
  echo "::error::$*" >&2
  exit 1
}

[ -d "$dist" ] || fail "no build at $dist -- run \`pnpm build\` in apps/web first"

# --- WHICH BUILD IS THIS? ---------------------------------------------------------------
#
# The claim above is about the PRODUCTION payload. A harness build is a different artifact:
# `astro.config.mjs`'s `harnessGating()` keeps `dist/harness/` and `dist/host/` only when
# `BURROW_HARNESS=1`, and `host/harness-driver.js` carries a comment naming
# `__burrow_pdfium_copy_in` while explaining that the poison hook moved OFF it. That comment
# is correct and should stay; scanning it is the error.
#
# Found by the security review, measured: after any `pnpm e2e` the leftover `dist` is a
# harness build, and this check then failed naming a PDFium symbol -- which reads as "PDFium
# came back" rather than "you are pointing me at the wrong build". A security gate whose
# verdict depends on an unstated precondition is a gate somebody will learn to re-run until
# it passes.
#
# So the precondition is stated and checked rather than assumed. CI is unaffected: its build
# step sets no flag, so neither directory exists there.
for variant in harness host; do
  [ -e "$dist/$variant" ] || continue
  fail "this is a HARNESS build ($dist/$variant exists), and the claim is about the \
production payload -- rebuild without BURROW_HARNESS (\`pnpm build\` in apps/web) and re-run"
done

# Scanned as text. The worker bundle and the page scripts are the files that could name
# PDFium; the `.wasm` modules are binary and are covered by the artifact rule instead.
mapfile -t text_files < <(find "$dist" -type f \( -name '*.js' -o -name '*.mjs' -o -name '*.html' -o -name '*.txt' \) | sort)
[ "${#text_files[@]}" -gt 0 ] || fail "no scannable files under $dist -- this would pass vacuously"

# --- the rules -------------------------------------------------------------------------
#
# Each is a string that must NOT appear in any shipped text file, with what its presence
# would mean.
#
# COMMENTS ARE NOT STRIPPED, AND THE BUNDLE IS NOT MINIFIED. An earlier version of this
# comment claimed it was, which was wrong -- `stage-web-engines.mjs` CONCATENATES the worker
# sources, so every comment ships. The first run of this check failed on three comments that
# named `FPDF_` while describing code that had already been removed.
#
# That is kept as the rule rather than worked around: the scan stays exact, and prose in a
# shipped file that names an engine which is not there is itself worth removing. Nothing is
# lost -- the reasoning those comments carried is still recorded, without the symbol.
NEEDLES=(
  'FPDF_'
  '__burrow_pdfium_'
  'pdfiumWasm'
)
MEANS=(
  'a PDFium C entry point is called from shipped code'
  'a PDFium bridge global is defined or imported by the worker bundle'
  'the staged engine manifest still names a PDFium artifact'
)

# --- the positive control --------------------------------------------------------------
#
# qpdf IS supposed to ship. If these are missing, the scan is not looking at a build and
# every absence above means nothing.
PRESENT=(
  'createQpdfModule'
  '__burrow_qpdf_copy_in'
)

echo "check-no-pdfium-on-the-web: ${#text_files[@]} shipped text file(s) under ${dist#"$repo"/}"

for index in "${!NEEDLES[@]}"; do
  needle="${NEEDLES[$index]}"
  hits="$(grep -rlF -- "$needle" "${text_files[@]}" || true)"
  if [ -n "$hits" ]; then
    fail "${MEANS[$index]} ($needle): $(printf '%s' "$hits" | tr '\n' ' ')"
  fi
done
echo "  ${#NEEDLES[@]} absence rule(s), none matched"

for marker in "${PRESENT[@]}"; do
  grep -rqF -- "$marker" "${text_files[@]}" ||
    fail "the positive control '$marker' is missing, so this scan was not looking at a build"
done
echo "  ${#PRESENT[@]} positive control(s), all found"

# --- the artifact itself ---------------------------------------------------------------
#
# The wasm module is binary, so it is checked by name rather than by content. `engines/` is
# what `connect-src` names and what the size budget weighs.
staged_pdfium="$(find "$dist/engines" -maxdepth 1 -name 'pdfium*' 2>/dev/null || true)"
[ -z "$staged_pdfium" ] ||
  fail "a PDFium artifact is staged: $(printf '%s' "$staged_pdfium" | tr '\n' ' ')"

wasm_count="$(find "$dist/engines" -maxdepth 1 -name '*.wasm' | wc -l)"
[ "$wasm_count" -eq 2 ] ||
  fail "expected exactly 2 wasm modules (qpdf and the Rust binding), found $wasm_count"

# AND BY CONTENT, NOT ONLY BY NAME. The two rules above are both about the file's NAME, so a
# PDFium module staged under any other name passes them and keeps the count at 2. `FPDF_` is
# an exported symbol name and survives into the binary; the shipped `qpdf.wasm` contains none,
# so this rule is not vacuous. `grep -a` because a `.wasm` is binary.
for module in "$dist/engines/"*.wasm; do
  ! grep -qaF 'FPDF_' "$module" ||
    fail "a wasm module exports PDFium symbols under a non-PDFium name: ${module#"$dist"/}"
done
echo "  2 wasm module(s) staged, neither of them PDFium by name or by content"

# --- and the policy that governs fetching them -------------------------------------------
#
# `connect-src` names the exact engine URLs (ADR 0014). If it still permitted a PDFium URL,
# the artifact could be reintroduced without the policy noticing.
headers="$dist/_headers"
[ -f "$headers" ] || fail "no _headers in the build, so the policy cannot be checked"
! grep -qF 'pdfium' "$headers" || fail "the generated CSP still names a PDFium URL"
connect_wasm="$(grep -o 'connect-src[^;]*' "$headers" | grep -o '\.wasm' | wc -l)"
[ "$connect_wasm" -eq 2 ] ||
  fail "connect-src names $connect_wasm wasm URLs, expected 2"
echo "  connect-src names 2 wasm URLs, neither of them PDFium"

echo "OK -- PDFium reaches no part of the web build."
