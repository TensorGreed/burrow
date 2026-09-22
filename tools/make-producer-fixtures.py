#!/usr/bin/env python3
"""Build the redaction corpus's REAL-PRODUCER fixtures, and commit them.

    python3 tools/make-producer-fixtures.py [--out tests/redaction/fixtures]

WHY REAL PRODUCERS, AND WHY THIS IS NOT OPTIONAL

Every fixture in `tests/conformance/fixtures/` is synthesised, and spike 0006's 23 are too.
That is the right default -- a hand-built file measures the shape you meant to measure. It is
also how this repository learned, twice, that a synthetic corpus misses what real documents
contain:

  * #112: `split` walked from `/Font` into `/FontDescriptor` into `/FontFile3` and lexed a CFF
    program as page content. An ordinary 11 MB course PDF found it in minutes; twenty-two
    synthetic cases never touched it, because none of them had an embedded subset font.
  * Spike 0006's own first draft scored four channels "gone" while their `/ToUnicode` still
    spelled the secret out. The fixtures were right and the instruments were looking for text.

A word processor, a typesetter and an OCR program each emit structures nobody here would think
to write: LibreOffice's `/Differences` arrays and its `/StructTreeRoot`, pdfTeX's Type1 subsets
and TJ kerning, tesseract's invisible `Tr 3` text layer over a scanned image.

WHY THE OUTPUTS ARE COMMITTED

They cannot be regenerated in CI. Reproducing them needs LibreOffice, a TeX distribution and
tesseract, none of which CI has or should grow, and each of which changes its output between
versions. So the PDFs are committed and this script records the EXACT producer versions that
made them, in `PROVENANCE.md` beside them. Regenerating needs the toolchains; testing does not.

That is the opposite of the hand-built fixtures, which are deterministic and are generated on
demand by `tools/make-redaction-fixtures.py`. Two kinds of fixture, two lifecycles, and the
manifest says which each one is.

IT REFUSES RATHER THAN SKIPPING

A missing toolchain produces a clear error and a non-zero exit, never a quietly smaller corpus.
A generator that silently emits four fixtures where the manifest expects seven is the shape
`CLAUDE.md` names directly: a check that examines nothing reads as coverage.
"""

from __future__ import annotations

import argparse
import platform
import re
import shutil
import subprocess
import sys
import textwrap
from dataclasses import dataclass, field
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# The canary each producer carries. Uppercase, digits and hyphen only: every one of these
# survives a word processor's autocorrect and a typesetter's ligature table, and the alphabet
# is the one `blockfont.py` draws, so a hand-built fixture can carry the same string.
KEEP_LINE = "KEEP-THIS-LINE"


@dataclass
class Tool:
    name: str
    binary: str
    version_argv: list[str]
    version_re: str
    install_hint: str
    found: str | None = field(default=None)


TOOLS = {
    "libreoffice": Tool(
        "LibreOffice",
        "libreoffice",
        ["--version"],
        r"LibreOffice\s+(\S+)",
        "apt-get install libreoffice-writer",
    ),
    "pdflatex": Tool(
        "pdfTeX",
        "pdflatex",
        ["--version"],
        r"(pdfTeX\s+\S+.*)",
        "apt-get install texlive-latex-base texlive-fonts-recommended",
    ),
    "tesseract": Tool(
        "tesseract",
        "tesseract",
        ["--version"],
        r"tesseract\s+(\S+)",
        "apt-get install tesseract-ocr",
    ),
}


def resolve_tools() -> dict[str, Tool]:
    """Find every toolchain, or refuse and say which is missing and how to get it."""
    missing: list[Tool] = []
    for tool in TOOLS.values():
        path = shutil.which(tool.binary)
        if path is None:
            missing.append(tool)
            continue
        out = subprocess.run(
            [path, *tool.version_argv], capture_output=True, text=True, check=False
        )
        text = (out.stdout or "") + (out.stderr or "")
        match = re.search(tool.version_re, text)
        tool.found = match.group(1) if match else "unknown version"
    if missing:
        sys.stderr.write(
            "make-producer-fixtures: REFUSING to run -- these fixtures need real producers,\n"
            "and a smaller corpus that looks complete is worse than a clear failure.\n\n"
        )
        for tool in missing:
            sys.stderr.write(f"  missing: {tool.name} (`{tool.binary}`)\n")
            sys.stderr.write(f"           {tool.install_hint}\n")
        sys.stderr.write(
            "\nThe committed PDFs in the output directory were made by the versions recorded\n"
            "in PROVENANCE.md. CI does not run this script and does not need these tools;\n"
            "only regenerating the fixtures does.\n"
        )
        raise SystemExit(2)
    return TOOLS


