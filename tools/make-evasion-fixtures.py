#!/usr/bin/env python3
"""Fixtures built to EVADE each refusal's proposed page-side signal, and their near-miss twins.

    python3 tools/make-evasion-fixtures.py <output-dir>

WHAT THIS IS FOR

[ADR 0029](docs/adr/0029-what-redaction-does-and-what-it-refuses-to-do.md) §5 states the rule
and the bar:

    A refusal keyed on something the operation cannot see is a bypass.

    No refusal may ship on its signal until that signal has been measured to fire on a fixture
    built to evade it.

Issue #125 is that work. These are the fixtures. Each one puts the thing a refusal is supposed
to catch somewhere the PROPOSED signal does not look.

TWO FILES PER CASE, AND THE SECOND IS THE HALF PEOPLE FORGET

Every evasion fixture has a **near-miss twin** that must NOT be refused. Without it a signal can
pass every evasion test by refusing everything, which is not a signal — it is an outage.
`split_no_leak.rs::a_document_without_layers_is_not_caught_by_the_layer_refusal` is the model,
and ADR 0019's pruning work needed exactly this pair: its first `/Annots` filter keyed on
"shared at all" and deleted every annotation from a split that excluded nothing.

So: `NN-evade-<slug>.pdf` must be refused, `NN-nearmiss-<slug>.pdf` must not.

WHAT THESE ARE NOT

They do not test the redaction. Nothing here runs an operation — the operation does not exist
(#131). They are the inputs #125 needs in order to be testable at all, and the manifest records
the verdict each must produce so the assertion is written before the code it judges.

NOT COMMITTED. Deterministic and toolchain-free, like the rest of `make-redaction-fixtures.py`'s
output.
"""

from __future__ import annotations

import subprocess
import sys
import zlib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from pdfbuild import (  # noqa: E402
    KEEP_LINE,
    PAGE_H,
    PAGE_W,
    REGION,
    SECRET_SIZE,
    SECRET_X,
    SECRET_Y,
    Pdf,
    helvetica,
    keep_line_ops,
    literal,
    outline_ops,
    qpdf_cli,
    raster,
    simple_page,
    utf16_hex,
)


def secret(tag: str) -> str:
    return f"BURROW-EVADE-{tag}"


# ===========================================================================================
# Refusal 1 — "a region intersecting an IMAGE". Proposed signal: the page's own content stream.
#
# Past it: a `Do` inside a Form XObject, an inline BI image, a tiling pattern, a shading fill.
# ADR 0029 §5 names all four; spike 0006's channel 20 draws its image directly on the page and
# therefore measured none of them.
# ===========================================================================================


def _grey_image_stream(pdf: Pdf, text: str, scale: int = 3) -> tuple[int, int, int]:
    w, h, pixels = raster(text, scale=scale)
    ref = pdf.stream(
        b"/Type /XObject /Subtype /Image /Width " + str(w).encode()
        + b" /Height " + str(h).encode()
        + b" /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode",
        zlib.compress(pixels, 9),
    )
    return ref, w, h


def evade_image_in_form() -> bytes:
    """The image is drawn by a Form XObject. The page's content stream says only `/X1 Do`."""
    pdf = Pdf()
    helv = helvetica(pdf)
    image, _w, _h = _grey_image_stream(pdf, secret("IMG-FORM"))
    x0, y0, x1, y1 = REGION
    form = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /XObject << /Im1 " + str(image).encode() + b" 0 R >> >>",
        b"q " + f"{x1 - x0} 0 0 {y1 - y0} {x0} {y0}".encode() + b" cm /Im1 Do Q\n",
    )
    content = b"/X1 Do\n" + keep_line_ops()
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /XObject << /X1 " + str(form).encode() + b" 0 R >>"
    )
    return simple_page(pdf, content, res)


def evade_inline_image() -> bytes:
    """An inline `BI` ... `ID` ... `EI` image. Not an XObject at all, so `/XObject` is empty.

    A signal that enumerates the page's `/XObject` resources finds nothing here. The tokeniser
    already has to skip inline image data (`pdfsyntax::lexer::skip_inline_image_data`), which is
    the same place a signal would have to notice it.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    w, h, pixels = raster(secret("IMG-INLINE"), scale=1)
    x0, y0, x1, y1 = REGION
    inline = (
        b"q " + f"{x1 - x0} 0 0 {y1 - y0} {x0} {y0}".encode() + b" cm\n"
        b"BI /W " + str(w).encode() + b" /H " + str(h).encode()
        + b" /CS /G /BPC 8 /F /Fl /L " + str(len(zlib.compress(pixels, 9))).encode() + b" ID "
    )
    content = inline + zlib.compress(pixels, 9) + b"\nEI Q\n" + keep_line_ops()
    return simple_page(pdf, content, b"/Font << /Helv " + str(helv).encode() + b" 0 R >>")


def evade_image_as_pattern() -> bytes:
    """The image is reached through a tiling pattern used as a fill colour.

    The page draws a rectangle. Everything about the image is one level down, in the pattern's
    own `/Resources`.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    image, w, h = _grey_image_stream(pdf, secret("IMG-PATTERN"), scale=2)
    x0, y0, x1, y1 = REGION
    pattern = pdf.stream(
        b"/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1"
        b" /BBox [0 0 " + f"{x1 - x0} {y1 - y0}".encode() + b"]"
        b" /XStep " + str(x1 - x0).encode() + b" /YStep " + str(y1 - y0).encode() +
        b" /Resources << /XObject << /Im1 " + str(image).encode() + b" 0 R >> >>",
        b"q " + f"{x1 - x0} 0 0 {y1 - y0} 0 0".encode() + b" cm /Im1 Do Q\n",
    )
    content = (
        b"q /Pattern cs /P1 scn "
        + f"{x0} {y0} {x1 - x0} {y1 - y0}".encode() + b" re f Q\n"
        + keep_line_ops()
    )
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /Pattern << /P1 " + str(pattern).encode() + b" 0 R >>"
    )
    return simple_page(pdf, content, res)


def evade_image_in_type3_glyph() -> bytes:
    """The image is drawn by a Type 3 glyph procedure — one level further than a Form XObject."""
    pdf = Pdf()
    helv = helvetica(pdf)
    image, _w, _h = _grey_image_stream(pdf, secret("IMG-TYPE3"), scale=2)
    x0, y0, x1, y1 = REGION
    proc = pdf.stream(
        b"",
        f"{x1 - x0} 0 0 0 {x1 - x0} {y1 - y0} d1\n".encode()
        + b"q " + f"{x1 - x0} 0 0 {y1 - y0} 0 0".encode() + b" cm /Im1 Do Q\n",
    )
    charprocs = pdf.add(b"<< /g " + str(proc).encode() + b" 0 R >>")
    encoding = pdf.add(b"<< /Type /Encoding /Differences [97 /g] >>")
    t3res = pdf.add(b"<< /XObject << /Im1 " + str(image).encode() + b" 0 R >> >>")
    t3 = pdf.add(
        b"<< /Type /Font /Subtype /Type3"
        b" /FontBBox [0 0 " + f"{x1 - x0} {y1 - y0}".encode() + b"]"
        b" /FontMatrix [1 0 0 1 0 0]"
        b" /CharProcs " + str(charprocs).encode() + b" 0 R"
        b" /Encoding " + str(encoding).encode() + b" 0 R"
        b" /FirstChar 97 /LastChar 97 /Widths [" + str(x1 - x0).encode() + b"]"
        b" /Resources " + str(t3res).encode() + b" 0 R >>"
    )
    content = (
        b"BT /T3 1 Tf " + f"{x0} {y0} Td ".encode() + literal("a") + b" Tj ET\n" + keep_line_ops()
    )
    res = (
        b"/Font << /T3 " + str(t3).encode() + b" 0 R /Helv " + str(helv).encode() + b" 0 R >>"
    )
    return simple_page(pdf, content, res)


