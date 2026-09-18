---
name: m1-render-capability
description: "#57 piece 2a (render): measured cost of one FPDF_RenderPageBitmap call, the measurement trap that inflates it, and the wasm-bindgen accessor names the worker gets wrong."
metadata:
  type: project
---

Measured 2026-09-16 on the working tree of branch `render-and-thumbnails`, aarch64, against
the vendored `engines/vendor/native-aarch64/lib/libpdfium.so`.

**ONE `FPDF_RenderPageBitmap` CALL IS UNBOUNDED IN BOTH TIME AND ENGINE MEMORY, and
`max_pixels` does not touch it.** `max_pixels` bounds the *output* raster; the rasteriser's
working set is bounded by nothing in `Limits`. Rendering into a 240x320 thumbnail box
(0.077 Mpx, 1/54th of the web ceiling) under `max_duration_ms = 12_000`,
`max_memory_bytes = 1 GiB`:

| input | content | wall | peak RSS | outcome |
|--:|---|--:|--:|---|
| 403 kB | 3 M stroked paths in one content stream | **34.0 s** | **819 MB** | `LimitExceeded{max_duration_ms}` *after* the work |
| 1.34 MB | 10 M stroked paths | **112.4 s** | **2717 MB** | same |

Linear in the operation count, so ~13 MB of input (`max_input_bytes` default is 512 MiB)
projects to ~19 min and ~27 GB. The deadline is checkpointed *before and after* the engine
call only, and **no `estimate::check_measured_memory` runs on the render path at all** — the
only call site in `pdfium/mod.rs` is inside `open` (line ~202). So on render, `max_memory_bytes`
is not even *detected*, which is weaker than the "detected, never bounded" the module's own
limits table claims.

**A THUMBNAIL-SIZED FLATE IMAGE BOMB DOES *NOT* BLOW UP.** 20000x20000 RGB8, 30000x30000
gray8, 30000x30000 1-bit, two 20000x20000 images on one page: all peak at **14-30 MB**.
PDFium decodes flate images scanline-wise into the downsampled destination. Above roughly
20000x20000 RGB it refuses the image and renders the page blank, in single-digit ms. So the
memory story on this path is the *content stream*, not the image.

**THE MEASUREMENT TRAP, worth more than the number.** The first run of that image sweep
reported 1152 MB / 865 MB / 112 MB and looked like a decisive finding. It was the *generator*:
the probe built the raw image as `vec![0x80; w*h*comps]` before deflating it, and
`VmHWM` is process-wide. The three figures matched the raw buffer sizes exactly. Fix is to
stream the deflate a row at a time **and print `VmHWM` immediately before the operation**, not
just `VmRSS` — a peak-so-far line is what separates the engine's cost from the harness's.

**`RenderedPage`'s two pixel accessors are named `pixel_length` and `take_pixels` in the
generated binding, and `render-main.js` reads `pixelLength` / `takePixels()`.**
`bindings/burrow-wasm/src/render.rs` omits `js_name` on those two members where `Reply` uses it
(`lib.rs` `js_name = outputLength` / `takeOutput`). Because the read is
`drawn.pixelLength > 0 ? drawn.takePixels() : new Uint8Array(0)`, `undefined > 0` is false, the
taker is never called, no TypeError is thrown, and **every page posts a zero-byte buffer with
`ok: true`**. `apps/web/src/worker/globals-render.d.ts` declares the camelCase names, so `tsc`
checks the worker against a hand-written declaration that disagrees with
`pkg-render/burrow_wasm.d.ts`; nothing derives one from the other. That missing derivation is
the control, not the rename.

**What held up under review** (so a later reviewer need not redo it): page handle never leaves
the engine-thread closure and is closed on every `?` path on both platforms; bitmap destroyed on
every path; `FPDFBitmap_Create(alpha=1)` + `FillRect` before render, and `bgra_to_rgba` drops the
row padding, so no uninitialised engine heap leaves the worker; `__burrow_pdfium_copy_out` uses
`slice` and the generated `take_pixels` glue also `.slice()`s, so the transferred `ArrayBuffer`
is never a view on wasm memory; none of the eleven bridge methods branches.

Related: [[m1-render-worker-bundle]], [[m1-limits-real-strength]], [[m1-web-engine-path]].
