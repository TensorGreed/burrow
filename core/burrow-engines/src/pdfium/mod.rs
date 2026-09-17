//! The PDFium implementation of [`DocumentEngine`].
//!
//! Compiled only when `build.rs` has confirmed the vendored engines are present and
//! checksum-verified. See `docs/adr/0004-native-engines.md` for acquisition,
//! `docs/adr/0011-pdfium-engine-thread.md` for why every call here runs on one thread,
//! and `docs/adr/0007-limit-enforcement-per-platform.md` for what the limits actually
//! promise.

// Private to `pdfium`, deliberately. Every function in it carries the unlisted
// precondition "call me only from the engine thread", and keeping the module private is
// what makes "no `FPDF_*` call exists outside this directory" checkable with `rg` rather
// than by reading `unsafe` blocks. Only the link check needs anything from it, and it gets
// exactly one item, below.
mod ffi;

pub(crate) mod thread;

use std::sync::Arc;

use core::ffi::{c_char, c_int, c_void};

use burrow_types::{Clock, Deadline, Error, Limits, Result, Stage};

use crate::{DocumentEngine, OpenOptions, PageRenderer, Raster};

/// Re-exported for the link check, which proves `FPDF_DestroyLibrary` binds without ever
/// calling it. The only part of [`ffi`] visible outside this module.
#[cfg(test)]
pub(crate) use self::ffi::destroy_library_symbol;

/// The PDFium-backed document engine.
///
/// Stateless: the engine itself is a process-wide singleton started on first use, so
/// constructing this is free and constructing several is the same as constructing one.
#[derive(Debug, Clone, Copy, Default)]
pub struct Pdfium;

impl Pdfium {
    /// A handle to the PDFium engine.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

/// An open PDFium document.
///
/// An identifier, a time budget, and the clock that budget is measured against. The raw
/// `FPDF_DOCUMENT` and the buffer it reads from stay inside the engine thread, which is
/// why this is `Send + Sync` without any `unsafe impl`.
///
/// **The clock is captured here at open, not passed per call.** An earlier design took a
/// `&dyn Clock` on every method, and that silently disabled `max_duration_ms`: a
/// [`Deadline`] records `start_ms` from one clock's *unspecified* epoch, so measuring it
/// against a second clock produced a saturated zero and the limit could never fire again —
/// no error, no warning. Capturing the clock removes the parameter that made that
/// expressible.
///
/// Dropping this closes the document. The close is submitted to the engine thread and not
/// waited for, so a drop never blocks.
pub struct PdfiumDocument {
    id: u64,
    deadline: Deadline,
    clock: Arc<dyn Clock>,
    pages_at_open: u64,
    /// The resident set when this document was opened.
    ///
    /// **The baseline a strip is measured against**, so `estimate::before_page_load` can refuse
    /// to enter another uninterruptible `FPDF_LoadPage` once this document's rendering has
    /// already spent the caller's budget. Taken at open rather than per call because the
    /// question is what the *strip so far* cost, not what one page did.
    memory_at_open: Option<u64>,
}

impl PdfiumDocument {
    /// The page count read at open, before any page was touched.
    ///
    /// This is the number `max_pages` was checked against, which is why it is kept rather
    /// than re-read: it is the value the limit decision was actually made on.
    #[must_use]
    pub fn pages_at_open(&self) -> u64 {
        self.pages_at_open
    }
}

impl core::fmt::Debug for PdfiumDocument {
    /// Hand-written because `Arc<dyn Clock>` has no `Debug`.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PdfiumDocument")
            .field("id", &self.id)
            .field("deadline", &self.deadline)
            .field("pages_at_open", &self.pages_at_open)
            .finish_non_exhaustive()
    }
}

impl Drop for PdfiumDocument {
    fn drop(&mut self) {
        thread::close(self.id);
    }
}

/// Widen a page count PDFium reported, having already established it is not negative.
fn page_count_to_u64(count: c_int) -> Result<u64> {
    u64::try_from(count)
        .map_err(|_| Error::Internal("pdfium reported a page count that is not a count".to_owned()))
}

impl DocumentEngine for Pdfium {
    type Document = PdfiumDocument;

