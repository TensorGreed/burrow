#!/usr/bin/env python3
"""Every proptest regression seed names the defect it records.

    python3 tools/check-proptest-regressions.py

WHY THIS EXISTS

A `*.proptest-regressions` file is a list of inputs proptest will re-run before it generates
anything new. Each line is a claim: *this input once broke this property.* proptest's own header
recommends checking the file in, and it is right to — a seed that reproduces a real defect is the
cheapest regression test there is.

A seed that reproduces NOTHING is a different object with the same shape. It is a false record:
it costs time on every run, it reads as evidence of a defect that was never there, and nobody can
tell the two apart by looking.

That is not hypothetical here. Twice a seed has been written by a test failing for an
ENVIRONMENTAL reason — the `qpdf` CLI absent from the shell's PATH, which `tools/ci-local.py`
supplies and a bare `cargo test` does not — and twice it was nearly committed. The test was fine.
The property was fine. The only thing the seed records is that a binary was missing.

THE RULE

Every `cc <hash>` line must be immediately preceded by a `#` comment that names the defect.
proptest's six-line boilerplate header does not count, and the check knows its text.

WHY A COMMENT IN THE FILE, NOT A COMMIT MESSAGE

The first design required the commit that adds a seed to name the defect. That is weaker in two
ways. A commit message is read once, by a reviewer who already knows; the file is read later, by
somebody wondering why a case is being re-run. And a commit-message rule needs a merge base,
which makes the check a property of a diff rather than of the tree — `CLAUDE.md` records why that
matters for `check-no-generated-files.sh`: a whole-tree scan needs no merge base and stays red
until the thing is actually fixed, rather than passing on the next commit.

IT REPORTS WHAT IT EXAMINED

Per `CLAUDE.md`: the count of files and of seeds, not a bare OK. Zero seeds is a legitimate state
and is reported as such — an empty scan that printed the same "OK" as a real one would be the
shape this file exists to refuse, one level up.
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# THE HEADER PROPTEST ACTUALLY WRITES, byte for byte, from `write_header` in proptest 1.11.0's
# `src/test_runner/failure_persistence/file.rs:300-311`. Five lines, one of them bare `#`, and
# `writeln!` means a seed follows IMMEDIATELY -- there is no blank line between.
#
# THAT LAST FACT IS WHY THIS IS COPIED EXACTLY RATHER THAN ABRIDGED. The first version's probe
# fixture used the first three lines, so the line above a real first seed -- "everyone who runs
# the test benefits from these saved cases" -- was never exercised. Deleting the last two
# entries below left every probe and the whole self-test green while the check ACCEPTED the
# byte-exact file proptest emits. Measured by code review, and it is the same defect as the one
# this file's `(rule, message)` fix was for, one level down: the probes measured a fixture the
# producer never writes.
PROPTEST_HEADER = """\
# Seeds for failure cases proptest has generated in the past. It is
# automatically read and these particular cases re-run before any
# novel cases are generated.
#
# It is recommended to check this file in to source control so that
# everyone who runs the test benefits from these saved cases.
"""

# Each line of it, as `is_boilerplate` sees them. Derived from the header rather than retyped,
# so the two cannot drift.
BOILERPLATE = tuple(
    line.lstrip("#").strip().lower() for line in PROPTEST_HEADER.strip().splitlines()
)

# Short enough to write by accident, long enough to be a sentence.
MIN_EXPLANATION = 25


def is_boilerplate(comment: str) -> bool:
    """Whether `comment` is one of proptest's own header lines, and so explains nothing."""
    text = comment.lstrip("#").strip().lower()
    return not text or text in BOILERPLATE


def problems_in(path: Path, text: str) -> list[tuple[str, str]]:
    """Every seed in `text` that names no defect, as `(rule, message)`.

    THE RULE IS RETURNED, NOT JUST THE VERDICT, and that is what makes the probes below able to
    fail. The first version returned messages only, so a probe could assert "this is caught"
    and be satisfied by a DIFFERENT rule catching it — breaking the no-comment branch left the
    bare-seed case caught by the boilerplate branch, and every probe stayed green over a rule
    that no longer ran. Found by this checker's own self-test on its first run, which is the
    same defect a code review had just found in `web/handle.rs`'s scan: overlapping probes
    measure the disjunction, not the rules.
    """
    found: list[tuple[str, str]] = []
    lines = text.splitlines()
    for index, line in enumerate(lines):
        if not line.strip().startswith("cc "):
            continue
        # The nearest non-blank line above.
        above = ""
        for previous in reversed(lines[:index]):
            if previous.strip():
                above = previous.strip()
                break
        seed = line.strip()[:34]
        if not above.startswith("#"):
            found.append(
                ("no-comment", f"{path}: `{seed}…` has no comment above it saying what it "
                 "reproduces")
            )
        elif is_boilerplate(above):
            found.append(
                ("boilerplate", f"{path}: `{seed}…` is preceded only by proptest's own header, "
                 "which explains nothing")
            )
        elif len(above.lstrip("#").strip()) < MIN_EXPLANATION:
            found.append(
                ("too-short", f"{path}: `{seed}…` is preceded by `{above}`, which is too short "
                 "to name a defect")
            )
    return found


