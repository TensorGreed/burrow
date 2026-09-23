---
name: m2-shared-contents
description: Review of 001e2f5 (#132, per-element shared /Contents detection) — the two disjoint sharing counters that miss every cross-route sharer, the 1 MB → 8.2 GB element amplification, and the 4 of 16 mutation survivors.
metadata:
  type: project
---

Reviewed `001e2f5` on `m2/132-shared-contents` on 2026-09-23 in a throwaway worktree.
Everything below was reproduced through `burrow_engines::redact_probe::redact_page`.

## The hole the piece does not close: two counters, neither of which is the total

`FormUseCounts` keeps **two disjoint maps**. `counts` (from `/XObject`, `/Annots /AP`,
patterns) is consulted only by `check_form_sharing`; `content_refs` (from `/Contents`) only
by `check_contents_sharing`. Neither check sums them, so an object reached by *one* route of
each kind reads as unshared to both. Three legal documents, all measured as **Ok** with the
shared object edited:

- page 0's whole `/Contents` is also a Form XObject `Do`-drawn by page 1 (the stream carries
  `/Type /XObject /Subtype /Form /BBox` — extra keys on a content stream are ignored, so one
  object is legal as both);
- the region on page 0 reaches a form that is also page 1's whole `/Contents`
  (`check_form_sharing` sees `uses == 1`);
- page 0's `/Contents` is also page 1's `/Annots /AP /N` appearance stream.

In all three the emitted object came back as `[-556 x6] TJ` and the other page still points
at it. ADR 0029's 2026-09-23 amendment says the gap "is closed"; it is closed only for the
`/Contents` → `/Contents` route.

## Cost

`redact_steps::page_contents` decodes **every element of every `/Contents` array** before any
sharing or limit check, and `MAX_ELEMENTS` is 4096. 4096 references to **one** 1 MB
uncompressed stream — no compression needed, the amplification is the repeat — is a 1.02 MB
file reaching **8.2 GB VmHWM in 2.9 s** (release), `bodies` plus `joined`. It then fails on
the *operations* ceiling in `glyphs_in`, i.e. after both allocations. No `Limits` and no
`Deadline` checkpoint inside either the element loop or `Contents::concatenate`.
Not publicly reachable today: redaction has no entry point outside `redact_probe`
(ADR 0022 / #134).

## Mutations: 16 planted, 4 survived

Caught: reversed write-back zip, reversed handle order, walk element ceiling, `references > 2`,
walk-first-element-only, skip-non-stream-element, page-level (not per-reference) counting,
dropping the `check_contents_sharing` call, writing only the first element, dropping
`count_distinct_operations`, `locate` off-by-one, walk skipping the whole-stream case.

Survivors:
- **`check_contents_sharing` always using `elements.len() - 1`** instead of the located index.
  Every array fixture has exactly two elements and the reached one is the last, so "always the
  last element" is indistinguishable from "the element the cut landed in". The per-element
  mapping is asserted by no fixture with ≥3 elements.
- **Deleting the `GlyphFromAnotherStream` guard in `remove_glyphs_across`.** The check is
  duplicated in `remove_glyphs` (line ~1098) and `_across` (~1146); both geometry tests drive
  `remove_glyphs`, so the copy on the array page path — the path the new code takes — has no
  test. Unreachable by construction today (the caller filters `form.is_none()`).
- The `seen` dedup in `check_contents_sharing` (behaviour-neutral) and the `parts.len()`
  invariant (the commit's own recorded survivor).

## The fuzz oracle

`Shape::references_to_the_reached_element` is sound for what the generator builds (only stream
0 draws in the band; everything else at y ≤ 150 is far outside). Its limit is the generator:
≤ 4 pages, ≤ 3 elements, ≤ 6 streams, arrays of direct stream refs only — no forms, no
annotations, no indirect/nested arrays, no non-stream elements, no duplicated page objects. So
none of the three cross-route documents above is in its reachable set, and a refusal returns
early, so over-refusal is invisible by design.

## Checked and clean

No network, no logging, no file content in the new error messages (counts only). No new
`unsafe`. `Contents::apply` refuses a replacement straddling an element boundary and
`rewrite_without` never emits an empty replacement, so a straddling cut fails closed rather
than writing into an element whose sharing was never checked. Repeated objects in one array
(`[5 0 R 6 0 R 5 0 R]`) are refused whenever the cut lands in the repeated element, because
the walk counts per reference.

See [[m2-remove-glyphs-and-sharing]] (which first reported the uncounted `/Contents`),
[[m2-qpdf-redact-steps]], [[m2-glyph-geometry]].