    fn name(&self) -> &'static str {
        "pdfium"
    }

    fn open(&self, bytes: Box<[u8]>, options: &OpenOptions<'_>) -> Result<Self::Document> {
        let limits = options.limits;

        // Empty input can never be a document, and refusing it here also avoids handing
        // PDFium the dangling (if correctly aligned) pointer an empty `Box<[u8]>` carries.
        if bytes.is_empty() {
            return Err(Error::Malformed("input is empty".to_owned()));
        }

        // 1. Input size, before the buffer goes anywhere near the engine.
        // EVERY CEILING THAT APPLIES BEFORE THE ENGINE, in one call: the byte count,
        // the size estimate, the structural pre-scan. Shared rather than spelled out
        // here, so a path that pre-scans without estimating is not writable --- see
        // `crate::estimate::before_open`, and #26 for what the drift cost last time.
        crate::estimate::before_open(&bytes, &limits)?;

        // 4. The password copy, before the deadline starts: it is our work, not the
        //    engine's, and a rejected password should not consume the caller's budget.
        let password = crate::password::nul_terminated(options.password, "pdfium")?;

        // 5. The budget for everything that follows, including later calls on the handle.
        let clock = Arc::clone(&options.clock);
        let deadline = Deadline::start(clock.as_ref(), &limits);
        deadline.checkpoint(clock.as_ref())?;

        let opened = thread::submit(move |registry| {
            // a. Claim the id, then park the buffer. Both happen before the load, so the
            //    only fallible step comes while there is still nothing to clean up, and
            //    the pointer handed to PDFium is read from the buffer's final home.
            let id = registry.reserve_id()?;
            let (data, size) = registry.park_buffer(id, bytes);
            let password_ptr = password
                .as_ref()
                .map_or(core::ptr::null(), |p| p.as_ptr().cast::<c_char>());

            // From here on, `registry.remove(id)` is the single cleanup for every failure:
            // it frees the buffer, and closes the document too once one is attached.
            let before = crate::rss::resident_bytes();

            // SAFETY: `data` is valid for `size` bytes -- it points into the buffer the
            // registry now owns, which is not moved again until `remove`, and PDFium reads
            // from it for as long as the document is open (`fpdfview.h:451`). `size` is
            // non-zero, so the pointer is a real allocation. `password_ptr` is null, or
            // points at a NUL-terminated buffer alive for the whole call. This runs inside
            // a job, so it is on the engine thread and after init.
            let (doc, code) =
                unsafe { ffi::load_mem_document64(data.cast::<c_void>(), size, password_ptr) };

            // The copy of the password has done its job; wipe it now rather than at the
            // end of the closure.
            drop(password);

            if doc.is_null() {
                // Failure is established by the null handle. The code only classifies it,
                // and it was read in the same call, so it is this load's code.
                registry.remove(id);
                return Err(crate::codes::pdfium::map_failure(code));
            }

            // b. Hand the document to the registry immediately, so from here every exit --
            //    including an unwind -- closes it exactly once.
            //
            // SAFETY: `doc` is non-null (checked above), came from the load on the line
            // before, was loaded from the buffer parked under `id`, and is not yet closed.
            unsafe { registry.attach(id, doc) };

            let checked = (|| -> Result<u64> {
                // c. The page count, immediately, before any page is touched.
                //
                // SAFETY: `doc` is non-null and open -- the registry holds it and nothing
                // has closed it. Still inside the job, so still on the engine thread.
                let (count, _code) = unsafe { ffi::get_page_count(doc) };
                if count < 0 {
                    // `_code` is deliberately unused. `fpdfview.h:625` says the error is
                    // only meaningful for APIs whose own documentation mentions
                    // `FPDF_GetLastError`, and `FPDF_GetPageCount`'s block does not. The
                    // global would hold whatever some earlier call left there, so
                    // classifying by it would pin another operation's error to this one --
                    // a stale `FPDF_ERR_PASSWORD` would surface as `PasswordRequired` on a
                    // document that is already open.
                    return Err(crate::codes::pdfium::unreadable_page_tree());
                }
                let pages = page_count_to_u64(count)?;
                if pages == 0 {
                    return Err(Error::Malformed("document has no pages".to_owned()));
                }

                // d. The page limit, on a number the engine produced.
                Limits::check(Stage::PageCount, "max_pages", pages, limits.max_pages)?;

                // e. What the open actually cost. Step 2's estimate is blind to anything
                //    the file *declares*, and a small file declaring an enormous structure
                //    is exactly the case it misses.
                crate::estimate::check_measured_memory(
                    before,
                    crate::rss::resident_bytes(),
                    &limits,
                )?;

                Ok(pages)
            })();

            match checked {
                Ok(pages) => Ok((id, pages)),
                Err(error) => {
                    // A document that has already broken a limit, or whose page tree
                    // cannot be read, is never handed back.
                    registry.remove(id);
                    Err(error)
                }
            }
        })?;

        deadline.checkpoint(clock.as_ref())?;

        let (id, pages) = opened;
        Ok(PdfiumDocument {
            id,
            deadline,
            clock,
            pages_at_open: pages,
            memory_at_open: crate::rss::resident_bytes(),
        })
    }

    fn page_count(&self, doc: &Self::Document) -> Result<u64> {
        // Against the clock captured at open, so the budget covers the whole operation and
        // cannot be reset by handing in a different one.
        doc.deadline.checkpoint(doc.clock.as_ref())?;

        let id = doc.id;
        let pages = thread::submit(move |registry| {
            let handle = registry.handle(id)?;
            // SAFETY: `handle` came from the registry, so the document is open and its
            // buffer is alive. Inside a job, so on the engine thread.
            let (count, _code) = unsafe { ffi::get_page_count(handle) };
            if count < 0 {
                // See the note in `open`: the error global is undefined after this call.
                return Err(crate::codes::pdfium::unreadable_page_tree());
            }
            page_count_to_u64(count)
        })?;

        doc.deadline.checkpoint(doc.clock.as_ref())?;
        Ok(pages)
    }
}

