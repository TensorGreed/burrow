//! The web implementation of [`DocumentEngine`], over a PDFium Emscripten module.
//!
//! The second implementation of the trait, and the reason PR 2 shaped it the way it did:
//! handle-based, input taken by value, and every bridge call returning its value together
//! with the engine error code. Nothing can be borrowed across a JS boundary, so a trait
//! that handed out references could not have been satisfied here at all.
//!
//! # What is identical to the native path, and why that is the point
//!
//! The order of checks, the arithmetic behind every limit, the error mapping, and the
//! deadline are **the same code**, not a parallel implementation:
//!
//! | Step | Shared with native |
//! |---|---|
//! | `max_input_bytes` | [`Limits::check`] |
//! | size-based memory estimate | [`crate::estimate::check_open_memory`] |
//! | structural pre-scan | [`crate::prescan::check`] |
//! | password preparation | [`crate::password::nul_terminated`] |
//! | deadline | [`Deadline`] |
//! | load failure → typed error | [`crate::codes::pdfium::map_failure`] |
//! | `max_pages` | [`Limits::check`] |
//! | measured memory | [`crate::estimate::check_measured_memory`] |
//!
//! Only the engine calls differ, and those are [`PdfiumBridge`]. ROADMAP item 12's
//! differential harness asserts the two paths reach identical typed outcomes; this table is
//! why that is a reasonable thing to expect rather than a coincidence to be maintained.
//!
//! # The one deliberate difference
//!
//! The memory counter. Native reads the process resident set; here it is the module's
//! `HEAPU8.byteLength`. WASM memory **only grows** — it is never returned to the host — so
//! an absolute reading would rise monotonically across a worker's life and eventually
//! reject everything. Only the delta across a single open is used, which is exactly what
//! [`crate::estimate::check_measured_memory`] already takes. Recycling a worker whose heap
//! has grown too far is a separate concern and belongs to the page, not here.

use std::sync::Arc;

use burrow_types::{Clock, Deadline, Error, Limits, Result, Stage};

use super::bridge::{PdfiumBridge, PdfiumPtr};
use crate::{DocumentEngine, OpenOptions, PageRenderer, Raster};

/// The web PDFium engine.
///
/// Holds the bridge to one Emscripten module. ADR 0006 requirement 1 is that there is
/// exactly one such module per worker and that its init *promise* is memoised — that is the
/// worker's job, and this type simply never creates a second one.
#[derive(Clone)]
pub struct WebPdfium {
    bridge: Arc<dyn PdfiumBridge>,
}

impl WebPdfium {
    /// An engine driving `bridge`.
    #[must_use]
    pub fn new(bridge: Arc<dyn PdfiumBridge>) -> Self {
        Self { bridge }
    }

    /// The module's current heap size, for the page's recycling decision.
    ///
    /// An **absolute** reading, unlike the deltas `crate::estimate::check_measured_memory`
    /// consumes, and that is the point: what the page needs to know is how far this worker
    /// has grown over its whole life, not what one operation cost. WASM memory never shrinks,
    /// so the two questions have different answers and only this one decides a respawn.
    ///
    /// Feed it to [`super::should_recycle`] rather than comparing it against anything here.
    #[must_use]
    pub fn heap_bytes(&self) -> u64 {
        self.bridge.heap_bytes()
    }
}

impl core::fmt::Debug for WebPdfium {
    /// Hand-written: `Arc<dyn PdfiumBridge>` has no `Debug`, and there is nothing about the
    /// bridge worth rendering anyway.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("WebPdfium").finish_non_exhaustive()
    }
}

