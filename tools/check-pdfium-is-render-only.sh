#!/usr/bin/env bash
# PDFium reaches the RENDER bundle and nothing else a person downloads to use a tool.
#
# Spike 0004 took PDFium out of the web payload entirely: 79.7% of everything a person
# downloaded before their first operation could run, reachable from ONE function (`page_count`),
# which qpdf answers through `StructureEngine::check`. The measured saving was 2,390,698 ->
# 462,146 brotli, an 80.7% reduction.
#
# ADR 0026 brings PDFium back, because rendering needs it, and the whole design is about not
# giving that saving back. It is in a SECOND worker bundle, `burrow-render-worker.js`, which a
# page fetches only when it needs a picture of a page. This file is the scan that says so.
#
# THE CLAIM IT CHECKS, in one sentence: a visitor who lands on `/merge-pdf` and merges two
# files downloads no PDFium at all.
#
# WHY THIS IS A CHECK AND NOT AN INSPECTION. The split touches Rust cargo features, two
# wasm-pack builds, two bundle definitions, the CSP generator and the size budget, and every
# one of them fails differently: a stale `BUNDLES` entry is a 404 at start-up, a leftover
# wasm-bindgen import is an instantiation error in the browser and nowhere else, and a
# `pdfium.js` that found its way into the base bundle is 164 KB nobody notices because the page
# still works. "I looked and it is only in the render bundle" does not distinguish those from
# each other, and none of them is visible in a green test run.
#
# IT IS THE THIRD OF THREE LAYERS, AND THE WEAKEST ON PURPOSE. Two of them are in the build
# graph, which is what the other two are for:
#
#   1. `bindings/burrow-wasm` is built twice under mutually exclusive cargo features, so the
#      base Rust module has no `__burrow_pdfium_*` import to leave dangling. `src/lib.rs`
#      refuses a wasm32 build that enables both.
#   2. `tools/stage-web-engines.mjs` builds each bundle from its own source list and generates
#      each bundle's own slice of the manifest into it, so the base bundle has no PDFium glue,
#      no PDFium bridge globals and no PDFium URL to fetch. `tools/first-load.mjs` then weighs
#      each bundle's own closure, so PDFium cannot enter the base payload without the base
#      size budget failing by 400%.
#   3. This scan, over the built output, which is what catches a way of getting it wrong that
#      neither of those models.
#
# WHAT IT EXAMINES, reported on every run rather than implied: every shipped file under
# `apps/web/dist`, partitioned into the render bundle and everything else -- and it refuses
# outright if that directory is a HARNESS build, because then it is not what a person
# downloads. The variant is a stated, checked precondition rather than an assumption; see below.
#
# A POSITIVE CONTROL IS THE HALF THAT MAKES IT A CHECK, AND THERE ARE NOW TWO KINDS. The base
# rules are all "this string is absent", and a rule like that passes on an empty directory, a
# mistyped path, or a build that never ran -- so the same scan requires qpdf's markers to be
# PRESENT in the base set. The partition adds a second vacuity: a build that failed to stage
# PDFium AT ALL satisfies every absence rule too, and serves a dead thumbnail strip with
# nothing failing. So the render bundle is required to contain what the base set may not.
#
# Adversarial self-test: tools/test-check-pdfium-is-render-only.sh.

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

# --- THE PARTITION ----------------------------------------------------------------------
#
# The render bundle is the one file PDFium is allowed in. Everything else shipped is the base
# set: the other bundle, every page script, every page. Derived from the directory rather than
# listed, so a third bundle nobody told this script about lands in the base set and fails the
# absence rules -- which is the direction to be wrong in.
mapfile -t render_bundles < <(find "$dist/engines" -maxdepth 1 -name 'burrow-render-worker.*.js' 2>/dev/null | sort)
[ "${#render_bundles[@]}" -eq 1 ] ||
  fail "expected exactly 1 render bundle under engines/, found ${#render_bundles[@]} -- the \
partition this check rests on is not what it thinks it is"
render_bundle="${render_bundles[0]}"

