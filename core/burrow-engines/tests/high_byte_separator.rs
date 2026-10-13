//! #309: the bytes 0x80 and 0xFF between tokens are white space to burrow's lexer, PDFium and pdf.js
//! (#228 measured PDFium) and part of a word to MuPDF, Ghostscript and Poppler. So `100\x80Ts` is a
//! rise to burrow and PDFium and one unknown word to the other three: burrow placed SECRET above the
//! region where those three draw it inside, and returned `Ok` -- 394, 400 and 394 dark pixels on its
//! output, measured. PDFium reads it as burrow does, so verification could not see it.
//!
//! The rule (DECISIONS rules 1 and 4): redaction's lexer refuses either byte wherever it separates
//! tokens -- between two words, next to a delimiter, at the end of a stream, after `ID`, before and
//! after `EI` -- as `[high-byte-separator]`, in every stream redaction lexes. Inside a string, a
//! comment or inline-image data all five readers read the bytes alike, and they are accepted.
//! `split` keeps PDFium's reading (#310). The leak is in readers these tests cannot render, so each
//! shape asserts PDFium's half -- SECRET where burrow placed it, outside the region -- and then the
//! refusal by name; each twin puts a space where the byte was and redacts. The lexer's own test
//! sweeps every position; 0 of the 99 real documents carry a high-byte separator.

#![cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::collections::BTreeSet;

use burrow_engines::pdfsyntax::region::Region;
use support::char_box_oracle::chars_on_page;

