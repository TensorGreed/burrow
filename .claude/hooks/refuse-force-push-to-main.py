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

    a `git push` that force-pushes OR deletes, whose destination ref is `main` --- or which
    names no refspec at all, because then it acts on the CURRENT branch, which may be `main`.
    `--mirror` counts unconditionally: it force-updates and deletes every ref on the remote.

WHAT THIS CANNOT REACH, said plainly rather than left for somebody to find. A hook is handed
a command STRING; it does not evaluate a shell. So `eval "git push -f origin main"`,
`bash -c '...'`, `$(...)` and backticks are all outside it, and no amount of parsing changes
that. An earlier version of this docstring said "in any spelling", which overclaimed: it is
any spelling that NAMES A GIT PUSH DIRECTLY. This is a guard against the accidental
spelling, which is the one that actually happens.

Exit codes. 2 blocks the call and sends stderr back to the model; anything else allows it.
On any internal error this ALLOWS, because a hook that fails closed on its own bug would
block every push in the repository for a reason nobody could see. The deny list is still
underneath.

Self-test: .claude/hooks/test-refuse-force-push-to-main.sh, which runs every row of the
probe table below plus every case that must be ALLOWED.

`.py`, NOT `.sh`. It is Python, and the extension is what `tools/check-python-syntax.py`
globs -- a Python file behind a `.sh` name matches neither that glob nor its
`git ls-files '*.py'` cross-check, so the SyntaxWarning-becomes-SyntaxError gate built after
M1 PR 4c did not cover it. Raised by code review; one line in `.claude/settings.json`.
"""

import json
import re
import shlex
import sys

# The branch this protects. One name, one place.
PROTECTED = "main"

FORCE_FLAGS = ("--force", "-f", "--force-with-lease", "--force-if-includes")

# `--delete` removes the remote branch. Not a force-push, and the same loss: the deny list
# already carried the flag spelling, which is what says this was always meant to be in scope.
DELETE_FLAGS = ("--delete", "-d")


def destination(refspec: str) -> str:
    """The ref a refspec writes TO, normalised.

    `src:dst` writes to `dst`; a bare `name` writes to `name`; a leading `+` is the
    refspec spelling of `--force` and is stripped here so the two cannot be told apart
    by accident. `refs/heads/main` and `main` are the same destination.
    """
    spec = refspec.removeprefix("+")
    _, _, dst = spec.rpartition(":")
    dst = dst or spec
    # BOTH PREFIXES. `refs/heads/main` was stripped from the first version and `heads/main`
    # was not -- and git's own DWIM resolves `HEAD:heads/main` to `refs/heads/main`, verified
    # against a real bare remote by a security review. Longest first, so the short strip
    # cannot eat half of the long form.
    for prefix in ("refs/heads/", "heads/"):
        if dst.startswith(prefix):
            return dst.removeprefix(prefix)
    return dst


def verdict(command: str) -> str | None:
    """The reason to refuse, or None to allow."""
    # A NEWLINE IS A COMMAND SEPARATOR, and `shlex` treats it as plain whitespace. So a
    # multi-line command -- the ordinary shape for anything non-trivial -- collapsed into one
    # segment whose first token was the FIRST command's name, and `echo hi\ngit push --force
    # origin main` was allowed. Found by security review, reproduced.
    #
    # A LINE CONTINUATION IS NOT a separator, and splitting on newlines alone would be worse
    # than not splitting: `git push \<newline> --force origin main` would become a harmless
    # `git push` and an orphan ` --force origin main`, and the force flag would vanish. So
    # continuations are joined FIRST and only the remaining newlines become separators.
    #
    # The substitution happens before lexing, so a newline inside a quoted argument becomes
    # literal text in that token rather than a separator -- `shlex` respects the quoting. That
    # is the harmless direction.
    command = command.replace("\\\n", " ").replace("\n", " ; ")

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
    if len(tokens) < 2:
        return None
    # `/usr/bin/git` is still git. The basename, not the whole path.
    if tokens[0].rsplit("/", 1)[-1] != "git":
        return None

    # THE FIRST `push` TOKEN, NOT "skip the options and see what is left".
    #
    # The first version walked the global options positionally, consuming two tokens for `-C`
    # and one for everything else -- and the comment claiming `-C` was the only global taking
    # a separate value was simply wrong. `-c`, `--git-dir`, `--work-tree`, `--namespace`,
    # `--exec-path` and `--config-env` all take one, so one `rest[1:]` left the VALUE at the
    # front, the loop stopped on it, and `git -c core.pager=cat push --force origin main`
    # walked past this hook. It walked past the deny list too, because every entry there is a
    # prefix of the literal `git push ...`, so BOTH layers of a control documented as
    # "layered rather than duplicated" were open on the same input. Found by security review,
    # reproduced, and the shape is one a model has ordinary reasons to type.
    #
    # Looking for the token instead removes the need to enumerate anything. The failure
    # direction also changes, which is the point: a command that merely CONTAINS `push`
    # somewhere is examined rather than skipped, and examining it costs nothing because a
    # refusal still needs a force flag AND a `main` destination.
    if "push" not in tokens[1:]:
        return None
    args = tokens[tokens.index("push", 1) + 1 :]

    forced = False
    deleting = False
    mirroring = False
    refspecs: list[str] = []
    for arg in args:
        base = arg.split("=", 1)[0]
        if base in FORCE_FLAGS:
            forced = True
        elif base in DELETE_FLAGS:
            deleting = True
        elif base == "--mirror":
            mirroring = True
        elif arg.startswith("+"):
            # `+ref` is the refspec spelling of --force. It is both a force flag and a
            # refspec, so it counts as each.
            forced = True
            refspecs.append(arg)
        elif arg.startswith("-"):
            continue  # some other option, e.g. --set-upstream, -u, --tags
        else:
            refspecs.append(arg)

    # A REFSPEC WITH AN EMPTY SOURCE IS A DELETION. `git push origin :main` removes the
    # branch and carries no force flag at all, so the first version of this hook allowed it --
    # and the deny list had only the `--delete` spelling. Verified against a real remote by a
    # security review: `- [deleted] main`. The harm is the same as a force-push or worse.
    if any(spec.startswith(":") for spec in refspecs):
        deleting = True

    # `--mirror` FORCE-UPDATES AND DELETES EVERY REF on the remote, `main` included, and names
    # no refspec to notice. It is unconditional.
    if mirroring:
        return (
            f"Refused: `git push --mirror` force-updates and DELETES every ref on the remote, "
            f"`{PROTECTED}` included. There is no refspec to narrow it. If this is genuinely "
            f"intended, a person has to do it, not the model."
        )

    if not (forced or deleting):
        return None

    verb = "force-pushes to" if forced else "deletes"
    # The FIRST positional after `push` is the remote; the rest are refspecs. A push with no
    # positional at all, or with only a remote, names no refspec.
    named = refspecs[1:] if refspecs else []

    if not named:
        return (
            f"Refused: `git push` with {' '.join(a for a in args if a.startswith(('-', '+'))) or 'a force flag'} "
            f"and no refspec acts on the CURRENT branch, which may be `{PROTECTED}`. "
            f"Name the branch explicitly: `git push --force-with-lease origin <branch>`."
        )

    # `HEAD` AND `@` NAME THE CURRENT BRANCH, which is the same hazard as naming no refspec at
    # all -- and the hook refused the bare form while allowing `git push --force origin HEAD`.
    # Found by code review. `@` is git's own shorthand for `HEAD`. Only as a whole refspec:
    # `HEAD:feature` has an explicit destination and is somebody's ordinary work.
    for refspec in named:
        if refspec.removeprefix("+") in ("HEAD", "@"):
            return (
                f"Refused: `{refspec}` names the CURRENT branch, which may be `{PROTECTED}`, "
                f"and this {verb} it. Name the branch explicitly: "
                f"`git push --force-with-lease origin <branch>`."
            )

    for refspec in named:
        if destination(refspec) == PROTECTED:
            return (
                f"Refused: this {verb} `{PROTECTED}` (via `{refspec}`). "
                f"Rewriting or removing published history on `{PROTECTED}` is not recoverable "
                f"for anyone who has already pulled it. If this is genuinely intended, a "
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
