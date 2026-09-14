//! What an operation promised, checked against what it produced.
//!
//! [ADR 0022](../../../docs/adr/0022-every-operation-verifies-its-own-output.md). An operation
//! here does not return bytes; it returns bytes **that have been checked**, and this is the
//! last thing between the engine and the caller.
//!
//! **The links in this header are code spans rather than intra-doc links, deliberately.**
//! `burrow-core` re-exports this crate as `ops`, and rustdoc re-resolves a re-exported
//! module's docs in *that* crate's scope — where `burrow_engines` is not a dependency and
//! `output` is not in scope. A link that only resolves in the crate it was written in is a
//! `cargo doc` failure in the crate that re-exports it, which is how CI found this.
//!
//! # Three properties, each load-bearing
//!
//! - **It runs on the emitted bytes**, not on a source, a copy, or the engine's in-memory
//!   state. That is ADR 0006's R10 exactly, and it is why `output` takes `&[u8]` rather than
//!   a handle.
//! - **It runs before the caller has the bytes.** The operation returns `Err` and drops the
//!   buffer, so nothing downstream ever sees an unverified document — R9's property, at the
//!   core layer where it holds for the native paths too rather than only inside a worker.
//! - **It reads through a FRESH engine**, never the one that produced the bytes. A document
//!   handle that has just been edited is not a neutral witness to its own output: it holds a
//!   parsed page tree it built, and an engine in a bad state can agree with itself. See
//!   `OutputReader::fresh` for what "fresh" does and does not mean on each platform.
//!
//! # The witness is each page's `/Rotate`, as written
//!
//! A page count cannot see a permutation, so the check needs something per page. What the
//! engine seam already offers on both platforms, without a new bridge method and without
//! decompressing anything, is the value a page displays at, followed up the page tree —
//! `PageRotator::rotations`.
//!
//! **Recorded, not judged.** The value is the one in the file: an out-of-spec `/Rotate 45`
//! reads back as 45. `effective_rotation` is the *judging* read and refuses that, because a
//! turn applied to 45 is undefined — but a witness only has to be **stable** between the read
//! before an operation and the read after it, and normalising it made a page nobody named fail
//! a whole reorder.
//!
//! It is a **weak identity and a real one**. Two pages sharing a rotation are
//! indistinguishable to it, so on a document where every page displays the same way the vector
//! degrades to a page count. On a document where they differ it catches a permutation that is
//! not the one asked for, a merge that dropped an input, and a rotation applied to the wrong
//! page. That asymmetry is stated per operation in `Expected` rather than left to be
//! discovered, and it is the whole of what this module claims.
//!
//! # What it cannot detect, in every case
//!
//! - **Wrong content on a right-numbered, right-rotated page.** A page displaying somebody
//!   else's drawing is invisible here.
//! - **Sub-object leaks** — an object shared by an included and an excluded page (#54).
//! - **Anything present only in transformed form**: a font subset still carrying glyphs for
//!   removed characters, an image's pixels. `qpdf --qdf` does not decode `/DCTDecode`,
//!   `/JPXDecode` or `/JBIG2Decode` and neither does this. **For redaction that transformed
//!   form *is* the leak**, so M2 must not inherit this as though it were sufficient.

use std::sync::Arc;

use burrow_engines::{OpenOptions, OutputReader};
use burrow_types::{Deadline, Error, Result};