def probe_the_rule() -> tuple[list[str], int]:
    """Each rule is exercised on inputs it must catch and must not, before the tree is read.

    # Every case is a shape proptest actually produces

    The first version's fixtures were abridged and the abridgement was where the holes were.
    Code review planted three mutations that left all six probes green while the check accepted
    real files:

    * two `BOILERPLATE` entries deleted — the line above a real FIRST seed is the last header
      line, which no fixture reached;
    * `above.startswith("#")` weakened to `above` — the line above a real SECOND seed is the
      first seed's own `cc` line, which no fixture had;
    * seed recognition narrowed to lines containing `# shrinks to` — proptest's CI message tells
      you to paste a BARE `cc <hash>`, which no fixture used.

    So the fixtures below are built from [`PROPTEST_HEADER`] and cover each of those shapes.
    """
    failures: list[str] = []
    seed = "cc 914a04fd01b8ba60dd38bae4a2a3cfde42b338f630aed3a9a598a9794a7d2095 # shrinks to n = 2\n"
    # WHAT PROPTEST TELLS YOU TO PASTE after a CI failure: no `# shrinks to` suffix at all.
    bare_seed = "cc 4444444444444444444444444444444444444444444444444444444444444444\n"
    explained = "# #112: reorder dropped the last page when the count was even.\n"

    cases: list[tuple[str, str, str | None]] = [
        ("a seed with nothing above it", seed, "no-comment"),
        ("a seed directly under proptest's header", PROPTEST_HEADER + seed, "boilerplate"),
        (
            "a SECOND seed, under the first seed's own line",
            PROPTEST_HEADER + explained + seed + bare_seed,
            "no-comment",
        ),
        (
            "a bare `cc` line with no `# shrinks to` suffix",
            PROPTEST_HEADER + bare_seed,
            "boilerplate",
        ),
        ("a seed under a too-short comment", "# flaky\n" + seed, "too-short"),
        # THE NEAR-MISS, one byte under the floor, and its twin one byte over. Without the pair
        # the floor could be any number at all: the old probes were 5 and 60.
        ("a seed one byte under the floor", "# " + "a" * 24 + "\n" + seed, "too-short"),
        ("a seed exactly at the floor", "# " + "a" * 25 + "\n" + seed, None),
        ("a seed that names its defect", explained + seed, None),
        ("a seed that names its defect, under the header", PROPTEST_HEADER + explained + seed, None),
        ("a file with no seeds at all", PROPTEST_HEADER, None),
    ]

    # ONE POSITIVE PER BOILERPLATE LINE. Each header line must be recognised as explaining
    # nothing, so deleting any entry fails here rather than silently widening what passes.
    for index, line in enumerate(BOILERPLATE):
        cases.append(
            (f"a seed under header line {index} ({line[:32]!r})", f"# {line}\n" + seed, "boilerplate")
        )

    for name, text, expected in cases:
        rules = [rule for rule, _ in problems_in(Path("probe"), text)]
        if expected is None:
            if rules:
                failures.append(f"rule probe: refused {name}, which it must not: {rules}")
        elif expected not in rules:
            failures.append(
                f"rule probe: {name} should be caught by rule `{expected}` and was caught by "
                f"{rules or 'nothing'}"
            )

    # AND THE PROBE SET COVERS EVERY BOILERPLATE LINE, gated on the count rather than trusted.
    # A `BOILERPLATE` entry added without a probe is the hole this whole block is about.
    per_line = sum(1 for name, _, _ in cases if name.startswith("a seed under header line "))
    if per_line != len(BOILERPLATE):
        failures.append(
            f"rule probe: {per_line} per-line probe(s) for {len(BOILERPLATE)} boilerplate line(s)"
        )
    return failures, len(cases)


def main() -> int:
    probe_failures, probes = probe_the_rule()
    if probe_failures:
        sys.stderr.write("check-proptest-regressions: THE RULE ITSELF FAILED\n")
        for failure in probe_failures:
            sys.stderr.write(f"  - {failure}\n")
        return 1

    # `-z`, so a path with a space in it is one path rather than two nonexistent ones.
    listing = subprocess.run(
        ["git", "ls-files", "-z", "*.proptest-regressions"],
        cwd=REPO,
        capture_output=True,
        text=True,
        check=True,
    )
    files = [REPO / name for name in listing.stdout.split("\0") if name]

    problems: list[str] = []
    seeds = 0
    for path in files:
        text = path.read_text(encoding="utf-8")
        seeds += sum(1 for line in text.splitlines() if line.strip().startswith("cc "))
        problems += [message for _, message in problems_in(path.relative_to(REPO), text)]

    # BY NAME, not by count. `CLAUDE.md`: where the expected count is not knowable, say what
    # was examined. A bare "0 files" cannot be told from a broken glob by anyone reading it.
    print(
        f"check-proptest-regressions: {len(files)} file(s), {seeds} seed(s); "
        f"{probes} rule probe(s) passed"
    )
    for path in files:
        print(f"    {path.relative_to(REPO)}")
    # FLUSHED, or the ordering above is a lie: stdout is block-buffered and stderr is not, so
    # the failure block below overtakes the summary and reads as if it described something else.
    sys.stdout.flush()

    if problems:
        # THE SUMMARY FIRST, then the detail: stdout and stderr interleave in a CI log, and the
        # count arriving after the failures reads as if it described something else.
        sys.stderr.write(f"\nFAILED — {len(problems)} seed(s) record no defect:\n")
        for problem in problems:
            sys.stderr.write(f"  - {problem}\n")
        sys.stderr.write(
            "\n  A seed with nothing behind it costs time on every run and reads as evidence of\n"
            "  a defect that was never there. Twice one has been written by a test failing\n"
            "  because the `qpdf` CLI was absent from PATH. Put the reason above the seed, or\n"
            "  delete the seed.\n"
        )
        return 1
    if not files:
        # A DIFFERENT SENTENCE, because the same one would be a claim about seeds that do not
        # exist -- and today this is the only path the check takes. The docstring says an empty
        # scan must not print what a real one prints; it did.
        print("OK — no regression seeds are tracked, so there is nothing to name.")
        return 0
    print("OK — every regression seed names the defect it records.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
