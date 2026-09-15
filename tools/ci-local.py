#!/usr/bin/env python3
"""Run locally what CI runs, and refuse to run at all if the two have drifted apart.

WHY THIS EXISTS

Four CI failures in two batches, every one of them a green local sweep that had skipped a job:

    cargo audit                  M1 PR 4c   the only Rust gate that consults the network
    tools/check-wasm-exports.sh  #63        two FFI declarations with no argued entry
    the fuzz target list         #63        `reorder` declared in Cargo.toml, run nowhere
    pnpm check                   #63        a bridge signature changed; lint and test passed

`CLAUDE.md` already carried the rule -- *"replicating CI locally means every job"* -- written
after the first one. It did not prevent the next three. **A habit that has failed four times is
not a control**, which is the same conclusion this repository reached about `git add -A`, and
the same answer applies: make it a script that fails.

WHAT MAKES THIS DIFFERENT FROM A LIST OF COMMANDS

A hand-written list of local commands drifts from `ci.yml` silently, and the drift is invisible
exactly when it matters -- the day a new check is added. So this does not merely run commands.
It **reads `ci.yml`**, extracts every significant command CI actually invokes, and refuses to
run anything unless each one is either covered locally or has an argued exemption naming why it
cannot be.

Add a check to CI and forget to add it here, and this fails before running a single test.

Usage:
  tools/ci-local.py              parity check, then run everything
  tools/ci-local.py --check      parity check only; run nothing
  tools/ci-local.py --list       print the coverage table and exit
  tools/ci-local.py --only NAME  run one local job (parity is still checked first)
"""

from __future__ import annotations

import json
import os
import re
import shlex
import shutil
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
CI = REPO / ".github" / "workflows" / "ci.yml"
CI_REL = ".github/workflows/ci.yml"
NVMRC_REL = ".nvmrc"

# --- what counts as a "significant command" in a CI run block --------------------------
#
# Deliberately narrow. Shell plumbing -- `export`, `for`, `echo`, `grep` -- is not a gate and
# matching it would make this noise. What is matched is the things that can FAIL for a reason
# a developer needs to see before pushing.
PATTERNS: list[tuple[str, str]] = [
    (r"\bcargo \+nightly fuzz run \"?\$?\{?(\w+)", r"fuzz:\1"),
    # A NAMED INTEGRATION SUITE IS ITS OWN GATE, and must be matched BEFORE the generic
    # `cargo test` below or it is swallowed by it.
    #
    # That swallowing is the sixth miss of the class this tool exists for, and the only one that
    # got past the parity check rather than past a habit. ADR 0019 §2's gate was a `run:` block
    # invoking `cargo test -p burrow-ops --test split_no_leak -- --exact <name>`; it mapped to
    # `cargo:test`, which the local `test` job already covered, so parity reported FULL coverage
    # while that gate had no local counterpart at all. The branch closing #54 ran this tool clean
    # and went red on that step.
    #
    # `--test <suite>` is the thing worth keying on: a step that names one suite is a gate about
    # that suite, not a re-run of the workspace. A `$`-interpolated name is skipped rather than
    # matched as the literal `$suite`, because a token nobody can cover is noise -- such a step
    # belongs in a script, which is how this one was fixed.
    (r"\bcargo test\b[^\n|&;]*?--test \"?([a-z_][\w-]*)", r"test:\1"),
    (r"\bcargo (fmt|clippy|test|deny|audit|build|doc|install)\b", r"cargo:\1"),
    # `wasm-pack build`, which is NOT a cargo subcommand and so matched nothing above. It
    # produced the fifth miss of the class this tool exists for: `bindings/burrow-wasm/pkg/`
    # is gitignored, `stage-web-engines.mjs` copies whatever is in it, and CI builds it
    # fresh -- so a local sweep measured the size budget against a binding from whenever
    # anyone last ran wasm-pack by hand. ADR 0022 grew that binding by 9.5% and every local
    # run said the budget was fine.
    # THE CRATE IS PART OF THE KEY, not just the verb: a second binding built by CI would
    # otherwise map to the same gate as the first and read as covered. Caught by the
    # self-test's own case, which is what that case is for.
    (r"\bwasm-pack build (\S+)", r"wasm-pack:\1"),
    (r"\bpython3 (tools/[\w.-]+\.py)", r"\1"),
    (r"(?<![\w/])(tools/[\w.-]+\.sh)", r"\1"),
    # A GATE MAY LIVE OUTSIDE `tools/`, AND ONE DOES. `.claude/hooks/*.sh` is where a hook's
    # self-test has to live, because a hook path is what `.claude/settings.json` names -- and
    # this pattern was added because parity REFUSED the first attempt to wire one in: CI ran
    # `.claude/hooks/test-refuse-force-push-to-main.sh`, the local runner claimed to cover it,
    # and the extractor saw neither. That is the fifth-row failure from CLAUDE.md, caught this
    # time by the check rather than by a red CI run, and the fix is the extractor rather than
    # the claim.
    (r"(?<![\w/])(\.claude/hooks/[\w.-]+\.sh)", r"\1"),
    # ANY pnpm script, not a fixed list. The first version enumerated lint|check|build|test|e2e,
    # which meant a NEW script added to CI matched nothing and was reported as covered -- the
    # precise class of miss this tool exists for, reproduced inside the tool. `exec`, `install`
    # and `dlx` are excluded because they are not scripts; `pnpm exec playwright test` is
    # matched by the playwright pattern below instead.
    (r"\bpnpm (?:-C \S+ )?(?:run )?(?!exec\b|install\b|dlx\b)([a-z][\w:-]*)", r"pnpm:\1"),
    # `pnpm exec playwright test` is the e2e gate; it does not spell itself `pnpm e2e`.
    (r"\bplaywright test\b", r"pnpm:e2e"),
    # Node tooling. `report-size-budget.mjs` is a real gate and was invisible to the first
    # version of this file, which only looked for `tools/*.py` and `tools/*.sh`.
    (r"\bnode \S*?(tools/[\w.-]+\.mjs)", r"\1"),
]

# --- gates CI runs as ACTIONS rather than as shell --------------------------------------
#
# `cargo deny` and `cargo audit` do not appear in any `run:` block -- they are third-party
# actions. The first version of this file therefore reported both as "covered locally but CI
# does not run them", which is backwards and would have invited deleting the local commands.
ACTIONS: dict[str, str] = {
    "EmbarkStudios/cargo-deny-action": "cargo:deny",
    "rustsec/audit-check": "cargo:audit",
}

# --- commands CI runs that CANNOT have a local counterpart -----------------------------
#
# Each needs a reason, and the reason is printed on every run. An exemption with a weak reason
# is visible rather than buried, which is the only thing keeping this list honest.
EXEMPT: dict[str, str] = {
    "cargo:install": (
        "installs a pinned tool (cargo-audit, cargo-fuzz, wasm-pack) into the runner. "
        "Locally these are already installed; installing them is not a gate."
    ),
    "tools/ci-local.py": (
        "this file. CI runs the parity check to keep this table honest; running it as a local "
        "job of itself would be circular. It is covered by being the thing you are running."
    ),
}

