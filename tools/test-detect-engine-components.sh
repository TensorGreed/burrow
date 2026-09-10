#!/usr/bin/env bash
# Test that detect-engine-components.py actually fails when a declaration is missing.
#
# A detector that silently passes is worse than no detector: it converts "nobody checked"
# into "something checked and it was fine". This project has shipped two such tools
# already (a format hook that found no formatter, a fetch script that verified nothing),
# so this one gets a test.
#
# Each case builds a temporary copy of engines/licenses.toml and points the tool at it
# with BURROW_ENGINE_LICENSES. The real manifest is never modified.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(dirname "$here")"
detector="$here/detect-engine-components.py"
manifest="$repo/engines/licenses.toml"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

pass=0
fail=0

check() { # description expected_exit manifest_file
  local desc="$1" want="$2" file="$3" got
  set +e
  BURROW_ENGINE_LICENSES="$file" python3 "$detector" >"$tmp/out" 2>"$tmp/err"
  got=$?
  set -e
  if [ "$got" = "$want" ]; then
    printf '  ok    %s (exit %s)\n' "$desc" "$got"
    pass=$((pass + 1))
  else
    printf '  FAIL  %s (expected exit %s, got %s)\n' "$desc" "$want" "$got"
    sed 's/^/          /' "$tmp/err" | head -12
    fail=$((fail + 1))
  fi
}

# Drop every [[component]] whose name matches a pattern.
drop_component() { # pattern outfile
  python3 - "$manifest" "$1" "$2" <<'PY'
import re, sys

src, pattern, dest = sys.argv[1], sys.argv[2].lower(), sys.argv[3]
text = open(src, encoding="utf-8").read()
# Split on the table header so whole entries can be dropped intact.
parts = re.split(r"(?m)^(\[\[component\]\])", text)
out = [parts[0]]
for header, body in zip(parts[1::2], parts[2::2]):
    name = re.search(r'(?m)^name\s*=\s*"([^"]*)"', body)
    if name and pattern in name.group(1).lower():
        continue
    out.append(header + body)
open(dest, "w", encoding="utf-8").write("".join(out))
PY
}

echo "detect-engine-components.py:"

# The baseline. If this does not pass, nothing below means anything.
check "unmodified manifest passes" 0 "$manifest"

# The case this tool exists for: HarfBuzz was linked with no declaration and no licence
# file in the artifact, and nothing noticed for a whole ADR cycle.
drop_component "harfbuzz" "$tmp/no-harfbuzz.toml"
check "removing harfbuzz is detected" 1 "$tmp/no-harfbuzz.toml"
grep -q "harfbuzz: found in" "$tmp/err" \
  && { echo "  ok    names harfbuzz in the failure"; pass=$((pass + 1)); } \
  || { echo "  FAIL  did not name harfbuzz in the failure"; fail=$((fail + 1)); }

# ICU: declared linked=false for a whole ADR cycle while 489 symbols sat in the binary.
drop_component "icu" "$tmp/no-icu.toml"
check "removing icu is detected" 1 "$tmp/no-icu.toml"

# A component with a mandatory credit line, so a missed declaration is a licence
# violation rather than untidy bookkeeping.
drop_component "freetype" "$tmp/no-freetype.toml"
check "removing freetype is detected" 1 "$tmp/no-freetype.toml"

# And one where the fingerprint is easy to get wrong: PDFium namespaces AGG as
# pdfium::agg, so a naive search for `agg::` misses it entirely.
drop_component "agg" "$tmp/no-agg.toml"
check "removing agg23 is detected" 1 "$tmp/no-agg.toml"

# A missing manifest must fail, not be treated as "nothing declared, nothing to check".
check "missing manifest fails" 1 "$tmp/does-not-exist.toml"

echo
echo "  $pass passed, $fail failed"
[ "$fail" = 0 ]
