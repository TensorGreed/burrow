#!/usr/bin/env python3
"""Generate a document in which every object is marked, plus a manifest of who owns what.

    python3 tools/make-marked-document.py <output-dir>

Writes `marked.pdf` and `marked.json`.

# Why this exists

`tools/make-split-fidelity-fixtures.py` plants canaries by FEATURE -- an outline title, a field
name, an attachment. A canary list can only ever confirm the enumeration it was given, and the
first such list for `split` enumerated four features while the thing that decided whether data
crossed was object *sharing*. Twenty canaries, twenty measurements of a case that was never at
risk (ADR 0019 SS2a).

This is the structural version. **Every object carries a unique marker, and the manifest
declares which pages each object belongs to.** A test can then assert a property rather than a
list: every object surviving in an output belongs to a page that output contains. That fails
for categories nobody named, which is the whole point -- the failure mode of a canary list is
that it passes.

# Ownership is DECLARED, not derived

The manifest says which pages an object belongs to, because that is a fact about the fixture's
intent and not something reachability can answer. Pure graph reachability gets it wrong in both
directions:

  * an inherited `/Resources` dictionary is reachable from every page through `/Parent`, and
    belongs to whichever page actually draws it;
  * an `/Annots` array shared by pages 1 and 4 makes page 4's annotation reachable from page 1,
    and it still belongs to page 4.

Declaring it keeps the test honest about what it is asserting. `pages: []` means document-level
scaffolding that belongs to no page -- the catalog, the page tree -- which any output may
legitimately contain.

# What this cannot see

A leak INSIDE a shared object. The `/V` of a form field owned by pages 1 and 4 carries what
somebody typed on page 4, and the object itself is legitimately present. Sub-object leaks need
the named-channel regression cases that sit on top of this (ADR 0019 SS3).
"""

import json
import sys
from pathlib import Path

PAGES = 5


def stream(data: bytes) -> bytes:
    return f"<< /Length {len(data)} >>\nstream\n".encode() + data + b"\nendstream"


def build(objects: list[bytes], root: int, extra: str = "") -> bytes:
    out = bytearray(b"%PDF-1.7\n")
    offsets = []
    for n, body in enumerate(objects, start=1):
        offsets.append(len(out))
        out += f"{n} 0 obj\n".encode() + body + b"\nendobj\n"
    xref = len(out)
    out += f"xref\n0 {len(objects)+1}\n".encode() + b"0000000000 65535 f \n"
    for offset in offsets:
        out += f"{offset:010d} 00000 n \n".encode()
    out += (
        f"trailer\n<< /Size {len(objects)+1} /Root {root} 0 R {extra}>>\n"
        f"startxref\n{xref}\n%%EOF\n"
    ).encode()
    return bytes(out)


