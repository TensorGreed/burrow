#!/usr/bin/env python3
"""Decide whether a fuzzing crash is one we already own, or a new finding.

    python3 tools/check-known-crashes.py --log fuzz.log [--binary path/to/target]
    python3 tools/check-known-crashes.py --check          # the ledger itself, offline
    python3 tools/check-known-crashes.py --check-issues   # every entry's issue is still open

# What this is for

The seeded nightly has been red since 2026-09-14 because #62 is open and reachable from every
qpdf-driving target. That is deliberate -- ADR 0025 calls a crash there "a report with an owner"
-- and it cost what a permanently-red gate always costs: #119 landed in it on 2026-09-18 and was
indistinguishable from the standing failure. A gate that is always red carries no bits.

So a crash is classified. Known -> reported, and the job stays green. Unmatched -> the run fails,
because that is a new defect and the whole point of running this nightly.

# The key, and why it is a pair

`(verdict, frame)`. Measured, in this repository, from two reproductions:
`QPDF::Doc::Pages::pushInheritedAttributesToPageInternal` is in the stack of BOTH #62's page-tree
use-after-free AND #119's stack overflow. Keyed on the frame alone, filing #62 would have
silenced #119. Keyed on the verdict alone, any use-after-free anywhere in qpdf would match.

The frame is matched anywhere in the stack, not just at the top: for a use-after-free the
"hottest" frame is often an allocator interceptor or the entry point, not the defect site --
`reorder`'s modal frame is `QPDF::processMemoryFile`, which identifies nothing.

# What this does NOT claim

* It does not prove a crash IS the known defect. It proves the report is consistent with one --
  the same verdict in the same function. A second defect in the same function under the same
  sanitiser verdict would be absorbed. That is the residual, and it is the price of any
  fingerprint short of the whole stack.
* It says nothing about severity. A known crash is an owned crash, not an acceptable one.
* It cannot see a crash that produces no report at all: a timeout, an OOM, or a build failure is
  handled by the caller, not here.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
import tomllib
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
LEDGER = Path(__file__).resolve().parent.parent / "fuzz" / "known-crashes.toml"

#: The sanitiser's verdict, normalised to the class token: `heap-use-after-free`,
#: `stack-overflow`, `SEGV`. Uppercase is included deliberately -- `[a-z-]` silently dropped
#: `SEGV`, which is exactly how #119's class surfaces when ASan's own detector does not engage.
VERDICT = re.compile(r"ERROR: (?:AddressSanitizer|libFuzzer):\s+(.+)")

#: Where the verdict stops and the incident's details begin. libFuzzer's verdicts are PHRASES --
#: `deadly signal`, `timeout after 10 seconds`, `out-of-memory (malloc(…))` -- and capturing a
#: single token turned `deadly signal` into `deadly`, so a ledger entry spelling it correctly
#: would never have matched. The neighbouring Describe step learned this after being wrong twice:
#: take the line and trim it, rather than trying to spell the set of verdicts.
VERDICT_ENDS = (" on ", " after", " (", " in ", ":")

#: A symbolised frame, ANCHORED TO A FRAME LINE (`    #3 0x… in Some::Function(…)`). Scanning
#: the whole log for ` in X` would let one prose match suppress the addr2line fallback entirely
#: and inflate the count of frames considered.
SYMBOL = re.compile(r"^\s*#\d+\s+0x[0-9a-f]+\s+in\s+([A-Za-z_][A-Za-z0-9_:~]*)", re.M)

#: The shape a ledger `verdict` must have once normalised, so an entry the classifier could never
#: resolve is a refusal rather than silence.
VERDICT_SHAPE = re.compile(r"^[A-Za-z][A-Za-z0-9-]*$")

#: A raw offset into the target binary, for a report that was not symbolised.
def offsets_for(target: str) -> re.Pattern[str]:
    return re.compile(re.escape(target) + r"\+0x([0-9a-f]+)")


class Problem(Exception):
    """The ledger or the classification is wrong."""


def load(path: Path | None = None) -> list[dict]:
    ledger = path or LEDGER
    if not ledger.is_file():
        raise Problem(f"no ledger at {ledger}")
    with ledger.open("rb") as handle:
        entries = tomllib.load(handle).get("known", [])
    for index, entry in enumerate(entries):
        for field in ("issue", "verdict", "frame", "note"):
            if not entry.get(field):
                raise Problem(f"entry {index} has no {field!r}; an unexplained entry is a mute button")
        if not isinstance(entry["issue"], int):
            raise Problem(f"entry {index}: issue must be a number, not {entry['issue']!r}")
        # AN ENTRY THE CLASSIFIER COULD NEVER RESOLVE IS A REFUSAL, NOT SILENCE. Verdicts are
        # compared against a normalised phrase, so `deadly signal` must be written
        # `deadly-signal`; an entry carrying a space would sit here inert forever with nobody
        # told -- the same family as a gate keyed on a hand-edited identifier.
        if not VERDICT_SHAPE.match(entry["verdict"]):
            raise Problem(
                f"entry {index}: verdict {entry['verdict']!r} can never match. Write the "
                f"sanitiser's phrase with spaces hyphenated: 'heap-use-after-free', "
                f"'stack-overflow', 'deadly-signal'."
            )
    return entries


def normalise_verdict(raw: str) -> str:
    """`heap-use-after-free on address 0x1` -> `heap-use-after-free`; `deadly signal` -> `deadly-signal`."""
    text = raw.strip()
    for marker in VERDICT_ENDS:
        index = text.find(marker)
        if index > 0:
            text = text[:index]
    return "-".join(text.split())


def frames_in(log: str, binary: Path | None, target: str | None) -> tuple[list[str], str]:
    """Every function named in the report, and HOW it was obtained.

    The second value matters: zero frames means "could not classify", not "nothing matched", and
    conflating them made the tool announce a five-nights-old defect as a new finding.
    """
    found = list(dict.fromkeys(SYMBOL.findall(log)))
    if found:
        return found, "symbolised report"
    if not (binary and target):
        return [], "no symbols in the report and no binary given"
    if not binary.is_file():
        return [], f"no symbols in the report and {binary} is not on disk"

    # NOT SYMBOLISED. CI's libFuzzer prints `target+0x…` and nothing else, so without this the
    # classifier sees no frames at all and calls every known crash new -- a gate that fails
    # closed, but uselessly.
    addresses = list(dict.fromkeys(offsets_for(target).findall(log)))[:40]
    if not addresses:
        return [], "no symbols and no resolvable offsets in the report"
    try:
        out = subprocess.run(
            ["addr2line", "-f", "-C", "-e", str(binary), *[f"0x{a}" for a in addresses]],
            capture_output=True, text=True, check=False,
        )
    except FileNotFoundError:
        return [], "no symbols in the report and addr2line is not installed"
    # `addr2line -f` alternates NAME then FILE:LINE. Taking every line put paths in the frame
    # list -- harmless for matching and wrong in the count this tool reports.
    lines = out.stdout.splitlines()
    names = [line.split("(")[0].strip() for line in lines[::2] if line and not line.startswith("?")]
    resolved = [n for n in dict.fromkeys(names) if n and not n.startswith(("__", "_ZN"))]
    return resolved, ("symbolised from offsets" if resolved else "offsets resolved to nothing")


def classify(log: str, entries: list[dict], binary: Path | None, target: str | None):
    """Return (verdict, frames, matching entries, how the frames were obtained)."""
    verdict_match = VERDICT.search(log)
    if not verdict_match:
        raise Problem(
            "no sanitiser verdict in this log: it is not a crash report. A timeout, an OOM or a "
            "build failure is the caller's to handle, not this tool's."
        )
    verdict = normalise_verdict(verdict_match.group(1))
    frames, how = frames_in(log, binary, target)
    matches = [e for e in entries if e["verdict"] == verdict and e["frame"] in frames]
    return verdict, frames, matches, how


def check_issues(entries: list[dict]) -> int:
    """Every entry names an issue that is still open. An entry is a claim that expires."""
    problems: list[str] = []
    read = 0
    for issue in sorted({e["issue"] for e in entries}):
        try:
            out = subprocess.run(
                ["gh", "issue", "view", str(issue), "--json", "state", "-q", ".state"],
                capture_output=True, text=True, check=False, cwd=REPO,
            )
        except FileNotFoundError:
            raise Problem(
                "gh is not installed here, so no entry's issue could be checked. This fails "
                "rather than passing: an unverified ledger is exactly what it exists to prevent."
            ) from None
        state = out.stdout.strip()
        if out.returncode != 0:
            problems.append(f"#{issue}: could not be read ({out.stderr.strip()[:80]})")
            continue
        read += 1
        if state != "OPEN":
            problems.append(
                f"#{issue} is {state}: the defect is fixed, so its crash must stop being "
                f"expected. Remove its entries from fuzz/known-crashes.toml -- a ledger that "
                f"outlives its issues is the thing that absorbs the next finding."
            )

    # REPORTS WHAT WAS READ, not what was attempted. The previous line printed "2 issue(s)
    # checked" even when every read had failed.
    print(f"check-known-crashes: {read} of {len({e['issue'] for e in entries})} issue(s) read, "
          f"{len(entries)} entry(ies)")
    if problems:
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 1
    print("OK -- every entry names an issue that is still open.")
    return 0


#: A ledger of its OWN, so the probes test the mechanism rather than today's contents. Coupling
#: them to the real file meant that removing an entry -- which is exactly what must happen when
#: an issue closes -- broke the probe gate and reported every crash as unmatched. The rule a
#: probe exists to protect must not be the rule it depends on.
#:
#: The two entries below are deliberately the hard case: the SAME frame under two verdicts, which
#: is the real relationship between #62's page-tree use-after-free and #119's stack overflow.
PROBE_LEDGER = [
    {"issue": 1, "verdict": "heap-use-after-free", "frame": "Shared::Frame", "note": "x"},
    {"issue": 2, "verdict": "stack-overflow", "frame": "Shared::Frame", "note": "x"},
    {"issue": 3, "verdict": "heap-use-after-free", "frame": "Other::Frame", "note": "x"},
]

#: Each rule against a report it must classify and one it must not.
PROBES = [
    ("a distinct frame matches its own entry",
     "ERROR: AddressSanitizer: heap-use-after-free on address 0x1\n"
     "    #0 0x1 in Other::Frame(Thing)\n", 3),
    ("a shared frame under one verdict",
     "ERROR: AddressSanitizer: heap-use-after-free on address 0x1\n"
     "    #3 0x1 in Shared::Frame(Thing)\n", 1),
    ("THE SAME FRAME under a different verdict is a DIFFERENT defect",
     "ERROR: AddressSanitizer: stack-overflow on address 0x1\n"
     "    #7 0x1 in Shared::Frame(Thing)\n", 2),
    ("an UPPERCASE verdict is not dropped",
     "ERROR: AddressSanitizer: SEGV on unknown address 0x0\n"
     "    #0 0x1 in Other::Frame(Thing)\n", None),
]

#: A report that must match NOTHING: a real defect we do not own yet.
UNKNOWN = (
    "ERROR: AddressSanitizer: heap-buffer-overflow on address 0x1\n"
    "    #0 0x1 in Something::New(Thing)\n"
)


def probe() -> int:
    """The mechanism, against a fixture ledger. Runs on every invocation."""
    problems: list[str] = []
    for name, report, want_issue in PROBES:
        _, _, matches, _ = classify(report, PROBE_LEDGER, None, None)
        issues = {m["issue"] for m in matches}
        expected = {want_issue} if want_issue is not None else set()
        if issues != expected:
            problems.append(f"{name}: matched {issues or 'nothing'}, expected {expected or 'nothing'}")
    _, _, matches, _ = classify(UNKNOWN, PROBE_LEDGER, None, None)
    if matches:
        problems.append(
            f"a crash in an unrelated function matched {[m['issue'] for m in matches]}; "
            f"this ledger would absorb a new finding"
        )
    print(f"check-known-crashes: {len(PROBES)} known shape(s) classified to their own issue, "
          f"1 unknown shape matched nothing")
    if problems:
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 1
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--log", help="a fuzzing log to classify")
    parser.add_argument("--binary", help="the fuzz target binary, to symbolise raw offsets")
    parser.add_argument("--target", help="the target's name, for matching its offsets")
    parser.add_argument("--ledger", help="use this ledger instead of fuzz/known-crashes.toml")
    parser.add_argument("--check", action="store_true", help="validate the ledger, offline")
    parser.add_argument("--check-issues", action="store_true", help="every entry's issue is open")
    args = parser.parse_args()

    try:
        if probe() != 0:
            return 1
        entries = load(Path(args.ledger) if args.ledger else None)
        if args.check:
            print(f"OK -- {len(entries)} ledger entry(ies), each naming an issue and a reason.")
            return 0
        if args.check_issues:
            return check_issues(entries)
        if not args.log:
            parser.error("one of --log, --check or --check-issues is required")

        log = Path(args.log).read_text(encoding="utf-8", errors="replace")
        binary = Path(args.binary) if args.binary else None
        verdict, frames, matches, how = classify(log, entries, binary, args.target)
    except Problem as exc:
        print(f"::error::{exc}", file=sys.stderr)
        return 1

    print(f"verdict: {verdict}")
    print(f"frames considered: {len(frames)} ({how})")

    if matches:
        owners = ", ".join(sorted({f"#{m['issue']}" for m in matches}))
        print(f"KNOWN -- owned by {owners} ({matches[0]['frame']}).")
        # THE RESIDUAL, WHERE THE READER MEETS IT. A green nightly's summary is the one place a
        # person sees this conclusion, so the limit of the evidence belongs beside it and not
        # only in an ADR.
        print(
            "  This means the report is CONSISTENT WITH that defect -- the same verdict in the "
            "same function -- not that it is that defect. A second defect in the same function "
            "under the same verdict would be absorbed here."
        )
        return 0

    # NOT A FINDING IF NOTHING WAS RESOLVED. With no frames, no entry can match, and calling that
    # a new defect announces a five-nights-old one as new. "I could not classify this" is the
    # honest verdict; the exit code is 1 either way, so nothing is absorbed.
    if not frames:
        print(
            f"UNCLASSIFIED -- a {verdict} crash whose frames could not be resolved ({how}), so "
            f"it could be matched against nothing. This is NOT evidence of a new defect; it is "
            f"this tool unable to do its job. Fix the symbols, then re-read.",
            file=sys.stderr,
        )
        return 1

    print(
        f"UNMATCHED -- {verdict} in a place no filed issue owns. This is a NEW finding; that is "
        f"what this nightly is for.\n"
        f"  frames: {', '.join(frames[:6])}",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
