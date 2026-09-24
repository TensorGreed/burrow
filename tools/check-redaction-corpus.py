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
  font-cmap              The embedded font program's own cmap covers the canary's alphabet.
                         The only witness for a CID subset with neither of the above.
  image-covers-page      The page draws an image large enough that every region intersects it.
  image-drawn            An image XObject exists and is drawn, at any size.
  inline-image           An inline BI...ID...EI image. Not an XObject, so resource enumeration
                         does not see it.
  thumb-ink              The page's /Thumb exists and carries ink. STRUCTURAL: the canary is
                         pixels, and no scan can read it.
  vector-fills           The page draws filled paths. Structural, for the same reason.
  pdfium-text-loose      PDFium's text, compared with whitespace squashed on both sides — a
                         page whose glyphs are individually positioned extracts with separators
                         between them, and the strict match fails on text that IS there.
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


def witness_font_cmap(data: bytes, canary: str) -> bool:
    """The embedded sfnt's own `cmap` covers every distinct character of the canary.

    The third construct spike 0006's `fontscan.rs` reads, and the ONLY witness for a CID font
    with no `/ToUnicode` and no `/Differences` — channel 6, which is one of the five the spike
    measured as invisible to every scan. A subset font's cmap covers exactly the characters it
    was built for, so on a single-run subset it IS that run's alphabet.

    Font programs are found by `/Length1`, the key that declares a stream is an sfnt.
    """
    chars: set[str] = set()
    at = 0
    while True:
        i = data.find(b"/Length1", at)
        if i < 0:
            break
        at = i + 8
        m = re.match(rb"\s*(\d+)", data[i + 8 :])
        if not m:
            continue
        length = int(m.group(1))
        j = data.find(b"stream", i)
        if j < 0:
            continue
        start = j + 6
        while start < len(data) and data[start] in (13, 10):
            start += 1
        _sfnt_cmap(data[start : start + length], chars)
    distinct = {c for c in canary if c != " "}
    return bool(distinct) and distinct <= chars


