#!/usr/bin/env python3
"""`corpus/manifest.toml` describes each corpus with the fields its KIND actually has.

    python3 tools/check-corpus-manifest.py

WHY PER-KIND FIELDS, AND WHY A CHECK RATHER THAN A CONVENTION

The manifest's original shape assumed every corpus is fetched: a `url` and a `sha256`. The
redaction corpus is first-party — half regenerated from a committed generator, half committed
outright — and it has neither. The tempting fix is to write something in those fields anyway.

**A digest that does not describe anything is worse than no digest**, because the next person
reads it as a guarantee. A generated corpus's bytes are whatever the generator makes today; a
committed corpus's bytes are in git, where git already hashes them. So the kinds differ, and
this refuses the fields each kind must not have as firmly as it requires the ones it must.

  fetched     third-party, downloaded          requires url, sha256      forbids generator, path
  generated   first-party, produced on demand  requires generator        forbids url, sha256, path
  committed   first-party, in the tree         requires path, provenance forbids url, sha256

IT REPORTS WHAT IT EXAMINED

Per `CLAUDE.md`: a check that prints OK over an empty list has measured nothing. This prints the
count per kind and per set, and refuses a set that is declared and empty unless the manifest
says empty is expected — the `redaction` set sat empty from M1 until #135, and nothing said so.

THE RULES ARE PROBED ON EVERY RUN

Each rule is exercised against a synthetic entry that must fail it, and a near-miss that must
pass, before the real manifest is read. A rule that matches nothing passes everything.
"""

from __future__ import annotations

import sys
import tomllib
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
MANIFEST = REPO / "corpus" / "manifest.toml"

# kind -> (required fields, forbidden fields)
KINDS: dict[str, tuple[set[str], set[str]]] = {
    "fetched": ({"url", "sha256"}, {"generator", "path", "provenance"}),
    "generated": ({"generator"}, {"url", "sha256", "path"}),
    "committed": ({"path", "provenance"}, {"url", "sha256", "generator"}),
}

COMMON_REQUIRED = {"name", "set", "kind", "license", "redistributable"}

# Sets that are allowed to be empty, each with the reason. A set that is empty and NOT here is
# a set nobody populated and nobody noticed -- which is what `redaction` was from M1 until #135.
EMPTY_IS_EXPECTED = {
    "smoke": "no corpus registered yet; the conformance fixtures cover this ground for now",
    "malformed": "no corpus registered yet; tests/conformance/fixtures carries the damaged files",
    "regression": "needs the self-hosted linux-arm64 machine and a corpus nobody here chose",
}


def check_entry(entry: dict, index: int) -> list[str]:
    """Every problem with one corpus entry. Pure, so the probes below can call it directly."""
    where = entry.get("name", f"corpora[{index}]")
    problems: list[str] = []

    missing_common = COMMON_REQUIRED - set(entry)
    if missing_common:
        problems.append(f"{where}: missing {', '.join(sorted(missing_common))}")

    kind = entry.get("kind")
    if kind is None:
        return problems
    if kind not in KINDS:
        problems.append(
            f"{where}: kind `{kind}` is not one of {', '.join(sorted(KINDS))}. "
            "A new kind needs an entry in KINDS saying which fields it has."
        )
        return problems

    required, forbidden = KINDS[kind]
    for field in sorted(required - set(entry)):
        problems.append(f"{where}: kind `{kind}` requires `{field}`")
    for field in sorted(forbidden & set(entry)):
        problems.append(
            f"{where}: kind `{kind}` must not carry `{field}` — "
            "a field that does not describe anything reads as a guarantee"
        )

    if kind == "fetched" and "sha256" in entry:
        digest = str(entry["sha256"])
        if len(digest) != 64 or set(digest) <= set("0"):
            problems.append(f"{where}: `sha256` is not a real digest")

    for field in ("generator", "path", "provenance", "verdicts"):
        value = entry.get(field)
        if value and not (REPO / value).exists():
            problems.append(f"{where}: `{field}` points at {value}, which does not exist")

    return problems


