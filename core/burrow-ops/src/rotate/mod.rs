//! Turn chosen pages of a document, and emit the document.
//!
//! # What a rotation is here
//!
//! **Relative, per page, and nothing else changes.** A page displaying at 90 that is turned
//! another 90 ends at 180. Pages not named are untouched. No page is added, removed or
//! reordered, and no page's content is re-encoded — `/Rotate` is an attribute of the page
//! dictionary, and changing an attribute is all this does.
//!
//! # It is not a subsetting operation, and the ROADMAP records why that matters
//!
//! [ADR 0019](../../../../docs/adr/0019-how-split-builds-its-outputs.md) §2's rule — an output
//! that is a subset of its input must carry nothing derived from the excluded pages — does not
//! apply, because nothing is excluded. The obligation is the inverse and is tested as such:
//! `tests/rotate_keeps_everything.rs` uses the shared object-closure harness in its
//! `assert_nothing_lost` form, and additionally asserts every page's content stream comes out
//! byte-identical. A rotation that re-encoded content would still "work"; it would just
//! quietly resample somebody's scan.
//!
//! # Inheritance is the whole difficulty
//!
//! `/Rotate` is an inheritable page attribute, so a page with no `/Rotate` of its own displays
//! at whatever the nearest ancestor in the page tree says. Two consequences, both of which are
//! failures if got wrong rather than fidelity losses:
//!
//! - reading a page's rotation means walking up `/Parent`. Reading the page dictionary alone
//!   reports 0 for every document that sets the key on a `/Pages` node, and then "rotate by
//!   90" silently *undoes* an inherited quarter turn instead of adding to it;
//! - writing a page's rotation means writing to the page dictionary. Writing to the ancestor
//!   the value was read from rotates every page under it, and reports success.
//!
//! Both live in the engine ([`burrow_engines::PageRotator`]), because both need the page tree.
//! `rotating_one_page_leaves_every_other_page_alone_including_ones_that_inherit`, in
//! `burrow-engines`, measures the second on a document whose pages inherit.
//!
//! # Limits
//!
//! | limit | where it applies |
//! |---|---|
//! | `max_input_bytes` | the input, once, at [`Stage::InputSize`] |
//! | `max_pages` | the input's page count, and the number of pages named, at [`Stage::PageCount`] |
//! | `max_duration_ms` | the whole operation, on the injected clock, checkpointed around the rotation |
//! | `max_memory_bytes` | **detected, never bounded** (ADR 0007) |
//!
//! There is one input and one output, so unlike `merge` there is no aggregate-versus-per-item
//! question: a document cannot be under a ceiling collectively and over it individually.

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::verify;
use burrow_engines::{OpenOptions, OutputReader, PageRotator};
use burrow_types::{Deadline, Error, Limits, Result, Rotation, Stage};

/// Which pages to turn.
///
/// One-based, because that is how a person names a page and this is the boundary a person's
/// request arrives at. The engine seam is zero-based; the conversion happens once, here.
#[derive(Debug, Clone, Copy)]
pub struct Pages<'a> {
    numbers: &'a [u64],
}

impl<'a> Pages<'a> {
    /// Turn these one-based page numbers.
    ///
    /// Validation happens in [`rotate`], where the page count is known: page 11 of a ten-page
    /// document is only wrong once there is a document.
    #[must_use]
    pub const fn numbered(numbers: &'a [u64]) -> Self {
        Self { numbers }
    }

    /// How many pages are named. Not how many distinct pages — repeats are refused, not
    /// counted, and refusing them is [`rotate`]'s job.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.numbers.len()
    }

    /// Whether no page is named at all.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.numbers.is_empty()
    }
}

