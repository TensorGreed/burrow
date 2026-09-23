#!/usr/bin/env python3
"""Seed every fuzz target's corpus from the committed fixtures.

WHY THIS EXISTS

`fuzz/corpus/` is gitignored, and until 2026-09-14 nothing here had ever shipped a seed set.
Every "runs clean for 60 s" this project recorded was therefore measured against a corpus
libFuzzer had grown from random bytes -- and libFuzzer does not invent a valid PDF.

Measured, by planting a defect in `reorder` that made the permutation do nothing:

    unseeded, 60 s   577,209 executions   found nothing
    one seed          1 execution         failed immediately

The same seeding run then found two real defects within minutes: a silently lost page on a
damaged-but-openable document (#61), and a use-after-free in qpdf's page-tree handling (#62).
Neither was reachable from random bytes.

So an unseeded run of any of these targets exercises the parser's rejection paths and very
little else. That is worth having and it is not what the targets claim to measure.

THE PART THAT IS NOT OBVIOUS: EVERY TARGET EATS ITS INPUT DIFFERENTLY

A seed is not just a PDF. Each target carves parameters off the front of the buffer, so the
same file needs a different prefix per target, and a seed with the wrong prefix is a seed of
a truncated document:

    document_open, prescan, qpdf_check   no prefix; the whole buffer is the document
    split                                1 byte  (the cut selection)
    rotate                               2 bytes (page count selector, angle)
    render                               3 bytes (box width, box height, page selector)
    reorder                              2 bytes (shuffle seed)
    merge                                see below -- it is not a prefix at all

`merge` splits the WHOLE buffer in two at an offset derived from its first two bytes, and
those bytes are part of the first half. So the offset a seed gets is
`(A[0] * 256 + A[1]) % total`, where `A[0..2]` is whatever the first document happens to start
with -- `%P` for every PDF, i.e. 9552. For the split to land on the document boundary and give
the target two REAL documents, `9552 % (len(A) + len(B)) == len(A)` has to hold, which for
arbitrary fixtures it does not.

Rather than give up and hand `merge` two halves of one file -- which is what an unaware
seeder does, and which only ever exercises the refusal path -- this pads the second document
with trailing bytes until the arithmetic lands. Trailing bytes after `%%EOF` are tolerated:
`tests/conformance/fixtures/bomb-hidden-trailing-junk.pdf` exists precisely because they are.

Usage: tools/seed-fuzz-corpus.py [--check]
  --check  report what would be written and verify the merge arithmetic, writing nothing.
"""

from __future__ import annotations

import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent
CORPUS = REPO / "fuzz" / "corpus"

# Where the seeds come from. Both are committed, both are small, and between them they cover
# valid documents, the adversarial ones, and the damaged-but-openable class that had no
# coverage at all until #61.
SOURCES = [
    REPO / "tests" / "conformance" / "fixtures",
    REPO / "tests" / "damaged",
]

# How many bytes each target takes off the front before the document starts.
#
# THESE ARE READ FROM THE TARGETS, NOT GUESSED -- see `verify_prefixes`, which checks each
# number against the target's own source on every run. A prefix that drifts silently turns
# every seed for that target into a truncated document, and the run still reports clean.
PREFIXES = {
    # COMPRESS TAKES NO PREFIX, and that is a property of the operation rather than a choice.
    # It has no page list, no angle and no cut, so there is nothing for a leading byte to
    # steer -- and taking one would shift every seeded fixture by one byte, breaking the PDF
    # header on every case. Its whole input is the document.
    "compress": 0,
    "document_open": 0,
    "prescan": 0,
    "qpdf_check": 0,
    "split": 1,
    "rotate": 2,
    "reorder": 2,
    # RENDER TAKES THREE: two for the box it asks for and one for the page selector. The box
    # is steered by the input on purpose -- `max_pixels` is the newest code on that path, and
    # a target that always asked for a thumbnail would never reach the refusal.
    "render": 3,
}

# THE PREFIX BYTE EVERY SEED CARRIES. Named rather than written twice: `LIVE_REQUESTS` below
# decodes it and the writer emits it, and the whole point of that check is that the two agree.
SEED_PREFIX_BYTE = 5

