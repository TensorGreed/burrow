---
name: m1-force-push-hook-and-deploy-gate
description: Measured gaps in .claude/hooks/refuse-force-push-to-main.sh (git -c / global-option and multi-line bypasses) and in tools/check-deployable-build.sh (page CSP not checked for extra origins; _headers stray rule uses expected as a regex)
metadata:
  type: project
---

Measured on branch `m1-deploy-origin`, 2026-09-15, by loading the hook as a module
(`SourceFileLoader`, the file is python despite its `.sh` name) and by running
`tools/check-deployable-build.sh` over a copy of `dist/` rewritten to a fake origin.

**Force-push hook.** `_verdict_for_one` strips git global options with
`rest = rest[2:] if rest[0] == "-C" else rest[1:]`, so every OTHER value-taking global
option (`-c k=v`, `--git-dir p`, `--work-tree p`, `--namespace`, `--exec-path p`) leaves
its value in `rest[0]`, the `rest[0] != "push"` test fails, and the command is allowed.
`git -c core.pager=cat push --force origin main` is allowed by the hook AND misses every
deny-list prefix (the string no longer starts with `git push`). Segment splitting is on
`&& || ; | &` only, so a NEWLINE-separated compound (`echo hi\ngit push -f origin main`)
becomes one segment whose `tokens[0]` is not `git`. Also allowed: `eval`/`bash -c`
wrappers, `$(...)`, `git push --force origin HEAD:heads/main` (verified against a real
bare repo: it updates `refs/heads/main`), `git push origin :main` (deletes main; deny list
covers only `--delete`), `git push --mirror`.

**Why:** the hook exists precisely because enumerating spellings failed, and it fails OPEN
by design with only the enumeration underneath.

**How to apply:** when this hook is next touched, re-run the probe list above; the
destination normaliser also needs `heads/` alongside `refs/heads/`.

**Deploy gate.** Per-page rule is `grep -qF "connect-src $expected/"` — presence only, so a
page whose in-markup CSP appends an extra `connect-src` origin passes and the gate still
prints "deployable to X and to nowhere else"; the `_headers` rule catches that case but
uses `grep -v "^$expected$"`, i.e. `$expected` as a REGEX, so a lookalike differing only
where the expected origin has a dot (`https://burrow-app` for `https://burrow.app`) reads
as the expected origin. Both reproduced. See [[m1-web-engine-path]].
