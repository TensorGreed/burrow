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
//! correct test is `result & QPDF_ERRORS`, which is what [`has_errors`] does. Both it and
//! the named test that pins it down live in [`crate::codes::qpdf`], so the web path gets
//! the same helper rather than a second chance to get the bitmask wrong.

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

/// Opaque stand-in for `qpdf_oh` — `qpdf-c.h:585`, an `unsigned int` index into the
/// owning `qpdf_data`'s handle table.
///
/// A distinct alias rather than a bare `c_uint` so a handle cannot be passed where a count
/// or a status is expected. Handles are **per-document**: one from document A means
/// something else entirely in document B, which is why `qpdf_add_page` takes the source
/// `qpdf_data` alongside the handle rather than inferring it.
pub(super) type QpdfObjectHandle = c_uint;

/// `#define QPDF_TRUE 1` — `qpdf-c.h:142`.
pub(super) const QPDF_TRUE: QpdfBool = 1;

/// `#define QPDF_FALSE 0` — `qpdf-c.h:141`.
///
/// Spelled out rather than written as a literal `0` at the call site: `qpdf_add_page`'s
/// last parameter is `first`, and `0` there reads as an index to anyone skimming when it
/// is in fact "append rather than prepend".
pub(super) const QPDF_FALSE: QpdfBool = 0;

// `QPDF_ERROR_CODE`, the `QPDF_ERRORS` bit and the `has_errors` test live in
// `crate::codes::qpdf` so the bitmask trap is pinned down once for both paths.
pub(super) use crate::codes::qpdf::{QpdfErrorCode, has_errors};

// `enum qpdf_error_code_e` (`Constants.h:85-96`) lives in `crate::codes::qpdf::code`, not
// here, so the native and web paths share one table. Its values are stable across major
// releases by upstream's explicit guarantee (`Constants.h:34-69`), which is what makes
// hard-coding them safe. Nothing in this module needs them: the mapping is the only
// consumer.

