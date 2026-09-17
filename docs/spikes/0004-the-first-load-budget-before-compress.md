# Spike 0004 — the first-load budget, and PDFium leaving the web

**Date:** 2026-09-15
**Status:** decided — PDFium does not ship to the web; the total budget is not raised.
**Amended 2026-09-16 by [ADR 0026](../adr/0026-how-rendering-loads-without-returning-to-the-old-payload.md):**
PDFium ships to the web again, in a **second worker bundle** fetched only when a page needs to
render. The measurement below is unchanged and so is what it bought — a visitor who merges two
files still downloads none of it — but the decision line above is no longer true as written,
and a reader landing here would otherwise read a falsified decision. Said on this document's
own status line because that is where this repository puts it (ADR 0002, 0007, 0008), not only
in the amending record.

## Why now

M1's split bridge took the payload to **2,389,341 brotli against a 2,403,482 budget**:
14,141 bytes of headroom, **0.59%**, where `apps/web/size-budget.json` records the margin as
3%. The gate holds and the margin does not, and those are different sentences.

`compress` is the last M1 operation and may want engines nothing here ships yet. This was
written so the decision is made deliberately rather than by a PR that cannot land without one.

## What the number measures

`tools/first-load.mjs` takes the payload as **everything under `engines/`** plus the heaviest
landing route's markup and scripts. So the total is the cost of **completing one operation**,
not the cost of a visit.

That distinction settles a question that has been open since B3 and was never written down:
**should engine load wait until a file is picked?** It already does. `tool-host.ts`'s
`ensure()` is called from the file-pick path in all three islands, so a person who lands on
`/rotate-pdf` and leaves downloads the page and none of the engines. B3 is **answered**, not
deferred, and it is not one of the options below.

## Where the bytes were

| artifact | brotli | share |
|---|--:|--:|
| `engines/pdfium.wasm` | 1,904,807 | **79.7%** |
| `engines/qpdf.wasm` | 327,629 | 13.7% |
| `engines/burrow-worker.js` | 56,936 | 2.4% |
| `engines/burrow_wasm_bg.wasm` | 56,159 | 2.4% |
| `page` + `page-js` | 43,742 | 1.8% |

