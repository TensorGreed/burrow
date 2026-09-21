#!/usr/bin/env python3
"""A minimal, deterministic TrueType font builder, for spike 0006's fixtures only.

WHY THIS EXISTS

Spike 0006 asks whether a scan can see text that is present only in transformed form. The
sharpest cases -- a subset font with a private encoding, a CID font with no `/ToUnicode`, and a
CID font whose `/ToUnicode` LIES -- cannot be built with a base-14 font, because the whole
point is that the glyph codes in the content stream are not the characters, and the mapping
back lives somewhere a byte scan does not look.

`tools/make-compress-fixtures.py` solves its own font problem by carrying "a real woff2 face's
bytes" and says, in its own words, that it is "**Not a real PDF font program** -- it is a
stand-in for one". A stand-in cannot work here: the fixture has to RENDER, because one of the
two verdicts this spike records per channel is "would a person looking at the page see the
secret". A font nothing can rasterise measures nothing on that column.

So the glyphs are generated. That also disposes of the licence question -- `CLAUDE.md`'s second
non-negotiable -- without a notices entry or an `add-dependency` pass, because these outlines
are drawn here and are not anybody's typeface.

WHAT IT CAN AND CANNOT PRODUCE, stated rather than discovered

- It produces a 5x7 block face: every glyph is a set of axis-aligned 100x100 unit squares, one
  per lit pixel, wound clockwise in a y-up space so nonzero winding unions them. Legible, and
  deliberately nothing more.
- It covers `A-Z`, `0-9`, `-` and space, and REFUSES any other character rather than
  substituting a blank. A silently-blank glyph would make a fixture that cannot fail the test.
- It emits `head, hhea, maxp, hmtx, cmap, loca, glyf, name, post, OS/2`. `post` is format 2.0
  carrying real glyph names, because channel 4 resolves text through `/Differences` glyph names
  and a format 3.0 `post` would remove the only route PDFium has to that mapping -- which would
  make channel 4's result an artefact of this builder rather than a finding about PDFs.
- It does NOT hint, kern, or compose. There is no `GSUB`/`GPOS`, no `gasp`, no `cvt`.
- Glyph order is the order characters are requested in, so the caller controls the GID of every
  character. Identity-H fixtures depend on that and assert it.

Not production code. Nothing here is linked, shipped, or depended on.
"""

from __future__ import annotations

import struct
from typing import Iterable

UNITS_PER_EM = 1000
PIXEL = 100  # each 5x7 cell is 100x100 units
GLYPH_ADVANCE = 600  # 5 columns plus one column of side bearing
ASCENT = 800
DESCENT = -200

# ---------------------------------------------------------------------------
# The 5x7 face. Each entry is seven rows of five bits, top row first.
# ---------------------------------------------------------------------------

