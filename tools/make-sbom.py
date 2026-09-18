#!/usr/bin/env python3
"""Generate `sbom/burrow.cdx.json`, and check it against the things it claims to describe.

    python3 tools/make-sbom.py            # regenerate and write
    python3 tools/make-sbom.py --check    # regenerate, compare, cross-check; write nothing

# What this is FOR, because a CycloneDX file nobody reads is a check that examines nothing

`docs/ROADMAP.md` calls an SBOM the part that answers *what went into the build* -- the question
`tools/check-live-routes.py` explicitly cannot answer, because byte-identity between our build
and the reader's browser says nothing about what our build contained.

That is a reason to HAVE one. It is not a reason to trust one. This repository has measured four
separate checks that reported success while examining nothing, so this file is generated **and
verified**. Stated precisely, because an earlier draft of this paragraph claimed three mutually
cross-checking sources and a review measured that only one of the three comparisons could fire:

  * **Two inputs are combined into the document**: `cargo metadata --locked` (every Rust crate,
    with the licence Cargo records) and `engines/licenses.toml` (the native C/C++ components,
    which Cargo cannot see at all). They describe disjoint sets and cannot contradict each other.
    What binds them is that the document is regenerated on every check and **diffed against the
    committed one**, so neither input can move without the diff naming what changed.
  * **One source is independent and can contradict both**: `tools/detect-engine-components.py`,
    which reads what is ACTUALLY INSIDE the shipped binaries by symbol inspection.

**The third is the one that makes this more than a tidy restatement of hand-maintained files.**
HarfBuzz was linked into PDFium through an entire ADR cycle with no entry in the manifest, and
ICU sat at `linked = false` while 489 of its symbols were in the binary. A supply-chain document
derived only from files people maintain by hand inherits their mistakes, and we have measured
instances. So a component detected in a shipped artifact and missing from this SBOM is an error,
not a note.

# The third source is only worth having if it cannot go quiet

Two reviews measured the same hole in the first version of this file: with the vendor tree fully
present and every fingerprint made inert, `--check` printed *"NO artifacts scanned -- the vendor
tree is absent"* and exited **0**. Both halves were false, and the verdict line then claimed
agreement with binaries nothing had compared. Three things close it, and each is load-bearing:

  1. `detector.check_probes()` runs FIRST, here, on every invocation. Importing the detector as a
     module rather than shelling out to it skips its `main()`, and `main()` is where its own probe
     gate lived -- so this file must carry it rather than assume the neighbouring CI step ran.
  2. **Artifacts scanned and labels found are counted separately.** The old message conflated them,
     so "the fingerprints broke" and "there is no vendor tree" were indistinguishable.
  3. **An absent vendor tree FAILS** unless `--allow-no-artifacts` says otherwise. The default for
     a check is to refuse to pass over the source that gives it its value.

# What is gated, and what is only reported -- the difference is measured, not assumed

For a **native** artifact the symbol table is complete, and the manifest's `linked_in` and the
detected set agree exactly (8 of 8 on `libpdfium.so`, measured). That is a knowable expectation,
so it is an equality gate: a component declared linked and not found, or found and not declared
linked, fails.

Two ways that gate was measured switching itself off, both now closed and both worth knowing
about, because each turned a hard check into a printed note without changing a word of the claim:

  * **An artifact no `[[artifact]]` names.** The id is resolved from a hand-maintained `files`
    list, so an ordinary manifest edit could unhook it. It is a *failure* for a native artifact
    now, not a note -- and the count of native artifacts actually gated is printed.
  * **A label that is not distinct.** `manifest_names` are substrings, and `("libjpeg", "jpeg")`
    matched `libopenjpeg`, so the IJG-licensed libjpeg-turbo entry could be deleted from the
    manifest while a BSD-2-Clause component stood in for it. `check_probes()` now requires every
    fingerprint's own label to match only itself.

**What remains ungated, stated rather than implied.** The gate covers the native PDFium artifact
and the eight components the detector fingerprints. It does NOT cover the components with no
fingerprint at all -- `pdfium` itself, abseil, fast_float, the LLVM runtimes, qpdf, sphlib,
rijndael, the Emscripten runtime -- whose `linked_in` lists nothing verifies, nor the qpdf, zlib
and libjpeg static archives, which are not scanned. A periodic audit is still what covers those.

For a **wasm** artifact the module's names are stripped and strings are the only signal, so
detection is genuinely weaker -- `lcms` and `libjpeg` are declared linked in `pdfium.wasm` and are
not detectable there. Gating on equality would fail for a reason that is not a defect. So wasm
divergence is **reported by name** instead, per CLAUDE.md's rule that where the expected count is
not knowable the check says what it examined rather than printing a bare total.

# Determinism, which is what makes "regenerate and compare" possible

**No timestamp, no serial number, no build metadata.** CycloneDX permits all three and each one
would make the document differ from itself on every run -- which would turn the drift check into
noise a reader learns to ignore, and it is the drift check that gives this file its value. What
identifies a version of this document is the content, and `git` already records when it changed
and who changed it.

# What this does NOT claim

* It describes the components, not their vulnerabilities. There is no CVE feed here.
* `scope: "optional"` means `linked = false`: the component ships in the package and its code was
  not found in the binary. It does NOT mean "may not be distributed", and every notice obligation
  applies to an optional component exactly as it does to a required one. `burrow:linked` and
  `burrow:artifacts` carry the two facts separately, and are the fields to read.
* The detector's own limits are inherited: fingerprints are heuristics, the wasm module's names
  are stripped so its scan is weaker than the native symbol table, and **a declared component
  whose version moved is invisible to it**. Re-deriving revisions on a PDFium bump is ADR 0010's
  job, not this file's.
* `cargo metadata --locked` describes the workspace as locked, not as built: a crate excluded by
  a feature flag is still listed. That over-declares, which is the safe direction for a document
  whose failure mode is omitting something that shipped.
* **Over-declaring a licence obligation is NOT the safe direction**, which is why engine scope is
  carried rather than flattened -- see `engine_components`.
* An `OR` expression is recorded as the crate declares it. The SBOM does not say which branch
  burrow elects; `deny.toml` is the record of that.
* Nothing here validates the document against the CycloneDX 1.5 schema. The shape is asserted by
  construction and by the drift diff, not by a validator.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import os
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import Any

REPO = Path(__file__).resolve().parent.parent

#: Overridable so the self-test can point every input somewhere else without touching the real
#: files. The detector does the same with `BURROW_ENGINE_LICENSES` and `BURROW_ENGINE_VENDOR`,
#: and `tools/test-make-sbom.sh` relies on all three honouring it -- a test that edited the
#: committed manifest would be a test that can leave the repository broken when it fails.
#: NEVER SET THESE IN CI. `--check` prints all three resolved paths for that reason: a run
#: against redirected inputs must not read in the log like a run against the real ones.
MANIFEST = Path(os.environ.get("BURROW_ENGINE_LICENSES", REPO / "engines" / "licenses.toml"))
SBOM = Path(os.environ.get("BURROW_SBOM", REPO / "sbom" / "burrow.cdx.json"))
DETECTOR = REPO / "tools" / "detect-engine-components.py"

#: CycloneDX spec version this document is written to.
SPEC_VERSION = "1.5"

#: Cargo's pre-SPDX spellings. `MIT/Apache-2.0` is not a valid SPDX expression, and a consumer
#: that parses `licenses[].expression` rejects or silently drops it -- which turns a licence
#: statement into no licence statement, in a document whose whole job is licence statements.
LEGACY_SPDX = "/"


def relative(path: Path) -> str:
    """`path` relative to the repository if it is inside it, else the path as given.

    Not `Path.relative_to`: the self-test points `BURROW_SBOM` outside the repository, and a
    ValueError raised while composing an error message replaces that message with a traceback.
    """
    try:
        return str(path.resolve().relative_to(REPO))
    except ValueError:
        return str(path)


def load_detector() -> Any:
    """Import the detector as a module, so its fingerprints have ONE implementation.

    Imported rather than shelled out to and parsed: a second copy of "what counts as HarfBuzz"
    is a second thing to keep current, and the one that rots is always the copy.

    The cost of importing rather than running is that `main()` never executes, and `main()` is
    where the detector runs its own probe gate. `check()` therefore calls `check_probes()`
    itself. That is not defence in depth; it is the only copy of that gate on this path.
    """
    if not DETECTOR.is_file():
        # `spec_from_file_location` returns a spec for a path that does not exist, so the guard
        # below never fires for a missing detector -- `exec_module` raises FileNotFoundError
        # several frames deeper instead, which reads as a bug in the detector.
        raise SystemExit(f"error: {relative(DETECTOR)} not found")
    spec = importlib.util.spec_from_file_location("detect_engine_components", DETECTOR)
    if spec is None or spec.loader is None:
        raise SystemExit(f"error: cannot load {relative(DETECTOR)}")
    module = importlib.util.module_from_spec(spec)
    # REGISTERED BEFORE EXECUTION. `@dataclass` resolves its own module out of `sys.modules`
    # while the class body runs, so a module executed before it is registered dies with
    # `'NoneType' object has no attribute '__dict__'` -- which reads like a bug in the detector
    # rather than in how it was loaded.
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def read_manifest() -> dict[str, Any]:
    """`engines/licenses.toml`, with a message rather than a traceback when it is not there."""
    if not MANIFEST.is_file():
        raise SystemExit(f"error: {relative(MANIFEST)} not found")
    try:
        with MANIFEST.open("rb") as handle:
            return tomllib.load(handle)
    except tomllib.TOMLDecodeError as exc:
        raise SystemExit(f"error: {relative(MANIFEST)} is not valid TOML: {exc}") from exc


def spdx(expression: str) -> str:
    """Normalise Cargo's legacy `A/B` spelling into a valid SPDX expression."""
    if LEGACY_SPDX in expression:
        return " OR ".join(part.strip() for part in expression.split(LEGACY_SPDX))
    return expression


