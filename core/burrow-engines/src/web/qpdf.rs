//! The web implementation of [`StructureEngine`], over a qpdf Emscripten module.
//!
//! The same sequence as the native path, with [`QpdfBridge`] where the `unsafe extern "C"`
//! calls are. The limits, the pre-scan, the deadline, the password preparation and the
//! code-to-error mapping are all the same Rust, so the two paths reach the same verdict on
//! the same file — which is what ROADMAP item 12 asserts.
//!
//! # Suppression matters more here, not less
//!
//! On native, qpdf's warnings go to the process's stderr. In a browser they go to the
//! devtools console, where the user can read them, and they carry object numbers and byte
//! offsets — file content by any reasonable reading (spike 0001, Finding 6). They also fire
//! for files that parse **successfully**, so "we only leak on failure" was never true.
//!
//! So the order below is load-bearing: silence first, read second. Every flag is set before
//! qpdf is given anything to complain about.

use std::sync::{Arc, OnceLock};

use burrow_types::{Deadline, Error, Limits, Result, Stage};

use super::bridge::{QpdfBridge, QpdfPtr};
use crate::{CheckOptions, StructureEngine, StructureReport};

/// The description qpdf puts in messages in place of a filename.
///
/// A fixed constant, never the real name — and NUL-terminated, because it is copied into
/// the engine heap and read as a C string. qpdf embeds this in warnings; those warnings are
/// discarded, but if one ever escaped it must carry nothing about the user's file.
const DESCRIPTION: &[u8] = b"input\0";

/// The web qpdf structure engine.
///
/// **Construct one per worker and keep it.** The process-global setup — qpdf's resource
/// limits and the discarding logger — is done once, on first use, and held here.
#[derive(Clone)]
pub struct WebQpdf {
    bridge: Arc<dyn QpdfBridge>,
    /// The shared discarding logger, created on first use.
    ///
    /// **One logger, not one per operation.** An earlier version called `logger_create()`
    /// inside `check()` and never released it: `qpdflogger_cleanup` is not declared on
    /// either path (`qpdf/ffi.rs` explains why — the native logger lives as long as the
    /// process), so each call leaked a `qpdflogger_handle` plus three `Pl_Discard`
    /// pipelines into a heap that never shrinks. Nothing bounded it and no `Limits`
    /// covered it.
    ///
    /// `qpdflogger-c.h:38-52` documents the underlying object as shared and
    /// reference-counted, which is what makes handing one handle to many documents correct
    /// — and is exactly what the native path already relies on.
    installed: Arc<OnceLock<QpdfPtr>>,
}

impl WebQpdf {
    /// An engine driving `bridge`.
    #[must_use]
    pub fn new(bridge: Arc<dyn QpdfBridge>) -> Self {
        Self {
            bridge,
            installed: Arc::new(OnceLock::new()),
        }
    }

    /// The module's current heap size, for the page's recycling decision.
    ///
    /// See [`super::WebPdfium::heap_bytes`]: absolute, not a delta, because the question the
    /// page is asking is how far this worker has grown over its whole life.
    #[must_use]
    /// The bridge this engine talks through.
    ///
    /// `pub(super)` so `assemble.rs` can drive the write path. Not public: the bridge is an
    /// audit surface (ADR 0009 §2), and widening who can call it widens what has to be
    /// argued about.
    pub(super) fn bridge(&self) -> &Arc<dyn QpdfBridge> {
        &self.bridge
    }

    /// The module's current heap size, for the page's recycling decision.
    ///
    /// See [`super::WebPdfium::heap_bytes`]: absolute, not a delta, because the question the
    /// page is asking is how far this worker has grown over its whole life.
    #[must_use]
    pub fn heap_bytes(&self) -> u64 {
        self.bridge.heap_bytes()
    }

    /// Apply the global limits and build the discarding logger. Runs at most once.
    ///
    /// The same policy the native path applies, from the same table
    /// (`crate::codes::qpdf::policy`). The web path previously applied **none of it**,
    /// which mattered more here than on native: every decompression limit qpdf offers
    /// defaults to unlimited, an allocator giving up inside C++ is an `abort()`, and an
    /// Emscripten `abort()` is quiet — it leaves the worker alive with its init flags set,
    /// so one crafted file bricks the engine for the rest of the session.
    fn install(&self) -> QpdfPtr {
        *self.installed.get_or_init(|| {
            for (param, value) in crate::codes::qpdf::policy::settings() {
                // Failures ignored, as on native: qpdf rejects a parameter it does not
                // recognise, and refusing to open any document because a hardening knob
                // moved would be the worse outcome.
                let _ = self.bridge.global_set_uint32(param, value);
            }
            let logger = self.bridge.logger_create();
            if !logger.is_null() {
                self.bridge
                    .logger_discard_all(logger, crate::codes::qpdf::policy::LOG_DEST_DISCARD);
            }
            logger
        })
    }
}

