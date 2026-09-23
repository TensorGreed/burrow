---
name: m2-nested-form-lookup
description: Review of 133b983 (#164 nested form lookup) — two measured /ActualText leaks the fix does not reach (intermediate-form span, and a form with no /Resources), and a 10 kB file that burns 122 s under max_duration_ms=100.
metadata:
  type: project
---

Reviewed `133b983` on `m2/164-nested-form-lookup`, 2026-09-23, throwaway worktree `/tmp/sec-164`.
Everything below was **run**.

## The /ActualText refusal has two open doors, and both beat the commit's own fixture

`check_marked_content` runs on the page stream and on **each form that contains a removed
glyph**. `form_names_for`/`reaches_wanted` fixed the *page* stream's view. Neither covers:

1. **The span lives in an intermediate form.** page → Outer → Inner, Inner holds the glyphs,
   Outer's stream is `/Span << /ActualText … >> BDC /Inner Do EMC`. Outer is not in
   `reached_forms`, so no `check_marked_content` call ever reads its bytes. Measured: `Ok`,
   carrier in the output, PDFium reads `BURROW-SEC164-MIDFORM.` off the redacted file.
2. **The intermediate declares no `/Resources`.** `Resources::within` returns `None` and the
   walk resolves the child through the *page's* resources (`qpdf/resources.rs:231`), so the
   glyph's form is a page-level name — but `reaches_wanted` bails at
   `own.type_code() != DICTIONARY` and never links `/Outer` to it. Page span around `/Outer Do`
   leaks. Measured the same way.

The commit's own `evade-actualtext-around-a-nested-form` (both levels carrying `/Resources`)
**does** refuse, as does inheritance-from-`/Pages` and a form aliased as both child and
grandchild (the latter via `shared-form-would-change-elsewhere`).

**Why:** the rule is "a stream answers for a span only if it holds a removed glyph", and a
stream that merely *draws* one is not asked. **How to apply:** build the mid-form fixture first
in any future marked-content work; it is 3 objects.

Closed on this commit: patterns (`pattern-may-draw-text`), annotations (`/Annots` comes out
empty), depth ≥ 16 (`form-depth`). A Type 3 `/CharProcs` that does `/Sec Do` renders nothing
after the edit but leaves the secret **verbatim in the orphaned form stream** of the output.

## `reaches_wanted` / `find_form_below` enumerate PATHS, not nodes

`open.remove(&here)` after the recursion makes the open set path-local, so a DAG is re-walked
once per path: cost is `branch^16`, and `dict::MAX_KEYS = 4096` is the only bound on `branch`.
No deadline checkpoint anywhere in either recursion; the `LimitExceeded` fires at the *next*
checkpoint, after the burn. Measured, release, `max_duration_ms = 100`:

| file | size | elapsed |
|---|--:|--:|
| branch 3, depth 12 | 7.9 kB | 2.1 s |
| branch 3, depth 14 | 9.0 kB | 19.5 s |
| branch 3, depth 16 | 10.1 kB | **122.6 s** |
| branch 4, depth 16 | 13.9 kB | >300 s (killed) |
| branch 6, depth 16 | 22.6 kB | >300 s (killed) |

RSS stays ~6.6 MB — it is pure CPU. **`wanted` must be non-empty**: `form_names_for` returns
early on an empty set, so the bomb needs one drawn form holding a cut glyph plus undrawn
bomb roots in the page's `/XObject`.

## Mutation sweep: 11 planted in `redact_steps.rs`, 11 applied and compiled, **8 survived**

Caught: deleting the descent in `find_form_below`; `reaches_wanted` → always false; dropping
`wanted.contains(&here) ||`. Survived: **both depth caps**, **both cycle guards**, **both
`open.remove` calls**, the depth increment, and `form_names_for`'s `type_code() != STREAM`
guard. Nothing in the corpus is cyclic or deep enough to notice.

## The corpus gate

- `no_placements_because` is an unbounded opt-out: replacing `01-plain-tj`'s placement with
  `no_placements_because = "nothing to see here"` leaves both `check-redaction-corpus.sh` and
  `redaction_corpus.rs` green. There is **no floor on the placement count** (54 today).
- Completeness compares **basenames**, so `file = "generated/../../../../../../tmp/01-plain-tj.pdf"`
  passes and the checker `read_bytes()`/`qpdf`s that path.
- `rm -f "$out"/*.pdf` is safe: `root` comes from `BASH_SOURCE`, cwd-independent, and a missing
  directory is a no-op (verified from `/`).
- There is **no `tools/test-check-redaction-corpus.sh`** — every other `check-*.sh` has one, and
  the new rules have no per-rule probes.

Related: [[m2_standard14_and_marked_content]], [[m2_qpdf_redact_steps]], [[m2_redact_verify]].
