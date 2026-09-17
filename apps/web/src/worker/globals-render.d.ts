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
}
