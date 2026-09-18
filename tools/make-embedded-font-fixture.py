#!/usr/bin/env python3
"""Build `font-program-with-a-paren.pdf`: a page whose font is a real embedded program.

WHY THIS FIXTURE EXISTS, AND WHY IT IS SYNTHESISED FROM A REAL SHAPE

Every fixture in `tests/conformance/fixtures/` is synthesised, and none of them had an embedded
subset font. That is why an ordinary 11 MB course PDF found in minutes what twenty-two cases
never touched: `split` walked from `/Font` into `/FontDescriptor` into `/FontFile3`, and lexed a
CFF program as though it were page content. It refused with

    Malformed("pdf syntax: a ')' with no string to close")

-- a byte inside a charstring -- and the page told the person their file might be damaged. #112.

This fixture is the smallest document with that shape: two pages, one font, a `/FontFile3`
stream whose DECOMPRESSED bytes contain a bare `)` at top level, exactly as a charstring does.
It is not a valid CFF program and does not need to be: nothing in burrow parses it, which is the
whole point of the fix. What it has to be is a stream that a resource walk can reach from a page
and that fails to lex as PDF syntax -- which is the property the defect turned on.

The `)` is at a known offset and is asserted by the test, so a future "tidy-up" that makes the
bytes innocuous cannot silently turn this fixture into one that proves nothing.

Run from the repository root:

    python3 tools/make-embedded-font-fixture.py tests/conformance/fixtures
"""

import sys
from pathlib import Path

# The bytes that stand in for a charstring. A bare `)` with no `(` before it is what a CFF
# program routinely contains and what a PDF content lexer must never be asked to read.
FONT_PROGRAM = (
    b"\x01\x00\x04\x02\x00\x01\x01\x01\x18"
    b"ABCDEF+NotARealFont\x00\x01\x01\x01 \x9b\x1b\x01"
    b"\x8d\x13\x04\x8e\x1c\x0c\x16`\x84Y"
    b")"  # <- the byte the lexer refused on, and the reason this file exists
    b"\x05\x9eY\x0f\x8bf\x10\x8dh\x11\x00\x02\x01\x01jz"
)


def build() -> bytes:
    objects: dict[int, bytes] = {}
    # UNCOMPRESSED, DELIBERATELY. A real `/FontFile3` is flate-compressed, and the defect is
    # about the DECODED bytes -- `stream_data` hands back what qpdf decoded either way, so
    # compression changes nothing about what the lexer was asked to read. Leaving it raw lets
    # the test assert the `)` is still in there without this repository growing an inflater,
    # which is a dependency decision (#24) rather than a fixture's business.
    program = FONT_PROGRAM

    objects[1] = b"<< /Type /Catalog /Pages 2 0 R >>"
    objects[2] = b"<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>"
    # Two pages, so a split has something to exclude and the pruning policy has work to do.
    for number, obj in ((3, 5), (4, 6)):
        objects[number] = (
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents %d 0 R "
            b"/Resources << /Font << /F1 7 0 R >> >> >>" % obj
        )
    for obj, text in ((5, b"page one"), (6, b"page two")):
        stream = b"BT /F1 12 Tf 20 100 Td (" + text + b") Tj ET\n"
        objects[obj] = (
            b"<< /Length %d >>\nstream\n" % len(stream) + stream + b"endstream"
        )
    # The font, its descriptor, and the program. THIS IS THE PATH THE WALK TOOK:
    # /Resources /Font /F1 -> 7 -> /FontDescriptor -> 8 -> /FontFile3 -> 9.
    objects[7] = (
        b"<< /Type /Font /Subtype /Type1 /BaseFont /ABCDEF+NotARealFont "
        b"/FirstChar 32 /LastChar 122 /FontDescriptor 8 0 R >>"
    )
    objects[8] = (
        b"<< /Type /FontDescriptor /FontName /ABCDEF+NotARealFont /Flags 4 "
        b"/FontBBox [0 0 200 200] /ItalicAngle 0 /Ascent 200 /Descent 0 /CapHeight 200 "
        b"/StemV 80 /FontFile3 9 0 R >>"
    )
    objects[9] = (
        b"<< /Subtype /Type1C /Length %d >>\nstream\n" % len(program)
        + program
        + b"\nendstream"
    )

    out = bytearray(b"%PDF-1.7\n")
    offsets: dict[int, int] = {}
    for number in sorted(objects):
        offsets[number] = len(out)
        out += b"%d 0 obj\n" % number + objects[number] + b"\nendobj\n"

    xref = len(out)
    top = max(objects) + 1
    out += b"xref\n0 %d\n0000000000 65535 f \n" % top
    for number in range(1, top):
        out += b"%010d 00000 n \n" % offsets[number]
    out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (top, xref)
    return bytes(out)


