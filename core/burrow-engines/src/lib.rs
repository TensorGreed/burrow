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

use burrow_types::{Clock, Deadline, Limits, Password, Permutation, Result, Rotation};

// Engine error codes and their mapping to typed errors. Ungated, like `prescan` below and
// for the same reason: M1 PR 4's web path needs the SAME mapping table, and two copies of
// a mapping cannot be relied on to stay identical. Nothing here touches FFI -- a code is
// an integer -- so there is nothing to link and nothing to gate on.
mod codes;

// Memory cost estimation, and the two checks built on it. Ungated for the same reason as
// `codes`: the web path enforces the SAME ceiling with the same arithmetic, and only the
// counter it reads differs (process RSS on native, the engine module's heap on the web).
mod estimate;
// The one item `burrow-ops` needs out of it: `verify`'s read-back has to raise
// `max_memory_bytes` to what opening its own output will be estimated at, or a finished
// operation rejects itself. The heuristic stays private; only the number is public.
pub use estimate::estimated_open_bytes;

// Preparing a password for a C API. Ungated, and shared by both engines and both
// platforms -- it copies bytes and appends a NUL, which needs no engine.
mod password;

// The pixel ceiling and the BGRA-to-RGBA copy. Ungated for the same reason as `estimate`:
// both platforms must apply the SAME ceiling and produce the SAME bytes, and the only thing
// that differs between them is the engine call that fills the bitmap. See ADR 0027.
mod raster;

// Reading how much memory an operation actually cost. Linux-only and gated with the engine
// modules, because procfs is where the number comes from; the web path supplies its own
// reading through the bridge instead. Both feed the same `estimate::check_measured_memory`.
#[cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
mod rss;

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

// Reading PDF syntax in Rust: a dictionary's keys, and the resource names a content stream
// mentions. Pure Rust and `forbid(unsafe_code)`, so like `prescan` it is compiled everywhere.
// It exists because qpdf's own answers to those two questions are not callable -- the
// dictionary-key iterator does not route through `trap_errors` and the content-stream parser
// is not in the C API (ADR 0013 §1). `split`'s pruning needs both, and redaction reuses the
// module unchanged.
// The one-blank-page destination a split builds into. Ungated: both engine paths read it.
pub(crate) mod blank;

pub mod pdfsyntax;
mod redact;

// The pruning policy ADR 0019 §2b states, written ONCE and implemented over a seam both engine
// paths satisfy. Ungated like `pdfsyntax` and for a stronger reason: a divergence between two
// prunings is a leak on one platform and not the other, and the differential corpus compares
// outcomes and page counts -- neither of which can see an object that should not have travelled.
pub mod prune;

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

// Reading emitted documents back -- decompression, tolerant array matching, the page-tree
// walk. Shared for the same reason as the fixtures: `merge`, `split` and `reorder` each
// rediscovered that qpdf flates content streams and spaces its arrays, and the third time
// it cost six failing tests against a working operation.
#[cfg(test)]
#[path = "../testsupport/pdf_reading.rs"]
mod pdf_reading;

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

/// An engine that assembles one document out of the pages of several.
///
/// **The first capability in this crate that produces bytes.** Everything else here reads:
/// [`DocumentEngine`] opens and counts, [`StructureEngine`] inspects. That asymmetry is why
/// this is a third trait rather than a method on either of them —
/// [ADR 0017](../../../docs/adr/0017-merge-engine-and-failure-semantics.md) records that
/// the engine best at reading a damaged file is **not** the one that should write, and a
/// method on `DocumentEngine` would have forced one answer for both questions.
///
/// # The loop belongs to the caller, and that is deliberate
///
/// This trait deliberately does **not** offer `merge(inputs) -> bytes`. It exposes begin,
/// append and finish, so the operation in `burrow-ops` runs the loop, checks the deadline
/// between inputs, and applies the aggregate ceilings that no single call can see.
///
/// That costs a round trip per input on the web path, and
/// [ADR 0009](../../../docs/adr/0009-web-panic-contract-and-binding-boundary.md) §2 is
/// explicit that paying it is the point: moving the loop across the boundary would put a
/// decision in the one place `cargo test` cannot reach and iOS and Android cannot reuse.
///
/// # Implementors must not panic
///
/// On `wasm32-unknown-unknown` a panic is an uncatchable trap that poisons the whole
/// instance ([ADR 0009]). Return a typed error instead.
///
/// [ADR 0009]: ../../../docs/adr/0009-web-panic-contract-and-binding-boundary.md
pub trait PageAssembler {
    /// An assembly in progress: the destination document, plus every source document
    /// whose pages have been appended to it.
    ///
    /// **It owns the sources, and that is a correctness requirement rather than
    /// convenience.** qpdf resolves a foreign page's indirect objects lazily, when the
    /// destination is written — so releasing a source early produces a *truncated output
    /// rather than an error*. A silently short document is exactly the failure
    /// [ADR 0017](../../../docs/adr/0017-merge-engine-and-failure-semantics.md) §2 refuses,
    /// and the type system is a better place to prevent it than a comment.
    ///
    /// **No `Send` bound, deliberately.** `qpdf_data` may be used from distinct threads —
    /// only *sharing one between them* is forbidden (`qpdf-c.h:43-45`) — so a `Send` bound
    /// would be defensible. It is left off because nothing needs it: `burrow-ops` runs the
    /// assembly on the caller's thread and the web path runs it in the worker. Adding it
    /// would mean an `unsafe impl Send` justified by a rule the type system currently
    /// enforces for free, which is a worse trade than it looks.
    type Assembly;

