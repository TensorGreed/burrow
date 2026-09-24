#!/usr/bin/env bash
# Redaction's code reaches neither of the Rust modules a tool page downloads.
#
# ADR 0029's 2026-09-21 amendment: redaction's Rust is compiled into its OWN binding module,
# fetched on the first redaction and never by anything else, because the base module is what a
# person downloads to rotate a PDF and its margin cannot absorb a redaction engine. It says a split
# nothing verifies closes quietly the first time someone imports across it, and names this check as
# the counterpart of `tools/check-pdfium-is-render-only.sh`.
#
# THE CLAIM IT CHECKS: every Rust wasm module in the build that is not the redaction module --
# today `burrow_wasm_bg` (base) and `burrow_wasm_render_bg` (render) -- carries none of the
# literals redaction's code formats its errors from.
#
# WHY LITERALS. A literal survives into a module's data section whenever the code that formats it
# is reachable, and LTO strips code nothing calls. So a needle present means redaction code is
# reachable from a shipped entry point. #191 moves redaction onto a handle trait with a web
# implementation, and one import from the base binding would put the whole policy on every page.
#
# THE NEEDLES ARE NOT ONLY REFUSALS. The first version looked only for refusal prefixes, and a code
# review built real modules reaching the rewriter's splice (+7.7 KB), §1's font surgery (+25 KB) and
# the string encoder (+5.5 KB): none formats a refusal, and the check passed all three. So the
# needles now cover each component the 2026-09-21 amendment names -- geometry, the splice, the
# `/ToUnicode` narrowing, strings, the region, the steps and the verification -- by a literal only
# that component spells.
#
# WHAT IT DOES NOT SCAN, stated because the first version's OK line claimed "no part of the base
# web payload" after reading two files:
#   - The JavaScript. Rust error strings live in the wasm, never in a bundle, so a bundle scan for
#     these needles could not fire. A redaction reaching the base BUNDLE would arrive as a redact
#     worker or glue imported into it, named `burrow-redact-worker` / `burrow_wasm_redact`, and those
#     names do not exist until #137 builds them; `src/production-build.test.ts` meanwhile holds the
#     `redact` op out of every shipped script.
#   - `qpdf.wasm` and `pdfium.*.wasm`, which are C++ and legitimately spell strings like
#     `/ToUnicode`.
#
# ITS POSITIVES. The PDFium check has a shipped positive (the render bundle); this one has none,
# because no redaction module exists until #191. What it has instead, and what each covers:
#   - every needle is checked on every run to still be spelled in redaction's Rust source, outside
#     comments, so a renamed message cannot leave a needle that matches nothing;
#   - `tools/test-check-redaction-not-in-base.sh` BUILDS a real base module with a probe export
#     (`__burrow_redaction_probe`, behind `--cfg burrow_redaction_probe`) and requires this check to
#     refuse it -- a real compiler, real LTO, real literals -- for every needle whose code compiles
#     for wasm today; and it plants the rest, which are native-only until #191, into a copy.
#
# `--print-needles` prints the needles, one per line, and exits: the self-test reads the list from
# here rather than parsing this file.
#
# Adversarial self-test: tools/test-check-redaction-not-in-base.sh.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
source_root="$repo/core/burrow-engines/src"

fail() {
  echo "::error::$*" >&2
  exit 1
}

# --- the needles ------------------------------------------------------------------------------
#
# Each is a literal only that redaction component spells, with what its presence would mean. NOT
# `pdf name [` or a bare `pdf syntax:`: `pdfsyntax::names` and the lexer are split's pruning too,
# and 29 `pdf syntax:` literals are in the base module legitimately.
NEEDLES=(
  'pdf geometry ['
  'content-stream edit'
  'a /ToUnicode'
  'a string token that begins with neither'
  'pdf region ['
  'pdf redaction ['
  'pdf resources ['
  'redact: the region is not cleared'
)
MEANS=(
  'the glyph geometry walk is reachable'
  "the rewriter's content-stream splice is reachable"
  "the font surgery's /ToUnicode narrowing is reachable"
  "the rewriter's string decoding is reachable"
  "the redaction region's coordinate handling is reachable"
  'the redaction steps are reachable'
  "redaction's font and resource resolver is reachable"
  "redaction's output verification is reachable"
)
[ "${#NEEDLES[@]}" -eq "${#MEANS[@]}" ] ||
  fail "NEEDLES has ${#NEEDLES[@]} entries and MEANS has ${#MEANS[@]}; each needle needs its meaning"

