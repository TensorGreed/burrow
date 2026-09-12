//! Property tests for the structural pre-scan.
//!
//! The pre-scan's whole claim is that it **cannot be blown up**: allocation is O(1)
//! regardless of input, every loop has a fixed cap, and there is no recursion. That is a
//! claim about all inputs, which is what a property test is for — and the fuzz target in
//! `fuzz/fuzz_targets/prescan.rs` is the other half.
//!
//! It runs on every platform, unlike the engine suites, because the pre-scan is pure Rust
//! with nothing to link. M1 PR 4's web path will use it unchanged.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::integer_division
)]

use burrow_engines::prescan;
use burrow_types::{Error, Limits, Stage};
use proptest::prelude::*;

/// Every message the pre-scan can produce. Fixed strings, all of them.
///
/// The same discipline as `tests/properties.rs`: a message assembled from the input would
/// not be in this list, so a leak fails the test rather than passing it.
const ALLOWED_MESSAGES: &[&str] = &["a stream declares more bytes than the file contains"];

fn check_outcome(result: &burrow_types::Result<prescan::Declared>) -> Result<(), String> {
    match result {
        Ok(_) => Ok(()),
        Err(Error::LimitExceeded { limit, .. }) => {
            if *limit == "max_memory_bytes" {
                Ok(())
            } else {
                Err(format!("unexpected limit name {limit:?}"))
            }
        }
        Err(Error::Malformed(message)) => {
            if ALLOWED_MESSAGES.contains(&message.as_str()) {
                Ok(())
            } else {
                Err(format!(
                    "a message that is not a fixed constant reached the caller, so it may \
                     carry input-derived bytes: {message:?}"
                ))
            }
        }
        Err(other) => Err(format!(
            "the pre-scan should only ever report LimitExceeded or Malformed, got {other:?}"
        )),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// The headline property: arbitrary bytes never panic and never produce an untyped
    /// outcome. No engine is involved, so this is fast enough to run at high volume.
    #[test]
    fn arbitrary_bytes_are_never_fatal(bytes in prop::collection::vec(any::<u8>(), 0..8192)) {
        let described = prescan::describe(&bytes);
        // The estimate must never wrap: an absurd declaration has to stay absurd, because
        // wrapping into a small number is exactly how a bomb would slip past the check.
        prop_assert!(
            described.estimated_xref_bytes() >= described.xref_entries
                || described.xref_entries == 0
        );

        if let Err(why) = check_outcome(&prescan::check(&bytes, &Limits::default())) {
            prop_assert!(false, "{why}");
        }
    }

    /// Input made largely of the tokens the scanner looks for, which is the shape most
    /// likely to drive its loops. Uniform random bytes almost never contain `/Length` or
    /// `startxref`, so without this the scanner's interesting paths are barely reached.
    #[test]
    fn dense_pdf_tokens_are_never_fatal(
        parts in prop::collection::vec(
            prop::sample::select(vec![
                "/Length ", "startxref", "xref", "trailer", "/Size ", "/W [", "/Index [",
                "/Prev ", "]", ">>", "<<", "0", "9", " ", "\n", "99999999999999999999",
                "%%EOF", "obj", "endobj", "stream",
            ]),
            0..512,
        ),
    ) {
        let bytes: Vec<u8> = parts.concat().into_bytes();
        if let Err(why) = check_outcome(&prescan::check(&bytes, &Limits::default())) {
            prop_assert!(false, "{why}");
        }
    }

    /// A cross-reference stream declaring `entries` is rejected exactly when the estimate
    /// exceeds the ceiling, and allowed otherwise. The boundary, not just the direction.
    #[test]
    fn the_declared_entry_count_decides_the_outcome(entries in 1u64..100_000_000) {
        let pdf = format!(
            "%PDF-1.5\n4 0 obj\n<< /Type /XRef /Size {entries} /W [1 8 8] >>\n\
             stream\nx\nendstream\nendobj\nstartxref\n9\n%%EOF\n"
        );
        let declared = prescan::describe(pdf.as_bytes());
        prop_assert_eq!(declared.xref_entries, entries);

        let estimated = declared.estimated_xref_bytes();

        // Exactly at the estimate: allowed.
        let at = prescan::check(pdf.as_bytes(), &Limits::with(|l| l.max_memory_bytes = estimated));
        prop_assert!(at.is_ok(), "the boundary should be allowed: {:?}", at.err());

        // One byte under: refused, naming the caller's own field.
        let under = prescan::check(
            pdf.as_bytes(),
            &Limits::with(|l| l.max_memory_bytes = estimated.saturating_sub(1)),
        );
        match under {
            Err(Error::LimitExceeded { limit, stage, requested, allowed }) => {
                prop_assert_eq!(limit, "max_memory_bytes");
                // The PRE-SCAN, not the length-based estimate and not the measured check.
                // All three report `max_memory_bytes`; only this one runs before an engine
                // sees a file that merely *declares* something enormous.
                prop_assert_eq!(stage, Stage::Prescan);
                prop_assert_eq!(requested, estimated);
                prop_assert_eq!(allowed, estimated.saturating_sub(1));
            }
            other => prop_assert!(false, "expected LimitExceeded, got {:?}", other.map(|_| ())),
        }
    }

    /// Truncating a valid file at any point leaves something the pre-scan survives.
    ///
    /// Truncation is the single most common real-world damage, and it puts the scanner's
    /// cursor at an arbitrary point inside every structure it understands.
    #[test]
    fn any_truncation_of_a_valid_file_is_survivable(cut in 0usize..400) {
        let full = valid_pdf();
        let at = cut.min(full.len());
        let bytes = &full[..at];
        if let Err(why) = check_outcome(&prescan::check(bytes, &Limits::default())) {
            prop_assert!(false, "truncated at {}: {}", at, why);
        }
    }

    /// Any single-byte mutation of a valid file, likewise.
    #[test]
    fn any_single_byte_mutation_is_survivable(
        index in any::<prop::sample::Index>(),
        value in any::<u8>(),
    ) {
        let mut bytes = valid_pdf();
        let at = index.index(bytes.len());
        bytes[at] = value;
        if let Err(why) = check_outcome(&prescan::check(&bytes, &Limits::default())) {
            prop_assert!(false, "mutated byte {}: {}", at, why);
        }
    }
}

/// A small, well-formed file with a classic cross-reference table.
fn valid_pdf() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-1.7\n");
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    out.extend_from_slice(b"2 0 obj\n<< /Length 12 >>\nstream\nhello world\nendstream\nendobj\n");
    out.extend_from_slice(
        b"xref\n0 3\n0000000000 65535 f \n0000000009 00000 n \n0000000060 00000 n \n",
    );
    out.extend_from_slice(b"trailer\n<< /Size 3 /Root 1 0 R >>\nstartxref\n120\n%%EOF\n");
    out
}

