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

/// Redact one page and return the bytes and the report.
///
/// # Errors
///
/// Every refusal the walk, the sharing rule and the write path raise. A failure discards the
/// document: there is no partial output.
#[cfg(all(feature = "native-engines", burrow_native_engines))]
pub fn redact_page(
    bytes: &[u8],
    page: usize,
    redacted: std::collections::BTreeSet<usize>,
    region: crate::pdfsyntax::region::Region,
) -> Result<(Vec<u8>, crate::redact::Report)> {
    redact_page_with_limits(
        bytes,
        page,
        redacted,
        region,
        burrow_types::Limits::default(),
    )
}

/// As [`redact_page`], with the ceilings stated rather than defaulted.
///
/// # It exists so a `Limits` ceiling can be tested at all
///
/// A test for "this refuses past `max_memory_bytes`" has two ways to go: build a fixture large
/// enough to pass the default gigabyte, or say what the ceiling is. The first is a memory
/// experiment rather than a test — and a fixture sized to a default is a fixture that stops
/// testing anything the day the default moves.
///
/// # Errors
///
/// Every refusal the walk, the sharing rules and the write path raise. A failure discards the
/// document: there is no partial output.
#[cfg(all(feature = "native-engines", burrow_native_engines))]
pub fn redact_page_with_limits(
    bytes: &[u8],
    page: usize,
    redacted: std::collections::BTreeSet<usize>,
    region: crate::pdfsyntax::region::Region,
    limits: burrow_types::Limits,
) -> Result<(Vec<u8>, crate::redact::Report)> {
    crate::qpdf::redact_page_for_probe(bytes, page, redacted, region, limits)
}
