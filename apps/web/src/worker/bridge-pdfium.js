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
