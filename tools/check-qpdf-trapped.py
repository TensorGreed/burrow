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
import hashlib
import pathlib
import re
import sys
import tomllib

REPO = pathlib.Path(__file__).resolve().parent.parent
PINS = REPO / "engines" / "pins.toml"
TRAPPED_LIST = REPO / "engines" / "qpdf-trapped-functions.txt"
ACCEPTED_LIST = REPO / "engines" / "qpdf-untrapped-accepted.toml"
# The helpers the generator is willing to follow, recorded BY HASH of their source bodies.
# `proven_wrappers` and `proven_forwarders` prove the bodies that are in the tree today; they
# cannot prove a body upstream has not written yet. A restructured helper that still satisfies
# the four rules would be re-proved silently, and the proof is subtle enough that "it still
# passes" is not the same as "somebody read it". So a changed body fails the bump and has to
# be re-verified by a person.
HELPERS_LIST = REPO / "engines" / "qpdf-proven-helpers.toml"

# Where declarations live. All three are checked, because a function reachable on ONE path
# is reachable, and the web bridge is a separate hand-written surface from the native FFI.
NATIVE_FFI = REPO / "core" / "burrow-engines" / "src" / "qpdf" / "ffi.rs"
WEB_BRIDGE_JS = REPO / "apps" / "web" / "src" / "worker" / "bridge.js"
WEB_TRAIT = REPO / "core" / "burrow-engines" / "src" / "web" / "bridge.rs"
WASM_BINDING = REPO / "bindings" / "burrow-wasm" / "src" / "bridge.rs"
# A fourth `extern "C"` block, easy to miss because it is `#[cfg(test)]`-gated and lives
# nowhere near the qpdf module. It declared `qpdf_get_qpdf_version` invisibly to an earlier
# version of this check, which made the stated invariant ("every qpdf C function burrow
# declares") false. Enumerating declaration sites by hand is the weak point of this tool;
# `every_extern_block_is_scanned()` guards it.
LINK_CHECK = REPO / "core" / "burrow-engines" / "src" / "link_check.rs"


def qpdf_version() -> str:
    """The pinned qpdf version, from the file that is the single source of truth."""
    with PINS.open("rb") as fh:
        return tomllib.load(fh)["qpdf"]["version"]


# Every translation unit that DEFINES a C API function burrow might call. More than one,
# which is easy to miss: `qpdf_global_set_uint32` lives in `global.cc`, not `qpdf-c.cc`, so a
# scan of the latter alone could never see it however upstream changed it.
#
# `sources_defining()` below fails if a declared function is defined in none of these, so the
# list being short is a loud failure rather than a silent exemption.
C_API_SOURCES = ("qpdf-c.cc", "qpdflogger-c.cc", "global.cc")


def qpdf_sources(version: str) -> list[pathlib.Path]:
    libqpdf = REPO / "engines" / "vendor" / "src" / f"qpdf-{version}" / "libqpdf"
    return [libqpdf / name for name in C_API_SOURCES]


def blank_literals_and_comments(source: str) -> str:
    """Replace every comment, string literal and char literal with spaces of equal length.

    **The brace walk below counts raw braces, and this is what makes that safe.** A `{` inside
    a string literal would extend an extracted body past its real closing brace and into the
    *next* function -- and if that next function calls `trap_errors`, the untrapped
    predecessor lands on the "safe to call" list. That is the one direction
    [`trapped_functions`] promises cannot happen, so it is closed here rather than argued
    about.

    It does not fire on qpdf 12.4.1: there is no brace inside any literal or comment in
    `qpdf-c.cc` today. But the shape is routine in this codebase -- `libqpdf/JSON.cc` writes
    `*p << "{";` -- and `qpdf-c.cc` already carries the JSON entry points, so one such literal
    added upstream would silently bless whatever function preceded it. Blanking rather than
    deleting keeps every byte offset identical, so the definition regex still points at the
    real source positions.
    """
    out = list(source)
    i, n = 0, len(source)
    while i < n:
        c = source[i]
        if c == "/" and i + 1 < n and source[i + 1] == "/":
            while i < n and source[i] != "\n":
                out[i] = " "
                i += 1
        elif c == "/" and i + 1 < n and source[i + 1] == "*":
            out[i] = out[i + 1] = " "
            i += 2
            while i + 1 < n and not (source[i] == "*" and source[i + 1] == "/"):
                if source[i] != "\n":
                    out[i] = " "
                i += 1
            if i + 1 < n:
                out[i] = out[i + 1] = " "
                i += 2
        elif c in "\"'":
            quote = c
            i += 1
            while i < n and source[i] != quote:
                if source[i] == "\\":
                    out[i] = " "
                    i += 1
                    if i < n and source[i] != "\n":
                        out[i] = " "
                elif source[i] != "\n":
                    out[i] = " "
                i += 1
            i += 1  # past the closing quote
        else:
            i += 1
    return "".join(out)


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


def proven_wrappers(source: str) -> dict[str, str]:
    """Static helpers that PROVABLY route their caller's work through `trap_errors`.

    Returns `{helper name: the proof, in words}`.

    # Why this exists, and why "mentions trap_errors" is not enough

    Until M1's `rotate` work this file counted direct calls only, and said so: following a
    helper "can only ever omit a function from the trapped set, never add an untrapped one to
    it". That was the conservative direction and it was the wrong answer for the `qpdf_oh_*`
    family, which reaches `trap_errors` through one static helper and is therefore just as
    protected as a direct caller. The alternative was writing exemptions in
    `qpdf-untrapped-accepted.toml` claiming those functions are non-parsing, which is false:
    `qpdf_oh_get_key` resolves an indirect object by definition.

    So exactly one level is followed, and the helper has to **earn** it. Grepping for the
    token would bless a helper that calls `trap_errors` on one path and not another.

    # The proof obligation

    A helper qualifies only if all of these hold:

    1. its body calls `trap_errors(` at least once;
    2. no `return` appears before the first `trap_errors(` call -- no early exit that skips it;
    3. every function-typed parameter that receives a `qpdf_data` -- the caller's work -- is
       invoked **only inside** a `trap_errors(` argument list;
    4. any function-typed parameter invoked outside the trap takes **no** `qpdf_data`, so it
       cannot reach the document and therefore cannot run the parser.

    Rule 4 is what admits `trap_oh_errors`, whose `fallback` is called on the error path after
    the trap has returned. It takes no document; the worst it can do is allocate, which is the
    defined-abort case `qpdf-untrapped-accepted.toml` already reasons about at length.

    Anything that fails a rule is not followed, and every function reaching `trap_errors` only
    through it stays out of the trapped set -- back to the old, conservative behaviour.
    """
    source = blank_literals_and_comments(source)
    proven: dict[str, str] = {}

    for match in DEFINITION.finditer(source):
        name = match.group(1)
        if name in _NOT_A_DEFINITION or name == "trap_errors":
            continue
        line_start = source.rfind("\n", 0, match.start()) + 1
        previous_line = source[source.rfind("\n", 0, line_start - 1) + 1 : line_start]
        if not previous_line.strip().startswith("static"):
            # Only static helpers are candidates: a non-static one is part of the C API and is
            # judged directly, not as a wrapper.
            continue

        params = _parameter_list(source, match.end())
        body = brace_body(source, match.end())
        if params is None or body is None:
            continue

        traps = [m.start() for m in re.finditer(r"\btrap_errors\s*\(", body)]
        if not traps:
            continue

        # Rule 2: nothing returns before the first trap -- except the `return` that IS the
        # trap. `return trap_errors(qpdf, fn);` is the most natural shape a wrapper can have
        # and it is not an early exit; a rule that counted it would reject the shape it most
        # needs to admit. So the return immediately preceding the trapping call, with nothing
        # between them, is the trap's own; any return before THAT is an exit that skips it.
        # `if traps else 0` keeps rule 1 the ONLY thing standing between a trapless helper and
        # the rules below, so the adversarial test can mutate rule 1 alone and see the proof
        # widen rather than see an IndexError. A crash is a failure for the wrong reason, and
        # an exit-code-only assertion reports that as the gate working.
        first_trap = traps[0] if traps else 0
        befores = [m for m in re.finditer(r"\breturn\b", body[:first_trap])]
        if befores and not body[befores[-1].end() : first_trap].strip():
            befores.pop()
        if befores:
            continue

        # The spans covered by each `trap_errors(...)` call, so "inside the trap" is a fact
        # about parentheses rather than about line numbers.
        spans = [_call_span(body, at) for at in traps]
        spans = [span for span in spans if span is not None]

        # Rules 3 and 4, per function-typed parameter -- or a refusal, if the parameter list
        # has a callable this tool cannot classify. See `_callback_parameters`: an
        # unrecognised callback would satisfy both rules by having nothing to check.
        callbacks = _callback_parameters(params)
        if callbacks is None:
            continue
        ok = True
        notes: list[str] = []
        for param_name, takes_data in callbacks:
            calls = [m.start() for m in re.finditer(rf"\b{re.escape(param_name)}\s*\(", body)]
            outside = [c for c in calls if not any(lo <= c < hi for lo, hi in spans)]
            if not outside:
                notes.append(f"`{param_name}` only inside trap_errors")
                continue
            if takes_data:
                ok = False
                break
            notes.append(f"`{param_name}` called outside the trap but takes no qpdf_data")
        if not ok:
            continue

        proven[name] = "; ".join(notes) if notes else "no callback parameters"

    return proven