impl core::fmt::Debug for WebQpdf {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("WebQpdf").finish_non_exhaustive()
    }
}

/// A live `qpdf_data` and the engine-heap buffer it reads from.
///
/// **`qpdf_read_memory` does not copy** (`QPDF.hh:88-90`), so the buffer must outlive the
/// handle. One struct owning both, with a [`Drop`] that releases the handle *before* it
/// frees the buffer, is the same structural answer the native path uses — and here it also
/// guarantees the ordering across a JS boundary where no borrow could have expressed it.
pub(super) struct Session {
    data: QpdfPtr,
    buffer: QpdfPtr,
    /// How many bytes `buffer` holds, so [`Drop`] can **wipe** it rather than only free it.
    ///
    /// Carried for that reason alone. Without it the only honest thing `Drop` could do was
    /// `free`, which returns the user's document to the module's free list intact — see the
    /// note there.
    buffer_len: u32,
    bridge: Arc<dyn QpdfBridge>,
}

impl Drop for Session {
    fn drop(&mut self) {
        // Drain any error still sitting in the slot FIRST.
        //
        // `qpdf_cleanup` (`qpdf-c.cc:109-120`) checks whether an error was left unretrieved
        // and, if so, writes
        //
        //   WARNING: application did not handle error: <text>
        //
        // to `QPDFLogger::defaultLogger()` — **not** this document's discarding logger, and
        // not gated on `qpdf_silence_errors`. That text carries byte offsets from the
        // user's file. On the web that lands in the devtools console, which is the leak
        // this whole path exists to prevent, arriving through the one door the suppression
        // does not cover.
        //
        // `get_error` consumes the slot, so the loop terminates.
        if !self.data.is_null() {
            while self.bridge.has_error(self.data) {
                let _ = self.bridge.get_error(self.data);
            }
            self.bridge.cleanup(self.data);
        }
        // After the handle, never before: qpdf reads from this buffer for as long as the
        // handle lives.
        //
        // WIPED, NOT MERELY FREED, AND THAT WAS A CHANGE. This buffer holds the user's
        // document. `free` hands it back to the module's free list with its contents intact,
        // where it stays for the life of the WORKER -- and a worker serves many documents in
        // one session, so the previous file's bytes were still sitting in the heap while the
        // next one was processed. Passwords were wiped here from the start (`wipe_and_free`
        // on both engines) precisely because "it sits in the free list, readable by whatever
        // allocates next" is unacceptable; the document itself is the larger object and had
        // the weaker treatment.
        //
        // The cost is one `HEAPU8.fill(0, ..)` per document, which is a memset over bytes
        // that are about to be released anyway.
        if !self.buffer.is_null() {
            self.bridge.wipe_and_free(self.buffer, self.buffer_len);
        }
    }
}

impl Session {
    /// The live `qpdf_data` handle.
    ///
    /// An accessor rather than a public field so the handle cannot be stored past the
    /// session that owns it -- the buffer qpdf reads from is freed by `Drop`, and a handle
    /// outliving that is a use-after-free across the bridge.
    pub(super) fn data(&self) -> QpdfPtr {
        self.data
    }

    /// The document's page count, with qpdf's `-1` sentinel turned into the typed error it
    /// stands for.
    ///
    /// Shared by the structure check and the assembler for the same reason the native one
    /// is: `-1` is reported out of band, and a caller that treated it as a count would
    /// produce a `u64` of eighteen quintillion.
    pub(super) fn page_count(&self) -> Result<u64> {
        let pages = self.bridge.get_num_pages(self.data);
        if pages < 0 {
            return Err(self.take_error().unwrap_or_else(|| {
                Error::Malformed("qpdf: the page structure is unusable".to_owned())
            }));
        }
        u64::try_from(pages).map_err(|_| {
            Error::Internal("qpdf reported a page count that is not a count".to_owned())
        })
    }

