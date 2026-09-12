//! Minimal, valid-by-construction PDFs, built in Rust.
//!
//! Included by both the unit tests in `src/pdfium/tests.rs` and the integration tests in
//! `tests/`, via `#[path]`, so there is one generator rather than two that drift.
//!
//! Everything here is generated, not fetched. That keeps the property tests free of any
//! corpus dependency, and it means the committed conformance fixtures have trivial
//! provenance: they are this code's output, under the project's own licence.
//!
//! The files are deliberately as small as a PDF can be while still being *correct* — a
//! real header, real objects, a real cross-reference table with real byte offsets, and a
//! real trailer. PDFium reconstructs a broken xref by itself
//! ([spike 0001](../../../docs/spikes/0001-wasm-engines.md) Finding 4), so writing a
//! sloppy one would silently test the repair path instead of the parse path.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::integer_division
)]

/// A well-formed PDF with `pages` empty US-Letter pages.
///
/// Object numbering: 1 is the catalogue, 2 is the page tree, and `3..3+pages` are the
/// pages.
pub fn pdf_with_pages(pages: usize) -> Vec<u8> {
    build(pages, None)
}

/// A well-formed PDF whose page tree is empty.
///
/// Legal enough to parse and meaningless as a document: `FPDF_GetPageCount` reports 0,
/// which burrow treats as [`Error::Malformed`](burrow_types::Error::Malformed) rather
/// than as a zero-page success.
pub fn pdf_with_no_pages() -> Vec<u8> {
    build(0, None)
}

/// A PDF that declares standard security with credentials nothing can authenticate.
///
/// The `/O` and `/U` strings are fixed arbitrary bytes, so the authentication step fails
/// for the empty password and for every other password. That is enough to drive PDFium
/// down its `FPDF_ERR_PASSWORD` path without implementing RC4 and MD5 here: the check is
/// over the values in the dictionary, not over the streams.
///
/// It therefore covers "encrypted, and the password did not work" — which is the error
/// variant — and *not* "encrypted, and the password did work". Writing a genuinely
/// encrypted file needs an encryptor, and qpdf arrives in M1 PR 3.
pub fn pdf_encrypted_with_unusable_credentials() -> Vec<u8> {
    build(1, Some(EncryptDict))
}

/// A well-formed PDF truncated part-way through, with no xref and no trailer.
pub fn truncated_pdf() -> Vec<u8> {
    let full = pdf_with_pages(3);
    let cut = full.len() / 3;
    full.into_iter().take(cut).collect()
}

/// Bytes that are not a PDF and do not claim to be.
pub fn not_a_pdf() -> Vec<u8> {
    let mut out = b"GIF89a".to_vec();
    out.extend(std::iter::repeat_n(0x41, 512));
    out
}

/// Marker for "add an `/Encrypt` dictionary this file cannot authenticate against".
struct EncryptDict;

fn build(pages: usize, encrypt: Option<EncryptDict>) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    // Offsets of object 1, 2, ... in order. Index `i` holds object `i + 1`.
    let mut offsets: Vec<usize> = Vec::new();

    // The binary comment on the second line is what tells a transfer agent this is not a
    // text file. Real PDFs have it; omitting it would make these fixtures unrealistic in
    // the one way that is free to get right.
    out.extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");

    let first_page_obj = 3;
    let mut kids = String::new();
    for i in 0..pages {
        kids.push_str(&format!("{} 0 R ", first_page_obj + i));
    }

    offsets.push(out.len());
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");

    offsets.push(out.len());
    out.extend_from_slice(
        format!(
            "2 0 obj\n<< /Type /Pages /Kids [{}] /Count {pages} >>\nendobj\n",
            kids.trim_end()
        )
        .as_bytes(),
    );

    for i in 0..pages {
        offsets.push(out.len());
        out.extend_from_slice(
            format!(
                "{} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
                 /Resources << >> >>\nendobj\n",
                first_page_obj + i
            )
            .as_bytes(),
        );
    }

    let encrypt_ref = encrypt.map(|EncryptDict| {
        let obj = offsets.len() + 1;
        offsets.push(out.len());
        // /V 1 /R 2 is the 40-bit standard security handler: the simplest thing every
        // reader implements. `O` and `U` are 32 bytes each, as the spec requires, and are
        // fixed nonsense so authentication cannot succeed.
        out.extend_from_slice(
            format!(
                "{obj} 0 obj\n<< /Filter /Standard /V 1 /R 2 /Length 40 \
                 /O <{o}> /U <{u}> /P -1 >>\nendobj\n",
                o = "A1".repeat(32),
                u = "B2".repeat(32),
            )
            .as_bytes(),
        );
        obj
    });

    let startxref = out.len();
    let size = offsets.len() + 1;

    out.extend_from_slice(format!("xref\n0 {size}\n").as_bytes());
    // Every entry is exactly 20 bytes: 10 digits, space, 5 digits, space, type, space,
    // newline. A reader is entitled to seek by multiplying, so a 19-byte entry breaks
    // every object after it.
    out.extend_from_slice(b"0000000000 65535 f \n");
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }

    let mut trailer = format!("trailer\n<< /Size {size} /Root 1 0 R");
    if let Some(obj) = encrypt_ref {
        // An encrypted file must carry an /ID; PDFium uses its first string in the key
        // derivation, and refuses the file outright without one.
        trailer.push_str(&format!(
            " /Encrypt {obj} 0 R /ID [<{id}> <{id}>]",
            id = "0F".repeat(16)
        ));
    }
    trailer.push_str(" >>\n");
    out.extend_from_slice(trailer.as_bytes());
    out.extend_from_slice(format!("startxref\n{startxref}\n%%EOF\n").as_bytes());

    out
}

