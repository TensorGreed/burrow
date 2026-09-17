// The PDFium half of the engine bridge: the `__burrow_pdfium_*` globals the render Rust
// module imports. Bundled into `burrow-render-worker.js` only.
//
// The rules this file follows, and the two decisions behind the shapes it uses, are in
// `bridge-common.js`, which is bundled immediately before it and supplies `u32`, `copyInto`,
// `wipeAndFree` and `heapPages`.
//
// THIS FILE CAME BACK WITH ITS ARTIFACT. Spike 0004 deleted these globals along with
// `pdfium.wasm`, because a `no-modules` wasm-bindgen import resolves from this scope and a
// declared import with no global fails at *instantiation* — in the browser and nowhere else.
// That is still true and it is now the mechanism: the base bundle has neither the globals nor
// the imports, so there is nothing to mismatch. What is here is exactly what left
// (`git show 1d9546a^`), because a re-derivation is where the packed load result below would
// quietly have become two calls again.

/** @type {EmscriptenModule | null} The PDFium Emscripten module. */
let pdfiumModule = null;

/**
 * Wire the bridge to the initialised module. Called once, by the worker.
 *
 * @param {EmscriptenModule} pdfium
 */
function __burrow_attach(pdfium) {
  pdfiumModule = pdfium;
}

/**
 * The PDFium module, or a loud failure.
 *
 * Not a branch on engine state — it cannot be, because no engine has spoken yet. It turns
 * "the bridge was called before init finished" into a thrown error instead of
 * `undefined._malloc`, which the worker would report as an opaque `Internal` with no clue
 * as to the cause.
 *
 * @returns {EmscriptenModule}
 */
function pdfium() {
  if (!pdfiumModule) {
    throw new Error("bridge used before attach");
  }
  return pdfiumModule;
}

// --- PDFium ---------------------------------------------------------------------------

self.__burrow_pdfium_copy_in = (bytes) => copyInto(pdfium(), bytes);
self.__burrow_pdfium_wipe_free = (ptr, len) => wipeAndFree(pdfium(), ptr, len);
self.__burrow_pdfium_free_input = (ptr, len) => wipeAndFree(pdfium(), ptr, len);

self.__burrow_pdfium_load = (data, len, password) => {
  const doc = pdfium()._FPDF_LoadMemDocument64(data, len, password);
  // Read immediately, before any other PDFium call can overwrite the global.
  const code = pdfium()._FPDF_GetLastError();
  return (BigInt(u32(code)) << 32n) | BigInt(u32(doc));
};

self.__burrow_pdfium_pages = (doc) => pdfium()._FPDF_GetPageCount(doc);

self.__burrow_pdfium_close = (doc, data, len) => {
  // Close, then wipe and free. PDFium reads from the input buffer for as long as the document
  // is open (fpdfview.h:451), so the other order is a use-after-free. Two statements in one
  // function is the only reason no call site can get it wrong.
  pdfium()._FPDF_CloseDocument(doc);
  wipeAndFree(pdfium(), data, len);
};

self.__burrow_pdfium_heap_pages = () => heapPages(pdfium());

// --- rendering (#57) ------------------------------------------------------------------
//
// Eleven forwards, one per Emscripten export, and not one of them decides anything. The
// nulls are checked in `burrow-engines`, the stride arithmetic is there, and the bitmap
// format, the fill colour, the flags and the zero rotation all arrive from Rust as numbers.
// ADR 0009 §2, and ADR 0027 for what the numbers mean.
//
// THE ONE-CALL SHORTCUT IS REFUSED ON PURPOSE. A single `__burrow_pdfium_render(doc, index,
// w, h)` would be one boundary crossing instead of nine -- load, create, fill, render, buffer,
// stride, copy out, destroy, close -- and it would put the cleanup order,
// the null checks and the stride arithmetic in the one place `cargo test` cannot reach and
// iOS and Android cannot reuse.

self.__burrow_pdfium_load_page = (doc, index) => pdfium()._FPDF_LoadPage(doc, index);
self.__burrow_pdfium_close_page = (page) => pdfium()._FPDF_ClosePage(page);
// A PAIR, and the only one on this boundary. `FS_SIZEF` is one out-parameter of one C call;
// two forwards would mean asking the engine twice for one answer. The struct is written into
// the C stack, which lives for exactly this call.
//
// IT DOES NOT LOAD THE PAGE, which is the entire point: reading a `/MediaBox` through
// `_FPDF_LoadPage` builds the display list -- the dominant cost of a strip -- and a strip then
// paid it again in the render. Undefined on failure: deciding what a failure means is Rust's.
self.__burrow_pdfium_page_size_by_index = (doc, index) => {
  const module = pdfium();
  // `_malloc`, NOT `stackAlloc`. THE C STACK HELPERS ARE NOT ON THIS MODULE: `stackSave`,
  // `stackAlloc` and `stackRestore` are exported by the qpdf build and not by the vendored
  // `pdfium.wasm` -- checked in the artifact, 0 occurrences against qpdf's 3. They were
  // declared on the SHARED `EmscriptenModule` type, so calling them here type-checked and
  // threw in the browser: every render came back `Internal`, the worker was discarded as
  // poisoned, and the only place it showed was the differential corpus.
  //
  // That is the hazard `globals.d.ts` already records about `_FPDF_*` and `_qpdf_*` -- "a call
  // that TYPE-CHECKS and then fails at run time in the browser and nowhere else" -- one level
  // up, in a member the two modules do not share. The declarations have moved to
  // `globals-qpdf.d.ts` so this cannot be written again.
  const size = module._malloc(8);
  if (!size) return undefined;
  try {
    if (!module._FPDF_GetPageSizeByIndexF(doc, index, size)) return undefined;
    // HEAPF32 is re-read after the allocation, which can grow memory and detach every view.
    const floats = module.HEAPF32;
    return [floats[size / 4], floats[size / 4 + 1]];
  } finally {
    module._free(size);
  }
};

