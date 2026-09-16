#!/usr/bin/env python3
"""Generate the category fixtures spike 0005's compression measurement runs on.

    python3 tools/make-compress-fixtures.py <output-dir>

Five categories, because "how much does compress save" has no single answer and an average
across them would be the least useful number available: a scan, a photo-heavy document, a
text-heavy report, a form, and a file that has already been optimised. Plus a linearized
variant, because dropping hint streams is one of the levers under measurement.

These are NOT conformance fixtures and do not belong in `tests/conformance/fixtures/`:
nothing asserts a typed outcome for them. They exist so spike 0005's table can be
re-measured, which is what stops it being a claim. Same reasoning, and the same hand-built
uncompressed style, as `tools/make-merge-fidelity-fixtures.py`.

WHAT THESE FIXTURES CAN AND CANNOT MEASURE
------------------------------------------
`CLAUDE.md`: *a test harness that generates its own inputs is measuring what it can
generate.* That applies here with force, and unevenly, so it is written per category rather
than waved at once:

- The STRUCTURAL properties are honest. Object count, object size distribution, whether an
  xref is a table or a stream, whether content streams arrive compressed, whether there are
  unreferenced objects -- these are things a real naive producer really does, and they are
  reproduced here faithfully. Every saving qpdf can actually deliver acts on exactly these.

- The IMAGE payloads are OUR ENCODER'S OUTPUT and nothing more. `scanned` and `photo-heavy`
  carry real DCT data from the vendored libjpeg-turbo `cjpeg` -- real, but at a quality
  WE chose, over synthetic imagery WE drew. How much a JPEG could be shrunk by re-encoding
  it is therefore a fact about this script, not about the world.

  That happens not to matter for the question being asked, and the reason is worth stating
  rather than relying on: **qpdf does not touch DCT data at any setting.** These two
  fixtures are here to show how little of such a file is reachable at all, which is a
  structural fact the fixtures CAN support.

- The EMBEDDED FONT in `text-report` is a real woff2 face's bytes carried as a font stream.
  Not a real PDF font program -- it is a stand-in for one, chosen because it has a real
  font's entropy rather than a synthesised blob's. It stands for the realistic case: an
  embedded font that already arrives entropy-coded and will not shrink. A fixture carrying
  an artificially compressible "font" would manufacture a saving.

- What NO fixture here can support is a claim about the real-world DISTRIBUTION of these
  categories. Five files are five files. The distribution arm of the measurement is qpdf's
  own test suite, which at least nobody here chose.
"""

import platform
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# DERIVED, not hard-coded. `engines/build-native.sh` names these directories from `uname -m`,
# so an `aarch64` literal makes this script unusable on x86_64 rather than merely slower.
ARCH = platform.machine()

# The vendored, checksum-pinned encoder -- not whatever `cjpeg` is on PATH. Provenance for a
# measurement input matters for the same reason it matters for a shipped artifact.
CJPEG = REPO / f"engines/vendor/native-{ARCH}/bin/cjpeg"
QPDF = REPO / f"engines/vendor/src/build-qpdf-plain-{ARCH}/qpdf/qpdf"
# A real face's bytes, used as a font-sized stream with a real font's entropy. See the
# module docstring: this is a stand-in, labelled as one.
FONT = REPO / "apps/web/public/fonts/atkinson-hyperlegible-next-v2.001-latin.woff2"


# ---------------------------------------------------------------- PDF construction
#
# Hand-built, uncompressed, byte-exact xref -- deliberately the shape a naive producer
# emits, because that is the shape the operation under measurement is supposed to improve.
# Building these through a library that already optimises would measure the library.