if [ "${1:-}" = "--print-needles" ]; then
  printf '%s\n' "${NEEDLES[@]}"
  exit 0
fi

dist="${1:-$repo/apps/web/dist}"
[ -d "$dist" ] || fail "no build at $dist -- run \`pnpm build\` in apps/web first"

# THE PRODUCTION PAYLOAD, NOT A HARNESS BUILD, for the reason `check-pdfium-is-render-only.sh`
# records: a verdict that depends on an unstated precondition is a gate somebody learns to re-run.
for variant in harness host; do
  [ -e "$dist/$variant" ] || continue
  fail "this is a HARNESS build ($dist/$variant exists), and the claim is about the \
production payload -- rebuild without BURROW_HARNESS (\`pnpm build\` in apps/web) and re-run"
done

# --- what is scanned --------------------------------------------------------------------------
#
# Every Rust module but the redaction one, derived from the directory: a fourth module nobody told
# this script about is scanned rather than skipped, and the count rule below then names it.
mapfile -t modules < <(
  find "$dist/engines" -maxdepth 1 -name 'burrow_wasm_*.wasm' ! -name 'burrow_wasm_redact_*' \
    2>/dev/null | sort
)
[ "${#modules[@]}" -eq 2 ] ||
  fail "expected exactly 2 non-redaction Rust modules (base and render) under engines/, found \
${#modules[@]}: $(printf '%s ' "${modules[@]##*/}")"

# --- the needles still mean something ---------------------------------------------------------
#
# Spelled in a non-comment line of the engines' Rust source. Counted by file, and every file named,
# so a needle kept alive by one stray line is visible.
for index in "${!NEEDLES[@]}"; do
  needle="${NEEDLES[$index]}"
  mapfile -t spelled < <(
    grep -rnF --include='*.rs' -- "$needle" "$source_root" |
      { grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' || true; } | cut -d: -f1 | sort -u
  )
  [ "${#spelled[@]}" -gt 0 ] ||
    fail "the needle '$needle' is not spelled outside a comment anywhere under \
${source_root#"$repo"/}, so it can match nothing -- update it to what the code now emits"
  echo "  needle '$needle': spelled in $(printf '%s\n' "${spelled[@]}" | sed "s|^$source_root/||" | tr '\n' ' ')"
done

# --- the positive controls ----------------------------------------------------------------------
#
# Each module imports its own engine's bridge. Without this, every absence rule passes on a
# truncated, empty or wrong file.
for module in "${modules[@]}"; do
  case "$(basename "$module")" in
  burrow_wasm_render_bg.*) control='__burrow_pdfium_copy_in' ;;
  *) control='__burrow_qpdf_copy_in' ;;
  esac
  grep -qaF -- "$control" "$module" ||
    fail "the positive control '$control' is missing from ${module#"$dist"/}, so this scan is \
not looking at a built module"
done

# --- the rules --------------------------------------------------------------------------------
for index in "${!NEEDLES[@]}"; do
  needle="${NEEDLES[$index]}"
  for module in "${modules[@]}"; do
    ! grep -qaF -- "$needle" "$module" ||
      fail "${MEANS[$index]} from a shipped module ('$needle' in ${module#"$dist"/}) -- ADR \
0029's 2026-09-21 amendment puts redaction in its own module, never the base or render one"
  done
done

# AND THE PROBE NEVER SHIPS. `__burrow_redaction_probe` exists to be this check's built positive and
# is compiled only under `--cfg burrow_redaction_probe`; in a shipped module it is a redaction entry
# point on every tool page.
for module in "${modules[@]}"; do
  ! grep -qaF -- '__burrow_redaction_probe' "$module" ||
    fail "the redaction probe export shipped in ${module#"$dist"/} -- it is built only under \
--cfg burrow_redaction_probe, and a shipped build must never set it"
done

echo "check-redaction-not-in-base: ${#NEEDLES[@]} needle(s) absent from ${#modules[@]} Rust \
module(s) -- $(printf '%s ' "${modules[@]##*/}")-- each module's own engine control present."
echo "  Scanned: those modules only. Not the JavaScript (no Rust literal reaches it) and not the \
C++ engines. No SHIPPED module is a positive yet; the self-test builds one."
echo "OK -- redaction's code reaches neither of the Rust modules a tool page downloads."
