#!/usr/bin/env python3
"""Every member the worker's hand-written declarations name must exist in the generated ones.

WHY THIS EXISTS, measured.

`RenderedPage::pixel_length` and `take_pixels` were declared with `#[wasm_bindgen(getter)]` and
no `js_name`, so wasm-bindgen exposed them as `pixel_length` and `take_pixels`. The worker reads
`drawn.pixelLength` and `drawn.takePixels()`, which `Reply` had taught it -- `Reply` carries
`js_name = outputLength` and `js_name = takeOutput`.

Nothing failed. `undefined > 0` is `false`, so the worker took the empty branch, never called
the missing method, threw nothing, and posted a **zero-byte** pixel buffer for every page of
every strip while reporting success. A blank strip indistinguishable from a working one.

`tsc -p src/worker` could not see it: `apps/web/src/worker/globals-*.d.ts` are hand-written
ambient declarations, and the worker was being checked against a file that agreed with it and
disagreed with the binding. Every other member of that interface matched, which is what made
the two odd ones out invisible.

So the expected set is DERIVED from `pkg*/burrow_wasm.d.ts` rather than restated -- the same
shape `tools/check-wasm-exports.sh` uses, and the reason that one works.

WHAT THIS CHECKS, AND WHAT IT DOES NOT

It compares MEMBER NAMES on the interfaces both files describe. It does not compare types, and
it does not check free functions -- `check-wasm-exports.sh` covers the export list. A name the
hand-written file declares and the generated file does not is the failure above and is refused.
A name the generated file has and the hand-written one does not is reported as an observation,
not a failure: the worker is entitled to ignore part of the binding.

Usage: tools/check-wasm-binding-names.py
"""

from __future__ import annotations

import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent
WORKER = REPO / "apps" / "web" / "src" / "worker"

# Which generated package backs which hand-written declaration file, and which interfaces in it
# describe a wasm-bindgen type. `BurrowWasm` is the module object and is covered by
# `check-wasm-exports.sh`; these are the CLASSES, whose members carry the `js_name` hazard.
PAIRS = [
    (
        REPO / "bindings" / "burrow-wasm" / "pkg" / "burrow_wasm.d.ts",
        WORKER / "globals-qpdf.d.ts",
        # Nothing today: the base artifact's only class, `SplitSession`, is declared in the
        # shared `globals.d.ts` below. Listed so a class added here is covered on arrival.
        ["SplitSession"],
    ),
    (
        REPO / "bindings" / "burrow-wasm" / "pkg-render" / "burrow_wasm.d.ts",
        WORKER / "globals-render.d.ts",
        ["RenderSession", "RenderedPage"],
    ),
    (
        REPO / "bindings" / "burrow-wasm" / "pkg" / "burrow_wasm.d.ts",
        WORKER / "globals.d.ts",
        ["Reply", "WebLimits", "SplitSession"],
    ),
]

# A member declaration: `name(...)`, `readonly name:` or `name:`. Comments and blank lines are
# skipped by requiring the line to start with an identifier.
MEMBER = re.compile(r"^\s*(?:readonly\s+)?(\w+)\s*[(:]")


def members(text: str, interface: str) -> set[str] | None:
    """The member names declared on `interface`, or None if it is not declared at all."""
    pattern = re.compile(
        rf"(?:export\s+)?(?:class|interface)\s+{re.escape(interface)}\b[^{{]*{{", re.M
    )
    match = pattern.search(text)
    if not match:
        return None
    depth = 0
    body_start = match.end()
    for index in range(match.end() - 1, len(text)):
        if text[index] == "{":
            depth += 1
        elif text[index] == "}":
            depth -= 1
            if depth == 0:
                body = text[body_start:index]
                break
    else:
        return None
    # Strip block comments. Belt and braces rather than load-bearing: `MEMBER` anchors on the
    # start of the line and a doc-comment line begins with `*`, which is not `\w`. Kept because
    # the alternative is a member pattern whose correctness depends on a fact about how
    # wasm-bindgen happens to indent its output.
    body = re.sub(r"/\*.*?\*/", "", body, flags=re.S)
    return {m.group(1) for line in body.splitlines() if (m := MEMBER.match(line))}


def main() -> int:
    print("check-wasm-binding-names: hand-written worker declarations vs the generated ones")
    problems: list[str] = []
    compared = 0
    interfaces = 0

    for generated_path, declared_path, names in PAIRS:
        if not generated_path.exists():
            problems.append(
                f"{generated_path.relative_to(REPO)} is missing -- run wasm-pack for both "
                f"artifacts before this check, or it compares nothing"
            )
            continue
        if not declared_path.exists():
            problems.append(f"{declared_path.relative_to(REPO)} is missing")
            continue
        generated = generated_path.read_text(encoding="utf-8")
        declared = declared_path.read_text(encoding="utf-8")

        for name in names:
            hand = members(declared, name)
            if hand is None:
                # Declared in a sibling file; not this pair's business.
                continue
            real = members(generated, name)
            if real is None:
                problems.append(
                    f"{name}: declared in {declared_path.name} and absent from "
                    f"{generated_path.parent.name}/burrow_wasm.d.ts -- the worker is written "
                    f"against a type the binding does not export"
                )
                continue
            interfaces += 1
            compared += len(hand)
            missing = sorted(hand - real)
            if missing:
                problems.append(
                    f"{name}: {declared_path.name} declares {', '.join(missing)}, which the "
                    f"binding does not expose. Reading one is `undefined`, which throws "
                    f"nothing and silently takes the wrong branch -- add a `js_name` in "
                    f"bindings/burrow-wasm, or fix the declaration."
                )
            extra = sorted(real - hand)
            if extra:
                print(f"  {name}: {len(hand)} declared, {len(extra)} generated member(s) unused"
                      f" ({', '.join(extra)})")
            else:
                print(f"  {name}: {len(hand)} declared member(s), all present")

    if interfaces == 0:
        print(
            "\nFAILED — no interface was compared, so this check examined nothing. That reads "
            "exactly like success, which is why it is a failure.",
            file=sys.stderr,
        )
        return 1

    if problems:
        print("\nFAILED — a hand-written declaration names something the binding does not:",
              file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 1

    print(f"OK — {compared} declared member(s) across {interfaces} interface(s) all exist.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
