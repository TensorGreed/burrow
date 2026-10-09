#!/usr/bin/env python3
"""Generate core/burrow-engines/src/pdfsyntax/standard14_table.rs (#290, ADR 0030).

THE TABLE IS GENERATED, NEVER EDITED (the owner's condition 2, 2026-10-09). It is the
INTERSECTION of two independent sources, measured per style:

- the published Adobe metrics: third_party/adobe-core14-afm/*.afm, committed unmodified;
- the pinned PDFium's own advances: a measurement file written by
  core/burrow-engines/tests/standard14_measure.rs (BURROW_STANDARD14_MEASURE=<file>).

A glyph name is accepted for a style only where its AFM width and PDFium's measured advance
(drawn through /Differences) are equal. A code under a base encoding is accepted only where the
encoding names a glyph accepted for that style AND PDFium's measured advance for the code under
that encoding equals the glyph's width -- so an error in the encoding map can only remove codes,
never admit a wrong width.

THE ENCODINGS, AND WHERE THEY COME FROM. Codes 32..=126 only, as before #290:
- StandardEncoding: each AFM's own `C` field, which is the font's StandardEncoding code. Every
  Latin AFM must agree on it, or this refuses to generate.
- WinAnsiEncoding and MacRomanEncoding: StandardEncoding over 32..=126 with code 39 `quotesingle`
  and code 96 `grave` -- the two codes where PDF 32000-1 Annex D's tables differ in that range.
  Codes past 126 under a base encoding are not tabulated and still refuse; a name past 126 is
  reached through /Differences, by name.

Symbol and ZapfDingbats carry their own built-in encodings and are not tabulated.

Usage:
  tools/make-standard14-table.py MEASUREMENT.json [--out PATH]   # write the table
  tools/make-standard14-table.py --bootstrap [--out PATH]         # an empty table, to compile the
                                                                  # measurement test the first time
Checked by tools/check-standard14-table.sh, which measures, generates and diffs.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
import tomllib
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
AFM_DIR = REPO / "third_party" / "adobe-core14-afm"
RECORD = REPO / "third_party" / "adobe-core14-afm.toml"
OUT = REPO / "core" / "burrow-engines" / "src" / "pdfsyntax" / "standard14_table.rs"

STYLES = [
    "Courier",
    "Courier-Bold",
    "Courier-BoldOblique",
    "Courier-Oblique",
    "Helvetica",
    "Helvetica-Bold",
    "Helvetica-BoldOblique",
    "Helvetica-Oblique",
    "Times-Bold",
    "Times-BoldItalic",
    "Times-Italic",
    "Times-Roman",
]
ENCODINGS = ["StandardEncoding", "WinAnsiEncoding", "MacRomanEncoding"]
FIRST, LAST = 32, 126
# The two codes in 32..=126 where WinAnsi and MacRoman name another glyph than Standard.
ANNEX_D_OVERRIDES = {39: "quotesingle", 96: "grave"}
NONE = 0xFFFF


def parse_afm(path: Path) -> tuple[list[tuple[int, str, int]], str, str]:
    """(code, name, width) per glyph, the Notice line, and the Comment Copyright line."""
    glyphs, notice, copyright_line = [], "", ""
    for line in path.read_text(encoding="latin-1").splitlines():
        if line.startswith("Notice "):
            notice = line[len("Notice "):].strip()
        elif line.startswith("Comment Copyright"):
            copyright_line = line[len("Comment "):].strip()
        elif line.startswith("C "):
            fields = {}
            for part in line.split(";"):
                part = part.strip()
                if " " in part:
                    key, value = part.split(" ", 1)
                    fields[key] = value.strip()
            glyphs.append((int(fields["C"]), fields["N"], int(fields["WX"])))
    if not glyphs or not notice:
        sys.exit(f"{path.name}: no glyphs or no Notice line; refusing to generate from it")
    return glyphs, notice, copyright_line


def notice_paragraph() -> str:
    html = (AFM_DIR / "MustRead.html").read_text(encoding="latin-1")
    text = re.sub(r"<[^>]+>", " ", html)
    text = re.sub(r"\s+", " ", text)
    start = text.index("This file and the 14 PostScript(R) AFM files")
    end = text.index("use of the AFM files.") + len("use of the AFM files.")
    return text[start:end]


def rust_bytes(name: str) -> str:
    return 'b"' + name.replace("\\", "\\\\").replace('"', '\\"') + '"'


def generate(measurement: dict | None) -> str:
    record = tomllib.loads(RECORD.read_text(encoding="utf-8"))
    afms = {style: parse_afm(AFM_DIR / f"{style}.afm") for style in STYLES}

    # STANDARD ENCODING from the AFMs' own C fields; every style must agree.
    standard: dict[int, str] = {}
    for style, (glyphs, _, _) in afms.items():
        for code, name, _ in glyphs:
            if FIRST <= code <= LAST:
                if standard.setdefault(code, name) != name:
                    sys.exit(f"{style}.afm names code {code} {name!r}, another AFM {standard[code]!r}")
    if sorted(standard) != list(range(FIRST, LAST + 1)):
        sys.exit("the AFMs do not name every code in 32..=126 under StandardEncoding")
    encodings = {
        "StandardEncoding": dict(standard),
        "WinAnsiEncoding": {**standard, **ANNEX_D_OVERRIDES},
        "MacRomanEncoding": {**standard, **ANNEX_D_OVERRIDES},
    }

    names = sorted({name for glyphs, _, _ in afms.values() for _, name, _ in glyphs})
    index = {name: i for i, name in enumerate(names)}

    faces, widths, accepted_codes = [], [], []
    agreeing, disagreeing = {}, {}
    for style in STYLES:
        glyphs, _, _ = afms[style]
        afm_width = {name: width for _, name, width in glyphs}
        row = [NONE] * len(names)
        if measurement is None:
            faces.append((style, "", ""))
        else:
            face = measurement["faces"][style]
            faces.append((style, face["family"], face["sha256"]))
            measured = measurement["names"][style]
            agreeing[style], disagreeing[style] = 0, []
            for name, width in afm_width.items():
                got = measured.get(name)
                if got is not None and abs(got - width) < 1e-6:
                    row[index[name]] = width
                    agreeing[style] += 1
                else:
                    disagreeing[style].append(name)
        widths.append(row)
        per_encoding = []
        for encoding in ENCODINGS:
            bits = [0, 0, 0, 0]
            if measurement is not None:
                measured_codes = measurement["codes"][style][encoding]
                for code in range(FIRST, LAST + 1):
                    name = encodings[encoding].get(code)
                    width = row[index[name]] if name in index else NONE
                    got = measured_codes.get(str(code))
                    if width != NONE and got is not None and abs(got - width) < 1e-6:
                        bits[code // 64] |= 1 << (code % 64)
            per_encoding.append(bits)
        accepted_codes.append(per_encoding)

    out = []
    w = out.append
    w("//! GENERATED by `tools/make-standard14-table.py`. DO NOT EDIT: regenerate with")
    w("//! `tools/check-standard14-table.sh --bless`, which `tools/check-standard14-table.sh` diffs.")
    w("//!")
    w("//! The standard-14 glyph widths burrow accepts, by NAME, per style (#290, ADR 0030): the")
    w("//! INTERSECTION of Adobe's published metrics and the pinned PDFium's measured advances. A")
    w("//! name absent here, or a code not marked accepted, refuses.")
    w("//!")
    w("//! # Source, and notice")
    w("//!")
    w("//! A MODIFIED EXTRACT of Adobe's Core 14 AFM files -- glyph names and advance widths only --")
    w("//! from `third_party/adobe-core14-afm/` (`Core14_AFMs.zip`, sha256")
    w(f"//! `{record['archive']['sha256']}`),")
    w("//! intersected with a measurement of the vendored PDFium")
    w("//! (`core/burrow-engines/tests/standard14_measure.rs`). The Notice of each AFM used, verbatim:")
    w("//!")
    for style in STYLES:
        line = f"//! - {style}:"
        for word in afms[style][1].split(" "):
            if len(line) + 1 + len(word) > 100:
                w(line)
                line = "//!  "
            line += " " + word
        w(line)
    w("//!")
    w("//! The notice that accompanies the AFM files, verbatim:")
    w("//!")
    paragraph = notice_paragraph()
    line = "//! >"
    for word in paragraph.split(" "):
        if len(line) + 1 + len(word) > 100:
            w(line)
            line = "//! >"
        line += " " + word
    w(line)
    w("")
    if measurement is None:
        w("// BOOTSTRAP: an empty table, written so the measurement test compiles the first time.")
        w("")
    w("/// Each style, the face the pinned PDFium drew it from: (style, family name, sha256 of the")
    w("/// font data). `tests/standard14_measure.rs` requires PDFium to load exactly these.")
    w(f"pub const FACES: [(&str, &str, &str); {len(faces)}] = [")
    for style, family, digest in faces:
        w(f'    ("{style}", "{family}", "{digest}"),')
    w("];")
    w("")
    w("/// The styles, in the order of [`WIDTHS`] and [`ACCEPTED_CODES`].")
    w(f"pub const STYLES: [&[u8]; {len(STYLES)}] = [")
    for style in STYLES:
        w(f"    {rust_bytes(style)},")
    w("];")
    w("")
    w("/// Every glyph name in the AFMs, sorted bytewise, for a binary search.")
    w(f"pub const NAMES: [&[u8]; {len(names)}] = [")
    for name in names:
        w(f"    {rust_bytes(name)},")
    w("];")
    w("")
    w("/// The marker for a name a style does not accept.")
    w(f"pub const NONE: u16 = {NONE};")
    w("")
    w("/// Per style, the width of each [`NAMES`] entry in glyph-space units, or [`NONE`].")
    w(f"pub const WIDTHS: [[u16; {len(names)}]; {len(STYLES)}] = [")
    for row in widths:
        w("    [")
        for start in range(0, len(row), 14):
            w("        " + " ".join(f"{v}," for v in row[start:start + 14]))
        w("    ],")
    w("];")
    w("")
    w("/// The first code a base encoding is tabulated for.")
    w(f"pub const FIRST_CODE: u32 = {FIRST};")
    w("/// The last code a base encoding is tabulated for.")
    w(f"pub const LAST_CODE: u32 = {LAST};")
    w("")
    w("/// The glyph each base encoding names at each code in [`FIRST_CODE`]..=[`LAST_CODE`], as an")
    w("/// index into [`NAMES`]: StandardEncoding, WinAnsiEncoding, MacRomanEncoding.")
    w(f"pub const ENCODINGS: [[u16; {LAST - FIRST + 1}]; 3] = [")
    for encoding in ENCODINGS:
        row = [index[encodings[encoding][code]] for code in range(FIRST, LAST + 1)]
        w("    [")
        for start in range(0, len(row), 14):
            w("        " + " ".join(f"{v}," for v in row[start:start + 14]))
        w("    ],")
    w("];")
    w("")
    w("/// Per style and base encoding (the order of [`ENCODINGS`]), a bit per code 0..=255: set where")
    w("/// PDFium's measured advance for the code equals the width of the glyph the encoding names.")
    w(f"pub const ACCEPTED_CODES: [[[u64; 4]; 3]; {len(STYLES)}] = [")
    for per_encoding in accepted_codes:
        w("    [")
        for bits in per_encoding:
            w("        [" + ", ".join(f"0x{b:016x}" for b in bits) + "],")
        w("    ],")
    w("];")
    w("")
    if measurement is not None:
        total = sum(agreeing.values())
        w(f"/// How many (style, name) widths agree, of {len(STYLES)} styles' AFM names: {total}.")
        w(f"pub const AGREEING: usize = {total};")
    else:
        w("/// How many (style, name) widths agree: none, in the bootstrap.")
        w("pub const AGREEING: usize = 0;")
    text = "\n".join(out) + "\n"
    if measurement is not None:
        report = ", ".join(f"{s} {agreeing[s]}/{agreeing[s] + len(disagreeing[s])}" for s in STYLES)
        print(f"make-standard14-table: names agreeing per style: {report}", file=sys.stderr)
    return text


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("measurement", nargs="?")
    parser.add_argument("--bootstrap", action="store_true")
    parser.add_argument("--out", default=str(OUT))
    args = parser.parse_args()
    if args.bootstrap == bool(args.measurement):
        parser.error("give a measurement file, or --bootstrap")
    measurement = None if args.bootstrap else json.loads(Path(args.measurement).read_text())
    Path(args.out).write_text(generate(measurement), encoding="utf-8")
    print(f"make-standard14-table: wrote {args.out}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
