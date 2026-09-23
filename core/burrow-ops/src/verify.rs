//! What an operation promised, checked against what it produced.
//!
//! **What this guarantees, precisely: the bytes an operation produced match what it believed
//! when it produced them. NOT that the output matches the document the caller gave it.** Every
//! `Expected` value below is computed from the operation's own reading of its input, so a
//! reading that was already wrong is compared against itself and agrees. The output side is a
//! neutral witness; the input side is not, and nothing here makes it one. See the fourth
//! property below and `#111`.
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
//! # Four properties, and the fourth is the one that is NOT independent
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
//! - **The promise itself is read from the operation's own source**, and this is the limit of
//!   the three above. `rotate`, `reorder` and `compress` compute it from the source handle;
//!   `split` from a sweep over its own source; `merge` from the running totals of the assembly
//!   it is building. So a wrong page count going in is a wrong page count on both sides of the
//!   comparison. Measured on `five-pages-or-six.pdf`, which declares six pages, is read as five
//!   by qpdf, and splits into five with this check agreeing. `#111` tracks closing it; the
//!   `#[ignore]`d `a_split_keeps_every_page_the_document_declares` states the property.
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

    /// This part holds the run of source pages it was cut from, displaying as `rotations` says.
    ///
    /// **`split`** computes this once, from the source, before any part is extracted: the source's
    /// rotation vector, sliced per run. Each part is verified against its own slice.
    ///
    /// # Why a slice per part rather than a page count per part
    ///
    /// ADR 0022's table originally wrote split's promise as "each part's page count, **and** the
    /// parts summing to the input's". The slice is strictly stronger and subsumes both: if each
    /// part's page count equals its slice's length, and the slices partition the source vector,
    /// then the parts sum to the input by construction — there is nothing left for a separate
    /// assertion to catch. What the slice adds is *which* pages: a part built from the wrong run
    /// moves its rotations, and a count cannot see that.
    ///
    /// It costs one sweep over the source, paid once for the whole split rather than per part.
    /// On a flat 10,000-page document that is 5.5 ms against a 91.6 ms split; on a 60-deep page
    /// tree it is 148.6 ms, because the sweep walks `/Parent` per page. ADR 0019's cost table has
    /// the composition.
    ///
    /// # Undetectable, and the first one is the reason `split` has a second layer
    ///
    /// - **Wrong content on a correctly-numbered, correctly-rotated page.** A part could hold the
    ///   right count of pages displaying the right way and the wrong drawings entirely.
    /// - **Anything about what the part carries from pages it excluded.** This checks a shape, not
    ///   a closure: `split`'s whole subject is objects that should not have travelled, and a
    ///   rotation vector says nothing about them. That is ADR 0019 §2's rule, and what holds it is
    ///   `subset_closure.rs` and `split_no_leak.rs` — not this.
    /// - **A partition of pages that all display the same way**, where it degrades to per-part page
    ///   counts. That still catches a part that lost or gained a page; it says nothing about which
    ///   run it came from. The same asymmetry `Reordered` has, and it is why the conformance
    ///   fixtures are built with distinguishable pages.
    Split {
        /// The source's `/Rotate` values for this part's run, in page order.
        rotations: Vec<i64>,
    },

    /// The same pages came out as went in, displaying exactly as they did before.
    ///
    /// **`compress`** computes this from the input's own rotations, read before the
    /// re-encoding. It is the input's vector **unchanged** — compression may not move a page,
    /// reattribute one, or alter what any page displays at. That makes it the strictest promise
    /// here in one narrow sense and the weakest in another, and both halves need saying.
    ///
    /// # Why this is a separate variant and not `Rotated` with an unchanged vector
    ///
    /// ADR 0022 pre-declared that compress "promises what `rotate` does", and as a *predicate*
    /// that is right — the same page count, the same vector. Reusing `Rotated` would check
    /// exactly the same thing.
    ///
    /// What differs is the **residue**, and `Rotated`'s rustdoc does not state compress's. A
    /// rotation changes one attribute of a page dictionary and cannot touch a content stream;
    /// compression re-encodes how every object in the document is stored. So the set of wrong
    /// outputs that pass this check is much larger here, and a variant whose documented residue
    /// belongs to a different operation is a variant somebody will over-trust. ADR 0022's own
    /// step 3 asks for a new variant exactly when no existing one states what yours leaves
    /// undetectable.
    ///
    /// # Undetectable, and the list is longer than any other variant's
    ///
    /// - **Any change to what a page contains.** A content stream re-encoded wrongly, an image
    ///   resampled, a font dropped or subsetted — every one of them leaves the page count and
    ///   the rotation vector identical. This is the whole of what compression touches and
    ///   almost none of it is visible here.
    /// - **A document that is smaller because something was thrown away.** The check cannot
    ///   tell a well-packed document from a lossy one; what makes `compress` lossless is that
    ///   the engine sets one storage lever and touches no content, not that this caught
    ///   anything.
    /// - The residues `Rotated` has: a value written to a shared ancestor rather than a page,
    ///   on a document where every page inherits the same one.
    ///
    /// **So this variant carries the least of any of them, and the tests carry the rest.**
    /// `compress_keeps_everything.rs` uses the object-closure harness's `assert_nothing_lost`
    /// entry point and asserts every page's content stream comes out byte-identical — which is
    /// the real check, and which is a test rather than a runtime one because ADR 0022 rejected
    /// decompressing every output in production.
    Compressed {
        /// Every page's `/Rotate` as written, in page order, read before the re-encoding.
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

    /// The same pages came out as went in, displaying as `rotations` says, and the region that
    /// was asked about no longer has glyphs in it.
    ///
    /// # The name is the promise, and `Redacted` would have been a different one
    ///
    /// **`RegionCleared` says what was checked.** A variant called `Redacted` would be read as
    /// *the secret is gone*, and that is the one thing this cannot say: the secret may survive
    /// in the embedded font program, on the unreachable catalogue, or as pixels — all of which
    /// ADR 0029 §7 **discloses** precisely because no read-back can turn them into assertions.
    ///
    /// # What this variant covers here, and what covers the rest
    ///
    /// This one carries the half every operation shares: the page count and the `/Rotate`
    /// vector. The region check itself is `burrow_engines::redact_verify`, which needs the
    /// page's glyphs, its fonts' mappings and its dictionary keys — three reads no other
    /// operation has ever wanted, so widening [`OutputReader`] for them would make four
    /// implementors grow methods they never call.
    ///
    /// **`redact` cannot return bytes without both.** The engine's only path to a `Vec<u8>`
    /// runs the region check, and this runs after it.
    ///
    /// **Undetectable:** everything §6 and `redact_verify`'s header list — that the secret is
    /// absent, that the font no longer describes the removed run, that the region is visually
    /// blank, that an annotation intersecting the region is gone, and that text outside the
    /// region is untouched beyond these two numbers. A page whose text reflowed has the same
    /// count and the same rotation.
    RegionCleared {
        /// Every page's `/Rotate` as written, in page order, read before the redaction.
        rotations: Vec<i64>,
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
            Self::Rotated { rotations }
            | Self::Reordered { rotations }
            | Self::Split { rotations }
            | Self::Compressed { rotations }
            | Self::RegionCleared { rotations } => {
                u64::try_from(rotations.len()).unwrap_or(u64::MAX)
            }
            Self::Merged { contributions } => contributions.iter().sum(),
        }
    }

    /// The rotation vector the output must have, where the operation can know it.
    const fn rotations(&self) -> Option<&Vec<i64>> {
        match self {
            Self::Rotated { rotations }
            | Self::Reordered { rotations }
            | Self::Split { rotations }
            | Self::Compressed { rotations }
            | Self::RegionCleared { rotations } => Some(rotations),
            // See the variant's rustdoc: knowing it would cost a second parse of every input.
            Self::Merged { .. } => None,
        }
    }

    /// What the operation is called, for the message.
    const fn operation(&self) -> &'static str {
        match self {
            Self::Rotated { .. } => "rotate",
            Self::Reordered { .. } => "reorder",
            Self::Split { .. } => "split",
            Self::Compressed { .. } => "compress",
            Self::RegionCleared { .. } => "redact",
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
/// **`Display`, not `Debug`, and the difference is 4,064 bytes of brotli** in every user's
/// first-load payload. `{error:?}` instantiates `Debug` for the whole `Error` enum and drags
/// in the formatting machinery behind it; `{error}` uses the `Display` `thiserror` already
/// generates, which was measured at 75 bytes more than dropping the detail altogether. It also
/// reads better: `malformed: …` rather than `Malformed("…")`. Found because the size budget
/// went red in CI — the local sweep had been staging a wasm binary built before any of this.
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
        // THE NUMBERS ARE INTEGERS THE ENGINE READ FROM THE PAGE TREE, not bytes of any
        // stream — and that is a narrower claim than this comment used to make.
        //
        // It said "a rotation is a multiple of 90 the engine computed". It is not: the sweep
        // is RECORDED, NOT JUDGED (`rotations_of` → `declared_rotation`), precisely so that an
        // out-of-spec `/Rotate 45` on a page nobody named does not fail a whole operation. So
        // a document's own `/Rotate 123456789` reaches this string verbatim as an `i64`.
        //
        // What holds, and what the message rests on: these are `i64`s the engine parsed out of
        // a page dictionary, never text and never stream content, so no string from the file
        // can reach a caller through here. The values are attacker-INFLUENCED, in a channel a
        // few integers wide, on a rejection, to a caller that already holds the document.
        //
        // Recorded rather than reworded away, because `core/CLAUDE.md` treats an overclaiming
        // comment as a bug and this one was load-bearing for three operations before
        // `Expected::Compressed` made it four. Found by security review.
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
/// **`max_memory_bytes` is raised the same way, and for the same reason**, since #26 put the
/// length-based estimate on the qpdf paths. Before that the read-back had no size estimate at
/// all and this could not fire. Now it can: a caller sets `max_memory_bytes` to 256 MiB and
/// merges ten 50 MiB files, each of which passes its own estimate at 78.5 MiB -- and the
/// read-back sees a ~500 MiB output, estimates 641 MiB, and rejects the finished merge. That
/// is the *identical* defect the paragraph above records for `max_input_bytes`, arriving
/// through the adjacent field, and it is worse in one way: the `LimitExceeded` it produces
/// carries `stage: SizeEstimate`, exactly like the one the INPUT pre-check emits, so nothing
/// in the typed error tells a page which of the two happened. `merge-messages.ts` keys on
/// `limit`, so a person would be told their file was too large. Found by security review.
///
/// It is raised only to what this read-back needs -- `estimated_open_bytes(size)` -- rather
/// than removed, so an output that is genuinely enormous is still refused.
///
/// `max_pages` and `max_duration_ms` are left exactly as they are. `max_pages` still bites --
/// an output over the ceiling is a real refusal -- and the deadline is the operation's own,
/// passed in rather than restarted.
///
/// The password is carried over: qpdf preserves a source document's encryption on write, so
/// an encrypted input produces an encrypted output, and dropping the password here would turn
/// every encrypted document into `PasswordRequired` on the way back.
fn read_back_options<'a>(options: &OpenOptions<'a>, produced: usize) -> Result<OpenOptions<'a>> {
    let size = u64::try_from(produced)
        .map_err(|_| Error::Internal("output length does not fit in u64".to_owned()))?;

    let mut limits = options.limits;
    limits.max_input_bytes = limits.max_input_bytes.max(size);
    limits.max_memory_bytes = limits
        .max_memory_bytes
        .max(burrow_engines::estimated_open_bytes(size));

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
        "{}: burrow produced a document {what} ({error})",
        expected.operation()
    ))
}
