---
name: whole-string-witness-partial-removal
description: Review technique — a "gone" judged by a whole-canary substring witness cannot see partial removal; shrink the redaction region and watch the gate stay green
metadata:
  type: feedback
---

A check that scores a redacted canary "gone" because the **whole string** is no longer found
(`canary in text`, raw substring, UTF-16 hex) passes any redaction that removes a single glyph
of it. To test this, shrink the region the check feeds the operation, then re-run the check.

**Why:** measured on #176 (ff9fa11). `check-redaction-corpus.py --after` stayed green, with
identical counts (42 gone), after `default_region()`'s width was multiplied by 0.3. A 40 pt
producer-writer region left `W-SECRET-WRITER` on the page and was still scored gone. On the
real region, 8 hand-built evasion fixtures already had canary tails such as `ARMISS` and
`OPARENT` past `pdfbuild.REGION`'s x=370, which disproves the checker's "drawn inside REGION by
construction" claim.

**How to apply:** for any after-state checker, (1) run the region-shrink mutation, and (2) grep
the output's pdfium text for 4-grams of the canary's distinctive tail. For each escape-hatch
marker (`owed_by`, `after_unwitnessed`), also run the witness it skips and report what it sees.
Here 13 of 14 owed-refused placements still witnessed their canary or carrier. Related:
[[feedback-inert-detector-path]], [[mutate-the-wiring-not-the-policy]].
