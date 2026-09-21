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

CASES: list[tuple[str, str, str]] = [
    # (filename stem, refusal it probes, expected verdict)
    ("evade-image-in-form", "image", "refuse"),
    ("evade-inline-image", "image", "refuse"),
    ("evade-image-as-pattern", "image", "refuse"),
    ("evade-image-in-type3-glyph", "image", "refuse"),
    ("nearmiss-image-outside-region", "image", "handle"),
    ("evade-paths-in-form", "vector paths", "refuse"),
    ("evade-paths-in-type3-glyph", "vector paths", "refuse"),
    ("nearmiss-paths-outside-region", "vector paths", "handle"),
    ("evade-widget-on-another-page", "/AcroForm", "refuse"),
    ("evade-field-with-no-widget", "/AcroForm", "refuse"),
    ("nearmiss-annotation-not-a-widget", "/AcroForm", "handle"),
    ("evade-struct-without-structparents", "/StructTreeRoot", "refuse"),
    ("nearmiss-structparents-but-nothing-in-region", "/StructTreeRoot", "handle"),
    ("evade-oc-two-levels-down", "optional content", "refuse"),
    ("nearmiss-nested-forms-no-oc", "optional content", "handle"),
]

BUILDERS = {
    "evade-image-in-form": evade_image_in_form,
    "evade-inline-image": evade_inline_image,
    "evade-image-as-pattern": evade_image_as_pattern,
    "evade-image-in-type3-glyph": evade_image_in_type3_glyph,
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

    for stem, refusal, verdict in CASES:
        data = BUILDERS[stem]()
        path = out / f"{stem}.pdf"
        path.write_bytes(data)
        # EVERY FIXTURE IS CHECKED BEFORE IT IS REPORTED. One qpdf will not read is one that
        # would later be "refused" for the wrong reason, which reads exactly like success.
        result = subprocess.run([str(qpdf), "--check", str(path)], capture_output=True, text=True)
        if result.returncode not in (0, 3):
            unreadable.append(f"{path.name}: {result.stdout.strip()[:200]}")
        by_refusal.setdefault(refusal, []).append(f"{stem} -> {verdict}")
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