def proven_forwarders(source: str, wrappers: dict[str, str]) -> dict[str, str]:
    """Static helpers that do nothing but hand their caller's work to something already proven.

    Returns `{forwarder: the chain it forwards along}`, e.g.
    `{"do_with_oh": "trap_oh_errors", "do_with_oh_void": "do_with_oh -> trap_oh_errors"}`.

    # Why this is a second category rather than "follow N levels"

    The `qpdf_oh_*` family does not call the trapping helper directly. `qpdf_oh_get_key` calls
    `do_with_oh`, which calls `trap_oh_errors`, which calls `trap_errors`; `qpdf_oh_replace_key`
    goes one step further through `do_with_oh_void`. Following N levels *of wrappers* would mean
    composing N proofs of the kind `proven_wrappers` makes, and that is a weaker argument than
    making it once: a wrapper has reachable code outside the trap **by construction** -- its
    `fallback` runs on the error path -- so each link would add a place for work to hide.

    A forwarder is the case where there is nowhere for work to hide, and that is what makes
    composing them different from composing wrappers:

    1. the body is a **single top-level statement** -- `return f(...);` or `f(...);` -- and the
       statement is a call to something already proven (a wrapper, or a forwarder proven on an
       earlier pass);
    2. nothing follows that call but the semicolon;
    3. every function-typed parameter is used **only inside** that call's argument list.

    A body satisfying those has no reachable code outside the proven call at all. Not "no code
    that looked dangerous" -- **no code**. So forwarding is transitive here in a way wrapping is
    not, and the chain is resolved to a fixed point rather than cut off at an arbitrary depth.
    Every link is recorded and printed, so the route reads `via do_with_oh_void -> do_with_oh
    -> trap_oh_errors` and an auditor can walk it.

    The chain still terminates at exactly one *wrapper*: a second helper that traps is not
    followed, because that is the composition this does not do.
    """
    source = blank_literals_and_comments(source)
    forwarders: dict[str, str] = {}

    # Fixed point: a forwarder to a forwarder is admitted only after the inner one is proven,
    # so nothing is followed on the strength of a link that has not itself been established.
    # Bounded by the number of definitions, so it terminates even on a cycle -- which, being a
    # cycle of infinite recursion, is not code anybody ships.
    for _pass in range(_MAX_FORWARDER_DEPTH):
        found_this_pass = False
        proven = dict(wrappers)
        proven.update(forwarders)

        for match in DEFINITION.finditer(source):
            name = match.group(1)
            if name in _NOT_A_DEFINITION or name in proven or name == "trap_errors":
                continue
            line_start = source.rfind("\n", 0, match.start()) + 1
            previous_line = source[source.rfind("\n", 0, line_start - 1) + 1 : line_start]
            if not previous_line.strip().startswith("static"):
                # Only static helpers are candidates: a non-static one is part of the C API and
                # is judged directly, not as a link in somebody else's route.
                continue

            params = _parameter_list(source, match.end())
            body = brace_body(source, match.end())
            if params is None or body is None:
                continue

            # Rule 1: one top-level statement. `brace_body` returns the braces too.
            statement = body.strip()
            if statement.startswith("{") and statement.endswith("}"):
                statement = statement[1:-1].strip()
            returns_it = statement.startswith("return ")
            if returns_it:
                statement = statement[len("return ") :].strip()

            target = None
            for candidate in proven:
                if re.match(rf"{re.escape(candidate)}\s*[<(]", statement):
                    target = candidate
                    break
            if target is None:
                continue

            span = _call_span(statement, 0)
            if span is None:
                continue

            # Rule 2: nothing after the call but the semicolon. This is what makes "no
            # reachable code outside the proven call" a fact about the text rather than a
            # reading of it -- and with the leading `return` already consumed, a second
            # top-level statement of any kind fails here.
            if statement[span[1] + 1 :].strip().rstrip(";").strip():
                continue

            # Rule 3: every function-typed parameter is used only inside that call. The
            # forwarded lambda's own body sits INSIDE the span -- `do_with_oh`'s handle lookup
            # and `do_with_oh_void`'s `fn(o); return true;` both do -- which is the trapped
            # side, so uses there are the point rather than a violation.
            callbacks = _callback_parameters(params)
            if callbacks is None:
                continue
            ok = True
            for param_name, _takes_data in callbacks:
                for use in re.finditer(rf"\b{re.escape(param_name)}\b", statement):
                    if not (span[0] <= use.start() < span[1]):
                        ok = False
                        break
                if not ok:
                    break
            if not ok:
                continue

            chain = target if target in wrappers else f"{target} -> {forwarders[target]}"
            forwarders[name] = chain
            found_this_pass = True

        if not found_this_pass:
            break

    return forwarders


# How many links of pure forwarding to resolve before giving up. Two is what qpdf 12.4.1 has
# (`do_with_oh_void` -> `do_with_oh`); the ceiling exists so a cycle cannot spin, not to express
# a view about depth -- each link carries its own proof.
_MAX_FORWARDER_DEPTH = 8

_NOT_A_DEFINITION = {"if", "for", "while", "switch", "return", "catch", "sizeof"}


def _parameter_list(source: str, start: int) -> str | None:
    """The text between the parentheses whose opening one is just before `start`."""
    depth = 1
    i = start
    while i < len(source) and depth:
        if source[i] == "(":
            depth += 1
        elif source[i] == ")":
            depth -= 1
            if depth == 0:
                return source[start:i]
        i += 1
    return None


def _call_span(body: str, at: int) -> tuple[int, int] | None:
    """`(start, end)` of the argument list of the call beginning at `at`."""
    open_paren = body.find("(", at)
    if open_paren < 0:
        return None
    depth = 0
    for i in range(open_paren, len(body)):
        if body[i] == "(":
            depth += 1
        elif body[i] == ")":
            depth -= 1
            if depth == 0:
                return (open_paren, i)
    return None


def _callback_parameters(params: str) -> list[tuple[str, bool]] | None:
    """`(name, takes_a_qpdf_data)` for every `std::function<...>` parameter, or `None`.

    The distinction is the whole of rule 4: a callback handed the document can reach qpdf's
    parser, and one that is not cannot.

    **`None` means "this parameter list has a callable this function cannot classify", and the
    caller must refuse to prove the helper.** Failing closed matters more here than anywhere
    else in the file: rules 3 and 4 are stated over callback parameters, so a parameter list
    where none is *recognised* satisfies both vacuously and the proof silently collapses to
    rules 1 and 2. A helper taking `F fn` on a `template <class F>`, or a raw function pointer,
    or a `std::function` whose argument list itself contains a `>` would all have read as "no
    callbacks, nothing to check". Found by security review; qpdf 12.4.1 has no such helper
    today, which is exactly why it would have gone unnoticed.
    """
    found: list[tuple[str, bool]] = []
    for match in re.finditer(
        r"std::function\s*<([^>]*)>\s*([A-Za-z_][A-Za-z0-9_]*)", params
    ):
        signature, name = match.group(1), match.group(2)
        found.append((name, "qpdf_data" in signature))

    # A raw function-pointer parameter: `RET (*fn)(qpdf_data)`.
    if re.search(r"\(\s*\*\s*[A-Za-z_][A-Za-z0-9_]*\s*\)\s*\(", params):
        return None
    # A parameter whose type is a bare template parameter -- `F fn`, `Fn&& fn`, `Callable c`.
    # Recognised by exclusion: a single capitalised identifier that is not a known qpdf type.
    known = ("qpdf_data", "qpdf_oh", "QPDF_BOOL", "char", "int", "unsigned", "size_t", "std::")
    for parameter in params.split(","):
        parameter = parameter.strip()
        if not parameter or any(token in parameter for token in known):
            continue
        if re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*\s*&{0,2}\s*[A-Za-z_][A-Za-z0-9_]*", parameter):
            return None
    return found


def trapped_functions(source: str, wrappers: dict[str, str] | None = None) -> list[str]:
    """Every C API function in `qpdf-c.cc` that reaches `trap_errors`.

    Directly, or through a proven wrapper -- optionally reached along a chain of pure
    forwarders. `proven_wrappers` says what a helper must establish to be followed and
    `proven_forwarders` says why forwarding composes where wrapping does not. The chain always
    terminates at exactly ONE wrapper; and the caller must forward purely too, or a function
    that throws on its way to the helper would be listed as safe.

    Brace-matched rather than scanned by proximity: `trap_errors` appears inside lambdas,
    inside `if` bodies, and several lines below the call that starts a statement, so
    "the nearest definition above" would be a guess. Walking the braces makes the enclosing
    function an answer rather than an estimate.
    """
    return sorted(trapped_routes(source, wrappers))