def rust_components() -> list[dict[str, Any]]:
    """Every crate in the locked workspace, as CycloneDX components."""
    try:
        out = subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--locked", "--all-features"],
            cwd=REPO,
            capture_output=True,
            text=True,
            check=False,
        )
    except FileNotFoundError as exc:
        raise SystemExit("error: cargo is not on PATH; this tool reads the locked workspace") from exc
    if out.returncode != 0:
        raise SystemExit(f"error: cargo metadata failed:\n{out.stderr.strip()}")

    packages = json.loads(out.stdout)["packages"]
    components: list[dict[str, Any]] = []
    for package in sorted(packages, key=lambda p: (p["name"], p["version"])):
        component: dict[str, Any] = {
            "type": "library",
            "name": package["name"],
            "version": package["version"],
            "purl": f"pkg:cargo/{package['name']}@{package['version']}",
            "scope": "required",
        }
        licence = package.get("license")
        if licence:
            # An SPDX EXPRESSION, not a list of ids: "Apache-2.0 OR MIT" is one licence
            # statement with a choice in it, and splitting it into two entries would assert
            # that both apply, which is a different and wrong claim.
            component["licenses"] = [{"expression": spdx(licence)}]
        elif package.get("license_file"):
            component["licenses"] = [{"license": {"name": f"see {package['license_file']}"}}]
        components.append(component)
    return components