    /// Short identifier for the backing engine, e.g. `"qpdf"`. Used in diagnostics.
    fn name(&self) -> &'static str;

    /// Begin an assembly from the first input, which becomes the destination document.
    ///
    /// **The output inherits this document's catalog** — its outline, `/AcroForm` and
    /// `/Names` are the ones that survive; later inputs contribute pages and not much
    /// else. ADR 0017 states that rather than leaving it to be discovered.
    ///
    /// Takes the bytes by value for the same reason [`DocumentEngine::open`] does: qpdf's
    /// in-memory read does not copy its input (`QPDF.hh:88-90`).
    ///
    /// # Errors
    ///
    /// The same set [`StructureEngine::check`] documents, for the same reasons: the input
    /// is opened and its structure walked before anything is appended to it.
    fn begin(&self, first: Box<[u8]>, options: &OpenOptions<'_>) -> Result<Self::Assembly>;

    /// How many pages the assembly currently holds.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`](burrow_types::Error::Internal) if the engine cannot answer —
    /// which, for an assembly it built itself, means the engine is in a state no input
    /// can explain.
    fn pages(&self, assembly: &Self::Assembly) -> Result<u64>;

    /// Append every page of `next`, in order, after the pages already present.
    ///
    /// Returns the number of pages appended, so the caller can apply an aggregate ceiling
    /// without asking the engine to count twice.
    ///
    /// # Errors
    ///
    /// The set [`StructureEngine::check`] documents. Note that an input the engine can
    /// *read* is not necessarily one it can append: ADR 0017 measured qpdf counting three
    /// pages in a trailer-less file and then failing to extract the first, so a
    /// [`Error::Malformed`](burrow_types::Error::Malformed) here is a normal outcome and
    /// not an engine fault.
    fn append(
        &self,
        assembly: &mut Self::Assembly,
        next: Box<[u8]>,
        options: &OpenOptions<'_>,
    ) -> Result<u64>;

    /// Serialise the assembly and return the bytes.
    ///
    /// Consumes the assembly: the sources are released here, once, after the write that
    /// needed them.
    ///
    /// # Errors
    ///
    /// - [`Error::Malformed`](burrow_types::Error::Malformed) — the assembled document
    ///   could not be written, which in practice means an input was worse than it looked.
    /// - [`Error::LimitExceeded`](burrow_types::Error::LimitExceeded) — a ceiling in
    ///   `options.limits`.
    /// - [`Error::Io`](burrow_types::Error::Io) — an allocation or engine failure.
    fn finish(&self, assembly: Self::Assembly) -> Result<Vec<u8>>;
}

/// An engine that can emit a document containing a chosen run of another document's pages.
///
/// The seam `split` is written against. It is deliberately **not** [`PageAssembler`] with an
/// extra method: the two operations differ in shape (many sources into one output, against one
/// source into many outputs) and in what they are allowed to keep, and giving them one trait
/// would mean a `begin` that sometimes wants a destination document and sometimes does not.
///
/// # What an extracted document may contain
///
/// **Only what travels with a page**, and nothing derived from the pages that were left
/// behind. This is a correctness requirement, not a fidelity preference:
/// [ADR 0019](../../../docs/adr/0019-how-split-builds-its-outputs.md) §2 measured an
/// implementation that kept the source's catalog and found a two-page output carrying the
/// outline titles of all five source pages — so somebody extracting pages 2-3 to send onward
/// would have been sending the titles of pages 4 and 5 with them.
///
/// It is the same property redaction depends on, stated for redaction in
/// [ADR 0006](../../../docs/adr/0006-wasm-linking-strategy.md)'s R8-R10: what matters is the
/// bytes that are **emitted**, never what the operation meant to emit.
///
/// So an implementation **builds** an output rather than carving one — start from nothing and
/// copy in what belongs, rather than starting from everything and removing what does not. The
/// first is an allowlist by construction; the second is a denylist over a structure whose
/// design permits keys nobody enumerated.
pub trait PageExtractor {
    /// A source document, opened once and read from for every output.
    ///
    /// **Opened once on purpose.** The alternative — reopening the source per output — parses
    /// it N times, and more importantly is the shape that invites carving, because a freshly
    /// opened source already has everything and only needs pages taken away.
    ///
    /// No `Send` bound, for the reason [`PageAssembler::Assembly`] gives.
    type Source;

    /// Short identifier for the backing engine, e.g. `"qpdf"`. Used in diagnostics.
    fn name(&self) -> &'static str;