def probe_the_rules() -> tuple[list[str], int]:
    """Each rule must reject its own bad entry and accept a near-miss. Run before the manifest."""
    failures: list[str] = []

    def must_reject(entry: dict, because: str) -> None:
        if not check_entry(entry, 0):
            failures.append(f"rule probe: accepted an entry it must reject — {because}")

    def must_accept(entry: dict, because: str) -> None:
        found = check_entry(entry, 0)
        if found:
            failures.append(f"rule probe: rejected a valid entry — {because}: {found}")

    base = {"name": "probe", "set": "smoke", "license": "MIT", "redistributable": True}
    must_reject(
        {**base, "kind": "generated", "generator": "tools/check-corpus-manifest.py",
         "sha256": "0" * 64},
        "a generated corpus carrying a digest",
    )
    must_reject({**base, "kind": "fetched", "url": "https://example.org/x"}, "fetched with no sha256")
    must_reject(
        {**base, "kind": "fetched", "url": "https://example.org/x", "sha256": "0" * 64},
        "a fetched corpus whose digest is all zeroes",
    )
    must_reject({**base, "kind": "committed", "path": "tools"}, "committed with no provenance")
    must_reject({**base, "kind": "invented"}, "an unknown kind")
    must_reject({"kind": "generated", "generator": "tools"}, "an entry missing the common fields")
    must_reject(
        {**base, "kind": "generated", "generator": "tools/does-not-exist.py"},
        "a generator that is not there",
    )
    must_accept(
        {**base, "kind": "generated", "generator": "tools/check-corpus-manifest.py"},
        "a well-formed generated entry",
    )
    must_accept(
        {**base, "kind": "committed", "path": "tools", "provenance": "tools/README.md"},
        "a well-formed committed entry",
    )
    return failures, 9


def main() -> int:
    probe_failures, probe_count = probe_the_rules()
    if probe_failures:
        sys.stderr.write("check-corpus-manifest: THE RULES THEMSELVES FAILED\n")
        for f in probe_failures:
            sys.stderr.write(f"  - {f}\n")
        return 1

    data = tomllib.loads(MANIFEST.read_text())
    sets = {s["name"]: s for s in data.get("sets", [])}
    corpora = data.get("corpora", [])

    problems: list[str] = []
    by_kind: dict[str, int] = {}
    by_set: dict[str, int] = {name: 0 for name in sets}

    for i, entry in enumerate(corpora):
        problems += check_entry(entry, i)
        by_kind[entry.get("kind", "?")] = by_kind.get(entry.get("kind", "?"), 0) + 1
        name = entry.get("set")
        if name not in sets:
            problems.append(f"{entry.get('name', i)}: set `{name}` is not declared")
        else:
            by_set[name] += 1

    for name, count in sorted(by_set.items()):
        if count == 0 and name not in EMPTY_IS_EXPECTED:
            problems.append(
                f"set `{name}` is declared and empty, and is not listed in EMPTY_IS_EXPECTED. "
                "An empty set that nobody expected is a set nobody populated and nobody noticed."
            )

    print(
        f"check-corpus-manifest: {len(corpora)} corpus/corpora across {len(sets)} set(s); "
        + ", ".join(f"{n} {k}" for k, n in sorted(by_kind.items()))
        + f"; {probe_count} rule probe(s) passed"
    )
    for name, count in sorted(by_set.items()):
        note = "" if count else f"  (empty — {EMPTY_IS_EXPECTED.get(name, 'UNEXPECTED')})"
        print(f"    {name:<12} {count}{note}")

    if problems:
        sys.stderr.write(f"\nFAILED — {len(problems)} problem(s):\n")
        for p in problems:
            sys.stderr.write(f"  - {p}\n")
        return 1
    print("OK — every corpus entry carries the fields its kind has, and no others.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
