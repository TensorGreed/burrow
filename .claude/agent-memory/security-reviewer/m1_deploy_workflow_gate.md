---
name: m1-deploy-workflow-gate
description: Measured bypasses of tools/check-deploy-workflow.sh (quoted key, space-before-colon, top-level env secret before on:, tags:, publish-job gate deletion) and the credential-scope questions deploy.yml cannot answer from the tree
metadata:
  type: project
---

Measured 2026-09-15 on branch `m1-deploy`, by mutating copies of
`.github/workflows/deploy.yml` in a scratch dir and running `tools/check-deploy-workflow.sh`
against each. All five below exited 0 and printed
`OK -- the deploy workflow cannot be triggered from outside main`.

**Rule 1+2 (trigger allowlist) bypasses.** Both rules key on the exact byte pattern
`^  <name>:`. Anything YAML accepts that is not that pattern is invisible to *both*, and
rule 2's `present` list silently under-counts rather than refusing:
- `  "pull_request":` (quoted key) — PyYAML resolves triggers to
  `['workflow_dispatch','pull_request','push']`; the checker printed `triggers: push workflow_dispatch`.
- `  pull_request :` (space before the colon) — same result.
- `tags: ['**']` added under `push:` alongside `branches: [main]` — rule 3 only tests for the
  presence of `branches: [main]`, so a tag push deploys.

**Rule 4 (secrets confined to `publish`) bypass.** The awk sets `job` on `^  [a-z...]+:$`,
which first fires on `  workflow_dispatch:` inside `on:`, and skips any `secrets.` line seen
while `job == ""`. A top-level `env:` block placed ABOVE `on:` therefore puts the token in
every job's environment and the checker reports `secrets referenced only in: publish`.
Also unanticipated: `secrets[format('{0}','CLOUDFLARE_API_TOKEN')]` has no `secrets.` at all.

**Rule 5 (gate before upload) is line-order only, not per-job.** Deleting the `publish` job's
`Re-verify the payload that is actually being uploaded` step still passes: the `build` job's
copy at line 139 is < the deploy line. The round-trip claim in the workflow comment is
unenforced.

**Not checkable from the tree, and the whole boundary rests on it:** whether
`CLOUDFLARE_API_TOKEN` is an *environment* secret on `production` with deployment branches
restricted to `main`. If it is a repo secret, `ci.yml` (which has `pull_request:`) can read it
from a same-repo branch, and `workflow_dispatch` on any branch runs that branch's deploy.yml.
`check-deploy-workflow.sh` only reads deploy.yml — it cannot see a secret added to ci.yml.

**How to apply:** if this checker is touched, re-run the five mutations above; the durable fix
is to parse with `python3 -c 'import yaml'` (already a CI dependency) and compare the parsed
trigger set, rather than grepping. See [[m1-force-push-hook-and-deploy-gate]].
