//! The JavaScript half of the engine bridge.
//!
//! This is the only file in the project that is allowed to be JavaScript-shaped, and it is
//! deliberately dull. Every function below either moves bytes between two heaps or forwards
//! one call to one Emscripten export. None of them branches on anything an engine said.
//!
//! # What ADR 0009 permits, and how that is visible here
//!
//! > Bindings may hold engine handles and marshal data across the boundary. **No branch on
//! > engine state may live in JS.**
//!
//! The shape that enforces it is in `burrow_core::engines::web::bridge`: the traits list
//! every capability the binding has, so this file cannot add one without a reviewer seeing
//! a new method appear. What is here is the mechanical half — `_malloc`, `HEAPU8.set`,
//! `_free`, and a call — and the decisions all live in `burrow-engines`, which is the same
//! Rust the native path runs.
//!
//! Two details are worth knowing before reading further:
//!
//! - **No `module = "..."` attribute.** The imports resolve from the worker's global scope.
//!   That is the only style `wasm-pack --target no-modules` supports, and `no-modules` is
//!   forced on us: the prebuilt `pdfium.js` is not modularised, so it can only be loaded by
//!   `importScripts`, which exists only in a **classic** worker — and a classic worker
//!   cannot `import` an ES module.
//! - **Every import is synchronous.** After the memoised init promise resolves, an
//!   Emscripten export is an ordinary function call, so a Rust → JS → wasm → JS → Rust round
//!   trip has no suspension point. This is what lets `DocumentEngine`, a synchronous trait,
//!   be implemented at all. An `-sASYNCIFY` or JSPI build would break it, which is why
//!   `engines/build-wasm.sh` fails if either appears in the glue.

use burrow_core::engines::web::{LoadOutcome, PdfiumBridge, PdfiumPtr, QpdfBridge, QpdfPtr};
use wasm_bindgen::prelude::wasm_bindgen;

#[wasm_bindgen]
extern "C" {
    // --- PDFium -------------------------------------------------------------------
    #[wasm_bindgen(js_name = __burrow_pdfium_copy_in)]
    fn pdfium_copy_in(bytes: &[u8]) -> u32;
    #[wasm_bindgen(js_name = __burrow_pdfium_wipe_free)]
    fn pdfium_wipe_free(ptr: u32, len: u32);
    #[wasm_bindgen(js_name = __burrow_pdfium_free_input)]
    fn pdfium_free_input(ptr: u32);
    /// Returns `(code << 32) | handle`, so the handle and `FPDF_GetLastError` cross in one
    /// call. Splitting them into two would let another PDFium call overwrite the global in
    /// between, and the code would then belong to a different operation.
    #[wasm_bindgen(js_name = __burrow_pdfium_load)]
    fn pdfium_load(data: u32, len: u32, password: u32) -> u64;
    #[wasm_bindgen(js_name = __burrow_pdfium_pages)]
    fn pdfium_pages(doc: u32) -> i32;
    #[wasm_bindgen(js_name = __burrow_pdfium_close)]
    fn pdfium_close(doc: u32, data: u32);
    /// The heap size in **WASM pages**, not bytes. See `pages_to_bytes`.
    #[wasm_bindgen(js_name = __burrow_pdfium_heap_pages)]
    fn pdfium_heap_pages() -> u32;

    // --- qpdf ---------------------------------------------------------------------
    #[wasm_bindgen(js_name = __burrow_qpdf_copy_in)]
    fn qpdf_copy_in(bytes: &[u8]) -> u32;
    #[wasm_bindgen(js_name = __burrow_qpdf_free)]
    fn qpdf_free(ptr: u32);
    #[wasm_bindgen(js_name = __burrow_qpdf_wipe_free)]
    fn qpdf_wipe_free(ptr: u32, len: u32);
    #[wasm_bindgen(js_name = __burrow_qpdf_init)]
    fn qpdf_init() -> u32;
    #[wasm_bindgen(js_name = __burrow_qpdf_cleanup)]
    fn qpdf_cleanup(data: u32);
    #[wasm_bindgen(js_name = __burrow_qpdf_silence_errors)]
    fn qpdf_silence_errors(data: u32);
    #[wasm_bindgen(js_name = __burrow_qpdf_set_suppress_warnings)]
    fn qpdf_set_suppress_warnings(data: u32, value: i32);
    #[wasm_bindgen(js_name = __burrow_qpdf_set_logger)]
    fn qpdf_set_logger(data: u32, logger: u32);
    #[wasm_bindgen(js_name = __burrow_qpdf_set_attempt_recovery)]
    fn qpdf_set_attempt_recovery(data: u32, value: i32);
    #[wasm_bindgen(js_name = __burrow_qpdf_read_memory)]
    fn qpdf_read_memory(data: u32, description: u32, buffer: u32, size: u64, password: u32) -> i32;
    #[wasm_bindgen(js_name = __burrow_qpdf_has_error)]
    fn qpdf_has_error(data: u32) -> i32;
    #[wasm_bindgen(js_name = __burrow_qpdf_get_error)]
    fn qpdf_get_error(data: u32) -> u32;
    #[wasm_bindgen(js_name = __burrow_qpdf_get_error_code)]
    fn qpdf_get_error_code(data: u32, error: u32) -> i32;
    #[wasm_bindgen(js_name = __burrow_qpdf_get_num_pages)]
    fn qpdf_get_num_pages(data: u32) -> i32;
    #[wasm_bindgen(js_name = __burrow_qpdflogger_create)]
    fn qpdflogger_create() -> u32;
    #[wasm_bindgen(js_name = __burrow_qpdflogger_discard_all)]
    fn qpdflogger_discard_all(logger: u32);
    /// The heap size in **WASM pages**, not bytes. See `pages_to_bytes`.
    #[wasm_bindgen(js_name = __burrow_qpdf_heap_pages)]
    fn qpdf_heap_pages() -> u32;
}