def forwards_purely_to(body: str, target: str) -> bool:
    """Whether `body`'s only top-level statement is a call to `target`.

    The purity rule, applied to the CALLER as well as to the helper. `proven_forwarders` made
    this argument about static helpers and `trapped_routes` did not apply it to the C API
    functions it routed through them -- it matched the helper's name anywhere in the body. So
    this shape was reported as trapped:

        void qpdf_foo(qpdf_data qpdf, qpdf_oh oh) {
            qpdf->qpdf->somethingThatThrows();   // untrapped; escapes into Rust
            do_with_oh_void(qpdf, oh, ...);
        }

    Found by security review, which measured 20 of the 71 indirect entries at qpdf 12.4.1
    carrying code outside the helper call. None of them is a function burrow declares, so
    nothing unsafe was ever called -- but the generated file's header says "only functions
    listed here are safe to call", and that was not true of the rule that built it.

    Work INSIDE the call's argument list is the point rather than a violation: that is the
    lambda the helper runs inside the trap, and `QTC::TC` coverage markers live there in most
    of the family.
    """
    statement = body.strip()
    if statement.startswith("{") and statement.endswith("}"):
        statement = statement[1:-1].strip()
    if statement.startswith("return "):
        statement = statement[len("return ") :].strip()
    if not re.match(rf"{re.escape(target)}\s*[<(]", statement):
        return False
    span = _call_span(statement, 0)
    if span is None:
        return False
    return not statement[span[1] + 1 :].strip().rstrip(";").strip()


def trapped_routes(source: str, wrappers: dict[str, str] | None = None) -> dict[str, str]:
    """`{function: the route it takes to `trap_errors`}`.

    The route is `"direct"`, `"via <helper>"`, or `"via <forwarder> -> ... -> <wrapper>"` with
    every link named. **Recorded rather than collapsed**, because a reader auditing this has to
    be able to see which functions are trusted on somebody else's proof and which on their own
    body -- a flat list of names hides exactly the thing the widening introduced.
    """
    # Offsets are preserved, so every index below still refers to the real source.
    source = blank_literals_and_comments(source)
    if wrappers is None:
        wrappers = proven_wrappers(source)
    forwarders = proven_forwarders(source, wrappers)

    routes: dict[str, str] = {}
    for match in DEFINITION.finditer(source):
        name = match.group(1)
        # `trap_errors` itself contains the token by definition, and control-flow keywords
        # at column 0 are not function definitions.
        if name in _NOT_A_DEFINITION or name == "trap_errors":
            continue

        # `static` helpers are not part of the C API and cannot be declared by a binding, so
        # they never appear in the output -- listing one would put a name on a list of
        # "functions safe to call" that no caller outside the translation unit can call at
        # all. They are candidates for `proven_wrappers` instead. The `static` keyword sits on
        # the return-type line, immediately above the name.
        line_start = source.rfind("\n", 0, match.start()) + 1
        previous_line = source[source.rfind("\n", 0, line_start - 1) + 1 : line_start]
        if previous_line.strip().startswith("static"):
            continue

        body = brace_body(source, match.end())
        if body is None:
            continue

        if re.search(r"\btrap_errors\s*\(", body):
            routes[name] = "direct"
            continue

        # AND THE CALLER MUST BE PURE TOO. A helper's proof is about the helper; it says
        # nothing about a caller that throws on its way to it. `forwards_purely_to` is the
        # same rule `proven_forwarders` applies to helpers, applied here -- see its docstring
        # for the shape this admitted before, and for what a strict reading costs.
        matched = False
        for helper in wrappers:
            if forwards_purely_to(body, helper):
                routes[name] = f"via {helper}"
                matched = True
                break
        if matched:
            continue
        # EVERY LINK RECORDED. A function reaching the trap through a forwarder says so in
        # full, so an auditor sees the whole chain rather than a name that looks direct.
        for forwarder, chain in forwarders.items():
            if forwards_purely_to(body, forwarder):
                routes[name] = f"via {forwarder} -> {chain}"
                break

    return routes


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


# ---------------------------------------------------------------------------------
# THE SECOND ROUTE.
#
# Every parser above is a PATTERN, and a pattern has a shape it cannot see past. M1 PR B
# proved that is not hypothetical: the character class was `[a-z_0-9]*`, so all three
# declaration parsers were blind to the four qpdf C functions with a capital in the name --
# `qpdf_set_deterministic_ID`, `qpdf_set_static_ID`, `qpdf_set_static_aes_IV`,
# `qpdf_set_suppress_original_object_IDs`. All four are untrapped. Declaring one would have
# passed this gate in silence, and the output would have said OK while examining one fewer
# function than existed. That is the "4 of 15" shape the root CLAUDE.md names, in the one
# check whose whole job is to be exhaustive.
#
# The fixtures below now cover capitals, and fixtures are the wrong instrument for this on
# their own: they can only cover a class of name somebody thought of. What catches the
# class nobody thought of is a SECOND ROUTE that does not share the first one's blind spot.
#
# So: tokenise instead of matching. `externs_by_token` walks the `extern "C"` block, finds
# each `fn`, skips whitespace, and takes the identifier that follows using Rust's own
# definition of an identifier. There is no qpdf-shaped pattern in it at all, so there is no
# qpdf-shaped pattern to get wrong. If the two routes disagree, the narrow one is blind and
# the check FAILS rather than quietly reporting the smaller number.
#
# The same argument applies to the JS bridge, where the token is whatever follows `._`.
# ---------------------------------------------------------------------------------

RUST_IDENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")


def externs_by_token(block: str) -> set[str]:
    """Every `fn NAME` in an extern block, found by walking rather than by matching.

    Deliberately NOT a regex over the whole declaration. The point is to share no pattern
    with `parse_ffi`, so that a class of names the pattern cannot see is still seen here.
    The only character class is Rust's own definition of an identifier, which is a fact
    about the language rather than a guess about qpdf's naming.
    """
    names: set[str] = set()
    i = 0
    while True:
        at = block.find("fn", i)
        if at == -1:
            return names
        i = at + 2
        # `fn` must be a WORD OF ITS OWN, checked on BOTH sides. Checking only the left
        # side was the first version and the fixture below caught it: `fn_like_name(` has a
        # space before the `fn`, so it passed, and the walk then read `_like_name` as a
        # declared symbol. A cross-check that invents functions is worse than no
        # cross-check, because every run would fail with a name nobody can find.
        before = block[at - 1] if at > 0 else " "
        after = block[i] if i < len(block) else " "
        if before.isalnum() or before == "_" or after.isalnum() or after == "_":
            continue
        rest = block[i:]
        stripped = rest.lstrip()
        match = RUST_IDENT.match(stripped)
        if match is None:
            continue
        # A declaration, not a use: the identifier is followed by `(` or a generic.
        tail = stripped[match.end() :].lstrip()
        if tail.startswith(("(", "<")):
            names.add(match.group(0))


def js_exports_by_token(text: str) -> set[str]:
    """Every `._NAME(` in the JS bridge, found by walking rather than by matching."""
    names: set[str] = set()
    i = 0
    while True:
        at = text.find("._", i)
        if at == -1:
            return names
        i = at + 2
        match = RUST_IDENT.match(text[i:])
        if match is None:
            continue
        if text[i + match.end() :].lstrip().startswith("("):
            names.add(match.group(0))


def strip_rust_comments(text: str) -> str:
    """Line and block comments removed, so a name in prose is not read as a declaration.

    `parse_ffi` achieves the same thing by requiring `pub(super) fn` at the start of a
    line. The two routes get there differently on purpose.
    """
    text = re.sub(r"/\*.*?\*/", " ", text, flags=re.S)
    return re.sub(r"//[^\n]*", "", text)


def cross_check(narrow: set[str], tokenised: set[str], where: str, prefix: str) -> list[str]:
    """Fail when the pattern route sees less than the token route.

    Only the qpdf-prefixed names are compared: an `extern "C"` block may legitimately hold
    other symbols, and this check is about qpdf's C API. The prefix is a contract, not a
    guess -- these are C symbol names, so they are exactly what qpdf exports.
    """
    tokenised = {n for n in tokenised if n.startswith(prefix)}
    missed = sorted(tokenised - narrow)
    if not missed:
        return []
    return [
        f"{where}: the declaration pattern missed {len(missed)} function(s) that a "
        f"token walk over the same text found: {', '.join(missed)}. The pattern is blind "
        f"to a class of names, so this check was examining fewer functions than exist -- "
        f"which reads as OK. Widen it and add a fixture for the class."
    ]


