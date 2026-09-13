#!/usr/bin/env python3
"""Generate the fidelity fixtures ADR 0017's engine comparison is measured on.

    python3 tools/make-merge-fidelity-fixtures.py <output-dir>

Each file carries a literal `BURROWMARK` **inside** the feature under test -- an outline
title, an annotation's `/Contents`, a form field's `/T`, an embedded file's bytes. That is
what makes "did a merge preserve this" answerable by looking for the marker in the output
after `qpdf --qdf` has decompressed it, rather than by a structural walk that would be a
second parser to trust. A marker is a fact; a walk is an opinion.

Why these five features
-----------------------
They are the things a merge can drop **silently**. Page count and order are checked by the
harness directly and are hard to get wrong without anyone noticing; an outline, an
attachment or an `/AcroForm` simply stops being there, and the output still opens.

The `/AcroForm` case is the one that decided ADR 0017, and the fixture is built to expose
it: the widget annotation and the form dictionary are separate objects, so an engine can
keep one and drop the other. PDFium does exactly that, producing a document with a form
field that is no longer a field -- which is worse than losing it outright, because it looks
present.

These are NOT conformance fixtures and do not belong in `tests/conformance/fixtures/`:
nothing asserts a typed outcome for them. They exist so ADR 0017's table can be
re-measured, which is what stops it being a claim.

Hand-built, uncompressed, byte-exact xref -- the same style as
`core/burrow-engines/testsupport/minimal_pdf.rs`, so a fixture that turns out to be worth
asserting on can be ported there without being re-derived.
"""

import sys
from pathlib import Path

def write(out: Path, name: str, data: bytes) -> None:
    (out / name).write_bytes(data)
    print(f"  {name:<16} {len(data):>6} bytes")


def build(objects, root, extra_trailer=""):
    out = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
    offsets = [0] * (len(objects) + 1)
    for n, body in enumerate(objects, start=1):
        offsets[n] = len(out)
        out += f"{n} 0 obj\n".encode() + body + b"\nendobj\n"
    xref = len(out)
    out += f"xref\n0 {len(objects)+1}\n".encode()
    out += b"0000000000 65535 f \n"
    for n in range(1, len(objects) + 1):
        out += f"{offsets[n]:010d} 00000 n \n".encode()
    out += f"trailer\n<< /Size {len(objects)+1} /Root {root} 0 R {extra_trailer}>>\nstartxref\n{xref}\n%%EOF\n".encode()
    return bytes(out)

def page(parent, contents, extra=""):
    return (f"<< /Type /Page /Parent {parent} 0 R /MediaBox [0 0 200 200] "
            f"/Contents {contents} 0 R /Resources << >> {extra}>>").encode()

def stream(data):
    return f"<< /Length {len(data)} >>\nstream\n".encode() + data + b"\nendstream"

def main(out: Path) -> None:
    # ---------------------------------------------------------------- outline
    # 1 catalog, 2 pages, 3 page, 4 contents, 5 outlines, 6 outline item
    objs = [
        b"<< /Type /Catalog /Pages 2 0 R /Outlines 5 0 R /PageMode /UseOutlines >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        page(2, 4),
        stream(b"BT /F1 12 Tf 20 100 Td (outline) Tj ET"),
        b"<< /Type /Outlines /First 6 0 R /Last 6 0 R /Count 1 >>",
        b"<< /Title (BURROWMARK chapter one) /Parent 5 0 R /Dest [3 0 R /Fit] >>",
    ]
    write(out, "outline.pdf", build(objs, 1))

    # ------------------------------------------------------------- annotation
    objs = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        page(2, 4, "/Annots [5 0 R] "),
        stream(b"BT /F1 12 Tf 20 100 Td (annot) Tj ET"),
        b"<< /Type /Annot /Subtype /Text /Rect [10 10 30 30] /Contents (BURROWMARK note) /F 4 >>",
    ]
    write(out, "annotation.pdf", build(objs, 1))

    # ------------------------------------------------------------- form field
    objs = [
        b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [5 0 R] /DA (/Helv 0 Tf 0 g) >> >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        page(2, 4, "/Annots [5 0 R] "),
        stream(b"BT /F1 12 Tf 20 100 Td (form) Tj ET"),
        (b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (BURROWMARKfield) /V (typed) "
         b"/Rect [10 10 120 40] /P 3 0 R /F 4 /DA (/Helv 0 Tf 0 g) >>"),
    ]
    write(out, "formfield.pdf", build(objs, 1))

    # -------------------------------------------------------------- attachment
    attached = b"BURROWMARK attached payload\n"
    objs = [
        b"<< /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles << /Names [(readme.txt) 5 0 R] >> >> >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        page(2, 4),
        stream(b"BT /F1 12 Tf 20 100 Td (attach) Tj ET"),
        b"<< /Type /Filespec /F (readme.txt) /UF (readme.txt) /EF << /F 6 0 R >> >>",
        (f"<< /Type /EmbeddedFile /Subtype /text#2Fplain /Length {len(attached)} >>\nstream\n".encode()
         + attached + b"\nendstream"),
    ]
    write(out, "attachment.pdf", build(objs, 1))

    # ---------------------------------------------------------------- cropbox
    objs = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /CropBox [20 20 180 180] "
        b"/Rotate 90 /Contents 4 0 R /Resources << >> >>",
        stream(b"BT /F1 12 Tf 20 100 Td (crop) Tj ET"),
    ]
    write(out, "cropbox.pdf", build(objs, 1))

    # ------------------------------------------------------------------ plain
    for n, name in ((3, "plain3.pdf"), (2, "plain2.pdf")):
        kids = " ".join(f"{3+i} 0 R" for i in range(n))
        objs = [
            b"<< /Type /Catalog /Pages 2 0 R >>",
            f"<< /Type /Pages /Kids [{kids}] /Count {n} >>".encode(),
        ]
        for i in range(n):
            objs.append(page(2, 3 + n + i))
        for i in range(n):
            objs.append(stream(f"BT /F1 12 Tf 20 100 Td (page {i+1} of {name}) Tj ET".encode()))
        write(out, name, build(objs, 1))

if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(f"usage: {sys.argv[0]} <output-dir>")
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    main(out)
    print(f"merge fidelity fixtures: 7 written to {out}")
