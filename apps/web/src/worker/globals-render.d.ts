// The render bundle's half of the worker's ambient declarations.
//
// Included by `tsconfig.render.json` and NOT by `tsconfig.json`. See `globals-qpdf.d.ts` for
// why the split exists rather than one file declaring both engines: a declaration that
// outlives the module it describes type-checks and then fails at run time in the browser and
// nowhere else, which is measured history here rather than a worry.
//
// Everything here MERGES into the interfaces `globals.d.ts` declares.

/** The wasm modules the render bundle fetches. Generated into it as `BURROW_ENGINE_MODULE_IDS`. */
type EngineModuleId = "pdfiumWasm" | "burrowRenderWasm";

interface BurrowEngines {
  pdfiumWasm: EngineEntry;
  burrowRenderWasm: EngineEntry;
}

interface EmscriptenModule {
  // Exactly the `fpdfview.h` declarations `core/burrow-engines/src/pdfium/ffi.rs` mirrors.
  //
  // `FPDF_LoadMemDocument64` is the 64-bit variant, as on native: a signed length turns an
  // input above 2 GiB into a huge out-of-bounds read.
  _FPDF_InitLibrary(): void;
  _FPDF_LoadMemDocument64(data: number, size: number, password: number): number;
  _FPDF_GetPageCount(doc: number): number;
  _FPDF_CloseDocument(doc: number): void;
  /**
   * The LAST error, from a process-global slot the next PDFium call overwrites.
   *
   * Which is why `__burrow_pdfium_load` packs it with the handle and returns both in one
   * call: fetching it in a second round trip could attach a different operation's error.
   */
  _FPDF_GetLastError(): number;

  // --- rendering (#57) --------------------------------------------------------------
  //
  // The ten `fpdfview.h` declarations `core/burrow-engines/src/pdfium/ffi.rs` mirrors for the
  // render path. Every one is already exported by the prebuilt `pdfium.wasm`: ADR 0020
  // measured 429 `FPDF_*` exports and no `EXPORTED_FUNCTIONS` allowlist, which is why
  // rendering costs no engine rebuild and no new content hash.
  _FPDF_LoadPage(doc: number, index: number): number;
  _FPDF_ClosePage(page: number): void;
  /** `fpdfview.h:793`. Writes an `FS_SIZEF` (two floats) at `size`. Non-zero for success. */
  _FPDF_GetPageSizeByIndexF(doc: number, index: number, size: number): number;
  _FPDFBitmap_Create(width: number, height: number, alpha: number): number;
  _FPDFBitmap_FillRect(
    bitmap: number,
    left: number,
    top: number,
    width: number,
    height: number,
    color: number,
  ): void;
  /** `fpdf_progressive.h:117`. Returns an `FPDF_RENDER_*` state. */
  _FPDF_RenderPageBitmap_Start(
    bitmap: number,
    page: number,
    startX: number,
    startY: number,
    sizeX: number,
    sizeY: number,
    rotate: number,
    flags: number,
    pause: number,
  ): number;
  /** `fpdf_progressive.h:138`. */
  _FPDF_RenderPage_Continue(page: number, pause: number): number;
  /** `fpdf_progressive.h:149`. Called on EVERY exit, including the refusals. */
  _FPDF_RenderPage_Close(page: number): void;
  _FPDFBitmap_GetBuffer(bitmap: number): number;
  /** Bytes per row, which PDFium MAY pad. Never assume `width * 4`. */
  _FPDFBitmap_GetStride(bitmap: number): number;
  _FPDFBitmap_Destroy(bitmap: number): void;

  // --- redaction's second reading (#206) ----------------------------------------------
  //
  // `HEAPF64` HERE, NOT IN `globals.d.ts`: the vendored `pdfium.js` exports it, checked; the
  // qpdf glue is not asserted to, and a member declared on the shared type for both modules is
  // the hazard the `HEAPF32` note in `bridge-pdfium.js` records.
  /** The module's memory as 64-bit floats, for reading `FPDFText_GetCharOrigin`'s doubles. */
  HEAPF64: Float64Array;
  //
  // The five `fpdf_text.h` declarations `core/burrow-engines/src/pdfium/ffi.rs` mirrors, and no
  // other: where characters are, never what they say. All five are exported by the prebuilt
  // `pdfium.wasm`, checked in the vendored glue.
  _FPDFText_LoadPage(page: number): number;
  _FPDFText_CountChars(text: number): number;
  /** `fpdf_text.h`. Writes two doubles at `x` and `y`. Non-zero for success. */
  _FPDFText_GetCharOrigin(text: number, index: number, x: number, y: number): number;
  /** 1, 0, or -1 for an error. */
  _FPDFText_IsGenerated(text: number, index: number): number;
  _FPDFText_ClosePage(text: number): void;
}

// `render-prelude.js` DEFINES `pdfiumReady` and `resolvePdfium`, so they are not declared
// here -- redeclaring a `const` that a checked file also declares is an error, and the
// definitions carry their own JSDoc. `globals.d.ts` says the same of `prelude.js`'s consts.

interface WorkerGlobalScope {
  /**
   * PDFium's glue reads this at load time, so `render-prelude.js` assigns it BEFORE the glue
   * is concatenated. It is an {@link EmscriptenConfig} at that moment and the glue populates
   * the same object with the exports — see `EmscriptenConfig`'s own note on the narrowing.
   *
   * It exists in THIS project only. In the base bundle an unused `Module` global would be
   * picked up as its configuration by the next Emscripten glue added there, which is why
   * spike 0004 deleted the assignment rather than leaving it harmlessly in place.
   */
  Module: EmscriptenConfig & Partial<EmscriptenModule>;

