#!/usr/bin/env python3
"""Build spike 0006's fixtures: one document per place the visible text of a page can survive.

    python3 spikes/redaction-survival/make-fixtures.py <output-dir>

WHAT THESE FIXTURES ARE FOR

Spike 0006 asks three questions per channel: does a naive "remove the glyphs from the content
stream" redaction leave the text behind, can any allowed instrument find it afterwards, and
would a person looking at the output page still see it. All three are measurements, so all
three need a file rather than an argument.

Every fixture has the same shape, so the harness can treat them uniformly:

    MediaBox [0 0 400 200]
    the SECRET, drawn at (40, 100) at 20pt, inside the redaction region [30 88 370 132]
    the KEEP line, drawn at (40, 40), OUTSIDE the region and identical in every fixture

The keep line is not decoration. A redactor that deletes the whole page passes every "is the
secret gone" test ever written, and `add-operation` §2c is about exactly that: a fidelity
fixture that cannot fail the test is not a fixture. The keep line is what separates a redaction
from a deletion, and the harness asserts it survives in every channel.

WHAT THESE FIXTURES CAN AND CANNOT SUPPORT

Per `CLAUDE.md`: *a test harness that generates its own inputs is measuring what it can
generate.* Said per category rather than waved at once.

- **They are hand-built, byte-exact, with a classic cross-reference table and, except where the
  filter IS the subject, uncompressed streams.** Building them through a library that already
  optimises would measure the library. This means the FIXTURES are trivially scannable -- which
  is why every instrument runs against a qpdf round-trip of the fixture, not the fixture
  itself. See `ROUNDTRIP` in the harness: the control and the measurement then differ by the
  redaction and nothing else.
- **The font is generated** (`blockfont.py`), so the glyphs are real outlines that rasterise,
  and no licence or provenance entry is involved. It is a 5x7 block face and nothing more: it
  has no ligatures, no kerning pairs, no contextual substitution, and no vertical writing. A
  channel that depends on any of those is NOT covered here and is recorded as not covered.
- **One page, one secret, one region.** Multi-page interactions, text straddling a region
  boundary, and text partly inside a clip path are not covered. Region-boundary behaviour is a
  property of a redactor, not of a survival channel, and belongs with the operation.
- **No encryption and no signatures.** A signed document's byte ranges interact with every
  rewrite; that is its own question and is out of this spike's scope.
- **The adversary here is the FORMAT, not a person.** Every fixture is a shape ordinary
  software emits, or a near-miss of one. Channels 6 and 21 are the exceptions and are labelled:
  a font with no `/ToUnicode`, and one whose `/ToUnicode` lies, are what a person WOULD build
  to defeat a text-extraction check.

Not production code. Nothing here is linked, shipped, or depended on.
"""

from __future__ import annotations

import os
import shutil
import struct
import subprocess
import sys
import zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import blockfont  # noqa: E402

# ---------------------------------------------------------------------------
# The shape every fixture shares.
# ---------------------------------------------------------------------------

PAGE_W, PAGE_H = 400, 200
SECRET_X, SECRET_Y, SECRET_SIZE = 40, 100, 20
KEEP_X, KEEP_Y, KEEP_SIZE = 40, 40, 14
REGION = (30, 88, 370, 132)  # the rectangle a redaction is asked to clear
KEEP_LINE = "KEEP-THIS-LINE"

# A decoy of exactly the secret's length, for channel 21's lying `/ToUnicode`.
DECOY = "HARMLESS-FILLER-"


def secret(channel: int) -> str:
    """The canary for a channel. 16 characters, all in the block face."""
    value = f"BURROW-SECRET-{channel:02d}"
    assert len(value) == 16, value
    return value


def carrier(channel: int) -> str:
    """A SECOND canary, for channels whose subject is a carrier that is not the page.

    Added after the first run of the harness could not attribute a removal. Channels 15 and 16
    put the secret in the metadata AND drew it on the page, so when `split` dropped the
    catalog-level object the scan still found the page copy and the table read "carried it
    through" -- crediting the machinery with a failure it had not committed, and hiding a
    removal it had. One canary per PLACE is what makes the table attributable.
    """
    value = f"BURROW-CARRIER-{channel:02d}"
    assert len(value) == 17, value
    return value


ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789- "


# ---------------------------------------------------------------------------
# A byte-exact PDF writer. Classic xref table, no object streams, no compression
# unless a channel's subject is the filter.
# ---------------------------------------------------------------------------


class Pdf:
    def __init__(self) -> None:
        self.objects: dict[int, bytes] = {}
        self._next = 1
        self.extra_trailer = b""

    def reserve(self) -> int:
        num = self._next
        self._next += 1
        return num

    def put(self, num: int, body: bytes) -> int:
        self.objects[num] = body
        return num

    def add(self, body: bytes) -> int:
        return self.put(self.reserve(), body)

    def stream(self, extra: bytes, data: bytes) -> int:
        body = b"<< " + extra + b" /Length " + str(len(data)).encode() + b" >>\nstream\n"
        body += data + b"\nendstream"
        return self.add(body)

    def build(self, root: int, info: int | None = None) -> bytes:
        out = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
        offsets: dict[int, int] = {}
        for num in sorted(self.objects):
            offsets[num] = len(out)
            out += f"{num} 0 obj\n".encode() + self.objects[num] + b"\nendobj\n"
        size = max(self.objects) + 1
        xref_at = len(out)
        out += f"xref\n0 {size}\n".encode()
        out += b"0000000000 65535 f \n"
        for num in range(1, size):
            if num in offsets:
                out += f"{offsets[num]:010d} 00000 n \n".encode()
            else:
                out += b"0000000000 65535 f \n"
        trailer = f"trailer\n<< /Size {size} /Root {root} 0 R".encode()
        if info is not None:
            trailer += f" /Info {info} 0 R".encode()
        trailer += self.extra_trailer + b" >>\n"
        out += trailer + f"startxref\n{xref_at}\n%%EOF\n".encode()
        return bytes(out)


