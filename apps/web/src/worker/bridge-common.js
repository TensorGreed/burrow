// The part of the engine bridge both bundles carry: the marshalling helpers and the clock.
//
// WHAT IS ALLOWED IN THIS FILE, AND IN THE TWO BESIDE IT
//
// ADR 0009: "Bindings may hold engine handles and marshal data across the boundary. No
// branch on engine state may live in JS."
//
// So every function here and in `bridge-qpdf.js` / `bridge-pdfium.js` does one of exactly
// three things: allocate in an engine heap, copy bytes, or forward one call to one Emscripten
// export. There is no `if` anywhere that looks at what an engine returned. Nothing here
// interprets an error code, chooses an engine, decides whether to retry, loops over pages, or
// compares anything against a limit -- all of that is Rust, in `burrow-engines`, and it is the
// same Rust the native path runs.
//
// The trait definitions in `core/burrow-engines/src/web/bridge.rs` are the audit surface:
// these files cannot grow a capability without a method appearing there first.
//
// ONE ENGINE PER BUNDLE, SINCE ADR 0026
//
// | bundle                     | engine | engine half        | Rust module                  |
// |----------------------------|--------|--------------------|------------------------------|
// | `burrow-worker.js`         | qpdf   | `bridge-qpdf.js`   | `burrow_wasm_bg.wasm`        |
// | `burrow-render-worker.js`  | PDFium | `bridge-pdfium.js` | `burrow_wasm_render_bg.wasm` |
//
// The two never appear in one bundle, so `__burrow_attach` is defined once per bundle and
// there is no name to collide. That split is what lets a person who merges two files download
// no PDFium at all, and it is enforced by which files this bundle is built from rather than by
// a check over the result.
//
// TWO DECISIONS THAT BELONG TO THE PAIR, kept here because each was rediscovered the hard way:
//
//   * A release that frees the user's document takes a LENGTH and wipes before freeing. A
//     plain `_free` returns those bytes to the module's free list intact, where they stay for
//     the life of the worker -- and one worker serves many documents in a session.
//
//   * A handle and its error code cross in ONE call, and that is PDFium's constraint
//     specifically: its last-error read is a PROCESS-GLOBAL slot which the next call
//     overwrites, so fetching it in a second round trip could attach a different operation's
//     error to this one.
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

// --- the clock ------------------------------------------------------------------------

// Truncated here so no float crosses into Rust: converting one there needs the numeric
// casts the workspace denies. `performance.now()` is monotonic from the context's own time
// origin, which is the "unspecified epoch" the Clock contract describes.
self.__burrow_now_ms = () => BigInt(Math.trunc(performance.now()));
