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

/// Opaque stand-in for PDFium's `struct fpdf_page_t__`. `fpdfview.h:73`.
///
/// **A SEPARATE STRUCT FROM [`FpdfBitmapOpaque`], AND THAT IS THE POINT.** They were one body
/// behind two aliases, which makes `FpdfPage` and `FpdfBitmap` the same type: transposing the
/// first two arguments of [`render_page`] compiled, and PDFium would have been handed a bitmap
/// where it expected a page. The comment claimed the opposite. Found by code review; eight
/// lines is what it costs to make the sentence true rather than aspirational.
#[repr(C)]
pub(crate) struct FpdfPageOpaque {
    _data: [u8; 0],
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}

/// Opaque stand-in for PDFium's `struct fpdf_bitmap_t__`. `fpdfview.h:76`.
///
/// Never dereferenced; only PDFium does that. See [`FpdfPageOpaque`] for why it is its own
/// struct rather than an alias.
#[repr(C)]
pub(crate) struct FpdfBitmapOpaque {
    _data: [u8; 0],
    _marker: PhantomData<(*mut u8, PhantomPinned)>,
}

/// `FPDF_PAGE`. Null means the page could not be loaded. `fpdfview.h:73`.
///
/// **A page handle must never leave the engine thread, and must never outlive its
/// document.** `super::thread`'s `Registry` is the only place one exists, and it holds one
/// only for the duration of a single render -- which is what keeps `PdfiumDocument`
/// `Send + Sync` with no `unsafe impl` anywhere (ADR 0011).
pub(crate) type FpdfPage = *mut FpdfPageOpaque;

/// `FPDF_BITMAP`. Null if the allocation failed. `fpdfview.h:76`.
pub(crate) type FpdfBitmap = *mut FpdfBitmapOpaque;

// The `FPDF_ERR_*` codes moved to `crate::codes::pdfium` so the web implementation shares
// one table; only the success sentinel is still needed in this module, for the init check.
pub(super) use crate::codes::pdfium::FPDF_ERR_SUCCESS;

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

    // ---- rendering (#57) -------------------------------------------------------------
    //
    // PDFium's FIRST SEAM HERE BEYOND OPEN-AND-COUNT. Every declaration above answers a
    // question about a document; these produce PIXELS, which is a different kind of thing
    // and brings a different kind of hazard -- a buffer whose size is a decision rather
    // than a fact the file states.

    /// ```c
    /// FPDF_EXPORT FPDF_PAGE FPDF_CALLCONV
    /// FPDF_LoadPage(FPDF_DOCUMENT document, int page_index);
    /// ```
    /// `fpdfview.h:1069`. Null on failure. The index is zero-based here; the one-based
    /// form a person types is converted once, at `burrow_ops`' boundary.
    fn FPDF_LoadPage(document: FpdfDocument, page_index: c_int) -> FpdfPage;

    /// `FPDF_EXPORT void FPDF_CALLCONV FPDF_ClosePage(FPDF_PAGE page)` — `fpdfview.h:1189`.
    fn FPDF_ClosePage(page: FpdfPage);

    /// `FPDF_EXPORT float FPDF_CALLCONV FPDF_GetPageWidthF(FPDF_PAGE page)` —
    /// `fpdfview.h:1103`. Points, at 72 per inch.
    ///
    /// The `F` suffix is the float form; the deprecated `double` one at `fpdfview.h:1092`
    /// is not declared, because two spellings of one number is how a caller ends up using
    /// whichever it happened to find.
    fn FPDF_GetPageWidthF(page: FpdfPage) -> f32;

    /// `FPDF_EXPORT float FPDF_CALLCONV FPDF_GetPageHeightF(FPDF_PAGE page)` —
    /// `fpdfview.h:1124`.
    fn FPDF_GetPageHeightF(page: FpdfPage) -> f32;

    /// ```c
    /// FPDF_EXPORT FPDF_BITMAP FPDF_CALLCONV
    /// FPDFBitmap_Create(int width, int height, int alpha);
    /// ```
    /// `fpdfview.h:1465`. Null if the allocation failed, which for a large page is a
    /// reachable outcome rather than a theoretical one -- and the reason `max_pixels` is
    /// checked in Rust BEFORE this is called rather than after it returns null.
    fn FPDFBitmap_Create(width: c_int, height: c_int, alpha: c_int) -> FpdfBitmap;

    /// ```c
    /// FPDF_EXPORT void FPDF_CALLCONV
    /// FPDFBitmap_FillRect(FPDF_BITMAP bitmap, int left, int top, int width, int height,
    ///                     FPDF_DWORD color);
    /// ```
    /// `fpdfview.h:1524`. A fresh bitmap's contents are UNDEFINED, so filling it is not
    /// cosmetic: rendering a page composites onto whatever is there, and skipping this
    /// would put uninitialised heap into a picture handed to the page.
    fn FPDFBitmap_FillRect(
        bitmap: FpdfBitmap,
        left: c_int,
        top: c_int,
        width: c_int,
        height: c_int,
        color: c_ulong,
    );

    /// ```c
    /// FPDF_EXPORT void FPDF_CALLCONV
    /// FPDF_RenderPageBitmap(FPDF_BITMAP bitmap, FPDF_PAGE page, int start_x, int start_y,
    ///                       int size_x, int size_y, int rotate, int flags);
    /// ```
    /// `fpdfview.h:1290`. `rotate` is 0 here always: the page's own `/Rotate` is applied
    /// by PDFium, and a second rotation at this seam would silently double it.
    fn FPDF_RenderPageBitmap(
        bitmap: FpdfBitmap,
        page: FpdfPage,
        start_x: c_int,
        start_y: c_int,
        size_x: c_int,
        size_y: c_int,
        rotate: c_int,
        flags: c_int,
    );

    /// `FPDF_EXPORT void* FPDF_CALLCONV FPDFBitmap_GetBuffer(FPDF_BITMAP bitmap)` —
    /// `fpdfview.h:1568`. Valid until the bitmap is destroyed.
    fn FPDFBitmap_GetBuffer(bitmap: FpdfBitmap) -> *mut c_void;

    /// `FPDF_EXPORT int FPDF_CALLCONV FPDFBitmap_GetStride(FPDF_BITMAP bitmap)` —
    /// `fpdfview.h:1592`. Bytes per row, which PDFium may pad -- so a reader that assumed
    /// `width * 4` would read the wrong pixels on any page where it does.
    fn FPDFBitmap_GetStride(bitmap: FpdfBitmap) -> c_int;

    /// `FPDF_EXPORT void FPDF_CALLCONV FPDFBitmap_Destroy(FPDF_BITMAP bitmap)` —
    /// `fpdfview.h:1601`.
    fn FPDFBitmap_Destroy(bitmap: FpdfBitmap);
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

