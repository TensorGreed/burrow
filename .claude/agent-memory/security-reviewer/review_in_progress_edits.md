---
name: review-in-progress-edits
description: The user edits reviewed files live during a review and often works uncommitted, so re-read before concluding
metadata:
  type: feedback
---

Re-read the files you are reviewing immediately before writing findings, and check
`git status` rather than assuming the work is committed.

**Why:** In the M1 PR 1 review (2026-09-10) the branch `feat/engine-acquisition` was at the
same commit as `main` — the entire change was uncommitted in the working tree, so
`git diff main...HEAD` returned nothing. During that same review the user rewrote
`core/burrow-engines/build.rs` twice while I was reading it, in one case adding a fix for an
issue I was mid-way through documenting. Reporting against the stale version would have
wasted the review.

**How to apply:** Start with `git status --short` plus reading the working-tree files, not
the diff, when a branch looks empty. Before writing up, re-read any file you flagged; if a
finding was already fixed, review the fix instead — the fix is where the new bug is (the
`$ORIGIN` rpath fix introduced a length-only integrity check).