/// Narrow a zero-based page index for PDFium's `int`, having established it is in range.
///
/// `total` is the count the document was opened with, so "in range" is checked against the
/// number `max_pages` was applied to rather than against a fresh read.
fn page_index(index: u64, total: u64) -> Result<c_int> {
    if index >= total {
        return Err(Error::InvalidArgument(format!(
            "page {} of a {total}-page document",
            index.saturating_add(1)
        )));
    }
    c_int::try_from(index)
        .map_err(|_| Error::Internal("page index does not fit in an engine index".to_owned()))
}

/// Narrow a raster dimension for PDFium's `int`.
///
/// Reached only after `crate::raster::check_pixels`, so a failure here means the caller set
/// `max_pixels` above what the engine's own API can express -- a refusal of the request, which
/// is why it is `InvalidArgument` and not `Internal`.
fn raster_dimension(value: u32) -> Result<c_int> {
    c_int::try_from(value).map_err(|_| {
        Error::InvalidArgument("a raster dimension is larger than the engine accepts".to_owned())
    })
}

/// How many slices pass between memory readings.
///
/// The deadline is checked on EVERY slice: it is a clock read, tens of nanoseconds, and a
/// hundred thousand of them are lost in the noise of a forty-second render. The resident-set
/// reading is a **procfs read** -- a syscall, and an open/read/parse at that -- so doing it per
/// slice on the 100,002-slice case would be a hundred thousand syscalls to measure something
/// that moves by a megabyte at a time.
///
/// 16 is chosen from the measurement: the worst single slice grew the resident set by 1.0 MiB,
/// so sixteen of them is at most 16 MiB between readings -- inside the 32 MiB-per-slice bar the
/// slice measurement was held to, with the same margin.
const SLICES_PER_MEMORY_READING: u32 = 16;

