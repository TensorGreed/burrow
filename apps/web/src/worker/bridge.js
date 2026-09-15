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
// ONE ENGINE, SINCE SPIKE 0004. PDFium left the web payload -- 79.7% of the first load,
// reachable from one function -- and its seven bridge globals went with it. Two decisions
// recorded here belonged to those globals and are kept as history, because the reasoning is
// the kind that gets rediscovered the hard way if a second engine comes back:
//
//   * A release that frees the user's document takes a LENGTH and wipes before freeing. A
//     plain `_free` returns those bytes to the module's free list intact, where they stay for
//     the life of the worker -- and one worker serves many documents in a session.
//
//   * A handle and its error code crossed in ONE call, and that was PDFium's constraint
//     specifically: its last-error read a PROCESS-GLOBAL slot which the next call overwrote,
//     so fetching it in a second round trip could attach a different operation's error to
//     this one.
//
//     **qpdf does not need this, and the pairing must not be copied onto it.** Its error is
//     latched per-`QPDF` handle rather than in a process global, so `__burrow_qpdf_read_memory`
//     and `__burrow_qpdf_get_error_code` are deliberately two calls and nothing overwrites the
//     second between them. An earlier draft of this comment said qpdf's error "is read the
//     same way and for the same reason", which would send the next reader to pack a pair that
//     is correct as it stands.

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

/** @type {EmscriptenModule | null} The qpdf Emscripten module. */
let qpdfModule = null;

/**
 * Wire the bridge to the initialised module. Called once, by the worker.
 *
 * @param {EmscriptenModule} qpdf
 */
function __burrow_attach(qpdf) {
  qpdfModule = qpdf;
}

