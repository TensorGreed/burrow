#!/usr/bin/env python3
"""Generate the fidelity fixtures the split ADR's comparison is measured on.

    python3 tools/make-split-fidelity-fixtures.py <output-dir>

WHY THESE AND NOT `make-merge-fidelity-fixtures.py`'s. That generator's files are ONE PAGE
each, which is right for merge -- what it asks is "did the feature survive being combined
with another document". A split asks a different question: what happens to a feature when
some of the pages it refers to are *taken away*. Measured against a one-page fixture, a
one-page split is the identity, every feature trivially survives, and the comparison reports
that both routes are perfect. That happened, and the table looked convincing.

So these are MULTI-PAGE, and every feature spans pages deliberately:

  * `outline-per-page.pdf`   -- five pages, one outline entry per page, each pointing at its
                                own page. Splitting it asks what an outline does when three
                                of its five destinations are gone.
  * `fields-across-pages.pdf`-- five pages, an /AcroForm whose fields live on pages 2 and 4,
                                so one output has a field, one has a field whose siblings are
                                gone, and three have an /AcroForm referring to nothing.
  * `attachment-5page.pdf`   -- five pages and one embedded file hanging off the catalog. The
                                attachment belongs to the DOCUMENT, not to a page, so what a
                                five-way split should do with it is a decision rather than a
                                mechanism.
  * `boxes-differ.pdf`       -- five pages with a different /MediaBox width each, so page
                                ORDER and page IDENTITY are both observable in the output
                                without decompressing anything.
  * `canary-per-page.pdf`    -- five pages where EVERY feature carries a canary naming its own
                                page: the outline title, the form field's /T, the annotation's
                                /Contents and the attachment's filename. This is the fixture
                                ADR 0019 SS3's required leak test runs on, and the reason each
                                canary names its page is that "did something from page 4 come
                                along" is then a search rather than a structural walk.

Each carries `BURROWMARK` inside the feature under test, exactly as the merge fixtures do, so
"did it survive" is a search rather than a structural walk.

Not conformance fixtures: nothing asserts a typed outcome for them, and they are generated
rather than committed for the reason `make-merge-fidelity-fixtures.py` gives.
"""

import sys
from pathlib import Path

PAGES = 5


def write(out: Path, name: str, data: bytes) -> None:
    (out / name).write_bytes(data)
    print(f"  {name:<24} {len(data):>6} bytes")


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
    out += (
        f"trailer\n<< /Size {len(objects)+1} /Root {root} 0 R {extra_trailer}>>\n"
        f"startxref\n{xref}\n%%EOF\n"
    ).encode()
    return bytes(out)


def page(parent, contents, width=200, extra=""):
    return (
        f"<< /Type /Page /Parent {parent} 0 R /MediaBox [0 0 {width} 200] "
        f"/Contents {contents} 0 R /Resources << >> {extra}>>"
    ).encode()


def stream(data):
    return f"<< /Length {len(data)} >>\nstream\n".encode() + data + b"\nendstream"


def content(n):
    return stream(f"BT /F1 12 Tf 20 100 Td (page {n} of 5) Tj ET".encode())


def five_pages(first_page_obj, widths=None):
    """Objects 3..(3+5-1) are the pages; the next five are their content streams."""
    pages = []
    streams = []
    for i in range(PAGES):
        width = 200 if widths is None else widths[i]
        pages.append(page(2, first_page_obj + PAGES + i, width=width))
        streams.append(content(i + 1))
    return pages, streams