/// Turn `pages` of `bytes` by `degrees`, and return the document.
///
/// `degrees` is normalised by [`Rotation::from_degrees`]: any multiple of 90 is accepted,
/// negative or over 360, and reduced to one of four turns. Anything else is refused before
/// the document is opened — a bad argument should not cost a parse.
///
/// # Errors
///
/// - [`Error::InvalidArgument`] — `degrees` is not a multiple of 90; no page is named; a page
///   number is zero, past the end, or named twice.
/// - [`Error::Malformed`], [`Error::Unsupported`], [`Error::PasswordRequired`] — the input
///   could not be read, or a page's existing `/Rotate` is not an integer multiple of 90.
/// - [`Error::LimitExceeded`] — a ceiling in `options.limits` was reached.
/// - [`Error::Io`] — the output could not be written.
/// - [`Error::OutputRejected`] — the document burrow produced is not the one it promised:
///   the wrong number of pages, or a page displaying at a rotation nobody asked for. It is returned **instead of** the output, which is
///   dropped (ADR 0022).
/// - [`Error::Internal`] — a number did not fit, or the engine returned something impossible.
pub fn rotate<E: PageRotator + OutputReader>(
    engine: &E,
    bytes: Box<[u8]>,
    pages: Pages<'_>,
    degrees: i64,
    options: &OpenOptions<'_>,
) -> Result<Vec<u8>> {
    // THE ARGUMENT BEFORE THE DOCUMENT. `from_degrees` rejects a non-multiple of 90, and doing
    // it here means a caller that meant `rotate(45)` finds out without an untrusted file
    // having been parsed on their behalf.
    let rotation = Rotation::from_degrees(degrees)?;

    if pages.is_empty() {
        return Err(Error::InvalidArgument("no pages to rotate".to_owned()));
    }

    let limits = options.limits;
    let clock = Arc::clone(&options.clock);

    let source = engine.open(bytes, options)?;
    let total = engine.pages(&source)?;

    // THE CEILING HERE TOO, not only in the engine. `split/mod.rs` checks `max_pages` itself
    // even though its engine also does, and its comment records why: a ceiling that lives in
    // exactly one place is a ceiling one engine can forget. The rustdoc table above says
    // `max_pages` applies to "the input's page count, and the number of pages named", and
    // until code review it was true of neither in this crate.
    let named = u64::try_from(pages.len())
        .map_err(|_| Error::Internal("page count does not fit in u64".to_owned()))?;
    Limits::check(Stage::PageCount, "max_pages", total, limits.max_pages)?;
    Limits::check(Stage::PageCount, "max_pages", named, limits.max_pages)?;

    // One-based to zero-based, with the range and repeat checks on the way. Page 0 is refused
    // rather than treated as page 1: a caller that is off by one should hear about it, and a
    // caller passing a zero-based index by mistake would otherwise rotate the wrong page.
    let mut indices = Vec::with_capacity(pages.len());
    let mut seen = BTreeSet::new();
    for &number in pages.numbers {
        if number == 0 {
            return Err(Error::InvalidArgument(
                "pages are numbered from one".to_owned(),
            ));
        }
        if number > total {
            return Err(Error::InvalidArgument(
                "page is not in the document".to_owned(),
            ));
        }
        // A page named twice would be rotated twice, which is a silently different document
        // from the one the caller asked for. Refused rather than deduplicated: the caller
        // meant something, and guessing which is worse than saying so. The engine refuses it
        // too; both, for the reason the ceiling above is checked twice.
        if !seen.insert(number) {
            return Err(Error::InvalidArgument(
                "the same page is named more than once".to_owned(),
            ));
        }
        indices.push(number - 1);
    }

    // One deadline for the whole operation, started after the input is open -- the open has
    // its own inside the engine. Checked around the rotation, which is the only granularity
    // available: the work inside `rotate` is engine calls, and no engine here offers a
    // timeout, a cancellation or an abort hook (ADR 0007). Saying so is better than implying
    // a limit checked more often than it is.
    let deadline = Deadline::start(clock.as_ref(), &limits);
    deadline.checkpoint(clock.as_ref())?;

    // WHAT THE OUTPUT MUST DISPLAY, computed BEFORE the operation runs (ADR 0022).
    //
    // From the input's own rotations, read through the handle that is about to be edited --
    // which is the only place they exist. Every page's expected value is its current one, plus
    // the turn for the pages that were named. So this is a statement about the request rather
    // than about the answer, which is what makes comparing it to the answer worth anything.
    // ONE SWEEP, in the engine, which checkpoints per page: the walk is one `/Parent` climb
    // per page, so it is sized by page count times tree depth -- both attacker-chosen -- and
    // security review measured it at 147 ms against 28 ms of edit-and-write on a 10,000-page
    // document with a 60-deep tree. Calling `effective_rotation` in a loop from here was the
    // same work through N trait calls, with the checkpoint on the wrong side of the seam.
    let before = PageRotator::rotations(engine, &source, options, &deadline)?;

    let mut promised = Vec::with_capacity(before.len());
    for (at, current) in before.iter().enumerate() {
        let number = u64::try_from(at)
            .map_err(|_| Error::Internal("page index does not fit in u64".to_owned()))?
            + 1;
        // NORMALISED ONLY WHERE A TURN IS APPLIED. `before` records what each page displays at
        // AS WRITTEN, so an out-of-spec `/Rotate 45` is carried through as 45 -- a witness only
        // has to be stable, and a page nobody named is not this operation's business.
        //
        // On a page that WAS named, a turn of an out-of-spec value is undefined, so
        // `from_degrees` refuses it and the operation fails. That refusal is the one this
        // function's `# Errors` has always promised; the regression was applying it to every
        // other page as well, which failed a whole operation over a page it was not touching.
        let turned = if seen.contains(&number) {
            // THE ENGINE'S OWN EXPRESSION, not a re-derivation of it: both qpdf modules write
            // `rotation.after(effective_rotation(page))`, so the promise is that, and the two
            // cannot drift.
            //
            // IT ALSO CANNOT OVERFLOW, which the first version could. `current` is now the raw
            // `/Rotate` from the file rather than one of four quarter turns, and
            // `from_degrees` accepts ANY multiple of 90 -- so `current + turn` on a
            // `/Rotate 9223372036854775710` panicked under `overflow-checks` (every test
            // build, and cargo-fuzz's, which drives this function directly) and wrapped in
            // release. Reproduced against real qpdf by security review. Normalising first
            // bounds both sides to a quarter turn before any arithmetic.
            //
            // MALFORMED, NOT INVALID ARGUMENT. The number `from_degrees` refuses here is the
            // document's own `/Rotate`, not the caller's argument -- and the old
            // `InvalidArgument` carried it into the message verbatim, which is file content in
            // an error string. The variant is matched rather than mapped wholesale so an
            // `Internal` from the normaliser stays `Internal`.
            let base = match Rotation::from_degrees(*current) {
                Ok(base) => base,
                Err(Error::InvalidArgument(_)) => {
                    return Err(Error::Malformed(
                        "a page's /Rotate is not a multiple of 90".to_owned(),
                    ));
                }
                Err(other) => return Err(other),
            };
            rotation.after(base).degrees()
        } else {
            *current
        };
        promised.push(turned);
    }
    deadline.checkpoint(clock.as_ref())?;

    let output = engine.rotate(&source, &indices, rotation, options)?;
    deadline.checkpoint(clock.as_ref())?;

    // THE INPUT DOCUMENT GOES BEFORE THE OUTPUT IS PARSED. Verification holds a second parsed
    // document plus a copy of the output bytes, and holding the source across it puts both
    // documents in memory at once for no purpose -- `max_memory_bytes` only detects, and the
    // web module's fixed 2 GiB is the only real bound anywhere. Found by security review,
    // which measured RSS 197 MB -> 387 MB across the read-back of a 200 MB output.
    drop(source);

    // AND THE LAST THING BEFORE THE CALLER HAS IT. Reopened through a fresh engine, never the
    // handle that just wrote it. See `verify`'s header and ADR 0022.
    verify::output(
        engine,
        &output,
        &verify::Expected::Rotated {
            rotations: promised,
        },
        options,
        &deadline,
    )?;

    Ok(output)
}
