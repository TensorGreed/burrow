---
name: verify-colour-and-size-claims
description: How to actually verify burrow's hand-written colour maths and size-budget prose instead of reasoning about them
metadata:
    type: feedback
---

Verify burrow's quoted measurements by computing them, never by reading the prose.

**Why:** `apps/web` carries hand-written CIEDE2000/WCAG code and a `size-budget.json` whose
`why` fields are long prose. Both have shipped wrong numbers before, and the file's own
header says a `why` written against a pre-final measurement is a bug. Nothing in CI reads
the prose, so a review is the only gate on it.

**How to apply:**

- Node 24 on this machine strips TypeScript natively, so `import('./tokens-check.ts')` from a
  plain `.mjs` in the scratchpad works with no build step. Copy the module out, do not edit
  it in the repo.
- To check `deltaE2000`, rewrite its first two lines to take Lab directly and run **Sharma's
  34-pair CIE test set**. Verified 2026-09-17: all 34 match to <2e-4, including the hue-wrap
  and near-neutral discontinuity pairs (9–15). The implementation is correct; do not re-derive
  it by eye.
- For colour-vision claims, the Viénot–Brettel–Mollon 1999 LMS simulation reproduces the
  deuteranopia and greyscale figures the docs quote; its tritanopia figure runs ~2 ΔE from
  Brettel's, so treat a tritan mismatch of that size as model choice, not an error.
- For `size-budget.json`, diff every `measured_brotli` against `git show HEAD:...` with a
  script and check the new `why` paragraph's headline delta equals the sum of the artifact
  deltas it names. `src/size-budget.test.ts`'s drift line ("engines/... = exact") is the
  ground truth for whether a non-reproducible artifact actually moved.
- Mutation-test `src/styles/tokens.test.ts` by editing `tokens.css` / `base.css` in place,
  running `npx vitest run src/styles/tokens.test.ts`, and restoring from a scratchpad copy.
  Assert the mutation applied first; check `git diff --stat` afterwards to prove the restore.

See [[feedback-review-expectations]] (in the repo-root copy of this memory) for the wider
review posture.
