---
name: m1-web-split-page
description: The /split-pdf island and tool-delivery.ts (branch m1-split-page, 2026-09-15) — why the #69 capture-before-await class is finally closed in code shape, and the four residuals plus the untested-wiring gap that remain.
metadata:
  type: project
---

Reviewed 2026-09-15 on branch `m1-split-page`: `tool-delivery.ts`, `split-cuts.ts`,
`split-messages.ts`, `SplitTool.svelte`, `split-pdf.astro`, `e2e/split-pdf.spec.ts`.

**The disclosure class is closed by SHAPE, not by discipline, and that is the change worth
remembering.** `delivery.beginParts({names, signature})` takes the names and the signature as
arguments, so "re-derive after the await" is not expressible — `handAll(bytes)` has no access to
the file, the cut list or the controls. The two prior findings ([[m1-web-tool-page-staleness]],
rotate's `choose()` hole) are both unreachable in this island: `choose()` invalidates *before*
`clearResults()`, `beginParts` is called before the first `await`, `run.live()` is checked after
both awaits, `handAll` checks staleness/count/holes *before the first URL exists*, and `results`
is assigned only in the statement before `phase = "done"`. Walked every interleaving asked for
(file swap mid-run, cut edit mid-run, cancel-then-resplit, slow page_count resolving late) and
found no path where bytes reach a name from other state.

**The gap is that nothing tests the WIRING.** `tool-delivery.test.ts` drives `createDelivery` in
isolation; vitest's `include` is `src/**/*.test.ts` and there is **no Svelte component test
anywhere in `apps/web`**, so no test asserts that `SplitTool` calls `beginParts` before the
awaits. `e2e/split-pdf.spec.ts` has no analogue of `reorder-pdf.spec.ts:232` ("editing the order
while it runs hides the link"), `:269` ("the order does not follow you to the next document") or
`:210`. The regression that happened twice would be caught by nothing.

Residuals, all Low: object URLs are **retained while `stale` hides them** (no `releaseAll` on the
stale transition); `announce("Done. N parts, ready to download.")` fires even when `stale` hid
every link; `onDestroy` does not `invalidate()` (safe only because `host.dispose()` →
`discard("Internal")` settles the request as a failure *and* the site has no `ClientRouter` —
grep confirmed none in `apps/web/src`); `MOST_CHARACTERS = 20_000` permits ~4,200 cuts against a
10,000-page document, and ADR 0023 §3 holds every part at once while
`web/extract.rs:274`'s `check_measured_memory` measures only the **engine heap**, not the
accumulated JS Blobs — see [[m1-split-copy-semantics]] for the 1.06 MB → 201 MB amplification.

**Two earlier findings confirmed fixed in the shipped tree:** the `every()`-skips-holes part gate
(`worker-host.js:776` is now an indexed count, and `handAll` repeats it), and the nested
optional-content bypass (`prune/mod.rs:978/990` now call `refuse_optional_content_in` on a nested
Form XObject's own `/Resources`) — which is the premise the hold-lifting rests on.

`split-messages.ts` is clean on ADR 0009: no engine prose, both `catch` blocks are bare
`catch {`, and the numbers pass through `Number()`. `size-budget.test.ts:190-244` picks up
`split-pdf.astro` automatically because it derives the page list from the directory.

See [[m1-web-split-bridge]], [[m1-split-pruning]], [[m1-web-tool-page-staleness]].