def literal(text: str) -> bytes:
    """A PDF literal string, with the three characters that need escaping escaped."""
    escaped = text.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)")
    return b"(" + escaped.encode("latin-1") + b")"


def utf16_hex(text: str) -> bytes:
    """A UTF-16BE hex string with a BOM -- how a real producer writes a field value."""
    return b"<" + (b"\xfe\xff" + text.encode("utf-16-be")).hex().upper().encode() + b">"


def keep_line_ops(font: bytes = b"/Helv") -> bytes:
    return (
        b"BT " + font + b" " + str(KEEP_SIZE).encode() + b" Tf "
        + f"{KEEP_X} {KEEP_Y} Td ".encode()
        + literal(KEEP_LINE) + b" Tj ET\n"
    )


def helvetica(pdf: Pdf) -> int:
    return pdf.add(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Name /Helv >>")


# ---------------------------------------------------------------------------
# Rasterising the canary, for the channels whose subject is pixels rather than text.
# ---------------------------------------------------------------------------


def raster(text: str, scale: int = 2) -> tuple[int, int, bytes]:
    """The canary as a 1-byte-per-pixel greyscale image: black glyphs on white.

    Uses `blockfont.BITMAP`, so the pixels a person sees here are the same shapes the font
    draws. That matters: channel 14 and channel 20 are only interesting if what is in the
    pixels is legibly the secret.
    """
    cols = len(text) * 6
    w, h = cols * scale, 7 * scale
    rows = bytearray(b"\xff" * (w * h))
    for i, ch in enumerate(text):
        bits = blockfont.BITMAP[ch]
        for r, row in enumerate(bits):
            for c, bit in enumerate(row):
                if bit != "1":
                    continue
                for dy in range(scale):
                    for dx in range(scale):
                        x = (i * 6 + c) * scale + dx
                        y = r * scale + dy
                        rows[y * w + x] = 0x00
    return w, h, bytes(rows)


def outline_ops(text: str, x: float, y: float, size: float) -> bytes:
    """The canary drawn as filled rectangles -- no font, no text object, no glyph."""
    unit = size / 7.0
    ops = [b"0 g\n"]
    for i, ch in enumerate(text):
        for r, row in enumerate(blockfont.BITMAP[ch]):
            for c, bit in enumerate(row):
                if bit != "1":
                    continue
                px = x + (i * 6 + c) * unit
                py = y + (6 - r) * unit
                ops.append(f"{px:.2f} {py:.2f} {unit:.2f} {unit:.2f} re f\n".encode())
    return b"".join(ops)


# ---------------------------------------------------------------------------
# Embedded-font helpers. The three CID channels and the `/Differences` channel all need the
# generated face; they differ only in how a reader is invited to map glyphs back to characters.
# ---------------------------------------------------------------------------


def embed_font_file(pdf: Pdf, font_bytes: bytes) -> int:
    return pdf.stream(
        b"/Length1 " + str(len(font_bytes)).encode(),
        font_bytes,
    )


def font_descriptor(pdf: Pdf, file_ref: int, name: bytes, flags: int = 4) -> int:
    """`flags` decides how a reader maps a code to a glyph, and it is load-bearing.

    Bit 3 (value 4) is "symbolic", and a symbolic TrueType font is read through its own
    (3,0) cmap with the code masked into 0xF0xx -- `/Differences` is IGNORED. The first
    version of this file set 4 on the `/Differences` fixture, and the result was a page that
    drew nothing at all: the non-vacuity control refused the run, which is what it is for.
    Bit 6 (value 32) is "nonsymbolic", and that is what makes `/Differences` the mapping.
    """
    return pdf.add(
        b"<< /Type /FontDescriptor /FontName " + name + b" /Flags " + str(flags).encode() +
        b" /FontBBox [0 0 600 700] /ItalicAngle 0 /Ascent 800 /Descent -200"
        b" /CapHeight 700 /StemV 80 /FontFile2 " + str(file_ref).encode() + b" 0 R >>"
    )


def simple_differences_font(pdf: Pdf, text: str) -> tuple[int, bytes]:
    """A simple TrueType font whose codes are 1..n and whose meaning lives in `/Differences`.

    The content stream will contain the bytes 01 02 03 ..., which are not the text and do not
    resemble it. Getting from those bytes back to characters means resolving each code to a
    glyph NAME through `/Differences` and then that name to a character through the Adobe glyph
    list -- two hops, neither of which a byte scan makes.
    """
    order = list(dict.fromkeys(text))  # unique, first-appearance order
    font_bytes, gids = blockfont.build("".join(order))
    file_ref = embed_font_file(pdf, font_bytes)
    desc = font_descriptor(pdf, file_ref, b"/BRWSPK+BurrowSpikeBlock", flags=32)
    codes = {ch: i + 1 for i, ch in enumerate(order)}
    names = b" ".join(b"/" + blockfont.GLYPH_NAME[ch].encode() for ch in order)
    widths = b" ".join(b"600" for _ in order)
    font = pdf.add(
        b"<< /Type /Font /Subtype /TrueType /BaseFont /BRWSPK+BurrowSpikeBlock"
        b" /FirstChar 1 /LastChar " + str(len(order)).encode() +
        b" /Widths [" + widths + b"]"
        b" /FontDescriptor " + str(desc).encode() + b" 0 R"
        b" /Encoding << /Type /Encoding /Differences [1 " + names + b"] >> >>"
    )
    encoded = bytes(codes[ch] for ch in text)
    return font, encoded


def cid_font(pdf: Pdf, text: str, tounicode: str | None) -> tuple[int, bytes]:
    """A Type0/Identity-H font. Codes are glyph ids; meaning lives only in `/ToUnicode`.

    `tounicode` is the string the CMap claims the glyphs spell. Pass the real text for the
    honest case, `None` for no CMap at all, and a decoy for a CMap that lies.
    """
    order = list(dict.fromkeys(text))
    font_bytes, gids = blockfont.build("".join(order))
    file_ref = embed_font_file(pdf, font_bytes)
    desc = font_descriptor(pdf, file_ref, b"/BRWSPK+BurrowSpikeBlock")
    w_entries = b" ".join(
        str(gids[ch]).encode() + b" [600]" for ch in order
    )
    descendant = pdf.add(
        b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /BRWSPK+BurrowSpikeBlock"
        b" /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >>"
        b" /FontDescriptor " + str(desc).encode() + b" 0 R"
        b" /DW 600 /W [" + w_entries + b"]"
        b" /CIDToGIDMap /Identity >>"
    )
    entries = b"<< /Type /Font /Subtype /Type0 /BaseFont /BRWSPK+BurrowSpikeBlock"
    entries += b" /Encoding /Identity-H /DescendantFonts [" + str(descendant).encode() + b" 0 R]"
    if tounicode is not None:
        assert len(tounicode) == len(text), "a lying CMap must still be the right length"
        pairs = b"".join(
            b"<%04X> <%04X>\n" % (gids[ch], ord(claim))
            for ch, claim in zip(text, tounicode)
        )
        cmap = (
            b"/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n"
            b"/CMapName /Burrow-Spike def\n/CMapType 2 def\n"
            b"/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n"
            b"1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n"
            + str(len(text)).encode() + b" beginbfchar\n" + pairs + b"endbfchar\n"
            b"endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n"
        )
        entries += b" /ToUnicode " + str(pdf.stream(b"", cmap)).encode() + b" 0 R"
    entries += b" >>"
    font = pdf.add(entries)
    encoded = b"".join(struct.pack(">H", gids[ch]) for ch in text)
    return font, encoded


def show_hex(data: bytes) -> bytes:
    return b"<" + data.hex().upper().encode() + b">"


# ---------------------------------------------------------------------------
# One builder per channel. Each returns the finished bytes.
# ---------------------------------------------------------------------------


def simple_page(
    pdf: Pdf,
    content: bytes,
    resources: bytes,
    page_extra: bytes = b"",
    catalog_extra: bytes = b"",
    info: int | None = None,
) -> bytes:
    pages = pdf.reserve()
    page = pdf.reserve()
    stream = pdf.stream(b"", content)
    pdf.put(
        page,
        b"<< /Type /Page /Parent " + str(pages).encode() + b" 0 R"
        b" /MediaBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << " + resources + b" >>"
        b" /Contents " + str(stream).encode() + b" 0 R" + page_extra + b" >>",
    )
    pdf.put(
        pages,
        b"<< /Type /Pages /Count 1 /Kids [" + str(page).encode() + b" 0 R] >>",
    )
    root = pdf.add(
        b"<< /Type /Catalog /Pages " + str(pages).encode() + b" 0 R" + catalog_extra + b" >>"
    )
    return pdf.build(root, info)


def ch01_plain_tj() -> bytes:
    pdf = Pdf()
    helv = helvetica(pdf)
    content = (
        b"BT /Helv 20 Tf " + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal(secret(1)) + b" Tj ET\n" + keep_line_ops()
    )
    return simple_page(pdf, content, b"/Font << /Helv " + str(helv).encode() + b" 0 R >>")


def ch02_kerned_tj() -> bytes:
    pdf = Pdf()
    helv = helvetica(pdf)
    parts = b"".join(literal(c) + b" -40 " for c in secret(2))
    content = (
        b"BT /Helv 20 Tf " + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + b"[" + parts + b"] TJ ET\n" + keep_line_ops()
    )
    return simple_page(pdf, content, b"/Font << /Helv " + str(helv).encode() + b" 0 R >>")


def ch03_positioned_runs() -> bytes:
    pdf = Pdf()
    helv = helvetica(pdf)
    ops = [b"BT /Helv 20 Tf\n"]
    for i, ch in enumerate(secret(3)):
        ops.append(f"1 0 0 1 {SECRET_X + i * 11.2:.2f} {SECRET_Y} Tm ".encode())
        ops.append(literal(ch) + b" Tj\n")
    ops.append(b"ET\n")
    content = b"".join(ops) + keep_line_ops()
    return simple_page(pdf, content, b"/Font << /Helv " + str(helv).encode() + b" 0 R >>")


def ch04_differences_encoding() -> bytes:
    pdf = Pdf()
    helv = helvetica(pdf)
    font, encoded = simple_differences_font(pdf, secret(4))
    content = (
        b"BT /F1 20 Tf " + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + show_hex(encoded) + b" Tj ET\n" + keep_line_ops()
    )
    res = (
        b"/Font << /F1 " + str(font).encode() + b" 0 R /Helv " + str(helv).encode() + b" 0 R >>"
    )
    return simple_page(pdf, content, res)


def _cid_page(channel: int, tounicode: str | None) -> bytes:
    pdf = Pdf()
    helv = helvetica(pdf)
    font, encoded = cid_font(pdf, secret(channel), tounicode)
    content = (
        b"BT /F1 20 Tf " + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + show_hex(encoded) + b" Tj ET\n" + keep_line_ops()
    )
    res = (
        b"/Font << /F1 " + str(font).encode() + b" 0 R /Helv " + str(helv).encode() + b" 0 R >>"
    )
    return simple_page(pdf, content, res)


def ch05_cid_with_tounicode() -> bytes:
    return _cid_page(5, secret(5))


def ch06_cid_without_tounicode() -> bytes:
    return _cid_page(6, None)


def ch21_cid_lying_tounicode() -> bytes:
    return _cid_page(21, DECOY)


def ch07_form_xobject() -> bytes:
    pdf = Pdf()
    helv = helvetica(pdf)
    inner = (
        b"BT /Helv 20 Tf " + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal(secret(7)) + b" Tj ET\n"
    )
    form = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 400 200]"
        b" /Resources << /Font << /Helv " + str(helv).encode() + b" 0 R >> >>",
        inner,
    )
    content = b"/X1 Do\n" + keep_line_ops()
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /XObject << /X1 " + str(form).encode() + b" 0 R >>"
    )
    return simple_page(pdf, content, res)