def evade_text_in_type3_via_form() -> bytes:
    """A Type 3 glyph procedure that draws a FORM holding the text — it shows no text itself.

    `check_type_three_procedure` refused a procedure containing `Tj`/`TJ`/`'`/`"`. A procedure
    that draws a Form XObject shows none of those, and the walk enters neither the procedure nor
    the form. Measured by a security review: the operation returned `Ok`, the page's own `Tj` was
    removed so the output rendered **nothing** — zero dark pixels against 660 in the input, and
    PDFium extracted nothing — and the emitted file still carried the secret's drawing operators
    in full, recoverable with `qpdf --qdf`.

    "Covered, not gone" is ADR 0029 §8's forbidden outcome, and the reason `Do` now counts.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    x0, y0, x1, y1 = REGION
    held = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /Font << /Helv " + str(helv).encode() + b" 0 R >> >>",
        b"BT /Helv 20 Tf 0 0 Td " + literal(secret("TYPE3-VIA-FORM")) + b" Tj ET\n",
    )
    proc = pdf.stream(
        b"",
        f"{x1 - x0} 0 0 0 {x1 - x0} {y1 - y0} d1\n".encode() + b"/Held Do\n",
    )
    charprocs = pdf.add(b"<< /g " + str(proc).encode() + b" 0 R >>")
    encoding = pdf.add(b"<< /Type /Encoding /Differences [97 /g] >>")
    t3res = pdf.add(b"<< /XObject << /Held " + str(held).encode() + b" 0 R >> >>")
    t3 = pdf.add(
        b"<< /Type /Font /Subtype /Type3"
        b" /FontBBox [0 0 " + f"{x1 - x0} {y1 - y0}".encode() + b"]"
        b" /FontMatrix [1 0 0 1 0 0]"
        b" /CharProcs " + str(charprocs).encode() + b" 0 R"
        b" /Encoding " + str(encoding).encode() + b" 0 R"
        b" /FirstChar 97 /LastChar 97 /Widths [" + str(x1 - x0).encode() + b"]"
        b" /Resources " + str(t3res).encode() + b" 0 R >>"
    )
    content = (
        b"BT /T3 1 Tf " + f"{x0} {y0} Td ".encode() + literal("a") + b" Tj ET\n" + keep_line_ops()
    )
    res = b"/Font << /T3 " + str(t3).encode() + b" 0 R /Helv " + str(helv).encode() + b" 0 R >>"
    return simple_page(pdf, content, res)


def evade_type3_font_named_only_inside_a_form() -> bytes:
    """The Type 3 font is named by a FORM's `/Resources`, and the page has a decoy of that name.

    `check_type_three` collected `glyph.source.font` -- a name in whatever scope drew the glyph --
    and resolved every one against the PAGE's `/Font`. The page's `/T3` here is an ordinary Type 1,
    so the check `continue`s, the procedure is never read, and the `Do` rule added for
    `evade-text-in-type3-via-form` never runs. Measured by a security review: `Ok`, and the
    secret's drawing operators still in the output.

    Name versus identity, one dictionary down -- the crossing `core/CLAUDE.md` forbids and the
    same root cause as every other round on this branch.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    x0, y0, x1, y1 = REGION
    held = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /Font << /Helv " + str(helv).encode() + b" 0 R >> >>",
        b"BT /Helv 20 Tf 0 0 Td " + literal(secret("TYPE3-IN-FORM")) + b" Tj ET\n",
    )
    proc = pdf.stream(b"", f"{x1 - x0} 0 0 0 {x1 - x0} {y1 - y0} d1\n".encode() + b"/Held Do\n")
    charprocs = pdf.add(b"<< /g " + str(proc).encode() + b" 0 R >>")
    encoding = pdf.add(b"<< /Type /Encoding /Differences [97 /g] >>")
    t3res = pdf.add(b"<< /XObject << /Held " + str(held).encode() + b" 0 R >> >>")
    real_t3 = pdf.add(
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 " + f"{x1 - x0} {y1 - y0}".encode() + b"]"
        b" /FontMatrix [1 0 0 1 0 0]"
        b" /CharProcs " + str(charprocs).encode() + b" 0 R"
        b" /Encoding " + str(encoding).encode() + b" 0 R"
        b" /FirstChar 97 /LastChar 97 /Widths [" + str(x1 - x0).encode() + b"]"
        b" /Resources " + str(t3res).encode() + b" 0 R >>"
    )
    # THE FORM NAMES THE REAL ONE; the page names a decoy under the same name.
    wrapper = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /Font << /T3 " + str(real_t3).encode() + b" 0 R >> >>",
        b"BT /T3 1 Tf " + f"{x0} {y0} Td ".encode() + literal("a") + b" Tj ET\n",
    )
    decoy = pdf.add(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Name /T3 >>")
    content = b"/Wrap Do\n" + keep_line_ops()
    res = (
        b"/Font << /T3 " + str(decoy).encode() + b" 0 R /Helv " + str(helv).encode() + b" 0 R >>"
        b" /XObject << /Wrap " + str(wrapper).encode() + b" 0 R >>"
    )
    return simple_page(pdf, content, res)


def evade_tounicode_in_a_form_local_font() -> bytes:
    """The font whose `/ToUnicode` maps the removed codes is named only by a FORM.

    `cut_fonts` enumerated the page's `/Font` keys, so a font named only inside a form was never
    narrowed. Measured: both glyphs came out of the content stream, the report said `cut: true`
    for the page's Helvetica, and the form font's `/ToUnicode` still mapped `<0058>` and `<0059>`
    in the output.

    `/ToUnicode` IS the removed character, in plain text, beside the page it was cut from -- the
    channel `narrow_to_unicode` exists to close, reached by naming the font one dictionary down.
    ADR 0029 §6's read-back missed it too, because `mapped_codes` enumerated the page as well.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    cmap = (
        b"/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapType 2 def\n"
        b"1 begincodespacerange\n<20> <7e>\nendcodespacerange\n"
        b"2 beginbfchar\n<58> <0058>\n<59> <0059>\nendbfchar\nendcmap\n"
        b"CMapName currentdict /CMap defineresource pop\nend\nend\n"
    )
    tou = pdf.stream(b"", cmap)
    font = pdf.add(
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Name /FF"
        b" /ToUnicode " + str(tou).encode() + b" 0 R >>"
    )
    form = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /Font << /FF " + str(font).encode() + b" 0 R >> >>",
        b"BT /FF " + str(SECRET_SIZE).encode() + b" Tf "
        + f"{SECRET_X} {SECRET_Y} Td ".encode() + b"(XY) Tj ET\n",
    )
    content = b"/Fm Do\n" + keep_line_ops()
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /XObject << /Fm " + str(form).encode() + b" 0 R >>"
    )
    return simple_page(pdf, content, res)


def nearmiss_tounicode_on_a_page_font() -> bytes:
    """The same document with the font named by the PAGE. Always worked; proves the shape is fine.

    The twin for `evade-tounicode-in-a-form-local-font`: if narrowing broke for both, the evasion
    fixture would be measuring the narrowing rather than the scope it is named for.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    cmap = (
        b"/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapType 2 def\n"
        b"1 begincodespacerange\n<20> <7e>\nendcodespacerange\n"
        b"2 beginbfchar\n<58> <0058>\n<59> <0059>\nendbfchar\nendcmap\n"
        b"CMapName currentdict /CMap defineresource pop\nend\nend\n"
    )
    tou = pdf.stream(b"", cmap)
    font = pdf.add(
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Name /FF"
        b" /ToUnicode " + str(tou).encode() + b" 0 R >>"
    )
    content = (
        b"BT /FF " + str(SECRET_SIZE).encode() + b" Tf "
        + f"{SECRET_X} {SECRET_Y} Td ".encode() + b"(XY) Tj ET\n" + keep_line_ops()
    )
    res = b"/Font << /Helv " + str(helv).encode() + b" 0 R /FF " + str(font).encode() + b" 0 R >>"
    return simple_page(pdf, content, res)


def nearmiss_form_carrying_its_own_font() -> bytes:
    """A form with its own `/Resources /Font`, plain text, no Type 3. MUST NOT be refused.

    The availability half of the same defect, and the reason it matters beyond the leak: resolving
    a form's font name against the page alone failed this ORDINARY document with
    `font-missing: the content stream selects a font the page's resources do not name` -- blaming
    the file for a lookup that searched one scope. Producers emit this shape routinely.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    own = pdf.add(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Name /F9 >>")
    form = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /Font << /F9 " + str(own).encode() + b" 0 R >> >>",
        b"BT /F9 " + str(SECRET_SIZE).encode() + b" Tf "
        + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal(secret("FORM-OWN-FONT")) + b" Tj ET\n",
    )
    content = b"/Fm Do\n" + keep_line_ops()
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /XObject << /Fm " + str(form).encode() + b" 0 R >>"
    )
    return simple_page(pdf, content, res)


def nearmiss_type3_procedure_that_only_shows_its_own_glyph() -> bytes:
    """A Type 3 glyph procedure that draws a PATH and nothing else. MUST NOT be refused.

    The twin for `evade-text-in-type3-via-form`. `check_type_three_procedure` refuses a procedure
    containing `Tj`/`TJ`/`'`/`"`/`Do`, and a procedure that fills its own outline contains none of
    them — which is what a Type 3 font is normally *for*. A rule that refused every Type 3 font
    would pass the evasion and refuse a whole legitimate font type.

    The canary is drawn by Helvetica inside the region; the Type 3 glyph sits outside it.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    proc = pdf.stream(b"", b"20 0 0 0 20 20 d1\n0 0 18 18 re f\n")
    charprocs = pdf.add(b"<< /g " + str(proc).encode() + b" 0 R >>")
    encoding = pdf.add(b"<< /Type /Encoding /Differences [97 /g] >>")
    t3 = pdf.add(
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 20 20]"
        b" /FontMatrix [1 0 0 1 0 0]"
        b" /CharProcs " + str(charprocs).encode() + b" 0 R"
        b" /Encoding " + str(encoding).encode() + b" 0 R"
        b" /FirstChar 97 /LastChar 97 /Widths [20]"
        b" /Resources << >> >>"
    )
    content = (
        b"BT /Helv " + str(SECRET_SIZE).encode() + b" Tf "
        + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal(secret("TYPE3-NEARMISS")) + b" Tj ET\n"
        b"BT /T3 1 Tf 20 20 Td " + literal("a") + b" Tj ET\n" + keep_line_ops()
    )
    res = b"/Font << /T3 " + str(t3).encode() + b" 0 R /Helv " + str(helv).encode() + b" 0 R >>"
    return simple_page(pdf, content, res)


def nearmiss_image_outside_region() -> bytes:
    """An image on the page, nowhere near the region. MUST NOT be refused.

    A signal that refuses any document containing an image refuses most documents, which is an
    outage rather than a refusal.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    image, _w, _h = _grey_image_stream(pdf, "DECORATION", scale=2)
    content = (
        b"q 60 0 0 18 20 8 cm /Im1 Do Q\n"
        b"BT /Helv 20 Tf " + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal(secret("IMG-NEARMISS")) + b" Tj ET\n" + keep_line_ops()
    )
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /XObject << /Im1 " + str(image).encode() + b" 0 R >>"
    )
    return simple_page(pdf, content, res)


# ===========================================================================================
# Refusal 2 — "a region intersecting VECTOR PATH content". Same proposed signal, same gap.
# ===========================================================================================


def evade_paths_in_form() -> bytes:
    """The filled paths are inside a Form XObject."""
    pdf = Pdf()
    helv = helvetica(pdf)
    form = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << >>",
        outline_ops(secret("PATH-FORM"), SECRET_X, SECRET_Y, SECRET_SIZE),
    )
    content = b"/X1 Do\n" + keep_line_ops()
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /XObject << /X1 " + str(form).encode() + b" 0 R >>"
    )
    return simple_page(pdf, content, res)


def evade_paths_in_type3_glyph() -> bytes:
    """The filled paths are inside a Type 3 glyph procedure."""
    pdf = Pdf()
    helv = helvetica(pdf)
    proc = pdf.stream(
        b"",
        b"200 0 0 0 200 20 d0\n" + outline_ops(secret("PATH-TYPE3"), 0, 0, SECRET_SIZE),
    )
    charprocs = pdf.add(b"<< /g " + str(proc).encode() + b" 0 R >>")
    encoding = pdf.add(b"<< /Type /Encoding /Differences [97 /g] >>")
    t3 = pdf.add(
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 200 20]"
        b" /FontMatrix [1 0 0 1 0 0]"
        b" /CharProcs " + str(charprocs).encode() + b" 0 R"
        b" /Encoding " + str(encoding).encode() + b" 0 R"
        b" /FirstChar 97 /LastChar 97 /Widths [200]"
        b" /Resources << >> >>"
    )
    content = (
        b"BT /T3 1 Tf " + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal("a") + b" Tj ET\n" + keep_line_ops()
    )
    res = (
        b"/Font << /T3 " + str(t3).encode() + b" 0 R /Helv " + str(helv).encode() + b" 0 R >>"
    )
    return simple_page(pdf, content, res)


def nearmiss_paths_outside_region() -> bytes:
    """A ruled line and a border, nowhere near the region. MUST NOT be refused.

    Almost every business document has a rule, a table border or a logo. A path signal that
    refuses on their existence refuses nearly everything.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    content = (
        b"0 g 20 6 360 2 re f\n"
        b"0.5 w 10 4 380 192 re S\n"
        b"BT /Helv 20 Tf " + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal(secret("PATH-NEARMISS")) + b" Tj ET\n" + keep_line_ops()
    )
    return simple_page(pdf, content, b"/Font << /Helv " + str(helv).encode() + b" 0 R >>")