def engine_components(manifest: dict[str, Any]) -> list[dict[str, Any]]:
    """The native components `engines/licenses.toml` declares, as CycloneDX components.

    SCOPE IS CARRIED, NOT FLATTENED, and that is a correctness decision rather than a detail.
    An earlier version emitted every entry as `scope: "required"`, which published `qtest` --
    `test_only = true`, Artistic-2.0, never in a shipped artifact -- as a component burrow
    REQUIRES under a licence ADR 0008's allowlist forbids. `check-engine-licences.py` exempts
    `test_only` for exactly that reason; the primary consumer of a CycloneDX document is a
    licence-policy scan, so handing it that claim is handing it a false positive against this
    project's own non-negotiable. Over-declaring provenance is safe. Over-declaring a licence
    obligation is not.
    """
    components: list[dict[str, Any]] = []
    for entry in sorted(manifest.get("component", []), key=lambda e: str(e.get("name", ""))):
        test_only = bool(entry.get("test_only", False))
        linked = bool(entry.get("linked", False))
        if test_only:
            scope = "excluded"
        elif linked:
            scope = "required"
        else:
            # Distributed with the artifact, but its code was not found in the binary.
            scope = "optional"
        component: dict[str, Any] = {
            "type": "library",
            "name": str(entry.get("name", "")),
            "version": str(entry.get("version", "")),
            "scope": scope,
            "licenses": [{"expression": spdx(str(entry.get("license", "")))}],
            # WHICH SHIPPED FILES THIS IS IN. `burrow:artifacts` is the manifest's own list of
            # what the component is DISTRIBUTED with; `burrow:linked_in` is the subset where its
            # code was actually FOUND. The two differ, and the difference is the point of the
            # field -- carrying only the hand-maintained one would drop precisely the
            # symbol-derived evidence whose absence was the original HarfBuzz and ICU bug.
            "properties": [
                {"name": "burrow:artifacts", "value": ",".join(entry.get("artifacts", []))},
                {"name": "burrow:linked_in", "value": ",".join(entry.get("linked_in", []))},
                {"name": "burrow:linked", "value": "true" if linked else "false"},
                {"name": "burrow:test_only", "value": "true" if test_only else "false"},
                {"name": "burrow:source", "value": "engines/licenses.toml"},
            ],
        }
        components.append(component)
    return components


