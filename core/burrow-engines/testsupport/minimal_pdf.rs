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
