#!/usr/bin/env bash
# No shipped build can read qpdf's error text (DECISIONS.md rule 13, #285).
#
# THE RULE THIS HOLDS. `core/burrow-engines/src/codes/qpdf.rs` rule 3: qpdf's error text quotes the
# file -- object numbers, byte offsets -- so burrow does not declare the functions that return it,
# and "a function that cannot be called cannot leak". On 2026-10-08 the owner allowed ONE exception:
# `qpdf_get_error_message_detail`, declared behind `burrow-engines`' `fuzzing` feature so the nightly
# can tell #285's qpdf invariant failure from burrow's own handle misuse. The exception is bounded by
# this check, on the build that ships:
#
#   1. THE FEATURE. For each of the three wasm feature sets the deploy builds -- `documents` (the
#      default), `render`, `redact` -- `cargo tree` must show `burrow-engines` WITHOUT its
#      `fuzzing` feature. A dependency edge that switched it on would compile the declaration into
#      a shipped module; this refuses that before anything is built from it. A positive control
#      runs every time: the same query, asked of a graph that does enable `fuzzing`, must see it,
#      so a query that went blind cannot read as a clean graph.
#   2. THE ARTIFACTS. Every shipped engine artifact in `dist/engines/` must carry none of qpdf's four
#      error-text function names -- as an import, an export or any other string. Measured against
#      the four production wasm modules by name (base, render, qpdf, pdfium) plus the worker
#      bundles; a build missing one of the four is refused, since a scan of fewer files than ship
#      is not a scan of the build (CLAUDE.md, "4 of 15 reads as success").
#
# Usage: tools/check-no-error-text-in-deploy.sh [dist-dir]   (default apps/web/dist)
# Self-test: tools/test-check-no-error-text-in-deploy.sh.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
dist="${1:-$repo/apps/web/dist}"

NEEDLES=(
  qpdf_get_error_message_detail
  qpdf_get_error_full_text
  qpdf_get_error_filename
  qpdf_get_error_file_position
)
# The production wasm modules, by the name before their content hash.
EXPECTED_WASM=(burrow_wasm_bg burrow_wasm_render_bg qpdf pdfium)

problems=0
fail() {
  echo "  FAIL $1" >&2
  problems=$((problems + 1))
}

# --- 1. the feature ------------------------------------------------------------------------------
fuzzing_on() {
  # `cargo tree -i burrow-engines -e features` prints `burrow-engines feature "fuzzing"` when the
  # feature is enabled in that graph. Read with grep -c so a zero count is a value, not a failed
  # pipeline (CLAUDE.md: never put a command whose status you need on the left of a pipe).
  local out
  out="$(cd "$repo" && cargo tree --offline -e features -i burrow-engines "$@" 2>/dev/null)" || {
    echo "unresolvable"
    return
  }
  grep -c 'burrow-engines feature "fuzzing"' <<<"$out" || true
}

# THE POSITIVE CONTROL: a graph that does enable `fuzzing` must be seen to.
control="$(fuzzing_on -p burrow-engines --features fuzzing)"
if [ "$control" = "unresolvable" ] || [ "$control" -lt 1 ]; then
  fail "the feature query cannot see 'fuzzing' even where it is enabled (got: $control); it is blind, so no deploy graph below can be judged"
fi

sets=0
for features in "" "--no-default-features --features render" "--no-default-features --features redact"; do
  # shellcheck disable=SC2086 # the feature flags are words on purpose
  found="$(fuzzing_on -p burrow-wasm --target wasm32-unknown-unknown $features)"
  sets=$((sets + 1))
  label="${features:-default (documents)}"
  if [ "$found" = "unresolvable" ]; then
    fail "the deploy graph for burrow-wasm [$label] could not be resolved"
  elif [ "$found" -ne 0 ]; then
    fail "burrow-wasm [$label] enables burrow-engines' 'fuzzing' feature, which compiles qpdf's error text into a shipped build"
  fi
done

# --- 2. the artifacts ----------------------------------------------------------------------------
engines="$dist/engines"
examined=0
modules_found=()
if [ ! -d "$engines" ]; then
  fail "no $engines -- build the web app first; a scan of nothing is not a scan"
else
  for artifact in "$engines"/*.wasm "$engines"/*.js; do
    [ -e "$artifact" ] || continue
    examined=$((examined + 1))
    base="$(basename "$artifact")"
    for module in "${EXPECTED_WASM[@]}"; do
      case "$base" in "$module".*.wasm) modules_found+=("$module") ;; esac
    done
    for needle in "${NEEDLES[@]}"; do
      if grep -aqF "$needle" "$artifact"; then
        fail "$base carries '$needle': qpdf's error text is reachable from a shipped artifact"
      fi
    done
  done
  for module in "${EXPECTED_WASM[@]}"; do
    count=0
    for seen in "${modules_found[@]:-}"; do [ "$seen" = "$module" ] && count=$((count + 1)); done
    if [ "$count" -ne 1 ]; then
      fail "expected exactly one $module.*.wasm in $engines, found $count"
    fi
  done
fi

echo "check-no-error-text-in-deploy: $sets of 3 deploy feature set(s) checked for 'fuzzing' (positive control: $control), $examined engine artifact(s) scanned for ${#NEEDLES[@]} name(s), ${#modules_found[@]} of ${#EXPECTED_WASM[@]} production wasm module(s) present"
if [ "$problems" -ne 0 ]; then
  echo "FAILED -- $problems problem(s)" >&2
  exit 1
fi
echo "OK -- no shipped build can read qpdf's error text."