def is_engine(component: dict[str, Any]) -> bool:
    """True for a component that came from the engine manifest rather than from Cargo."""
    return any(
        prop.get("name") == "burrow:source" and prop.get("value") == "engines/licenses.toml"
        for prop in component.get("properties", [])
    )


def build(manifest: dict[str, Any]) -> dict[str, Any]:
    """The whole document, deterministically."""
    return {
        "bomFormat": "CycloneDX",
        "specVersion": SPEC_VERSION,
        "version": 1,
        "metadata": {
            # NO `timestamp`, NO `serialNumber`: see the module docstring. Determinism is what
            # makes the drift check meaningful.
            "component": {
                "type": "application",
                "name": "burrow",
                "description": (
                    "Free, open-source file tools that run entirely on the user's device."
                ),
                "licenses": [{"expression": "MIT OR Apache-2.0"}],
            },
        },
        "components": rust_components() + engine_components(manifest),
    }


def artifact_ids(manifest: dict[str, Any], path: Path) -> list[str]:
    """The manifest artifact id(s) whose `files` name this scanned file."""
    try:
        vendor_relative = str(path.resolve().relative_to(REPO / "engines"))
    except ValueError:
        return []
    ids: list[str] = []
    for art in manifest.get("artifact", []):
        # SPLIT, not a substring search over the whole field: `vendor/x` is a substring of
        # `vendor/xy`, and an id resolved by accident is worse here than one not resolved at
        # all -- it would attach the wrong expectation to a real binary.
        named = {part.strip() for part in str(art.get("files", "")).replace("+", ",").split(",")}
        if vendor_relative in named:
            ids.append(str(art.get("id", "")))
    return ids


def linked_labels(manifest: dict[str, Any], detector: Any) -> dict[str, set[str]]:
    """Per artifact id, the detector labels the manifest declares are LINKED INTO it.

    Derived from `linked_in` rather than `artifacts`: the question a scan answers is "is this
    component's code in this binary", which is what `linked_in` records.
    """
    expected: dict[str, set[str]] = {}
    for entry in manifest.get("component", []):
        labels = detector.labels_for(str(entry.get("name", "")))
        for identifier in entry.get("linked_in", []):
            expected.setdefault(str(identifier), set()).update(labels)
    return expected


def differing_keys(was: dict[str, Any], now: dict[str, Any]) -> list[str]:
    """Which components changed, and in which fields -- so the reader need not diff 1,100 lines."""
    old = {c.get("name", ""): c for c in was.get("components", [])}
    new = {c.get("name", ""): c for c in now.get("components", [])}
    changes: list[str] = []
    for name in sorted(set(old) & set(new)):
        for key in sorted(set(old[name]) | set(new[name])):
            if old[name].get(key) != new[name].get(key):
                changes.append(f"{name}: {key} was {old[name].get(key)!r}, is {new[name].get(key)!r}")
    if old.keys() == new.keys() and not changes:
        changes.append("the components are identical; the difference is in the document metadata")
    return changes


