//! The web implementation of [`PageExtractor`], and the second caller of the shared prune.
//!
//! **A sibling of `qpdf/extract.rs`, and deliberately so**: the two build an output the same way —
//! read our own blank document, copy the wanted pages in, take the blank page back out, prune, and
//! write. What they do *not* duplicate is the pruning policy, which lives in [`crate::prune`] and
//! is written once. `crate::prune`'s header has the argument for why this one capability is shared
//! where `rotate`, `reorder` and `merge` are not.
//!
//! # The destination is our own bytes here too
//!
//! [`BLANK_DOCUMENT`] goes in through the trapped `qpdf_read_memory`, for the reason ADR 0019
//! records: `qpdf_empty_pdf` runs the full parser with no `trap_errors` wrapper, and a `QPDFExc`
//! out of it crosses an `extern "C"` frame. On the web that takes the worker down rather than the
//! process, which is not an improvement.

use std::sync::Arc;

use burrow_types::{Deadline, Error, Limits, Result, Stage};

use super::handle::WebHandle;
use super::prune::WebGraph;
use super::qpdf::{Session, WebQpdf};
use crate::blank::BLANK_DOCUMENT;
use crate::codes::qpdf::has_errors;
use crate::{OpenOptions, PageExtractor};

/// A source document held open for the life of a split, across the bridge.
pub struct WebExtractable {
    session: Session,
    pages: u64,
    /// The engine heap before the source was opened, for the measured check.
    heap_before: u64,
    /// The ceilings the source was opened under, so `extract` cannot be handed laxer ones.
    limits: Limits,
    /// For each source page, the other source pages its `/Annots` array is shared with.
    ///
    /// Read here and not in `extract`, because by then it is gone: the destination holds a *copy*
    /// of the array, and a copy shared with an excluded page is indistinguishable from a copy
    /// shared with nobody.
    annots_shared: Vec<Vec<u64>>,
    /// The deadline `open` started, so the pruning pass spends the same budget.
    deadline: Deadline,
}

impl PageExtractor for WebQpdf {
    type Source = WebExtractable;

