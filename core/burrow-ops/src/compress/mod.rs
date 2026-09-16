//! Re-encode a document smaller without changing what it says.
//!
//! # What compression is here, and what it is not
//!
//! **Lossless, and by construction rather than by policy.** No image is re-encoded, no font is
//! subsetted, no content stream's meaning changes. The engine sets one storage lever —
//! object stream generation — and touches nothing a page contains. That is what lets
//! `docs/ROADMAP.md`'s invariant *text remains extractable* hold without a test having to
//! establish it, and it is why `assert_nothing_lost` is a claim this operation can make.
//!
//! [Spike 0005](../../../../docs/spikes/0005-what-qpdf-alone-compresses.md) is why there is one
//! lever: four of the five qpdf's C API exposes are already the writer's own defaults, so every
//! other burrow operation has been emitting them since `merge` shipped.
//!
//! # It is not a subsetting operation
//!
//! Every input page appears in the output, so
//! [ADR 0019](../../../../docs/adr/0019-how-split-builds-its-outputs.md) §2's rule — an output
//! that is a subset of its input may carry nothing derived from the excluded pages — has
//! nothing to bite on. The obligation is the **inverse**, and it is the operation's entire
//! point: the bytes change and the content does not. `tests/compress_keeps_everything.rs` uses
//! the shared object-closure harness in its `assert_nothing_lost` form, **not** `assert_closed`
//! — which is mathematically vacuous when every page is kept, measured: it accepted a one-page
//! output while being told all five pages were included.
//!
//! # What it is worth, so a small number is not a surprise
//!
//! Measured against what the same engine writes without the lever: **85.6%** on a form of many
//! small objects, **12.0%** on a text-heavy report, **0.15%** on a scan, **0.12%** on a
//! photo-heavy document, median **16.0%** across qpdf's own 618-file corpus.
//!
//! The spread is the finding. A document that is mostly image payload has almost nothing
//! reachable, because object streams act on structure and qpdf does not touch DCT data at any
//! setting. A caller that reports a percentage without reporting which kind of document it
//! measured is reporting an average across incomparable things.
//!
//! # Limits
//!
//! | limit | where it applies |
//! |---|---|
//! | `max_input_bytes` | the input, once, at [`Stage::InputSize`] |
//! | `max_pages` | the input's page count, at [`Stage::PageCount`] |
//! | `max_duration_ms` | the whole operation, on the injected clock, checkpointed around the sweep and the write |
//! | `max_memory_bytes` | **detected, never bounded** (ADR 0007) |
//! | `max_pixels` | not applicable — compression decodes no raster |
//!
//! One input and one output, so unlike `merge` there is no aggregate-versus-per-item question.
//!
//! **`max_duration_ms` is weaker here than anywhere else, and that is worth stating.** The
//! write is a single engine call and it is the most expensive one in the crate; qpdf offers no
//! timeout, cancellation or abort hook (ADR 0007), so a document that takes ten minutes to
//! re-encode takes ten minutes and is refused afterwards. On the web the worker watchdog is
//! what actually bounds it. Natively nothing does.

#[cfg(test)]
mod tests;

use std::sync::Arc;

use crate::verify;
use burrow_engines::{DocumentCompressor, OpenOptions, OutputReader};
use burrow_types::{Deadline, Error, Limits, Result, Stage};

