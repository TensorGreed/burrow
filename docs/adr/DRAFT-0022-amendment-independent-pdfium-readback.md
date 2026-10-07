# DRAFT — proposed amendment to ADR 0022: an independent PDFium read-back

**Status: DRAFT, for the owner to decide. Nothing is built. Does not go ahead of #253/#257
(both now merged).** This is the write-up requested, not an accepted decision.

## The gap it addresses

ADR 0022's `verify::output` re-reads the output through a **fresh qpdf** and compares page count
and rotation. Redaction's own read-back re-lexes the output **with burrow's own content lexer**.
Both share a blind spot by construction: wherever burrow's reading of the bytes differs from the
*renderer's* — the #152 / #228 / #253 / #257 family, and the structural redaction leaks measured
on `/Span`/array/nested-value shapes that returned `Ok` with `SECRETINK` still drawn — a check
built from burrow's lexer cannot see the leak, because it is the lexer that is wrong. Today the
answer is "refuse rather than model PDFium" (DECISIONS rule 1): correct, but it refuses legitimate
documents it cannot prove safe. An **independent** read-back would let some of those be *measured*
safe instead of refused.

## 1. What it reads

After an operation produces output, render the affected pages of **that output** with PDFium
(`raster.rs` already rasterises a page to RGBA under `max_pixels`) and read back **pixels**, not
structure:

- **redaction:** render each redacted page and assert **zero marked pixels inside each redaction
  region** (the measurement the leak reports already use: "N dark pixels in the region").
- **split / rotate / reorder / merge:** render the output and the input's corresponding page and
  assert the visible result matches the operation's intent (e.g. a page carries no optional-content
  layer the source hid; a rotation shows the turned page).

It is *independent* precisely because the reader is PDFium, not burrow re-lexing with the lexer
whose disagreement is the bug.

## 2. Disagreement classes it closes — and the ones it does not