# PARSER FIXTURES.
#
# THE SHARP CASE FOR THIS FILE: `engines/build-wasm.sh` parses the SAME `ffi.rs` with the
# SAME shape, and got exactly this treatment in PR #40 -- one fixture and four near-misses,
# including a commented-out declaration. This file did not, and its per-source floors would
# not have noticed the difference that matters. A floor of 10 declarations passes with 15 of
# 16 functions parsed, which is the "4 of 15" shape: a regex tightened so it stops matching
# one real declaration NARROWS the set required to be trapped, and the count still looks fine.
#
# A loosened regex is the noisy direction -- it would demand justification for a function
# nobody calls. A tightened one is the silent direction, and it is the one that lets an
# untrapped call through.
FFI_CASES: tuple[tuple[str, str | None, str], ...] = (
    ("    pub(super) fn qpdf_read_memory(", "qpdf_read_memory", "an ordinary declaration"),
    ("    pub(super) fn qpdf_init() -> QpdfData;", "qpdf_init", "one with no parameters"),
    ("    pub(super) fn qpdflogger_create() -> QpdfLoggerHandle;", "qpdflogger_create",
     "the logger family, which is a different prefix"),
    ("    // pub(super) fn qpdf_is_linearized(", None, "a commented-out declaration"),
    ("    /// `qpdf_is_linearized` is deliberately absent", None, "prose naming a function"),
    ("    pub fn qpdf_is_linearized(", None, "`pub fn` without `(super)`"),
    ("    pub(super) fn pdfium_load(", None, "a non-qpdf function"),
    # A CAPITAL LETTER IN THE NAME. The character class here was `[a-z_0-9]*` until M1 PR B,
    # so every one of these parsers was blind to the four qpdf C functions that have one --
    # `qpdf_set_deterministic_ID`, `qpdf_set_static_ID`, `qpdf_set_static_aes_IV` and
    # `qpdf_set_suppress_original_object_IDs`. All four are UNTRAPPED and all four
    # dereference the writer, so declaring one would have passed this gate in silence:
    # `declared_functions()` would not have seen it, and a name nobody sees is a name nobody
    # can refuse. The generated trapped list was never blind to them (`DEFINITION` already
    # allowed capitals), which is what made the asymmetry invisible from the output.
    ("    pub(super) fn qpdf_set_deterministic_ID(", "qpdf_set_deterministic_ID",
     "a name with a capital letter, which this parser used to miss entirely"),
    ("    pub(super) fn qpdf_set_static_aes_IV(", "qpdf_set_static_aes_IV",
     "a capital letter after an underscore, not at the end"),
)

JS_CASES: tuple[tuple[str, str | None, str], ...] = (
    ("self.__burrow_qpdf_read_memory = (d) => qpdf()._qpdf_read_memory(d);", "qpdf_read_memory",
     "an ordinary bridge call"),
    ("  qpdf()._qpdf_get_num_pages(data);", "qpdf_get_num_pages", "one on its own line"),
    ("// qpdf()._qpdf_is_linearized(d)", "qpdf_is_linearized",
     "a COMMENTED-OUT bridge call -- deliberately still matched, see below"),
    ("self.__burrow_qpdf_copy_in = (b) => copyInto(qpdf(), b);", None,
     "a bridge helper that never enters qpdf's C API"),
    ("pdfium()._FPDF_LoadMemDocument64(x);", None, "a PDFium call"),
    ("  qpdf()._qpdf_set_deterministic_ID(d, 1);", "qpdf_set_deterministic_ID",
     "a name with a capital letter -- see the FFI cases for why this is here"),
)


def parse_ffi(text: str) -> list[str]:
    """Declarations in `qpdf/ffi.rs`'s `extern \"C\"` block."""
    return re.findall(r"^\s*pub\(super\) fn (qpdf[A-Za-z_0-9]*)\s*\(", text, re.MULTILINE)


def parse_js_bridge(text: str) -> list[str]:
    """qpdf C exports the JS bridge calls into the Emscripten module."""
    return re.findall(r"\._(qpdf[A-Za-z_0-9]*)\s*\(", text)


# Filled in by `check_wrapper_probes` on every run, so the summary line can report the number
# of probes that actually ran rather than a literal somebody has to remember to update.
WRAPPER_PROBES_RUN: list[str] = []


def check_parser_fixtures() -> list[str]:
    """Every declaration parser must find its own cases and reject its near-misses."""
    problems: list[str] = []

    for line, expected, label in FFI_CASES:
        got = parse_ffi(line)
        if expected is None and got:
            problems.append(f"the ffi.rs parser matches {label}: {line.strip()!r} -> {got!r}")
        elif expected is not None and got != [expected]:
            problems.append(
                f"the ffi.rs parser does not find {label}: {line.strip()!r} -> {got!r}, "
                f"expected [{expected!r}]"
            )

    for line, expected, label in JS_CASES:
        got = parse_js_bridge(line)
        if expected is None and got:
            problems.append(f"the JS bridge parser matches {label}: {line.strip()!r} -> {got!r}")
        elif expected is not None and got != [expected]:
            problems.append(
                f"the JS bridge parser does not find {label}: {line.strip()!r} -> {got!r}, "
                f"expected [{expected!r}]"
            )

    # The trapped-set parser, on a synthetic qpdf-c.cc. Both directions matter: a function
    # whose body calls `trap_errors` must be found, and one that merely mentions it in a
    # comment must not.
    synthetic = """\
QPDF_ERROR_CODE
qpdf_trapped_example(qpdf_data qpdf)
{
    return trap_errors(qpdf, &call_thing);
}

int
qpdf_untrapped_example(qpdf_data qpdf)
{
    // this one does not go through trap_errors
    return qpdf->qpdf->isLinearized();
}

static int
qpdf_static_helper(qpdf_data qpdf)
{
    return trap_errors(qpdf, &call_other);
}
"""
    trapped = trapped_functions(synthetic)
    if "qpdf_trapped_example" not in trapped:
        problems.append("trapped_functions misses a function whose body calls trap_errors")
    if "qpdf_untrapped_example" in trapped:
        problems.append(
            "trapped_functions reports an UNTRAPPED function as trapped -- it would bless a "
            "call that can abort the process"
        )
    if "qpdf_static_helper" in trapped:
        problems.append("trapped_functions includes a `static` helper, which is not C API")

    problems += check_wrapper_probes(WRAPPER_PROBES_RUN)
    problems += check_cross_check_fixtures()

    return problems



# A synthetic translation unit for the wrapper proof, in three variants. `WRAPPER_SOURCE` is
# the shape qpdf actually has -- a helper that traps, a forwarder that does nothing but hand
# its work to that helper, and one C API function reaching the trap through each -- plus
# `helper_that_does_not_trap`, which is the near-miss for rule 1: identical in shape, handing
# its callback to a helper of its own, and it never goes near `trap_errors`. It deliberately
# passes `fn` along rather than calling it, so rule 1 is the ONLY rule that rejects it -- a
# near-miss caught by two rules cannot tell you which of them is still working.
# `helper_that_traps_then_calls_out` is the near-miss for rules 3 and 4: it does trap, and
# then calls the callback a second time outside the trap.
WRAPPER_SOURCE = """\
template <class RET>
static RET
helper_that_traps(qpdf_data qpdf, std::function<RET()> fallback, std::function<RET(qpdf_data)> fn)
{
    return trap_errors(qpdf, fn);
}

template <class RET>
static RET
helper_that_calls_its_fallback(qpdf_data qpdf, std::function<RET()> fallback, std::function<RET(qpdf_data)> fn)
{
    RET result = trap_errors(qpdf, fn);
    if (qpdf->has_error) {
        return fallback();
    }
    return result;
}

template <class RET>
static RET
helper_that_returns_before_the_trap(qpdf_data qpdf, std::function<RET()> fallback, std::function<RET(qpdf_data)> fn)
{
    if (!qpdf) {
        return fallback();
    }
    return trap_errors(qpdf, fn);
}

template <class RET>
static RET
helper_that_does_not_trap(qpdf_data qpdf, std::function<RET()> fallback, std::function<RET(qpdf_data)> fn)
{
    return catch_nothing(qpdf, fn);
}

template <class RET>
static RET
helper_that_traps_then_calls_out(qpdf_data qpdf, std::function<RET()> fallback, std::function<RET(qpdf_data)> fn)
{
    RET first = trap_errors(qpdf, fn);
    return fn(qpdf);
}

template <class RET>
static RET
pure_forwarder(qpdf_data qpdf, std::function<RET()> fallback, std::function<RET(qpdf_data)> fn)
{
    return helper_that_traps<RET>(qpdf, fallback, [fn](qpdf_data q) {
        return fn(q);
    });
}

int
qpdf_reached_through_the_wrapper(qpdf_data qpdf)
{
    return helper_that_traps<int>(qpdf, [] { return 0; }, [](qpdf_data q) { return 1; });
}

int
qpdf_reached_through_the_forwarder(qpdf_data qpdf)
{
    return pure_forwarder<int>(qpdf, [] { return 0; }, [](qpdf_data q) { return 1; });
}

int
qpdf_reached_through_nothing(qpdf_data qpdf)
{
    return helper_that_does_not_trap<int>(qpdf, [] { return 0; }, [](qpdf_data q) { return 1; });
}

static void
void_forwarder(qpdf_data qpdf, std::function<void(qpdf_data)> fn)
{
    pure_forwarder<bool>(qpdf, [] { return false; }, [fn](qpdf_data q) {
        fn(q);
        return true;
    });
}

template <class RET>
static RET
forwarder_that_does_more(qpdf_data qpdf, std::function<RET()> fallback, std::function<RET(qpdf_data)> fn)
{
    helper_that_traps<RET>(qpdf, fallback, fn);
    return fn(qpdf);
}

void
qpdf_reached_through_a_forwarder_chain(qpdf_data qpdf)
{
    void_forwarder(qpdf, [](qpdf_data q) { });
}

int
qpdf_reached_through_a_busy_forwarder(qpdf_data qpdf)
{
    return forwarder_that_does_more<int>(qpdf, [] { return 0; }, [](qpdf_data q) { return 1; });
}

int
qpdf_reached_through_an_early_exit(qpdf_data qpdf)
{
    return helper_that_returns_before_the_trap<int>(qpdf, [] { return 0; }, [](qpdf_data q) { return 1; });
}

int
qpdf_does_work_then_forwards(qpdf_data qpdf)
{
    qpdf->qpdf->somethingThatCanThrow();
    return helper_that_traps<int>(qpdf, [] { return 0; }, [](qpdf_data q) { return 1; });
}

int
qpdf_reached_through_a_leaky_helper(qpdf_data qpdf)
{
    return helper_that_traps_then_calls_out<int>(qpdf, [] { return 0; }, [](qpdf_data q) { return 1; });
}
"""


