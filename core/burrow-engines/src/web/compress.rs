//! The web implementation of [`DocumentCompressor`], over the JS bridge.
//!
//! **The same Rust as the native path, against a different seam.** `qpdf/compress.rs` calls
//! `extract::write_out` with an `ObjectStreams`; this calls `bridge.set_object_stream_mode`
//! with the number it maps to. Everything between — which ceilings apply, where the deadline
//! is checkpointed, that the sweep records rather than judges — is duplicated deliberately
//! rather than shared, for the reason `web/mod.rs` gives: the two are separate implementations
//! of one trait and the differential harness is what holds them together. Code shared between
//! them could not diverge; code that *cannot* diverge cannot be caught diverging either.
//!
//! # One lever here too, and the four defaults stay unwritten
//!
//! [Spike 0005](../../../../docs/spikes/0005-what-qpdf-alone-compresses.md): four of the five
//! compression levers qpdf's C API exposes are already the writer's own defaults, so `merge`,
//! `rotate`, `reorder` and `split` have been emitting them on this path as well. Only
//! `qpdf_o_generate` is set, and the other four are deliberately **not** re-stated as
//! configuration — a default spelled out as a setting invites someone to tune what every other
//! operation already emits.
//!
//! # Handles, and why there are none to release here
//!
//! `web/rotate.rs`'s header is about a discipline this module does not need: compression takes
//! no page selection and touches no page dictionary, so it obtains no object handle at all.
//! The one place a handle appears is the rotation sweep, and that is `rotate`'s helper, called
//! here exactly as `reorder` calls it — releasing on every path including the error paths.

use std::sync::Arc;

use burrow_types::{Deadline, Error, Limits, Result, Stage};

use super::qpdf::{Session, WebQpdf};
use crate::{DocumentCompressor, OpenOptions};

/// `qpdf_o_generate` — pack non-stream objects into object streams, with an xref stream.
///
/// `Constants.h:134-138`. Spelled here rather than imported from the native `ffi` module,
/// which is gated on the engines being linked and does not exist on this path. The native
/// constant carries the same value and `the_web_and_native_object_stream_modes_agree` in
/// `web/tests.rs` is what keeps them equal — a divergence here would be `compress` silently
/// not compressing on the web, with every page count and rotation still correct.
pub(super) const QPDF_O_GENERATE: u32 = 2;

/// A document held open for compression, on the web.
pub struct WebCompressible {
    session: Session,
    pages: u64,
    /// The engine heap before the document was opened. `max_memory_bytes` **detects rather
    /// than bounds** (ADR 0007), and detecting it needs a before.
    heap_before: u64,
    /// The ceilings the document was opened under, so a later caller cannot loosen them.
    limits: Limits,
}

impl DocumentCompressor for WebQpdf {
    type Source = WebCompressible;

