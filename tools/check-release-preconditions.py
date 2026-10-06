#!/usr/bin/env python3
"""Decide whether a `v*` tag may be deployed, from facts the deploy workflow gathers.

    tools/check-release-preconditions.py < facts.json

# Why a script, and why it only decides

`deploy.yml`'s `publish` job holds the Cloudflare credential and, since deploy-on-tag, runs on a
`v*` tag push rather than a `main` merge. A tag is not covered by the branch ruleset: whoever can
push one can reach the credential, and a tag can point at a commit `ci` never ran. So before any
byte is uploaded, every one of these must hold, and a single missing one refuses:

  * the ref is a `v*` tag, and the tag object is **annotated** (a lightweight tag is a bare
    pointer anyone can move; an annotated tag is an object with an author and a message);
  * the tagged commit is an ancestor of `origin/main`, or equal to it -- i.e. it is merged, so it
    passed `ci` through the main ruleset;
  * the `ci` run for the **main push** of that exact commit concluded success -- every job, with
    `headSha` equal to the tagged commit and the event a branch `push`, not a pull request;
  * the `deploy` build run for that same main push concluded success -- that is the run that built
    the production-origin bytes and uploaded them as an artifact;
  * the downloaded artifact's `.release-sha` **equals the tagged commit** -- the bytes are the
    ones that commit builds, checked with `tools/check-release-artifact.sh`, the same command the
    build job runs on the fresh payload on every main push.

This script is the DECISION over those facts, gathered by the workflow (git, `gh run`,
`check-release-artifact.sh`) and handed in as JSON on stdin. The split is what makes it testable: the
gather touches the network and the live repo and cannot run in a unit test, but the decision is a
pure function of the facts, so `tools/test-check-release-preconditions.sh` feeds it a fixture for
each refusal and one that passes. The workflow's job is to GATHER honestly and to invoke this
before the upload; `tools/check-deploy-workflow.py` asserts that structure.

# The facts (stdin JSON)

    {
      "ref": "refs/tags/v1.2.3",          # GITHUB_REF of the tag push
      "tag_object_type": "tag",            # `git cat-file -t <tag>`: "tag" annotated, "commit" lightweight
      "tagged_sha": "<40 hex>",            # the commit the tag resolves to
      "merged": true,                      # tagged commit is ancestor of / equal to origin/main
      "ci_run": {"conclusion": "success", "head_sha": "<40 hex>", "event": "push",
                 "jobs": [{"name": "...", "conclusion": "success"}, ...]},
      "build_run": {"conclusion": "success", "head_sha": "<40 hex>", "event": "push",
                    "jobs": [...]},
      "release_sha_ok": true               # .release-sha in the artifact == tagged sha (and it is complete)
    }

A missing key is a refusal, never a pass: a gather step that failed to produce a fact must not
read as the fact being satisfied.

# Output

Prints one `refuse: <reason>` per failed gate and exits non-zero, or `OK -- <sha> may deploy` and
exits zero. It names every failure rather than the first, so a tag that is wrong three ways says so.
"""

from __future__ import annotations

import json
import re
import sys

SHA = re.compile(r"\A[0-9a-f]{40}\Z")


def run_is_green(run: object, sha: str, label: str) -> list[str]:
    """Every way a run's facts can fail the 'green on this commit, from a main push' test."""
    problems: list[str] = []
    if not isinstance(run, dict):
        return [f"the {label} run is absent, so it cannot be confirmed green"]
    if run.get("conclusion") != "success":
        problems.append(f"the {label} run concluded {run.get('conclusion')!r}, not success")
    head = run.get("head_sha")
    if head != sha:
        problems.append(
            f"the {label} run is for {head!r}, not the tagged commit {sha!r} -- "
            f"a run on another commit says nothing about this one"
        )
    if run.get("event") != "push":
        problems.append(
            f"the {label} run's event is {run.get('event')!r}, not a branch push -- "
            f"only the main push is gated by the ruleset"
        )
    jobs = run.get("jobs")
    if not isinstance(jobs, list) or not jobs:
        problems.append(f"the {label} run lists no jobs, so 'success' cannot be read per job")
    else:
        for job in jobs:
            name = job.get("name", "?") if isinstance(job, dict) else "?"
            concl = job.get("conclusion") if isinstance(job, dict) else None
            # `skipped` is fine (a job may not apply); anything but success/skipped is not.
            if concl not in ("success", "skipped"):
                problems.append(f"the {label} run's job {name!r} concluded {concl!r}, not success")
    return problems


def decide(facts: object) -> list[str]:
    if not isinstance(facts, dict):
        return ["the facts are not an object"]
    problems: list[str] = []

    ref = facts.get("ref")
    if not isinstance(ref, str) or not ref.startswith("refs/tags/v"):
        problems.append(f"the ref {ref!r} is not a refs/tags/v* tag")

    if facts.get("tag_object_type") != "tag":
        problems.append(
            f"the tag is {facts.get('tag_object_type')!r}, not an annotated tag object "
            f"(a lightweight tag is a bare pointer and is refused)"
        )

    sha = facts.get("tagged_sha")
    if not isinstance(sha, str) or not SHA.match(sha):
        problems.append(f"the tagged commit {sha!r} is not a 40-hex sha")
        sha = None

    if facts.get("merged") is not True:
        problems.append(
            "the tagged commit is not an ancestor of origin/main (or equal to it), so it is not "
            "merged and did not pass ci through the main ruleset"
        )

    if sha is not None:
        problems += run_is_green(facts.get("ci_run"), sha, "ci")
        problems += run_is_green(facts.get("build_run"), sha, "deploy build")

    if facts.get("release_sha_ok") is not True:
        problems.append(
            "the downloaded artifact's .release-sha does not equal the tagged commit (or the "
            "artifact is missing or incomplete), so the bytes are not the ones this commit builds"
        )

    return problems


def main() -> int:
    try:
        facts = json.load(sys.stdin)
    except (json.JSONDecodeError, ValueError) as exc:
        print(f"refuse: the facts are not valid JSON ({exc})", file=sys.stderr)
        return 2
    problems = decide(facts)
    if problems:
        for problem in problems:
            print(f"refuse: {problem}", file=sys.stderr)
        return 1
    print(f"OK -- {facts['tagged_sha']} may deploy")
    return 0


if __name__ == "__main__":
    sys.exit(main())
