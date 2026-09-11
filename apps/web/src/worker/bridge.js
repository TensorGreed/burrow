// The JavaScript half of the engine bridge: `__burrow_*` globals the Rust module imports.
//
// WHAT IS ALLOWED IN THIS FILE
//
// ADR 0009: "Bindings may hold engine handles and marshal data across the boundary. No
// branch on engine state may live in JS."
//
// So every function here does one of exactly three things: allocate in an engine heap, copy
// bytes, or forward one call to one Emscripten export. There is no `if` anywhere that looks
// at what an engine returned. Nothing here interprets an error code, chooses an engine,
// decides whether to retry, loops over pages, or compares anything against a limit -- all of
// that is Rust, in `burrow-engines`, and it is the same Rust the native path runs.
//
// The trait definitions in `core/burrow-engines/src/web/bridge.rs` are the audit surface:
// this file cannot grow a capability without a method appearing there first.
//
// TWO THINGS THAT LOOK LIKE OMISSIONS AND ARE NOT
//
//   * `__burrow_pdfium_free_input` and `__burrow_pdfium_close` are separate, rather than one
//     function with `if (doc !== 0)`. That `if` would be a branch on engine state. Rust
//     knows whether a document was created and picks; the bridge just does as it is told.
//
//   * `__burrow_pdfium_load` returns the handle and the error code packed into one BigInt.
//     `FPDF_GetLastError` reads a process-global slot that the next PDFium call overwrites,
//     so fetching it in a second round trip could attach a different operation's error to
//     this one. One call, both values.

// NO "use strict" HERE, deliberately.
//
// It was here and it was INERT: the bundle emits the generated `BURROW_ENGINES` const before
// this file, so the directive is no longer in a directive prologue and has no effect on any
// of the bundle's ~1,000 lines. Leaving it in would be a comment that claims a guarantee the
// code does not have.
//
// Making it real would mean emitting it as the bundle's genuine first statement, which would
// also flip 160 KB of third-party Emscripten glue to strict mode -- a much larger and
// entirely untested change for no benefit we need. What strict mode would buy here is a
// `ReferenceError` on an undeclared assignment, and `tsc -p src/worker` already reports that
// as "Cannot find name" (verified). `src/production-build.test.ts` asserts the bundle's mode
// so this cannot drift back silently.

/** @type {EmscriptenModule | null} The PDFium Emscripten module. */
let pdfiumModule = null;
/** @type {EmscriptenModule | null} The qpdf Emscripten module. */
let qpdfModule = null;

/**
 * Wire the bridge to the two initialised modules. Called once, by the worker.
 *
 * @param {EmscriptenModule} pdfium
 * @param {EmscriptenModule} qpdf
 */