# EXCLUDED BY PATTERN, NOT BY THE ONE RESOLVED PATH. If a second file matching the render
# bundle's shape ever appears, the count rule above is what should refuse -- and it cannot be
# what refuses if the extra copy has meanwhile landed in the base set and tripped the absence
# needles instead. Two rules catching one defect makes neither of them the one doing the work.
mapfile -t base_files < <(
  find "$dist" -type f \( -name '*.js' -o -name '*.mjs' -o -name '*.html' -o -name '*.txt' \) \
    ! -name 'burrow-render-worker.*.js' | sort
)
[ "${#base_files[@]}" -gt 0 ] || fail "no scannable files under $dist -- this would pass vacuously"

# WHAT WAS EXAMINED, BY KIND RATHER THAN AS A BARE TOTAL. `CLAUDE.md`: "4 of 15" reads exactly
# like success, and the expected number of shipped files is not derivable — so the next best
# thing is to say what they were, which is what makes a suspiciously small scan visible.
base_html="$(printf '%s\n' "${base_files[@]}" | grep -c '\.html$' || true)"
base_js="$(printf '%s\n' "${base_files[@]}" | grep -c '\.m\?js$' || true)"
base_other=$((${#base_files[@]} - base_html - base_js))
echo "check-pdfium-is-render-only: base set is ${#base_files[@]} file(s) \
(${base_html} page(s), ${base_js} script(s) incl. 1 bundle, ${base_other} other), \
plus 1 render bundle, under ${dist#"$repo"/}"

# --- the base rules ---------------------------------------------------------------------
#
# Each is a string that must NOT appear in any shipped file outside the render bundle, with
# what its presence would mean.
#
# COMMENTS ARE NOT STRIPPED, AND THE BUNDLE IS NOT MINIFIED. `stage-web-engines.mjs`
# CONCATENATES the worker sources, so every comment ships. That is kept as the rule rather
# than worked around: the scan stays exact, and prose in a shipped file that names an engine
# which is not there is itself worth removing. The first run of this check failed on three
# comments that named `FPDF_` while describing code that had already been removed.
#
# IT SURVIVES THE SHARED FILES, AND THAT WAS CHECKED RATHER THAN ASSUMED. `prelude.js` and
# `worker-protocol.js` are byte-identical in both bundles now, and their prose does discuss
# PDFium -- but it says "PDFium", never `FPDF_` or `__burrow_pdfium_`. The needles are symbol
# spellings for exactly this reason. `src/production-build.test.ts` checks the opposite
# direction (no qpdf CODE in the render bundle) and cannot stay exact, because those same
# shared comments spell `createQpdfModule`; it uses call shapes instead, and says so.
NEEDLES=(
  'FPDF_'
  '__burrow_pdfium_'
  'pdfiumWasm'
)
MEANS=(
  'a PDFium C entry point is called from code outside the render bundle'
  'a PDFium bridge global is defined or imported outside the render bundle'
  'a PDFium artifact is named in a manifest outside the render bundle'
)

# --- the positive controls --------------------------------------------------------------
#
# qpdf IS supposed to be in the base set. If these are missing, the scan is not looking at a
# build and every absence above means nothing.
# CALL SHAPES, NOT BARE NAMES. `prelude.js` is in BOTH bundles and its closing paragraph says
# "qpdf is MODULARIZE'd and takes its options from the object passed to `createQpdfModule()`",
# so a bare `createQpdfModule` cannot distinguish "the base bundle contains qpdf" from "the
# base bundle contains prelude.js". Found by security review. `src/production-build.test.ts`
# uses call shapes for the same reason, in the other direction.
PRESENT=(
  'createQpdfModule('
  'self.__burrow_qpdf_'
)

# --- the partition controls ---------------------------------------------------------------
#
# And PDFium IS supposed to be in the render bundle. Without this, a build that staged no
# PDFium at all passes every rule above -- and it would serve a page-picture strip that cannot
# render, with nothing failing.
RENDER_PRESENT=(
  'FPDF_'
  '__burrow_pdfium_'
  'pdfiumWasm'
)

for index in "${!NEEDLES[@]}"; do
  needle="${NEEDLES[$index]}"
  hits="$(grep -rlF -- "$needle" "${base_files[@]}" || true)"
  if [ -n "$hits" ]; then
    fail "${MEANS[$index]} ($needle): $(printf '%s' "$hits" | tr '\n' ' ')"
  fi
done
echo "  ${#NEEDLES[@]} absence rule(s) over the base set, none matched"

for marker in "${PRESENT[@]}"; do
  grep -rqF -- "$marker" "${base_files[@]}" ||
    fail "the positive control '$marker' is missing from the base set, so this scan was not \
looking at a build"
done
echo "  ${#PRESENT[@]} positive control(s) in the base set, all found"

for marker in "${RENDER_PRESENT[@]}"; do
  grep -qF -- "$marker" "$render_bundle" ||
    fail "the render bundle does not contain '$marker', so the partition is vacuous -- this \
build stages no usable PDFium and every absence rule above passes for the wrong reason"
done
echo "  ${#RENDER_PRESENT[@]} partition control(s) in the render bundle, all found"

# --- the base bundle cannot ASK for PDFium ------------------------------------------------
#
# The rule that decides the claim, rather than merely being consistent with it. Each bundle
# carries its own generated `BURROW_ENGINES` manifest, and that manifest is the only place a
# URL a worker fetches can come from -- `prelude.js` loops over it and Emscripten's own
# `locateFile` never runs. A base bundle whose manifest names no PDFium URL cannot fetch one,
# whatever else is true of the build.
base_bundles="$(find "$dist/engines" -maxdepth 1 -name 'burrow-worker.*.js' | wc -l)"
[ "$base_bundles" -eq 1 ] ||
  fail "expected exactly 1 base bundle under engines/, found $base_bundles"
base_bundle="$(find "$dist/engines" -maxdepth 1 -name 'burrow-worker.*.js' | head -1)"
! grep -qE '"url": "[^"]*pdfium' "$base_bundle" ||
  fail "the base bundle's own engine manifest names a PDFium URL, so it can fetch one"
grep -qE '"url": "[^"]*pdfium' "$render_bundle" ||
  fail "the render bundle's engine manifest names no PDFium URL, so it cannot fetch one"
echo "  the base bundle's manifest names no PDFium URL; the render bundle's names one"

# --- the artifacts themselves -------------------------------------------------------------
#
# The wasm modules are binary, so they are checked by name and then by content. `engines/` is
# what `connect-src` names and what the size budget weighs.
wasm_count="$(find "$dist/engines" -maxdepth 1 -name '*.wasm' | wc -l)"
[ "$wasm_count" -eq 4 ] ||
  fail "expected exactly 4 wasm modules (qpdf, PDFium, and one Rust module per bundle), found $wasm_count"

staged_pdfium="$(find "$dist/engines" -maxdepth 1 -name 'pdfium*' | wc -l)"
[ "$staged_pdfium" -eq 1 ] ||
  fail "expected exactly 1 PDFium artifact staged, found $staged_pdfium"

# AND BY CONTENT, NOT ONLY BY NAME. The rules above are about the file's NAME, so a PDFium
# module staged under any other name passes them and keeps the count at 4. `FPDF_` is an
# exported symbol name and survives into the binary; `qpdf.wasm` contains none, so this rule
# is not vacuous. `grep -a` because a `.wasm` is binary.
#
# THE RUST MODULES ARE CHECKED BY THEIR IMPORTS, WHICH IS A DIFFERENT NEEDLE AND WAS MISSING.
# ADR 0026 §2 calls the cargo-feature split "the half that makes the claim structural" — and
# nothing scanned it: neither Rust module contains `FPDF_` (they call PDFium through the
# bridge, never directly), and the base set was filtered to text files, so a `pkg/` built with
# the wrong features shipped a base module importing globals that do not exist and only a
# browser instantiation failure said so. Found by code review, which measured the needle:
#
#   burrow_wasm_bg.*.wasm         __burrow_qpdf_copy_in=1  __burrow_pdfium_copy_in=0
#   burrow_wasm_render_bg.*.wasm  __burrow_qpdf_copy_in=0  __burrow_pdfium_copy_in=1
#
# An import name is in the module's import section as a string, so `grep -a` reaches it. Each
# module is checked in BOTH directions, because "has the right engine" and "has only the right
# engine" are different claims and a stale output directory satisfies the first.
for module in "$dist/engines/"*.wasm; do
  case "$(basename "$module")" in
  pdfium.*)
    grep -qaF 'FPDF_' "$module" ||
      fail "the staged PDFium module exports no PDFium symbols: ${module#"$dist"/}"
    ;;
  burrow_wasm_render_bg.*)
    grep -qaF '__burrow_pdfium_copy_in' "$module" ||
      fail "the render Rust module imports no PDFium bridge global, so it was not built \
\`--features render\`: ${module#"$dist"/}"
    ! grep -qaF '__burrow_qpdf_copy_in' "$module" ||
      fail "the render Rust module imports a qpdf bridge global, so it was built with the \
wrong features: ${module#"$dist"/}"
    ! grep -qaF 'FPDF_' "$module" ||
      fail "a wasm module exports PDFium symbols under a non-PDFium name: ${module#"$dist"/}"
    ;;
  burrow_wasm_bg.*)
    grep -qaF '__burrow_qpdf_copy_in' "$module" ||
      fail "the base Rust module imports no qpdf bridge global, so it was not built with the \
default features: ${module#"$dist"/}"
    ! grep -qaF '__burrow_pdfium_copy_in' "$module" ||
      fail "the base Rust module imports a PDFium bridge global: a person who merges two \
files would download it, and it would fail at instantiation: ${module#"$dist"/}"
    ! grep -qaF 'FPDF_' "$module" ||
      fail "a wasm module exports PDFium symbols under a non-PDFium name: ${module#"$dist"/}"
    ;;
  *)
    ! grep -qaF 'FPDF_' "$module" ||
      fail "a wasm module exports PDFium symbols under a non-PDFium name: ${module#"$dist"/}"
    ;;
  esac
