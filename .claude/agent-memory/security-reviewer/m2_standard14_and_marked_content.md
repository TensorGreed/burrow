---
name: m2-standard14-and-marked-content
description: Measured on 16e4c56 — the /ActualText refusal is evaded by moving the covered glyphs into a Form XObject, the DISPUTED width exclusions miss every alias name, and a 161-byte ToUnicode stream reaches 2.5 GB.
metadata:
  type: project
---

Review of `16e4c56` (m2/standard-14-metrics). Everything below was **run**, not reasoned about,
in a throwaway worktree with `engines/vendor` symlinked from the main repo.

## `check_marked_content` is per-stream, and marked content is not

`geometry.rs::check_marked_content` filters `glyph.source.form == stream`, so a page-level call
sees only page glyphs and a form-level call only that form's. A `BDC` carrying `/ActualText` in the
**page** stream that wraps a `Do` whose glyphs are the removed ones therefore passes both calls.

**Why:** the marked-content stack does not descend through `Do`, but PDF's marked content does — a
span opened on the page stays open across the XObject it draws.

**How to apply:** measured end to end. Fixture: page = `/Span << /ActualText (BURROW-CARRIER-09) >>
BDC  /X1 Do  BT (.) Tj ET  EMC`, form draws the secret. burrow redacts happily and PDFium reads
`BURROW-CARRIER-09` off the **output**; poppler agrees. The kept `.` inside the span matters — with
an empty span no extractor renders the string, but the bytes survive either way. The fix has to
carry the open-span state into the form pass (or refuse when a removed glyph's form is drawn inside
a carrying span). Related: [[m2_qpdf_redact_steps]], [[spike_0006_redaction_survival]].

Evasions that are **closed**: `#`-escaped names (`/Actual#54ext` — the lexer decodes before the
check), `/ActualText` nested in a sub-dict or array, an inner span or `BMC` left open, `/Alt`, a
`BDC` inside the form itself. Interior NULs in names fail closed at `Name::from_stripped`.

## `DISPUTED` is keyed on the literal /BaseFont string

`standard14.rs`'s four disputed `(font, code)` pairs name only canonical spellings, but `family()`
also accepts `Arial*`, `Helvetica-Italic` and `TimesNewRoman*`. Measured against PDFium with the
calibration's own instrument: **10 disagreements** across `Arial-Bold`, `Arial-Italic`,
`Arial-BoldItalic`, `Helvetica-Italic` — the same 4 glyph shapes, reached by an alias. The
calibration's `FONTS` list is the 12 canonical names, so no alias is measured at all. Consequence
is silent glyph drift (975 vs 1072 at code 64), not a crash.

## The ToUnicode duplicate list multiplies an unbounded quantity

`MAX_DESTINATIONS_PER_CODE = 8` bounds destinations per code and nothing bounds a destination's
**length**. Measured: a 161-byte Flate stream (four `<0000> <FFFF> <dst>` ranges, `dst` = 5,000
UTF-16 units) → 80 kB program → **2,525 MB peak RSS** in `ToUnicode::parse`, and `narrowed()` emits
**5.2 GB** in 52 s. Through a real 1,038-byte PDF, `redact::page` with `Limits::default()`
(`max_memory_bytes` = 1 GiB) returned **Ok** at 2,525 MB — the post-hoc detection did not fire.
Keeping every duplicate is a 4x worsening of a class that pre-dates the commit (1 repeat = 640 MB).

## Mutation sweep: 21 planted, 21 applied, 8 survived `cargo test -p burrow-engines --all-features`

Survivors worth remembering: the per-form `check_marked_content` call can be deleted with no test
failing; `open.contains(Text)` → `open.last() == Text` survives because every probe closes the inner
span before the glyph; `holds_text_key`'s recursion, `BMC` pushing, and the `_ => Carried::Unknown`
arm are all untested; raising `MAX_DESTINATIONS_PER_CODE` to 4096 survives; the `/Encoding`
dictionary → `WinAnsi` branch is untested (the calibration only uses the name form).

**Corpus note:** the hand-built corpus is generated, not committed — run both
`tools/make-redaction-fixtures.py` and `tools/make-evasion-fixtures.py` into
`tests/redaction/generated` (24 + 15) or `geometry_calibration` and `redaction_corpus` fail for the
wrong reason.
