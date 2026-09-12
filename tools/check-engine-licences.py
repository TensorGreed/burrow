#!/usr/bin/env python3
"""Validate engines/licenses.toml against ADR 0008's allowlist.

cargo-deny covers Rust crates. This covers the native engines and everything they
bundle, which is the other 82% of the shipped bytes. It runs in the same CI job as
cargo-deny so both halves of the dependency graph are gated together.

Exits non-zero on any violation. Prints every problem rather than stopping at the first,
because a licensing review wants the whole picture.

What this CANNOT do: detect a component nobody declared. The manifest is
hand-maintained, so this check is a floor, not a guarantee -- see the note at the top of
engines/licenses.toml.
"""

from __future__ import annotations

import pathlib
import sys
import tomllib

REPO = pathlib.Path(__file__).resolve().parent.parent
MANIFEST = REPO / "engines" / "licenses.toml"

# The only two directories a `license_text` may name. Every committed licence text is in one
# of them; engines/licences/README.md says why the second exists.
#
# A CONTAINMENT CHECK, NOT TIDINESS. `license_text` is a free-form string from a manifest,
# and it is read and embedded verbatim into the published credits page by
# tools/generate-credits.mjs. Without this, one line of a file reviewers skim as a manifest
# rather than as code could publish the contents of any path on the build machine.
LICENCE_ROOTS = (REPO / "engines" / "licences", REPO / "docs" / "adr" / "licences")

# ADR 0008's allowlist. Keep in sync with deny.toml's `allow` (Rust crates) and the
# lists in CLAUDE.md, .claude/agents/license-auditor.md, and the add-dependency skill.
#
# Changing this set is a POLICY change: it requires a new ADR, not an edit here. See
# docs/adr/0008-widened-licence-allowlist.md and its amendment
# docs/adr/0010-harfbuzz-and-icu-in-pdfium.md.
ALLOWED = {
    # From ADR 0003.
    "MIT",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "ISC",
    "Zlib",
    "MPL-2.0",
    "OFL-1.1",
    "Unicode-3.0",
    "CC0-1.0",
    "Unlicense",
    # Added by ADR 0008, for bundled native engine components.
    "FTL",
    "IJG",
    "libpng-2.0",
    "LicenseRef-AGG-2.3",
    # Added by ADR 0010, for components bundled inside PDFium that ADR 0008 missed.
    # HarfBuzz 14.3.1 -- SPDX id verified byte-for-byte against SPDX's canonical text,
    # and its notice is committed at docs/adr/licences/ because the artifact ships none.
    "MIT-Modern-Variant",
    # ICU's legacy "ICU 1.8.1 to 57.1" section. X11-style. Admitted rather than argued
    # inapplicable at ICU 78.2 -- see ADR 0010 for why that argument was refused.
    "ICU",
}

# Never acceptable. Listed explicitly so the failure message says why rather than just
# "not allowed" -- these are the ones that would force burrow's own licence to change.
FORBIDDEN_MARKERS = ("GPL", "AGPL", "SSPL", "LGPL", "CC-BY-NC", "NONCOMMERCIAL")


def split_expression(expr: str) -> list[str]:
    """Split a simple SPDX expression into its constituent licence ids.

    Handles the `A AND B`/`A OR B` forms the manifest uses. Deliberately not a full SPDX
    parser: if an expression ever needs one, that is a signal the manifest entry should
    be split into separate components instead.
    """
    parts = expr.replace("(", " ").replace(")", " ").split()
    out: list[str] = []
    buf: list[str] = []
    for tok in parts:
        if tok in {"AND", "OR"}:
            if buf:
                out.append(" ".join(buf))
                buf = []
        else:
            buf.append(tok)
    if buf:
        out.append(" ".join(buf))
    return out


