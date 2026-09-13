//! The qpdf implementation of [`PageAssembler`].
//!
//! # Why qpdf and not PDFium
//!
//! [ADR 0017](../../../../docs/adr/0017-merge-engine-and-failure-semantics.md) measured
//! both. qpdf preserves an outline, an attachment and a working form field across a merge;
//! PDFium loses all three. The form-field case is the one that decided it, and it is worse
//! than "loses": PDFium keeps the widget annotation and drops the `/AcroForm`, so the
//! merged document shows a field that is no longer a field. A feature that disappears is a
//! disappointment; one that looks present and is dead is the failure class non-negotiable
//! #4 exists for.
//!
//! # The three ordering rules, each of which cost something to find
//!
//! 1. **Sources must outlive the write.** qpdf resolves a foreign page's indirect objects
//!    lazily, when the destination is serialised — so releasing a source after `add_page`
//!    but before `qpdf_write` yields a *truncated document rather than an error*. The
//!    `Assembly` owns them for exactly this reason; it is not tidiness.
//! 2. **Write parameters come after `qpdf_init_write_memory`, never before.** `qpdf-c.h`
//!    says they are set between `init_write` and `write`; calling
//!    `qpdf_set_deterministic_ID` first dereferences a writer that does not exist and
//!    aborts the process. Found by core dump.
//! 3. **Status is a bitmask.** `QPDF_WARNINGS` is bit 0, `QPDF_ERRORS` is bit 1. `!= 0`
//!    treats a warnings-only result as a failure, and every damaged file in the corpus
//!    produces warnings.
//!
//! # An input qpdf can read is not necessarily one it can merge
//!
//! ADR 0013 records that qpdf reads a truncated file and a trailer-less one that PDFium
//! refuses outright. That finding is about `qpdf_get_num_pages`. Appending needs
//! `qpdf_get_page_n` and `qpdf_add_page` as well, and those are a strictly higher bar:
//! measured, qpdf counts three pages in `trailer-removed.pdf` and then fails to extract
//! the first. So a [`Error::Malformed`] out of [`PageAssembler::append`] is an ordinary
//! outcome, not evidence of a bug.

use std::sync::Arc;

use burrow_types::{Deadline, Error, Limits, Result, Stage};

use super::{Document, ffi};
use crate::{OpenOptions, PageAssembler};

/// A merge in progress.
///
/// **Every source stays open until [`finish`](PageAssembler::finish).** See rule 1 in the
/// module documentation: qpdf reads them during the write, and dropping one early produces
/// a short document rather than a failure — the silent data loss
/// [ADR 0017](../../../../docs/adr/0017-merge-engine-and-failure-semantics.md) §2 refuses.
pub struct Assembly {
    /// The destination, which is the first input. The output inherits its catalog.
    dest: Document,
    /// Held, not used. Dropping one before `finish` truncates the output.
    sources: Vec<Document>,
    /// Pages appended so far, counted as we go rather than asked for twice.
    pages: u64,
    /// Resident set before the first document was opened, for the measured memory check.
    rss_before: Option<u64>,
    /// The ceilings this assembly was begun under.
    ///
    /// Carried rather than re-supplied at `finish`, because the aggregate checks are about
    /// the assembly as a whole: the page total and the measured memory both span every
    /// input, so they have to be judged against one set of ceilings rather than whichever
    /// happened to be passed last.
    limits: Limits,
}

impl PageAssembler for super::Qpdf {
    type Assembly = Assembly;

