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

use burrow_core::engines::web::{QpdfBridge, QpdfPtr};
use wasm_bindgen::prelude::wasm_bindgen;

#[wasm_bindgen]
extern "C" {
    // NO PDFIUM. Seven `__burrow_pdfium_*` imports stood here until spike 0004 took PDFium
    // out of the web payload --- 79.7% of the first load, reachable from one function
    // (`page_count`), which qpdf answers through `StructureEngine::check`.
    //
    // THE IMPORTS HAD TO GO WITH THE ARTIFACT, not merely stop being used. A `no-modules`
    // wasm-bindgen import resolves from the worker's global scope, and those globals are
    // defined by `bridge.js` from the PDFium Emscripten module --- so leaving them declared
    // against a module that is no longer loaded would fail at INSTANTIATION, in the browser
    // and nowhere else. ADR 0009 §2: this list is the audit surface, so it says what is
    // actually there.
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
    #[wasm_bindgen(js_name = __burrow_qpdf_global_set_uint32)]
    fn qpdf_global_set_uint32(param: i32, value: u32) -> i32;
    // The write path, M1 PR B2. `copy_out` is the only one whose SHAPE is new: it is the
    // first import that carries bytes back out of an engine heap.
    #[wasm_bindgen(js_name = __burrow_qpdf_get_page_n)]
    fn qpdf_get_page_n(data: u32, n: u32) -> u32;
    #[wasm_bindgen(js_name = __burrow_qpdf_add_page)]
    fn qpdf_add_page(data: u32, source: u32, page: u32, first: u32) -> i32;
    #[wasm_bindgen(js_name = __burrow_qpdf_init_write_memory)]
    fn qpdf_init_write_memory(data: u32) -> i32;
    #[wasm_bindgen(js_name = __burrow_qpdf_set_deterministic_id)]
    fn qpdf_set_deterministic_id(data: u32, value: u32);
    #[wasm_bindgen(js_name = __burrow_qpdf_set_object_stream_mode)]
    fn qpdf_set_object_stream_mode(data: u32, mode: u32);
    // The object-handle API, added for `rotate`. `key` is a pointer to a NUL-terminated
    // string already in the module's heap: the caller copies it in, so no string crosses
    // this boundary and nothing on the JS side builds one.
    #[wasm_bindgen(js_name = __burrow_qpdf_oh_get_key)]
    fn qpdf_oh_get_key(data: u32, oh: u32, key: u32) -> u32;
    #[wasm_bindgen(js_name = __burrow_qpdf_oh_get_type_code)]
    fn qpdf_oh_get_type_code(data: u32, oh: u32) -> i32;
    #[wasm_bindgen(js_name = __burrow_qpdf_oh_get_int_value)]
    fn qpdf_oh_get_int_value(data: u32, oh: u32) -> i64;
    #[wasm_bindgen(js_name = __burrow_qpdf_oh_new_integer)]
    fn qpdf_oh_new_integer(data: u32, value: i64) -> u32;
    #[wasm_bindgen(js_name = __burrow_qpdf_oh_replace_key)]
    fn qpdf_oh_replace_key(data: u32, oh: u32, key: u32, item: u32);
    #[wasm_bindgen(js_name = __burrow_qpdf_oh_new_null)]
    fn qpdf_oh_new_null(data: u32) -> u32;
    // REDACTION'S WRITE (#130, ADR 0029 SS1), and the only import here that carries bytes INTO
    // a live object graph. It takes a `&[u8]`, so wasm-bindgen copies the slice into a JS
    // `Uint8Array` at the boundary and the JS side copies that into the engine heap -- two
    // copies of a content stream, which is the cost of not handing a raw pointer across.
    // `false` means the engine could not take the bytes, which on the web is an allocation
    // failure and is a refusal rather than a panic.
    #[wasm_bindgen(js_name = __burrow_qpdf_oh_replace_stream_data)]
    fn qpdf_oh_replace_stream_data(
        data: u32,
        stream: u32,
        bytes: &[u8],
        filter: u32,
        decode_parms: u32,
    ) -> bool;
    #[wasm_bindgen(js_name = __burrow_qpdf_remove_page)]
    fn qpdf_remove_page(data: u32, page: u32) -> i32;
    #[wasm_bindgen(js_name = __burrow_qpdf_add_page_at)]
    fn qpdf_add_page_at(data: u32, source: u32, page: u32, before: u32, refpage: u32) -> i32;
    /// `(object_number << 32) | generation`, so the two halves of an object's identity cross
    /// in one call and cannot be used apart. See `QpdfBridge::oh_object`.
    #[wasm_bindgen(js_name = __burrow_qpdf_oh_object)]
    fn qpdf_oh_object(data: u32, oh: u32) -> u64;
    // ---- split's pruning ------------------------------------------------------------
    //
    // `__burrow_qpdf_oh_page_content` and `__burrow_qpdf_oh_stream_data` return the DATA
    // rather than a pointer, and free qpdf's buffer on the JS side. Those two are the only
    // qpdf calls whose buffer belongs to the caller rather than to qpdf, and keeping the
    // free next to the allocation is what stops that ownership rule becoming a discipline
    // every caller has to remember. `null` means qpdf reported an error or could not decode.
    #[wasm_bindgen(js_name = __burrow_qpdf_oh_unparse_resolved)]
    fn qpdf_oh_unparse_resolved(data: u32, oh: u32) -> u32;
    #[wasm_bindgen(js_name = __burrow_qpdf_oh_get_name)]
    fn qpdf_oh_get_name(data: u32, oh: u32) -> u32;
    #[wasm_bindgen(js_name = __burrow_qpdf_oh_remove_key)]
    fn qpdf_oh_remove_key(data: u32, oh: u32, key: u32);
    #[wasm_bindgen(js_name = __burrow_qpdf_oh_get_array_n_items)]
    fn qpdf_oh_get_array_n_items(data: u32, oh: u32) -> i32;
    #[wasm_bindgen(js_name = __burrow_qpdf_oh_get_array_item)]
    fn qpdf_oh_get_array_item(data: u32, oh: u32, at: i32) -> u32;
    #[wasm_bindgen(js_name = __burrow_qpdf_oh_erase_item)]
    fn qpdf_oh_erase_item(data: u32, oh: u32, at: i32);
    #[wasm_bindgen(js_name = __burrow_qpdf_oh_get_dict)]
    fn qpdf_oh_get_dict(data: u32, oh: u32) -> u32;
    #[wasm_bindgen(js_name = __burrow_qpdf_oh_page_content)]
    fn qpdf_oh_page_content(data: u32, page: u32) -> Option<Vec<u8>>;
    #[wasm_bindgen(js_name = __burrow_qpdf_oh_stream_data)]
    fn qpdf_oh_stream_data(data: u32, oh: u32) -> Option<Vec<u8>>;
    #[wasm_bindgen(js_name = __burrow_qpdf_copy_c_string)]
    fn qpdf_copy_c_string(ptr: u32) -> Vec<u8>;

    #[wasm_bindgen(js_name = __burrow_qpdf_oh_release)]
    fn qpdf_oh_release(data: u32, oh: u32);
    #[wasm_bindgen(js_name = __burrow_qpdf_write)]
    fn qpdf_write(data: u32) -> i32;
    #[wasm_bindgen(js_name = __burrow_qpdf_get_buffer_length)]
    fn qpdf_get_buffer_length(data: u32) -> u32;
    #[wasm_bindgen(js_name = __burrow_qpdf_get_buffer)]
    fn qpdf_get_buffer(data: u32) -> u32;
    #[wasm_bindgen(js_name = __burrow_qpdf_copy_out)]
    fn qpdf_copy_out(ptr: u32, len: u32) -> Vec<u8>;
    #[wasm_bindgen(js_name = __burrow_qpdflogger_create)]
    fn qpdflogger_create() -> u32;
    #[wasm_bindgen(js_name = __burrow_qpdflogger_discard_all)]
    fn qpdflogger_discard_all(logger: u32, destination: i32);
    /// The heap size in **WASM pages**, not bytes. See `pages_to_bytes`.
    #[wasm_bindgen(js_name = __burrow_qpdf_heap_pages)]
    fn qpdf_heap_pages() -> u32;
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

    fn global_set_uint32(&self, param: i32, value: u32) -> i32 {
        qpdf_global_set_uint32(param, value)
    }

    fn logger_create(&self) -> QpdfPtr {
        QpdfPtr(qpdflogger_create())
    }

    fn logger_discard_all(&self, logger: QpdfPtr, destination: i32) {
        qpdflogger_discard_all(logger.0, destination);
    }

    fn get_page_n(&self, data: QpdfPtr, n: u32) -> u32 {
        qpdf_get_page_n(data.0, n)
    }

    fn remove_page(&self, data: QpdfPtr, page: u32) -> i32 {
        qpdf_remove_page(data.0, page)
    }

    fn add_page_at(
        &self,
        data: QpdfPtr,
        source: QpdfPtr,
        page: u32,
        before: bool,
        refpage: u32,
    ) -> i32 {
        qpdf_add_page_at(data.0, source.0, page, u32::from(before), refpage)
    }

    fn oh_object(&self, data: QpdfPtr, oh: u32) -> u64 {
        qpdf_oh_object(data.0, oh)
    }

    fn add_page(&self, data: QpdfPtr, source: QpdfPtr, page: u32, first: bool) -> i32 {
        // `u32::from(bool)` rather than a JS boolean: every other import in this block
        // passes numbers, and QPDF_BOOL is an int on the other side. One representation
        // across the boundary is one fewer thing to get wrong.
        qpdf_add_page(data.0, source.0, page, u32::from(first))
    }

    fn init_write_memory(&self, data: QpdfPtr) -> i32 {
        qpdf_init_write_memory(data.0)
    }

    fn set_deterministic_id(&self, data: QpdfPtr, value: bool) {
        qpdf_set_deterministic_id(data.0, u32::from(value));
    }

    fn set_object_stream_mode(&self, data: QpdfPtr, mode: u32) {
        qpdf_set_object_stream_mode(data.0, mode);
    }

    fn write(&self, data: QpdfPtr) -> i32 {
        qpdf_write(data.0)
    }

    fn get_buffer_length(&self, data: QpdfPtr) -> u32 {
        qpdf_get_buffer_length(data.0)
    }

    fn get_buffer(&self, data: QpdfPtr) -> QpdfPtr {
        QpdfPtr(qpdf_get_buffer(data.0))
    }

    fn copy_out(&self, ptr: QpdfPtr, len: u32) -> Vec<u8> {
        qpdf_copy_out(ptr.0, len)
    }

    fn oh_get_key(&self, data: QpdfPtr, oh: u32, key: QpdfPtr) -> u32 {
        qpdf_oh_get_key(data.0, oh, key.0)
    }

    fn oh_unparse_resolved(&self, data: QpdfPtr, oh: u32) -> QpdfPtr {
        QpdfPtr(qpdf_oh_unparse_resolved(data.0, oh))
    }

    fn oh_get_name(&self, data: QpdfPtr, oh: u32) -> QpdfPtr {
        QpdfPtr(qpdf_oh_get_name(data.0, oh))
    }

    fn oh_remove_key(&self, data: QpdfPtr, oh: u32, key: QpdfPtr) {
        qpdf_oh_remove_key(data.0, oh, key.0);
    }

    fn oh_get_array_n_items(&self, data: QpdfPtr, oh: u32) -> i32 {
        qpdf_oh_get_array_n_items(data.0, oh)
    }

    fn oh_get_array_item(&self, data: QpdfPtr, oh: u32, at: i32) -> u32 {
        qpdf_oh_get_array_item(data.0, oh, at)
    }

    fn oh_erase_item(&self, data: QpdfPtr, oh: u32, at: i32) {
        qpdf_oh_erase_item(data.0, oh, at);
    }

    fn oh_get_dict(&self, data: QpdfPtr, oh: u32) -> u32 {
        qpdf_oh_get_dict(data.0, oh)
    }

    fn oh_get_int_value_i64(&self, data: QpdfPtr, oh: u32) -> i64 {
        qpdf_oh_get_int_value(data.0, oh)
    }

    fn oh_new_null(&self, data: QpdfPtr) -> u32 {
        qpdf_oh_new_null(data.0)
    }

    fn oh_replace_stream_data(
        &self,
        data: QpdfPtr,
        stream: u32,
        bytes: &[u8],
        filter: u32,
        decode_parms: u32,
    ) -> bool {
        qpdf_oh_replace_stream_data(data.0, stream, bytes, filter, decode_parms)
    }

    fn oh_page_content(&self, data: QpdfPtr, page: u32) -> Option<Vec<u8>> {
        qpdf_oh_page_content(data.0, page)
    }

    fn oh_stream_data(&self, data: QpdfPtr, oh: u32) -> Option<Vec<u8>> {
        qpdf_oh_stream_data(data.0, oh)
    }

    fn copy_c_string(&self, ptr: QpdfPtr) -> Vec<u8> {
        qpdf_copy_c_string(ptr.0)
    }

    fn oh_get_type_code(&self, data: QpdfPtr, oh: u32) -> i32 {
        qpdf_oh_get_type_code(data.0, oh)
    }

    fn oh_get_int_value(&self, data: QpdfPtr, oh: u32) -> i64 {
        qpdf_oh_get_int_value(data.0, oh)
    }

    fn oh_new_integer(&self, data: QpdfPtr, value: i64) -> u32 {
        qpdf_oh_new_integer(data.0, value)
    }

    fn oh_replace_key(&self, data: QpdfPtr, oh: u32, key: QpdfPtr, item: u32) {
        qpdf_oh_replace_key(data.0, oh, key.0, item);
    }

    fn oh_release(&self, data: QpdfPtr, oh: u32) {
        qpdf_oh_release(data.0, oh);
    }

    fn heap_bytes(&self) -> u64 {
        crate::pages_to_bytes(qpdf_heap_pages())
    }
}