    /// Open the document the outputs are drawn from.
    ///
    /// Takes the bytes by value for the same reason [`DocumentEngine::open`] does: qpdf's
    /// in-memory read does not copy its input.
    ///
    /// # Errors
    ///
    /// The same set [`StructureEngine::check`] documents: the input is opened and its
    /// structure walked before anything is extracted from it.
    fn open(&self, bytes: Box<[u8]>, options: &OpenOptions<'_>) -> Result<Self::Source>;

    /// How many pages the source has.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`](burrow_types::Error::Internal) if the engine reports a count that is not a count.
    fn pages(&self, source: &Self::Source) -> Result<u64>;

    /// Every page's `/Rotate` as written, in page order, read from the **source**.
    ///
    /// The promise [ADR 0022](../../../docs/adr/0022-every-operation-verifies-its-own-output.md)
    /// makes `split` verify each part against: the source's rotation vector, sliced per run.
    /// Recorded rather than judged — an out-of-spec value reads back as itself — because a
    /// witness only has to be stable between the read before the operation and the read after
    /// it. See `PageRotator::rotations`, whose walk this is.
    ///
    /// **The caller's deadline, not a new one.** `Deadline::start` resets the origin *and* the
    /// budget, so a sweep that made its own would be handed a whole second `max_duration_ms` —
    /// the defect ADR 0022 records being fixed three times over. This parameter is why the three
    /// `rotations` methods have one no other engine method has.
    ///
    /// # Errors
    ///
    /// - [`Error::Malformed`](burrow_types::Error::Malformed) — a page's `/Rotate` is not a number, or the page
    ///   tree could not be walked.
    /// - [`Error::LimitExceeded`](burrow_types::Error::LimitExceeded) — the deadline ran out mid-sweep. It is
    ///   checkpointed per page: on a deep page tree this walk is the largest part of the
    ///   operation, which ADR 0022 measured at 84%.
    fn rotations(
        &self,
        source: &Self::Source,
        options: &OpenOptions<'_>,
        deadline: &Deadline,
    ) -> Result<Vec<i64>>;

    /// A document containing `count` pages of `source`, starting at zero-based `first`.
    ///
    /// # Errors
    ///
    /// - [`Error::InvalidArgument`](burrow_types::Error::InvalidArgument) — the run is not inside the source.
    /// - [`Error::Malformed`](burrow_types::Error::Malformed) — a page could not be copied.
    /// - [`Error::Io`](burrow_types::Error::Io) — the output could not be written.
    /// - [`Error::LimitExceeded`](burrow_types::Error::LimitExceeded) — a ceiling was reached. **The ceilings
    ///   are the ones the source was opened under**, not `options.limits`, for the reason
    ///   [`PageRotator::rotate`] gives.
    fn extract(
        &self,
        source: &Self::Source,
        first: u64,
        count: u64,
        options: &OpenOptions<'_>,
    ) -> Result<Vec<u8>>;
}

/// Reading a document back, to check what an operation produced.
///
/// [ADR 0022](../../../docs/adr/0022-every-operation-verifies-its-own-output.md). Every
/// operation's last act is to reopen its own output through this and compare it against what
/// it promised.
///
/// # Why a trait of its own, rather than a `PageRotator` bound
///
/// It reads the same two things [`PageRotator`] does, and it is separate because the *purpose*
/// is what has to be visible at the call site. `merge<E: PageAssembler + PageRotator>` would
/// say merge rotates. This says merge checks its own work, which is the property ADR 0022 is
/// about — and it means a fake can be made to read back something other than what it wrote,
/// which is the only way to test that the check fires.
pub trait OutputReader {
    /// A document opened for reading back. Never the one that produced the bytes.
    type Read;

    /// An engine that shares no **document** state with this one.
    ///
    /// # What "fresh" means, and what it does not
    ///
    /// The handle that produced the bytes is not a neutral witness to them: it holds a page
    /// tree it built and then edited, and an engine in a bad state can agree with itself.
    /// So verification opens the output through a new engine value and a new document handle
    /// — a new `qpdf_data` — rather than asking the one that just wrote it.
    ///
    /// **On the web that is a new document handle inside the same WebAssembly module**, and
    /// it cannot be more than that. A genuinely fresh module means a fresh worker, which is a
    /// whole-payload fetch and a recompile per operation (6.8 MB when ADR 0022 measured it,
    /// about 1.8 MB since spike 0004); `docs/adr/0022` records the measurement and
    /// the decision. What that leaves undetectable is stated there too: a corrupted module
    /// heap can corrupt the writer and the reader alike, because they are the same heap.
    ///
    /// Natively there is no shared heap between the two handles beyond the process allocator,
    /// so the separation is as complete as it can be short of a subprocess.
    #[must_use]
    fn fresh(&self) -> Self
    where
        Self: Sized;