function __burrow_attach(pdfium, qpdf) {
  pdfiumModule = pdfium;
  qpdfModule = qpdf;
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

/**
 * The qpdf module, or a loud failure. See {@link pdfium}.
 *
 * @returns {EmscriptenModule}
 */
function qpdf() {
  if (!qpdfModule) {
    throw new Error("bridge used before attach");
  }
  return qpdfModule;
}

// A pointer is an i32 in the module's address space, and JS sign-extends the high bit. The
// modules are built `-sMAXIMUM_MEMORY=2GB` so no address ever sets it, but coercing anyway
// costs nothing and means a future memory bump cannot turn a pointer into a negative number.
/** @param {number} n @returns {number} */
const u32 = (n) => n >>> 0;

/**
 * Copy bytes into a module's heap. Returns 0 if the allocation failed.
 *
 * @param {EmscriptenModule} module
 * @param {Uint8Array} bytes
 * @returns {number}
 */
function copyInto(module, bytes) {
  const ptr = module._malloc(bytes.length);
  if (ptr === 0) {
    return 0;
  }
  module.HEAPU8.set(bytes, ptr);
  return u32(ptr);
}

/**
 * Zero `len` bytes at `ptr`, then free it.
 *
 * @param {EmscriptenModule} module
 * @param {number} ptr
 * @param {number} len
 */
function wipeAndFree(module, ptr, len) {
  // The wipe matters: a password copied here is outside Rust's allocator, so `Zeroizing`
  // cannot reach it. Without this it sits in the module's free list for the life of the
  // worker, readable by anything that allocates next.
  module.HEAPU8.fill(0, ptr, ptr + len);
  module._free(ptr);
}

/**
 * A module's heap size in WASM pages. See `pages_to_bytes` on the Rust side for why.
 *
 * @param {EmscriptenModule} module
 * @returns {number}
 */
const heapPages = (module) => module.HEAPU8.byteLength / 65536;

// --- PDFium ---------------------------------------------------------------------------

self.__burrow_pdfium_copy_in = (bytes) => copyInto(pdfium(), bytes);
self.__burrow_pdfium_wipe_free = (ptr, len) => wipeAndFree(pdfium(), ptr, len);
self.__burrow_pdfium_free_input = (ptr) => pdfium()._free(ptr);

self.__burrow_pdfium_load = (data, len, password) => {
  const doc = pdfium()._FPDF_LoadMemDocument64(data, len, password);
  // Read immediately, before any other PDFium call can overwrite the global.
  const code = pdfium()._FPDF_GetLastError();
  return (BigInt(u32(code)) << 32n) | BigInt(u32(doc));
};

self.__burrow_pdfium_pages = (doc) => pdfium()._FPDF_GetPageCount(doc);

self.__burrow_pdfium_close = (doc, data) => {
  // Close, then free. PDFium reads from the input buffer for as long as the document is
  // open (fpdfview.h:451), so the other order is a use-after-free. Two statements in one
  // function is the only reason no call site can get it wrong.
  pdfium()._FPDF_CloseDocument(doc);
  pdfium()._free(data);
};

self.__burrow_pdfium_heap_pages = () => heapPages(pdfium());

// --- qpdf -----------------------------------------------------------------------------

self.__burrow_qpdf_copy_in = (bytes) => copyInto(qpdf(), bytes);
self.__burrow_qpdf_free = (ptr) => qpdf()._free(ptr);
self.__burrow_qpdf_wipe_free = (ptr, len) => wipeAndFree(qpdf(), ptr, len);

self.__burrow_qpdf_init = () => u32(qpdf()._qpdf_init());

self.__burrow_qpdf_cleanup = (data) => {
  // `qpdf_cleanup` takes `qpdf_data*`, not `qpdf_data`, so the handle needs a scratch word
  // it can null out. The C stack is the right place for a value that lives for one call.
  const engine = qpdf();
  const saved = engine.stackSave();
  try {
    const slot = engine.stackAlloc(4);
    engine.HEAPU32[slot >> 2] = data;
    engine._qpdf_cleanup(slot);
  } finally {
    engine.stackRestore(saved);
  }
};

self.__burrow_qpdf_silence_errors = (data) => qpdf()._qpdf_silence_errors(data);
self.__burrow_qpdf_set_suppress_warnings = (data, v) => qpdf()._qpdf_set_suppress_warnings(data, v);
self.__burrow_qpdf_set_logger = (data, logger) => qpdf()._qpdf_set_logger(data, logger);
self.__burrow_qpdf_set_attempt_recovery = (data, v) => qpdf()._qpdf_set_attempt_recovery(data, v);

self.__burrow_qpdf_read_memory = (data, description, buffer, size, password) =>
  // `size` arrives as a BigInt, which is exactly what the module takes: it is built
  // `-sWASM_BIGINT=1` so `unsigned long long` is one argument rather than a legalized
  // (lo, hi) pair. Forwarded untouched -- no split, no rounding, no 64-bit arithmetic here.
  qpdf()._qpdf_read_memory(data, description, buffer, size, password);

self.__burrow_qpdf_has_error = (data) => qpdf()._qpdf_has_error(data);
self.__burrow_qpdf_get_error = (data) => u32(qpdf()._qpdf_get_error(data));
self.__burrow_qpdf_get_error_code = (data, error) => qpdf()._qpdf_get_error_code(data, error);
self.__burrow_qpdf_get_num_pages = (data) => qpdf()._qpdf_get_num_pages(data);

self.__burrow_qpdf_global_set_uint32 = (param, value) =>
  qpdf()._qpdf_global_set_uint32(param, value);

self.__burrow_qpdflogger_create = () => u32(qpdf()._qpdflogger_create());

self.__burrow_qpdflogger_discard_all = (logger, destination) => {
  // `destination` is `qpdf_log_dest_discard` (3, qpdflogger-c.h:62) -- qpdf's own Pl_Discard,
  // reachable by name, so no callback and no JS running on a C++ stack.
  //
  // Passed from Rust rather than written here. It was `const DISCARD = 3` in this file, which
  // meant two independent definitions of one qpdf enum value: Rust's went unused (and failed
  // the build without the native engines, where nothing else referenced it) while this copy
  // was the one that actually ran.
  const engine = qpdf();
  engine._qpdflogger_set_info(logger, destination, 0, 0);
  engine._qpdflogger_set_warn(logger, destination, 0, 0);
  engine._qpdflogger_set_error(logger, destination, 0, 0);
};

self.__burrow_qpdf_heap_pages = () => heapPages(qpdf());

// --- the clock ------------------------------------------------------------------------

// Truncated here so no float crosses into Rust: converting one there needs the numeric
// casts the workspace denies. `performance.now()` is monotonic from the context's own time
// origin, which is the "unspecified epoch" the Clock contract describes.
self.__burrow_now_ms = () => BigInt(Math.trunc(performance.now()));
