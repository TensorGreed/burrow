#!/usr/bin/env python3
"""The `ci-ok` gate: one required check that is green only when every CI job that SHOULD have run
succeeded.

WHY (owner, 2026-10-16). The old aggregate `ci` job (removed 2026-10-17) failed only on `failure`
or `cancelled`, so a SKIPPED job read as passing. `ci-ok`, which the ruleset has required in its
place since 2026-10-16, closes that: it `needs:` every other job directly and fails on any result
that is not `success` -- except a skip the rules below say was meant.

WHICH SKIPS ARE MEANT (owner (82)/(83), 2026-10-29). Since the CI restructure some jobs do not run
on every event, and `ci-ok` must tell a skip that was meant from one that was a mistake:

  fuzz          skipped only on `pull_request`/`merge_group`, only when the `changes` job
                SUCCEEDED and said `fuzz=false` (the diff touches none of FUZZ_PREFIXES or
                FUZZ_NAMES). Anything else -- a push to main, a failed or skipped `changes` job, an
                empty or missing output, an unknown event -- means it should have run.
  web-firefox   skipped only on `pull_request`/`merge_group`: those run Chromium alone, and the
  web-webkit    other two run on push to main and in the nightly (`fuzz-nightly.yml`).

Every other job: skipped is a failure, as before. `failure`, `cancelled` and anything else that is
not `success` fail every job, gated or not. The rules live HERE, next to the `scope` decision the
`changes` job runs, so the decision and its judge are one file; `needs` mode holds ci.yml's `if:`
expressions to them, so the workflow cannot gate a job this file does not know about.

WHAT IT DOES NOT CLOSE, stated: the merge queue enforces a required check-run, not the run-level
conclusion DECISIONS rule 11 is about, and #275's shape -- a run-level `failure` with every job
`success` -- gives this gate nothing but `success` results to read, so it passes. That shape is
still read from `main`'s post-merge push run, as rule 11's 2026-10-16 amendment says. Nor does it
see the nightly: a Firefox or WebKit failure there is read per job under rule 11's 2026-10-29 line.

Three modes:

  check-ci-ok.py needs [<ci.yml>]   STRUCTURE, run in CI's `deny` job and by `ci-local`: every job
                                     other than `ci-ok` is in its `needs:` directly; `ci-ok` runs
                                     under `if: always()` with a step running `results` and setting
                                     EVENT; every job carrying an `if:` is one GATED names, with
                                     exactly the expression GATED records, and every GATED job
                                     exists with it; `fuzz` needs `changes`. Reports the counts.
  check-ci-ok.py scope               CHANGE DETECTION, run in CI's `changes` job. Reads $EVENT,
                                     $BASE and $HEAD, diffs (`BASE...HEAD` for a pull request,
                                     `BASE..HEAD` for a merge group) and writes `fuzz=true|false`
                                     to $GITHUB_OUTPUT (stdout when unset). If the diff cannot be
                                     computed -- a bad SHA, a git error, an empty list, anything
                                     raised -- it writes `fuzz=true`: fuzz smoke runs. Every other
                                     event: `fuzz=true`.
  check-ci-ok.py results             RESULTS, run inside `ci-ok`: reads `toJSON(needs)` from $NEEDS
                                     and the event from $EVENT, and fails naming each job whose
                                     result is not `success` and is not a skip the rules allow. An
                                     empty or unreadable $NEEDS, or an empty $EVENT, fails too.

Self-test: tools/test-check-ci-ok.sh.
"""

from __future__ import annotations

import json
import os
import pathlib
import re
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent
GATE = "ci-ok"
CHANGES = "changes"
FUZZ = "fuzz"

#: The events where the narrowed jobs may be skipped. Every other event (push to main,
#: workflow_dispatch, anything added later) runs everything.
NARROWED_EVENTS = ("pull_request", "merge_group")

#: owner (83)(2): fuzz smoke runs on a PR or merge group when the diff touches one of these.
#: Prefixes are directories from the repository root; NAMES match a file of that name at any depth.
FUZZ_PREFIXES = ("core/", "bindings/", "fuzz/", "engines/", ".github/workflows/", "tools/")
FUZZ_NAMES = ("Cargo.lock", "Cargo.toml", "rust-toolchain.toml")

#: Every job ci.yml may gate with an `if:`, and the exact expression it must carry. A job with an
#: `if:` not listed here, or listed with a different expression, fails `needs` mode: the skip
#: rules in `results` are only sound for the expressions they were written against.
NOT_NARROWED = "${{ github.event_name != 'pull_request' && github.event_name != 'merge_group' }}"
GATED = {
    FUZZ: "${{ !cancelled() && needs.changes.outputs.fuzz != 'false' }}",
    "web-firefox": NOT_NARROWED,
    "web-webkit": NOT_NARROWED,
}

