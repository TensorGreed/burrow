//! The qpdf implementation of [`StructureEngine`].
//!
//! qpdf answers different questions from PDFium: document structure, encryption state,
//! object streams and linearisation ([ADR 0004](../../../../docs/adr/0004-native-engines.md)).
//! It is not a [`DocumentEngine`](crate::DocumentEngine) and does not try to be.
//!
//! # Threading: the caller's thread, not the engine thread
//!
//! PDFium is serialised onto one dedicated thread because it is not thread-safe anywhere
//! ([ADR 0011](../../../../docs/adr/0011-pdfium-engine-thread.md)). qpdf is different:
//! `qpdf-c.h:43-45` says distinct `qpdf_data` objects may be used from distinct threads,
//! and only sharing *one* between threads is forbidden. So `qpdf_data` is `Send` and not
//! `Sync`, and every one created here is born, used and destroyed inside a single function
//! call — it never escapes the stack frame, so the type system enforces the rule for free.
//!
//! Putting this on PDFium's engine thread would be actively worse. ADR 0011 records that
//! one slow document already delays every other caller by up to 716 ms; adding qpdf's work
//! to that queue deepens exactly that problem, and structural checks are meant to be the
//! cheap part that runs in parallel.
//!
//! What *is* process-global — the resource limits and the discarding logger — is set once
//! behind a `OnceLock` before any `qpdf_data` exists. See this module's `limits`
//! submodule, which is private.
//!
//! # No C++ shim
//!
//! Every function in `qpdf-c.h` catches C++ exceptions internally and has since qpdf 10.5;
//! `qpdf-c.h:113-115` calls a leaked exception a bug to be reported. The declarations
//! live in this module's private `ffi` submodule, and ADR 0013 §1 has the full argument.

mod assemble;
mod ffi;
mod limits;

#[cfg(feature = "fuzzing")]
pub use limits::enable_fuzz_mode;

use core::ffi::{c_char, c_ulonglong};

use std::sync::Arc;

use burrow_types::{Deadline, Error, Limits, Result, Stage};

use crate::{CheckOptions, StructureEngine, StructureReport};

/// The qpdf-backed structure engine.
///
/// Stateless. The process-global setup happens on first use.
#[derive(Debug, Clone, Copy, Default)]
pub struct Qpdf;

