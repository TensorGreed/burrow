//! #312: a page's `/Contents` array is one content stream, divided only between tokens (PDF
//! 32000-1 §7.8.2), and readers join the elements differently. burrow and qpdf, whose join
//! verification reads back, insert a line feed; PDFium and MuPDF a space; Poppler nothing inside a
//! string; pdf.js nothing at all. Measured by #307's round 1 and #312's round 0 (region x 100..300,
//! y 250..300):
//!
//! - a comment with no line end (`%c` | `200 Ts …`) runs on through the next element in PDFium,
//!   MuPDF, Poppler and pdf.js, so the rise burrow applied is skipped: `Ok`, and SECRET drawn in the
//!   region -- 374 dark pixels in PDFium, 394 in MuPDF and Poppler;
//! - a word split over the boundary (`/G0 g` | `s`) is `gs` to pdf.js alone, which set a 200-point
//!   font and put SECRET in the region after an `Ok`.
//!
//! The rule: every element a boundary follows, empty ones skipped, ends in PDF white space with
//! burrow's lexer between tokens, or the page refuses `[contents-boundary]`. The comment shapes are
//! leaks in PDFium itself, so they are shown to be leaks here before they are shown to refuse; the
//! pdf.js shape asserts PDFium's half, SECRET outside the region. Each twin ends the element at a
//! line end and redacts clean. 0 of 915 boundaries in the 99 real documents are refused.

#![cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::collections::BTreeSet;

use burrow_engines::pdfsyntax::region::Region;
use support::char_box_oracle::{chars_on_page, dark_pixels_with_annotations};

/// The region, in display space: x 100..300, and y 250..300 in PDF space on a 400-point page.
const REGION: Region = Region {
    left: 100.0,
    top: 100.0,
    width: 200.0,
    height: 50.0,
};
const PIXELS: (i32, i32, i32, i32) = (100, 100, 200, 50);

fn stream(content: &[u8]) -> Vec<u8> {
    [
        format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        content,
        b"\nendstream",
    ]
    .concat()
}

/// A one-page document whose `/Contents` array is `elements`, with `/F2` Helvetica and `/G0` an
/// ExtGState that sets `/F2` at 200 points.
fn document(elements: &[&[u8]]) -> Vec<u8> {
    let first = 5;
    let references: Vec<String> = (0..elements.len())
        .map(|i| format!("{} 0 R", first + i))
        .collect();
    let page = format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] /Resources << /Font << /F2 4 0 R \
         >> /ExtGState << /G0 << /Font [4 0 R 200] >> >> >> /Contents [{}] >>",
        references.join(" ")
    );
    let mut all: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        page.into_bytes(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ];
    for element in elements {
        all.push(stream(element));
    }
    let mut out = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in all.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", all.len() + 1).as_bytes());
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            all.len() + 1
        )
        .as_bytes(),
    );
    out
}

fn redact(pdf: &[u8]) -> burrow_types::Result<(Vec<u8>, burrow_engines::redact::Report)> {
    let covered: BTreeSet<usize> = [0].into_iter().collect();
    support::redact_page(pdf, 0, covered, REGION)
}

/// Where PDFium draws SECRET's `S`, by its origin.
fn secret_origin(pdf: &[u8]) -> Option<(f64, f64)> {
    chars_on_page(pdf, 0)
        .into_iter()
        .find(|c| !c.generated && c.unicode == u32::from('S'))
        .map(|c| c.origin)
}

fn in_region((x, y): (f64, f64)) -> bool {
    (100.0..300.0).contains(&x) && (250.0..300.0).contains(&y)
}

#[track_caller]
fn refuses(what: &str, pdf: &[u8]) {
    match redact(pdf) {
        Err(error) => assert!(
            error.to_string().contains("[contents-boundary]"),
            "{what}: refused, but not by this rule: {error}"
        ),
        Ok(_) => panic!("{what}: redacted, where [contents-boundary] must refuse"),
    }
}

/// The shape is a leak in PDFium -- SECRET's `S` drawn inside the region -- and it refuses by name.
#[track_caller]
fn refuses_a_leak(what: &str, pdf: &[u8]) {
    let at = secret_origin(pdf).unwrap_or_else(|| panic!("{what}: PDFium draws no SECRET"));
    assert!(
        in_region(at),
        "{what}: PDFium draws SECRET at {at:?}, outside the region -- not a leak shape"
    );
    refuses(what, pdf);
}

/// The twin redacts, and leaves no ink in the region.
#[track_caller]
fn redacts_clean(what: &str, pdf: &[u8]) {
    let (out, _) = redact(pdf).unwrap_or_else(|error| panic!("{what}: {error}"));
    assert_eq!(
        dark_pixels_with_annotations(&out, 0, PIXELS).0,
        0,
        "{what}: ink left in the region"
    );
}

/// SECRET raised by 200 above the region, so a reader that skips the rise draws it inside.
const RAISED: &[u8] = b"200 Ts\nBT /F2 20 Tf 110 260 Td (SECRET) Tj ET";