SHA = re.compile(r"^[0-9a-f]{40}$")


def fail(message: str) -> int:
    print(f"::error::check-ci-ok: {message}", file=sys.stderr)
    return 1


# --- needs ------------------------------------------------------------------------------------


def _list(value) -> list:
    if value is None:
        return []
    return [value] if isinstance(value, str) else list(value)


def check_needs(path: pathlib.Path) -> int:
    # LAZILY: the `results` mode is the required check, and it should depend on nothing but the
    # standard library.
    import yaml

    workflow = yaml.safe_load(path.read_text(encoding="utf-8"))
    jobs = workflow.get("jobs") or {}
    if not jobs:
        return fail(f"{path} has no jobs, so this examined nothing")
    if GATE not in jobs:
        return fail(f"{path} has no `{GATE}` job, so nothing gates on every job's result")
    gate = jobs[GATE]
    needs = _list(gate.get("needs"))
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
    results_steps = [s for s in steps if "check-ci-ok.py results" in str(s.get("run", ""))]
    if not results_steps:
        problems.append(f"`{GATE}` has no step running `tools/check-ci-ok.py results`")
    elif not any(
        str((s.get("env") or {}).get("EVENT", "")).replace(" ", "") == "${{github.event_name}}"
        for s in results_steps
    ):
        problems.append(
            f"`{GATE}`'s results step does not set EVENT to `github.event_name`: a skip cannot be "
            f"judged without the event"
        )

    # THE GATED JOBS. An `if:` this file does not know is a skip `results` cannot judge.
    gated_seen = []
    for name in others:
        expression = jobs[name].get("if")
        if expression is None:
            if name in GATED:
                problems.append(
                    f"`{name}` has no `if:` but GATED expects `{GATED[name]}`: the table and the "
                    f"workflow disagree"
                )
            continue
        gated_seen.append(name)
        if name not in GATED:
            problems.append(
                f"`{name}` carries `if: {expression}`, which no skip rule here judges"
            )
        elif str(expression).strip() != GATED[name]:
            problems.append(
                f"`{name}` carries `if: {expression}`, not `{GATED[name]}`: its skip rule was "
                f"written against that expression"
            )
    for name in sorted(GATED):
        if name not in jobs:
            problems.append(f"GATED names `{name}`, which the workflow does not have")
    if CHANGES not in jobs:
        problems.append(f"the workflow has no `{CHANGES}` job, so nothing decides whether fuzz runs")
    elif FUZZ in jobs and CHANGES not in _list(jobs[FUZZ].get("needs")):
        problems.append(f"`{FUZZ}` does not need `{CHANGES}`, so its `if:` reads no output")

    if problems:
        for problem in problems:
            fail(problem)
        return 1
    print(
        f"check-ci-ok: {len(others)} of {len(others)} job(s) in {path.name} other than `{GATE}` are "
        f"in its needs ({', '.join(others)}); it runs under always() and checks every result; "
        f"{len(gated_seen)} of {len(GATED)} gated job(s) carry their recorded `if:` "
        f"({', '.join(gated_seen) or 'none'})"
    )
    return 0


# --- scope ------------------------------------------------------------------------------------


def touches_fuzz(path: str) -> bool:
    if any(path.startswith(prefix) for prefix in FUZZ_PREFIXES):
        return True
    return path.rsplit("/", 1)[-1] in FUZZ_NAMES


def decide_scope(event: str, base: str, head: str) -> tuple[bool, str, list[str]]:
    """(fuzz?, why, changed paths). Every failure to compute the diff answers True."""
    if event not in NARROWED_EVENTS:
        return True, f"event `{event or '<unset>'}` always runs fuzz smoke", []
    for label, sha in (("BASE", base), ("HEAD", head)):
        if not SHA.match(sha or ""):
            return True, f"{label} is not a commit SHA ({sha!r}), so the diff cannot be computed", []
    spec = f"{base}...{head}" if event == "pull_request" else f"{base}..{head}"
    try:
        done = subprocess.run(
            # In the CURRENT directory, which in CI is the checkout: the self-test runs this
            # from repositories it builds, so no path into this one is assumed.
            ["git", "diff", "--name-only", "--no-renames", spec],
            capture_output=True,
            text=True,
            timeout=120,
        )
    except (OSError, subprocess.SubprocessError) as error:
        return True, f"git diff {spec} could not run ({error})", []
    if done.returncode != 0:
        return True, f"git diff {spec} failed ({done.stderr.strip() or done.returncode})", []
    paths = [line for line in done.stdout.splitlines() if line.strip()]
    if not paths:
        # A diff that names nothing cannot be told from one that silently read nothing.
        return True, f"git diff {spec} named no files, which cannot be told from a broken diff", []
    hits = [p for p in paths if touches_fuzz(p)]
    if hits:
        return True, f"{len(hits)} of {len(paths)} changed path(s) touch fuzz's inputs, first {hits[0]}", paths
    return False, f"none of {len(paths)} changed path(s) touch fuzz's inputs", paths


