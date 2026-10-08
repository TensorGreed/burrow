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
#      default), `render`, `redact` -- and for `burrow-ffi` on each mobile target, `cargo tree`
#      must show `burrow-engines` WITHOUT its `fuzzing` feature. WHAT THIS BOUNDS, said exactly
#      (the review of #285): the declaration sits inside `qpdf`, which compiles only natively on
#      Linux, so on wasm32 it cannot exist whatever the feature says -- there the real bound is
#      qpdf.wasm's export allowlist (`engines/build-wasm.sh`, `check-wasm-exports.sh`), and the wasm
#      half of this check is defence in depth. The `burrow-ffi` half is the one that will bound
#      something: the day a mobile app links qpdf natively, the cfg widens, and a feature edge into
#      that graph would compile the accessor in. It covers burrow-ffi AT ITS DEFAULT FEATURES on
#      aarch64-linux-android and aarch64-apple-ios only -- not the emulator and simulator targets
#      (x86_64-linux-android, armv7-linux-androideabi, aarch64-apple-ios-sim) and not a feature
#      the app build might pass. Nothing in burrow-ffi's manifest varies by either today; the
#      day it does, they join the loop. A positive control runs every time: the same
#      query, asked of a wasm graph that does enable `fuzzing`, must see it, so a query that went
#      blind cannot read as a clean graph. Every query excludes dev-dependencies, so the control
#      resolves from the crates a wasm build already fetched (`--offline`).
#   2. THE ARTIFACTS. Every file under `dist/` must carry none of qpdf's four error-text function
#      names -- as an import, an export or any other string. A positive control: qpdf's
#      `qpdf_get_error_code`, which is exported, must be FOUND in `qpdf.*.wasm`, so a scan that went
#      blind (compressed artifacts, a moved tree) cannot read as clean. The four production wasm
#      modules (base, render, qpdf, pdfium) must each be present exactly once; a scan of fewer files
#      than ship is not a scan of the build (CLAUDE.md, "4 of 15 reads as success").
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
  out="$(cd "$repo" && cargo tree --offline -e features,no-dev -i burrow-engines "$@" 2>/dev/null)" || {
    echo "unresolvable"
    return
  }
  grep -c 'burrow-engines feature "fuzzing"' <<<"$out" || true
}

# THE POSITIVE CONTROL: a graph that does enable `fuzzing` must be seen to.
control="$(fuzzing_on -p burrow-engines --target wasm32-unknown-unknown --features fuzzing)"
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
for target in aarch64-linux-android aarch64-apple-ios; do
  found="$(fuzzing_on -p burrow-ffi --target "$target")"
  sets=$((sets + 1))
  if [ "$found" = "unresolvable" ]; then
    fail "the graph for burrow-ffi [$target] could not be resolved"
  elif [ "$found" -ne 0 ]; then
    fail "burrow-ffi [$target] enables burrow-engines' 'fuzzing' feature, which compiles qpdf's error text into a shipped build"
  fi
done

# --- 2. the artifacts ----------------------------------------------------------------------------
engines="$dist/engines"
examined=0
modules_found=()
seen_control=0
if [ ! -d "$engines" ]; then
  fail "no $engines -- build the web app first; a scan of nothing is not a scan"
else
  # THE LIST FIRST, AND ITS STATUS: `done < <(find …)` loses find's exit status, so a find that
  # failed part-way would have ended the loop early, a partial scan reading as a whole one.
  listing="$(mktemp)"
  if ! find "$dist" -type f -print0 >"$listing"; then
    fail "find over $dist failed; the scan would be partial"
  fi
  while IFS= read -r -d '' artifact; do
    examined=$((examined + 1))
    base="$(basename "$artifact")"
    for module in "${EXPECTED_WASM[@]}"; do
      case "$artifact" in "$engines/$module".*.wasm) modules_found+=("$module") ;; esac
    done
    case "$artifact" in
      "$engines"/qpdf.*.wasm) grep -aqF qpdf_get_error_code "$artifact" && seen_control=1 ;;
    esac
    for needle in "${NEEDLES[@]}"; do
      # THREE OUTCOMES, not two: grep's 2 (an unreadable file) is not "clean".
      set +e
      grep -aqF "$needle" "$artifact"
      found=$?
      set -e
      case "$found" in
        0) fail "$base carries '$needle': qpdf's error text is reachable from a shipped artifact" ;;
        1) ;;
        *) fail "could not read $base; a file the scan cannot read is not clean" ;;
      esac
    done
  done <"$listing"
  rm -f "$listing"
  if [ "$seen_control" -ne 1 ]; then
    fail "the scan cannot see qpdf_get_error_code in qpdf.*.wasm, which exports it; it is blind, so a clean scan means nothing"
  fi
  for module in "${EXPECTED_WASM[@]}"; do
    count=0
    for seen in "${modules_found[@]:-}"; do [ "$seen" = "$module" ] && count=$((count + 1)); done
    if [ "$count" -ne 1 ]; then
      fail "expected exactly one $module.*.wasm in $engines, found $count"
    fi
  done
fi

echo "check-no-error-text-in-deploy: $sets of 5 shipped graph(s) checked for 'fuzzing' (positive control: $control), $examined file(s) under dist scanned for ${#NEEDLES[@]} name(s) (positive control seen: $seen_control), ${#modules_found[@]} of ${#EXPECTED_WASM[@]} production wasm module(s) present"
if [ "$problems" -ne 0 ]; then
  echo "FAILED -- $problems problem(s)" >&2
  exit 1
fi
echo "OK -- no shipped build can read qpdf's error text."