/**
 * The qpdf module, or a loud failure.
 *
 * Not a branch on engine state — it cannot be, because no engine has spoken yet. It turns
 * "the bridge was called before init finished" into a thrown error instead of
 * `undefined._malloc`, which the worker would report as an opaque `Internal` with no clue
 * as to the cause.
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

// NO PDFIUM. Seven of its bridge globals stood here until spike 0004 took PDFium out of
// the web payload. They are removed rather than left defined against a module that is no
// longer loaded: `bridge.rs` declares the matching imports, a `no-modules` wasm-bindgen import
// resolves from this scope, and a declared import with no global fails at INSTANTIATION --- in
// the browser and nowhere else.
//
// ADR 0009 §2: the bridge method list IS the audit surface, so it lists what exists.

// --- qpdf -----------------------------------------------------------------------------

self.__burrow_qpdf_copy_in = (bytes) => copyInto(qpdf(), bytes);
// Plain `_free`, for buffers this crate wrote itself and that hold nothing of the user's --
// the fixed `DESCRIPTION` string, and the `/Rotate` and `/Parent` key names. The user's
// DOCUMENT goes through `__burrow_qpdf_wipe_free`, as the password always has.
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

// --- qpdf: the write path (M1 PR B2) ---------------------------------------------------
//
// Every one of these forwards a single call and returns whatever the module returns. None
// of them branches on the result -- `add_page` and the write functions return qpdf's status
// BITMASK, and deciding whether a bitmask means failure is a decision, so it happens in
// Rust (`codes::qpdf::has_errors`). ADR 0009 §2.

self.__burrow_qpdf_get_page_n = (data, n) => u32(qpdf()._qpdf_get_page_n(data, n));

self.__burrow_qpdf_add_page = (data, source, page, first) =>
  qpdf()._qpdf_add_page(data, source, page, first);

self.__burrow_qpdf_remove_page = (data, page) => qpdf()._qpdf_remove_page(data, page);

self.__burrow_qpdf_add_page_at = (data, source, page, before, refpage) =>
  qpdf()._qpdf_add_page_at(data, source, page, before, refpage);

self.__burrow_qpdf_init_write_memory = (data) => qpdf()._qpdf_init_write_memory(data);

self.__burrow_qpdf_set_deterministic_id = (data, value) =>
  qpdf()._qpdf_set_deterministic_ID(data, value);

self.__burrow_qpdf_write = (data) => qpdf()._qpdf_write(data);

// ---- the object-handle API, added for `rotate` -------------------------------------
//
// Each is one call with its arguments passed through. Nothing here decides anything: the
// type code is fetched and returned unexamined, and whether a `/Rotate` is usable is
// decided in Rust (`web/rotate.rs`), exactly as it is on the native path. ADR 0009 §2.
//
// `key` is a POINTER into the module's heap, not a string. Rust copies the key in and
// passes the address, so no string is built here and nothing on this side can choose which
// key is asked for -- the only two burrow asks for are `/Rotate` and `/Parent`, both
// constants in Rust.

self.__burrow_qpdf_oh_get_key = (data, oh, key) => u32(qpdf()._qpdf_oh_get_key(data, oh, key));

self.__burrow_qpdf_oh_get_type_code = (data, oh) => qpdf()._qpdf_oh_get_type_code(data, oh);

// `long long` on the C side, so it crosses as a BigInt -- the module is built with
// -sWASM_BIGINT=1 for exactly this reason, and `qpdf_read_memory`'s size argument is the
// other case. Converting here rather than splitting into (lo, hi) keeps 64-bit arithmetic
// out of the binding layer.
self.__burrow_qpdf_oh_get_int_value = (data, oh) => BigInt(qpdf()._qpdf_oh_get_int_value(data, oh));

self.__burrow_qpdf_oh_new_integer = (data, value) => u32(qpdf()._qpdf_oh_new_integer(data, value));

self.__burrow_qpdf_oh_replace_key = (data, oh, key, item) =>
  qpdf()._qpdf_oh_replace_key(data, oh, key, item);

// BOTH HALVES OF AN OBJECT'S IDENTITY, PACKED INTO ONE VALUE.
//
// Two objects may share an object number across generations, so a caller holding only the
// number would call two different objects the same one. Offering the halves as separate
// bridge methods invites exactly that; packing them means they cannot be used apart. Same
// argument as PDFium's load did, packing a handle with its error code (see the header).
//
// A `qpdf_oh` is NOT an identity -- qpdf issues a fresh one per call -- which is why this
// exists at all. See `QpdfBridge::oh_object`.
self.__burrow_qpdf_oh_object = (data, oh) => {
  const id = qpdf()._qpdf_oh_get_object_id(data, oh);
  const generation = qpdf()._qpdf_oh_get_generation(data, oh);
  return (BigInt(u32(id)) << 32n) | BigInt(u32(generation));
};

// ---- split's pruning ----------------------------------------------------------------
//
// Ten forwarders and two that are not. `__burrow_qpdf_oh_page_content` and
// `__burrow_qpdf_oh_stream_data` allocate a scratch word, call qpdf, copy the bytes out and
// FREE QPDF'S BUFFER -- because those two are the only qpdf calls whose buffer belongs to the
// caller rather than to qpdf. Keeping the free beside the allocation is what stops that
// ownership rule becoming a discipline every caller has to remember; leaking it would cost the
// decompressed size of a content stream, per page, per part.
//
// They decide nothing. `null` on a non-zero status or an undecoded stream is not a judgement:
// the Rust side treats both the same way and must, because undecoded bytes are still
// compressed and reading names out of them is worse than not reading them at all.
self.__burrow_qpdf_oh_unparse_resolved = (data, oh) => qpdf()._qpdf_oh_unparse_resolved(data, oh);
self.__burrow_qpdf_oh_get_name = (data, oh) => qpdf()._qpdf_oh_get_name(data, oh);
self.__burrow_qpdf_oh_remove_key = (data, oh, key) => qpdf()._qpdf_oh_remove_key(data, oh, key);
self.__burrow_qpdf_oh_get_array_n_items = (data, oh) => qpdf()._qpdf_oh_get_array_n_items(data, oh);
self.__burrow_qpdf_oh_get_array_item = (data, oh, at) =>
  qpdf()._qpdf_oh_get_array_item(data, oh, at);
self.__burrow_qpdf_oh_erase_item = (data, oh, at) => qpdf()._qpdf_oh_erase_item(data, oh, at);
self.__burrow_qpdf_oh_get_dict = (data, oh) => qpdf()._qpdf_oh_get_dict(data, oh);

/**
 * Copy a NUL-terminated string qpdf owns out of the engine heap.
 *
 * The pointer dies on the next call that returns one, which is why this copies rather than
 * handing the address back.
 *
 * @param {number} ptr
 * @returns {Uint8Array}
 */
