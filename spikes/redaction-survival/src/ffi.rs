//! Engine declarations, made HERE rather than in `core/burrow-engines`.
//!
//! Spike 0006 exists partly to decide which entry points the crate should grow. Declaring them
//! in the crate first would be deciding that by writing it down. `measure-compress.rs` makes
//! the same move for the same reason; the difference is that this one lives under `spikes/`,
//! so nothing lands in `core/`.
//!
//! **Nothing here is a model for production.** There is no `Limits`, no `Deadline`, no typed
//! error, no pre-scan, and no `catch_unwind`. Every `// SAFETY:` note below is about this
//! program's own correctness, not about the invariants a shipped wrapper would have to hold.
//!
//! # Which of these burrow may actually call, which is one of the findings
//!
//! | function | on `engines/qpdf-trapped-functions.txt`? |
//! |---|---|
//! | `qpdf_read_memory`, `qpdf_get_num_pages`, `qpdf_get_page_n` | yes, direct |
//! | `qpdf_oh_get_page_content_data`, `qpdf_oh_get_stream_data` | yes, direct |
//! | `qpdf_oh_replace_stream_data` | **yes**, via `do_with_oh_void -> do_with_oh -> trap_oh_errors` |
//! | `qpdf_oh_get_key`, `qpdf_oh_get_type_code`, `qpdf_oh_get_array_*` | yes, via `do_with_oh` |
//! | `qpdf_init_write_memory`, `qpdf_write`, `qpdf_get_buffer*` | yes |
//! | `qpdf_oh_new_null`, `qpdf_oh_new_name` | **NO — neither trapped nor exempted today** |
//! | `qpdf_init`, `qpdf_cleanup`, `qpdf_silence_errors`, `qpdf_set_*` | no, and accepted in `qpdf-untrapped-accepted.toml` |
//!
//! The two `qpdf_oh_new_*` rows are the interesting ones and the spike measures around them.

#![allow(non_camel_case_types, non_snake_case, dead_code)]

use std::os::raw::{c_char, c_int, c_uint, c_ulong, c_void};

pub type qpdf_data = *mut c_void;
pub type qpdf_oh = c_uint;
pub type QPDF_ERROR_CODE = c_int;
pub type QPDF_BOOL = c_int;

pub const QPDF_DL_SPECIALIZED: c_int = 2;

unsafe extern "C" {
    pub fn qpdf_init() -> qpdf_data;
    pub fn qpdf_cleanup(qpdf: *mut qpdf_data);
    pub fn qpdf_silence_errors(qpdf: qpdf_data);
    pub fn qpdf_set_suppress_warnings(qpdf: qpdf_data, value: QPDF_BOOL);
    pub fn qpdf_set_attempt_recovery(qpdf: qpdf_data, value: QPDF_BOOL);
    pub fn qpdf_has_error(qpdf: qpdf_data) -> QPDF_BOOL;
    pub fn qpdf_get_error(qpdf: qpdf_data) -> *mut c_void;
    pub fn qpdf_read_memory(
        qpdf: qpdf_data,
        description: *const c_char,
        buffer: *const c_char,
        size: c_ulong,
        password: *const c_char,
    ) -> QPDF_ERROR_CODE;
    pub fn qpdf_get_num_pages(qpdf: qpdf_data) -> c_int;
    pub fn qpdf_get_page_n(qpdf: qpdf_data, n: usize) -> qpdf_oh;

    pub fn qpdf_oh_get_type_code(qpdf: qpdf_data, oh: qpdf_oh) -> c_int;
    pub fn qpdf_oh_get_int_value(qpdf: qpdf_data, oh: qpdf_oh) -> ::std::os::raw::c_longlong;
    pub fn qpdf_oh_get_key(qpdf: qpdf_data, oh: qpdf_oh, key: *const c_char) -> qpdf_oh;
    /// A stream's dictionary. `qpdf_oh_get_key` on a stream handle does NOT work -- getKey
    /// requires a dictionary -- which is why the thumbnail probe read `/Width` as 0 until this
    /// was declared. Trapped, `via do_with_oh -> trap_oh_errors`.
    pub fn qpdf_oh_get_dict(qpdf: qpdf_data, oh: qpdf_oh) -> qpdf_oh;
    pub fn qpdf_oh_get_array_n_items(qpdf: qpdf_data, oh: qpdf_oh) -> c_int;
    pub fn qpdf_oh_get_array_item(qpdf: qpdf_data, oh: qpdf_oh, n: c_int) -> qpdf_oh;
    pub fn qpdf_oh_get_object_id(qpdf: qpdf_data, oh: qpdf_oh) -> c_int;
    pub fn qpdf_oh_get_generation(qpdf: qpdf_data, oh: qpdf_oh) -> c_int;
    pub fn qpdf_oh_unparse_resolved(qpdf: qpdf_data, oh: qpdf_oh) -> *const c_char;

    pub fn qpdf_oh_get_page_content_data(
        qpdf: qpdf_data,
        page_oh: qpdf_oh,
        bufp: *mut *mut u8,
        len: *mut usize,
    ) -> QPDF_ERROR_CODE;
    pub fn qpdf_oh_get_stream_data(
        qpdf: qpdf_data,
        stream_oh: qpdf_oh,
        decode_level: c_int,
        filtered: *mut QPDF_BOOL,
        bufp: *mut *mut u8,
        len: *mut usize,
    ) -> QPDF_ERROR_CODE;
    /// The write verb. Trapped, and declared nowhere in `core/` today.
    pub fn qpdf_oh_replace_stream_data(
        qpdf: qpdf_data,
        stream_oh: qpdf_oh,
        buf: *const u8,
        len: usize,
        filter: qpdf_oh,
        decode_parms: qpdf_oh,
    );
    pub fn qpdf_oh_free_buffer(bufp: *mut *mut u8);

    /// NOT trapped and NOT exempted. Called here only to measure whether the alternative
    /// route (a missing key, which yields a null handle through a trapped function) is
    /// equivalent -- see `qpdf::null_handle`.
    pub fn qpdf_oh_new_null(qpdf: qpdf_data) -> qpdf_oh;

    pub fn qpdf_init_write_memory(qpdf: qpdf_data) -> QPDF_ERROR_CODE;
    pub fn qpdf_set_deterministic_ID(qpdf: qpdf_data, value: QPDF_BOOL);
    pub fn qpdf_set_object_stream_mode(qpdf: qpdf_data, mode: c_int);
    pub fn qpdf_write(qpdf: qpdf_data) -> QPDF_ERROR_CODE;
    pub fn qpdf_get_buffer_length(qpdf: qpdf_data) -> usize;
    pub fn qpdf_get_buffer(qpdf: qpdf_data) -> *const u8;
}

