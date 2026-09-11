//! The seam between Rust and the Emscripten engine modules.
//!
//! Under [ADR 0006] option 1, PDFium and qpdf are *separate* Emscripten modules with their
//! own linear memories, and Rust on `wasm32-unknown-unknown` can only reach them through
//! JavaScript. These traits are that boundary, stated once, in Rust.
//!
//! # Why the traits live here and the JavaScript does not
//!
//! [ADR 0009] replaced "bindings contain no logic" with something testable:
//!
//! > Bindings may hold engine handles and marshal data across the boundary. **No branch on
//! > engine state may live in JS.**
//!
//! A trait makes that structural rather than a convention. Every capability the binding is
//! permitted is a method here, so the method list *is* the audit surface: adding a decision
//! to JavaScript means adding a method whose name gives it away. There is no method that
//! interprets an error code, chooses an engine, decides whether to retry, or compares
//! anything against a [`Limits`](burrow_types::Limits) — because all of that stays in
//! [`super`], which is the same Rust the native path runs.
//!
//! # Why this module is not `cfg(target_arch = "wasm32")`
//!
//! It compiles everywhere, like [`crate::prescan`]. That is not tidiness: it is what lets
//! `cargo test` on an ordinary Linux host drive the **whole web orchestration** — limit
//! ordering, the deadline, error mapping, close-before-free — against a fake bridge, with
//! no browser and no engines. A `cfg(wasm32)` module would be code CI never executes, and
//! "the same rules on both paths" would be an assertion rather than a test.
//!
//! # The two address spaces
//!
//! [`PdfiumPtr`] and [`QpdfPtr`] are distinct types on purpose. Two Emscripten modules mean
//! two heaps with overlapping 32-bit ranges, so pdfium `0x5f_0a10` and qpdf `0x5f_0a10` are
//! both valid and entirely unrelated. A single `u32` everywhere would make mixing them a
//! typo; making them separate types makes it a compile error.
//!
//! Neither is a Rust pointer and neither can be dereferenced from Rust — they are numbers
//! naming a location in someone else's address space. That is a stronger position than the
//! native path's raw `*mut c_void`, and it is why nothing in this module is `unsafe`.
//!
//! [ADR 0006]: https://github.com/TensorGreed/burrow/blob/main/docs/adr/0006-wasm-linking-strategy.md
//! [ADR 0009]: https://github.com/TensorGreed/burrow/blob/main/docs/adr/0009-web-panic-contract-and-binding-boundary.md

use core::ffi::c_ulong;

/// An address in the **PDFium** module's linear memory.
///
/// Not dereferenceable from Rust. Zero means null, exactly as it does in C.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PdfiumPtr(pub u32);

impl PdfiumPtr {
    /// The null pointer, which every engine call treats as "absent".
    pub const NULL: Self = Self(0);

    /// Whether this is the null pointer.
    #[must_use]
    pub const fn is_null(self) -> bool {
        self.0 == 0
    }
}

/// An address in the **qpdf** module's linear memory.
///
/// Not dereferenceable from Rust. Zero means null.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct QpdfPtr(pub u32);

impl QpdfPtr {
    /// The null pointer.
    pub const NULL: Self = Self(0);

    /// Whether this is the null pointer.
    #[must_use]
    pub const fn is_null(self) -> bool {
        self.0 == 0
    }
}

/// What `FPDF_LoadMemDocument64` returned, **and** the error code from the same call.
///
/// The two travel together because they must. `FPDF_GetLastError` reads a process-global
/// slot that the next PDFium call overwrites, so a code fetched in a second round trip is
/// not reliably this load's code. The native path solves this by reading both inside one
/// `unsafe` block; the web path solves it by making one bridge call return both.
///
/// Failure is established by `handle` being null. `code` only *classifies* a failure that
/// is already established — see `codes::pdfium::map_failure` (not linked: it is private,
/// which is itself the point — no raw engine code escapes this crate), and ADR 0006
/// requirement 6 for what happens when those two jobs are confused.
#[derive(Debug, Clone, Copy)]
pub struct LoadOutcome {
    /// The `FPDF_DOCUMENT` handle, or [`PdfiumPtr::NULL`] on failure.
    pub handle: PdfiumPtr,
    /// The value `FPDF_GetLastError()` returned immediately after the load.
    pub code: c_ulong,
}

