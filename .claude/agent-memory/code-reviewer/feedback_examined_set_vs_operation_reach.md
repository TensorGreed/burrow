---
name: examined-set-vs-operation-reach
description: A read-back check's examined set must equal what the operation edits; too wide over-refuses, "exactly what the op says it touched" inherits the op's misresolution. Probe both.
metadata:
  type: feedback
---

**Too wide.** #218 round 1 (2026-09-28) examined every unshared font in any scope: annotation /AP,
form fonts nothing was cut from, every page's /ToUnicode. The operation narrows only page /Font
plus the fonts cut glyphs came from, and `drawn_codes` never walks /Annots. Four hand-built PDFs
that redacted Ok at the parent were refused. The golden corpus (463 outcomes, "nothing refused")
had none of those shapes. The whole-document survey also cost 16x per page at 1000 pages.

**Too narrow.** Round 2 examined "exactly the fonts the operation narrowed, by path", which
inherits the operation's resolution errors. Glyph in a NESTED form (page→Fm0→Fm1) plus a
same-named decoy in the page /Font: `font_in_scope` narrowed the decoy, the real font kept
`<53> <0053>` and `/S`, and verification returned Ok. This was already present at the base
commit; the round-1 design would have caught it.

**Why:** "fail-closed widening" reads as safer and passes the corpus. "Exactly what the operation
touched" reads as precise, but it is circular, so the check shares the operation's blind spots.

**How to apply:** build one fixture per scope the rule reaches but the operation does not edit,
plus a decoy name in the wrong scope, and run each at the parent and at HEAD. Then mutate
"refused, never skipped" in the REAL witness to a skip. A Liar test that re-implements the
refusal proves nothing (see [[probe-reimplements-rule]]).
