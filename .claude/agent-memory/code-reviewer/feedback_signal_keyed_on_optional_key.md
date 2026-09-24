---
name: signal-keyed-on-optional-key
description: Review technique — a refusal whose signal is a dictionary's /Type (OCG/OCMD) is evaded by omitting it; PDFium defaults a missing /Type to OCG. Measure by rendering, not reasoning.
metadata:
  type: feedback
---

When a detector recognises a hostile construct by a key the spec calls *required* (`/Type /OCG`),
check what the **reader** does when the key is absent. PDFium's `CheckOCGDictVisible` reads
`GetNameFor("Type", "OCG")`, so an untyped `/Properties` entry listed in `/OCProperties /D /OFF`
**is a hidden layer**, and burrow's `is_optional_content_group` says it is not.

**Why:** measured on 92f11ea (#166). A full-page fill under `/OC /L0 BDC` rendered 0 of 800 dark
pixels for both a typed and an untyped OCG, 800 of 800 for `/Type /Foo`, and 800 with no layer.
The same untyped page went through `redact_page` with no `optional-content` refusal. `prune`'s
split-side check has the same signal.

**How to apply:** for any new refusal keyed on a type or subtype name, build the untyped twin
and render it through `PageRenderer::render` in a temporary `pdfium/tests.rs` test (the `open` and
`render_at` helpers are already there). Prefer a signal the reader itself keys on — for optional
content that is the `/OC` BDC tag or membership in `/OCProperties /OCGs`.

Also: a subagent's scratchpad is **shared** with concurrent reviewers of the same session. Give
every log and script a unique prefix (`cr-…`) — a generic `mut.log` was interleaved by another
reviewer's sweep. Related: [[inert-detector-path]], [[rewriter-reach-vs-detector-reach]].
