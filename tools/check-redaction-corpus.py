#!/usr/bin/env python3
"""Every canary the redaction manifest claims is present must actually be present.

    python3 tools/check-redaction-corpus.py

ADR 0029 §8's rule has two halves:

    Where a check reports that something is GONE, it must also show that something CHANGED.

The "after" half needs the operation, which does not exist yet (#131, #132, #133). The "before"
half is checkable today, and it is the half a corpus can get wrong on its own: a manifest that
says a canary is in a file, where it is not, produces a fixture that will later report the
secret "removed" having never contained it. That is the shape spike 0006 measured — four
channels scored gone by instruments that were never able to see them.

So this refuses when a `witness_before` does not hold, and it reports what it examined rather
than printing OK: "4 of 11" reads exactly like success.

WITNESS KINDS, and each is a different question

  pdfium-text            PDFium's text page contains the canary. Decodes THROUGH the font, so it
                         sees a subset encoding a literal scan cannot.
  raw-utf16-hex          The canary is in the emitted bytes as UTF-16BE, hex or literal — how a
                         producer writes /Info and /Keywords.
  font-mapping           The font's own /ToUnicode or /Differences describes the canary's
                         characters. Channel 23.
  image-covers-page      The page draws an image large enough that every region intersects it.
  structure-tree-present The document carries a /StructTreeRoot.

A witness that cannot be evaluated is an ERROR, never a silent pass.
"""

from __future__ import annotations

import re
import shutil
import subprocess
import sys
import tomllib
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
MANIFEST = REPO / "tests" / "redaction" / "manifest.toml"


def qpdf_cli() -> Path:
    import platform

    vendored = (
        REPO / "engines" / "vendor" / "src"
        / f"build-qpdf-plain-{platform.machine()}" / "qpdf" / "qpdf"
    )
    if vendored.is_file():
        return vendored
    found = shutil.which("qpdf")
    if found:
        return Path(found)
    sys.exit("check-redaction-corpus: no qpdf. Run engines/fetch.sh && engines/build-native.sh")


def expanded(qpdf: Path, pdf: Path, scratch: Path) -> bytes:
    """`qpdf --qdf`, with NO FALLBACK.

    Returning the raw bytes when the CLI is unavailable would make every witness that depends on
    decompression report absent, which reads as a corpus problem rather than a tooling one.
    `split_no_leak.rs` records the same rule for the same reason.
    """
    out = scratch / f"{pdf.stem}.qdf.pdf"
    result = subprocess.run(
        [str(qpdf), "--qdf", "--object-streams=disable", str(pdf), str(out)],
        capture_output=True,
        text=True,
    )
    if result.returncode not in (0, 3):
        sys.exit(f"check-redaction-corpus: qpdf --qdf failed on {pdf.name}: {result.stderr[:200]}")
    return out.read_bytes()


def spellings(needle: str) -> list[bytes]:
    ascii_ = needle.encode()
    utf16 = needle.encode("utf-16-be")
    bom = b"\xfe\xff" + utf16
    out = [ascii_, utf16, bom]
    for raw in (utf16, bom):
        out.append(raw.hex().upper().encode())
        out.append(raw.hex().lower().encode())
    return out


def witness_raw(data: bytes, canary: str) -> bool:
    return any(s in data for s in spellings(canary))


def witness_font_mapping(data: bytes, canary: str) -> bool:
    """The font's own description of the removed run — /ToUnicode targets or /Differences names.

    Same three constructs spike 0006's `fontscan.rs` reads, in the same order of preference.
    Deliberately does NOT parse the embedded font's cmap: that is a floor, and a floor is what
    the manifest should be measured against rather than a ceiling nobody implemented.
    """
    text = data.decode("latin-1")
    targets = ""
    for block in re.findall(r"beginbfchar(.*?)endbfchar", text, re.S):
        for line in block.splitlines():
            groups = re.findall(r"<([0-9A-Fa-f]+)>", line)
            if len(groups) >= 2:
                try:
                    targets += bytes.fromhex(groups[1]).decode("utf-16-be", errors="replace")
                except ValueError:
                    pass
    for block in re.findall(r"beginbfrange(.*?)endbfrange", text, re.S):
        for lo, _hi, dst in re.findall(r"<([0-9A-Fa-f]+)>\s*<([0-9A-Fa-f]+)>\s*<([0-9A-Fa-f]+)>", block):
            try:
                targets += bytes.fromhex(dst).decode("utf-16-be", errors="replace")
            except ValueError:
                pass
    names = ""
    for block in re.findall(r"/Differences\s*\[(.*?)\]", text, re.S):
        for token in block.split():
            if token.startswith("/"):
                n = token[1:]
                if len(n) == 1 and n.isalpha():
                    names += n
                elif re.fullmatch(r"a\d+", n):
                    # SYNTHETIC GLYPH NAMES, which pdfTeX emits for every Type1 subset: `/a66`
                    # rather than `/B`. The number IS the character code, so the array still
                    # names the alphabet of the run it was built for -- just not in a form the
                    # Adobe Glyph List covers.
                    #
                    # Found by this checker refusing a placement the manifest asserted, on a
                    # real LaTeX document. No hand-built fixture in this repository uses this
                    # encoding, which is the argument for having real producer output at all.
                    code = int(n[1:])
                    if 0 <= code < 0x110000:
                        names += chr(code)
                else:
                    names += {
                        "hyphen": "-", "space": " ", "zero": "0", "one": "1", "two": "2",
                        "three": "3", "four": "4", "five": "5", "six": "6", "seven": "7",
                        "eight": "8", "nine": "9",
                    }.get(n, "")
    distinct = {c for c in canary if c != " "}
    return (
        canary in targets
        or canary in names
        or (bool(distinct) and distinct <= set(targets))
        or (bool(distinct) and distinct <= set(names))
    )


