//! Split one document into several, at chosen page boundaries.
//!
//! # What a split is here
//!
//! A **partition**, not an extraction. `split_at([3, 7])` on a ten-page document gives pages
//! 1-3, 4-7 and 8-10: every input page appears in exactly one output, which is the invariant
//! `docs/ROADMAP.md` states and the property tests assert. There is deliberately no way to ask
//! for a subset that drops pages -- an operation that can silently lose a page is the failure
//! class [ADR 0017](../../../../docs/adr/0017-merge-engine-and-failure-semantics.md) §2 refuses
//! for merge, and it would be worse here because the loss would look like a successful split.
//!
//! "Every N pages" is the same thing said differently, and belongs to the caller: it is a list
//! of boundaries computed from a number. Keeping it out of the core means one rule to test.
//!
//! # An output carries nothing from the pages it excludes
//!
//! [ADR 0019](../../../../docs/adr/0019-how-split-builds-its-outputs.md) §2, and the reason
//! the engine seam is [`PageExtractor`] rather than a method that removes pages. The measured
//! failure: an implementation that kept the source's catalog produced a two-page output
//! carrying the outline titles of all five source pages. It is the property redaction depends
//! on, stated for redaction in [ADR 0006](../../../../docs/adr/0006-wasm-linking-strategy.md)'s
//! R8-R10 -- what matters is the bytes that are emitted, not what the operation meant to emit.
//!
//! `tests/split_no_leak.rs` is what holds this, with a control that deliberately leaks.
//!
//! # Limits
//!
//! | limit | where it applies |
//! |---|---|
//! | `max_input_bytes` | the input, once, at [`Stage::InputSize`] -- there is one input |
//! | `max_pages` | the input's page count, and each output's, at [`Stage::PageCount`] |
//! | `max_duration_ms` | the whole operation, checkpointed **between outputs** -- and one output now includes the pruning pass, which decodes every page's streams |
//! | `max_memory_bytes` | **detected, never bounded** (ADR 0007), measured across the whole split |
//!
//! Unlike `merge`, there is no aggregate-versus-per-item question for size: one input cannot
//! be under a ceiling collectively and over it individually. The page ceiling is checked twice
//! anyway -- once on what came in and once per output -- because an output can never be larger
//! than the input but a caller reading only one of those checks would not know that.

#[cfg(test)]
mod tests;

use std::sync::Arc;

use burrow_engines::{OpenOptions, OutputReader, PageExtractor};
use burrow_types::{Deadline, Error, Result};

use burrow_types::{Limits, Stage};

use crate::verify;

/// Where to cut, as one-based page numbers **after** which a new document starts.
///
/// `[3, 7]` on ten pages means 1-3, 4-7, 8-10. An empty list is a one-way split: the whole
/// document, copied. That is a legitimate request rather than a mistake -- it is what "split
/// this into one part" means, and it is also the identity case the property tests need.
#[derive(Debug, Clone, Copy)]
pub struct Cuts<'a> {
    /// Strictly increasing, each strictly inside the document.
    after: &'a [u64],
}

impl<'a> Cuts<'a> {
    /// Cut after each of these one-based page numbers.
    ///
    /// Validation happens in [`split`], where the page count is known: a cut at page 11 of a
    /// ten-page document is only wrong once there is a document.
    #[must_use]
    pub fn after_pages(after: &'a [u64]) -> Self {
        Self { after }
    }

    /// How many documents this produces.
    #[must_use]
    pub fn parts(&self) -> usize {
        self.after.len() + 1
    }
}

