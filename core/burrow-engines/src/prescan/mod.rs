//! A bounded structural pre-scan, run before any engine sees the file.
//!
//! # The attack this exists to stop
//!
//! M1 PR 2's memory pre-check is a function of the input's **length**, so it is blind to
//! everything a PDF *declares*. `tests/conformance/fixtures/xref-bomb.pdf` is 330 KB and
//! declares a cross-reference stream of twenty million entries — 340 MB uncompressed,
//! which PDFium must inflate to parse the file at all, ending up resident in about
//! 1.2 GB. The size-based estimate predicts 16 MB for it and waves it through.
//!
//! PR 2 added a *measured* check after the load, which turns that into `LimitExceeded`
//! instead of `Ok`. But the gigabyte is allocated by then, and on a constrained device the
//! engine can hit its own out-of-memory path and `abort()` first — which is not a panic,
//! so nothing in Rust sees it. The only real defence is to refuse the file **before**
//! anything parses it. That is this module.
//!
//! # Why it is written in Rust rather than driven by qpdf
//!
//! A pre-scan that can itself be blown up has not removed the bomb, only moved it. Using
//! qpdf here would mean running a full C++ parser on completely ungated hostile input —
//! the pre-scan would become the new attack surface, bounded only by whatever limits the
//! engine happens to offer.
//!
//! So this reads **declared numbers and nothing else**. It never decompresses, never
//! builds an object graph, never follows a reference, and never allocates in proportion to
//! its input. Every loop has a fixed cap. The module is `forbid(unsafe_code)` — the one
//! part of this crate that can be, which is worth stating rather than leaving implicit.
//!
//! # What it does NOT do
//!
//! It sees *declarations*, not truth. A file whose declarations are modest and whose
//! content is hostile passes it, and that is what the `pdfium` module's measured
//! post-open check is still for. These are layers, not alternatives: this one is cheap
//! and pre-emptive, that one is expensive and retrospective.
//!
//! It is also not a PDF parser and must never grow into one. If a question needs the
//! object graph, it belongs in the `qpdf` module, behind qpdf's own resource limits.

#![forbid(unsafe_code)]

mod lengths;
mod xref;

use burrow_types::{Error, Limits, Result};

/// How far back from the end of the file to look for `startxref`.
///
/// The specification puts it in the last few dozen bytes. 64 KiB is far beyond any real
/// file's trailing junk, and the search cost still does not depend on the input's size.
///
/// Raised from 2 KiB after review: appending four kilobytes of junk after `%%EOF` pushed
/// `startxref` out of the old window, so the scan found no cross-reference at all and
/// passed a 2.5 GB bomb. Widening alone is not a fix — a file can always pad further —
/// which is why [`describe`] falls back to a whole-buffer scan when this path finds
/// nothing.
const STARTXREF_SEARCH_WINDOW: usize = 64 * 1024;

/// What one cross-reference entry costs an engine, in bytes.
///
/// **This is the load-bearing constant, and it is not `sum(/W)`.** `/W` is how many bytes
/// the *file* spends encoding an entry; a cross-reference stream then compresses that, so
/// twenty million entries occupy 330 KB on disk. What matters is what an engine allocates
/// once it has them, which is an in-memory record per entry and is far larger.
///
/// Measured, not guessed: `tests/conformance/fixtures/xref-bomb.pdf` declares 20,000,000
/// entries and drives PDFium to a resident-set growth of 1,250,388 KB — almost exactly 64
/// bytes per entry. An earlier version of this check compared `entries x sum(W)`
/// (340 MB) against the limit and let the bomb straight through, because 340 MB fits
/// comfortably inside the 1 GiB default.
///
/// It is an estimate of one engine's behaviour on one platform, so it is deliberately not
/// tuned finer than a power of two.
///
/// # Headroom, so the false-positive risk is a number rather than a hope
///
/// Rejecting a legitimate document is a bug users experience directly, so the margin
/// matters as much as the threshold. Measured:
///
/// | | entries | estimate |
/// |---|---|---|
/// | the default 1 GiB ceiling allows | 16,777,216 | 1 GiB |
/// | a 20,000-page generated document declares | 20,003 | 1.2 MB |
/// | `tests/conformance/fixtures/xref-bomb.pdf` declares | 20,000,000 | 1.28 GB |
///
/// So a real document would need roughly **eight hundred times** the objects of a
/// 20,000-page file before this fired, and even a phone-sized 64 MiB ceiling still allows
/// a million entries. If the constant were wrong by a factor of four in either direction
/// the outcome for both columns would be unchanged, which is the property that makes a
/// one-platform measurement safe to build on.
const ENGINE_BYTES_PER_XREF_ENTRY: u64 = 64;

/// Longest `/Prev` chain followed before giving up.
///
/// Each hop is an incremental update. Real files have a handful; a file with hundreds is
/// either pathological or trying to make this loop expensive. Combined with the visited
/// set in [`xref`], this makes the chain walk terminate on any input, including one whose
/// `/Prev` points at itself.
const MAX_XREF_CHAIN: usize = 32;

/// What the file says about itself.
///
/// Every field is a **declaration**, read from the file without verifying it. That is the
/// point: the declarations are what an engine will act on, so they are what must be
/// checked before the engine acts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct Declared {
    /// The most cross-reference entries any one section of the `/Prev` chain declares.
    ///
    /// The number that drives cost. See [`Declared::estimated_xref_bytes`].
    ///
    /// The **maximum** across sections rather than the sum: an engine materialises one
    /// merged table, and a cross-reference stream without `/Index` declares the whole
    /// document's `/Size` in every section — so summing a twenty-update file would
    /// over-estimate twentyfold and reject a document nothing is wrong with.
    pub xref_entries: u64,
    /// The largest single `/Length` found anywhere in the file.
    pub largest_stream: u64,
    /// How many cross-reference sections the `/Prev` chain visited.
    ///
    /// Kept because it is the evidence that the chain walk terminates: a file whose
    /// `/Prev` points at itself reports one section, not a hang.
    pub xref_sections: u32,
}