# ===========================================================================================
# Refusal 3 — "/AcroForm". Proposed signal: an /Annots entry with /Subtype /Widget.
# ===========================================================================================


def evade_widget_on_another_page() -> bytes:
    """The form field's widget is on page 2. Page 1 is the one being redacted.

    ADR 0029 §5 names this bypass in those words. The field's `/V` -- the value somebody typed --
    is on the catalogue, reachable from neither page.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    pages = pdf.reserve()
    page1 = pdf.reserve()
    page2 = pdf.reserve()
    field = pdf.reserve()
    acroform = pdf.reserve()
    ap = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 200 24]"
        b" /Resources << /Font << /Helv " + str(helv).encode() + b" 0 R >> >>",
        b"/Tx BMC q BT /Helv 14 Tf 2 6 Td " + literal(secret("FORM-OTHERPAGE")) + b" Tj ET Q EMC\n",
    )
    pdf.put(
        field,
        b"<< /Type /Annot /Subtype /Widget /FT /Tx /T " + literal("field-on-page-two") +
        b" /V " + utf16_hex(secret("FORM-OTHERPAGE")) +
        b" /DA (/Helv 14 Tf 0 g) /F 4 /Rect [40 40 240 64]"
        b" /P " + str(page2).encode() + b" 0 R"
        b" /AP << /N " + str(ap).encode() + b" 0 R >> >>",
    )
    pdf.put(
        acroform,
        b"<< /Fields [" + str(field).encode() + b" 0 R] /DA (/Helv 0 Tf 0 g)"
        b" /DR << /Font << /Helv " + str(helv).encode() + b" 0 R >> >> >>",
    )
    body1 = pdf.stream(
        b"",
        b"BT /Helv 20 Tf " + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal("PAGE-ONE-TEXT") + b" Tj ET\n" + keep_line_ops(),
    )
    body2 = pdf.stream(b"", keep_line_ops())
    common = (
        b" /MediaBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /Font << /Helv " + str(helv).encode() + b" 0 R >> >>"
    )
    pdf.put(
        page1,
        b"<< /Type /Page /Parent " + str(pages).encode() + b" 0 R" + common
        + b" /Contents " + str(body1).encode() + b" 0 R >>",
    )
    pdf.put(
        page2,
        b"<< /Type /Page /Parent " + str(pages).encode() + b" 0 R" + common
        + b" /Annots [" + str(field).encode() + b" 0 R]"
        b" /Contents " + str(body2).encode() + b" 0 R >>",
    )
    pdf.put(
        pages,
        b"<< /Type /Pages /Count 2 /Kids ["
        + str(page1).encode() + b" 0 R " + str(page2).encode() + b" 0 R] >>",
    )
    root = pdf.add(
        b"<< /Type /Catalog /Pages " + str(pages).encode() + b" 0 R"
        b" /AcroForm " + str(acroform).encode() + b" 0 R >>"
    )
    return pdf.build(root)


def evade_field_with_no_widget() -> bytes:
    """An /AcroForm field with no widget anywhere. No /Annots entry on any page names it."""
    pdf = Pdf()
    helv = helvetica(pdf)
    field = pdf.add(
        b"<< /FT /Tx /T " + literal("widgetless") +
        b" /V " + utf16_hex(secret("FORM-NOWIDGET")) + b" >>"
    )
    acroform = pdf.add(
        b"<< /Fields [" + str(field).encode() + b" 0 R] /DA (/Helv 0 Tf 0 g) >>"
    )
    content = (
        b"BT /Helv 20 Tf " + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal("PAGE-TEXT-ONLY") + b" Tj ET\n" + keep_line_ops()
    )
    return simple_page(
        pdf,
        content,
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>",
        catalog_extra=b" /AcroForm " + str(acroform).encode() + b" 0 R",
    )


def nearmiss_annotation_not_a_widget() -> bytes:
    """A plain text annotation and no /AcroForm at all. MUST NOT be refused.

    Keys on `/Subtype /Widget`, so an ordinary sticky note must pass. A signal that refuses any
    annotated document refuses a very large share of real ones.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    annot = pdf.add(
        b"<< /Type /Annot /Subtype /Text /Rect [300 10 320 30] /F 4"
        b" /Contents " + literal("an ordinary note, not a form field") + b" >>"
    )
    content = (
        b"BT /Helv 20 Tf " + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal(secret("FORM-NEARMISS")) + b" Tj ET\n" + keep_line_ops()
    )
    return simple_page(
        pdf,
        content,
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>",
        page_extra=b" /Annots [" + str(annot).encode() + b" 0 R]",
    )


# ===========================================================================================
# Refusal 4 — "/StructTreeRoot reaching the region". Proposed signal: the page's /StructParents.
# ===========================================================================================


def evade_struct_without_structparents() -> bytes:
    """A /StructElem reaches this page's marked content, and the page carries NO /StructParents.

    ADR 0029 §5 names this bypass too. The element points at the page through `/Pg`, so the
    structure tree reaches it; the page has nothing that points back.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    pages = pdf.reserve()
    page = pdf.reserve()
    struct_root = pdf.reserve()
    elem = pdf.reserve()
    content = (
        b"/P << /MCID 0 >> BDC\n"
        b"BT /Helv 20 Tf " + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal("MARKED-CONTENT-HERE") + b" Tj ET\nEMC\n" + keep_line_ops()
    )
    stream = pdf.stream(b"", content)
    pdf.put(
        elem,
        b"<< /Type /StructElem /S /P /P " + str(struct_root).encode() + b" 0 R"
        b" /Pg " + str(page).encode() + b" 0 R /K 0"
        b" /ActualText " + literal(secret("STRUCT-NOPARENTS")) + b" >>",
    )
    pdf.put(struct_root, b"<< /Type /StructTreeRoot /K [" + str(elem).encode() + b" 0 R] >>")
    # NO /StructParents on the page. That is the evasion.
    pdf.put(
        page,
        b"<< /Type /Page /Parent " + str(pages).encode() + b" 0 R"
        b" /MediaBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /Font << /Helv " + str(helv).encode() + b" 0 R >> >>"
        b" /Contents " + str(stream).encode() + b" 0 R >>",
    )
    pdf.put(pages, b"<< /Type /Pages /Count 1 /Kids [" + str(page).encode() + b" 0 R] >>")
    root = pdf.add(
        b"<< /Type /Catalog /Pages " + str(pages).encode() + b" 0 R"
        b" /StructTreeRoot " + str(struct_root).encode() + b" 0 R"
        b" /MarkInfo << /Marked true >> >>"
    )
    return pdf.build(root)


def nearmiss_structparents_but_nothing_in_region() -> bytes:
    """A tagged document whose structure element covers content OUTSIDE the region.

    MUST NOT be refused. Nearly every accessible PDF is tagged; refusing all of them is an
    outage. The signal has to be "reaches the region", not "the document is tagged".
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    pages = pdf.reserve()
    page = pdf.reserve()
    struct_root = pdf.reserve()
    elem = pdf.reserve()
    content = (
        b"BT /Helv 20 Tf " + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal(secret("STRUCT-NEARMISS")) + b" Tj ET\n"
        b"/P << /MCID 0 >> BDC\n" + keep_line_ops() + b"EMC\n"
    )
    stream = pdf.stream(b"", content)
    pdf.put(
        elem,
        b"<< /Type /StructElem /S /P /P " + str(struct_root).encode() + b" 0 R"
        b" /Pg " + str(page).encode() + b" 0 R /K 0"
        b" /ActualText " + literal(KEEP_LINE) + b" >>",
    )
    pdf.put(struct_root, b"<< /Type /StructTreeRoot /K [" + str(elem).encode() + b" 0 R] >>")
    pdf.put(
        page,
        b"<< /Type /Page /Parent " + str(pages).encode() + b" 0 R"
        b" /MediaBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /Font << /Helv " + str(helv).encode() + b" 0 R >> >>"
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


# ===========================================================================================
# Refusal 5 — optional content. The ONE signal ADR 0029 §5 marks measured. It still owes depth:
# its evade fixture (`oc-nested.pdf`) pins ONE level and the code walks deeper.
# ===========================================================================================


def evade_oc_two_levels_down() -> bytes:
    """An OCG referenced by a form inside a form — two levels, where the fixture pins one."""
    pdf = Pdf()
    helv = helvetica(pdf)
    ocg = pdf.add(b"<< /Type /OCG /Name " + literal("Layer holding " + secret("OC-DEPTH2")) + b" >>")
    inner = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /Properties << /MC0 " + str(ocg).encode() + b" 0 R >> >>",
        b"/OC /MC0 BDC\nBT /Helv 20 Tf " + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal(secret("OC-DEPTH2")) + b" Tj ET\nEMC\n",
    )
    outer = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /XObject << /Inner " + str(inner).encode() + b" 0 R >> >>",
        b"/Inner Do\n",
    )
    content = b"/Outer Do\n" + keep_line_ops()
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /XObject << /Outer " + str(outer).encode() + b" 0 R >>"
    )
    return simple_page(
        pdf,
        content,
        res,
        catalog_extra=(
            b" /OCProperties << /OCGs [" + str(ocg).encode() + b" 0 R]"
            b" /D << /OFF [" + str(ocg).encode() + b" 0 R] >> >>"
        ),
    )


