---
name: m2-218-fix-review
description: Seven rounds on the #218 cut-font path fix. r1-r3 scope leaks; r4 inherited-Tf leak; r5 refusal sound (prefix invariant), gs /Font (#152); r6 cut-count invariant inert; r7 gate moved into the check, sound, but its reference count is unpinned.
metadata:
  type: project
---

## Round 2 (9787e23, path-based redesign), 2026-09-28

- Paths (`FontPath {form, name}`) mirror the op's `font_path_in_scope`, which scans only page-level
  `/XObject`. A glyph in a NESTED form falls through to the page `/Font` by name. N3 (nested form's
  own /F9 draws the secret, page also names a different /F9): **Ok, secret font un-narrowed**
  (`/Differences [83 /Sacute]`, `<53> <0058>`), report cuts the page font. Base 1a0f6a7: same leak;
  round 1 (8c8a43d) refused it. The read-back now shares the op's resolution by construction, so
  any op mis-resolution is invisible. Without the name collision: Internal refusal (base too).
- Fan-out fixed: 4,000 cut fonts, 1.9 MB -> 0.6 s, 127 MB.
- Survivors: read-back (0,0) refusal (unreachable behind [direct-font]), unresolved path ->
  `continue`, examine-only-first-cut-font (no real test with 2+ cut fonts where one is un-narrowed).
- Stale round-1 "sharing survey / cross-checked by count" text left in redact_verify.rs header,
  burrow-ops redact/mod.rs, ADR 0029 ~l.1771 and ~l.1789; comment names nonexistent `cut_font_codes`.

## Round 3 (5bc58e6, chain_to DFS by form id), 2026-09-28

- **Resolving by searching for the form id is not the walk's resolution.** A no-/Resources form X
  listed in BOTH the page /XObject (undrawn there) and A's own /XObject inherits a different
  scope per chain; uses(X)=2 is refused only for CUT glyphs, so X's *kept* glyphs get attributed
  by `codes_still_drawn`/witness `drawn_codes` to whichever chain the DFS meets first (key order,
  attacker-chosen). Measured: page font P's `<53> -> Q` kept after its only use was removed, `Ok`
  (reverse key order: P narrowed correctly). Mirror: A's font had all /Widths zeroed and
  /ToUnicode dropped while X still draws `K` with it, `Ok`. Base 1a0f6a7 leaked in both orders.
  Fix: carry the walk's Do-name chain on the glyph; never search.
- DoS: no memo, no checkpoint inside the DFS, called per still-drawn glyph (x2). 1000 forms with
  /Resources << /XObject D >>, D = 1000 non-streams, 178 kB: 0.57 s/glyph release; g=100 58 s Ok
  (base 0.78 s); 5 s budget -> 89.8 s. Depth cap + visited-on-first-push: 40 undrawn no-res forms
  sorted first -> `[font-scope-unresolved]` over-refusal. Recursing into a no-res form is a no-op
  (mutation M16 equivalent).
- Survivors: depth guard (load-bearing vs stack), `[font-scope-unresolved]`->page fallback (!),
  read-back (0,0), Form-subtype filter. `cut_fonts` still `let Ok(..) else continue` over the new
  refusal; only check_type_three running first keeps it fail-closed.
- Method: mutation runner = python replace + assert count==1 + require "Compiling burrow-engines".

## Round 4 (cd88891, route recorded by the walk), 2026-09-28

- The route mirrors the walk exactly (`resolve_from` == `within`/`form`: STREAM check, own
  `/Resources` iff DICTIONARY else inherit). r3's two-scopes leak closed both orders. Memo per page
  is correct. Arc route cost: depth-15 x 255-byte names x 4000 draws x 200k glyphs, 2.1 MB:
  1.44 s vs 0.96 s base; ~0.4 s unwatched overshoot at a 1 s budget.
- **The walk itself diverges from PDFium: a font selected by `Tf` in an enclosing stream is
  re-resolved BY NAME in the form's own `/Resources`** (`draw_form` passes `..state.clone()`,
  `show` calls `resources.glyph(&font)` on the inner scope). PDFium binds the font object at Tf.
  Measured with pypdfium2 (read-only venv at ~/Desktop/OpenSource/future-agi/futureagi/.venv):
  page `/F1 24 Tf` (P, 556 widths), `/X Do`, X own `/Font /F1` = zero-width S, `BT 20 700 Td
  (AAASECRET) Tj ET` -> PDFium draws at 20..141 in region; burrow `Ok`, text verbatim (also via
  `burrow_ops::redact::page`). Pre-existing on main. Variant (Tf in form A, shown in B with own
  /F1): `Ok` with A's font still mapping the removed code -- **main refused it (Internal)**, so
  the branch turns a refusal into the #218 leak.
- Page `/Resources null` with a `/Pages` `/Resources`: qpdf reads null as absent and climbs;
  PDFium stops (stock Helvetica). `Ok`, secret intact. `dictionary_key`'s NULL => Ok(None) is the
  gap; [], int, name are refused. Same class as #166 R3.