impl Qpdf {
    /// A handle to the qpdf engine.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// The description qpdf puts in messages in place of a filename.
///
/// A fixed constant, never the real name. qpdf embeds this in its warnings, and those
/// warnings are discarded — but if one ever escaped, it must not carry anything about the
/// user's file. The trailing NUL makes it usable as a C string with no allocation.
const DESCRIPTION: &[u8] = b"input\0";

/// An open qpdf document, and the buffer it reads from.
///
/// **`qpdf_read_memory` does not copy** (`QPDF.hh:88-90`): the buffer must stay valid and
/// at a stable address for the whole lifetime of the handle. That is the same hazard
/// PDFium poses, and it gets the same structural answer rather than a comment — one struct
/// owning both, with a `Drop` that cleans up the handle *before* the buffer field is
/// dropped.
///
/// Holds a raw pointer, so it is `!Send`. It never leaves the function that made it.
pub(super) struct Document {
    pub(super) data: ffi::QpdfData,
    /// Read by qpdf for as long as `data` lives. Never touched from Rust after the read.
    _bytes: Box<[u8]>,
}

impl Drop for Document {
    fn drop(&mut self) {
        // `Drop::drop` runs before the struct's fields are dropped, so the handle is
        // always released before the buffer it reads from is freed. Declaration order does
        // not have to be trusted, and neither does any call site.
        if !self.data.is_null() {
            // Drain any error still sitting in the slot FIRST.
            //
            // `qpdf_cleanup` (`qpdf-c.cc:109-120`) checks whether an error was left
            // unretrieved and, if so, writes
            //
            //   WARNING: application did not handle error: <text>
            //
            // to `QPDFLogger::defaultLogger()` -- **not** the discarding logger installed
            // on this document, and not gated on `qpdf_silence_errors`. That text carries
            // byte offsets from the user's file, so it is exactly the leak this PR exists
            // to close, arriving through the one door the suppression does not cover.
            //
            // Every current path happens to retrieve the error before returning, so this is
            // unreachable today. That is a property of the control flow rather than of the
            // type, and control flow changes -- so draining here makes it structural, the
            // same argument that keeps the buffer and the handle in one struct.
            //
            // SAFETY: `self.data` is a live handle owned by this struct. `qpdf_get_error`
            // consumes the slot, so the loop terminates; the returned pointer is
            // deliberately not read.
            unsafe {
                while ffi::qpdf_has_error(self.data) != 0 {
                    let _ = ffi::qpdf_get_error(self.data);
                }
            }
            // SAFETY: `data` came from `qpdf_init` and has not been cleaned up -- this
            // type owns it and this is the only `qpdf_cleanup`. The call takes a pointer
            // to the handle and nulls it. The buffer is still alive: it is a field of
            // `self`, dropped after this returns.
            unsafe { ffi::qpdf_cleanup(&raw mut self.data) }
        }
    }
}

impl Document {
    /// Whether qpdf is holding an error, and what it is.
    ///
    /// Called after **every** FFI call, whatever that call returned. `qpdf-c.h:70-73`:
    /// functions that do not return a code report errors only this way, and even those
    /// that do are documented as safe to check like this. Trusting return values alone
    /// would miss half the failure modes.
    pub(super) fn take_error(&self) -> Option<Error> {
        // SAFETY: `self.data` is a live handle owned by this struct.
        if unsafe { ffi::qpdf_has_error(self.data) } == 0 {
            return None;
        }
        // SAFETY: `qpdf_has_error` just reported an error, so `qpdf_get_error` returns a
        // valid `qpdf_error` for it. The pointer is used immediately, before any other
        // qpdf call can invalidate it (`qpdf-c.h:180-183`), and only its *code* is read --
        // never its text, filename or byte offset. See `errors.rs`.
        let code = unsafe {
            let error = ffi::qpdf_get_error(self.data);
            ffi::qpdf_get_error_code(self.data, error)
        };
        Some(crate::codes::qpdf::map_code(code))
    }
}

impl Document {
    /// Open a document: validate the input, install the suppression, and read.
    ///
    /// **One implementation of an ordering that is load-bearing.** Extracted in M1 PR B,
    /// when `merge` became the second caller — the alternative was two copies of a
    /// sequence in which every step is placed for a reason:
    ///
    /// 1. **Empty first.** A zero-length buffer has no valid pointer to hand qpdf.
    /// 2. **`Stage::InputSize` before anything allocates**, per non-negotiable #3.
    /// 3. **The structural pre-scan before the engine**, because it is the only defence
    ///    that runs before a C++ parser sees the bytes (ADR 0013).
    /// 4. **The password is copied before the handle exists**, so the copy is wiped by its
    ///    own `Drop` on every path including an early return.
    /// 5. **Suppression installed before the read**, because `qpdf_read_memory` is one of
    ///    the calls that warns — and its warnings carry byte offsets from the file.
    /// 6. **Failure established by the ERROR bit, never `!= 0`.**
    ///
    /// Step 6 is the one that bites. `QPDF_WARNINGS` is bit 0 and `QPDF_ERRORS` is bit 1;
    /// every damaged file in the conformance corpus reads with warnings, so `!= 0` reports
    /// qpdf refusing files it does not refuse. ADR 0017's measurement harness reproduced
    /// that bug even though this module already had it right, which is most of the reason
    /// the sequence now lives in one place.
    ///
    /// The caller supplies the deadline and the aggregate ceilings: those differ between a
    /// single-document check and a multi-input merge, and nothing here should pretend
    /// otherwise.
    fn open(
        bytes: Box<[u8]>,
        password: Option<&burrow_types::Password>,
        attempt_recovery: bool,
    ) -> Result<Self> {
        if bytes.is_empty() {
            return Err(Error::Malformed("input is empty".to_owned()));
        }

        let password = crate::password::nul_terminated(password, "qpdf")?;
        let size = c_ulonglong::try_from(bytes.len())
            .map_err(|_| Error::Internal("input length does not fit in size_t".to_owned()))?;

        // Process-global setup: resource limits and the discarding logger, once.
        let logger = limits::install();

        // SAFETY: `qpdf_init` returns an owned handle or null; null is checked below.
        let data = unsafe { ffi::qpdf_init() };
        if data.is_null() {
            return Err(Error::Internal("qpdf could not be initialised".to_owned()));
        }

        // Take ownership immediately, so every path from here frees the handle -- including
        // an unwind. Nothing between `qpdf_init` and this line can fail.
        let document = Self {
            data,
            _bytes: bytes,
        };

        // SAFETY: `document.data` is live. `logger` is the process-wide discarding logger
        // from `limits::install`, which outlives every document. All four calls only set
        // flags or store a pointer.
        unsafe {
            ffi::qpdf_silence_errors(document.data);
            ffi::qpdf_set_suppress_warnings(document.data, ffi::QPDF_TRUE);
            if !logger.is_null() {
                ffi::qpdf_set_logger(document.data, logger);
            }
            ffi::qpdf_set_attempt_recovery(document.data, i32::from(attempt_recovery));
        }

        let password_ptr = password
            .as_ref()
            .map_or(core::ptr::null(), |p| p.as_ptr().cast::<c_char>());

        // SAFETY: the buffer pointer addresses `document._bytes`, which this struct owns
        // and which outlives `document.data` by construction. `size` is its length.
        // `DESCRIPTION` is a NUL-terminated literal. `password_ptr` is null or a
        // NUL-terminated buffer alive for the whole call. qpdf does not copy the buffer
        // (`QPDF.hh:88-90`), which is exactly why the two live in one struct.
        let read = unsafe {
            ffi::qpdf_read_memory(
                document.data,
                DESCRIPTION.as_ptr().cast::<c_char>(),
                document._bytes.as_ptr().cast::<c_char>(),
                size,
                password_ptr,
            )
        };
        drop(password);

        // Failure is established by the return value's ERROR bit -- never by `!= 0`, which
        // would treat a warnings-only read as a failure. See `ffi::has_errors`.
        if ffi::has_errors(read) {
            return Err(document.take_error().unwrap_or_else(|| {
                Error::Malformed("qpdf: the document could not be read".to_owned())
            }));
        }

        Ok(document)
    }