def main(out: Path) -> None:
    out.mkdir(parents=True, exist_ok=True)

    kids = " ".join(f"{3+i} 0 R" for i in range(PAGES))

    # ---------------------------------------------------- an outline entry per page
    #
    # Objects: 1 catalog, 2 pages tree, 3..7 pages, 8..12 contents, 13 outlines,
    # 14..18 outline items.
    pages, streams = five_pages(3)
    first_item = 14
    items = []
    for i in range(PAGES):
        prev = f"/Prev {first_item + i - 1} 0 R " if i > 0 else ""
        nxt = f"/Next {first_item + i + 1} 0 R " if i < PAGES - 1 else ""
        items.append(
            (
                f"<< /Title (BURROWMARK page {i+1}) /Parent 13 0 R {prev}{nxt}"
                f"/Dest [{3+i} 0 R /Fit] >>"
            ).encode()
        )
    objs = (
        [
            b"<< /Type /Catalog /Pages 2 0 R /Outlines 13 0 R /PageMode /UseOutlines >>",
            f"<< /Type /Pages /Kids [{kids}] /Count {PAGES} >>".encode(),
        ]
        + pages
        + streams
        + [
            f"<< /Type /Outlines /First {first_item} 0 R /Last {first_item+PAGES-1} 0 R "
            f"/Count {PAGES} >>".encode()
        ]
        + items
    )
    write(out, "outline-per-page.pdf", build(objs, 1))

    # ---------------------------------------------------- fields on pages 2 and 4
    pages, streams = five_pages(3)
    # Widget annotations on pages 2 (obj 4) and 4 (obj 6).
    field_objs = [13, 14]
    pages[1] = page(2, 3 + PAGES + 1, extra="/Annots [13 0 R] ")
    pages[3] = page(2, 3 + PAGES + 3, extra="/Annots [14 0 R] ")
    fields = [
        (
            f"<< /Type /Annot /Subtype /Widget /FT /Tx /T (BURROWMARK field {n}) "
            f"/Rect [10 10 90 30] /P {4 if n == 1 else 6} 0 R >>"
        ).encode()
        for n in (1, 2)
    ]
    objs = (
        [
            b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [13 0 R 14 0 R] >> >>",
            f"<< /Type /Pages /Kids [{kids}] /Count {PAGES} >>".encode(),
        ]
        + pages
        + streams
        + fields
    )
    write(out, "fields-across-pages.pdf", build(objs, 1))

    # ---------------------------------------------------- an attachment on the catalog
    pages, streams = five_pages(3)
    payload = b"BURROWMARK attached bytes"
    objs = (
        [
            b"<< /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles "
            b"<< /Names [(note.txt) 13 0 R] >> >> >>",
            f"<< /Type /Pages /Kids [{kids}] /Count {PAGES} >>".encode(),
        ]
        + pages
        + streams
        + [
            b"<< /Type /Filespec /F (note.txt) /EF << /F 14 0 R >> >>",
            stream(payload),
        ]
    )
    write(out, "attachment-5page.pdf", build(objs, 1))

    # ---------------------------------------------------- a different width per page
    #
    # No marker: the POINT of this one is that identity is readable without decompressing
    # anything, so "page 3 came out of a split as page 1" is a fact about /MediaBox rather
    # than about a string a writer might move.
    widths = [210, 220, 230, 240, 250]
    pages, streams = five_pages(3, widths=widths)
    objs = (
        [
            b"<< /Type /Catalog /Pages 2 0 R >>",
            f"<< /Type /Pages /Kids [{kids}] /Count {PAGES} >>".encode(),
        ]
        + pages
        + streams
    )
    write(out, "boxes-differ.pdf", build(objs, 1))

    canary_per_page(out)
    shared_objects(out)


def canary_per_page(out: Path) -> None:
    """Five pages, and every feature on each page names that page.

    One fixture rather than four, because the property under test is about the OUTPUT as a
    whole: a split that leaked an excluded page's field name but not its outline title would
    pass four separate single-feature fixtures three times over and fail once, and the reader
    would have to work out which. Here a single scan of the emitted bytes answers it.
    """
    kids = " ".join(f"{3+i} 0 R" for i in range(PAGES))
    pages = []
    streams = []
    for i in range(PAGES):
        # 3..7 pages, 8..12 contents, 13 outlines, 14..18 outline items,
        # 19..23 widgets, 24..28 annotations, 29 names, 30..34 filespecs, 35..39 file streams.
        pages.append(
            page(
                2,
                3 + PAGES + i,
                extra=f"/Annots [{19+i} 0 R {24+i} 0 R] ",
            )
        )
        streams.append(content(i + 1))

    outline_items = []
    for i in range(PAGES):
        prev = f"/Prev {14 + i - 1} 0 R " if i > 0 else ""
        nxt = f"/Next {14 + i + 1} 0 R " if i < PAGES - 1 else ""
        outline_items.append(
            (
                f"<< /Title (BURROWCANARY-outline-{i+1}) /Parent 13 0 R {prev}{nxt}"
                f"/Dest [{3+i} 0 R /Fit] >>"
            ).encode()
        )

    widgets = [
        (
            f"<< /Type /Annot /Subtype /Widget /FT /Tx /T (BURROWCANARY-field-{i+1}) "
            f"/Rect [10 10 90 30] /P {3+i} 0 R >>"
        ).encode()
        for i in range(PAGES)
    ]
    notes = [
        (
            f"<< /Type /Annot /Subtype /Text /Contents (BURROWCANARY-annot-{i+1}) "
            f"/Rect [100 10 180 30] /P {3+i} 0 R >>"
        ).encode()
        for i in range(PAGES)
    ]

    names = " ".join(f"(BURROWCANARY-attach-{i+1}) {30+i} 0 R" for i in range(PAGES))
    filespecs = [
        (
            f"<< /Type /Filespec /F (BURROWCANARY-attach-{i+1}) /EF << /F {35+i} 0 R >> >>"
        ).encode()
        for i in range(PAGES)
    ]
    file_streams = [stream(f"BURROWCANARY-bytes-{i+1}".encode()) for i in range(PAGES)]

    fields = " ".join(f"{19+i} 0 R" for i in range(PAGES))
    objs = (
        [
            b"<< /Type /Catalog /Pages 2 0 R /Outlines 13 0 R /PageMode /UseOutlines "
            + f"/AcroForm << /Fields [{fields}] >> ".encode()
            + b"/Names << /EmbeddedFiles << /Names ["
            + names.encode()
            + b"] >> >> >>",
            f"<< /Type /Pages /Kids [{kids}] /Count {PAGES} >>".encode(),
        ]
        + pages
        + streams
        + [
            f"<< /Type /Outlines /First 14 0 R /Last {14+PAGES-1} 0 R /Count {PAGES} >>".encode()
        ]
        + outline_items
        + widgets
        + notes
        + [b"<< >>"]  # object 29, unused filler so the numbering above stays readable
        + filespecs
        + file_streams
    )
    write(out, "canary-per-page.pdf", build(objs, 1))


