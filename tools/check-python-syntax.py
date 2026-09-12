#!/usr/bin/env python3
"""Compile every tool in tools/ with SyntaxWarning promoted to an error.

WHY THIS EXISTS

M1 PR 4c shipped `tools/check-qpdf-trapped.py` with a regex in a non-raw docstring, so
every run printed:

    SyntaxWarning: invalid escape sequence '\\.'

Nothing failed. It printed to stderr in a CI job that was otherwise green, which is the
worst place for a warning to live: loud enough to be noise, quiet enough to be ignored, and
in a *security-critical* checker where noise is exactly what stops people reading output.
Python has been escalating this class for several releases and it becomes a `SyntaxError`
in a future version, so the eventual failure is a build that breaks on an interpreter
upgrade rather than on the commit that caused it.

So the class fails now, on the commit that introduces it.

**`compile()` rather than `py_compile`, deliberately.** `py_compile` writes a `.pyc`, and a
stray `.pyc` is how this repository got an opaque binary committed to `main` (see the
`__pycache__/` note in `.gitignore`). Compiling the source text in memory writes nothing at
all, which is the right shape for a check that runs in a working tree.

This file checks itself along with the rest.

Usage: tools/check-python-syntax.py [file ...]
With no arguments, checks every *.py under tools/.
"""

from __future__ import annotations

import pathlib
import subprocess
import sys
import warnings

REPO = pathlib.Path(__file__).resolve().parent.parent
TOOLS = REPO / "tools"


def uncovered_tracked_python(files: list[pathlib.Path]) -> list[str]:
    """Every tracked `.py` in the repository must be in the set being checked.

    The default glob is `tools/*.py` -- non-recursive, and one directory. That covers
    everything today, and it would silently stop covering `tools/sub/x.py` or `fuzz/foo.py`
    the moment either existed. `CLAUDE.md` names that shape directly: *a check that silently
    examines nothing is worse than no check, because it reads as coverage.* So the glob is
    compared against what git actually tracks, and a file outside it is a failure rather than
    an absence.

    Falls back to no finding when git is unavailable (a source tarball, a container without
    it) rather than failing -- the check itself still runs over the glob.
    """
    try:
        out = subprocess.run(
            ["git", "ls-files", "*.py"],
            cwd=REPO,
            capture_output=True,
            text=True,
            check=True,
        ).stdout
    except (OSError, subprocess.CalledProcessError):
        return []

    checked = {p.resolve() for p in files}
    missing = sorted(
        line for line in out.splitlines() if line and (REPO / line).resolve() not in checked
    )
    return [
        f"{path}: tracked Python that this check does not cover. "
        f"Widen the glob in tools/check-python-syntax.py, or pass it explicitly."
        for path in missing
    ]


def main(argv: list[str]) -> int:
    if argv:
        files = [pathlib.Path(a).resolve() for a in argv]
    else:
        files = sorted(TOOLS.glob("*.py"))

    if not files:
        print("error: no Python files to check; this run would be vacuous", file=sys.stderr)
        return 1

    problems: list[str] = []
    if not argv:
        problems.extend(uncovered_tracked_python(files))

    for path in files:
        try:
            # BYTES, not text. `read_text()` decodes with the locale encoding and discards
            # the PEP 263 coding cookie, so a file declaring its own encoding would be read
            # wrongly; `compile()` honours the cookie itself. A decode error is also a
            # ValueError rather than an OSError, so the text path could traceback here.
            source = path.read_bytes()
        except OSError as exc:
            problems.append(f"{path}: cannot read: {exc}")
            continue

        # Every warning raised during compilation becomes an exception, so a file cannot
        # pass by emitting a warning nobody reads. `error` rather than `error::SyntaxWarning`
        # because anything the compiler chooses to warn about in a checker is worth seeing --
        # and the set is small enough that a false positive is a conversation, not a burden.
        with warnings.catch_warnings():
            warnings.simplefilter("error")
            try:
                compile(source, str(path), "exec")
            except SyntaxError as exc:
                # THE ESCALATED WARNING ARRIVES HERE, NOT IN A SyntaxWarning HANDLER.
                # CPython converts an escalated SyntaxWarning into a SyntaxError inside
                # `compile()` before it can propagate, so a `except SyntaxWarning` arm is
                # dead code -- measured on 3.12: `compile('x = "\\."')` under
                # `simplefilter("error")` raises SyntaxError, not SyntaxWarning. The
                # remediation hint lives here for that reason; in the first version it sat
                # in an arm that never ran.
                rel = path.relative_to(REPO) if path.is_relative_to(REPO) else path
                hint = ""
                if "escape sequence" in (exc.msg or ""):
                    hint = (
                        "\n      Usually a regex or a Windows path in a non-raw string. "
                        "Prefix the literal with `r`."
                    )
                problems.append(f"{rel}:{exc.lineno}: {exc.msg}{hint}")
            except Warning as exc:
                rel = path.relative_to(REPO) if path.is_relative_to(REPO) else path
                problems.append(f"{rel}: {type(exc).__name__}: {exc}")

    print(f"tools/: compiled {len(files)} Python file(s) with warnings as errors")

    if problems:
        print(f"\nFAILED — {len(problems)} problem(s):", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 1

    print("OK — no syntax warnings.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
