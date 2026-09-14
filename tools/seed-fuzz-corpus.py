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
    "document_open": 0,
    "prescan": 0,
    "qpdf_check": 0,
    "split": 1,
    "rotate": 2,
    "reorder": 2,
}

# `merge` is not in the table: its parameters are not a prefix. See the module docstring.
MERGE_TARGET = "merge"


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
            seed = bytes([5] * prefix) + body
            if problem := decodes_back(target, prefix, seed, body):
                print(f"\nFAILED — {problem}", file=sys.stderr)
                return 1
            if not check_only:
                (directory / f"seed-{path.name}").write_bytes(seed)
            count += 1
        written[target] = count

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
