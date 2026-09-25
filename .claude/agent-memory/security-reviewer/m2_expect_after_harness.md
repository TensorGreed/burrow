---
name: m2-expect-after-harness
description: "#176's --after (ff9fa11): owed refused placements are never witnessed after, whole-string 'gone', a witness that exists for ch 6/21, and the ungated escape hatches."
metadata:
  type: project
---

# `check-redaction-corpus.py --after` (#176, commit ff9fa11), reviewed 2026-09-24

Baseline green: 90 of 90, 42 gone / 5 present / 26 refused / 15 owed / 2 unwitnessed.

**Why:** this is the independent after-state judge for redaction, so its blind spots are
where a leak can pass as "gone".

**How to apply:** re-check these whenever `after()` or the manifest's escape fields change.

- **`owed_by` on a `refused` placement: the witness never runs** when the document redacts
  (`examined += len(owed)`, no witness call). Measured: 12 of 13 still witness their canary or
  carrier in the Ok output, 4 read the canary itself (/AcroForm /V, struct elem). An OCR
  fixture with a missed region leaves `BURROW-SECRET-SCAN` in PDFium's text and --after says OK.
  The ADR amendment says "none was a leak"; they are known #125 leaks (redaction is held from
  production, `apps/web/src/production-build.test.ts` held-list).
- **"gone" is a whole-string match**, and nothing checks the region covers the canary. A
  +60 pt shift in `Glyph::conservative_box` left `T-WRITER` / `CRET-LATEX` in PDFium's text,
  the op's own read-back agreed, --after said 42 gone. The native `glyph_geometry`
  `a_real_redaction_removes_the_region_and_moves_nothing_else` DOES catch that uniform shift.
- **Channels 6 and 21 have a witness.** PDFium's text on ch06 is the raw GIDs
  (`\x01\x02\x03\x03…`); decoding the content-stream Identity-H codes through the inverse of the
  embedded font's format-4 cmap reads `BURROW-SECRET-06`/`-21` before, nothing after, nothing on
  the control. ~50 lines of Python. A region missing the canary there passes --after.
- **`after_unwitnessed = "x"`** on plain-tj's pdfium-text placement + a missed region: both
  halves exit 0 with the full canary in the output. No restriction on which witnesses may
  claim it; counts of owed/unwitnessed are printed, not gated.
- **Raw witnesses are blind to the rewriter's own encoding**: a touched run is re-emitted as
  hex of the single-byte codes (`<425552524f57…>`), and `spellings()` has only UTF-16 hex.
  Latent: no raw-witnessed fixture has a canary that is a proper sub-run.
- Refusals count by any `[rule]` (page-out-of-range included); `BURROW_AFTER_ONLY` is not
  cleared by the .sh; `keep_line` over-removal is unjudged.

Related: [[m2_redact_verify]], [[spike_0006_redaction_survival]], [[m2_glyph_geometry]].
