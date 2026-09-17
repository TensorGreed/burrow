//! The rendering artifact: PDFium, and nothing that produces a document.
//!
//! Compiled only under `--features render`, into `burrow_wasm_render_bg.wasm`, and loaded
//! only by `burrow-render-worker.js` — which a page fetches the first time it needs a
//! picture of a page. Nobody who merges two files ever downloads any of it.
//! [ADR 0026](../../../docs/adr/0026-how-rendering-loads-without-returning-to-the-old-payload.md).
//!
//! # What is here now, and what is not
//!
//! **`page_count` and nothing else.** That is deliberate and it is not a placeholder:
//! rendering a page means opening the document first, so `open` + `page_count` is the first
//! half of the capability rather than a stand-in for it. It is what makes this artifact
//! *exercisable* end to end — through the real CSP guard, the real integrity fetch, the real
//! blob worker and the real lifecycle — before there is a bitmap to argue about.
//!
//! A boundary with nothing behind it is a boundary no browser test can drive, and an
//! untested `blob:` worker with its own fail-closed policy guard is exactly the thing that
//! goes wrong quietly. So the loading change ships with the smallest honest payload rather
//! than with none.
//!
//! **`PageRenderer`, the bitmap reply and the pixel ceiling are #57's second piece.** They
//! are not deferred vaguely: the ceiling is a design decision with its own tests
//! (`max_pixels` finally enforced, one render in flight, a windowed strip), and ADR 0020
//! records why that must not be discovered under UI pressure.
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
use burrow_core::{Clock, Password};
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