self.__burrow_pdfium_bitmap_create = (width, height, alpha) =>
  pdfium()._FPDFBitmap_Create(width, height, alpha);

self.__burrow_pdfium_bitmap_fill_rect = (bitmap, left, top, width, height, color) =>
  pdfium()._FPDFBitmap_FillRect(bitmap, left, top, width, height, color);

// --- progressive rendering (ADR 0027's 2026-09-17 amendment) ---------------------------
//
// The one-shot `_FPDF_RenderPageBitmap` forward is GONE rather than kept beside these: ADR 0009
// §2 makes the bridge list the audit surface, so a forward nothing calls is a capability the
// binding still has.
//
// `IFSDK_PAUSE` is a C struct with a FUNCTION POINTER in it, which is the only thing on this
// boundary that cannot be a number. Building it needs `addFunction`, and `addFunction` needs a
// growable function table -- Emscripten emits one only with `ALLOW_TABLE_GROWTH`, and we vendor
// `pdfium.wasm` rather than building it. **Its table declares `min=3299` with no maximum**, so
// it grows; checked by reading the artifact's table section, not assumed.
//
// THE CALLBACK IS A CONSTANT, NOT A DECISION. `() => 1` means "pause now", every time, so
// PDFium completes one slice and returns and the Rust loop decides whether to continue. The
// native path holds the identical constant in Rust. What crosses here is a struct layout and a
// table entry -- ABI, not policy.

/** The struct is three 32-bit fields on wasm32: `int version`, a function pointer, `void* user`. */
const IFSDK_PAUSE_BYTES = 12;

/**
 * The always-pause callback's function-table index, created once for the worker's life.
 *
 * ONE TABLE ENTRY, NOT ONE PER RENDER, and the reason is not only tidiness. `addFunction` on a
 * fresh arrow takes Emscripten's `convertJsFunctionToWasm` path, which compiles a small
 * `WebAssembly.Module` -- per rendered page, for a callback that returns a constant. Hoisting
 * the arrow instead would be WRONG: `getFunctionAddress` caches it in a WeakMap, so after the
 * first `removeFunction` a second `addFunction` returns an index whose table slot is null and
 * PDFium calls through it. Creating the entry once and never removing it avoids both.
 *
 * @type {number | null}
 */
let pauseCallback = null;

self.__burrow_pdfium_pause_create = () => {
  const module = pdfium();
  // The table entry first: if this throws there is nothing to free. `"ii"` is the Emscripten
  // signature for `int f(int)` -- `FPDF_BOOL NeedToPauseNow(IFSDK_PAUSE*)`: one return, one
  // argument. (The comment here said `"i"`, which is the signature for `int f(void)` and would
  // have talked the next reader into breaking the ABI. Found by code review.)
  if (pauseCallback === null) {
    try {
      pauseCallback = module.addFunction(() => 1, "ii");
    } catch {
      return 0;
    }
  }
  const pause = module._malloc(IFSDK_PAUSE_BYTES);
  if (!pause) {
    return 0;
  }
  // HEAPU32 IS RE-READ HERE, not captured: an allocation can grow the module's memory, which
  // detaches every existing view of it. The same rule `bridge-common.js` states for copies.
  const words = module.HEAPU32;
  words[pause / 4] = 1; // version, which `fpdf_progressive.h:27` says must be 1
  words[pause / 4 + 1] = pauseCallback;
  words[pause / 4 + 2] = 0; // user, "can be NULL"
  return pause;
};

self.__burrow_pdfium_pause_destroy = (pause) => {
  // THE ALLOCATION ONLY. The table entry is the worker's, created once and kept -- see
  // `pauseCallback`. This used to read the index back out of `HEAPU32`, i.e. out of memory the
  // engine can write, and hand it to `removeFunction`: a wrong index nulls a live table slot or
  // throws a RangeError that unwinds through wasm, leaking the page and the bitmap. Nothing was
  // found that writes there, so it was hardening rather than a defect. Found by security review.
  pdfium()._free(pause);
};

self.__burrow_pdfium_render_page_start = (
  bitmap,
  page,
  startX,
  startY,
  sizeX,
  sizeY,
  rotate,
  flags,
  pause,
) =>
  pdfium()._FPDF_RenderPageBitmap_Start(
    bitmap,
    page,
    startX,
    startY,
    sizeX,
    sizeY,
    rotate,
    flags,
    pause,
  );

self.__burrow_pdfium_render_page_continue = (page, pause) =>
  pdfium()._FPDF_RenderPage_Continue(page, pause);

self.__burrow_pdfium_render_page_close = (page) => pdfium()._FPDF_RenderPage_Close(page);

self.__burrow_pdfium_bitmap_buffer = (bitmap) => pdfium()._FPDFBitmap_GetBuffer(bitmap);
self.__burrow_pdfium_bitmap_stride = (bitmap) => pdfium()._FPDFBitmap_GetStride(bitmap);
self.__burrow_pdfium_bitmap_destroy = (bitmap) => pdfium()._FPDFBitmap_Destroy(bitmap);

// `slice`, not `subarray`, and the difference is the whole of it: `subarray` is a VIEW into
// the engine heap, and wasm memory growth detaches every view of it. The copy is what makes
// these bytes the caller's. The same rule `__burrow_qpdf_copy_out` follows.
self.__burrow_pdfium_copy_out = (ptr, len) => pdfium().HEAPU8.slice(ptr, ptr + len);