# --- the local counterparts -------------------------------------------------------------
#
# `covers` is what each entry discharges from the extracted set. A command listed in `covers`
# that CI does not actually run is dead weight and is reported, so this list cannot rot in the
# other direction either.
JOBS: list[dict] = [
    {
        "name": "fmt",
        "run": "cargo fmt --all -- --check",
        "covers": ["cargo:fmt"],
    },
    {
        "name": "clippy",
        "run": "cargo clippy --workspace --all-targets --all-features -- -D warnings",
        "covers": ["cargo:clippy"],
    },
    {
        "name": "test",
        "run": "cargo test --workspace --all-features",
        "covers": ["cargo:test"],
        "needs_qpdf_cli": True,
    },
    {
        "name": "ignored-tests",
        "run": (
            "cargo test -p burrow-ops --all-features --test reorder_keeps_everything -- "
            "--ignored --exact a_damaged_document_loses_pages_on_write"
        ),
        "covers": [],
        "needs_qpdf_cli": True,
        "why": "the engine-seam defect CI requires to keep reproducing beneath ADR 0022's refusal (#61)",
    },
    {
        # THE SEVENTH MISS, AND THE SECOND FROM THE SAME CAUSE AS `subsetting-gate` BELOW.
        #
        # PR #76 added `core/burrow-ops/tests/optimistic_counts.rs`, ran this tool clean, and
        # went red in CI on a gate that was a `run:` block invoking `cargo test … --test
        # "$suite" -- --list`. That maps to `cargo:test`, which the `test` job above covers, so
        # parity reported FULL coverage over a gate `cargo test --workspace` does not perform
        # at all. Same shape, same answer: the gate moved into a script, and this is its job.
        "name": "integration-suites",
        "run": "tools/check-integration-suites.sh && tools/test-check-integration-suites.sh",
        "covers": [
            "tools/check-integration-suites.sh",
            "tools/test-check-integration-suites.sh",
        ],
        "why": "every integration suite on disk is named and discovers tests (#76)",
    },
    {
        # ADDED AFTER THIS RUNNER REPORTED FULL PARITY AND CI WENT RED ANYWAY.
        #
        # The gate was a `run:` block invoking `cargo test`, and the extractor below maps that to
        # a token the `test` job already covers -- so parity passed while the specific gate had no
        # local counterpart. Moving it into a script is what makes it visible here, and this entry
        # is the other half of that: a named command with a named local job.
        "name": "subsetting-gate",
        "run": "tools/check-subsetting-gate.sh && tools/test-check-subsetting-gate.sh",
        "covers": [
            "tools/check-subsetting-gate.sh",
            "tools/test-check-subsetting-gate.sh",
        ],
        "needs_qpdf_cli": True,
        "why": "ADR 0019 §2's rule, and that the tests asserting it are not ignored (#54)",
    },
    {
        # The mutation sweep that stands behind the shared pruning policy. It plants the deleted
        # `prune_output` call on each extractor in turn and requires the suite covering that side
        # to turn red, which is the only thing left that can catch one path not reaching the
        # policy -- see `testsupport/expectations.rs`'s `Operation::Split`.
        "name": "prune-is-reached",
        "run": "tools/test-prune-is-reached.sh",
        "covers": ["tools/test-prune-is-reached.sh"],
        "why": "each split path is shown to reach the shared pruning policy (#54)",
    },
    {
        "name": "corpus",
        "run": (
            "cargo run -p burrow-engines --all-features "
            "--example make-conformance-fixtures -- M1 --check"
        ),
        "covers": [],
        "why": "the committed corpus is what its generator produces",
    },
    {
        "name": "doc",
        "run": 'RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps',
        "covers": ["cargo:doc"],
    },
    {
        "name": "deny",
        "run": "cargo deny check",
        "covers": ["cargo:deny"],
    },
    {
        "name": "audit",
        "run": "cargo audit",
        "covers": ["cargo:audit"],
    },
    {
        "name": "wasm",
        "run": "cargo build -p burrow-wasm --target wasm32-unknown-unknown",
        "covers": ["cargo:build"],
    },
    {
        "name": "checkers",
        "run": " && ".join(
            [
                "python3 tools/check-engine-licences.py",
                "python3 tools/check-qpdf-trapped.py",
                "python3 tools/check-handle-identity.py",
                "python3 tools/check-python-syntax.py",
                "tools/check-no-generated-files.sh",
                "tools/check-no-network-deps.sh",
                "tools/check-wasm-exports.sh",
                "tools/check-qpdf-crypto.sh",
                "python3 tools/detect-engine-components.py",
            ]
        ),
        "covers": [
            "tools/check-engine-licences.py",
            "tools/check-qpdf-trapped.py",
            "tools/check-handle-identity.py",
            "tools/check-python-syntax.py",
            "tools/check-no-generated-files.sh",
            "tools/check-no-network-deps.sh",
            "tools/check-wasm-exports.sh",
            "tools/check-qpdf-crypto.sh",
            "tools/detect-engine-components.py",
        ],
    },
    {
        "name": "checker-self-tests",
        "run": " && ".join(
            [
                "tools/test-check-qpdf-trapped.sh",
                "tools/test-check-handle-identity.sh",
                "tools/test-check-no-generated-files.sh",
                "tools/test-check-wasm-exports.sh",
                "tools/test-check-qpdf-crypto.sh",
                "tools/test-detect-engine-components.sh",
                "tools/test-check-engine-licences.sh",
                "tools/test-check-no-network-deps.sh",
                "tools/test-seed-fuzz-corpus.sh",
                "tools/test-ci-local.sh",
                # NOT UNDER tools/, and that is the only reason it stands out. It is the
                # self-test for a PreToolUse hook, which lives beside the hook it tests
                # because a hook path is what `.claude/settings.json` names. It is a control
                # like any other here: it refuses a force-push to `main` in any spelling, and
                # a control nothing runs is a control nobody knows is broken.
                ".claude/hooks/test-refuse-force-push-to-main.sh",
            ]
        ),
        "covers": [
            "tools/test-check-qpdf-trapped.sh",
            "tools/test-check-handle-identity.sh",
            "tools/test-check-no-generated-files.sh",
            "tools/test-check-wasm-exports.sh",
            "tools/test-check-qpdf-crypto.sh",
            "tools/test-detect-engine-components.sh",
            "tools/test-check-engine-licences.sh",
            "tools/test-check-no-network-deps.sh",
            "tools/test-seed-fuzz-corpus.sh",
            "tools/test-ci-local.sh",
            ".claude/hooks/test-refuse-force-push-to-main.sh",
        ],
    },
    {
        "name": "fuzz-seed",
        "run": "python3 tools/seed-fuzz-corpus.py --check",
        "covers": ["tools/seed-fuzz-corpus.py"],
    },
    {
        "name": "fuzz",
        "run": (
            "cd fuzz && "
            'export LD_LIBRARY_PATH="$PWD/../engines/vendor/native-$(uname -m)/lib" && '
            'export RUSTFLAGS="$RUSTFLAGS -L native=$PWD/../engines/vendor/native-$(uname -m)/lib/fuzz" && '
            "export ASAN_OPTIONS=detect_leaks=0 && "
            "for t in document_open prescan pdfsyntax_names pdfsyntax_dict_keys "
            "qpdf_check rotate reorder merge split; do "
            # UNSEEDED, matching CI, and `rm -rf` is what makes it so: the corpus persists
            # between runs, so a local sweep would otherwise be seeded from whatever the last
            # nightly-style run left behind and reproduce #62 while CI stayed green.
            'rm -rf "corpus/$t" && mkdir -p "corpus/$t" && '
            'cargo +nightly fuzz run "$t" -- -max_total_time=60 -timeout=10 -rss_limit_mb=2048 '
            "|| exit 1; done"
        ),
        "covers": [
            "fuzz:document_open",
            "fuzz:prescan",
            "fuzz:pdfsyntax_names",
            "fuzz:pdfsyntax_dict_keys",
            "fuzz:qpdf_check",
            "fuzz:rotate",
            "fuzz:reorder",
            "fuzz:merge",
            "fuzz:split",
        ],
        "slow": True,
    },
    {
        "name": "wasm-pack",
        "run": (
            "wasm-pack build bindings/burrow-wasm --target no-modules --out-dir pkg --release"
        ),
        "covers": ["wasm-pack:bindings/burrow-wasm"],
        # BEFORE `web`, because `web` stages `pkg/` into the app and measures the result
        # against the size budget. Running them the other way round measures the previous
        # build, which is what happened when this job did not exist.
        "why": "the wasm binding the size budget is measured against",
    },
    {
        "name": "web",
        "run": (
            "cd apps/web && pnpm prebuild && pnpm lint && pnpm check && pnpm build && "
            "pnpm test && node ../../tools/report-size-budget.mjs dist"
        ),
        # `prebuild` stages the engines and regenerates the CSP. It was missing from every
        # local sweep until this tool's pnpm pattern stopped enumerating script names and
        # started matching any of them -- so an engine change could have produced a stale CSP
        # locally and been caught only in CI. `pnpm build` runs it as a lifecycle hook, but
        # naming it is what makes the coverage checkable.
        "covers": [
            "pnpm:prebuild",
            "pnpm:lint",
            "pnpm:check",
            "pnpm:build",
            "pnpm:test",
            "tools/report-size-budget.mjs",
            # NOT `stage-web-engines.mjs` or `generate-credits.mjs`. They run through the
            # `prebuild` lifecycle hook rather than from ci.yml, so claiming them here is
            # coverage of something CI does not invoke -- which the stale check refuses, and
            # did refuse when this entry first claimed them.
        ],
    },
    {
        # AFTER `web`, because it reads the `dist/` that job produces. Its own self-test runs
        # beside it: the checker is made entirely of ABSENCE rules, which is the shape that
        # passes on an empty directory or a build that never ran.
        "name": "no-pdfium-on-the-web",
        "run": (
            "tools/check-no-pdfium-on-the-web.sh && tools/test-check-no-pdfium-on-the-web.sh"
        ),
        "covers": [
            "tools/check-no-pdfium-on-the-web.sh",
            "tools/test-check-no-pdfium-on-the-web.sh",
        ],
        "why": "PDFium reaches no part of the web build (spike 0004)",
    },
    {
        "name": "web-e2e",
        "run": "cd apps/web && pnpm e2e",
        "covers": ["pnpm:e2e"],
        "slow": True,
    },
]


