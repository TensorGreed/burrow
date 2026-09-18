---
name: m1-render-progressive-and-strip
description: "#57 piece 2b (progressive render + thumbnail strip): the modelled strip thrash loop, the ungated page_size double load, and the three baselines one ceiling now has."
metadata:
  type: project
---

Reviewed 2026-09-17 on the uncommitted working tree of `main` (progressive render,
`PageThumbnails.svelte`, `IFSDK_PAUSE`).

**THE STRIP NEVER SETTLES WHEN THE VIEWPORT HOLDS MORE TILES THAN `LIVE_THUMBNAIL_WINDOW`.**
`accept()` releases the oldest live tile back to `"waiting"` without asking whether it is still
in `wanted`, so `outstanding()` is immediately non-empty and `pump()`'s tail `await pump()`
recurses forever. Modelled faithfully in node: 112 wanted (the figure
`thumbnail-policy.ts` itself computes for 1920x1080), window 64 → 200 requests, 9,664 page
renders, `outstanding` pinned at 48, no scrolling. Needs only a 65-page PDF and a desktop (or
browser zoom-out); a hostile page makes each round a full `max_duration_ms`. The policy file
documents this as "tiles furthest from the scroll position will be redrawn when they come back",
which is the benign reading of the same fact. `page-thumbnails.test.ts` is a source scan only —
nothing exercises `pump`/`accept`.

**`page_size` IS A SECOND, UNGATED `FPDF_LoadPage`, AND IT RUNS FIRST EVERY TIME.**
`burrow_ops::render::Render::draw` calls `engine.page_size()` then `engine.render()`; both
`pdfium::page_size` and `web::page_size` do a full `load_page`/`close_page` with **no**
`estimate::before_page_load` and no memory reading at all. So the uninterruptible 757 MiB /
1,765 MiB load the new guard exists to refuse is paid *before* the guard is consulted, and then
paid again inside `render`. `FPDF_GetPageSizeByIndexF` is exported by both vendored artifacts
(native `.so` and `pdfium.js`) and does not build a display list — that is the fix that removes
the double load rather than just gating it.

**ONE CEILING, THREE BASELINES.** `max_memory_bytes` on the render path is now compared against
(a) the reading at document open — `before_page_load`, and native takes it *after* the load
while web takes it *before*, so the same file diverges across platforms; (b) the reading after
`FPDF_LoadPage` — the in-loop slice check, so the load's ~70% of the cost never counts while the
render is running; (c) the reading before `FPDF_LoadPage` — the post-render check. Each is
defensible alone; the doc comments read as if they were the same question.

**WHAT HELD UP** (so a later reviewer need not redo it): `IfsdkPause` matches
`fpdf_progressive.h:25-43` (int / fn ptr / void*, `Option<extern "C" fn>` is null-pointer
optimised); `always_pause` returns a constant, allocates nothing, and `extern "C"` aborts rather
than unwinds even hypothetically; the `Box` pins the pause for the whole render and is dropped
after `render_page_close` on both platforms; `render_page_close` runs on every exit including
the deadline refusal; the loop breaks on any non-`TOBECONTINUED` state. JS side: `HEAPU32` is
re-read after `_malloc`, `addFunction` failure is caught and fails closed to `PdfiumPtr::NULL`,
and the table index is returned to `freeTableIndexes` so the table does not grow —
`functionsInTableMap` is a **WeakMap**, so the stale key is collectable and there is no leak
there either. Residual cost only: a fresh arrow function per render means
`convertJsFunctionToWasm` compiles a tiny `WebAssembly.Module` per page. **Do not "fix" that by
hoisting the arrow function** — `getFunctionAddress` would then return the index whose table slot
`removeFunction` nulled. Create the pause once per worker and never destroy it, or leave it.

No `<img>`, no object URL, no `blob:`/`data:` anywhere in the strip; pixels reach the DOM only
through `putImageData`; the only strings rendered are static. `render-one-quadrant.pdf` is 465
bytes, one page, one `re`/`f` in the upper-left quadrant, no filters, no JS, no attachments.

Related: [[m1-render-capability]], [[m1-render-worker-bundle]], [[m1-limits-real-strength]].
