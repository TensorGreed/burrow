#!/usr/bin/env python3
"""The deploy workflow cannot be reached from outside `main`, and gates before it uploads.

WHY THIS IS A CHECK AND NOT A COMMENT. `.github/workflows/deploy.yml` holds a credential that
can write to the live site. Its triggers are the security boundary, and a trigger is one line:
somebody adding `pull_request:` to "test the deploy on a branch" would hand that credential to
a workflow any fork can start.

WHY IT IS A PARSER AND NOT A GREP, which is the more important half. The first version of this
file matched YAML with `grep` and `sed`, and a security review walked past it five ways:

  * ``  "pull_request":``       -- quoted key; the trigger allowlist did not see it
  * ``  pull_request :``        -- a space before the colon; valid YAML, invisible to the regex
  * ``  push:\\n    tags: ['**']`` -- the branch rule tested PRESENCE, so a sibling tag filter
                                   passed and any tag push deployed
  * a workflow-level ``env:``   -- placed above ``on:``, it reaches EVERY job, and the job
                                   scanner reported "secrets referenced only in: publish"
  * a comment naming the gate   -- read as an invocation, in the rule about ordering

Each produced ``OK -- the deploy workflow cannot be triggered from outside main``, which is the
exact false statement this file exists to make impossible. Worse than any one of them: the
allowlist rule UNDER-COUNTED silently instead of refusing a line it could not parse, printing
two of three triggers -- the "4 of 15 reads as success" shape `CLAUDE.md` has a rule about,
inside the report.

A byte pattern over a language with more than one spelling cannot be made safe by adding
patterns; the next spelling is always one nobody thought of. So the rules run over the PARSED
structure, and an allowlist over a parsed structure cannot under-count.

WHAT IT EXAMINES, reported on every run: the workflow's trigger set, its push filter, which
jobs reference secrets, the gate steps in the publish job and their order against the upload,
and every other workflow that mentions the same credential.

Self-test: tools/test-check-deploy-workflow.sh.
"""

from __future__ import annotations

import pathlib
import re
import sys

REPO = pathlib.Path(__file__).resolve().parent.parent
DEFAULT_WORKFLOW = REPO / ".github" / "workflows" / "deploy.yml"

#: Exactly these, no more and no fewer. Gated on the SET rather than on the absence of bad
#: ones: a workflow that lost `workflow_dispatch` would pass every "must not" rule and be
#: undeployable by hand, and one that gained `schedule` would deploy on a timer nobody asked
#: for.
ALLOWED_TRIGGERS = {"push", "workflow_dispatch"}

#: The job that may hold the credential, and the only one.
CREDENTIAL_JOB = "publish"

#: Gates that must run in that job before the upload, by the script each one invokes.
REQUIRED_PRE_UPLOAD_GATES = ["tools/check-deployable-build.sh"]

#: And after it: reading the state back. `CLAUDE.md` records three measured cases of a green
#: exit over something that had not happened, and this is the workflow whose outcome is a live
#: website. Both of these were inline shell once; the first was wrong in a way nothing could
#: catch, because an inline gate is a gate nothing can test.
REQUIRED_POST_UPLOAD_CHECKS = ["tools/check-deployment-url.sh"]

#: The secret prefix that must appear in no other workflow. `deploy.yml` being airtight is
#: worth nothing if `ci.yml` -- which DOES run on `pull_request` -- can read the same token.
CREDENTIAL_PREFIX = "CLOUDFLARE_"


class Refused(Exception):
    """A rule failed. The message is the reason, and it names what to do."""


def load(path: pathlib.Path):
    try:
        import yaml
    except ImportError as exc:  # pragma: no cover - environment, not logic
        # LOUDLY, NOT AS A SKIP. A check that quietly stops checking because an import failed
        # is worse than no check: the run is green and nothing says why.
        raise Refused(
            f"PyYAML is not available ({exc}), and this check refuses to run on a YAML file "
            f"without a YAML parser. Install it, or run this where it exists -- do not "
            f"replace it with a grep, which is what the five bypasses in this file's "
            f"docstring were."
        ) from exc
    text = path.read_text(encoding="utf-8")
    documents = list(yaml.safe_load_all(text))
    if len(documents) != 1:
        raise Refused(
            f"{show(path)} contains {len(documents)} YAML documents; GitHub Actions reads one"
        )
    return documents[0], text