def commands_ci_runs() -> dict[str, list[str]]:
    """Every significant command in `ci.yml`, mapped to the step names that invoke it.

    Read as TEXT rather than through a YAML parser, deliberately. The interesting content is
    inside `run:` block scalars -- shell, not YAML -- and a parser would hand back the same
    strings after more ceremony. What matters is that nothing is filtered out on the way.
    """
    text = CI.read_text(encoding="utf-8")
    found: dict[str, list[str]] = {}
    step = "(unnamed step)"
    # The most recent `for target in a b c` seen, so `cargo +nightly fuzz run "$target"`
    # resolves to the targets it will actually run. Without this the extractor yields the
    # literal token `fuzz:target` and every real target looks uncovered.
    loop_targets: list[str] = []

    def record(token: str) -> None:
        found.setdefault(token, [])
        if step not in found[token]:
            found[token].append(step)

    for line in text.splitlines():
        named = re.search(r"^\s*- name:\s*(.+?)\s*$", line)
        if named:
            step = named.group(1)
            # A STEP NAME IS PROSE, NOT A COMMAND, and the patterns used to run over it too.
            # A step called "The installed node and pnpm match the pins" yielded the token
            # `pnpm:match`, which parity then reported as a gate with no local counterpart --
            # a phantom that can only ever be satisfied by inventing a job to cover it.
            # Nothing is lost by skipping: a gate lives in `run:` or `uses:`, never in a label.
            continue
        stripped = line.strip()
        # A comment cannot invoke anything, and these files are heavily commented.
        if stripped.startswith("#"):
            continue

        loop = re.search(r"\bfor \w+ in ([a-z_ ]+); do", line)
        if loop:
            loop_targets = loop.group(1).split()

        # `- uses:` as well as a bare `uses:`. The first version matched only the bare form,
        # so it picked up cargo-deny (which happens to sit under a `- name:`) and missed
        # cargo-audit (which does not) -- covering one gate and reporting the other as stale.
        uses = re.search(r"^\s*-?\s*uses:\s*([\w.-]+/[\w.-]+)@", line)
        if uses and uses.group(1) in ACTIONS:
            record(ACTIONS[uses.group(1)])

        for pattern, template in PATTERNS:
            for match in re.finditer(pattern, line):
                token = match.expand(template)
                if token in ("fuzz:target", "fuzz:t"):
                    for name in loop_targets:
                        record(f"fuzz:{name}")
                    continue
                record(token)
    return found


def parity(found: dict[str, list[str]]) -> tuple[list[str], list[str]]:
    """Commands CI runs that nothing here covers, and coverage claims CI does not back."""
    covered = {c for job in JOBS for c in job["covers"]}
    uncovered = sorted(t for t in found if t not in covered and t not in EXEMPT)
    stale = sorted(c for c in covered if c not in found)
    return uncovered, stale


def workflow_env() -> dict[str, str]:
    """`ci.yml`'s workflow-level `env:` block, which every job inherits.

    **READ, NOT COPIED.** It carries `RUSTFLAGS: -D warnings`, and this runner did not apply
    it --- so every local cargo job ran under weaker lints than CI, and a warning-level
    regression passed here and failed there. That is the sixth instance of the class this
    file exists for, and the fix has to be the same shape as the parity table: derived from
    `ci.yml` so it cannot drift, rather than a constant somebody remembers to update.

    Parsed rather than YAML-loaded for the reason the extractor below gives: no dependency.
    The block is flat `KEY: value` pairs at one indent level, and a shape this does not
    understand is reported rather than skipped.
    """
    text = CI.read_text(encoding="utf-8")
    out: dict[str, str] = {}
    inside = False
    for line in text.splitlines():
        if line.startswith("env:"):
            inside = True
            continue
        if inside:
            if not line.strip() or line.lstrip().startswith("#"):
                continue
            # The block ends at the next top-level key.
            if not line.startswith(" "):
                break
            key, separator, value = line.strip().partition(":")
            if not separator:
                raise SystemExit(f"ci-local: unparsed line in ci.yml's env block: {line!r}")
            out[key.strip()] = value.strip().strip('"').strip("'")
    if not out:
        # AN EMPTY RESULT IS A FAILURE, NOT A DEFAULT. Delete or rename `ci.yml`'s
        # workflow-level `env:` and this would return `{}`, print nothing, and run every job
        # WITHOUT `-D warnings` -- reintroducing the exact regression this function closes, by
        # removing the thing it reads. "A check that silently examines nothing is worse than
        # no check" (CLAUDE.md), and this is that check examining nothing.
        raise SystemExit(
            "ci-local: ci.yml has no workflow-level `env:` block.\n"
            "  It is where `RUSTFLAGS: -D warnings` lives, and without it every local job\n"
            "  runs under weaker lints than CI. If the block genuinely moved, teach\n"
            "  workflow_env() where it went -- do not let it return nothing."
        )
    return out


# --- the environment preflight ----------------------------------------------------------
#
# REFUSE ON A BROKEN ENVIRONMENT; DO NOT REPORT IT AS TEST FAILURES.
#
# This is `needs_qpdf_cli`'s argument generalised, and it is here because that argument was
# right and was applied to exactly one hand-declared tool. A machine with no `cargo` on PATH
# does not get a refusal: it gets `cargo: not found` from every Rust job, seven failures inside
# `tools/test-check-no-network-deps.sh` whose message is `cargo tree failed`, and a sweep that
# reads as "this change broke the network-dependency checker". It did not. Nothing was measured.
#
# TWICE NOW. M0's rustfmt hook failed SILENTLY on the same cause; the split-page batch produced
# the seven-failure run above. A habit that has failed twice is not a control -- the conclusion
# this repository already reached about `git add -A` and about replicating CI by hand.
#
# DERIVED, NOT ENUMERATED, in both halves. The tool list is read out of the JOBS table's own
# `run` strings, and every pinned version is read out of the file that pins it. A hand-written
# list of required tools drifts the day somebody adds a job; a hand-written version number is a
# second copy of a pin, and the two disagree silently.
#
# WHAT IT DOES NOT CATCH, said here rather than discovered:
#
#   * A tool that is present, correctly versioned and broken. `which` answers "is there a
#     file"; a `--version` probe answers "what does it say it is".
#   * A missing FILE argument -- `python3 tools/x.py` requires `python3`, and a deleted `x.py`
#     is a different failure that git already makes visible.
#   * A cargo subcommand's own dependencies: `cargo fuzz` is checked for presence and version,
#     and its need for a C toolchain is not modelled.

SHELL_KEYWORDS = frozenset({"do", "then", "else", "elif"})
SHELL_BUILTINS = frozenset(
    {
        "cd", "export", "exit", "set", "unset", "echo", "true", "false",
        "source", ".", "return", "shift", "read", "eval", "trap", "wait",
        "for", "while", "until", "if", "fi", "done", "esac", "case", "in",
    }
)

