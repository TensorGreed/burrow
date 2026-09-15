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


def stream(data, extra=""):
    """A stream object, optionally with keys of its own on the dictionary.

    `extra` is what lets a fixture build a Form XObject or an image rather than a bare content
    stream -- the walk's behaviour depends entirely on `/Subtype`, so a fixture that cannot set
    it cannot exercise the distinction.
    """
    return f"<< /Length {len(data)} {extra}>>\nstream\n".encode() + data + b"\nendstream"


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
    optional_content(out)
    refusal_shapes(out)
    walk_shapes(out)
    destinations(out)


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

    This fixture carries the five channels that a byte scan can see, each named for the page it
    belongs to so a scan of an output can say which excluded page it came from:

      * an inherited ``/Resources`` on the ``/Pages`` node, holding a stream page 4 draws and
        nobody else does -- and page 4's content stream really does draw it, which it did not
        until #54 closed;
      * a hierarchical ``/AcroForm``: one field, widgets on pages 1 and 4, and a ``/V`` holding
        what a person "typed" on page 4;
      * an ``/Annots`` array shared between pages 1 and 4;
      * an article thread whose bead on page 1 reaches a thread title naming page 4;
      * a **named destination** in a kept page's own link annotation -- ADR 0019 §2a row 5, which
        had no fixture and no test until #54 closed. It is the odd one out: the leak is not an
        object belonging to another page, it is a NAME sitting inside an object that legitimately
        survives, and names of destinations are routinely descriptive ("Appendix-C-Salaries").
        The structural closure harness cannot see it by construction, which is why the named
        canaries exist alongside it.

    Row 6, ``/OCProperties``, is not here and cannot be: it is not a string arriving where it
    should not, it is content becoming *visible*, and a byte scan is the wrong instrument. It has
    its own fixture, ``optional-content.pdf``, and its own test -- a refusal rather than a scan.

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
        # PAGE 1 CARRIES A `/Properties` ENTRY THAT IS NOT OPTIONAL CONTENT, and it is the
        # near-miss for the layer refusal rather than decoration. Every tagged PDF has marked
        # content with a property list; a refusal keyed on `/Properties` being present rather
        # than on `/Type /OCG` would reject all of them. Code review MEASURED the gap: with
        # `is_optional_content_group` mutated to return true for any dictionary, the whole suite
        # stayed green, because no fixture had a `/Properties` at all.
        if i == 0:
            extra += "/Resources << /Properties << /MC0 25 0 R >> >> "
        pages.append(
            (
                f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] "
                f"/Contents {3 + PAGES + i} 0 R {extra}>>"
            ).encode()
        )
        # PAGE 4 ACTUALLY DRAWS THE INHERITED RESOURCE. Until this line existed the fixture
        # claimed "a stream only page 4 draws" -- in its docstring AND in the canary's own name
        # -- while no page's content mentioned `/Only4`, so the headline channel of ADR 0019
        # §2a was inert. `add-operation` §2c; the sibling generator had the identical defect.
        if i == 3:
            streams.append(stream(f"BURROWMARK page 4 /Only4 Do".encode()))
        elif i == 0:
            streams.append(
                stream(b"/OC /MC0 BDC BT (BURROWMARK page 1) Tj ET EMC")
            )
        else:
            streams.append(content(i + 1))

    objs = (
        [
            b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [16 0 R] >> "
            b"/Threads [23 0 R] >>",
            # INHERITED RESOURCES. A kept page inherits this whole dictionary from its parent.
            # INHERITED RESOURCES, holding a stream page 4 draws AND a key that is not one of
            # the seven resource categories. `/Stash` is the second one: the filter iterated the
            # categories and filtered inside each, so anything else survived whole. Security
            # review measured `/Stash << /Secret … >>` coming through untouched while `/Font` was
            # pruned correctly -- ADR 0019 §2a row 1 leaking through a key nobody enumerated,
            # which is the shape the page-key rule was made an allowlist to avoid.
            f"<< /Type /Pages /Kids [{kids}] /Count {PAGES} "
            f"/Resources << /XObject << /Only4 13 0 R >> "
            f"/Stash << /Secret 26 0 R >> >> >>".encode(),
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
            # 20: the SHARED /Annots array, on pages 1 and 4 -- and object 24, the link whose
            # named destination is ADR 0019 §2a row 5.
            b"[17 0 R 21 0 R 24 0 R]",
            # 21: an annotation that belongs to page 4
            b"<< /Type /Annot /Subtype /Text /Contents (LEAKCANARY-annot-page-4) "
            b"/Rect [0 0 9 9] >>",
            # 22: an article bead on page 1, reaching a thread that names page 4
            b"<< /T 23 0 R /P 3 0 R >>",
            b"<< /Type /Thread /I << /Title (LEAKCANARY-thread-covers-page-4) >> "
            b"/F 22 0 R >>",
            # 24: A LINK ON PAGE 1 whose action names a destination elsewhere in the document.
            # `/P 3 0 R` is page 1, so the annotation itself belongs here and a filter keyed on
            # ownership keeps it -- which is the point. The leak rides INSIDE an object that is
            # allowed to survive, and only dropping the action closes it.
            b"<< /Type /Annot /Subtype /Link /P 3 0 R /Rect [0 0 9 9] "
            b"/A << /S /GoTo /D (LEAKCANARY-namedest-page-4) >> >>",
            # 25: A MARKED-CONTENT PROPERTY LIST THAT IS NOT AN OCG. `/Type` is absent, which is
            # what an ordinary tagged-PDF property list looks like. This document must still
            # split; `a_document_without_layers_is_not_caught_by_the_layer_refusal` is the test,
            # and without this object that test proved only that a document with no
            # `/Properties` is not refused.
            b"<< /Metadata (an ordinary marked-content property list) >>",
            # 26: what `/Stash` points at. Reachable only through a resource-dictionary key that
            # is not a category, which is the whole point of it.
            b"<< /Note (LEAKCANARY-stash-belongs-to-page-4) >>",
        ]
    )
    write(out, "shared-objects.pdf", build(objs, 1))


