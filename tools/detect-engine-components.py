#!/usr/bin/env python3
"""Detect third-party components inside engine artifacts and require them to be declared.

`check-engine-licences.py` asks "is every DECLARED licence allowed?".
This asks the other question: **"is every DETECTED component declared?"**

That second question is the one that mattered. HarfBuzz was linked into PDFium through a
whole ADR cycle with no licence file in the artifact and no entry in the manifest, and it
was found only because a human ran `nm` and noticed. ICU was recorded `linked = false`
for the same length of time while 489 of its symbols sat in the binary. Neither is
catchable by reading licence files, because the licence files were the thing that was
wrong.

Exits non-zero when a detected component has no entry in `engines/licenses.toml`.

## Honest limits

- **Fingerprints are heuristics.** A false positive costs a human a few minutes; a false
  negative costs a licence violation. Tuned accordingly, and kept readable rather than
  clever.
- **The wasm module has its names stripped**, so detection there is string-based and much
  weaker than the native symbol table. Native is authoritative for what PDFium contains;
  the wasm scan is a cross-check on the artifact users actually download.
- **It cannot detect a declared component whose version moved.** A PDFium bump that
  changes the bundled HarfBuzz version passes this check silently. Re-deriving the
  revisions from PDFium's `DEPS` is part of a bump — see ADR 0010.
"""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import sys
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
# Overridable so the tool's own test can point it at a manifest with an entry removed.
# Never set this in CI.
MANIFEST = Path(
    os.environ.get("BURROW_ENGINE_LICENSES", REPO / "engines" / "licenses.toml")
)
VENDOR = REPO / "engines" / "vendor"


@dataclass(frozen=True)
class Component:
    """A third-party component and how to recognise it in a binary.

    `manifest_names` are substrings; a manifest entry matches if any of them appears in
    its `name`. Kept loose on purpose, because manifest entries carry qualifiers like
    "(bundled in pdfium)".
    """

    label: str
    manifest_names: tuple[str, ...]
    # Symbol-name patterns, matched against demangled `nm` output (native only).
    symbols: tuple[str, ...] = ()
    # String patterns, matched against `strings` output (used for wasm, and as a
    # fallback for native).
    strings: tuple[str, ...] = ()
    # How many hits before we call it present. Above 1 for fingerprints that could
    # plausibly appear by coincidence.
    threshold: int = 1
    # PROBES. Text this component's fingerprints MUST detect, and text they must NOT.
    #
    # Checked on every run, against the real `detect()`, before any artifact is scanned.
    # Without them the hit counts this tool prints are a claim rather than a measurement: a
    # fingerprint that silently stopped matching would report "nothing detected" for a
    # component that is right there in the binary, and the only signal would be a number
    # nobody has a baseline for. `check-no-generated-files.sh` shipped with exactly that bug
    # -- 15 of 16 patterns inert while the output said 16 -- which is why every pattern set
    # in this repository now carries its own fixture.
    probe_symbols: str = ""
    probe_strings: str = ""
    near_miss: str = ""
    note: str = ""


