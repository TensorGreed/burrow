#!/usr/bin/env python3
"""The `ci-ok` gate: one required check that is green only when EVERY other CI job succeeded.

WHY (owner, 2026-10-16). The merge queue enforces a required check-run, not the run-level
conclusion DECISIONS rule 11 is about, so a #275-shaped run -- a run-level `failure` with every job
green -- would merge. And the old aggregate `ci` job failed only on `failure` or `cancelled`: a
SKIPPED job read as passing. `ci-ok` closes both, once the ruleset requires it in place of `ci`:
it `needs:` every other job directly and fails on any result that is not `success`.

Two modes, because the gate has two halves and each needs a check:

  check-ci-ok.py needs [<ci.yml>]   STRUCTURE, run in CI's `deny` job and by `ci-local`: every job
                                     in the workflow other than `ci-ok` is in `ci-ok`'s `needs:`,
                                     named directly (a job reached only transitively does not
                                     count -- `needs.*.result` lists only direct needs), `ci-ok`
                                     runs under `if: always()`, and its step runs `results`. Reports
                                     how many jobs it examined against the count in the file.
  check-ci-ok.py results             RESULTS, run inside the `ci-ok` job: reads `toJSON(needs)` from
                                     $NEEDS and fails unless every result is `success`, naming each
                                     one that is not (`failure`, `cancelled`, `skipped`, anything
                                     else). An empty or unreadable $NEEDS fails too: a gate that saw
                                     no jobs has not checked any.

Self-test: tools/test-check-ci-ok.sh.
"""

from __future__ import annotations

import json
import os
import pathlib
import sys

import yaml

REPO = pathlib.Path(__file__).resolve().parent.parent
GATE = "ci-ok"


def fail(message: str) -> int:
    print(f"::error::check-ci-ok: {message}", file=sys.stderr)
    return 1


def check_needs(path: pathlib.Path) -> int:
    workflow = yaml.safe_load(path.read_text(encoding="utf-8"))
    jobs = workflow.get("jobs") or {}
    if not jobs:
        return fail(f"{path} has no jobs, so this examined nothing")
    if GATE not in jobs:
        return fail(f"{path} has no `{GATE}` job, so nothing gates on every job's result")
    gate = jobs[GATE]
    needs = gate.get("needs") or []
    needs = [needs] if isinstance(needs, str) else list(needs)
    others = sorted(name for name in jobs if name != GATE)
    missing = [name for name in others if name not in needs]
    unknown = sorted(name for name in needs if name not in jobs)
    problems = []
    if missing:
        problems.append(
            f"`{GATE}` does not need {', '.join(missing)}: a job missing from its `needs:` can fail, "
            f"be cancelled or be skipped and `{GATE}` still passes"
        )
    if unknown:
        problems.append(f"`{GATE}` needs job(s) the workflow does not have: {', '.join(unknown)}")
    if str(gate.get("if", "")).strip() != "always()":
        problems.append(
            f"`{GATE}` must run under `if: always()`; without it a failed need SKIPS `{GATE}`, and a "
            f"skipped required check is not a red one"
        )
    steps = gate.get("steps") or []
    if not any("check-ci-ok.py results" in str(step.get("run", "")) for step in steps):
        problems.append(f"`{GATE}` has no step running `tools/check-ci-ok.py results`")
    if problems:
        for problem in problems:
            fail(problem)
        return 1
    print(
        f"check-ci-ok: {len(others)} of {len(others)} job(s) in {path.name} other than `{GATE}` are "
        f"in its needs ({', '.join(others)}); it runs under always() and checks every result"
    )
    return 0


def check_results(raw: str | None) -> int:
    if not raw:
        return fail("$NEEDS is empty: pass `toJSON(needs)`; a gate that saw no jobs checked none")
    try:
        needs = json.loads(raw)
    except json.JSONDecodeError as error:
        return fail(f"$NEEDS is not JSON ({error})")
    if not isinstance(needs, dict) or not needs:
        return fail("$NEEDS names no jobs; a gate that saw no jobs checked none")
    not_ok = []
    for name in sorted(needs):
        result = (needs[name] or {}).get("result")
        print(f"  {name}: {result}")
        if result != "success":
            not_ok.append(f"{name} ({result})")
    if not_ok:
        return fail(f"{len(not_ok)} of {len(needs)} job(s) did not succeed: {', '.join(not_ok)}")
    print(f"check-ci-ok: all {len(needs)} job(s) succeeded")
    return 0


def main(argv: list[str]) -> int:
    if len(argv) >= 2 and argv[1] == "needs":
        path = pathlib.Path(argv[2]) if len(argv) > 2 else REPO / ".github" / "workflows" / "ci.yml"
        return check_needs(path)
    if len(argv) == 2 and argv[1] == "results":
        return check_results(os.environ.get("NEEDS"))
    print("usage: tools/check-ci-ok.py needs [<ci.yml>] | results", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
