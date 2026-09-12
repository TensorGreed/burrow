#!/usr/bin/env python3
"""Check every qpdf C function burrow declares against qpdf's own `trap_errors` set.

WHY THIS EXISTS
===============

`qpdf-c.h:113-115` reads like a blanket guarantee that C++ exceptions never escape the C
API. It is not true per-function. The catching is done by one static helper, `trap_errors`
(`libqpdf/qpdf-c.cc`), and only functions routed through it are covered.

`qpdf_is_linearized` is not one of them: it is a bare `return qpdf->qpdf->isLinearized()`,
which calls `QIntC::to_int` on an object number and throws `std::range_error` above
`INT_MAX`. An ordinary 356-byte PDF containing `9999999999 0 obj` therefore kills the
process:

    fatal runtime error: Rust cannot catch foreign exceptions, aborting

Measured end to end during M1 PR 3's review. `catch_unwind` is no help -- a foreign
exception is not a Rust panic. On the web that is the worker; on mobile, the app.

So ADR 0013 §1 set the rule: **call only functions verified to route through
`trap_errors`**, verified by reading `qpdf-c.cc` function by function rather than by
reading the header's prose. That verification was done by hand, once, by a reviewer. This
script is what makes it repeatable, and what makes a version bump re-run it.

The lesson generalises past qpdf and is worth keeping in view: a header's prose described
a guarantee its implementation provides per-function. The reviewer read the `.cc`; the
author read the `.h`. **For any FFI boundary where an exception could cross, the source is
the contract.**

TWO BUCKETS, BECAUSE "EVERYTHING MUST BE TRAPPED" IS NOT THE RULE
=================================================================

burrow declares sixteen qpdf functions and only two of them -- `qpdf_read_memory` and
`qpdf_get_num_pages` -- route through `trap_errors`. The other fourteen are the setters,
the error accessors, the logger calls and `qpdf_init`/`qpdf_cleanup`: they assign fields,
flip flags, or read a stored value, and can fail only by allocation. Since Rust 1.81,
unwinding out of an `extern "C"` frame is a defined abort rather than undefined
behaviour, so ADR 0013 §1 accepts them explicitly.

A check that demanded every declaration be trapped would fail on day one and its only
remedies would be the C++ shim ADR 0013 rejected or dropping fourteen calls the crate
needs. So a declaration must be in exactly one of:

  1. the **generated** trapped set, `engines/qpdf-trapped-functions.txt`; or
  2. the **committed, justified** exemption list,
     `engines/qpdf-untrapped-accepted.toml`, one entry per function with its reason.

Anything in neither fails. A function that *leaves* the trapped set on a version bump also
fails, which is the regression this is really here to catch: upstream could stop routing a
function through the helper without changing its signature, and nothing else would notice.

USAGE
=====

    python3 tools/check-qpdf-trapped.py --generate   # rewrite the trapped set from source
    python3 tools/check-qpdf-trapped.py              # verify declarations against it

`--generate` needs the vendored qpdf source (`engines/fetch.sh`). Plain verification does
not: it reads only committed files, so it runs on a clean checkout and in CI's licence
job. CI runs `--generate` into a temporary file and diffs, in the job that already has the
vendor tree -- a stale list is a failure, not a warning.
"""

from __future__ import annotations

import argparse
import pathlib
import re
import sys
import tomllib

REPO = pathlib.Path(__file__).resolve().parent.parent
PINS = REPO / "engines" / "pins.toml"
TRAPPED_LIST = REPO / "engines" / "qpdf-trapped-functions.txt"
ACCEPTED_LIST = REPO / "engines" / "qpdf-untrapped-accepted.toml"

# Where declarations live. All three are checked, because a function reachable on ONE path
# is reachable, and the web bridge is a separate hand-written surface from the native FFI.
NATIVE_FFI = REPO / "core" / "burrow-engines" / "src" / "qpdf" / "ffi.rs"
WEB_BRIDGE_JS = REPO / "apps" / "web" / "src" / "worker" / "bridge.js"
WEB_TRAIT = REPO / "core" / "burrow-engines" / "src" / "web" / "bridge.rs"
WASM_BINDING = REPO / "bindings" / "burrow-wasm" / "src" / "bridge.rs"