def destinations(out: Path) -> None:
    """Four links on page 1, one of each kind the destination rule decides between.

    `split` used to drop every `/A` and `/Dest`, on the reasoning that a named destination is a
    name and an explicit one points at a page the copier stopped at. The second half is wrong,
    measured: `qpdf_add_page`'s copier maps a reference to a page it copied and reserves a **null**
    for one it did not, so a link INTO the output survives and works while an outward one is
    already inert. Dropping the first was a fidelity loss with no privacy gain.

    So the fixture has to make all four cases distinguishable in the emitted bytes:

      * a `/Dest` array naming page 2, which is in the same part as page 1 -- must SURVIVE;
      * a `/Dest` array naming page 4, which is not -- must go;
      * a NAMED destination, whose name is the leak (ADR 0019 §2a row 5) -- must go;
      * a `/URI` action, which is the kept page's own data -- must SURVIVE.

    Without the last two the rule could be "keep everything" and pass; without the first two it
    could be "drop everything", which is what it used to be.
    """
    kids = " ".join(f"{3+i} 0 R" for i in range(PAGES))
    pages, streams = [], []
    for i in range(PAGES):
        extra = "/Annots [13 0 R 14 0 R 15 0 R 16 0 R] " if i == 0 else ""
        pages.append(
            (
                f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] "
                f"{extra}/Contents {3 + PAGES + i} 0 R >>"
            ).encode()
        )
        streams.append(content(i + 1))
    objs = (
        [
            b"<< /Type /Catalog /Pages 2 0 R >>",
            f"<< /Type /Pages /Kids [{kids}] /Count {PAGES} >>".encode(),
        ]
        + pages
        + streams
        + [
            # 13: INTO the output (page 2 is object 4, kept alongside page 1).
            b"<< /Type /Annot /Subtype /Link /P 3 0 R /Rect [0 0 9 9] "
            b"/Dest [4 0 R /Fit] /Contents (BURROWMARK dest-into-part) >>",
            # 14: OUT of the output (page 4 is object 6).
            b"<< /Type /Annot /Subtype /Link /P 3 0 R /Rect [0 9 9 18] "
            b"/Dest [6 0 R /Fit] /Contents (BURROWMARK dest-out-of-part) >>",
            # 15: a NAMED destination -- the name is the leak.
            b"<< /Type /Annot /Subtype /Link /P 3 0 R /Rect [0 18 9 27] "
            b"/A << /S /GoTo /D (LEAKCANARY-namedest-page-4) >> >>",
            # 16: a web address, which describes nothing that was excluded.
            b"<< /Type /Annot /Subtype /Link /P 3 0 R /Rect [0 27 9 36] "
            b"/A << /S /URI /URI (https://example.invalid/BURROWMARK-uri) >> >>",
        ]
    )
    write(out, "destinations.pdf", build(objs, 1))


