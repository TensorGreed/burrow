#!/usr/bin/env python3
r"""Refuse any code that treats a `qpdf_oh` handle as an object's identity.

WHY THIS EXISTS

`qpdf_oh` is a cache key, not an object. From `qpdf-c.cc`:

    qpdf_oh oh = ++qpdf->next_oh;
    qpdf->oh_cache[oh] = qoh;

Every call that yields a handle allocates a **fresh** id, so two handles to the same object
never compare equal. Asking `qpdf_get_page_n` for page 3 twice gives two different numbers
naming one page.

`reorder` compared `current.raw() == wanted_page.raw()` to ask "is this page already where it
belongs?". The comparison was false for every page including the ones that had not moved, so
every permutation removed a page and tried to insert it immediately before itself, and qpdf
returned `qpdf_e_pages` for all of them -- the identity permutation included. The code looked
exactly like the correct code.

The identity of a PDF object is its **object number and generation**
(`qpdf_oh_get_object_id` + `qpdf_oh_get_generation`), which is what `ObjectHandle::object`
returns. Two operations still to be written depend on this being right rather than plausible:

  - `split`'s pruning decides which objects an output may carry. A comparison that is always
    false prunes nothing, which is a LEAK, and the output is a valid PDF either way.
  - `redaction` decides which objects carry the content being removed. The same mistake there
    is the failure this project treats most seriously.

Neither of those fails loudly the way `reorder` did. So the rule is a check rather than a
paragraph: ADR 0013 and `core/CLAUDE.md` state it, and this refuses the code shape.

WHAT IT SCANS

Rust that actually deals in qpdf object handles -- a file under a `qpdf/` directory, or one
mentioning `ObjectHandle` or `qpdf_oh`. Scoped that way rather than over all of `core/`
because `.handle` is also a PDFium document pointer in `web/pdfium.rs`, where comparing two
is meaningful. The selected files are listed by name, so a run that examined the wrong set is
visible rather than inferred from a healthy-looking total.

ESCAPE HATCH

A line ending `// handle-identity-ok: <reason>` is accepted, and the reason is required and
printed. There is no way to silence a rule without saying why in the diff.

Usage: tools/check-handle-identity.py [file ...]
"""

from __future__ import annotations

import pathlib
import re
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent

# The qpdf functions that ISSUE a handle -- an explicit list, not `qpdf_oh_*`. Most of that
# family RETURNS something that is not a handle: `qpdf_oh_get_type_code(..) == QPDF_TT_INTEGER`
# and `qpdf_oh_get_int_value(..) == 90` are both correct code, and a rule that flagged them
# would be turned off, which is the same outcome as not having the rule.
ISSUING = "|".join(
    (
        "qpdf_get_page_n",
        "qpdf_get_root",
        "qpdf_get_trailer",
        "qpdf_oh_get_key",
        "qpdf_oh_get_array_item",
        "qpdf_oh_new_integer",
        "qpdf_oh_new_name",
        "qpdf_oh_new_dictionary",
        "qpdf_oh_new_array",
    )
)

# Anything that looks like a raw handle: `x.raw()`, `self.handle`, `page.handle`, or a call to
# one of the issuing functions above compared where it stands.
HANDLE = (
    r"(?:[A-Za-z_][A-Za-z0-9_]*\.raw\(\)"
    r"|(?:self|[a-z_][a-z0-9_]*)\.handle\b"
    rf"|(?:ffi::)?(?:{ISSUING})\([^;]*?\))"
)

# A handle-issuing call is usually wrapped: `unsafe {{ ffi::qpdf_get_page_n(d, 3) }}`. The
# closing braces sit between the call and the operator, and without this the in-place
# comparison -- the most direct spelling of the bug there is -- went unflagged. Found by this
# check's own self-test, before it was wired into CI.
GAP = r"[\s})]*"

# The accessor alone, for the binding rule below. NOT the issuing calls: a raw `qpdf_oh` from
# `qpdf_get_page_n` has to be bound to be passed on -- that is the ordinary shape in
# `examples/measure-*.rs`, which deal in the C API directly and own no `ObjectHandle`. What the
# binding rule is about is `ObjectHandle::raw`, whose rustdoc says the value is handed to one
# call and never stored. Narrowed after the broad version reported three of those examples;
# a rule that flags correct code gets turned off, which is the same outcome as not having it.
ACCESSOR = r"(?:[A-Za-z_][A-Za-z0-9_]*\.raw\(\)|(?:self|[a-z_][a-z0-9_]*)\.handle\b)"

