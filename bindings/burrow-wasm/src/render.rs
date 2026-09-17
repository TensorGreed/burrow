//! The rendering artifact: PDFium, and nothing that produces a document.
//!
//! Compiled only under `--features render`, into `burrow_wasm_render_bg.wasm`, and loaded
//! only by `burrow-render-worker.js` — which a page fetches the first time it needs a
//! picture of a page. Nobody who merges two files ever downloads any of it.
//! [ADR 0026](../../../docs/adr/0026-how-rendering-loads-without-returning-to-the-old-payload.md).
//!
//! # What is here
//!
//! `page_count`, and the strip: [`render_begin`], [`RenderSession`] and [`RenderedPage`].
//!
//! **This paragraph said "`page_count` and nothing else" for one merge, and that was true of
//! exactly that merge.** ADR 0026 shipped the loading boundary with the smallest honest
//! payload behind it, because a boundary with nothing behind it is one no browser test can
//! drive — and an untested `blob:` worker with its own fail-closed policy guard is exactly the
//! thing that goes wrong quietly. #57's second piece is what filled it in, and the ceiling it
//! arrived with is
//! [ADR 0027](../../../docs/adr/0027-what-a-render-promises-and-what-it-refuses.md).
//!
//! `page_count` is still here and is still PDFium's, for the reason below.
//!
//! # Why this answers `page_count` when the base artifact already does
//!
//! Because they are different engines, and the two artifacts never meet. Spike 0004 moved
//! the *web's* `page_count` to qpdf because qpdf could answer it and PDFium cost 79.7% of
//! the payload; that reasoning is untouched, and the base artifact still answers it with
//! qpdf. This one answers it with PDFium because PDFium is the engine it has.
//!
//! **No conformance case compares the two.** The corpus asks one question per operation per
//! platform, and adding a second web answer for `page_count` would mean deciding what a
//! disagreement between two *web* engines means — which is a real question, with five known
//! divergences already recorded in spike 0004, and it is not this change's question.

use std::sync::Arc;

use burrow_core::engines::web::WebPdfium;
use burrow_core::engines::{DocumentEngine, OpenOptions};
use burrow_core::{Clock, Limits, Password};
use wasm_bindgen::prelude::wasm_bindgen;

use crate::{Reply, WebClock, WebLimits, bridge_pdfium};

thread_local! {
    /// One PDFium instance per worker, built on first use.
    ///
    /// The same rule the base artifact's `QPDF` follows, for the same reason (ADR 0006
    /// requirement 1): a `thread_local` in a wasm worker is one instance per worker, and a
    /// fresh engine per call would re-run process-global setup into a heap that never
    /// shrinks.
    ///
    /// PDFium's own global init is handled inside `WebPdfium`, not here. On native that is
    /// a `Once` around `FPDF_InitLibrary`, because two threads calling it concurrently
    /// abort the process with `SIGTRAP` (`core/CLAUDE.md`); on the web there is one thread,
    /// so the hazard is absent and the shape is kept anyway.
    static PDFIUM: WebPdfium = WebPdfium::new(Arc::new(bridge_pdfium::JsPdfium));
}

pub(crate) fn pdfium() -> WebPdfium {
    PDFIUM.with(Clone::clone)
}

/// Open a document with **PDFium** and report its page count.
///
/// `bytes` is taken by value: it arrives as a transferred `ArrayBuffer`, so there is no
/// copy from the page, and ownership passing to Rust is what the trait requires anyway.
///
/// `password` is **bytes, not a string**. A PDF password is a byte sequence and need not be
/// valid UTF-8; taking a `String` would corrupt some passwords and make others unusable.
///
/// Every ceiling in `limits` is applied by `WebPdfium::open` — the input size, the
/// length-based estimate, the structural pre-scan, the page count and the measured memory
/// check, in that order, which is the same order and the same code the native PDFium path
/// runs. The binding enforces none of it (ADR 0009 §2).
#[wasm_bindgen]
#[must_use]
pub fn page_count(bytes: Box<[u8]>, password: Option<Box<[u8]>>, limits: WebLimits) -> Reply {
    let limits = limits.to_core();
    let clock: Arc<dyn Clock> = Arc::new(WebClock);
    let password = password.map(|p| Password::new(&p));

    let mut options = OpenOptions::new(limits, clock);
    options.password = password.as_ref();

    match pdfium().open(bytes, &options) {
        Ok(document) => Reply::success(document.pages_at_open()),
        Err(error) => Reply::failure(&error),
    }
    .with_lifecycle(&limits)
}

