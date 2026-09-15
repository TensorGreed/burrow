#!/usr/bin/env python3
"""PreToolUse hook: refuse any force-push that lands on `main`, in any spelling.

WHY THIS EXISTS AND THE DENY LIST DOES NOT SUFFICE.

`permissions.deny` prefix-matches the command STRING. That can only ever be an
enumeration of spellings, and a security review of M1 #80 found four the list missed:

    git push --force origin HEAD:main
    git push --force origin refs/heads/main
    git push origin main --force          # the flag after the refspec
    git push --force fork main            # any remote not named origin/upstream

Widening the list closes those four and leaves the shape intact -- the next spelling
nobody thought of is still allowed. The RULE is a property of the command, not of how it
was typed, so it belongs somewhere that can evaluate a property. That is here.

The deny list is kept anyway, and deliberately: it refuses before this hook runs and
before the model sees a prompt, so the two are layered rather than duplicated. This hook
is what makes the pair complete rather than merely long.

THE PROPERTY, stated once:

    a `git push` that force-pushes, whose destination ref is `main` --- or which names no
    refspec at all, because then it pushes the CURRENT branch, which may be `main`.

Exit codes. 2 blocks the call and sends stderr back to the model; anything else allows it.
On any internal error this ALLOWS, because a hook that fails closed on its own bug would
block every push in the repository for a reason nobody could see. The deny list is still
underneath.

Self-test: .claude/hooks/test-refuse-force-push-to-main.sh, which runs every row of the
probe table below plus every case that must be ALLOWED.
"""

import json
import re
import shlex
import sys

# The branch this protects. One name, one place.
PROTECTED = "main"

FORCE_FLAGS = ("--force", "-f", "--force-with-lease", "--force-if-includes")


def destination(refspec: str) -> str:
    """The ref a refspec writes TO, normalised.

    `src:dst` writes to `dst`; a bare `name` writes to `name`; a leading `+` is the
    refspec spelling of `--force` and is stripped here so the two cannot be told apart
    by accident. `refs/heads/main` and `main` are the same destination.
    """
    spec = refspec.removeprefix("+")
    _, _, dst = spec.rpartition(":")
    dst = dst or spec
    return dst.removeprefix("refs/heads/")


def verdict(command: str) -> str | None:
    """The reason to refuse, or None to allow."""
    # `punctuation_chars=True`, NOT `shlex.split`. Plain `shlex.split` leaves `hi;` as a
    # single token, so `echo hi; git push -f origin main` never shows a separator and the
    # segment loop below sees one segment that does not start with `git`. Measured by this
    # hook's own self-test on its first run -- and it is the second time that exact
    # difference has cost something here: `tools/ci-local.py`'s preflight lost an `rm` the
    # same way.
    try:
        lexer = shlex.shlex(command, posix=True, punctuation_chars=True)
        lexer.whitespace_split = True
        tokens = list(lexer)
    except ValueError:
        # An unparseable command line. Not ours to judge; the deny list still applies.
        return None

    # A COMPOUND COMMAND IS EVERY COMMAND IN IT. `x && git push --force` is a force push,
    # and a hook that only read the first word would be a hook somebody routes around by
    # typing `true &&` in front of it.
    segments: list[list[str]] = [[]]
    for token in tokens:
        if token in ("&&", "||", ";", "|", "&"):
            segments.append([])
        else:
            segments[-1].append(token)

    for segment in segments:
        reason = _verdict_for_one(segment)
        if reason:
            return reason
    return None


def _verdict_for_one(tokens: list[str]) -> str | None:
    if len(tokens) < 2 or tokens[0] != "git":
        return None
    # `git -C path push …`, `git --no-pager push …`
    rest = tokens[1:]
    while rest and rest[0].startswith("-"):
        # `-C <path>` takes a value; the bare long options do not.
        rest = rest[2:] if rest[0] == "-C" else rest[1:]
    if not rest or rest[0] != "push":
        return None
    args = rest[1:]

    forced = False
    refspecs: list[str] = []
    for arg in args:
        base = arg.split("=", 1)[0]
        if base in FORCE_FLAGS:
            forced = True
        elif arg.startswith("+"):
            # `+ref` is the refspec spelling of --force. It is both a force flag and a
            # refspec, so it counts as each.
            forced = True
            refspecs.append(arg)
        elif arg.startswith("-"):
            continue  # some other option, e.g. --set-upstream, -u, --tags
        else:
            refspecs.append(arg)

    if not forced:
        return None

    # The FIRST positional after `push` is the remote; the rest are refspecs. A push with
    # no positional at all, or with only a remote, names no refspec.
    named = refspecs[1:] if refspecs else []

    if not named:
        return (
            f"Refused: `git push` with {' '.join(a for a in args if a.startswith(('-', '+'))) or 'a force flag'} "
            f"and no refspec pushes the CURRENT branch, which may be `{PROTECTED}`. "
            f"Name the branch explicitly: `git push --force-with-lease origin <branch>`."
        )

    for refspec in named:
        if destination(refspec) == PROTECTED:
            return (
                f"Refused: this force-pushes to `{PROTECTED}` (via `{refspec}`). "
                f"Rewriting published history on `{PROTECTED}` is not recoverable for "
                f"anyone who has already pulled it. If this is genuinely intended, a "
                f"person has to do it, not the model."
            )
    return None


def main() -> int:
    try:
        payload = json.load(sys.stdin)
    except (json.JSONDecodeError, ValueError):
        return 0
    if payload.get("tool_name") != "Bash":
        return 0
    command = payload.get("tool_input", {}).get("command", "")
    if not isinstance(command, str):
        return 0

    reason = verdict(command)
    if reason:
        print(reason, file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except Exception as error:  # noqa: BLE001 -- see the module docstring
        print(f"refuse-force-push-to-main: allowing, internal error: {error}", file=sys.stderr)
        sys.exit(0)