def nearmiss_nested_forms_no_oc() -> bytes:
    """A form inside a form, with NO optional content anywhere. MUST NOT be refused.

    The twin for `evade-oc-two-levels-down`. The depth-2 walk has to distinguish "an OCG two
    levels down" from "a form two levels down", and nested forms are ordinary — every drawing
    program emits them. A signal that refuses on nesting alone refuses a large share of real
    documents, which is an outage rather than a refusal.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    inner = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /Font << /Helv " + str(helv).encode() + b" 0 R >> >>",
        b"BT /Helv 20 Tf " + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal(secret("OC-NEARMISS")) + b" Tj ET\n",
    )
    outer = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /XObject << /Inner " + str(inner).encode() + b" 0 R >> >>",
        b"/Inner Do\n",
    )
    content = b"/Outer Do\n" + keep_line_ops()
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /XObject << /Outer " + str(outer).encode() + b" 0 R >>"
    )
    return simple_page(pdf, content, res)


# ===========================================================================================

def _form_drawing_the_secret(pdf: Pdf, helv: int) -> int:
    """A Form XObject drawing the canary at the region's origin, in the page's Helvetica."""
    return pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 "
        + f"{PAGE_W} {PAGE_H}".encode()
        + b"] /Resources << /Font << /Helv "
        + str(helv).encode()
        + b" 0 R >> >>",
        b"BT /Helv "
        + str(SECRET_SIZE).encode()
        + b" Tf "
        + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal(secret("ACTUALTEXT-FORM"))
        + b" Tj ET\n",
    )


def evade_actualtext_around_a_form() -> bytes:
    """The `/ActualText` is in the PAGE stream; the glyphs it covers are in a Form XObject.

    The cross-stream case. `check_marked_content` is called once per stream, and each call sees
    only its own stream's removed glyphs -- so a `BDC` on the page wrapping a `Do` was seen by
    neither: the page call had no removed glyph of its own, and the form's stream has no `BDC`.

    Measured before the fix: the operation returned `Ok`, the form's glyphs were removed, and
    both PDFium and `pdftotext` read the carrier off the output. Marked content descends through
    `Do`; the walk did not.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    form = _form_drawing_the_secret(pdf, helv)
    content = (
        b"/Span << /ActualText " + literal(secret("ACTUALTEXT-FORM")) + b" >> BDC\n"
        b"/X1 Do\n"
        b"EMC\n" + keep_line_ops()
    )
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /XObject << /X1 " + str(form).encode() + b" 0 R >>"
    )
    return simple_page(pdf, content, res)


def evade_actualtext_inside_a_form() -> bytes:
    """The `/ActualText` and the glyphs it covers are both inside a Form XObject.

    The twin of `evade-actualtext-around-a-form`, one level in. It exists because the per-form
    `check_marked_content` pass could be deleted outright with the whole suite green: every
    fixture kept its `BDC` in the page stream, so the form branch was unverified code on a leak
    path. A security review measured that as a surviving mutation.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    form = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 "
        + f"{PAGE_W} {PAGE_H}".encode()
        + b"] /Resources << /Font << /Helv "
        + str(helv).encode()
        + b" 0 R >> >>",
        b"/Span << /ActualText " + literal(secret("ACTUALTEXT-IN-FORM")) + b" >> BDC\n"
        b"BT /Helv "
        + str(SECRET_SIZE).encode()
        + b" Tf "
        + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal(secret("ACTUALTEXT-IN-FORM"))
        + b" Tj ET\n"
        b"EMC\n",
    )
    content = b"/X1 Do\n" + keep_line_ops()
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /XObject << /X1 " + str(form).encode() + b" 0 R >>"
    )
    return simple_page(pdf, content, res)


def evade_actualtext_around_a_nested_form() -> bytes:
    """The `/ActualText` is on the page; the glyphs are two levels down, inside a nested form.

    The deeper twin of `evade-actualtext-around-a-form`, and it exists because **fixing one bug
    re-opened another**. While #164 refused every nested form with `form-vanished`, a removed
    glyph's form was always a direct child of the page, so matching the page's `/XObject` by
    object identity was enough to answer "does this `Do` draw something the removal reaches".

    Making nested forms redactable breaks that assumption: the removed glyph's form is `Inner`,
    which the page's `/XObject` never names, so the name set came back empty and the page's
    `/ActualText` span had nothing to answer for. The same cross-stream hole a security review
    measured, one level further down.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    inner = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /Font << /Helv " + str(helv).encode() + b" 0 R >> >>",
        b"BT /Helv " + str(SECRET_SIZE).encode() + b" Tf "
        + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal(secret("ACTUALTEXT-NESTED")) + b" Tj ET\n",
    )
    outer = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /XObject << /Inner " + str(inner).encode() + b" 0 R >> >>",
        b"/Inner Do\n",
    )
    content = (
        b"/Span << /ActualText " + literal(secret("ACTUALTEXT-NESTED")) + b" >> BDC\n"
        b"/Outer Do\n"
        b"EMC\n" + keep_line_ops()
    )
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /XObject << /Outer " + str(outer).encode() + b" 0 R >>"
    )
    return simple_page(pdf, content, res)


def evade_actualtext_in_the_middle_form() -> bytes:
    """The `/ActualText` lives in the INTERMEDIATE form, not the page and not the leaf.

    `check_marked_content` ran on the page stream and on each form *holding* a removed glyph. A
    form that merely **draws** such a form was in neither set, so its bytes were read by nobody
    -- and a span sitting there covered glyphs nothing checked. Measured by a security review:
    the operation returned `Ok` and PDFium read the carrier off the output.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    inner = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /Font << /Helv " + str(helv).encode() + b" 0 R >> >>",
        b"BT /Helv " + str(SECRET_SIZE).encode() + b" Tf "
        + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal(secret("ACTUALTEXT-MIDFORM")) + b" Tj ET\n",
    )
    outer = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /XObject << /Inner " + str(inner).encode() + b" 0 R >> >>",
        b"/Span << /ActualText " + literal(secret("ACTUALTEXT-MIDFORM")) + b" >> BDC\n"
        b"/Inner Do\n"
        b"EMC\n",
    )
    content = b"/Outer Do\n" + keep_line_ops()
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /XObject << /Outer " + str(outer).encode() + b" 0 R >>"
    )
    return simple_page(pdf, content, res)


def evade_actualtext_over_a_form_without_resources() -> bytes:
    """The covered form declares NO `/Resources`, so its children resolve against the page's.

    `Resources::within` returns `None` for such a form and the walk continues with the enclosing
    dictionary -- so the glyph's form is a page-level object. The scope walk returned "reaches
    nothing" instead, which unlinked the page's `Do` from the wanted set: deleting one dictionary
    from the file turned a correct refusal into a leak. Anything resolving names differently from
    the walk is a bypass by construction.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    inner = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /Font << /Helv " + str(helv).encode() + b" 0 R >> >>",
        b"BT /Helv " + str(SECRET_SIZE).encode() + b" Tf "
        + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal(secret("ACTUALTEXT-NORES")) + b" Tj ET\n",
    )
    # NO /Resources AT ALL on this one -- that is the whole fixture.
    outer = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]",
        b"/Inner Do\n",
    )
    content = (
        b"/Span << /ActualText " + literal(secret("ACTUALTEXT-NORES")) + b" >> BDC\n"
        b"/Outer Do\n"
        b"EMC\n" + keep_line_ops()
    )
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /XObject << /Outer " + str(outer).encode() + b" 0 R"
        b" /Inner " + str(inner).encode() + b" 0 R >>"
    )
    return simple_page(pdf, content, res)