def ch08_type3_glyph() -> bytes:
    pdf = Pdf()
    helv = helvetica(pdf)
    proc = pdf.stream(
        b"",
        b"200 0 0 0 200 20 d1\n"
        b"BT /Helv 20 Tf 0 0 Td " + literal(secret(8)) + b" Tj ET\n",
    )
    charprocs = pdf.add(b"<< /g " + str(proc).encode() + b" 0 R >>")
    encoding = pdf.add(b"<< /Type /Encoding /Differences [97 /g] >>")
    t3res = pdf.add(b"<< /Font << /Helv " + str(helv).encode() + b" 0 R >> >>")
    t3 = pdf.add(
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 200 20]"
        b" /FontMatrix [1 0 0 1 0 0]"
        b" /CharProcs " + str(charprocs).encode() + b" 0 R"
        b" /Encoding " + str(encoding).encode() + b" 0 R"
        b" /FirstChar 97 /LastChar 97 /Widths [200]"
        b" /Resources " + str(t3res).encode() + b" 0 R >>"
    )
    content = (
        b"BT /T3 1 Tf " + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal("a") + b" Tj ET\n" + keep_line_ops()
    )
    res = (
        b"/Font << /T3 " + str(t3).encode() + b" 0 R /Helv " + str(helv).encode() + b" 0 R >>"
    )
    return simple_page(pdf, content, res)


