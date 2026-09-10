#!/usr/bin/env bash
# Fetch every engine artifact named in engines/pins.toml, verifying each against its
# pinned sha256 before it can be used.
#
# This is the ONLY place in the build that touches the network. build.rs reads the
# vendored tree and pins.toml; it never downloads.
#
# Fails closed:
#   - a checksum mismatch is an error, and the artifact is quarantined, not left in place
#   - a `TBD` pin is an error; the observed hash is printed so it can be committed
#     deliberately rather than trusted silently
#
# Idempotent: an artifact already present and matching its pin is left alone.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
vendor="$here/vendor"
pins="$here/pins.toml"

[ -f "$pins" ] || { echo "fetch: $pins not found" >&2; exit 1; }
command -v python3 >/dev/null || { echo "fetch: python3 required to read pins.toml" >&2; exit 1; }
command -v curl >/dev/null || { echo "fetch: curl required" >&2; exit 1; }

mkdir -p "$vendor"

# Emit one "name<TAB>url<TAB>sha256" line per artifact. Parsing TOML in bash is a bad
# idea; python3's tomllib is in the standard library and already required by
# tools/check-engine-licences.py.
manifest() {
  python3 - "$pins" <<'PY'
import sys, tomllib
with open(sys.argv[1], "rb") as fh:
    d = tomllib.load(fh)

p = d["pdfium"]
for name, a in p["artifacts"].items():
    print(f"pdfium-{name}\t{p['base_url']}/{a['file']}\t{a['sha256']}\t{a['file']}")

for key in ("qpdf", "zlib", "libjpeg-turbo"):
    s = d[key]
    print(f"{key}\t{s['url']}\t{s['sha256']}\t{s['url'].rsplit('/', 1)[-1]}")
PY
}

rc=0
tbd=0

while IFS=$'\t' read -r name url want file; do
  out="$vendor/$file"

  if [ ! -f "$out" ]; then
    echo "fetching $name -> $file"
    # --fail so an HTML error page never lands on disk as if it were the artifact.
    if ! curl -sSfL --max-time 600 -o "$out.part" "$url"; then
      echo "  !! $name: download failed" >&2
      rm -f "$out.part"
      rc=1
      continue
    fi
    mv "$out.part" "$out"
  fi

  got="$(sha256sum "$out" | cut -d' ' -f1)"

  if [ "$want" = "TBD" ]; then
    echo "  !! $name pin is TBD."
    echo "     observed sha256: $got"
    tbd=1
    rc=1
    continue
  fi

  if [ "$got" != "$want" ]; then
    echo "  !! $name CHECKSUM MISMATCH" >&2
    echo "     expected $want" >&2
    echo "     got      $got" >&2
    # Quarantine rather than delete: keep the evidence, but make it unusable as the
    # artifact so a later build cannot pick it up.
    mv "$out" "$out.REJECTED"
    echo "     moved to $out.REJECTED" >&2
    rc=1
    continue
  fi

  echo "  ok $name ($got)"
done < <(manifest)

if [ "$tbd" = 1 ]; then
  echo
  echo "One or more pins are TBD. Put the observed hashes in engines/pins.toml and"
  echo "re-run. Pins are committed deliberately, never filled in automatically."
fi

exit $rc