# A rule is (name, compiled pattern, a line it MUST match, a line it MUST NOT match, message).
# The fixture and the near-miss are checked on every invocation -- a rule that matches nothing
# passes everything, and that is not visible from a green run.
RULES: list[tuple[str, re.Pattern[str], str, str, str]] = [
    (
        "compared",
        re.compile(
            rf"{HANDLE}{GAP}[=!]=|[=!]={GAP}{HANDLE}"
            rf"|assert(?:_eq|_ne)!\s*\([^;]*{HANDLE}"
        ),
        "if current.raw() == wanted_page.raw() {",
        "if current.object() == wanted_page.object() {",
        "a raw handle is compared for equality. qpdf issues a fresh handle per call, so this "
        "is false even for two handles to the same object. Compare `.object()`, which is the "
        "object number and generation.",
    ),
    (
        "searched",
        re.compile(
            rf"\.(?:contains|contains_key|get|get_mut|entry|position|find|any|all|"
            rf"binary_search|remove|retain)\s*\([^;]*{HANDLE}"
        ),
        "if seen.contains(&page.raw()) {",
        "if seen.contains(&page.object()) {",
        "a raw handle is used as a needle in a search. Equality on handles is equality on "
        "cache keys, so the search can only ever find a handle that is literally the same "
        "value. Search on `.object()`.",
    ),
    (
        "collected",
        re.compile(
            rf"\.(?:insert|push|extend|append|dedup_by_key|sort_by_key|max_by_key|"
            rf"min_by_key)\s*\([^;]*{HANDLE}|{HANDLE}\s*\.\s*(?:cmp|partial_cmp|eq|ne)\s*\("
        ),
        "kept.insert(page.raw());",
        "kept.insert(page.object());",
        "a raw handle is put into a set, or a collection is deduplicated or sorted by one. "
        "Every element is distinct by construction, so the set never collapses duplicates "
        "and the sort orders by call order. Store `.object()`.",
    ),
    (
        "bound",
        re.compile(
            rf"\blet\s+(?:mut\s+)?[A-Za-z_][A-Za-z0-9_]*\s*(?::[^=;]+)?=\s*{ACCESSOR}\s*;"
        ),
        "let mine = page.raw();",
        "let mine = ffi::qpdf_get_page_n(d, 3);",
        "a raw handle is bound to a local. `ObjectHandle::raw` is documented as handing the "
        "value to qpdf for the duration of one call and never storing it -- and once it is in "
        "a variable, every rule below is one refactor away from being invisible: "
        "`let a = x.raw(); let b = y.raw(); if a == b` is the original defect with a `let` in "
        "front of it. Pass `.raw()` directly to the call that needs it, or bind `.object()`.",
    ),
    (
        "matched",
        re.compile(rf"matches!\s*\(\s*{HANDLE}"),
        "matches!(page.raw(), other)",
        "matches!(page.object(), other)",
        "a raw handle is pattern-matched against another value. Same reason as an equality "
        "comparison: the handle is a cache key, not the object.",
    ),
]

ALLOW = re.compile(r"//\s*handle-identity-ok:\s*(?P<reason>\S.*)$")


def probe() -> list[str]:
    """Every rule matches its own fixture and rejects its near-miss. Every run."""
    problems = []
    for name, pattern, fixture, near_miss, _ in RULES:
        print(f"  probe {name}", end="")
        if not pattern.search(fixture):
            problems.append(
                f"rule {name!r} does not match its own fixture {fixture!r}: it would report "
                f"a clean tree whatever the tree contained"
            )
            print(" — BROKEN (matches nothing)")
            continue
        if pattern.search(near_miss):
            problems.append(
                f"rule {name!r} also matches its near-miss {near_miss!r}, which is the "
                f"CORRECT spelling: it would report every correct file as a violation"
            )
            print(" — BROKEN (matches the correct code)")
            continue
        print(" ok")
    return problems