/// Bytes in one WebAssembly page.
const WASM_PAGE_BYTES: u64 = 64 * 1024;

/// Convert a heap size reported in WASM pages to bytes.
///
/// **The heap crosses as a page count, not a byte count, and that is deliberate.** A byte
/// count would have to arrive as a JavaScript `Number` — an `f64` — and converting a float
/// to an integer needs exactly the casts this workspace denies (`cast_possible_truncation`,
/// `cast_sign_loss`), because silent numeric damage to a size is a real bug class. Silencing
/// the lint here would have been the easy fix and the wrong one.
///
/// A page count avoids the problem rather than suppressing it: `HEAPU8.byteLength` is always
/// a whole number of 64 KiB pages, so `byteLength / 65536` is an exact small integer that
/// crosses as a `u32` with nothing lost. `-sMAXIMUM_MEMORY=2GB` caps it at 32,768.
///
/// Saturating, so a nonsensical reading becomes a large number rather than wrapping to a
/// small one — the safe direction for a value a limit check rejects on.
fn pages_to_bytes(pages: u32) -> u64 {
    u64::from(pages).saturating_mul(WASM_PAGE_BYTES)
}

/// The PDFium Emscripten module.
pub(crate) struct JsPdfium;

impl PdfiumBridge for JsPdfium {
    fn copy_in(&self, bytes: &[u8]) -> PdfiumPtr {
        PdfiumPtr(pdfium_copy_in(bytes))
    }

    fn wipe_and_free(&self, ptr: PdfiumPtr, len: u32) {
        pdfium_wipe_free(ptr.0, len);
    }

    fn abandon_input(&self, ptr: PdfiumPtr) {
        pdfium_free_input(ptr.0);
    }

    fn load_mem_document64(&self, data: PdfiumPtr, len: u32, password: PdfiumPtr) -> LoadOutcome {
        let packed = pdfium_load(data.0, len, password.0);
        // The unpacking is arithmetic, not a decision: the low word is the handle and the
        // high word is the error code, and which is which was fixed when they were packed.
        LoadOutcome {
            handle: PdfiumPtr(u32::try_from(packed & 0xFFFF_FFFF).unwrap_or(0)),
            code: core::ffi::c_ulong::try_from(packed >> 32).unwrap_or(0),
        }
    }

