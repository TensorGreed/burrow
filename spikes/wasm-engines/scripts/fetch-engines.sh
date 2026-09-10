#!/usr/bin/env bash
# Fetch the third-party engine artifacts this spike needs.
#
# Every download is pinned by version and verified against a sha256 in pins.env before
# it is used. On the first run a pin may be "TBD": the script then prints the observed
# checksum and FAILS, so the value gets committed deliberately rather than trusted
# silently. A later fetch returning different bytes fails the same way.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(dirname "$here")"
vendor="$root/vendor"
source "$here/pins.env"

mkdir -p "$vendor"

fetch() { # name url expected_sha
  local name="$1" url="$2" want="$3" out="$vendor/$1"
  if [ ! -f "$out" ]; then
    echo "fetching $name"
    curl -sSfL --max-time 300 -o "$out.part" "$url"
    mv "$out.part" "$out"
  fi
  local got
  got="$(sha256sum "$out" | cut -d' ' -f1)"
  if [ "$want" = "TBD" ]; then
    echo "  !! $name pin is TBD. Observed sha256:"
    echo "     $got"
    echo "     Put that in pins.env and re-run."
    return 1
  fi
  if [ "$got" != "$want" ]; then
    echo "  !! $name CHECKSUM MISMATCH" >&2
    echo "     expected $want" >&2
    echo "     got      $got" >&2
    return 1
  fi
  echo "  ok $name ($got)"
}

rc=0
fetch pdfium-wasm.tgz "$PDFIUM_WASM_URL" "$PDFIUM_WASM_SHA256" || rc=1
fetch "qpdf-$QPDF_VERSION.tar.gz" "$QPDF_URL" "$QPDF_SHA256" || rc=1
exit $rc