def evade_actualtext_under_a_form_with_two_parents() -> bytes:
    """The covered form is reachable by TWO routes, and only one of them resolves its names.

    `scope_of` memoised each form's leading names with `or_insert`, and those names are not a
    property of the form: a form declaring no `/Resources` resolves its children against whatever
    encloses it. Reached from the page it inherits the page's names; reached through the
    intermediate form it resolves to the forms actually holding the glyphs. First path won, and
    when that was the page route the span around it was never examined.

    Measured by a code review: `Ok`, with the carrier in the output. The single-parent control is
    `evade-actualtext-in-the-middle-form`, which refuses correctly — so the difference between
    leaking and refusing is one extra, undrawn reference in the page's `/XObject`.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    held_a = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /Font << /Helv " + str(helv).encode() + b" 0 R >> >>",
        b"BT /Helv " + str(SECRET_SIZE).encode() + b" Tf "
        + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal(secret("ACTUALTEXT-TWOPARENT-A")) + b" Tj ET\n",
    )
    held_b = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /Font << /Helv " + str(helv).encode() + b" 0 R >> >>",
        b"BT /Helv " + str(SECRET_SIZE).encode() + b" Tf "
        + f"{SECRET_X} {SECRET_Y - SECRET_SIZE} Td ".encode()
        + literal(secret("ACTUALTEXT-TWOPARENT")) + b" Tj ET\n",
    )
    # NO /Resources: what `/W1` and `/W2` mean here depends on which route reached this form.
    middle = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]",
        b"/W1 Do\n"
        b"/Span << /ActualText " + literal(secret("ACTUALTEXT-TWOPARENT")) + b" >> BDC\n"
        b"/W2 Do\n"
        b"EMC\n",
    )
    outer = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /XObject << /X " + str(middle).encode() + b" 0 R"
        b" /W1 " + str(held_a).encode() + b" 0 R"
        b" /W2 " + str(held_b).encode() + b" 0 R >> >>",
        b"/X Do\n",
    )
    content = b"/Y Do\n" + keep_line_ops()
    # `/X` ALSO NAMED HERE, and undrawn. That second reference is the whole fixture: it gives
    # the walk a route to `X` that resolves its names against the page instead.
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /XObject << /Y " + str(outer).encode() + b" 0 R"
        b" /X " + str(middle).encode() + b" 0 R >>"
    )
    return simple_page(pdf, content, res)


def evade_actualtext_on_a_page_that_draws_nothing_itself() -> bytes:
    """The page's ONLY involvement is the covering span: its own text is outside the region.

    `affected_streams` builds the stream list from the removed glyphs and then widens it to the
    streams that carry a span. A code review showed the widening was dead in every test: each
    existing fixture's keep line falls inside the band `redaction_defences` redacts, so the page
    already had removed glyphs of its own and was already in the list. Deleting the page half of
    the widening survived the whole suite.

    Here the keep line sits at the very bottom of the page, below the band, so the page
    contributes no removed glyph at all — and the stream holding the text to drop is the page.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    form = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /Font << /Helv " + str(helv).encode() + b" 0 R >> >>",
        b"BT /Helv " + str(SECRET_SIZE).encode() + b" Tf "
        + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal(secret("ACTUALTEXT-BARE-PAGE")) + b" Tj ET\n",
    )
    # THE KEEP LINE AT y=5, not `keep_line_ops()`'s y=40: 40 is inside the band the defence
    # tests redact, which is what made every other fixture's page a removal site by accident.
    content = (
        b"/Span << /ActualText " + literal(secret("ACTUALTEXT-BARE-PAGE")) + b" >> BDC\n"
        b"/X1 Do\n"
        b"EMC\n"
        b"BT /Helv 10 Tf 40 5 Td " + literal(KEEP_LINE) + b" Tj ET\n"
    )
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /XObject << /X1 " + str(form).encode() + b" 0 R >>"
    )
    return simple_page(pdf, content, res)


def nearmiss_actualtext_around_an_untouched_form() -> bytes:
    """The same shape, with the `/ActualText` span around a form the region never reaches.

    The twin that stops the rule being "refuse any page whose content has a `/ActualText`".
    The covered form draws the keep line, well outside the region; the canary is drawn by the
    page itself, outside the span. A check that refuses this refuses most tagged documents.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    elsewhere = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 "
        + f"{PAGE_W} {PAGE_H}".encode()
        + b"] /Resources << /Font << /Helv "
        + str(helv).encode()
        + b" 0 R >> >>",
        keep_line_ops(),
    )
    content = (
        b"/Span << /ActualText " + literal(KEEP_LINE) + b" >> BDC\n"
        b"/X1 Do\n"
        b"EMC\n"
        b"BT /Helv " + str(SECRET_SIZE).encode() + b" Tf "
        + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal(secret("ACTUALTEXT-NEARMISS"))
        + b" Tj ET\n"
    )
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /XObject << /X1 " + str(elsewhere).encode() + b" 0 R >>"
    )
    return simple_page(pdf, content, res)


def evade_actualtext_named_outside_key_position() -> bytes:
    """The `/ActualText` name is an ARRAY ITEM, not a key, and a string sits beside it.

    The residue of narrowing the detector to keys. The wide detector matched a name anywhere, so
    this classified as carrying text; the rewriter only removes keys, so it removed nothing and
    produced a replacement byte-identical to its input. Measured: the string reached the output
    and a raw byte scan found it.

    Refused by `marked-content-carries-opaque-string`, which is exactly the gap between the two
    readings. The glyphs are drawn by the page inside the span, so the span genuinely covers a
    removal and the rule is reached.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    content = (
        b"/Span << /MCID 0 /K [ /ActualText "
        + literal(secret("ACTUALTEXT-NOT-A-KEY"))
        + b" ] >> BDC\n"
        b"BT /Helv " + str(SECRET_SIZE).encode() + b" Tf "
        + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal(secret("ACTUALTEXT-NOT-A-KEY"))
        + b" Tj ET\n"
        b"EMC\n" + keep_line_ops()
    )
    return simple_page(pdf, content, b"/Font << /Helv " + str(helv).encode() + b" 0 R >>")


def nearmiss_ordinary_string_in_a_property_list() -> bytes:
    """The same covering span, carrying `/Lang (en-US)` instead.

    The twin that stops the rule being "refuse any property list holding a string". `/Lang` is
    ordinary tagged output and repeats no glyphs; the first version of this rule refused it, and
    would have refused most tagged documents. The canary is drawn inside the span and must be
    redacted rather than refused.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    content = (
        b"/Span << /MCID 0 /Lang (en-US) >> BDC\n"
        b"BT /Helv " + str(SECRET_SIZE).encode() + b" Tf "
        + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal(secret("ACTUALTEXT-ORDINARY-STRING"))
        + b" Tj ET\n"
        b"EMC\n" + keep_line_ops()
    )
    return simple_page(pdf, content, b"/Font << /Helv " + str(helv).encode() + b" 0 R >>")


# ===========================================================================================
# #166 — a marked-content property list NAMED through `/Properties`. A name is scoped: `/MC0`
# resolves in the `/Properties` of whatever drew the stream -- the page, or a form's own
# `/Resources`, or, for a form declaring none, its enclosure. Five consumers got that wrong for
# fonts; these put the list somewhere a page-level answer gets wrong in each direction.
# ===========================================================================================


def _secret_run(tag: str) -> bytes:
    """The canary drawn at the region, as a text object."""
    return (
        b"BT /Helv " + str(SECRET_SIZE).encode() + b" Tf "
        + f"{SECRET_X} {SECRET_Y} Td ".encode()
        + literal(secret(tag)) + b" Tj ET\n"
    )


def evade_actualtext_named_through_properties() -> bytes:
    """`/Span /MC0 BDC` over the canary, `/MC0` carrying `/ActualText` in the page's `/Properties`.

    The plain named shape. The text is in a resource dictionary rather than the stream, so the
    rewriter has nothing to drop, and dropping it from the resource would edit every span that
    names it. Refused by `marked-content-named-properties-carry-text`.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    content = b"/Span /MC0 BDC\n" + _secret_run("ACTUALTEXT-NAMED") + b"EMC\n" + keep_line_ops()
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /Properties << /MC0 << /ActualText " + literal(secret("ACTUALTEXT-NAMED")) + b" >> >>"
    )
    return simple_page(pdf, content, res)


def evade_actualtext_named_in_a_form_scope() -> bytes:
    """The span is in a form, and the form's OWN `/Properties` carries the text. The page's does not.

    A resolver answering from the page would read the page's `/MC0` -- an ordinary `/MCID` list --
    and pass the span, emitting the form's `/ActualText` untouched. The font defect, in property
    lists: a decoy in the wrong scope shadowing the one that is read.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    form = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /Properties << /MC0 << /ActualText " + literal(secret("ACTUALTEXT-NAMED-FORM"))
        + b" >> >> >>",
        b"/Span /MC0 BDC\n" + _secret_run("ACTUALTEXT-NAMED-FORM") + b"EMC\n",
    )
    content = b"/X1 Do\n" + keep_line_ops()
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /XObject << /X1 " + str(form).encode() + b" 0 R >>"
        b" /Properties << /MC0 << /MCID 0 >> >>"
    )
    return simple_page(pdf, content, res)


def evade_actualtext_named_in_a_form_that_inherits() -> bytes:
    """The span is in a form declaring NO `/Resources`, so `/MC0` resolves in the page's.

    The other direction of the scope rule: a resolver that looked only in a form's own resources
    would find nothing here and, if it read "nothing" as "no text", pass the span. It must
    inherit exactly as `Resources::within` does for fonts.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    form = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]",
        b"/Span /MC0 BDC\n" + _secret_run("ACTUALTEXT-NAMED-INHERITED") + b"EMC\n",
    )
    content = b"/X1 Do\n" + keep_line_ops()
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /XObject << /X1 " + str(form).encode() + b" 0 R >>"
        b" /Properties << /MC0 << /ActualText "
        + literal(secret("ACTUALTEXT-NAMED-INHERITED")) + b" >> >>"
    )
    return simple_page(pdf, content, res)


def evade_actualtext_behind_a_reference_in_named_properties() -> bytes:
    """`/MC0` resolves, and holds an INDIRECT REFERENCE whose target carries `/ActualText`.

    Resolving the name reads the list itself; a reference inside it is a door that was not
    opened, and the object behind it could hold the text. Refused as unresolved, not passed.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    hidden = pdf.add(b"<< /ActualText " + literal(secret("ACTUALTEXT-NAMED-REF")) + b" >>")
    content = (
        b"/Span /MC0 BDC\n" + _secret_run("ACTUALTEXT-NAMED-REF") + b"EMC\n" + keep_line_ops()
    )
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /Properties << /MC0 << /MCID 0 /Pad " + str(hidden).encode() + b" 0 R >> >>"
    )
    return simple_page(pdf, content, res)


def nearmiss_named_properties_without_text() -> bytes:
    """`/P /MC0 BDC` over the canary, `/MC0` an ordinary `/MCID` list. MUST be redacted.

    The twin that measures whether the resolver bought anything: before it, every named list
    refused as unresolved, including this one, which is what tagged output looks like.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    content = (
        b"/P /MC0 BDC\n" + _secret_run("NAMED-NEARMISS") + b"EMC\n" + keep_line_ops()
    )
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /Properties << /MC0 << /MCID 0 >> >>"
    )
    return simple_page(pdf, content, res)