def run(argv: list[str], **kw) -> subprocess.CompletedProcess:
    result = subprocess.run(argv, capture_output=True, text=True, check=False, **kw)
    if result.returncode != 0:
        sys.stderr.write(f"FAILED: {' '.join(argv[:4])} ...\n{result.stdout}\n{result.stderr}\n")
        raise SystemExit(1)
    return result


# ---------------------------------------------------------------------------
# 1. A word processor's export.
# ---------------------------------------------------------------------------


def writer_export(out: Path, work: Path) -> Path:
    """LibreOffice Writer, exported to PDF the way a person would.

    Carries the structures a word processor emits and nobody hand-writes: a tagged
    `/StructTreeRoot`, `/Differences`-encoded subsets of its bundled fonts, and document
    `/Info` and XMP the application fills in from its own metadata fields.

    The title is set DELIBERATELY to the secret. ADR 0029 discloses catalogue `/Metadata` and
    `/Info` rather than stripping them -- they are unreachable behind `qpdf_get_root` -- and
    this is the fixture that makes that disclosure concrete instead of hypothetical.
    """
    secret = "BURROW-SECRET-WRITER"
    fodt = work / "writer.fodt"
    # Flat ODF: one XML file, no zip, so the source of the fixture is readable in a diff.
    fodt.write_text(
        textwrap.dedent(f"""\
        <?xml version="1.0" encoding="UTF-8"?>
        <office:document
          xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
          xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
          xmlns:meta="urn:oasis:names:tc:opendocument:xmlns:meta:1.0"
          xmlns:dc="http://purl.org/dc/elements/1.1/"
          office:version="1.3" office:mimetype="application/vnd.oasis.opendocument.text">
         <office:meta>
          <dc:title>{secret}</dc:title>
          <meta:keyword>{secret}</meta:keyword>
         </office:meta>
         <office:body>
          <office:text>
           <text:h text:outline-level="1">Quarterly review</text:h>
           <text:p>The account identifier is {secret} and must not be shared.</text:p>
           <text:p>{KEEP_LINE}</text:p>
          </office:text>
         </office:body>
        </office:document>
        """),
        encoding="utf-8",
    )
    run(
        [
            "libreoffice",
            "--headless",
            "--convert-to",
            "pdf",
            "--outdir",
            str(work),
            str(fodt),
        ]
    )
    produced = work / "writer.pdf"
    target = out / "producer-writer.pdf"
    shutil.copyfile(produced, target)
    return target


def vertical_writing(out: Path, work: Path) -> Path:
    """LibreOffice Writer again, asked for a vertically written paragraph.

    # What this fixture is for, and the finding it carries

    #129 refuses a font whose CMap declares `WMode 1`, and the obvious worry is a vertical
    document evading it. This fixture is the measurement that says what a real producer
    actually emits, and the answer was not the expected one: asked for `tb-rl` Japanese,
    LibreOffice emits **no CID font, no `Identity-V` and no `WMode` at all** -- a subset
    *simple* font and one `Tm` per glyph, stepping `y` down the page. The ordinary horizontal
    walk places it correctly and there is nothing to refuse.

    So this is a fixture for the case that must **not** be refused, which is the half a
    refusal test usually lacks. `/Widths [0 -1000 -1000 ...]` -- negative advances -- is the
    detail nobody would hand-write.

    # Third-party content, which the other three do not have

    This is the only committed fixture in the repository that embeds a font from outside it:
    a six-glyph Type 1 subset of Noto Serif CJK SC, OFL-1.1. `PROVENANCE.md` beside it and
    `THIRD_PARTY_NOTICES.md` both carry the audit. A CJK face is unavoidable here -- vertical
    writing is a CJK feature and no bundled Latin font exercises it.
    """
    fodt = work / "vertical.fodt"
    # Flat ODF again, for the same reason: the source of the fixture is readable in a diff.
    # `style:writing-mode="tb-rl"` on BOTH the page layout and the paragraph -- Writer honours
    # the paragraph only if the page agrees, and a fixture that silently came out horizontal
    # would be a fixture measuring nothing.
    fodt.write_text(
        textwrap.dedent("""\
        <?xml version="1.0" encoding="UTF-8"?>
        <office:document xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
         xmlns:style="urn:oasis:names:tc:opendocument:xmlns:style:1.0"
         xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
         xmlns:fo="urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0"
         office:version="1.2" office:mimetype="application/vnd.oasis.opendocument.text">
         <office:automatic-styles>
          <style:page-layout style:name="pm1">
           <style:page-layout-properties fo:page-width="21cm" fo:page-height="29.7cm"
             style:writing-mode="tb-rl"/>
          </style:page-layout>
          <style:style style:name="P1" style:family="paragraph">
           <style:paragraph-properties style:writing-mode="tb-rl"/>
           <style:text-properties style:font-name-asian="Noto Sans CJK JP" style:font-size-asian="24pt"/>
          </style:style>
         </office:automatic-styles>
         <office:master-styles>
          <style:master-page style:name="Standard" style:page-layout-name="pm1"/>
         </office:master-styles>
         <office:body><office:text>
          <text:p text:style-name="P1">\u79d8\u5bc6\u6587\u66f8\u3067\u3059</text:p>
         </office:text></office:body>
        </office:document>
        """),
        encoding="utf-8",
    )
    run(
        [
            "libreoffice",
            "--headless",
            "--convert-to",
            "pdf",
            "--outdir",
            str(work),
            str(fodt),
        ]
    )
    target = out / "producer-vertical-writing.pdf"
    shutil.copyfile(work / "vertical.pdf", target)
    return target