def ch09_actualtext() -> bytes:
    """The glyphs are unreadable to a byte scan; `/ActualText` states what they say."""
    pdf = Pdf()
    helv = helvetica(pdf)
    font, encoded = simple_differences_font(pdf, secret(9))
    content = (
        b"/Span << /ActualText " + literal(carrier(9)) + b" >> BDC\n"
        b"BT /F1 20 Tf " + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + show_hex(encoded) + b" Tj ET\n"
        b"EMC\n" + keep_line_ops()
    )
    res = (
        b"/Font << /F1 " + str(font).encode() + b" 0 R /Helv " + str(helv).encode() + b" 0 R >>"
    )
    return simple_page(pdf, content, res)


def ch10_structure_tree() -> bytes:
    pdf = Pdf()
    helv = helvetica(pdf)
    font, encoded = simple_differences_font(pdf, secret(10))
    pages = pdf.reserve()
    page = pdf.reserve()
    struct_root = pdf.reserve()
    elem = pdf.reserve()
    content = (
        b"/P << /MCID 0 >> BDC\n"
        b"BT /F1 20 Tf " + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + show_hex(encoded) + b" Tj ET\nEMC\n" + keep_line_ops()
    )
    stream = pdf.stream(b"", content)
    pdf.put(
        elem,
        b"<< /Type /StructElem /S /P /P " + str(struct_root).encode() + b" 0 R"
        b" /Pg " + str(page).encode() + b" 0 R /K 0"
        b" /Alt " + literal("Alt text: " + carrier(10)) + b""
        b" /ActualText " + literal(carrier(10)) + b" >>",
    )
    pdf.put(
        struct_root,
        b"<< /Type /StructTreeRoot /K [" + str(elem).encode() + b" 0 R] >>",
    )
    pdf.put(
        page,
        b"<< /Type /Page /Parent " + str(pages).encode() + b" 0 R"
        b" /MediaBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /Font << /F1 " + str(font).encode() + b" 0 R"
        b" /Helv " + str(helv).encode() + b" 0 R >> >>"
        b" /StructParents 0"
        b" /Contents " + str(stream).encode() + b" 0 R >>",
    )
    pdf.put(pages, b"<< /Type /Pages /Count 1 /Kids [" + str(page).encode() + b" 0 R] >>")
    root = pdf.add(
        b"<< /Type /Catalog /Pages " + str(pages).encode() + b" 0 R"
        b" /StructTreeRoot " + str(struct_root).encode() + b" 0 R"
        b" /MarkInfo << /Marked true >> >>"
    )
    return pdf.build(root)


