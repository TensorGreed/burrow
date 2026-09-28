---
name: all-went-red-includes-near-miss
description: A doc claim "with the rule neutered, all N self-test cases went red" is suspect when one of the N is a near-miss that must pass; re-run the neutering.
metadata:
  type: feedback
---

When a self-test has positive cases plus a near-miss that must PASS, neutering the rule should
turn the positives red and leave the near-miss green. A doc or commit claiming "all four went
red" is either wrong or the neutering also broke the near-miss's path (skip handling).
Measured on #137's ledger self-test (2026-09-28): neutering the ledger's verdict gave 3 red,
near-miss green; ADR 0029 and the commit said all four.

**Why:** these "shown to fail" sentences get cited as evidence later; a miscounted one
overclaims what the ablation proved (see [[review-expectations]]).

**How to apply:** for any "N went red with X neutered" claim, plant the obvious neutering in
the worktree, run the self-test, and compare per case. Also check `grep -c` count guards
under `set -e`: a zero count exits before the guard's message prints.