def check_wrapper_probes(ran: list[str] | None = None) -> list[str]:
    """The wrapper and forwarder proofs, exercised both ways, on every run.

    Following a helper is the widening that let `qpdf_oh_*` be called at all, and it is the
    one part of this tool that trusts a function OTHER than the one being judged. Three
    probes, because the failure modes are not symmetric:

    1. **a helper that does not trap must not be followed** -- if it were, every function
       using it would be listed as safe while a C++ exception walked straight out of it;
    2. **a function reached through a trapping helper must be found** -- otherwise the
       widening silently does nothing and the `qpdf_oh_*` family goes back to failing, which
       reads as "the tool got stricter" rather than "the tool stopped working";
    3. **mutating `trap_errors` out of the helper must turn every function using it red** --
       the proof has to be load-bearing, not decorative. This is the mutation probe
       `CLAUDE.md` asks for, and the mutation is asserted to have applied before it is run.
    """
    problems: list[str] = []
    # EVERY PROBE REGISTERS ITSELF, so the count in the output is a measurement. It was a
    # literal `7` beside two derived counts on the same line -- delete a probe and the tool
    # still said seven. `CLAUDE.md`: a number with no expectation beside it is not a report.
    locally_ran: list[str] = []

    def probe(name: str, holds: bool, complaint: str) -> None:
        locally_ran.append(name)
        if not holds:
            problems.append(complaint)

    wrappers = proven_wrappers(WRAPPER_SOURCE)
    forwarders = proven_forwarders(WRAPPER_SOURCE, wrappers)
    routes = trapped_routes(WRAPPER_SOURCE, wrappers)

    # THE PROOF, RULE BY RULE, both ways. Each rule has a near-miss that only it rejects: a
    # near-miss two rules catch cannot tell you which of them still works.
    probe(
        "rule 1: a helper that never calls trap_errors is not proven",
        "helper_that_does_not_trap" not in wrappers,
        "proven_wrappers proves a helper that never calls trap_errors -- every function "
        "using it would be reported safe while an exception escapes",
    )
    probe(
        "rule 2: a helper that can return before the trap is not proven",
        "helper_that_returns_before_the_trap" not in wrappers,
        "proven_wrappers proves a helper with an early exit that skips the trap; a call "
        "taking that branch is untrapped and nothing says so",
    )
    probe(
        "rules 3/4: a helper that calls its qpdf_data callback outside the trap is not proven",
        "helper_that_traps_then_calls_out" not in wrappers,
        "proven_wrappers proves a helper that calls its qpdf_data callback OUTSIDE the "
        "trap -- trapping one of two call sites traps nothing",
    )
    probe(
        "rule 4, permissive: a fallback taking no qpdf_data may be called outside the trap",
        "helper_that_calls_its_fallback" in wrappers,
        "proven_wrappers refuses a helper whose only out-of-trap call is a callback taking "
        "no qpdf_data -- that is the rule that admits trap_oh_errors, so refusing it would "
        "take the whole qpdf_oh_* family off the trapped list",
    )
    probe(
        "a trapping helper is proven",
        "helper_that_traps" in wrappers,
        "proven_wrappers does not prove a helper that wraps its callback in trap_errors, "
        "so the qpdf_oh_* family cannot be reached at all",
    )
    probe(
        "a pure forwarder is followed",
        forwarders.get("pure_forwarder") == "helper_that_traps",
        f"proven_forwarders does not admit a helper whose whole body forwards to a proven "
        f"wrapper: {forwarders!r}",
    )
    # A CHAIN, and a VOID one. `qpdf_oh_replace_key` -- the call rotate needs to write
    # /Rotate -- reaches the trap through `do_with_oh_void` -> `do_with_oh`, and the outer
    # link's body is a bare statement rather than a return.
    probe(
        "a chain of pure forwarders is followed, with every link recorded",
        forwarders.get("void_forwarder") == "pure_forwarder -> helper_that_traps",
        f"proven_forwarders does not follow a chain of pure forwarders, or does not record "
        f"every link of it: {forwarders!r}",
    )
    probe(
        "a forwarder that does work after the call is not followed",
        "forwarder_that_does_more" not in forwarders,
        "proven_forwarders admits a helper that calls the trap and then does more work -- "
        "the whole basis for following a forwarder is that there is NO code outside the "
        "proven call",
    )

    # THE ROUTES, which is where a caller's own body is judged.
    expected = {
        "qpdf_reached_through_the_wrapper": "via helper_that_traps",
        "qpdf_reached_through_the_forwarder": "via pure_forwarder -> helper_that_traps",
        "qpdf_reached_through_a_forwarder_chain": (
            "via void_forwarder -> pure_forwarder -> helper_that_traps"
        ),
    }
    for name, route in expected.items():
        probe(
            f"{name} is reported as {route}",
            routes.get(name) == route,
            f"{name} should be trapped {route!r}, reported {routes.get(name)!r}",
        )
    for name, why in (
        ("qpdf_reached_through_nothing", "an UNTRAPPED helper"),
        ("qpdf_reached_through_a_leaky_helper", "a helper that calls out past its trap"),
        ("qpdf_reached_through_an_early_exit", "a helper that can return before its trap"),
        # THE CALLER'S OWN BODY. A helper's proof says nothing about a caller that throws on
        # the way to it. Security review measured 20 of 71 indirect entries at qpdf 12.4.1
        # carrying code outside the helper call; burrow declared none of them, so nothing
        # unsafe was called -- but the rule that built the list was wrong, and the file's
        # header claims everything on it is safe to call.
        ("qpdf_does_work_then_forwards", "a helper, after doing untrapped work of its own"),
    ):
        probe(
            f"{name} is not reported as trapped",
            name not in routes,
            f"a function reaching the trap only through {why} is reported as trapped -- this "
            f"is the direction that aborts the process",
        )

    # Probe 4: the fingerprint gate, which is what stops a RESTRUCTURED helper from being
    # re-proved silently at a version bump. Three states, because "no changes" has to be a
    # measurement too -- a gate that reports a change unconditionally is as useless as one
    # that never does.
    pinned = {"helper_that_traps": ("wrapper", "a" * 64)}
    probe(
        "an unchanged body reports no change",
        not helper_changes({"helper_that_traps": ("wrapper", "a" * 64)}, pinned),
        "the helper fingerprint gate reports a change where none is, which would make every "
        "regeneration a refusal and teach people to pass --accept-helper-changes blindly",
    )
    probe(
        "a moved body is reported",
        any(
            "BODY HAS CHANGED" in change
            for change in helper_changes({"helper_that_traps": ("wrapper", "b" * 64)}, pinned)
        ),
        "the helper fingerprint gate does not notice a helper body changing -- a "
        "restructured helper would be re-proved with nobody reading it",
    )
    probe(
        "a newly followed helper is reported",
        any(
            "newly followed" in change
            for change in helper_changes(
                {"helper_that_traps": ("wrapper", "a" * 64), "brand_new": ("wrapper", "c" * 64)},
                pinned,
            )
        ),
        "the helper fingerprint gate does not notice a NEW helper being followed",
    )
    probe(
        "a helper that stopped qualifying is reported",
        any("no longer satisfies the rules" in change for change in helper_changes({}, pinned)),
        "the helper fingerprint gate does not notice a recorded helper dropping out, which "
        "is how a function silently stops being callable",
    )

    # Probe 5: the mutation. Take `trap_errors` out of the helper and nothing that depends on
    # it may survive. ASSERT IT APPLIED FIRST -- a str.replace that matches nothing leaves a
    # green run that says the defence works when nothing was mutated.
    original = "    return trap_errors(qpdf, fn);"
    probe(
        "the mutation probe's target is present, so the mutation below applies",
        original in WRAPPER_SOURCE,
        "the wrapper mutation probe did not apply: its target line is not in WRAPPER_SOURCE, "
        "so the probe below proves nothing",
    )
    if original in WRAPPER_SOURCE:
        mutated_source = WRAPPER_SOURCE.replace(original, "    return fn(qpdf);", 1)
        mutated = trapped_routes(mutated_source)
        survivors = sorted(n for n in expected if n in mutated)
        probe(
            "removing trap_errors from the helper turns every function using it red",
            not survivors,
            f"removing trap_errors from the helper left {', '.join(survivors)} reported "
            f"as trapped -- the helper's proof is not load-bearing",
        )

    if ran is not None:
        ran.extend(locally_ran)
    return problems


