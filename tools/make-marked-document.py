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

        `page-tree` is structure that exists only to HOLD other pages: an intermediate
        `/Pages` node. Nothing in it is anybody's content, and no reader can observe it. qpdf
        flattens the tree whenever a page moves, so `reorder` legitimately loses these
        (ADR 0021) while still losing nothing of the other two kinds.

        Without the second kind the inverse assertion had to require every owned object, and
        a correct one-way split failed it by dropping five outline entries on purpose. The
        third is the same argument one operation further on -- and it is a third kind rather
        than a weakening of `assert_nothing_lost` to a subset check, which would have quietly
        weakened it for `rotate` and `compress` too.
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

    # TWO LEVELS, NOT ONE, so a flattening is observable. A flat tree makes `reorder`'s
    # structural cost invisible: qpdf flattens a tree that is already flat into itself and the
    # harness reports a clean run, which would be a measurement of nothing.
    #
    # The branches are declared `page-tree` and OWNED by the pages they hold, so `owned_by`
    # with the content and navigation kinds excludes them BY NAME rather than by their page
    # list being empty -- a branch that is merely scaffolding would be allowed anywhere, and
    # a branch surviving into an output that excluded its pages is a real trespass.
    half = PAGES // 2
    left = add(b"", list(range(1, half + 1)), kind="page-tree")
    right = add(b"", list(range(half + 1, PAGES + 1)), kind="page-tree")
    branches = [left, right]

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
    #
    # `navigation`, not `content`, and the kind changed when #54 closed. A field is reached from
    # the catalog's `/AcroForm` and describes the whole document; `split` drops it by cutting
    # every widget's `/Parent` (ADR 0019 §2b's "drop the widget's field rather than ship a dead
    # one"), so requiring it to survive would require the opposite of the decision. The WIDGETS
    # stay `content`: they are annotations on their own pages and must survive.
    field = add(b"", [1, 4], kind="navigation")
    widget1 = add(b"", [1])
    widget4 = add(b"", [4])

    # One /Annots array referenced by pages 1 and 4, holding each page's own annotation.
    shared_annots = add(b"", [1, 4])
    annot4 = add(b"", [4])

    # An article bead on page 1 reaching a thread whose info names page 4.
    #
    # All three are `navigation` for the same reason the field is: `/B` is not on split's page
    # allowlist and `/Threads` lives on the catalog, so the whole article structure is dropped
    # (ADR 0019 §2a row 4). PDF classes articles as a navigation feature, which is the same
    # sentence this kind already carried for outlines.
    bead = add(b"", [1], kind="navigation")
    thread = add(b"", [1, 4], kind="navigation")
    thread_info = add(b"", [4], kind="navigation")

    # An outline entry per page, and the outline root.
    outline_root = add(b"", [], kind="navigation")
    outline_items = [add(b"", [i + 1], kind="navigation") for i in range(PAGES)]

    # Fill the bodies now that every number is known.
    objects[catalog - 1] = (
        f"<< /Type /Catalog /BM ({mark(catalog)}) /Pages {tree} 0 R "
        f"/Outlines {outline_root} 0 R /AcroForm << /Fields [{field} 0 R] >> "
        f"/Threads [{thread} 0 R] >>"
    ).encode()
    # The root holds the two branches; the inherited `/Resources` stays here, so every page
    # gets it through two levels of `/Parent` rather than one.
    objects[tree - 1] = (
        f"<< /Type /Pages /BM ({mark(tree)}) /Kids [{left} 0 R {right} 0 R] /Count {PAGES} "
        f"/Resources << /XObject << /Only4 {only4} 0 R >> >> >>"
    ).encode()
    for branch, held in ((left, page_objs[:half]), (right, page_objs[half:])):
        kids = " ".join(f"{p} 0 R" for p in held)
        objects[branch - 1] = (
            f"<< /Type /Pages /BM ({mark(branch)}) /Parent {tree} 0 R /Kids [{kids}] "
            f"/Count {len(held)} >>"
        ).encode()
    for i, p in enumerate(page_objs):
        extra = ""
        if i in (0, 3):
            extra += f"/Annots {shared_annots} 0 R "
        if i == 0:
            extra += f"/B [{bead} 0 R] "
        # No `/Resources` of its own, so the inherited one is what it gets.
        parent = left if i < half else right
        # NO `/BM` ON THE PAGE ITSELF. `split`'s pruning removes every page key outside the
        # specified set (ADR 0019 §2b), which is the rule that closes `/B`, `/AA`, `/Thumb`
        # and everything nobody enumerated -- and `/BM` is exactly such a key. A marker here
        # would be removed from a page that survived perfectly well, and the harness would
        # report five lost pages on a correct operation: a measurement of the marker rather
        # than of the object. The page is declared `unmarkable` below, with that reason, and
        # its survival is witnessed by its content stream, which is marked and owned by the
        # same page.
        objects[p - 1] = (
            f"<< /Type /Page /Parent {parent} 0 R /MediaBox [0 0 200 200] "
            f"/Contents {contents[i]} 0 R {extra}>>"
        ).encode()
        # PAGE 4 ACTUALLY DRAWS THE INHERITED RESOURCE. Until split pruned by usage, this
        # fixture claimed "a resource only page 4 draws" while no page's content stream
        # mentioned `/Only4` at all -- so the object survived into every output for the
        # trivial reason that nothing had a reason to remove it, and a resource filter that
        # pruned it from EVERY output (including page 4's own) looked identical to one that
        # pruned it correctly. `add-operation` §2c, from the other direction: a fixture whose
        # channel is never exercised cannot distinguish a correct implementation from a
        # destructive one. Measured: `a_one_way_split_loses_no_page_content` failed on an
        # operation that excluded nothing.
        draws = " /Only4 Do" if i == 3 else ""
        objects[contents[i] - 1] = stream(
            f"{mark(contents[i])} page {i+1}{draws}".encode()
        )
    objects[field - 1] = (
        f"<< /FT /Tx /BM ({mark(field)}) /T (field) /V ({mark(thread_info)}-typed-on-page-4) "
        f"/Kids [{widget1} 0 R {widget4} 0 R] >>"
    ).encode()
    # EACH WIDGET CARRIES `/P`, and each is in the shared `/Annots` array.
    #
    # Both were wrong until #54 closed, and both mattered. `widget4` was in the field's `/Kids`
    # and in no page's `/Annots` at all -- a widget no page displays, which no real producer
    # emits and which made it unreachable the moment split cut the `/Parent` edge. And no
    # annotation had a `/P`, so the fixture only ever exercised the *conservative* half of the
    # annotation filter (drop what cannot prove where it lives) and never the precise half
    # (keep what says it is here). `annot4` is deliberately left WITHOUT a `/P`, so one of each
    # is present: the precise path and the fallback.
    objects[widget1 - 1] = (
        f"<< /Type /Annot /Subtype /Widget /BM ({mark(widget1)}) /Parent {field} 0 R "
        f"/P {page_objs[0]} 0 R /Rect [0 0 9 9] >>"
    ).encode()
    objects[widget4 - 1] = (
        f"<< /Type /Annot /Subtype /Widget /BM ({mark(widget4)}) /Parent {field} 0 R "
        f"/P {page_objs[3]} 0 R /Rect [0 0 9 9] >>"
    ).encode()
    objects[shared_annots - 1] = f"[{widget1} 0 R {widget4} 0 R {annot4} 0 R]".encode()
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

    # What cannot carry a marker, declared rather than silently absent from the manifest.
    #
    #   * the shared `/Annots` array -- an array has nowhere to put one;
    #   * every page dictionary -- see the comment where they are built. A marker is a key
    #     outside the specified page set, and removing exactly those keys is the rule that
    #     closes half of ADR 0019 §2a. Marking a page would measure the rule rather than the
    #     page.
    #
    # PER-OBJECT REASONS, not one reason for the list. They are unmarkable for two entirely
    # different causes, and a single shared string would have said "an array, which has nowhere
    # to carry a marker" about five page dictionaries.
    unmarkable = {
        shared_annots: "an array, which has nowhere to carry a marker; its members are marked",
        **{
            p: (
                "a page dictionary, whose every key outside the specified set split's pruning "
                "removes -- so a marker here would measure the rule rather than the page. Its "
                "survival is witnessed by its content stream."
            )
            for p in page_objs
        },
    }

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
        "unmarkable": [{"object": n, "why": why} for n, why in unmarkable.items()],
        # NAMED ROLES, so a test can say "the resource only page 4 draws" without hardcoding an
        # object number. The numbers here shift whenever this generator gains an object, and a
        # test pinned to a literal would then assert something about whatever moved into its
        # place -- silently, and in the direction of passing. `marker` is carried alongside so a
        # failure message can name the thing rather than a number.
        # BARE NUMBERS, not objects with their own fields. The manifest reader in
        # `object_closure.rs` is hand-rolled and finds objects by splitting on `"marker":`, so a
        # role carrying a marker of its own was read as three extra objects and every closure
        # test failed with "a pages list". A role is a pointer; the marker is already in
        # `objects`, which is where a reader should look it up.
        "roles": {
            "inherited_resource": only4,
            "shared_annots_array": shared_annots,
            "annotation_without_p": annot4,
        },
    }
    (out / "marked.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"  marked.pdf               {len((out / 'marked.pdf').read_bytes()):>6} bytes")
    print(f"  marked.json              {len(manifest['objects']):>6} marked objects")
    print(f"  page tree                two levels, branches {branches} (kind page-tree)")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(f"usage: {sys.argv[0]} <output-dir>")
    main(Path(sys.argv[1]))