    /// Open bytes this crate just produced.
    ///
    /// # Errors
    ///
    /// Whatever the engine reports. A failure here is not a malformed *input* — the input was
    /// fine and burrow wrote this — so the caller reports it as a rejected output.
    fn open_output(&self, bytes: &[u8], options: &OpenOptions<'_>) -> Result<Self::Read>;

    /// How many pages the document that was read back has.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`](burrow_types::Error::Internal) if the engine reports a count that
    /// is not a count.
    fn page_count(&self, read: &Self::Read) -> Result<u64>;

    /// Every page's `/Rotate` **as written**, in page order, following inheritance — the same
    /// recorded-not-judged value [`PageRotator::rotations`] describes.
    ///
    /// **The witness.** A page count cannot see a permutation, and this is what the seam
    /// offers per page on both platforms without a new bridge method and without decompressing
    /// anything. It is a weak identity — two pages sharing a rotation are indistinguishable —
    /// and `burrow_ops::verify` states per operation what that leaves undetectable.
    ///
    /// # Errors
    ///
    /// - [`Error::Malformed`](burrow_types::Error::Malformed) — a `/Rotate` that is not an
    ///   integer at all, or a page tree that cannot be walked.
    /// - [`Error::LimitExceeded`](burrow_types::Error::LimitExceeded) — `max_duration_ms`. The
    ///   sweep is one call from outside and one FFI call per page times the page tree's depth
    ///   from inside, so it **checkpoints per page**. `options` is here for the clock; ignoring
    ///   it would put a loop the caller sized outside every deadline, which is what code and
    ///   security review both found it doing.
    ///
    /// # The deadline is the CALLER'S
    ///
    /// Passed in rather than started here, unlike every other looping method on these traits.
    /// Those bracket work the caller cannot see inside; this one is a sweep the caller asked
    /// for as part of its own operation, and `Deadline::start` resets the origin AND the
    /// budget -- so a sweep that started its own gave the operation a second full
    /// `max_duration_ms`. Measured by security review at 56 ms against a 50 ms budget, and it
    /// contradicts what `verify::output` says in capitals about spending one budget.
    fn rotations(
        &self,
        read: &Self::Read,
        options: &OpenOptions<'_>,
        deadline: &Deadline,
    ) -> Result<Vec<i64>>;
}

/// An engine that can change a page's rotation and emit the document.
///
/// The seam `rotate` is written against. Separate from [`PageExtractor`] and
/// [`PageAssembler`] because it is a different shape again: one document in, the *same*
/// document out, with an attribute changed. Nothing is selected and nothing is combined.
///
/// # It is not a subsetting operation, and that is load-bearing
///
/// [ADR 0019](../../../docs/adr/0019-how-split-builds-its-outputs.md) §2's rule — an output
/// that is a subset of its input must contain no data derived from what was excluded — does
/// not apply here, because nothing is excluded. The obligation runs the other way: **every
/// object in the input must still be in the output**, and the shared closure harness is used
/// in its inverted form (`assert_nothing_lost`) to say so.
///
/// # `/Rotate` is inheritable, which is the whole difficulty
///
/// A page's effective rotation is the nearest `/Rotate` on the page or any ancestor in the
/// page tree (PDF 32000-1 §7.7.3.4 lists it among the inheritable page attributes). So:
///
/// - **reading** one page's rotation means walking up `/Parent`, not reading the page
///   dictionary and concluding it has none;
/// - **writing** one page's rotation means writing to the page dictionary, never to the
///   ancestor the value was read from — that node may be the parent of hundreds of pages, and
///   setting it there rotates all of them.
///
/// [`PageRotator::effective_rotation`] is on the trait rather than being an implementation
/// detail precisely so that second property can be tested from outside: rotate one page, then
/// ask every other page what its rotation is.
pub trait PageRotator {
    /// A document opened once and rotated in place.
    ///
    /// No `Send` bound, for the reason [`PageAssembler::Assembly`] gives.
    type Source;

    /// Short identifier for the backing engine, e.g. `"qpdf"`. Used in diagnostics.
    fn name(&self) -> &'static str;