def nearmiss_named_actualtext_outside_the_region() -> bytes:
    """A named span carrying `/ActualText` around a footnote, and the canary outside any span.

    MUST be redacted. The refusal is scoped to spans the removal is inside, as the form rule is
    scoped to glyphs being removed; a carrying span elsewhere on the page is not a bar. Its
    `/ActualText` is not the canary -- it survives, untouched, because nothing asked about it.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    # ITS OWN LINE, AT THE FOOT OF THE PAGE. Around the keep line it sat inside the band
    # `redaction_defences.rs` redacts, which covers the keep line -- so the span was genuinely
    # covered and the refusal was right. A near-miss has to be outside EVERY region a harness
    # asks about, and y=10 is below both that band and the census's single-glyph region.
    content = (
        _secret_run("NAMED-OUTSIDE") + keep_line_ops()
        + b"/Span /MC0 BDC BT /Helv 8 Tf 40 10 Td " + literal("SPANNED-FOOTNOTE")
        + b" Tj ET EMC\n"
    )
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /Properties << /MC0 << /ActualText " + literal("ALT-FOR-THE-KEPT-LINE") + b" >> >>"
    )
    return simple_page(pdf, content, res)


def nearmiss_named_properties_decoy_on_the_page() -> bytes:
    """The span is in a form whose OWN `/MC0` is ordinary; the page's `/MC0` carries text.

    MUST be redacted. The twin of `evade-actualtext-named-in-a-form-scope`: a resolver answering
    from the page would read the page's carrying list and refuse a document it can handle. Scope
    errors fail in both directions, and only this one shows the refusing direction.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    form = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /Properties << /MC0 << /MCID 0 >> >> >>",
        b"/P /MC0 BDC\n" + _secret_run("NAMED-DECOY") + b"EMC\n",
    )
    content = b"/X1 Do\n" + keep_line_ops()
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /XObject << /X1 " + str(form).encode() + b" 0 R >>"
        b" /Properties << /MC0 << /ActualText " + literal("ALT-DECOY-ON-THE-PAGE") + b" >> >>"
    )
    return simple_page(pdf, content, res)


# ===========================================================================================
# #166 — optional content, which redaction never refused. ADR 0029 §3 refuses optional content
# referenced by a kept page, and until #166 `13-optional-content.pdf` refused only because its
# span was named and nothing resolved names. These are the shapes a region-scoped or
# `/Properties`-only signal walks past.
# ===========================================================================================


def _ocg(pdf: Pdf, label: str) -> int:
    return pdf.add(b"<< /Type /OCG /Name " + literal(label) + b" >>")


def _layer_off(ocg: int) -> bytes:
    return (
        b" /OCProperties << /OCGs [" + str(ocg).encode() + b" 0 R]"
        b" /D << /OFF [" + str(ocg).encode() + b" 0 R] >> >>"
    )


def evade_oc_outside_the_region() -> bytes:
    """A hidden layer on the page, nowhere near the region. The canary is drawn in plain view.

    A refusal keyed on the spans the removal is inside -- the way the marked-content rules are
    scoped -- walks straight past this. §3's rule is about the page, not the region.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    ocg = _ocg(pdf, "a layer elsewhere on the page")
    content = (
        _secret_run("OC-ELSEWHERE")
        + b"/OC /L0 BDC BT /Helv 10 Tf 40 175 Td " + literal("HIDDEN-LAYER-TEXT")
        + b" Tj ET EMC\n" + keep_line_ops()
    )
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /Properties << /L0 " + str(ocg).encode() + b" 0 R >>"
    )
    return simple_page(pdf, content, res, catalog_extra=_layer_off(ocg))


def evade_oc_on_an_annotation() -> bytes:
    """The layer is an annotation's `/OC`. Nothing in `/Resources` names it."""
    pdf = Pdf()
    helv = helvetica(pdf)
    ocg = _ocg(pdf, "a layered note")
    annot = pdf.add(
        b"<< /Type /Annot /Subtype /Text /Rect [300 170 320 190] /F 4"
        b" /Contents " + literal("a note on a hidden layer")
        + b" /OC " + str(ocg).encode() + b" 0 R >>"
    )
    return simple_page(
        pdf,
        _secret_run("OC-ANNOT") + keep_line_ops(),
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>",
        page_extra=b" /Annots [" + str(annot).encode() + b" 0 R]",
        catalog_extra=_layer_off(ocg),
    )


def evade_oc_on_an_image() -> bytes:
    """The layer is an IMAGE XObject's `/OC`. No `BDC` names it and it is not a form."""
    pdf = Pdf()
    helv = helvetica(pdf)
    ocg = _ocg(pdf, "a layered image")
    image = pdf.stream(
        b"/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray"
        b" /BitsPerComponent 8 /OC " + str(ocg).encode() + b" 0 R",
        b"\x00",
    )
    content = _secret_run("OC-IMAGE") + b"q 20 0 0 20 300 170 cm /Im0 Do Q\n" + keep_line_ops()
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /XObject << /Im0 " + str(image).encode() + b" 0 R >>"
    )
    return simple_page(pdf, content, res, catalog_extra=_layer_off(ocg))


def evade_oc_inside_an_unused_pattern() -> bytes:
    """The layer is in a tiling pattern's own `/Resources`, and the page never fills with it.

    Unused on purpose: a pattern the page fills with is refused by `pattern-may-draw-text`, which
    would answer first and leave this reach unmeasured. The page still references the pattern,
    and the pattern references the layer.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    ocg = _ocg(pdf, "a layer inside a pattern")
    # A MEMBERSHIP DICTIONARY, not the group itself: the one fixture where `/Type /OCMD` is what
    # the walk has to recognise. A security review's mutation dropping that arm survived.
    ocmd = pdf.add(b"<< /Type /OCMD /OCGs [" + str(ocg).encode() + b" 0 R] >>")
    pattern = pdf.stream(
        b"/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10]"
        b" /XStep 10 /YStep 10 /Resources << /Properties << /L0 " + str(ocmd).encode()
        + b" 0 R >> >>",
        b"/OC /L0 BDC 0 0 5 5 re f EMC\n",
    )
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /Pattern << /P0 " + str(pattern).encode() + b" 0 R >>"
    )
    return simple_page(
        pdf, _secret_run("OC-PATTERN") + keep_line_ops(), res, catalog_extra=_layer_off(ocg)
    )


def evade_oc_inside_an_unused_type3_font() -> bytes:
    """The layer is in a Type 3 font's `/Resources`, and the page never draws with the font.

    Unused for the same reason as the pattern: a drawn Type 3 font's procedures are checked by
    the Type 3 rule, which would answer first.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    ocg = _ocg(pdf, "a layer inside a Type 3 font")
    proc = pdf.stream(b"", b"1000 0 d0 /OC /L0 BDC 0 0 500 500 re f EMC\n")
    font = pdf.add(
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000]"
        b" /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a " + str(proc).encode() + b" 0 R >>"
        b" /Encoding << /Type /Encoding /Differences [97 /a] >>"
        b" /FirstChar 97 /LastChar 97 /Widths [1000]"
        b" /Resources << /Properties << /L0 " + str(ocg).encode() + b" 0 R >> >> >>"
    )
    res = b"/Font << /Helv " + str(helv).encode() + b" 0 R /T3 " + str(font).encode() + b" 0 R >>"
    return simple_page(
        pdf, _secret_run("OC-TYPE3") + keep_line_ops(), res, catalog_extra=_layer_off(ocg)
    )


def evade_oc_on_an_appearance_stream() -> bytes:
    """The layer is the `/OC` of an annotation's APPEARANCE stream; the annotation has none."""
    pdf = Pdf()
    helv = helvetica(pdf)
    ocg = _ocg(pdf, "a layered appearance")
    appearance = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 20 20] /OC " + str(ocg).encode() + b" 0 R",
        b"0 0 20 20 re f\n",
    )
    annot = pdf.add(
        b"<< /Type /Annot /Subtype /Square /Rect [300 170 320 190] /F 4"
        b" /AP << /N " + str(appearance).encode() + b" 0 R >> >>"
    )
    return simple_page(
        pdf,
        _secret_run("OC-APPEARANCE") + keep_line_ops(),
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>",
        page_extra=b" /Annots [" + str(annot).encode() + b" 0 R]",
        catalog_extra=_layer_off(ocg),
    )


def evade_oc_untyped_membership_dictionary() -> bytes:
    """`/OC /OC1 BDC`, `/OC1` a membership dictionary written WITHOUT `/Type`.

    Found by a security review: the resource walk keys on `/Type /OCG` or `/OCMD`, and PDFium
    treats any dictionary under an `/OC` mark as a membership dictionary, so this layer hides its
    content while every type check passes. The mark is the signal (`optional-content-marked`).
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    ocg = _ocg(pdf, "a layer named by an untyped membership dictionary")
    content = (
        _secret_run("OC-UNTYPED")
        + b"/OC /OC1 BDC BT /Helv 10 Tf 40 175 Td " + literal("HIDDEN-UNTYPED")
        + b" Tj ET EMC\n" + keep_line_ops()
    )
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /Properties << /OC1 << /OCGs [" + str(ocg).encode() + b" 0 R] >> >>"
    )
    return simple_page(pdf, content, res, catalog_extra=_layer_off(ocg))


def evade_oc_untyped_group() -> bytes:
    """`/OC /L0 BDC`, `/L0` an optional-content GROUP written without `/Type`.

    Found by the #166 code review, measured by rendering: PDFium reads an untyped entry under an
    `/OC` mark as an OCG (`GetNameFor("Type", "OCG")` defaults it), so the OFF layer hides its
    content. The mark is the signal, as for the untyped membership dictionary.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    group = pdf.add(b"<< /Name " + literal("an untyped layer") + b" >>")
    content = (
        _secret_run("OC-UNTYPED-GROUP")
        + b"/OC /L0 BDC BT /Helv 10 Tf 40 175 Td " + literal("HIDDEN-UNTYPED-GROUP")
        + b" Tj ET EMC\n" + keep_line_ops()
    )
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /Properties << /L0 " + str(group).encode() + b" 0 R >>"
    )
    return simple_page(pdf, content, res, catalog_extra=_layer_off(group))


