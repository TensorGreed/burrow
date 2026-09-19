#!/usr/bin/env python3
"""PPM -> PNG, and a contact sheet, so the "would a person see it" column is an OBSERVATION.

The harness writes binary PPM because it has no dependencies and will not grow any. This turns
them into something a person (or a model) can look at. Spike-only.
"""
import struct, sys, zlib
from pathlib import Path


def read_ppm(path):
    data = path.read_bytes()
    fields, at = [], 2
    while len(fields) < 3:
        while at < len(data) and data[at : at + 1].isspace():
            at += 1
        if data[at : at + 1] == b"#":
            while data[at] != 0x0A:
                at += 1
            continue
        start = at
        while at < len(data) and not data[at : at + 1].isspace():
            at += 1
        fields.append(int(data[start:at]))
    at += 1
    w, h, _maxv = fields
    return w, h, data[at : at + w * h * 3]


def write_png(path, w, h, rgb):
    raw = b"".join(b"\0" + rgb[y * w * 3 : (y + 1) * w * 3] for y in range(h))

    def chunk(tag, body):
        return (
            struct.pack(">I", len(body))
            + tag
            + body
            + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFFFFFF)
        )

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 9))
    png += chunk(b"IEND", b"")
    path.write_bytes(png)


def downsample(w, h, rgb, k):
    nw, nh = w // k, h // k
    out = bytearray(nw * nh * 3)
    for y in range(nh):
        for x in range(nw):
            o = (y * nw + x) * 3
            i = ((y * k) * w + x * k) * 3
            out[o : o + 3] = rgb[i : i + 3]
    return nw, nh, bytes(out)


def sheet(paths, out, cols=2, gap=8, shrink=1):
    tiles = [read_ppm(p) for p in paths]
    if shrink > 1:
        tiles = [downsample(*t, shrink) for t in tiles]
    tw = max(t[0] for t in tiles)
    th = max(t[1] for t in tiles)
    rows = (len(tiles) + cols - 1) // cols
    W = cols * tw + (cols + 1) * gap
    H = rows * th + (rows + 1) * gap
    canvas = bytearray(b"\x60" * (W * H * 3))
    for i, (w, h, rgb) in enumerate(tiles):
        cx = gap + (i % cols) * (tw + gap)
        cy = gap + (i // cols) * (th + gap)
        for y in range(h):
            off = ((cy + y) * W + cx) * 3
            canvas[off : off + w * 3] = rgb[y * w * 3 : (y + 1) * w * 3]
    write_png(out, W, H, bytes(canvas))


if __name__ == "__main__":
    args = sys.argv[1:]
    if args[0] == "--sheet":
        sheet([Path(p) for p in args[3:]], Path(args[1]), cols=2, shrink=int(args[2]))
    else:
        for a in args:
            p = Path(a)
            w, h, rgb = read_ppm(p)
            write_png(p.with_suffix(".png"), w, h, rgb)
    print("ok")