/// The `alpha` argument to [`create_bitmap`] that selects `FPDFBitmap_BGRA`.
///
/// `1`, not `0`. `0` selects `FPDFBitmap_BGRx`, where the fourth byte is padding whose value
/// PDFium does not define -- and that byte becomes the alpha channel of the picture the page
/// displays. Four bytes per pixel either way, so the difference is invisible in every size
/// calculation and visible only on screen.
pub(crate) const BITMAP_BGRA: c_int = 1;

/// Opaque white, as `FPDFBitmap_FillRect` wants it: `0xAARRGGBB`.
pub(crate) const OPAQUE_WHITE: c_ulong = 0xFFFF_FFFF;

/// `FPDF_RenderPageBitmap`'s flags. None of them.
///
/// In particular **not** `FPDF_ANNOT` (`fpdfview.h:1237`): annotations are not part of the
/// page, and a thumbnail that drew a reviewer's comments would be showing content the
/// document's pages do not have. If that becomes wanted it is a decision with a record, not a
/// constant somebody widens.
pub(crate) const RENDER_FLAGS: c_int = 0;

/// Load one page.
///
/// Returns null if the page could not be loaded. **No error code is read**, for the reason
/// [`get_page_count`] gives: `fpdfview.h:625` makes the global meaningful only for APIs whose
/// own documentation names `FPDF_GetLastError`, and this one's does not -- so reading it would
/// pin some earlier call's failure to this one.
///
/// # Safety
///
/// Must be called on the engine thread with a live `document`. The returned page must be
/// closed with [`close_page`] before the document is closed, and must not outlive it.
pub(crate) unsafe fn load_page(document: FpdfDocument, index: c_int) -> FpdfPage {
    // SAFETY: the caller guarantees the document is open and that this is the engine thread.
    unsafe { FPDF_LoadPage(document, index) }
}