    /// Open the document to be rotated.
    ///
    /// # Errors
    ///
    /// The same set [`StructureEngine::check`] documents.
    fn open(&self, bytes: Box<[u8]>, options: &OpenOptions<'_>) -> Result<Self::Source>;

    /// How many pages the document has.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`](burrow_types::Error::Internal) if the engine reports a count that is not a count.
    fn pages(&self, source: &Self::Source) -> Result<u64>;

    /// The rotation page `index` displays at, following `/Rotate` up the page tree.
    ///
    /// # Errors
    ///
    /// - [`Error::InvalidArgument`](burrow_types::Error::InvalidArgument) — `index` is past the end.
    /// - [`Error::Malformed`](burrow_types::Error::Malformed) — `/Rotate` is not an integer, the page
    ///   tree is deeper than the engine will walk, or `/Parent` forms a cycle.
    fn effective_rotation(&self, source: &Self::Source, index: u64) -> Result<Rotation>;

    /// Every page's `/Rotate` **as written**, in page order, following inheritance.
    ///
    /// # This records; it does not judge
    ///
    /// The value is the one in the file, not a normalised one: a `/Rotate 45` reads back as
    /// `45`. [`PageRotator::effective_rotation`] is the judging read, and it refuses an
    /// out-of-spec value because a *turn* applied to one is undefined.
    ///
    /// The difference is ADR 0022's, and it was a regression before it was a rule: the promise
    /// sweep normalised every page, so a `/Rotate 45` on a page the request never named failed
    /// the whole operation. A witness only has to be **stable** between the read before an
    /// operation and the read after it, and 45 is a perfectly good witness.
    ///
    /// # Errors
    ///
    /// - [`Error::Malformed`](burrow_types::Error::Malformed) — a `/Rotate` that is not an
    ///   integer at all, or a page tree that cannot be walked. A value that cannot be *read*
    ///   is still a refusal; only the in-spec judgement is relaxed.
    /// - [`Error::LimitExceeded`](burrow_types::Error::LimitExceeded) — `max_duration_ms`,
    ///   checkpointed **per page** for the reason [`OutputReader::rotations`] gives, against
    ///   the **caller's** deadline for the reason it gives too.
    fn rotations(
        &self,
        source: &Self::Source,
        options: &OpenOptions<'_>,
        deadline: &Deadline,
    ) -> Result<Vec<i64>>;

    /// Turn every page in `pages` by `rotation`, and emit the document.
    ///
    /// The rotation is **relative**: a page already displaying at 90 that is turned another 90
    /// ends at 180. Pages not named are untouched, including pages that inherit their
    /// rotation from an ancestor a named page also inherits from.
    ///
    /// # Errors
    ///
    /// - [`Error::InvalidArgument`](burrow_types::Error::InvalidArgument) — `pages` is empty, names a page
    ///   past the end, or names the same page twice.
    /// - [`Error::Malformed`](burrow_types::Error::Malformed) — a page's existing rotation could not be read.
    /// - [`Error::Io`](burrow_types::Error::Io) — the output could not be written.
    /// - [`Error::LimitExceeded`](burrow_types::Error::LimitExceeded) — a ceiling was reached. **The ceilings
    ///   are the ones the source was opened under**, not `options.limits`: a caller must not be
    ///   able to loosen a limit after the document is already in memory. `options` is still read
    ///   for the clock, which is what `max_duration_ms` is checked against between pages.
    fn rotate(
        &self,
        source: &Self::Source,
        pages: &[u64],
        rotation: Rotation,
        options: &OpenOptions<'_>,
    ) -> Result<Vec<u8>>;
}

/// An engine that can put a document's pages in a different order.
///
/// The seam `reorder` is written against. Separate from [`PageRotator`] for the reason every
/// one of these is separate: the operations differ in what they take and in what they are
/// allowed to change, and one trait with an extra method would mean an `open` that sometimes
/// means one thing and sometimes another.
///
/// # It is not a subsetting operation either
///
/// Every input page appears in the output — that is what a permutation is — so
/// [ADR 0019](../../../docs/adr/0019-how-split-builds-its-outputs.md) §2's rule has nothing to
/// bite on, and the obligation is the inverse: nothing may be lost. `reorder` therefore edits
/// the document in place rather than building a new one, as [`PageRotator`] does and unlike
/// [`PageExtractor`]. Building would drop the outline, the attachments and the `/AcroForm`
/// (ADR 0019 §1), which is the right trade for a subsetting operation and pure loss here.
///
/// # The page tree is rewritten, and an implementation must say so
///
/// [ADR 0021](../../../docs/adr/0021-how-reorder-permutes-a-page-tree.md) measured what qpdf
/// does when a page moves: it **flattens the page tree**, after pushing inherited attributes
/// down onto each page. So what every page displays is preserved — an inherited `/Rotate`
/// survives as an explicit one — while intermediate `/Pages` nodes are gone. On a two-level
/// six-page fixture that is 16 objects in and 14 out.
///
/// That is a real change to the document and it is **not** a defect to be hidden: the inverse
/// closure test excludes page-tree scaffolding by name rather than the harness being weakened,
/// and `/reorder-pdf` says what changes in a person's words.
pub trait PageReorderer {
    /// A document opened once and permuted in place.
    ///
    /// No `Send` bound, for the reason [`PageAssembler::Assembly`] gives.
    type Source;

    /// Short identifier for the backing engine, e.g. `"qpdf"`. Used in diagnostics.
    fn name(&self) -> &'static str;