// ---------------------------------------------------------------------------------
// Canary fixtures, for the secret-leak test.
// ---------------------------------------------------------------------------------

/// A PDF with `canary` embedded in every place an engine message might quote it.
///
/// The point of the secret-leak test is that nothing derived from a user's file reaches
/// stdout, stderr, or an error string. A canary is only convincing if it sits where the
/// engines actually quote from, so this puts it in all of them at once:
///
/// - a **name object** (`/BURROW-CANARY-…`), which qpdf prints in "expected n n obj"
///   style warnings;
/// - a **string object** (`(BURROW-CANARY-…)`), the most obvious candidate;
/// - a **stream's contents**, which a decoder error would be quoting from;
/// - a **dictionary key**, which a type error names;
/// - and a deliberately **broken token immediately next to the canary**, so the parser
///   fails at exactly the offset where the canary is, which is the case most likely to
///   pull it into a message.
///
/// The file is intentionally damaged: it must reach a failure path, because a file that
/// parses cleanly proves nothing about what failure messages contain.
pub fn pdf_with_canary(canary: &str) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    out.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
    out.extend_from_slice(
        format!(
            "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /{canary} /{canary}Value /Title ({canary}) /Contents 4 0 R >>\nendobj\n"
        )
        .as_bytes(),
    );
    // A stream whose contents are the canary. `/Length` is deliberately a *plausible*
    // wrong value rather than an enormous one: an enormous one would be refused by our own
    // structural pre-scan before either engine saw the file, which would make every case
    // built on this fixture a test of the pre-scan rather than of the engines. (It was,
    // until a mutation test caught it.)
    out.extend_from_slice(
        format!(
            "4 0 obj\n<< /Length 40 /{canary}Key ({canary}) >>\nstream\n\
             BT /F1 12 Tf ({canary}) Tj ET\n"
        )
        .as_bytes(),
    );
    // The broken token, immediately after the canary and with no `endstream`.
    out.extend_from_slice(format!("{canary} 0 obj obj obj\n").as_bytes());
    out.extend_from_slice(b"trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n9\n%%EOF\n");
    out
}

/// An ordinary PDF carrying an object number above `INT_MAX`.
///
/// # Why this exists
///
/// It is a regression guard for a **process abort**, found by review in M1 PR 3. qpdf's
/// `qpdf_is_linearized` is one of the C API functions that does *not* route through
/// `trap_errors`, and the `isLinearized()` behind it converts an object number with
/// `QIntC::to_int`, which throws `std::range_error` above `INT_MAX`. A foreign exception
/// is not a Rust panic, so nothing catches it:
///
/// ```text
/// fatal runtime error: Rust cannot catch foreign exceptions, aborting
/// ```
///
/// Measured on a 356-byte file exactly like this one. The document is otherwise
/// completely ordinary — it parses, its page count is right, and then the process dies.
///
/// `burrow` no longer calls either untrapped function, so this file is now harmless. It
/// is in the damaged corpus so that it stops being harmless the moment somebody adds one
/// back: the corpus test would abort rather than fail, which is loud in exactly the right
/// way.
pub fn pdf_with_object_number_above_int_max() -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"%PDF-1.7\n");
    // The oversized object number has to sit in the first 1024 bytes, because that is the
    // window `isLinearized()` tokenises looking for `N N obj`.
    out.extend_from_slice(b"9999999999 0 obj\nnull\nendobj\n");
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    out.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
    out.extend_from_slice(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\nendobj\n",
    );
    out.extend_from_slice(b"trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n9\n%%EOF\n");
    out
}