/// Three ways to hide a declared-size bomb from the pre-scan, all of which worked.
///
/// # Why these are here
///
/// Found by security review of M1 PR 3. Each is a one-line change to
/// `tests/conformance/fixtures/xref-bomb.pdf`, each made the scan report **zero** entries,
/// and each therefore restored the exact pre-PR behaviour: PDFium allocating ~2.5 GB over
/// about seven seconds before the measured post-open check noticed.
///
/// The common shape is worth naming, because it is what the fix is really about: all three
/// worked by making the directed walk read *nothing*, and a scan that found no
/// cross-reference treated "nothing declared" as "nothing to declare". Widening a window
/// does not fix that — a file can always pad further — which is why `describe` now falls
/// back to a whole-buffer scan whenever the directed walk comes up empty.
#[test]
fn a_declared_size_bomb_cannot_hide_from_the_scan() {
    const DECLARED: u64 = 20_000_000;

    // 1. A decoy key. `/Sizes` contains `/Size`, so a literal first-match search read
    //    `s 1` as the value, failed, and reported nothing.
    let decoy = format!(
        "%PDF-1.5\n4 0 obj\n<< /Type /XRef /Sizes 1 /Size {DECLARED} /W [1 8 8] >>\n\
         stream\nx\nendstream\nendobj\nstartxref\n9\n%%EOF\n"
    );

    // 2. Padding, so the real `/Size` falls outside the dictionary window.
    let padding = " ".repeat(8 * 1024);
    let padded = format!(
        "%PDF-1.5\n4 0 obj\n<< /Type /XRef{padding} /Size {DECLARED} /W [1 8 8] >>\n\
         stream\nx\nendstream\nendobj\nstartxref\n9\n%%EOF\n"
    );

    // 3. Trailing junk, so `startxref` falls outside the tail window.
    let junk = "%".repeat(8 * 1024);
    let tail = format!(
        "%PDF-1.5\n4 0 obj\n<< /Type /XRef /Size {DECLARED} /W [1 8 8] >>\n\
         stream\nx\nendstream\nendobj\nstartxref\n9\n%%EOF\n{junk}"
    );

    for (what, bytes) in [
        ("a decoy /Sizes key", decoy),
        ("padding past the dictionary window", padded),
        ("junk past the tail window", tail),
    ] {
        let declared = prescan::describe(bytes.as_bytes());
        assert_eq!(
            declared.xref_entries, DECLARED,
            "{what}: the scan read {} entries, so the declaration was hidden from it",
            declared.xref_entries
        );
        assert!(
            prescan::check(bytes.as_bytes(), &Limits::default()).is_err(),
            "{what}: the bomb was not rejected"
        );
    }
}