BITMAP: dict[str, tuple[str, ...]] = {
    " ": ("00000", "00000", "00000", "00000", "00000", "00000", "00000"),
    "-": ("00000", "00000", "00000", "11111", "00000", "00000", "00000"),
    "A": ("01110", "10001", "10001", "11111", "10001", "10001", "10001"),
    "B": ("11110", "10001", "10001", "11110", "10001", "10001", "11110"),
    "C": ("01110", "10001", "10000", "10000", "10000", "10001", "01110"),
    "D": ("11110", "10001", "10001", "10001", "10001", "10001", "11110"),
    "E": ("11111", "10000", "10000", "11110", "10000", "10000", "11111"),
    "F": ("11111", "10000", "10000", "11110", "10000", "10000", "10000"),
    "G": ("01110", "10001", "10000", "10111", "10001", "10001", "01111"),
    "H": ("10001", "10001", "10001", "11111", "10001", "10001", "10001"),
    "I": ("11111", "00100", "00100", "00100", "00100", "00100", "11111"),
    "J": ("00111", "00010", "00010", "00010", "00010", "10010", "01100"),
    "K": ("10001", "10010", "10100", "11000", "10100", "10010", "10001"),
    "L": ("10000", "10000", "10000", "10000", "10000", "10000", "11111"),
    "M": ("10001", "11011", "10101", "10101", "10001", "10001", "10001"),
    "N": ("10001", "11001", "10101", "10011", "10001", "10001", "10001"),
    "O": ("01110", "10001", "10001", "10001", "10001", "10001", "01110"),
    "P": ("11110", "10001", "10001", "11110", "10000", "10000", "10000"),
    "Q": ("01110", "10001", "10001", "10001", "10101", "10010", "01101"),
    "R": ("11110", "10001", "10001", "11110", "10100", "10010", "10001"),
    "S": ("01111", "10000", "10000", "01110", "00001", "00001", "11110"),
    "T": ("11111", "00100", "00100", "00100", "00100", "00100", "00100"),
    "U": ("10001", "10001", "10001", "10001", "10001", "10001", "01110"),
    "V": ("10001", "10001", "10001", "10001", "10001", "01010", "00100"),
    "W": ("10001", "10001", "10001", "10101", "10101", "11011", "10001"),
    "X": ("10001", "10001", "01010", "00100", "01010", "10001", "10001"),
    "Y": ("10001", "10001", "01010", "00100", "00100", "00100", "00100"),
    "Z": ("11111", "00001", "00010", "00100", "01000", "10000", "11111"),
    "0": ("01110", "10001", "10011", "10101", "11001", "10001", "01110"),
    "1": ("00100", "01100", "00100", "00100", "00100", "00100", "01110"),
    "2": ("01110", "10001", "00001", "00010", "00100", "01000", "11111"),
    "3": ("11111", "00010", "00100", "00010", "00001", "10001", "01110"),
    "4": ("00010", "00110", "01010", "10010", "11111", "00010", "00010"),
    "5": ("11111", "10000", "11110", "00001", "00001", "10001", "01110"),
    "6": ("00110", "01000", "10000", "11110", "10001", "10001", "01110"),
    "7": ("11111", "00001", "00010", "00100", "01000", "01000", "01000"),
    "8": ("01110", "10001", "10001", "01110", "10001", "10001", "01110"),
    "9": ("01110", "10001", "10001", "01111", "00001", "00010", "01100"),
}

