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

use burrow_engines::{OpenOptions, PageRotator};
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
/// - [`Error::Internal`] — a number did not fit, or the engine returned something impossible.
pub fn rotate<E: PageRotator>(
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
    let output = engine.rotate(&source, &indices, rotation, options)?;
    deadline.checkpoint(clock.as_ref())?;

    Ok(output)
}
