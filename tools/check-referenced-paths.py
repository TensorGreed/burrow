#!/usr/bin/env python3
"""Every path this repository's own tooling names exists.

Two rules, because the same defect arrived twice in one change and `CLAUDE.md` is explicit
that a habit which has failed twice is not a control:

  1. every script a **workflow** invokes exists and is executable;
  2. every repository path a **tool** constructs with `pathlib` resolves.

WHY THIS EXISTS, MEASURED RATHER THAN IMAGINED

ADR 0026 renamed `tools/check-no-pdfium-on-the-web.sh` to
`tools/check-pdfium-is-render-only.sh`. `ci.yml` was updated; `deploy.yml` was not, and code
review found it. The consequence was not a red test: it was the **deploy** job dying at its
last gate before upload, on `main`, after merge -- and the gate itself silently gone.

`tools/ci-local.py` could not catch it and should not be asked to. It reads `ci.yml` and models
what a developer can run locally; `deploy.yml` holds a Cloudflare token, runs only on `main`,
and has steps that cannot have a local counterpart at all. Teaching the parity model a second
workflow with different rules would make the model the thing that rots.

So this asks a much smaller question, of **every** workflow: does the file named here exist?
That has one answer, it is the same answer for every workflow, and it needs no model of what a
gate is.

WHAT IT EXAMINES, reported on every run rather than implied: every `.yml`/`.yaml` under
`.github/workflows/` and `.github/actions/`, and every repository-relative script path any of
them names. The expected workflow count is derived from `git ls-files`, so a workflow added
in a directory this does not scan fails here rather than being skipped.

Adversarial self-test: tools/test-check-referenced-paths.sh
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

# A repository-relative script path, as a workflow spells one.
#
# ANCHORED ON THE DIRECTORY, not on the extension alone, because `foo.sh` on its own is as
# likely to be prose as a command -- and an over-eager pattern that fired on a comment is a
# check somebody turns off. These three directories are where every gate in this repository
# lives; `tools/README.md` and `CLAUDE.md` both say so.
SCRIPT = re.compile(r"(?<![\w./-])((?:tools|scripts|\.claude/hooks)/[\w.-]+\.(?:sh|py|mjs|js))")


def workflow_files() -> list[Path]:
    """Every workflow and composite action in the repository, from git rather than a glob.

    `git ls-files` rather than `Path.rglob`, for the reason `check-python-syntax.py` gives:
    a glob reports what is on disk, and what is on disk includes whatever an interrupted run
    left there. What must be checked is what is committed.
    """
    listed = subprocess.run(
        ["git", "ls-files", "*.yml", "*.yaml"],
        cwd=REPO,
        capture_output=True,
        text=True,
        check=True,
    ).stdout.split()
    return [REPO / p for p in listed if p.startswith(".github/")]


# `REPO / "a" / "b" / "c.rs"` -- a whole chain of quoted segments after `REPO /`.
#
# Anchored on `REPO` because that is what makes it a repository path rather than any other
# string. A construction built from a variable (`REPO / directory / pattern`) is deliberately
# NOT matched: it is not knowable statically, and a rule that guessed would be the noisy kind.
# Those are the globs this file's own second rule pushed tooling towards, and a glob that
# matches nothing refuses on its own.
PATHLIB_CHAIN = re.compile(r'REPO\s*((?:/\s*"[\w.-]+"\s*)+)')
SEGMENT = re.compile(r'"([\w.-]+)"')


def tool_files() -> list[Path]:
    """Every Python tool, from git rather than a glob. See `workflow_files`."""
    listed = subprocess.run(
        ["git", "ls-files", "tools/*.py", ".claude/hooks/*.py"],
        cwd=REPO,
        capture_output=True,
        text=True,
        check=True,
    ).stdout.split()
    return [REPO / p for p in listed]


def is_generated(target: Path) -> bool:
    """Whether `target` is build output this repository deliberately does not contain.

    ASKED OF GIT, not of a list here. `git check-ignore` consults the same `.gitignore` rules
    the repository already maintains, so a new build directory is covered the moment it is
    ignored -- and an allowlist in this file would be a second statement of the same thing,
    free to drift from the first.
    """
    return (
        subprocess.run(
            ["git", "check-ignore", "-q", target.relative_to(REPO).as_posix()],
            cwd=REPO,
            capture_output=True,
            check=False,
        ).returncode
        == 0
    )


def constructed_paths() -> tuple[list[str], int, int]:
    """Repository paths the tools build, how many were examined, and how many were skipped."""
    problems: list[str] = []
    examined = 0
    skipped = 0
    for path in tool_files():
        rel = path.relative_to(REPO).as_posix()
        for line in path.read_text(encoding="utf-8").splitlines():
            if line.lstrip().startswith("#"):
                continue
            for match in PATHLIB_CHAIN.finditer(line):
                segments = SEGMENT.findall(match.group(1))
                # A DIRECTORY IS NOT A FINDING. Plenty of constructions name a directory that
                # is created later, or a gitignored build output -- `engines/vendor/...` is the
                # obvious one, and refusing on it would make this fail on a clean checkout.
                # Only a path with a FILE EXTENSION is asserted to exist, which is exactly the
                # shape the two measured defects had.
                if "." not in segments[-1]:
                    continue
                target = REPO.joinpath(*segments)
                # A GITIGNORED FILE IS NOT A FINDING EITHER, and the rule above only exempted
                # directories. `tools/check-wasm-binding-names.py` reads
                # `bindings/burrow-wasm/pkg/burrow_wasm.d.ts` -- a wasm-pack output, gitignored,
                # absent on a clean checkout, and present in the job that builds it. Refusing on
                # it would make this fail on exactly the checkout CI's `checkers` job runs, which
                # fetches nothing.
                #
                # WHAT IS GIVEN UP, said rather than left to be discovered: a typo inside a
                # generated path is no longer caught here. What catches it instead is the tool
                # itself -- a checker that reads a generated file must refuse when it is absent
                # rather than reporting OK over nothing, which is the "examined nothing reads as
                # coverage" rule those tools are already held to.
                if is_generated(target):
                    skipped += 1
                    continue
                examined += 1
                if not target.exists():
                    problems.append(f"{rel} builds {'/'.join(segments)}, which does not exist")
    return problems, examined, skipped


def main() -> int:
    files = workflow_files()
    if not files:
        print(
            "::error::no workflow files under .github/ -- this check would pass vacuously",
            file=sys.stderr,
        )
        return 1

    missing: list[str] = []
    not_executable: list[str] = []
    examined: list[tuple[str, str]] = []

    for path in files:
        rel = path.relative_to(REPO).as_posix()
        for line in path.read_text(encoding="utf-8").splitlines():
            # A comment cannot invoke anything, and these files are heavily commented -- the
            # prose in `ci.yml` names scripts by path constantly, including ones being
            # discussed rather than run.
            if line.lstrip().startswith("#"):
                continue
            for match in SCRIPT.finditer(line):
                script = match.group(1)
                examined.append((rel, script))
                target = REPO / script
                if not target.is_file():
                    missing.append(f"{rel} names {script}, which does not exist")
                elif not os.access(target, os.X_OK) and script.endswith(".sh"):
                    not_executable.append(f"{rel} runs {script}, which is not executable")

    constructed, constructions, generated = constructed_paths()

    # A COUNT WITH AN EXPECTATION BESIDE IT. `CLAUDE.md`: "4 of 15" reads exactly like success,
    # so the number of workflows is compared against what git lists rather than printed alone.
    unique = {script for _, script in examined}
    print(
        f"check-referenced-paths: {len(files)} workflow file(s) under .github/, "
        f"{len(examined)} script reference(s) to {len(unique)} distinct script(s); "
        f"{len(tool_files())} tool(s), {constructions} constructed path(s) "
        f"({generated} generated, skipped)"
    )
    if constructions == 0:
        print(
            "::error::no tool constructs a repository path -- either the pattern stopped "
            "matching or the tools stopped using pathlib; both are findings",
            file=sys.stderr,
        )
        return 1
    if not unique:
        print(
            "::error::no workflow names any tools/ script -- either the pattern stopped "
            "matching or the gates left the workflows; both are findings",
            file=sys.stderr,
        )
        return 1

    for problem in missing + not_executable + constructed:
        print(f"::error::{problem}", file=sys.stderr)
    if missing or not_executable or constructed:
        print(
            "\n  Something names a file that is not there. A rename with a reference left "
            "behind is the usual cause, and the symptom depends on who was holding the stale "
            "path: a workflow dies at that step -- which for `deploy.yml` is after merge to "
            "main, at the last gate before bytes leave -- and a checker either crashes or, "
            "worse, quietly reads less than it claims to.",
            file=sys.stderr,
        )
        return 1

    print(
        f"OK -- every script {len(files)} workflow file(s) name is runnable, and every "
        f"repository path {len(tool_files())} tool(s) construct resolves."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
