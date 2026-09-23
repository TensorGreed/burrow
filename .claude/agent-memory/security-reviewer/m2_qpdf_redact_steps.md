---
name: m2-qpdf-redact-steps
description: Review of 22ceffa (M2 qpdf redaction Steps) — measured leaks via annotations and Type 3, the shared-sub-object font edit, and the 9-of-20 mutation survival in redact_steps.rs
metadata:
  type: project
---

Reviewed `22ceffa` on `m2/131-font-resolver` (first real `redact::Steps` impl). Findings were
reproduced with `burrow_engines::redact_probe::redact_page`, which is `pub` behind
`native-engines` and is the only way in.

**The coverage fact that explains most of it.** A 20-mutation sweep over the whole
`--features native-engines` suite: every unit-level defence in `pdfsyntax::tounicode` and
`pdfsyntax::ops` was caught, and **9 of 20 survived, all of them in `qpdf/redact_steps.rs`** —
deleting the `/Differences` narrowing, the `/ToUnicode` narrowing, the `/Widths` zeroing, the
Type 3 refusal, the form-sharing refusal, the `/Contents`-is-a-stream check, the
`strip_page_keys` error drain and the page-key strip each left the suite green. The unit is
tested; the seam that calls it is not.

**Leak channels measured (rendered with poppler, both directions):**
- An `/Annots` `/AP /N` appearance stream drawing text over the region survives untouched —
  `affected_streams` never enumerates annotations and `KEPT_PAGE_KEYS` keeps `/Annots`.
  ADR 0029 §3 says *handle*, so this is an unimplemented requirement, not a recorded residual.
- A Type 3 glyph procedure drawing ink outside the glyph's own `/FontBBox`/advance is invisible:
  `check_type_three` is scoped to `cut`, and a region over the visible ink puts nothing in `cut`.
  Also bypassable honestly: `/Differences [5 /g 5 /secret]` — burrow takes the FIRST assignment
  for a code, poppler takes the LAST (rendered identical to the `/secret` fixture).
- `narrow_differences` keeps every name whose *code* is kept; duplicate anchors
  (`[0 /S 0 /e 0 /c 0 /r]`, all legal) leave the whole spelling in the file. `u32::try_from(..)
  .unwrap_or(0)` makes `4294967296` and `-1` do the same.

**The granularity bug worth remembering.** `fonts_wholly_within` tracks *font objects*;
`narrow_font` edits their *sub-objects*. Two fonts, one cuttable and one on an unredacted page,
sharing an indirect `/Encoding` or `/ToUnicode` → the shared object is rewritten and the
untouched page's text loses its CMap and its glyph names. The report never mentions that font,
so `discloses_a_retained_font()` is false.

**Cost.** `ToUnicode::parse` expands `bfrange` unconditionally: 752 kB of repeated
`<0000> <FFFF> <0000>` = **61.8 s release**, linear. 30 kB of Flate → 10 MB → ~14 min, and
`stream_data()` bounds no decompressed output. No deadline checkpoint inside `parse`.

**Two test-infra facts.** The corpus needs BOTH `make-redaction-fixtures.py` AND
`make-evasion-fixtures.py` into `tests/redaction/generated` to reach the 43 the commit claims
(24 + 15 + 4 producers); with only the first the `expected >= 40` gate fails.
`sharing_tests::an_annots_array_of_non_dictionaries_does_not_retain_a_warning_each` reads
process-wide `VmHWM` and is flaky under the parallel lib suite — it failed once here and passed
in isolation and on re-runs; it can also pass spuriously when a neighbouring test inflates the
control.

Clean: no network, no logging, no file content in any new error message; one `unsafe` block
(`ObjectHandle::page`) whose SAFETY comment attributes the bounds check to `QpdfRedaction::new`,
which does not make it — the check is in `redact_page_for_probe`, and `qpdf_get_page_n` uses
`.at(i)` so an out-of-range index latches rather than being UB.

See [[spike-0006-redaction-survival]], [[m2-glyph-geometry]], [[m2-remove-glyphs-and-sharing]].