def check_cross_check_fixtures() -> list[str]:
    """The second route must find what the first cannot, and must not cry wolf.

    THE FIXTURES ABOVE ARE NOT ENOUGH ON THEIR OWN, and that is the whole reason this
    function exists. They can only cover a class of name somebody thought of -- capitals
    are in there now because M1 PR B was bitten by them, which is to say they were added
    after the fact. The token walk is what covers the class nobody has thought of yet, and
    a defence nobody has seen fail is one nobody knows works. So it is exercised here, on
    every run, against planted input rather than only against the real files where both
    routes agree.
    """
    problems: list[str] = []

    block = """
        pub(super) fn qpdf_read_memory(a: A) -> B;
        pub(super) fn qpdf_set_deterministic_ID(a: A);
        pub(super) fn pdfium_load(a: A);
    """

    found = externs_by_token(block)
    for expected in ("qpdf_read_memory", "qpdf_set_deterministic_ID", "pdfium_load"):
        if expected not in found:
            problems.append(f"externs_by_token misses {expected}: {sorted(found)}")

    # The near-miss: a use is not a declaration.
    if externs_by_token("    let x = fn_like_name(3);"):
        problems.append("externs_by_token treats `fn_like_name(` as a declaration")
    if "fnord" in externs_by_token("    fnord(x);"):
        problems.append("externs_by_token matched `fn` inside a longer word")

    # Comment stripping, so prose naming a function is not read as declaring it.
    stripped = strip_rust_comments("    /// `qpdf_is_linearized` is absent\n    pub(super) fn qpdf_init();")
    if "qpdf_is_linearized" in externs_by_token(stripped):
        problems.append("strip_rust_comments left a name in prose visible to the token walk")
    if "qpdf_init" not in externs_by_token(stripped):
        problems.append("strip_rust_comments removed a real declaration")

    # THE CROSS-CHECK ITSELF. A narrow set missing what the walk found must be reported,
    # and the message must name the function -- a complaint that does not say which
    # function is one nobody can act on.
    narrow = {"qpdf_read_memory"}
    tokenised = {"qpdf_read_memory", "qpdf_set_deterministic_ID", "pdfium_load"}
    reported = cross_check(narrow, tokenised, "fixture", "qpdf")
    if not reported:
        problems.append(
            "cross_check passed a narrow set that is missing qpdf_set_deterministic_ID -- "
            "it would not have caught the M1 PR B blindness it exists for"
        )
    elif "qpdf_set_deterministic_ID" not in reported[0]:
        problems.append(f"cross_check does not name the missed function: {reported[0]}")
    elif "pdfium_load" in reported[0]:
        problems.append("cross_check complained about a non-qpdf extern, which is not its business")

    # And it must be silent when the two agree, or it is noise rather than a check.
    if cross_check({"qpdf_a", "qpdf_b"}, {"qpdf_a", "qpdf_b"}, "fixture", "qpdf"):
        problems.append("cross_check reports a difference when the two routes agree")

    # The JS walk, both directions.
    js = "self.x = () => qpdf()._qpdf_set_static_ID(d);"
    if "qpdf_set_static_ID" not in js_exports_by_token(js):
        problems.append("js_exports_by_token misses a capitalised export")
    if js_exports_by_token("const a = b._c;"):
        problems.append("js_exports_by_token matched a property access that is not a call")

    return problems


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

    ffi_text = NATIVE_FFI.read_text()
    bridge_text = WEB_BRIDGE_JS.read_text()

    for name in parse_ffi(ffi_text):
        record(name, "qpdf/ffi.rs")

    for name in parse_js_bridge(bridge_text):
        record(name, "worker/bridge.js")

    # THE SECOND ROUTE, run against the same text. See its section above: a pattern has a
    # shape it cannot see past, and this walk has no qpdf-shaped pattern in it. A
    # disagreement means the pattern is blind and the count above is an under-report, so it
    # is raised here rather than left to look like a clean run.
    cross: list[str] = []
    cross += cross_check(
        set(parse_ffi(ffi_text)),
        externs_by_token(strip_rust_comments(ffi_text)),
        "qpdf/ffi.rs",
        "qpdf",
    )
    cross += cross_check(
        set(parse_js_bridge(bridge_text)),
        js_exports_by_token(bridge_text),
        "worker/bridge.js",
        "qpdf",
    )
    for block in re.findall(r'unsafe extern "C" \{(.*?)\n\}', LINK_CHECK.read_text(), re.S):
        cross += cross_check(
            set(re.findall(r"^\s*fn (qpdf[A-Za-z_0-9]*)\s*\(", block, re.MULTILINE)),
            externs_by_token(strip_rust_comments(block)),
            "link_check.rs",
            "qpdf",
        )
    if cross:
        raise SystemExit(
            "FAILED — the declaration patterns and an independent token walk disagree:\n  - "
            + "\n  - ".join(cross)
        )

    for name in re.findall(r"^\s*/// `(qpdf[A-Za-z_0-9]*)`\.", WEB_TRAIT.read_text(), re.MULTILINE):
        record(name, "web/bridge.rs (doc)")

    # Scoped to the `extern "C"` block. A bare `fn qpdf…` regex over the whole file also
    # matches Rust test functions -- `qpdf_links_and_reports_the_pinned_version` -- which are
    # not C symbols and would demand nonsense exemptions.
    for block in re.findall(r'unsafe extern "C" \{(.*?)\n\}', LINK_CHECK.read_text(), re.S):
        for name in re.findall(r"^\s*fn (qpdf[A-Za-z_0-9]*)\s*\(", block, re.MULTILINE):
            record(name, "link_check.rs")

    # PER-SOURCE GUARD. Merging sources means one can fall silent without the total reaching
    # zero: if `bridge.js` changed call style -- `mod["_qpdf_read_memory"](x)`, or a hoisted
    # member -- its regex would match nothing while `ffi.rs` kept `declared` non-empty, and
    # this check would keep reporting OK while covering the native path alone. That is the
    # exact vacuity shape the rest of this tool is careful about.
    for label, minimum in (("qpdf/ffi.rs", 10), ("worker/bridge.js", 10), ("link_check.rs", 1)):
        found = sum(1 for where in out.values() if label in where)
        if found < minimum:
            sys.exit(
                f"error: parsed only {found} qpdf declarations out of {label} (expected at "
                f"least {minimum}). Its call style has probably changed and the regex in "
                f"declared_functions() no longer matches -- this check would silently stop "
                f"covering that path."
            )

    return {name: sorted(set(where)) for name, where in out.items()}


def wasm_binding_is_covered() -> list[str]:
    r"""Every `__burrow_qpdf_*` the wasm binding imports must be defined in the JS bridge.

    This is part of what makes it safe for `declared_functions()` to read the JS bridge alone
    for the web path. The Rust half names bridge functions; the JS half is the only place that
    says which qpdf C export each one actually calls. If a `__burrow_qpdf_*` import existed
    with no JS definition, the web path would be calling something this check never saw --
    and at runtime it would be `undefined`, so the failure would surface as a trap in a
    worker rather than as a problem anyone could read.

    **The regex is not what makes the web surface complete, and it should not be mistaken for
    it.** `\._(qpdf…)\(` is shape-dependent and trivially evaded by hand:
    `mod["_qpdf_is_linearized"](x)`, or hoisting the member into a variable. What closes that
    is the layer below, and it is worth stating because it is not obvious:

      * `engines/build-wasm.sh` builds qpdf.wasm with a CLOSED `EXPORTED_FUNCTIONS` allowlist,
        derived from `qpdf/ffi.rs` and asserted against the built module's actual export
        table. A qpdf symbol not on that list is not in the module at all.
      * `EXPORTED_RUNTIME_METHODS` is `HEAPU8, HEAPU32, stackSave, stackAlloc, stackRestore`
        -- **no `cwrap`, no `ccall`**. So there is no dynamic call surface either.

    So any qpdf function the JS could reach, by any syntax, must first appear in the file this
    check already reads. The composition is what holds; the regex only has to catch honest
    code, and the per-source guard in `declared_functions()` catches it falling silent.
    """
    js = WEB_BRIDGE_JS.read_text()
    # Capitals included, like every other declaration pattern here. These names are ours
    # rather than qpdf's, so none has one today -- but the failure mode if one did is the
    # silent half: an import the check cannot see is reported as covered, and the worker
    # traps on `undefined` at runtime instead.
    defined = set(re.findall(r"self\.__burrow_(qpdf[A-Za-z_0-9]*)\s*=", js))
    imported = set(re.findall(r"\b__burrow_(qpdf[A-Za-z_0-9]*)\b", WASM_BINDING.read_text()))

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


