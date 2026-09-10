#!/usr/bin/env bash
# Assemble the servable directory for option 1. Same script locally and in CI.
set -euo pipefail
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(dirname "$here")"
www="$here/www"

rm -rf "$www"
mkdir -p "$www/pdfium" "$www/qpdf" "$www/corpus"

cp "$here/bridge.js" "$www/"
cp "$here/pkg/spike_option1.js" "$here/pkg/spike_option1_bg.wasm" "$www/"
cp "$root/vendor/pdfium/lib/pdfium.js" "$root/vendor/pdfium/lib/pdfium.wasm" "$www/pdfium/"
cp "$root/build/qpdf.js" "$root/build/qpdf.wasm" "$www/qpdf/"
cp "$root/common/corpus/"* "$www/corpus/"
cp "$here/index.html" "$here/worker.js" "$www/"

echo "assembled $www"
du -sh "$www"