/// Draw a page with a checkpoint between slices.
///
/// **This replaced a single `FPDF_RenderPageBitmap`**, which has no checkpoint inside it at all:
/// one call on a hostile content stream was measured at 112 seconds and 2.7 GB while drawing a
/// 240x320 thumbnail, and the only thing that could end it was the page terminating the worker.
///
/// Measured on the same two inputs, PDFium yields every ~100 paths: **30,002 and 100,002
/// slices, p99 0.4 ms, longest 0.7 ms**. So a deadline lands within a millisecond of coming due
/// rather than minutes, and the operation ends with a typed refusal instead of a dead instance.
///
/// # What this does NOT cover, and it is most of the cost
///
/// `FPDF_LoadPage` runs before any of this and **cannot be checkpointed**: it parses the content
/// stream and builds the display list in one uninterruptible call, measured at **1.2 s / 757 MiB**
/// and **3.5 s / 1,765 MiB** on those two inputs -- around 70% of the peak. Interruptibility here
/// is a property of *rendering*, not of *loading*. ADR 0027's amendment says so in those words,
/// and the load phase is bounded by refusing to start one at all when the process is already
/// over budget -- see `crate::estimate::before_page_load`.
fn render_progressively(
    bitmap: ffi::FpdfBitmap,
    page: ffi::FpdfPage,
    width: c_int,
    height: c_int,
    deadline: &Deadline,
    clock: &dyn Clock,
    limits: &Limits,
) -> Result<()> {
    // PINNED FOR THE WHOLE RENDER, not just the first call. PDFium keeps this pointer until
    // `render_page_close`, so a `&mut` to a local that the compiler is free to move would be a
    // dangling callback on the second slice. The `Box` is what gives it a stable address.
    let mut pause = Box::new(ffi::IfsdkPause::always());
    let pause_ptr: *mut ffi::IfsdkPause = &mut *pause;

    let before = crate::rss::resident_bytes();

    // SAFETY: both handles are live and from the same document, the extent is the bitmap's own,
    // `pause` outlives every call below (it is dropped after `render_page_close`), and this runs
    // inside a job so it is on the engine thread.
    let mut state = unsafe { ffi::render_page_start(bitmap, page, width, height, pause_ptr) };

    let mut slices: u32 = 0;
    let outcome = loop {
        if state != ffi::FPDF_RENDER_TOBECONTINUED {
            break Ok(state);
        }

        // THE CHECKPOINT THE WHOLE MECHANISM EXISTS FOR. Every slice, because it is cheap.
        if let Err(error) = deadline.checkpoint(clock) {
            break Err(error);
        }

        // A COUNTDOWN MODULO THE INTERVAL, not a total. `saturating_add` pinned at `u32::MAX`,
        // where `is_multiple_of` is false forever and the readings silently stopped for the rest
        // of the render. Unreachable at 0.4 ms a slice -- about twenty days -- and a counter that
        // cannot run out costs nothing. Found by security review.
        slices = (slices + 1) % SLICES_PER_MEMORY_READING;
        // DETECTED BETWEEN SLICES RATHER THAN AFTER THE FACT, which is the second thing
        // progressive render buys and the first place in this crate where a memory reading can
        // stop work that is still going. It still DETECTS rather than bounds (ADR 0007): the
        // megabyte that crossed the line has already been allocated.
        if slices == 0
            && let Err(error) =
                crate::estimate::check_measured_memory(before, crate::rss::resident_bytes(), limits)
        {
            break Err(error);
        }

        // SAFETY: the page has a render in progress, started above with this same `pause`,
        // which is still alive. Engine thread.
        state = unsafe { ffi::render_page_continue(page, pause_ptr) };
    };

    // ON EVERY EXIT, including the refusals. PDFium holds the progressive context on the page
    // until this runs, so a `?` that skipped it would leak for the life of the document.
    //
    // SAFETY: a render was started on this page above; this is its only close, and it happens
    // before the page itself is closed by the caller.
    unsafe { ffi::render_page_close(page) };
    drop(pause);

    match outcome? {
        s if s == ffi::FPDF_RENDER_DONE => Ok(()),
        // FAILED (3) is the DOCUMENT: the engine tried and gave up.
        s if s == ffi::FPDF_RENDER_FAILED => {
            Err(Error::Malformed("the page could not be drawn".to_owned()))
        }
        // READY (0) means the engine NEVER STARTED, which is our call sequence rather than
        // anything about the file -- so it is `Internal`, not `Malformed`. Both were mapped to
        // `Malformed` under a comment that named the distinction and then collapsed it. Found
        // by code review. Neither is looped on: a loop that treats an unknown state as "keep
        // going" is a hang, which is the failure this whole change exists to remove.
        _ => Err(Error::Internal(
            "pdfium did not begin the render it was asked for".to_owned(),
        )),
    }
}

