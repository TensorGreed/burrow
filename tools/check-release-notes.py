#!/usr/bin/env python3
"""Write a release's notes, and check that the PUBLISHED notes still carry what matters.

    python3 tools/check-release-notes.py --write notes.md --archive A --tag T
    python3 tools/check-release-notes.py --check published.md --archive A --tag T

# Why a signature needs a checkable instruction beside it

A signature nobody verifies is a checkbox. `cosign verify-blob` needs `--certificate-identity`
and `--certificate-oidc-issuer` to mean anything -- without them a reader is trusting whatever
certificate came in the file, which verifies that SOMEBODY signed it and nothing about who. Both
values are properties of this repository and this workflow, so the notes carry the command with
them already filled in: a reader pastes it, and it either passes or it does not.

The same function writes the notes and checks them, so there is one definition of the claim. The
check then runs against what GitHub actually PUBLISHED rather than the file that was passed in --
`gh release create` prints warnings and still exits 0, and CLAUDE.md records three measured cases
of a green exit over something that had not happened.

# What the signature claims, stated exactly, because the interesting part is the limit

**These bytes came from this repository at this commit, and this workflow produced them** --
that is what the certificate binds, and the Rekor entry records the repository, commit, workflow
file, ref and trigger. The release also carries a *deploy-time* attestation: the same run's
`publish` job diffed the live origin against these bytes after uploading them, so at that moment
the edge served this payload.

**It is NOT a continuing guarantee about what the edge serves now.** Cloudflare can be
reconfigured, a later deploy replaces the payload, and neither event touches this signature. A
reader who wants to know what is live today has to check what is live today;
`tools/check-live-routes.py` is that check, and it runs on every deploy rather than once.

This distinction is the whole reason the wording is generated rather than typed: an
over-broad sentence in a release note is a claim people rely on, and this repository treats an
overclaiming doc comment as a bug.
"""

from __future__ import annotations

import argparse
import contextlib
import io
import sys
from pathlib import Path

#: The repository the certificate identity names. Not derived from `git remote`: a fork's remote
#: would silently generate a command that verifies against the fork, and the value belongs to the
#: workflow that does the signing.
SLUG = "TensorGreed/burrow"

#: Keyless signing through GitHub's OIDC provider binds the certificate to the workflow FILE at a
#: ref. Both halves matter: the identity without the issuer would accept a certificate from any
#: provider willing to assert that string.
IDENTITY = f"https://github.com/{SLUG}/.github/workflows/deploy.yml@refs/heads/main"
ISSUER = "https://token.actions.githubusercontent.com"

#: The sentence a reader must not be able to lose. Checked as a substring of the published body.
CLAIM = (
    "These bytes came from this repository at this commit, and this workflow produced them. "
    "The release also carries a deploy-time attestation that the live origin served exactly "
    "these bytes at that moment -- excepting the platform configuration files (`_headers` and "
    "its siblings), which Cloudflare consumes rather than serves, so nothing serves them and "
    "nothing could compare them. It is NOT a continuing guarantee about what the edge serves "
    "now."
)


def verify_command(archive: str) -> str:
    """The exact invocation, with identity and issuer filled in."""
    return (
        f"cosign verify-blob {archive} \\\n"
        f"  --signature {archive}.sig \\\n"
        f"  --certificate {archive}.pem \\\n"
        f"  --certificate-identity {IDENTITY} \\\n"
        f"  --certificate-oidc-issuer {ISSUER}"
    )


def notes(archive: str, tag: str) -> str:
    """The whole body."""
    return f"""The web payload deployed at `{tag}`, as a tarball of exactly the bytes that were
uploaded to the live origin in that run. Nothing here was rebuilt for the release: the archive is
packed from the same `production-dist` artifact the deploy consumed.

## Verify it

```
{verify_command(archive)}
```

`--certificate-identity` and `--certificate-oidc-issuer` are not optional decoration. Without
them `cosign` checks that the bundled certificate signs the blob and tells you nothing about who
holds it, which any signer can satisfy.

Then check the archive against its digest:

```
sha256sum -c {archive}.sha256
```

## What this claims, and what it does not

{CLAIM}

Cloudflare can be reconfigured and a later deploy replaces the payload; neither touches this
signature. `tools/check-live-routes.py` is what answers "what is served right now", and it runs
on every deploy rather than once.

## Contents

- `{archive}` — the web payload, deterministically packed: sorted names, fixed owner, normalised
  modes, no gzip timestamp, and an mtime taken from the commit. So the archive is a function of
  the payload and this commit — not of the machine that packed it, which is the property a third
  party reconstructing it needs. The workflow packs it twice and compares, rather than asserting
  this.
- `{archive}.sha256`, `{archive}.sig`, `{archive}.pem`
- `burrow.cdx.json` — the CycloneDX SBOM for this commit: the Rust crates of the locked
  workspace and the native engine components, verified in CI against `engines/licenses.toml` and
  against symbol inspection of the shipped binaries.

  **Read what it covers before relying on it.** It does NOT list npm dependencies, and this
  archive is mostly JavaScript and HTML built from the pnpm tree — so the SBOM describes what
  went into the engines, not every input to these bytes. The symbol-inspection cross-check is an
  equality gate for *native* artifacts only; for the wasm engines in this payload, detection is
  weaker by construction (a wasm module's names are stripped) and divergence is reported rather
  than gated.
"""


