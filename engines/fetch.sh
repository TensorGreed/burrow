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

def safe(name: str) -> str:
    """Reject a filename that could write outside engines/vendor/.

    pins.toml is reviewed, so this is only reachable via a PR that edits it -- but it
    turns "review this hash" into "review for arbitrary file overwrite", which is a much
    worse review task than it looks. `file = "../../../.bashrc"` would otherwise be
    honoured by the `mv` in the caller.
    """
    if "/" in name or name.startswith(".") or name in ("", ".."):
        raise SystemExit(f"pins.toml: unsafe artifact filename {name!r}")
    return name


p = d["pdfium"]
for name, a in p["artifacts"].items():
    print(f"pdfium-{name}\t{p['base_url']}/{a['file']}\t{a['sha256']}\t{safe(a['file'])}")

for key in ("qpdf", "zlib", "libjpeg-turbo"):
    s = d[key]
    print(f"{key}\t{s['url']}\t{s['sha256']}\t{safe(s['url'].rsplit('/', 1)[-1])}")
PY
}

# Materialise the manifest and CHECK ITS STATUS. A process substitution's exit code is
# not propagated and neither `set -e` nor `pipefail` covers it, so `done < <(manifest)`
# meant that any failure in the parser -- malformed TOML, a renamed section, no tomllib --
# printed a traceback and exited 0 with zero artifacts verified. For the "re-verify after
# a cache restore" step in CI that is a silent pass on unverified bytes.
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
manifest > "$tmp" || { echo "fetch: could not read $pins" >&2; exit 1; }
[ -s "$tmp" ] || { echo "fetch: $pins yielded no artifacts" >&2; exit 1; }
expected_count="$(wc -l < "$tmp")"

rc=0
tbd=0
verified=0

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
  verified=$((verified + 1))
done < "$tmp"

# Assert we actually processed everything, so a mid-loop `continue` on every artifact
# cannot be mistaken for a pass.
if [ "$rc" = 0 ] && [ "$verified" -ne "$expected_count" ]; then
  echo "fetch: verified $verified of $expected_count artifacts -- refusing to report success" >&2
  rc=1
fi

if [ "$rc" = 0 ]; then
  echo "  all $verified artifact(s) verified against $pins"
fi

if [ "$tbd" = 1 ]; then
  echo
  echo "One or more pins are TBD. Put the observed hashes in engines/pins.toml and"
  echo "re-run. Pins are committed deliberately, never filled in automatically."
fi

exit $rc
