#!/usr/bin/env bash
# Redaction's code reaches no part of the base web payload.
#
# ADR 0029's 2026-09-21 amendment: redaction's Rust is compiled into its OWN binding module,
# fetched on the first redaction and never by anything else, because the base module is what a
# person downloads to rotate a PDF and its margin cannot absorb a redaction engine. The amendment
# says a split nothing verifies closes quietly the first time someone imports across it, and names
# this check as the counterpart of `tools/check-pdfium-is-render-only.sh`.
#
# THE CLAIM IT CHECKS: the base Rust module (`burrow_wasm_bg.*.wasm`) and the base worker bundle
# (`burrow-worker.*.js`) contain none of redaction's own error spellings.
#
# WHY ERROR SPELLINGS. Every refusal and error redaction raises is built from a literal prefix --
# `pdf geometry [<rule>]`, `pdf redaction [<rule>]`, and so on -- and a literal survives into a
# wasm module's data section whenever the code that formats it is reachable. LTO strips code
# nothing calls, so a prefix present in the base module means redaction code is reachable from a
# base entry point. That is the defect this exists to catch: #191 moves redaction onto a handle
# trait with a web implementation, and the one import that lets the base binding reach it puts
# the whole policy on every tool page.
#
# WHAT IT CANNOT YET SHOW, stated because the PDFium check it copies CAN show it. That check has a
# real positive: the render bundle, which must contain PDFium. This one has none. Until #191
# lands there is no redaction module, and LTO strips redaction from every module that exists, so
# no artifact in a real build is known to contain these spellings. The positives are:
#   - each needle is checked, on every run, to still be spelled in redaction's Rust source, so a
#     renamed message cannot leave a needle that matches nothing;
#   - `tools/test-check-redaction-not-in-base.sh` plants each needle into a copy of a real build
#     and requires a refusal naming it, with near-misses that must not refuse.
# When #191 builds the redact module, it gains the real positive -- the redact module must contain
# them -- and this header should lose this paragraph.
#
# WHAT IT EXAMINES, reported on every run: exactly one base Rust module and exactly one base worker
# bundle, by name, and the needles with the source files that still spell each one.
#
# Adversarial self-test: tools/test-check-redaction-not-in-base.sh.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
dist="${1:-$repo/apps/web/dist}"
source_root="$repo/core/burrow-engines/src"

fail() {
  echo "::error::$*" >&2
  exit 1
}

[ -d "$dist" ] || fail "no build at $dist -- run \`pnpm build\` in apps/web first"

# THE PRODUCTION PAYLOAD, NOT A HARNESS BUILD. The claim is about what a person downloads, and a
# harness build carries routes that do not ship. `check-pdfium-is-render-only.sh` refuses the same
# way, for the reason its own header records: a verdict that depends on an unstated precondition is
# a gate somebody learns to re-run until it passes.
for variant in harness host; do
  [ -e "$dist/$variant" ] || continue
  fail "this is a HARNESS build ($dist/$variant exists), and the claim is about the \
production payload -- rebuild without BURROW_HARNESS (\`pnpm build\` in apps/web) and re-run"
done

# --- what is scanned --------------------------------------------------------------------------
mapfile -t modules < <(find "$dist/engines" -maxdepth 1 -name 'burrow_wasm_bg.*.wasm' 2>/dev/null | sort)
[ "${#modules[@]}" -eq 1 ] ||
  fail "expected exactly 1 base Rust module (burrow_wasm_bg.*.wasm) under engines/, found \
${#modules[@]}"
mapfile -t bundles < <(find "$dist/engines" -maxdepth 1 -name 'burrow-worker.*.js' 2>/dev/null | sort)
[ "${#bundles[@]}" -eq 1 ] ||
  fail "expected exactly 1 base worker bundle (burrow-worker.*.js) under engines/, found \
${#bundles[@]}"
base_module="${modules[0]}"
base_bundle="${bundles[0]}"

# --- the needles ------------------------------------------------------------------------------
#
# Each is a literal prefix redaction's Rust formats its errors from, with what its presence in the
# base payload would mean. NOT `pdf name [`: that is `pdfsyntax::names`, which split's pruning
# uses, so it belongs in the base module.
NEEDLES=(
  'pdf geometry ['
  'pdf redaction ['
  'pdf resources ['
  'pdf region ['
)
MEANS=(
  'the glyph geometry walk and its refusals are reachable from a base entry point'
  "the redaction steps' own errors are reachable from a base entry point"
  "redaction's font and resource resolver is reachable from a base entry point"
  "the redaction region's coordinate handling is reachable from a base entry point"
)

# THE NEEDLES ARE CHECKED AGAINST THE SOURCE, EVERY RUN. An absence rule whose needle nothing
# spells any more passes for ever. Counted by file, and every file is named, so a needle kept
# alive by one stray comment is visible.
for index in "${!NEEDLES[@]}"; do
  needle="${NEEDLES[$index]}"
  mapfile -t spelled < <(grep -rlF --include='*.rs' -- "\"$needle" "$source_root" | sort)
  [ "${#spelled[@]}" -gt 0 ] ||
    fail "the needle '$needle' is not spelled as a string literal anywhere under \
${source_root#"$repo"/}, so it can match nothing -- update it to what the code now emits"
  echo "  needle '$needle' is spelled in ${#spelled[@]} source file(s): \
$(printf '%s\n' "${spelled[@]}" | sed "s|^$source_root/||" | tr '\n' ' ')"
done

# --- the positive control ---------------------------------------------------------------------
#
# qpdf IS in the base payload. Without this, every absence rule passes on a truncated, empty or
# wrong file.
for artifact in "$base_module" "$base_bundle"; do
  grep -qaF -- '__burrow_qpdf_copy_in' "$artifact" ||
    fail "the positive control '__burrow_qpdf_copy_in' is missing from \
${artifact#"$dist"/}, so this scan is not looking at a base build"
done

# --- the rules --------------------------------------------------------------------------------
for index in "${!NEEDLES[@]}"; do
  needle="${NEEDLES[$index]}"
  for artifact in "$base_module" "$base_bundle"; do
    ! grep -qaF -- "$needle" "$artifact" ||
      fail "${MEANS[$index]} ('$needle' in ${artifact#"$dist"/}) -- ADR 0029's 2026-09-21 \
amendment puts redaction in its own module, never the base one"
  done
done

echo "check-redaction-not-in-base: ${#NEEDLES[@]} needle(s) absent from 2 artifact(s) -- \
${base_module#"$dist"/} and ${base_bundle#"$dist"/}; the qpdf positive control present in both."
echo "  NOT YET SHOWN ON A REAL MODULE: no redaction module exists until #191, so the only \
positives are the planted ones in tools/test-check-redaction-not-in-base.sh."
echo "OK -- redaction's code reaches no part of the base web payload."