self.__burrow_qpdf_copy_c_string = (ptr) => {
  const heap = qpdf().HEAPU8;
  let end = ptr;
  // BOUNDED BY THE HEAP. An out-of-range typed-array index reads `undefined`, and
  // `undefined !== 0`, so an unterminated string would walk past the end of the heap forever
  // rather than throwing -- a hung worker, which no watchdog inside the worker can break.
  // qpdf's `unparse_resolved` and `get_name` return `c_str()` from its own std::string, so
  // this is not reachable today; an unbounded loop over engine memory is not something to
  // leave resting on that.
  while (end < heap.length && heap[end] !== 0) end += 1;
  if (end >= heap.length) {
    throw new Error("unterminated string from the engine");
  }
  return heap.slice(ptr, end);
};

/**
 * `qpdf_oh_get_page_content_data`, copied out and freed.
 *
 * @param {number} data
 * @param {number} page
 * @returns {Uint8Array | null}
 */
self.__burrow_qpdf_oh_page_content = (data, page) => {
  const module = qpdf();
  const bufp = module._malloc(4);
  const lenp = module._malloc(4);
  if (bufp === 0 || lenp === 0) {
    if (bufp !== 0) module._free(bufp);
    if (lenp !== 0) module._free(lenp);
    return null;
  }
  try {
    // ZEROED BEFORE THE CALL. qpdf's trapped functions do not write their out-parameters on
    // the throw path -- `getMallocBuffer` is the last statement in the lambda -- and
    // `_malloc` does not zero. So an uninitialised `*bufp` is whatever was last in that
    // chunk, and the free below would hand it to the allocator.
    //
    // That is not hypothetical and it is not a wild pointer either: `page_content` uses two
    // scratch words and `stream_data` uses three, so `stream_data`'s `bufp` lands on the
    // chunk that was `page_content`'s `lenp` -- which holds the decoded LENGTH of the
    // previous content stream, a number the file chooses. Measured by security review:
    // `free(0xc0ffee)` from a two-page document. Inside the wasm sandbox that is heap
    // metadata corruption, and because the operation fails as `Malformed` rather than
    // `Internal` the worker is NOT discarded, so it persists into the next document.
    module.HEAPU32[bufp >>> 2] = 0;
    module.HEAPU32[lenp >>> 2] = 0;

    module._qpdf_oh_get_page_content_data(data, page, bufp, lenp);
    const buf = module.HEAPU32[bufp >>> 2];
    const len = module.HEAPU32[lenp >>> 2];
    try {
      // NO STATUS BRANCH. It used to test `(status & 2)`, which is ADR 0009 §2's "interpret
      // an engine error code" and contradicted this file's own header. It is not needed: a
      // trapped function LATCHES its error on the `qpdf_data`, and `WebGraph` drains that
      // after every call -- so Rust sees the real typed error before it ever looks at these
      // bytes. Returning what is here decides nothing.
      return buf === 0 || len === 0 ? new Uint8Array(0) : module.HEAPU8.slice(buf, buf + len);
    } finally {
      if (buf !== 0) {
        module.HEAPU32[bufp >>> 2] = buf;
        module._qpdf_oh_free_buffer(bufp);
      }
    }
  } finally {
    module._free(bufp);
    module._free(lenp);
  }
};