impl PageRenderer for Pdfium {
    fn page_size(&self, source: &Self::Document, index: u64) -> Result<(f32, f32)> {
        source.deadline.checkpoint(source.clock.as_ref())?;

        let id = source.id;
        let page_index = page_index(index, source.pages_at_open)?;
        let size = thread::submit(move |registry| {
            let handle = registry.handle(id)?;

            // NO `FPDF_LoadPage` HERE, AND THAT IS THE WHOLE POINT. This used to load the page,
            // read two floats and close it -- which builds the display list, the single most
            // expensive uninterruptible thing on this path (1,765 MiB and 3.5 s on the 10 M-path
            // input, #103), to answer a question the `/MediaBox` already contains. `render`
            // then loaded the same page again, so a strip paid the dominant cost twice per
            // page, and the first of the two went round `estimate::before_page_load` -- the
            // guard whose entire justification is that an uninterruptible load can only be
            // refused before it starts. Found by security review.
            //
            // SAFETY: `handle` came from the registry, so the document is open. Inside a job,
            // so on the engine thread.
            let size = unsafe { ffi::page_size_by_index(handle, page_index) };
            size.ok_or_else(|| Error::Malformed("a page has no usable size".to_owned()))
        })?;

        source.deadline.checkpoint(source.clock.as_ref())?;
        Ok(size)
    }