/// An open document in a PDFium Emscripten module.
///
/// `Send + Sync` with no `unsafe impl`, for a stronger reason than the native path's: every
/// field is a plain number or an `Arc`, and [`PdfiumPtr`] is *not a Rust pointer*. It names
/// a location in another module's address space and cannot be dereferenced from Rust at
/// all.
///
/// Dropping this closes the document and frees its input buffer, in that order.
pub struct WebDocument {
    doc: PdfiumPtr,
    data: PdfiumPtr,
    /// How many bytes `data` holds, so [`Drop`] can wipe it. See
    /// [`PdfiumBridge::close_document`].
    data_len: u32,
    deadline: Deadline,
    clock: Arc<dyn Clock>,
    pages_at_open: u64,
    bridge: Arc<dyn PdfiumBridge>,
}

impl WebDocument {
    /// The page count read at open, before any page was touched.
    ///
    /// The number `max_pages` was actually checked against. Mirrors
    /// `PdfiumDocument::pages_at_open` so the differential harness can compare both this
    /// and [`page_count`](DocumentEngine::page_count) across the two implementations.
    #[must_use]
    pub fn pages_at_open(&self) -> u64 {
        self.pages_at_open
    }
}

impl core::fmt::Debug for WebDocument {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("WebDocument")
            .field("deadline", &self.deadline)
            .field("pages_at_open", &self.pages_at_open)
            .finish_non_exhaustive()
    }
}

impl Drop for WebDocument {
    /// Close, then wipe and free — never the other way round.
    ///
    /// PDFium reads from the input buffer for as long as the document is open
    /// (`fpdfview.h:451`), so freeing first is a use-after-free inside the engine heap. The
    /// ordering is not enforced here by care: [`PdfiumBridge::close_document`] is a single
    /// call that does both, so there is no call site at which the order is expressible.
    fn drop(&mut self) {
        self.bridge
            .close_document(self.doc, self.data, self.data_len);
    }
}

impl DocumentEngine for WebPdfium {
    type Document = WebDocument;

