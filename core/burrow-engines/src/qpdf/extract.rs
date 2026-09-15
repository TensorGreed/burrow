//! The qpdf implementation of [`PageExtractor`].
//!
//! # It builds an output; it does not carve one
//!
//! [ADR 0019](../../../../docs/adr/0019-how-split-builds-its-outputs.md) measured the
//! alternative. Reading the source and removing the unwanted pages keeps the source's catalog
//! whole, so a two-page output carried the outline titles of **all five** source pages — three
//! of them pointing at pages that are not in the document. Somebody extracting pages 2-3 to
//! send to another person would have been sending the titles of pages 4 and 5 with them.
//!
//! So the destination starts empty and pages are copied **in**. That is an allowlist by
//! construction: nothing is in the output unless this file put it there. Carving is a denylist
//! over a structure whose design permits keys nobody enumerated, and it closes only the leaks
//! somebody thought of.
//!
//! # The empty destination is our bytes, and that is not incidental
//!
//! qpdf offers `qpdf_empty_pdf` and **burrow may not call it**. It is not on
//! `engines/qpdf-trapped-functions.txt`, and it cannot earn an entry in
//! `engines/qpdf-untrapped-accepted.toml`, whose bar is "non-parsing … never touches the PDF's
//! bytes": its body is `QPDF::emptyPDF()`, which is `processMemoryFile("empty PDF", EMPTY_PDF,
//! …)` — the full parser, with no `trap_errors` wrapper on the C function. A `QPDFExc` out of
//! that crosses an `extern "C"` frame and aborts the process (ADR 0013 §1).
//!
//! [`BLANK_DOCUMENT`] goes in through the trapped `qpdf_read_memory` instead. It is a
//! compile-time constant in this crate, so it is not attacker-controlled and it is not
//! something an engine handed us.
//!
//! # One new FFI declaration, already trapped
//!
//! `open`, `get_page_n`, `add_page` and the write path were all declared for `merge`; this
//! route adds `qpdf_remove_page`, which is on `engines/qpdf-trapped-functions.txt` and so
//! needs no argued exception. The checklist's six-artifact table for a new engine capability
//! (`add-operation` §2a) therefore applies to one row rather than six: no qpdf rebuild, no
//! export-list change, no new content hashes, no size-budget re-measure. (An earlier version
//! of this comment said "no new FFI", which was false the moment the blank page needed
//! removing.)

use burrow_types::{Deadline, Error, Limits, Result, Stage};

use super::handle::ObjectHandle;
use super::{Document, Qpdf, ffi};
use crate::blank::BLANK_DOCUMENT;
use crate::{OpenOptions, PageExtractor};

/// A source document held open for the life of a split.
pub struct QpdfSource {
    document: Document,
    pages: u64,
    /// The resident set before the source was opened.
    ///
    /// `max_memory_bytes` **detects rather than bounds** (ADR 0007), and detecting it needs a
    /// before. Split is the operation with the worst amplification for it: every output is
    /// held in full while the source stays in the engine heap, so a 500-page document cut 499
    /// ways holds 500 written documents at once.
    rss_before: Option<u64>,
    /// The ceilings the source was opened under, so `extract` can apply the measured check
    /// without the caller having to pass them twice.
    limits: Limits,
    /// For each source page, the other source pages its `/Annots` array is shared with.
    ///
    /// **Read here and not in `extract`, because by then it is gone.** The destination holds a
    /// *copy* of the array, and a copy shared with an excluded page is indistinguishable from a
    /// copy shared with nobody — so the question can only be asked of the source, before
    /// anything is copied. See `prune::annots_sharing` for why sharedness rather than `/P`
    /// alone decides the filter.
    ///
    /// Computed once per split rather than once per output: it is a sweep over the source's
    /// pages, and the answer does not depend on which run is being extracted. What DOES depend
    /// on the run is whether any of those sharers falls outside it, which `extract` works out
    /// per output.
    annots_shared: Vec<Vec<u64>>,
    /// The deadline `open` started, so the pruning pass spends the same budget.
    ///
    /// **Not a fresh one.** `Deadline::start` resets the origin *and* the budget, so a pass that
    /// made its own would be handed a whole second `max_duration_ms` — the defect ADR 0022
    /// records being fixed three times over in `rotations`.
    deadline: Deadline,
}