- Sweep 14/14 applied+compiled: survivors = memo keyed by name only (4 plants, steps + witness)
  and `cut_fonts` propagate->continue (unreachable: walk already resolved it). no-restore caught
  only by the web differential test.
- My python port of r3's deep() was wrong (G pointed at an A); use r3mod.rs itself.

## Round 5 (af19c39, [font-selected-in-another-scope]), 2026-09-28 -- not blocked

- Rule is sound by construction: state flows only downward (draw_form clones, Q restores a
  same-stream save), so `font_selected_along` is always a PREFIX of the current route and
  equality <=> the Tf ran in this stream. Hence compare-by-len is an equivalent mutation;
  compare-last-only is not (route [X] vs [X,X]: A's own /XObject names a different form `/X`).
- All round-4 shapes refuse; `'`/`"`, Type 3 via inherited Tf, Tf-in-form-then-page-text
  (with and without q/Q) all correct. Over-refusal: 0 hits in 1,918 pages / 224 local PDFs
  (texlive docs + burrow corpus/tests); it does refuse a resource-less child form (same scope).
- **gs /Font is a live Ok+plaintext leak, pre-existing, disclosed as #152**. PDFium, on ANY `gs`
  whose ExtGState has /Font, drops the Tf font for stock Helvetica metrics (even when /Font names
  the same object; ToUnicode ignored). Tf zero-width font + `/GS0 gs` -> walk puts all glyphs at
  x=20, PDFium spreads 20..165 -> Ok, AAASECRET verbatim. 0 of 224 docs (56 with ExtGState) have
  an ExtGState /Font array, so refusing gs-with-/Font (graph-level) would be free.
- Sweep (apply-asserted, fresh builds): refusal-off / rebind-at-Do / no-record all killed.
  Survivors: compare-last-only (witness xx.pdf -> Ok + leak), and the INSERT-side cache-by-name
  mutations in steps and witness (commit only planted lookup-side). Witness: form drawn BEFORE
  page text with same-named font -> e2 alone OutputRejected, f2 alone OutputRejected, both Ok + leak.
- Fixtures/scripts: scratchpad/r5/{mk,mkgs,mkctl,mk3}.py, probe.rs, mut.py, mutprobe.py.

## Round 6 (8475536, cut-count invariant), 2026-09-28 -- not blocked

- `Redaction::cut_fonts` compares `#outcomes{cut}` to `cut_paths.len()` on the Steps' Vec. In the
  only real Steps (steps.rs) the two pushes are adjacent after `seen.insert` dedupe, so the
  invariant cannot fire in production; identity dedupe (one object, two names/scopes) yields one
  outcome + one path -> no over-refusal (by construction; `continue`s all precede both pushes).
- It counts ENTRIES, and sits upstream of where the check is told (observe closure ->
  BTreeSet -> orphaned_codes' own identity dedupe). Measured, apply-asserted, 784-test base:
  invariant-off killed only by the new fake test; steps pushing a duplicate of the first path
  (count equal, set collapses) passes the invariant, killed only by the nested-form tests;
  **observe keeping only the LAST path survives all 784** (first-only killed by nested tests,
  which order page [] paths first). ADR "the check cannot be told about fewer fonts than were cut"
  overclaims. Fix: gate distinct output identities examined in orphaned_codes == report cut count.
- Runner: scratchpad/r6/mut.py (target-sec-r6).

## Round 7 (b3e2423, examined == Cleared::cut in region_is_cleared), 2026-09-28 -- not blocked

- Gate sound: paths come from the same `resolve_from` on the same in-memory graph that is written;
  nothing after `cut_fonts` edits /Resources and qpdf never merges objects, so input->output
  identity is injective. Two paths -> one output font can only refuse (fail closed); a path
  reaching a different font than cut needs the writer to change resolution -- none found.
  No over-refusal path (steps dedupe by identity, one path per cut font). Removing
  `ScopedFont::drawn_in` changes no key equivalence: form id is a function of the route.
- Sweep (apply-asserted, fresh compile, 785 base): gate-off killed only by the Liar test;
  observe-last-only and steps-dup-first each kill 13 (author said 6); observe-nothing kills 6;
  witness orphans-first-only kills 2. Survivors: `examined: cut.len()` (equivalent, injective)
  and **`cut: cut_fonts.borrow().len()`** -- sourcing the reference from the set, not the report.
  Combined with observe-last-only: **all 785 pass**, r6's survivor back. Fix: the hook test
  (`the_check_is_told_the_fonts_the_report_says_were_cut`) should assert recorded `cut` ==
  report's cut count on a 2+-font case. Also ADR's "zero against zero = stops cutting" omits a
  hand-off dropping both (killed by 6 tests, so covered).
- Runner: scratchpad/r7/mut.py (worktree sr7, target-sec-r6), removed after.
