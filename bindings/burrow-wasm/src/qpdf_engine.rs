//! The one qpdf engine a worker holds, shared by the two artifacts that link qpdf.
//!
//! Moved out of `documents.rs` when redaction got its own artifact (#137, ADR 0029's
//! 2026-09-21 amendment): `documents` and `redact` both drive qpdf through the same bridge,
//! and neither artifact may carry the other's operations. The engine is what they share, so it
//! lives here and nothing else does.

use std::sync::Arc;

use burrow_core::engines::web::WebQpdf;

use crate::bridge_qpdf;

thread_local! {
    /// One engine per worker, built on first use.
    ///
    /// **Not constructed per operation.** `WebQpdf` holds the process-global setup — qpdf's
    /// resource limits and the shared discarding logger — behind a `OnceLock`, so a fresh
    /// engine per call would apply the limits repeatedly and, worse, create a logger per
    /// call into a heap that never shrinks.
    ///
    /// `thread_local` rather than a `static`: a wasm worker is one thread, so this is one
    /// instance per worker, which is exactly the lifetime ADR 0006 requirement 1 describes.
    static QPDF: WebQpdf = WebQpdf::new(Arc::new(bridge_qpdf::JsQpdf));
}

pub(crate) fn qpdf() -> WebQpdf {
    QPDF.with(Clone::clone)
}