    fn name(&self) -> &'static str {
        "pdfium-wasm"
    }

    fn open(&self, bytes: Box<[u8]>, options: &OpenOptions<'_>) -> Result<Self::Document> {
        let limits = options.limits;

        // Empty input can never be a document. Refusing it here also keeps a zero-length
        // allocation out of the engine heap, where a null return would be ambiguous
        // between "empty" and "out of memory".
        if bytes.is_empty() {
            return Err(Error::Malformed("input is empty".to_owned()));
        }

        // 1. Input size, before anything is copied across the bridge. Note this bounds the
        //    *parse*, not the copy: the bytes are already in Rust's heap by now. A binding
        //    that reads a File must check its length before reading it in.
        // EVERY CEILING THAT APPLIES BEFORE THE ENGINE, in one call: the byte count,
        // the size estimate, the structural pre-scan. Shared rather than spelled out
        // here, so a path that pre-scans without estimating is not writable --- see
        // `crate::estimate::before_open`, and #26 for what the drift cost last time.
        let input_len = crate::estimate::before_open(&bytes, &limits)?;

        // 4. The password copy, before the deadline starts: our work, not the engine's.
        let password = crate::password::nul_terminated(options.password, "pdfium")?;

        // 5. The budget for everything that follows, including later calls on the handle.
        let clock = Arc::clone(&options.clock);
        let deadline = Deadline::start(clock.as_ref(), &limits);
        deadline.checkpoint(clock.as_ref())?;

        // The length PDFium is told about. Narrowed once, here, so the engine and the
        // limit check cannot disagree about how big the buffer is.
        let engine_len = u32::try_from(bytes.len()).map_err(|_| Error::LimitExceeded {
            limit: "max_input_bytes",
            // The same stage as the `max_input_bytes` check above, because it is the same
            // question asked against a second ceiling: this one is the engine's, not the
            // caller's. A caller who sets `max_input_bytes` above 4 GiB gets told here.
            stage: Stage::InputSize,
            requested: input_len,
            allowed: u64::from(u32::MAX),
        })?;

        // a. The input into the engine heap. From here there is something to free.
        let data = self.bridge.copy_in(&bytes);
        if data.is_null() {
            return Err(Error::Io(
                "the pdfium module could not allocate for the input".to_owned(),
            ));
        }

        // b. The password into the engine heap, wiped again as soon as the load returns.
        //    Rust's `Zeroizing` cannot reach this copy: it is outside Rust's allocator.
        //
        //    The length is narrowed HERE, before anything is copied. It was narrowed at the
        //    wipe instead, so the failure path returned with the password already in the
        //    engine heap, unwiped, and the input buffer unfreed -- leaving the copy in the
        //    module's free list for the life of the worker, which is the exact harm the
        //    wipe exists to prevent. Unreachable on wasm32, where `usize` is 32 bits, but a
        //    cleanup path that runs only on success is the wrong shape whatever guards it.
        let password_len = password.as_ref().map_or(0, |p| p.len());
        let Ok(password_len_u32) = u32::try_from(password_len) else {
            self.bridge.abandon_input(data, engine_len);
            return Err(Error::InvalidArgument(
                "password is too large for the engine's address space".to_owned(),
            ));
        };
        let password_ptr = match password.as_ref() {
            None => PdfiumPtr::NULL,
            Some(p) => {
                let ptr = self.bridge.copy_in(p);
                if ptr.is_null() {
                    self.bridge.abandon_input(data, engine_len);
                    return Err(Error::Io(
                        "the pdfium module could not allocate for the password".to_owned(),
                    ));
                }
                ptr
            }
        };

        let before = self.bridge.heap_bytes();
        let outcome = self
            .bridge
            .load_mem_document64(data, engine_len, password_ptr);

        if !password_ptr.is_null() {
            // `password_len_u32` is the Rust-side length of the same copy, narrowed BEFORE
            // the copy was made, so the wipe covers exactly the bytes written -- no over- or
            // under-run, and no way to reach here with a length that does not fit.
            //
            // Narrowing it here instead meant the failure path returned with the password
            // already in the engine heap, unwiped, and the input buffer unfreed -- leaving
            // the copy in the module's free list for the life of the worker, which is the
            // exact harm the wipe exists to prevent.
            self.bridge.wipe_and_free(password_ptr, password_len_u32);
        }
        drop(password);

        if outcome.handle.is_null() {
            // Failure is established by the null handle; the code only classifies it, and
            // it came back from the same bridge call, so it is this load's code.
            self.bridge.abandon_input(data, engine_len);
            return Err(crate::codes::pdfium::map_failure(outcome.code));
        }

        // c. The document is live. Build the guard immediately, so every exit below --
        //    including one taken by `?` -- closes and frees exactly once. `pages_at_open`
        //    is filled in once the count is known; it is not part of the guard's job.
        let mut document = WebDocument {
            doc: outcome.handle,
            data,
            data_len: engine_len,
            deadline,
            clock,
            pages_at_open: 0,
            bridge: Arc::clone(&self.bridge),
        };

        // d. The page count, immediately, before any page is touched.
        let count = self.bridge.get_page_count(document.doc);
        if count < 0 {
            // No error code is fetched, and the bridge offers no way to fetch one. See
            // `PdfiumBridge::get_page_count`.
            return Err(crate::codes::pdfium::unreadable_page_tree());
        }
        let pages = u64::try_from(count).map_err(|_| {
            Error::Internal("pdfium reported a page count that is not a count".to_owned())
        })?;
        if pages == 0 {
            return Err(Error::Malformed("document has no pages".to_owned()));
        }

        // e. The page limit, on a number the engine produced.
        Limits::check(Stage::PageCount, "max_pages", pages, limits.max_pages)?;

        // f. What the open actually cost. Step 2's estimate is blind to anything the file
        //    *declares*; this is the reading that is not.
        crate::estimate::check_measured_memory(
            Some(before),
            Some(self.bridge.heap_bytes()),
            &limits,
        )?;

        document.deadline.checkpoint(document.clock.as_ref())?;

        document.pages_at_open = pages;
        Ok(document)
    }

    fn page_count(&self, doc: &Self::Document) -> Result<u64> {
        // Against the clock captured at open, so the budget covers the whole operation and
        // cannot be reset by handing in a different one.
        doc.deadline.checkpoint(doc.clock.as_ref())?;

        let count = self.bridge.get_page_count(doc.doc);
        if count < 0 {
            return Err(crate::codes::pdfium::unreadable_page_tree());
        }
        let pages = u64::try_from(count).map_err(|_| {
            Error::Internal("pdfium reported a page count that is not a count".to_owned())
        })?;

        doc.deadline.checkpoint(doc.clock.as_ref())?;
        Ok(pages)
    }
}