/// What compressing a document produced.
///
/// # Why this is not `Result<Vec<u8>>`
///
/// Compression can make a document **larger** — measured at 3.4% of qpdf's own corpus, with a
/// further 5.3% left byte-identical, because an object stream carries a fixed overhead and
/// loses on documents with little structure to pack. "Refuse to make it worse" is therefore a
/// real requirement and not a nicety.
///
/// The obvious shape is to return the caller's own input bytes when that happens. **That was
/// considered and rejected**, for two reasons that point the same way:
///
/// - **It costs a copy of the input, for nothing.** The engine seam takes its bytes by value —
///   it must, because the web path copies them into a separate heap and cannot borrow — so
///   returning them means holding a second copy across the whole operation, on every call,
///   against a `max_memory_bytes` that only *detects*. The overwhelming majority of calls are
///   the ones that do get smaller, and they would pay it too.
/// - **It makes the caller unable to say anything true.** A person who is handed back a file
///   of exactly the size they supplied has been told nothing. Both numbers are what lets a page
///   say *"this file is already efficiently stored — we produced 1.2 MB against your 1.1 MB, so
///   we kept yours"*, which is the difference between a tool that reports and a tool that
///   silently returns the same file.
///
/// There is also a correctness reason not to hand the input back as though burrow produced it:
/// [ADR 0022](../../../../docs/adr/0022-every-operation-verifies-its-own-output.md)'s read-back
/// would then be verifying bytes burrow did not author, with nothing saying so.
///
/// Every caller already holds the input. On the web that is explicit — ADR 0015 requires the
/// page to send a `Blob` rather than a transferred buffer precisely so the caller keeps a
/// usable handle — so a page offering the original file needs no bytes from here at all.
///
/// # Deliberately NOT `#[non_exhaustive]`, which is the opposite of what `Error` does
///
/// `burrow_types::Error` is `#[non_exhaustive]` so that adding a variant is not a breaking
/// change. The reasoning does not carry over, and applying it here by habit would be a defect:
///
/// - An `Error` variant a caller has not heard of degrades safely — it is still an error, and
///   a catch-all arm reports it as one.
/// - An `Outcome` variant a caller has not heard of does **not** degrade safely. Every arm
///   here decides what a person is told and whether they are offered a file. A third outcome
///   falling silently into somebody's `_` arm is a page saying the wrong thing about their
///   document, which is exactly the class of failure this operation's honesty rests on.
///
/// So adding a variant here is a **compile error at every call site**, on purpose. That is a
/// breaking change and it should be: the callers are the bindings and the tool page, and both
/// have to be re-read when the set of answers changes.
#[derive(Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The re-encoding is smaller, and here it is — verified (ADR 0022).
    Smaller {
        /// The compressed document.
        document: Vec<u8>,
        /// What the caller supplied, in bytes.
        original_bytes: u64,
    },
    /// The re-encoding was **not** smaller, so it was discarded and nothing is returned.
    ///
    /// Both numbers are counts of bytes this crate measured; neither is derived from file
    /// content, so a caller may show them.
    ///
    /// `produced_bytes >= original_bytes` always holds here — equality included, because a
    /// document that re-encodes to exactly its own size is one where compression achieved
    /// nothing, and handing back an identical-sized file as an achievement is the claim this
    /// variant exists to avoid making.
    NotSmaller {
        /// What the caller supplied, in bytes.
        original_bytes: u64,
        /// What the re-encoding came to, in bytes. Those bytes are gone.
        produced_bytes: u64,
    },
}

/// Hand-written, so a document's bytes cannot reach a log through a derive.
///
/// `core/CLAUDE.md`: *never log or embed file content in an error, a debug print, or a panic
/// message*. `merge`'s `Input` and `burrow_types::Password` both do this for the same reason;
/// `Outcome` is the first **operation return type** that carries document bytes, because the
/// other four return a bare `Vec<u8>` that nobody formats.
///
/// That is not a theoretical exposure: the tests in this crate format outcomes with `{:?}` in
/// their failure messages, and on the `Smaller` arm a derive would have dumped the whole
/// document into the test output. Found by code review.
///
/// The length is what a reader is diagnosing anyway — every assertion about an `Outcome` is
/// about sizes.
impl core::fmt::Debug for Outcome {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Smaller {
                document,
                original_bytes,
            } => f
                .debug_struct("Smaller")
                .field("document", &format_args!("{} bytes", document.len()))
                .field("original_bytes", original_bytes)
                .finish(),
            Self::NotSmaller {
                original_bytes,
                produced_bytes,
            } => f
                .debug_struct("NotSmaller")
                .field("original_bytes", original_bytes)
                .field("produced_bytes", produced_bytes)
                .finish(),
        }
    }
}

impl Outcome {
    /// The input's size in bytes, whichever way it went.
    #[must_use]
    pub const fn original_bytes(&self) -> u64 {
        match self {
            Self::Smaller { original_bytes, .. } | Self::NotSmaller { original_bytes, .. } => {
                *original_bytes
            }
        }
    }

    /// How many bytes the re-encoding came to, whichever way it went.
    #[must_use]
    pub fn produced_bytes(&self) -> u64 {
        match self {
            Self::Smaller { document, .. } => u64::try_from(document.len()).unwrap_or(u64::MAX),
            Self::NotSmaller { produced_bytes, .. } => *produced_bytes,
        }
    }

    /// The compressed document, if there is one.
    ///
    /// # Returns
    ///
    /// `None` on [`Outcome::NotSmaller`], which means a document **was** produced, measured and
    /// then discarded for being no smaller than the input — not that the operation produced
    /// nothing. The two read alike and are different facts, and the one a caller shows a person
    /// is the second.
    #[must_use]
    pub fn document(&self) -> Option<&[u8]> {
        match self {
            Self::Smaller { document, .. } => Some(document),
            Self::NotSmaller { .. } => None,
        }
    }
}