SEPARATORS = frozenset({"&&", "||", ";", "|"})

# `VAR=value`, AFTER shlex has removed the quoting -- so `RUSTDOCFLAGS="-D warnings"` arrives as
# the single word `RUSTDOCFLAGS=-D warnings` and matches. Splitting on whitespace instead of
# shlex broke exactly here, and PROGRAM_CASES caught it: the assignment's own quoted space ended
# the word, `RUSTDOCFLAGS="-D` was stripped as an assignment, and `warnings"` was reported as a
# program to look for on PATH.
ASSIGNMENT = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*=")

TOOLCHAIN = re.compile(r"^\+(\S+)$")

# A name no package manager will ever install. Resolved on EVERY run, through every branch of
# the resolver, and required to come back missing -- a resolver that answers "present" to
# everything reports a perfect environment on a machine with nothing installed.
SENTINEL = "burrow-no-such-program-preflight-canary"

# One tool implying another, because it cannot run without it.
#
# MEASURED, NOT PRECAUTIONARY. `node` reached the requirement set only because ONE step in the
# web job spells it literally (`node ../../tools/report-size-budget.mjs`). Rewrite that step as
# a pnpm script -- a legitimate refactor that parity accepts once `JOBS` is updated with it --
# and `node` leaves `required`, `due` shrinks with it, and the whole node pin goes quiet:
# security review measured `1 of 1 pinned version(s) compared`, exit 0, on a machine running
# node 24 against `.nvmrc` pinned at 22. The `consumer` and `conflict` guards sit inside the
# same gate and go inert in the same move.
#
# This is a fact about the TOOL, not a list of jobs: pnpm is a node program and cannot run
# without one. That is what keeps it from being the enumeration this file argues against.
IMPLIES: dict[str, frozenset[str]] = {"pnpm": frozenset({"node"})}

# The binary each derived kind is asked of. If that binary is itself missing, the derived rows
# say nothing new -- nine "cargo subcommand missing" lines for one absent cargo is noise
# standing in front of the finding.
DERIVED_HOST = {"cargo:": "cargo", "rustup:": "rustup"}


def _commands(command: str) -> list[list[str]]:
    """`command` as a list of argv-shaped commands, split on the shell separators."""
    # `punctuation_chars=True` is what makes `;` and `&&` their own tokens. `shlex.split` does
    # NOT do this -- it splits on whitespace respecting quotes and nothing else, so
    # `...split; do rm -rf ...` arrives as one command beginning with `for`, the whole segment
    # is discarded as a keyword, and `rm` is never looked for. PROGRAM_CASES caught that; the
    # union over the real JOBS table did not, because `mkdir` sat behind an `&&` and survived.
    lexer = shlex.shlex(command, posix=True, punctuation_chars=True)
    lexer.whitespace_split = True
    out: list[list[str]] = []
    current: list[str] = []
    for word in lexer:
        if word in SEPARATORS:
            if current:
                out.append(current)
            current = []
            continue
        current.append(word)
    if current:
        out.append(current)
    return out


def programs_in(command: str) -> set[str]:
    """Every program `command` invokes, as written.

    A token containing `/` is a path this repository owns and is checked on disk; a bare token
    is looked up on PATH. `cargo X` additionally yields `cargo:X`, so a missing subcommand is a
    refusal rather than sixty seconds of fuzzing that could not start, and `cargo +T` yields
    `rustup:T`.
    """
    found: set[str] = set()
    for words in _commands(command):
        while words and (words[0] in SHELL_KEYWORDS or ASSIGNMENT.match(words[0])):
            words = words[1:]
        if not words:
            continue
        program = words[0]
        if program in SHELL_BUILTINS or program in SHELL_KEYWORDS:
            continue
        found.add(program)

        if program == "cargo":
            rest = words[1:]
            if rest and (m := TOOLCHAIN.match(rest[0])):
                found.add(f"rustup:{m.group(1)}")
                rest = rest[1:]
            if rest and not rest[0].startswith("-"):
                found.add(f"cargo:{rest[0]}")
    return found


def programs_used_in(text: str, candidates: frozenset[str] | set[str]) -> set[str]:
    """Which `candidates` a script's BODY invokes.

    THE PREFLIGHT IS TRANSITIVE OR IT DOES NOT CLOSE ITS OWN MOTIVATING INCIDENT. Five jobs
    have a `run` string that is nothing but script names, so the only requirement derived from
    them was "that file exists". `checkers` runs `tools/check-no-network-deps.sh`, which calls
    `cargo tree` eight times -- so on a machine with no cargo the preflight passed `checkers`
    and the job then failed with `cargo tree failed`, which is the LITERAL STRING this file's
    own docstring cites as the incident it exists to prevent. Found by security review, after
    the first version shipped that gap.

    NARROW ON PURPOSE. Parsing a script body as shell was tried and is unusable: these files
    are heavily commented and carry embedded Python heredocs, Rust snippets and prose, so
    `programs_in` over every line yielded 23 real tools and about 200 tokens like `fn f() {`
    and `Playwright report output`. So the search is inverted -- the candidate set is the
    programs the JOBS table already names DIRECTLY, and a body is only asked whether it uses
    one of those. That keeps the property this file argues for: derived, not enumerated. A tool
    reached only from inside a script and never named in `ci.yml` is out of scope, and that is
    what `needs_qpdf_cli` is for.
    """
    found: set[str] = set()
    # HEREDOC BODIES ARE DATA, NOT COMMANDS, and this is where every false positive lived.
    # `tools/test-ci-local.sh` embeds Python that rewrites ci.yml, so it contains the literal
    # `"pnpm test && pnpm run size-budget"` -- and `&&` is a command position, so the scan
    # reported `checker-self-tests` as needing pnpm and probed it in the wrong directory. The
    # refusal was against a correct machine, which is the direction that makes a check
    # useless. Skipping heredoc bodies is exact rather than heuristic: the shell does not run
    # them either.
    heredoc: str | None = None
    for line in text.splitlines():
        if heredoc is not None:
            if line.strip() == heredoc:
                heredoc = None
            continue
        opener = re.search(r"<<-?\s*['\"]?([A-Za-z_][A-Za-z0-9_]*)['\"]?\s*$", line)
        if opener:
            heredoc = opener.group(1)
            continue
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        for tool in candidates:
            if tool in found:
                continue
            # AT A COMMAND POSITION, not merely present. "present anywhere on a non-comment
            # line" was the first rule and the full sweep refused on it: `pnpm` matched a
            # regex literal in this very file and `run: pnpm typecheck` inside a heredoc in
            # `test-ci-local.sh`, so six jobs were reported as needing pnpm and the probe then
            # ran in the wrong directory. A rule that fires on a correct machine is not a
            # check -- the same sentence the per-pin probe directory is written under.
            #
            # A command position is the start of a line, or immediately after one of the
            # operators that begins a new command. `tree="$(cargo tree ...` matches on `$(`;
            # `run: pnpm typecheck` does not, because `:` begins nothing.
            if COMMAND_POSITION[tool].search(line):
                found.add(tool)
    return found


SCRIPT_REF = re.compile(
    r"(?<![\w/.\-])((?:tools|\.claude/hooks)/[\w.\-]+\.(?:sh|py|mjs))"
)


class _CommandPosition(dict):
    """Per-tool regexes for "invoked here", built on demand and cached."""

    def __missing__(self, tool: str) -> re.Pattern[str]:
        pattern = re.compile(
            r"(?:^|&&|\|\||[;|({`]|\$\()\s*" + re.escape(tool) + r"(?=\s)",
            re.MULTILINE,
        )
        self[tool] = pattern
        return pattern


COMMAND_POSITION = _CommandPosition()

# ONLY SHELL SCRIPTS ARE SCANNED, and this is a stated limit rather than an oversight.
#
# A Python file's docstring can contain a table that reads exactly like a command -- this
# file's own opening docstring has the line `    pnpm check   #63   a bridge signature
# changed`, which no positional rule can distinguish from an invocation. Scanning `.py` and
# `.mjs` produced six jobs falsely requiring pnpm, which is the false-positive direction and
# the worse one.
#
# The residual: a Python or Node checker that shells out to a pinned tool is invisible here.
# Measured before accepting it -- no `tools/*.py` invokes cargo, wasm-pack or pnpm today; the
# two `cargo` mentions in `check-engine-licences.py` are prose. If one ever does, it belongs
# in a `.sh` wrapper or in `needs_qpdf_cli`-style declaration.
SCANNABLE = (".sh",)