def build_type3() -> bytes:
    """A page drawing with a Type 3 font whose GLYPH draws an XObject.

    THE UNDER-APPROXIMATION TEST'S FIXTURE, and the reason the fix is narrow rather than
    "stop following fonts". A Type 3 font's `/CharProcs` ARE content streams: the glyph named
    here draws `/Im1`, and `/Im1` is named nowhere else in the document -- not on the page, not
    in the page's content. A walk that stopped at the font dictionary would never see that name,
    the pruning policy would delete `/Im1` as unused, and the page would come out of a split
    drawing a glyph whose XObject is gone.

    The marker in the form's content is what the test scans for after the split.
    """
    objects: dict[int, bytes] = {}
    objects[1] = b"<< /Type /Catalog /Pages 2 0 R >>"
    objects[2] = b"<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>"
    objects[3] = (
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 5 0 R "
        b"/Resources << /Font << /T3 7 0 R >> /XObject << /Im1 10 0 R >> >> >>"
    )
    objects[4] = (
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 6 0 R "
        b"/Resources << >> >>"
    )
    page_one = b"BT /T3 12 Tf 20 100 Td (a) Tj ET\n"
    objects[5] = b"<< /Length %d >>\nstream\n" % len(page_one) + page_one + b"endstream"
    page_two = b"0 0 1 RG 4 w 10 10 m 190 190 l S\n"
    objects[6] = b"<< /Length %d >>\nstream\n" % len(page_two) + page_two + b"endstream"
    # The Type 3 font. Its `/Resources` is where `/Im1` resolves, and its `/CharProcs` is the
    # only place `/Im1` is NAMED.
    objects[7] = (
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 200 200] /FontMatrix [0.001 0 0 0.001 0 0] "
        b"/CharProcs 8 0 R /Encoding << /Type /Encoding /Differences [97 /glyphA] >> "
        b"/FirstChar 97 /LastChar 97 /Widths [500] >>"
    )
    objects[8] = b"<< /glyphA 9 0 R >>"
    glyph = b"500 0 d0\nq 100 0 0 100 0 0 cm /Im1 Do Q\n"
    objects[9] = b"<< /Length %d >>\nstream\n" % len(glyph) + glyph + b"endstream"
    form = b"MARKER-THE-GLYPH-DREW-THIS\n0 0 1 rg 0 0 1 1 re f\n"
    objects[10] = (
        b"<< /Type /XObject /Subtype /Form /BBox [0 0 1 1] /Length %d >>\nstream\n" % len(form)
        + form
        + b"endstream"
    )

    out = bytearray(b"%PDF-1.7\n")
    offsets: dict[int, int] = {}
    for number in sorted(objects):
        offsets[number] = len(out)
        out += b"%d 0 obj\n" % number + objects[number] + b"\nendobj\n"
    xref = len(out)
    top = max(objects) + 1
    out += b"xref\n0 %d\n0000000000 65535 f \n" % top
    for number in range(1, top):
        out += b"%010d 00000 n \n" % offsets[number]
    out += b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (top, xref)
    return bytes(out)


def objects_of(body: bytes, number: int) -> bytes:
    """One object's bytes, for the generator's own assertions."""
    start = body.index(b"%d 0 obj" % number)
    return body[start : body.index(b"endobj", start)]


def main() -> int:
    if len(sys.argv) != 2:
        print(__doc__)
        return 2
    directory = Path(sys.argv[1])
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / "font-program-with-a-paren.pdf"
    document = build()
    path.write_bytes(document)

    # THE FIXTURE IS CHECKED, NOT ASSUMED. A generator that quietly stopped producing the
    # property under test would leave a fixture that passes everything.
    assert b")" in FONT_PROGRAM, "the font program must contain the byte the lexer refused on"
    assert b"(" not in FONT_PROGRAM.split(b")")[0], "the `)` must have no `(` before it"
    print(f"{path}: {len(document)} bytes, font program {len(FONT_PROGRAM)} bytes")

    type3 = directory / "type3-glyph-draws-an-xobject.pdf"
    body = build_type3()
    type3.write_bytes(body)
    assert b"MARKER-THE-GLYPH-DREW-THIS" in body, "the marker must be in the form"
    # /Im1 appears exactly twice: once in the PAGE's /Resources, where the pruning policy can
    # delete it, and once inside the glyph's content, which is the only place it is USED. If a
    # walk misses the second, the policy deletes the first.
    assert body.count(b"/Im1") == 2, "`/Im1` must be named in the page resources and the glyph"
    assert b"/Im1" not in objects_of(body, 5), "the PAGE content must not name /Im1"
    print(f"{type3}: {len(body)} bytes")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