# Ordered roughly by how much trouble each one has already caused.
COMPONENTS: tuple[Component, ...] = (
    Component(
        label="harfbuzz",
        manifest_names=("harfbuzz",),
        symbols=(r"\bhb_[a-z_]+\b",),
        strings=(r"HB_FONT_FUNCS", r"HB_SCRIPT_", r"harfbuzz"),
        note="Linked via PDFium's third_party/harfbuzz. Ships NO licence file in the "
        "artifact -- the whole reason this tool exists. See ADR 0010.",
        probe_symbols="hb_font_create hb_buffer_add_utf8",
        probe_strings="HB_FONT_FUNCS harfbuzz",
        near_miss="xhb_font_create Thb_buffer",
    ),
    Component(
        label="icu",
        manifest_names=("icu",),
        # ICU exports version-suffixed C symbols (ubidi_setPara_78) and, in C++,
        # namespaced RTTI names that only exist if the classes were compiled in.
        #
        # `[A-Za-z_]`, not `[a-z_]`: the character class used to be lowercase-only, which
        # rejected `ubidi_setPara_78` -- the example in the line above. It matched only the
        # all-lowercase minority (`ubidi_close_78`, `udata_open_78`), so it worked by
        # accident on a real artifact and would have gone on doing so. Found by the probe
        # below, which is exactly the case probes exist for.
        symbols=(r"\b(u|ubidi|ucase|ucptrie|udata|uscript|unorm2)_[A-Za-z_]*_\d\d\b",),
        strings=(r"N6icu_\d\d\d?\d?[A-Za-z]+E", r"icudt\d\d"),
        threshold=3,
        note="Arrives via HarfBuzz's hb-icu integration, NOT via XFA. ADR 0008 "
        "recorded linked=false on that mistaken reasoning; ADR 0010 corrects it.",
        probe_symbols="ubidi_setPara_78 ucase_addCaseClosure_78 u_charType_78",
        probe_strings="N6icu_78UnicodeStringE icudt78 N6icu_78LocaleE",
        near_miss="ubidi_setPara ucase_toFullLower u_charType",
    ),
    Component(
        label="freetype",
        manifest_names=("freetype",),
        symbols=(r"\bFT_(Init|Done|Load|New|Get|Set)[A-Za-z_]*\b",),
        strings=(r"FREETYPE_PROPERTIES", r"tt-glyf", r"tt-cmaps"),
        note="FTL: carries a mandatory binary-distribution credit line.",
        probe_symbols="FT_Init_FreeType FT_Load_Glyph",
        probe_strings="FREETYPE_PROPERTIES tt-glyf",
        near_miss="FT_Render_Glyph FT_Outline_Decompose",
    ),
    Component(
        label="libjpeg",
        manifest_names=("libjpeg", "jpeg"),
        symbols=(r"\bj(peg|init|copy)_[a-z_]+\b",),
        strings=(r"Bogus Huffman table", r"Unsupported JPEG", r"Independent JPEG Group"),
        note="IJG: carries a mandatory documentation credit line.",
        probe_symbols="jpeg_read_header jinit_memory_mgr",
        probe_strings="Independent JPEG Group",
        near_miss="jxx_read_header jfoo_bar",
    ),
    Component(
        label="libpng",
        manifest_names=("libpng",),
        symbols=(r"\bpng_(create|read|write|set|get)_[a-z_]+\b",),
        strings=(r"libpng version", r"png_create_read_struct"),
        note="Shipped in PDFium's licences but not found linked; declared anyway.",
        probe_symbols="png_create_read_struct png_set_sig_bytes",
        probe_strings="libpng version 1.6",
        near_miss="png_destroy_read_struct png_free_data",
    ),
    Component(
        label="zlib",
        manifest_names=("zlib",),
        symbols=(r"\b(inflate|deflate)(Init2?|End|Reset)?\b",),
        strings=(r"incorrect header check", r"invalid distance too far back"),
        threshold=2,
        note="PDFium bundles its own copy in addition to the one we vendor.",
        probe_symbols="inflateInit2 deflateEnd",
        probe_strings="incorrect header check\ninvalid distance too far back",
        near_miss="deflated inflator",
    ),
    Component(
        label="agg23",
        manifest_names=("agg",),
        # PDFium namespaces it, so a bare `agg::` search misses it entirely.
        symbols=(r"pdfium::agg::", r"\bagg::"),
        strings=(r"outline_aa", r"rasterizer_scanline", r"vcgen_stroke"),
        note="AGG 2.3 only -- 2.4+ is GPL. LicenseRef-AGG-2.3.",
        probe_symbols="pdfium::agg::rasterizer_scanline_aa agg::path_storage",
        probe_strings="outline_aa rasterizer_scanline",
        near_miss="xagg::foo myagg::bar",
    ),
    Component(
        label="libopenjpeg",
        manifest_names=("libopenjpeg", "openjpeg"),
        symbols=(r"\bopj_[a-z_]+\b",),
        # `\bopj_`, not a bare `opj_`: unanchored, it matched any substring -- `xopj_image`
        # counted as two hits and cleared the threshold. Found by the near-miss probe.
        strings=(r"\bopj_[a-z_]+", r"openjpeg"),
        threshold=2,
        probe_symbols="opj_image_create opj_stream_destroy",
        probe_strings="opj_image_create opj_stream_destroy openjpeg",
        near_miss="xopj_image yopj_stream",
    ),
    Component(
        label="lcms",
        manifest_names=("lcms",),
        symbols=(r"\bcms[A-Z][A-Za-z]+\b",),
        strings=(r"little cms", r"lcms"),
        threshold=2,
        probe_symbols="cmsCreateContext cmsOpenProfileFromMem",
        probe_strings="little cms lcms",
        near_miss="cmsopen cms_create",
    ),
)