def tool_candidates() -> frozenset[str]:
    """Every bare program the JOBS table names directly, anywhere.

    THE UNIVERSE IS THE WHOLE TABLE, not the job being examined, and getting that wrong made
    the transitive scan inert on exactly the job it was written for: `checkers` names only
    `python3` directly, so a per-job universe was `{python3}` and `cargo tree` inside
    `check-no-network-deps.sh` matched nothing. The report said "5 program(s) required", the
    same number as before the scan existed.
    """
    universe: set[str] = set()
    for job in JOBS:
        universe |= {p for p in programs_in(job["run"]) if "/" not in p and ":" not in p}
    return frozenset(universe)


def transitive_programs(direct: set[str], candidates: frozenset[str]) -> dict[str, set[str]]:
    """Every repository script reachable from `direct`, mapped to the candidates it uses.

    Follows script-to-script references too, with a visited set, because a self-test that
    invokes a checker inherits that checker's tools.
    """
    seen: set[str] = set()
    queue = [p for p in direct if "/" in p]
    out: dict[str, set[str]] = {}
    while queue:
        rel = queue.pop()
        if rel in seen:
            continue
        seen.add(rel)
        if not rel.endswith(SCANNABLE):
            continue
        try:
            text = (REPO / rel).read_text()
        except OSError:
            continue  # its absence is the presence check's finding
        out[rel] = programs_used_in(text, candidates)
        for ref in SCRIPT_REF.findall(text):
            if ref not in seen:
                queue.append(ref)
    return out


# EVERY PARSER RULE, WITH ITS OWN CASE AND A NEAR-MISS, CHECKED ON EVERY RUN.
#
# `tools/check-engine-licences.py`'s SPLIT_CASES is the shape and the reason: a parser that
# mis-reads a command makes every verdict below it meaningless, and this one decides whether a
# sweep runs at all. Each entry names the rule it pins.
PROGRAM_CASES: list[tuple[str, set[str]]] = [
    # a plain command, and its subcommand
    ("cargo fmt --all -- --check", {"cargo", "cargo:fmt"}),
    # a leading environment assignment is not the command
    ('RUSTDOCFLAGS="-D warnings" cargo doc --workspace', {"cargo", "cargo:doc"}),
    # `&&` starts a new command, and a repository path is itself a program
    (
        "tools/check-subsetting-gate.sh && tools/test-check-subsetting-gate.sh",
        {"tools/check-subsetting-gate.sh", "tools/test-check-subsetting-gate.sh"},
    ),
    # an interpreter is the program; its script is an argument
    ("python3 tools/check-python-syntax.py", {"python3"}),
    # `cd` is a builtin and contributes nothing; pnpm is reached through it
    ("cd apps/web && pnpm lint && pnpm check", {"pnpm"}),
    # a `for` loop: the keyword, its list and `do` are not programs; the body is
    (
        'for t in a b; do rm -rf "corpus/$t" && cargo +nightly fuzz run "$t" || exit 1; done',
        {"rm", "cargo", "rustup:nightly", "cargo:fuzz"},
    ),
    # NEAR-MISS: a bare `--flag` after cargo is not a subcommand
    ("cargo --list", {"cargo"}),
    # NEAR-MISS: an assignment with no command is not a command
    ("export ASAN_OPTIONS=detect_leaks=0", set()),
]

# A body-scan fixture per rule, with its near-misses. `programs_used_in` decides whether a job
# needs `cargo` at all, so a rule that matched nothing here would restore the exact gap
# security review found.
USES_CASES: list[tuple[str, set[str]]] = [
    ("cargo tree --workspace --edges normal\n", {"cargo"}),
    ("  node ../../tools/report-size-budget.mjs dist\n", {"node"}),
    # The real shape in `check-no-network-deps.sh`: a command substitution.
    ('tree="$(cargo tree --manifest-path "$m" --workspace)"\n', {"cargo"}),
    ("mkdir -p x && cargo build\n", {"cargo"}),
    # NEAR-MISS: a comment cannot invoke anything.
    ("# cargo tree is what this checker does\n", set()),
    # NEAR-MISS: a longer word that merely contains the tool's name.
    ("mycargo tree --workspace\n", set()),
    # NEAR-MISS: the name inside a quoted string is not a command word.
    ("echo 'cargo'\n", set()),
    # NEAR-MISS: a path ending in the tool's name is a different program.
    ("engines/cargo build\n", set()),
    # NEAR-MISS: a heredoc body is data. This exact shape -- a Python heredoc rewriting a
    # command string -- made `checker-self-tests` falsely require pnpm and refuse a correct
    # machine, because `&&` is a command position even inside a quoted literal.
    (
        "python3 - \"$ci\" <<'PYEOF'\n"
        "dst.write_text(text.replace(old, '\"x && cargo run size-budget\"', 1))\n"
        "PYEOF\n",
        set(),
    ),
    # And the heredoc must END: a command after the terminator is a command again.
    (
        "cat <<'EOF'\ncargo build\nEOF\nnode tools/x.mjs\n",
        {"node"},
    ),
    # NEAR-MISS: the two shapes that made the full sweep refuse. A tool named after a YAML
    # key inside a heredoc, and a tool inside a regex literal. Neither is an invocation.
    ("        run: cargo typecheck\n", set()),
    ('    (r"\\bcargo (?:-C \\S+ )?(?:run )?([a-z][\\w:-]*)", r"cargo:\\1"),\n', set()),
]


def verify_parser() -> list[str]:
    """Every parser rule against its own case and its near-miss.

    SEPARATE FROM THE ENVIRONMENT CHECK, and run on every invocation including `--check`. The
    two answer different questions: this one asks whether the thing that decides what to look
    for works at all, and a parser that reads nothing reports a healthy, EMPTY requirement set
    on a machine with no tools installed. That is the failure the gate exists to refuse,
    arriving through the gate itself.
    """
    problems = [
        f"{cmd!r}: expected programs {sorted(exp)}, got {sorted(programs_in(cmd))}"
        for cmd, exp in PROGRAM_CASES
        if programs_in(cmd) != exp
    ]
    candidates = frozenset({"cargo", "node"})
    problems += [
        f"body scan of {text!r}: expected {sorted(exp)}, got {sorted(programs_used_in(text, candidates))}"
        for text, exp in USES_CASES
        if programs_used_in(text, candidates) != exp
    ]
    # AND THE PARSER OVER THE REAL TABLE, not only over fixtures. `shlex` raises on an
    # unbalanced quote, so a job string containing an apostrophe -- an `awk` one-liner, a
    # `don't` in a message -- would give a traceback instead of a refusal. That is the failure
    # mode `_listing`'s docstring rejects, and fixtures cannot see it because fixtures are not
    # the table. Found by security review.
    for job in JOBS:
        try:
            programs_in(job["run"])
        except ValueError as error:
            problems.append(f"job {job['name']!r} has a run string the parser cannot read: {error}")
    return [f"the command parser is broken, so nothing it reports means anything -- {p}" for p in problems]