def show(path: pathlib.Path) -> str:
    try:
        return str(path.relative_to(REPO))
    except ValueError:
        return str(path)


def triggers_of(workflow: dict) -> dict:
    """The `on:` block, whichever way YAML decided to spell the key.

    `on` is a YAML 1.1 boolean, so `yaml.safe_load` turns the bare key into `True` while
    `"on":` stays a string. Both are the same workflow to GitHub. Reading only one of them
    would make the quoted spelling invisible, which is precisely the class of bug this file
    was rewritten to remove.
    """
    for key in (True, "on"):
        if key in workflow:
            block = workflow[key]
            break
    else:
        raise Refused("no `on:` block: this file does not look like a workflow")

    # `on: [push]` and `on: push` are both legal and neither carries a filter.
    if isinstance(block, str):
        return {block: None}
    if isinstance(block, list):
        return {name: None for name in block}
    if isinstance(block, dict):
        return block
    raise Refused(f"the `on:` block is a {type(block).__name__}, which is not a trigger list")


def check_triggers(workflow: dict, report: list[str]) -> None:
    triggers = triggers_of(workflow)
    names = set(triggers)
    if names != ALLOWED_TRIGGERS:
        extra = sorted(names - ALLOWED_TRIGGERS)
        missing = sorted(ALLOWED_TRIGGERS - names)
        detail = []
        if extra:
            detail.append(f"unexpected: {', '.join(extra)}")
        if missing:
            detail.append(f"missing: {', '.join(missing)}")
        raise Refused(
            f"triggers are {{{', '.join(sorted(names))}}}, expected "
            f"{{{', '.join(sorted(ALLOWED_TRIGGERS))}}} ({'; '.join(detail)}). This workflow "
            f"holds a credential that writes to the live site, so anything that can start it "
            f"from outside `main` hands that credential over. To exercise a deploy without "
            f"merging, dispatch it -- and note that a dispatch runs the workflow file from the "
            f"ref it is dispatched FROM, so the `production` environment's deployment-branch "
            f"rule is what bounds that, not this check."
        )
    report.append(f"triggers: {' '.join(sorted(names))}")

    push = triggers.get("push")
    if push is None:
        raise Refused(
            "the `push` trigger has no filter, so a push to ANY branch or tag deploys. "
            "It must be exactly `branches: [main]`."
        )
    if not isinstance(push, dict):
        raise Refused(f"the `push` trigger is a {type(push).__name__}, not a filter map")
    # EXACT, NOT "branches is present". A sibling `tags: ['**']` passed the first version of
    # this rule, and a push of any tag then deployed that tag's tree.
    if push != {"branches": ["main"]}:
        raise Refused(
            f"the `push` filter is {push!r}, expected exactly {{'branches': ['main']}}. "
            f"A `tags:` filter beside it deploys on any tag; a wider `branches:` deploys "
            f"from any branch."
        )
    report.append("push restricted to exactly branches: [main]")


def secret_references(value, path: str = "") -> list[str]:
    """Every place a secret is read, found structurally rather than by scanning lines.

    Both spellings: `secrets.NAME` and `secrets['NAME']` / `secrets[format(...)]`, the second
    of which carries no `secrets.` substring at all and was invisible to the first version.
    """
    found: list[str] = []
    if isinstance(value, dict):
        for key, item in value.items():
            found.extend(secret_references(item, f"{path}.{key}"))
    elif isinstance(value, list):
        for index, item in enumerate(value):
            found.extend(secret_references(item, f"{path}[{index}]"))
    elif isinstance(value, str):
        if re.search(r"\bsecrets\s*[.\[]", value):
            found.append(path or "<root>")
    return found


def check_credential_scope(workflow: dict, report: list[str]) -> None:
    # THE WHOLE FILE, NOT JUST THE JOBS. A workflow-level `env:` is inherited by EVERY job, so
    # a token declared there sits in the environment that clones emsdk, compiles third-party
    # C++ and runs a package manager. The first version scanned for a "last two-space key" and
    # reported `secrets referenced only in: publish` over exactly that arrangement.
    outside = [
        where
        for key, value in workflow.items()
        if key != "jobs"
        for where in secret_references(value, str(key))
    ]
    if outside:
        raise Refused(
            f"a secret is referenced OUTSIDE the jobs block, at: {', '.join(outside)}. A "
            f"workflow-level `env:` reaches every job, including the one that builds -- which "
            f"is the arrangement this rule exists to prevent."
        )

    jobs = workflow.get("jobs")
    if not isinstance(jobs, dict) or not jobs:
        raise Refused("no `jobs:` block, so there is nothing to attribute a credential to")

    using = sorted(name for name, job in jobs.items() if secret_references(job))
    if using != [CREDENTIAL_JOB]:
        raise Refused(
            f"jobs referencing secrets: {using or ['<none>']}, expected exactly "
            f"['{CREDENTIAL_JOB}']. A credential that can write to the live site must not be "
            f"in the environment of a job that builds anything."
        )
    report.append(f"secrets referenced only in the `{CREDENTIAL_JOB}` job")