// `enum qpdf_log_dest_e` and `enum qpdf_param_e` live in `crate::codes::qpdf::policy`,
// ungated, with the values burrow sets them to -- so the native and web paths apply one
// policy rather than two copies. The web path previously applied none at all.

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

    // ---------------------------------------------------------------- the write path
    //
    // Added in M1 PR B for `merge`. ADR 0017 records why qpdf does the merging rather than
    // PDFium: it is the only one of the two that preserves an outline, an attachment or a
    // working form field, and PDFium's failure on the last of those is silent -- it keeps
    // the widget annotation and drops the `/AcroForm`, so the merged document shows a form
    // field that no longer is one.

    /// `qpdf_oh qpdf_get_page_n(qpdf_data qpdf, size_t n)` — `qpdf-c.h:977`.
    ///
    /// Routes through `trap_errors`. Returns an object handle belonging to **this**
    /// `qpdf_data`; handles are per-document and are not interchangeable, which is why
    /// `qpdf_add_page` takes the source document alongside the handle.
    pub(super) fn qpdf_get_page_n(qpdf: QpdfData, n: usize) -> QpdfObjectHandle;

    /// ```c
    /// QPDF_ERROR_CODE qpdf_add_page(qpdf_data qpdf, qpdf_data newpage_qpdf,
    ///     qpdf_oh newpage, QPDF_BOOL first);
    /// ```
    /// `qpdf-c.h:996-997`. Routes through `trap_errors`.
    ///
    /// **The source document must outlive the write, not just this call.** qpdf resolves
    /// the foreign page's indirect objects lazily, so cleaning the source up early yields a
    /// truncated output rather than an error — a silent short document, which is precisely
    /// the failure ADR 0017 §2 refuses. `write.rs` holds every source open until
    /// `qpdf_write` has returned.
    pub(super) fn qpdf_add_page(
        qpdf: QpdfData,
        newpage_qpdf: QpdfData,
        newpage: QpdfObjectHandle,
        first: QpdfBool,
    ) -> QpdfErrorCode;

    /// Remove a page from a document — `qpdf-c.h:1004`.
    ///
    /// **Trapped** (`qpdf-c.cc`), so a C++ exception inside it becomes a status plus a
    /// recorded error rather than an unwind into Rust. Verified against
    /// `engines/qpdf-trapped-functions.txt` rather than assumed; `qpdf-c.h`'s blanket
    /// promise is not true per function (ADR 0013 §1).
    ///
    /// `split` uses it for exactly one thing: removing the blank page its destination had to
    /// start with. qpdf refuses to open a document with no pages — the corpus records
    /// `no-pages.pdf` as `Malformed` — so the empty destination `extract` builds into cannot
    /// actually be empty, and the blank is taken back out once the wanted pages are in.
    ///
    /// **`reorder` uses it with the page still in hand.** `QPDF::removePage` is
    /// `m->pages.erase(page)` and `Pages::erase` does `kids.eraseItem(pos)` — it takes the page
    /// out of `/Kids` and does **not** destroy the object, so the handle stays usable and can
    /// be put back elsewhere with [`qpdf_add_page_at`]. Read from `libqpdf/QPDF_pages.cc`
    /// rather than assumed; the whole permutation rests on it (ADR 0021).
    pub(super) fn qpdf_remove_page(qpdf: QpdfData, page: QpdfObjectHandle) -> QpdfErrorCode;

    /// ```c
    /// QPDF_ERROR_CODE qpdf_add_page_at(qpdf_data qpdf, qpdf_data newpage_qpdf,
    ///     qpdf_oh newpage, QPDF_BOOL before, qpdf_oh refpage);
    /// ```
    /// `qpdf-c.h:1000-1001`. **Trapped**, `direct` — it calls `trap_errors` in its own body,
    /// so it needs none of the helper-following argument the `qpdf_oh_*` family required.
    ///
    /// Inserts `newpage` next to `refpage`: before it when `before` is `QPDF_TRUE`, after it
    /// otherwise. `reorder` uses the `before` form exclusively, so a page always lands at a
    /// known index rather than at "one past something".
    ///
    /// # It flattens the page tree, and that is not incidental
    ///
    /// Both this and [`qpdf_remove_page`] go through `Pages::erase`/`insert`, which call
    /// `findPage` — whose own comment says it "also ensures flat /Pages" — and
    /// `flattenPagesTree` begins with `pushInheritedAttributesToPage(true, true)`.
    ///
    /// So reordering **rewrites the document's page tree**: intermediate `/Pages` nodes are
    /// gone and every page carries the attributes it used to inherit. Measured on a two-level
    /// fixture whose root held `/Rotate 90`: 16 objects in and 14 out, one `/Rotate` in and
    /// six out, and every page still displaying turned. ADR 0021 records the measurement and
    /// why the alternative — rewriting `/Kids` by hand — is worse.
    pub(super) fn qpdf_add_page_at(
        qpdf: QpdfData,
        newpage_qpdf: QpdfData,
        newpage: QpdfObjectHandle,
        before: QpdfBool,
        refpage: QpdfObjectHandle,
    ) -> QpdfErrorCode;

    /// `QPDF_ERROR_CODE qpdf_init_write_memory(qpdf_data qpdf)` — `qpdf-c.h:424`.
    ///
    /// Routes through `trap_errors` (`qpdf-c.cc:483-490`). **Its status must be checked.**
    /// It sets `qpdf->write_memory = true` unconditionally, after the trapped call that
    /// creates the writer — so ignoring a failure here leaves a caller able to dereference
    /// a null writer through the three untrapped functions below.
    pub(super) fn qpdf_init_write_memory(qpdf: QpdfData) -> QpdfErrorCode;

    /// `void qpdf_set_deterministic_ID(qpdf_data qpdf, QPDF_BOOL value)` — `qpdf-c.h:576`.
    ///
    /// **Untrapped**, and argued in `engines/qpdf-untrapped-accepted.toml`: it assigns a
    /// bool on the writer and parses nothing.
    ///
    /// **Called after `qpdf_init_write_memory`, never before.** The header is explicit that
    /// write parameters are set between `qpdf_init_write*` and `qpdf_write`; calling this
    /// first dereferences a writer that does not exist and aborts the process. Found by
    /// core dump during ADR 0017's measurement.
    ///
    /// Without it the output `/ID` is drawn from the clock and the random pool, and a
    /// golden test could only ever assert a page count.
    pub(super) fn qpdf_set_deterministic_ID(qpdf: QpdfData, value: QpdfBool);

    /// `QPDF_ERROR_CODE qpdf_write(qpdf_data qpdf)` — `qpdf-c.h:436`.
    ///
    /// Routes through `trap_errors`. Returns the status **bitmask**, like every
    /// `QPDF_ERROR_CODE` here — see `has_errors`.
    pub(super) fn qpdf_write(qpdf: QpdfData) -> QpdfErrorCode;

    /// `size_t qpdf_get_buffer_length(qpdf_data qpdf)` — `qpdf-c.h:430`.
    ///
    /// **Untrapped**, argued in `engines/qpdf-untrapped-accepted.toml`. A field read on a
    /// buffer qpdf has already produced.
    pub(super) fn qpdf_get_buffer_length(qpdf: QpdfData) -> usize;

    /// `unsigned char const* qpdf_get_buffer(qpdf_data qpdf)` — `qpdf-c.h:432`.
    ///
    /// **Untrapped**, argued in `engines/qpdf-untrapped-accepted.toml`.
    ///
    /// The pointer is owned by the `qpdf_data` and dies on the next `qpdf_init_write*` or
    /// `qpdf_cleanup` (`qpdf-c.h:426-428`). Copy out of it immediately; never store it.
    pub(super) fn qpdf_get_buffer(qpdf: QpdfData) -> *const u8;

    // ------------------------------------------------------- the object-handle API
    //
    // Added in M1 for `rotate`, which has to read an INHERITED `/Rotate`: the key may sit on
    // any ancestor in the page tree, so the page dictionary alone does not answer the
    // question. Reading a key means resolving objects, which means the parser, which means
    // every one of these must be trapped or have an argued exemption.
    //
    // Four of the six below are trapped -- not in their own bodies, which is why an earlier
    // version of `tools/check-qpdf-trapped.py` reported them as untrapped and this module
    // could not use them at all. `qpdf_oh_get_key` reaches `trap_errors` through
    // `do_with_oh` -> `trap_oh_errors`; `qpdf_oh_replace_key` through `do_with_oh_void` ->
    // `do_with_oh` -> `trap_oh_errors`. Each chain link is proven and hashed
    // (`engines/qpdf-proven-helpers.toml`) and each route is recorded per line in
    // `engines/qpdf-trapped-functions.txt`. ADR 0013's 2026-09-13 amendment is the argument.
    //
    // # Trapping answers crashes, not wrong answers
    //
    // These accessors do NOT throw on a type mismatch: `qpdf_oh_get_int_value` on a name
    // returns 0, and `qpdf_oh_get_key` on a non-dictionary returns a null object. A trapped
    // call can therefore hand back a perfectly-formed wrong answer, and a silently wrong
    // inherited `/Rotate` is worse than a refusal -- it produces a document nobody can tell
    // is wrong by looking at the operation that made it. So `rotate.rs` asserts the type
    // before reading, and maps anything unexpected to `Malformed`.

    /// `qpdf_oh qpdf_oh_get_key(qpdf_data qpdf, qpdf_oh oh, char const* key)` —
    /// `qpdf-c.h:808`.
    ///
    /// **Trapped** via `do_with_oh` -> `trap_oh_errors`. It resolves an indirect object,
    /// which runs the parser on file-controlled bytes, so it could never earn an entry in
    /// `engines/qpdf-untrapped-accepted.toml` — that file's bar is "non-parsing … never
    /// resolves an object". If a future qpdf stops routing it through the trap, the answer is
    /// to stop calling it.
    ///
    /// Returns a **new handle** that lives until released or the document is cleaned up.
    pub(super) fn qpdf_oh_get_key(
        qpdf: QpdfData,
        oh: QpdfObjectHandle,
        key: *const c_char,
    ) -> QpdfObjectHandle;

    // `qpdf_oh_has_key` (`qpdf-c.h:806`) is deliberately NOT declared, for the reason
    // `qpdflogger_cleanup` is not: nothing calls it. `qpdf_oh_get_key` on an absent key
    // returns a null object, and the type check that has to happen anyway answers "absent"
    // and "present but not an integer" in one place. A second route to the same fact is a
    // second signature that could be wrong, and a second thing a future reader might reach
    // for in preference to the one that checks the type.

    /// `enum qpdf_object_type_e qpdf_oh_get_type_code(qpdf_data qpdf, qpdf_oh oh)` —
    /// `qpdf-c.h:697`. **Trapped** via `do_with_oh` -> `trap_oh_errors`.
    ///
    /// The type is asked for **before** any value is read. See the note above: these
    /// accessors return defaults rather than raising, so the type code is the only thing
    /// standing between a malformed `/Rotate` and a confidently wrong answer.
    pub(super) fn qpdf_oh_get_type_code(qpdf: QpdfData, oh: QpdfObjectHandle) -> c_int;

    /// `long long qpdf_oh_get_int_value(qpdf_data qpdf, qpdf_oh oh)` — `qpdf-c.h:714`.
    /// **Trapped** via `do_with_oh` -> `trap_oh_errors`.
    ///
    /// Returns 0 for anything that is not an integer. Only called once
    /// [`qpdf_oh_get_type_code`] has said otherwise.
    pub(super) fn qpdf_oh_get_int_value(qpdf: QpdfData, oh: QpdfObjectHandle) -> i64;

    /// `void qpdf_oh_replace_key(qpdf_data, qpdf_oh, char const* key, qpdf_oh item)` —
    /// `qpdf-c.h:867`. **Trapped** via `do_with_oh_void` -> `do_with_oh` -> `trap_oh_errors`.
    ///
    /// Writes to the page dictionary itself, never to a shared ancestor: `/Rotate` is
    /// inheritable, so setting it on a `/Pages` node would silently rotate every page under
    /// that node. `rotate.rs` writes per page for exactly that reason.
    pub(super) fn qpdf_oh_replace_key(
        qpdf: QpdfData,
        oh: QpdfObjectHandle,
        key: *const c_char,
        item: QpdfObjectHandle,
    );

    /// `qpdf_oh qpdf_oh_new_integer(qpdf_data qpdf, long long value)` — `qpdf-c.h:823`.
    ///
    /// **Untrapped**, and argued in `engines/qpdf-untrapped-accepted.toml`. Its body is
    /// `new_object(qpdf, QPDFObjectHandle::newInteger(value))`: it constructs an integer from
    /// a number this crate chose and puts it in the handle cache. It never reads the
    /// document, never resolves an object, and cannot see a byte of the input — so it meets
    /// that file's bar exactly, and can fail only by allocation.
    pub(super) fn qpdf_oh_new_integer(qpdf: QpdfData, value: i64) -> QpdfObjectHandle;

    /// `int qpdf_oh_get_object_id(qpdf_data qpdf, qpdf_oh oh)` — `qpdf-c.h:877`.
    /// **Trapped** via `do_with_oh` -> `trap_oh_errors`.
    ///
    /// # Why `reorder` needs it, and what it cost to find out
    ///
    /// **A handle is not an identity.** `qpdf_get_page_n` issues a NEW handle on every call
    /// (`qpdf_oh oh = ++qpdf->next_oh`), so two handles to the same page compare unequal.
    /// `reorder`'s first version compared handle ids to decide whether a page was already in
    /// the right place, concluded "no" for every page that was, removed it and tried to insert
    /// it before itself — and qpdf answered `qpdf_e_pages` because the reference page was no
    /// longer in the tree. Every permutation failed, including the identity.
    ///
    /// The object number and generation are what identity means in a PDF, and they are stable
    /// across handles. Paired with [`qpdf_oh_get_generation`], because an object number alone
    /// is not unique.
    pub(super) fn qpdf_oh_get_object_id(qpdf: QpdfData, oh: QpdfObjectHandle) -> c_int;

    /// `int qpdf_oh_get_generation(qpdf_data qpdf, qpdf_oh oh)` — `qpdf-c.h:879`.
    /// **Trapped** via `do_with_oh` -> `trap_oh_errors`. See [`qpdf_oh_get_object_id`].
    pub(super) fn qpdf_oh_get_generation(qpdf: QpdfData, oh: QpdfObjectHandle) -> c_int;

    /// `void qpdf_oh_release(qpdf_data qpdf, qpdf_oh oh)` — `qpdf-c.h:630`.
    ///
    /// **Untrapped**, argued in `engines/qpdf-untrapped-accepted.toml`. Its whole body is
    /// `qpdf->oh_cache.erase(oh)` — a map erase, which parses nothing and resolves nothing.
    ///
    /// **Handles accumulate for the document's lifetime unless this is called.** The cache is
    /// a `std::map<qpdf_oh, QPDFObjectHandle>` that only ever grows; `next_oh` is a counter
    /// that never reuses an id. A per-page inheritance walk that allocated a handle per
    /// ancestor and released none would leak one entry per node for as long as the document
    /// is open — invisible to `max_memory_bytes`, which measures RSS at operation boundaries
    /// and would see this only as slow growth. `handle.rs` makes release automatic.
    pub(super) fn qpdf_oh_release(qpdf: QpdfData, oh: QpdfObjectHandle);

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
