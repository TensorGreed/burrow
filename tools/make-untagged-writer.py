#!/usr/bin/env python3
"""Derive `producer-writer-untagged.pdf` from `producer-writer.pdf`, deterministically.

    python3 tools/make-untagged-writer.py            # write tests/redaction/fixtures/producer-writer-untagged.pdf
    python3 tools/make-untagged-writer.py --check    # regenerate in memory and require the committed bytes

WHY IT EXISTS (#125 slice 3, 2026-10-15)

Slice 3 refuses every document with a structure tree, `[structure-tree]`, and the LibreOffice
export `producer-writer.pdf` is tagged. It was the redaction suite's one real producer document
that redacted -- the e2e suite's canonical "redacts Ok" document (`REDACTS`, `WRITER`) and
`glyph_geometry`'s real-producer redaction -- so slice 3 left those with nothing to redact. This
is the same export with its tagging taken off and nothing else changed: the same fonts, subsets,
`/ToUnicode`, `/Info` and layout, so what those tests measure is still a real producer's output.
`producer-writer.pdf` stays, as slice 3's refusal fixture; this is its near-miss twin.

It is INTERIM coverage for #359, not what #359 asks for: a real untagged export from a producer,
made by the producer. A file derived by burrow's own tooling can only carry what this script
leaves in it.

WHAT IT REMOVES, and only that

  * the catalogue's `/StructTreeRoot` and `/MarkInfo`;
  * each page's `/StructParents`;
  * each marked-content sequence's `/MCID` property list: `/Tag <</MCID n>> BDC` becomes
    `/Tag BMC`. Same tag, same enclosed content, same nesting; `EMC` is untouched.

Each count is asserted against the source -- 1 `/StructTreeRoot` key, 1 `/MarkInfo`, 1
`/StructParents`, 3 `/MCID` -- so a different source, or an edit that matched nothing, refuses
rather than writing a file that is still tagged. The output is written through qpdf, which writes
only what the trailer reaches, so the structure elements the removed root held are dropped too.
The result is then checked for none of the four names anywhere in its QDF expansion.

DETERMINISTIC: qpdf `--deterministic-id`, fixed options, no timestamps. `--check` regenerates
and requires the committed bytes, and `tools/check-redaction-corpus.sh` runs it.

Needs the qpdf CLI and `fix-qdf`: the vendored build's (`engines/build-native.sh`), or `QPDF` /
`FIX_QDF` set, or both on PATH.
"""

from __future__ import annotations

import argparse
import os
import platform
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SOURCE = REPO / "tests/redaction/fixtures/producer-writer.pdf"
OUTPUT = REPO / "tests/redaction/fixtures/producer-writer-untagged.pdf"
# The vendored build for this machine, as `check-redaction-corpus.py` finds it: the same qpdf and
# the same zlib on every architecture, so the bytes do not depend on where this runs.
VENDORED = REPO / "engines" / "vendor" / "src" / f"build-qpdf-plain-{platform.machine()}" / "qpdf"

# What the source carries, measured on the committed file (2026-10-16). An edit is asserted to
# have removed exactly this many, so a source that changed under this script refuses.
EXPECTED = {"StructTreeRoot": 1, "MarkInfo": 1, "StructParents": 1, "MCID": 3}
TAGGING = ("/StructTreeRoot", "/MarkInfo", "/StructParents", "/MCID", "/StructElem", "/ParentTree")


def tool(name: str, env: str) -> str:
    """The named qpdf tool: `$env`, the vendored build's, or PATH's -- refusing if none."""
    if os.environ.get(env):
        return os.environ[env]
    candidate = VENDORED / name
    if candidate.is_file() and os.access(candidate, os.X_OK):
        return str(candidate)
    found = shutil.which(name)
    if found:
        return found
    sys.exit(f"make-untagged-writer: no `{name}` -- run engines/build-native.sh or set {env}")


def run(argv: list[str], stdin: bytes | None = None) -> bytes:
    result = subprocess.run(argv, input=stdin, capture_output=True, check=False)
    # qpdf exits 3 for warnings; anything it warns about here is a source this does not expect.
    if result.returncode != 0:
        sys.exit(
            f"make-untagged-writer: {' '.join(argv[:2])}… exited {result.returncode}: "
            f"{result.stderr.decode(errors='replace').strip()[:400]}"
        )
    return result.stdout


