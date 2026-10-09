#!/usr/bin/env bash
# The committed standard-14 table is what the AFMs and the pinned PDFium produce today (#290, ADR 0030).
#
# The owner's condition 2 (2026-10-09): the table is generated, never edited by hand, and a check
# regenerates it and diffs it against the committed copy, so a PDFium bump that moves a bundled
# width goes red rather than stale. This is that check:
#
#   1. measure the pinned PDFium (core/burrow-engines/tests/standard14_measure.rs, written to a
#      temporary file) -- every AFM glyph name per style, every code 32..=126 per base encoding, and
#      the face it loaded;
#   2. generate the table from the AFMs and that measurement (tools/make-standard14-table.py);
#   3. diff against core/burrow-engines/src/pdfsyntax/standard14_table.rs.
#
# Needs the native engines (PDFium). Reports what it measured and how many names agree per style.
#
# Usage: tools/check-standard14-table.sh [--bless]   (--bless writes the regenerated table)
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/.." && pwd)"
committed="$repo/core/burrow-engines/src/pdfsyntax/standard14_table.rs"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

(cd "$repo" && BURROW_STANDARD14_MEASURE="$work/measure.json" cargo test --quiet -p burrow-engines \
  --features native-engines --test standard14_measure measure_into_a_file -- --nocapture --exact)
if [ ! -s "$work/measure.json" ]; then
  echo "FAILED -- the measurement wrote nothing; a table cannot be checked against no measurement" >&2
  exit 1
fi
python3 "$here/make-standard14-table.py" "$work/measure.json" --out "$work/standard14_table.rs"

# THE NOTICES TRAVEL WITH THE EXTRACT (ADR 0030, the license-auditor's condition): every AFM Notice
# line the table is drawn from must appear in THIRD_PARTY_NOTICES.md byte for byte, so the two
# cannot drift apart. A Rust comment does not survive into the wasm; this file does.
python3 - "$repo" <<'PY'
import pathlib, sys
repo = pathlib.Path(sys.argv[1])
notices = (repo / "THIRD_PARTY_NOTICES.md").read_text(encoding="utf-8")
styles = ["Courier", "Courier-Bold", "Courier-BoldOblique", "Courier-Oblique", "Helvetica",
          "Helvetica-Bold", "Helvetica-BoldOblique", "Helvetica-Oblique", "Times-Bold",
          "Times-BoldItalic", "Times-Italic", "Times-Roman"]
missing = []
for style in styles:
    afm = (repo / "third_party" / "adobe-core14-afm" / f"{style}.afm").read_text(encoding="latin-1")
    notice = next(l[len("Notice "):].strip() for l in afm.splitlines() if l.startswith("Notice "))
    # PER STYLE: several AFMs carry identical Notice text (Courier and Courier-Oblique), so the
    # text alone would let one style's line stand in for another's.
    if f"- {style}: `{notice}`" not in notices:
        missing.append(style)
print(f"check-standard14-table: {len(styles) - len(missing)} of {len(styles)} AFM Notice lines found verbatim in THIRD_PARTY_NOTICES.md")
if missing:
    print("FAILED -- THIRD_PARTY_NOTICES.md lacks the verbatim Notice line of: " + ", ".join(missing), file=sys.stderr)
    sys.exit(1)
PY

if [ "${1:-}" = "--bless" ]; then
  cp "$work/standard14_table.rs" "$committed"
  echo "check-standard14-table: wrote $committed"
  exit 0
fi
if ! diff -u "$committed" "$work/standard14_table.rs" >"$work/diff"; then
  head -60 "$work/diff" >&2
  echo "FAILED -- the committed standard-14 table is not what the AFMs and the pinned PDFium produce; if a PDFium or AFM change moved it on purpose, re-run with --bless and say why" >&2
  exit 1
fi
echo "OK -- the committed standard-14 table is the one the AFMs and the pinned PDFium generate."
