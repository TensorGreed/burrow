//! #307: readers do not agree on where a word in a content stream ends. burrow's lexer, PDFium and
//! MuPDF read `100Ts` as one unknown keyword and skip it; Ghostscript, Poppler and pdf.js read `100`
//! and `Ts` and raise the text. pdf.js also splits an unknown word on the longest operator it begins
//! with (`100 Tsx`), and treats bytes such as 0x0B and 0xA0 as white space where the others do not.
//! Measured by #304's round-1 and #307's round-0 security reviews: `100Ts` before SECRET at y 160
//! returned `Ok`, and on burrow's output Ghostscript drew 400 dark pixels in the region, Poppler 395,
//! and pdf.js put SECRET at y 260; `100 Tsx`, `100\x0bTs` and `100\xa0Ts` did the same in pdf.js. A
//! Type 3 glyph procedure is read by a scan of its own, and `re 0f` there drew 10,000 dark pixels in
//! Ghostscript and Poppler after an `Ok`.
//!
//! The rule (DECISIONS rules 1 and 4): a word that is not a PDF content operator refuses
//! `[unknown-operator]`, on the page, in a form, and in a Type 3 procedure. The native oracle is
//! PDFium, which reads these shapes as burrow did, so the leak shapes here assert PDFium's half --
//! SECRET drawn outside the region -- and the other readers' figures are in this header. Each twin
//! spells the same thing as operators and redacts clean. 0 of 99 real documents carry an unknown
//! word, in 3,256 content streams. 0x80 and 0xFF, which split in the opposite direction, are #309.

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

const HELVETICA: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>";

/// A Type 3 font whose one glyph, `S` at code 83, is drawn by the procedure the test supplies.
const TYPE_THREE: &str = "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 600 700] \
     /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /S 6 0 R >> \
     /Encoding << /Type /Encoding /Differences [83 /S] >> /FirstChar 83 /LastChar 83 \
     /Widths [600] /Resources << >> >>";

/// A Type 3 procedure that paints a box, which the scan refuses for its own reason.
const BOX: &[u8] = b"600 0 d0 0 0 600 700 re f";

fn stream(content: &[u8], dictionary: &str) -> Vec<u8> {
    [
        format!("<< {dictionary} /Length {} >>\nstream\n", content.len()).as_bytes(),
        content,
        b"\nendstream",
    ]
    .concat()
}

