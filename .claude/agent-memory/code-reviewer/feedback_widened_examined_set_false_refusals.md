---
name: widened-examined-set-false-refusals
description: When a read-back check widens what it examines (e.g. "every unshared font the page reaches"), probe scopes the operation never edits; the golden "nothing refused" missed them.
metadata:
  type: feedback
---

When a verification stops trusting the operation's list and instead examines everything a
wider rule selects, diff that rule's reach against what the operation actually edits and what
the check's other half (e.g. `drawn_codes`) counts. #218 (2026-09-28): the check examined every
unshared font in any scope (annotation /AP, untouched form-local fonts, all pages' /ToUnicode)
while the operation narrowed only page /Font + fonts with cut glyphs, and `drawn_codes` never
walks /Annots. Four hand-built PDFs that redacted Ok at the parent were refused at HEAD; the
golden corpus (463 outcomes, "nothing refused") had none of those shapes.

**Why:** fail-closed widening reads as "safer" and passes the corpus, but refuses ordinary
documents (single-page AcroForm with /Helv + PDFDocEncoding /Differences).

**How to apply:** build one fixture per scope the wider rule reaches but the operation does not
edit, plus a malformed part on a page outside the operation; run at parent and HEAD. Also time it
at 1000 pages: a whole-document survey per page-call is quadratic across a multi-page redaction.
Related: [[memoised-walk-undercounts]], [[rewriter-reach-vs-detector-reach]].
