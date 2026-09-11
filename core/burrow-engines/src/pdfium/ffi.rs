//! PDFium's C API, declared by hand.
//!
//! Declared rather than bindgen-generated: this is a handful of functions, and
//! hand-written declarations are auditable in a way generated ones are not. Every one
//! carries the line of `engines/vendor/native-*/include/pdfium/fpdfview.h` it was checked
//! against. **Nothing verifies these automatically, and a wrong signature is undefined
//! behaviour no test would catch — re-check them against the header on any PDFium bump.**
//!
//! # `extern "C"` is only correct because this is Linux-only
//!
//! `FPDF_CALLCONV` is `__stdcall` on Windows (`fpdfview.h:221`) and empty everywhere else.
//! These declarations say `extern "C"`, which is right for every target this module
//! compiles for — `lib.rs` gates it on `target_os = "linux"`. **If that gate is ever
//! widened to Windows, every signature here needs `extern "stdcall"`**, and the mismatch
//! would corrupt the stack rather than fail to link.
//!
//! # Nothing here may be called from outside the engine thread
//!
//! Every function in this module is `unsafe` and carries the same unlisted precondition:
//! it must be called from `super::thread`'s engine thread. PDFium is not thread-safe
//! anywhere — not just at init — so a concurrent call is undefined behaviour even when
//! each call is individually well-formed. See `docs/adr/0011-pdfium-engine-thread.md`.
//!
//! # Why the fallible calls are paired with the error read
//!
//! `FPDF_GetLastError` reads a **global**. Any other PDFium call in between overwrites
//! it, so a failure would be classified by some other call's error. The wrappers below
//! therefore make the call and read the code inside one `unsafe` block with nothing
//! between them, and return both. There is no way to use this module that separates them.

use core::ffi::{c_char, c_int, c_ulong, c_void};
use core::marker::{PhantomData, PhantomPinned};

/// Opaque stand-in for PDFium's `struct fpdf_document_t__`.
///
/// Never constructed on the Rust side; it exists so a document pointer is not
/// interchangeable with any other pointer. `fpdfview.h:70`.
#[repr(C)]
pub(crate) struct FpdfDocumentOpaque {
    _data: [u8; 0],
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}

/// `FPDF_DOCUMENT`. Null means "no document". `fpdfview.h:70`.
pub(crate) type FpdfDocument = *mut FpdfDocumentOpaque;

/// `FPDF_ERR_SUCCESS` — no error. `fpdfview.h:605`.
pub(crate) const FPDF_ERR_SUCCESS: c_ulong = 0;
/// `FPDF_ERR_UNKNOWN`. `fpdfview.h:606`.
pub(crate) const FPDF_ERR_UNKNOWN: c_ulong = 1;
/// `FPDF_ERR_FILE` — file not found or could not be opened. `fpdfview.h:607`.
pub(crate) const FPDF_ERR_FILE: c_ulong = 2;
/// `FPDF_ERR_FORMAT` — not in PDF format, or corrupted. `fpdfview.h:608`.
pub(crate) const FPDF_ERR_FORMAT: c_ulong = 3;
/// `FPDF_ERR_PASSWORD` — password required, or incorrect. `fpdfview.h:609`.
pub(crate) const FPDF_ERR_PASSWORD: c_ulong = 4;
/// `FPDF_ERR_SECURITY` — unsupported security scheme. `fpdfview.h:610`.
pub(crate) const FPDF_ERR_SECURITY: c_ulong = 5;
/// `FPDF_ERR_PAGE` — page not found or content error. `fpdfview.h:611`.
pub(crate) const FPDF_ERR_PAGE: c_ulong = 6;
/// `FPDF_ERR_XFALOAD`. `fpdfview.h:613`, behind `PDF_ENABLE_XFA` upstream.
///
/// Declared even though our build is not expected to define `PDF_ENABLE_XFA`: the code
/// space is the engine's, not ours, and an unhandled value would fall into the
/// "unrecognised" arm and cost a web worker.
pub(crate) const FPDF_ERR_XFALOAD: c_ulong = 7;
/// `FPDF_ERR_XFALAYOUT`. `fpdfview.h:614`, behind `PDF_ENABLE_XFA` upstream.
pub(crate) const FPDF_ERR_XFALAYOUT: c_ulong = 8;

