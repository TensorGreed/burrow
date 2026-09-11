//! qpdf's C API, declared by hand.
//!
//! Every declaration carries the line of
//! `engines/vendor/native-*/include/qpdf/{qpdf-c.h,qpdflogger-c.h,Constants.h}` it was
//! checked against. **Nothing verifies these automatically, and a wrong signature is
//! undefined behaviour no test would catch — re-check them against the headers on any
//! qpdf bump.**
//!
//! # Which functions are safe to call, and why it is not all of them
//!
//! qpdf is a C++ library that signals failure by throwing, and an exception unwinding into
//! Rust is undefined behaviour. `qpdf-c.h:113-115` reads like a blanket guarantee that the
//! C API converts every exception into an error code:
//!
//! > "If you encounter a situation where an exception from the C++ code is not properly
//! > converted to an error as described above, it is a bug in qpdf."
//!
//! **That is not true of every function, and believing it cost this crate a process
//! abort.** Reading `libqpdf/qpdf-c.cc`: the `trap_errors` helper (`qpdf-c.cc:69`) is what
//! does the catching, and only functions that route through it are covered.
//! `qpdf_is_linearized` (`qpdf-c.cc:382`) does not — it is a bare
//! `return qpdf->qpdf->isLinearized()`. Its `isLinearized()` converts an object number
//! with `QIntC::to_int`, which throws `std::range_error` above `INT_MAX`, so an ordinary
//! 356-byte PDF containing `9999999999 0 obj` kills the process:
//!
//! ```text
//! fatal runtime error: Rust cannot catch foreign exceptions, aborting
//! ```
//!
//! Measured, end to end through `Qpdf::check`. No `catch_unwind` helps: a foreign
//! exception is not a Rust panic.
//!
//! ## The rule this module follows
//!
//! **Only call a function verified to route through `trap_errors`.** Verified by reading
//! `qpdf-c.cc`, function by function, not by reading the header's prose:
//!
//! | function | trapped? | |
//! |---|---|---|
//! | `qpdf_read_memory` | yes | `qpdf-c.cc:297` |
//! | `qpdf_get_num_pages` | yes | `qpdf-c.cc:1801` |
//! | `qpdf_is_encrypted` | **no** | `qpdf-c.cc:389` — not declared here |
//! | `qpdf_is_linearized` | **no** | `qpdf-c.cc:382` — not declared here |
//!
//! The remaining declarations (`qpdf_init`, `qpdf_cleanup`, the setters, the error
//! accessors, the logger and the globals) do no parsing: they assign fields, flip flags or
//! read a stored value. They can fail only by allocation, which on Rust 1.81+ is a defined
//! abort rather than undefined behaviour.
//!
//! So these are plain `extern "C"`, and that is now a checked claim rather than an
//! inherited one. Re-verify on any qpdf bump: the header will not tell you.
//!
//! ## Two caveats on that rule
//!
//! **It is about `qpdf-c.h`.** The four `qpdflogger-c.h` functions declared below live in
//! `qpdflogger-c.cc`, which contains no try/catch at all — `qpdflogger_create` does a
//! `new`, and the setters build a `std::function`. They can throw only `std::bad_alloc`,
//! which on Rust 1.81+ unwinding out of an `extern "C"` frame is a defined abort rather
//! than undefined behaviour. Accepted: an allocation failure while building a logger is
//! not a recoverable situation anyway. Stated because the rule above is quoted from a
//! different header.
//!
//! **The other place unwinding could be ours is a callback** — Rust called *from* C++.
//! This module registers none: the logger uses `qpdf_log_dest_discard`, which needs no
//! function pointer.
//!
//! # `QPDF_ERROR_CODE` is a bitmask, not an enum
//!
//! `qpdf-c.h:134-139`. `QPDF_SUCCESS` is 0, but `QPDF_WARNINGS` and `QPDF_ERRORS` are
//! separate bits that can both be set or neither. **`result != QPDF_SUCCESS` is wrong** —
//! it treats a file that parsed perfectly but emitted a warning as a failure. The only
//! correct test is `result & QPDF_ERRORS`, which is what [`has_errors`] does and what a
//! named test in `errors.rs` pins down.