    /// Whether qpdf is holding an error, and what it is.
    ///
    /// Called after **every** bridge call, whatever that call returned. `qpdf-c.h:70-73`:
    /// functions that do not return a code report errors only this way. Only the *code* is
    /// read — never text, filename or byte offset, none of which the bridge can even
    /// obtain, because the module's `EXPORTED_FUNCTIONS` allowlist does not include them.
    pub(super) fn take_error(&self) -> Option<Error> {
        if !self.bridge.has_error(self.data) {
            return None;
        }
        let error = self.bridge.get_error(self.data);
        let code = self.bridge.get_error_code(self.data, error);
        Some(crate::codes::qpdf::map_code(code))
    }
}

impl Session {
    /// Open a document in the engine module: validate, install the suppression, and read.
    ///
    /// **One implementation of an ordering that is load-bearing**, extracted in M1 PR B2
    /// when `merge` became the second caller. The native path factored the same sequence
    /// for the same reason, and the two are meant to be read side by side -- `qpdf/mod.rs`'s
    /// `Document::open` lists why each step sits where it does.
    ///
    /// Two differences from native, both real rather than incidental:
    ///
    /// - it takes `&[u8]` rather than `Box<[u8]>`, because the bytes are COPIED into the
    ///   engine's heap and the Rust-side buffer is not what qpdf reads. Native hands qpdf a
    ///   pointer into Rust memory, which is why that side must own it;
    /// - the password, the description and the input are separate engine-heap allocations,
    ///   each with its own failure path, and the password copy is wiped explicitly because
    ///   `Zeroizing` cannot reach across the boundary.
    ///
    /// The caller reads `heap_bytes` BEFORE calling this, so the measured-memory check
    /// covers the `copy_in` as well as the read. That is a slightly wider window than the
    /// native path measures and the wider one is the honest one: the copy is real memory
    /// the operation caused.
    pub(super) fn open(
        engine: &WebQpdf,
        bytes: &[u8],
        password: Option<&burrow_types::Password>,
        attempt_recovery: bool,
    ) -> Result<Self> {
        let input_len = u64::try_from(bytes.len())
            .map_err(|_| Error::Internal("input length does not fit in u64".to_owned()))?;
        let password = crate::password::nul_terminated(password, "qpdf")?;

        // Process-global setup: the resource limits and the discarding logger, once per
        // worker. Before any `qpdf_data` exists, as on native.
        let logger = engine.install();

        let data = engine.bridge.init();
        if data.is_null() {
            return Err(Error::Internal("qpdf could not be initialised".to_owned()));
        }

        // Take ownership immediately, so every path from here cleans up. Nothing between
        // `init` and this line can fail.
        let mut session = Session {
            data,
            buffer: QpdfPtr::NULL,
            buffer_len: 0,
            bridge: Arc::clone(&engine.bridge),
        };

        // Silence qpdf before it is given anything to complain about.
        engine.bridge.silence_errors(session.data);
        engine.bridge.set_suppress_warnings(session.data, true);
        if !logger.is_null() {
            engine.bridge.set_logger(session.data, logger);
        }
        // `QPDF.hh:233-235`: with recovery off qpdf reports the first problem it finds
        // instead of reconstructing. Off by default because a structural check that
        // silently repairs what it is checking is not a check.
        engine
            .bridge
            .set_attempt_recovery(session.data, attempt_recovery);

        // The input and the description into the engine heap. The description is freed
        // before returning; the input cannot be, and belongs to the session.
        //
        // THE LENGTH IS NARROWED BEFORE THE COPY, not at the wipe -- the same ordering the
        // password below uses, and for the same reason: narrowing afterwards leaves a failure
        // path that returns with the bytes already in the engine heap and no length with
        // which to wipe them.
        let Ok(buffer_len) = u32::try_from(bytes.len()) else {
            return Err(Error::InvalidArgument(
                "the input is too large for the engine's address space".to_owned(),
            ));
        };
        let buffer = engine.bridge.copy_in(bytes);
        if buffer.is_null() {
            return Err(Error::Io(
                "the qpdf module could not allocate for the input".to_owned(),
            ));
        }
        session.buffer = buffer;
        session.buffer_len = buffer_len;

        let description = engine.bridge.copy_in(DESCRIPTION);
        if description.is_null() {
            return Err(Error::Io("the qpdf module could not allocate".to_owned()));
        }

        // Narrowed before the copy, not at the wipe. See the note on the PDFium path: the
        // failure path otherwise returned with the password already in the engine heap,
        // unwiped, and the description buffer unfreed.
        let password_len = password.as_ref().map_or(0, |p| p.len());
        let Ok(password_len_u32) = u32::try_from(password_len) else {
            engine.bridge.free(description);
            return Err(Error::InvalidArgument(
                "password is too large for the engine's address space".to_owned(),
            ));
        };

        let password_ptr = match password.as_ref() {
            None => QpdfPtr::NULL,
            Some(p) => {
                let ptr = engine.bridge.copy_in(p);
                if ptr.is_null() {
                    engine.bridge.free(description);
                    return Err(Error::Io(
                        "the qpdf module could not allocate for the password".to_owned(),
                    ));
                }
                ptr
            }
        };

        let read = engine.bridge.read_memory(
            session.data,
            description,
            session.buffer,
            input_len,
            password_ptr,
        );

        // The password copy in the engine heap is outside Rust's allocator, so `Zeroizing`
        // cannot reach it. Wipe it explicitly, as soon as qpdf has read it.
        if !password_ptr.is_null() {
            engine.bridge.wipe_and_free(password_ptr, password_len_u32);
        }
        drop(password);
        engine.bridge.free(description);

        // Failure is established by the ERROR *bit* — never by `!= 0`, which would treat a
        // warnings-only read as a failure. See `crate::codes::qpdf::has_errors`.
        if crate::codes::qpdf::has_errors(read) {
            return Err(session.take_error().unwrap_or_else(|| {
                Error::Malformed("qpdf: the document could not be read".to_owned())
            }));
        }

        Ok(session)
    }
}