# ---------------------------------------------------------------------------
# 2. A typesetter's output.
# ---------------------------------------------------------------------------


def latex_document(out: Path, work: Path) -> Path:
    """pdfTeX, with the Computer Modern Type1 subsets and TJ kerning it always emits.

    Structurally unlike anything else in the corpus: Type1 (`/FontFile`) subsets rather than
    TrueType, per-pair kerning that splits a word across many `TJ` elements, and ligature
    substitution that means the glyphs drawn are not the characters typed.

    The secret is set in `\\texttt`, so it is one font and no ligature rewrites it -- the point
    is the kerning and the subset, not a fight with `fi`. The surrounding prose uses the roman
    face and DOES get ligatures, so the fixture carries both.
    """
    secret = "BURROW-SECRET-LATEX"
    tex = work / "doc.tex"
    tex.write_text(
        textwrap.dedent(rf"""
        \documentclass[11pt]{{article}}
        \usepackage[T1]{{fontenc}}
        \pagestyle{{empty}}
        \begin{{document}}
        \section*{{Field office finding}}
        The certificate fingerprint was verified in the field and filed
        efficiently; the identifier is \texttt{{{secret}}} and is confidential.

        \bigskip
        {KEEP_LINE}
        \end{{document}}
        """).strip(),
        encoding="utf-8",
    )
    run(
        ["pdflatex", "-interaction=nonstopmode", "-halt-on-error", "doc.tex"],
        cwd=work,
    )
    target = out / "producer-latex.pdf"
    shutil.copyfile(work / "doc.pdf", target)
    return target


# ---------------------------------------------------------------------------
# 3. An OCR'd scan.
# ---------------------------------------------------------------------------


def render_page_to_pgm(pdf: Path, page: int, width: int, height: int, dest: Path) -> None:
    """Rasterise through PDFium, via the SHIPPED render path.

    `pdftoppm` is on the usual dev machine and is deliberately not used: poppler is GPL, and
    `CLAUDE.md`'s second non-negotiable is permissive-only. Nothing here links poppler, so the
    licence rule is not literally engaged -- but a fixture pipeline is exactly where a tool
    becomes load-bearing without anyone deciding it should, and the next person reaches for
    whatever is already in the build. PDFium is vendored, permissive, and is the renderer the
    site itself uses.
    """
    # Through `cargo run`, not the built binary directly. The example links `libpdfium.so`
    # dynamically and build.rs bakes an rpath for the crate's own test binaries; an example
    # invoked by path outside cargo does not resolve it and dies with
    # "cannot open shared object file". Measured, not guessed.
    if shutil.which("cargo") is None:
        sys.stderr.write(
            "make-producer-fixtures: REFUSING -- `cargo` is not on PATH, and the OCR fixture\n"
            "needs it to rasterise a page through PDFium.\n"
        )
        raise SystemExit(2)
    run(
        [
            "cargo", "run", "-q", "-p", "burrow-ops",
            "--features", "native-engines", "--release",
            "--example", "render-page", "--",
            str(pdf), str(page), str(width), str(height), str(dest),
        ],
        cwd=REPO,
    )
    if not dest.is_file():
        sys.stderr.write(
            "make-producer-fixtures: the rasteriser reported success and wrote nothing.\n"
            "The vendored engines may be missing: engines/fetch.sh && engines/build-native.sh\n"
        )
        raise SystemExit(1)


