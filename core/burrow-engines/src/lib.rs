//! Trait seams over the native engines burrow builds on.
//!
//! `burrow-ops` is written against the traits here, never against PDFium's or qpdf's C
//! API, which keeps engines swappable and keeps `unsafe` confined to this crate. Engine
//! selection and the prebuilt-versus-source tradeoff are recorded in
//! `docs/adr/0004-native-engines.md`.
//!
//! # Two implementations, one trait
//!
//! [`DocumentEngine`] has to be satisfiable twice: once by `pdfium::Pdfium` here (not
//! linked from this sentence because that module only exists when the engines are, so a
//! link would break the docs built without them), and
//! once — in M1 PR 4 — by an implementation that drives a *separate* Emscripten PDFium
//! module across a JS bridge, per [ADR 0006]. Nothing can be borrowed across that
//! boundary, so the trait is handle-based and takes its input by value. See the
//! [`DocumentEngine`] docs for what that costs and why the alternative does not work.
//!
//! [ADR 0006]: ../../../docs/adr/0006-wasm-linking-strategy.md

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        // Tests do arithmetic on known-good numbers. The lint exists for
        // attacker-controlled sizes in library code, which is where it stays denied.
        clippy::integer_division
    )
)]

use std::sync::Arc;

use burrow_types::{Clock, Limits, Password, Result};

// The PDFium implementation. Gated on the libraries actually being linked: `build.rs`
// sets `burrow_native_engines` only when the `native-engines` feature is on, the target
// is Linux, and every vendored library has passed its checksum. The trait above stays
// ungated so `cargo check --target aarch64-apple-ios --all-features` keeps working.
#[cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
pub mod pdfium;

// Proof that the vendored native engines link and run: PDFium's provenance, and qpdf's
// version. Test-only -- nothing in the library needs it, and keeping it out of the
// non-test build keeps a raw `FPDF_GetLastError` value from being reachable through a
// public item, which core/CLAUDE.md forbids.
//
// It survives PR 2 (its own comments once predicted it would not) because the questions
// it answers are ones no other test asks: whether the loader mapped the *pinned*
// libpdfium, and whether the linked qpdf is the pinned version. It no longer declares
// PDFium's FFI itself -- it borrows `pdfium::ffi` -- and it no longer initialises the
// library, which is now the engine thread's job alone.
#[cfg(all(
    test,
    feature = "native-engines",
    burrow_native_engines,
    target_os = "linux"
))]
mod link_check;

/// Everything an engine needs to open one document safely.
///
/// Grouped into a struct rather than passed as four arguments so that adding a knob later
/// does not break every implementation and every call site.
#[non_exhaustive]
pub struct OpenOptions<'a> {
    /// The ceilings this open must enforce. Not advisory.
    pub limits: Limits,
    /// The clock the operation's deadline is measured against.
    ///
    /// Injected rather than read from `Instant::now()`, which panics on
    /// `wasm32-unknown-unknown`. See `docs/adr/0007-limit-enforcement-per-platform.md`.
    ///
    /// Shared rather than borrowed, because the engine **keeps** it: a document's budget
    /// has to be measured against the clock it started on. See
    /// [`page_count`](DocumentEngine::page_count).
    pub clock: Arc<dyn Clock>,
    /// The password for an encrypted document, if one is known.
    ///
    /// `None` and `Some(empty)` are different: PDF's standard security handler treats the
    /// empty string as a real user password.
    pub password: Option<&'a Password>,
}

impl<'a> OpenOptions<'a> {
    /// Options with the given limits and clock, and no password.
    ///
    /// Set [`password`](OpenOptions::password) directly for an encrypted document.
    #[must_use]
    pub fn new(limits: Limits, clock: Arc<dyn Clock>) -> Self {
        Self {
            limits,
            clock,
            password: None,
        }
    }
}

impl core::fmt::Debug for OpenOptions<'_> {
    /// Hand-written because a `&dyn Clock` has no `Debug`, and because this is a struct
    /// that holds a password: the derive would be one field away from rendering a secret
    /// if [`Password`]'s own `Debug` ever stopped redacting. Only whether a password is
    /// present is shown, never the password.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("OpenOptions")
            .field("limits", &self.limits)
            .field("has_password", &self.password.is_some())
            .finish_non_exhaustive()
    }
}