/// Close a page.
///
/// # Safety
///
/// Must be called on the engine thread, exactly once per page returned by [`load_page`], and
/// before its document is closed.
pub(crate) unsafe fn close_page(page: FpdfPage) {
    // SAFETY: the caller guarantees single-use, liveness, and the engine thread.
    unsafe { FPDF_ClosePage(page) }
}

/// A page's width and height in points, with its own `/Rotate` applied.
///
/// # Safety
///
/// Must be called on the engine thread with a live page from [`load_page`].
pub(crate) unsafe fn page_size(page: FpdfPage) -> (f32, f32) {
    // SAFETY: the caller guarantees the page is loaded and that this is the engine thread.
    // Neither call takes or returns a pointer.
    unsafe { (FPDF_GetPageWidthF(page), FPDF_GetPageHeightF(page)) }
}

/// Allocate a bitmap. Null if the allocation failed.
///
/// **`max_pixels` is checked before this is called, never on its null return.** By the time
/// null comes back the cost has already been paid, which is the whole thing the ceiling exists
/// to prevent (ADR 0027).
///
/// # Safety
///
/// Must be called on the engine thread. The returned bitmap must be freed with
/// [`destroy_bitmap`].
pub(crate) unsafe fn create_bitmap(width: c_int, height: c_int, alpha: c_int) -> FpdfBitmap {
    // SAFETY: the caller guarantees the engine thread. The dimensions are plain integers.
    unsafe { FPDFBitmap_Create(width, height, alpha) }
}

/// Fill the whole bitmap with `color`.
///
/// **Not cosmetic.** A fresh bitmap's contents are undefined and rendering composites onto
/// them, so skipping this puts uninitialised engine heap into a picture handed to the page.
///
/// # Safety
///
/// Must be called on the engine thread with a live bitmap, and `width` and `height` must be
/// the ones it was created with.
pub(crate) unsafe fn fill_bitmap(bitmap: FpdfBitmap, width: c_int, height: c_int, color: c_ulong) {
    // SAFETY: the caller guarantees the bitmap is live, that the rectangle is the bitmap's own
    // extent, and that this is the engine thread.
    unsafe { FPDFBitmap_FillRect(bitmap, 0, 0, width, height, color) }
}

/// Draw `page` into `bitmap`, filling it.
///
/// `rotate` is **0** and is not a parameter: PDFium applies the page's own `/Rotate`, and a
/// second rotation here would silently double it.
///
/// # Safety
///
/// Must be called on the engine thread with a live bitmap and a live page from the same
/// document, and `width` and `height` must be the bitmap's own.
pub(crate) unsafe fn render_page(bitmap: FpdfBitmap, page: FpdfPage, width: c_int, height: c_int) {
    // SAFETY: the caller guarantees both handles are live, that the extent is the bitmap's,
    // and that this is the engine thread. PDFium writes only inside the bitmap it was given.
    unsafe { FPDF_RenderPageBitmap(bitmap, page, 0, 0, width, height, 0, RENDER_FLAGS) }
}

/// The bitmap's pixel buffer and its row stride, read together.
///
/// The two belong together because reading the buffer without the stride is how a caller ends
/// up assuming `width * 4` -- which is right on every unpadded page and wrong on the rest.
///
/// # Safety
///
/// Must be called on the engine thread with a live bitmap. The returned pointer is owned by
/// the bitmap and dies with it.
pub(crate) unsafe fn bitmap_buffer(bitmap: FpdfBitmap) -> (*const u8, c_int) {
    // SAFETY: the caller guarantees the bitmap is live and that this is the engine thread.
    // Neither call frees anything; the pointer's lifetime is the bitmap's.
    unsafe {
        let buffer = FPDFBitmap_GetBuffer(bitmap);
        let stride = FPDFBitmap_GetStride(bitmap);
        (buffer.cast::<u8>(), stride)
    }
}

/// Free a bitmap.
///
/// # Safety
///
/// Must be called on the engine thread, exactly once per bitmap from [`create_bitmap`], and
/// after every read of its buffer has finished.
pub(crate) unsafe fn destroy_bitmap(bitmap: FpdfBitmap) {
    // SAFETY: the caller guarantees single-use, liveness, and the engine thread.
    unsafe { FPDFBitmap_Destroy(bitmap) }
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