    fn get_page_count(&self, doc: PdfiumPtr) -> i32 {
        pdfium_pages(doc.0)
    }

    fn close_document(&self, doc: PdfiumPtr, data: PdfiumPtr) {
        pdfium_close(doc.0, data.0);
    }

    fn heap_bytes(&self) -> u64 {
        pages_to_bytes(pdfium_heap_pages())
    }
}

/// The qpdf Emscripten module.
pub(crate) struct JsQpdf;

impl QpdfBridge for JsQpdf {
    fn copy_in(&self, bytes: &[u8]) -> QpdfPtr {
        QpdfPtr(qpdf_copy_in(bytes))
    }

    fn free(&self, ptr: QpdfPtr) {
        qpdf_free(ptr.0);
    }

    fn wipe_and_free(&self, ptr: QpdfPtr, len: u32) {
        qpdf_wipe_free(ptr.0, len);
    }

    fn init(&self) -> QpdfPtr {
        QpdfPtr(qpdf_init())
    }

    fn cleanup(&self, data: QpdfPtr) {
        qpdf_cleanup(data.0);
    }

    fn silence_errors(&self, data: QpdfPtr) {
        qpdf_silence_errors(data.0);
    }

    fn set_suppress_warnings(&self, data: QpdfPtr, value: bool) {
        qpdf_set_suppress_warnings(data.0, i32::from(value));
    }

    fn set_logger(&self, data: QpdfPtr, logger: QpdfPtr) {
        qpdf_set_logger(data.0, logger.0);
    }

    fn set_attempt_recovery(&self, data: QpdfPtr, value: bool) {
        qpdf_set_attempt_recovery(data.0, i32::from(value));
    }

    fn read_memory(
        &self,
        data: QpdfPtr,
        description: QpdfPtr,
        buffer: QpdfPtr,
        size: u64,
        password: QpdfPtr,
    ) -> i32 {
        // The size crosses as a `Number`, which the JS side widens with `BigInt(...)`
        // because the module is built `-sWASM_BIGINT=1`. Exact: the value is bounded by
        // `max_input_bytes`, which is far below 2^53.
        qpdf_read_memory(data.0, description.0, buffer.0, size, password.0)
    }

    fn has_error(&self, data: QpdfPtr) -> bool {
        // qpdf's QPDF_BOOL is an int. Anything non-zero is true; deciding that here rather
        // than in JS keeps the comparison on the Rust side even though it is trivial.
        qpdf_has_error(data.0) != 0
    }

    fn get_error(&self, data: QpdfPtr) -> QpdfPtr {
        QpdfPtr(qpdf_get_error(data.0))
    }

    fn get_error_code(&self, data: QpdfPtr, error: QpdfPtr) -> i32 {
        qpdf_get_error_code(data.0, error.0)
    }

    fn get_num_pages(&self, data: QpdfPtr) -> i32 {
        qpdf_get_num_pages(data.0)
    }

    fn logger_create(&self) -> QpdfPtr {
        QpdfPtr(qpdflogger_create())
    }

    fn logger_discard_all(&self, logger: QpdfPtr) {
        qpdflogger_discard_all(logger.0);
    }

    fn heap_bytes(&self) -> u64 {
        pages_to_bytes(qpdf_heap_pages())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_count_becomes_the_right_number_of_bytes() {
        assert_eq!(pages_to_bytes(0), 0);
        assert_eq!(pages_to_bytes(1), 65_536);
        assert_eq!(pages_to_bytes(256), 16_777_216);
        // -sMAXIMUM_MEMORY=2GB is 32,768 pages.
        assert_eq!(pages_to_bytes(32_768), 2 * 1024 * 1024 * 1024);
    }

    /// A nonsensical reading must not wrap into a plausible small number: the measured
    /// memory check rejects on this value, so under-reporting hides a real overshoot.
    #[test]
    fn an_absurd_page_count_saturates_rather_than_wrapping() {
        assert_eq!(pages_to_bytes(u32::MAX), u64::from(u32::MAX) * 65_536);
        assert!(pages_to_bytes(u32::MAX) > pages_to_bytes(32_768));
    }
}