def load_trapped() -> dict[str, str]:
    """`{function: route}` from the generated list.

    The route is carried through to the output rather than dropped on load: `verify()` prints
    which route each DECLARED function takes, so the one place a reader looks -- the CI log --
    says whether a call is trusted on qpdf's own body or on a helper's proof.
    """
    if not TRAPPED_LIST.is_file():
        sys.exit(
            f"error: {TRAPPED_LIST.relative_to(REPO)} is missing. "
            f"Generate it with --generate (needs engines/fetch.sh to have run)."
        )
    trapped: dict[str, str] = {}
    for line in TRAPPED_LIST.read_text().splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        name, _, route = line.strip().partition("\t")
        trapped[name] = route or "(no route recorded)"
    return trapped


def load_accepted() -> dict[str, str]:
    if not ACCEPTED_LIST.is_file():
        sys.exit(f"error: {ACCEPTED_LIST.relative_to(REPO)} is missing")
    with ACCEPTED_LIST.open("rb") as fh:
        data = tomllib.load(fh)

    # The exemptions are arguments about a SPECIFIC version's implementation -- upstream can
    # start throwing from a function without changing its signature. `qpdf_version` records
    # which version they were checked against, and engines/pins.toml's bump procedure tells
    # the reader that field means something. This is what makes it mean something.
    checked = data.get("meta", {}).get("qpdf_version")
    pinned = qpdf_version()
    if checked != pinned:
        sys.exit(
            f"error: {ACCEPTED_LIST.name} records its arguments as checked against qpdf "
            f"{checked}, but engines/pins.toml pins {pinned}. Re-read each exemption against "
            f"the new source, then update meta.qpdf_version. See 'Bumping an engine pin'."
        )

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


def helper_bodies(source: str) -> dict[str, tuple[str, str]]:
    """`{helper: (role, sha256 of its source body)}` for every helper the generator may follow.

    The hash is over the **raw** body, comments and whitespace included, not the blanked form
    the proofs run on. That is deliberate: a comment change is cheap to re-approve, and an
    upstream comment explaining a restructure is exactly what a re-verification should put in
    front of a reader. The proofs answer "do the rules hold?"; this answers "is this still the
    body the rules were read against?" -- and only the second survives a rewrite that happens
    to keep passing.
    """
    blanked = blank_literals_and_comments(source)
    wrappers = proven_wrappers(blanked)
    forwarders = proven_forwarders(blanked, wrappers)
    roles = {name: "wrapper" for name in wrappers}
    roles.update({name: "forwarder" for name in forwarders})

    bodies: dict[str, tuple[str, str]] = {}
    for match in DEFINITION.finditer(blanked):
        name = match.group(1)
        if name not in roles:
            continue
        # Blanking preserves every offset, so a span found in `blanked` indexes the REAL source.
        body = brace_body(blanked, match.end())
        if body is None:
            continue
        start = blanked.index(body, match.end())
        raw = source[start : start + len(body)]
        bodies[name] = (roles[name], hashlib.sha256(raw.encode()).hexdigest())
    return bodies


def load_helpers() -> dict[str, tuple[str, str]]:
    """The recorded helper fingerprints, held to the pinned version like the exemptions are."""
    if not HELPERS_LIST.is_file():
        sys.exit(
            f"error: {HELPERS_LIST.relative_to(REPO)} is missing. Generate it with --generate."
        )
    with HELPERS_LIST.open("rb") as fh:
        data = tomllib.load(fh)

    checked = data.get("meta", {}).get("qpdf_version")
    pinned = qpdf_version()
    if checked != pinned:
        sys.exit(
            f"error: {HELPERS_LIST.name} records helper bodies read at qpdf {checked}, but "
            f"engines/pins.toml pins {pinned}. Re-read each helper against the new source, "
            f"then regenerate. See 'Bumping an engine pin' in engines/pins.toml."
        )

    recorded: dict[str, tuple[str, str]] = {}
    for entry in data.get("helper", []):
        name = entry.get("name")
        if not name:
            sys.exit(f"error: {HELPERS_LIST.name} has an entry with no `name`")
        if len(entry.get("why", "")) < 40:
            sys.exit(
                f"error: {name} is recorded in {HELPERS_LIST.name} with no argument for why "
                f"following it is safe. Following a helper is what makes most of the "
                f"trapped set callable, the whole qpdf_oh_* family included; write it out."
            )
        if len(entry.get("sha256", "")) != 64:
            sys.exit(f"error: {name} in {HELPERS_LIST.name} has no usable sha256")
        recorded[name] = (entry.get("role", ""), entry["sha256"])
    return recorded


HELPER_REASONS = {
    "trap_oh_errors": (
        "Wraps its work in trap_errors. Its `fallback` runs outside the trap but takes no "
        "qpdf_data, so it cannot reach the document or the parser -- it can fail only by "
        "allocation, the defined-abort case qpdf-untrapped-accepted.toml already argues. "
        "Three other statements also sit outside the try, on the error path: "
        "qpdf->qpdf->getFilename(), qpdf->warnings.emplace_back(...), and a write of "
        "qpdf->error->what() to the default logger -- which would put file byte offsets on "
        "process stderr. All three are inside `if (!qpdf->silence_errors)`, and "
        "Document::open calls qpdf_silence_errors on every document, so the branch is dead "
        "in burrow. Named here because the proof rules do not know about that runtime "
        "setting: if the silencing call is ever dropped, this argument goes with it."
    ),
    "do_with_oh_void": (
        "A pure forwarder to `do_with_oh`, which is itself one: its whole body is a single "
        "call, with the caller's callback used only inside that call's argument list. There "
        "is no reachable code outside the proven call, which is what makes forwarding "
        "transitive where wrapping is not."
    ),
    "do_with_oh": (
        "A pure forwarder: one top-level return of trap_oh_errors(...), with the handle "
        "lookup and the caller's callback both inside that call's argument list, so a throw "
        "from either is inside the trap. There is no reachable code outside it."
    ),
}


def write_helpers(bodies: dict[str, tuple[str, str]], version: str) -> None:
    """Write the fingerprint file. `--generate` decides whether it is ALLOWED to; this writes."""
    lines = [
        "# The qpdf static helpers tools/check-qpdf-trapped.py is allowed to follow, recorded",
        "# by sha256 of their source bodies. GENERATED -- but not automatically re-accepted.",
        "#",
        "# A function reaches `trap_errors` in its own body, or through one of these helpers.",
        "# The proof rules (ADR 0013's 2026-09-13 amendment) are re-checked against the source",
        "# on every regeneration, so a helper that stops trapping stops being followed and every",
        "# function depending on it leaves the trapped list.",
        "#",
        "# THE RULES PROVE THESE BODIES, NOT ANY FUTURE ONES. A restructured helper that still",
        "# satisfies the rules would be re-proved with nobody reading it, and the argument is",
        "# subtle enough that passing is not the same as verified. So --generate REFUSES when a",
        "# recorded hash has moved, names the helper, and stops. A person re-reads it and runs",
        "# --generate --accept-helper-changes. Step 2 of 'Bumping an engine pin' in pins.toml.",
        "#",
        f"# qpdf version: {version}",
        "",
        "[meta]",
        f'qpdf_version = "{version}"',
        "",
    ]
    for name, (role, sha) in sorted(bodies.items()):
        why = " ".join(HELPER_REASONS.get(name, "").split())
        lines += [
            "[[helper]]",
            f'name = "{name}"',
            f'role = "{role}"',
            f'sha256 = "{sha}"',
            f'why = "{why}"',
            "",
        ]
    HELPERS_LIST.write_text("\n".join(lines))


def helper_changes(
    bodies: dict[str, tuple[str, str]],
    recorded: dict[str, tuple[str, str]] | None = None,
) -> list[str]:
    """What re-verification a person owes before this helper set can be accepted.

    `recorded` is a parameter rather than always being read from disk so the probes can drive
    this with a synthetic pair. The gate is the one part of the helper machinery that needs no
    vendor tree to exercise, and a gate nobody exercises until a version bump is a gate whose
    first run is the one that matters.
    """
    if recorded is None:
        if not HELPERS_LIST.is_file():
            return []
        recorded = load_helpers()
    changes: list[str] = []
    for name, (role, sha) in sorted(bodies.items()):
        if name not in recorded:
            changes.append(
                f"{name} ({role}) is newly followed and is not recorded. Read its body and "
                f"satisfy yourself it cannot let an exception escape before accepting it."
            )
        elif recorded[name][1] != sha:
            changes.append(
                f"{name} ({role}) still satisfies the proof rules, but its BODY HAS CHANGED "
                f"(recorded sha256 {recorded[name][1][:12]}…, now {sha[:12]}…). "
                f"The rules prove the body they were read against; re-read this one."
            )
    for name in sorted(recorded):
        if name not in bodies:
            changes.append(
                f"{name} is recorded as a proven helper but no longer satisfies the rules, so "
                f"nothing is followed through it. Every function that reached the trap only "
                f"that way has left the trapped list -- check what stopped being callable."
            )
    return changes

