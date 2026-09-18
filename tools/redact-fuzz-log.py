#!/usr/bin/env python3
"""Print a fuzzing log with everything that is not a known-safe line removed.

    cargo +nightly fuzz run t 2>&1 > >(python3 tools/redact-fuzz-log.py --tee full.log)
    python3 tools/redact-fuzz-log.py --check some.log     # would anything leak?

# Why this exists

A fuzzing log publishes the crashing input. Not as an attachment -- in the log text, on a public
job page, in full:

  * **cargo-fuzz** re-runs the target with `RUST_LIBFUZZER_DEBUG_PATH` set and prints
    ``Output of `std::fmt::Debug`:`` followed by the ENTIRE input as a decimal byte array. There
    is no size limit. Measured on the 2026-09-18 nightly: all 16,208 bytes of #119's reproducer
    were recoverable from one line of a public log.
  * **libFuzzer** prints `MS: ...`, a hex dump, an ASCII dump and `Base64: <whole input>` for any
    unit up to `kMaxUnitSizeToPrint` (256 bytes). Most of this project's seed units are smaller
    than that.

Deleting the run artifacts did not address this, and for four days it meant working reproducers
for the unpatched qpdf defects of #62 sat in public logs. See
`docs/security/exposure-2026-09-14-published-reproducers.md`.

# Why an allowlist, and not a list of things to strip

The obvious fix is to delete the dump sections. That is a denylist, and it is wrong here: the
next cargo-fuzz or libFuzzer release can add a new way of printing the input, and a denylist
publishes anything it has not been taught about. This keeps only lines matching a pattern that is
known to carry no input bytes, and drops everything else -- so an unrecognised line is withheld
rather than published, and a new dump format costs a missing progress line rather than a leak.

The cost is real and is stated: a genuinely new, useful line from a future libFuzzer is dropped
until somebody adds a rule for it. That is the right direction for a public log on a repository
whose fuzzing finds unpatched defects in someone else's library.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

#: Lines that may be printed, each with the reason it carries no input bytes. Anything not
#: matched here is dropped. Adding a rule means arguing that the line cannot contain the input.
ALLOWED: list[tuple[str, str]] = [
    (r"^#\d+\s+(READ|INITED|NEW|REDUCE|RELOAD|pulse|DONE|MIN)", "libFuzzer progress: counters only"),
    (r"^INFO:", "libFuzzer informational: seed, corpus size, paths"),
    (r"^Done \d+ runs", "libFuzzer's closing line"),
    (r"^ERROR:\s", "the sanitiser or libFuzzer verdict"),
    (r"^SUMMARY:\s", "the sanitiser's one-line summary"),
    (r"^==\d+==(ERROR|WARNING|ABORTING)", "ASan's banner and abort line"),
    (r"^\s+#\d+ 0x[0-9a-f]+", "a stack frame: addresses and symbols, never input"),
    (r"^\s*(Compiling|Finished|Running|warning:|error(\[E\d+\])?:)", "cargo's own output"),
    (r"^artifact_prefix=", "where an artifact was written -- a path, not its contents"),
    (r"^Test unit written to ", "the artifact path"),
    (r"^(Shadow bytes|Shadow byte legend|  Addressable|  Partially|  Heap|  Freed|  Stack|"
     r"  Global|  Container|  Array|  Intra|  Right|  Left|  ASan|=>|0x[0-9a-f]+:)",
     "ASan's shadow-memory legend: allocator state, not input"),
]

#: Lines that MUST be dropped, named, so the probe below has something to assert against. This is
#: not the mechanism -- the allowlist is -- but a rule with no counter-fixture is untested.
MUST_DROP: list[tuple[str, str]] = [
    ("\tOutput of `std::fmt::Debug`:", "cargo-fuzz's dump header"),
    ("\t[1, 5, 5, 37, 80, 68, 70]", "cargo-fuzz's dump body: the whole input, decimal"),
    ("Base64: JVBERi0xLjcK", "libFuzzer's base64 of the whole unit"),
    ("MS: 1 CrossOver-; base unit: e9b8e4b5", "libFuzzer's mutation trace"),
    ("0x25,0x50,0x44,0x46,", "libFuzzer's hex dump of the unit"),
    ("%PDF-1.7 1 0 obj << /Type /Catalog", "libFuzzer's ASCII rendering of the unit"),
]

#: Lines that ARE allowed but must come out shortened, with what must survive and what must not.
MUST_TRIM: list[tuple[str, str, str]] = [
    (
        '#4362\tNEW    cov: 4048 ft: 11219 corp: 365/1177Kb MS: 3 CrossOver- DE: "%PDF-1.7"',
        "cov: 4048",
        "%PDF-1.7",
    ),
    (
        '#100\tREDUCE cov: 10 ft: 20 corp: 1/1b lim: 4 exec/s: 0 rss: 30Mb L: 1/1 MS: 1 EraseBytes-',
        "exec/s: 0",
        "EraseBytes",
    ),
]

#: Markers after which the REST OF AN OTHERWISE ALLOWED LINE is input-derived and must go.
#: A line is not simply safe or unsafe: libFuzzer's progress lines are counters -- and then
#: `MS: ... DE: "%PDF-1.7"`, where `DE` is a dictionary entry, a literal FRAGMENT OF THE INPUT
#: that the mutator discovered. The allowlist alone published those, which the redactor's own
#: test against a real leaking log is what caught.
TRUNCATE_AT = (" MS: ", " DE: ")


def trim(line: str) -> str:
    """An allowed line with any input-derived tail removed."""
    for marker in TRUNCATE_AT:
        index = line.find(marker)
        if index != -1:
            line = line[:index] + "\n" if line.endswith("\n") else line[:index]
    return line


PATTERNS = [(re.compile(pattern), why) for pattern, why in ALLOWED]


def allowed(line: str) -> bool:
    return any(pattern.search(line) for pattern, _ in PATTERNS)


def probe() -> list[str]:
    """Every rule keeps its own fixture, and every named leak shape is dropped. Every run."""
    problems: list[str] = []
    fixtures = {
        r"^#\d+\s+(READ|INITED|NEW|REDUCE|RELOAD|pulse|DONE|MIN)": "#1234 NEW cov: 10 ft: 20",
        r"^INFO:": "INFO: seed corpus: files: 12 min: 1b max: 900b",
        r"^Done \d+ runs": "Done 335428 runs in 601 second(s)",
        r"^ERROR:\s": "ERROR: AddressSanitizer: stack-overflow on address 0x7ffc",
        r"^SUMMARY:\s": "SUMMARY: AddressSanitizer: stack-overflow",
        r"^==\d+==(ERROR|WARNING|ABORTING)": "==2850==ABORTING",
        r"^\s+#\d+ 0x[0-9a-f]+": "    #7 0x55f99897b9cc  (/path/rotate+0x5589cc)",
        r"^\s*(Compiling|Finished|Running|warning:|error(\[E\d+\])?:)": "Running `target/release/x`",
        r"^artifact_prefix=": "artifact_prefix='/home/runner/fuzz/artifacts/rotate/'",
        r"^Test unit written to ": "Test unit written to /home/runner/artifacts/crash-46610c95",
        r"^(Shadow bytes|Shadow byte legend|  Addressable|  Partially|  Heap|  Freed|  Stack|"
        r"  Global|  Container|  Array|  Intra|  Right|  Left|  ASan|=>|0x[0-9a-f]+:)":
            "Shadow bytes around the buggy address:",
    }
    for pattern, why in ALLOWED:
        fixture = fixtures.get(pattern)
        if fixture is None:
            problems.append(f"no fixture for the rule allowing {why!r}; it is untested")
            continue
        if not re.search(pattern, fixture):
            problems.append(f"the rule for {why!r} does not match its own fixture; it is inert")

    for leak, why in MUST_DROP:
        if allowed(leak):
            problems.append(f"a line carrying {why} would be PUBLISHED: {leak[:40]!r}")

    for line, must_keep, must_go in MUST_TRIM:
        trimmed = trim(line)
        if must_keep not in trimmed:
            problems.append(f"trimming removed the signal too: {must_keep!r} is gone from {trimmed!r}")
        if must_go in trimmed:
            problems.append(
                f"an input-derived tail survived trimming: {must_go!r} is still in {trimmed!r}"
            )
    return problems


def redact(source, out, tee: Path | None) -> tuple[int, int]:
    """Copy allowed lines to `out`; write everything to `tee` if given. Returns (kept, dropped)."""
    kept = dropped = 0
    handle = tee.open("w", encoding="utf-8") if tee else None
    try:
        for line in source:
            if handle:
                handle.write(line)
            if allowed(line.rstrip("\n")):
                out.write(trim(line))
                kept += 1
            else:
                dropped += 1
        out.flush()
    finally:
        if handle:
            handle.close()
    return kept, dropped


def main() -> int:
    parser = argparse.ArgumentParser(description="Redact a fuzzing log for publication.")
    parser.add_argument("--tee", metavar="PATH", help="write the UNREDACTED log here (stays on the runner)")
    parser.add_argument("--check", metavar="PATH", help="report what would be dropped, publish nothing")
    parser.add_argument("--probe", action="store_true", help="run the rule probes and exit")
    args = parser.parse_args()

    problems = probe()
    if problems:
        print(f"redact-fuzz-log: {len(problems)} problem(s):", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 1
    if args.probe:
        print(f"redact-fuzz-log: {len(ALLOWED)} allow rule(s) matched their own fixture; "
              f"{len(MUST_DROP)} known leak shape(s) dropped; "
              f"{len(MUST_TRIM)} line(s) trimmed of an input-derived tail")
        return 0

    if args.check:
        with open(args.check, encoding="utf-8", errors="replace") as handle:
            kept, dropped = redact(handle, open("/dev/null", "w", encoding="utf-8"), None)
        print(f"redact-fuzz-log: {kept} line(s) publishable, {dropped} withheld")
        return 0

    tee = Path(args.tee) if args.tee else None
    kept, dropped = redact(sys.stdin, sys.stdout, tee)
    print(f"redact-fuzz-log: {kept} line(s) published, {dropped} withheld", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