/// A PDF whose objects nest `depth` levels deep.
///
/// The other bomb shape: an array inside an array inside an array. Where the xref bomb
/// attacks memory, this attacks the parser's *stack*, and a recursive-descent parser
/// without a depth limit overflows and dies with a signal no `catch_unwind` can see.
/// qpdf's `parser_max_nesting` is what stops it; this generates the input that proves it.
pub fn pdf_with_nesting(depth: usize) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    out.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
    out.extend_from_slice(
        b"3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] >>\nendobj\n",
    );

    out.extend_from_slice(b"4 0 obj\n");
    out.extend(std::iter::repeat_n(b'[', depth));
    out.push(b'0');
    out.extend(std::iter::repeat_n(b']', depth));
    out.extend_from_slice(b"\nendobj\n");
    out.extend_from_slice(b"trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n9\n%%EOF\n");
    out
}

/// A PDF that **parses successfully** but makes qpdf warn, with the canary in the warning's
/// neighbourhood.
///
/// [Spike 0001](../../../docs/spikes/0001-wasm-engines.md) Finding 6's sharpest point: qpdf
/// emits warnings quoting object numbers and byte offsets *for files that parse fine*, not
/// only for broken ones. A leak test that exercised only failure paths would miss the case
/// that actually shipped.
///
/// The `startxref` offset here is deliberately wrong, which makes qpdf reconstruct the
/// cross-reference table and warn about doing so — while still producing a usable document,
/// because everything else is intact.
pub fn pdf_with_warnings_and_canary(canary: &str) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n");
    out.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n");
    out.extend_from_slice(
        format!(
            "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
             /{canary} ({canary}) >>\nendobj\n"
        )
        .as_bytes(),
    );
    // A cross-reference table whose offsets are all wrong. qpdf notices, warns, rebuilds
    // it, and carries on -- which is precisely the "successful parse, noisy stderr" case.
    out.extend_from_slice(b"xref\n0 4\n");
    out.extend_from_slice(b"0000000000 65535 f \n");
    for _ in 0..3 {
        out.extend_from_slice(b"0000009999 00000 n \n");
    }
    out.extend_from_slice(b"trailer\n<< /Size 4 /Root 1 0 R >>\n");
    // And a startxref pointing at nothing in particular, so the rebuild is forced.
    out.extend_from_slice(b"startxref\n4242\n%%EOF\n");
    out
}

// ---------------------------------------------------------------------------------
// The adversarial corpus, for the differential harness (M1 PR 4b).
// ---------------------------------------------------------------------------------
//
// Every file below already existed as a `format!` inside one test. They are here now
// because ROADMAP item 12 runs the corpus through **both** engine implementations, and a
// fixture that lives inside a native `#[test]` is one the web path never sees — which is
// precisely the asymmetry item 12 exists to close.
//
// Moving them here rather than copying them is the same rule this file's header states:
// one generator, so there is nothing to drift.

/// Entries a hidden-bomb fixture declares. Chosen to match `xref-bomb.pdf`, so the three
/// bypass fixtures are comparable with the bomb they were derived from.
const HIDDEN_BOMB_ENTRIES: u64 = 20_000_000;

/// A declared-size bomb hidden behind a **decoy key**.
///
/// `/Sizes` contains `/Size`, so a literal first-match search read `s 1` as the value,
/// failed to parse it, and reported nothing declared. Found by security review of M1 PR 3;
/// each of these three restored the full 2.5 GB behaviour with a one-line edit.
pub fn xref_bomb_hidden_by_decoy_key() -> Vec<u8> {
    format!(
        "%PDF-1.5\n4 0 obj\n<< /Type /XRef /Sizes 1 /Size {HIDDEN_BOMB_ENTRIES} /W [1 8 8] >>\n\
         stream\nx\nendstream\nendobj\nstartxref\n9\n%%EOF\n"
    )
    .into_bytes()
}

/// The same bomb, with the real `/Size` padded out of the dictionary window.
pub fn xref_bomb_hidden_by_dictionary_padding() -> Vec<u8> {
    let padding = " ".repeat(8 * 1024);
    format!(
        "%PDF-1.5\n4 0 obj\n<< /Type /XRef{padding} /Size {HIDDEN_BOMB_ENTRIES} /W [1 8 8] >>\n\
         stream\nx\nendstream\nendobj\nstartxref\n9\n%%EOF\n"
    )
    .into_bytes()
}

