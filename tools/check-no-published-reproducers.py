#!/usr/bin/env python3
"""No workflow may publish a fuzzing reproducer from a public repository.

    python3 tools/check-no-published-reproducers.py

# Why this exists

`fuzz-nightly.yml` ended with `upload-artifact` over `fuzz/artifacts/`, which on a public
repository publishes the input that caused every crash it finds. For four days that included
unminimised reproducers for the two unpatched qpdf use-after-frees of #62 — whose own text says
*"no reproducer is attached, here or anywhere in this repository, and none should be added until
upstream has had the chance to fix it"*, with the minimised input deliberately held outside the
repository. `gh run download` was all it took.

**The step was written in good faith**, to solve a real problem: a crash is only useful if the
input survives the runner. The answer is that it survives *privately*. The step now publishes
the sanitiser's verdict, the symbolised frame and the input's hash, which is what a reader of a
red nightly actually needs, and never the bytes.

A comment saying "do not upload crash inputs" would not have prevented the original, because the
original was not written by someone ignoring such a comment. So it is a gate.

# What it refuses

Any `actions/upload-artifact` step whose `path` names a directory that holds fuzzing inputs —
`fuzz/artifacts`, `fuzz/corpus`, or anything with `crash` in it. It reads the parsed workflow
rather than grepping, so a path split across a YAML block scalar is still seen.

# What it does NOT claim

It cannot see a path assembled at run time (`path: ${{ env.SOMEWHERE }}`), and it does not read
`run:` steps — a step that `curl`s an input somewhere is invisible to it. Those are stated rather
than implied: this gate closes the shape that actually happened, and the posture it protects is
recorded in #62 rather than here.
"""

from __future__ import annotations

import sys
from pathlib import Path

import yaml

REPO = Path(__file__).resolve().parent.parent
WORKFLOWS = REPO / ".github" / "workflows"

#: Path fragments that mean "this is a fuzzing input". `crash` catches an artifact named for what
#: it carries even if it lives somewhere new; `fuzz.log`/`fuzz-` catch the RAW LOG, which carries
#: the whole input via cargo-fuzz's Debug dump and is now the highest-value thing to publish.
FORBIDDEN = ("fuzz/artifacts", "fuzz/corpus", "crash", "fuzz.log", "fuzz-log")

#: Directories whose CONTENTS are inputs. Uploading an ancestor publishes them without naming
#: them: `path: fuzz/` and `path: .` both ship `fuzz/artifacts/`, and neither contains any
#: fragment above. A review demonstrated exactly this against the first version of this gate --
#: the same mistake as the original incident, with a shorter path.
INPUT_DIRS = ("fuzz/artifacts", "fuzz/corpus")

#: The action that publishes. Matched on the repository part, so a version bump does not silently
#: exempt a step.
UPLOAD = "actions/upload-artifact"  # compared lower-case


class Refused(Exception):
    """A workflow publishes something it must not."""


def steps_of(document: dict):
    """Every step in a workflow OR a composite action, with the job it sits in.

    Both shapes, because both can publish. A workflow keeps steps under `jobs.<name>.steps`; a
    composite action keeps them under `runs.steps` with no job at all -- and this repository has
    such an action (`.github/actions/build-web-payload`), so a gate that read only workflows had
    a live blind spot rather than a theoretical one.
    """
    for job_name, job in (document.get("jobs") or {}).items():
        if not isinstance(job, dict):
            continue
        for step in job.get("steps") or []:
            if isinstance(step, dict):
                yield job_name, step

    runs = document.get("runs")
    if isinstance(runs, dict):
        for step in runs.get("steps") or []:
            if isinstance(step, dict):
                yield "<composite action>", step


def is_upload(step: dict) -> bool:
    """True for an upload-artifact step, however `uses` is spelled.

    Case-insensitive: GitHub resolves `uses` without regard to case, so `ACTIONS/upload-artifact`
    is the same action and was invisible to a `startswith` on the exact spelling.
    """
    return str(step.get("uses", "")).lower().startswith(UPLOAD)


def offending_paths(step: dict) -> list[str]:
    """Why this upload step must not run: named fragments, and ancestors of the input dirs."""
    if not is_upload(step):
        return []
    raw = str((step.get("with") or {}).get("path", ""))
    reasons: list[str] = []
    for line in raw.splitlines():
        candidate = line.strip().lstrip("!").strip()
        if not candidate:
            continue
        lowered = candidate.lower()
        reasons += [f"names {fragment!r}" for fragment in FORBIDDEN if fragment in lowered]

        # ANCESTOR CONTAINMENT, by path parts rather than substring. `fuzz/`, `.` and
        # `${{ github.workspace }}` all publish `fuzz/artifacts/` while containing none of the
        # fragments above.
        normalised = lowered.strip("./").rstrip("/")
        for directory in INPUT_DIRS:
            if normalised in ("", "*", "**") or directory.startswith(f"{normalised}/"):
                reasons.append(f"is an ancestor of {directory!r}")
    # An exclusion (`!fuzz/artifacts/**`) still trips this, which fails CLOSED. Left deliberately:
    # a gate that reasons about negated globs is a gate with a parser in it.
    return reasons