impl Declared {
    /// What an engine would allocate for this file's cross-reference table.
    ///
    /// `xref_entries` multiplied by the measured per-entry cost, saturating. See this
    /// module's `ENGINE_BYTES_PER_XREF_ENTRY`, which is private.
    #[must_use]
    pub fn estimated_xref_bytes(&self) -> u64 {
        self.xref_entries
            .saturating_mul(ENGINE_BYTES_PER_XREF_ENTRY)
    }
}

/// Check what a file declares against `limits`, before anything parses it.
///
/// # Errors
///
/// - [`Error::LimitExceeded`] when a declared quantity exceeds what `limits` allows. The
///   limit named is the one the declaration would breach, so the caller sees the same
///   field they set.
/// - [`Error::Malformed`] when the declarations are internally impossible — most usefully,
///   a stream claiming more bytes than the whole file contains.
///
/// A file with no recognisable cross-reference is **not** an error here. PDFium
/// reconstructs a missing or corrupt xref by itself (spike 0001, Finding 4), and refusing
/// such files would reject documents that work today. There is simply nothing declared to
/// check, so there is nothing to object to.
pub fn check(bytes: &[u8], limits: &Limits) -> Result<Declared> {
    let declared = describe(bytes);

    // A stream that claims more bytes than the file holds cannot be honest, whatever else
    // is true. This is cheap and catches the crudest amplification outright.
    let file_len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if declared.largest_stream > file_len {
        return Err(Error::Malformed(
            "a stream declares more bytes than the file contains".to_owned(),
        ));
    }

    // The cross-reference table an engine would have to materialise. This is the check
    // that stops the declared-size bomb, and it is the whole reason this module exists.
    Limits::check(
        "max_memory_bytes",
        declared.estimated_xref_bytes(),
        limits.max_memory_bytes,
    )?;

    Ok(declared)
}

/// Read every declaration this module understands, without judging any of them.
///
/// Separate from [`check`] so tests can assert what was *read* independently of what was
/// *rejected*, and so the fuzz target can exercise the reading without a limits policy.
#[must_use]
pub fn describe(bytes: &[u8]) -> Declared {
    let mut declared = xref::describe(bytes, STARTXREF_SEARCH_WINDOW, MAX_XREF_CHAIN);

    // The backstop.
    //
    // Every bypass review found had the same shape: make the directed walk read *nothing*
    // -- by hiding `/Size` behind a decoy key, by padding it out of the dictionary window,
    // or by pushing `startxref` out of the tail window -- and a scan that found no
    // cross-reference waved the file through, because "nothing declared" is
    // indistinguishable from "nothing to declare".
    //
    // So when the directed walk finds nothing, fall back to scanning the **whole buffer**
    // for the largest `/Size` anywhere in it. That is immune to all three, because it uses
    // neither `startxref` nor a window.
    //
    // It is only a fallback, not the primary, because it is blunt: `/Size` in an unrelated
    // dictionary would be read as an object count. On a file where the directed walk
    // works -- which is every well-formed document -- this never runs, so the bluntness
    // costs nothing and is confined to files that are already lying about their structure.
    if declared.xref_entries == 0 {
        declared.xref_entries = xref::largest_declared_size(bytes);
    }

    declared.largest_stream = lengths::largest(bytes);
    declared
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_input_declares_nothing_and_is_not_an_error() {
        assert_eq!(describe(b""), Declared::default());
        assert!(check(b"", &Limits::default()).is_ok());
    }

    #[test]
    fn arbitrary_junk_never_panics() {
        // Not a property test -- that lives in tests/prescan.rs, alongside the fuzz
        // target. This is only the smoke check that the entry point survives the obvious
        // shapes, so reaching the end of the loop is the whole assertion.
        for junk in [
            b"%PDF-1.7".as_slice(),
            b"startxref".as_slice(),
            b"startxref\n".as_slice(),
            b"startxref\n999999999999999999999\n%%EOF".as_slice(),
            b"startxref\n-1\n%%EOF".as_slice(),
            b"/Length".as_slice(),
            b"/Length 99999999999999999999999999".as_slice(),
            &[0xFF; 512],
        ] {
            let _ = describe(junk);
            let _ = check(junk, &Limits::default());
        }
    }

    #[test]
    fn a_stream_claiming_more_than_the_file_holds_is_malformed() {
        let bytes = b"%PDF-1.7\n1 0 obj\n<< /Length 999999 >>\nstream\nx\nendstream\n";
        match check(bytes, &Limits::default()) {
            Err(Error::Malformed(message)) => {
                assert!(!message.chars().any(|c| c.is_ascii_digit()), "{message}");
            }
            other => panic!("expected Malformed, got {other:?}"),
        }
    }

    #[test]
    fn a_file_with_no_xref_at_all_is_allowed_through() {
        // PDFium reconstructs a missing xref (spike 0001 Finding 4). Rejecting here would
        // break files that work today, and this module's job is bombs, not tidiness.
        let bytes = b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n";
        let declared = check(bytes, &Limits::default()).expect("no xref is not a failure");
        assert_eq!(declared.xref_entries, 0);
        assert_eq!(declared.estimated_xref_bytes(), 0);
    }
}