    fn name(&self) -> &'static str {
        "qpdf"
    }

    fn begin(&self, first: Box<[u8]>, options: &OpenOptions<'_>) -> Result<Self::Assembly> {
        let limits = options.limits;

        let input_len = u64::try_from(first.len())
            .map_err(|_| Error::Internal("input length does not fit in u64".to_owned()))?;
        Limits::check(
            Stage::InputSize,
            "max_input_bytes",
            input_len,
            limits.max_input_bytes,
        )?;

        // The structural pre-scan, per input, before any C++ parser sees the bytes. It is
        // the only pre-emptive defence there is (ADR 0013); `max_memory_bytes` detects
        // rather than bounds everywhere else.
        crate::prescan::check(&first, &limits)?;

        let clock = Arc::clone(&options.clock);
        let deadline = Deadline::start(clock.as_ref(), &limits);
        deadline.checkpoint(clock.as_ref())?;

        let rss_before = crate::rss::resident_bytes();

        // Recovery off, like `StructureEngine::check`'s default and for a stronger reason:
        // a merge that silently reconstructs a damaged input produces an output whose
        // relationship to what the person handed us is unclear, and they will never know.
        let dest = Document::open(first, options.password, false)?;

        let pages = dest.page_count()?;
        Limits::check(Stage::PageCount, "max_pages", pages, limits.max_pages)?;

        if let Some(error) = dest.take_error() {
            return Err(error);
        }

        Ok(Assembly {
            dest,
            sources: Vec::new(),
            pages,
            rss_before,
            limits,
        })
    }

    fn pages(&self, assembly: &Self::Assembly) -> Result<u64> {
        Ok(assembly.pages)
    }

    fn append(
        &self,
        assembly: &mut Self::Assembly,
        next: Box<[u8]>,
        options: &OpenOptions<'_>,
    ) -> Result<u64> {
        let limits = options.limits;

        let input_len = u64::try_from(next.len())
            .map_err(|_| Error::Internal("input length does not fit in u64".to_owned()))?;
        Limits::check(
            Stage::InputSize,
            "max_input_bytes",
            input_len,
            limits.max_input_bytes,
        )?;
        crate::prescan::check(&next, &limits)?;

        let source = Document::open(next, options.password, false)?;
        let count = source.page_count()?;

        // The ceiling is on the OUTPUT, checked before a single page is copied. A hundred
        // inputs of a hundred pages each is ten thousand pages from a caller who set
        // `max_pages` to a thousand, and per-input checking would wave every one of them
        // through. ADR 0017 §3.
        let total = assembly
            .pages
            .checked_add(count)
            .ok_or_else(|| Error::Internal("merged page count does not fit in u64".to_owned()))?;
        Limits::check(Stage::PageCount, "max_pages", total, limits.max_pages)?;

        for index in 0..count {
            let n = usize::try_from(index)
                .map_err(|_| Error::Internal("page index does not fit in usize".to_owned()))?;

            // SAFETY: `source.data` is a live handle whose document read successfully, and
            // `n` is below the page count it just reported. `qpdf_get_page_n` routes
            // through `trap_errors`, so a C++ exception becomes a recorded error rather
            // than an unwind into Rust.
            let page = unsafe { ffi::qpdf_get_page_n(source.data, n) };
            if let Some(error) = source.take_error() {
                return Err(error);
            }

            // SAFETY: both handles are live. `page` belongs to `source`, which is passed
            // alongside it — handles are per-document and qpdf resolves it against the one
            // it is given. `qpdf_add_page` routes through `trap_errors`.
            //
            // `source` is moved into `assembly.sources` below and dropped only by
            // `finish`, so the objects this call registers are still resolvable when the
            // write reads them.
            let added = unsafe {
                ffi::qpdf_add_page(assembly.dest.data, source.data, page, ffi::QPDF_FALSE)
            };

            // The ERROR bit, never `!= 0`: a warning here is ordinary.
            if ffi::has_errors(added) {
                return Err(assembly
                    .dest
                    .take_error()
                    .or_else(|| source.take_error())
                    .unwrap_or_else(|| {
                        Error::Malformed("qpdf: a page could not be appended".to_owned())
                    }));
            }
            if let Some(error) = assembly.dest.take_error() {
                return Err(error);
            }
        }

        assembly.sources.push(source);
        assembly.pages = total;
        Ok(count)
    }

    fn finish(&self, assembly: Self::Assembly) -> Result<Vec<u8>> {
        let Assembly {
            dest,
            sources,
            pages: _,
            rss_before,
            limits,
        } = assembly;

        // SAFETY: `dest.data` is a live handle. Routes through `trap_errors`.
        let init = unsafe { ffi::qpdf_init_write_memory(dest.data) };
        if ffi::has_errors(init) {
            return Err(dest.take_error().unwrap_or_else(|| {
                Error::Io("qpdf could not prepare an in-memory write".to_owned())
            }));
        }

        // AFTER `qpdf_init_write_memory`, never before -- rule 2 in the module docs. The
        // writer does not exist until the call above succeeds, and this dereferences it.
        //
        // SAFETY: `dest.data` is live and its writer was just created successfully. The
        // call assigns a bool and parses nothing; it is argued in
        // `engines/qpdf-untrapped-accepted.toml`.
        unsafe { ffi::qpdf_set_deterministic_ID(dest.data, ffi::QPDF_TRUE) };

        // SAFETY: `dest.data` is live with a prepared writer, and every source is still
        // alive in `sources` -- which is why that binding exists and is dropped only at the
        // end of this function. Routes through `trap_errors`.
        let wrote = unsafe { ffi::qpdf_write(dest.data) };
        if ffi::has_errors(wrote) {
            return Err(dest.take_error().unwrap_or_else(|| {
                Error::Malformed("qpdf: the merge could not be written".to_owned())
            }));
        }
        if let Some(error) = dest.take_error() {
            return Err(error);
        }

        // SAFETY: the write succeeded, so `output_buffer` exists. Both accessors are
        // untrapped and argued in `engines/qpdf-untrapped-accepted.toml`; the precondition
        // recorded there is exactly the one established above -- `qpdf_init_write_memory`
        // returned success and `qpdf_write` did too.
        let len = unsafe { ffi::qpdf_get_buffer_length(dest.data) };
        // SAFETY: as above. The pointer is owned by `dest.data` and dies on the next
        // `qpdf_init_write*` or `qpdf_cleanup`, so it is copied out immediately below and
        // never stored.
        let ptr = unsafe { ffi::qpdf_get_buffer(dest.data) };

        if ptr.is_null() || len == 0 {
            return Err(Error::Io("qpdf produced no output".to_owned()));
        }

        // SAFETY: `ptr` is non-null and `len` is the length qpdf reports for that same
        // buffer, read from the same `qpdf_data` with no call in between that could
        // invalidate either. The copy happens before `dest` is dropped.
        let out = unsafe { core::slice::from_raw_parts(ptr, len) }.to_vec();

        // Dropped here, after the write and after the copy, and in this order so the
        // reason is visible: the sources were needed until `qpdf_write` returned.
        drop(sources);
        drop(dest);

        // What the whole assembly cost. `max_memory_bytes` DETECTS rather than bounds --
        // ADR 0007's 2026-09-12 amendment -- and a merge is the clearest case for saying so:
        // the allocation has already happened across every input by the time this runs.
        crate::estimate::check_measured_memory(rss_before, crate::rss::resident_bytes(), &limits)?;

        Ok(out)
    }
}