/// What an operation promises about the document it produced.
///
/// **The three are not equally strong, and the difference is the point of them being three.**
/// Each variant carries what its operation can honestly know *without parsing its input a
/// second time*, and each one's rustdoc says what that leaves undetectable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expected {
    /// The same pages came out as went in, displaying as `rotations` says.
    ///
    /// **`rotate`** computes this from the input's own rotations plus the turn that was asked
    /// for, read through the handle that is about to be edited — the only place they exist,
    /// and free, because rotate's source is already a rotator's source. So it catches a turn
    /// applied to a page nobody named: the failure both rotate modules call their worst.
    ///
    /// The turn is normalised **only** on the pages the request names, which is also the only
    /// place an out-of-spec `/Rotate` is refused; every other page's value is carried through
    /// as written.
    ///
    /// **Undetectable:** a rotation written to a shared ancestor rather than to the page, on a
    /// document where every page inherits the same value — the vector is identical either way.
    /// `the_rotation_is_written_to_the_page_and_never_to_an_ancestor` covers that, and needs a
    /// fake to do it.
    Rotated {
        /// Every page's `/Rotate` as written, in page order — recorded, not judged.
        rotations: Vec<i64>,
    },

    /// The same pages came out as went in, permuted by the order that was asked for.
    ///
    /// **`reorder`** computes this as the input's rotations read through the permutation. A
    /// permutation cannot change what a page displays, only where it is, so any permutation
    /// that puts differently-rotated pages in different places from the one requested is
    /// caught.
    ///
    /// **Undetectable:** a permutation of pages that all display the same way. There it
    /// degrades to the page count — which still catches a page lost or duplicated, the failure
    /// #61 is about, and says nothing about order.
    Reordered {
        /// Every page's `/Rotate` as written, in the order the permutation asked for.
        rotations: Vec<i64>,
    },

    /// Every input contributed the pages it had, in order.
    ///
    /// **`merge`** records how many pages each input added, taken from the assembly's running
    /// total before and after each append. That is free, and it is **input-page presence**: an
    /// input dropped entirely, truncated, or appended twice moves its entry.
    ///
    /// # Why merge gets a count per input and not a rotation vector
    ///
    /// The expected rotations would have to come from the inputs, independently — and the only
    /// way to get them is to parse every input **a second time**, doubling the cost of the
    /// operation this check is attached to. Reading them from the *assembly* instead would be
    /// free and worthless: that is the engine state that produced the output, so it is the
    /// instance agreeing with itself, which is the thing `OutputReader::fresh` exists to stop.
    ///
    /// # A zero-page input is refused, deliberately
    ///
    /// The deltas are all merge has, so an input that *had* no pages and one whose pages were
    /// dropped are the same entry, and this check calls both a failure. That is the direction
    /// to be wrong in: a zero-page page tree is malformed by the specification anyway, and the
    /// alternative — parsing each input again to tell the two apart — is the cost this variant
    /// exists to avoid. The message says "contributed no pages" rather than "was dropped" for
    /// exactly that reason.
    ///
    /// **Undetectable, and this is the largest residue of the three:** two inputs' pages
    /// interleaved wrongly, or one input's pages substituted for another's, as long as the
    /// counts land the same. Merge is the operation where this check is weakest, which is the
    /// opposite of where the pressure was when ADR 0022 was written — worth knowing.
    Merged {
        /// How many pages each input added, in the order they were appended.
        contributions: Vec<u64>,
    },
}

impl Expected {
    /// The page count this promise implies.
    ///
    /// A vector longer than `u64::MAX` saturates rather than casting. It cannot happen -- the
    /// vector has one entry per page and `max_pages` is checked long before -- and `as u64`
    /// was still wrong here: `core/CLAUDE.md` denies casts on numbers a document can
    /// influence, and every neighbouring conversion in this crate is a `try_from`.
    fn pages(&self) -> u64 {
        match self {
            Self::Rotated { rotations } | Self::Reordered { rotations } => {
                u64::try_from(rotations.len()).unwrap_or(u64::MAX)
            }
            Self::Merged { contributions } => contributions.iter().sum(),
        }
    }

    /// The rotation vector the output must have, where the operation can know it.
    const fn rotations(&self) -> Option<&Vec<i64>> {
        match self {
            Self::Rotated { rotations } | Self::Reordered { rotations } => Some(rotations),
            // See the variant's rustdoc: knowing it would cost a second parse of every input.
            Self::Merged { .. } => None,
        }
    }

    /// What the operation is called, for the message.
    const fn operation(&self) -> &'static str {
        match self {
            Self::Rotated { .. } => "rotate",
            Self::Reordered { .. } => "reorder",
            Self::Merged { .. } => "merge",
        }
    }
}