// ---------------------------------------------------------------------------
// PDFium
// ---------------------------------------------------------------------------

pub type FPDF_DOCUMENT = *mut c_void;
pub type FPDF_PAGE = *mut c_void;
pub type FPDF_TEXTPAGE = *mut c_void;
pub type FPDF_BITMAP = *mut c_void;
pub type FPDF_BOOL = c_int;

pub const FPDFBITMAP_BGRA: c_int = 4;
/// `fpdfview.h`: draw annotation appearance streams. burrow's render passes 0 instead.
pub const FPDF_ANNOT: c_int = 0x01;

unsafe extern "C" {
    pub fn FPDF_InitLibrary();
    pub fn FPDF_GetLastError() -> c_ulong;
    pub fn FPDF_LoadMemDocument64(
        data: *const c_void,
        size: usize,
        password: *const c_char,
    ) -> FPDF_DOCUMENT;
    pub fn FPDF_GetPageCount(document: FPDF_DOCUMENT) -> c_int;
    pub fn FPDF_CloseDocument(document: FPDF_DOCUMENT);
    pub fn FPDF_LoadPage(document: FPDF_DOCUMENT, index: c_int) -> FPDF_PAGE;
    pub fn FPDF_ClosePage(page: FPDF_PAGE);
    pub fn FPDF_GetPageWidthF(page: FPDF_PAGE) -> f32;
    pub fn FPDF_GetPageHeightF(page: FPDF_PAGE) -> f32;

    // fpdf_text.h -- ZERO references anywhere in the repository outside engines/vendor.
    pub fn FPDFText_LoadPage(page: FPDF_PAGE) -> FPDF_TEXTPAGE;
    pub fn FPDFText_ClosePage(text_page: FPDF_TEXTPAGE);
    pub fn FPDFText_CountChars(text_page: FPDF_TEXTPAGE) -> c_int;
    pub fn FPDFText_GetUnicode(text_page: FPDF_TEXTPAGE, index: c_int) -> c_uint;
    pub fn FPDFText_GetCharBox(
        text_page: FPDF_TEXTPAGE,
        index: c_int,
        left: *mut f64,
        right: *mut f64,
        bottom: *mut f64,
        top: *mut f64,
    ) -> FPDF_BOOL;
    pub fn FPDFText_GetText(
        text_page: FPDF_TEXTPAGE,
        start: c_int,
        count: c_int,
        result: *mut u16,
    ) -> c_int;

    pub fn FPDFBitmap_Create(width: c_int, height: c_int, alpha: c_int) -> FPDF_BITMAP;
    pub fn FPDFBitmap_FillRect(
        bitmap: FPDF_BITMAP,
        left: c_int,
        top: c_int,
        width: c_int,
        height: c_int,
        color: c_ulong,
    );
    pub fn FPDFBitmap_GetBuffer(bitmap: FPDF_BITMAP) -> *mut c_void;
    pub fn FPDFBitmap_GetStride(bitmap: FPDF_BITMAP) -> c_int;
    pub fn FPDFBitmap_Destroy(bitmap: FPDF_BITMAP);
    pub fn FPDF_RenderPageBitmap(
        bitmap: FPDF_BITMAP,
        page: FPDF_PAGE,
        start_x: c_int,
        start_y: c_int,
        size_x: c_int,
        size_y: c_int,
        rotate: c_int,
        flags: c_int,
    );
}
