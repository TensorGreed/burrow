//! #304: PDFium starts a Form XObject with the text rise (`Ts`), horizontal scaling (`Tz`) and
//! leading (`TL`) at their defaults; MuPDF, Ghostscript, Poppler and pdf.js carry the caller's
//! values in, as the specification says. burrow's walk carried them in too, so a caller's `100 Ts`
//! over a form placed SECRET above the region for burrow while PDFium drew it inside: `Ok` with
//! SECRET intact, measured. Resetting them as PDFium does would open the reverse leak in the other
//! four readers (#304's round 0 measured each of them), and verification reads back through PDFium,
//! so it could not see that one.
//!
//! The rule (DECISIONS rules 1 and 4): a form drawn while any of the three is off its default
//! refuses `[text-state-into-form]`. One drawn with all three at their defaults starts alike in every
//! reader. Each leak shape here is first shown to be a leak in PDFium -- SECRET drawn inside the
//! region -- and then shown to refuse; the reverse shapes, where PDFium draws SECRET outside and the
//! other readers inside, are shown to be outside for PDFium and then to refuse. The twins set the
//! value back, close it in a `q`/`Q`, or set it after the `Do`, and redact clean. The list is
//! round 0's: the three fields, `'` and `"`, `TJ`, nested forms and a Type 3 font in a form, with
//! the recorded over-refusal of the wider rule beside them.

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

/// A Type 3 font whose one glyph, `S` at code 83, is a filled box 600 wide.
const TYPE_THREE: &str = "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 600 700] \
     /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /S 6 0 R >> \
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