# --- the pinned versions, each read from the file that pins it ---------------------------
#
# A cargo on PATH at the wrong version is the same failure wearing a disguise: the sweep runs,
# every job reports something, and none of it describes what CI will do. So presence is not
# enough.
#
# EVERY EXPECTED VERSION IS DERIVED. Writing `1.98.1` here would be a second copy of
# `rust-toolchain.toml`'s pin, and the day somebody bumps one the other says nothing while
# looking authoritative -- the same defect as a hand-written list of local commands, one layer
# down. A pin that cannot be RESOLVED is therefore a refusal, not a skip: a regex that stops
# matching because a file was reformatted would otherwise silently check nothing.
#
# EACH PROBE RUNS IN THE DIRECTORY OF THE FILE THAT PINS IT, which is load-bearing rather than
# tidy: `pnpm --version` answers 12.3.4 at the repository root and 11.26.0 inside `apps/web` on
# one machine at one moment, because corepack reads `packageManager`. Deriving that directory
# from a JOB made the answer depend on which jobs were selected and in what order, and refused
# a correct machine. The file that states a version is where that version is authoritative.
PINS: list[dict] = [
    {
        "program": "cargo",
        "source": "rust-toolchain.toml",
        "pin": (r'^channel = "([^"]+)"', "rust-toolchain.toml"),
        "probe": ["cargo", "--version"],
        "field": 1,
        "match": "exact",
    },
    {
        "program": "wasm-pack",
        "source": CI_REL,
        "pin": (r"cargo install wasm-pack --locked --version ([0-9][\w.+-]*)", CI_REL),
        "probe": ["wasm-pack", "--version"],
        "field": 1,
        "match": "exact",
    },
    {
        "program": "cargo:fuzz",
        "source": CI_REL,
        "pin": (r"cargo install --locked --version ([0-9][\w.+-]*) cargo-fuzz", CI_REL),
        "probe": ["cargo", "fuzz", "--version"],
        "field": 1,
        "match": "exact",
    },
    {
        "program": "cargo:audit",
        "source": CI_REL,
        "pin": (r"cargo install cargo-audit --version ([0-9][\w.+-]*)", CI_REL),
        "probe": ["cargo", "audit", "--version"],
        "field": 1,
        "match": "exact",
    },
    {
        "program": "pnpm",
        "source": "apps/web/package.json",
        "pin": ("packageManager", "pnpm-package-json"),
        "probe": ["pnpm", "--version"],
        "field": 0,
        "match": "exact",
    },
    {
        # `.nvmrc`, WHICH IS ALSO WHAT CI AND `nvm use` READ. The pin used to be a literal
        # `node-version: 22` in ci.yml, so a developer's shell had nothing to agree with: this
        # check measured node 24 against CI's 22 on a machine with no way of knowing which was
        # wanted, and the answer was "run nvm use 22", which nothing recorded. One file now
        # answers for both sides.
        #
        # MAJOR ONLY, because that is what the pin says and what the runner honours. Narrowing
        # to an exact patch would be a policy change rather than a relocation.
        "program": "node",
        "source": NVMRC_REL,
        "pin": (r"^\s*v?([0-9]+)", NVMRC_REL),
        "probe": ["node", "--version"],
        "field": 0,
        "match": "major",
        # THE PIN AND THE THING THAT READS IT ARE IN DIFFERENT FILES NOW, so they are tied
        # together here. `actions/setup-node` PREFERS `node-version` over `node-version-file`
        # and only WARNS when both are given -- so somebody adding the literal back to test
        # something would leave CI on their version while `.nvmrc` sat inert and this check
        # went on comparing every developer machine against it, printing
        # "2 of 2 pinned version(s) compared". A check that silently examines nothing reads
        # as coverage. Code review.
        "consumer": (r"node-version-file:\s*\.nvmrc", CI_REL),
        "conflict": (r"^\s*node-version:\s*\S", CI_REL),
        "fix": "run `nvm use` at the repository root, which reads the same file",
    },
]

# Versions this repository has deliberately decided not to hold a machine to, with the reason
# printed on every run. Same idea as EXEMPT, and the same rule: an allowance with a weak reason
# is visible rather than buried. Empty is the correct default -- an allowance added to make a
# sweep pass is the thing this block exists to make somebody argue for.
VERSION_ALLOWANCES: dict[str, str] = {}


def _pinned_version(spec: tuple[str, str]) -> str | None:
    """The expected version, read from the file that pins it."""
    pattern, kind = spec  # `kind` is a repository-relative FILE, or a named handler
    if kind == "pnpm-package-json":
        try:
            data = json.loads((REPO / "apps/web/package.json").read_text())
        except (OSError, ValueError):
            return None
        field = data.get(pattern, "")
        m = re.match(r"pnpm@([0-9][\w.+-]*?)(?:\+sha|$)", field)
        return m.group(1) if m else None
    # RELATIVE, ALWAYS. `Path.__truediv__` discards the left side when the right is absolute,
    # so a `PINS` entry written as "/etc/..." would read outside the repository with no error.
    # Not reachable today -- every entry is a relative literal -- and one line to keep so.
    if Path(kind).is_absolute():
        return None
    path = REPO / kind
    try:
        text = path.read_text()
    except OSError:
        return None
    m = re.search(pattern, text, re.MULTILINE)
    return m.group(1) if m else None


def _reported_version(probe: list[str], field: int, cwd: Path) -> tuple[str | None, str]:
    """What the installed tool says its version is, asked in `cwd`, and why if it did not say.

    TWO OUTCOMES, NOT ONE, and collapsing them was a real defect. The first version returned
    `None` for both "the binary is absent" and "the binary is present and did not answer", and
    `check_versions` treated every `None` as "absence, reported elsewhere" -- so a `cargo` that
    answered `--list` but exited non-zero on `--version` produced `0 pinned version(s)
    compared` and a sweep that ran anyway reporting success. Zero of six compared reads exactly
    like six of six.

    IT ALSO CHECKS THE RETURN CODE, which is what made the message honest. With a mutated
    `rust-toolchain.toml`, `cargo --version` fails and rustup prints an error whose first word
    is `custom` -- so the refusal read `cargo is custom, pinned at 99.99.99`, and the
    self-test greped only for the pinned half and passed over the garbage.
    """
    if shutil.which(probe[0]) is None:
        return None, "absent"
    try:
        result = subprocess.run(
            probe, capture_output=True, text=True, check=False, cwd=cwd, timeout=60
        )
    except subprocess.TimeoutExpired:
        return None, f"{' '.join(probe)} did not answer within 60s"
    except (OSError, subprocess.SubprocessError) as error:
        return None, f"{' '.join(probe)} could not be run ({error})"
    if result.returncode != 0:
        first = (result.stderr or result.stdout).strip().splitlines()
        detail = first[0] if first else "no output"
        return None, f"{' '.join(probe)} exited {result.returncode}: {detail}"
    lines = (result.stdout or result.stderr).strip().splitlines()
    if not lines:
        return None, f"{' '.join(probe)} printed nothing"
    parts = lines[0].split()
    if len(parts) <= field:
        return None, f"{' '.join(probe)} printed {lines[0]!r}, which has no field {field}"
    return parts[field].lstrip("v"), "ok"


