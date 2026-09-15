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

import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
CI = REPO / ".github" / "workflows" / "ci.yml"

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


def main(argv: list[str]) -> int:
    check_only = "--check" in argv
    listing = "--list" in argv
    only = None
    if "--only" in argv:
        index = argv.index("--only")
        if index + 1 >= len(argv):
            print("error: --only needs a job name", file=sys.stderr)
            return 1
        only = argv[index + 1]

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

    # CI's workflow-level env, applied to every local job. `RUSTFLAGS: -D warnings` is the
    # one that matters and the one that was missing: every local cargo job ran under weaker
    # lints than CI, so a warning-level regression passed here and went red there.
    inherited = workflow_env()
    env = dict(os.environ)
    env.update(inherited)
    if inherited:
        applied = ", ".join(f"{k}={v}" for k, v in sorted(inherited.items()))
        print(f"\nci.yml env applied: {applied}")

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
