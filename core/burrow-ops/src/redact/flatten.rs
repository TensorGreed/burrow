//! Redaction v1: page *N* replaced by a picture of itself (ADR 0031).
//!
//! # Not a route
//!
//! **Nothing calls this but its tests.** `burrow-core` does not export it, neither binding names
//! it, and the redaction tool is held (#136 S9). It is the first slice of ADR 0031's build: the
//! write half, and V1 to V3 of its verification. §2's walks and rules, §5's page identity and V4's
//! element walk to V7 are later slices, and until they land an `Ok` here says only what V1 to V3
//! read back -- not that nothing else in the output still holds page *N*'s content.
//!
//! # What this adds to the engine's answer
//!
//! The half ADR 0022 shares with every operation, through [`verify::output`]: the page count and
//! the rotation vector, which is the **input's**, read before anything is edited, with page *N*'s
//! entry set to 0 because the written page carries `/Rotate 0` (ADR 0031 §3, V1).

use std::sync::Arc;

use burrow_engines::redact::flatten::Picture;
use burrow_engines::{OpenOptions, OutputReader, PageFlattener};
use burrow_types::{Deadline, Error, Result};

use crate::verify;

/// Replace `page` with `picture`, and return the document only if it reads back as promised.
///
/// `picture` is page `page` drawn at 150 dpi in display orientation, with the region painted:
/// in this slice its caller is a test, and in the slices that follow it is PDFium's render. Its
/// size is never trusted: the engine re-derives it from the page's frame.
///
/// # Errors
///
/// - [`Error::InvalidArgument`] for a page past the end.
/// - Everything the engine's flatten refuses with: every ceiling, the page's own frame refusal,
///   a picture of the wrong size, a repair, and [`Error::OutputRejected`] for V1's frames, V2 or
///   V3.
/// - [`Error::OutputRejected`] if the page count or the rotation vector read back differs from
///   the promise.
pub fn page<E>(
    engine: &E,
    bytes: &[u8],
    page: usize,
    picture: &Picture<'_>,
    options: &OpenOptions<'_>,
) -> Result<Vec<u8>>
where
    E: PageFlattener + OutputReader,
{
    let clock = Arc::clone(&options.clock);
    let deadline = Deadline::start(clock.as_ref(), &options.limits);

    // THE PROMISE, FROM THE INPUT, before anything is edited: page N's entry becomes 0.
    let mut promised = engine.rotations_before_flattening(bytes, options, &deadline)?;
    *promised.get_mut(page).ok_or_else(|| {
        Error::InvalidArgument("redact: a page index past the end of the document".to_owned())
    })? = 0;
    deadline.checkpoint(clock.as_ref())?;

    // V1'S FRAMES, V2 AND V3 ARE INSIDE: the engine's only path to bytes runs them.
    let document = engine.flatten_page(bytes, page, picture, options)?;
    deadline.checkpoint(clock.as_ref())?;

    verify::output(
        engine,
        &document,
        &verify::Expected::PageFlattened {
            rotations: promised,
        },
        options,
        &deadline,
    )?;
    Ok(document)
}

#[cfg(test)]
mod tests;