def evade_oc_on_an_appearance_state() -> bytes:
    """`/AP /N` is a dictionary of NAMED STATES, and one state's stream carries `/OC`.

    The checkbox shape. A security review's mutation dropping the named-state branch survived:
    every other appearance fixture wrote `/N` as a stream.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    ocg = _ocg(pdf, "a layered appearance state")
    on = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 20 20] /OC " + str(ocg).encode() + b" 0 R",
        b"0 0 20 20 re f\n",
    )
    off = pdf.stream(b"/Type /XObject /Subtype /Form /BBox [0 0 20 20]", b"")
    annot = pdf.add(
        b"<< /Type /Annot /Subtype /Square /Rect [300 170 320 190] /F 4 /AS /On"
        b" /AP << /N << /On " + str(on).encode() + b" 0 R /Off " + str(off).encode()
        + b" 0 R >> >> >>"
    )
    return simple_page(
        pdf,
        _secret_run("OC-APPEARANCE-STATE") + keep_line_ops(),
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>",
        page_extra=b" /Annots [" + str(annot).encode() + b" 0 R]",
        catalog_extra=_layer_off(ocg),
    )


def nearmiss_oc_on_another_page() -> bytes:
    """Page 2 has a hidden layer; page 1, the one redacted, references none. MUST be redacted.

    The twin for all three above: the rule is about the page being redacted, and a document with
    a layer somewhere else is not a page that has one.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    ocg = _ocg(pdf, "a layer on page two")
    pages = pdf.reserve()
    page1 = pdf.reserve()
    page2 = pdf.reserve()
    body1 = pdf.stream(b"", _secret_run("OC-OTHER-PAGE") + keep_line_ops())
    body2 = pdf.stream(
        b"",
        b"/OC /L0 BDC BT /Helv 10 Tf 40 175 Td " + literal("PAGE-TWO-LAYER") + b" Tj ET EMC\n"
        + keep_line_ops(),
    )
    font = b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
    box = b" /MediaBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
    pdf.put(
        page1,
        b"<< /Type /Page /Parent " + str(pages).encode() + b" 0 R" + box
        + b" /Resources << " + font + b" >> /Contents " + str(body1).encode() + b" 0 R >>",
    )
    pdf.put(
        page2,
        b"<< /Type /Page /Parent " + str(pages).encode() + b" 0 R" + box
        + b" /Resources << " + font + b" /Properties << /L0 " + str(ocg).encode() + b" 0 R >> >>"
        b" /Contents " + str(body2).encode() + b" 0 R >>",
    )
    pdf.put(
        pages,
        b"<< /Type /Pages /Count 2 /Kids ["
        + str(page1).encode() + b" 0 R " + str(page2).encode() + b" 0 R] >>",
    )
    root = pdf.add(
        b"<< /Type /Catalog /Pages " + str(pages).encode() + b" 0 R" + _layer_off(ocg) + b" >>"
    )
    return pdf.build(root)


# ===========================================================================================
# A STREAM WHERE A DICTIONARY BELONGS. PDFium's `GetDictFor` answers a stream with the stream's
# own dictionary; burrow read it as absent and inherited or skipped. Found by a security review of
# #166, three leaks, each returning `Ok`. Refused as `not-a-dictionary-where-one-belongs`.
# ===========================================================================================


def _page_with_resources(pdf: Pdf, content: bytes, page_resources: bytes, pages_resources: bytes) -> bytes:
    """A one-page document whose page and `/Pages` node carry the given `/Resources` values."""
    pages = pdf.reserve()
    page = pdf.reserve()
    stream = pdf.stream(b"", content)
    pdf.put(
        page,
        b"<< /Type /Page /Parent " + str(pages).encode() + b" 0 R"
        b" /MediaBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        + page_resources + b" /Contents " + str(stream).encode() + b" 0 R >>",
    )
    pdf.put(
        pages,
        b"<< /Type /Pages /Count 1 /Kids [" + str(page).encode() + b" 0 R]"
        + pages_resources + b" >>",
    )
    root = pdf.add(b"<< /Type /Catalog /Pages " + str(pages).encode() + b" 0 R >>")
    return pdf.build(root)


def evade_resources_stream_on_the_page() -> bytes:
    """The page's `/Resources` is a STREAM holding the carrying `/MC0`; `/Pages` holds a plain one."""
    pdf = Pdf()
    helv = helvetica(pdf)
    real = pdf.stream(
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /Properties << /MC0 << /ActualText " + literal(secret("RESOURCES-STREAM-PAGE"))
        + b" >> >>",
        b"",
    )
    content = b"/Span /MC0 BDC\n" + _secret_run("RESOURCES-STREAM-PAGE") + b"EMC\n" + keep_line_ops()
    return _page_with_resources(
        pdf,
        content,
        b" /Resources " + str(real).encode() + b" 0 R",
        b" /Resources << /Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /Properties << /MC0 << /MCID 0 >> >> >>",
    )


def evade_resources_stream_on_a_form() -> bytes:
    """A form's own `/Resources` is a STREAM holding the carrying `/MC0`; the page's is plain."""
    pdf = Pdf()
    helv = helvetica(pdf)
    real = pdf.stream(
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /Properties << /MC0 << /ActualText " + literal(secret("RESOURCES-STREAM-FORM"))
        + b" >> >>",
        b"",
    )
    form = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources " + str(real).encode() + b" 0 R",
        b"/Span /MC0 BDC\n" + _secret_run("RESOURCES-STREAM-FORM") + b"EMC\n",
    )
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /XObject << /X1 " + str(form).encode() + b" 0 R >>"
        b" /Properties << /MC0 << /MCID 0 >> >>"
    )
    return simple_page(pdf, b"/X1 Do\n" + keep_line_ops(), res)


def evade_font_decoy_behind_a_resources_stream() -> bytes:
    """The real Helvetica is in a stream-valued page `/Resources`; `/Pages` holds a decoy `/Helv`.

    The decoy's widths are enormous, so a walk resolving `/Helv` through `/Pages` places every
    glyph far outside the region and removes nothing, while PDFium draws the secret in place with
    the real font. Older than #166, and invisible to the read-back, which walks the same way.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    decoy = pdf.add(
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 32 /LastChar 126"
        b" /Widths [" + b" ".join([b"20000"] * 95) + b"] >>"
    )
    real = pdf.stream(b"/Font << /Helv " + str(helv).encode() + b" 0 R >>", b"")
    return _page_with_resources(
        pdf,
        _secret_run("FONT-DECOY") + keep_line_ops(),
        b" /Resources " + str(real).encode() + b" 0 R",
        b" /Resources << /Font << /Helv " + str(decoy).encode() + b" 0 R >> >>",
    )


def evade_xobject_category_as_a_stream() -> bytes:
    """`/XObject` is a STREAM whose dictionary names the form drawing the secret.

    Read as absent, `/X1 Do` resolves to nothing and draws no glyph the region can reach; PDFium
    draws the form.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    form = pdf.stream(
        b"/Type /XObject /Subtype /Form /BBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << /Font << /Helv " + str(helv).encode() + b" 0 R >> >>",
        _secret_run("XOBJECT-STREAM"),
    )
    category = pdf.stream(b"/X1 " + str(form).encode() + b" 0 R", b"")
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /XObject " + str(category).encode() + b" 0 R"
    )
    return simple_page(pdf, b"/X1 Do\n" + keep_line_ops(), res)


def evade_properties_as_a_stream_holding_a_layer() -> bytes:
    """`/Properties` is a STREAM whose dictionary names an OFF layer, and no span is marked `/OC`."""
    pdf = Pdf()
    helv = helvetica(pdf)
    ocg = _ocg(pdf, "a layer behind a stream-valued /Properties")
    category = pdf.stream(b"/L0 " + str(ocg).encode() + b" 0 R", b"")
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /Properties " + str(category).encode() + b" 0 R"
    )
    return simple_page(
        pdf, _secret_run("PROPERTIES-STREAM") + keep_line_ops(), res, catalog_extra=_layer_off(ocg)
    )