/// The PDFium Emscripten module, as seen from Rust.
///
/// `Send + Sync` so that [`super::pdfium::WebDocument`] can be, which [`DocumentEngine`]
/// requires of its associated type. On `wasm32-unknown-unknown` there is one thread, so
/// this costs nothing and claims nothing false.
///
/// [`DocumentEngine`]: crate::DocumentEngine
pub trait PdfiumBridge: Send + Sync {
    /// Copy `bytes` into the module's heap.
    ///
    /// Returns [`PdfiumPtr::NULL`] if the allocation failed. The caller decides what that
    /// means; this reports it.
    fn copy_in(&self, bytes: &[u8]) -> PdfiumPtr;

    /// Zero `len` bytes at `ptr`, then free it.
    ///
    /// The wipe is not decoration. A password copied into the engine heap is outside
    /// Rust's allocator, so [`zeroize`](burrow_types::Password) cannot reach it; without
    /// this it would sit in the module's free list for the life of the worker.
    fn wipe_and_free(&self, ptr: PdfiumPtr, len: u32);

    /// Free an input buffer that no document was ever attached to.
    ///
    /// Separate from [`close_document`](PdfiumBridge::close_document) rather than being the
    /// same call with a null check, because a null check in JavaScript is a branch on
    /// engine state and ADR 0009 forbids it. Two straight-line functions, and Rust picks.
    fn abandon_input(&self, ptr: PdfiumPtr);

    /// `FPDF_LoadMemDocument64`, returning the handle and this call's error code together.
    ///
    /// The 64-bit variant, as on native: a signed length turns an input above 2 GiB into a
    /// huge out-of-bounds read.
    fn load_mem_document64(&self, data: PdfiumPtr, len: u32, password: PdfiumPtr) -> LoadOutcome;

    /// `FPDF_GetPageCount`. Negative means the page tree could not be read.
    ///
    /// Deliberately returns **no** error code. `fpdfview.h:625` says the global is only
    /// meaningful for APIs whose own documentation mentions `FPDF_GetLastError`, and this
    /// one's does not — so the global would hold whatever an earlier call left there. Not
    /// being *able* to fetch it is how that mistake is prevented rather than merely
    /// avoided.
    fn get_page_count(&self, doc: PdfiumPtr) -> i32;

    /// `FPDF_CloseDocument(doc)` and then free `data`, in that order.
    ///
    /// One call, so the order cannot be got wrong at a call site. PDFium reads from the
    /// input buffer for as long as the document is open (`fpdfview.h:451`), so freeing
    /// first is a use-after-free.
    fn close_document(&self, doc: PdfiumPtr, data: PdfiumPtr);

    /// The module's current heap size, `HEAPU8.byteLength`.
    ///
    /// The web stand-in for the native path's resident-set reading. Better attributed —
    /// it measures this engine rather than the whole process — but it only ever grows, so
    /// only the *difference* across an operation is meaningful.
    fn heap_bytes(&self) -> u64;
}

/// The qpdf Emscripten module, as seen from Rust.
///
/// One method per C function, and **only** the functions [ADR 0013] verified route through
/// qpdf's `trap_errors` helper. This list is a mirror of `qpdf::ffi`'s `extern "C"` block:
/// a name in one and not the other means either the native path calls something the web
/// cannot, or the web calls something that was never cleared to cross back into Rust
/// without unwinding.
///
/// `qpdf_is_encrypted` and `qpdf_is_linearized` are absent for the reason they are absent
/// there — neither is trapped, and an object number above `INT_MAX` makes the latter throw
/// `std::range_error` straight out of the C API. On the web that surfaces as a JS exception
/// at the bridge rather than a process abort, which is *quieter*, not safer.
///
/// [ADR 0013]: https://github.com/TensorGreed/burrow/blob/main/docs/adr/0013-qpdf-c-api-and-prescan.md
pub trait QpdfBridge: Send + Sync {
    /// Copy `bytes` into the module's heap. [`QpdfPtr::NULL`] if the allocation failed.
    fn copy_in(&self, bytes: &[u8]) -> QpdfPtr;