impl PageExtractor for Qpdf {
    type Source = QpdfSource;

    fn name(&self) -> &'static str {
        "qpdf"
    }

    fn open(&self, bytes: Box<[u8]>, options: &OpenOptions<'_>) -> Result<Self::Source> {
        // Every ceiling, in one place, shared with the other operations that open a document
        // this way -- see `open_document` for why this is not written out here.
        let (document, pages, rss_before, deadline) = super::open_document(bytes, options)?;
        // INSIDE THE OPEN'S OWN DEADLINE, not a fresh one. This is a sweep over every source
        // page, so on a `max_pages`-sized document it is not "one engine call" of overshoot --
        // and `Deadline::start` resets the budget as well as the origin, so making one here
        // would hand the sweep a whole second `max_duration_ms`. Found by code review.
        let annots_shared = crate::prune::annots_sharing(
            &super::prune::QpdfGraph::over(&document),
            pages,
            options,
            &deadline,
        )?;
        Ok(QpdfSource {
            document,
            pages,
            rss_before,
            limits: options.limits,
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
        // THE SAME WALK `rotate` AND `reorder` DO, against the document this is about to take
        // pages out of. Reusing their helpers rather than repeating the `/Parent` climb: the
        // depth ceiling and the type assertion live there, and a second copy of a walk over
        // hostile input is a second place to get them wrong.
        let capacity = usize::try_from(source.pages)
            .map_err(|_| Error::Internal("page count does not fit in usize".to_owned()))?;
        let mut rotations = Vec::with_capacity(capacity);

        // PER PAGE, and the caller's deadline rather than a new one -- see the trait's docs.
        // This is the sweep ADR 0022 measured at 84% of the operation on a 10,000-page document
        // with a 60-deep page tree, which is the shape it is sensitive to: it walks `/Parent`
        // per page, where the pruning pass runs after the tree has been flattened and does not.
        let clock = std::sync::Arc::clone(&options.clock);
        for index in 0..source.pages {
            deadline.checkpoint(clock.as_ref())?;
            let page = super::reorder::page_handle(&source.document, index, source.pages)?;
            // RECORDED, NOT JUDGED, for the reason `reorder` gives: `effective_rotation` refuses
            // a `/Rotate` that is not a multiple of 90, and a split names no page to turn -- so
            // normalising here would fail a whole operation over a page it only ever copies.
            rotations.push(super::rotate::declared_rotation(&source.document, &page)?.unwrap_or(0));
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
        // `options` carries the clock and the password; the CEILINGS come from `source.limits`
        // -- see the checks below for why. Binding `options.limits` here would put the laxer
        // set one line from the stricter one, which is how the two checks came to disagree.
        let _ = options;

        let end = first
            .checked_add(count)
            .ok_or_else(|| Error::InvalidArgument("page run overflows".to_owned()))?;
        if count == 0 || end > source.pages {
            return Err(Error::InvalidArgument(
                "page run is not inside the document".to_owned(),
            ));
        }
        // `source.limits`, for the reason the memory check below gives: the ceiling that
        // counts is the one the document was opened under, and a caller passing laxer options
        // to a later `extract` must not be able to raise it after the fact. `split` passes the
        // same options throughout, so this is unreachable from there -- but `PageExtractor` is
        // a public trait and the two checks disagreeing was an inconsistency, not a design.
        Limits::check(
            Stage::PageCount,
            "max_pages",
            count,
            source.limits.max_pages,
        )?;

        // The destination: our own blank document, through the trapped read.
        let dest = Document::open(BLANK_DOCUMENT.to_vec().into_boxed_slice(), None, false)?;
        if let Some(error) = dest.take_error() {
            return Err(error);
        }

        for n in first..end {
            let index = usize::try_from(n)
                .map_err(|_| Error::Internal("page index does not fit in usize".to_owned()))?;

            // SAFETY: `index` is below the source's page count, checked above. Routes
            // through `trap_errors`. Wrapped rather than held raw, so the handle is released
            // at the end of this iteration instead of living until the document is dropped —
            // see `handle.rs`.
            let page = unsafe { ObjectHandle::page(&source.document, index) };
            if let Some(error) = source.document.take_error() {
                return Err(error);
            }

            // `QPDF_FALSE` is `first`: append rather than prepend, so pages arrive in source
            // order. The handle belongs to the source, which is why the source is passed
            // alongside it — handles are per-document.
            //
            // SAFETY: both handles are live, and `page` was just obtained from the source.
            // Routes through `trap_errors`.
            let added = unsafe {
                ffi::qpdf_add_page(dest.data, source.document.data, page.raw(), ffi::QPDF_FALSE)
            };
            // The ERROR bit, never `!= 0`: a warning here is an ordinary outcome.
            if ffi::has_errors(added) {
                return Err(dest
                    .take_error()
                    .or_else(|| source.document.take_error())
                    .unwrap_or_else(|| {
                        Error::Malformed("qpdf: a page could not be copied".to_owned())
                    }));
            }
            if let Some(error) = dest.take_error() {
                return Err(error);
            }
        }

        // THE BLANK PAGE COMES BACK OUT, now that the document has real pages in it. It has
        // to be removed last rather than first, because a document with no pages is not one
        // qpdf will hold — which is the same fact that made this constant one page instead of
        // none.
        //
        // SAFETY: index 0 is in range: the blank page is still there and at least one more
        // page was just added. Routes through `trap_errors`.
        let blank = unsafe { ObjectHandle::page(&dest, 0) };
        if let Some(error) = dest.take_error() {
            return Err(error);
        }
        // SAFETY: `blank` was just obtained from `dest` and handles are per-document, so it
        // belongs to the document it is being removed from. Routes through `trap_errors`.
        let removed = unsafe { ffi::qpdf_remove_page(dest.data, blank.raw()) };
        if ffi::has_errors(removed) {
            return Err(dest.take_error().unwrap_or_else(|| {
                Error::Internal("qpdf: the blank destination page could not be removed".to_owned())
            }));
        }
        if let Some(error) = dest.take_error() {
            return Err(error);
        }

        // WHAT THE COPY DRAGGED ALONG, taken back out. `qpdf_add_page` copies a reachability
        // closure rather than a page (ADR 0019 §2a), so at this point the destination holds
        // every object the wanted pages could reach -- an inherited resource dictionary, a form
        // field whose siblings are on pages this output excludes, a shared annotation array, an
        // article bead. `prune` is the second half of the rule §2 states: build, and then ask
        // separately what came with it.
        //
        // AFTER the blank page is gone, so the indices below are over real pages only.
        //
        // WHOLE-OUTPUT, not per page, because the objects being pruned are SHARED between pages
        // -- the flattening pushes one `/Resources` reference onto every page under a node, so a
        // per-page pass would delete page 4's font while computing page 1's answer. See
        // `prune_output`.
        let kept = usize::try_from(count)
            .map_err(|_| Error::Internal("page count does not fit in usize".to_owned()))?;
        let mut ambiguous = Vec::with_capacity(kept);
        for at in 0..kept {
            // THE SOURCE PAGE'S sharing answer, not the destination page's: `at` is an index
            // into this output and `first + at` is the page it came from. Getting this wrong
            // would filter one page's annotations by another page's rule, which on a document
            // where only some arrays are shared is a leak on exactly the pages that have one.
            let source_index = usize::try_from(first)
                .ok()
                .and_then(|first| first.checked_add(at))
                .ok_or_else(|| Error::Internal("source page index does not fit".to_owned()))?;
            let sharers = source.annots_shared.get(source_index).ok_or_else(|| {
                Error::Internal("a copied page has no recorded source page".to_owned())
            })?;
            // AMBIGUOUS MEANS "SHARED WITH A PAGE THIS OUTPUT DOES NOT CONTAIN", not "shared".
            // An array every one of whose pages is in this output carries nothing from excluded
            // content, and requiring `/P` of its annotations would delete the kept pages' own --
            // which a one-way split, excluding nothing, measured as losing every annotation in
            // the document.
            ambiguous.push(sharers.iter().any(|page| *page < first || *page >= end));
        }
        crate::prune::prune_output(
            &super::prune::QpdfGraph::over(&dest),
            &ambiguous,
            options,
            &source.deadline,
        )?;

        let output = write_out(&dest, &source.document)?;

        // WHAT THE SPLIT HAS COST SO FAR. `max_memory_bytes` DETECTS rather than bounds
        // (ADR 0007's 2026-09-12 amendment), and this is the only place on the split path
        // that can detect anything: the caller accumulates every output, the source stays
        // open in the engine heap throughout, and neither is visible from inside a single
        // extraction. Checked per output rather than once at the end, because "once at the
        // end" is after the memory has already been spent.
        //
        // `source.limits` rather than `options.limits`: the ceiling that matters is the one
        // the document was opened under, and a caller passing a laxer one to a later
        // `extract` must not be able to raise it after the fact.
        crate::estimate::check_measured_memory(
            source.rss_before,
            crate::rss::resident_bytes(),
            &source.limits,
        )?;

        Ok(output)
    }
}

/// Serialise the destination, with the source still alive.
///
/// **The source must outlive this call.** qpdf resolves a foreign page's indirect objects
/// lazily, when the destination is written, so releasing it first yields a *truncated document
/// rather than an error* — the same rule `assemble.rs` records, arriving from the other
/// direction. It is a parameter here so a caller cannot forget.
///
/// **`rotate` passes the same document twice, and that is correct.** It edits a page attribute
/// in place rather than copying pages between documents, so the document that must outlive the
/// write *is* the one being written. `write_out(&doc, &doc)` reads like a mistake and is not;
/// the parameter states a rule, and in the in-place case the same document satisfies it.
pub(super) fn write_out(dest: &Document, _source_must_outlive_this: &Document) -> Result<Vec<u8>> {
    // SAFETY: `dest.data` is a live handle. Routes through `trap_errors`.
    let init = unsafe { ffi::qpdf_init_write_memory(dest.data) };
    if ffi::has_errors(init) {
        return Err(dest
            .take_error()
            .unwrap_or_else(|| Error::Io("qpdf could not prepare an in-memory write".to_owned())));
    }

    // AFTER `qpdf_init_write_memory`, never before: the writer does not exist until that call
    // succeeds, and this dereferences it. Found by core dump during ADR 0017's comparison.
    //
    // SAFETY: the writer was just created successfully. The call assigns a bool and parses
    // nothing; argued in `engines/qpdf-untrapped-accepted.toml`.
    unsafe { ffi::qpdf_set_deterministic_ID(dest.data, ffi::QPDF_TRUE) };

    // SAFETY: `dest.data` is live with a prepared writer, and the source is alive for the
    // whole of this call because it is borrowed by it. Routes through `trap_errors`.
    let wrote = unsafe { ffi::qpdf_write(dest.data) };
    if ffi::has_errors(wrote) {
        return Err(dest.take_error().unwrap_or_else(|| {
            Error::Malformed("qpdf: the output could not be written".to_owned())
        }));
    }
    if let Some(error) = dest.take_error() {
        return Err(error);
    }

    // SAFETY: the write succeeded, so the output buffer exists. Both accessors are untrapped
    // and argued in `engines/qpdf-untrapped-accepted.toml` under exactly this precondition.
    let len = unsafe { ffi::qpdf_get_buffer_length(dest.data) };
    // SAFETY: as above. The pointer is owned by `dest.data` and dies on the next
    // `qpdf_init_write*` or `qpdf_cleanup`, so it is copied out immediately and never stored.
    let ptr = unsafe { ffi::qpdf_get_buffer(dest.data) };

    // BOTH are checked, not one. qpdf returns a null buffer when the writer has none and
    // reports the length from a separate accessor — so a caller trusting only the length
    // would read from null, and one trusting only the pointer would copy zero bytes and call
    // it a document.
    if ptr.is_null() || len == 0 {
        return Err(Error::Io("qpdf produced no output".to_owned()));
    }

    // SAFETY: `ptr` is non-null and qpdf reports `len` readable bytes at it, valid until the
    // next write call or cleanup on this handle. Copied out immediately.
    Ok(unsafe { std::slice::from_raw_parts(ptr, len) }.to_vec())
}