/// A one-page document drawing `content`, with `/F2` Helvetica, `/F3` a Type 3 font whose `S` is
/// `procedure`, and `form`, when given, as `/Fm0`.
fn document(content: &[u8], procedure: &[u8], form: Option<&str>) -> Vec<u8> {
    let xobjects = if form.is_some() {
        "/XObject << /Fm0 8 0 R >>"
    } else {
        ""
    };
    let page = format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] /Resources << /Font << /F2 4 0 R \
         /F3 5 0 R >> {xobjects} >> /Contents 7 0 R >>"
    );
    let mut all: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        page.into_bytes(),
        HELVETICA.as_bytes().to_vec(),
        TYPE_THREE.as_bytes().to_vec(),
        stream(procedure, ""),
        stream(content, ""),
    ];
    if let Some(form) = form {
        all.push(stream(
            form.as_bytes(),
            "/Type /XObject /Subtype /Form /BBox [0 0 400 400] /Resources << /Font << /F2 4 0 R \
             >> >>",
        ));
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

fn page(content: &[u8]) -> Vec<u8> {
    document(content, BOX, None)
}

fn redact(pdf: &[u8]) -> burrow_types::Result<(Vec<u8>, burrow_engines::redact::Report)> {
    let covered: BTreeSet<usize> = [0].into_iter().collect();
    support::redact_page(pdf, 0, covered, REGION)
}

/// PDFium draws SECRET's `S` at `(x, y)`, to within a point.
#[track_caller]
fn secret_drawn_at(what: &str, pdf: &[u8], x: f64, y: f64) {
    let (at_x, at_y) = chars_on_page(pdf, 0)
        .into_iter()
        .find(|c| !c.generated && c.unicode == u32::from('S'))
        .map(|c| c.origin)
        .unwrap_or_else(|| panic!("{what}: PDFium draws no SECRET"));
    assert!(
        (at_x - x).abs() < 1.0 && (at_y - y).abs() < 1.0,
        "{what}: PDFium draws SECRET at ({at_x}, {at_y}), not ({x}, {y})"
    );
}

#[track_caller]
fn refuses_as(what: &str, pdf: &[u8], rule: &str) {
    match redact(pdf) {
        Err(error) => assert!(
            error.to_string().contains(rule),
            "{what}: refused, but not by {rule}: {error}"
        ),
        Ok(_) => panic!("{what}: redacted, where {rule} must refuse"),
    }
}

#[track_caller]
fn refuses(what: &str, pdf: &[u8]) {
    refuses_as(what, pdf, "[unknown-operator]");
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

const BELOW: &str = "BT /F2 20 Tf 110 160 Td (SECRET) Tj ET";

/// THE GLUED NUMBER, the issue's shape: before the text, after it, and before a form `Do`. PDFium
/// skips `100Ts` and draws SECRET below the region, where the other three readers raise it into
/// it. The twin spells `100 Ts`: every reader raises SECRET into the region, and it is removed;
/// before a form, the same spelling refuses for #304's reason instead.
#[test]
fn a_number_glued_to_an_operator_refuses_wherever_it_stands() {
    let before = page(format!("100Ts {BELOW}").as_bytes());
    secret_drawn_at("100Ts", &before, 110.0, 160.0);
    refuses("100Ts before the text", &before);
    refuses(
        "100Ts after the text",
        &page(format!("{BELOW} 100Ts").as_bytes()),
    );
    refuses(
        "100Ts before a form",
        &document(b"100Ts /Fm0 Do", BOX, Some(BELOW)),
    );

    let twin = page(format!("100 Ts {BELOW}").as_bytes());
    secret_drawn_at("100 Ts", &twin, 110.0, 260.0);
    redacts_clean("100 Ts", &twin);
    refuses_as(
        "100 Ts before a form",
        &document(b"100 Ts /Fm0 Do", BOX, Some(BELOW)),
        "[text-state-into-form]",
    );
}

/// THE SHAPES ONLY PDF.JS SPLITS: an operator with a suffix, two operators glued, and a byte that
/// is white space to pdf.js alone (0x0B, 0xA0). PDFium reads each as one unknown word and draws
/// SECRET below the region. `TsBT` refuses by this rule's name, not as text outside a text object,
/// because the word is judged before the `Tf` after it.
#[test]
fn a_word_pdf_js_alone_splits_refuses() {
    for (what, glue) in [
        ("100 Tsx", b"100 Tsx".as_slice()),
        ("100 Ts\\x0b", b"100\x0bTs".as_slice()),
        ("100 Ts\\xa0", b"100\xa0Ts".as_slice()),
    ] {
        let pdf = page(&[glue, b" ", BELOW.as_bytes()].concat());
        secret_drawn_at(what, &pdf, 110.0, 160.0);
        refuses(what, &pdf);
    }
    refuses(
        "100 TsBT",
        &page(b"100 TsBT /F2 20 Tf 110 160 Td (SECRET) Tj ET"),
    );
}

/// AN UNKNOWN WORD WITH OPERANDS AFTER IT: pdf.js carries `100` across it to the `Ts`, the others
/// clear it. The word refuses before the `Ts`'s count is read, so the rule is named.
#[test]
fn an_unknown_word_between_operands_and_their_operator_refuses_by_this_rule() {
    for word in ["null", "foo"] {
        refuses(word, &page(format!("100 {word} Ts {BELOW}").as_bytes()));
    }
}

/// THE TYPE 3 SCAN, which reads its procedures in a loop of its own. A glued paint, `re 0f`, and a
/// glued transform before an inline image, `4500 0cm`, are a box over the region and an image moved
/// out of the glyph's box to Ghostscript and Poppler; the scan saw neither. Each twin spaces the
/// word, and refuses for the paint's or the image's own reason. The leak is ink, not a character's
/// position -- PDFium reports no character for a procedure holding an unknown word -- so nothing
/// here is asked of the native oracle; round 0's figures are in this file's header.
#[test]
fn a_glued_operator_in_a_type_three_procedure_refuses() {
    let shown = b"BT /F3 20 Tf 110 260 Td (S) Tj ET";
    for (procedure, twin, rule) in [
        (
            b"600 0 d0 0 0 600 700 re 0f".as_slice(),
            b"600 0 d0 0 0 600 700 re f".as_slice(),
            "[type-three-procedure-paints]",
        ),
        (
            b"600 0 d0 q 10 0 0 10 4500 0cm BI /W 1 /H 1 /BPC 1 /IM true ID \x00 EI Q".as_slice(),
            b"600 0 d0 q 10 0 0 10 4500 0 cm BI /W 1 /H 1 /BPC 1 /IM true ID \x00 EI Q".as_slice(),
            "[type-three-image-outside-its-box]",
        ),
    ] {
        let what = String::from_utf8_lossy(procedure).into_owned();
        refuses(&what, &document(shown, procedure, None));
        refuses_as(&what, &document(shown, twin, None), rule);
    }
}

/// THE RECORDED OVER-REFUSAL (DECISIONS rule 4): an unknown operator inside `BX`...`EX`, which the
/// specification lets a reader skip, refuses too. The twin's compatibility section holds only
/// known operators, and redacts clean.
#[test]
fn an_unknown_operator_inside_a_compatibility_section_refuses() {
    let raised = "BT /F2 20 Tf 110 260 Td (SECRET) Tj ET";
    refuses(
        "BX foo EX",
        &page(format!("BX 1 foo EX {raised}").as_bytes()),
    );
    redacts_clean(
        "BX known EX",
        &page(format!("BX 0 Ts EX {raised}").as_bytes()),
    );
}

/// THE NUMBERS THAT STAY: each spelling round 0 measured all five readers reading alike, and an
/// inline image whose dictionary holds `true` -- a value there, which arrives as a bare word. SECRET
/// is raised into the region by a `. 100 Td`, where PDFium draws it, and removed.
#[test]
fn numbers_every_reader_reads_alike_and_an_image_dictionarys_values_redact() {
    let pdf = page(
        b"+100 Ts 100. Ts -.5 Ts 00000100 Ts 0 Ts \
          q 10 0 0 10 20 20 cm BI /W 1 /H 1 /BPC 1 /IM true ID \x00 EI Q \
          BT /F2 20 Tf 110 160 Td . 100 Td (SECRET) Tj ET",
    );
    secret_drawn_at("the numbers", &pdf, 110.0, 260.0);
    redacts_clean("the numbers", &pdf);
}