def ch11_annotation() -> bytes:
    pdf = Pdf()
    helv = helvetica(pdf)
    ap = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 340 44]"
        b" /Resources << /Font << /Helv " + str(helv).encode() + b" 0 R >> >>",
        b"BT /Helv 20 Tf 4 12 Td " + literal(secret(11)) + b" Tj ET\n",
    )
    annot = pdf.add(
        b"<< /Type /Annot /Subtype /FreeText"
        b" /Rect [" + f"{REGION[0]} {REGION[1]} {REGION[2]} {REGION[3]}".encode() + b"]"
        b" /F 4 /Contents " + literal(carrier(11)) +
        b" /DA (/Helv 20 Tf 0 g)"
        b" /AP << /N " + str(ap).encode() + b" 0 R >> >>"
    )
    content = keep_line_ops()
    return simple_page(
        pdf,
        content,
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>",
        page_extra=b" /Annots [" + str(annot).encode() + b" 0 R]",
    )


def ch12_acroform_field() -> bytes:
    pdf = Pdf()
    helv = helvetica(pdf)
    ap = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 340 44]"
        b" /Resources << /Font << /Helv " + str(helv).encode() + b" 0 R >> >>",
        b"/Tx BMC q BT /Helv 20 Tf 4 12 Td " + literal(secret(12)) + b" Tj ET Q EMC\n",
    )
    pages = pdf.reserve()
    page = pdf.reserve()
    field = pdf.reserve()
    acroform = pdf.reserve()
    pdf.put(
        field,
        b"<< /Type /Annot /Subtype /Widget /FT /Tx /T " + literal("secret-field") +
        b" /V " + utf16_hex(carrier(12)) +
        b" /DA (/Helv 20 Tf 0 g) /F 4"
        b" /Rect [" + f"{REGION[0]} {REGION[1]} {REGION[2]} {REGION[3]}".encode() + b"]"
        b" /P " + str(page).encode() + b" 0 R"
        b" /AP << /N " + str(ap).encode() + b" 0 R >> >>",
    )
    pdf.put(
        acroform,
        b"<< /Fields [" + str(field).encode() + b" 0 R] /DA (/Helv 0 Tf 0 g)"
        b" /DR << /Font << /Helv " + str(helv).encode() + b" 0 R >> >> >>",
    )
    stream = pdf.stream(b"", keep_line_ops())
    pdf.put(
        page,
        b"<< /Type /Page /Parent " + str(pages).encode() + b" 0 R"
        b" /MediaBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /Font << /Helv " + str(helv).encode() + b" 0 R >> >>"
        b" /Annots [" + str(field).encode() + b" 0 R]"
        b" /Contents " + str(stream).encode() + b" 0 R >>",
    )
    pdf.put(pages, b"<< /Type /Pages /Count 1 /Kids [" + str(page).encode() + b" 0 R] >>")
    root = pdf.add(
        b"<< /Type /Catalog /Pages " + str(pages).encode() + b" 0 R"
        b" /AcroForm " + str(acroform).encode() + b" 0 R >>"
    )
    return pdf.build(root)


def ch13_optional_content() -> bytes:
    pdf = Pdf()
    helv = helvetica(pdf)
    ocg = pdf.add(b"<< /Type /OCG /Name " + literal("Layer holding " + carrier(13)) + b" >>")
    content = (
        b"/OC /MC0 BDC\n"
        b"BT /Helv 20 Tf " + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal(secret(13)) + b" Tj ET\nEMC\n" + keep_line_ops()
    )
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /Properties << /MC0 " + str(ocg).encode() + b" 0 R >>"
    )
    return simple_page(
        pdf,
        content,
        res,
        catalog_extra=(
            b" /OCProperties << /OCGs [" + str(ocg).encode() + b" 0 R]"
            b" /D << /OFF [" + str(ocg).encode() + b" 0 R]"
            b" /Order [" + str(ocg).encode() + b" 0 R] >> >>"
        ),
    )