def witness_image_covers_page(data: bytes, _canary: str) -> bool:
    """A page that draws an image big enough that every region intersects it."""
    if b"/Subtype /Image" not in data and b"/Subtype/Image" not in data:
        return False
    # A `cm` whose scale is at least half the page in both axes, immediately before a `Do`.
    for m in re.finditer(rb"([\d.]+)\s+0\s+0\s+([\d.]+)\s+[-\d.]+\s+[-\d.]+\s+cm\s*/\w+\s+Do", data):
        if float(m.group(1)) >= 300 and float(m.group(2)) >= 300:
            return True
    return False


def witness_structure_tree(data: bytes, _canary: str) -> bool:
    return b"/StructTreeRoot" in data


def witness_pdfium_text(pdf: Path, canary: str, cache: dict[Path, str]) -> bool:
    """PDFium's text page, through the committed spike harness's renderer path.

    Uses `cargo run --example dump-page-text`, added alongside `render-page`, because reading
    text is the one witness a byte scan cannot stand in for: a subset font's content-stream
    bytes are not the text, and that is precisely the case this corpus exists to carry.
    """
    if pdf in cache:
        return canary in cache[pdf]
    cargo = shutil.which("cargo")
    if cargo is None:
        sys.exit("check-redaction-corpus: `cargo` is required for the pdfium-text witness")
    result = subprocess.run(
        [
            cargo, "run", "-q", "-p", "burrow-engines",
            "--features", "native-engines", "--release",
            "--example", "dump-page-text", "--", str(pdf),
        ],
        capture_output=True, text=True, cwd=REPO,
    )
    if result.returncode != 0:
        sys.exit(
            "check-redaction-corpus: dump-page-text failed on "
            f"{pdf.name}:\n{result.stderr[:400]}"
        )
    cache[pdf] = result.stdout
    return canary in result.stdout


BYTE_WITNESSES = {
    "raw-utf16-hex": witness_raw,
    "font-mapping": witness_font_mapping,
    "image-covers-page": witness_image_covers_page,
    "structure-tree-present": witness_structure_tree,
}


def main() -> int:
    if not MANIFEST.is_file():
        sys.exit(f"check-redaction-corpus: {MANIFEST} is missing")
    manifest = tomllib.loads(MANIFEST.read_text())
    qpdf = qpdf_cli()
    scratch = REPO / "target" / "redaction-corpus-check"
    scratch.mkdir(parents=True, exist_ok=True)
    text_cache: dict[Path, str] = {}

    fixtures = manifest.get("fixture", [])
    if not fixtures:
        sys.exit("check-redaction-corpus: the manifest declares no fixtures; this run is vacuous")

    checked = 0
    failures: list[str] = []
    verdicts: dict[str, int] = {}
    missing_files: list[str] = []

    for fixture in fixtures:
        path = MANIFEST.parent / fixture["file"]
        if not path.is_file():
            if fixture.get("kind") == "hand-built":
                missing_files.append(
                    f"{fixture['name']}: not generated — run tools/make-redaction-fixtures.py"
                )
            else:
                failures.append(f"{fixture['name']}: committed fixture {path} is missing")
            continue
        data = expanded(qpdf, path, scratch)
        for placement in fixture.get("placement", []):
            checked += 1
            verdicts[placement["verdict"]] = verdicts.get(placement["verdict"], 0) + 1
            kind = placement["witness_before"]
            canary = placement["canary"]
            if kind == "pdfium-text":
                ok = witness_pdfium_text(path, canary, text_cache)
            elif kind in BYTE_WITNESSES:
                ok = BYTE_WITNESSES[kind](data, canary)
            else:
                failures.append(
                    f"{fixture['name']} / {placement['channel']}: unknown witness `{kind}`"
                )
                continue
            if not ok:
                failures.append(
                    f"{fixture['name']} / {placement['channel']}: "
                    f"witness `{kind}` does not find `{canary}`. "
                    "A placement whose canary cannot be witnessed measures nothing."
                )

    print(
        f"check-redaction-corpus: {len(fixtures)} fixture(s), {checked} placement(s) — "
        + ", ".join(f"{n} {v}" for v, n in sorted(verdicts.items()))
    )
    for note in missing_files:
        print(f"  not generated: {note}")
    if failures:
        sys.stderr.write("\nFAILED — %d problem(s):\n" % len(failures))
        for f in failures:
            sys.stderr.write(f"  - {f}\n")
        return 1
    print("OK — every canary the manifest claims is present was witnessed present.")
    print(
        "     The `expect_after` half of ADR 0029 §8's rule is NOT checked here: it needs the "
        "operation (#131, #132, #133)."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