done
echo "  4 wasm module(s) staged: one PDFium by name and content, and the two Rust modules \
import one engine each"

# --- and the policy that governs fetching them ---------------------------------------------
#
# `connect-src` names the exact engine URLs (ADR 0014).
#
# THE POLICY IS THE ONE PLACE THE SPLIT IS NOT STRUCTURAL, and this check says so rather than
# implying otherwise. A CSP is per document, so the page that merges two files is served a
# policy that PERMITS fetching PDFium. What stops it is that nothing on that page asks: the
# base bundle's manifest has no such URL (asserted above) and `e2e/zero-requests.spec.ts`
# reads the server's own accept log rather than the policy. So what is checked here is that
# the policy names what the build actually stages -- no more and no fewer.
headers="$dist/_headers"
[ -f "$headers" ] || fail "no _headers in the build, so the policy cannot be checked"
# `{ grep … || true; }` INSIDE THE PIPELINE, because `set -o pipefail` makes a grep that
# matches nothing kill the script -- before the comparison that was supposed to report it.
# Found by this rule's own probe gate: with the count rule gutted the checker still exited 1,
# and the reason was that it never reached the rule at all. A build with no wasm URL in its
# policy would have died with no message rather than naming the count.
connect_wasm="$(grep -o 'connect-src[^;]*' "$headers" | { grep -o '\.wasm' || true; } | wc -l)"
[ "$connect_wasm" -eq 4 ] ||
  fail "connect-src names $connect_wasm wasm URLs, expected 4"
# `grep -o … | wc -l`, NOT `grep -oc`: `-c` overrides `-o` and counts matching LINES, and
# `connect-src` is one line — so two PDFium URLs would report 1 and this rule could not see
# them. Found by both reviews; masked today by the `-eq 4` rule above, which is why it is a
# consistency fix rather than a live hole. The line above already does it correctly.
connect_pdfium="$(grep -o 'connect-src[^;]*' "$headers" |
  { grep -o 'pdfium\.[0-9a-f]*\.wasm' || true; } | wc -l)"
[ "$connect_pdfium" -eq 1 ] ||
  fail "connect-src names $connect_pdfium PDFium URLs, expected exactly 1"
echo "  connect-src names 4 wasm URLs, exactly one of them PDFium"

echo "OK -- PDFium reaches the render bundle and no other part of the web build."