def qpdf_version() -> str:
    """The pinned qpdf version, from the file that is the single source of truth."""
    with PINS.open("rb") as fh:
        return tomllib.load(fh)["qpdf"]["version"]


def qpdf_c_source(version: str) -> pathlib.Path:
    return REPO / "engines" / "vendor" / "src" / f"qpdf-{version}" / "libqpdf" / "qpdf-c.cc"


# A function definition in qpdf-c.cc is formatted with the return type alone on one line and
# the name at column 0 on the next, brace on its own line:
#
#     QPDF_ERROR_CODE
#     qpdf_read_memory(qpdf_data qpdf, ...)
#     {
#
# Anchored at column 0 so a call inside another function's body -- always indented -- can
# never be mistaken for a definition.
DEFINITION = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*)\s*\(", re.MULTILINE)


def trapped_functions(source: str) -> list[str]:
    """Every function in `qpdf-c.cc` whose own body calls `trap_errors`.

    Brace-matched rather than scanned by proximity: `trap_errors` appears inside lambdas,
    inside `if` bodies, and several lines below the call that starts a statement, so
    "the nearest definition above" would be a guess. Walking the braces makes the enclosing
    function an answer rather than an estimate.

    **Direct calls only, deliberately.** A function that reaches `trap_errors` through a
    static helper is not counted, and that is the conservative direction: it can only ever
    omit a function from the trapped set, never add an untrapped one to it. Omitting one
    means somebody must justify it in the exemption list, with a reason a reviewer reads.
    Adding one would mean the check blessed a call that can abort the process.
    """
    found: list[str] = []
    for match in DEFINITION.finditer(source):
        name = match.group(1)
        # `trap_errors` itself contains the token by definition, and control-flow keywords
        # at column 0 are not function definitions.
        if name in {"trap_errors", "if", "for", "while", "switch", "return", "catch"}:
            continue

        # `static` helpers are not part of the C API and cannot be declared by a binding.
        # `trap_oh_errors` is one: it wraps `trap_errors` for the `qpdf_oh_*` family, and
        # listing it would put a name on a list of "functions safe to call" that no caller
        # outside the translation unit can call at all. The `static` keyword sits on the
        # return-type line, immediately above the name.
        line_start = source.rfind("\n", 0, match.start()) + 1
        previous_line = source[source.rfind("\n", 0, line_start - 1) + 1 : line_start]
        if previous_line.strip().startswith("static"):
            continue

        body = brace_body(source, match.end())
        if body is None:
            continue
        if re.search(r"\btrap_errors\s*\(", body):
            found.append(name)

    return sorted(set(found))


def brace_body(source: str, start: int) -> str | None:
    """The `{...}` body of the definition whose parameter list starts at `start`.

    Returns `None` if this is a declaration rather than a definition -- a prototype ends in
    `;` and has no body -- or if the braces do not balance before the file ends.
    """
    # Skip the parameter list.
    depth = 1
    i = start
    while i < len(source) and depth:
        if source[i] == "(":
            depth += 1
        elif source[i] == ")":
            depth -= 1
        i += 1
    if depth:
        return None

    # Whatever sits between the parameter list and the body -- qualifiers, a newline. A `;`
    # here means this was a prototype.
    while i < len(source) and source[i] not in "{;":
        if not source[i].isspace() and source[i] not in "abcdefghijklmnopqrstuvwxyz_":
            return None
        i += 1
    if i >= len(source) or source[i] == ";":
        return None

    body_start = i
    depth = 0
    while i < len(source):
        if source[i] == "{":
            depth += 1
        elif source[i] == "}":
            depth -= 1
            if depth == 0:
                return source[body_start : i + 1]
        i += 1
    return None


def declared_functions() -> dict[str, list[str]]:
    """Every qpdf C function burrow declares, mapped to where it is declared.

    **Native path:** `core/burrow-engines/src/qpdf/ffi.rs`, the `extern "C"` block. Parsed
    with the same `pub(super) fn qpdf…` shape `engines/build-wasm.sh` uses for its
    export-table check, so the two agree by construction rather than by coincidence.

    **Web path:** `apps/web/src/worker/bridge.js`, parsed from the underscore-prefixed
    Emscripten exports it calls (`qpdf()._qpdf_read_memory(...)`). That is deliberately the
    surface chosen, and it is the *narrowest correct* one. The Rust side of the seam --
    `bindings/burrow-wasm/src/bridge.rs` -- declares `__burrow_qpdf_*` imports whose names
    are bridge functions, not qpdf C functions: `__burrow_qpdf_copy_in`,
    `__burrow_qpdf_heap_pages` and friends never enter qpdf's C API at all. Only the JS
    bridge says which `_qpdf_*` module export each one resolves to, so only the JS bridge
    can tell a qpdf call from a bridge helper. `wasm_binding_is_covered()` checks that
    nothing in the Rust half escapes the JS half, which is what lets this stay narrow.

    **`core/burrow-engines/src/web/bridge.rs`** names qpdf functions only in doc comments
    -- it is a Rust trait, not a symbol declaration. Its documented names are read anyway,
    in the exact ``/// `name`.`` form the file uses, because ADR 0013 calls that trait "a
    mirror of `qpdf::ffi`'s `extern \"C\"` block" and a doc comment introducing a function
    nobody cleared is how the mirror stops being one. Prose *about* a function -- the file
    explains at length why `qpdf_is_encrypted` and `qpdf_is_linearized` are absent -- does
    not match, because there the backtick is followed by " and", not ".".
    """
    out: dict[str, list[str]] = {}

    def record(name: str, where: str) -> None:
        out.setdefault(name, []).append(where)

    for name in re.findall(
        r"^\s*pub\(super\) fn (qpdf[a-z_0-9]*)\s*\(", NATIVE_FFI.read_text(), re.MULTILINE
    ):
        record(name, "qpdf/ffi.rs")

    for name in re.findall(r"\._(qpdf[a-z_0-9]*)\s*\(", WEB_BRIDGE_JS.read_text()):
        record(name, "worker/bridge.js")

    for name in re.findall(r"^\s*/// `(qpdf[a-z_0-9]*)`\.", WEB_TRAIT.read_text(), re.MULTILINE):
        record(name, "web/bridge.rs (doc)")

    return {name: sorted(set(where)) for name, where in out.items()}


def wasm_binding_is_covered() -> list[str]:
    """Every `__burrow_qpdf_*` the wasm binding imports must be defined in the JS bridge.

    This is what makes it safe for `declared_functions()` to read the JS bridge alone for
    the web path. The Rust half names bridge functions; the JS half is the only place that
    says which qpdf C export each one actually calls. If a `__burrow_qpdf_*` import existed
    with no JS definition, the web path would be calling something this check never saw --
    and at runtime it would be `undefined`, so the failure would surface as a trap in a
    worker rather than as a licence-of-abstraction problem anyone could read.
    """
    js = WEB_BRIDGE_JS.read_text()
    defined = set(re.findall(r"self\.__burrow_(qpdf[a-z_0-9]*)\s*=", js))
    imported = set(re.findall(r"\b__burrow_(qpdf[a-z_0-9]*)\b", WASM_BINDING.read_text()))

    if not imported:
        return [
            f"parsed ZERO __burrow_qpdf_* imports out of "
            f"{WASM_BINDING.relative_to(REPO)}; this check would be vacuous"
        ]
    return [
        f"{WASM_BINDING.relative_to(REPO)} imports __burrow_{name}, which "
        f"{WEB_BRIDGE_JS.relative_to(REPO)} does not define -- so the web path calls "
        f"something this check cannot see"
        for name in sorted(imported - defined)
    ]


def load_trapped() -> set[str]:
    if not TRAPPED_LIST.is_file():
        sys.exit(
            f"error: {TRAPPED_LIST.relative_to(REPO)} is missing. "
            f"Generate it with --generate (needs engines/fetch.sh to have run)."
        )
    return {
        line.strip()
        for line in TRAPPED_LIST.read_text().splitlines()
        if line.strip() and not line.startswith("#")
    }


def load_accepted() -> dict[str, str]:
    if not ACCEPTED_LIST.is_file():
        sys.exit(f"error: {ACCEPTED_LIST.relative_to(REPO)} is missing")
    with ACCEPTED_LIST.open("rb") as fh:
        data = tomllib.load(fh)
    accepted = {}
    for entry in data.get("function", []):
        name = entry.get("name")
        reason = entry.get("reason", "")
        if not name:
            sys.exit(f"error: {ACCEPTED_LIST.name} has an entry with no `name`")
        if len(reason) < 40:
            sys.exit(
                f"error: {name} is exempted with no real reason. An exemption is an "
                f"argument that an untrapped call is safe; write it out."
            )
        accepted[name] = reason
    return accepted


def generate() -> int:
    version = qpdf_version()
    source_path = qpdf_c_source(version)
    if not source_path.is_file():
        sys.exit(
            f"error: {source_path.relative_to(REPO)} not found. Run engines/fetch.sh first "
            f"-- engines/vendor/ is gitignored and reproducible from engines/pins.toml."
        )

    trapped = trapped_functions(source_path.read_text())
    if len(trapped) < 10:
        # The parser silently returning nothing would produce an empty list that failed
        # every declaration, or -- worse, if it were ever inverted -- passed everything.
        sys.exit(
            f"error: parsed only {len(trapped)} trapped functions out of qpdf-c.cc. "
            f"The parser is broken; a real qpdf has ~24."
        )

    header = f"""\
# qpdf C API functions that route through `trap_errors`, GENERATED. Do not edit by hand.
#
# Source: engines/vendor/src/qpdf-{version}/libqpdf/qpdf-c.cc
# Regenerate: python3 tools/check-qpdf-trapped.py --generate
#
# Only functions listed here are safe to call from Rust: everything else can let a C++
# exception cross the FFI boundary, which aborts the process. `qpdf-c.h`'s blanket
# guarantee is not true per-function -- see ADR 0013 §1 and this script's docstring.
#
# REGENERATING THIS FILE IS PART OF EVERY qpdf VERSION BUMP. See "Bumping an engine pin"
# in engines/pins.toml.
#
# qpdf version: {version}
# functions: {len(trapped)}
"""
    TRAPPED_LIST.write_text(header + "\n".join(trapped) + "\n")
    print(f"wrote {TRAPPED_LIST.relative_to(REPO)}: {len(trapped)} trapped functions (qpdf {version})")
    return 0


def verify() -> int:
    trapped = load_trapped()
    accepted = load_accepted()
    declared = declared_functions()

    if not declared:
        sys.exit("error: parsed ZERO declarations. The check would be vacuous.")

    problems: list[str] = wasm_binding_is_covered()

    for name, where in sorted(declared.items()):
        if name in trapped:
            continue
        if name in accepted:
            continue
        problems.append(
            f"{name} (declared in {', '.join(where)}) is NOT routed through qpdf's "
            f"`trap_errors` and is not in {ACCEPTED_LIST.name}. A C++ exception from it "
            f"aborts the process -- see ADR 0013 §1. Either stop calling it, or add it to "
            f"{ACCEPTED_LIST.name} with an argument for why it cannot throw."
        )

    # An exemption for a function nobody declares is dead weight that reads as coverage.
    for name in sorted(accepted):
        if name not in declared:
            problems.append(
                f"{name} is exempted in {ACCEPTED_LIST.name} but is declared nowhere. "
                f"Remove the entry."
            )
        elif name in trapped:
            problems.append(
                f"{name} is exempted in {ACCEPTED_LIST.name} but upstream DOES route it "
                f"through `trap_errors`. Remove the exemption; the guarantee is real."
            )

    both = sorted(n for n in declared if n in trapped)
    exempt = sorted(n for n in declared if n in accepted)
    print(
        f"qpdf C API: {len(declared)} functions declared "
        f"({len(both)} trapped, {len(exempt)} accepted-untrapped), "
        f"against a generated set of {len(trapped)}"
    )
    print(f"  trapped:            {', '.join(both) or '(none)'}")
    print(f"  accepted-untrapped: {', '.join(exempt) or '(none)'}")

    if problems:
        print(f"\nFAILED — {len(problems)} problem(s):", file=sys.stderr)
        for p in problems:
            print(f"  - {p}", file=sys.stderr)
        return 1

    print("\nOK — every declared qpdf function is trapped or explicitly accepted.")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--generate",
        action="store_true",
        help="rewrite engines/qpdf-trapped-functions.txt from the pinned qpdf source",
    )
    args = parser.parse_args()
    return generate() if args.generate else verify()


if __name__ == "__main__":
    sys.exit(main())