def ch14_thumbnail() -> bytes:
    pdf = Pdf()
    helv = helvetica(pdf)
    w, h, pixels = raster(carrier(14), scale=2)
    thumb = pdf.stream(
        b"/Type /XObject /Subtype /Image /Width " + str(w).encode() +
        b" /Height " + str(h).encode() +
        b" /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode",
        zlib.compress(pixels, 9),
    )
    content = (
        b"BT /Helv 20 Tf " + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal(secret(14)) + b" Tj ET\n" + keep_line_ops()
    )
    return simple_page(
        pdf,
        content,
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>",
        page_extra=b" /Thumb " + str(thumb).encode() + b" 0 R",
    )


def ch15_metadata() -> bytes:
    pdf = Pdf()
    helv = helvetica(pdf)
    value = carrier(15)
    xmp = (
        b'<?xpacket begin="\xef\xbb\xbf" id="W5M0MpCehiHzreSzNTczkc9d"?>\n'
        b'<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF '
        b'xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">'
        b'<rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/">'
        b"<dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">" + value.encode() +
        b"</rdf:li></rdf:Alt></dc:title></rdf:Description></rdf:RDF></x:xmpmeta>\n"
        b'<?xpacket end="w"?>'
    )
    meta = pdf.stream(b"/Type /Metadata /Subtype /XML", xmp)
    info = pdf.add(
        b"<< /Title " + literal(value) + b" /Keywords " + literal(value) + b" >>"
    )
    content = (
        b"BT /Helv 20 Tf " + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal(secret(15)) + b" Tj ET\n" + keep_line_ops()
    )
    return simple_page(
        pdf,
        content,
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>",
        catalog_extra=b" /Metadata " + str(meta).encode() + b" 0 R",
        info=info,
    )


def ch16_attachment() -> bytes:
    pdf = Pdf()
    helv = helvetica(pdf)
    value = carrier(16)
    payload = f"this attachment contains {value}\n".encode()
    ef = pdf.stream(b"/Type /EmbeddedFile /Subtype /text#2Fplain", payload)
    spec = pdf.add(
        b"<< /Type /Filespec /F " + literal(value + ".txt") +
        b" /UF " + utf16_hex(value + ".txt") +
        b" /Desc " + literal(value) +
        b" /EF << /F " + str(ef).encode() + b" 0 R >> >>"
    )
    names = pdf.add(
        b"<< /EmbeddedFiles << /Names [" + literal(value) + b" " +
        str(spec).encode() + b" 0 R] >> >>"
    )
    content = (
        b"BT /Helv 20 Tf " + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal(secret(16)) + b" Tj ET\n" + keep_line_ops()
    )
    return simple_page(
        pdf,
        content,
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>",
        catalog_extra=b" /Names " + str(names).encode() + b" 0 R",
    )


