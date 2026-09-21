#!/usr/bin/env python3
"""Shared PDF construction for the redaction corpus generators.

Extracted from `tools/make-redaction-fixtures.py` when `tools/make-evasion-fixtures.py` needed
the same builders. Two generators copying a hand-rolled PDF writer between them is two writers
to keep in step, and the fixtures they produce have to share a page geometry exactly -- the
region a redaction is asked to clear is the same rectangle in every one of them, or the corpus
is comparing different questions.

Not production code. No `Limits`, no typed errors: a generator that fails should fail loudly and
stop.
"""

from __future__ import annotations

import shutil
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import blockfont  # noqa: E402

PAGE_W, PAGE_H = 400, 200
SECRET_X, SECRET_Y, SECRET_SIZE = 40, 100, 20
KEEP_X, KEEP_Y, KEEP_SIZE = 40, 40, 14
REGION = (30, 88, 370, 132)  # the rectangle a redaction is asked to clear
KEEP_LINE = "KEEP-THIS-LINE"

# A decoy of exactly the secret's length, for channel 21's lying `/ToUnicode`.
DECOY = "HARMLESS-FILLER-"


ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789- "


# ---------------------------------------------------------------------------
# A byte-exact PDF writer. Classic xref table, no object streams, no compression
# unless a channel's subject is the filter.
# ---------------------------------------------------------------------------


class Pdf:
    def __init__(self) -> None:
        self.objects: dict[int, bytes] = {}
        self._next = 1
        self.extra_trailer = b""

    def reserve(self) -> int:
        num = self._next
        self._next += 1
        return num

    def put(self, num: int, body: bytes) -> int:
        self.objects[num] = body
        return num

    def add(self, body: bytes) -> int:
        return self.put(self.reserve(), body)

    def stream(self, extra: bytes, data: bytes) -> int:
        body = b"<< " + extra + b" /Length " + str(len(data)).encode() + b" >>\nstream\n"
        body += data + b"\nendstream"
        return self.add(body)

    def build(self, root: int, info: int | None = None) -> bytes:
        out = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
        offsets: dict[int, int] = {}
        for num in sorted(self.objects):
            offsets[num] = len(out)
            out += f"{num} 0 obj\n".encode() + self.objects[num] + b"\nendobj\n"
        size = max(self.objects) + 1
        xref_at = len(out)
        out += f"xref\n0 {size}\n".encode()
        out += b"0000000000 65535 f \n"
        for num in range(1, size):
            if num in offsets:
                out += f"{offsets[num]:010d} 00000 n \n".encode()
            else:
                out += b"0000000000 65535 f \n"
        trailer = f"trailer\n<< /Size {size} /Root {root} 0 R".encode()
        if info is not None:
            trailer += f" /Info {info} 0 R".encode()
        trailer += self.extra_trailer + b" >>\n"
        out += trailer + f"startxref\n{xref_at}\n%%EOF\n".encode()
        return bytes(out)


def literal(text: str) -> bytes:
    """A PDF literal string, with the three characters that need escaping escaped."""
    escaped = text.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)")
    return b"(" + escaped.encode("latin-1") + b")"


def utf16_hex(text: str) -> bytes:
    """A UTF-16BE hex string with a BOM -- how a real producer writes a field value."""
    return b"<" + (b"\xfe\xff" + text.encode("utf-16-be")).hex().upper().encode() + b">"


def keep_line_ops(font: bytes = b"/Helv") -> bytes:
    return (
        b"BT " + font + b" " + str(KEEP_SIZE).encode() + b" Tf "
        + f"{KEEP_X} {KEEP_Y} Td ".encode()
        + literal(KEEP_LINE) + b" Tj ET\n"
    )


def helvetica(pdf: Pdf) -> int:
    return pdf.add(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Name /Helv >>")


# ---------------------------------------------------------------------------
# Rasterising the canary, for the channels whose subject is pixels rather than text.
# ---------------------------------------------------------------------------


def raster(text: str, scale: int = 2) -> tuple[int, int, bytes]:
    """The canary as a 1-byte-per-pixel greyscale image: black glyphs on white.

    Uses `blockfont.BITMAP`, so the pixels a person sees here are the same shapes the font
    draws. That matters: channel 14 and channel 20 are only interesting if what is in the
    pixels is legibly the secret.
    """
    cols = len(text) * 6
    w, h = cols * scale, 7 * scale
    rows = bytearray(b"\xff" * (w * h))
    for i, ch in enumerate(text):
        bits = blockfont.BITMAP[ch]
        for r, row in enumerate(bits):
            for c, bit in enumerate(row):
                if bit != "1":
                    continue
                for dy in range(scale):
                    for dx in range(scale):
                        x = (i * 6 + c) * scale + dx
                        y = r * scale + dy
                        rows[y * w + x] = 0x00
    return w, h, bytes(rows)


def outline_ops(text: str, x: float, y: float, size: float) -> bytes:
    """The canary drawn as filled rectangles -- no font, no text object, no glyph."""
    unit = size / 7.0
    ops = [b"0 g\n"]
    for i, ch in enumerate(text):
        for r, row in enumerate(blockfont.BITMAP[ch]):
            for c, bit in enumerate(row):
                if bit != "1":
                    continue
                px = x + (i * 6 + c) * unit
                py = y + (6 - r) * unit
                ops.append(f"{px:.2f} {py:.2f} {unit:.2f} {unit:.2f} re f\n".encode())
    return b"".join(ops)



def simple_page(
    pdf: Pdf,
    content: bytes,
    resources: bytes,
    page_extra: bytes = b"",
    catalog_extra: bytes = b"",
    info: int | None = None,
) -> bytes:
    pages = pdf.reserve()
    page = pdf.reserve()
    stream = pdf.stream(b"", content)
    pdf.put(
        page,
        b"<< /Type /Page /Parent " + str(pages).encode() + b" 0 R"
        b" /MediaBox [0 0 " + f"{PAGE_W} {PAGE_H}".encode() + b"]"
        b" /Resources << " + resources + b" >>"
        b" /Contents " + str(stream).encode() + b" 0 R" + page_extra + b" >>",
    )
    pdf.put(
        pages,
        b"<< /Type /Pages /Count 1 /Kids [" + str(page).encode() + b" 0 R] >>",
    )
    root = pdf.add(
        b"<< /Type /Catalog /Pages " + str(pages).encode() + b" 0 R" + catalog_extra + b" >>"
    )
    return pdf.build(root, info)


def repo_root() -> Path:
    # ONE level up, not two. The spike lived at `spikes/redaction-survival/`; this lives in
    # `tools/`, and the ported copy kept the spike's depth until it could not find qpdf.
    return Path(__file__).resolve().parent.parent


def vendored(*parts: str) -> Path:
    import platform

    arch = platform.machine()
    return repo_root() / "engines" / "vendor" / f"native-{arch}" / Path(*parts)


def qpdf_cli() -> Path:
    import platform

    arch = platform.machine()
    path = (
        repo_root() / "engines" / "vendor" / "src" / f"build-qpdf-plain-{arch}" / "qpdf" / "qpdf"
    )
    if not path.exists():
        found = shutil.which("qpdf")
        if found:
            return Path(found)
        raise SystemExit(
            f"no qpdf: expected the vendored build at {path}. Run engines/fetch.sh and "
            "engines/build-native.sh."
        )
    return path