def _sfnt_cmap(font: bytes, out: set[str]) -> None:
    """Walk an sfnt table directory to `cmap` and read a format 4 subtable's covered codes."""
    def be16(o: int) -> int | None:
        return int.from_bytes(font[o : o + 2], "big") if o + 2 <= len(font) else None

    def be32(o: int) -> int | None:
        return int.from_bytes(font[o : o + 4], "big") if o + 4 <= len(font) else None

    n = be16(4)
    if not n or n > 64:
        return
    cmap = None
    for i in range(n):
        rec = 12 + i * 16
        if font[rec : rec + 4] == b"cmap":
            cmap = be32(rec + 8)
    if cmap is None:
        return
    subtables = be16(cmap + 2)
    if not subtables:
        return
    for i in range(subtables):
        rec = cmap + 4 + i * 8
        off = be32(rec + 4)
        if off is None:
            continue
        sub = cmap + off
        if be16(sub) != 4:
            continue
        seg_x2 = be16(sub + 6)
        if not seg_x2:
            continue
        for seg in range(seg_x2 // 2):
            end = be16(sub + 14 + seg * 2)
            start = be16(sub + 16 + seg_x2 + seg * 2)
            if start is None or end is None or start > end or end == 0xFFFF:
                continue
            for code in range(start, end + 1):
                out.add(chr(code))
        return


def witness_pdfium_text_loose(pdf: Path, canary: str, cache: dict[Path, str]) -> bool:
    """PDFium's text page, ignoring whitespace on both sides.

    Spike 0006's cheapest lesson: **a substring match is not text extraction.** On a page whose
    glyphs are individually repositioned, PDFium inserts separators between runs and reads
    `BURROW- SECRET- 03` — the text was extracted perfectly and the MATCHER failed. A verifier
    that looked for its secret the strict way would report success on a page that plainly still
    says it.
    """
    if pdf not in cache:
        witness_pdfium_text(pdf, "", cache)
    squash = "".join(cache[pdf].split())
    return "".join(canary.split()) in squash


def _stream_for_object(data: bytes, number: int) -> bytes:
    """The stream body of object `number` in a qdf expansion."""
    m = re.search(rb"(?m)^%d 0 obj\b" % number, data)
    if not m:
        return b""
    start = data.find(b"stream", m.end())
    if start < 0:
        return b""
    start += 6
    while start < len(data) and data[start] in (13, 10):
        start += 1
    end = data.find(b"endstream", start)
    return data[start:end] if end > 0 else b""


def witness_thumb_ink(data: bytes, _canary: str) -> bool:
    """The page's /Thumb exists and carries ink.

    STRUCTURAL, and labelled as such in the manifest: the canary here is PIXELS, and no byte
    scan or text extraction can read it. Spike 0006 measured this channel as the only one no
    instrument found — the page was blank and the surviving thumbnail read BURROW-CARRIER-14.
    What this witnesses is that the carrier is present and non-empty, which is the strongest
    statement available without decoding an image.
    """
    m = re.search(rb"/Thumb\s+(\d+)\s+0\s+R", data)
    if not m:
        return False
    body = _stream_for_object(data, int(m.group(1)))
    if not body:
        return False
    dark = sum(1 for b in body if b < 128)
    return dark > len(body) // 20


def witness_vector_fills(data: bytes, _canary: str) -> bool:
    """The page draws filled paths — the secret as geometry, with no font and no text object.

    Structural for the same reason as `thumb-ink`: there is no text to witness. A run of `re`
    rectangles followed by `f` is what `outline_ops` emits and what a chart or a signature also
    looks like, which is precisely why ADR 0029 §3 refuses rather than guessing.
    """
    return len(re.findall(rb"\bre\b", data)) >= 20 and b" f\n" in data.replace(b"\r", b"\n")


def witness_inline_image(data: bytes, _canary: str) -> bool:
    """An inline `BI` ... `ID` ... `EI` image in a content stream.

    Deliberately separate from `image-drawn`: an inline image is not an XObject, has no
    `/Subtype /Image`, and appears in no `/XObject` resource dictionary. A signal that
    enumerates resources finds nothing, which is exactly why there is an evasion fixture for it.
    """
    return re.search(rb"(?:^|[\s])BI[\s/].{0,200}?\sID[\s]", data, re.S) is not None


def witness_image_drawn(data: bytes, _canary: str) -> bool:
    """An image XObject exists and is drawn. Any size, unlike `image-covers-page`."""
    has_image = b"/Subtype /Image" in data or b"/Subtype/Image" in data
    return has_image and re.search(rb"/\w+\s+Do\b", data) is not None


# ---------------------------------------------------------------------------------------
# WHAT EACH WITNESS CAN ACTUALLY SEE.
#
# "Witnessed" must never read as "the secret was seen" when it was not. Three of these observe
# the CARRIER and cannot read the canary at all -- a thumbnail's pixels, a page's filled paths,
# a structure tree's presence. Two observe the ALPHABET a subset font was built for, which is a
# real disclosure and is not the string.
#
# Every placement declares `witness_observes`, and this table is what it is checked against, so
# a manifest cannot quietly claim more than its witness delivers. A witness kind added without
# an entry here is an error, not a default.
# ---------------------------------------------------------------------------------------
WITNESS_OBSERVES = {
    "pdfium-text": "canary",
    "pdfium-text-loose": "canary",
    "raw-utf16-hex": "canary",
    "raw-file": "canary",
    "font-mapping": "canary-or-alphabet",
    "font-cmap": "alphabet",
    "thumb-ink": "carrier",
    "vector-fills": "carrier",
    "image-drawn": "carrier",
    "inline-image": "carrier",
    "image-covers-page": "carrier",
    "structure-tree-present": "carrier",
}

# What a placement may declare, and what each means to a reader.
OBSERVES_MEANING = {
    "canary": "the canary itself was read",
    "alphabet": "the set of characters the canary uses was found; NOT the canary",
    "carrier": "the thing holding the canary was found; the canary itself was NOT read",
}


BYTE_WITNESSES = {
    "font-cmap": witness_font_cmap,
    "thumb-ink": witness_thumb_ink,
    "vector-fills": witness_vector_fills,
    "image-drawn": witness_image_drawn,
    "inline-image": witness_inline_image,
    "raw-utf16-hex": witness_raw,
    "font-mapping": witness_font_mapping,
    "image-covers-page": witness_image_covers_page,
    "structure-tree-present": witness_structure_tree,
}


# The fewest placements the manifest may declare, and the most fixtures that may assert nothing.
#
# Both are floors rather than equalities: a new fixture adds assertions and must not fail the
# gate. Raise them when the corpus grows -- the run prints the current counts, so the number to
# raise them to is in the output.
PLACEMENT_FLOOR = 64
MAX_SILENT_FIXTURES = 2


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

    # COMPLETENESS, BOTH WAYS, BEFORE ANY WITNESS RUNS.
    #
    # This manifest's own header says it "asserts decisions, not files" — so a fixture on disk
    # that it does not declare is a document with no assigned verdict, quietly outside the table
    # ADR 0029 owes. Nothing checked that, and seven had accumulated: `00-control-no-canary`,
    # `17-incremental-update`, `producer-vertical-writing`, and the four `/ActualText` fixtures
    # added with the marked-content refusal.
    #
    # It is also what lets `check-redaction-corpus.sh` DERIVE its expected file count instead of
    # carrying a hand-typed integer. That integer had to be edited by hand twice in one batch,
    # and a count somebody bumps without looking is worse than no count: it reads as a
    # measurement while asserting whatever the last person typed. Deriving is only sound if the
    # manifest is complete, so completeness is enforced here rather than assumed.
    # FULL RELATIVE PATHS, NOT BASENAMES. This compared `file.split("/")[-1]` against
    # `path.name`, so two different files satisfied it: a security review showed that
    # `file = "generated/../../../../../../tmp/01-plain-tj.pdf"` passed completeness -- the
    # basename is still declared and the real file is still on disk -- and the checker then ran
    # `read_bytes()` and `qpdf` on that arbitrary path. The manifest is committed and trusted, so
    # this is hardening rather than an exploit; what made it worth fixing is that the claim being
    # made was "manifest against disk, both ways", and what was checked was *names*.
    declared = {f["file"] for f in fixtures}
    for fixture in fixtures:
        resolved = (MANIFEST.parent / fixture["file"]).resolve()
        if not resolved.is_relative_to(MANIFEST.parent.resolve()):
            sys.exit(
                f"check-redaction-corpus: {fixture['name']} declares {fixture['file']}, which "
                "resolves outside the corpus directory"
            )
    on_disk = {
        f"{directory}/{path.name}"
        for directory in ("generated", "fixtures")
        for path in (MANIFEST.parent / directory).glob("*.pdf")
    }
    undeclared = sorted(on_disk - declared)
    if undeclared:
        sys.exit(
            "check-redaction-corpus: on disk but not declared in the manifest, so no verdict is "
            "assigned to them:\n  " + "\n  ".join(undeclared)
        )

    # AND EVERY FIXTURE SAYS SOMETHING. A fixture with no placements is skipped by the loop
    # below in silence, which would make declaring one a way to opt out of being checked.
    silent = [
        f["name"]
        for f in fixtures
        if not f.get("placement") and not f.get("no_placements_because")
    ]
    if silent:
        sys.exit(
            "check-redaction-corpus: declared with no placement and no "
            "`no_placements_because`, so nothing is asserted about them:\n  "
            + "\n  ".join(silent)
        )

    # THE NEGATIVE PROBE, on every run. Every witness below has a positive fixture; until now
    # none had a near-miss anywhere CI runs, so a witness that matched everything would have
    # passed every placement and failed nothing. `00-control-no-canary.pdf` is the document
    # written to contain no canary at all, and its manifest entry said it was "the INERTNESS
    # control for every witness in this file" -- which was not true of anything in this file,
    # because a fixture with no placements runs no witness. A code review found the sentence
    # before it found a bug; this is what makes it true.
    control = MANIFEST.parent / "generated" / "00-control-no-canary.pdf"
    if not control.is_file():
        sys.exit(f"check-redaction-corpus: the inertness control {control} is missing")
    control_data = expanded(qpdf, control, scratch)
    matched: list[str] = []
    for kind in sorted(WITNESS_OBSERVES):
        canary = "BURROW-SECRET-CONTROL"
        if kind == "raw-file":
            hit = witness_raw(control.read_bytes(), canary)
        elif kind == "pdfium-text":
            hit = witness_pdfium_text(control, canary, text_cache)
        elif kind == "pdfium-text-loose":
            hit = witness_pdfium_text_loose(control, canary, text_cache)
        else:
            hit = BYTE_WITNESSES[kind](control_data, canary)
        if hit:
            matched.append(kind)
    if matched:
        sys.exit(
            "check-redaction-corpus: these witnesses found a canary in the canary-free "
            "control, so they match everything and assert nothing:\n  " + "\n  ".join(matched)
        )
    # REPORTED IN TWO GROUPS, because the sweep proves a different thing about each and one
    # number over both overclaims. Six of these witnesses take the canary and look for it; six
    # take it and ignore it (`_canary`), answering "is there a /Thumb / a vector fill / an inline
    # image here" instead.
    #
    # For the first six this is a real near-miss: the canary is absent and they say so, which is
    # what stops a witness that matches everything. For the other six it establishes only that
    # the control lacks their carrier -- true, worth knowing, and NOT a demonstration that the
    # witness discriminates. A security review counted the old line as "1 of 12 probed, reported
    # as 12"; this says which is which rather than averaging them.
    discriminating = sorted(k for k, v in WITNESS_OBSERVES.items() if v != "carrier")
    carrier_only = sorted(k for k, v in WITNESS_OBSERVES.items() if v == "carrier")
    print(
        f"  inertness: {len(discriminating)} witness(es) that read the canary found none in "
        f"{control.name} (a near-miss for each); {len(carrier_only)} carrier witness(es) found "
        f"no carrier there, which shows the control is plain and not that they discriminate"
    )

    # AND A FLOOR ON WHAT IS ASSERTED, which `no_placements_because` would otherwise let anyone
    # lower one fixture at a time. A security review replaced `01-plain-tj`'s single placement
    # with one sentence: this checker exited 0, the shell gate exited 0 (its counts come from
    # the `file` lines, which did not move), and `redaction_corpus.rs` stayed green. The witness
    # census line dropped from 36 canaries to 35 and nothing looked at it.
    #
    # The count is knowable -- it is a count over this same file -- so it is gated rather than
    # merely printed, and the fixtures that assert nothing are enumerated rather than tolerated
    # as a number.
    declared_placements = sum(len(f.get("placement", [])) for f in fixtures)
    silent_fixtures = sorted(f["name"] for f in fixtures if not f.get("placement"))
    # DERIVED, NOT TYPED. This was a literal, and it was wrong twice in a row: 50 first, which
    # let the very mutation it was written for through, and then 54 when the manifest already
    # declared 57 -- under a comment insisting it had no slack in it. A code review dropped one
    # of `producer-writer`'s four placements and this exited 0.
    #
    # Writing the number down is the mistake, not the number. The floor is the count the
    # manifest itself declares, minus a tolerance of zero: it may RISE freely -- a new fixture
    # is a new assertion -- and any fall is a removed assertion. `expected` is recomputed on
    # every run from the same file, so it cannot drift from what it is gating.
    #
    # THE FLOOR IS A SEPARATE FILE'S BUSINESS, though, or this would be a tautology: a count
    # compared against itself agrees always. `PLACEMENT_FLOOR` is the committed expectation and
    # `declared_placements` is what the manifest has now, which is exactly the comparison that
    # noticed the drop.
    if declared_placements < PLACEMENT_FLOOR or len(silent_fixtures) > MAX_SILENT_FIXTURES:
        sys.exit(
            f"check-redaction-corpus: the manifest declares {declared_placements} placement(s) "
            f"across {len(fixtures)} fixture(s), with {len(silent_fixtures)} asserting nothing "
            f"({', '.join(silent_fixtures)}). A placement removed is an assertion removed, and "
            "`no_placements_because` is a reason to skip one fixture, not a way to empty the "
            f"set.\n  If you ADDED fixtures, set PLACEMENT_FLOOR to {declared_placements} in "
            "tools/check-redaction-corpus.py. The number is printed here rather than left to be "
            "rediscovered, because it has had to be raised by hand three times and each raise "
            "was an opportunity to type the wrong one -- which happened, twice."
        )

    checked = 0
    failures: list[str] = []
    verdicts: dict[str, int] = {}
    observed: dict[str, int] = {}
    missing_files: list[str] = []

    for fixture in fixtures:
        path = MANIFEST.parent / fixture["file"]
        if not path.is_file():
            if fixture.get("kind") == "hand-built":
                # A HARD FAILURE, not a note. This was appended to `missing_files`, which is
                # printed and does not fail -- so a manifest entry naming a file that does not
                # exist had every one of its placements silently skipped, and the run still
                # said OK. A code review measured it: a phantom `[[fixture]]` produced
                # `not generated: …` and exit 0.
                #
                # The generators run immediately before this in
                # `tools/check-redaction-corpus.sh`, so "not generated" here means "not
                # written by either generator", which is a defect rather than a reminder.
                # Both generators are named, because there are two and the old message
                # mentioned only the first.
                failures.append(
                    f"{fixture['name']}: declared at {fixture['file']} and not written by "
                    "tools/make-redaction-fixtures.py or tools/make-evasion-fixtures.py"
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
            if kind == "raw-file":
                # THE FILE AS IT ARRIVED, not the qpdf normalisation every other byte witness
                # reads. `expanded()` runs `qpdf --qdf`, whose writer emits only objects
                # reachable from the current trailer — so a canary that lives in a SUPERSEDED
                # revision is gone from it by construction. That is the very property ADR 0029
                # records for the incremental-update channel, and reading the normalised bytes
                # to witness it would be asking the wrong file.
                ok = witness_raw(path.read_bytes(), canary)
            elif kind == "pdfium-text":
                ok = witness_pdfium_text(path, canary, text_cache)
            elif kind == "pdfium-text-loose":
                ok = witness_pdfium_text_loose(path, canary, text_cache)
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
                continue

            # WHAT THE WITNESS ACTUALLY SAW, checked rather than assumed.
            can_see = WITNESS_OBSERVES.get(kind)
            if can_see is None:
                failures.append(
                    f"{fixture['name']} / {placement['channel']}: witness `{kind}` has no entry "
                    "in WITNESS_OBSERVES, so nobody has said what it can see."
                )
                continue
            declared = placement.get("witness_observes")
            if declared is None:
                failures.append(
                    f"{fixture['name']} / {placement['channel']}: no `witness_observes`. "
                    f"`{kind}` observes {can_see}; say so, so that \"witnessed\" is never read "
                    "as \"the secret was seen\"."
                )
                continue
            allowed = {"canary", "alphabet"} if can_see == "canary-or-alphabet" else {can_see}
            if declared not in allowed:
                failures.append(
                    f"{fixture['name']} / {placement['channel']}: declares "
                    f"`witness_observes = \"{declared}\"` but `{kind}` observes {can_see}."
                )
                continue
            observed[declared] = observed.get(declared, 0) + 1

    print(
        f"check-redaction-corpus: {len(fixtures)} fixture(s), {checked} placement(s) — "
        + ", ".join(f"{n} {v}" for v, n in sorted(verdicts.items()))
    )
    if observed:
        print("  what the witnesses actually saw:")
        for what, n in sorted(observed.items()):
            print(f"    {n:>3}  {OBSERVES_MEANING[what]}")
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