/// Split `bytes` into one document per run of pages.
///
/// # Errors
///
/// - [`Error::InvalidArgument`] -- a cut is out of range, repeated, or out of order.
/// - [`Error::Malformed`], [`Error::Unsupported`], [`Error::PasswordRequired`] -- the input
///   could not be read. A split has one input, so there is no `InputFailed` wrapper: there is
///   nothing to disambiguate.
/// - [`Error::LimitExceeded`] -- a ceiling in `options.limits` was reached.
/// - [`Error::Io`] -- an output could not be written.
/// - [`Error::Internal`] -- a number did not fit, or the engine returned something impossible.
pub fn split<E: PageExtractor + OutputReader>(
    engine: &E,
    bytes: Box<[u8]>,
    cuts: Cuts<'_>,
    options: &OpenOptions<'_>,
) -> Result<Vec<Vec<u8>>> {
    let limits = options.limits;
    let clock = Arc::clone(&options.clock);

    let source = engine.open(bytes, options)?;
    let pages = engine.pages(&source)?;

    let runs = runs_from(cuts, pages)?;

    // One deadline for the whole operation, started after the input is open -- the open has its
    // own inside the engine, and since #54 the sharing sweep `PageExtractor::open` runs is
    // checkpointed against THAT one rather than starting a third.
    //
    // Checked BETWEEN outputs, which is the granularity available: no engine here offers a
    // timeout, a cancellation or an abort hook (ADR 0007). What that overshoot now covers is
    // more than it was. `extract` was one engine call per page; it is now that plus the pruning
    // pass -- per page, an `unparse` and a key sweep, an `/Annots` walk, a `/Properties` and
    // `/XObject` scan, and a DECODE of every content and appearance stream the page reaches.
    // ADR 0019's 2026-09-14 amendment measures it at +33% to +167% of the split. So a single
    // output is a larger unit of unchecked work than this comment used to describe, and saying
    // so is better than implying a limit that is checked more often than it is.
    let deadline = Deadline::start(clock.as_ref(), &limits);

    // WHAT EACH PART MUST DISPLAY, computed BEFORE anything is extracted (ADR 0022): the
    // source's rotation vector, which is then sliced per run. Once for the whole split rather
    // than once per part -- it is a property of the source, and paying it per part would
    // multiply the sweep ADR 0022 measured at 84% of the operation on a deep page tree.
    let before = PageExtractor::rotations(engine, &source, options, &deadline)?;
    if u64::try_from(before.len()).unwrap_or(u64::MAX) != pages {
        // Unreachable: the sweep walks the page count the engine reported. A mismatch would
        // mean the slices below could not partition the vector, and every part's promise would
        // be built from the wrong offsets -- silently, which is the direction that matters.
        return Err(Error::Internal(
            "the engine reported a different page count than it has rotations for".to_owned(),
        ));
    }

    let mut outputs = Vec::with_capacity(runs.len());
    let mut promised = Vec::with_capacity(runs.len());
    for (first, count) in runs {
        deadline.checkpoint(clock.as_ref())?;
        let at = usize::try_from(first)
            .map_err(|_| Error::Internal("page index does not fit in usize".to_owned()))?;
        let run = usize::try_from(count)
            .map_err(|_| Error::Internal("page count does not fit in usize".to_owned()))?;
        let slice = at
            .checked_add(run)
            .and_then(|end| before.get(at..end))
            .ok_or_else(|| {
                Error::Internal("a validated run is not inside the rotation vector".to_owned())
            })?;
        promised.push(slice.to_vec());
        outputs.push(engine.extract(&source, first, count, options)?);
    }
    deadline.checkpoint(clock.as_ref())?;

    // THE SOURCE GOES BEFORE ANY OUTPUT IS PARSED, for the reason `rotate` and `reorder` record:
    // verification holds a second parsed document, and both at once is peak memory nothing
    // bounds. Split is the worst case for it -- every part is already held in `outputs`.
    drop(source);

    // AND THE LAST THING BEFORE THE CALLER HAS THEM, each through a fresh engine.
    //
    // EVERY part, not the first: "one part is right" says nothing about the partition, which is
    // the sentence ADR 0022's table used to make about page counts and which is the whole reason
    // the promise is per part. A part that fails takes the WHOLE split with it -- there is no
    // partial success here, because a caller handed four good parts and one refusal has no way
    // to act on that which is not worse than being told the operation failed.
    for (output, expected) in outputs.iter().zip(promised) {
        verify::output(
            engine,
            output,
            &verify::Expected::Split {
                rotations: expected,
            },
            options,
            &deadline,
        )?;
    }

    Ok(outputs)
}

/// Turn cut points into `(first, count)` runs, or say why they are not usable.
///
/// Separated from [`split`] so the rule is testable without an engine. Every rejection here is
/// about the REQUEST rather than the document, which is why they are all `InvalidArgument`:
/// nothing in this function has looked at a single byte of anybody's file.
fn runs_from(cuts: Cuts<'_>, pages: u64) -> Result<Vec<(u64, u64)>> {
    if pages == 0 {
        return Err(Error::InvalidArgument(
            "a document with no pages cannot be split".to_owned(),
        ));
    }

    let mut previous = 0u64;
    for &cut in cuts.after {
        if cut == 0 {
            return Err(Error::InvalidArgument(
                "a cut must name a page, and pages are numbered from one".to_owned(),
            ));
        }
        if cut <= previous {
            // Covers both repeats and descending order, which are the same mistake seen from
            // two angles and would both produce an empty or negative run.
            return Err(Error::InvalidArgument(
                "cuts must be in increasing order, with no repeats".to_owned(),
            ));
        }
        if cut >= pages {
            // `>=` rather than `>`: cutting after the last page would ask for an empty final
            // document, which is not a document.
            return Err(Error::InvalidArgument(
                "a cut must fall before the last page".to_owned(),
            ));
        }
        previous = cut;
    }

    let mut runs = Vec::with_capacity(cuts.parts());
    let mut start = 0u64;
    for &cut in cuts.after {
        runs.push((start, cut - start));
        start = cut;
    }
    runs.push((start, pages - start));
    Ok(runs)
}

/// Cut points that divide a document of `pages` into runs of at most `every`.
///
/// The "every N pages" affordance, as a pure function over numbers. It lives here rather than
/// in the page so there is **one** implementation of it to test, and so the web and a future
/// mobile app cannot disagree about whether a ten-page document split every five gives two
/// parts or three.
///
/// # Errors
///
/// - [`Error::InvalidArgument`] if `every` is zero, which would describe an infinite number of
///   empty documents.
/// - [`Error::LimitExceeded`] if `pages` is over `max_pages`. The result is one cut per run, so
///   an unchecked page count is an unchecked allocation.
pub fn every(every: u64, pages: u64, limits: &Limits) -> Result<Vec<u64>> {
    if every == 0 {
        return Err(Error::InvalidArgument(
            "a run must be at least one page".to_owned(),
        ));
    }
    // THE CEILING BEFORE THE ALLOCATION, because the result length is `pages / every` and both
    // come from the caller. `every(1, u64::MAX)` allocated until it panicked with a capacity
    // overflow -- in library code, which `core/CLAUDE.md` forbids -- and `every(1, 1 << 30)`
    // quietly asked for 8 GiB. Found by code review. Every real caller has already had its
    // page count checked, so this is the signature carrying a guarantee that used to live in
    // the call sites' history.
    Limits::check(Stage::PageCount, "max_pages", pages, limits.max_pages)?;
    // A cut AFTER each multiple of `every` that is strictly inside the document. Ten pages
    // every five is one cut (after 5), not two -- a cut after page 10 would ask for an empty
    // eleventh-page document.
    Ok((1..pages.div_ceil(every)).map(|n| n * every).collect())
}