/// The region, in display space: x 100..300, and y 250..300 in PDF space on a 400-point page.
const REGION: Region = Region {
    left: 100.0,
    top: 100.0,
    width: 200.0,
    height: 50.0,
};

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
             >> >>",
        ),
        stream(&with(procedure), ""),
        // `/F4`: Helvetica with a width for every code from `A`, 0x80 and 0xFF among them.
        format!(
            "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 65 /LastChar 255 \
             /Widths [{}] >>",
            vec!["500"; 191].join(" ")
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

/// A plain form and a plain glyph, for the shapes that put the byte on the page.
const FORM: &str = "0 0 m";
const GLYPH: &str = "600 0 d0 q 10 0 0 10 0 0 cm BI /W 1 /H 1 /BPC 1 /IM true ID \0 EI Q";

fn redact(pdf: &[u8]) -> burrow_types::Result<(Vec<u8>, burrow_engines::redact::Report)> {
    let covered: BTreeSet<usize> = [0].into_iter().collect();
    support::redact_page(pdf, 0, covered, REGION)
}

/// PDFium draws SECRET's `S` outside the region, where burrow's walk placed it. MuPDF, Ghostscript
/// and Poppler draw it inside (round 0), which no reader here can show.
#[track_caller]
fn pdfium_draws_secret_outside(what: &str, pdf: &[u8]) {
    let (x, y) = chars_on_page(pdf, 0)
        .into_iter()
        .find(|c| !c.generated && c.unicode == u32::from('S'))
        .map(|c| c.origin)
        .unwrap_or_else(|| panic!("{what}: PDFium draws no SECRET"));
    assert!(
        !((100.0..300.0).contains(&x) && (250.0..300.0).contains(&y)),
        "{what}: PDFium draws SECRET at ({x}, {y}), inside the region"
    );
}

#[track_caller]
fn refuses(what: &str, pdf: &[u8]) {
    match redact(pdf) {
        Err(error) => assert!(
            error.to_string().contains("[high-byte-separator]"),
            "{what}: refused, but not by this rule: {error}"
        ),
        Ok(_) => panic!("{what}: redacted, where [high-byte-separator] must refuse"),
    }
}

#[track_caller]
fn redacts(what: &str, pdf: &[u8]) {
    if let Err(error) = redact(pdf) {
        panic!("{what}: {error}");
    }
}

/// THE PAGE SHAPES round 0 measured leaking: a rise, a `Td` and a `cm` whose operator the three
/// readers do not see; `(A)\x80Tj` beside a delimiter, whose advance they never make; and `Q\x80`,
/// which they do not restore. Each for both bytes, and each twin with a space.
#[test]
fn a_high_byte_separating_tokens_on_the_page_refuses() {
    let shapes = [
        ("Ts", "100@Ts BT /F2 20 Tf 110 260 Td (SECRET) Tj ET"),
        ("Td", "BT /F2 20 Tf 110 260 Td 0 100@Td (SECRET) Tj ET"),
        (
            "cm",
            "1 0 0 1 0 100@cm BT /F2 20 Tf 110 260 Td (SECRET) Tj ET",
        ),
        (
            "beside a delimiter",
            "BT /F2 20 Tf 300 Tc 100 260 Td (A)@Tj (SECRET) Tj ET",
        ),
        (
            "Q",
            "q 1 0 0 1 0 -100 cm Q@BT /F2 20 Tf 110 360 Td (SECRET) Tj ET",
        ),
    ];
    for byte in [0x80u8, 0xff] {
        for (what, content) in shapes {
            let what = format!("{what}, {byte:#04x}");
            let pdf = document(&[content], FORM, GLYPH, byte);
            pdfium_draws_secret_outside(&what, &pdf);
            refuses(&what, &pdf);
            redacts(
                &format!("{what}, a space"),
                &document(&[content], FORM, GLYPH, b' '),
            );
        }
    }
}

/// IN EVERY STREAM REDACTION LEXES: a form's `cm`, a Type 3 glyph procedure (the scan's own entry
/// point), and the middle of one `/Contents` element on a page of two, which names this rule rather
/// than `[contents-boundary]`.
#[test]
fn a_high_byte_separator_in_a_form_a_glyph_or_a_contents_element_refuses() {
    for byte in [0x80u8, 0xff] {
        let in_form = |b| {
            document(
                &["/Fm0 Do"],
                "1 0 0 1 0 100@cm BT /F2 20 Tf 110 260 Td (SECRET) Tj ET",
                GLYPH,
                b,
            )
        };
        pdfium_draws_secret_outside("form", &in_form(byte));
        refuses("form", &in_form(byte));
        redacts("form, a space", &in_form(b' '));

        let in_glyph = |b| {
            document(
                &["BT /F3 20 Tf 10 10 Td (S) Tj ET"],
                FORM,
                "600 0 d0 q 10 0 0 10 0 0@cm BI /W 1 /H 1 /BPC 1 /IM true ID \0 EI Q",
                b,
            )
        };
        refuses("Type 3 procedure", &in_glyph(byte));
        redacts("Type 3 procedure, a space", &in_glyph(b' '));

        let split = |b| {
            document(
                &["100@Ts\n", "BT /F2 20 Tf 110 260 Td (SECRET) Tj ET\n"],
                FORM,
                GLYPH,
                b,
            )
        };
        pdfium_draws_secret_outside("contents", &split(byte));
        refuses("contents", &split(byte));
        redacts("contents, a space", &split(b' '));
    }
}

/// THE NEAR-MISSES, where all five readers read the byte alike: inside a comment, and inside a
/// literal string the region does not reach, shown in a font with a width for it.
#[test]
fn a_high_byte_inside_a_comment_or_a_string_redacts() {
    for byte in [0x80u8, 0xff] {
        redacts(
            "comment",
            &document(
                &["%c@\n BT /F2 20 Tf 110 260 Td (SECRET) Tj ET"],
                FORM,
                GLYPH,
                byte,
            ),
        );
        redacts(
            "string",
            &document(
                &["BT /F4 20 Tf 10 10 Td (A@B) Tj ET BT /F2 20 Tf 110 260 Td (SECRET) Tj ET"],
                FORM,
                GLYPH,
                byte,
            ),
        );
    }
}