def ocr_scan(out: Path, work: Path) -> Path:
    """A page that is an IMAGE, with an invisible text layer over it -- what a scanner and an
    OCR program produce together.

    Built the way the real pipeline runs: typeset a page, rasterise it (so the PDF's text is
    gone and only pixels remain), degrade it the way a scanner does, then let tesseract read it
    back and emit a searchable PDF. The text layer is therefore genuine OCR output, with
    whatever tesseract actually recognised -- not the string we started from.

    THIS FIXTURE IS EXPECTED TO BE REFUSED. See `PROVENANCE.md` and the manifest entry: ADR
    0029 §3 refuses a region intersecting an image, and every region on this page intersects
    one. That is a deliberate consequence and is recorded as a decision rather than left to be
    discovered by the first person who scans a document.
    """
    secret = "BURROW-SECRET-SCAN"
    tex = work / "scan-source.tex"
    # Large and plain: OCR accuracy is not the subject, and a fixture whose canary tesseract
    # misreads cannot witness its own presence.
    tex.write_text(
        textwrap.dedent(rf"""
        \documentclass[12pt]{{article}}
        \usepackage[T1]{{fontenc}}
        \usepackage[margin=1in]{{geometry}}
        \pagestyle{{empty}}
        \begin{{document}}
        \Large
        Case file summary

        \bigskip
        Reference {secret}

        \bigskip
        {KEEP_LINE}
        \end{{document}}
        """).strip(),
        encoding="utf-8",
    )
    run(["pdflatex", "-interaction=nonstopmode", "-halt-on-error", "scan-source.tex"], cwd=work)

    pgm = work / "page.pgm"
    # 200 dpi on US Letter. A real desk scanner's default, and enough for tesseract.
    render_page_to_pgm(work / "scan-source.pdf", 1, 1700, 2200, pgm)

    from PIL import Image, ImageFilter  # noqa: PLC0415 -- optional dep, only this path needs it

    image = Image.open(pgm).convert("L")
    # Scanner artefacts, so the fixture is a scan rather than a clean render: a slight skew, a
    # little blur, and paper grey. Enough to be realistic, not so much that OCR fails -- a
    # fixture whose canary cannot be read back cannot witness its own presence (ADR 0029 §8).
    image = image.rotate(0.4, resample=Image.BICUBIC, fillcolor=255, expand=False)
    image = image.filter(ImageFilter.GaussianBlur(0.6))
    image = image.point(lambda v: min(255, int(v * 0.94) + 12))
    scan_png = work / "scan.png"
    image.save(scan_png)

    stem = work / "ocr"
    run(["tesseract", str(scan_png), str(stem), "pdf"])
    target = out / "producer-ocr-scan.pdf"
    shutil.copyfile(stem.with_suffix(".pdf"), target)
    return target


# ---------------------------------------------------------------------------

BUILDERS = {
    "producer-writer.pdf": ("LibreOffice Writer export", writer_export),
    "producer-latex.pdf": ("pdfTeX / LaTeX", latex_document),
    "producer-ocr-scan.pdf": ("scanner + tesseract OCR", ocr_scan),
    "producer-vertical-writing.pdf": (
        "LibreOffice Writer, vertical tb-rl text",
        vertical_writing,
    ),
}


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--out",
        default=str(REPO / "tests" / "redaction" / "fixtures"),
        help="where the committed PDFs go",
    )
    args = parser.parse_args(argv[1:])
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)

    tools = resolve_tools()
    work = out.parent / ".work"
    if work.exists():
        shutil.rmtree(work)
    work.mkdir(parents=True)

    made: list[tuple[str, int]] = []
    for name, (_label, builder) in BUILDERS.items():
        path = builder(out, work)
        assert path.name == name, f"{builder.__name__} wrote {path.name}, expected {name}"
        made.append((name, path.stat().st_size))

    provenance = out.parent / "PROVENANCE.md"
    lines = [
        "# Producer fixtures — provenance",
        "",
        "Generated by `tools/make-producer-fixtures.py` and **committed**, because they cannot",
        "be regenerated in CI: reproducing them needs LibreOffice, a TeX distribution and",
        "tesseract, none of which CI has, and each of which changes its output between",
        "versions. Testing needs only the committed PDFs; regenerating needs the toolchains.",
        "",
        "The hand-built fixtures are the other way round — deterministic, generated on demand",
        "by `tools/make-redaction-fixtures.py`, and not committed.",
        "",
        "## The versions that produced the committed files",
        "",
        "| producer | version |",
        "|---|---|",
    ]
    for tool in tools.values():
        lines.append(f"| {tool.name} | `{tool.found}` |")
    lines += [
        f"| host | `{platform.system()} {platform.machine()}` |",
        "",
        "## Files",
        "",
        "| file | what it is |",
        "|---|---|",
    ]
    for name, size in made:
        lines.append(f"| `fixtures/{name}` | {BUILDERS[name][0]}, {size:,} bytes |")
    lines += [
        "",
        "Every canary in these files is a string this repository invented. No third-party",
        "document, image or font is embedded: the text is ours, the page images are rendered",
        "from our own source, and the fonts are the producers' own bundled faces.",
        "",
    ]
    provenance.write_text("\n".join(lines), encoding="utf-8")

    shutil.rmtree(work)
    for name, size in made:
        print(f"  {name:28s} {size:>9,} bytes")
    print(f"wrote {len(made)} producer fixture(s) to {out}")
    print(f"provenance: {provenance.relative_to(REPO)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
