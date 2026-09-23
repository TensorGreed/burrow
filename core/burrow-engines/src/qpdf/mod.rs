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
// The one compression lever, and spike 0005's finding that there is only one.
mod compress;
mod extract;
mod ffi;
mod handle;
mod name;
// Removing what `qpdf_add_page`'s reachability closure dragged along (ADR 0019 §2b, #54).
mod limits;
// Removing what `qpdf_add_page`'s reachability closure dragged along (ADR 0019 §2b, #54).
mod prune;
mod redact_frame;
mod redact_steps;
mod reorder;
mod resources;
#[cfg(test)]
mod resources_tests;
mod rotate;
mod sharing;
#[cfg(test)]
mod sharing_tests;

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
        unsafe { Self::take_error_on(self.data) }
    }

    /// The same, for a caller that holds the `qpdf_data` rather than the [`Document`].
    ///
    /// `ObjectHandle::replace_stream_data` is the caller: it must drain the slot belonging to
    /// the document that issued its own handle, and taking a `&Document` from the caller let
    /// the wrong one be passed — which returns `Ok` on a write that did not happen. Security
    /// review found that; the handle's own `data` is the only answer that cannot be wrong.
    ///
    /// # Safety
    ///
    /// `data` must be a live `qpdf_data`.
    #[allow(
        dead_code,
        reason = "the caller arrives with #131; declared, exported and wrapped first so the error drain cannot be forgotten when it does. Exercised by `qpdf::write_path_tests` against the real engine"
    )]
    pub(super) unsafe fn take_error_on(data: ffi::QpdfData) -> Option<Error> {
        // SAFETY: the caller guarantees `data` is live.
        if unsafe { ffi::qpdf_has_error(data) } == 0 {
            return None;
        }
        // SAFETY: `qpdf_has_error` just reported an error, so `qpdf_get_error` returns a
        // valid `qpdf_error` for it. The pointer is used immediately, before any other
        // qpdf call can invalidate it (`qpdf-c.h:180-183`), and only its *code* is read --
        // never its text, filename or byte offset. See `errors.rs`.
        let code = unsafe {
            let error = ffi::qpdf_get_error(data);
            ffi::qpdf_get_error_code(data, error)
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

        // EVERY CEILING THAT APPLIES BEFORE THE ENGINE, in one call: the byte count,
        // the size estimate, the structural pre-scan. Shared rather than spelled out
        // here, so a path that pre-scans without estimating is not writable --- see
        // `crate::estimate::before_open`, and #26 for what the drift cost last time.
        crate::estimate::before_open(&bytes, &limits)?;

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

/// Open a document under `options`' ceilings, applying every one of them in order.
///
/// Returns the document, its page count, and the resident set measured before the open —
/// which `max_memory_bytes` needs as a "before", since it **detects rather than bounds**
/// (ADR 0007).
///
/// # Why this is shared rather than written per operation
///
/// `extract` and `rotate` had byte-identical copies of this sequence: the size check, the
/// pre-scan, the deadline, recovery off, the page check, the error drain, the measured check,
/// in that order. It is security-critical limit enforcement, and `extract`'s own comment
/// records that one of these checks had already gone missing once and was restored by code
/// review — from a copy that had drifted. Two copies agree until they do not, and the failure
/// mode is a ceiling that silently stops applying on one path. Code review flagged the
/// duplication; this is the answer.
///
/// The operations keep their own source structs. Only the ceiling sequence is shared.
///
/// # Errors
///
/// - [`Error::LimitExceeded`] — `max_input_bytes` at [`Stage::InputSize`], `max_pages` at
///   [`Stage::PageCount`], `max_duration_ms`, or a measured `max_memory_bytes` overrun.
/// - [`Error::Malformed`], [`Error::Unsupported`], [`Error::PasswordRequired`] — the input
///   could not be read.
pub(super) fn open_document(
    bytes: Box<[u8]>,
    options: &crate::OpenOptions<'_>,
) -> Result<(Document, u64, Option<u64>, Deadline)> {
    let limits = options.limits;

    // EVERY CEILING THAT APPLIES BEFORE THE ENGINE, in one call: the byte count,
    // the size estimate, the structural pre-scan. Shared rather than spelled out
    // here, so a path that pre-scans without estimating is not writable --- see
    // `crate::estimate::before_open`, and #26 for what the drift cost last time.
    crate::estimate::before_open(&bytes, &limits)?;

    // ESTABLISHES THE START POINT; it does not check anything. Elapsed is zero here, so this
    // checkpoint passes for every budget including zero. Said plainly because the line reads
    // like a time check and is not one, and because the same pattern appears in `assemble.rs`
    // and `extract.rs` where it reads the same way. Code review asked for the sentence.
    let clock = Arc::clone(&options.clock);
    let deadline = Deadline::start(clock.as_ref(), &limits);
    deadline.checkpoint(clock.as_ref())?;

    // Recovery OFF: an operation that silently reconstructs a damaged input produces a
    // document whose relationship to what the person handed us is unclear, and they will
    // never know.
    let rss_before = crate::rss::resident_bytes();

    let document = Document::open(bytes, options.password, false)?;
    let pages = document.page_count()?;
    Limits::check(Stage::PageCount, "max_pages", pages, limits.max_pages)?;

    if let Some(error) = document.take_error() {
        return Err(error);
    }

    // CHECKED HERE, not only in the operation. A document that blew past the ceiling and was
    // then given an unusable request once returned `InvalidArgument` with no `LimitExceeded`
    // ever reported -- the memory was spent and no ceiling said so, because the only check
    // lived in a function that was never reached. Found by code review on `extract`.
    crate::estimate::check_measured_memory(rss_before, crate::rss::resident_bytes(), &limits)?;

    // THE DEADLINE GOES BACK TO THE CALLER, so work an engine does after this function
    // returns but still inside its own `open` spends the SAME budget rather than starting a
    // third. `split`'s `annots_sharing` is a sweep over every source page and it ran outside
    // every ceiling until code review measured it: `Deadline::start` resets the origin and the
    // budget, so a sweep that made its own would have been handed a fresh `max_duration_ms` —
    // the exact defect ADR 0022 records being fixed three times over in `rotations`.
    Ok((document, pages, rss_before, deadline))
}

/// Every page's `/Rotate` **as written**, in page order, following inheritance.
///
/// # One sweep implementation, and this is the second time that has had to be said
///
/// Every seam that promises this vector wants the same walk, and each operation writing it out
/// is another place for the per-page checkpoint, the recorded-not-judged read and the handle
/// discipline to drift apart.
///
/// **It already drifted once, at two copies.** `OutputReader::rotations`' first version wrote
/// the loop out again and kept `effective_rotation`, so the witness refused an out-of-spec
/// page the promise sweep had already accepted; that copy is a one-line delegation now and its
/// comment records why. Code review found the copies multiplying again and pointed at that
/// same comment. This is the answer, and it is the one `open_document` already gives for the
/// ceilings: *two copies agree until they do not*.
///
/// What stays per module is the **commentary at the call site** about which deadline is being
/// passed and why — that is a statement about the caller, and it differs.
///
/// # Errors
///
/// - [`Error::Malformed`] — a `/Rotate` that is not an integer, or a page tree that cannot be
///   walked. A value that cannot be *read* is still a refusal; only the in-spec judgement is
///   relaxed, which is what "recorded, not judged" means.
/// - [`Error::LimitExceeded`] — `max_duration_ms`, checkpointed **per page**. The walk is one
///   `/Parent` climb per page, so it is sized by page count times tree depth — both
///   attacker-chosen, and measured at 147 ms against 28 ms of edit-and-write on a 10,000-page
///   document with a 60-deep tree (ADR 0022).
///
/// The deadline is the **caller's**, never a fresh one: `Deadline::start` resets the origin
/// *and* the budget, so a sweep that made its own would hand the operation another full
/// `max_duration_ms`. ADR 0022 records that being got wrong three times.
pub(super) fn rotations_of(
    document: &Document,
    pages: u64,
    options: &crate::OpenOptions<'_>,
    deadline: &Deadline,
) -> Result<Vec<i64>> {
    let capacity = usize::try_from(pages)
        .map_err(|_| Error::Internal("page count does not fit in usize".to_owned()))?;
    let mut rotations = Vec::with_capacity(capacity);

    let clock = Arc::clone(&options.clock);

    for index in 0..pages {
        deadline.checkpoint(clock.as_ref())?;
        let page = reorder::page_handle(document, index, pages)?;
        // RECORDED, NOT JUDGED. `effective_rotation` would refuse an out-of-spec value on a
        // page nobody named, which is the drift this function exists to prevent repeating.
        rotations.push(rotate::declared_rotation(document, &page)?.unwrap_or(0));
    }
    Ok(rotations)
}

#[cfg(test)]
mod compress_tests;

#[cfg(test)]
mod reorder_tests;

#[cfg(test)]
mod rotate_tests;

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "native-engines"))]
mod write_path_tests;