    /// Open the document to be reordered.
    ///
    /// # Errors
    ///
    /// The same set [`StructureEngine::check`] documents.
    fn open(&self, bytes: Box<[u8]>, options: &OpenOptions<'_>) -> Result<Self::Source>;

    /// How many pages the document has.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`](burrow_types::Error::Internal) if the engine reports a count that is not a count.
    fn pages(&self, source: &Self::Source) -> Result<u64>;

    /// Every page's `/Rotate` **as written**, in page order, following inheritance — the same
    /// recorded-not-judged value [`PageRotator::rotations`] describes.
    ///
    /// **The witness a reorder is verified against** (ADR 0022), read from the document that
    /// is about to be permuted — the only place the *input's* rotations exist once the bytes
    /// have been consumed. Comparing the output's vector against this one read through the
    /// permutation is what catches a permutation that is not the one asked for.
    ///
    /// Here rather than on [`OutputReader`] because it reads the INPUT, and reading it through
    /// `OutputReader` would mean parsing the input a second time — the cost `merge` declines
    /// to pay and this one does not have to.
    ///
    /// # Errors
    ///
    /// - [`Error::Malformed`](burrow_types::Error::Malformed) — a `/Rotate` that is not an
    ///   integer at all, or a page tree that cannot be walked. A value that cannot be *read*
    ///   is still a refusal; only the in-spec judgement is relaxed.
    /// - [`Error::LimitExceeded`](burrow_types::Error::LimitExceeded) — `max_duration_ms`,
    ///   checkpointed **per page** for the reason [`OutputReader::rotations`] gives, against
    ///   the **caller's** deadline for the reason it gives too.
    fn rotations(
        &self,
        source: &Self::Source,
        options: &OpenOptions<'_>,
        deadline: &Deadline,
    ) -> Result<Vec<i64>>;

    /// Put the pages in `order` and emit the document.
    ///
    /// [`Permutation`] has already established that the order names every page exactly once,
    /// which is the invariant `docs/ROADMAP.md` states — so an implementation may rely on it
    /// and does not re-derive it. What it must still check is that the permutation is for
    /// **this** document: a `Permutation` built against a different page count is a caller's
    /// mistake, not a malformed file.
    ///
    /// # Errors
    ///
    /// - [`Error::InvalidArgument`](burrow_types::Error::InvalidArgument) — the permutation is not for a document
    ///   of this size.
    /// - [`Error::Malformed`](burrow_types::Error::Malformed) — a page could not be moved.
    /// - [`Error::Io`](burrow_types::Error::Io) — the output could not be written.
    /// - [`Error::LimitExceeded`](burrow_types::Error::LimitExceeded) — a ceiling was reached. **The ceilings are
    ///   the ones the source was opened under**, not `options.limits`, for the reason
    ///   [`PageRotator::rotate`] gives.
    fn reorder(
        &self,
        source: &Self::Source,
        order: &Permutation,
        options: &OpenOptions<'_>,
    ) -> Result<Vec<u8>>;
}

/// An engine that can re-encode a document smaller without changing what it says.
///
/// The seam `compress` is written against. Separate from [`PageRotator`] and
/// [`PageReorderer`] for the reason every one of these is separate: the operations differ in
/// what they take and in what they are allowed to change.
///
/// # It is not a subsetting operation, and it is not a lossy one
///
/// Every input page appears in the output, so
/// [ADR 0019](../../../docs/adr/0019-how-split-builds-its-outputs.md) §2's rule has nothing to
/// bite on and the obligation is the inverse: **nothing may be lost**. The whole point of the
/// operation is that the bytes change while the content does not, which is a stronger claim
/// than the other operations make and is the one an implementation must not quietly weaken.
///
/// So: no image is re-encoded, no font is subsetted, and no content stream's meaning changes.
/// Only how objects are stored.
///
/// # There is exactly one lever, and that is a measurement rather than a simplification
///
/// [Spike 0005](../../../docs/spikes/0005-what-qpdf-alone-compresses.md) established that four
/// of the five compression levers qpdf's C API exposes are already qpdf's own writer defaults
/// (`QPDFWriter_private.hh:296-315`) — stream compression, dropping unreferenced objects, the
/// decode level, and not linearizing. Every burrow operation has had them since `merge`
/// shipped. **Generating object streams is the only thing this operation adds.**
///
/// Two consequences an implementation must respect:
///
/// - the other four must **not** be re-stated as configuration. Spelling a default out as a
///   setting invites someone to tune it later, and the thing they would be tuning is what
///   every other operation already emits;
/// - `qpdf` has **no duplicate-object removal** and no image or font handling at any setting,
///   so an implementation that appears to offer either is doing something this trait does not
///   sanction.
///
/// # What it is worth, so nobody is surprised by a small number
///
/// Measured, against what the same engine writes without the lever: **85.6%** on a form of
/// many small objects, **12.0%** on a text-heavy report, **0.15%** on a scan and **0.12%** on a
/// photo-heavy document, with a median of **16.0%** across qpdf's own 618-file corpus.
///
/// The spread is the point. A document that is mostly image payload has almost nothing
/// reachable, because object streams act on structure and qpdf does not touch DCT data.
///
/// # The output can be larger, and this trait does not hide it
///
/// Measured at **3.4%** of that corpus strictly larger, with a further 5.3% byte-identical —
/// an object stream has a fixed overhead, so it loses on documents with little structure to
/// pack. `compress` returns what the engine produced; **deciding that a larger output is not
/// worth returning is the operation's job, not the engine's**, because "give the caller their
/// own bytes back" is a policy about the request rather than a fact about the document.
///
/// Keeping that out of here matters for a second reason: an engine that silently returned the
/// input when its own output was bigger would make [ADR 0022]'s read-back verify bytes burrow
/// did not produce, without anything saying so.
///
/// [ADR 0022]: ../../../docs/adr/0022-every-operation-verifies-its-own-output.md
pub trait DocumentCompressor {
    /// A document opened once and re-encoded.
    ///
    /// No `Send` bound, for the reason [`PageAssembler::Assembly`] gives.
    type Source;

    /// Short identifier for the backing engine, e.g. `"qpdf"`. Used in diagnostics.
    fn name(&self) -> &'static str;

