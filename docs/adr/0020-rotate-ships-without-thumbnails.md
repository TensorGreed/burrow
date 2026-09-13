# 0020. Rotate ships without page thumbnails

Date: 2026-09-13

## Status

Accepted.

This is the `add-operation` checklist's §2a gate, answered before the web page is written
rather than during it. §2a exists because `merge` discovered mid-implementation that it needed
an engine capability nothing had, and that capability crossed six individually-gated artifacts.
Rotate's UI raises the same question and the answer is different, which is worth recording:
§2a is a gate, not a formality, and "no new capability" is a legitimate outcome of walking it.

## Context

Rotating a page is the one operation whose result a person cannot check from a page count. A
merge of three documents into sixteen pages is verifiable by reading "16 pages"; a rotation is
verifiable only by looking at the page. So `/rotate-pdf` has a real pull toward thumbnails that
`/merge-pdf` did not, and the honest version of this decision has to say what is being given up.

### What thumbnails would actually cost, measured

The obvious objection — "it means rendering, which means a PDFium rebuild" — is **wrong**, and
the measurement is the reason this ADR is not two lines long.

`engines/vendor/wasm/lib/pdfium.wasm` is a prebuilt module with **429 `FPDF_*` exports**, and
unlike qpdf it has no `EXPORTED_FUNCTIONS` allowlist in `engines/build-wasm.sh` — that allowlist
exists for qpdf, which we link ourselves. Measured against the staged module today:

| symbol | exported? |
|---|---|
| `FPDF_RenderPageBitmap` | yes |
| `FPDFBitmap_Create` | yes |
| `FPDFBitmap_GetBuffer` | yes |
| `FPDFBitmap_Destroy` | yes |
| `FPDFBitmap_FillRect` | yes |
| `FPDF_LoadPage` / `FPDF_ClosePage` | yes |
| `FPDF_GetPageWidthF` | yes |

So of §2a's six rows, **three cost nothing**: no export-list change, no engine rebuild, no new
content hashes, and therefore no CSP regeneration and no size-budget re-measure. That is the
cheap half, and pretending otherwise would be the kind of convenient reasoning this project is
supposed to distrust.

The three that remain are the expensive ones:

1. **A new engine trait method and its native/web implementations.** Rendering is the first
   capability that produces *pixels* rather than a document, and the two implementations are
   deliberately separate so the differential corpus can catch them diverging.
2. **A new bridge method carrying bitmap bytes.** ADR 0009 §2: the bridge method list **is** the
   audit surface. Rendering adds a method that takes a page index and a target size and returns
   a buffer, plus its `worker/bridge.js` half and its `web/fake.rs` `Call` variant.
3. **The memory question, which is the one that decides this.**

### The memory question

A thumbnail at 120×160 in PDFium's BGRA is 76,800 bytes. `Limits::DEFAULT.max_pages` is
**10,000**, so a full set for a document at the ceiling is **768 MB** — held in the engine heap
while it is rendered, copied across the bridge, and then held again on the main thread as
`ImageBitmap` or canvas data. On the web engine modules' fixed 2 GiB maximum that is not a
theoretical figure.

And **nothing would stop it.** `max_memory_bytes` **detects, never bounds** (ADR 0007's
2026-09-12 amendment, and `core/CLAUDE.md`), so it would report the overrun after the allocation
that caused it. `max_pixels` is exact and would bound a *single* render, not a set of ten
thousand. A thumbnail strip therefore needs a ceiling that does not exist yet — a render budget,
or a windowed strip that renders only what is on screen and releases what is not — and that is a
design decision with its own limits, its own tests, and its own failure modes.

That is the real cost: not the rendering, but **a new class of resource ceiling arriving at the
same time as an operation's first UI**. Rotate is not the place to introduce it.

## Decision

**We will ship `rotate` v1 with no page thumbnails.** The page offers:

- **rotate every page** — one control, the common case, and the one that needs no page identity
  at all;
- **selection by page number and range** — `3`, `2-5`, `1, 4, 7-9` — parsed in one place and
  reported against the document's real page count, so a person can rotate a subset without the
  UI having to show them the pages.

No engine capability is added. `PageRotator` is the whole seam; §2a's six-row table applies to
zero rows.

**Thumbnails are filed as [#57](https://github.com/TensorGreed/burrow/issues/57)**, with the render capability, the bridge method, and
the memory ceiling as its scope. The issue records that **`/split-pdf` will want it too** — split
is the other operation where a person is choosing *which pages*, and choosing page 7 of a scanned
document by number is a worse experience than choosing it by sight. So thumbnails should be built
once, for both, with the ceiling designed rather than discovered — not bolted onto whichever tool
page happens to reach for them first.

## Consequences

**Easier.** Rotate's page is one Svelte island with no new bridge surface, no new wasm exports,
no size-budget movement, and no new ceiling to design and test. The bridge PR that precedes it
carries only what `rotate` itself needs, so the audit surface ADR 0009 §2 describes grows by one
operation rather than by one operation plus a renderer.

**Harder, and accepted.** A person rotating a subset of a scanned document has to know which page
numbers they want. For a document whose pages are not self-identifying that is genuinely worse
than a thumbnail strip, and the page should not pretend otherwise — it says what it can do and
what it cannot, in the same voice `/merge-pdf` states what it keeps and `/split-pdf` states what
it drops.

**A second-order cost worth naming.** Shipping the selection UI first means the thumbnail work,
when it comes, has to fit an existing page rather than being designed alongside it. That is a
real risk of rework. It is accepted because the alternative is designing a resource ceiling under
deadline pressure from a UI, which is how ceilings end up detecting rather than bounding.

## Alternatives considered

**Thumbnails in rotate v1.** Rejected on the memory question above, not on the build cost —
which measurement showed to be near zero. A feature whose worst case is 768 MB under a ceiling
that only detects overruns is not a feature to add while also adding an operation's first UI.

**Thumbnails for the first N pages only, say 20.** Tempting, and it bounds the memory. Rejected
because the bound is arbitrary and the failure is silent in the way this project most dislikes:
a person with a 40-page document sees pictures for half of it and page numbers for the rest, with
nothing explaining why. If a window is the answer it should be a designed window — render what is
visible, release what is not — which is the issue's scope, not a constant.

**A single preview of one chosen page rather than a strip.** Bounded memory, one render, and it
answers "did that rotate the way I meant?" for the page being looked at. Genuinely attractive, and
still rejected for v1 — it needs every one of the three expensive §2a rows (trait method, bridge
method, reply carrying bytes), so it buys a smaller feature for the same structural cost. It is
recorded in the thumbnails issue as the cheapest first increment, because it probably is.

**Rotate-all only, with no selection.** Simpler still, and rejected as too little: rotating one
page of a scan that came in sideways is the second most common reason to reach for this tool, and
a range parser is a day's work with no new capability behind it.