/// Reading a document back to check what an operation produced (ADR 0022).
///
/// Built on [`crate::PageRotator`], which already opens a document and reads each page's effective
/// rotation. Nothing new reaches qpdf through this — it is the same three calls with a
/// different purpose, and the purpose is the part worth naming at the call site.
impl crate::OutputReader for Qpdf {
    // Trait paths are written out here rather than imported at the top: this block was
    // appended and a file-wide import would move behaviour for everything above it.
    type Read = <Self as crate::PageRotator>::Source;

    fn fresh(&self) -> Self {
        // `Qpdf` is a unit struct: the engine carries no state, and every `open` makes its own
        // `qpdf_data`. So a fresh value plus a fresh open is a genuinely separate parse, with
        // nothing shared but the process allocator.
        Self::new()
    }

    fn open_output(&self, bytes: &[u8], options: &crate::OpenOptions<'_>) -> Result<Self::Read> {
        // RECOVERY OFF, as every other open here has it. Reading our own output back with
        // reconstruction enabled would let a document burrow wrote badly be repaired on the
        // way in and pass — the verifier agreeing with the writer through a repair neither
        // asked for.
        crate::PageRotator::open(self, bytes.to_vec().into_boxed_slice(), options)
    }

    fn page_count(&self, read: &Self::Read) -> Result<u64> {
        crate::PageRotator::pages(self, read)
    }