/// `FPDFBitmap_Create`'s `alpha` argument selecting `FPDFBitmap_BGRA`.
///
/// `1`, not `0`, for the reason the native side gives: `0` is `FPDFBitmap_BGRx`, whose fourth
/// byte is padding PDFium does not define — and that byte becomes the alpha channel of the
/// picture a person looks at.
const BITMAP_BGRA: i32 = 1;

/// Opaque white for `FPDFBitmap_FillRect`, as `0xAARRGGBB`.
const OPAQUE_WHITE: u32 = 0xFFFF_FFFF;

/// `FPDF_RenderPageBitmap`'s flags: none. **Not** `FPDF_ANNOT`; see the native constant.
const RENDER_FLAGS: i32 = 0;

/// `FPDF_RenderPageBitmap`'s `rotate`: always zero.
///
/// PDFium applies the page's own `/Rotate`, and a second rotation here would silently double
/// it. A constant rather than a parameter so no call site can pass anything else.
const NO_EXTRA_ROTATION: i32 = 0;

/// Narrow a zero-based page index for the engine, having established it is in range.
fn page_index(index: u64, total: u64) -> Result<i32> {
    if index >= total {
        return Err(Error::InvalidArgument(format!(
            "page {} of a {total}-page document",
            index.saturating_add(1)
        )));
    }
    i32::try_from(index)
        .map_err(|_| Error::Internal("page index does not fit in an engine index".to_owned()))
}

/// Narrow a raster dimension for the engine.
///
/// Reached only after `crate::raster::check_pixels`, so a failure means the caller set
/// `max_pixels` above what the engine's own API can express — a refusal of the request.
fn raster_dimension(value: u32) -> Result<i32> {
    i32::try_from(value).map_err(|_| {
        Error::InvalidArgument("a raster dimension is larger than the engine accepts".to_owned())
    })
}