def ch17_incremental_update() -> bytes:
    """The secret is drawn, then an incremental update replaces the content stream.

    The original stream object is still in the file, reachable only through the first
    cross-reference section. This is what "redact, then save" does in a viewer that appends.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    pages = pdf.reserve()
    page = pdf.reserve()
    original = pdf.stream(
        b"",
        b"BT /Helv 20 Tf " + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal(secret(17)) + b" Tj ET\n" + keep_line_ops(),
    )
    pdf.put(
        page,
        b"<< /Type /Page /Parent " + str(pages).encode() + b" 0 R"
        b" /MediaBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /Font << /Helv " + str(helv).encode() + b" 0 R >> >>"
        b" /Contents " + str(original).encode() + b" 0 R >>",
    )
    pdf.put(pages, b"<< /Type /Pages /Count 1 /Kids [" + str(page).encode() + b" 0 R] >>")
    root = pdf.add(b"<< /Type /Catalog /Pages " + str(pages).encode() + b" 0 R >>")
    base = pdf.build(root)
    first_startxref = int(base.rsplit(b"startxref\n", 1)[1].split(b"\n")[0])

    # The update: the same object number, new bytes, appended with its own xref section.
    replacement = keep_line_ops()
    body = (
        b"<< /Length " + str(len(replacement)).encode() + b" >>\nstream\n"
        + replacement + b"\nendstream"
    )
    out = bytearray(base)
    offset = len(out)
    out += f"{original} 0 obj\n".encode() + body + b"\nendobj\n"
    xref_at = len(out)
    out += b"xref\n"
    out += f"{original} 1\n".encode()
    out += f"{offset:010d} 00000 n \n".encode()
    size = max(pdf.objects) + 1
    out += (
        f"trailer\n<< /Size {size} /Root {root} 0 R"
        f" /Prev {first_startxref} >>\n".encode()
    )
    out += f"startxref\n{xref_at}\n%%EOF\n".encode()
    return bytes(out)


def ch18_vector_outlines() -> bytes:
    pdf = Pdf()
    helv = helvetica(pdf)
    content = outline_ops(secret(18), SECRET_X, SECRET_Y, SECRET_SIZE) + keep_line_ops()
    return simple_page(pdf, content, b"/Font << /Helv " + str(helv).encode() + b" 0 R >>")


def ch19_covered_by_a_rectangle() -> bytes:
    pdf = Pdf()
    helv = helvetica(pdf)
    x0, y0, x1, y1 = REGION
    content = (
        b"BT /Helv 20 Tf " + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal(secret(19)) + b" Tj ET\n"
        + f"0 g {x0} {y0} {x1 - x0} {y1 - y0} re f\n".encode()
        + keep_line_ops()
    )
    return simple_page(pdf, content, b"/Font << /Helv " + str(helv).encode() + b" 0 R >>")


def ch20_image_pixels(cjpeg: Path) -> bytes:
    pdf = Pdf()
    helv = helvetica(pdf)
    w, h, pixels = raster(secret(20), scale=4)
    pgm = f"P5\n{w} {h}\n255\n".encode() + pixels
    jpeg = subprocess.run(
        [str(cjpeg), "-quality", "92", "-grayscale"],
        input=pgm,
        capture_output=True,
        check=True,
    ).stdout
    image = pdf.stream(
        b"/Type /XObject /Subtype /Image /Width " + str(w).encode() +
        b" /Height " + str(h).encode() +
        b" /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /DCTDecode",
        jpeg,
    )
    x0, y0, x1, y1 = REGION
    content = (
        b"q " + f"{x1 - x0} 0 0 {y1 - y0} {x0} {y0} cm".encode() + b" /Im1 Do Q\n"
        + keep_line_ops()
    )
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /XObject << /Im1 " + str(image).encode() + b" 0 R >>"
    )
    return simple_page(pdf, content, res)


def ch24_page_level_metadata() -> bytes:
    """`/Metadata` and `/PieceInfo` on the PAGE, not the catalogue.

    Added after the recommendation had to answer "does v1 strip page metadata or only warn about
    it". Channel 15 puts the XMP on the catalogue, which finding 4 measured as unreachable -- so
    it could only ever support a disclosure. This one is page-side, and the whole question is
    whether the page-key allowlist `split` already has reaches it.

    `/PieceInfo` is included because it is the page key most likely to hold something a producer
    put there and nobody thinks about: private application data, in a dictionary whose shape is
    the application's business. It is exactly the "key nobody enumerated" that ADR 0019's
    allowlist exists for.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    value = carrier(24)
    xmp = (
        b'<?xpacket begin="\xef\xbb\xbf" id="W5M0MpCehiHzreSzNTczkc9d"?>\n'
        b'<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF '
        b'xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">'
        b'<rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/">'
        b"<dc:description>" + value.encode() +
        b"</dc:description></rdf:Description></rdf:RDF></x:xmpmeta>\n"
        b'<?xpacket end="w"?>'
    )
    meta = pdf.stream(b"/Type /Metadata /Subtype /XML", xmp)
    piece = pdf.add(
        b"<< /SomeApplication << /LastModified (D:20260919000000Z)"
        b" /Private " + literal(value) + b" >> >>"
    )
    content = (
        b"BT /Helv 20 Tf " + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal(secret(24)) + b" Tj ET\n" + keep_line_ops()
    )
    return simple_page(
        pdf,
        content,
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>",
        page_extra=(
            b" /Metadata " + str(meta).encode() + b" 0 R"
            b" /PieceInfo " + str(piece).encode() + b" 0 R"
        ),
    )


def ch22_split_content_streams() -> bytes:
    """`/Contents` is an ARRAY, and the text object straddles two of its streams.

    Added while building the harness rather than while enumerating channels, because it is two
    things at once. It is a survival channel -- a tokeniser that reads each stream separately
    sees a `BT` with no `ET` and an `ET` with no `BT`, and neither stream contains a complete
    text object. And it is the case that decides what the WRITE verb can do: qpdf hands back a
    page's content streams CONCATENATED, so the identity of the individual streams is already
    gone by the time a redactor has something to edit. See finding 4.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    first = pdf.stream(b"", b"BT /Helv 20 Tf " + f"{SECRET_X} {SECRET_Y} Td".encode() + b"\n")
    second = pdf.stream(b"", literal(secret(22)) + b" Tj ET\n" + keep_line_ops())
    pages = pdf.reserve()
    page = pdf.reserve()
    pdf.put(
        page,
        b"<< /Type /Page /Parent " + str(pages).encode() + b" 0 R"
        b" /MediaBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /Font << /Helv " + str(helv).encode() + b" 0 R >> >>"
        b" /Contents [" + str(first).encode() + b" 0 R " + str(second).encode() + b" 0 R] >>",
    )
    pdf.put(pages, b"<< /Type /Pages /Count 1 /Kids [" + str(page).encode() + b" 0 R] >>")
    root = pdf.add(b"<< /Type /Catalog /Pages " + str(pages).encode() + b" 0 R >>")
    return pdf.build(root)


def control_no_canary() -> bytes:
    """The inertness control: the same shape, carrying no canary anywhere.

    Every instrument must report ABSENT on this file. A scan that matches everything fails
    everything, and without this control the two are indistinguishable from a passing run.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    content = (
        b"BT /Helv 20 Tf " + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal("NOTHING-TO-SEE-HERE") + b" Tj ET\n" + keep_line_ops()
    )
    return simple_page(pdf, content, b"/Font << /Helv " + str(helv).encode() + b" 0 R >>")


# ---------------------------------------------------------------------------