/// A one-page document drawing `content`, with `/F2` Helvetica and `/F3` a Type 3 font, and each
/// of `forms` as a chain: the page's resources name `/Fm0`, `/Fm0`'s name `/Fm1`, and so on, so
/// each form is reached from one scope only -- one named twice is a template the redaction refuses
/// to edit for one page.
fn document(content: &str, forms: &[&str]) -> Vec<u8> {
    let first_form = 8;
    let resources = |from: usize| {
        let xobjects: String = (from..forms.len().min(from + 1))
            .map(|i| format!("/Fm{i} {} 0 R ", first_form + i))
            .collect();
        format!("<< /Font << /F2 4 0 R /F3 5 0 R >> /XObject << {xobjects}>> >>")
    };
    let page_resources = resources(0);
    let page = format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] /Resources {page_resources} /Contents 7 \
         0 R >>"
    );
    let mut all: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        page.into_bytes(),
        HELVETICA.as_bytes().to_vec(),
        TYPE_THREE.as_bytes().to_vec(),
        stream(b"600 0 d0 0 0 600 700 re f", ""),
        stream(content.as_bytes(), ""),
    ];
    for (i, form) in forms.iter().enumerate() {
        all.push(stream(
            form.as_bytes(),
            &format!(
                "/Type /XObject /Subtype /Form /BBox [0 0 400 400] /Resources {}",
                resources(i + 1)
            ),
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

fn redact(pdf: &[u8]) -> burrow_types::Result<(Vec<u8>, burrow_engines::redact::Report)> {
    let covered: BTreeSet<usize> = [0].into_iter().collect();
    support::redact_page(pdf, 0, covered, REGION)
}

fn dark_in_region(pdf: &[u8]) -> u64 {
    dark_pixels_with_annotations(pdf, 0, PIXELS).0
}

/// Where PDFium draws SECRET's `S`, by its origin.
fn secret_origin(pdf: &[u8]) -> Option<(f64, f64)> {
    chars_on_page(pdf, 0)
        .into_iter()
        .find(|c| !c.generated && c.unicode == u32::from('S'))
        .map(|c| c.origin)
}

/// PDFium draws SECRET's `S` at `(x, y)`, to within a point.
#[track_caller]
fn secret_drawn_at(what: &str, pdf: &[u8], x: f64, y: f64) {
    let (at_x, at_y) =
        secret_origin(pdf).unwrap_or_else(|| panic!("{what}: PDFium draws no SECRET"));
    assert!(
        (at_x - x).abs() < 1.0 && (at_y - y).abs() < 1.0,
        "{what}: PDFium draws SECRET at ({at_x}, {at_y}), not ({x}, {y})"
    );
}

/// The shape is a leak -- PDFium draws SECRET's `S` inside the region -- and it refuses by name.
#[track_caller]
fn refuses_a_leak(what: &str, pdf: &[u8]) {
    let (x, y) = secret_origin(pdf).unwrap_or_else(|| panic!("{what}: PDFium draws no SECRET"));
    assert!(
        (100.0..300.0).contains(&x) && (250.0..300.0).contains(&y),
        "{what}: PDFium draws SECRET at ({x}, {y}), outside the region -- not a leak shape"
    );
    refuses(what, pdf);
}

#[track_caller]
fn refuses(what: &str, pdf: &[u8]) {
    match redact(pdf) {
        Err(error) => assert!(
            error.to_string().contains("[text-state-into-form]"),
            "{what}: refused, but not by this rule: {error}"
        ),
        Ok(_) => panic!("{what}: redacted, where [text-state-into-form] must refuse"),
    }
}

/// The twin redacts, and leaves no ink in the region.
#[track_caller]
fn redacts_clean(what: &str, pdf: &[u8]) {
    let (out, _) = redact(pdf).unwrap_or_else(|error| panic!("{what}: {error}"));
    assert_eq!(dark_in_region(&out), 0, "{what}: ink left in the region");
}

const IN_REGION: &str = "BT /F2 20 Tf 110 260 Td (SECRET) Tj ET";

/// THE ISSUE'S THREE SHAPES, and the leading reaching the form through `'` and `"` as well as `T*`.
/// Each field set by the caller and dropped by PDFium leaves SECRET inside the region, where burrow
/// placed it above (`Ts`), below (`TL`) or past (`Tz`) it. Every twin draws the same form after the
/// value is set back to its default, and redacts clean.
#[test]
fn a_form_drawn_under_the_callers_rise_scale_or_leading_refuses() {
    for (caller, form) in [
        ("100 Ts", IN_REGION),
        ("200 TL", "BT /F2 20 Tf 110 260 Td T* (SECRET) Tj ET"),
        ("200 TL", "BT /F2 20 Tf 110 260 Td (SECRET) ' ET"),
        ("200 TL", "BT /F2 20 Tf 110 260 Td 0 0 (SECRET) \" ET"),
        // `TD` SETS THE LEADING TOO (TL = -ty), and it outlives the text object that ran it. This is
        // the shape #304's census found on a real page: dvipdfmx moves by `TD`, and wraps each image
        // in a form drawn later.
        (
            "BT 0 -200 TD ET",
            "BT /F2 20 Tf 110 260 Td T* (SECRET) Tj ET",
        ),
        ("1000 Tz", "BT /F2 20 Tf 110 260 Td (W) Tj (SECRET) Tj ET"),
    ] {
        refuses_a_leak(caller, &document(&format!("{caller} /Fm0 Do"), &[form]));
    }
    // MEASURED, not stated: the `Tz` shape's S follows one unscaled W, at 110 + 18.9.
    secret_drawn_at(
        "1000 Tz",
        &document(
            "1000 Tz /Fm0 Do",
            &["BT /F2 20 Tf 110 260 Td (W) Tj (SECRET) Tj ET"],
        ),
        128.9,
        260.0,
    );
    for (caller, form) in [
        ("100 Ts 0 Ts", IN_REGION),
        ("200 TL 0 TL", "BT /F2 20 Tf 110 260 Td T* (SECRET) Tj ET"),
        // `0 0 TD` sets the leading to -0, which is the default: no reader moves a `T*` by it.
        (
            "BT 0 -200 TD 0 0 TD ET",
            "BT /F2 20 Tf 110 260 Td T* (SECRET) Tj ET",
        ),
        (
            "1000 Tz 100 Tz",
            "BT /F2 20 Tf 110 260 Td (W) Tj (SECRET) Tj ET",
        ),
    ] {
        redacts_clean(caller, &document(&format!("{caller} /Fm0 Do"), &[form]));
    }
}

/// THE REVERSE SHAPES: a form placed so PDFium's reset puts SECRET outside the region, which the
/// other readers draw inside under the caller's value. Round 0 measured, region x 100..300, y
/// 250..300, dark pixels in it: `100 Ts` -- MuPDF 394, Ghostscript 400, Poppler 394, pdf.js SECRET
/// at (110, 260); `-200 TL` -- 394, 400, 394, (110, 260); `30 Tz` with a -18000 `TJ` kern -- 120,
/// 151, 134, x 108. The native oracle is PDFium alone, so what is asserted here is PDFium's half:
/// SECRET outside. Modelling PDFium's reset would place it there too and return `Ok`; the rule
/// refuses before either reading is taken.
#[test]
fn a_form_placed_to_dodge_the_region_under_pdfiums_reset_refuses() {
    for (caller, form, x, y) in [
        (
            "100 Ts",
            "BT /F2 20 Tf 110 160 Td (SECRET) Tj ET",
            110.0,
            160.0,
        ),
        (
            "-200 TL",
            "BT /F2 20 Tf 110 60 Td T* (SECRET) Tj ET",
            110.0,
            60.0,
        ),
        (
            "30 Tz",
            "BT /F2 20 Tf 0 260 Td [-18000 (SECRET)] TJ ET",
            360.0,
            260.0,
        ),
    ] {
        let pdf = document(&format!("{caller} /Fm0 Do"), &[form]);
        secret_drawn_at(caller, &pdf, x, y);
        refuses(caller, &pdf);
    }
}

/// NESTED FORMS: PDFium resets at every form boundary, measured by round 0, so a value set by the
/// page and one set by an outer form both stop at the inner form for PDFium and reach it for the
/// other readers. Both refuse -- the page's at the outer `Do`, the outer form's at the inner. The
/// twin nests the same two forms with nothing set, and redacts clean.
#[test]
fn a_value_carried_into_a_nested_form_refuses_at_whichever_do_it_crosses() {
    refuses_a_leak(
        "the page's rise, through Fm0 into Fm1",
        &document("100 Ts /Fm0 Do", &["/Fm1 Do", IN_REGION]),
    );
    refuses_a_leak(
        "Fm0's own rise, into Fm1",
        &document("/Fm0 Do", &["100 Ts /Fm1 Do", IN_REGION]),
    );
    redacts_clean(
        "nested, nothing set",
        &document("/Fm0 Do", &["/Fm1 Do", IN_REGION]),
    );
}

/// A TYPE 3 FONT shown in the form: the rule fires at the `Do`, before any glyph of any font kind
/// is placed, so a Type 3 glyph under a caller's rise refuses the same way.
#[test]
fn a_type_three_glyph_in_a_form_under_the_callers_rise_refuses() {
    refuses_a_leak(
        "Type 3, 100 Ts",
        &document("100 Ts /Fm0 Do", &["BT /F3 20 Tf 110 260 Td (S) Tj ET"]),
    );
}

/// THE TWINS THAT KEEP THE VALUE OUT OF THE FORM: set inside a `q`/`Q` that closes before the
/// `Do`, or set after it. And `Tc`, which all five readers carry into a form, measured, so it is
/// not refused: the form still draws SECRET in the region, and the redaction removes it.
#[test]
fn a_value_closed_off_before_the_do_or_set_after_it_redacts() {
    redacts_clean(
        "q 100 Ts 1000 Tz 200 TL Q",
        &document("q 100 Ts 1000 Tz 200 TL Q /Fm0 Do", &[IN_REGION]),
    );
    redacts_clean(
        "after the Do",
        &document("/Fm0 Do 100 Ts 1000 Tz 200 TL", &[IN_REGION]),
    );
    let spaced = document("50 Tc /Fm0 Do", &[IN_REGION]);
    secret_drawn_at("50 Tc", &spaced, 110.0, 260.0);
    redacts_clean("50 Tc", &spaced);
}

/// THE RECORDED OVER-REFUSAL (DECISIONS rule 4). The rule reads the caller's state at the `Do`, not
/// what the form does with it, so these refuse though every reader draws them alike: a form that
/// sets its own rise before showing, a caller's leading under a form that moves only by `Td`, and a
/// form that paints no text. The last is the one real documents carry: #304's census found it on 1
/// of 99, `tug2003-slides.pdf` (dvipdfmx), whose image and shading forms are drawn after a `TD` has
/// left the leading set -- 48 tiles over two pages, each already refused for the shading's ink. A narrower rule would carry a second text state through every form
/// in step with PDFium's; #304's census measured what the wider one costs instead.
#[test]
fn a_form_that_never_reads_the_callers_value_still_refuses() {
    for (caller, form) in [
        ("100 Ts", "0 Ts BT /F2 20 Tf 110 260 Td (SECRET) Tj ET"),
        ("200 TL", IN_REGION),
        ("100 Ts", "10 10 5 5 re f"),
    ] {
        refuses(caller, &document(&format!("{caller} /Fm0 Do"), &[form]));
    }
}