    fn rotations(
        &self,
        read: &Self::Read,
        options: &crate::OpenOptions<'_>,
        deadline: &burrow_types::Deadline,
    ) -> Result<Vec<i64>> {
        // ONE SWEEP IMPLEMENTATION, not a second copy of it. `Read` is a rotatable source, so
        // this is the same walk under a different name -- and the first version wrote the loop
        // out again, which is two places for the checkpoint, the recorded-not-judged read and
        // the handle discipline to drift apart. It drifted immediately: this copy kept
        // `effective_rotation`, so the witness refused an out-of-spec page the promise sweep
        // had already accepted.
        crate::PageRotator::rotations(self, read, options, deadline)
    }
}

/// Walk a document's first page with the real font resolver, for the differential test.
///
/// Crate-internal on purpose: ADR 0022 forbids a public path that emits a redacted document,
/// and this emits geometry rather than bytes. `redact_probe` is the one caller.
pub(crate) fn walk_first_page_for_probe(
    bytes: &[u8],
) -> Result<Vec<crate::pdfsyntax::geometry::Glyph>> {
    use std::sync::Arc;

    use burrow_types::{Clock, Limits, ManualClock};

    let options = crate::OpenOptions::new(
        Limits::default(),
        Arc::new(ManualClock::new(0)) as Arc<dyn Clock>,
    );
    let (document, pages, _, _) = open_document(bytes.to_vec().into_boxed_slice(), &options)?;
    if pages == 0 {
        return Err(Error::Malformed(
            "pdf redaction: a document with no pages".to_owned(),
        ));
    }
    // SAFETY: page 0 is below the page count just read from this document.
    let page = unsafe { handle::ObjectHandle::page(&document, 0) };
    if let Some(error) = document.take_error() {
        return Err(error);
    }
    let content = page.page_content()?;
    let resources = resources::PageResources::of(&page)?;
    crate::pdfsyntax::geometry::glyphs_in(&content, &resources)
}

/// Redact one page of a document, for the differential test and nothing else.
///
/// Crate-internal per ADR 0022: there is no caller-visible redaction until #134's verification
/// exists. `redact_probe` is the one seam and it goes when that lands.
pub(crate) fn redact_page_for_probe(
    bytes: &[u8],
    page: usize,
    redacted: std::collections::BTreeSet<usize>,
    region: crate::pdfsyntax::region::Region,
    limits: Limits,
) -> Result<(Vec<u8>, crate::redact::Report)> {
    use std::sync::Arc;

    use burrow_types::{Clock, SystemClock};

    // A REAL CLOCK. It was `ManualClock::new(0)`, which never advances -- so every
    // `deadline.checkpoint` in `redact_steps` was inert on the only route into the operation,
    // and the time bound was threaded but unmeasured. A security review found six checkpoints
    // that could not have fired.
    let clock: Arc<dyn Clock> = Arc::new(SystemClock::new());
    let options = crate::OpenOptions::new(limits, Arc::clone(&clock));
    let (document, _, _, deadline) = open_document(bytes.to_vec().into_boxed_slice(), &options)?;
    // THE PAGE BOUND IS THE CONSTRUCTOR'S, and it is checked there and only there.
    //
    // It used to be checked here as well. That is one check too many rather than one too few:
    // `QpdfRedaction::page_handle` calls the unsafe `ObjectHandle::page`, and its SAFETY
    // comment names the constructor as where the invariant is established. With the check
    // duplicated in this caller, deleting the constructor's changed nothing any test could
    // see — a mutation sweep planted exactly that and the suite stayed green, which is a
    // defence with no test standing behind an `unsafe` block.
    let steps = redact_steps::QpdfRedaction::new(document, page, region, limits, deadline, clock)?;
    crate::redact::run(steps, redacted)
}