    /// The document's page count, with qpdf's `-1` sentinel turned into the typed error it
    /// stands for.
    ///
    /// Shared for the same reason `open` is: `-1` is reported out of band, and a caller
    /// that treated it as a count would produce a `u64` of 18 quintillion.
    pub(super) fn page_count(&self) -> Result<u64> {
        // SAFETY: `self.data` is a live handle whose document read successfully.
        // `qpdf_get_num_pages` routes through qpdf's `trap_errors` (`qpdf-c.cc:1801`), so
        // a C++ exception inside it becomes -1 plus a recorded error rather than an unwind
        // into Rust. That is verified per function -- see `ffi`'s module docs.
        let pages = unsafe { ffi::qpdf_get_num_pages(self.data) };

        if pages < 0 {
            return Err(self.take_error().unwrap_or_else(|| {
                Error::Malformed("qpdf: the page structure is unusable".to_owned())
            }));
        }
        u64::try_from(pages).map_err(|_| {
            Error::Internal("qpdf reported a page count that is not a count".to_owned())
        })
    }
}

impl StructureEngine for Qpdf {
    fn name(&self) -> &'static str {
        "qpdf"
    }

    fn check(&self, bytes: Box<[u8]>, options: &CheckOptions<'_>) -> Result<StructureReport> {
        let limits = options.limits;

        if bytes.is_empty() {
            return Err(Error::Malformed("input is empty".to_owned()));
        }

        let input_len = u64::try_from(bytes.len())
            .map_err(|_| Error::Internal("input length does not fit in u64".to_owned()))?;
        Limits::check(
            Stage::InputSize,
            "max_input_bytes",
            input_len,
            limits.max_input_bytes,
        )?;

        // The structural pre-scan runs here too, for the same reason it runs before
        // PDFium: qpdf is a C++ parser and this is untrusted input. qpdf's own global
        // limits are a second layer under it, not a replacement for it.
        crate::prescan::check(&bytes, &limits)?;

        // The budget for everything that follows. See `StructureEngine::check` for what
        // this can and cannot catch.
        let clock = Arc::clone(&options.clock);
        let deadline = Deadline::start(clock.as_ref(), &limits);
        deadline.checkpoint(clock.as_ref())?;

        let before = crate::rss::resident_bytes();
        let document = Document::open(bytes, options.password, options.attempt_recovery)?;

        let pages = document.page_count()?;

        // An error can be pending even when nothing above returned a failure -- that is the
        // whole point of `qpdf_has_error`. Check before reporting success.
        if let Some(error) = document.take_error() {
            return Err(error);
        }

        Limits::check(Stage::PageCount, "max_pages", pages, limits.max_pages)?;

        // What the read actually cost. The pre-scan sees only what the file *declares*;
        // this sees what qpdf did with it -- a decompression bomb costs memory here exactly
        // as a declared-size bomb costs PDFium memory there.
        //
        // Added in M1 PR 4a-i, when the web qpdf path grew one and the two would otherwise
        // have disagreed. It is the same function and the same tolerance the PDFium path
        // uses; only the counter differs per platform.
        crate::estimate::check_measured_memory(before, crate::rss::resident_bytes(), &limits)?;

        deadline.checkpoint(clock.as_ref())?;

        Ok(StructureReport { pages })
    }
}

#[cfg(test)]
mod tests;