def check_license_text(comp: dict, name: str) -> list[str]:
    """Every `linked` component must carry a committed copy of its licence text.

    `license_file` points at the audited original under `engines/vendor/`, which is
    gitignored. The website credits page is generated from this manifest at build time and
    has to build from a clean checkout, and CI's web job stages only the *wasm* vendor
    prefix while most `license_file` paths name `vendor/native-aarch64/`. So the text that
    ships is a committed copy under `engines/licences/`, recorded as `license_text`.

    Committing a copy introduces exactly one failure mode -- the copy drifting from the
    original -- so **when the vendor tree is present the two are compared byte for byte**.
    Absent (a clean checkout, or CI's licence job, which fetches nothing) the comparison is
    skipped rather than failed: it would make this check unrunnable where it is most useful.
    """
    if not comp.get("linked"):
        # A component that is not in any shipped artifact carries no notice obligation we
        # have to discharge in the product, so it needs no shipped copy.
        return []

    rel = comp.get("license_text")
    if rel is None:
        return [
            f"{name}: linked, but no `license_text` recorded. Every linked component "
            f"needs a committed copy under engines/licences/ -- see its README"
        ]

    copy = (REPO / rel).resolve()
    if not any(copy == root or root in copy.parents for root in LICENCE_ROOTS):
        return [
            f"{name}: `license_text` {rel} resolves outside the committed licence "
            f"directories. Licence text must live in engines/licences/ or docs/adr/licences/."
        ]
    if not copy.is_file():
        return [f"{name}: `license_text` {rel} does not exist"]
    if copy.stat().st_size == 0:
        return [f"{name}: `license_text` {rel} is empty"]

    original_rel = comp.get("license_file") or ""
    # `emsdk:...` is a pseudo-path into the pinned SDK install, not into engines/vendor/,
    # so there is nothing here to resolve. The SDK is version-pinned in engines/pins.toml
    # and verifies its own downloads; engines/licences/README.md records the exemption.
    if original_rel.startswith("emsdk:"):
        return []
    # agg23 and harfbuzz point `license_text` at the same committed file as `license_file`,
    # because those texts exist nowhere else. Comparing a file with itself proves nothing.
    original = REPO / "engines" / original_rel if original_rel.startswith("vendor/") else REPO / original_rel
    if original.resolve() == copy.resolve():
        return []
    if not original.is_file():
        # No vendor tree. Not a failure: see the docstring.
        return []
    if original.read_bytes() != copy.read_bytes():
        return [
            f"{name}: `license_text` {rel} has drifted from the audited original "
            f"{original_rel}. Re-copy it; do not edit either by hand"
        ]
    return []


def main() -> int:
    if not MANIFEST.is_file():
        print(f"error: {MANIFEST.relative_to(REPO)} not found", file=sys.stderr)
        return 1

    with MANIFEST.open("rb") as fh:
        data = tomllib.load(fh)

    components = data.get("component", [])
    if not components:
        print("error: manifest declares no components", file=sys.stderr)
        return 1

    problems: list[str] = []
    notices: list[tuple[str, str]] = []
    linked = 0

    for comp in components:
        name = comp.get("name", "<unnamed>")
        expr = comp.get("license")
        if not expr:
            problems.append(f"{name}: no `license` field")
            continue

        if comp.get("linked"):
            linked += 1
        if comp.get("notice_required"):
            notices.append((name, comp["notice_required"]))

        # A test-only component never reaches a shipped artifact, so its licence is not
        # bound by the distribution allowlist. It must say so explicitly.
        test_only = comp.get("test_only", False)

        for lic in split_expression(expr):
            upper = lic.upper()
            if any(m in upper for m in FORBIDDEN_MARKERS):
                problems.append(
                    f"{name}: '{lic}' is forbidden outright "
                    f"(copyleft or non-commercial); this cannot be waived"
                )
            elif lic not in ALLOWED and not test_only:
                problems.append(
                    f"{name}: '{lic}' is not on ADR 0008's allowlist. "
                    f"Adding it requires a new ADR, not an edit to this script"
                )

        if comp.get("license_file") is None:
            problems.append(f"{name}: no `license_file` recorded")

        problems.extend(check_license_text(comp, name))

    print(f"engines/licenses.toml: {len(components)} components, {linked} linked")

    if notices:
        print(f"\n{len(notices)} affirmative notice obligation(s) — these bind")
        print("executable-only distribution and must appear in THIRD_PARTY_NOTICES.md,")
        print("the website credits page, and both apps' licences screens:")
        for name, text in notices:
            print(f"  - {name}: {text}")

    if problems:
        print(f"\nFAILED — {len(problems)} problem(s):", file=sys.stderr)
        for p in problems:
            print(f"  - {p}", file=sys.stderr)
        return 1

    print("\nOK — every declared licence is on ADR 0008's allowlist.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
