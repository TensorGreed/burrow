//! Clear a region on one page, and verify the document that comes out (#134, ADR 0029).
//!
//! # What this operation promises, in the order the promises get weaker
//!
//! 1. **The glyphs the region reached are no longer drawn.** Re-derived from the emitted bytes
//!    through a fresh parse — not from the operation's record of what it removed.
//! 2. **The fonts it cut no longer map codes the page no longer draws**, and the page carries
//!    no key outside ADR 0029 §2's allowlist. Both read back from the output.
//! 3. **The same pages came out, displaying the same way.** The half every operation makes,
//!    through [`crate::verify::output`].
//!
//! # What it does not promise, and the difference matters
//!
//! Not that **the secret is gone**. The removed characters may survive in the embedded font
//! program, in a channel on the catalogue qpdf's C API cannot reach, or as pixels in an image —
//! and the operation *refuses* the shapes it cannot handle rather than pretending. ADR 0029 §7
//! is the disclosure that covers the rest, and it is a disclosure precisely because nothing
//! here can turn it into an assertion. [`crate::verify::Expected::RegionCleared`] is named for
//! that distinction.
//!
//! # Not a route
//!
//! This makes redaction callable from the core, on both engines: the native `Qpdf` and, since
//! #191, the web `WebQpdf` implement [`burrow_engines::PageRedactor`] over one policy. It does not
//! put redaction on the site: #125 blocks that and is untouched here, and there is no wasm binding
//! entry point (#137) and no page (#136).

use std::collections::BTreeSet;
use std::sync::Arc;

use burrow_engines::pdfsyntax::region::Region;
use burrow_engines::redact::Report;
use burrow_engines::{OpenOptions, OutputReader, PageRedactor};
use burrow_types::{Deadline, Error, Result};

use crate::verify;

/// What a redaction produced.
#[derive(Debug)]
#[non_exhaustive]
pub struct Redaction {
    /// The emitted document, verified.
    pub document: Vec<u8>,
    /// What the operation did beyond the bytes — per font, whether it was cut or retained.
    ///
    /// **Carried out of the operation rather than left inside it** because ADR 0029 §7's
    /// disclosure applies to the documents it applies to, and only this says which those are.
    /// A caller showing a redaction's result needs it to decide what to put on the page.
    pub report: Report,
}

/// Clear `region` on `page`, and return the document only if it reads back as promised.
///
/// `redacted` is the set of pages this operation covers. It decides which fonts may be cut: a
/// font is cut only when every page using it is in the set, because editing one used elsewhere
/// reflows a page nobody selected. See ADR 0029's font-sharing rule.
///
/// # Errors
///
/// - Every refusal the geometry walk and the sharing rules raise, each naming its rule.
/// - [`Error::OutputRejected`] if the emitted document does not read back as promised —
///   either the region check inside the engine, or the page count and rotation vector here.
/// - [`Error::InvalidArgument`] if `page` is not in `redacted`: an operation asked to redact a
///   page it does not say it covers would cut fonts on the strength of a set that excludes the
///   page being edited.
/// - Whatever opening the input failed with.
pub fn page<E>(
    engine: &E,
    bytes: &[u8],
    page: usize,
    redacted: &BTreeSet<usize>,
    region: Region,
    options: &OpenOptions<'_>,
) -> Result<Redaction>
where
    E: PageRedactor + OutputReader,
{
    // THE PAGE MUST BE IN THE SET IT IS REDACTED UNDER. `cut_fonts` decides cuttability across
    // `redacted`, so a page outside it would have its own fonts judged against a set that does
    // not contain it -- which is not a wrong answer so much as an unanswerable question.
    if !redacted.contains(&page) {
        return Err(Error::InvalidArgument(
            "redact: the page being redacted must be one the operation covers".to_owned(),
        ));
    }

    let clock = Arc::clone(&options.clock);
    let deadline = Deadline::start(clock.as_ref(), &options.limits);

    // THE PROMISE IS READ FROM THE INPUT, BEFORE ANYTHING IS EDITED. Reading it from the output
    // would be the output agreeing with itself, which is what `OutputReader::fresh` exists to
    // stop -- and reading it after the edit would be reading a document the edit produced.
    let promised = engine.input_rotations(bytes, options, &deadline)?;
    deadline.checkpoint(clock.as_ref())?;

    // THE REGION CHECK IS INSIDE. The engine's only path to bytes runs it; see `PageRedactor`.
    let (document, report) = engine.redact_page(bytes, page, redacted, region, options)?;
    deadline.checkpoint(clock.as_ref())?;

    // AND THE HALF EVERY OPERATION SHARES, last, through a fresh engine.
    verify::output(
        engine,
        &document,
        &verify::Expected::RegionCleared {
            rotations: promised,
        },
        options,
        &deadline,
    )?;

    Ok(Redaction { document, report })
}

#[cfg(test)]
mod tests;