/// Reopen `output` through a fresh engine and refuse it if it is not what was promised.
///
/// # Errors
///
/// - [`Error::OutputRejected`] — the output does not match the promise. The message names the
///   operation and the two page counts; **both are counts this crate computed or an engine
///   reported, and neither is derived from file content**, so naming them is safe.
/// - Whatever the engine reports if the output cannot be reopened at all, wrapped the same
///   way: a document burrow just wrote and cannot read back is a rejected output, not a
///   malformed input, and calling it `Malformed` would blame the user's file.
///
/// # On including the wrapped error's `Debug`
///
/// It is included rather than dropped because "cannot read back its own output" is the one
/// case where the reason is the whole diagnosis. What makes that safe is that every message
/// `burrow-engines` constructs is a fixed constant or a constant plus a number it computed —
/// see `core/burrow-engines/src/codes/` and `core/burrow-engines/tests/secret_leak.rs`.
///
/// **Not** `core/burrow-engines/tests/properties.rs`, which an earlier version of this
/// paragraph cited: that file is PDFium-only, and the only `OutputReader` there is runs on
/// qpdf. A citation that does not support its claim is the bug, even when the claim holds.
pub fn output<E: OutputReader>(
    engine: &E,
    bytes: &[u8],
    expected: &Expected,
    options: &OpenOptions<'_>,
    deadline: &Deadline,
) -> Result<()> {
    // THE OPERATION'S DEADLINE, NOT A NEW ONE. Verification is part of the operation, so it
    // spends the operation's budget: giving it a fresh `Deadline` would let a document take
    // `max_duration_ms` to produce and `max_duration_ms` again to check, and the caller was
    // promised one. Checkpointed around each engine call, which is the granularity available
    // here -- the per-page granularity is inside `rotations`. Both were missing until security
    // review measured the read-back at 174 ms outside every deadline on a 10,000-page
    // document, against 12 ms of edit-and-write inside one.
    let clock = Arc::clone(&options.clock);
    deadline.checkpoint(clock.as_ref())?;

    let reading = read_back_options(options, bytes.len())?;

    // A FRESH ENGINE. Not `engine` -- see the module header and `OutputReader::fresh`.
    let witness = engine.fresh();

    let read = witness
        .open_output(bytes, &reading)
        .map_err(|error| rejected(expected, "it cannot read back", error))?;

    deadline.checkpoint(clock.as_ref())?;

    let produced = witness
        .page_count(&read)
        .map_err(|error| rejected(expected, "whose pages cannot be counted", error))?;

    if produced != expected.pages() {
        return Err(Error::OutputRejected(format!(
            "{}: {} pages were expected and {produced} came out",
            expected.operation(),
            expected.pages()
        )));
    }

    // EVERY INPUT CONTRIBUTED SOMETHING. Free, and it catches the one thing a total cannot:
    // an input silently dropped while another supplied its pages. Checked before the vector
    // below, because it is a statement about the request and reads better first.
    if let Expected::Merged { contributions } = expected
        && let Some(at) = contributions.iter().position(|pages| *pages == 0)
    {
        return Err(Error::OutputRejected(format!(
            "merge: input {} contributed no pages to the output",
            at + 1
        )));
    }

    let Some(promised) = expected.rotations() else {
        // `merge` promises a count per input and no vector; see `Expected::Merged`.
        return Ok(());
    };

    deadline.checkpoint(clock.as_ref())?;

    let actual = witness
        .rotations(&read, &reading, deadline)
        .map_err(|error| rejected(expected, "whose pages cannot be read", error))?;

    if &actual != promised {
        // THE NUMBERS ARE OURS. A rotation is a multiple of 90 the engine computed from the
        // page tree; nothing here is a byte of the document.
        return Err(Error::OutputRejected(format!(
            "{}: the pages that came out are not the pages that were promised — expected \
             {promised:?}, got {actual:?}",
            expected.operation()
        )));
    }

    Ok(())
}

/// The options the read-back runs under.
///
/// **`max_input_bytes` is raised to fit the output, and nothing else changes.**
///
/// That ceiling is about what a *caller* may hand burrow, and the read-back's input is a
/// document burrow just produced. Applying it unchanged denies a supported operation: a merge
/// of inputs totalling exactly `max_input_bytes` is allowed by `check_total_input_bytes` on
/// purpose, and qpdf's output is normally a few hundred bytes larger than the sum of its
/// inputs -- so the operation succeeded and then refused itself, with a message blaming
/// burrow for a document that was fine. Measured by security review: two 16,204-byte inputs
/// under a 32,408-byte ceiling produced 32,995 bytes and a rejection.
///
/// `max_pages`, `max_memory_bytes` and `max_duration_ms` are left exactly as they are.
/// `max_pages` still bites -- an output over the ceiling is a real refusal -- and the
/// deadline is the operation's own, passed in rather than restarted.
///
/// The password is carried over: qpdf preserves a source document's encryption on write, so
/// an encrypted input produces an encrypted output, and dropping the password here would turn
/// every encrypted document into `PasswordRequired` on the way back.
fn read_back_options<'a>(options: &OpenOptions<'a>, produced: usize) -> Result<OpenOptions<'a>> {
    let size = u64::try_from(produced)
        .map_err(|_| Error::Internal("output length does not fit in u64".to_owned()))?;

    let mut limits = options.limits;
    limits.max_input_bytes = limits.max_input_bytes.max(size);

    // `OpenOptions` is `#[non_exhaustive]`, so it is built through its constructor and then
    // adjusted -- which is the right way round anyway: a field added later arrives at its own
    // default rather than being silently omitted here.
    let mut reading = OpenOptions::new(limits, Arc::clone(&options.clock));
    reading.password = options.password;
    Ok(reading)
}

/// Wrap a failure to read the output back, **except** the ones that are not about the output.
///
/// # `LimitExceeded` passes through unchanged
///
/// Running out of time is the operation's outcome, not a verdict on the document — the same
/// distinction `merge` draws when it refuses to blame the input its clock happened to stop on.
/// Wrapping it produced "burrow produced a document whose pages cannot be read (LimitExceeded
/// …)", which tells a person their file is broken when what happened is that the work did not
/// fit in `max_duration_ms`. The page then offers them the wrong next step.
///
/// Everything else really is a rejected output: a document burrow just wrote and cannot read
/// back is not a malformed input, and calling it `Malformed` would blame the user's file.
fn rejected(expected: &Expected, what: &str, error: Error) -> Error {
    if matches!(error, Error::LimitExceeded { .. }) {
        return error;
    }
    Error::OutputRejected(format!(
        "{}: burrow produced a document {what} ({error:?})",
        expected.operation()
    ))
}