#![allow(non_camel_case_types)]

use core::ffi::{c_char, c_int, c_uint, c_ulonglong};

/// `typedef struct _qpdf_data* qpdf_data` — `qpdf-c.h:130`.
pub(super) type QpdfData = *mut core::ffi::c_void;

/// `typedef struct _qpdf_error* qpdf_error` — `qpdf-c.h:131`.
pub(super) type QpdfError = *mut core::ffi::c_void;

/// `typedef struct _qpdflogger_handle* qpdflogger_handle` — `qpdflogger-c.h:45`.
pub(super) type QpdfLoggerHandle = *mut core::ffi::c_void;

/// `typedef int QPDF_BOOL` — `qpdf-c.h:141`.
pub(super) type QpdfBool = c_int;

/// `#define QPDF_TRUE 1` — `qpdf-c.h:142`.
pub(super) const QPDF_TRUE: QpdfBool = 1;

/// `typedef int QPDF_ERROR_CODE` — `qpdf-c.h:136`.
pub(super) type QpdfErrorCode = c_int;

/// `#define QPDF_ERRORS 1 << 1` — `qpdf-c.h:139`. The **only** bit that means failure.
pub(super) const QPDF_ERRORS: QpdfErrorCode = 1 << 1;

/// Whether a `QPDF_ERROR_CODE` reports an actual error.
///
/// Exists so the bitmask test appears exactly once in the crate. See the module docs for
/// why the obvious `!= QPDF_SUCCESS` is a bug.
pub(super) const fn has_errors(code: QpdfErrorCode) -> bool {
    code & QPDF_ERRORS != 0
}

// `enum qpdf_error_code_e` (`Constants.h:85-96`) lives in `crate::codes::qpdf::code`, not
// here, so the native and web paths share one table. Its values are stable across major
// releases by upstream's explicit guarantee (`Constants.h:34-69`), which is what makes
// hard-coding them safe. Nothing in this module needs them: the mapping is the only
// consumer.

/// `enum qpdf_log_dest_e` — `qpdflogger-c.h:58-64`.
pub(super) mod log_dest {
    use core::ffi::c_int;
    /// `qpdf_log_dest_discard` — throw the output away. `qpdflogger-c.h:62`.
    pub(in crate::qpdf) const DISCARD: c_int = 3;
}

/// `enum qpdf_param_e` — `Constants.h:275-319`. Process-global options and limits.
pub(super) mod param {
    use core::ffi::c_int;
    /// `qpdf_p_limit_errors` — read-only count of limits exceeded. `Constants.h:277`.
    ///
    /// **Read-only**: `qpdf_global_set_uint32` has no case for it and returns
    /// `qpdf_r_bad_parameter`. Kept only so `limits.rs` can assert it is never set.
    #[cfg(test)]
    pub(in crate::qpdf) const LIMIT_ERRORS: c_int = 0x0001_0020;
    /// `qpdf_p_fuzz_mode` — tighten limits for fuzzing. `Constants.h:281`.
    #[cfg(feature = "fuzzing")]
    pub(in crate::qpdf) const FUZZ_MODE: c_int = 0x0001_1010;
    /// `qpdf_p_doc_max_warnings` — 0 means unlimited. `Constants.h:290`.
    pub(in crate::qpdf) const DOC_MAX_WARNINGS: c_int = 0x0001_2000;
    /// `qpdf_p_parser_max_nesting` — object nesting depth. `Constants.h:293`.
    pub(in crate::qpdf) const PARSER_MAX_NESTING: c_int = 0x0001_3000;
    /// `qpdf_p_dct_max_memory` — 0 means unlimited. `Constants.h:302`.
    pub(in crate::qpdf) const DCT_MAX_MEMORY: c_int = 0x0001_4020;
    /// `qpdf_p_flate_max_memory` — 0 means unlimited. `Constants.h:306`.
    pub(in crate::qpdf) const FLATE_MAX_MEMORY: c_int = 0x0001_4030;
    /// `qpdf_p_png_max_memory` — 0 means unlimited. `Constants.h:309`.
    pub(in crate::qpdf) const PNG_MAX_MEMORY: c_int = 0x0001_4040;
    /// `qpdf_p_run_length_max_memory` — 0 means unlimited. `Constants.h:312`.
    pub(in crate::qpdf) const RUN_LENGTH_MAX_MEMORY: c_int = 0x0001_4050;
    /// `qpdf_p_tiff_max_memory` — 0 means unlimited. `Constants.h:315`.
    pub(in crate::qpdf) const TIFF_MAX_MEMORY: c_int = 0x0001_4060;
}

