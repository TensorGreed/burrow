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

// Engine error codes and their mapping to typed errors. Ungated, like `prescan` below and
// for the same reason: M1 PR 4's web path needs the SAME mapping table, and two copies of
// a mapping cannot be relied on to stay identical. Nothing here touches FFI -- a code is
// an integer -- so there is nothing to link and nothing to gate on.
mod codes;

// Memory cost estimation, and the two checks built on it. Ungated for the same reason as
// `codes`: the web path enforces the SAME ceiling with the same arithmetic, and only the
// counter it reads differs (process RSS on native, the engine module's heap on the web).
mod estimate;

// Preparing a password for a C API. Ungated, and shared by both engines and both
// platforms -- it copies bytes and appends a NUL, which needs no engine.
mod password;

// The PDFium implementation. Gated on the libraries actually being linked: `build.rs`
// sets `burrow_native_engines` only when the `native-engines` feature is on, the target
// is Linux, and every vendored library has passed its checksum. The trait above stays
// ungated so `cargo check --target aarch64-apple-ios --all-features` keeps working.
#[cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
pub mod pdfium;

// The qpdf implementation, gated the same way. Answers different questions from PDFium --
// structure, encryption, object streams, linearisation (ADR 0004) -- and runs on the
// caller's thread rather than PDFium's engine thread (ADR 0013).
#[cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
pub mod qpdf;

// The structural pre-scan. Pure Rust and `forbid(unsafe_code)`, so unlike the engine
// modules it is compiled everywhere -- there is nothing to link and nothing to gate on.
// That is deliberate: M1 PR 4's web path needs exactly this check before it hands bytes to
// the JS bridge, and it should get it without a second implementation.
pub mod prescan;

// The web implementations of both engine traits, plus the bridge traits the JS binding
// implements. Ungated for the same reason as `prescan`: the orchestration is shared Rust,
// and compiling it everywhere is what lets `cargo test` drive the whole web path against a
// fake bridge on an ordinary host. See ADR 0006 and ADR 0009.
pub mod web;

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

// The generated PDF fixtures, included once for the whole crate. Both engine test modules
// need them, and including the file twice in one crate is a duplicate module -- so the
// `#[path]` lives here and they `use crate::minimal_pdf`.
#[cfg(test)]
#[path = "../testsupport/minimal_pdf.rs"]
mod minimal_pdf;

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

/// Everything a structure engine needs to inspect one document.
#[non_exhaustive]
pub struct CheckOptions<'a> {
    /// The ceilings this check must enforce.
    pub limits: Limits,
    /// The clock the operation's deadline is measured against.
    ///
    /// Present for the same reason [`OpenOptions`] has one, and enforced the same way —
    /// see [`StructureEngine::check`] for how little it buys and why it is here anyway.
    pub clock: Arc<dyn Clock>,
    /// The password for an encrypted document, if one is known.
    pub password: Option<&'a Password>,
    /// Whether the engine may reconstruct a damaged document to answer the question.
    ///
    /// **Off by default, deliberately.** A structural check that silently repairs the
    /// structure it is checking is not a check — it answers "could this be made to work?"
    /// when the caller asked "does this work?". Repair is also where a damaged file makes
    /// an engine expensive.
    ///
    /// Turning it on is how you ask the other question, and it is what M1 item 6's
    /// damaged-file corpus needs. Note that it makes engines chattier: qpdf's
    /// reconstruction warnings quote byte offsets and object numbers, which is exactly
    /// what `tests/secret_leak.rs` exists to keep out of any output.
    pub attempt_recovery: bool,
}

impl<'a> CheckOptions<'a> {
    /// Options with the given limits and clock, and no password.
    #[must_use]
    pub fn new(limits: Limits, clock: Arc<dyn Clock>) -> Self {
        Self {
            limits,
            clock,
            password: None,
            attempt_recovery: false,
        }
    }
}

impl core::fmt::Debug for CheckOptions<'_> {
    /// Hand-written for the same reason [`OpenOptions`]'s is: this struct holds a
    /// password, and a derive would be one field away from rendering a secret if
    /// [`Password`]'s own `Debug` ever stopped redacting.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CheckOptions")
            .field("limits", &self.limits)
            .field("has_password", &self.password.is_some())
            .field("attempt_recovery", &self.attempt_recovery)
            .finish_non_exhaustive()
    }
}

/// What an engine can say about a document's structure without rendering it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct StructureReport {
    /// Pages the document's page tree actually yields.
    ///
    /// The only field, for now. `encrypted` and `linearized` were here and were removed:
    /// qpdf's `qpdf_is_encrypted` and `qpdf_is_linearized` do not catch C++ exceptions,
    /// and the latter aborts the process on an ordinary small file. Reporting them needs a
    /// C++ shim, which is a decision rather than a detail — see
    /// [ADR 0013](../../../docs/adr/0013-qpdf-c-api-and-prescan.md).
    ///
    /// Encryption is still detectable, through the error path:
    /// [`Error::PasswordRequired`](burrow_types::Error::PasswordRequired) means encrypted
    /// and not openable with what was supplied.
    pub pages: u64,
}

/// An engine that inspects a document's structure.
///
/// Deliberately **not** [`DocumentEngine`]. That trait is about opening a document to work
/// with its pages; this one is about deciding whether a document's structure hangs
/// together at all, and which engine is right for each is not the same question
/// ([ADR 0004](../../../docs/adr/0004-native-engines.md)).
pub trait StructureEngine {
    /// Short identifier for the backing engine, e.g. `"qpdf"`. Used in diagnostics.
    fn name(&self) -> &'static str;

    /// Inspect `bytes` without rendering anything.
    ///
    /// Takes the bytes by value for the same reason [`DocumentEngine::open`] does: qpdf's
    /// in-memory read does not copy its input (`QPDF.hh:88-90`), so the engine must own
    /// the buffer for as long as it holds the document.
    ///
    /// # Errors
    ///
    /// - [`Error::Malformed`](burrow_types::Error::Malformed) — the structure is unusable.
    /// - [`Error::PasswordRequired`](burrow_types::Error::PasswordRequired) — encrypted,
    ///   and the supplied password (or its absence) did not open it.
    /// - [`Error::Unsupported`](burrow_types::Error::Unsupported) — a recognised feature
    ///   the engine will not handle.
    /// - [`Error::LimitExceeded`](burrow_types::Error::LimitExceeded) — a ceiling in
    ///   `options.limits`, or the structural pre-scan, rejected the file.
    /// - [`Error::InvalidArgument`](burrow_types::Error::InvalidArgument) — the arguments
    ///   cannot describe a valid check.
    /// - [`Error::Io`](burrow_types::Error::Io) — an I/O or memory failure inside the
    ///   engine. Not the document's fault, and deliberately not reported as if it were.
    ///
    /// # What `max_duration_ms` buys here, which is little
    ///
    /// A structure check is essentially **one uninterruptible engine call**, so the
    /// deadline can only be observed on either side of it: it catches a caller whose
    /// budget was already spent, and a check that overran — but it cannot stop one that is
    /// running. No engine offers a timeout, a cancellation or an abort hook, so that is
    /// the whole of what is available in-process
    /// ([ADR 0013](../../../docs/adr/0013-qpdf-c-api-and-prescan.md) records the platform
    /// answers).
    ///
    /// It is enforced anyway, because a `Limits` field that is silently ignored is worse
    /// than one whose weakness is written down.
    fn check(&self, bytes: Box<[u8]>, options: &CheckOptions<'_>) -> Result<StructureReport>;
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