# Glyph names for `post` 2.0 and for `/Differences`. Standard Adobe names, so a reader that
# knows the AGL can map them back -- which is exactly the route channel 4 is testing.
GLYPH_NAME = {
    " ": "space",
    "-": "hyphen",
    **{c: c for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZ"},
    **{
        d: n
        for d, n in zip(
            "0123456789",
            "zero one two three four five six seven eight nine".split(),
        )
    },
}

# `post` format 2.0 indexes the 258 standard Macintosh names before any custom ones. Using the
# standard index where it exists keeps the table small and keeps the names canonical.
MAC_GLYPH_ORDER = (
    ".notdef .null nonmarkingreturn space exclam quotedbl numbersign dollar percent "
    "ampersand quotesingle parenleft parenright asterisk plus comma hyphen period slash "
    "zero one two three four five six seven eight nine colon semicolon less equal greater "
    "question at A B C D E F G H I J K L M N O P Q R S T U V W X Y Z"
).split()


class UnsupportedCharacter(Exception):
    """A character with no glyph. Refused rather than replaced -- see the module docstring."""


def _contours_for(ch: str) -> list[list[tuple[int, int]]]:
    """Every lit pixel as one clockwise square, in a y-up em space."""
    rows = BITMAP[ch]
    contours: list[list[tuple[int, int]]] = []
    for r, row in enumerate(rows):
        for c, bit in enumerate(row):
            if bit != "1":
                continue
            x0 = c * PIXEL + PIXEL // 2  # half a pixel of left side bearing
            y0 = (6 - r) * PIXEL
            x1, y1 = x0 + PIXEL, y0 + PIXEL
            # Clockwise with y up: top-left, top-right, bottom-right, bottom-left.
            contours.append([(x0, y1), (x1, y1), (x1, y0), (x0, y0)])
    return contours


def _glyf_entry(contours: list[list[tuple[int, int]]]) -> bytes:
    if not contours:
        return b""  # an empty glyph is a zero-length `loca` run, which is legal
    points = [p for contour in contours for p in contour]
    xs = [p[0] for p in points]
    ys = [p[1] for p in points]
    out = struct.pack(">hhhhh", len(contours), min(xs), min(ys), max(xs), max(ys))
    end = -1
    ends = []
    for contour in contours:
        end += len(contour)
        ends.append(end)
    out += b"".join(struct.pack(">H", e) for e in ends)
    out += struct.pack(">H", 0)  # instructionLength
    # Every point is on-curve and every delta is a signed 16-bit value: flag 0x01 alone.
    out += bytes([0x01]) * len(points)
    prev = 0
    for x in xs:
        out += struct.pack(">h", x - prev)
        prev = x
    prev = 0
    for y in ys:
        out += struct.pack(">h", y - prev)
        prev = y
    if len(out) % 4:
        out += b"\0" * (4 - len(out) % 4)
    return out


def _checksum(table: bytes) -> int:
    padded = table + b"\0" * ((4 - len(table) % 4) % 4)
    total = 0
    for i in range(0, len(padded), 4):
        total += struct.unpack(">I", padded[i : i + 4])[0]
    return total & 0xFFFFFFFF


def build(characters: str) -> tuple[bytes, dict[str, int]]:
    """A TrueType font covering `characters`, plus the character to glyph-id map.

    Glyph 0 is `.notdef` (empty). The rest follow `characters` in order, so the caller can
    predict every glyph id -- which the Identity-H fixtures rely on and assert.

    Raises `UnsupportedCharacter` for anything the face does not draw.
    """
    for ch in characters:
        if ch not in BITMAP:
            raise UnsupportedCharacter(f"blockfont has no glyph for {ch!r}")
    if len(set(characters)) != len(characters):
        raise ValueError("characters must be unique; glyph ids are positional")

    glyph_chars = [None] + list(characters)
    gids = {ch: i + 1 for i, ch in enumerate(characters)}

    glyf_parts: list[bytes] = []
    for ch in glyph_chars:
        glyf_parts.append(b"" if ch is None else _glyf_entry(_contours_for(ch)))
    loca_offsets = [0]
    for part in glyf_parts:
        loca_offsets.append(loca_offsets[-1] + len(part))
    glyf = b"".join(glyf_parts)
    loca = b"".join(struct.pack(">I", o) for o in loca_offsets)

    n = len(glyph_chars)
    all_points = [
        p
        for ch in characters
        for contour in _contours_for(ch)
        for p in contour
    ]
    x_min = min((p[0] for p in all_points), default=0)
    y_min = min((p[1] for p in all_points), default=0)
    x_max = max((p[0] for p in all_points), default=0)
    y_max = max((p[1] for p in all_points), default=0)
    max_contours = max((len(_contours_for(ch)) for ch in characters), default=0)
    max_points = max_contours * 4

    head = struct.pack(
        ">IIIIHHQQhhhhHHhhh",
        0x00010000,  # version
        0x00010000,  # fontRevision
        0,  # checkSumAdjustment, patched below
        0x5F0F3CF5,  # magicNumber
        0x000B,  # flags
        UNITS_PER_EM,
        0,  # created  -- a constant, so the font is byte-identical on every run
        0,  # modified
        x_min,
        y_min,
        x_max,
        y_max,
        0,  # macStyle
        8,  # lowestRecPPEM
        2,  # fontDirectionHint
        1,  # indexToLocFormat: long
        0,  # glyphDataFormat
    )

    hhea = struct.pack(
        ">IhhhHhhhhhhhhhhhH",
        0x00010000,
        ASCENT,
        DESCENT,
        0,  # lineGap
        GLYPH_ADVANCE,  # advanceWidthMax
        0,  # minLeftSideBearing
        0,  # minRightSideBearing
        x_max,  # xMaxExtent
        1,  # caretSlopeRise
        0,
        0,
        0,
        0,
        0,
        0,
        0,  # metricDataFormat
        n,  # numberOfHMetrics
    )

    maxp = struct.pack(
        ">IHHHHHHHHHHHHHH",
        0x00010000,
        n,
        max_points,
        max_contours,
        0,
        0,
        2,  # maxZones
        0,
        0,
        0,
        0,
        0,
        0,
        0,
        0,
    )

    hmtx = b"".join(struct.pack(">Hh", GLYPH_ADVANCE, 0) for _ in glyph_chars)

    # cmap: one format 4 subtable under (3, 1). Identity-H fixtures never consult it; the
    # simple-font fixtures do.
    codes = sorted(ord(ch) for ch in characters)
    # ONE SEGMENT PER CODE, deliberately. Glyph ids are positional in `characters`, so a
    # contiguous run of character codes does not imply a contiguous run of glyph ids, and a
    # merged segment with a single `idDelta` would map most of its codes to the wrong glyph --
    # a fixture that draws the wrong letters and still looks like it worked.
    seg_entries = [(c, c) for c in codes] + [(0xFFFF, 0xFFFF)]
    seg_count = len(seg_entries)
    end_codes = b"".join(struct.pack(">H", e) for _, e in seg_entries)
    start_codes = b"".join(struct.pack(">H", s) for s, _ in seg_entries)
    id_deltas = b""
    for s, e in seg_entries:
        if s == 0xFFFF:
            id_deltas += struct.pack(">h", 1)
        else:
            delta = (gids[chr(s)] - s) & 0xFFFF
            id_deltas += struct.pack(">H", delta)
    id_range = b"".join(struct.pack(">H", 0) for _ in seg_entries)
    search_range = 2
    entry_selector = 0
    while search_range * 2 <= seg_count * 2:
        search_range *= 2
        entry_selector += 1
    sub = (
        struct.pack(
            ">HHHHHHH",
            4,
            16 + 8 * seg_count,
            0,
            seg_count * 2,
            search_range,
            entry_selector,
            seg_count * 2 - search_range,
        )
        + end_codes
        + struct.pack(">H", 0)
        + start_codes
        + id_deltas
        + id_range
    )
    cmap = struct.pack(">HHHHI", 0, 1, 3, 1, 12) + sub

    # post 2.0 with real names: the only route from a `/Differences` glyph name to a character.
    indices = []
    extra: list[bytes] = []
    for ch in glyph_chars:
        name = ".notdef" if ch is None else GLYPH_NAME[ch]
        if name in MAC_GLYPH_ORDER:
            indices.append(MAC_GLYPH_ORDER.index(name))
        else:
            indices.append(258 + len(extra))
            extra.append(bytes([len(name)]) + name.encode("ascii"))
    post = (
        struct.pack(">IIhhIIIII", 0x00020000, 0, 0, 0, 0, 0, 0, 0, 0)
        + struct.pack(">H", n)
        + b"".join(struct.pack(">H", i) for i in indices)
        + b"".join(extra)
    )

    # OS/2 version 4, built field by field rather than through one long format string. The
    # format-string version of this was written first and was silently one field short, which
    # `check_font` below would have reported as a length mismatch and a reader would have
    # reported as a font it declined to load -- two very different-looking symptoms of the
    # same slip.
    os2 = b"".join(
        [
            struct.pack(">H", 4),  # version
            struct.pack(">h", GLYPH_ADVANCE),  # xAvgCharWidth
            struct.pack(">H", 400),  # usWeightClass
            struct.pack(">H", 5),  # usWidthClass
            struct.pack(">H", 0),  # fsType: installable embedding
            struct.pack(">hhhh", 650, 700, 0, 140),  # ySubscript{XSize,YSize,XOffset,YOffset}
            struct.pack(">hhhh", 650, 700, 0, 480),  # ySuperscript*
            struct.pack(">hh", 50, 260),  # yStrikeout{Size,Position}
            struct.pack(">h", 0),  # sFamilyClass
            b"\0" * 10,  # panose
            struct.pack(">IIII", 0, 0, 0, 0),  # ulUnicodeRange1..4
            b"SPKE",  # achVendID
            struct.pack(">H", 0x0040),  # fsSelection: REGULAR
            struct.pack(">H", min(codes)),  # usFirstCharIndex
            struct.pack(">H", max(codes)),  # usLastCharIndex
            struct.pack(">h", ASCENT),  # sTypoAscender
            struct.pack(">h", DESCENT),  # sTypoDescender
            struct.pack(">h", 0),  # sTypoLineGap
            struct.pack(">H", ASCENT),  # usWinAscent
            struct.pack(">H", -DESCENT),  # usWinDescent
            struct.pack(">II", 0, 0),  # ulCodePageRange1..2
            struct.pack(">h", 500),  # sxHeight
            struct.pack(">h", 700),  # sCapHeight
            struct.pack(">H", 0),  # usDefaultChar
            struct.pack(">H", 32),  # usBreakChar
            struct.pack(">H", 1),  # usMaxContext
        ]
    )
    assert len(os2) == 96, f"OS/2 v4 is 96 bytes, built {len(os2)}"

    name_records = [
        (1, "BurrowSpikeBlock"),
        (2, "Regular"),
        (4, "BurrowSpikeBlock"),
        (6, "BurrowSpikeBlock"),
    ]
    strings = b""
    record_data = b""
    for name_id, value in name_records:
        encoded = value.encode("utf-16-be")
        record_data += struct.pack(">HHHHHH", 3, 1, 0x0409, name_id, len(encoded), len(strings))
        strings += encoded
    name = struct.pack(">HHH", 0, len(name_records), 6 + 12 * len(name_records))
    name += record_data + strings

    tables = {
        b"OS/2": os2,
        b"cmap": cmap,
        b"glyf": glyf,
        b"head": head,
        b"hhea": hhea,
        b"hmtx": hmtx,
        b"loca": loca,
        b"maxp": maxp,
        b"name": name,
        b"post": post,
    }

    tags = sorted(tables)
    num_tables = len(tags)
    search_range = 16
    entry_selector = 0
    while search_range * 2 <= num_tables * 16:
        search_range *= 2
        entry_selector += 1
    header = struct.pack(
        ">IHHHH",
        0x00010000,
        num_tables,
        search_range,
        entry_selector,
        num_tables * 16 - search_range,
    )

    offset = len(header) + 16 * num_tables
    directory = b""
    body = b""
    for tag in tags:
        data = tables[tag]
        directory += struct.pack(">4sIII", tag, _checksum(data), offset, len(data))
        padded = data + b"\0" * ((4 - len(data) % 4) % 4)
        body += padded
        offset += len(padded)

    font = header + directory + body
    adjustment = (0xB1B0AFBA - _checksum(font)) & 0xFFFFFFFF
    head_offset = len(header) + 16 * tags.index(b"head")
    head_start = struct.unpack(">I", font[head_offset + 8 : head_offset + 12])[0]
    font = (
        font[: head_start + 8]
        + struct.pack(">I", adjustment)
        + font[head_start + 12 :]
    )
    return font, gids


def glyph_names(characters: Iterable[str]) -> list[str]:
    """The `/Differences` names for `characters`, in order."""
    return [GLYPH_NAME[ch] for ch in characters]


if __name__ == "__main__":
    import sys

    font, gids = build("ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789- ")
    sys.stderr.write(f"{len(font)} bytes, {len(gids)} glyphs\n")
    sys.stdout.buffer.write(font)