  // MIRRORS `PdfiumBridge` in `core/burrow-engines/src/web/bridge.rs`, which is the audit
  // surface ADR 0009 relies on: every capability the binding has is one entry there and one
  // entry here. A name in one and not the other is a mismatch worth noticing.
  __burrow_pdfium_copy_in(bytes: Uint8Array): number;
  __burrow_pdfium_wipe_free(ptr: number, len: number): void;
  __burrow_pdfium_free_input(ptr: number, len: number): void;
  /** `(code << 32) | handle`. Both halves in one call; see `bridge-pdfium.js`. */
  __burrow_pdfium_load(data: number, len: number, password: number): bigint;
  __burrow_pdfium_pages(doc: number): number;
  __burrow_pdfium_close(doc: number, data: number, len: number): void;
  __burrow_pdfium_heap_pages(): number;

  // The render half. Eleven names, one per Emscripten export, mirroring the eleven methods
  // #57 added to `PdfiumBridge`.
  __burrow_pdfium_load_page(doc: number, index: number): number;
  __burrow_pdfium_close_page(page: number): void;
  /** Points, without loading the page. `undefined` when PDFium reports failure. */
  __burrow_pdfium_page_size_by_index(doc: number, index: number): [number, number] | undefined;
  __burrow_pdfium_bitmap_create(width: number, height: number, alpha: number): number;
  __burrow_pdfium_bitmap_fill_rect(
    bitmap: number,
    left: number,
    top: number,
    width: number,
    height: number,
    color: number,
  ): void;
  /** Allocate an `IFSDK_PAUSE` whose callback always pauses. 0 if it could not be made. */
  __burrow_pdfium_pause_create(): number;
  /** Free it AND release its function-table entry -- the half that leaks quietly. */
  __burrow_pdfium_pause_destroy(pause: number): void;
  __burrow_pdfium_render_page_start(
    bitmap: number,
    page: number,
    startX: number,
    startY: number,
    sizeX: number,
    sizeY: number,
    rotate: number,
    flags: number,
    pause: number,
  ): number;
  __burrow_pdfium_render_page_continue(page: number, pause: number): number;
  __burrow_pdfium_render_page_close(page: number): void;
  __burrow_pdfium_bitmap_buffer(bitmap: number): number;
  __burrow_pdfium_bitmap_stride(bitmap: number): number;
  __burrow_pdfium_bitmap_destroy(bitmap: number): void;
  /** A COPY, not a view: wasm memory growth detaches every view of the engine heap. */
  __burrow_pdfium_copy_out(ptr: number, len: number): Uint8Array;

  // --- redaction's second reading (#206): five forwards, no other text function ---------
  __burrow_pdfium_text_load_page(page: number): number;
  __burrow_pdfium_text_count_chars(text: number): number;
  /** `[x, y]`, or `undefined` when PDFium reports failure. */
  __burrow_pdfium_text_char_origin(text: number, index: number): number[] | undefined;
  __burrow_pdfium_text_is_generated(text: number, index: number): number;
  __burrow_pdfium_text_close_page(text: number): void;
}

/** The operations the render artifact exports. */
interface BurrowWasm {
  /** Open with **PDFium** and report the page count. The base artifact's is qpdf's. */
  page_count(bytes: Uint8Array, password: Uint8Array | undefined, limits: WebLimits): Reply;
  /**
   * ADR 0023's shape for ADR 0027 §2's reason: a strip in progress, pulled one page at a time,
   * so the engine heap holds at most one bitmap however long the strip is.
   *
   * `boxWidth` x `boxHeight` is the box each page is fitted INSIDE, keeping its proportions —
   * so what comes back is usually smaller in one dimension, which is why every
   * {@link RenderedPage} carries its own size. The aspect-ratio arithmetic is in Rust
   * (ADR 0009 §2), and `max_pixels` is checked against the box before the document is opened.
   */
  render_begin(
    bytes: Uint8Array,
    pages: Uint32Array,
    boxWidth: number,
    boxHeight: number,
    password: Uint8Array | undefined,
    limits: WebLimits,
  ): RenderSession;
  /**
   * Redaction's second reading (#206): the answer, encoded -- a recording bound to the two
   * digests passed in, or a typed failure carrying no text. No verdict is reached here.
   */
  secondReading(
    input: Uint8Array,
    output: Uint8Array,
    page: number,
    inputDigest: Uint8Array,
    outputDigest: Uint8Array,
    password: Uint8Array | undefined,
    limits: WebLimits,
  ): Uint8Array;
}

/**
 * A strip in progress. `pages` is known before the first page is; `next_page` draws one.
 *
 * A reply with `ok === false` ends the strip and does **not** invalidate the pages already
 * delivered — the opposite of `SplitSession`'s rule, because a strip is a set of independent
 * pictures rather than a partition (ADR 0027).
 */
interface RenderSession {
  readonly ok: boolean;
  readonly pages: number;
  begin_reply(): Reply;
  next_page(): RenderedPage;
  free(): void;
}

/** One page, drawn. Returned owned, so it MUST be freed. */
interface RenderedPage {
  reply(): Reply;
  /** The ONE-BASED page number these pixels are of. Carried, never inferred from arrival order. */
  readonly page: bigint;
  readonly width: number;
  readonly height: number;
  /** Read before taking: "no pixels" and "already taken" both come back empty. */
  readonly pixelLength: number;
  /** MOVES the pixels out. A getter would copy them, and this is the largest thing the boundary carries. */
  takePixels(): Uint8Array;
  free(): void;
}