def step_label(step: dict) -> str:
    return step.get("name") or step.get("uses") or "<unnamed step>"


def check_gates_before_upload(workflow: dict, report: list[str]) -> None:
    job = workflow["jobs"].get(CREDENTIAL_JOB)
    if not isinstance(job, dict):
        raise Refused(f"there is no `{CREDENTIAL_JOB}` job to check")
    steps = job.get("steps")
    if not isinstance(steps, list) or not steps:
        raise Refused(f"the `{CREDENTIAL_JOB}` job has no steps")

    # BY JOB AND BY STEP INDEX, never by line number in the file. The first version compared
    # line numbers over the whole file, so the BUILD job's copy of the gate satisfied the rule
    # and the publish-side one could be deleted entirely without anything noticing -- which is
    # the only gate over the artifact bytes.
    def index_of(needle: str) -> int | None:
        for position, step in enumerate(steps):
            # `run:` only. A comment naming the script is not an invocation, and reading one
            # as such produced "the origin gate runs AFTER the upload" for a comment, which
            # cannot run at all.
            body = step.get("run")
            if isinstance(body, str) and re.search(needle, body):
                return position
        return None

    upload = index_of(r"wrangler\b[^\n]*pages\s+deploy")
    if upload is None:
        raise Refused(
            f"the `{CREDENTIAL_JOB}` job never runs `wrangler pages deploy`; this check is "
            f"looking at a workflow that does not deploy"
        )

    for gate in REQUIRED_PRE_UPLOAD_GATES:
        at = index_of(re.escape(gate))
        if at is None:
            raise Refused(
                f"`{gate}` never runs in the `{CREDENTIAL_JOB}` job. It is the only gate over "
                f"the bytes that are actually uploaded -- the build job's copy runs before the "
                f"artifact round trip, so a dropped `_headers` or a mangled `connect-src` "
                f"would reach the live site."
            )
        if at > upload:
            raise Refused(
                f"`{gate}` runs at step {at + 1} ({step_label(steps[at])}), AFTER the upload "
                f"at step {upload + 1}. That is a report, not a gate."
            )
    for check in REQUIRED_POST_UPLOAD_CHECKS:
        at = index_of(re.escape(check))
        if at is None:
            raise Refused(
                f"`{check}` never runs in the `{CREDENTIAL_JOB}` job. Nothing would then read "
                f"back where the deploy went, and a build uploaded to the wrong project is "
                f"green and wrong."
            )
        if at < upload:
            raise Refused(
                f"`{check}` runs at step {at + 1}, BEFORE the upload at step {upload + 1}. "
                f"It reads back what the upload reported, so it cannot precede it."
            )
    report.append(
        f"{len(REQUIRED_PRE_UPLOAD_GATES)} pre-upload gate(s) and "
        f"{len(REQUIRED_POST_UPLOAD_CHECKS)} post-upload read-back(s) in `{CREDENTIAL_JOB}`, "
        f"around the upload at step {upload + 1}"
    )


def check_no_harness(workflow: dict, report: list[str]) -> None:
    """`BURROW_HARNESS` is never SET. Naming it in prose is fine and is how it is explained."""
    def envs(value):
        if isinstance(value, dict):
            for key, item in value.items():
                if key == "env" and isinstance(item, dict):
                    yield from item
                yield from envs(item)
        elif isinstance(value, list):
            for item in value:
                yield from envs(item)

    offending = sorted({name for name in envs(workflow) if "BURROW_HARNESS" in str(name)})
    if offending:
        raise Refused(
            f"the workflow SETS {', '.join(offending)}. `/harness` drives a real file through "
            f"the real worker and must never be on a public build."
        )
    report.append("BURROW_HARNESS is never set (the variable, not the word)")