def generate(accept_helper_changes: bool = False) -> int:
    version = qpdf_version()
    sources = qpdf_sources(version)
    for path in sources:
        if not path.is_file():
            sys.exit(
                f"error: {path.relative_to(REPO)} not found. Run engines/fetch.sh first "
                f"-- engines/vendor/ is gitignored and reproducible from engines/pins.toml."
            )

    bodies: dict[str, tuple[str, str]] = {}
    routes_by_source: list[dict[str, str]] = []
    for path in sources:
        text = path.read_text()
        bodies.update(helper_bodies(text))
        routes_by_source.append(trapped_routes(text))

    # ONLY THE LOAD-BEARING ONES ARE RECORDED. `helper_bodies` returns every helper the rules
    # would follow; some are followed by nothing. `qpdf_oh_item_internal` is a proven forwarder
    # that no C API function routes through -- it is called from inside other functions'
    # lambdas, which is the trapped side. Pinning it would put a hash in the file that no
    # route depends on, and `verify` would then have to accept a recorded helper nothing uses,
    # which is exactly the staleness that check is for. If a future qpdf routes something
    # through it, it arrives as "newly followed" and gets read then.
    used = {
        part.strip()
        for routes in routes_by_source
        for route in routes.values()
        if route.startswith("via ")
        for part in route.removeprefix("via ").split("->")
        if part.strip()
    }
    bodies = {name: value for name, value in bodies.items() if name in used}

    # BEFORE ANYTHING IS WRITTEN. A changed helper body means the generated list would be
    # rebuilt on a proof nobody has re-read, and the list is what says a call is safe.
    changes = helper_changes(bodies)
    if changes and not accept_helper_changes:
        print(
            f"REFUSED — {len(changes)} proven-helper change(s) need re-verification by a "
            f"person before the trapped set can be regenerated:",
            file=sys.stderr,
        )
        for change in changes:
            print(f"  - {change}", file=sys.stderr)
        print(
            f"\nRe-read the helper(s) in {sources[0].parent.name}/qpdf-c.cc against the four "
            f"rules in ADR 0013's 2026-09-13 amendment, then re-run with "
            f"--accept-helper-changes.",
            file=sys.stderr,
        )
        return 1

    routes: dict[str, str] = {}
    for per_source in routes_by_source:
        routes.update(per_source)
    trapped = sorted(routes)
    if len(trapped) < 10:
        # The parser silently returning nothing would produce an empty list that failed
        # every declaration, or -- worse, if it were ever inverted -- passed everything.
        sys.exit(
            f"error: parsed only {len(trapped)} trapped functions out of qpdf-c.cc. "
            f"The parser is broken; qpdf 12.4.1 has 73."
        )

    # PER-ROUTE COUNTS, because the widening is exactly what a reader has to be able to
    # audit: which functions are trusted on their own body, and which on somebody else's
    # proof. A bare total hides the distinction the proof obligations exist to draw.
    by_route: dict[str, int] = {}
    for route in routes.values():
        by_route[route] = by_route.get(route, 0) + 1
    breakdown = "\n".join(
        f"#   {count:>3}  {route}" for route, count in sorted(by_route.items())
    )

    scanned = ", ".join(C_API_SOURCES)
    header = f"""\
# qpdf C API functions that route through `trap_errors`, GENERATED. Do not edit by hand.
#
# Source: engines/vendor/src/qpdf-{version}/libqpdf/{{{scanned}}}
# Regenerate: python3 tools/check-qpdf-trapped.py --generate
#
# Only functions listed here are safe to call from Rust: everything else can let a C++
# exception cross the FFI boundary, which aborts the process. `qpdf-c.h`'s blanket
# guarantee is not true per-function -- see ADR 0013 §1 and this script's docstring.
#
# REGENERATING THIS FILE IS PART OF EVERY qpdf VERSION BUMP. See "Bumping an engine pin"
# in engines/pins.toml.
#
# Each line is a function and the ROUTE by which it reaches `trap_errors`: "direct" if its
# own body calls the helper, "via <helper>" if it reaches it through a wrapper proven to wrap
# every return path in `trap_errors`, and "via <forwarder> -> <helper>" where a pure forwarder
# stands between. ADR 0013 §1's amendment records why indirect trapping is acceptable and what
# a helper has to prove to be followed.
#
# qpdf version: {version}
# functions: {len(trapped)}
# by route:
{breakdown}
"""
    TRAPPED_LIST.write_text(
        header + "\n".join(f"{name}\t{routes[name]}" for name in trapped) + "\n"
    )
    write_helpers(bodies, version)
    if changes:
        print(f"accepted {len(changes)} proven-helper change(s) on your word:")
        for change in changes:
            print(f"  - {change}")
    print(f"wrote {TRAPPED_LIST.relative_to(REPO)}: {len(trapped)} trapped functions (qpdf {version})")
    for route, count in sorted(by_route.items()):
        print(f"  {count:>3}  {route}")
    return 0


def verify() -> int:
    # FIXTURES FIRST. The per-source floors below catch a parser that falls silent; they do
    # not catch one that quietly finds one function fewer, which is the direction that lets an
    # untrapped call through.
    fixture_problems = check_parser_fixtures()
    if fixture_problems:
        print(
            f"FAILED — {len(fixture_problems)} parsing rule(s) do not behave as declared:",
            file=sys.stderr,
        )
        for problem in fixture_problems:
            print(f"  - {problem}", file=sys.stderr)
        return 1

    trapped = load_trapped()
    accepted = load_accepted()
    declared = declared_functions()
    problems: list[str] = []

    # The helper fingerprints are checked here too, not only on regeneration: verification runs
    # on a clean checkout with no vendor tree, so this is the half of the rule that reads only
    # committed files. It cannot hash a body it does not have; it CAN catch the file drifting
    # from the list it explains -- a route naming a helper nobody recorded, or a recorded helper
    # no route uses.
    def links(route: str) -> list[str]:
        r"""Every helper named in a route, not only the first.

        `re.findall(r"via (\w+)")` returned `['do_with_oh']` for
        `"via do_with_oh -> trap_oh_errors"` -- the `->` links were never extracted, so the
        check below held only because `trap_oh_errors` happens to appear as a FIRST link on
        two other functions. Remove those two and the wrapper 68 functions depend on could be
        deleted from the fingerprint file with verification still green. Found by code review.
        """
        if not route.startswith("via "):
            return []
        return [part.strip() for part in route.removeprefix("via ").split("->") if part.strip()]

    helpers = load_helpers()
    for name, route in sorted(trapped.items()):
        for helper in links(route):
            if helper not in helpers:
                problems.append(
                    f"{name} is listed as trapped {route!r}, but `{helper}` is not recorded in "
                    f"{HELPERS_LIST.name}. A helper followed without a recorded body is a "
                    f"proof nobody has pinned -- regenerate."
                )
    used = {helper for route in trapped.values() for helper in links(route)}
    for name in sorted(helpers):
        if name not in used:
            problems.append(
                f"`{name}` is recorded in {HELPERS_LIST.name} but no function reaches the trap "
                f"through it. Either the list is stale or something stopped being callable."
            )

    if not declared:
        sys.exit("error: parsed ZERO declarations. The check would be vacuous.")

    problems += wasm_binding_is_covered()

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
    print(
        f"  parsers verified against {len(FFI_CASES)} ffi.rs case(s), {len(JS_CASES)} "
        f"bridge case(s), a synthetic qpdf-c.cc, and {len(WRAPPER_PROBES_RUN)} wrapper "
        f"and fingerprint probes"
    )
    for name in WRAPPER_PROBES_RUN:
        print(f"    probe: {name}")
    print(
        f"  proven helpers, pinned by body hash: "
        + ", ".join(f"{name} ({role})" for name, (role, _) in sorted(helpers.items()))
    )
    print("  trapped:")
    for name in both:
        print(f"    {name}  [{trapped[name]}]")
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
    parser.add_argument(
        "--accept-helper-changes",
        action="store_true",
        help=(
            "re-record the proven helpers even though a body changed. Only after reading the "
            "changed body against ADR 0013's proof rules yourself."
        ),
    )
    args = parser.parse_args()
    if args.accept_helper_changes and not args.generate:
        sys.exit("error: --accept-helper-changes only means anything with --generate")
    return generate(args.accept_helper_changes) if args.generate else verify()


if __name__ == "__main__":
    sys.exit(main())