/// The same bomb, with `startxref` pushed out of the tail window by trailing junk.
pub fn xref_bomb_hidden_by_trailing_junk() -> Vec<u8> {
    let junk = "%".repeat(8 * 1024);
    format!(
        "%PDF-1.5\n4 0 obj\n<< /Type /XRef /Size {HIDDEN_BOMB_ENTRIES} /W [1 8 8] >>\n\
         stream\nx\nendstream\nendobj\nstartxref\n9\n%%EOF\n{junk}"
    )
    .into_bytes()
}

/// A valid three-page file cut in half: no xref, no trailer, and an object left open.
///
/// One of the two files **qpdf reads and PDFium refuses** (ADR 0013, *The repair question,
/// answered*). It is in the corpus because the two engines legitimately disagree about it,
/// which is exactly what a per-operation expectation exists to express.
pub fn truncated_mid_object() -> Vec<u8> {
    let valid = pdf_with_pages(3);
    let cut = valid.len() / 2;
    valid.into_iter().take(cut).collect()
}

/// The same file with its trailer removed entirely.
///
/// The other half of ADR 0013's pair, and the one pinned exactly: PDFium returns
/// `Malformed`, qpdf recovers all three pages. Unlike [`truncated_mid_object`], its
/// recovered page count does not depend on where a byte cut landed.
pub fn trailer_removed() -> Vec<u8> {
    let valid = pdf_with_pages(3);
    let needle = b"trailer";
    let at = valid
        .windows(needle.len())
        .position(|w| w == needle)
        .expect("the generator writes a trailer");
    valid.into_iter().take(at).collect()
}

/// The canary string the **committed** canary fixture carries.
///
/// Fixed, and deliberately different from the per-run canary
/// `core/burrow-engines/tests/secret_leak.rs` and
/// `apps/web/e2e/console-silence.spec.ts` generate.
///
/// A leak matcher needs a canary nothing else could have produced, so it generates a fresh
/// one per run. A corpus fixture is pinned by sha256 and cannot. They are different jobs:
/// this file asks "do both implementations reach the same typed outcome on a file built to
/// make a parser quote it", and the answer must not depend on a random string.
pub const FIXED_CANARY: &str = "BURROW-CANARY-FIXED-0000";

/// The canary fixture, with [`FIXED_CANARY`] embedded.
pub fn pdf_with_fixed_canary() -> Vec<u8> {
    pdf_with_canary(FIXED_CANARY)
}

#[cfg(test)]
mod generator_tests {
    use super::*;

    /// Find `needle` in `haystack`, by bytes.
    ///
    /// Deliberately not via `String::from_utf8_lossy`: the binary comment on line 2 is
    /// not UTF-8, and the replacement character is a different length, so every offset
    /// taken from the lossy string is wrong by three bytes. These tests are about byte
    /// offsets, so they work in bytes.
    fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack
            .windows(needle.len())
            .position(|window| window == needle)
    }

    #[test]
    fn every_xref_entry_is_exactly_twenty_bytes() {
        let pdf = pdf_with_pages(4);
        let free = find(&pdf, b"0000000000 65535 f \n").expect("the free entry");
        let trailer = find(&pdf, b"trailer").expect("a trailer");
        let entries = trailer - free;
        assert_eq!(
            entries % 20,
            0,
            "{entries} bytes of xref entries is not a multiple of 20"
        );
        // The free entry, plus the catalogue, the page tree, and four pages.
        assert_eq!(entries / 20, 1 + 2 + 4);
    }

    #[test]
    fn the_declared_offsets_point_at_their_objects() {
        let pdf = pdf_with_pages(3);
        let free = find(&pdf, b"0000000000 65535 f \n").expect("the free entry");

        // Objects 1 (catalogue), 2 (page tree), and 3..=5 (pages).
        for object in 1..=5usize {
            let entry = free + object * 20;
            let digits = std::str::from_utf8(&pdf[entry..entry + 10]).expect("ascii digits");
            let offset: usize = digits.parse().expect("a decimal offset");
            let expected = format!("{object} 0 obj");
            assert!(
                pdf[offset..].starts_with(expected.as_bytes()),
                "the xref says object {object} is at {offset}, but that is {:?}",
                String::from_utf8_lossy(&pdf[offset..(offset + 12).min(pdf.len())])
            );
        }
    }

    #[test]
    fn a_truncated_pdf_keeps_the_header_and_loses_the_trailer() {
        let pdf = truncated_pdf();
        assert!(pdf.starts_with(b"%PDF-1.7"));
        assert!(!pdf.ends_with(b"%%EOF\n"));
    }
}