/// The whole-buffer backstop must not fire on documents the directed walk handles.
///
/// It is blunt by design — it reads the largest `/Size` anywhere in the file — so it runs
/// only when the directed walk found nothing. This is the check that it stays that way:
/// an ordinary document must be judged on its real cross-reference, not on a stray key.
#[test]
fn the_backstop_does_not_fire_on_ordinary_documents() {
    for pages in [1usize, 10, 137] {
        let pdf = valid_pdf_with_pages(pages);
        let declared = prescan::describe(&pdf);
        assert!(
            declared.xref_entries <= 200,
            "{pages} pages declared {} entries; the backstop read something it should not \
             have",
            declared.xref_entries
        );
        assert!(prescan::check(&pdf, &Limits::default()).is_ok());
    }
}

/// A small, well-formed file with `pages` pages and a real cross-reference table.
fn valid_pdf_with_pages(pages: usize) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-1.7\n");
    let kids: String = (0..pages).map(|i| format!("{} 0 R ", i + 3)).collect();
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    out.extend_from_slice(
        format!(
            "2 0 obj\n<< /Type /Pages /Kids [{}] /Count {pages} >>\nendobj\n",
            kids.trim_end()
        )
        .as_bytes(),
    );
    for i in 0..pages {
        out.extend_from_slice(
            format!("{} 0 obj\n<< /Type /Page /Parent 2 0 R >>\nendobj\n", i + 3).as_bytes(),
        );
    }
    let startxref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n", pages + 3).as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for _ in 0..pages + 2 {
        out.extend_from_slice(b"0000000009 00000 n \n");
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{startxref}\n%%EOF\n",
            pages + 3
        )
        .as_bytes(),
    );
    out
}

/// The pre-scan does no work proportional to how deeply an input nests.
///
/// It has no recursion at all, so this is really a check that nothing crept in. Half a
/// million brackets would overflow a recursive-descent parser's stack; here it is one
/// linear pass.
#[test]
fn nesting_costs_the_prescan_nothing() {
    for depth in [1_000usize, 100_000, 1_000_000] {
        let mut bytes = Vec::with_capacity(depth * 2 + 64);
        bytes.extend_from_slice(b"%PDF-1.7\n1 0 obj\n");
        bytes.extend(std::iter::repeat_n(b'[', depth));
        bytes.extend(std::iter::repeat_n(b']', depth));
        bytes.extend_from_slice(b"\nendobj\nstartxref\n9\n%%EOF\n");

        let result = prescan::check(&bytes, &Limits::default());
        assert!(
            check_outcome(&result).is_ok(),
            "depth {depth}: {:?}",
            check_outcome(&result)
        );
    }
}

/// A very large input costs the pre-scan one linear pass and no allocation.
///
/// The scan is O(n) in time by design — it has to look at the bytes — but O(1) in
/// allocation, which is the property that makes it un-blowable-up.
#[test]
fn a_large_input_is_scanned_without_allocating_in_proportion_to_it() {
    // 32 MiB of `/Length` tokens: the densest possible work for the scanner.
    let unit = b"/Length 1 ";
    let repeats = (32 * 1024 * 1024) / unit.len();
    let mut bytes = Vec::with_capacity(repeats * unit.len());
    for _ in 0..repeats {
        bytes.extend_from_slice(unit);
    }

    let before = resident_kb();
    let declared = prescan::describe(&bytes);
    let after = resident_kb();

    assert_eq!(declared.largest_stream, 1, "every /Length here declares 1");

    // The input itself is 32 MiB; the scan on top of it must be negligible.
    let grew_mb = after.saturating_sub(before) / 1024;
    assert!(
        grew_mb < 8,
        "scanning grew the resident set by {grew_mb} MB, which means it is allocating in \
         proportion to its input"
    );
}

fn resident_kb() -> u64 {
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1).and_then(|f| f.parse().ok()))
        .map_or(0, |pages: u64| pages * 4)
}