def run_scope() -> int:
    event = os.environ.get("EVENT", "")
    try:
        fuzz, why, paths = decide_scope(event, os.environ.get("BASE", ""), os.environ.get("HEAD", ""))
    except Exception as error:  # noqa: BLE001 -- any failure must run fuzz, never skip it
        fuzz, why, paths = True, f"the scope decision raised {type(error).__name__}: {error}", []
    for path in paths:
        print(f"  changed: {path}{'  [fuzz]' if touches_fuzz(path) else ''}")
    answer = "true" if fuzz else "false"
    print(f"check-ci-ok scope: event {event or '<unset>'}: fuzz={answer} -- {why}")
    out = os.environ.get("GITHUB_OUTPUT")
    if out:
        with open(out, "a", encoding="utf-8") as handle:
            handle.write(f"fuzz={answer}\n")
    else:
        print(f"fuzz={answer}")
    return 0


# --- results ----------------------------------------------------------------------------------


def skip_allowed(name: str, needs: dict, event: str) -> tuple[bool, str]:
    """Whether a SKIPPED `name` was meant, and why (or why not)."""
    if name not in GATED:
        return False, "it is not a gated job, so it always runs"
    if event not in NARROWED_EVENTS:
        return False, f"event `{event}` runs every job"
    if name == FUZZ:
        changes = needs.get(CHANGES)
        if not isinstance(changes, dict) or changes.get("result") != "success":
            got = changes.get("result") if isinstance(changes, dict) else "absent"
            return False, f"`{CHANGES}` did not succeed ({got}), so fuzz should have run"
        said = (changes.get("outputs") or {}).get("fuzz")
        if said != "false":
            return False, f"`{CHANGES}` said fuzz={said!r}, so fuzz should have run"
        return True, f"`{CHANGES}` said the diff touches none of fuzz's inputs"
    return True, f"event `{event}` runs Chromium only"


def check_results(raw: str | None, event: str | None) -> int:
    if not raw:
        return fail("$NEEDS is empty: pass `toJSON(needs)`; a gate that saw no jobs checked none")
    if not event:
        return fail("$EVENT is empty: pass `github.event_name`; a skip cannot be judged without it")
    try:
        needs = json.loads(raw)
    except json.JSONDecodeError as error:
        return fail(f"$NEEDS is not JSON ({error})")
    if not isinstance(needs, dict):
        return fail(f"$NEEDS is not a JSON object of jobs (it is a {type(needs).__name__})")
    if not needs:
        return fail("$NEEDS names no jobs; a gate that saw no jobs checked none")
    not_ok = []
    skipped_ok = []
    for name in sorted(needs):
        entry = needs[name]
        if not isinstance(entry, dict):
            print(f"  {name}: not a job record ({type(entry).__name__})")
            not_ok.append(f"{name} (no result: not a job record)")
            continue
        result = entry.get("result")
        if result == "skipped":
            allowed, why = skip_allowed(name, needs, event)
            print(f"  {name}: skipped -- {'allowed' if allowed else 'NOT allowed'}: {why}")
            if allowed:
                skipped_ok.append(name)
            else:
                not_ok.append(f"{name} (skipped, but should have run: {why})")
            continue
        print(f"  {name}: {result}")
        if result != "success":
            not_ok.append(f"{name} ({result})")
    if not_ok:
        return fail(f"{len(not_ok)} of {len(needs)} job(s) did not succeed: {'; '.join(not_ok)}")
    ran = len(needs) - len(skipped_ok)
    print(
        f"check-ci-ok: event {event}: {ran} of {len(needs)} job(s) ran and succeeded, "
        f"{len(skipped_ok)} skipped by rule ({', '.join(skipped_ok) or 'none'})"
    )
    return 0


def main(argv: list[str]) -> int:
    if len(argv) >= 2 and argv[1] == "needs":
        path = pathlib.Path(argv[2]) if len(argv) > 2 else REPO / ".github" / "workflows" / "ci.yml"
        return check_needs(path)
    if len(argv) == 2 and argv[1] == "scope":
        return run_scope()
    if len(argv) == 2 and argv[1] == "results":
        return check_results(os.environ.get("NEEDS"), os.environ.get("EVENT"))
    print("usage: tools/check-ci-ok.py needs [<ci.yml>] | scope | results", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