/**
 * `qpdf_oh_get_stream_data` at `qpdf_dl_specialized`, copied out and freed.
 *
 * `null` for an error AND for a stream qpdf could not decode: the caller treats them the same
 * way and must, because undecoded bytes are still compressed.
 *
 * @param {number} data
 * @param {number} oh
 * @returns {Uint8Array | null}
 */
self.__burrow_qpdf_oh_stream_data = (data, oh) => {
  const module = qpdf();
  const filteredp = module._malloc(4);
  const bufp = module._malloc(4);
  const lenp = module._malloc(4);
  if (filteredp === 0 || bufp === 0 || lenp === 0) {
    if (filteredp !== 0) module._free(filteredp);
    if (bufp !== 0) module._free(bufp);
    if (lenp !== 0) module._free(lenp);
    return null;
  }
  try {
    // ZEROED BEFORE THE CALL, for the reason `oh_page_content` gives at length.
    module.HEAPU32[filteredp >>> 2] = 0;
    module.HEAPU32[bufp >>> 2] = 0;
    module.HEAPU32[lenp >>> 2] = 0;

    // 2 is `qpdf_dl_specialized`: general-purpose filters plus the other non-lossy ones, and
    // NOT the lossy ones -- a content stream is never `/DCTDecode`, and decoding an image
    // here would spend an image's memory to lex bytes that are not syntax.
    module._qpdf_oh_get_stream_data(data, oh, 2, filteredp, bufp, lenp);
    const buf = module.HEAPU32[bufp >>> 2];
    const len = module.HEAPU32[lenp >>> 2];
    const filtered = module.HEAPU32[filteredp >>> 2];
    try {
      // `filtered` IS NOT AN ERROR CODE, which is why reading it here is not the thing ADR
      // 0009 §2 forbids: it is a two-state out-parameter saying whether qpdf decoded the
      // stream, and `null` carries that state to Rust unchanged. Rust decides what it means
      // -- and it means a refusal, because undecoded bytes are still compressed and reading
      // resource names out of them deletes resources the page draws with.
      //
      // A throw is a different thing and is not read here at all: it is latched on the
      // `qpdf_data` and `WebGraph` drains it after every call.
      if (filtered === 0) return null;
      return buf === 0 || len === 0 ? new Uint8Array(0) : module.HEAPU8.slice(buf, buf + len);
    } finally {
      if (buf !== 0) {
        module.HEAPU32[bufp >>> 2] = buf;
        module._qpdf_oh_free_buffer(bufp);
      }
    }
  } finally {
    module._free(filteredp);
    module._free(bufp);
    module._free(lenp);
  }
};

self.__burrow_qpdf_oh_release = (data, oh) => qpdf()._qpdf_oh_release(data, oh);

self.__burrow_qpdf_get_buffer_length = (data) => u32(qpdf()._qpdf_get_buffer_length(data));

self.__burrow_qpdf_get_buffer = (data) => u32(qpdf()._qpdf_get_buffer(data));

// THE ONLY BRIDGE FUNCTION THAT CARRIES BYTES OUT.
//
// `slice`, not `subarray`: a subarray is a VIEW into the module's heap, and the heap moves
// whenever the module grows it (`ALLOW_MEMORY_GROWTH=1` detaches the old buffer). Handing
// wasm-bindgen a view would mean the bytes are read at some later moment, possibly out of a
// detached buffer or out of memory qpdf has since reused. A copy is the whole point.
//
// It decides nothing: `ptr` and `len` are qpdf's own answers, passed straight back from
// Rust, and nothing here looks at what the bytes are.
/**
 * @param {number} ptr
 * @param {number} len
 * @returns {Uint8Array}
 */
self.__burrow_qpdf_copy_out = (ptr, len) => qpdf().HEAPU8.slice(ptr, ptr + len);

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
