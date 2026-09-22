//! Walking a real document's first page, for the differential test and nothing else.
//!
//! # Why this exists rather than a public entry point
//!
//! ADR 0022 forbids a public function that emits a redacted document until #134's verification
//! exists, and this does not emit one — it reports where the glyphs are. But it does need the
//! qpdf-backed resolver, which is `pub(crate)`, and the differential test lives in `tests/`.
//!
//! So: one narrow, documented seam, behind `native-engines`, returning geometry rather than
//! bytes. When #134 lands and the real entry point appears, this goes.

use burrow_types::Result;

use crate::pdfsyntax::geometry::Glyph;

/// Every glyph the first page draws, with the real font resolver.
///
/// # Errors
///
/// Whatever opening, reading or walking the document failed with — including the resolver's
/// own refusals, which are the interesting ones: a font with no `/Widths` carries no advance
/// in the document at all.
#[cfg(all(feature = "native-engines", burrow_native_engines))]
pub fn walk_first_page(bytes: &[u8]) -> Result<Vec<Glyph>> {
    crate::qpdf::walk_first_page_for_probe(bytes)
}

/// Without the engines there is nothing to walk with.
#[cfg(not(all(feature = "native-engines", burrow_native_engines)))]
pub fn walk_first_page(_bytes: &[u8]) -> Result<Vec<Glyph>> {
    Err(burrow_types::Error::Unsupported(
        "the native engines are not built, so no document can be opened".to_owned(),
    ))
}

/// Re-exported so the differential test can name the type.
pub use crate::pdfsyntax::geometry::Glyph as ProbeGlyph;

const _: fn(&[u8]) -> Result<Vec<Glyph>> = walk_first_page;