def main(out: Path) -> None:
    out.mkdir(parents=True, exist_ok=True)

    # `owners[n]` is the set of one-based page numbers object n belongs to. An empty list is
    # document scaffolding. The numbering below is written out rather than computed so the
    # manifest and the objects cannot drift apart silently.
    owners: dict[int, list[int]] = {}
    objects: list[bytes] = []

    kinds: dict[int, str] = {}

    def add(body: bytes, pages: list[int], kind: str = "content") -> int:
        """Record an object, who it belongs to, and what KIND of thing it is.

        `kind` is what lets one harness serve operations with different obligations.
        `content` is what a page is made of -- an operation that keeps the page must keep it.
        `navigation` is document-level furniture that points AT pages: an outline entry. A
        subsetting operation may legitimately drop it (ADR 0019 SS1 does, and the page says
        so), while `rotate`, `reorder` and `compress` may not lose it.

        Without this the inverse assertion had to require every owned object, and a correct
        one-way split failed it by dropping five outline entries on purpose.
        """
        objects.append(body)
        owners[len(objects)] = pages
        kinds[len(objects)] = kind
        return len(objects)

    # Every object carries `/BM (BM-n)` or the marker inside its stream, so "did object n
    # survive" is a search. A marker is a fact; a structural walk would be a second parser.
    def mark(n: int) -> str:
        return f"BM-{n:03d}"

    # 1 catalog, 2 page tree -- scaffolding, owned by nobody.
    catalog = add(b"", [])
    tree = add(b"", [])

    page_objs = [add(b"", [i + 1]) for i in range(PAGES)]
    contents = [add(b"", [i + 1]) for i in range(PAGES)]

    # A resource only page 4 draws, hung off the page TREE so every page inherits it.
    #
    # ITS BODY IS FILLED AFTER ITS NUMBER IS KNOWN, like every other object here. The first
    # version wrote `mark(0)` inline, so the stream carried `BM-000` while the manifest
    # declared `BM-013` — and the headline channel of ADR 0019 §2a could never be detected by
    # the harness built to detect it. Nothing failed: the scan looked for a string nobody had
    # written, found nothing, and reported no leak. Exactly the failure the structural harness
    # was supposed to replace, reproduced inside it. Found by security review; the
    # every-marker-is-present assertion below is what stops it happening a third time.
    only4 = add(b"", [4])
    objects[only4 - 1] = stream(f"{mark(only4)} only page 4 draws this".encode())

    # A form field spanning pages 1 and 4, with widgets on each.
    field = add(b"", [1, 4])
    widget1 = add(b"", [1])
    widget4 = add(b"", [4])

    # One /Annots array referenced by pages 1 and 4, holding each page's own annotation.
    shared_annots = add(b"", [1, 4])
    annot4 = add(b"", [4])

    # An article bead on page 1 reaching a thread whose info names page 4.
    bead = add(b"", [1])
    thread = add(b"", [1, 4])
    thread_info = add(b"", [4])

    # An outline entry per page, and the outline root.
    outline_root = add(b"", [], kind="navigation")
    outline_items = [add(b"", [i + 1], kind="navigation") for i in range(PAGES)]

    # Fill the bodies now that every number is known.
    objects[catalog - 1] = (
        f"<< /Type /Catalog /BM ({mark(catalog)}) /Pages {tree} 0 R "
        f"/Outlines {outline_root} 0 R /AcroForm << /Fields [{field} 0 R] >> "
        f"/Threads [{thread} 0 R] >>"
    ).encode()
    kids = " ".join(f"{p} 0 R" for p in page_objs)
    objects[tree - 1] = (
        f"<< /Type /Pages /BM ({mark(tree)}) /Kids [{kids}] /Count {PAGES} "
        f"/Resources << /XObject << /Only4 {only4} 0 R >> >> >>"
    ).encode()
    for i, p in enumerate(page_objs):
        extra = ""
        if i in (0, 3):
            extra += f"/Annots {shared_annots} 0 R "
        if i == 0:
            extra += f"/B [{bead} 0 R] "
        # No `/Resources` of its own, so the inherited one is what it gets.
        objects[p - 1] = (
            f"<< /Type /Page /BM ({mark(p)}) /Parent {tree} 0 R /MediaBox [0 0 200 200] "
            f"/Contents {contents[i]} 0 R {extra}>>"
        ).encode()
        objects[contents[i] - 1] = stream(
            f"{mark(contents[i])} page {i+1}".encode()
        )
    objects[field - 1] = (
        f"<< /FT /Tx /BM ({mark(field)}) /T (field) /V ({mark(thread_info)}-typed-on-page-4) "
        f"/Kids [{widget1} 0 R {widget4} 0 R] >>"
    ).encode()
    objects[widget1 - 1] = (
        f"<< /Type /Annot /Subtype /Widget /BM ({mark(widget1)}) /Parent {field} 0 R "
        f"/Rect [0 0 9 9] >>"
    ).encode()
    objects[widget4 - 1] = (
        f"<< /Type /Annot /Subtype /Widget /BM ({mark(widget4)}) /Parent {field} 0 R "
        f"/Rect [0 0 9 9] >>"
    ).encode()
    objects[shared_annots - 1] = f"[{widget1} 0 R {annot4} 0 R]".encode()
    objects[annot4 - 1] = (
        f"<< /Type /Annot /Subtype /Text /BM ({mark(annot4)}) /Contents (note) "
        f"/Rect [0 0 9 9] >>"
    ).encode()
    objects[bead - 1] = (
        f"<< /BM ({mark(bead)}) /T {thread} 0 R /P {page_objs[0]} 0 R >>"
    ).encode()
    objects[thread - 1] = (
        f"<< /Type /Thread /BM ({mark(thread)}) /I {thread_info} 0 R /F {bead} 0 R >>"
    ).encode()
    objects[thread_info - 1] = f"<< /BM ({mark(thread_info)}) /Title (thread) >>".encode()
    first, last = outline_items[0], outline_items[-1]
    objects[outline_root - 1] = (
        f"<< /Type /Outlines /BM ({mark(outline_root)}) /First {first} 0 R "
        f"/Last {last} 0 R /Count {PAGES} >>"
    ).encode()
    for i, item in enumerate(outline_items):
        prev = f"/Prev {outline_items[i-1]} 0 R " if i > 0 else ""
        nxt = f"/Next {outline_items[i+1]} 0 R " if i < PAGES - 1 else ""
        objects[item - 1] = (
            f"<< /BM ({mark(item)}) /Title (chapter {i+1}) /Parent {outline_root} 0 R "
            f"{prev}{nxt}/Dest [{page_objs[i]} 0 R /Fit] >>"
        ).encode()

    # The shared array has no marker of its own -- it is an array, not a dictionary -- so it is
    # declared unmarkable rather than silently absent from the manifest.
    unmarkable = [shared_annots]

    # EVERY DECLARED MARKER IS IN THE DOCUMENT. A manifest entry naming a string nobody wrote
    # is a channel that cannot fail any assertion, and it reads as coverage -- the denominator
    # in "N of 26 survived" counts it either way.
    document = build(objects, catalog)
    absent = [
        n for n, pages in owners.items()
        if n not in unmarkable and mark(n).encode() not in document
    ]
    if absent:
        raise SystemExit(
            f"objects {absent} are declared in the manifest but their markers are not in the "
            f"document, so those channels cannot fail any closure assertion"
        )

    (out / "marked.pdf").write_bytes(document)

    manifest = {
        "pages": PAGES,
        "marker_prefix": "BM-",
        "marked_count": len(owners) - len(unmarkable),
        "objects": {
            str(n): {"marker": mark(n), "pages": pages, "kind": kinds[n]}
            for n, pages in owners.items()
            if n not in unmarkable
        },
        "unmarkable": [
            {
                "object": n,
                "why": "an array, which has nowhere to carry a marker; its members are marked",
            }
            for n in unmarkable
        ],
    }
    (out / "marked.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"  marked.pdf               {len((out / 'marked.pdf').read_bytes()):>6} bytes")
    print(f"  marked.json              {len(manifest['objects']):>6} marked objects")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(f"usage: {sys.argv[0]} <output-dir>")
    main(Path(sys.argv[1]))
