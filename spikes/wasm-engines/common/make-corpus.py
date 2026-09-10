#!/usr/bin/env python3
"""Generate the spike's test PDFs.

Generated rather than downloaded: reproducible byte-for-byte, no licensing question,
and the malformed cases are precisely controlled. Run from anywhere; writes into
common/corpus/ beside this file.
"""
import pathlib
import sys
import zlib

OUT = pathlib.Path(__file__).parent / "corpus"


def build_pdf(pages: int, filler: bytes = b"") -> bytes:
    """A minimal but valid PDF with `pages` pages and an optional incompressible blob."""
    objs: list[bytes] = []

    def add(body: bytes) -> int:
        objs.append(body)
        return len(objs)  # 1-based object number

    # Page content, shared by every page.
    content = b"BT /F1 24 Tf 72 700 Td (burrow spike test page) Tj ET"
    content_num = add(b"<< /Length %d >>\nstream\n%s\nendstream" % (len(content), content))
    font_num = add(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>")

    # Reserve the Pages object number so Kids can reference it as parent.
    pages_num = len(objs) + 1
    objs.append(b"")  # placeholder, filled below

    kids = []
    for _ in range(pages):
        n = add(
            b"<< /Type /Page /Parent %d 0 R /MediaBox [0 0 612 792] "
            b"/Resources << /Font << /F1 %d 0 R >> >> /Contents %d 0 R >>"
            % (pages_num, font_num, content_num)
        )
        kids.append(n)

    objs[pages_num - 1] = b"<< /Type /Pages /Count %d /Kids [%s] >>" % (
        pages,
        b" ".join(b"%d 0 R" % k for k in kids),
    )

    catalog_num = add(b"<< /Type /Catalog /Pages %d 0 R >>" % pages_num)

    # An incompressible stream, to make a file genuinely large rather than
    # large-when-decompressed. A compression bomb would test a different thing.
    if filler:
        add(b"<< /Length %d >>\nstream\n%s\nendstream" % (len(filler), filler))

    # Serialise with a correct classic xref table.
    out = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
    offsets = []
    for i, body in enumerate(objs, start=1):
        offsets.append(len(out))
        out += b"%d 0 obj\n" % i + body + b"\nendobj\n"

    xref_at = len(out)
    out += b"xref\n0 %d\n" % (len(objs) + 1)
    out += b"0000000000 65535 f \n"
    for off in offsets:
        out += b"%010d 00000 n \n" % off
    out += b"trailer\n<< /Size %d /Root %d 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (
        len(objs) + 1,
        catalog_num,
        xref_at,
    )
    return bytes(out)


def main() -> int:
    OUT.mkdir(parents=True, exist_ok=True)
    written = []

    small = build_pdf(1)
    (OUT / "small-1page.pdf").write_bytes(small)
    written.append(("small-1page.pdf", len(small), "1 page, baseline open"))

    medium = build_pdf(100)
    (OUT / "medium-100page.pdf").write_bytes(medium)
    written.append(("medium-100page.pdf", len(medium), "100 pages, per-page scaling"))

    # ~50 MB of data that does not compress: a deterministic LCG, not random, so the
    # file is byte-identical on every run.
    n = 50 * 1024 * 1024
    buf = bytearray(n)
    x = 0x12345678
    for i in range(n):
        x = (1103515245 * x + 12345) & 0xFFFFFFFF
        buf[i] = (x >> 16) & 0xFF
    large = build_pdf(1, filler=bytes(buf))
    (OUT / "large-50mb.pdf").write_bytes(large)
    written.append(("large-50mb.pdf", len(large), "~50 MB incompressible, peak memory"))

    # Truncated mid-object: the xref promises objects that are not there.
    trunc = small[: len(small) * 2 // 3]
    (OUT / "malformed-truncated.pdf").write_bytes(trunc)
    written.append(("malformed-truncated.pdf", len(trunc), "truncated, must not abort"))

    # Valid structure, corrupted xref offsets: the repair path, not the reject path.
    bad = bytearray(medium)
    i = bad.find(b"\nxref\n")
    assert i > 0, "xref marker not found"
    # Point every entry at a bogus offset.
    j = bad.find(b"0000000000 65535 f", i)
    for k in range(j, min(j + 2000, len(bad) - 20), 20):
        if bad[k : k + 10].isdigit():
            bad[k : k + 10] = b"9999999999"
    (OUT / "malformed-badxref.pdf").write_bytes(bytes(bad))
    written.append(("malformed-badxref.pdf", len(bad), "corrupt xref, repair path"))

    # Not a PDF at all.
    notpdf = b"this is not a pdf, not even slightly\n" * 100
    (OUT / "malformed-notpdf.bin").write_bytes(notpdf)
    written.append(("malformed-notpdf.bin", len(notpdf), "not a PDF, header check"))

    w = max(len(n) for n, _, _ in written)
    for name, size, why in written:
        print(f"  {name:<{w}}  {size:>10,} B  {why}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