def evade_property_list_as_a_stream() -> bytes:
    """A `/Properties` ENTRY is a stream whose dictionary carries `/ActualText`.

    The category is a dictionary; the entry under it is not. Read as absent, the name resolves to
    nothing and refuses as unresolved, which is safe and names the wrong reason; PDFium reads the
    stream's dictionary as the property list. A mutation dropping the entry check survived until
    this fixture pinned the rule.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    entry = pdf.stream(b"/ActualText " + literal(secret("PROPERTY-ENTRY-STREAM")), b"")
    content = (
        b"/Span /MC0 BDC\n" + _secret_run("PROPERTY-ENTRY-STREAM") + b"EMC\n" + keep_line_ops()
    )
    res = (
        b"/Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /Properties << /MC0 " + str(entry).encode() + b" 0 R >>"
    )
    return simple_page(pdf, content, res)


def nearmiss_resources_inherited_from_pages() -> bytes:
    """The page declares no `/Resources` and `/Pages` holds an ordinary dictionary. MUST redact.

    Inheritance is how a word processor writes a shared letterhead. The refusal is about a stream
    in a dictionary's place, not about inheriting.
    """
    pdf = Pdf()
    helv = helvetica(pdf)
    return _page_with_resources(
        pdf,
        b"/P /MC0 BDC\n" + _secret_run("INHERITED-RESOURCES") + b"EMC\n" + keep_line_ops(),
        b"",
        b" /Resources << /Font << /Helv " + str(helv).encode() + b" 0 R >>"
        b" /Properties << /MC0 << /MCID 0 >> >> >>",
    )


CASES: list[tuple[str, str]] = [
    # (filename stem, refusal it probes)
    #
    # THE VERDICT COLUMN IS GONE. It held "refuse" or "handle" per fixture, was interpolated into
    # a string this script never prints, and was read only by `startswith` on the *stem* -- so it
    # was dead. It was also wrong: all seven `evade-actualtext-*` entries still said "refuse"
    # after they started redacting, while `tests/redaction/manifest.toml` said "handle". Two
    # sources of truth, one of them stale, neither checked against the other. The manifest owns
    # the verdict and `tools/check-redaction-corpus.py` validates it; this list owns only which
    # refusal each fixture probes, which is what the twin-coverage report below needs.
    ("evade-image-in-form", "image"),
    ("evade-inline-image", "image"),
    ("evade-image-as-pattern", "image"),
    ("evade-image-in-type3-glyph", "image"),
    ("evade-text-in-type3-via-form", "Type 3 procedure"),
    ("nearmiss-type3-procedure-that-only-shows-its-own-glyph", "Type 3 procedure"),
    ("evade-type3-font-named-only-inside-a-form", "Type 3 procedure"),
    ("nearmiss-form-carrying-its-own-font", "Type 3 procedure"),
    ("evade-tounicode-in-a-form-local-font", "font surgery"),
    ("nearmiss-tounicode-on-a-page-font", "font surgery"),
    ("nearmiss-image-outside-region", "image"),
    ("evade-paths-in-form", "vector paths"),
    ("evade-paths-in-type3-glyph", "vector paths"),
    ("nearmiss-paths-outside-region", "vector paths"),
    ("evade-widget-on-another-page", "/AcroForm"),
    ("evade-field-with-no-widget", "/AcroForm"),
    ("nearmiss-annotation-not-a-widget", "/AcroForm"),
    ("evade-struct-without-structparents", "/StructTreeRoot"),
    ("nearmiss-structparents-but-nothing-in-region", "/StructTreeRoot"),
    ("evade-oc-two-levels-down", "optional content"),
    ("nearmiss-nested-forms-no-oc", "optional content"),
    ("evade-actualtext-around-a-form", "/ActualText"),
    ("evade-actualtext-inside-a-form", "/ActualText"),
    ("evade-actualtext-around-a-nested-form", "/ActualText"),
    ("evade-actualtext-in-the-middle-form", "/ActualText"),
    ("evade-actualtext-over-a-form-without-resources", "/ActualText"),
    ("evade-actualtext-under-a-form-with-two-parents", "/ActualText"),
    ("evade-actualtext-on-a-page-that-draws-nothing-itself", "/ActualText"),
    ("nearmiss-actualtext-around-an-untouched-form", "/ActualText"),
    ("evade-actualtext-named-outside-key-position", "/ActualText"),
    ("nearmiss-ordinary-string-in-a-property-list", "/ActualText"),
    ("evade-actualtext-named-through-properties", "named /Properties"),
    ("evade-actualtext-named-in-a-form-scope", "named /Properties"),
    ("evade-actualtext-named-in-a-form-that-inherits", "named /Properties"),
    ("evade-actualtext-behind-a-reference-in-named-properties", "named /Properties"),
    ("nearmiss-named-properties-without-text", "named /Properties"),
    ("nearmiss-named-actualtext-outside-the-region", "named /Properties"),
    ("nearmiss-named-properties-decoy-on-the-page", "named /Properties"),
    ("evade-oc-outside-the-region", "optional content"),
    ("evade-oc-on-an-annotation", "optional content"),
    ("evade-oc-on-an-image", "optional content"),
    ("evade-oc-inside-an-unused-pattern", "optional content"),
    ("evade-oc-inside-an-unused-type3-font", "optional content"),
    ("evade-oc-on-an-appearance-stream", "optional content"),
    ("evade-oc-untyped-membership-dictionary", "optional content"),
    ("evade-oc-untyped-group", "optional content"),
    ("evade-oc-on-an-appearance-state", "optional content"),
    ("nearmiss-oc-on-another-page", "optional content"),
    ("evade-resources-stream-on-the-page", "not a dictionary"),
    ("evade-resources-stream-on-a-form", "not a dictionary"),
    ("evade-font-decoy-behind-a-resources-stream", "not a dictionary"),
    ("evade-xobject-category-as-a-stream", "not a dictionary"),
    ("evade-properties-as-a-stream-holding-a-layer", "not a dictionary"),
    ("evade-property-list-as-a-stream", "not a dictionary"),
    ("nearmiss-resources-inherited-from-pages", "not a dictionary"),
]

BUILDERS = {
    "evade-image-in-form": evade_image_in_form,
    "evade-inline-image": evade_inline_image,
    "evade-image-as-pattern": evade_image_as_pattern,
    "evade-image-in-type3-glyph": evade_image_in_type3_glyph,
    "evade-text-in-type3-via-form": evade_text_in_type3_via_form,
    "nearmiss-type3-procedure-that-only-shows-its-own-glyph": nearmiss_type3_procedure_that_only_shows_its_own_glyph,
    "evade-type3-font-named-only-inside-a-form": evade_type3_font_named_only_inside_a_form,
    "nearmiss-form-carrying-its-own-font": nearmiss_form_carrying_its_own_font,
    "evade-tounicode-in-a-form-local-font": evade_tounicode_in_a_form_local_font,
    "nearmiss-tounicode-on-a-page-font": nearmiss_tounicode_on_a_page_font,
    "nearmiss-image-outside-region": nearmiss_image_outside_region,
    "evade-paths-in-form": evade_paths_in_form,
    "evade-paths-in-type3-glyph": evade_paths_in_type3_glyph,
    "nearmiss-paths-outside-region": nearmiss_paths_outside_region,
    "evade-widget-on-another-page": evade_widget_on_another_page,
    "evade-field-with-no-widget": evade_field_with_no_widget,
    "nearmiss-annotation-not-a-widget": nearmiss_annotation_not_a_widget,
    "evade-struct-without-structparents": evade_struct_without_structparents,
    "nearmiss-structparents-but-nothing-in-region": nearmiss_structparents_but_nothing_in_region,
    "evade-oc-two-levels-down": evade_oc_two_levels_down,
    "nearmiss-nested-forms-no-oc": nearmiss_nested_forms_no_oc,
    "evade-actualtext-around-a-form": evade_actualtext_around_a_form,
    "evade-actualtext-inside-a-form": evade_actualtext_inside_a_form,
    "evade-actualtext-around-a-nested-form": evade_actualtext_around_a_nested_form,
    "evade-actualtext-in-the-middle-form": evade_actualtext_in_the_middle_form,
    "evade-actualtext-over-a-form-without-resources": evade_actualtext_over_a_form_without_resources,
    "evade-actualtext-under-a-form-with-two-parents": evade_actualtext_under_a_form_with_two_parents,
    "evade-actualtext-on-a-page-that-draws-nothing-itself": evade_actualtext_on_a_page_that_draws_nothing_itself,
    "nearmiss-actualtext-around-an-untouched-form": nearmiss_actualtext_around_an_untouched_form,
    "evade-actualtext-named-outside-key-position": evade_actualtext_named_outside_key_position,
    "nearmiss-ordinary-string-in-a-property-list": nearmiss_ordinary_string_in_a_property_list,
    "evade-actualtext-named-through-properties": evade_actualtext_named_through_properties,
    "evade-actualtext-named-in-a-form-scope": evade_actualtext_named_in_a_form_scope,
    "evade-actualtext-named-in-a-form-that-inherits": evade_actualtext_named_in_a_form_that_inherits,
    "evade-actualtext-behind-a-reference-in-named-properties": evade_actualtext_behind_a_reference_in_named_properties,
    "nearmiss-named-properties-without-text": nearmiss_named_properties_without_text,
    "nearmiss-named-actualtext-outside-the-region": nearmiss_named_actualtext_outside_the_region,
    "nearmiss-named-properties-decoy-on-the-page": nearmiss_named_properties_decoy_on_the_page,
    "evade-oc-outside-the-region": evade_oc_outside_the_region,
    "evade-oc-on-an-annotation": evade_oc_on_an_annotation,
    "evade-oc-on-an-image": evade_oc_on_an_image,
    "evade-oc-inside-an-unused-pattern": evade_oc_inside_an_unused_pattern,
    "evade-oc-inside-an-unused-type3-font": evade_oc_inside_an_unused_type3_font,
    "evade-oc-on-an-appearance-stream": evade_oc_on_an_appearance_stream,
    "evade-oc-untyped-membership-dictionary": evade_oc_untyped_membership_dictionary,
    "evade-oc-untyped-group": evade_oc_untyped_group,
    "evade-oc-on-an-appearance-state": evade_oc_on_an_appearance_state,
    "nearmiss-oc-on-another-page": nearmiss_oc_on_another_page,
    "evade-resources-stream-on-the-page": evade_resources_stream_on_the_page,
    "evade-resources-stream-on-a-form": evade_resources_stream_on_a_form,
    "evade-font-decoy-behind-a-resources-stream": evade_font_decoy_behind_a_resources_stream,
    "evade-xobject-category-as-a-stream": evade_xobject_category_as_a_stream,
    "evade-properties-as-a-stream-holding-a-layer": evade_properties_as_a_stream_holding_a_layer,
    "evade-property-list-as-a-stream": evade_property_list_as_a_stream,
    "nearmiss-resources-inherited-from-pages": nearmiss_resources_inherited_from_pages,
}


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        sys.stderr.write(f"usage: {argv[0]} <output-dir>\n")
        return 2
    out = Path(argv[1])
    out.mkdir(parents=True, exist_ok=True)

    qpdf = qpdf_cli()
    unreadable: list[str] = []
    written = 0
    by_refusal: dict[str, list[str]] = {}

    for stem, refusal in CASES:
        data = BUILDERS[stem]()
        path = out / f"{stem}.pdf"
        path.write_bytes(data)
        # EVERY FIXTURE IS CHECKED BEFORE IT IS REPORTED. One qpdf will not read is one that
        # would later be "refused" for the wrong reason, which reads exactly like success.
        result = subprocess.run([str(qpdf), "--check", str(path)], capture_output=True, text=True)
        if result.returncode not in (0, 3):
            unreadable.append(f"{path.name}: {result.stdout.strip()[:200]}")
        by_refusal.setdefault(refusal, []).append(stem)
        written += 1

    for refusal, names in by_refusal.items():
        evades = sum(1 for n in names if n.startswith("evade-"))
        misses = sum(1 for n in names if n.startswith("nearmiss-"))
        print(f"  {refusal:<18} {evades} evasion(s), {misses} near-miss twin(s)")
        if misses == 0:
            unreadable.append(
                f"{refusal}: has evasion fixtures and NO near-miss twin. A signal can pass "
                "every evasion by refusing everything; the twin is what stops that."
            )
    print(f"wrote {written} fixture(s) to {out}")

    if unreadable:
        sys.stderr.write("\nFAILED:\n")
        for line in unreadable:
            sys.stderr.write(f"  {line}\n")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