def files_to_scan(argv: list[str]) -> list[pathlib.Path]:
    if argv:
        return [pathlib.Path(a).resolve() for a in argv]
    # `--others --exclude-standard` as well as the index, and that was a correction the
    # check's own first run made. Scanning only tracked files left `qpdf/reorder.rs` out of
    # the list -- the file the rule was written FROM, still untracked on its own branch. A
    # check that cannot see the code being written is a check that arrives one commit late.
    try:
        tracked = subprocess.run(
            ["git", "ls-files", "--cached", "--others", "--exclude-standard", "*.rs"],
            cwd=REPO,
            capture_output=True,
            text=True,
            check=True,
        ).stdout.split()
    except (OSError, subprocess.CalledProcessError):
        tracked = [str(p.relative_to(REPO)) for p in REPO.glob("**/*.rs")]

    selected = []
    for rel in tracked:
        path = REPO / rel
        try:
            text = path.read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        # `ISSUING` too, not only `qpdf_oh`/`ObjectHandle`. A file that calls
        # `qpdf_get_page_n` through the web bridge and compares the results need never write
        # either word -- and `web/qpdf.rs` is exactly the file that grows a `PageReorderer`
        # next. Found by security review.
        if (
            "/qpdf/" in rel
            or "ObjectHandle" in text
            or "qpdf_oh" in text
            or any(name in text for name in ISSUING.split("|"))
        ):
            selected.append(path)
    return sorted(selected)


def scan(path: pathlib.Path) -> tuple[list[str], list[str]]:
    """Findings and accepted exemptions in one file."""
    findings: list[str] = []
    allowed: list[str] = []
    try:
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError as exc:
        return [f"{path}: cannot read: {exc}"], []

    rel = path.relative_to(REPO) if path.is_relative_to(REPO) else path
    for number, line in enumerate(lines, start=1):
        # A comment line cannot BE a violation -- this file and every rustdoc note about the
        # rule quote the wrong spelling on purpose, and flagging those would make the check
        # unrunnable against its own documentation.
        stripped = line.lstrip()
        if stripped.startswith("//") and not ALLOW.search(line):
            continue
        for name, pattern, _, _, message in RULES:
            if not pattern.search(line):
                continue
            waiver = ALLOW.search(line)
            if waiver:
                allowed.append(f"{rel}:{number} [{name}] — {waiver.group('reason')}")
            else:
                findings.append(f"{rel}:{number} [{name}] {message}\n      {stripped}")
    return findings, allowed


def main(argv: list[str]) -> int:
    print("checking that no qpdf object handle is used as an identity")
    broken = probe()
    if broken:
        print("\nFAILED — the checker's own rules are broken:", file=sys.stderr)
        for problem in broken:
            print(f"  - {problem}", file=sys.stderr)
        return 1

    files = files_to_scan(argv)
    if not files:
        print(
            "error: no file mentions ObjectHandle or qpdf_oh, so this run examined nothing",
            file=sys.stderr,
        )
        return 1

    findings: list[str] = []
    allowed: list[str] = []
    for path in files:
        f, a = scan(path)
        findings.extend(f)
        allowed.extend(a)

    # BY NAME, not merely a count. The set is selected by content, so a total on its own
    # cannot distinguish a correct 9 from a resolver that found the wrong nine.
    print(f"\nexamined {len(files)} file(s) dealing in qpdf object handles:")
    for path in files:
        rel = path.relative_to(REPO) if path.is_relative_to(REPO) else path
        print(f"  {rel}")

    if allowed:
        print(f"\n{len(allowed)} argued exemption(s):")
        for note in allowed:
            print(f"  {note}")

    if findings:
        print(f"\nFAILED — {len(findings)} handle-identity violation(s):", file=sys.stderr)
        for finding in findings:
            print(f"  - {finding}", file=sys.stderr)
        print(
            "\n  A `qpdf_oh` is a cache key that is fresh on every call, not an object.\n"
            "  Identity is object number + generation: use `ObjectHandle::object()`.\n"
            "  See docs/adr/0013-qpdf-c-api-and-prescan.md and core/CLAUDE.md.",
            file=sys.stderr,
        )
        return 1

    print(f"\nOK — {len(RULES)} rule(s), no handle compared as an identity.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
