---
name: m2-glyph-geometry
description: Measured facts about core/burrow-engines/src/pdfsyntax/geometry.rs (#129) — operand indexing vs PDFium, the form-DAG cost curve, what the two source probes cannot see, and which content the walk never reaches.
metadata:
  type: project
---

Reviewed at `m2/129-glyph-geometry` HEAD `8e40c5c` on 2026-09-21. The module was not yet
wired to any production caller — `glyphs_in` and `writing_mode_of` had no callers outside
their own tests — so everything below is a defect in infrastructure, not a live leak yet.

**Why:** the module decides where each glyph is, which decides what a redaction removes; a
box computed in the wrong place is a miss, and a miss is the leak.

**How to apply:** re-check each of these before the module is wired into `redact`.

## Measured, with the repro shape

- **Operand indexing counts from the front; PDFium counts from the back.** `number(at)`
  reads `operands[0..5]`. PDFium takes the *last* N. Measured: body
  `/F1 12 Tf 0 0 0 0 0 0 1 0 0 1 100 700 Tm (SECRET) Tj` — PDFium origin `(100, 700)` with a
  full loose box; burrow origin `(0, 0)`, conservative box `0,0,0,0`, intersects a region at
  `95..200 x 695..720` = **false**. Same class for `cm`, `Td`, `TD`, `"`, and for `Tf`'s
  font name via `operands.first()`. Not fixed by the `NumericOperandNotANumber` work — every
  operand there is a valid number.
- **Form DAG cost is branch^depth, and nothing bounds it.** `open_forms` stops cycles and
  `MAX_FORM_DEPTH = 16` stops depth; neither stops repetition. 16 forms, each drawing the
  next **three** times = **405 bytes** of content stream → 14,348,907 glyphs, 21.5 M
  `Resources::form` calls, **1.64 GiB peak RSS in 9.7 s** (release). Branch 4 extrapolates to
  ~1e9 glyphs. `Limits` is not threaded into the module at all.
- **`usecmap` losing to a declared `WMode` is silent.** `program_writing_mode` ends
  `declared.or(inherited)`. `/WMode 0 def /UniJIS-UCS2-V usecmap` → `Ok(Horizontal)`, while
  the *dictionary*-vs-program form of the same contradiction is refused. Two `usecmap`s with
  different modes: last wins, no refusal.
- **Content the walk never reaches, and returns `Ok` over.** A page whose only text is inside
  a **tiling pattern** content stream: `glyphs_in("/Pattern cs /P1 scn 0 0 600 300 re f")` =
  `Ok(0)`, PDFium renders **740 dark pixels** of "SECRET", and PDFium's *text layer* reports
  **0 chars** — so the ADR 0029 §6 read-back would be blind to it too. `gs` with an
  ExtGState `/Font` is the same shape.
- **Non-finite arithmetic makes the box the inverted-empty rect.** Two `cm` of 1e300 →
  origin `(NaN, NaN)`, `conservative_box()` = `{left: inf, right: -inf, …}` because
  `f64::min(inf, NaN) == inf`; `intersects` is then false against everything. Could not
  demonstrate a renderer that still draws it — PDFium reported 0 chars on that fixture.

## What the two source probes cannot see (both mutations verified)

- `every_refusal_is_both_raised_and_tested` strips whitespace but **not comments**, in both
  halves. Deleting the entire `a_vertical_writing_mode_is_refused` test and leaving the line
  `// the vertical test used to assert Refusal::VerticalWriting, and was deleted` → the whole
  suite goes green, 33 passed. Same on the production half: cycle detection deleted, a
  comment mentioning `Refusal::FormCycle.refuse("...")` left behind, probe green.
  The sibling probe already filters `//` lines; this one does not.
- `refusals_are_the_only_way_to_refuse_in_this_module` looks only for `Error::Malformed(`
  and `Error::Unsupported(`. `Error::Internal(` passes, and `show` already contains one.

## Checked and clean

No `unsafe`, no network, no logging. `#xx` escapes are decoded by the lexer *before*
`predefined_writing_mode` sees the name, so `/UniJIS-UCS2#2DV usecmap` is still caught;
lowercase `-v` fails closed via `UndeterminedWritingMode`; `/WMode` in a comment or a
literal string is correctly not a declaration; `WMode` declared twice takes the last, which
is what a PostScript `def` does. `chunks(per_code)` is guarded by `ZeroBytesPerCode`.

See [[spike-0006-redaction-survival]] control 5 — the pattern finding is that shape again:
nothing observed the removal.
