---
name: m2-named-properties-resolver
description: Two rounds on #166. R1 (92f11ea) stream-valued /Resources leaks, untyped OCMD, 5.4 GB / 113 s. R2 (b10326b) the stream-only gate misses non-stream non-dicts and non-gated lookups: 6 measured leaks, an OC memo regression, 44 s past a 1 s deadline.
metadata:
  type: project
---

Reviewed 2026-09-24 in a throwaway worktree at 92f11ea. Everything below was **run** end to end
through `burrow_ops::redact::page`, with the PDFium oracle (`chars_on_page`) as the witness.
Probe file kept at scratchpad `zz_sec166.rs.keep` for that session only.

## The root cause: PDFium reads a STREAM's dictionary where burrow sees "not a dictionary"

PDFium's `GetDictFor` returns a stream's dict, so `/Resources 7 0 R` (a stream) is the page's or
form's resources to PDFium. burrow's `PageResources::of`, `scope_of`, `Resources::within`, the
sharing walk and the new OC walk all treat a non-dictionary as absent and **inherit** instead.
Measured, all returning `Ok`:

- page `/Resources` = stream carrying `/Properties /MC0 << /ActualText (C) >>`, `/Pages` carrying
  a benign `/MC0` -> PDFium reads `C` off the output. **New in 92f11ea** (HEAD~1 refused as unresolved).
- same, for a form's `/Resources` -> same leak. New in 92f11ea.
- page `/Resources` stream holding an OCG, or `/Properties` category itself a stream -> OC walk misses.
- **Pre-existing, worse:** stream-dict `/F1` = real Helvetica, `/Pages` `/F1` = widths 20000 ->
  burrow places the glyphs off-region, `Ok`, PDFium reads `SECRETWORD` from the output. Same at
  form level. The read-back cannot see it (same walk).

**How to apply:** in any geometry/redaction review, build the stream-valued `/Resources`
fixture alongside the form-local-resources one. It is the #1 divergence class not yet closed.

## Other findings

- `is_optional_content_group` needs `/Type /OCG|/OCMD`; PDFium treats any non-OCG dict under an
  `/OC` mark as an OCMD. `<< /OCGs [n 0 R] >>` with no `/Type` -> `Ok`. PDFium hiding it is
  source-read, not rendered.
- `NamedProperties` stores unparsed bytes per name, cloned per form by `union`: 4000 names -> one
  200 kB indirect dict, 5 resource-less forms: 252 kB input -> **5.4 GB** RSS, `Ok`, and
  `max_memory_bytes` did not fire.
- `named_list_carries` re-parses the list per `BDC`, no memo, no checkpoint: 40k `BDC`s x 200 kB
  list -> **113 s** against `max_duration_ms = 60 000` (control with a tiny list: 0.12 s).
  One fix closes both: classify once at read time and store `Carried`, not bytes.
- The OC walk re-reads a shared resources dict per stream (N^2); the sharing walk's budget caps
  it near N=2800, about +2.3 s. The ADR's "no ceiling of its own" argument roughly holds.

## Mutation sweep: 20 planted, 20 applied and compiled, 4 survived

M3 (OCMD not recognised) and M6 (AP state-dictionary branch) are real fixture gaps. M14 is
equivalent. M15 (owners first-wins) survives because **the sharing refusal fires first**: a form
reached by two enclosures is counted as shared even through one shared `/XObject` dict. Measured
both shapes. The first M6 plant (`NULL =>` arm) failed for the wrong reason and was re-planted.

**Scratchpad hazard:** the scratchpad was shared with two other reviewers' worktrees, and a
`base.log` I wrote was overwritten by one of them. Use unique file names.

## Round 2, b10326b (2026-09-24): the fix narrowed the class to STREAMS, the class is TYPES

Probe file kept at scratchpad `sec2-probe.rs.keep` (that session only). Old repros all refuse now;
5.4 GB -> 39 MB, 113 s -> 0.35 s. Measured leaks, each `Ok`, PDFium reads the secret afterwards:
- page `/Resources []` or `0` (not a stream): burrow still climbs to `/Pages`, PDFium's
  GetPageAttr stops and uses stock Helvetica. Same lookup the fix is about.
- Type 0 `/DescendantFonts [stream]`: qpdf `getKey` on a STREAM returns null (+warning), so
  `/DW` reads 1000; PDFium's GetDictAt reads 500. Not a key the sharing gate visits.
- `unparse` prints indirect ARRAY ITEMS as `N G R`, `numbers_in` reads two numbers: `/Widths`
  of indirect ints shifts every width; `/CropBox [0 0 612 7 0 R]` drops to MediaBox (region
  mapped 392 pt off). `/LastChar` ignored by burrow (PDFium zeroes codes past it).
- form whose own `/Resources` lacks `/XObject`: PDFium falls back to the page's, burrow skips `Do`.
- `/Annots` entry that is a stream: kept by `remove_annotations_in`; MuPDF renders it (witness).
- OC walk's single `seen` set: an object first queued as a FONT is later skipped as a resources
  dict -> typed OCG in an appearance stream's `/Resources` passes. Regression (memo-less refuses).
- `sharing::properties()` unparses a shared `/Properties` per resource dict, page-only checkpoint:
  1.75 MB -> 44 s vs 1 s deadline (release); stub it -> 1.03 s.
Mutations 15/15 applied+rebuilt, 6 survived (entry-stream check, /AP+/CharProcs, /Font, OC memo,
per-entry checkpoint, Unknown~Nothing rank). poppler hides neither untyped nor inline OC marks.

**How to apply:** a gate keyed on one wrong TYPE invites the next; ask what every lookup does on
every non-dictionary type, and on array items that are references.

Related: [[m2_nested_form_lookup]], [[m2_redact_verify]], [[m2_actualtext_rewriter]], [[m2_standard14_and_marked_content]].