unsafe extern "C" {
    /// `qpdf_data qpdf_init()` — `qpdf-c.h:163`.
    pub(super) fn qpdf_init() -> QpdfData;

    /// `void qpdf_cleanup(qpdf_data* qpdf)` — `qpdf-c.h:170`.
    pub(super) fn qpdf_cleanup(qpdf: *mut QpdfData);

    /// `void qpdf_silence_errors(qpdf_data qpdf)` — `qpdf-c.h:151`.
    ///
    /// Without this, functions that do not return an error code print to the process's
    /// **stderr**. A library has no business doing that; see `limits.rs`.
    pub(super) fn qpdf_silence_errors(qpdf: QpdfData);

    /// `void qpdf_set_suppress_warnings(qpdf_data, QPDF_BOOL)` — `qpdf-c.h:221`.
    pub(super) fn qpdf_set_suppress_warnings(qpdf: QpdfData, value: QpdfBool);

    /// `void qpdf_set_logger(qpdf_data, qpdflogger_handle)` — `qpdf-c.h:231`.
    pub(super) fn qpdf_set_logger(qpdf: QpdfData, logger: QpdfLoggerHandle);

    /// `void qpdf_set_attempt_recovery(qpdf_data, QPDF_BOOL)` — `qpdf-c.h:248`.
    pub(super) fn qpdf_set_attempt_recovery(qpdf: QpdfData, value: QpdfBool);

    /// ```c
    /// QPDF_ERROR_CODE qpdf_read_memory(qpdf_data, char const* description,
    ///     char const* buffer, unsigned long long size, char const* password);
    /// ```
    /// `qpdf-c.h:267-272`.
    ///
    /// **The buffer is not copied.** `QPDF.hh:88-90`: it "must remain valid for the
    /// lifetime of the QPDF object". Getting that wrong is a use-after-free, not a logic
    /// bug, which is why `mod.rs` keeps both in one struct rather than trusting a comment.
    pub(super) fn qpdf_read_memory(
        qpdf: QpdfData,
        description: *const c_char,
        buffer: *const c_char,
        size: c_ulonglong,
        password: *const c_char,
    ) -> QpdfErrorCode;

    /// `QPDF_BOOL qpdf_has_error(qpdf_data)` — `qpdf-c.h:178`.
    ///
    /// Required after **every** call, including ones that return a code: `qpdf-c.h:70-73`
    /// says functions that return a value report errors only this way.
    pub(super) fn qpdf_has_error(qpdf: QpdfData) -> QpdfBool;

    /// `qpdf_error qpdf_get_error(qpdf_data)` — `qpdf-c.h:186`.
    ///
    /// Consumes the error: afterwards `qpdf_has_error` is false until the next failure.
    /// The returned pointer dies on the next call to this or `qpdf_cleanup`.
    pub(super) fn qpdf_get_error(qpdf: QpdfData) -> QpdfError;

    /// `enum qpdf_error_code_e qpdf_get_error_code(qpdf_data, qpdf_error)` —
    /// `qpdf-c.h:208`.
    ///
    /// The **only** thing read from an error. Its siblings —
    /// `qpdf_get_error_full_text`, `_message_detail`, `_filename`, `_file_position` —
    /// are deliberately not declared in this module: they carry text and byte offsets
    /// derived from the file, and nothing derived from a user's file may reach a
    /// `burrow::Error`. Not declaring them is the cheapest way to guarantee that.
    pub(super) fn qpdf_get_error_code(qpdf: QpdfData, error: QpdfError) -> c_int;

    // `qpdf_is_encrypted` (`qpdf-c.h:355`) and `qpdf_is_linearized` (`qpdf-c.h:351`) are
    // deliberately NOT declared. See "Which functions are safe to call" above: neither
    // goes through `trap_errors`, and `qpdf_is_linearized` demonstrably aborts the process
    // on an ordinary 356-byte file. Restoring either needs a C++ shim, not a declaration.

    /// `int qpdf_get_num_pages(qpdf_data)` — `qpdf-c.h:969`. Returns -1 on error.
    ///
    /// Walks the page tree on first call and caches, so it is the cheapest real probe of
    /// whether a document's structure hangs together.
    pub(super) fn qpdf_get_num_pages(qpdf: QpdfData) -> c_int;

    /// `enum qpdf_result_e qpdf_global_set_uint32(enum qpdf_param_e, uint32_t)` —
    /// `qpdf-c.h:1048`, since qpdf 12.3.
    ///
    /// **Process-global**: no `qpdf_data` argument. See `limits.rs`.
    pub(super) fn qpdf_global_set_uint32(param: c_int, value: c_uint) -> c_int;

    /// `qpdflogger_handle qpdflogger_create()` — `qpdflogger-c.h:53`.
    pub(super) fn qpdflogger_create() -> QpdfLoggerHandle;

    // `qpdflogger_cleanup` is deliberately NOT declared. The one logger this crate makes
    // lives for the process and is shared by every document, so there is no point at which
    // cleaning it up would be correct -- and `qpdflogger-c.h:49-52` notes it would only
    // destroy the handle rather than the underlying object in any case. An FFI declaration
    // nothing calls is pure risk: it is one more signature that could be wrong and one
    // more thing a future reader might reach for.

    /// `void qpdflogger_set_info(qpdflogger_handle, enum qpdf_log_dest_e, qpdf_log_fn_t,
    /// void*)` — `qpdflogger-c.h:70`.
    pub(super) fn qpdflogger_set_info(
        logger: QpdfLoggerHandle,
        dest: c_int,
        func: *const core::ffi::c_void,
        udata: *mut core::ffi::c_void,
    );

    /// `void qpdflogger_set_warn(...)` — `qpdflogger-c.h:73`.
    pub(super) fn qpdflogger_set_warn(
        logger: QpdfLoggerHandle,
        dest: c_int,
        func: *const core::ffi::c_void,
        udata: *mut core::ffi::c_void,
    );

    /// `void qpdflogger_set_error(...)` — `qpdflogger-c.h:76`.
    pub(super) fn qpdflogger_set_error(
        logger: QpdfLoggerHandle,
        dest: c_int,
        func: *const core::ffi::c_void,
        udata: *mut core::ffi::c_void,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The trap the module docs describe, pinned down.
    ///
    /// `QPDF_ERROR_CODE` is a bitmask. A file that parses perfectly but emits a warning
    /// returns `QPDF_WARNINGS`, which is non-zero — so `!= QPDF_SUCCESS` would report it
    /// as a failure. Same class as PDFium's `-0` sentinel, same treatment.
    #[test]
    fn warnings_alone_are_not_an_error() {
        const QPDF_SUCCESS: QpdfErrorCode = 0;
        const QPDF_WARNINGS: QpdfErrorCode = 1 << 0;

        assert!(!has_errors(QPDF_SUCCESS));
        assert!(
            !has_errors(QPDF_WARNINGS),
            "a warnings-only result must not read as an error"
        );
        assert!(has_errors(QPDF_ERRORS));
        assert!(
            has_errors(QPDF_ERRORS | QPDF_WARNINGS),
            "errors alongside warnings are still errors"
        );

        // And the naive test that this helper exists to replace really is wrong.
        assert_ne!(QPDF_WARNINGS, QPDF_SUCCESS);
    }
}