# A PREFIX BYTE THAT DECODES TO A REQUEST THE TARGET THROWS AWAY.
#
# WHY THIS EXISTS, measured. `render`'s first version took `usize::from(data[2] % 5)` pages and
# returned early on zero. The prefix byte is 5, and `5 % 5` is zero -- so all twenty of its
# seeds returned before PDFium was opened: no page load, no rasteriser, no ceiling. The run was
# clean, the CI step passed, and the definition of done's "runs clean for 60 s against a seeded
# corpus" was hollow for that target. `verify_prefixes` could not see it: it checks that the
# DOCUMENT survives the prefix, and the document did.
#
# It is the failure CLAUDE.md already records for the unseeded corpus -- "a defect planted in
# `reorder` survived 577,209 unseeded executions and died on the first seeded one" -- arriving
# through the PARAMETER byte rather than through the document. Writing the rule down again was
# not going to work; this is the control.
#
# IT IS NOT A LIST SOMEBODY HAS TO KEEP UP TO DATE. `verify_live_requests` reads each target's
# source, finds the names bound from its prefix bytes, and looks for a guard that discards a
# request on one of them. A target that HAS such a guard must have an entry here; a target that
# does not need none, and today only `render` does -- which the scan establishes rather than
# this comment asserting it.
#
# Each entry is `(needle, decode)`. `needle` must appear VERBATIM in the target's source, so a
# decoder describing an expression the target no longer has fails rather than passing on a
# stale mirror. `decode` is that expression in Python, applied to the seed's prefix bytes, and
# must not return the value the guard discards.
LIVE_REQUESTS = {
    "render": (
        "1 + usize::from(data[2] % 4)",
        lambda prefix: 1 + prefix[2] % 4,
    ),
}

# `merge` is not in the table: its parameters are not a prefix. See the module docstring.
MERGE_TARGET = "merge"

# THE TWO TARGETS WHOSE INPUT IS NOT A DOCUMENT, and a whole fixture is the wrong seed for
# both. `pdfsyntax_dict_keys` reads what `qpdf_oh_unparse` produces for one dictionary, and a
# whole PDF does not begin with `<<` -- it is rejected on the first token, so it would be a
# seed that teaches libFuzzer nothing while inflating the reported seed count.
# `pdfsyntax_names` reads a DECODED content stream, which is the inside of a `stream` object
# rather than the file around it.
#
# So these two are CARVED out of the fixtures rather than copied. It is the same lesson this
# script was written from, one level in: a seed of the wrong shape is not a seed.
CARVED_TARGETS = (
    "pdfsyntax_dict_keys",
    "pdfsyntax_names",
    # #128's two. Same carve as `pdfsyntax_names`: the input is a decoded content stream.
    # `pdfsyntax_operations` reads one; `pdfsyntax_contents` cuts one into `/Contents` elements
    # at a delimiter its first input byte chooses, so the seed is still a content stream.
    "pdfsyntax_operations",
    "pdfsyntax_contents",
    # #129's. The input is a content stream again -- the glyph walk reads exactly one -- but the
    # target consumes its FIRST SIX BYTES as the resources a real font dictionary would supply,
    # so the seed carries a prefix for the same reason `pdfsyntax_contents` carries a delimiter.
    "pdfsyntax_geometry",
)