    fn name(&self) -> &'static str {
        "qpdf-wasm"
    }

    fn open(&self, bytes: Box<[u8]>, options: &OpenOptions<'_>) -> Result<Self::Source> {
        let limits = options.limits;

        // EVERY CEILING THAT APPLIES BEFORE THE ENGINE, in one call: the byte count, the size
        // estimate, the structural pre-scan. Shared rather than spelled out, so a path that
        // pre-scans without estimating is not writable -- see #26 for what the drift cost.
        crate::estimate::before_open(&bytes, &limits)?;

        let clock = Arc::clone(&options.clock);
        let deadline = Deadline::start(clock.as_ref(), &limits);
        deadline.checkpoint(clock.as_ref())?;

        // READ BEFORE THE OPEN, so the measured check covers the `copy_in` into the engine
        // heap as well as the read itself.
        let heap_before = self.bridge().heap_bytes();

        // Recovery OFF, as every other operation has it.
        let session = Session::open(self, &bytes, options.password, false)?;
        let pages = session.page_count()?;
        Limits::check(Stage::PageCount, "max_pages", pages, limits.max_pages)?;

        if let Some(error) = session.take_error() {
            return Err(error);
        }

        crate::estimate::check_measured_memory(
            Some(heap_before),
            Some(self.bridge().heap_bytes()),
            &limits,
        )?;

        Ok(WebCompressible {
            session,
            pages,
            heap_before,
            limits,
        })
    }

    fn pages(&self, source: &Self::Source) -> Result<u64> {
        Ok(source.pages)
    }

    fn rotations(
        &self,
        source: &Self::Source,
        options: &OpenOptions<'_>,
        deadline: &Deadline,
    ) -> Result<Vec<i64>> {
        // THE SAME WALK `rotate` AND `reorder` DO on this path, through their helpers: the
        // depth ceiling, the type assertion and the handle release live there, and a second
        // copy of a walk over hostile input is a second place to get them wrong.
        //
        // The keys are allocated for the duration of the sweep and freed by `Keys`' `Drop`.
        // `rotate` holds them on its source because it reads them per page across several
        // calls; compression sweeps once, so they are local.
        let keys = super::rotate::Keys::copy_in(self)?;

        let capacity = usize::try_from(source.pages)
            .map_err(|_| Error::Internal("page count does not fit in usize".to_owned()))?;
        let mut rotations = Vec::with_capacity(capacity);

        // PER PAGE, as the native sweep checkpoints and for the same measured reason -- and
        // against the CALLER'S deadline, not a new one. `Deadline::start` resets the origin
        // AND the budget, so a sweep that made its own would hand the operation another full
        // `max_duration_ms`; ADR 0022 records that being got wrong three times.
        let clock = Arc::clone(&options.clock);

        for index in 0..source.pages {
            // BEFORE THE HANDLE IS ISSUED, so a refusal cannot leave one behind.
            deadline.checkpoint(clock.as_ref())?;
            let page = super::rotate::page_handle(self, &source.session, index, source.pages)?;
            // EVERY PATH RELEASES: `?` inside the loop would return past the release.
            // RECORDED, NOT JUDGED -- compression names no page to turn, so normalising here
            // would fail a whole operation over a page it was never going to touch.
            let outcome = super::rotate::declared_rotation(self, &source.session, &keys, page);
            self.bridge().oh_release(source.session.data(), page);
            rotations.push(outcome?.unwrap_or(0));
        }
        Ok(rotations)
    }

    fn compress(&self, source: &Self::Source, options: &OpenOptions<'_>) -> Result<Vec<u8>> {
        // `options` carries the CLOCK; the CEILINGS come from `source.limits` -- the ceiling
        // that counts is the one the document was opened under, and a caller passing laxer
        // options to a later call must not be able to raise it after the fact.
        let clock = Arc::clone(&options.clock);
        // ESTABLISHES THE START POINT; it does not check anything. `Deadline::start` reads the
        // clock for its origin, so a checkpoint taken immediately after compares an elapsed
        // time of zero and cannot fire -- the same thing `open_document` says about its own.
        //
        // WHICH MEANS THE WRITE IS UNBOUNDED, and on this path that is survivable where it is
        // not natively: the worker watchdog (ADR 0015) terminates a worker that has gone
        // quiet, which is the only thing that actually bounds a single engine call. qpdf
        // offers no timeout, cancellation or abort hook (ADR 0007).
        let deadline = Deadline::start(clock.as_ref(), &source.limits);

        // THERE IS NO `max_pages` CHECK HERE, and that is deliberate rather than an omission.
        // `open` already refused `source.pages > max_pages` under these same `Limits`, and
        // compression takes no page selection -- there is no caller-chosen count that could
        // exceed the document's own. `web/rotate.rs` keeps its equivalent because a rotation's
        // page list IS caller-chosen; `web/reorder.rs` drops its for the reason this does.
        //
        // A ceiling that cannot fire reads as coverage, which `CLAUDE.md` calls worse than no
        // check at all.

        let init = self.bridge().init_write_memory(source.session.data());
        if crate::codes::qpdf::has_errors(init) {
            return Err(source.session.take_error().unwrap_or_else(|| {
                Error::Io("qpdf could not prepare an in-memory write".to_owned())
            }));
        }

        // AFTER `init_write_memory`, never before: the writer does not exist until that call
        // succeeds, and both of these dereference it.
        self.bridge()
            .set_deterministic_id(source.session.data(), true);

        // THE WHOLE OPERATION. One call, one lever -- and the only line that distinguishes
        // this module from a plain write.
        self.bridge()
            .set_object_stream_mode(source.session.data(), QPDF_O_GENERATE);

        let wrote = self.bridge().write(source.session.data());
        if crate::codes::qpdf::has_errors(wrote) {
            return Err(source.session.take_error().unwrap_or_else(|| {
                Error::Malformed("qpdf: the output could not be written".to_owned())
            }));
        }
        if let Some(error) = source.session.take_error() {
            return Err(error);
        }

        let len = self.bridge().get_buffer_length(source.session.data());
        let ptr = self.bridge().get_buffer(source.session.data());
        // BOTH are checked, not one. qpdf returns a null buffer when the writer has none and
        // reports the length from a separate accessor, so a caller trusting only the length
        // would read from null and one trusting only the pointer would copy zero bytes and
        // call it a document.
        if ptr.is_null() || len == 0 {
            return Err(Error::Io("qpdf produced no output".to_owned()));
        }
        let output = self.bridge().copy_out(ptr, len);

        deadline.checkpoint(clock.as_ref())?;

        // `max_memory_bytes` DETECTS rather than bounds (ADR 0007). Checked after the write,
        // where the retained output buffer is the largest thing this operation still holds.
        crate::estimate::check_measured_memory(
            Some(source.heap_before),
            Some(self.bridge().heap_bytes()),
            &source.limits,
        )?;

        Ok(output)
    }
}
