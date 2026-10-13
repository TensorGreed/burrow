//! #320: pdf.js ends an unfiltered inline image by scanning its bytes, not by the size its
//! dictionary implies, as burrow, PDFium, MuPDF, Ghostscript and Poppler do. It takes the first `EI`
//! and SP, LF or CR whose next 15 bytes are printable and whose next 75 reach a known operator, and
//! otherwise falls back to the last candidate it saw (`findDefaultInlineStreamEnd`, 5.7.284). So an
//! `EI` inside the data ends the image early, and the text after it -- image bytes to the other
//! four -- is drawn; and an `EI` pdf.js does not accept lets it scan on past burrow's end, into a
//! later string it then resumes inside. #320's round 0 measured both: burrow `Ok`, the other four 0
//! dark pixels in the region, pdf.js's operator list drawing SECRET at (110, 260).
//!
//! The rule (DECISIONS rules 1 and 4), in redaction's lexer mode, for an unfiltered image:
//! `[inline-image-data-holds-ei]` for data holding an `EI` a scan could stop at, or ending in `E`
//! right before the `EI`; `[inline-image-end-unrecognised]` for an `EI` not followed by SP, LF or CR,
//! or by 64 bytes (line feeds aside) that are not all printable, or by a next word that is not `Q`,
//! `q`, `EMC`, `BT`, `ET` or `BI` within them. The lexer's own test pins each byte position; these
//! put the shapes on a page, in a form and in a Type 3 glyph, and assert PDFium's half where it has
//! one -- no SECRET drawn, since PDFium reads it as image data. Each twin is the same image with
//! data and an end every reader agrees on. 0 of the 75 unfiltered inline images in the 99 real
//! documents are refused.

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

/// A Type 3 font whose one glyph, `S` at code 83, runs `procedure`.
const TYPE_THREE: &str = "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 600 700] \
     /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /S 7 0 R >> \
     /Encoding << /Type /Encoding /Differences [83 /S] >> /FirstChar 83 /LastChar 83 \
     /Widths [600] /Resources << >> >>";

fn stream(content: &[u8], dictionary: &str) -> Vec<u8> {
    [
        format!("<< {dictionary} /Length {} >>\nstream\n", content.len()).as_bytes(),
        content,
        b"\nendstream",
    ]
    .concat()
}