def check_project(workflow: dict, report: list[str]) -> None:
    """Both inputs are stated, and the upload reads the variable rather than a literal.

    THE PROJECT NAME WAS DERIVED FROM THE HOSTNAME AND THAT WAS WRONG. Cloudflare names a
    project's subdomain after the project *by convention*, not by rule -- this project is
    `burrow` and its generated subdomain is `burrow-f2s`, because `burrow.pages.dev` was taken.
    The first deploy failed with `The Pages project "burrow-f2s" does not exist`.

    It failed loudly and uploaded nothing, which is the only reason it was cheap. What makes it
    worth a rule is the shape: the derivation was CORRECT REASONING about a real convention,
    and it would have gone on looking correct. A rule that is right for the wrong reason
    survives review, because the reasoning is what gets reviewed.

    So both are explicit, and this asserts:

      * both are set -- a missing project name would deploy nowhere, a missing origin would
        build for the development default;
      * they are allowed to DIFFER, which is why nothing here compares them;
      * the upload step reads `$BURROW_PAGES_PROJECT` rather than restating the name, so the
        one place it is written is the one place it is read.

    What stops a wrong project name reaching the live site is not this rule but the read-back
    after the upload, which compares wrangler's reported alias against `BURROW_PAGES_HOST`.
    (It compared against `BURROW_SITE` until the custom domain: wrangler reports an alias on
    the project's pages.dev host and never on the custom domain, so that comparison could not
    be satisfied by any correct deploy.)
    """
    env = workflow.get("env") or {}
    project = env.get("BURROW_PAGES_PROJECT")
    if not project:
        raise Refused(
            "BURROW_PAGES_PROJECT is not set. It was derived from BURROW_SITE until a deploy "
            "failed on it -- Cloudflare's subdomain matches the project name by convention "
            "rather than by rule. State it."
        )
    project = str(project)
    if not re.fullmatch(r"[a-z0-9][a-z0-9-]{0,57}", project):
        raise Refused(
            f"BURROW_PAGES_PROJECT is {project!r}, which is not a Cloudflare Pages project "
            f"name (lowercase letters, digits and hyphens)."
        )

    # READ, NOT RESTATED. A literal in the `wrangler` command would be a second statement of
    # the same fact, which is how the two drift -- and the whole point of naming it once.
    steps = workflow["jobs"][CREDENTIAL_JOB]["steps"]
    upload = next(
        (
            step
            for step in steps
            if isinstance(step.get("run"), str)
            and re.search(r"wrangler\b[^\n]*pages\s+deploy", step["run"])
        ),
        None,
    )
    if upload is None:
        raise Refused("no `wrangler pages deploy` step to check the project name against")
    if "BURROW_PAGES_PROJECT" not in upload["run"]:
        raise Refused(
            "the upload step does not read $BURROW_PAGES_PROJECT; it states the project name "
            "a second time, which is the drift this rule exists to prevent"
        )
    report.append(f"project `{project}`, read from the variable rather than restated")


def check_pages_host(workflow: dict, report: list[str]) -> None:
    """The alias the read-back trusts is a project's host, and the read-back reads it.

    A THIRD FACT ARRIVED WITH THE CUSTOM DOMAIN, and it is a trust anchor. `wrangler` reports a
    per-deployment alias on the PROJECT's `pages.dev` host, never on the custom domain, so the
    post-upload check compares the alias against `BURROW_PAGES_HOST` rather than against
    `BURROW_SITE` -- with a custom domain, no correct deploy satisfies the old comparison.

    Once that value is supplied it carries the whole comparison. A value that is too SHORT
    silently widens it: `pages.dev` is a valid-looking hostname, and under it every Cloudflare
    Pages project in the world is "an alias of ours". `tools/check-deployment-url.sh` validates
    its own argument; this rule is the other half, so that a wrong value in the workflow is
    refused before a run rather than at the point it stops catching anything.

    Nothing here compares it to `BURROW_PAGES_PROJECT`. They are allowed to differ -- that is
    the whole lesson of the rule above -- and the convention that links them is exactly what
    must not be re-derived.
    """
    env = workflow.get("env") or {}
    host = env.get("BURROW_PAGES_HOST")
    if not host:
        raise Refused(
            "BURROW_PAGES_HOST is not set. The post-upload read-back compares wrangler's "
            "reported alias against it; without it the check falls back to BURROW_SITE, which "
            "no custom-domain deploy can satisfy."
        )
    host = str(host)
    if "://" in host or "/" in host:
        raise Refused(f"BURROW_PAGES_HOST is {host!r}, which is a URL. It is a HOST.")
    if not re.fullmatch(r"[a-z0-9][a-z0-9-]{0,57}\.pages\.dev", host):
        raise Refused(
            f"BURROW_PAGES_HOST is {host!r}, which is not a <project>.pages.dev name. "
            f"`pages.dev` alone is the zone: under it every Pages project would be accepted "
            f"as a deployment of ours."
        )

    steps = workflow["jobs"][CREDENTIAL_JOB]["steps"]
    if not any(
        isinstance(step.get("run"), str) and "BURROW_PAGES_HOST" in step["run"] for step in steps
    ):
        raise Refused(
            "no step reads $BURROW_PAGES_HOST, so the alias read-back is still comparing "
            "against something else and this variable is decoration"
        )
    report.append(f"alias read-back anchored on `{host}`, read from the variable")