/// One page, drawn, on its way to the worker.
///
/// **A type of its own rather than fields on [`Reply`], and the reason is the size budget.**
/// `Reply` is compiled into *both* artifacts — it is defined in `lib.rs`, which both features
/// share — so four more fields on it would be four more fields in `burrow_wasm_bg.wasm`, the
/// base payload that
/// [ADR 0026](../../../docs/adr/0026-how-rendering-loads-without-returning-to-the-old-payload.md)
/// exists to keep small. This type is `render`-only, so a visitor who merges two files carries
/// none of it.
///
/// It carries a whole `Reply` rather than duplicating the failure fields, so a render failure
/// is reported in exactly the shape every other failure is and `worker-protocol.js`'s refusal
/// vocabulary needs nothing new.
#[wasm_bindgen]
pub struct RenderedPage {
    reply: Reply,
    page: u64,
    width: u32,
    height: u32,
    /// RGBA, `width * height * 4` bytes. Moved out by [`RenderedPage::take_pixels`].
    pixels: Vec<u8>,
}

#[wasm_bindgen]
impl RenderedPage {
    /// The outcome: a success carrying pixels, or the typed failure that ended the strip.
    ///
    /// A copy, for the reason [`SplitSession::begin_reply`] is a copy: `wasm_bindgen` moves
    /// what it returns into JS, and the worker reads this before deciding whether to take the
    /// pixels. It carries no bytes, so the copy is small.
    #[must_use]
    pub fn reply(&self) -> Reply {
        self.reply.clone()
    }

    /// The **one-based** page number these pixels are of.
    ///
    /// Carried rather than left to the worker's own counting. A strip arrives as a sequence of
    /// messages and is laid out by position; matching them up by arrival order means a dropped
    /// message shows the wrong page under the right label — which is exactly the failure a
    /// picture of a page exists to prevent.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn page(&self) -> u64 {
        self.page
    }

    /// The raster's width in pixels. **Not the box**: a page is fitted inside it.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// The raster's height in pixels.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// How many bytes [`RenderedPage::take_pixels`] will hand over.
    ///
    /// Read before taking, for the reason `Reply::output_length` is: both "no pixels" and
    /// "already taken" come back as an empty array, and only the length can tell them apart.
    ///
    /// **`js_name` IS LOAD-BEARING AND WAS MISSING, AND THE FAILURE WAS SILENT.** Without it
    /// wasm-bindgen exposes `pixel_length`, the worker's `drawn.pixelLength` is `undefined`,
    /// `undefined > 0` is `false`, and the strip posts a zero-byte buffer for every page while
    /// reporting success — a blank strip indistinguishable from a working one, with no
    /// exception to catch. `Reply` gets this right (`js_name = outputLength`), which is why
    /// `worker-protocol.js` works and this did not. Found by security review;
    /// `tools/check-wasm-binding-names.py` is the gate that would have caught it.
    #[wasm_bindgen(getter, js_name = pixelLength)]
    #[must_use]
    pub fn pixel_length(&self) -> usize {
        self.pixels.len()
    }

    /// **Move** the pixels out. A second call yields nothing.
    ///
    /// A getter would copy them, and this is the largest thing the render boundary carries —
    /// 300 KB for an ordinary thumbnail, 16 MB at the ceiling ADR 0027 chose.
    ///
    /// `js_name` for the reason above: `takeOutput` is what `Reply` calls its counterpart, and
    /// the worker is written against that spelling.
    #[wasm_bindgen(js_name = takePixels)]
    #[must_use]
    pub fn take_pixels(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.pixels)
    }
}

/// A strip in progress: the source held open, pages drawn one at a time.
///
/// **ADR 0023's shape, for ADR 0027 §2's reason.** The worker pulls one page, posts it, and
/// drops it, so the engine heap holds at most one bitmap however long the strip is — and the
/// main thread never receives the whole set at once.
///
/// Pull, not push, and the session owns its password: both for the reasons
/// [`SplitSession`](crate::documents::SplitSession) gives.
///
/// # A failure does NOT invalidate the pages already delivered
///
/// The opposite of `split`'s rule, and the two sessions look alike enough that it is worth
/// saying twice. A split is a partition, so a subset of the parts is not a partition of
/// anything. A strip is a set of independent pictures: the ones that arrived are still true
/// pictures of their pages, and a caller may keep them and report the gap.
#[wasm_bindgen]
pub struct RenderSession {
    /// `None` when `begin` failed, or once every page has been produced.
    inner: Option<burrow_core::ops::Render<WebPdfium>>,
    /// Kept alive for the life of the session; `OpenOptions` borrows it per call.
    password: Option<Password>,
    limits: Limits,
    /// How many pages this strip produces. Known before the first one is.
    pages: u32,
    /// The failure that ended this strip, if one did.
    ///
    /// Held so that a call after a failure is that failure again rather than the empty success
    /// that means "exhausted" — the distinction `SplitSession` records, and it matters here for
    /// a different reason: a worker that read a failure as exhaustion would stop the strip and
    /// report it complete.
    failed: Option<Reply>,
    /// The failure `begin` reported, if it failed. The session IS the reply for that phase.
    began: Reply,
}