Four fifths of the payload is PDFium, and **the web build called into it from one place**:
`page_count`. `bindings/burrow-wasm/src/lib.rs` names `pdfium()` three times — the factory,
the heap-lifecycle read, and `page_count`'s `open`. `merge`, `rotate`, `reorder`, `split`,
`page_rotations` and `structure_check` are all qpdf. `PdfiumBridge` has six methods — copy in,
load, count, close, free, heap — so there is **no rendering entry point**, and thumbnails
(#57) are not a hidden dependency on it.

qpdf already answers the page-count question: `qpdf_get_num_pages` is exported and `WebQpdf`
implements `StructureEngine::page_count`.

## The decision

**PDFium does not ship to the web.** `page_count` moves to qpdf; `pdfium.wasm` and the
`pdfium.js` glue leave the payload. Not a per-page or per-engine loading scheme — there is no
split to build, no change to the init memoisation that spike 0001's HIGH finding put there,
and no second instance made reachable. Per-page loading stays on the shelf unless `compress`
proves it needs PDFium for image recompression, at which point it becomes a live option again
for the tools that do not.

**The total budget is not raised.**

### The measured new total

`pdfium.wasm` is exact; the glue was measured by recompressing the worker bundle both ways.

| | brotli |
|---|--:|
| today | 2,389,341 |
| − `engines/pdfium.wasm` | −1,904,807 |
| − `pdfium.js` from the worker bundle | −23,640 |
| **new total** | **460,894** |

**−80.7%.** Against the unchanged 2,403,482 budget that is 421% headroom. First-operation
download at 400 kbps (Chrome's Slow 3G, which `src/host/start-up-bound.test.ts` already
calibrates against): **47.8 s → 9.2 s**; at 1.6 Mbps, 11.9 s → 2.3 s.

Two further reductions are not counted and both run the same way: dropping `WebPdfium` and
`PdfiumBridge` shrinks `burrow_wasm_bg.wasm`, and qpdf needs **no new export**.

### What a raise would have bought instead

3% of the current total is **71,680 bytes**: +1.4 s at 400 kbps, +0.4 s at 1.6 Mbps. That is
the honest cost of the alternative and it is small. The reason not to take it is not the
1.4 s — it is that every previous raise restored a stated margin after a *named* increase,
and a raise taken in advance of an engine nobody has measured would be the first taken to
make room rather than to record something. If option 1 lands, a raise is not needed. If it
does not and `compress` needs PDFium plus a codec, a raise would not have been enough either.

### What `compress` might need, unmeasured and said so

- **hb-subset** — HarfBuzz is already linked inside PDFium (`engines/licenses.toml` records
  1192 `hb_*` symbols; ADR 0010 resolved its licence), but the *subsetting* library is a
  separate target PDFium does not build. New code of unknown size. It is listed in
  `docs/ROADMAP.md` under M2's redaction risks, not under compress.
- **jbig2enc** — not pinned, not licence-audited, not mentioned anywhere in `docs/`. Its
  licence needs an ADR under the allowlist rule before its size matters.

Neither has a measured wasm size here and this document does not invent one.

## The two consequences, both handled rather than noted

### 1. `max_memory_bytes` lost its only pre-allocation check on the web — so qpdf gained one

`estimate::check_open_memory`, the length-based estimate that fires *before* the engine
allocates, was called from exactly two places: `pdfium/mod.rs` and `web/pdfium.rs`. No qpdf
path called it. That is issue #26, and removing PDFium from the web would have turned "only
the PDFium path has it" into "no web path has it".

**So the estimate now runs on both engines**, at nine qpdf open sites plus `web/qpdf.rs`.
`check_measured_memory` was already everywhere and is unchanged.

`web/qpdf.rs` carried a comment arguing the opposite — that PDFium's constants "would reject
files qpdf handles comfortably" — and an earlier version of that function had called the
estimate and had it removed. Reasonable, unverified, and **measurably wrong**.
`examples/measure-open-cost.rs`, one open per process because resident set is a high-water
mark:

| file | bytes | qpdf peak | PDFium peak | estimate |
|---|--:|--:|--:|--:|
| `pages-10.pdf` | 1,388 | 1.28 MB | 1.16 MB | 16.78 MB |
| 1 MB, 5,480 pages | 1,081,041 | 12.28 MB | 2.91 MB | 18.13 MB |
| 1 MB, 9,000 pages | 1,096,125 | **19.30 MB** | 3.39 MB | 18.15 MB |

qpdf is the hungrier engine — 5.7× PDFium on the last row — and its cost tracks **page count**
where the estimate tracks length. On that row it **exceeds** the estimate, on a file PDFium
opens in a fifth of it. The estimate is not too strict for qpdf; it is too lenient, which is
#26's successor rather than an argument for leaving it out.

**The first version of this table was measured wrongly and the error is worth recording.** It
read `VmRSS`, which is the *current* resident set and falls when memory is freed — measured,
521,724 kB down to 9,720 kB across one `free`. The PDFium branch held its document open across
the reading while the qpdf branch sampled after `StructureEngine::check` had already dropped
its own, so the two columns were taken at different points. The conclusion survived only
because the under-measured engine was the one that read higher. Code review caught it; the
figures above read `VmHWM`, a genuine high-water mark, so the sampling point stops mattering
and both branches are written the same way.

`tests/limits.rs::both_engines_refuse_at_the_same_estimate_with_the_same_reason` walks the
boundary from both sides: one byte below the estimate both refuse with the same `limit`,
`stage`, `requested` and `allowed`; at exactly the estimate both accept. The second half
matters — without it the test would pass against an engine that refused everything.

### 2. The page-count answer changes on the damaged corpus — and it is not the #61 shape

The corpus already ran both engines over every fixture (`page_count` on PDFium,
`structure_check` on qpdf, both recording a page count). **22 cases declare both.** Before this
change they agreed on 16 and differed on 6; `size-estimate-refuses-an-ordinary-file` converged
when the estimate went onto the qpdf paths, so it is now **17 agree, 5 differ**.

All five were checked for [#61](https://github.com/TensorGreed/burrow/issues/61)'s shape — an
optimistic count followed by a write that quietly comes back short — because an accepted
difference is fine and a silently short document is not. The first draft of this section said
"the remaining four", which read as an exhaustive enumeration and was not one: it omitted
`objstm-bomb-tight-ceiling`, which is also an optimistic-count direction. Code review caught
that.

**None of the five is that shape**, and two of them are not engine differences at all:

| fixture | PDFium | qpdf | measured |
|---|---|---|---|
| `truncated-mid-object.pdf` | Malformed | Ok, 1 | qpdf counts **only with recovery on**; with it off, `Malformed` |
| `trailer-removed.pdf` | Malformed | Ok, 3 | same |
| `object-number-above-int-max.pdf` | Ok, 1 | Malformed | a real difference, in the **stricter** direction |
| `canary.pdf` | Ok, 1 | Malformed | same |
| `objstm-bomb-tight-ceiling` | LimitExceeded (`measured`) | Ok, 1 | safe twice over — see below |

`attempt_recovery` lives on `CheckOptions` and **not** on `OpenOptions`, so no operation opens
with recovery. The optimistic count exists only under a posture a structural check can ask for
and no write path uses: rotating either file by zero degrees returns
`Malformed("qpdf: the document is damaged")`. At equal posture the two engines **agree**.

The next two are genuine and safe: qpdf refuses what PDFium accepts, so the web becomes
stricter, and a refusal cannot deliver a document short of a page.

`objstm-bomb-tight-ceiling` is safe for two independent reasons, and both are asserted rather
than argued. The write path refuses it; and on the **web** — the only platform the `page_count`
move affects — `expectations.json` already carries a `platform_expectations` entry making
`structure_check` refuse at `measured` as well, because WASM linear memory never shrinks, so
the web reading sees the peak qpdf inflates and frees where the native one does not.

`core/burrow-ops/tests/optimistic_counts.rs` asserts all of it, including the direction — so
the day recovery-off stops refusing, or the stricter engine stops being the stricter one, the
question gets asked again instead of the entry continuing to say "engines differ".

## What the implementation still has to do

This spike closed #26 and settled the two consequences. **PDFium has not yet been removed from
the web payload** — that is its own change, and it is mechanical:

- `worker/prelude.js` and `stage-web-engines.mjs`: `pdfium.js` leaves the bundle, and
  `self.Module` and the load-order comment go with it
- `EXPECTED_ENGINE_MODULES` 3 → 2, and `start-up-bound.test.ts`'s calibration moves to
  `qpdf.wasm` as the largest module
- `e2e/integrity.spec.ts`: the id list loses `pdfiumWasm`; two pinned symbols go
- `generated/engines.js`: one fewer `connect-src`, hashes regenerate
- `harness-driver.js`: the hang injection wraps `__burrow_pdfium_copy_in` and must wrap a qpdf
  call instead
- `engines/licenses.toml`: **25 lines name the artifact id `pdfium-wasm`**, in `artifacts` and
  `linked_in` across ten components, under a heading that says "shipped artifacts".
  ~~`check-engine-licences.py` enforces the consistency, so this is bounded work~~

  **AMENDED WHEN THE WORK WAS DONE — it did not.** The script never read `artifacts` or
  `linked_in` at all, so deleting the `[[artifact]]` block while leaving fourteen components
  referring to it (or the reverse) would have passed silently. The bounding came from `grep`.
  The consistency is enforced now, because the fix is to build the control rather than to
  note the number: three rules, both directions of the artifact-id cross-check plus
  `linked_in ⊆ artifacts` per component, each with its own planted-manifest probe in
  `tools/test-check-engine-licences.sh`.
- ~~the credits page needs **no** change: it is generated from `linked`, which stays true
  because native still links PDFium~~

  **AMENDED: the page's DATA moves.** The obligation is unaffected and `linked` does stay
  true, so the direction is safe — but `generate-credits.mjs` publishes each component's
  `artifacts` list, so the rendered page changes with those 25 lines. The residual is worth
  stating rather than leaving as "no change": the page is generated from the whole manifest,
  so it now over-declares **for the web** — it credits FreeType, HarfBuzz, ICU, lcms and
  OpenJPEG to a reader whose download contains none of them, because those obligations arrive
  through PDFium and PDFium is still linked natively and on mobile. Over-declaring breaches
  nothing. It does loosen the page's own claim that it cannot fall behind what we ship, in the
  other direction, and an `artifacts`-aware filter is the answer if that matters later. Raised
  by security review; the licence surface is *stop and ask*, so it is recorded here rather
  than changed
- `detect-engine-components.py` is unaffected as long as pdfium is still **built** for wasm
  and merely not staged
