---
name: calibration-list-vs-accepted-set
description: A calibration/probe test with a hand-written list of inputs measures that list, not the set the code accepts — diff the two, and check exclusion lists are keyed on the same side of any aliasing as the lookup.
metadata:
  type: feedback
---

When a table is calibrated against a second source, compare the test's hand-written input list
against the set of inputs the code actually accepts. Any name the code accepts but the list omits
is uncalibrated, and every claim the module makes about "the whole table" is false there. Then
check that any exclusion list (`DISPUTED`, deny-list, skip-list) is keyed on the **same side of
the aliasing** as the lookup: if the lookup canonicalises `Arial-Bold` → `Helvetica-Bold` *after*
consulting an exclusion list keyed on `Helvetica-Bold`, the alias walks straight through.

**Why:** measured on burrow 16e4c56. `standard14::width_of` accepts 20 `/BaseFont` names but the
calibration's `FONTS` listed 12; `DISPUTED` was keyed on the raw name and checked before
`family()` aliased it. Result: `/BaseFont /Arial-Bold` with no `/Widths` drew `@` at 975 where
PDFium measures 1072 — a 9.7 pt glyph misplacement at 100 pt in redaction geometry — while the
identical page named `/Helvetica-Bold` refused. 2,248 widths "compared, all agreed" and none of
them was an alias.

**How to apply:** on any commit adding a data table plus a cross-check test. Grep the match arms
(or accepted-input set) in the source, diff against the test's array literal, and report the
difference as a number. Then run the aliases through the real entry point and print both sources'
answers side by side — the disagreement is the evidence, not the reasoning.

Related: [[probe-fixture-vs-real-producer]], [[scope-set-wider-than-the-edit]],
[[mutate-the-wiring-not-the-policy]].