def check_versions(required: dict[str, list[str]]) -> tuple[list[str], str]:
    """Version findings, and a report of how many pins were compared against how many were due.

    THE COUNT IS GATED, because the expected value is knowable: it is the number of PINS whose
    program this selection of jobs requires. "0 pinned version(s) compared" reads exactly like
    "6 of 6" unless something says what it should have been -- the "4 of 15" failure this
    repository has now hit in three different tools.
    """
    problems: list[str] = []
    compared = 0
    absent = 0
    due = sum(1 for spec in PINS if spec["program"] in required)
    for spec in PINS:
        program = spec["program"]
        if program not in required:
            continue
        # THE CONSUMER FIRST. A pin nothing reads is not a pin, and comparing against it
        # would be the most confident kind of nothing.
        consumer = spec.get("consumer")
        if consumer is not None:
            pattern, rel = consumer
            try:
                text = (REPO / rel).read_text()
            except OSError:
                text = ""
            if not re.search(pattern, text, re.MULTILINE):
                problems.append(
                    f"{program}: {rel} no longer reads the pin in {spec['source']} "
                    f"(nothing matches {pattern!r}), so the pin is inert"
                )
                continue
        conflict = spec.get("conflict")
        if conflict is not None:
            pattern, rel = conflict
            try:
                text = (REPO / rel).read_text()
            except OSError:
                text = ""
            if re.search(pattern, text, re.MULTILINE):
                problems.append(
                    f"{program}: {rel} states a version inline (matches {pattern!r}), which "
                    f"takes precedence over {spec['source']} and makes it inert"
                )
                continue

        expected = _pinned_version(spec["pin"])
        if expected is None:
            # A PIN THAT CANNOT BE READ IS A REFUSAL. Skipping would mean this check quietly
            # stops examining that tool the day its pin file is reformatted, and reports the
            # same OK either way.
            problems.append(
                f"{program}: cannot read the pinned version out of {spec['source']}, "
                f"so nothing was compared for it"
            )
            continue
        # PROBED WHERE ITS PIN LIVES, not where some job happens to run. `pnpm --version`
        # answers differently per directory because corepack reads `packageManager`, and
        # deriving the directory from a JOB made the answer depend on which jobs were selected
        # and in what order -- `--only checker-self-tests` probed at the repository root and
        # refused a machine whose pnpm was exactly right for `apps/web`. The file that states
        # the version is the place that version is authoritative.
        workdir = Path(spec["source"]).parent
        # A JOB'S `cd` STAYS INSIDE THE REPOSITORY. No job has an absolute or climbing `cd`
        # today; this makes a future one a fallback rather than a probe executed somewhere
        # nobody intended. Security review.
        if workdir.is_absolute() or ".." in workdir.parts:
            cwd = REPO
        else:
            cwd = REPO / workdir
            if not cwd.is_dir():
                cwd = REPO
        actual, why = _reported_version(spec["probe"], spec["field"], cwd)
        if actual is None:
            if why == "absent":
                # Absence is the presence check's finding, not this one's; reported there.
                absent += 1
                continue
            # PRESENT AND DID NOT ANSWER IS THIS CHECK'S FINDING, not a skip. Collapsing the
            # two meant a cargo that answered `--list` and failed `--version` produced
            # "0 pinned version(s) compared" and a sweep that ran anyway reporting success.
            # The realistic trigger is not exotic: a newly-bumped `rust-toolchain.toml` makes
            # `cargo --version` install a toolchain, which can exceed the timeout.
            problems.append(
                f"{program}: the version probe did not answer, so nothing was compared "
                f"for it -- {why}"
            )
            continue
        compared += 1
        ok = (
            actual.split(".")[0] == expected.split(".")[0]
            if spec["match"] == "major"
            else actual == expected
        )
        if ok:
            continue
        if program in VERSION_ALLOWANCES:
            print(f"  version allowance: {program} {actual} against pinned {expected} "
                  f"-- {VERSION_ALLOWANCES[program]}")
            continue
        hint = f" -- {spec['fix']}" if spec.get("fix") else ""
        problems.append(
            f"{program} is {actual}, pinned at {expected} in {spec['source']} "
            f"(asked in {cwd.relative_to(REPO) if cwd != REPO else '.'}); "
            f"needed by: {', '.join(sorted(set(required[program])))}{hint}"
        )

    # EVERY PIN DUE IS ACCOUNTED FOR: compared, or absent (which the presence check reports),
    # or already a finding above. A shortfall means one slipped through silently.
    accounted = compared + absent + len(problems)
    if accounted < due:
        problems.append(
            f"{due - accounted} pinned version(s) were neither compared nor reported, so this "
            f"check examined less than it was due to"
        )
    return problems, f"{compared} of {due} pinned version(s) compared"


def resolve_program(program: str, subcommands: set[str], toolchains: set[str]) -> tuple[bool, str]:
    """Whether `program` is available, and how that was decided."""
    if program.startswith("cargo:"):
        return (program.split(":", 1)[1] in subcommands, "cargo subcommand (cargo --list)")
    if program.startswith("rustup:"):
        if shutil.which("rustup") is None:
            # Say what is actually wrong. "toolchain not installed" sends somebody to
            # `rustup toolchain install` on a machine with no rustup to run it.
            return (False, "rustup itself is not on PATH, so no toolchain can be resolved")
        return (program.split(":", 1)[1] in toolchains, "rust toolchain (rustup toolchain list)")
    if "/" in program:
        path = REPO / program
        return (path.is_file() and os.access(path, os.X_OK), "repository script")
    found = shutil.which(program)
    return (found is not None, f"PATH ({found})" if found else "PATH")


def _listing(argv: list[str]) -> str:
    """Stdout of a listing command, or empty if it is not installed.

    NOT ALLOWED TO RAISE, and that is the whole reason it is a function. The first version
    called `cargo --list` unguarded, so on a machine with no cargo the preflight itself died
    with a `FileNotFoundError` traceback -- on precisely the machine it exists to give a clear
    answer about. A gate whose failure mode is a stack trace has moved the confusion rather
    than removed it.
    """
    if shutil.which(argv[0]) is None:
        return ""
    try:
        return subprocess.run(argv, capture_output=True, text=True, check=False, timeout=60).stdout
    except (OSError, subprocess.SubprocessError):
        return ""


def _cargo_subcommands() -> set[str]:
    names: set[str] = set()
    for line in _listing(["cargo", "--list"]).splitlines():
        if not line.startswith(("    ", "\t")):
            continue
        parts = line.strip().split()
        if parts:
            names.add(parts[0])
    return names


def _rustup_toolchains() -> set[str]:
    names: set[str] = set()
    for line in _listing(["rustup", "toolchain", "list"]).splitlines():
        parts = line.split()
        if not parts:
            continue
        names.add(parts[0])
        # `nightly-aarch64-unknown-linux-gnu` is how a channel is spelled once installed; the
        # job asks for `+nightly`. Record the channel as well as the full triple.
        names.add(parts[0].split("-")[0])
    return names


def preflight(jobs: list[dict]) -> list[str]:
    """Refuse-worthy findings about this machine, plus a report of what was examined."""
    required: dict[str, list[str]] = {}
    candidates = tool_candidates()
    for job in jobs:
        direct = programs_in(job["run"])
        for program in direct:
            required.setdefault(program, []).append(job["name"])
        # AND WHAT THOSE SCRIPTS USE FROM INSIDE. Without this, `checkers` requires `python3`
        # and four file paths while `check-no-network-deps.sh` calls `cargo tree` eight times.
        # SEEDED FROM SCRIPT PATHS IN THE RUN STRING, not only from `direct`. `python3
        # tools/x.py` yields the program `python3` and the script as an ARGUMENT, so a Python
        # checker's own tool use would be invisible the same way a shell one's was. No checker
        # invokes cargo from Python today -- the two mentions in `check-engine-licences.py` are
        # prose -- so this closes the analogous gap before it is live rather than after.
        reachable = set(direct) | set(SCRIPT_REF.findall(job["run"]))
        for script, used in transitive_programs(reachable, candidates).items():
            for program in used:
                required.setdefault(program, []).append(f"{job['name']} (via {script})")

        # AND WHAT THOSE TOOLS CANNOT RUN WITHOUT.
        for program in list(required):
            for implied in IMPLIES.get(program, ()):
                if job["name"] in required.get(program, []):
                    required.setdefault(implied, []).append(f"{job['name']} (needs {program})")

    subs = _cargo_subcommands()
    chains = _rustup_toolchains()

    # THE NEGATIVE CONTROL, through every branch of the resolver, every run.
    controls = (SENTINEL, f"cargo:{SENTINEL}", f"rustup:{SENTINEL}", f"tools/{SENTINEL}.sh")
    for probe in controls:
        if resolve_program(probe, subs, chains)[0]:
            # NOTE: this returns before the `environment:` report line below, so a machine that
            # trips a control gets the refusal and no report. That is the right order -- there
            # is nothing honest to report once the resolver is known to be lying.
            return [f"the resolver reports {probe!r} as present, so its verdicts mean nothing"]

    missing_base = {p for p in required if ":" not in p and not resolve_program(p, subs, chains)[0]}

    problems: list[str] = []
    for program in sorted(required):
        prefix = next((k for k in DERIVED_HOST if program.startswith(k)), None)
        if prefix and DERIVED_HOST[prefix] in missing_base:
            continue  # its host is already the finding
        ok, how = resolve_program(program, subs, chains)
        if not ok:
            problems.append(
                f"{program} not found -- {how}; needed by: {', '.join(sorted(set(required[program])))}"
            )

    version_problems, version_report = check_versions(required)
    problems += version_problems

    print(
        f"environment: {len(required)} program(s) required by {len(jobs)} job(s), "
        f"{version_report}, {len(controls)} resolver control(s) verified"
    )
    if VERSION_ALLOWANCES:
        print(f"{len(VERSION_ALLOWANCES)} version allowance(s), with reasons, applied above")
    return problems


