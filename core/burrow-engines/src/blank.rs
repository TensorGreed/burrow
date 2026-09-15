//! The empty document every split builds its parts into.
//!
//! **Ungated, because both engine paths need the same bytes.** It lived in `qpdf::extract` until
//! the web path needed it, and a second copy would have been two constants that agree until the
//! day they do not — with the xref offsets below as the thing that silently disagrees.

/// A valid PDF with a catalog and **one blank page**, used as the destination to build into.
///
/// # Why it is not empty, which is what it wants to be
///
/// **qpdf refuses to open a document with no pages.** The corpus already records this — the
/// conformance case `no-pages` expects `Malformed` — and the first version of this constant
/// was a zero-page document that `Document::open` rejected before any page could be copied
/// into it. So the destination starts with one blank page, and `extract` takes that page back
/// out once the wanted ones are in.
///
/// # Why it is not `qpdf_empty_pdf`
///
/// burrow may not call that: it is not on `engines/qpdf-trapped-functions.txt` and cannot earn
/// an entry in `engines/qpdf-untrapped-accepted.toml`, whose bar is "non-parsing … never
/// touches the PDF's bytes". Its body is `QPDF::emptyPDF()`, which is
/// `processMemoryFile("empty PDF", EMPTY_PDF, …)` — the full parser, with no `trap_errors`
/// wrapper. This constant goes in through the trapped `qpdf_read_memory` instead.
///
/// # What is in it
///
/// A catalog, a page tree, and a 1×1 page with no content stream and no resources. Nothing
/// that could reach an output: no metadata, no producer string, no dates. It is removed before
/// the write in any case, but "it gets removed" is a weaker guarantee than "there was nothing
/// in it", and ADR 0019 §2 is about what the emitted bytes contain.
///
/// The xref offsets are load-bearing and were **wrong when written by hand** — a unit test
/// computes them from the bytes rather than trusting the literal. The failure they prevent is
/// loud rather than silent: recovery is off, so a wrong offset makes `Document::open` return
/// `Malformed` and *every* extraction fail. (An earlier version of this comment claimed qpdf
/// would reconstruct the table and carry on. It does not, with recovery off, and the test is
/// worth having anyway — the constant is unreadable by eye.)
pub(crate) const BLANK_DOCUMENT: &[u8] = b"%PDF-1.7\n\
1 0 obj\n\
<< /Type /Catalog /Pages 2 0 R >>\n\
endobj\n\
2 0 obj\n\
<< /Type /Pages /Kids [3 0 R] /Count 1 >>\n\
endobj\n\
3 0 obj\n\
<< /Type /Page /Parent 2 0 R /MediaBox [0 0 1 1] /Resources << >> >>\n\
endobj\n\
xref\n\
0 4\n\
0000000000 65535 f \n\
0000000009 00000 n \n\
0000000058 00000 n \n\
0000000115 00000 n \n\
trailer\n\
<< /Size 4 /Root 1 0 R >>\n\
startxref\n\
199\n\
%%EOF\n";

#[cfg(test)]
mod tests {
    use super::BLANK_DOCUMENT;

    /// The constant's xref offsets are right.
    ///
    /// **Written by hand and wrong the first time** -- object 2 was recorded at 60 when it is
    /// at 58, and `startxref` at 117 when it is at 110. qpdf would have reconstructed the
    /// table and carried on, so every split output would silently have come from the repair
    /// path and nothing would have said so. That is why this is a test and not a comment
    /// saying the offsets are load-bearing.
    #[test]
    fn the_empty_documents_xref_points_where_it_says() {
        let doc = BLANK_DOCUMENT;
        let find = |needle: &[u8]| {
            doc.windows(needle.len())
                .position(|w| w == needle)
                .unwrap_or_else(|| panic!("{} not found", String::from_utf8_lossy(needle)))
        };

        let recorded = |line: usize| -> usize {
            // The xref entries are fixed-width: 10 digits, a space, 5 digits, a space, a
            // type letter, a space. Line 0 is the free head, so object N is line N.
            let xref = find(b"xref\n0 4\n") + b"xref\n0 4\n".len();
            let at = xref + line * 20;
            std::str::from_utf8(&doc[at..at + 10])
                .expect("ascii")
                .parse()
                .expect("a number")
        };

        assert_eq!(recorded(1), find(b"1 0 obj"), "object 1's offset");
        assert_eq!(recorded(2), find(b"2 0 obj"), "object 2's offset");
        assert_eq!(recorded(3), find(b"3 0 obj"), "object 3's offset");

        let startxref = find(b"startxref\n") + b"startxref\n".len();
        let stated: usize = std::str::from_utf8(&doc[startxref..startxref + 3])
            .expect("ascii")
            .parse()
            .expect("a number");
        assert_eq!(stated, find(b"xref\n"), "startxref");
    }

    /// It has exactly one page, and nothing else that could reach an output.
    #[test]
    fn the_blank_document_is_one_empty_page_and_nothing_more() {
        let text = String::from_utf8_lossy(BLANK_DOCUMENT);
        assert!(text.contains("/Count 1"), "{text}");
        // No content stream, no resources, and nothing that carries a string into an output.
        for forbidden in ["/Contents", "/Producer", "/Creator", "/Title", "/Metadata"] {
            assert!(
                !text.contains(forbidden),
                "the blank destination carries {forbidden}, which would reach every split output"
            );
        }
    }
}
