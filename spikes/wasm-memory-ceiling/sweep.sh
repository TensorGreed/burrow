#!/usr/bin/env bash
# Sweep the declared memory ceiling and record what changes, for spike 0002.
#
# ONE ENGINE AT A TIME. Patching both at once cannot attribute a change to either, and
# ADR 0015 §6 measured them behaving two orders of magnitude apart -- xref-bomb drives PDFium
# to 1900.7 MiB via page_count and qpdf to 513 MiB via structure_check. So each run lowers one
# engine and leaves the other at its declared 2048 MiB.
#
# THE VACUITY GUARD (spike measurement 0). A bomb that failed for some unrelated reason -- a
# pre-scan refusal, qpdf's own 256 MiB flate ceiling, a watchdog kill -- would read as success.
# Two things guard against it:
#
#   * `measure.spec.ts` prints per-case peak heap for both engines, captured here verbatim;
#   * the TRANSITION must track the patched value. PDFium's xref-bomb peak is 1900.7 MiB, so
#     if the patch is what binds, that case must succeed above ~1901 MiB and fail below it.
#     A failure that appears at every ceiling, or at a ceiling unrelated to the known peak,
#     is not evidence the patch bound anything.
#
# Usage: sweep.sh <engine> <mib> [<mib> ...]
#        sweep.sh pdfium 2048 1920 1856 1024 512 256 128
#        sweep.sh qpdf   2048 512 256 128
#
# Results land in results/<engine>-<mib>.txt, and a summary on stdout.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
web="$repo/apps/web"
out="$here/results"
mkdir -p "$out"

engine="${1:?usage: sweep.sh <pdfium|qpdf> <mib> [<mib> ...]}"
shift
case "$engine" in
  pdfium|qpdf) ;;
  *) echo "sweep: engine must be pdfium or qpdf" >&2; exit 1 ;;
esac

project="${BURROW_SWEEP_PROJECT:-chromium}"
specs="${BURROW_SWEEP_SPECS:-e2e/measure.spec.ts e2e/conformance.spec.ts}"

examined=0
for mib in "$@"; do
  if [ "$engine" = "pdfium" ]; then
    "$here/stage.sh" "$mib" 2048 >/dev/null
  else
    "$here/stage.sh" 2048 "$mib" >/dev/null
  fi

  log="$out/$engine-$mib.txt"
  {
    echo "# spike 0002 sweep: $engine ceiling $mib MiB, project $project"
    echo "# $(date -u +%Y-%m-%dT%H:%M:%SZ)"
    echo
  } >"$log"

  # No BURROW_SKIP_BUILD: the build IS the staging step, so skipping it would silently
  # measure the previous ceiling's engines.
  set +e
  ( cd "$web" && BURROW_ENGINE_ARCH=wasm-spike pnpm exec playwright test $specs --project="$project" ) \
    >>"$log" 2>&1
  status=$?
  set -e

  examined=$((examined + 1))
  printf '%-7s %5s MiB  exit=%d  ' "$engine" "$mib" "$status"

  # The two lines that carry the answer: peak heap, and any corpus outcome that moved.
  peak="$(grep -oE 'peak pdfium [0-9.]+ MiB, qpdf [0-9.]+ MiB' "$log" | head -1 || true)"
  diffs="$(grep -cE 'did not match the corpus' "$log" || true)"
  printf 'diffs=%-2s %s\n' "${diffs:-0}" "${peak:-<no heap line: measure.spec did not report>}"
done

echo
echo "swept $examined ceiling(s) for $engine on $project; logs in $out/"
[ "$examined" -eq "$#" ] || true