def check(workflow: dict, where: str, report: list[str]) -> tuple[int, int]:
    """Returns (steps examined, upload steps seen). Raises on a publishing step."""
    examined = uploads = 0
    for job_name, step in steps_of(workflow):
        examined += 1
        if is_upload(step):
            uploads += 1
            bad = offending_paths(step)
            if bad:
                raise Refused(
                    f"{where}, job `{job_name}`, step "
                    f"{step.get('name', step.get('uses'))!r} uploads "
                    f"{(step.get('with') or {}).get('path')!r}, which {bad[0]}. "
                    f"This repository is public: that publishes a working reproducer for every "
                    f"crash the job finds. Publish the verdict, the symbolised frame and the "
                    f"input's hash instead -- see #62."
                )
    return examined, uploads


#: Each rule against a structure it must refuse and a near-miss it must accept, every run.
PROBES = [
    (
        "an upload of the fuzz artifact directory",
        {"jobs": {"fuzz": {"steps": [
            {"uses": "actions/upload-artifact@v4", "with": {"path": "fuzz/artifacts/"}}
        ]}}},
        {"jobs": {"fuzz": {"steps": [
            {"uses": "actions/upload-artifact@v4", "with": {"path": "apps/web/dist"}}
        ]}}},
    ),
    (
        "an upload named for a crash, from somewhere else",
        {"jobs": {"x": {"steps": [
            {"uses": "actions/upload-artifact@v4", "with": {"path": "/tmp/crash-inputs"}}
        ]}}},
        {"jobs": {"x": {"steps": [
            {"uses": "actions/upload-artifact@v4", "with": {"path": "playwright-report/"}}
        ]}}},
    ),
    (
        "the corpus, which is inputs too",
        {"jobs": {"x": {"steps": [
            {"uses": "actions/upload-artifact@v4", "with": {"path": "fuzz/corpus/rotate"}}
        ]}}},
        {"jobs": {"x": {"steps": [
            {"uses": "actions/upload-artifact@v4", "with": {"path": "tests/conformance"}}
        ]}}},
    ),
]


def run_probes() -> int:
    for name, must_refuse, must_accept in PROBES:
        try:
            check(must_refuse, "<probe>", [])
        except Refused:
            pass
        else:
            raise Refused(f"probe: '{name}' accepted its own counter-fixture; the rule is inert")
        try:
            check(must_accept, "<probe>", [])
        except Refused as exc:
            raise Refused(
                f"probe: '{name}' refused a near-miss it must accept ({exc}); it would refuse a "
                f"safe workflow"
            ) from exc
    return len(PROBES)


def main() -> int:
    report: list[str] = []
    try:
        probes = run_probes()
        print(f"check-no-published-reproducers: {probes} rule(s) probed against a fixture and a near-miss")

        # WORKFLOWS AND COMPOSITE ACTIONS. The second half is not decoration: a step added to
        # `.github/actions/*/action.yml` runs inside a job exactly like an inline one.
        files = sorted(WORKFLOWS.glob("*.yml")) + sorted(WORKFLOWS.glob("*.yaml"))
        files += sorted((REPO / ".github" / "actions").glob("*/action.yml"))
        files += sorted((REPO / ".github" / "actions").glob("*/action.yaml"))
        if not files:
            raise Refused(f"no workflows under {WORKFLOWS.relative_to(REPO)}; nothing was checked")

        total_steps = total_uploads = 0
        for path in files:
            workflow = yaml.safe_load(path.read_text(encoding="utf-8"))
            if not isinstance(workflow, dict):
                continue
            steps, uploads = check(workflow, str(path.relative_to(REPO)), report)
            total_steps += steps
            total_uploads += uploads
            report.append(f"{path.relative_to(REPO)}: {steps} step(s), {uploads} upload step(s)")

        # THE COUNTS ARE THE MEASUREMENT. An upload count of zero would mean this gate is
        # examining nothing -- the repository does upload artifacts, legitimately.
        if total_uploads == 0:
            raise Refused(
                "no upload-artifact step found in any workflow. This repository has several; "
                "finding none means the parser stopped seeing them, not that none exist."
            )
    except Refused as exc:
        for line in report:
            print(f"  {line}")
        print(f"::error::{exc}", file=sys.stderr)
        return 1

    for line in report:
        print(f"  {line}")
    print(
        f"OK -- {total_steps} step(s) across {len(files)} workflow(s); {total_uploads} publish "
        f"artifacts and none publishes a fuzzing input."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