def shared_objects(out: Path) -> None:
    """Five pages that SHARE indirect objects, which is what makes a leak possible at all.

    `canary_per_page.pdf` gives every page its own annotations, its own resources and its own
    everything. Under that structure the subsetting leak class **cannot occur**, so its twenty
    canaries confirmed a case that was never at risk -- measured, and recorded in ADR 0019 §2a
    and `add-operation` §2c.

    This fixture carries the four channels that are plantable, each named for the page it
    belongs to so a scan of an output can say which excluded page it came from:

      * an inherited ``/Resources`` on the ``/Pages`` node, holding a stream only page 4 draws;
      * a hierarchical ``/AcroForm``: one field, widgets on pages 1 and 4, and a ``/V`` holding
        what a person "typed" on page 4;
      * an ``/Annots`` array shared between pages 1 and 4;
      * an article thread whose bead on page 1 reaches a thread title naming page 4.

    Two encodings for the field value, because a byte-literal ASCII search cannot see the
    UTF-16BE strings real producers write.
    """
    # UTF-16BE with a BOM, hex-encoded, as a producer would write it.
    typed = "LEAKCANARY-utf16-typed-on-page-4"
    utf16 = "FEFF" + "".join(f"{ord(c):04X}" for c in typed)

    kids = " ".join(f"{3+i} 0 R" for i in range(PAGES))
    pages = []
    streams = []
    for i in range(PAGES):
        extra = ""
        if i in (0, 3):
            # ONE array, referenced by two pages. Page 1 is kept, page 4 is not.
            extra = "/Annots 20 0 R "
        if i == 0:
            extra += "/B [22 0 R] "
        # NO `/Resources` OF ITS OWN, deliberately: a page that carries an empty one overrides
        # the inherited dictionary and inherits nothing, which is how the first version of this
        # fixture failed to reproduce the channel it was written for.
        pages.append(
            (
                f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] "
                f"/Contents {3 + PAGES + i} 0 R {extra}>>"
            ).encode()
        )
        streams.append(content(i + 1))

    objs = (
        [
            b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [16 0 R] >> "
            b"/Threads [23 0 R] >>",
            # INHERITED RESOURCES. A kept page inherits this whole dictionary from its parent.
            f"<< /Type /Pages /Kids [{kids}] /Count {PAGES} "
            f"/Resources << /XObject << /Only4 13 0 R >> >> >>".encode(),
        ]
        + pages
        + streams
        + [
            # 13: a stream only page 4 would ever draw
            stream(b"LEAKCANARY-resource-only-page-4-draws-it"),
            b"<< >>",  # 14, filler so the numbering below stays readable
            b"<< >>",  # 15
            # 16: the field, with widgets on pages 1 and 4 and a typed value
            f"<< /FT /Tx /T (LEAKCANARY-fieldgroup-spans-1-and-4) "
            f"/V <{utf16}> /Kids [17 0 R 18 0 R] >>".encode(),
            # 17: the widget on page 1 (kept), 18: the widget on page 4 (excluded)
            b"<< /Type /Annot /Subtype /Widget /Parent 16 0 R /Rect [0 0 9 9] "
            b"/T (kept-widget-page-1) >>",
            b"<< /Type /Annot /Subtype /Widget /Parent 16 0 R /Rect [0 0 9 9] "
            b"/T (LEAKCANARY-widget-page-4) >>",
            b"<< >>",  # 19
            # 20: the SHARED /Annots array, on pages 1 and 4
            b"[17 0 R 21 0 R]",
            # 21: an annotation that belongs to page 4
            b"<< /Type /Annot /Subtype /Text /Contents (LEAKCANARY-annot-page-4) "
            b"/Rect [0 0 9 9] >>",
            # 22: an article bead on page 1, reaching a thread that names page 4
            b"<< /T 23 0 R /P 3 0 R >>",
            b"<< /Type /Thread /I << /Title (LEAKCANARY-thread-covers-page-4) >> "
            b"/F 22 0 R >>",
        ]
    )
    write(out, "shared-objects.pdf", build(objs, 1))


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(f"usage: {sys.argv[0]} <output-dir>")
    target = Path(sys.argv[1])
    main(target)
    print(f"split fidelity fixtures: 6 written to {target}")