    /// Open the document to be compressed.
    ///
    /// # Errors
    ///
    /// The same set [`StructureEngine::check`] documents.
    fn open(&self, bytes: Box<[u8]>, options: &OpenOptions<'_>) -> Result<Self::Source>;

    /// How many pages the document has.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`](burrow_types::Error::Internal) if the engine reports a count that is
    /// not a count.
    fn pages(&self, source: &Self::Source) -> Result<u64>;

    /// Every page's `/Rotate` **as written**, in page order, following inheritance.
    ///
    /// Identical in meaning to [`PageRotator::rotations`], including that it records rather
    /// than judges, and present for the same reason: it is [ADR 0022]'s promise, and
    /// compression must leave it untouched on every page. A compression that moved or
    /// reattributed a page would be a different document wearing the right page count.
    ///
    /// [ADR 0022]: ../../../docs/adr/0022-every-operation-verifies-its-own-output.md
    ///
    /// # Errors
    ///
    /// - [`Error::Malformed`](burrow_types::Error::Malformed) — a `/Rotate` that is not an
    ///   integer at all, or a page tree that cannot be walked.
    /// - [`Error::LimitExceeded`](burrow_types::Error::LimitExceeded) — `max_duration_ms`,
    ///   checkpointed **per page** and against the **caller's** deadline, for the reasons
    ///   [`PageRotator::rotations`] gives.
    fn rotations(
        &self,
        source: &Self::Source,
        options: &OpenOptions<'_>,
        deadline: &Deadline,
    ) -> Result<Vec<i64>>;

    /// Re-encode the document and emit it.
    ///
    /// Returns what the engine produced, **even if it is larger than the input** — see the
    /// trait docs for why that decision does not live here.
    ///
    /// # Errors
    ///
    /// - [`Error::Io`](burrow_types::Error::Io) — the output could not be written.
    /// - [`Error::Malformed`](burrow_types::Error::Malformed) — the document could not be
    ///   re-encoded.
    /// - [`Error::LimitExceeded`](burrow_types::Error::LimitExceeded) — a ceiling was reached.
    ///   **The ceilings are the ones the source was opened under**, not `options.limits`: a
    ///   caller must not be able to loosen a limit after the document is already in memory.
    ///   `options` is still read for the clock.
    fn compress(&self, source: &Self::Source, options: &OpenOptions<'_>) -> Result<Vec<u8>>;
}

/// One page, rasterised.
///
/// **RGBA, eight bits per channel, no padding between rows.** `rgba.len()` is exactly
/// `width * height * 4`, and that is asserted by [`PageRenderer::render`]'s implementors
/// rather than promised — the length is a *decision* the caller made, not a fact the file
/// stated, which is what makes it worth checking.
///
/// # Why RGBA, when PDFium produces BGRA
///
/// The swizzle happens **in Rust**, in the implementation, on both platforms. The web path
/// could have done it in JavaScript while copying out of the engine heap and saved a pass,
/// and that was rejected: [ADR 0009] §2 lets the bridge marshal and forbids it deciding, and
/// a channel order is exactly the kind of "small" decision that ends up differing between the
/// two platforms with nothing to catch it. The differential harness compares typed outcomes,
/// and two channel orders would look identical in every one of them.
///
/// **PDFium may pad its rows** (`FPDFBitmap_GetStride`), so the copy is per row and the stride
/// is read rather than assumed. A reader that computed `width * 4` would return the wrong
/// pixels on exactly the pages where padding happens, and only on those.
///
/// [ADR 0009]: ../../../docs/adr/0009-web-panic-contract-and-binding-boundary.md
#[derive(Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Raster {
    /// Width in pixels. What was asked for, not what the page measures.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// `width * height * 4` bytes, row-major, RGBA.
    pub rgba: Vec<u8>,
}

impl core::fmt::Debug for Raster {
    /// Hand-written so that formatting one CANNOT print page content.
    ///
    /// A derived `Debug` renders `rgba` in full: decoded pixels of somebody's document, into
    /// whatever consumed the format — a log line, a panic message, an `assert_eq!` failure.
    /// `core/CLAUDE.md` says file content must never reach any of those, and a derive is a
    /// footgun nobody has to pull deliberately. No call site formats one today; this is so
    /// that the next one is safe by construction. Found by security review.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Raster")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("rgba_len", &self.rgba.len())
            .finish_non_exhaustive()
    }
}

impl Raster {
    /// A raster, with its length checked against its dimensions.
    ///
    /// `#[non_exhaustive]` makes this the only way to build one from outside this crate, and
    /// that is the point: **the length is a decision rather than a fact the file stated**, so
    /// the invariant is enforced at construction instead of being described in a doc comment
    /// somebody has to keep honouring. Every implementor of [`PageRenderer`] goes through
    /// here, including the fakes in tests, so a fake cannot hold itself to a weaker rule than
    /// the engine.
    ///
    /// # Errors
    ///
    /// [`Error::Internal`](burrow_types::Error::Internal) if `rgba.len()` is not
    /// `width * height * 4`, or if that product does not fit this target. Never the
    /// document's fault, and deliberately not reported as if it were.
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self> {
        let expected = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|pixels| pixels.checked_mul(raster::BYTES_PER_PIXEL))
            .and_then(|bytes| usize::try_from(bytes).ok())
            .ok_or_else(|| {
                burrow_types::Error::Internal("raster byte count does not fit".to_owned())
            })?;
        if rgba.len() != expected {
            return Err(burrow_types::Error::Internal(
                "the rendered page is not the size that was asked for".to_owned(),
            ));
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }
}

/// An engine that draws a page.
///
/// **The first capability in this crate that produces PIXELS.** Everything else either reads a
/// document ([`DocumentEngine`], [`StructureEngine`]) or produces another document
/// ([`PageAssembler`], [`PageRotator`], [`PageReorderer`], [`DocumentCompressor`]), and the
/// difference is not cosmetic — a raster's size is chosen by the caller rather than implied by
/// the input, so it is the first seam here where a ceiling guards *the request*.
///
/// # What it promises, and the one thing it cannot
///
/// [ADR 0027] states the whole contract, and the residue it leaves is stated there rather than
/// implied: the returned buffer's length is checked against `width * height * 4`, and the page
/// index is checked against the count — but **nothing in the returned pixels identifies which
/// page they came from**. A render is a read (like [`DocumentEngine::page_count`]), so
/// [ADR 0022]'s read-back through a fresh engine does not apply and `verify::Expected` gains no
/// variant for it. That is a decision, not an omission.
///
/// # Why it EXTENDS `DocumentEngine` rather than declaring its own `open`
///
/// Every other seam here ([`PageRotator`], [`PageExtractor`], [`DocumentCompressor`]) declares
/// its own `open` and its own `Source`, because each is satisfied by qpdf, which is not a
/// [`DocumentEngine`] at all. Rendering is different: it is PDFium's, and PDFium *is* the
/// `DocumentEngine` — so a second `open` here would have been the same open written twice,
/// with two places for the limit order to drift apart.
///
/// It also keeps [`Self::Document`](DocumentEngine::Document)'s `Send + Sync` bound as the
/// one statement of it. **A PDFium `FPDF_PAGE` may never live in that type**: a page handle is
/// neither, and holding one across calls is what would cost the native document its derived
/// `Send + Sync` and force an `unsafe impl` nothing in this crate has. Every page this trait
/// loads is closed before the call that loaded it returns.
///
/// # `max_pixels`, and why it is checked here rather than only in `burrow-ops`
///
/// The check has to sit immediately before the allocation it guards, because the thing it
/// prevents is the allocation. `FPDFBitmap_Create` returning null for a large page is a
/// reachable outcome, not a theoretical one — and by then the cost has already been paid.
/// `crate::raster::check_pixels` is the one implementation both platforms call, for the same
/// reason `crate::estimate::before_open` exists. (Not linked: both are private, which is itself
/// the point -- a ceiling the caller could reach around would not be one.)
///
/// # Implementors must not panic
///
/// On `wasm32-unknown-unknown` a panic is an uncatchable trap that poisons the whole instance
/// ([ADR 0009]). Return a typed error instead.
///
/// [ADR 0009]: ../../../docs/adr/0009-web-panic-contract-and-binding-boundary.md
/// [ADR 0022]: ../../../docs/adr/0022-every-operation-verifies-its-own-output.md
/// [ADR 0027]: ../../../docs/adr/0027-what-a-render-promises-and-what-it-refuses.md
pub trait PageRenderer: DocumentEngine {
    /// A page's size in **points**, at 72 to the inch, with its own `/Rotate` applied.
    ///
    /// `index` is **zero-based**, like every engine seam here; the one-based number a person
    /// types is converted once, at `burrow-ops`' boundary.
    ///
    /// This is what a caller needs to preserve a page's aspect ratio, and it is a separate
    /// call rather than a field of [`Raster`] because the caller has to know it *before* it
    /// can choose the width and height to ask for.
    ///
    /// # Errors
    ///
    /// - [`Error::InvalidArgument`](burrow_types::Error::InvalidArgument) — the index is past
    ///   the end of the document.
    /// - [`Error::Malformed`](burrow_types::Error::Malformed) — the page could not be loaded.
    /// - [`Error::LimitExceeded`](burrow_types::Error::LimitExceeded) — the deadline.
    fn page_size(&self, doc: &Self::Document, index: u64) -> Result<(f32, f32)>;

