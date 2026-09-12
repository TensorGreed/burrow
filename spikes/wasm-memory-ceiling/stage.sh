#!/usr/bin/env bash
# Stage a copy of the wasm engine prefix with lowered memory maxima, for spike 0002.
#
# Writes engines/vendor/wasm-spike/lib/, which `tools/stage-web-engines.mjs` will read when
# BURROW_ENGINE_ARCH=wasm-spike. Everything except the two .wasm maxima is copied byte for
# byte, so the only variable in a spike run is the declared ceiling.
#
# A SEPARATE PREFIX, not an in-place patch, because engines/build-wasm.sh:54 does
# `rm -rf "$prefix"` on the real one -- an in-place rewrite would vanish on the next build and
# the run after that would silently measure stock engines.
#
# Usage: stage.sh <pdfium-mib> [qpdf-mib]
#        stage.sh 512        -> both engines at 512 MiB
#        stage.sh 512 2048   -> pdfium at 512, qpdf left at its declared maximum

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
src="$repo/engines/vendor/wasm/lib"
dst="$repo/engines/vendor/wasm-spike/lib"

pdfium_mib="${1:?usage: stage.sh <pdfium-mib> [qpdf-mib]}"
qpdf_mib="${2:-$pdfium_mib}"

[ -d "$src" ] || { echo "stage: $src not found; run engines/build-wasm.sh first" >&2; exit 1; }

rm -rf "$repo/engines/vendor/wasm-spike"
mkdir -p "$dst"
cp -a "$src/." "$dst/"

echo "staged from $src -> $dst"

patch_one() {
  local name="$1" mib="$2"
  local original="$src/$name" target="$dst/$name"
  local declared
  declared="$(node "$here/rewrite-max-memory.mjs" "$original" --report | awk '/maximum/ {print $2}')"
  if [ "$((mib * 1024 * 1024 / 65536))" -eq "$declared" ]; then
    echo "  $name: left at its declared maximum ($mib MiB)"
    return 0
  fi
  node "$here/rewrite-max-memory.mjs" "$target" --max-mib "$mib" --verify "$original" \
    | sed 's/^/  /'
}

patch_one pdfium.wasm "$pdfium_mib"
patch_one qpdf.wasm "$qpdf_mib"

# COVERAGE: say what is in the staged prefix and how it differs from the source, rather than
# just "done". A prefix that silently lost a file would otherwise look like a clean stage.
echo
echo "staged prefix contents, vs $src:"
identical=0
differing=0
# Every regular file, recursively -- `for f in "$src"/*` also yields directories, which `cmp`
# reports as differing, so the first version counted `cmake/` and `pkgconfig/` as modified and
# tripped its own guard. Compare files, and assert the file SET matches too.
while IFS= read -r rel; do
  if [ ! -f "$dst/$rel" ]; then
    echo "  MISSING  $rel" >&2
    exit 1
  fi
  if cmp -s "$src/$rel" "$dst/$rel"; then
    identical=$((identical + 1))
  else
    differing=$((differing + 1))
    echo "  differs  $rel ($(cmp -l "$src/$rel" "$dst/$rel" | wc -l) byte(s))"
  fi
done < <(cd "$src" && find . -type f -printf '%P\n' | sort)

src_count="$(cd "$src" && find . -type f | wc -l)"
dst_count="$(cd "$dst" && find . -type f | wc -l)"
if [ "$src_count" -ne "$dst_count" ]; then
  echo "  file count differs: source $src_count, staged $dst_count" >&2
  exit 1
fi
echo "  $identical file(s) byte-identical, $differing modified, $src_count examined"
[ "$differing" -le 2 ] || { echo "stage: more than the two .wasm files changed" >&2; exit 1; }

echo
echo "run the suite with:  BURROW_ENGINE_ARCH=wasm-spike pnpm -C apps/web exec playwright test"