#[wasm_bindgen]
impl RenderSession {
    /// Whether the strip could be started at all.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn ok(&self) -> bool {
        self.began.ok()
    }

    /// How many pages this strip will produce. Zero when `begin` failed.
    #[wasm_bindgen(getter)]
    #[must_use]
    pub fn pages(&self) -> u32 {
        self.pages
    }

    /// The outcome of starting the strip: the failure, or an empty success.
    #[must_use]
    pub fn begin_reply(&self) -> Reply {
        self.began.clone()
    }

    /// Draw and return the next page.
    ///
    /// Returns a page whose reply is an empty success with `pixel_length == 0` once every page
    /// has been produced; the worker stops on the count rather than on that, which is why
    /// `pages` is known up front.
    #[must_use]
    pub fn next_page(&mut self) -> RenderedPage {
        let clock: Arc<dyn Clock> = Arc::new(WebClock);
        let mut options = OpenOptions::new(self.limits, clock);
        options.password = self.password.as_ref();

        if let Some(failed) = &self.failed {
            return RenderedPage {
                reply: failed.clone(),
                page: 0,
                width: 0,
                height: 0,
                pixels: Vec::new(),
            };
        }

        let exhausted = |limits: &Limits| RenderedPage {
            reply: Reply::success(0).with_lifecycle(limits),
            page: 0,
            width: 0,
            height: 0,
            pixels: Vec::new(),
        };

        let Some(session) = self.inner.as_mut() else {
            return exhausted(&self.limits);
        };

        match session.next(&pdfium(), &options) {
            Some(Ok(rendered)) => RenderedPage {
                // ZERO, not `rendered.page`. `Reply::pages` means "the page count of a
                // document" for every other operation and is drained to `flat.pages` by the
                // SHARED `drainReply`, so a strip of pages 3, 1, 5 would have produced replies
                // claiming three different page counts. The page number travels in
                // `RenderedPage::page`, where it means one thing. Found by code review.
                reply: Reply::success(0).with_lifecycle(&self.limits),
                page: rendered.page,
                width: rendered.raster.width,
                height: rendered.raster.height,
                pixels: rendered.raster.rgba,
            },
            Some(Err(error)) => {
                // THE SOURCE GOES ON FAILURE. A parsed document held open for a caller that has
                // already been told the strip ended is the worst of both, and this heap has a
                // fixed ceiling.
                self.inner = None;
                let reply = Reply::failure(&error).with_lifecycle(&self.limits);
                self.failed = Some(reply.clone());
                RenderedPage {
                    reply,
                    page: 0,
                    width: 0,
                    height: 0,
                    pixels: Vec::new(),
                }
            }
            None => {
                // EXHAUSTED. Dropped here rather than when the session is, because the worker
                // posts the last page and then does other things.
                self.inner = None;
                exhausted(&self.limits)
            }
        }
    }
}

/// Begin a strip, returning a session the caller pulls pages from.
///
/// `pages` is **one-based**. `box_width` x `box_height` is the box each page is fitted inside,
/// keeping its proportions — so what comes back is usually smaller than the box in one
/// dimension, and every [`RenderedPage`] carries its own size for that reason.
///
/// Everything that can fail for a reason the caller could have avoided happens here: the page
/// numbers are validated against the real count, and `max_pixels` is checked against the box
/// **before the document is opened**, so a caller asking for something impossible is told so
/// without an untrusted file having been parsed on their behalf.
#[wasm_bindgen]
#[must_use]
pub fn render_begin(
    bytes: Box<[u8]>,
    pages: &[u32],
    box_width: u32,
    box_height: u32,
    password: Option<Box<[u8]>>,
    limits: WebLimits,
) -> RenderSession {
    let limits = limits.to_core();
    let clock: Arc<dyn Clock> = Arc::new(WebClock);
    let password = password.map(|p| Password::new(&p));

    let mut options = OpenOptions::new(limits, Arc::clone(&clock));
    options.password = password.as_ref();

    let wanted: Vec<u64> = pages.iter().map(|n| u64::from(*n)).collect();

    match burrow_core::ops::render_begin(
        &pdfium(),
        bytes,
        &wanted,
        burrow_core::ops::Fit::box_of(box_width, box_height),
        &options,
    ) {
        Ok(session) => {
            let pages = u32::try_from(session.pages()).unwrap_or(u32::MAX);
            RenderSession {
                inner: Some(session),
                password,
                limits,
                pages,
                failed: None,
                began: Reply::success(0).with_lifecycle(&limits),
            }
        }
        Err(error) => {
            // `failed` AND `began`, not just `began`. With `failed` left `None` a puller that
            // did not check `ok` first fell through to the exhausted arm and read a REFUSED
            // request as a completed strip of zero pages -- which is the exact confusion the
            // `failed` field's own comment says it exists to prevent. `render-main.js` checks
            // `ok` and so never reached it; the conformance harness would have. Found by
            // security review.
            let reply = Reply::failure(&error).with_lifecycle(&limits);
            RenderSession {
                inner: None,
                password,
                limits,
                pages: 0,
                failed: Some(reply.clone()),
                began: reply,
            }
        }
    }
}