**Closes** (turns a refusal or a silent `Ok` into a measurement):
- inline-image extent (#228), name-token truncation (#152, #257), optional-content / hidden layers
  one or many levels down (#253, ADR 0019 §2a) — anything PDFium *draws differently* becomes
  visible as ink where burrow expected none.
- the structural redaction leaks (`/Span`, nested values, arrays) — they draw `SECRETINK`; a pixel
  read-back sees it.

**Cannot close:**
- **Non-visual leaks.** Text/selection layer with no ink, `/ToUnicode`, metadata, attachments,
  incremental-update history, a font subset still carrying removed glyphs — none render as pixels.
  For redaction *the transformed form is the leak* (ADR 0022 already warns this), and this does not
  reach it.
- **Renderer-relative truth.** It closes **PDFium** disagreement, not every viewer's. A different
  reader could still draw differently; this measures one renderer, the dominant one.
- **A page already wrong going in** (ADR 0022's largest residue) — unchanged.
- **Intent.** It checks a promise, not what the user meant.

## 2a. Measured on the leaks fixed since Oct 1 — run, not reasoned

A throwaway harness rendered each issue's existing fixture **input** with `burrow_ops::render`
(`Pdfium`, 2 px/pt) and counted near-black pixels (R,G,B < 64) **inside the region that issue's
test redacts** — a leaking output would carry the same region ink, so this is what a pixel
read-back would see. Three positive controls confirm the region math (content text lands where
expected).

| issue | near-black px in region | a pixel read-back alone | why |
|---|--:|---|---|
| #152 | 1956 (reconstruction + control) | **catches** | ExtGState `/Font` draws the secret as content ink in the region |
| #224 | 452 | **catches** | inherited `/Resources` font draws the secret in the region |
| #228 | 1955 | **catches** | inline-image `/L` makes PDFium draw skipped text as ink in the region |
| #227 | 0 (secret) | **misses** | PDFium rotates the page; the secret draws **outside** the targeted region |
| #229 | 0 | **misses** | the leak is an annotation **appearance**, and the render path draws no annotations; its ink is also outside the `/Rect` |
| #239 | 0 | **misses** | the secret is an annotation `/Contents` string, never page ink |
| #240 | 0 | **misses** | **over-removal** (a shared `/Annots` loses an annotation) — the opposite of a leak; a leak-detector cannot see it |

**Caught 3 of 7; missed 4.** The misses are not noise — they are three distinct limits this
amendment must own:

1. **The render path draws no annotations.** `burrow_ops::render` passes `RENDER_FLAGS = 0` — no
   `FPDF_ANNOT` (`core/burrow-engines/src/pdfium/ffi.rs:444`). A read-back built on it is blind to
   the whole annotation family — **#229, #239, #240**, all redaction ship-blockers. Rendering
   *with* `FPDF_ANNOT` would reach #229's appearance ink (`annotation_ink.rs` measures >300 that
   way) but still not #239/#240, which are not page ink at all. Turning annotations on is also a
   larger adversarial surface.
2. **A region-scoped scan misses ink drawn elsewhere.** #227's secret renders, but rotation moves
   it out of the targeted region. A read-back that scans only the redaction region misses a secret
   the operation displaced; scanning the whole page trades that for distinguishing the secret from
   legitimate content.
3. **It only sees ink.** #239's `/Contents` string and #240's over-removal are invisible to any
   pixel check — the non-visual half ADR 0022 already warns about, here as measured fact.

Honesty note: #152's committed fixture is a non-renderable `Fake` object, so its "catches" is a
faithful **reconstruction** plus a rendered control, not the committed bytes.

**The bearing on the launch exit rule:** of the redaction blockers this measured, the read-back
alone would have caught the content-ink ones and **none of the annotation family**. So "a final
spec review finds nothing the read-back wouldn't catch" (ROADMAP) is not satisfied by a pixel
read-back as currently rendered — the annotation refusals (#229/#239/#240) and the non-visual and
displaced-ink classes remain the structural checks' job.

## 3. Module size and per-operation time

- **Size:** a new `verify`-side module wrapping render-region-and-scan on top of `raster.rs` and
  `pdfium/` — on the order of **200–400 lines** plus its own fixtures, no new dependency (PDFium is
  already linked; `raster.rs` already renders).
- **Time:** dominated by rasterisation. At a verification DPI (not display DPI) a page is ~tens of
  ms; a pixel scan of the region is negligible. Per operation ≈ **(pages rendered) × ~10–40 ms**.
  Redaction renders only redacted pages; split/rotate render the changed pages. It roughly
  **doubles** an operation that already renders once (redaction reads the frame via PDFium today) and
  **adds a render** to ones that do not. Must run against the **operation's own** `max_duration_ms`
  (ADR 0022 already frames verification cost this way) — overshoot is one render, cooperative.

## 4. Trapped and exempt surface

- The render is a **new engine call on bytes derived from hostile input** (burrow's own output, but
  provenance is an adversarial file) → it is **trapped**: inside `catch_unwind`, mapped to
  `Error::Internal`, no panic across FFI, and it rides the existing PDFium **global-init `Once`**
  and the single render thread (`pdfium/thread.rs`) — `FPDF_InitLibrary` is not re-entrant. It is
  **not exempt**; it is one more call `verify::output` makes, alongside the qpdf re-read.
- It adds **no new *parser* entry point** of burrow's (no new fuzz target of ours) — the hostile
  surface is PDFium's, already shipped behind the render bundle (ADR 0026) and the
  `check-pdfium-is-render-only` gate. It does widen *when* PDFium runs: on every verified
  operation, not only redaction.

## 5. Fit with ADR 0013's "never resolves an object"

ADR 0013's line bounds the **prescan**: "non-parsing, never touches the PDF's bytes, never resolves
an object," run *before* the engine to size and refuse. This read-back is the **opposite phase** —
*after* the operation, on burrow's finished output — and it deliberately resolves and renders. It
does not touch the prescan, does not run under the trap-free prescan contract, and changes nothing
about ADR 0013. It belongs with `verify::output` (ADR 0022), which already resolves objects through
qpdf; this adds a second resolver (PDFium) whose whole value is that it resolves the way the reader
a person uses does.

## What the owner is deciding

The measurement (§2a) reframes the question. A pixel read-back is **not a replacement** for the
structural refusals: run on the seven leaks fixed since Oct 1 it would have caught **3** (the
content-ink divergences #152/#224/#228) and missed **4** — the whole annotation family
(#229/#239/#240) and a rotation-displaced secret (#227). So the decision is **not** "read-back
instead of refusing," it is whether a read-back is worth adding as a **second, independent
witness** for the content-ink divergence class (#152/#228/#253/#257 and the structural redaction
leaks), where today burrow either refuses or trusts its own lexer.

Weigh that narrower value against: the per-operation render cost (§3); widening PDFium execution
to every verified operation (§4); and the firm limits §2a measured — it sees only page ink, only
inside the scanned area, and (as `burrow_ops::render` stands) **no annotations at all**. It does
**not** on its own satisfy the ROADMAP launch exit rule: the annotation refusals and the
non-visual and displaced-ink classes stay the structural checks' job.

If you want it, it is a spec-review-first change (DECISIONS rule 10): the ADR 0029 §3 / ADR 0019
§2b accept-vs-refuse table and a census of what it would now *accept* that is refused today, plus
a decision on `FPDF_ANNOT` and region-vs-whole-page scanning — before any code.