/// Re-encode `bytes` and return the smaller of the two, or say that there was not one.
///
/// # Errors
///
/// - [`Error::Malformed`], [`Error::Unsupported`], [`Error::PasswordRequired`] — the input
///   could not be read.
/// - [`Error::LimitExceeded`] — a ceiling in `options.limits` was reached.
/// - [`Error::Io`] — the output could not be written.
/// - [`Error::OutputRejected`] — the document burrow produced is not the one it promised: the
///   wrong number of pages, or a page displaying at a rotation it did not arrive with. It is
///   returned **instead of** the output, which is dropped (ADR 0022).
/// - [`Error::Internal`] — a number did not fit, or the engine returned something impossible.
///
/// A document that does not get smaller is **not** an error. It is
/// [`Outcome::NotSmaller`], because nothing went wrong: the file was already efficiently
/// stored, which is a fact about the file rather than a failure of the operation.
pub fn compress<E: DocumentCompressor + OutputReader>(
    engine: &E,
    bytes: Box<[u8]>,
    options: &OpenOptions<'_>,
) -> Result<Outcome> {
    // MEASURED BEFORE THE ENGINE TAKES THE BUFFER, because after `open` it is gone -- the seam
    // takes its bytes by value, since the web path copies them into a separate heap and has
    // nothing to borrow. A length is eight bytes; the alternative is keeping the whole input.
    let original_bytes = u64::try_from(bytes.len())
        .map_err(|_| Error::Internal("input length does not fit in u64".to_owned()))?;

    let limits = options.limits;
    let clock = Arc::clone(&options.clock);

    let source = engine.open(bytes, options)?;
    let total = engine.pages(&source)?;

    // THE CEILING HERE TOO, not only in the engine. `split` and `rotate` both check `max_pages`
    // in the operation as well as in the engine, and their comments record why: a ceiling that
    // lives in exactly one place is a ceiling one engine can forget. Unlike `rotate` there is
    // no second, caller-chosen count to check -- compression takes no page selection.
    Limits::check(Stage::PageCount, "max_pages", total, limits.max_pages)?;

    // One deadline for the whole operation, started after the input is open -- the open has its
    // own inside the engine. Checked around the sweep and around the write, which is the only
    // granularity available: the work is engine calls and no engine here offers a timeout, a
    // cancellation or an abort hook (ADR 0007).
    let deadline = Deadline::start(clock.as_ref(), &limits);

    // WHAT THE OUTPUT MUST DISPLAY, computed BEFORE the operation runs (ADR 0022).
    //
    // Compression's promise is the input's own vector, UNCHANGED: it may not move a page,
    // reattribute one, or alter what any page displays at. So unlike `rotate` there is nothing
    // to derive -- the promise IS the reading, which makes it the cheapest promise of the four
    // and, as `verify::Expected::Compressed` says at length, the one that leaves the most
    // undetectable.
    //
    // ONE SWEEP, in the engine, which checkpoints per page against THIS deadline: the walk is
    // one `/Parent` climb per page, so it is sized by page count times tree depth -- both
    // attacker-chosen.
    let promised = DocumentCompressor::rotations(engine, &source, options, &deadline)?;
    deadline.checkpoint(clock.as_ref())?;

    let produced = engine.compress(&source, options)?;
    deadline.checkpoint(clock.as_ref())?;

    let produced_bytes = u64::try_from(produced.len())
        .map_err(|_| Error::Internal("output length does not fit in u64".to_owned()))?;

    // REFUSE TO MAKE IT WORSE, and the comparison comes BEFORE the verification.
    //
    // That ordering is deliberate rather than an optimisation. ADR 0022 puts verification
    // between the engine and the CALLER; a document that is not going to the caller has no
    // caller to protect, so verifying it would be paying roughly the cost of the operation
    // again to check bytes that are about to be dropped. What the caller receives in this
    // branch is two numbers it measured itself.
    //
    // `>=` RATHER THAN `>`. A re-encoding that lands on exactly the input's size achieved
    // nothing, and returning it as a compressed document would be claiming a saving of zero
    // bytes as a result. Equality is not rare: 5.3% of qpdf's own corpus.
    if produced_bytes >= original_bytes {
        drop(produced);
        return Ok(Outcome::NotSmaller {
            original_bytes,
            produced_bytes,
        });
    }

    // THE INPUT DOCUMENT GOES BEFORE THE OUTPUT IS PARSED. Verification holds a second parsed
    // document plus a copy of the output bytes, and holding the source across it puts both in
    // memory at once for no purpose -- `max_memory_bytes` only detects, and the web module's
    // fixed 2 GiB is the only real bound anywhere. Security review measured RSS 197 MB -> 387 MB
    // across the read-back of a 200 MB output on `rotate`.
    drop(source);

    // AND THE LAST THING BEFORE THE CALLER HAS IT. Reopened through a fresh engine, never the
    // handle that just wrote it. See `verify`'s header and ADR 0022.
    verify::output(
        engine,
        &produced,
        &verify::Expected::Compressed {
            rotations: promised,
        },
        options,
        &deadline,
    )?;

    Ok(Outcome::Smaller {
        document: produced,
        original_bytes,
    })
}