    fn name(&self) -> &'static str {
        "qpdf-wasm"
    }

    fn open(&self, bytes: Box<[u8]>, options: &OpenOptions<'_>) -> Result<Self::Source> {
        let limits = options.limits;

        // EVERY CEILING THAT APPLIES BEFORE THE ENGINE, in one call: the byte count,
        // the size estimate, the structural pre-scan. Shared rather than spelled out
        // here, so a path that pre-scans without estimating is not writable --- see
        // `crate::estimate::before_open`, and #26 for what the drift cost last time.
        crate::estimate::before_open(&bytes, &limits)?;

        let clock = Arc::clone(&options.clock);
        let deadline = Deadline::start(clock.as_ref(), &limits);
        deadline.checkpoint(clock.as_ref())?;

        let heap_before = self.bridge().heap_bytes();

        // Recovery OFF, as every other operation has it.
        let session = Session::open(self, &bytes, options.password, false)?;
        let pages = session.page_count()?;
        Limits::check(Stage::PageCount, "max_pages", pages, limits.max_pages)?;
        if let Some(error) = session.take_error() {
            return Err(error);
        }

        // THE SHARING SWEEP, inside the open's own deadline rather than a fresh one. It is
        // O(source pages) bridge crossings, so on a `max_pages`-sized document it is not "one
        // engine call" of overshoot -- and `Deadline::start` resets the budget as well as the
        // origin, so making one here would hand the sweep a whole second `max_duration_ms`.
        let annots_shared = {
            let graph = WebGraph::over(self, &session);
            crate::prune::annots_sharing(&graph, pages, options, &deadline)?
        };

        crate::estimate::check_measured_memory(
            Some(heap_before),
            Some(self.bridge().heap_bytes()),
            &limits,
        )?;

        Ok(WebExtractable {
            session,
            pages,
            heap_before,
            limits,
            annots_shared,
            deadline,
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
        // THE SAME WALK `web/rotate.rs` DOES, reusing its helpers rather than repeating the
        // `/Parent` climb -- the depth ceiling and the type assertion live there, and a second
        // copy of a walk over hostile input is a second place to get them wrong.
        let capacity = usize::try_from(source.pages)
            .map_err(|_| Error::Internal("page count does not fit in usize".to_owned()))?;
        let mut rotations = Vec::with_capacity(capacity);

        // PER PAGE, and the caller's deadline rather than a new one -- see the trait's docs.
        let clock = Arc::clone(&options.clock);
        // The two keys the `/Parent` climb needs, copied into the engine heap once rather than
        // per page, and freed when this returns.
        let keys = super::rotate::Keys::copy_in(self)?;
        for index in 0..source.pages {
            // BEFORE THE HANDLE IS ISSUED, so a refusal cannot leave one behind.
            deadline.checkpoint(clock.as_ref())?;
            let page = super::rotate::page_handle(self, &source.session, index, source.pages)?;
            // EVERY PATH RELEASES, by the handle's `Drop` rather than a line a `?` could skip.
            // RECORDED, NOT JUDGED -- see the trait's docs.
            let outcome = super::rotate::declared_rotation(&page, &keys);
            rotations.push(outcome?.unwrap_or(0));
        }
        Ok(rotations)
    }

    fn extract(
        &self,
        source: &Self::Source,
        first: u64,
        count: u64,
        options: &OpenOptions<'_>,
    ) -> Result<Vec<u8>> {
        // `options` carries the clock and the password; the CEILINGS come from `source.limits`,
        // for the reason the native twin records: a caller passing laxer options to a later
        // `extract` must not be able to raise a ceiling after the fact.
        let _ = options;

        let end = first
            .checked_add(count)
            .ok_or_else(|| Error::InvalidArgument("page run overflows".to_owned()))?;
        if count == 0 || end > source.pages {
            return Err(Error::InvalidArgument(
                "page run is not inside the document".to_owned(),
            ));
        }
        Limits::check(
            Stage::PageCount,
            "max_pages",
            count,
            source.limits.max_pages,
        )?;

        // The destination: our own blank document, through the trapped read.
        let dest = Session::open(self, BLANK_DOCUMENT, None, false)?;
        if let Some(error) = dest.take_error() {
            return Err(error);
        }

        for n in first..end {
            let index = u32::try_from(n)
                .map_err(|_| Error::Internal("page index does not fit in u32".to_owned()))?;
            // OWNED BEFORE THE DRAIN. qpdf allocates a handle on the error path too, and this
            // one would otherwise live in the SOURCE's cache for the whole split -- found by
            // security review, and now held by the type rather than by two lines here, along
            // with the "only what was issued" rule that came with it.
            let page = WebHandle::owned(
                self,
                &source.session,
                self.bridge().get_page_n(source.session.data(), index),
            );
            page.drained()?;
            // `false` is `first`: append rather than prepend, so pages arrive in source order.
            let added =
                self.bridge()
                    .add_page(dest.data(), source.session.data(), page.raw(), false);
            if has_errors(added) {
                return Err(dest
                    .take_error()
                    .or_else(|| source.session.take_error())
                    .unwrap_or_else(|| {
                        Error::Malformed("qpdf: a page could not be copied".to_owned())
                    }));
            }
            if let Some(error) = dest.take_error() {
                return Err(error);
            }
        }

        // THE BLANK PAGE COMES BACK OUT, now that the document has real pages in it. Last rather
        // than first: qpdf will not hold a document with no pages, which is the same fact that
        // made the constant one page instead of none.
        // OWNED BEFORE THE DRAIN, and released by `Drop` whichever way this returns -- including
        // the "only what was issued" rule, which the type holds now rather than each call site.
        //
        // IT LIVES TO THE END OF THIS FUNCTION, where it used to be released right after
        // `remove_page`. Benign and worth saying rather than leaving to be re-derived: nothing
        // reads `blank.raw()` again, and qpdf-c's `oh_cache` is a separate map from `QPDF`'s
        // own object cache, so holding the id does not affect what `QPDFWriter` emits. It is
        // one extra live handle on the split path, and `web/tests.rs`'s `live == 0` assertions
        // still hold. Measured by security review.
        let blank = WebHandle::owned(self, &dest, self.bridge().get_page_n(dest.data(), 0));
        blank.drained()?;
        let removed = self.bridge().remove_page(dest.data(), blank.raw());
        if has_errors(removed) {
            return Err(dest.take_error().unwrap_or_else(|| {
                Error::Internal("qpdf: the blank destination page could not be removed".to_owned())
            }));
        }
        if let Some(error) = dest.take_error() {
            return Err(error);
        }

        // WHAT THE COPY DRAGGED ALONG, taken back out -- through the SAME policy the native path
        // runs. See `crate::prune`.
        let kept = usize::try_from(count)
            .map_err(|_| Error::Internal("page count does not fit in usize".to_owned()))?;
        let mut ambiguous = Vec::with_capacity(kept);
        for at in 0..kept {
            let source_index = usize::try_from(first)
                .ok()
                .and_then(|first| first.checked_add(at))
                .ok_or_else(|| Error::Internal("source page index does not fit".to_owned()))?;
            let sharers = source.annots_shared.get(source_index).ok_or_else(|| {
                Error::Internal("a copied page has no recorded source page".to_owned())
            })?;
            ambiguous.push(sharers.iter().any(|page| *page < first || *page >= end));
        }
        {
            let graph = WebGraph::over(self, &dest);
            crate::prune::prune_output(&graph, &ambiguous, options, &source.deadline)?;
        }

        let init = self.bridge().init_write_memory(dest.data());
        if has_errors(init) {
            return Err(dest.take_error().unwrap_or_else(|| {
                Error::Io("qpdf could not prepare an in-memory write".to_owned())
            }));
        }
        // AFTER `init_write_memory`, never before: the writer does not exist until that call
        // succeeds, and this dereferences it. Found by core dump on the native path.
        self.bridge().set_deterministic_id(dest.data(), true);
        let wrote = self.bridge().write(dest.data());
        if has_errors(wrote) {
            return Err(dest.take_error().unwrap_or_else(|| {
                Error::Malformed("qpdf: the output could not be written".to_owned())
            }));
        }
        if let Some(error) = dest.take_error() {
            return Err(error);
        }

        let len = self.bridge().get_buffer_length(dest.data());
        let ptr = self.bridge().get_buffer(dest.data());
        // BOTH are checked, not one: qpdf returns a null buffer when the writer has none and
        // reports the length from a separate accessor, so a caller trusting only the length
        // would read from null and one trusting only the pointer would call zero bytes a
        // document.
        if ptr.is_null() || len == 0 {
            return Err(Error::Io("qpdf produced no output".to_owned()));
        }
        let output = self.bridge().copy_out(ptr, len);

        // WHAT THE SPLIT HAS COST SO FAR. `max_memory_bytes` DETECTS rather than bounds (ADR
        // 0007), and this is the only place on the split path that can detect anything: the
        // caller accumulates every part, and the source stays open in the engine heap throughout.
        crate::estimate::check_measured_memory(
            Some(source.heap_before),
            Some(self.bridge().heap_bytes()),
            &source.limits,
        )?;

        Ok(output)
    }
}