@dataclass
class Artifact:
    """One engine binary to scan."""

    path: Path
    kind: str  # "native" or "wasm"
    detected: dict[str, int] = field(default_factory=dict)


def find_artifacts() -> list[Artifact]:
    """Locate every engine artifact present in the vendor tree.

    Missing artifacts are not an error: a contributor may have built only their host
    architecture, and the wasm tree is optional. Having NONE is an error, because then
    this tool would pass while checking nothing.
    """
    out: list[Artifact] = []
    for so in sorted(VENDOR.glob("native-*/lib/libpdfium.so")):
        out.append(Artifact(so, "native"))
    for wasm in sorted(VENDOR.glob("wasm/lib/*.wasm")):
        # Skip our own probe modules: they exist to prove a link works and contain no
        # third-party code, so "nothing detected" there is expected, not a signal.
        if "probe" in wasm.name:
            continue
        out.append(Artifact(wasm, "wasm"))
    return out


def symbol_text(path: Path) -> str:
    """Demangled defined symbols, or an empty string if `nm` cannot read the file."""
    if not shutil.which("nm"):
        return ""
    try:
        r = subprocess.run(
            ["nm", "--defined-only", "-C", str(path)],
            capture_output=True,
            text=True,
            timeout=180,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return ""
    return r.stdout


def string_text(path: Path) -> str:
    """Printable strings, or an empty string if `strings` is unavailable."""
    if not shutil.which("strings"):
        return ""
    try:
        r = subprocess.run(
            ["strings", "-a", str(path)],
            capture_output=True,
            text=True,
            timeout=180,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return ""
    return r.stdout


def detect(syms: str, strs: str) -> dict[str, int]:
    """Hit count per component label, for one artifact's symbol and string text.

    Split out of `scan` so the probes below exercise **this** function rather than a
    re-implementation of it. A probe that tested a copy of the matching logic would pass
    while the real path was broken, which is the failure it exists to prevent.
    """
    detected: dict[str, int] = {}
    for comp in COMPONENTS:
        hits = 0
        for pat in comp.symbols:
            if syms:
                hits += len(re.findall(pat, syms))
        # Strings are the only signal for wasm, and a useful fallback for a stripped
        # native library.
        if hits < comp.threshold:
            for pat in comp.strings:
                if strs:
                    hits += len(re.findall(pat, strs))
        if hits >= comp.threshold:
            detected[comp.label] = hits
    return detected


def scan(artifact: Artifact) -> None:
    """Fill in `artifact.detected` with a hit count per component label."""
    syms = symbol_text(artifact.path) if artifact.kind == "native" else ""
    strs = string_text(artifact.path)
    artifact.detected = detect(syms, strs)


def check_probes() -> list[str]:
    """Every fingerprint must detect its own probe and reject its near-miss.

    Run before any artifact is scanned, on every invocation. Returns a list of problems.
    """
    problems: list[str] = []
    for comp in COMPONENTS:
        if not comp.probe_symbols or not comp.probe_strings or not comp.near_miss:
            problems.append(f"{comp.label}: no probe fixtures; the fingerprint is untested")
            continue

        # Positive, through the symbol path and the string path separately -- for a wasm
        # artifact the string patterns are the ONLY signal, so a broken string pattern
        # behind a working symbol pattern would be invisible.
        if comp.label not in detect(comp.probe_symbols, ""):
            problems.append(
                f"{comp.label}: its symbol fingerprints do not detect their own probe "
                f"({comp.probe_symbols!r}) -- the pattern is inert"
            )
        if comp.label not in detect("", comp.probe_strings):
            problems.append(
                f"{comp.label}: its string fingerprints do not detect their own probe "
                f"({comp.probe_strings!r}) -- the pattern is inert, and strings are the "
                f"only signal for wasm"
            )
        # Negative. A fingerprint loose enough to match anything reports every artifact as
        # containing every component, which is as useless as matching nothing.
        if comp.label in detect(comp.near_miss, comp.near_miss):
            problems.append(
                f"{comp.label}: its fingerprints match the near-miss "
                f"({comp.near_miss!r}) -- too loose to distinguish anything"
            )
    return problems


def declared_labels() -> tuple[set[str], int]:
    """Component labels the manifest declares, plus the total entry count."""
    with MANIFEST.open("rb") as fh:
        data = tomllib.load(fh)
    entries = data.get("component", [])
    labels: set[str] = set()
    for entry in entries:
        name = str(entry.get("name", "")).lower()
        for comp in COMPONENTS:
            if any(m in name for m in comp.manifest_names):
                labels.add(comp.label)
    return labels, len(entries)


def main() -> int:
    if not MANIFEST.is_file():
        print(f"error: {MANIFEST.relative_to(REPO)} not found", file=sys.stderr)
        return 1

    # PROBES FIRST. If a fingerprint cannot detect its own probe, every count this tool
    # prints afterwards is meaningless -- and "nothing detected" would read as "nothing
    # there". Fail before scanning rather than reporting numbers nobody can trust.
    probe_problems = check_probes()
    if probe_problems:
        print(
            f"FAILED — {len(probe_problems)} fingerprint(s) do not behave as declared:",
            file=sys.stderr,
        )
        for problem in probe_problems:
            print(f"  - {problem}", file=sys.stderr)
        print(
            "\n  A fingerprint that matches nothing reports a linked component as absent;\n"
            "  one that matches everything reports every component as present. Both make\n"
            "  this tool's output a claim rather than a measurement.",
            file=sys.stderr,
        )
        return 1

    artifacts = find_artifacts()
    if not artifacts:
        print(
            "error: no engine artifacts found under engines/vendor/.\n"
            "Run: engines/fetch.sh && engines/build-native.sh (and build-wasm.sh)\n"
            "Refusing to pass without scanning anything.",
            file=sys.stderr,
        )
        return 1

    for art in artifacts:
        scan(art)

    declared, total = declared_labels()

    broken: list[str] = []

    print(f"{MANIFEST.name}: {total} entries")
    print(f"{len(COMPONENTS)} fingerprint(s), all verified against their own probes")
    print(f"scanned {len(artifacts)} artifact(s):\n")
    for art in artifacts:
        rel = art.path.relative_to(REPO)
        print(f"  {rel} ({art.kind})")
        if not art.detected:
            # For PDFium this means the fingerprints have broken, which is worse than a
            # false positive: the tool would pass while checking nothing.
            if "pdfium" in art.path.name:
                print(
                    "    !! NOTHING DETECTED in a PDFium artifact -- the fingerprints "
                    "are broken",
                    file=sys.stderr,
                )
                broken.append(str(art.path.relative_to(REPO)))
            else:
                print("    (nothing detected)")
        for label, hits in sorted(art.detected.items()):
            mark = "ok " if label in declared else "!! "
            print(f"    {mark}{label:<14} {hits:>6} hits")
    print()

    missing: dict[str, list[str]] = {}
    for art in artifacts:
        for label in art.detected:
            if label not in declared:
                missing.setdefault(label, []).append(str(art.path.relative_to(REPO)))

    if broken:
        print(
            f"FAILED — no components detected in: {', '.join(broken)}.\n"
            "  A PDF engine with no detectable third-party code means the fingerprints\n"
            "  in this tool no longer match the artifact. Fix the tool, do not ignore it.",
            file=sys.stderr,
        )
        return 1

    if missing:
        print(f"FAILED — {len(missing)} detected component(s) not declared:", file=sys.stderr)
        for label, where in sorted(missing.items()):
            comp = next(c for c in COMPONENTS if c.label == label)
            print(f"  - {label}: found in {', '.join(where)}", file=sys.stderr)
            if comp.note:
                print(f"      {comp.note}", file=sys.stderr)
            print(
                "      Add it to engines/licenses.toml with its licence, version and"
                " evidence.\n"
                "      If its licence is not on the allowlist, that needs an ADR --"
                " not an edit here.",
                file=sys.stderr,
            )
        return 1

    print("OK — every detected component is declared in engines/licenses.toml.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