    /// Free a buffer.
    fn free(&self, ptr: QpdfPtr);

    /// Zero `len` bytes at `ptr`, then free it. See [`PdfiumBridge::wipe_and_free`].
    fn wipe_and_free(&self, ptr: QpdfPtr, len: u32);

    /// `qpdf_init`. [`QpdfPtr::NULL`] if qpdf could not allocate.
    fn init(&self) -> QpdfPtr;

    /// `qpdf_cleanup`.
    fn cleanup(&self, data: QpdfPtr);

    /// `qpdf_silence_errors`.
    ///
    /// The one most easily missed and the one that matters most: without it, functions
    /// that do not return an error code print to the host's stderr, which in a browser is
    /// the devtools console.
    fn silence_errors(&self, data: QpdfPtr);

    /// `qpdf_set_suppress_warnings`.
    fn set_suppress_warnings(&self, data: QpdfPtr, value: bool);

    /// `qpdf_set_logger`.
    fn set_logger(&self, data: QpdfPtr, logger: QpdfPtr);

    /// `qpdf_set_attempt_recovery`.
    fn set_attempt_recovery(&self, data: QpdfPtr, value: bool);

    /// `qpdf_read_memory`. Returns the raw `QPDF_ERROR_CODE`, which is a **bitmask**.
    ///
    /// Passed through unexamined. `codes::qpdf::has_errors` is what decides whether it
    /// reports an error, because the natural `!= QPDF_SUCCESS` would treat a file that
    /// parsed perfectly but emitted a warning as a failure — `QPDF_ERROR_CODE` is a
    /// bitmask, not an enum.
    fn read_memory(
        &self,
        data: QpdfPtr,
        description: QpdfPtr,
        buffer: QpdfPtr,
        size: u64,
        password: QpdfPtr,
    ) -> i32;

    /// `qpdf_has_error`.
    fn has_error(&self, data: QpdfPtr) -> bool;

    /// `qpdf_get_error`, which *consumes* the error slot.
    fn get_error(&self, data: QpdfPtr) -> QpdfPtr;

    /// `qpdf_get_error_code`.
    ///
    /// Kept separate from [`get_error`](QpdfBridge::get_error) rather than fused, because
    /// fusing them would hide that the error must be taken from the slot first — and
    /// draining the slot is what stops `qpdf_cleanup` writing
    /// `"application did not handle error: <text>"`, byte offsets and all, to the default
    /// logger.
    fn get_error_code(&self, data: QpdfPtr, error: QpdfPtr) -> i32;

    /// `qpdf_get_num_pages`. Negative means the page structure could not be read.
    fn get_num_pages(&self, data: QpdfPtr) -> i32;

    /// `qpdf_global_set_uint32`. Process-global; takes no `qpdf_data`.
    ///
    /// Returns qpdf's status code, passed through unexamined. The caller ignores it: qpdf
    /// rejects a parameter it does not recognise, which is what a future version removing a
    /// hardening knob looks like, and refusing to open any document because a knob moved
    /// would be worse than opening it with qpdf's own defaults.
    fn global_set_uint32(&self, param: i32, value: u32) -> i32;

    /// `qpdflogger_create`. [`QpdfPtr::NULL`] if qpdf could not allocate.
    fn logger_create(&self) -> QpdfPtr;

    /// Point a logger's info, warn and error streams at `destination`.
    ///
    /// One call for all three: they are always set together, and a bridge method that could
    /// set one and not the others is a way to ship a half-silenced logger.
    ///
    /// `destination` is `qpdf_log_dest_e`, passed from Rust rather than hardcoded in the
    /// bridge. It is a protocol constant, not a decision — but it is qpdf's constant, and
    /// having the JavaScript carry its own copy meant two definitions of the same enum value
    /// with nothing tying them together.
    fn logger_discard_all(&self, logger: QpdfPtr, destination: i32);

    /// The module's current heap size. See [`PdfiumBridge::heap_bytes`].
    fn heap_bytes(&self) -> u64;
}