def check_origin(workflow: dict, report: list[str]) -> None:
    site = (workflow.get("env") or {}).get("BURROW_SITE")
    if not site:
        raise Refused(
            "BURROW_SITE is not set. The build would be for the development default and "
            "every engine fetch on the live site would be refused (ADR 0014 §4)."
        )
    site = str(site)
    if not site.startswith("https://"):
        # SRI, the blob: worker and the whole CSP story assume a secure context. The first
        # version refused only localhost, which let any plain-http host through.
        raise Refused(f"BURROW_SITE is {site}; a deploy origin must be https://")
    if re.match(r"https://(localhost|127\.0\.0\.1|\[::1\])(:|/|$)", site):
        raise Refused(f"BURROW_SITE is {site}, which is a development origin")
    report.append(f"BURROW_SITE: {site}")


def check_no_other_workflow_holds_the_credential(report: list[str]) -> None:
    """The route this file structurally could not see.

    `deploy.yml` being airtight is worth nothing if another workflow can read the same token.
    `ci.yml` runs on `pull_request`, so a branch pushed to this repository that adds one line
    to it would read the credential out on the next PR -- and a check that reads only
    `deploy.yml` would never look.
    """
    directory = REPO / ".github" / "workflows"
    others = sorted(p for p in directory.glob("*.yml") if p.name != DEFAULT_WORKFLOW.name)
    others += sorted(directory.glob("*.yaml"))
    offenders = [
        show(path) for path in others if CREDENTIAL_PREFIX in path.read_text(encoding="utf-8")
    ]
    if offenders:
        raise Refused(
            f"{', '.join(offenders)} mention{'s' if len(offenders) == 1 else ''} "
            f"{CREDENTIAL_PREFIX}*. Only the deploy workflow may read that credential: every "
            f"other workflow here runs on `pull_request`, so a one-line edit on a branch would "
            f"read it out. If a second workflow genuinely needs it, that is an ADR, not an edit."
        )
    if not others:
        raise Refused(
            "no other workflows found to check, which means this rule examined nothing"
        )
    report.append(f"{len(others)} other workflow(s), none mentioning {CREDENTIAL_PREFIX}*")


