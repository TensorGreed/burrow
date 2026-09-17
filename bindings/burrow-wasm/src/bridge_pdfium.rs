//! The JavaScript half of the **PDFium** bridge.
//!
//! Compiled only into the `render` artifact. See `bridge_qpdf.rs` for the rules both halves
//! follow — every function below either moves bytes between two heaps or forwards one call
//! to one Emscripten export, and none of them branches on anything an engine said
//! (ADR 0009 §2).
//!
//! # This file was deleted once, and it came back with the artifact
//!
//! Spike 0004 removed these seven imports along with `pdfium.wasm`, because a `no-modules`
//! wasm-bindgen import resolves from the worker's **global scope**: an import declared
//! against a module the worker does not load fails at *instantiation*, in the browser and
//! nowhere else. That is still true, and it is now the mechanism rather than the hazard.
//! This module is `#[cfg(feature = "render")]`, the base artifact is built without that
//! feature, and so the base artifact has no PDFium import to leave dangling.
//!
//! What came back is exactly what left — `git show 1d9546a^` is the original. Nothing was
//! re-derived, because a re-derivation is where the `(code << 32) | handle` packing below
//! would have quietly become two calls again.
//!
//! [ADR 0026](../../../docs/adr/0026-how-rendering-loads-without-returning-to-the-old-payload.md) is the decision.

use burrow_core::engines::web::{LoadOutcome, PdfiumBridge, PdfiumPtr};
use wasm_bindgen::prelude::wasm_bindgen;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_name = __burrow_pdfium_copy_in)]
    fn pdfium_copy_in(bytes: &[u8]) -> u32;
    #[wasm_bindgen(js_name = __burrow_pdfium_wipe_free)]
    fn pdfium_wipe_free(ptr: u32, len: u32);
    #[wasm_bindgen(js_name = __burrow_pdfium_free_input)]
    fn pdfium_free_input(ptr: u32, len: u32);
    /// Returns `(code << 32) | handle`, so the handle and `FPDF_GetLastError` cross in one
    /// call. Splitting them into two would let another PDFium call overwrite the global in
    /// between, and the code would then belong to a different operation.
    #[wasm_bindgen(js_name = __burrow_pdfium_load)]
    fn pdfium_load(data: u32, len: u32, password: u32) -> u64;
    #[wasm_bindgen(js_name = __burrow_pdfium_pages)]
    fn pdfium_pages(doc: u32) -> i32;
    #[wasm_bindgen(js_name = __burrow_pdfium_close)]
    fn pdfium_close(doc: u32, data: u32, len: u32);
    /// The heap size in **WASM pages**, not bytes. See `pages_to_bytes`.
    #[wasm_bindgen(js_name = __burrow_pdfium_heap_pages)]
    fn pdfium_heap_pages() -> u32;
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

    fn abandon_input(&self, ptr: PdfiumPtr, len: u32) {
        pdfium_free_input(ptr.0, len);
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

    fn close_document(&self, doc: PdfiumPtr, data: PdfiumPtr, len: u32) {
        pdfium_close(doc.0, data.0, len);
    }

    fn heap_bytes(&self) -> u64 {
        crate::pages_to_bytes(pdfium_heap_pages())
    }
}