def walk_shapes(out: Path) -> None:
    """Three documents the resource walk has to handle, each a defect security review measured.

      * ``images.pdf`` — a page drawing a ``/DCTDecode`` image and a ``/FlateDecode`` one. The
        walk followed anything in ``/XObject`` that was a stream, so it tried to decode images:
        a JPEG cannot be decoded at ``qpdf_dl_specialized`` and **every JPEG-bearing document was
        refused**, while a flate image decoded and then failed to lex as PDF syntax roughly
        whenever its pixels contained an unbalanced ``(``. An image names no resources; following
        one was never useful.
      * ``nested-forms.pdf`` — a form drawing a form drawing text in a font named only two levels
        down, where the inner form has no ``/Resources`` of its own. The walk resolved collected
        names against the PAGE's categories only, so it never reached the inner form and pruned
        the font off the page while the form still asked for it. The font object left the file
        entirely.
      * ``oc-nested.pdf`` — an optional content group one level down, inside a form XObject's own
        ``/Resources``. The refusal looked at the page's dictionary only, so a document whose
        hidden layer lived in a form split happily, with the hidden text visible in the output
        and the layer's name still in it. This is the shape Illustrator and InDesign emit.
    """
    kids = " ".join(f"{3+i} 0 R" for i in range(PAGES))

    def document(
        page1_resources: str,
        page1_stream: bytes,
        extra: list[bytes],
        catalog_extra: str = "",
    ) -> bytes:
        pages, streams = [], []
        for i in range(PAGES):
            resources = page1_resources if i == 0 else ""
            pages.append(
                (
                    f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] "
                    f"{resources}/Contents {3 + PAGES + i} 0 R >>"
                ).encode()
            )
            streams.append(page1_stream if i == 0 else content(i + 1))
        return build(
            [
                f"<< /Type /Catalog /Pages 2 0 R {catalog_extra}>>".encode(),
                f"<< /Type /Pages /Kids [{kids}] /Count {PAGES} >>".encode(),
            ]
            + pages
            + streams
            + extra,
            1,
        )

    # A JPEG and a flate image whose "pixels" contain an unbalanced `(`, which is what made the
    # flate case fail rather than merely waste time.
    write(
        out,
        "images.pdf",
        document(
            "/Resources << /XObject << /Jpeg 13 0 R /Flat 14 0 R >> >> ",
            stream(b"BURROWMARK page 1 q /Jpeg Do /Flat Do Q"),
            [
                b"<< /Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray "
                b"/BitsPerComponent 8 /Filter /DCTDecode /Length 4 >>\nstream\n"
                b"\xff\xd8\xff\xd9\nendstream",
                stream(b"(((( unbalanced pixels", extra="/Type /XObject /Subtype /Image "
                       "/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 "),
            ],
        ),
    )

    # form -> form -> /F1, with the inner form carrying no `/Resources`.
    write(
        out,
        "nested-forms.pdf",
        document(
            "/Resources << /XObject << /Fx1 13 0 R >> /Font << /F1 15 0 R >> >> ",
            stream(b"BURROWMARK page 1 q /Fx1 Do Q"),
            [
                stream(
                    b"q /Fx2 Do Q",
                    extra="/Type /XObject /Subtype /Form /BBox [0 0 9 9] "
                    "/Resources << /XObject << /Fx2 14 0 R >> >> ",
                ),
                stream(
                    b"BT /F1 12 Tf (drawn two levels down) Tj ET",
                    extra="/Type /XObject /Subtype /Form /BBox [0 0 9 9] ",
                ),
                b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
            ],
        ),
    )

    # An OCG inside a form's own `/Resources`, turned off by the catalog's configuration.
    #
    # THE CATALOG IS BUILT WITH `/OCProperties`, not patched afterwards. The first version wrote
    # the document and then `bytes.replace`d the catalog -- which changed an object's length
    # after `build` had computed the xref offsets, so every offset past it was wrong and qpdf
    # refused the file as damaged. The test caught it, which is the only reason this is a comment
    # rather than a fixture that quietly exercised nothing.
    write(
        out,
        "oc-nested.pdf",
        document(
        "/Resources << /XObject << /Fx1 13 0 R >> >> ",
        stream(b"BURROWMARK page 1 q /Fx1 Do Q"),
        [
            stream(
                b"/OC /MC0 BDC BT (BURROWMARK hidden one level down) Tj ET EMC",
                extra="/Type /XObject /Subtype /Form /BBox [0 0 9 9] "
                "/Resources << /Properties << /MC0 14 0 R >> >> ",
            ),
            b"<< /Type /OCG /Name (BURROWMARK nested layer) >>",
        ],
            catalog_extra="/OCProperties << /OCGs [14 0 R] /D << /OFF [14 0 R] >> >> ",
        ),
    )