/// THE COMMENT, round 0's Critical: an element ending in a comment with no line end, a comment then
/// a space, a comment then an empty element, and a comment after a token. PDFium runs the comment
/// on through the `200 Ts`, so SECRET is drawn in the region (asserted); burrow's line feed ended
/// it. Each twin ends the comment with a line feed or a carriage return, and PDFium raises SECRET
/// off the page, as burrow does.
#[test]
fn a_comment_the_boundary_would_end_refuses() {
    for (what, elements) in [
        ("%c", vec![b"%c".as_slice(), RAISED]),
        ("%c then a space", vec![b"%c ".as_slice(), RAISED]),
        (
            "%c then an empty element",
            vec![b"%c".as_slice(), b"", RAISED],
        ),
        (
            "a comment after a token",
            vec![b"0 g %c".as_slice(), RAISED],
        ),
    ] {
        refuses_a_leak(what, &document(&elements));
    }
    for (what, ended) in [("%c\\n", b"%c\n".as_slice()), ("%c\\r", b"%c\r")] {
        let twin = document(&[ended, RAISED]);
        let at = secret_origin(&twin).unwrap_or_else(|| panic!("{what}: no SECRET"));
        assert!(!in_region(at), "{what}: PDFium draws SECRET at {at:?}");
        redacts_clean(what, &twin);
    }
}

/// THE ISSUE'S SHAPE: `/G0 g` | `s` is gray then close-and-stroke to PDFium (SECRET at size 20,
/// below x 100 -- asserted) and `/G0 gs` to pdf.js, which round 1 measured setting the 200-point
/// font and drawing SECRET at x 110..230, in the region. The twin ends the first element with a
/// line feed, and the 20-point text that does reach the region is removed.
#[test]
fn a_word_split_over_the_boundary_refuses() {
    let first = b"BT /F2 20 Tf 10 260 Td /G0 g".as_slice();
    let second = b"s (XXXXXSECRET) Tj ET".as_slice();
    let pdf = document(&[first, second]);
    let at = secret_origin(&pdf).expect("PDFium draws SECRET");
    assert!(at.0 < 100.0, "PDFium draws SECRET at {at:?}");
    refuses("/G0 g | s", &pdf);
    refuses("100 T | s", &document(&[b"100 T", b"s"]));
    refuses("100 T | (empty) | s", &document(&[b"100 T", b"", b"s"]));
    redacts_clean(
        "/G0 g\\n | s",
        &document(&[b"BT /F2 20 Tf 10 260 Td /G0 g\n", second]),
    );
}

/// A STRING OR A HEX STRING OVER THE BOUNDARY: readers put a line feed, a space, nothing or an
/// end there (round 0), so the glyph run differs. Refused whether the element ends mid-word or in
/// white space inside the string. The twin splits a text object between whole tokens, which ADR
/// 0029 §3 handles, and redacts clean.
#[test]
fn a_string_over_the_boundary_refuses() {
    for (what, first, second) in [
        (
            "(SEC | RET)",
            b"BT /F2 20 Tf 110 260 Td (SEC".as_slice(),
            b"RET) Tj ET".as_slice(),
        ),
        (
            "( | SECRET)",
            b"BT /F2 20 Tf 110 260 Td ( ",
            b"SECRET) Tj ET",
        ),
        (
            "<53 | 45>",
            b"BT /F2 20 Tf 110 260 Td <53 ",
            b"45> Tj (SECRET) Tj ET",
        ),
    ] {
        refuses(what, &document(&[first, second]));
    }
    redacts_clean(
        "BT | ET",
        &document(&[b"BT /F2 20 Tf 110 260 Td\n", b"(SECRET) Tj ET"]),
    );
}

/// AN INLINE IMAGE OVER THE BOUNDARY, in its dictionary or its data: every reader read these alike
/// in round 0, so this is the wider rule's recorded over-refusal (DECISIONS rule 4). The twin
/// closes the image before the boundary.
#[test]
fn an_inline_image_over_the_boundary_refuses() {
    let after = b"BT /F2 20 Tf 110 260 Td (SECRET) Tj ET".as_slice();
    refuses(
        "the dictionary",
        &document(&[
            b"q 10 0 0 10 20 20 cm BI /W 1 ",
            b"/H 1 /BPC 8 /CS /G ID \x00 EI Q\n",
            after,
        ]),
    );
    refuses(
        "the data",
        &document(&[
            b"q 10 0 0 10 20 20 cm BI /W 1 /H 1 /BPC 8 /CS /G ID \x00 ",
            b"EI Q\n",
            after,
        ]),
    );
    redacts_clean(
        "closed",
        &document(&[
            b"q 10 0 0 10 20 20 cm BI /W 1 /H 1 /BPC 8 /CS /G ID \x00 EI Q\n",
            after,
        ]),
    );
}