#: Each rule: a name, the text it requires, and why losing it would matter. Gated on the SET, so
#: a rule added here without a probe below fails rather than passing quietly.
def rules(archive: str) -> list[tuple[str, str]]:
    return [
        ("the verify-blob invocation", verify_command(archive)),
        ("the certificate identity", IDENTITY),
        ("the OIDC issuer", ISSUER),
        ("the claim, and its limit", CLAIM),
        ("the checksum command", f"sha256sum -c {archive}.sha256"),
    ]


def tag_rule(tag: str) -> tuple[str, str]:
    """The body must name the tag it belongs to.

    `--tag` used to be required, documented, and then discarded: `check` never received it, so a
    body belonging to a DIFFERENT release passed the read-back. Making it a rule is also what
    makes the read-back assert that the release it read is the release it just created.
    """
    return ("the tag this body belongs to", f"deployed at `{tag}`")


def check(body: str, archive: str, tag: str | None = None) -> int:
    applied = rules(archive) + ([tag_rule(tag)] if tag else [])
    missing = [name for name, text in applied if text not in body]
    print(f"check-release-notes: {len(applied)} rule(s) checked against the published body")
    if missing:
        print(
            f"\nFAILED -- the published notes are missing {len(missing)} of them:", file=sys.stderr
        )
        for name in missing:
            print(f"  - {name}", file=sys.stderr)
        print(
            "\nThe notes are generated by this file. If the body was edited after publishing, "
            "restore it; if the wording changed here, the release predates the change.",
            file=sys.stderr,
        )
        return 1
    print("OK -- the published notes carry the verification command and the stated claim.")
    return 0


def probe(quiet: bool = False) -> int:
    """Every rule must reject a body with that rule removed, and accept the full one.

    A checker whose rules all match everything passes everything. This DOES now run on every
    invocation, before the real body is looked at, so the count printed above is a measurement --
    the sentence claiming so was here first and was false: `main()` called this only under
    `--probe`, so the post-publish read-back ran unprobed rules. It costs microseconds.
    """
    archive = "burrow-web-deploy-2026-01-01-abc1234.tar.gz"
    sample_tag = "deploy-2026-01-01-abc1234"
    full = notes(archive, sample_tag)
    problems: list[str] = []

    def quietly(body: str) -> int:
        """`check` with its own reporting suppressed -- the probe's failures are expected."""
        out, err = io.StringIO(), io.StringIO()
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            return check(body, archive, sample_tag)

    # THE POSITIVE CONTROL, FIRST. If the generated notes do not pass their own rules, every
    # refusal below is the checker being broken rather than the rule biting.
    if quietly(full) != 0:
        problems.append("the generated notes do not pass their own rules")

    for name, text in rules(archive) + [tag_rule(sample_tag)]:
        if text not in full:
            problems.append(f"{name}: the generated notes do not contain it; the rule is inert")
            continue
        if quietly(full.replace(text, "", 1)) == 0:
            problems.append(f"{name}: a body with it removed still passed; the rule is inert")

    if not quiet:
        print(f"check-release-notes: {len(rules(archive)) + 1} rule(s) probed against the full "
              f"notes and against the notes with that rule removed")
    if problems:
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 1
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description="Write or check a release's notes.")
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--write", metavar="PATH", help="write the notes to PATH")
    group.add_argument("--check", metavar="PATH", help="check the published body at PATH")
    group.add_argument("--probe", action="store_true", help="run the rule probes and exit")
    group.add_argument(
        "--list-rules",
        action="store_true",
        help="print each rule's name, one per line, so a test can derive its own expectation",
    )
    group.add_argument(
        "--verify-command",
        action="store_true",
        help="print the exact cosign invocation the notes publish, for the job to run",
    )
    parser.add_argument("--archive", help="the archive's file name")
    parser.add_argument("--tag", help="the release tag")
    args = parser.parse_args()

    # THE PROBES, IN EVERY MODE. A rule that matches everything passes everything, and the
    # read-back after a release is exactly where that would go unnoticed.
    status = probe(quiet=not args.probe)
    if status != 0 or args.probe:
        return status

    if args.list_rules:
        # SO A NEW RULE CANNOT ARRIVE WITHOUT A CASE. `tools/test-check-release-notes.sh` reads
        # this rather than carrying a hardcoded list: a review added a sixth rule to a scratch
        # copy and the suite passed 8 of 8, because the count it gated on was a constant.
        for name, _ in rules("ARCHIVE") + [tag_rule("TAG")]:
            print(name)
        return 0

    if args.verify_command:
        if not args.archive:
            parser.error("--archive is required")
        # ONE DEFINITION. The job runs what the notes publish, character for character; a second
        # copy of this command in the workflow is the copy that would rot.
        print(verify_command(args.archive))
        return 0

    if not args.archive or not args.tag:
        parser.error("--archive and --tag are required")

    if args.write:
        Path(args.write).write_text(notes(args.archive, args.tag), encoding="utf-8")
        print(f"wrote {args.write} for {args.tag}")
        return 0

    return check(Path(args.check).read_text(encoding="utf-8"), args.archive, args.tag)


if __name__ == "__main__":
    raise SystemExit(main())