impl StructureEngine for WebQpdf {
    fn name(&self) -> &'static str {
        "qpdf-wasm"
    }

    fn check(&self, bytes: Box<[u8]>, options: &CheckOptions<'_>) -> Result<StructureReport> {
        let limits = options.limits;

        if bytes.is_empty() {
            return Err(Error::Malformed("input is empty".to_owned()));
        }

        // EVERY CEILING THAT APPLIES BEFORE THE ENGINE, in one call --- and the size estimate
        // is now among them, which REVERSES what this comment used to say. It argued that
        // `crate::estimate`'s PDFium-derived constants "would reject files qpdf handles
        // comfortably", and an earlier version of this function did call it and had it
        // removed. Reasonable, and measurably wrong: `examples/measure-open-cost.rs` finds
        // qpdf the HUNGRIER engine: on a 1 MB, 9,000-page document it peaks at 19.30 MB and
        // EXCEEDS the 18.15 MB estimate, where PDFium uses 3.39 MB. Its cost tracks PAGE COUNT
        // where the estimate tracks length, so the estimate is too lenient here --- #26's
        // successor, not an argument for leaving it out.
        //
        // The old comment's other half was right and is kept by construction: web and native
        // must not disagree. They do not, because the call went into the shared gate.
        crate::estimate::before_open(&bytes, &limits)?;

        let clock = Arc::clone(&options.clock);
        let deadline = Deadline::start(clock.as_ref(), &limits);
        deadline.checkpoint(clock.as_ref())?;

        let before = self.bridge.heap_bytes();
        let session = Session::open(self, &bytes, options.password, options.attempt_recovery)?;

        let pages = self.bridge.get_num_pages(session.data);
        if pages < 0 {
            return Err(session.take_error().unwrap_or_else(|| {
                Error::Malformed("qpdf: the page structure is unusable".to_owned())
            }));
        }
        let pages = u64::try_from(pages).map_err(|_| {
            Error::Internal("qpdf reported a page count that is not a count".to_owned())
        })?;

        // An error can be pending even when nothing above returned a failure — that is the
        // whole point of `qpdf_has_error`. Check before reporting success.
        if let Some(error) = session.take_error() {
            return Err(error);
        }

        Limits::check(Stage::PageCount, "max_pages", pages, limits.max_pages)?;

        // What the read actually cost, measured rather than estimated. The pre-scan above
        // sees only what the file *declares*; this sees what qpdf did with it.
        //
        // `heap_bytes` existed on the bridge and was called from nowhere before M1 PR 4a-i:
        // a trait method that only made the two bridges look symmetric, in a trait whose
        // docs call the method list the audit surface.
        crate::estimate::check_measured_memory(
            Some(before),
            Some(self.bridge.heap_bytes()),
            &limits,
        )?;

        deadline.checkpoint(clock.as_ref())?;

        Ok(StructureReport { pages })
    }
}