CHANNELS: list[tuple[int, str, str]] = [
    (1, "plain-tj", "a plain `Tj` in an unembedded simple font"),
    (2, "kerned-tj", "one `TJ` array with a kern between every glyph"),
    (3, "positioned-runs", "one `Tj` per glyph, repositioned by `Tm` between each"),
    (4, "differences-encoding", "embedded subset font, codes 1..n via `/Differences`, no `/ToUnicode`"),
    (5, "cid-with-tounicode", "Type0/Identity-H, glyph ids as codes, honest `/ToUnicode`"),
    (6, "cid-without-tounicode", "the same with no `/ToUnicode` at all"),
    (7, "form-xobject", "the text lives in a Form XObject, not the page's `/Contents`"),
    (8, "type3-glyph", "the text lives in a Type 3 glyph procedure"),
    (9, "actualtext", "unreadable glyphs, with `/ActualText` on the marked-content span"),
    (10, "structure-tree", "`/StructElem` `/Alt` and `/ActualText` under `/StructTreeRoot`"),
    (11, "annotation", "a FreeText annotation's `/Contents` and its `/AP /N` appearance"),
    (12, "acroform-field", "an AcroForm text field's `/V`, UTF-16BE, plus its appearance"),
    (13, "optional-content", "inside an optional-content group the document turns OFF"),
    (14, "thumbnail", "the page's `/Thumb`, as pixels"),
    (15, "metadata", "catalog `/Metadata` XMP packet and the document `/Info`"),
    (16, "attachment", "an `/EmbeddedFiles` attachment, its name and its bytes"),
    (17, "incremental-update", "the pre-redaction content stream, still in the file"),
    (18, "vector-outlines", "drawn as filled rectangles: no font, no text object"),
    (19, "covered-by-a-rectangle", "drawn, then a black rectangle drawn over it"),
    (20, "image-pixels", "inside a `/DCTDecode` image"),
    (21, "cid-lying-tounicode", "Type0/Identity-H whose `/ToUnicode` maps to a decoy"),
    (22, "split-content-streams", "one text object straddling two `/Contents` streams"),
    (24, "page-level-metadata", "`/Metadata` and `/PieceInfo` on the PAGE, not the catalogue"),
]

BUILDERS = {
    1: ch01_plain_tj,
    2: ch02_kerned_tj,
    3: ch03_positioned_runs,
    4: ch04_differences_encoding,
    5: ch05_cid_with_tounicode,
    6: ch06_cid_without_tounicode,
    7: ch07_form_xobject,
    8: ch08_type3_glyph,
    9: ch09_actualtext,
    10: ch10_structure_tree,
    11: ch11_annotation,
    12: ch12_acroform_field,
    13: ch13_optional_content,
    14: ch14_thumbnail,
    15: ch15_metadata,
    16: ch16_attachment,
    17: ch17_incremental_update,
    18: ch18_vector_outlines,
    19: ch19_covered_by_a_rectangle,
    20: ch20_image_pixels,
    21: ch21_cid_lying_tounicode,
    22: ch22_split_content_streams,
    24: ch24_page_level_metadata,
}


def repo_root() -> Path:
    return Path(__file__).resolve().parents[2]


def vendored(*parts: str) -> Path:
    import platform

    arch = platform.machine()
    return repo_root() / "engines" / "vendor" / f"native-{arch}" / Path(*parts)


def qpdf_cli() -> Path:
    import platform

    arch = platform.machine()
    path = (
        repo_root() / "engines" / "vendor" / "src" / f"build-qpdf-plain-{arch}" / "qpdf" / "qpdf"
    )
    if not path.exists():
        found = shutil.which("qpdf")
        if found:
            return Path(found)
        raise SystemExit(
            f"no qpdf: expected the vendored build at {path}. Run engines/fetch.sh and "
            "engines/build-native.sh."
        )
    return path


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        sys.stderr.write(f"usage: {argv[0]} <output-dir>\n")
        return 2
    out = Path(argv[1])
    out.mkdir(parents=True, exist_ok=True)
    qpdf = qpdf_cli()
    cjpeg = vendored("bin", "cjpeg")
    if not cjpeg.exists():
        raise SystemExit(f"no cjpeg at {cjpeg}; channel 20 needs it")

    written: list[tuple[str, int]] = []
    unreadable: list[str] = []

    for number, slug, _description in CHANNELS:
        builder = BUILDERS[number]
        data = builder(cjpeg) if number == 20 else builder()
        path = out / f"{number:02d}-{slug}.pdf"
        path.write_bytes(data)
        # EVERY FIXTURE IS CHECKED BEFORE IT IS REPORTED. A fixture qpdf will not read is one
        # the harness would measure as "nothing survived", which reads exactly like success.
        result = subprocess.run(
            [str(qpdf), "--check", str(path)], capture_output=True, text=True
        )
        if result.returncode not in (0, 3):  # 3 is warnings-only
            unreadable.append(f"{path.name}: {result.stdout.strip()[:300]}")
        written.append((path.name, len(data)))

    control = out / "00-control-no-canary.pdf"
    control.write_bytes(control_no_canary())
    written.append((control.name, control.stat().st_size))

    for name, size in written:
        print(f"  {name:34s} {size:>8,} bytes")
    print(f"wrote {len(written)} files to {out} ({len(CHANNELS)} channels + 1 control)")

    if unreadable:
        sys.stderr.write("\nUNREADABLE FIXTURES -- refusing to report a measurement on these:\n")
        for line in unreadable:
            sys.stderr.write(f"  {line}\n")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
