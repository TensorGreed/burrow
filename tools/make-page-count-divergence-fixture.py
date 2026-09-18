#!/usr/bin/env python3
"""Build `five-pages-or-six.pdf`: a document whose page count depends on who is asked.

Six pages, declared `/Count 6` with six `/Kids`. Page three's `/Resources` points its `/XObject`
at object number 4294967296 -- above INT_MAX, the same shape the corpus already records for
`object-number-above-int-max.pdf`, but reachable through a page rather than through the trailer.

WHAT IT MEASURES, AND WHY IT IS COMMITTED WITH A FAILING TEST

  * a textual walk of the page tree reads SIX pages;
  * PDFium reads SIX;
  * qpdf's library `page_count` reads FIVE -- it drops the page it cannot resolve;
  * `split` therefore emits 2 + 3 = FIVE pages and reports SUCCESS.

One page is gone and nothing says so. ADR 0022 has every operation verify its output by reading
it back through a fresh engine -- and that verification passes here, because it compares the
output against the count the OPERATION read. Five went in, five came out, the promise is kept
against a number that was already wrong.

This is not a defect in `split`. It is the shape of the promise, and every operation that
carries a page count inherits it. See #111.

Run from the repository root:

    python3 tools/make-page-count-divergence-fixture.py tests/conformance/fixtures/five-pages-or-six.pdf
"""

import zlib, sys
pages = 6
buf = bytearray(b"%PDF-1.7\n"); offsets = {}
def obj(n, body):
    offsets[n] = len(buf)
    buf.extend(b"%d 0 obj\n" % n); buf.extend(body); buf.extend(b"\nendobj\n")
kids = [3 + 2*i for i in range(pages)]
obj(1, b"<< /Type /Catalog /Pages 2 0 R >>")
obj(2, b"<< /Type /Pages /Count %d /Kids [%s] >>" % (pages, b" ".join(b"%d 0 R" % k for k in kids)))
for i, k in enumerate(kids):
    content = zlib.compress(b"0 0 1 RG 4 w 50 %d m 500 %d l S\n" % (100 + i*40, 700 - i*40))
    res = b"<< /XObject << /X0 4294967296 0 R >> >>" if i == 2 else b"<< >>"
    obj(k, b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents %d 0 R /Resources %s >>" % (k+1, res))
    obj(k+1, b"<< /Length %d /Filter /FlateDecode >>\nstream\n" % len(content) + content + b"\nendstream")
xref = len(buf); top = max(offsets) + 1
buf.extend(b"xref\n0 %d\n0000000000 65535 f \n" % top)
for n in range(1, top): buf.extend(b"%010d 00000 n \n" % offsets.get(n, 0))
buf.extend(b"trailer\n<< /Size %d /Root 1 0 R >>\nstartxref\n%d\n%%%%EOF\n" % (top, xref))
open(sys.argv[1],"wb").write(bytes(buf))
print(sys.argv[1], len(buf), "bytes")