impl PageRenderer for WebPdfium {
    fn page_size(&self, source: &Self::Document, index: u64) -> Result<(f32, f32)> {
        source.deadline.checkpoint(source.clock.as_ref())?;

        let at = page_index(index, source.pages_at_open)?;
        let page = self.bridge.load_page(source.doc, at);
        if page.is_null() {
            return Err(Error::Malformed("a page could not be loaded".to_owned()));
        }
        let width = self.bridge.page_width(page);
        let height = self.bridge.page_height(page);
        // No `?` between the load and the close: on this path there is no `Drop` on the other
        // side of the bridge, so an early return is a page handle left open for the life of
        // the worker. `FakeHeap::assert_empty` is what makes that a failing test rather than
        // a slow leak.
        self.bridge.close_page(page);

        source.deadline.checkpoint(source.clock.as_ref())?;
        Ok((width, height))
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

        // THE CEILING BEFORE ANYTHING IS ALLOCATED. The same `check_pixels` the native path
        // calls, not a mirror of it (ADR 0027).
        // The returned pixel count is not needed here: `check_pixels` has already refused a
        // product that would not fit this target, and the final length check lives in
        // `Raster::new`, where it cannot be skipped.
        crate::raster::check_pixels(width, height, &options.limits)?;

        let at = page_index(index, source.pages_at_open)?;
        let w = raster_dimension(width)?;
        let h = raster_dimension(height)?;

        // WHAT THE RENDER ACTUALLY COST, read either side of it -- the module's heap here,
        // the process resident set on native, which is ADR 0007's one deliberate difference.
        // `max_pixels` bounds the buffer handed back and nothing else: PDFium's rasteriser
        // allocates its own working set, and on this platform a large enough content stream
        // reaches the module's fixed 2 GiB maximum and aborts. See ADR 0027's *What is bounded
        // and what is not*.
        let before = self.bridge.heap_bytes();

        let page = self.bridge.load_page(source.doc, at);
        if page.is_null() {
            return Err(Error::Malformed("a page could not be loaded".to_owned()));
        }

        // Everything fallible sits inside this closure so that the close after it runs on
        // every exit, including the `?` paths. There is no `Drop` across the bridge to fall
        // back on, which is why the shape is explicit rather than idiomatic.
        let drawn = (|| -> Result<Vec<u8>> {
            let bitmap = self.bridge.bitmap_create(w, h, BITMAP_BGRA);
            if bitmap.is_null() {
                // An allocation failure, NOT a ceiling: `max_pixels` was checked above.
                return Err(Error::Io(
                    "the pdfium module could not allocate a bitmap for the page".to_owned(),
                ));
            }

            let filled = (|| -> Result<Vec<u8>> {
                self.bridge
                    .bitmap_fill_rect(bitmap, 0, 0, w, h, OPAQUE_WHITE);
                self.bridge.render_page_bitmap(
                    bitmap,
                    page,
                    0,
                    0,
                    w,
                    h,
                    NO_EXTRA_ROTATION,
                    RENDER_FLAGS,
                );

                let buffer = self.bridge.bitmap_buffer(bitmap);
                if buffer.is_null() {
                    return Err(Error::Internal(
                        "pdfium returned no buffer for a bitmap it allocated".to_owned(),
                    ));
                }
                let stride = self.bridge.bitmap_stride(bitmap);
                let stride = usize::try_from(stride)
                    .map_err(|_| Error::Internal("pdfium reported a negative stride".to_owned()))?;

                // The length is computed from what the ENGINE reported -- its stride, and the
                // height we asked for -- and never from `width * 4`. `bgra_to_rgba` refuses a
                // stride narrower than a row, so an engine contradicting itself is an error
                // rather than a read past the end of its own bitmap.
                let rows = usize::try_from(height).map_err(|_| {
                    Error::Internal("raster height does not fit in usize".to_owned())
                })?;
                let span = stride.checked_mul(rows).ok_or_else(|| {
                    Error::Internal("the bitmap does not fit in usize".to_owned())
                })?;
                let span = u32::try_from(span).map_err(|_| {
                    Error::Internal(
                        "the bitmap does not fit in the engine's address space".to_owned(),
                    )
                })?;

                let bytes = self.bridge.copy_out(buffer, span);
                crate::raster::bgra_to_rgba(&bytes, stride, width, height)
            })();

            self.bridge.bitmap_destroy(bitmap);
            filled
        })();

        self.bridge.close_page(page);
        let rgba = drawn?;

        // AFTER the page and the bitmap are released. It DETECTS, it does not bound (ADR 0007):
        // the allocation has already happened. What it buys is that the operation fails rather
        // than returning a raster that cost more than the caller allowed.
        crate::estimate::check_measured_memory(
            Some(before),
            Some(self.bridge.heap_bytes()),
            &options.limits,
        )?;

        deadline.checkpoint(clock.as_ref())?;

        // THE LENGTH IS A DECISION, SO IT IS CHECKED RATHER THAN TRUSTED. Every other buffer
        // this crate hands back is as long as the engine said it was; this one is as long as
        // WE said it should be, which is why the check lives in `Raster::new` -- there is no
        // way to build one that skips it. ADR 0027 section 4.
        Raster::new(width, height, rgba)
    }
}