def check(document: dict[str, Any], manifest: dict[str, Any], *, allow_no_artifacts: bool) -> int:
    """Compare the document with the committed one, and with what the artifacts contain."""
    problems: list[str] = []
    notes: list[str] = []

    detector = load_detector()

    # 0. THE FINGERPRINTS, BEFORE ANYTHING THEY REPORT IS BELIEVED. The detector runs this in
    #    its `main()`, which importing it skips -- so an inert fingerprint would otherwise read
    #    here as "nothing is in the binary", which is this repository's most-repeated failure.
    problems.extend(f"detector fingerprint: {problem}" for problem in detector.check_probes())

    # 1. DRIFT. A crate appearing, leaving, or moving version must be visible.
    committed: dict[str, Any] | None = None
    if not SBOM.is_file():
        problems.append(f"{relative(SBOM)} does not exist; run without --check to write it")
    else:
        try:
            committed = json.loads(SBOM.read_text(encoding="utf-8"))
        except json.JSONDecodeError as exc:
            problems.append(f"{relative(SBOM)} is not valid JSON: {exc}")
        if committed is not None and committed != document:
            was = {(c.get("name", ""), c.get("version")) for c in committed.get("components", [])}
            now = {(c.get("name", ""), c.get("version")) for c in document["components"]}
            for name, version in sorted(now - was):
                problems.append(f"not in the committed SBOM: {name} {version}")
            for name, version in sorted(was - now):
                problems.append(f"in the committed SBOM and no longer in the build: {name} {version}")
            if was == now:
                # NAMED, not just reported. "a licence or a property changed" left the reader to
                # diff the document by hand, and a licence expression flipping underneath us is
                # exactly the case worth spelling out.
                for change in differing_keys(committed, document):
                    problems.append(f"the committed SBOM differs: {change}")

    # 2. THE MANIFEST AND THE COMMITTED SBOM MUST AGREE.
    #    Against the COMMITTED document, never the freshly generated one: the generated engine
    #    half is built from this same manifest seconds earlier, so comparing the two would be
    #    comparing the generator with its own input. A review measured that the earlier version
    #    of this rule was structurally incapable of firing.
    declared = [str(e.get("name", "")) for e in manifest.get("component", [])]
    if committed is not None:
        committed_names = {str(c.get("name", "")).lower() for c in committed.get("components", [])}
        for name in declared:
            if name.lower() not in committed_names:
                problems.append(
                    f"declared in engines/licenses.toml and missing from the committed SBOM: {name}"
                )

    # 3. WHAT IS ACTUALLY IN THE BINARIES. The input that is not hand-maintained.
    #    Engine components only: the labels include generic strings like `icu` and `zlib`, and
    #    searching all 55 crate names as well would let a future `icu_properties` crate satisfy
    #    "ICU is in the SBOM" vacuously.
    engine_names = {str(c.get("name", "")).lower() for c in document["components"] if is_engine(c)}
    #: Native artifacts whose detected set was actually compared against the manifest.
    gated = 0
    labels, _ = detector.declared_labels()
    artifacts = detector.find_artifacts()
    expected = linked_labels(manifest, detector)

    for art in artifacts:
        detector.scan(art)

    for art in artifacts:
        where = relative(art.path)
        for label in sorted(art.detected):
            if label not in labels:
                # The detector's own job; it fails on this too. Repeated here because an SBOM
                # that omitted it would be wrong in the direction that matters.
                problems.append(f"detected in {where} and not declared anywhere: {label}")
            elif not any(label in name for name in engine_names):
                problems.append(f"detected in {where} and missing from the SBOM: {label}")

        if not art.detected:
            # NOT a note. An engine artifact with no third-party code in it means the
            # fingerprints stopped matching, which would otherwise read as a clean scan.
            problems.append(
                f"NOTHING detected in {where} -- the fingerprints no longer match this artifact"
            )

        ids = artifact_ids(manifest, art.path)
        if not ids:
            if art.kind == "native":
                # A PROBLEM, NOT A NOTE. `continue` here would skip the equality gate below,
                # which is the only hard check over the shipped binaries -- and a review measured
                # exactly that: rewrite one path in the manifest's [[artifact]] files list and
                # the gate switched itself off while the tool printed OK. A gate keyed on a
                # hand-edited identifier must fail when the identifier stops resolving.
                problems.append(
                    f"{where} is a native artifact that no [[artifact]] in the manifest names, "
                    f"so nothing gated it -- add it, or correct the [[artifact]] files list"
                )
            else:
                notes.append(f"{where} is scanned but no [[artifact]] in the manifest names it")
            continue
        if art.kind == "native":
            gated += 1
        for identifier in ids:
            want = expected.get(identifier, set())
            found = set(art.detected)
            if art.kind == "native":
                # GATED. The symbol table is complete, so `linked_in` and the scan agree exactly
                # -- measured at 8 of 8 on libpdfium.so. Divergence either way is a defect.
                for label in sorted(want - found):
                    problems.append(
                        f"{identifier}: declared linked_in and NOT found in {where}: {label}"
                    )
                for label in sorted(found - want):
                    problems.append(
                        f"{identifier}: found in {where} and not declared linked_in: {label}"
                    )
            else:
                # REPORTED BY NAME, not gated. A wasm module's names are stripped, so strings are
                # the only signal and detection is genuinely weaker: `lcms` and `libjpeg` are
                # linked into pdfium.wasm and are not detectable there. Failing on that would be
                # failing on a limit of the scanner rather than on a defect.
                for label in sorted(want - found):
                    notes.append(
                        f"{identifier}: declared linked_in, not detectable in {where} "
                        f"(wasm names are stripped): {label}"
                    )
                for label in sorted(found - want):
                    notes.append(
                        f"{identifier}: found in {where} and not declared linked_in: {label}"
                    )

    crates = sum(1 for c in document["components"] if c.get("purl", "").startswith("pkg:cargo/"))
    engines = len(document["components"]) - crates

    # THE COUNTS ARE THE MEASUREMENT, not the verdict. "OK" over nothing is this repository's
    # most-repeated failure, so the numbers are printed whether or not anything failed -- and
    # artifacts are printed BY NAME, because `detect-engine-components.py` printing "1" in CI and
    # "3" on a dev box is in CLAUDE.md as a count nobody can tell a legitimate value from.
    # WHICH INPUTS, not just which counts. All three are environment-overridable, so a run
    # against redirected inputs would otherwise be indistinguishable in the log from a real one.
    print(f"inputs: manifest {relative(MANIFEST)}, document {relative(SBOM)}, vendor tree {relative(detector.VENDOR)}")
    print(f"sbom: {len(document['components'])} component(s) -- {crates} crate(s), {engines} engine component(s)")
    print(f"engines/licenses.toml: {len(declared)} declared component(s)")
    print(f"detector: {len(detector.COMPONENTS)} fingerprint(s), each verified against its own probe")
    if not artifacts:
        message = (
            f"detector: NO artifacts found under {relative(detector.VENDOR)}, so the SBOM was "
            "NOT checked against what the binaries contain. "
            "Run engines/fetch.sh && engines/build-native.sh."
        )
        if allow_no_artifacts:
            print(f"{message} Passing anyway: --allow-no-artifacts.")
        else:
            # THE DEFAULT IS TO REFUSE. A check that passes over the one source that is not
            # hand-maintained has given up the thing that made it worth running.
            problems.append(message)
    else:
        native = sum(1 for art in artifacts if art.kind == "native")
        print(
            f"detector: scanned {len(artifacts)} artifact(s); "
            f"{gated} of {native} native artifact(s) gated against the manifest's linked_in"
        )
        for art in artifacts:
            found = ", ".join(sorted(art.detected)) if art.detected else "NOTHING"
            ids = artifact_ids(manifest, art.path) or ["not named by any [[artifact]]"]
            print(f"  {relative(art.path)} ({art.kind}, {'/'.join(ids)}): {found}")

    if notes:
        # PRINTED, NOT GATED, and pointed at their record: a reader who meets these for the first
        # time should not have to re-diagnose whether they are defects. #116 has the analysis.
        for note in notes:
            print(f"  note: {note}")
        print("  (the notes above are tracked in #116; wasm detection is weaker by construction)")

    if problems:
        # Flushed, so a merged log reads measurement-then-verdict rather than the other way up.
        sys.stdout.flush()
        print(f"\nFAILED -- {len(problems)} problem(s):", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        print(
            "\nRegenerate with `python3 tools/make-sbom.py` and commit the result, or fix the "
            "disagreement between the sources.",
            file=sys.stderr,
        )
        return 1

    print("\nOK -- the SBOM, the engine manifest and the shipped binaries agree.")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="Generate and verify the CycloneDX SBOM.")
    parser.add_argument("--check", action="store_true", help="verify without writing")
    parser.add_argument(
        "--allow-no-artifacts",
        action="store_true",
        help="pass even when engines/vendor holds nothing to scan (default: fail)",
    )
    args = parser.parse_args()

    manifest = read_manifest()
    document = build(manifest)
    if args.check:
        return check(document, manifest, allow_no_artifacts=args.allow_no_artifacts)

    # REAL, not decorative. The previous form also required BURROW_SBOM to be unset, which is
    # the only way SBOM can differ from the default -- so the branch could not execute, while
    # reading as a safety guard. Writing is still allowed anywhere INSIDE the repository, because
    # that is what the self-test needs; outside it is refused.
    try:
        SBOM.resolve().relative_to(REPO)
    except ValueError:
        raise SystemExit(f"error: refusing to write outside the repository: {SBOM}") from None
    SBOM.parent.mkdir(parents=True, exist_ok=True)
    SBOM.write_text(json.dumps(document, indent=2, sort_keys=False) + "\n", encoding="utf-8")
    crates = sum(1 for c in document["components"] if c.get("purl", "").startswith("pkg:cargo/"))
    print(f"wrote {relative(SBOM)}: {len(document['components'])} components ({crates} crates)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