    /// Draw page `index` at exactly `width` x `height` pixels.
    ///
    /// The page is scaled into the box given; **the aspect ratio is the caller's to preserve**
    /// (with [`page_size`](PageRenderer::page_size)), and deliberately not this seam's to
    /// impose. That is also what makes the ceiling below a bound on the *request*: a
    /// 14400-point page asked for at thumbnail size costs what a thumbnail costs.
    ///
    /// The page's own `/Rotate` is applied by the engine. Nothing here rotates a second time.
    ///
    /// # Errors
    ///
    /// - [`Error::InvalidArgument`](burrow_types::Error::InvalidArgument) — a zero dimension,
    ///   or an index past the end of the document.
    /// - [`Error::LimitExceeded`](burrow_types::Error::LimitExceeded) — `width * height`
    ///   exceeds `max_pixels`, at [`Stage::Pixels`](burrow_types::Stage::Pixels), **before any
    ///   raster is allocated**; or the deadline has passed.
    ///
    /// # `max_pixels` comes from `options`, not from the open, and that is the exception
    ///
    /// [`DocumentCompressor::compress`] reads the ceilings the source was opened under, so a
    /// caller cannot loosen a limit once the document is already in memory. That argument does
    /// not transfer here, because **`max_pixels` bounds the request rather than the
    /// document**: the size is chosen at this call and nothing about it was decided at the
    /// open. Reading it from the open would mean a caller could not ask for a thumbnail and a
    /// larger view of the same document without reopening it, which is the ordinary case.
    ///
    /// Every ceiling that *is* about the document -- `max_input_bytes`, `max_memory_bytes`,
    /// `max_pages` -- was applied at [`DocumentEngine::open`] and is not re-read here.
    /// - [`Error::Malformed`](burrow_types::Error::Malformed) — the page could not be loaded.
    /// - [`Error::Io`](burrow_types::Error::Io) — the bitmap could not be allocated.
    /// - [`Error::Internal`](burrow_types::Error::Internal) — the engine handed back a buffer
    ///   that is not `width * height * 4` bytes, or contradicted itself some other way.
    fn render(
        &self,
        doc: &Self::Document,
        index: u64,
        width: u32,
        height: u32,
        options: &OpenOptions<'_>,
        deadline: &Deadline,
    ) -> Result<Raster>;
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
    /// disabled `max_duration_ms`. A [`Deadline`] stores a start
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