def build(objects, root, extra_trailer=""):
    """Assemble numbered objects into a PDF with a classic cross-reference TABLE.

    A table rather than a stream, and no object streams, on purpose: generating those is
    precisely what arm A is being measured for, so a fixture that already had them would
    start at the finish line.
    """
    out = bytearray(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n")
    offsets = [0] * (len(objects) + 1)
    for n, body in enumerate(objects, start=1):
        offsets[n] = len(out)
        out += f"{n} 0 obj\n".encode() + body + b"\nendobj\n"
    xref = len(out)
    out += f"xref\n0 {len(objects) + 1}\n".encode()
    out += b"0000000000 65535 f \n"
    for n in range(1, len(objects) + 1):
        out += f"{offsets[n]:010d} 00000 n \n".encode()
    out += (
        f"trailer\n<< /Size {len(objects) + 1} /Root {root} 0 R {extra_trailer}>>\n"
        f"startxref\n{xref}\n%%EOF\n"
    ).encode()
    return bytes(out)


def stream(data, extra=""):
    """An UNCOMPRESSED stream. `qpdf_set_compress_streams` is what should fix that."""
    return f"<< /Length {len(data)} {extra}>>\nstream\n".encode() + data + b"\nendstream"


def raw_stream(data, extra=""):
    """A stream whose bytes are already encoded -- DCT, or an entropy-coded font.

    Identical to `stream` by construction, and named separately only so the call sites read
    as what they are: this is the data qpdf cannot improve, and a measurement that could not
    tell the two apart would not be able to say where a saving came from. It delegates rather
    than repeating the body, so the next reader does not have to check whether the two
    spellings had quietly drifted.
    """
    return stream(data, extra)


# ---------------------------------------------------------------- imagery
#
# Synthetic, and drawn here rather than fetched, so the fixtures need no licence and no
# provenance entry. What they are NOT is photographic: see the module docstring.


def ppm_scan(width, height, seed=1):
    """A page that looks like a scan: near-white, with text-like marks and sensor noise.

    Grayscale. The noise is what makes it a scan rather than a drawing -- a clean synthetic
    page would JPEG down to almost nothing and would flatter every number in the table.
    """
    rnd = seed
    rows = bytearray()
    for y in range(height):
        row = bytearray(b"\xf2" * width)
        # Text-like marks: bands of short dark runs, the way lines of type scan.
        if 60 < y % 140 < 92 and y > 80:
            x = 70
            while x < width - 90:
                rnd = (rnd * 1103515245 + 12345) & 0x7FFFFFFF
                run = 3 + rnd % 9
                for i in range(run):
                    if x + i < width:
                        row[x + i] = 0x14
                x += run + 2 + (rnd >> 8) % 5
        for x in range(width):
            rnd = (rnd * 1103515245 + 12345) & 0x7FFFFFFF
            v = row[x] + ((rnd >> 16) % 13) - 6
            row[x] = 0 if v < 0 else (255 if v > 255 else v)
        rows += row
    return b"P5\n%d %d\n255\n" % (width, height) + bytes(rows)


def ppm_photo(width, height, seed=7):
    """A colour image with smooth gradients and detail, which is what JPEG is built for."""
    rnd = seed
    rows = bytearray()
    for y in range(height):
        for x in range(width):
            rnd = (rnd * 1103515245 + 12345) & 0x7FFFFFFF
            n = (rnd >> 16) % 23 - 11
            r = (x * 255) // width
            g = (y * 255) // height
            b = ((x + y) * 255) // (width + height)
            # Detail, so the encoder has something to spend bits on.
            if (x // 17 + y // 13) % 3 == 0:
                r, g, b = 255 - r, 255 - b, g
            rows += bytes(
                (
                    max(0, min(255, r + n)),
                    max(0, min(255, g + n)),
                    max(0, min(255, b + n)),
                )
            )
    return b"P6\n%d %d\n255\n" % (width, height) + bytes(rows)


def jpeg(ppm, quality):
    """Encode through the VENDORED libjpeg-turbo, not whatever is on PATH."""
    return subprocess.run(
        [str(CJPEG), "-quality", str(quality)],
        input=ppm,
        stdout=subprocess.PIPE,
        check=True,
    ).stdout


# ---------------------------------------------------------------- the five categories


def scanned(pages=12):
    """A scan: every page one full-page DCT image, and almost nothing else.

    The point of the category. Such a file is ~99% DCT bytes, and DCT bytes are the one
    thing qpdf cannot touch at any setting -- so whatever arm A saves here is the ceiling on
    what structure alone is worth when structure is a rounding error of the file.
    """
    objs = []
    kids = []
    page_objs = []
    for i in range(pages):
        img = jpeg(ppm_scan(850, 1100, seed=i * 977 + 3), 55)
        objs.append(
            raw_stream(
                img,
                "/Type /XObject /Subtype /Image /Width 850 /Height 1100 "
                "/ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /DCTDecode ",
            )
        )
        img_num = len(objs)
        content = b"q 612 0 0 792 0 0 cm /Im0 Do Q\n"
        objs.append(stream(content))
        content_num = len(objs)
        # PER-PAGE DUPLICATED RESOURCES. A naive producer writes the resource dictionary out
        # again for every page rather than inheriting it, which is one of the things object
        # streams and garbage collection are supposed to make cheaper.
        objs.append(
            (
                f"<< /Type /Page /Parent {{PAGES}} 0 R /MediaBox [0 0 612 792] "
                f"/Contents {content_num} 0 R "
                f"/Resources << /XObject << /Im0 {img_num} 0 R >> >> >>"
            ).encode()
        )
        page_objs.append(len(objs))
    objs.append(b"{PAGES_SELF}")
    pages_num = len(objs)
    kids = " ".join(f"{n} 0 R" for n in page_objs)
    objs[pages_num - 1] = (
        f"<< /Type /Pages /Count {pages} /Kids [{kids}] >>"
    ).encode()
    objs.append(f"<< /Type /Catalog /Pages {pages_num} 0 R >>".encode())
    root = len(objs)
    objs = [o.replace(b"{PAGES}", str(pages_num).encode()) for o in objs]
    return build(objs, root)


def photo_heavy(pages=6):
    """Photographs plus a little text -- the shape of a brochure or a photo report."""
    objs = []
    page_objs = []
    for i in range(pages):
        img = jpeg(ppm_photo(900, 640, seed=i * 613 + 11), 80)
        objs.append(
            raw_stream(
                img,
                "/Type /XObject /Subtype /Image /Width 900 /Height 640 "
                "/ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode ",
            )
        )
        img_num = len(objs)
        content = (
            b"q 540 0 0 384 36 360 cm /Im0 Do Q\n"
            b"BT /F1 11 Tf 36 320 Td (Figure " + str(i + 1).encode() + b". "
            b"A caption of the sort a photo report carries under every plate.) Tj ET\n"
        )
        objs.append(stream(content))
        content_num = len(objs)
        objs.append(
            (
                f"<< /Type /Page /Parent {{PAGES}} 0 R /MediaBox [0 0 612 792] "
                f"/Contents {content_num} 0 R /Resources << /XObject << /Im0 {img_num} 0 R >> "
                f"/Font << /F1 << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> >> >> >>"
            ).encode()
        )
        page_objs.append(len(objs))
    objs.append(b"placeholder")
    pages_num = len(objs)
    kids = " ".join(f"{n} 0 R" for n in page_objs)
    objs[pages_num - 1] = f"<< /Type /Pages /Count {pages} /Kids [{kids}] >>".encode()
    objs.append(f"<< /Type /Catalog /Pages {pages_num} 0 R >>".encode())
    root = len(objs)
    objs = [o.replace(b"{PAGES}", str(pages_num).encode()) for o in objs]
    return build(objs, root)


PROSE = (
    "The measurement below is reported per category rather than as an average, because an "
    "average across document kinds is the least useful number available. A scan and a form "
    "do not respond to the same treatment, and a single figure hides which one a reader is "
    "holding. Where a saving is structural it is stated as structural; where it is absent "
    "it is stated as absent."
)


def text_report(pages=40, embed_font=True):
    """A text-heavy report: long UNCOMPRESSED content streams, one embedded font stream.

    The category where structure is most of the file, so it is where arm A has the most to
    work with. The font stream is deliberately incompressible (see the module docstring) so
    the saving reported here comes from the content streams and the object structure rather
    than from a blob chosen to shrink.
    """
    # `embed_font=False` produces the variant Finding 5 measures the font's share BY
    # SUBTRACTION against. Without it that number is a figure nobody can re-derive, which is
    # the thing this whole file exists to prevent.
    font_bytes = FONT.read_bytes() if embed_font else b""
    objs = [
        raw_stream(font_bytes, "/Subtype /OpenType "),
    ]
    font_file = len(objs)
    objs.append(
        (
            f"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica "
            f"/FontDescriptor << /Type /FontDescriptor /FontName /Helvetica /Flags 32 "
            f"/FontFile3 {font_file} 0 R >> >>"
        ).encode()
    )
    font_num = len(objs)
    page_objs = []
    for i in range(pages):
        lines = [b"BT /F1 10 Tf 54 738 Td 13 TL"]
        words = PROSE.split()
        for ln in range(46):
            chunk = " ".join(
                words[(ln * 7 + j) % len(words)] for j in range(11)
            )
            lines.append(f"({chunk} {i + 1}.{ln + 1}) Tj T*".encode())
        lines.append(b"ET")
        objs.append(stream(b"\n".join(lines) + b"\n"))
        content_num = len(objs)
        objs.append(
            (
                f"<< /Type /Page /Parent {{PAGES}} 0 R /MediaBox [0 0 612 792] "
                f"/Contents {content_num} 0 R "
                f"/Resources << /Font << /F1 {font_num} 0 R >> >> >>"
            ).encode()
        )
        page_objs.append(len(objs))
    # UNREFERENCED OBJECTS. A producer that rewrites a document in place leaves these behind,
    # and dropping them is `qpdf_set_preserve_unreferenced_objects(false)`. Without any here
    # that lever would measure zero and read as "the lever does nothing".
    for i in range(12):
        objs.append(stream(f"% orphaned revision {i}\n".encode() * 40))
    objs.append(b"placeholder")
    pages_num = len(objs)
    kids = " ".join(f"{n} 0 R" for n in page_objs)
    objs[pages_num - 1] = f"<< /Type /Pages /Count {pages} /Kids [{kids}] >>".encode()
    objs.append(f"<< /Type /Catalog /Pages {pages_num} 0 R >>".encode())
    root = len(objs)
    objs = [o.replace(b"{PAGES}", str(pages_num).encode()) for o in objs]
    return build(objs, root)


def form(pages=8, fields_per_page=40):
    """A form: many small dictionary objects, which is object streams' best case.

    Each field is two objects -- the field dictionary and its widget annotation -- so this
    fixture is 640 tiny objects plus their cross-reference entries. If `qpdf_o_generate` is
    worth anything anywhere, it is worth it here, and a measurement that did not include
    this shape would understate the lever.
    """
    objs = []
    page_objs = []
    field_refs = []
    for p in range(pages):
        annots = []
        for f in range(fields_per_page):
            name = f"field_{p + 1}_{f + 1}"
            y = 720 - (f % 20) * 34
            x = 54 if f < 20 else 320
            objs.append(
                (
                    f"<< /Type /Annot /Subtype /Widget /Rect [{x} {y} {x + 220} {y + 22}] "
                    f"/FT /Tx /T ({name}) /V (value for {name}) /DA (/Helv 9 Tf 0 g) "
                    f"/F 4 /P {{PAGE_{p}}} 0 R >>"
                ).encode()
            )
            annots.append(len(objs))
            field_refs.append(len(objs))
        content = (
            f"BT /F1 14 Tf 54 756 Td (Application form, sheet {p + 1}) Tj ET\n".encode()
        )
        objs.append(stream(content))
        content_num = len(objs)
        annot_list = " ".join(f"{n} 0 R" for n in annots)
        objs.append(
            (
                f"<< /Type /Page /Parent {{PAGES}} 0 R /MediaBox [0 0 612 792] "
                f"/Contents {content_num} 0 R /Annots [{annot_list}] "
                f"/Resources << /Font << /F1 << /Type /Font /Subtype /Type1 "
                f"/BaseFont /Helvetica >> >> >> >>"
            ).encode()
        )
        page_objs.append(len(objs))
    objs.append(b"placeholder")
    pages_num = len(objs)
    kids = " ".join(f"{n} 0 R" for n in page_objs)
    objs[pages_num - 1] = f"<< /Type /Pages /Count {pages} /Kids [{kids}] >>".encode()
    fields = " ".join(f"{n} 0 R" for n in field_refs)
    objs.append(
        (
            f"<< /Type /Catalog /Pages {pages_num} 0 R "
            f"/AcroForm << /Fields [{fields}] /DA (/Helv 0 Tf 0 g) "
            f"/DR << /Font << /Helv << /Type /Font /Subtype /Type1 "
            f"/BaseFont /Helvetica >> >> >> >> >>"
        ).encode()
    )
    root = len(objs)
    out = []
    for o in objs:
        o = o.replace(b"{PAGES}", str(pages_num).encode())
        # DESCENDING, because the tokens share a prefix. Ascending, `{PAGE_1}` is substituted
        # before `{PAGE_10}` is reached and corrupts it into `<n>0}` -- a widget whose `/P`
        # points at the wrong page, in a file that still opens. Harmless at pages=8 and a
        # silent wrong answer the moment anyone raises it to get a bigger form fixture.
        for p in range(len(page_objs) - 1, -1, -1):
            o = o.replace(f"{{PAGE_{p}}}".encode(), str(page_objs[p]).encode())
        out.append(o)
    return build(out, root)


# ---------------------------------------------------------------- driver


def main():
    if len(sys.argv) != 2:
        print(__doc__.splitlines()[2].strip(), file=sys.stderr)
        return 2
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)

    for tool in (CJPEG, QPDF):
        if not tool.exists():
            print(
                f"make-compress-fixtures: {tool} is missing. Run engines/fetch.sh and "
                f"engines/build-native.sh first.",
                file=sys.stderr,
            )
            return 1
    if not FONT.exists():
        print(f"make-compress-fixtures: {FONT} is missing.", file=sys.stderr)
        return 1

    built = {
        "scanned.pdf": scanned(),
        "photo-heavy.pdf": photo_heavy(),
        "text-report.pdf": text_report(),
        "text-report-no-font.pdf": text_report(embed_font=False),
        "form.pdf": form(),
    }
    for name, data in built.items():
        (out / name).write_bytes(data)

    # ALREADY-OPTIMISED, and produced by the thing under measurement rather than by a
    # different tool. Arm A's own settings, applied to the text report -- so the
    # already-optimised row answers "what does compress save on its own output", which is
    # the question a person re-running the tool actually asks.
    subprocess.run(
        [
            str(QPDF),
            "--object-streams=generate",
            "--compress-streams=y",
            "--decode-level=generalized",
            str(out / "text-report.pdf"),
            str(out / "already-optimised.pdf"),
        ],
        check=True,
    )

    # LINEARIZED, because dropping hint streams is one of the levers and no naive producer
    # emits one. Without this row `qpdf_set_linearization(false)` measures nothing.
    subprocess.run(
        [
            str(QPDF),
            "--linearize",
            str(out / "text-report.pdf"),
            str(out / "linearized.pdf"),
        ],
        check=True,
    )

    print(f"fixtures in {out}:")
    total = 0
    for f in sorted(out.glob("*.pdf")):
        n = f.stat().st_size
        total += n
        # Every fixture is opened by the pinned qpdf before it is reported. A fixture that
        # does not parse would otherwise reach the harness and be reported as a refusal by
        # the thing under measurement, which is the wrong place to find out.
        check = subprocess.run(
            [str(QPDF), "--check", str(f)], capture_output=True, text=True
        )
        status = "ok" if check.returncode in (0, 3) else "UNREADABLE"
        print(f"  {f.name:<24} {n:>10,} bytes  {status}")
        if status == "UNREADABLE":
            print(check.stdout[-2000:], file=sys.stderr)
            return 1
    print(f"  {'total':<24} {total:>10,} bytes")
    return 0


if __name__ == "__main__":
    sys.exit(main())
