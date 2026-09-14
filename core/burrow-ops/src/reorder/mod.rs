//! Put a document's pages in a different order.
//!
//! # What a reorder is here
//!
//! **A permutation, and nothing else.** No page is lost, added or duplicated — that is the
//! invariant `docs/ROADMAP.md` states, and [`Permutation`] enforces it at the boundary rather
//! than leaving each layer to re-derive it. The identity permutation is accepted and is a
//! no-op, because it is what a person dragging a page and putting it back has asked for.
//!
//! # It is not a subsetting operation
//!
//! Every input page appears in the output, so
//! [ADR 0019](../../../../docs/adr/0019-how-split-builds-its-outputs.md) §2's rule — an output
//! carries nothing derived from what it excluded — has nothing to bite on. The obligation is
//! the inverse: nothing may be lost. So reorder edits the document in place, as `rotate` does,
//! rather than building a new one as `split` must. Building would drop the outline, the
//! attachments and the `/AcroForm`, which is right when carrying them would be a leak and pure
//! loss when it would not.
//!
//! # The page tree is rebuilt, and the page says so
//!
//! [ADR 0021](../../../../docs/adr/0021-how-reorder-permutes-a-page-tree.md) measured it: qpdf
//! flattens the page tree when a page moves, after pushing inherited attributes down onto each
//! page. What every page displays is preserved; intermediate `/Pages` nodes are not. On a
//! two-level six-page fixture that is 16 objects in and 14 out.
//!
//! That is a real change to the document rather than a defect to hide, so `/reorder-pdf` owes
//! the same "what is kept, and what is not" paragraph `/merge-pdf` and `/rotate-pdf` have.
//!
//! # Limits
//!
//! | limit | where it applies |
//! |---|---|
//! | `max_input_bytes` | the input, once, at [`Stage::InputSize`] |
//! | `max_pages` | the input's page count, and the order's length, at [`Stage::PageCount`] |
//! | `max_duration_ms` | the whole operation, on the injected clock, checkpointed per page |
//! | `max_memory_bytes` | **detected, never bounded** (ADR 0007) |

#[cfg(test)]
mod tests;

use std::sync::Arc;

use burrow_engines::{OpenOptions, OutputReader, PageReorderer};

use crate::verify;
use burrow_types::{Deadline, Error, Limits, Permutation, Result, Stage};

/// Put the pages of `bytes` in `order`, and return the document.
///
/// `order` is **one-based** — the numbers a person uses — and is converted once, here. The
/// engine seam is zero-based, and [`Permutation`] is the type that carries the converted form
/// along with the guarantee that it names every page exactly once.
///
/// # Errors
///
/// - [`Error::InvalidArgument`] — the order does not name every page exactly once: the wrong
///   length, a page number of zero, one past the end, or one named twice.
/// - [`Error::Malformed`], [`Error::Unsupported`], [`Error::PasswordRequired`] — the input
///   could not be read.
/// - [`Error::LimitExceeded`] — a ceiling in `options.limits` was reached.
/// - [`Error::Io`] — the output could not be written.
/// - [`Error::OutputRejected`] — the document burrow produced is not the one it promised:
///   the wrong number of pages, or the pages in an order that is not the one requested. It is returned **instead of** the output, which is
///   dropped (ADR 0022).
/// - [`Error::Internal`] — a number did not fit, or the engine returned something impossible.
pub fn reorder<E: PageReorderer + OutputReader>(
    engine: &E,
    bytes: Box<[u8]>,
    order: &[u64],
    options: &OpenOptions<'_>,
) -> Result<Vec<u8>> {
    let limits = options.limits;
    let clock = Arc::clone(&options.clock);

    let source = engine.open(bytes, options)?;
    let total = engine.pages(&source)?;

    // THE CEILING HERE TOO, not only in the engine. `split` checks `max_pages` itself even
    // though its engine also does, and its comment records why: a ceiling that lives in
    // exactly one place is a ceiling one engine can forget.
    let named = u64::try_from(order.len())
        .map_err(|_| Error::Internal("page order length does not fit in u64".to_owned()))?;
    Limits::check(Stage::PageCount, "max_pages", total, limits.max_pages)?;
    Limits::check(Stage::PageCount, "max_pages", named, limits.max_pages)?;

    // ONE-BASED TO ZERO-BASED, with page 0 refused rather than treated as page 1. A caller
    // passing a zero-based list by mistake would otherwise get a silently different document —
    // and "silently different" is the whole failure class this operation is careful about.
    let mut zero_based = Vec::with_capacity(order.len());
    for &number in order {
        let Some(index) = number.checked_sub(1) else {
            return Err(Error::InvalidArgument(
                "pages are numbered from one".to_owned(),
            ));
        };
        zero_based.push(index);
    }

    // `Permutation::of` is where "every page exactly once" is decided, against the count the
    // engine reported. Everything past this point may rely on it.
    let permutation = Permutation::of(zero_based, total)?;

    // One deadline for the whole operation, started after the input is open — the open has its
    // own inside the engine. The engine checkpoints per page; these bracket the call, so a
    // refusal is attributable either to the operation or to what surrounds it.
    let deadline = Deadline::start(clock.as_ref(), &limits);
    deadline.checkpoint(clock.as_ref())?;

    // WHAT THE OUTPUT MUST DISPLAY, computed BEFORE the operation runs (ADR 0022): the input's
    // own rotations, permuted by the order that was asked for. A permutation cannot change
    // what a page displays -- only where it is -- so the promised vector is the input's read
    // through `permutation`, and comparing it to the answer catches a permutation that is not
    // the one requested wherever the two put differently-rotated pages in different places.
    // The engine checkpoints this sweep per page against the limits the document was opened
    // under; see `PageReorderer::rotations`.
    let before = PageReorderer::rotations(engine, &source, options, &deadline)?;
    let mut promised = Vec::with_capacity(before.len());
    for &from in permutation.order() {
        let at = usize::try_from(from)
            .map_err(|_| Error::Internal("page index does not fit in usize".to_owned()))?;
        let Some(rotation) = before.get(at) else {
            // Unreachable: `Permutation::of` established the order names every page of a
            // document of `total` pages, and `before` has `total` entries.
            return Err(Error::Internal(
                "a validated page order named a page the document does not have".to_owned(),
            ));
        };
        promised.push(*rotation);
    }

    let output = engine.reorder(&source, &permutation, options)?;
    deadline.checkpoint(clock.as_ref())?;

    // THE INPUT DOCUMENT GOES BEFORE THE OUTPUT IS PARSED, for the reason `rotate` records at
    // length: verification holds a second parsed document, and both at once is peak memory
    // nothing bounds.
    drop(source);

    // AND THE LAST THING BEFORE THE CALLER HAS IT, through a fresh engine.
    verify::output(
        engine,
        &output,
        &verify::Expected::Reordered {
            rotations: promised,
        },
        options,
        &deadline,
    )?;

    Ok(output)
}