def qpdf_cli() -> tuple[str | None, str]:
    """Where a `qpdf` CLI can be found, and how it was found.

    Two leak suites and the subsetting gate shell out to `qpdf --qdf` to decompress output
    before scanning it, because a marker inside a flate stream is invisible to a byte scan.
    CI installs the distro package and then asserts it is on PATH.

    Locally the better answer is usually already built: `engines/build-native.sh` produces the
    CLI for the PINNED qpdf beside the library, so preferring it means the tests decompress
    with the same version they link against rather than with whatever the distro ships.
    """
    found = shutil.which("qpdf")
    if found:
        return found, "on PATH"
    for built in sorted(REPO.glob("engines/vendor/src/build-qpdf-*/qpdf/qpdf")):
        if built.is_file() and os.access(built, os.X_OK):
            return str(built), "built by engines/build-native.sh"
    return None, "not found"


def run(job: dict, env: dict[str, str] | None = None) -> bool:
    print(f"\n=== {job['name']}")
    if job.get("why"):
        print(f"    ({job['why']})")
    print(f"    $ {job['run']}")
    result = subprocess.run(job["run"], shell=True, cwd=REPO, env=env)
    ok = result.returncode == 0
    print(f"    {'PASS' if ok else 'FAIL'}  {job['name']}")
    return ok


def report_environment(findings: list[str]) -> None:
    """Say what is wrong with this machine, and why nothing was run."""
    print(f"\nREFUSED — environment not ready ({len(findings)}):", file=sys.stderr)
    for finding in findings:
        print(f"  - {finding}", file=sys.stderr)
    print(
        "\n  Nothing was run. A sweep on a machine that cannot complete it reports\n"
        "  failures that look like the change's fault: a missing cargo produces\n"
        "  `cargo tree failed` inside the network-dependency checker, which reads as a\n"
        "  broken checker. Fix the environment, or argue an entry into\n"
        "  VERSION_ALLOWANCES in tools/ci-local.py.",
        file=sys.stderr,
    )


def main(argv: list[str]) -> int:
    check_only = "--check" in argv
    listing = "--list" in argv
    # THE ENVIRONMENT CHECK, REACHABLE WITHOUT RUNNING A JOB.
    #
    # It cannot simply live under `--check`: CI runs `--check` as a parity gate on a runner that
    # deliberately has none of these tools, and refusing there would fail every PR. And it must
    # not ONLY live on the running path, because then the test that says "the preflight does not
    # refuse a job needing none of the missing tools" has no way to reach it -- which is exactly
    # what code review found: that case ran `--check`, returned before the preflight, and passed
    # against a mutant whose `preflight()` refused everything unconditionally. A case that
    # cannot observe the behaviour it names is the failure this whole file is about.
    preflight_only = "--preflight" in argv
    only = None
    if "--only" in argv:
        index = argv.index("--only")
        if index + 1 >= len(argv):
            print("error: --only needs a job name", file=sys.stderr)
            return 1
        only = argv[index + 1]

    # THE PARSER FIRST, AND ALWAYS -- including under `--check`, which runs no job. It decides
    # what the preflight looks for, so a broken one reports an empty requirement set and a
    # clean bill of health for a machine with nothing on it.
    broken = verify_parser()
    if broken:
        print("\nFAILED — the command parser does not pass its own cases:", file=sys.stderr)
        for problem in broken:
            print(f"  - {problem}", file=sys.stderr)
        return 1

    found = commands_ci_runs()
    uncovered, stale = parity(found)

    print(f"ci.yml invokes {len(found)} significant command(s)")
    if EXEMPT:
        print(f"{len(EXEMPT)} exempt, with reasons:")
        for token, reason in sorted(EXEMPT.items()):
            print(f"  {token}: {reason}")

    if listing:
        print("\ncoverage:")
        for token in sorted(found):
            owner = next((j["name"] for j in JOBS if token in j["covers"]), None)
            mark = owner or ("exempt" if token in EXEMPT else "UNCOVERED")
            print(f"  {token:38} {mark}")
        return 1 if uncovered or stale else 0

    problems = False
    if uncovered:
        problems = True
        print(
            f"\nFAILED — {len(uncovered)} command(s) CI runs have no local counterpart:",
            file=sys.stderr,
        )
        for token in uncovered:
            where = ", ".join(found[token])
            print(f"  - {token}   (ci.yml: {where})", file=sys.stderr)
        print(
            "\n  Add it to JOBS in tools/ci-local.py, or to EXEMPT with a reason.\n"
            "  This is the check that exists because four CI failures in two batches were\n"
            "  each a local sweep that had skipped a job.",
            file=sys.stderr,
        )
    if stale:
        problems = True
        print(
            f"\nFAILED — {len(stale)} local coverage claim(s) CI does not back:", file=sys.stderr
        )
        for token in stale:
            print(f"  - {token}", file=sys.stderr)
        print(
            "\n  Either the check was removed from ci.yml and should go from here too, or the\n"
            "  token changed shape and this file is now covering something that never runs.",
            file=sys.stderr,
        )
    if problems:
        return 1

    print(f"\nOK — every command ci.yml runs is covered locally or argued exempt.")

    if check_only:
        return 0

    jobs = [j for j in JOBS if only is None or j["name"] == only]
    if only and not jobs:
        print(f"error: no local job named {only!r}", file=sys.stderr)
        return 1

    if preflight_only:
        findings = preflight(jobs)
        if findings:
            report_environment(findings)
            return 1
        return 0

    # CI's workflow-level env, applied to every local job. `RUSTFLAGS: -D warnings` is the
    # one that matters and the one that was missing: every local cargo job ran under weaker
    # lints than CI, so a warning-level regression passed here and went red there.
    inherited = workflow_env()
    env = dict(os.environ)
    env.update(inherited)
    if inherited:
        applied = ", ".join(f"{k}={v}" for k, v in sorted(inherited.items()))
        print(f"\nci.yml env applied: {applied}")

    # THE ENVIRONMENT, BEFORE ANY JOB RUNS. Scoped to the jobs actually selected, so
    # `--only prune-is-reached` is not refused for a cargo it never invokes -- and so the qpdf
    # case below still reaches its own refusal rather than being pre-empted by this one.
    findings = preflight(jobs)
    if findings:
        report_environment(findings)
        return 1

    # THE `qpdf` CLI, RESOLVED BEFORE ANYTHING RUNS. `needs_qpdf_cli` was declared on three
    # jobs and read by nothing -- so on a machine without the CLI those three did not refuse,
    # they FAILED, with five tests panicking inside a testsupport helper about a missing
    # binary. That reads as "the change broke the leak tests" and it is not what happened.
    # This is the same write-only-flag defect the reviewers found twice elsewhere in this
    # batch, in a file whose entire purpose is refusing to run a sweep it cannot complete.
    needing = [j["name"] for j in jobs if j.get("needs_qpdf_cli")]
    if needing:
        cli, how = qpdf_cli()
        if cli is None:
            print(
                f"\nREFUSED — {len(needing)} job(s) need the `qpdf` CLI, which is not "
                f"installed: {', '.join(needing)}",
                file=sys.stderr,
            )
            print(
                "\n  Two leak suites and the subsetting gate decompress output with\n"
                "  `qpdf --qdf` before scanning it; without it a marker inside a flate\n"
                "  stream is invisible and the scan reports silence for the wrong reason.\n"
                "\n  Either install it (apt-get install qpdf), or run\n"
                "  engines/build-native.sh, which builds the pinned one this would prefer.\n"
                "\n  Refusing rather than running: a sweep that cannot complete must not\n"
                "  report a failure that looks like the change's fault.",
                file=sys.stderr,
            )
            return 1
        print(f"\nqpdf CLI: {cli} ({how})")
        env["PATH"] = f"{os.path.dirname(cli)}{os.pathsep}{env.get('PATH', '')}"

    failed = [j["name"] for j in jobs if not run(j, env)]
    print("\n" + "=" * 60)
    if failed:
        print(f"FAILED — {len(failed)} of {len(jobs)}: {', '.join(failed)}", file=sys.stderr)
        return 1
    print(f"OK — {len(jobs)} local job(s) passed.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