def refusal_shapes(out: Path) -> None:
    """Two documents that open and split on any other tool, and that burrow refuses.

    Both are behaviour changes that arrived with the pruning in #54, and neither is obvious from
    the operation's signature — so they are fixtures with tests rather than facts somebody finds
    out from a bug report.

      * ``unlexable-content.pdf`` — a page whose content stream contains a stray ``)``. The
        resource filter has to know which names the page uses, a partial answer deletes a
        resource the page draws with (see ``pdfsyntax::names``), and there is no third option, so
        a stream that cannot be tokenised refuses the whole split.
      * ``undecodable-stream.pdf`` — a Form XObject declaring ``/JPXDecode``. qpdf will not
        decode a lossy filter at ``qpdf_dl_specialized``, so the bytes come back compressed, and
        lexing those for names yields accidents rather than the names that are there.

    Both are the safe direction and both cost something real: a document burrow could have split
    is refused. That trade is the subject of ADR 0019 §2b's "prune correctly, or drop entirely" —
    a refusal is neither, and it is stronger than both.
    """
    kids = " ".join(f"{3+i} 0 R" for i in range(PAGES))
    for name, first_stream, extra_objs, page1_extra in (
        (
            "unlexable-content.pdf",
            # A stray `)` with no string to close. Real producers do emit malformed content.
            stream(b"BT (ok) Tj ET ) q Q"),
            [],
            "",
        ),
        (
            "undecodable-stream.pdf",
            stream(b"BURROWMARK page 1 /Lossy Do"),
            [
                b"<< /Type /XObject /Subtype /Form /BBox [0 0 9 9] /Filter /JPXDecode "
                b"/Length 4 >>\nstream\n\x00\x00\x00\x00\nendstream",
            ],
            "/Resources << /XObject << /Lossy 13 0 R >> >> ",
        ),
    ):
        pages = []
        streams = []
        for i in range(PAGES):
            resources = page1_extra if i == 0 else ""
            pages.append(
                (
                    f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] "
                    f"{resources}/Contents {3 + PAGES + i} 0 R >>"
                ).encode()
            )
            streams.append(first_stream if i == 0 else content(i + 1))
        objs = (
            [
                b"<< /Type /Catalog /Pages 2 0 R >>",
                f"<< /Type /Pages /Kids [{kids}] /Count {PAGES} >>".encode(),
            ]
            + pages
            + streams
            + extra_objs
        )
        write(out, name, build(objs, 1))


def optional_content(out: Path) -> None:
    """A document with a layer that is turned OFF, which `split` refuses rather than splits.

    ADR 0019 §2a row 6, and the one channel a canary scan is the wrong instrument for. Dropping
    the catalog's ``/OCProperties`` while keeping the OCGs a page references does not put a
    string somewhere it should not be -- it makes content the source **hid** visible, which is a
    disclosure a byte scan cannot detect and a structural closure check cannot either.

    §2b forbids dropping the configuration alone for exactly that reason, and carrying a pruned
    one needs the destination's catalog, which ADR 0013's caller rule puts out of reach
    (``qpdf_get_root`` is not on the trapped list). So the answer is a refusal, and this is the
    fixture that proves the refusal fires: page 1 draws content inside a layer that ``/D /OFF``
    turns off, and page 1 is in every part of any split.
    """
    kids = " ".join(f"{3+i} 0 R" for i in range(PAGES))
    pages = []
    streams = []
    for i in range(PAGES):
        resources = "/Resources << /Properties << /MC0 13 0 R >> >> " if i == 0 else ""
        pages.append(
            (
                f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] "
                f"{resources}/Contents {3 + PAGES + i} 0 R >>"
            ).encode()
        )
        if i == 0:
            # Marked content inside the layer. Without the configuration that turns `/MC0` off,
            # a viewer draws this.
            streams.append(
                stream(b"/OC /MC0 BDC BT (BURROWMARK hidden by a layer) Tj ET EMC")
            )
        else:
            streams.append(content(i + 1))

    objs = (
        [
            b"<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [13 0 R] "
            b"/D << /OFF [13 0 R] >> >> >>",
            f"<< /Type /Pages /Kids [{kids}] /Count {PAGES} >>".encode(),
        ]
        + pages
        + streams
        + [
            # 13: the optional content group, named as layers usually are.
            b"<< /Type /OCG /Name (BURROWMARK draft watermark) >>",
        ]
    )
    write(out, "optional-content.pdf", build(objs, 1))


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(f"usage: {sys.argv[0]} <output-dir>")
    target = Path(sys.argv[1])
    main(target)
    print(f"split fidelity fixtures: 13 written to {target}")
