---
name: scope-set-wider-than-the-edit
description: When an operation takes a caller-supplied set of "pages this covers" but edits only one, read back the pages it did not edit — the control test that passes is the one doing the damage.
metadata:
  type: feedback
---

When a seam takes a caller-supplied scope set (`redacted: &BTreeSet<usize>`, "the pages this
operation covers") and the implementation behind it edits only **one** page, run the operation
with a set larger than what it edits and read the **other** pages back with the oracle. Do not
stop at the returned report.

**Why:** measured on `22ceffa` (M2 #131). `QpdfRedaction` redacts one page; `cut_fonts` trusts
`redacted` to decide a font is cuttable, and `codes_still_drawn` walks only `self.page`. The
project's own control test `redacting_every_page_leaves_nothing_to_disclose` calls
`redact_page(&pdf, 0, (0..4).collect(), region)` and asserts only on `Report`. Reading pages 1–3
back through PDFium showed each dropping from 4 drawn glyphs to 1 stacked at the same origin —
every `/Widths` entry zeroed on pages nobody redacted. Exactly the "corruption rather than
leakage" outcome ADR 0029 says the rule exists to prevent, produced by the test written to
prove the rule.

**How to apply:** for any `(target, scope_set)` pair, ask whether the implementation actually
covers `scope_set` or only `target`, and whether anything refuses when they disagree. Then
measure the pages outside `target`. See [[inert-detector-path]] for the sibling shape — a
report that says OK about work that did not happen.