PROBES = [
    # Each rule against a structure it must reject and a near-miss it must accept, every run.
    # These are the parsed-structure analogue of the pattern probes the shell version had, and
    # they exist for the same reason: a rule that accepts everything passes everything.
    (
        "the trigger allowlist",
        lambda: check_triggers({True: {"push": {"branches": ["main"]}, "pull_request": None}}, []),
        lambda: check_triggers({True: {"push": {"branches": ["main"]}, "workflow_dispatch": None}}, []),
    ),
    (
        "the quoted-key spelling",
        lambda: check_triggers({"on": {"push": {"branches": ["main"]}, "pull_request": None}}, []),
        lambda: check_triggers({"on": {"push": {"branches": ["main"]}, "workflow_dispatch": None}}, []),
    ),
    (
        "the push filter",
        lambda: check_triggers(
            {True: {"push": {"branches": ["main"], "tags": ["**"]}, "workflow_dispatch": None}}, []
        ),
        lambda: check_triggers({True: {"push": {"branches": ["main"]}, "workflow_dispatch": None}}, []),
    ),
    (
        "the pages-host anchor",
        # MUST REJECT: the zone itself, under which any project's alias is "ours".
        lambda: check_pages_host({"env": {"BURROW_PAGES_HOST": "pages.dev"}}, []),
        # MUST ACCEPT: a real project host, read by a step in the credential job.
        lambda: check_pages_host(
            {
                "env": {"BURROW_PAGES_HOST": "burrow-f2s.pages.dev"},
                "jobs": {CREDENTIAL_JOB: {"steps": [{"run": "x \"$BURROW_PAGES_HOST\""}]}},
            },
            [],
        ),
    ),
    (
        "the credential scope",
        lambda: check_credential_scope(
            {"env": {"T": "${{ secrets.X }}"}, "jobs": {"publish": {}}}, []
        ),
        lambda: check_credential_scope(
            {"env": {"T": "plain"}, "jobs": {"publish": {"env": {"T": "${{ secrets.X }}"}}}}, []
        ),
    ),
    (
        "the bracket spelling of a secret",
        lambda: check_credential_scope(
            {"env": {"T": "${{ secrets['X'] }}"}, "jobs": {"publish": {}}}, []
        ),
        lambda: check_credential_scope(
            {"env": {"T": "not a secret at all"}, "jobs": {"publish": {"run": "${{ secrets.X }}"}}}, []
        ),
    ),
    (
        "the harness rule",
        lambda: check_no_harness({"env": {"BURROW_HARNESS": "1"}}, []),
        lambda: check_no_harness({"env": {"BURROW_SITE": "https://x"}}, []),
    ),
    (
        "the project-name rule",
        lambda: check_project(
            {"env": {"BURROW_SITE": "https://x.pages.dev"}, "jobs": {"publish": {"steps": []}}}, []
        ),
        lambda: check_project(
            {
                "env": {"BURROW_SITE": "https://x.pages.dev", "BURROW_PAGES_PROJECT": "burrow"},
                "jobs": {
                    "publish": {
                        "steps": [{"run": "wrangler pages deploy d --project-name \"$BURROW_PAGES_PROJECT\""}]
                    }
                },
            },
            [],
        ),
    ),
    (
        "the project name is READ, not restated",
        lambda: check_project(
            {
                "env": {"BURROW_SITE": "https://x.pages.dev", "BURROW_PAGES_PROJECT": "burrow"},
                "jobs": {
                    "publish": {"steps": [{"run": "wrangler pages deploy d --project-name burrow"}]}
                },
            },
            [],
        ),
        lambda: check_project(
            {
                "env": {"BURROW_SITE": "https://x.pages.dev", "BURROW_PAGES_PROJECT": "burrow"},
                "jobs": {
                    "publish": {
                        "steps": [{"run": "wrangler pages deploy d --project-name \"$BURROW_PAGES_PROJECT\""}]
                    }
                },
            },
            [],
        ),
    ),
    (
        "the origin rule",
        lambda: check_origin({"env": {"BURROW_SITE": "http://localhost:4321"}}, []),
        lambda: check_origin({"env": {"BURROW_SITE": "https://burrow.example"}}, []),
    ),
]


def run_probes() -> int:
    for name, must_refuse, must_accept in PROBES:
        try:
            must_refuse()
        except Refused:
            pass
        else:
            raise Refused(
                f"probe: '{name}' accepted its own counter-fixture, so the rule is inert"
            )
        try:
            must_accept()
        except Refused as exc:
            raise Refused(
                f"probe: '{name}' refused a near-miss it must accept ({exc}), so it would "
                f"refuse a safe workflow"
            ) from exc
    return len(PROBES)


def main(argv: list[str]) -> int:
    path = pathlib.Path(argv[0]).resolve() if argv else DEFAULT_WORKFLOW
    report: list[str] = []
    try:
        probes = run_probes()
        print(f"check-deploy-workflow: {probes} rule(s) probed against a fixture and a near-miss")
        if not path.is_file():
            raise Refused(f"no deploy workflow at {show(path)}")
        workflow, _ = load(path)
        if not isinstance(workflow, dict):
            raise Refused(f"{show(path)} does not parse as a workflow mapping")
        check_triggers(workflow, report)
        check_credential_scope(workflow, report)
        check_gates_before_upload(workflow, report)
        check_no_harness(workflow, report)
        check_origin(workflow, report)
        check_project(workflow, report)
        check_pages_host(workflow, report)
        check_no_other_workflow_holds_the_credential(report)
    except Refused as exc:
        for line in report:
            print(f"  {line}")
        print(f"::error::{exc}", file=sys.stderr)
        return 1

    for line in report:
        print(f"  {line}")
    print("OK -- the deploy workflow cannot be reached from outside main, and gates before it uploads.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