# `pdfsyntax_cmap_wmode` is NOT carved, and that is the point of the exception.
#
# Its grammar is a PostScript CMap program -- `def`, `usecmap`, `begincmap` -- and the corpus
# holds content streams. A carved content stream handed to it is a seed of the wrong shape, and
# this script's own `CARVED_TARGETS` comment says what that is worth. So its seeds are written
# here, one per case the derivation has to get right, and each is a program a real font could
# carry. The first byte is the target's dictionary-`WMode` parameter.
SYNTHETIC_SEEDS: dict[str, tuple[bytes, ...]] = {
    "pdfsyntax_cmap_wmode": (
        b"\x00/CIDInit /ProcSet findresource begin\n/CMapName /Identity-H def\n/WMode 0 def\n",
        b"\x00/CMapName /Identity-H def /WMode 1 def",
        b"\x00/CMapName /Perfectly-Ordinary-H def /WMode 1 def",
        b"\x00/CMapName /Custom def /UniJIS-UCS2-V usecmap",
        b"\x00/WMode 1 def /CMapName /X def /WMode 0 def",
        b"\x01/WMode 1 def",
        b"\x02/CMapName /Identity-H def",
        b"\x00%% /WMode 1 def\n/CMapName /Plain-H def",
        b"\x00(/WMode 1 def) pop /CMapName /Plain-H def",
        b"\x00Identity-V",
        b"\x00begincmap /WMode 1 def endcmap",
    ),
    # `pdfsyntax_tounicode` is not carved either, and for the same reason: its grammar is a
    # `/ToUnicode` CMap and the corpus holds content streams. Unlike the target above it takes
    # no parameter byte -- the whole input is the program, and the filter it narrows with is
    # derived from the first byte of that program.
    #
    # One seed per shape the reader has to get right, because a target seeded only from valid
    # producer output explores the front door and nothing past it. The last three are the
    # shapes that were defects: a code wider than four bytes, a range whose destination runs
    # past U+FFFF, and a section long enough to matter.
    "pdfsyntax_tounicode": (
        b"/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapType 2 def\n"
        b"1 begincodespacerange\n<00> <FF>\nendcodespacerange\n"
        b"4 beginbfchar\n<01> <0053>\n<02> <0065>\n<03> <0063>\n<04> <0072>\nendbfchar\n"
        b"endcmap\nend\nend\n",
        b"1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n"
        b"1 beginbfchar\n<0041> <0061>\nendbfchar\n",
        b"1 beginbfrange\n<20> <7E> <0020>\nendbfrange\n",
        b"1 beginbfrange\n<10> <12> [<0041> <00C6> <0042>]\nendbfrange\n",
        b"1 beginbfrange\n<10> <14> [<0041>]\nendbfrange\n",
        b"1 beginbfchar\n<0000000041> <0041>\nendbfchar\n",
        b"1 beginbfrange\n<00> <10> <FFFF>\nendbfrange\n",
        b"1 beginbfrange\n<40> <20> <0041>\nendbfrange\n",
        b"/CMapName /ABCDEF+Helvetica-UCS def\n1 beginbfchar\n<41> <0041>\nendbfchar\n",
        b"2 beginbfchar\n<41> <D83DDE00>\n<42> <0042>\nendbfchar\n",
    ),
    # `redact_shared_contents` reads its bytes as a SHAPE, not as a document: page count,
    # stream count, then each page's element-to-stream slots. Seeds are therefore the sharing
    # structures worth reaching first, written out rather than waited for -- a fuzzer will find
    # "two pages, one stream" eventually and there is no reason to spend the budget on it.
    #
    # Layout: [pages-1, streams-1, then per page: elements-1, then one byte per element].
    "redact_shared_contents": (
        # One page, one stream: nothing shared, must redact.
        bytes([0, 0, 0, 0]),
        # Two pages, one stream, one element each: the whole /Contents shared. ADR 0029's case.
        bytes([1, 0, 0, 0, 0, 0]),
        # Two pages, two streams. Page 0 is [0, 1], page 1 is [1]: the SHARED element is the one
        # the region misses, so this must redact. The letterhead shape.
        bytes([1, 1, 1, 0, 1, 0, 1]),
        # Two pages, two streams. Page 0 is [0, 1], page 1 is [0]: the shared element is the one
        # the region reaches, so this must refuse.
        bytes([1, 1, 1, 0, 1, 0, 0]),
        # One page referencing one stream twice: sharing with itself.
        bytes([0, 0, 1, 0, 0]),
        # Page 0 is [1, 0]: the drawn text is in element 1, not element 0. The shape the oracle
        # got wrong first time round.
        bytes([1, 1, 1, 1, 0, 0, 0]),
        # Three pages, three streams, no sharing at all.
        bytes([2, 2, 0, 0, 0, 1, 0, 2]),
        # Four pages all pointing at stream 0: maximum sharing.
        bytes([3, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
    ),
}

# Not every span is worth writing, and a fixture with 277 dictionaries would otherwise
# contribute 277 near-identical seeds. libFuzzer mutates from what it is given; more copies of
# the same shape is not more coverage.
MAX_SPANS_PER_FIXTURE = 24


def carve_dictionaries(body: bytes) -> list[bytes]:
    """Balanced `<<` ... `>>` spans, outermost first.

    Crude on purpose: it counts `<<` and `>>` and does not know that either can appear inside a
    string. A span that ends in the wrong place is still valid input for the target -- it is a
    SEED, and the target's business is surviving bytes that are not what they claim to be. What
    it must not do is produce something that is not a dictionary at all, which `carved_is_usable`
    checks before anything is written.
    """
    spans: list[bytes] = []
    at = 0
    while at < len(body) and len(spans) < MAX_SPANS_PER_FIXTURE:
        start = body.find(b"<<", at)
        if start < 0:
            break
        depth = 0
        cursor = start
        end = -1
        while cursor < len(body) - 1:
            pair = body[cursor : cursor + 2]
            if pair == b"<<":
                depth += 1
                cursor += 2
            elif pair == b">>":
                depth -= 1
                cursor += 2
                if depth == 0:
                    end = cursor
                    break
            else:
                cursor += 1
        if end < 0:
            break
        spans.append(body[start:end])
        # ONE PAST THE OPENING `<<`, NOT PAST THE WHOLE SPAN. Advancing to `end` meant only
        # OUTERMOST dictionaries were ever carved -- and a page dictionary's interesting shapes
        # are the nested ones, which is exactly what `top_level_keys` walks over with
        # `skip_one_object`. So the seed set had no example of the thing the target spends most
        # of its time on. Found by code review.
        at = start + 2
    return spans


def carve_streams(body: bytes) -> list[bytes]:
    """The bytes between `stream` and `endstream`, which is what a decoded content stream is.

    The fixtures this reads are hand-written and uncompressed, so a carved span really is
    content-stream syntax. A compressed one would be a span of flate output -- still a legal
    input to the target, and not a useful seed; the count is reported either way so a corpus
    that quietly became noise is visible.
    """
    spans: list[bytes] = []
    at = 0
    while at < len(body) and len(spans) < MAX_SPANS_PER_FIXTURE:
        start = body.find(b"stream", at)
        if start < 0:
            break
        # `endstream` also contains `stream`; skip past it rather than carving from inside it.
        if start >= 3 and body[start - 3 : start] == b"end":
            at = start + len(b"stream")
            continue
        payload = start + len(b"stream")
        # One EOL after the keyword belongs to the keyword, per PDF 32000-1 §7.3.8.
        if body[payload : payload + 2] == b"\r\n":
            payload += 2
        elif body[payload : payload + 1] in (b"\n", b"\r"):
            payload += 1
        end = body.find(b"endstream", payload)
        if end < 0:
            break
        spans.append(body[payload:end])
        at = end + len(b"endstream")
    return spans


# `pdfsyntax_contents` consumes its FIRST BYTE as the delimiter to cut the rest into `/Contents`
# elements, so a carved span handed over raw is a seed whose first token has been eaten and whose
# delimiter is whatever byte happened to be there. Measured by code review over the 38 carved
# spans: 38 of 38 lost their first byte, and **33 of 38 produced a single element** -- no boundary
# at all, which is the one axis the target exists to explore.
#
# So its seeds are built rather than copied, two per span, and each is a different question:
#
#   * cut on white space -- the divisions fall between tokens, which is where the specification
#     says a real `/Contents` array divides;
#   * cut on a byte that occurs INSIDE a token, which is where a hostile document divides.
#
# This is the same lesson as `CARVED_TARGETS` itself, one level further in: a seed of the wrong
# shape is not a seed, and a target that eats a parameter needs the parameter chosen.
MAX_VARIANTS_PER_SPAN = 2


def contents_seed_variants(span: bytes) -> list[bytes]:
    """`[delimiter] + span` for a boundary between tokens and one inside a token."""
    variants: list[bytes] = []
    for delimiter in (b" ", b"\n"):
        if span.count(delimiter[0]) >= 1:
            variants.append(delimiter + span)
            break
    # A byte that is neither white space nor a delimiter character is inside a token by
    # construction: the lexer would not have ended a token there.
    for byte in span:
        if byte not in b" \t\r\n\x00\x0c()<>[]{}/%":
            variants.append(bytes([byte]) + span)
            break
    return variants[:MAX_VARIANTS_PER_SPAN]


# `pdfsyntax_geometry` consumes its FIRST SIX BYTES as the metrics its `Resources` stub returns
# -- the widths, font matrix, bytes-per-code and `/FontBBox` a real font dictionary supplies and
# a real attacker chooses. A span handed over raw loses six bytes off the front of the content
# stream AND pins the resources to whatever those bytes decoded to, which is the same "the target
# eats a parameter, so the parameter must be chosen" lesson as the delimiter above.
#
# Two per span, because the two ends of the resource space ask different questions:
#
#   * ordinary metrics -- 500/1000 widths, a millimetre-scale font matrix, single-byte codes, no
#     declared `/FontBBox`, horizontal. This is the seed that actually places glyphs, so it is
#     the one that reaches the arithmetic at all;
#   * degenerate metrics -- a width of `f64::MAX`, a two-byte encoding and a declared box, which
#     is where the box composition overflows and where the conservative-box claim is load-bearing.
GEOMETRY_CONTROL_PREFIXES = (bytes([1, 7, 1, 0, 1, 0]), bytes([3, 5, 2, 1, 3, 1]))


def geometry_seed_variants(span: bytes) -> list[bytes]:
    """`[six control bytes] + span`, once for ordinary metrics and once for degenerate ones."""
    return [prefix + span for prefix in GEOMETRY_CONTROL_PREFIXES]


def carved_is_usable(target: str, seed: bytes) -> str | None:
    """Why this carved seed would teach the target nothing, or `None` if it is fine.

    The counterpart of `decodes_back` for the carved targets. Without it a carving bug writes a
    directory full of empty files and the run reports a healthy seed count -- the exact failure
    this script exists to prevent, arriving through the new code rather than the old.
    """
    if not seed:
        return f"{target}: an empty span is not a seed"
    if target == "pdfsyntax_dict_keys":
        if not seed.startswith(b"<<"):
            return f"{target}: a seed that does not begin with '<<' is rejected on the first token"
        if not seed.endswith(b">>"):
            return f"{target}: a seed with no closing '>>' can only ever exercise the refusal"
    if target == "pdfsyntax_geometry":
        # REPLAY THE TARGET'S OWN PARAMETER READ, as for `pdfsyntax_contents` below: a seed
        # shorter than the control prefix is discarded by `split_at_checked` before the walk
        # ever runs, so it would be a file in the corpus that exercises nothing.
        if len(seed) <= len(GEOMETRY_CONTROL_PREFIXES[0]):
            return (
                f"{target}: a seed no longer than its six control bytes leaves an empty content "
                "stream, so the walk never runs"
            )
    if target == "pdfsyntax_contents":
        # REPLAY THE TARGET'S OWN PARAMETER READ. A seed that cuts into one element exercises
        # no boundary, which is the whole subject -- and 33 of 38 did before this existed.
        delimiter, body = seed[0], seed[1:]
        if len(body.split(bytes([delimiter]))) < 2:
            return (
                f"{target}: a seed whose delimiter byte does not occur in it produces a single "
                "/Contents element, so it exercises no boundary"
            )
    return None


def verify_prefixes() -> list[str]:
    """Each declared prefix must match what the target's source actually does.

    A rule that is written down once and never checked is the shape this repository keeps
    getting caught by. The check is crude on purpose -- it looks for the slice the target
    takes -- but it fails loudly when a target changes its input layout, which is the event
    that would otherwise silently truncate every seed.
    """
    problems = []
    for target, prefix in PREFIXES.items():
        source = REPO / "fuzz" / "fuzz_targets" / f"{target}.rs"
        if not source.exists():
            problems.append(f"{target}: no such fuzz target")
            continue
        text = source.read_text(encoding="utf-8")
        expected = "data" if prefix == 0 else f"&data[{prefix}..]"
        if prefix == 0:
            # A zero-prefix target must NOT slice the front off its input.
            if "&data[1..]" in text or "&data[2..]" in text:
                problems.append(
                    f"{target}: declared prefix 0, but the target slices its input -- "
                    f"every seed for it is now a truncated document"
                )
        elif expected not in text:
            problems.append(
                f"{target}: declared prefix {prefix}, but {expected!r} is not in the target -- "
                f"either the prefix changed or this table is stale"
            )
    return problems


def sources() -> list[pathlib.Path]:
    found: list[pathlib.Path] = []
    for directory in SOURCES:
        if not directory.is_dir():
            continue
        found.extend(sorted(p for p in directory.iterdir() if p.is_file() and p.suffix != ".md"))
    return found


def merge_pairing(first: bytes, second: bytes) -> bytes | None:
    """`first` and `second` as one buffer the merge target will split on the boundary.

    Returns `None` if no padding under the cap makes the arithmetic land, rather than
    returning a buffer that splits somewhere else -- a seed that silently splits mid-document
    is worse than no seed, because it looks like coverage of a two-document merge and is not.
    """
    if len(first) < 2:
        return None
    offset = first[0] * 256 + first[1]
    for padding in range(0, 4096):
        total = len(first) + len(second) + padding
        if total <= len(first):
            continue
        # The target clamps to [1, total - 1]; a landing point outside that is not reachable.
        if not 1 <= len(first) <= total - 1:
            continue
        if offset % total == len(first):
            return first + second + (b"\n" * padding)
    return None


def parameter_names(text: str, prefix: int) -> set[str]:
    """Names a target binds from its PREFIX bytes.

    Crude on purpose, like `verify_prefixes`: a regular expression over `let NAME = ... data[i]`
    for `i` inside the prefix. What it has to be right about is the direction of its failure --
    a name it misses means a guard it cannot see, so the scan below is a floor on coverage
    rather than a proof of it, and that is said rather than implied.
    """
    names = set()
    for match in re.finditer(r"let\s+(\w+)\s*=\s*([^;]+);", text):
        name, expression = match.group(1), match.group(2)
        for index in re.findall(r"data\[(\d+)\]", expression):
            if int(index) < prefix:
                names.add(name)
    return names


# A synthetic target that DOES discard a request on a parameter, and one that does not.
#
# THE SCAN NEEDS A POSITIVE FIXTURE OR IT IS NOT A CHECK. Today no committed target carries
# such a guard -- `render`'s was removed, which is the fix -- so the scan examines four targets
# and finds zero, and "zero found" reads exactly like "the rule works". These two strings are
# run through the same code on every invocation: the first must be caught, the second must not.
# A rule that matches nothing passes everything; one that matches everything fails everything.
PROBE_GUARDED = """
    let wanted = usize::from(data[0] % 5);
    let document = &data[1..];
    if document.is_empty() || wanted == 0 {
        return;
    }
"""

PROBE_UNGUARDED = """
    let wanted = 1 + usize::from(data[0] % 4);
    let document = &data[1..];
    if document.is_empty() {
        return;
    }
"""


def guarded_parameters(text: str, prefix: int) -> list[str]:
    """Names bound from the prefix that a `== 0` test can discard a request on."""
    return sorted(
        name
        for name in parameter_names(text, prefix)
        if re.search(rf"\b{re.escape(name)}\s*==\s*0\b", text)
    )


def probe_live_request_scan() -> list[str]:
    """The scan must catch its own positive fixture and clear its near-miss, every run."""
    problems = []
    if guarded_parameters(PROBE_GUARDED, 1) != ["wanted"]:
        problems.append(
            "the guard scan does not catch its own positive fixture -- it would report every "
            "target clean whatever they contained"
        )
    if guarded_parameters(PROBE_UNGUARDED, 1) != []:
        problems.append(
            "the guard scan flags its near-miss -- it matches a target with no guard at all, "
            "so every target would need a rule and the rules would mean nothing"
        )
    return problems


def verify_live_requests() -> list[str]:
    """A guard that discards a request on a prefix-derived value must have a live-request rule.

    Two halves, and both are needed. The SCAN finds targets that can throw a request away on a
    parameter; the RULE proves the constant prefix byte does not produce that value. A scan
    with no rule reports a hazard and cannot say whether it fires; a rule with no scan is a
    list somebody has to remember to add to.
    """
    problems = []
    checked = 0
    for target, prefix in sorted(PREFIXES.items()):
        if prefix == 0:
            continue
        source = REPO / "fuzz" / "fuzz_targets" / f"{target}.rs"
        if not source.exists():
            problems.append(f"{target}: no such fuzz target")
            continue
        text = source.read_text(encoding="utf-8")

        guarded = guarded_parameters(text, prefix)
        rule = LIVE_REQUESTS.get(target)

        if guarded and rule is None:
            problems.append(
                f"{target}: discards a request when {' or '.join(guarded)} is zero, and has no "
                f"live-request rule -- so nothing establishes that a seed reaches the engine"
            )
            continue
        if rule is None:
            continue

        needle, decode = rule
        checked += 1
        if needle not in text:
            problems.append(
                f"{target}: the live-request rule mirrors {needle!r}, which is not in the "
                f"target -- either the target changed or this rule is stale, and a stale "
                f"mirror reports a live seed for a target that discards it"
            )
            continue
        if decode(bytes([SEED_PREFIX_BYTE] * prefix)) == 0:
            problems.append(
                f"{target}: a seed prefix of {SEED_PREFIX_BYTE} decodes to an empty request, "
                f"so every seed for it returns before the engine is touched -- the run is "
                f"clean because nothing ran"
            )
    for target in LIVE_REQUESTS:
        if target not in PREFIXES:
            problems.append(f"{target}: has a live-request rule but no declared prefix")
    return problems


def decodes_back(target: str, prefix: int, seed: bytes, document: bytes) -> str | None:
    """Re-derive the document from the seed the way the TARGET will, and compare.

    The point of a seed is that the target sees a whole document. Building one is three lines
    and getting it wrong is silent: a seed whose prefix is one byte short is a seed of a PDF
    missing its `%`, which parses as nothing and teaches libFuzzer nothing. So every seed is
    decoded back here, on every run, before it is written.
    """
    if seed[prefix:] != document:
        return f"{target}: the document does not survive the {prefix}-byte prefix"
    return None


def merge_decodes_back(seed: bytes, first: bytes) -> str | None:
    """The merge seed must split exactly on the document boundary, by the target's own rule."""
    if len(seed) < 4:
        return "merge: the seed is below the target's four-byte floor"
    at = (seed[0] * 256 + seed[1]) % len(seed)
    at = max(1, min(at, len(seed) - 1))
    if at != len(first):
        return (
            f"merge: the target will split at {at}, and the first document ends at "
            f"{len(first)} -- this seed is two half-documents, not two documents"
        )
    if seed[:at] != first:
        return "merge: the first half is not the first document"
    return None


def main(argv: list[str]) -> int:
    check_only = "--check" in argv

    print("seeding fuzz corpora from the committed fixtures")
    problems = verify_prefixes()
    if problems:
        print("\nFAILED — the prefix table does not match the targets:", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 1
    print(f"  prefix table verified against {len(PREFIXES)} target source(s)")

    problems = probe_live_request_scan()
    if problems:
        print("\nFAILED — the guard scan's own probes did not behave:", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 1
    print("  guard scan probes: 1 positive fixture caught, 1 near-miss cleared")

    problems = verify_live_requests()
    if problems:
        print("\nFAILED — a seed prefix produces a request its target discards:", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 1
    print(
        f"  {len(PREFIXES) - sum(1 for p in PREFIXES.values() if p == 0)} prefixed target(s) "
        f"scanned for a guard on a parameter; {len(LIVE_REQUESTS)} carry one and were decoded"
    )

    files = sources()
    if not files:
        print("error: no fixtures found, so this run would seed nothing", file=sys.stderr)
        return 1
    print(f"  {len(files)} fixture(s) from {len(SOURCES)} directory(ies)")

    written: dict[str, int] = {}

    for target, prefix in PREFIXES.items():
        directory = CORPUS / target
        if not check_only:
            directory.mkdir(parents=True, exist_ok=True)
        count = 0
        for path in files:
            body = path.read_bytes()
            # A fixed, boring prefix. libFuzzer mutates it freely from there; what the seed
            # has to get right is that the DOCUMENT is intact, not that the parameters are
            # interesting.
            seed = bytes([SEED_PREFIX_BYTE] * prefix) + body
            if problem := decodes_back(target, prefix, seed, body):
                print(f"\nFAILED — {problem}", file=sys.stderr)
                return 1
            if not check_only:
                (directory / f"seed-{path.name}").write_bytes(seed)
            count += 1
        written[target] = count

    # The carved targets. See CARVED_TARGETS: a whole fixture is the wrong shape for both.
    for target in CARVED_TARGETS:
        directory = CORPUS / target
        if not check_only:
            directory.mkdir(parents=True, exist_ok=True)
        carve = carve_dictionaries if target == "pdfsyntax_dict_keys" else carve_streams
        count = 0
        # GATED ON A DERIVABLE COUNT, not on non-zero. Every fixture that CAN contribute a span
        # must contribute one; a carving bug that produced spans for two fixtures out of
        # seventeen would otherwise report a healthy total. `CLAUDE.md`: a number with no
        # expectation beside it is not a report.
        #
        # "Can contribute" is a closing delimiter, not an opening one, and the difference is a
        # real fixture rather than a nicety. `canary.pdf` opens a `stream` and never writes
        # `endstream` -- it is a damaged fixture and that is the point of it -- so an
        # expectation keyed on `stream` demanded a span that cannot exist, and the first run of
        # this gate failed on it. Keyed on the closing delimiter it demands exactly the spans
        # that are there.
        marker = b">>" if target == "pdfsyntax_dict_keys" else b"endstream"
        expected_contributors = [p for p in files if marker in p.read_bytes()]
        contributed = []
        for path in files:
            spans = carve(path.read_bytes())
            wrote_any = False
            for index, span in enumerate(spans):
                if target == "pdfsyntax_contents":
                    variants = contents_seed_variants(span)
                elif target == "pdfsyntax_geometry":
                    variants = geometry_seed_variants(span)
                else:
                    variants = [span]
                for variant_index, seed in enumerate(variants):
                    if problem := carved_is_usable(target, seed):
                        print(f"\nFAILED — {problem} (from {path.name})", file=sys.stderr)
                        return 1
                    if not check_only:
                        name = f"seed-{path.name}-{index}"
                        if len(variants) > 1:
                            name = f"{name}-{variant_index}"
                        (directory / name).write_bytes(seed)
                    count += 1
                    wrote_any = True
            if wrote_any:
                contributed.append(path)
        written[target] = count
        silent = [p.name for p in expected_contributors if p not in contributed]
        if silent:
            print(
                f"\nerror: {target} carved nothing from {len(silent)} fixture(s) that contain "
                f"{marker!r}, so those shapes are absent from its corpus: {silent}",
                file=sys.stderr,
            )
            return 1
        # NAMED, not counted. A fixture that opens a construct and never closes it cannot
        # contribute, and saying which ones those are is what lets a reader tell a legitimate
        # shortfall from a carving that stopped working.
        opener = b"<<" if target == "pdfsyntax_dict_keys" else b"stream"
        unclosed = [p.name for p in files if opener in p.read_bytes() and marker not in p.read_bytes()]
        print(
            f"  {target}: {count} span(s) from {len(contributed)} of "
            f"{len(expected_contributors)} fixture(s) containing {marker!r}"
        )
        if unclosed:
            print(f"    (no {marker!r} to close a {opener!r} in: {', '.join(unclosed)})")

    # merge: every ordered pair would be n^2 seeds for no extra coverage, so each fixture is
    # paired with the next one round-robin -- every fixture appears as a first input and as a
    # second one.
    directory = CORPUS / MERGE_TARGET
    if not check_only:
        directory.mkdir(parents=True, exist_ok=True)
    count = 0
    unpaired = []
    for i, path in enumerate(files):
        other = files[(i + 1) % len(files)]
        body = path.read_bytes()
        buffer = merge_pairing(body, other.read_bytes())
        if buffer is None:
            unpaired.append(f"{path.name}+{other.name}")
            continue
        if problem := merge_decodes_back(buffer, body):
            print(f"\nFAILED — {problem}", file=sys.stderr)
            return 1
        if not check_only:
            (directory / f"seed-{path.name}-{other.name}").write_bytes(buffer)
        count += 1
    written[MERGE_TARGET] = count

    print()
    # The synthetic seeds, written after the carved ones so the report counts both.
    for target, seeds in SYNTHETIC_SEEDS.items():
        directory = CORPUS / target
        if not check_only:
            directory.mkdir(parents=True, exist_ok=True)
        for index, seed in enumerate(seeds):
            if not check_only:
                (directory / f"seed-synthetic-{index}").write_bytes(seed)
        written[target] = len(seeds)
        print(f"  {target}: {len(seeds)} synthetic seed(s), hand-written (see SYNTHETIC_SEEDS)")

    for target in sorted(written):
        print(f"  {target:16} {written[target]:>3} seed(s)")
    if unpaired:
        # Reported, never silently skipped: a merge corpus that is quietly half-empty is the
        # thing this whole script exists to stop.
        print(f"\n  {len(unpaired)} merge pairing(s) had no landing offset under the cap:")
        for name in unpaired:
            print(f"    {name}")

    if written[MERGE_TARGET] == 0:
        print(
            "\nerror: no merge seed pairs a real document with another, so the merge target "
            "would still never see a two-document merge",
            file=sys.stderr,
        )
        return 1

    print("\n" + ("OK — nothing written (--check)." if check_only else "OK — corpora seeded."))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