unsafe extern "C" {
    /// `FPDF_EXPORT void FPDF_CALLCONV FPDF_InitLibrary()` — `fpdfview.h:330`.
    fn FPDF_InitLibrary();

    /// `FPDF_EXPORT void FPDF_CALLCONV FPDF_DestroyLibrary()` — `fpdfview.h:346`.
    ///
    /// Declared only for the link check. It is never called: tearing PDFium down while
    /// anything could still be inside it is unsound, and there is no point in the process
    /// where that is provably not the case.
    #[cfg(test)]
    fn FPDF_DestroyLibrary();

    /// `FPDF_EXPORT unsigned long FPDF_CALLCONV FPDF_GetLastError()` — `fpdfview.h:627`.
    fn FPDF_GetLastError() -> c_ulong;

    /// ```c
    /// FPDF_EXPORT FPDF_DOCUMENT FPDF_CALLCONV
    /// FPDF_LoadMemDocument64(const void* data_buf, size_t size, FPDF_BYTESTRING password);
    /// ```
    /// `fpdfview.h:462`. The `size_t` form, never the `int` one at `fpdfview.h:437`: a
    /// signed length turns an input above 2 GiB into a huge out-of-bounds read.
    fn FPDF_LoadMemDocument64(
        data_buf: *const c_void,
        size: usize,
        password: *const c_char,
    ) -> FpdfDocument;

    /// `FPDF_EXPORT int FPDF_CALLCONV FPDF_GetPageCount(FPDF_DOCUMENT document)` —
    /// `fpdfview.h:703`.
    fn FPDF_GetPageCount(document: FpdfDocument) -> c_int;

    /// `FPDF_EXPORT void FPDF_CALLCONV FPDF_CloseDocument(FPDF_DOCUMENT document)` —
    /// `fpdfview.h:986`.
    fn FPDF_CloseDocument(document: FpdfDocument);
}

/// Initialise the library, and return `FPDF_GetLastError()` read immediately after.
///
/// # Safety
///
/// Must be called **exactly once per process**, from the engine thread, before any other
/// function here. `FPDF_InitLibrary` mutates global state and is not re-entrant: two
/// threads calling it at once trip an internal `CHECK` and the process dies with
/// `SIGTRAP`. `super::thread` is the only caller and guarantees both properties.
pub(crate) unsafe fn init_library() -> c_ulong {
    // SAFETY: the caller guarantees this runs once, on the engine thread, before any
    // other PDFium call. Neither function takes or returns a pointer, so there is nothing
    // to alias, borrow, or free. The error read is the next instruction after init, so no
    // other call can have overwritten the global in between.
    //
    // `FPDF_DestroyLibrary` is deliberately never called: tearing the library down while
    // anything could still be inside it is unsound, and process exit reclaims everything.
    unsafe {
        FPDF_InitLibrary();
        FPDF_GetLastError()
    }
}

/// Load a document from memory, and read `FPDF_GetLastError()` immediately after.
///
/// Returns the handle (null on failure) and the error code, together, because they cannot
/// be obtained separately without racing the global.
///
/// # Safety
///
/// - Must be called on the engine thread, after [`init_library`].
/// - `data_buf` must be valid for reads of `size` bytes, and must **stay** valid and at
///   the same address for as long as the returned document is open. PDFium reads from it
///   lazily; `fpdfview.h:451` says so explicitly.
/// - `password` must be null, or a pointer to a NUL-terminated byte string valid for the
///   duration of the call.
pub(crate) unsafe fn load_mem_document64(
    data_buf: *const c_void,
    size: usize,
    password: *const c_char,
) -> (FpdfDocument, c_ulong) {
    // SAFETY: the caller guarantees the buffer and password pointers above, and that this
    // runs on the engine thread after init. The error read is the next call with nothing
    // in between, so the code belongs to this load and no other.
    unsafe {
        let doc = FPDF_LoadMemDocument64(data_buf, size, password);
        let code = FPDF_GetLastError();
        (doc, code)
    }
}

/// Read a document's page count, and `FPDF_GetLastError()` immediately after.
///
/// A negative count is the failure signal; the code only classifies it.
///
/// # Safety
///
/// Must be called on the engine thread, with a `document` returned by
/// [`load_mem_document64`] that has not yet been closed.
pub(crate) unsafe fn get_page_count(document: FpdfDocument) -> (c_int, c_ulong) {
    // SAFETY: the caller guarantees `document` is live and that this runs on the engine
    // thread. As above, the error read is adjacent to the call it describes.
    unsafe {
        let count = FPDF_GetPageCount(document);
        let code = FPDF_GetLastError();
        (count, code)
    }
}

/// Close a document.
///
/// # Safety
///
/// Must be called on the engine thread, exactly once per `document`, and before the
/// buffer the document was loaded from is freed.
pub(crate) unsafe fn close_document(document: FpdfDocument) {
    // SAFETY: the caller guarantees single-use, liveness, and the engine thread. This
    // reports nothing, so there is no error to read.
    unsafe { FPDF_CloseDocument(document) }
}

/// Resolve `FPDF_DestroyLibrary` without calling it.
///
///
/// Taking the function pointer forces the linker to bind the symbol, which is the only
/// thing worth proving about a function we must never call. Used by the link check.
#[cfg(test)]
pub(crate) fn destroy_library_symbol() -> unsafe extern "C" fn() {
    FPDF_DestroyLibrary
}
