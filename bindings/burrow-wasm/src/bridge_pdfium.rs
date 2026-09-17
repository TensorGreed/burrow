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

    // ---- the render path (#57) ---------------------------------------------------------
    //
    // Eleven imports, one per Emscripten export, and nothing else. The one-call temptation
    // is named in `PdfiumBridge`'s docs and refused there; what this file adds is the
    // observation that these are also the imports that make the SPLIT work. A base artifact
    // built with `render` off declares none of them, so there is nothing for the base
    // worker's global scope to fail to resolve -- which is ADR 0026's mechanism, applied to
    // a surface that is now eleven names larger than it was.

    #[wasm_bindgen(js_name = __burrow_pdfium_load_page)]
    fn pdfium_load_page(doc: u32, index: i32) -> u32;
    #[wasm_bindgen(js_name = __burrow_pdfium_close_page)]
    fn pdfium_close_page(page: u32);
    #[wasm_bindgen(js_name = __burrow_pdfium_page_width)]
    fn pdfium_page_width(page: u32) -> f32;
    #[wasm_bindgen(js_name = __burrow_pdfium_page_height)]
    fn pdfium_page_height(page: u32) -> f32;
    #[wasm_bindgen(js_name = __burrow_pdfium_bitmap_create)]
    fn pdfium_bitmap_create(width: i32, height: i32, alpha: i32) -> u32;
    #[wasm_bindgen(js_name = __burrow_pdfium_bitmap_fill_rect)]
    fn pdfium_bitmap_fill_rect(
        bitmap: u32,
        left: i32,
        top: i32,
        width: i32,
        height: i32,
        color: u32,
    );
    #[wasm_bindgen(js_name = __burrow_pdfium_render_page_bitmap)]
    fn pdfium_render_page_bitmap(
        bitmap: u32,
        page: u32,
        start_x: i32,
        start_y: i32,
        size_x: i32,
        size_y: i32,
        rotate: i32,
        flags: i32,
    );
    #[wasm_bindgen(js_name = __burrow_pdfium_bitmap_buffer)]
    fn pdfium_bitmap_buffer(bitmap: u32) -> u32;
    #[wasm_bindgen(js_name = __burrow_pdfium_bitmap_stride)]
    fn pdfium_bitmap_stride(bitmap: u32) -> i32;
    #[wasm_bindgen(js_name = __burrow_pdfium_bitmap_destroy)]
    fn pdfium_bitmap_destroy(bitmap: u32);
    /// Copy `len` bytes out of the PDFium heap. The first PDFium import carrying bytes OUT.
    #[wasm_bindgen(js_name = __burrow_pdfium_copy_out)]
    fn pdfium_copy_out(ptr: u32, len: u32) -> Vec<u8>;
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

    // ---- the render path (#57) ---------------------------------------------------------
    //
    // Every one of these forwards and nothing more. No null check, no length arithmetic, no
    // choice of format: the nulls are checked in `burrow-engines`, the stride arithmetic is
    // there too, and `BITMAP_BGRA`, the fill colour, the flags and the zero rotation are all
    // passed in from Rust. ADR 0009 §2 -- and a reader auditing this file should be able to
    // confirm that by reading it, which is why the bodies are one line each.

    fn load_page(&self, doc: PdfiumPtr, index: i32) -> PdfiumPtr {
        PdfiumPtr(pdfium_load_page(doc.0, index))
    }

    fn close_page(&self, page: PdfiumPtr) {
        pdfium_close_page(page.0);
    }

    fn page_width(&self, page: PdfiumPtr) -> f32 {
        pdfium_page_width(page.0)
    }

    fn page_height(&self, page: PdfiumPtr) -> f32 {
        pdfium_page_height(page.0)
    }

    fn bitmap_create(&self, width: i32, height: i32, alpha: i32) -> PdfiumPtr {
        PdfiumPtr(pdfium_bitmap_create(width, height, alpha))
    }

    fn bitmap_fill_rect(
        &self,
        bitmap: PdfiumPtr,
        left: i32,
        top: i32,
        width: i32,
        height: i32,
        color: u32,
    ) {
        pdfium_bitmap_fill_rect(bitmap.0, left, top, width, height, color);
    }

    fn render_page_bitmap(
        &self,
        bitmap: PdfiumPtr,
        page: PdfiumPtr,
        start_x: i32,
        start_y: i32,
        size_x: i32,
        size_y: i32,
        rotate: i32,
        flags: i32,
    ) {
        pdfium_render_page_bitmap(
            bitmap.0, page.0, start_x, start_y, size_x, size_y, rotate, flags,
        );
    }

    fn bitmap_buffer(&self, bitmap: PdfiumPtr) -> PdfiumPtr {
        PdfiumPtr(pdfium_bitmap_buffer(bitmap.0))
    }

    fn bitmap_stride(&self, bitmap: PdfiumPtr) -> i32 {
        pdfium_bitmap_stride(bitmap.0)
    }

    fn bitmap_destroy(&self, bitmap: PdfiumPtr) {
        pdfium_bitmap_destroy(bitmap.0);
    }

    fn copy_out(&self, ptr: PdfiumPtr, len: u32) -> Vec<u8> {
        pdfium_copy_out(ptr.0, len)
    }
}
