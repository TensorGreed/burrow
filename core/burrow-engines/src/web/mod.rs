//! The web implementations of [`DocumentEngine`](crate::DocumentEngine) and
//! [`StructureEngine`](crate::StructureEngine).
//!
//! Under [ADR 0006] option 1, PDFium and qpdf run as separate Emscripten modules and Rust
//! reaches them through JavaScript. Everything in this module is the Rust half of that:
//! the orchestration, which is shared with the native path, and the [`bridge`] traits,
//! which are the only part JavaScript implements.
//!
//! # Compiled on every target, deliberately
//!
//! There is no `cfg(target_arch = "wasm32")` here. That is what lets `cargo test` on an
//! ordinary Linux host drive the entire web orchestration against a fake bridge — the
//! limit ordering, the deadline, the error mapping, the close-before-free ordering — with
//! no browser, no Emscripten and no engines.
//!
//! It matters because of what the web path is for. ROADMAP item 12 requires the native and
//! web implementations to produce **identical typed outcomes**, and at M2 a divergence
//! between them is a redaction bug rather than a test failure. A `cfg(wasm32)` module would
//! be code CI never executes, and the claim would rest on Playwright alone.
//!
//! # Where the JavaScript is
//!
//! `bindings/burrow-wasm` implements [`bridge::PdfiumBridge`] and [`bridge::QpdfBridge`]
//! with `#[wasm_bindgen]` externs. The dependency points that way round on purpose:
//! `burrow-engines` is already in the `wasm32-unknown-unknown` dependency graph that
//! `cargo deny` checks, and pulling wasm-bindgen and its proc-macro chain in here would
//! drag all of it into the licence and ban review for no benefit.
//!
//! [ADR 0006]: https://github.com/TensorGreed/burrow/blob/main/docs/adr/0006-wasm-linking-strategy.md

pub mod bridge;
mod pdfium;
mod qpdf;
pub mod recycle;

pub use self::bridge::{LoadOutcome, PdfiumBridge, PdfiumPtr, QpdfBridge, QpdfPtr};
pub use self::pdfium::{WebDocument, WebPdfium};
pub use self::qpdf::WebQpdf;
pub use self::recycle::{MIN_CONVERGING_MEMORY_BYTES, should_recycle, threshold_bytes};

#[cfg(test)]
mod fake;
#[cfg(test)]
mod tests;