/// A paged document engine: opens a document and reports its shape.
///
/// Implementors wrap an untrusted parser, so every method is fallible and every operation
/// enforces the [`Limits`] it was given. **Implementors must not panic** — on
/// `wasm32-unknown-unknown` a panic is an uncatchable trap that poisons the instance
/// ([ADR 0009]).
///
/// # Why `open` takes the bytes by value
///
/// PDFium's in-memory load requires the input buffer to stay valid, at a stable address,
/// for as long as the document is open. Rather than document that and hope, the engine
/// *owns* the buffer: it is stored beside the document handle and dropped after it. The
/// buffer cannot be freed early, moved out, or mutated, because the caller no longer has
/// it.
///
/// A borrowing `Document<'a>` would prove the same thing on native, and was the obvious
/// first design. It was rejected because it forces a lifetime — in practice a GAT — into
/// this trait, and the web implementation has nothing to borrow: it copies the bytes into
/// a separate Emscripten heap and gets back an integer handle. A lifetime it can only
/// fake is worse than no lifetime at all.
///
/// # Why every call returns a `Result` and never a sentinel
///
/// Engine error state is global and is read *separately* from the value a call returns.
/// Combining the two into one number is how `-FPDF_GetLastError()` came to yield `-0`,
/// which `=== 0` in JS, so a failed load read as "opened fine, zero pages"
/// ([ADR 0006] requirement 6). Failure is decided by the primary return value; the error
/// code only classifies a failure that has already been established.
///
/// [ADR 0009]: ../../../docs/adr/0009-web-panic-contract-and-binding-boundary.md
/// [ADR 0006]: ../../../docs/adr/0006-wasm-linking-strategy.md
pub trait DocumentEngine {
    /// An opened document.
    ///
    /// Opaque, and `Send + Sync`: a caller may open a document on one thread and use it
    /// from another. That is a requirement on implementors, not a description of them —
    /// it is what forces an engine with thread-bound state to keep that state inside
    /// itself rather than in the handle it hands out.
    type Document: Send + Sync;

    /// Short identifier for the backing engine, e.g. `"pdfium"`. Used in diagnostics.
    fn name(&self) -> &'static str;

    /// Opens a document from an in-memory buffer.
    ///
    /// `bytes` is untrusted and possibly adversarial. Ownership transfers to the engine,
    /// which keeps the buffer alive for exactly as long as the document needs it.
    ///
    /// Limits are enforced in order: input size **before** the buffer reaches the engine,
    /// then the size-based memory estimate, then — immediately after the document loads,
    /// before any page is touched — the page count, and finally what the open actually
    /// cost. A document that already breaks a limit is closed rather than returned.
    ///
    /// Note that `max_input_bytes` bounds the *parse*, not the caller's allocation: the
    /// buffer already exists by the time it is checked. A binding that reads a file from
    /// disk or a `File` object must check the length before reading it in.
    ///
    /// # Errors
    ///
    /// - [`Error::Malformed`](burrow_types::Error::Malformed) — unparseable input.
    /// - [`Error::PasswordRequired`](burrow_types::Error::PasswordRequired) — encrypted,
    ///   and the supplied password (or its absence) did not open it.
    /// - [`Error::Unsupported`](burrow_types::Error::Unsupported) — a recognised
    ///   construct the engine will not handle, such as an unsupported security scheme.
    /// - [`Error::LimitExceeded`](burrow_types::Error::LimitExceeded) — a ceiling in
    ///   `options.limits` was reached.
    /// - [`Error::InvalidArgument`](burrow_types::Error::InvalidArgument) — the arguments
    ///   cannot describe a valid open, e.g. a password the engine's C API cannot carry.
    /// - [`Error::Internal`](burrow_types::Error::Internal) — the engine contradicted
    ///   itself, or the engine is no longer usable. Never expected.
    fn open(&self, bytes: Box<[u8]>, options: &OpenOptions<'_>) -> Result<Self::Document>;

    /// Number of pages in an opened document.
    ///
    /// Measured against the budget and the clock captured at
    /// [`open`](DocumentEngine::open): a budget covers the whole operation, not one call
    /// of it.
    ///
    /// **This deliberately takes no clock.** An earlier signature did, and it silently
    /// disabled `max_duration_ms`. A [`Deadline`](burrow_types::Deadline) stores a start
    /// reading from one clock's *unspecified* epoch; measured against a different clock
    /// the elapsed time saturates to zero, so the limit could never fire again — with no
    /// error and no warning. Implementors must hold the clock, not accept one.
    ///
    /// # Errors
    ///
    /// [`Error::LimitExceeded`](burrow_types::Error::LimitExceeded) if the document's
    /// deadline has passed, or a typed error if the page tree cannot be read.
    fn page_count(&self, doc: &Self::Document) -> Result<u64>;
}
