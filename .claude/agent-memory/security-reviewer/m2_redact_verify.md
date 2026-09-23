---
name: m2-redact-verify
description: "#134's read-back: the form-local /Resources leak measured against PDFium, the six mutations that survived 1004 tests, and what the deadline really costs."
metadata:
  type: project
---

# Redaction's own read-back (#134, commit bab54c3)

Reviewed 2026-09-23 in a throwaway worktree at `/tmp/sec-134`.

## The leak that is real, and it is not in #134's code

`pdfsyntax::geometry::walk` recurses into a Form XObject with **the caller's `resources`**
(`draw_form` → `walk(&form.content, resources, …)`). A form's own `/Resources` is never
consulted, so a form's `/F1` is resolved against the **page's** `/F1`.

**Why:** measured against the PDFium oracle. Page `/F1` = `/Widths` all 0 + `/FontBBox [0 0 0 0]`;
form `/F1` = width 556. PDFium places 12 glyphs over x 72→233; burrow collapses all 12 to a
zero-width box at x=72. A region over x 150→450 redacts nothing, `redact::page` returns `Ok`,
and PDFium still reads 7 characters inside the region afterwards. The report also said
`cut: true` for the **page's** font — font surgery edited the wrong object.

`redact_verify` cannot see it: both passes use the same walk (#111's residue, disclosed in
ADR 0029 §6 — but this is a *resolution* bug, not a placement inaccuracy, and PDFium
disagrees, so the §6 oracle would catch it if a fixture had form-local resources).

**How to apply:** any future redaction/geometry review should build the form-local-resources
fixture first; it is two objects and defeats the whole check.

## Mutations that survived the full native suite (1004 tests, 0 failed)

Six of sixteen. The author's own sweep reported 0/15.

- `glyphs_on` returning `Ok(Vec::new())` after walking — **check 1, the headline assertion, is
  inert end to end**. The direct witness tests pin `mapped_codes`, `drawn_codes`, `page_keys`;
  there is none for `glyphs_on`.
- `cut_fonts` forced empty in `qpdf/mod.rs`'s `observe` closure — the mapping check is inert
  on every committed document. Only the `Liar` unit tests exercise it, and they bypass the
  wiring that computes the set.
- `Cleared { page: 0 }` regardless of the redacted page — no end-to-end case redacts a page
  other than 0.
- point-origin instead of `conservative_box()` in check 1.
- `mapped_codes` scanning `0..=255` instead of `0..=65535` — no CID/2-byte-code fixture.
- witness carrying `Limits::default()` (both in `fresh` and in `open_output`).

## Cost, measured — and the attribution that surprised me

470 kB input, 4000 decoy font dicts sharing one `/ToUnicode`, `max_duration_ms = 100` →
**19.77 s** before the deadline could fire. But stubbing `region_is_cleared` to `Ok(())`
changed it by 0.4%: the cost is `redact_steps::cut_fonts`' per-font `narrow_font` loop
(pre-existing, #131), not the read-back. `MAX_KEYS = 4096` caps the `/Font` dict.

**Why:** I nearly reported this as #134's. Always stub the new code out and re-measure before
attributing a cost to it.

**How to apply:** the shape of the defect is one deadline checkpoint at the *entry* of a loop
over a file-controlled count. `mapped_codes`, `cut_fonts` and `drawn_codes` all have it.

Related: [[m2_qpdf_redact_steps]], [[m2_glyph_geometry]], [[spike_0006_redaction_survival]],
[[m1_verify_output]].