    fn render(
        &self,
        source: &Self::Document,
        index: u64,
        width: u32,
        height: u32,
        options: &OpenOptions<'_>,
        deadline: &Deadline,
    ) -> Result<Raster> {
        let clock = Arc::clone(&options.clock);
        deadline.checkpoint(clock.as_ref())?;

        // THE CEILING BEFORE ANYTHING IS ALLOCATED, and before the engine is even asked for
        // the page. `check_pixels` is the one implementation both platforms call (ADR 0027).
        // The returned pixel count is not needed here: `check_pixels` has already refused a
        // product that would not fit this target, and the final length check lives in
        // `Raster::new`, where it cannot be skipped.
        crate::raster::check_pixels(width, height, &options.limits)?;

        // ONE PAGE LOAD AT A TIME IS STRUCTURAL -- the engine thread is serialised, so a second
        // `FPDF_LoadPage` cannot begin while one is running. WHETHER TO BEGIN THE NEXT AT ALL
        // is this check: the load cannot be checkpointed once entered, so the only lever on it
        // is the decision to enter.
        crate::estimate::before_page_load(
            source.memory_at_open,
            crate::rss::resident_bytes(),
            &options.limits,
        )?;

        let page_index = page_index(index, source.pages_at_open)?;
        let w = raster_dimension(width)?;
        let h = raster_dimension(height)?;
        let id = source.id;

        let limits = options.limits;
        let deadline_in = *deadline;
        let clock_in = Arc::clone(&clock);
        let rgba = thread::submit(move |registry| {
            let handle = registry.handle(id)?;

            // WHAT THE RENDER ACTUALLY COST, read either side of it. `max_pixels` bounds the
            // BUFFER WE HAND BACK and nothing else -- PDFium's rasteriser allocates its own
            // working set, and a content stream of three million stroked paths reaches 819 MB
            // while producing a 0.077 Mpx thumbnail (measured; ADR 0027's *What is bounded and
            // what is not*). Without this the render path was the one engine call in this crate
            // with no memory reading at all, not even the weak after-the-fact one.
            //
            // It DETECTS, it does not bound -- ADR 0007, and the whole of `Limits`' caveat. The
            // allocation has already happened by the time this fires. What it buys is that the
            // operation fails rather than returning a raster that cost more than the caller
            // allowed, and that a caller sees the overrun at all.
            let before = crate::rss::resident_bytes();

            // SAFETY: `handle` came from the registry, so the document is open. Engine thread.
            let page = unsafe { ffi::load_page(handle, page_index) };
            if page.is_null() {
                return Err(Error::Malformed("a page could not be loaded".to_owned()));
            }

            // Everything fallible happens inside this closure so that the page is closed on
            // every exit by the single call after it -- including the `?` paths. There is no
            // arrangement of these calls in which a `return` skips the close.
            let drawn = (|| -> Result<Vec<u8>> {
                // SAFETY: engine thread; the dimensions are plain integers. `max_pixels` was
                // checked above, so this is not the null-return-as-ceiling path the API makes
                // tempting.
                let bitmap = unsafe { ffi::create_bitmap(w, h, ffi::BITMAP_BGRA) };
                if bitmap.is_null() {
                    return Err(Error::Io(
                        "pdfium could not allocate a bitmap for the page".to_owned(),
                    ));
                }

                let filled = (|| -> Result<Vec<u8>> {
                    // SAFETY: `bitmap` is live and `w`/`h` are the extent it was created with.
                    // A fresh bitmap's contents are undefined, so this is what keeps
                    // uninitialised engine heap out of the picture the page displays.
                    unsafe { ffi::fill_bitmap(bitmap, w, h, ffi::OPAQUE_WHITE) };

                    render_progressively(
                        bitmap,
                        page,
                        w,
                        h,
                        &deadline_in,
                        clock_in.as_ref(),
                        &limits,
                    )?;

                    // SAFETY: `bitmap` is live; the pointer it returns is owned by the bitmap
                    // and is read before the destroy below.
                    let (buffer, stride) = unsafe { ffi::bitmap_buffer(bitmap) };
                    if buffer.is_null() {
                        return Err(Error::Internal(
                            "pdfium returned no buffer for a bitmap it allocated".to_owned(),
                        ));
                    }
                    let stride = usize::try_from(stride).map_err(|_| {
                        Error::Internal("pdfium reported a negative stride".to_owned())
                    })?;
                    let rows = usize::try_from(height).map_err(|_| {
                        Error::Internal("raster height does not fit in usize".to_owned())
                    })?;

                    // THE STRIDE IS BOUNDED BEFORE THE SLICE EXISTS, not inside `bgra_to_rgba`.
                    //
                    // `bgra_to_rgba` checks `stride >= row_bytes` and `src.len() >= span`, and
                    // both of those run AFTER `from_raw_parts` has already built a slice of
                    // `stride * rows` bytes -- so an engine reporting a stride wider than its
                    // own buffer is undefined behaviour that has happened before any check
                    // runs. Not reachable today: for `FPDFBitmap_BGRA` PDFium's pitch is
                    // exactly `width * 4`. But "not reachable today" is an assumption about
                    // PDFium, and the SAFETY comment below used to state it as established
                    // fact. Found by security review; these six lines are what make it one.
                    //
                    // The upper bound is the pitch PDFium documents, plus a row: a padded
                    // stride is legitimate and an unbounded one is not.
                    let row_bytes = usize::try_from(width)
                        .ok()
                        .and_then(|w| w.checked_mul(4))
                        .ok_or_else(|| {
                            Error::Internal("raster row does not fit in usize".to_owned())
                        })?;
                    if stride < row_bytes || stride > row_bytes.saturating_add(row_bytes) {
                        return Err(Error::Internal(
                            "pdfium reported a stride its own bitmap cannot have".to_owned(),
                        ));
                    }
                    let span = stride.checked_mul(rows).ok_or_else(|| {
                        Error::Internal("pdfium's bitmap does not fit in usize".to_owned())
                    })?;

                    // SAFETY: `buffer` is non-null and points at the bitmap's pixels. `stride`
                    // has just been bounded to `[width * 4, 2 * width * 4]`, so `span` is at
                    // most twice the bitmap's minimum size and cannot exceed an allocation
                    // PDFium made for a bitmap of these dimensions. The bitmap is alive for the
                    // whole of this borrow -- it is destroyed below, after `bgra_to_rgba` has
                    // returned an owned `Vec` -- nothing else on this thread writes to it, and
                    // `u8` has no alignment requirement.
                    let src = unsafe { core::slice::from_raw_parts(buffer, span) };
                    crate::raster::bgra_to_rgba(src, stride, width, height)
                })();

                // SAFETY: `bitmap` is live, this is its only destroy, and every read of its
                // buffer finished above -- `filled` owns its bytes.
                unsafe { ffi::destroy_bitmap(bitmap) };
                filled
            })();

            // SAFETY: `page` is live, this is its only close, and the document is still open.
            // No page handle escapes this closure or this thread.
            unsafe { ffi::close_page(page) };

            let rgba = drawn?;
            // AFTER the page and the bitmap are released, so what is measured is what the
            // render left behind rather than what it was holding mid-call.
            crate::estimate::check_measured_memory(before, crate::rss::resident_bytes(), &limits)?;
            Ok(rgba)
        })?;

        deadline.checkpoint(clock.as_ref())?;

        // THE LENGTH IS A DECISION, SO IT IS CHECKED RATHER THAN TRUSTED. Every other buffer
        // this crate hands back is as long as the engine said it was; this one is as long as
        // WE said it should be, which is why the check lives in `Raster::new` -- there is no
        // way to build one that skips it. ADR 0027 section 4.
        Raster::new(width, height, rgba)
    }
}

#[cfg(test)]
mod tests;