def remove_key_lines(qdf: str, key: str) -> tuple[str, int]:
    """Remove each `  /Key value` line, or `/Key <<` … its matching `>>`, from a QDF expansion."""
    lines = qdf.split("\n")
    out: list[str] = []
    removed = 0
    i = 0
    while i < len(lines):
        line = lines[i]
        if re.fullmatch(rf"\s*/{key}(\s.*)?", line):
            removed += 1
            if line.rstrip().endswith("<<"):
                depth = 1
                i += 1
                while i < len(lines) and depth:
                    depth += lines[i].count("<<") - lines[i].count(">>")
                    i += 1
                continue
            i += 1
            continue
        out.append(line)
        i += 1
    return "\n".join(out), removed


def untagged(source: bytes) -> bytes:
    qpdf = tool("qpdf", "QPDF")
    fix_qdf = tool("fix-qdf", "FIX_QDF")
    with tempfile.TemporaryDirectory() as scratch:
        return untagged_in(Path(scratch), source, qpdf, fix_qdf)


def untagged_in(scratch: Path, source: bytes, qpdf: str, fix_qdf: str) -> bytes:
    # qpdf reads its input from a file, never from standard input; each step stages one.
    original = scratch / "original.pdf"
    original.write_bytes(source)
    qdf = run([qpdf, "--qdf", "--object-streams=disable", "--deterministic-id", str(original), "-"])
    text = qdf.decode("latin-1")

    counts: dict[str, int] = {}
    for key in ("StructTreeRoot", "MarkInfo", "StructParents"):
        text, counts[key] = remove_key_lines(text, key)
    # A SEPARATOR BEFORE `BMC`, ALWAYS. LibreOffice writes `/H1<</MCID 0>>BDC` with nothing between
    # the tag and the property list, so replacing the list and `BDC` with a bare `BMC` glued them
    # into one name, `/H1BMC`, and the page refused `[operand-count-mismatch]` -- the corpus check
    # caught it (2026-10-16). Each rewritten sequence is then required to read `/Tag BMC`.
    text, counts["MCID"] = re.subn(r"<<\s*/MCID\s+\d+\s*>>\s*BDC", " BMC", text)
    glued = re.findall(r"/[A-Za-z0-9]+BMC\b", text)
    if glued:
        sys.exit(f"make-untagged-writer: a tag glued to its operator: {glued}")
    if counts != EXPECTED:
        sys.exit(
            f"make-untagged-writer: removed {counts}, expected {EXPECTED} -- the source is not "
            f"the file this was written against, or an edit matched nothing"
        )

    edited = scratch / "edited.qdf"
    edited.write_bytes(text.encode("latin-1"))
    stage = scratch / "stage.pdf"
    stage.write_bytes(run([fix_qdf, str(edited)]))
    out = run(
        [
            qpdf,
            "--deterministic-id",
            "--object-streams=preserve",
            "--compress-streams=y",
            "--decode-level=generalized",
            str(stage),
            "-",
        ]
    )
    written = scratch / "written.pdf"
    written.write_bytes(out)
    expansion = run([qpdf, "--qdf", "--object-streams=disable", str(written), "-"])
    left = [name for name in TAGGING if re.search(rb"%s\b" % re.escape(name.encode()), expansion)]
    if left:
        sys.exit(f"make-untagged-writer: the output still carries {left}")
    return out


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--check", action="store_true", help="require the committed bytes")
    args = parser.parse_args(argv)
    made = untagged(SOURCE.read_bytes())
    if args.check:
        if not OUTPUT.is_file():
            print(f"make-untagged-writer: {OUTPUT.relative_to(REPO)} is not committed", file=sys.stderr)
            return 1
        if OUTPUT.read_bytes() != made:
            print(
                f"make-untagged-writer: {OUTPUT.relative_to(REPO)} is not what this script makes "
                f"from {SOURCE.relative_to(REPO)} -- regenerate it, or say why it differs",
                file=sys.stderr,
            )
            return 1
        print(
            f"make-untagged-writer: OK -- {OUTPUT.relative_to(REPO)} ({len(made):,} bytes) is "
            f"what this script makes; {', '.join(f'{v} {k}' for k, v in EXPECTED.items())} "
            f"removed, none left"
        )
        return 0
    OUTPUT.write_bytes(made)
    print(f"make-untagged-writer: wrote {OUTPUT.relative_to(REPO)} ({len(made):,} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
