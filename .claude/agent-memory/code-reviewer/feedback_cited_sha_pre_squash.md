---
name: cited-sha-pre-squash
description: A doc or ADR citing a commit SHA — check it is an ancestor of origin/main; branch SHAs die at squash-merge (#218 cited 1a0f6a7, main has 2178c9b)
metadata:
  type: feedback
---

When a doc, ADR or comment cites a commit by SHA, run `git merge-base --is-ancestor <sha> origin/main`.
PRs squash-merge, so a SHA from a PR branch is unreachable once the branch is deleted; cite the
squash commit or the PR number instead. Check `origin/main`, not local `main` — local main was stale
in the #218 review and made the right SHA look missing too.

**Why:** #218's b3e2423 fixed a dangling "note above" by quoting the note "with its commit" — and
cited `1a0f6a7`, the pre-squash branch commit, while the note landed on main as `2178c9b` (#222).
The fix for one dangling reference was another.

**How to apply:** any review where a touched doc gains a SHA. Related: [[removed-note-leaves-dangling-refs]].
Also in that round: a `!=` coverage gate's `>` half was unwitnessed (`<` mutation stayed green) —
for an equality gate, mutate to each one-sided comparison.
