#!/usr/bin/env python3
"""Generate the object-stream decompression bomb, which the pre-scan does not see.

    python3 tools/make-objstm-bomb.py tests/conformance/fixtures/objstm-bomb.pdf

A small, structurally valid PDF whose catalogue, page tree and page all live inside a
**compressed object stream** padded with megabytes of spaces. PDFium must inflate that
stream to find the catalogue, so the expansion happens before it can know anything about
the document.

Why this file exists
--------------------
It is the regression test for a gap M1 PR 4b measured, and it is a different gap from the
one `xref-bomb.pdf` covers.

`xref-bomb.pdf` declares an enormous cross-reference, and the structural pre-scan
(`core/burrow-engines/src/prescan/`) reads that declaration and refuses the file before any
engine touches it. This file declares **nothing large at all**: its cross-reference has a
`/Size` of 6, and its one big stream's `/Length` is the *compressed* length, which is a few
hundred kilobytes and entirely honest. Every number the pre-scan reads is modest.

Measured on this build, with a 1.4 MB variant against the default 1 GiB ceiling:

    pre-scan          passes, estimating 384 bytes
    PDFium peak RSS   2,437 MB   -- roughly 1,750x the input
    outcome           Ok(1 page)

Neither check fires. The measured post-open check compares the resident set *before* and
*after*, so a peak that occurs *during* and is partly released is invisible to it: it saw a
1,028 MB delta against a 1,024 MiB ceiling and passed the file.

The committed fixture is deliberately much smaller than that variant -- a bomb that costs
gigabytes on every CI run buys nothing the same shape at 128 MiB does not. The 1.4 MB
version is what the numbers above were measured with; this generator's default is what
ships.

Deterministic: the same arguments always produce byte-identical output, so the digest
recorded in `expectations.json` is stable.
"""

import sys
import zlib

# How many mebibytes the object stream inflates to.
#
# 200 MiB, and it is pinned from three directions at once.
#
#   * It must exceed the conformance case's 96 MiB ceiling plus `MEASURED_NOISE_MARGIN_BYTES`
#     (64 MiB) with room to spare, so the measured check fires rather than the outcome
#     depending on allocator noise.
#   * The ceiling itself must stay above `MIN_CONVERGING_MEMORY_BYTES` (64 MiB), below which a
#     worker recycles on every operation and the case would be exercising that documented
#     pathology instead of the one it is here for.
#   * It must stay **below 256 MiB**, which is qpdf's `flate_max_memory` (ADR 0013 §5). Past
#     that, qpdf refuses the file outright and the case stops isolating the gap: measured at
#     256 MiB, qpdf returns `Malformed` while PDFium still opens it.
#
# That last constraint is worth reading twice, because it is the finding in miniature: qpdf has
# a decompression ceiling and honours it, and PDFium does not have one at all.
DEFAULT_PAD_MIB = 200

# Objects that live INSIDE the object stream. The catalogue in particular: PDFium cannot
# open the document without it, so the stream must be inflated before anything else.
INNER_OBJECTS = [
    (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
    (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
    (3, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] >>"),
]


def build(pad_mib: int) -> bytes:
    pairs, body = b"", b""
    for number, data in INNER_OBJECTS:
        pairs += b"%d %d " % (number, len(body))
        body += data + b"\n"

    # The padding is inside the stream, after the objects, so it is never parsed as an
    # object -- it only has to be inflated. Spaces compress to almost nothing.
    payload = pairs + body + b" " * (pad_mib * 1024 * 1024)
    stream = zlib.compress(payload, 9)

    out = bytearray(b"%PDF-1.5\n")
    objstm_offset = len(out)
    out += (
        b"4 0 obj\n<< /Type /ObjStm /N %d /First %d /Filter /FlateDecode /Length %d >>\nstream\n"
        % (len(INNER_OBJECTS), len(pairs), len(stream))
    )
    out += stream + b"\nendstream\nendobj\n"

    xref_offset = len(out)

    def entry(kind: int, a: int, b: int) -> bytes:
        return bytes([kind]) + a.to_bytes(4, "big") + b.to_bytes(2, "big")

    # Six entries, and that is the whole declaration the pre-scan gets to see: type 2
    # entries say "object N is item M of object stream 4", type 1 say "at this offset".
    table = (
        entry(0, 0, 65535)
        + entry(2, 4, 0)
        + entry(2, 4, 1)
        + entry(2, 4, 2)
        + entry(1, objstm_offset, 0)
        + entry(1, xref_offset, 0)
    )
    compressed_table = zlib.compress(table, 9)
    out += (
        b"5 0 obj\n<< /Type /XRef /Size 6 /W [1 4 2] /Root 1 0 R "
        b"/Filter /FlateDecode /Length %d >>\nstream\n" % len(compressed_table)
    )
    out += compressed_table + b"\nendstream\nendobj\n"
    out += b"startxref\n%d\n%%%%EOF\n" % xref_offset
    return bytes(out)


def main() -> int:
    if not 2 <= len(sys.argv) <= 3:
        print(f"usage: {sys.argv[0]} <output.pdf> [pad-mib]", file=sys.stderr)
        return 2
    pad = int(sys.argv[2]) if len(sys.argv) == 3 else DEFAULT_PAD_MIB
    data = build(pad)
    with open(sys.argv[1], "wb") as handle:
        handle.write(data)
    ratio = (pad * 1024 * 1024) / len(data)
    print(f"wrote {sys.argv[1]}: {len(data)} bytes, inflates ~{ratio:.0f}x to {pad} MiB")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
