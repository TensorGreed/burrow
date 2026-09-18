---
name: feedback-effective-permissions-blind-spot
description: Review technique — any burrow check that reads GitHub Actions job `permissions` must be mutated with a WORKFLOW-level grant, because jobs without their own block inherit it and the rule reads as satisfied
metadata:
  type: feedback
---

When a check asserts something about a job's `permissions` (e.g. "`id-token: write` is never in
the job holding the Cloudflare credential"), mutate the **workflow-level** `permissions:` block,
not the job's, and re-run.

**Why:** measured on `tools/check-deploy-workflow.py`'s
`check_token_and_credential_are_separated` (2026-09-18). It collected holders from
`jobs[*].permissions` only. `publish` has no `permissions:` block of its own, so it inherits the
workflow-level map — adding `id-token: write` at workflow level put the OIDC token and the
Cloudflare credential in the same job while the tool printed ``id-token: write` in ['sign'],
never in `publish`` and exited 0. The check states a security property it structurally cannot
see the main violation of.

**How to apply:** compute *effective* permissions (job block if present, else workflow block)
before believing any such rule, and check the empty case too — no job holding the permission at
all should be a refusal when the job that needs it exists, not a `['<no job>']` note. Same family
as [[feedback-gate-keyed-on-hand-edited-id]] and [[feedback-inert-detector-path]].
