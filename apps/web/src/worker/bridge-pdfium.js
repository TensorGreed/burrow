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
self.__burrow_pdfium_page_width = (page) => pdfium()._FPDF_GetPageWidthF(page);
self.__burrow_pdfium_page_height = (page) => pdfium()._FPDF_GetPageHeightF(page);

self.__burrow_pdfium_bitmap_create = (width, height, alpha) =>
  pdfium()._FPDFBitmap_Create(width, height, alpha);

self.__burrow_pdfium_bitmap_fill_rect = (bitmap, left, top, width, height, color) =>
  pdfium()._FPDFBitmap_FillRect(bitmap, left, top, width, height, color);

self.__burrow_pdfium_render_page_bitmap = (
  bitmap,
  page,
  startX,
  startY,
  sizeX,
  sizeY,
  rotate,
  flags,
) => pdfium()._FPDF_RenderPageBitmap(bitmap, page, startX, startY, sizeX, sizeY, rotate, flags);

self.__burrow_pdfium_bitmap_buffer = (bitmap) => pdfium()._FPDFBitmap_GetBuffer(bitmap);
self.__burrow_pdfium_bitmap_stride = (bitmap) => pdfium()._FPDFBitmap_GetStride(bitmap);
self.__burrow_pdfium_bitmap_destroy = (bitmap) => pdfium()._FPDFBitmap_Destroy(bitmap);

// `slice`, not `subarray`, and the difference is the whole of it: `subarray` is a VIEW into
// the engine heap, and wasm memory growth detaches every view of it. The copy is what makes
// these bytes the caller's. The same rule `__burrow_qpdf_copy_out` follows.
self.__burrow_pdfium_copy_out = (ptr, len) => pdfium().HEAPU8.slice(ptr, ptr + len);