/// A one-page document whose `/Contents` array is `elements`, with `/F2` Helvetica, `/F3` a Type 3
/// font whose glyph runs `procedure`, `/F4` Helvetica with `/Widths`, and `/Fm0` a form drawing
/// `form`. `@` in any of them stands
/// for `byte`.
fn document(elements: &[&str], form: &str, procedure: &str, byte: u8) -> Vec<u8> {
    let with = |text: &str| -> Vec<u8> {
        text.bytes()
            .map(|b| if b == b'@' { byte } else { b })
            .collect()
    };
    let first_element = 9;
    let contents: Vec<String> = (0..elements.len())
        .map(|i| format!("{} 0 R", first_element + i))
        .collect();
    let page = format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] /Resources << /Font << /F2 4 0 R \
         /F3 5 0 R /F4 8 0 R >> /XObject << /Fm0 6 0 R >> >> /Contents [{}] >>",
        contents.join(" ")
    );
    let mut all: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        page.into_bytes(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
        TYPE_THREE.as_bytes().to_vec(),
        stream(
            &with(form),
            "/Type /XObject /Subtype /Form /BBox [0 0 400 400] /Resources << /Font << /F2 4 0 R \
             /F4 8 0 R >> >>",
        ),
        stream(&with(procedure), ""),
        // `/F4`: Helvetica with a width for every code.
        format!(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 0 /LastChar 255 \
             /Widths [{}] >>",
            vec!["500"; 256].join(" ")
        )
        .into_bytes(),
    ];
    for element in elements {
        all.push(stream(&with(element), ""));
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

/// A plain form and a plain glyph, for the shapes that put the image on the page.
const FORM: &str = "0 0 m";
const GLYPH: &str = "600 0 d0 q 10 0 0 10 0 0 cm BI /W 1 /H 1 /BPC 1 /IM true ID A EI Q";

fn redact(pdf: &[u8]) -> burrow_types::Result<(Vec<u8>, burrow_engines::redact::Report)> {
    let covered: BTreeSet<usize> = [0].into_iter().collect();
    support::redact_page(pdf, 0, covered, REGION)
}

/// PDFium reads the hidden text object as image bytes and draws no character from it.
#[track_caller]
fn pdfium_draws_no_secret(what: &str, pdf: &[u8]) {
    assert!(
        !chars_on_page(pdf, 0)
            .iter()
            .any(|c| !c.generated && c.unicode == u32::from('S')),
        "{what}: PDFium draws SECRET, so the image does not hide it"
    );
}

#[track_caller]
fn refuses(what: &str, pdf: &[u8], rule: &str) {
    match redact(pdf) {
        Err(error) => assert!(
            error.to_string().contains(rule),
            "{what}: refused, but not by {rule}: {error}"
        ),
        Ok(_) => panic!("{what}: redacted, where {rule} must refuse"),
    }
}

/// The twin has SECRET's ink in the region, redacts, and leaves none there.
#[track_caller]
fn redacts_clean(what: &str, pdf: &[u8]) {
    assert!(
        dark_pixels_with_annotations(pdf, 0, PIXELS).0 > 0,
        "{what}: no ink in the region to remove"
    );
    let (out, _) = redact(pdf).unwrap_or_else(|error| panic!("{what}: {error}"));
    assert_eq!(
        dark_pixels_with_annotations(&out, 0, PIXELS).0,
        0,
        "{what}: ink left in the region"
    );
}

/// SECRET in the region, which the twins draw after the image for the redaction to remove.
const IN_REGION: &str = "BT /F2 20 Tf 110 260 Td (SECRET) Tj ET";

/// A one-row gray inline image at (20, 20) whose data is `data`, ended by `end`.
fn gray(data: &str, end: &str) -> String {
    format!(
        "q 10 0 0 10 20 20 cm BI /W {} /H 1 /BPC 8 /CS /G ID {data} {end}",
        data.len()
    )
}

/// THE ISSUE'S SHAPE, on a page and in a form: data that holds `\nEI Q BT .. (SECRET) Tj ET q\n`,
/// which pdf.js ends at that `EI` and draws, and PDFium reads as image bytes. The twin's data is
/// the same length with no `EI` in it, and the SECRET it draws after the image is removed.
#[test]
fn image_data_pdfjs_ends_early_refuses_on_a_page_and_in_a_form() {
    let hidden = format!("X\nEI Q {IN_REGION} q\n");
    let leak = gray(&hidden, "EI Q");
    let twin = format!("{} {IN_REGION}", gray(&"x".repeat(hidden.len()), "EI Q"));
    let page = document(&[&leak], FORM, GLYPH, b'@');
    pdfium_draws_no_secret("page", &page);
    refuses("page", &page, "[inline-image-data-holds-ei]");
    redacts_clean("page twin", &document(&[&twin], FORM, GLYPH, b'@'));

    let form = document(&["/Fm0 Do"], &leak, GLYPH, b'@');
    pdfium_draws_no_secret("form", &form);
    refuses("form", &form, "[inline-image-data-holds-ei]");
    redacts_clean("form twin", &document(&["/Fm0 Do"], &twin, GLYPH, b'@'));
}

/// IN A TYPE 3 GLYPH: an image mask whose data hides a fill pdf.js would draw in glyph space, which
/// the procedure scan reads as image bytes. The twin's mask data has no `EI`, and it refuses as the
/// image glyph in the region it is, `[type-three-image-cut]`.
#[test]
fn image_data_pdfjs_ends_early_in_a_type_three_glyph_refuses() {
    let procedure = |data: &str| {
        format!(
            "600 0 d0 q 10 0 0 10 0 0 cm BI /W {} /H 1 /BPC 1 /IM true ID {data} EI Q",
            8 * data.len()
        )
    };
    let hidden = "X\nEI Q 0 0 600 700 re f q\n";
    let shown = "BT /F3 20 Tf 110 260 Td (S) Tj ET";
    refuses(
        "Type 3",
        &document(&[shown], FORM, &procedure(hidden), b'@'),
        "[inline-image-data-holds-ei]",
    );
    // The twin is an image glyph the region reaches, which refuses for that reason and not this.
    refuses(
        "Type 3 twin",
        &document(&[shown], FORM, &procedure(&"x".repeat(hidden.len())), b'@'),
        "[type-three-image-cut]",
    );
}

/// AN `EI` PDF.JS DOES NOT TAKE, on a page and in a form: followed by a tab, so pdf.js scans on to
/// the `EI` inside a later string and resumes there, drawing the hex SECRET the string carries;
/// burrow draws the string at (10, 10). The twin ends the image with a space.
#[test]
fn an_end_pdfjs_reads_past_refuses_on_a_page_and_in_a_form() {
    let after = format!(
        "BT /F4 20 Tf 10 10 Td (EI\n BT /F2 20 Tf 110 260 Td <534543524554> Tj ET %) Tj ET {IN_REGION}"
    );
    let leak = format!("{} {after}", gray("X", "EI\tQ"));
    let twin = format!("{} {after}", gray("X", "EI Q"));
    refuses(
        "page",
        &document(&[&leak], FORM, GLYPH, b'@'),
        "[inline-image-end-unrecognised]",
    );
    redacts_clean("page twin", &document(&[&twin], FORM, GLYPH, b'@'));
    refuses(
        "form",
        &document(&["/Fm0 Do"], &leak, GLYPH, b'@'),
        "[inline-image-end-unrecognised]",
    );
    redacts_clean("form twin", &document(&["/Fm0 Do"], &twin, GLYPH, b'@'));
}
