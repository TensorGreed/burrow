//! #306: a `Do` inside a text object, which the specification does not allow and every reader
//! runs. MuPDF shares the caller's text matrix with a form drawn there, so what the form does to
//! the pen moves it for the text after the `Do`; PDFium, Ghostscript, Poppler and pdf.js keep the
//! caller's pen, and so does burrow's walk, whose form starts with no text object open (#300).
//! Measured by #306's round 0, region x 100..300, y 250..300: SECRET that the four place at
//! (110, 100) lands in the region in MuPDF, 170 to 394 dark pixels, after an `Ok` -- whether the
//! form sets `Tm`, opens a `BT` alone under a `cm` (resetting the shared pen), shows a string whose
//! advance carries back, or does any of these one form deeper, or in a text object inside a form.
//!
//! The rule (DECISIONS rules 1 and 4): a `Do` while a text object is open refuses
//! `[do-in-text-object]`, in every stream the walk reads, before its name is resolved. The leak is
//! MuPDF's, which these tests cannot render, so each shape asserts PDFium's half -- SECRET drawn
//! outside the region, where burrow placed it -- and then the refusal. Each twin draws the same
//! form after the `ET`, with SECRET in the region, and redacts clean. An image `Do` inside a text
//! object moves no reader's pen and refuses too: the wider rule's recorded over-refusal. 0 of the
//! 99 real documents and 0 of the generated fixtures draw anything inside a text object.

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

fn stream(content: &[u8], dictionary: &str) -> Vec<u8> {
    [
        format!("<< {dictionary} /Length {} >>\nstream\n", content.len()).as_bytes(),
        content,
        b"\nendstream",
    ]
    .concat()
}

/// A one-page document whose `/Contents` array is `elements`, with `/F2` Helvetica and each of
/// `forms` as a chain: the page's resources name `/Fm0`, `/Fm0`'s name `/Fm1`, and so on, so each
/// form is reached from one scope only. The page also names `/Im0`, a one-pixel black image.
fn document(elements: &[&str], forms: &[&str]) -> Vec<u8> {
    let first_form = 5 + elements.len();
    let resources = |from: usize| {
        let xobjects: String = (from..forms.len().min(from + 1))
            .map(|i| format!("/Fm{i} {} 0 R ", first_form + i))
            .collect();
        format!("<< /Font << /F2 4 0 R >> /XObject << {xobjects}>> >>")
    };
    let image = first_form + forms.len();
    let page_resources =
        resources(0).replacen("/XObject << ", &format!("/XObject << /Im0 {image} 0 R "), 1);
    let contents: Vec<String> = (0..elements.len())
        .map(|i| format!("{} 0 R", 5 + i))
        .collect();
    let page = format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] /Resources {page_resources} \
         /Contents [{}] >>",
        contents.join(" ")
    );
    let mut all: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        page.into_bytes(),
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_vec(),
    ];
    for element in elements {
        all.push(stream(element.as_bytes(), ""));
    }
    for (i, form) in forms.iter().enumerate() {
        all.push(stream(
            form.as_bytes(),
            &format!(
                "/Type /XObject /Subtype /Form /BBox [0 0 400 400] /Resources {}",
                resources(i + 1)
            ),
        ));
    }
    all.push(stream(
        b"\x00",
        "/Type /XObject /Subtype /Image /Width 1 /Height 1 /ColorSpace /DeviceGray \
         /BitsPerComponent 8",
    ));
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

/// A document whose page content is the one stream `content`.
fn page(content: &str, forms: &[&str]) -> Vec<u8> {
    document(&[content], forms)
}

fn redact(pdf: &[u8]) -> burrow_types::Result<(Vec<u8>, burrow_engines::redact::Report)> {
    let covered: BTreeSet<usize> = [0].into_iter().collect();
    support::redact_page(pdf, 0, covered, REGION)
}

/// PDFium draws SECRET's `S` at (110, 100), below the region, as burrow's walk places it. MuPDF
/// draws it in the region (round 0), which no reader here can show.
#[track_caller]
fn pdfium_keeps_the_callers_pen(what: &str, pdf: &[u8]) {
    let (x, y) = chars_on_page(pdf, 0)
        .into_iter()
        .find(|c| !c.generated && c.unicode == u32::from('S'))
        .map(|c| c.origin)
        .unwrap_or_else(|| panic!("{what}: PDFium draws no SECRET"));
    assert!(
        (x - 110.0).abs() < 1.0 && (y - 100.0).abs() < 1.0,
        "{what}: PDFium draws SECRET at ({x}, {y}), not (110, 100)"
    );
}

#[track_caller]
fn refuses(what: &str, pdf: &[u8]) {
    match redact(pdf) {
        Err(error) => assert!(
            error.to_string().contains("[do-in-text-object]"),
            "{what}: refused, but not by this rule: {error}"
        ),
        Ok(_) => panic!("{what}: redacted, where [do-in-text-object] must refuse"),
    }
}

/// The twin has ink in the region, redacts, and leaves none there.
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

/// SECRET in the region, for the twins: text the redaction must find and remove.
const IN_REGION: &str = "BT /F2 20 Tf 110 260 Td (SECRET) Tj ET";

/// THE FORM SHAPES round 0 measured leaking in MuPDF: a `Tm` (the issue's), a `BT` alone under a
/// `cm` -- MuPDF's form resets the shared pen to the origin, which the `cm` puts at (110, 260) --
/// and a string whose advance carries back to the caller (the form selects its own font, as a form
/// showing text must here; MuPDF draws SECRET in the region either way, 357 dark pixels, measured).
/// The narrower rule "the form sets `Tm`"
/// would accept the second and third. Each twin draws the same form after the `ET`.
#[test]
fn a_form_drawn_inside_a_text_object_refuses() {
    for (what, caller, form) in [
        (
            "Tm",
            "BT /F2 20 Tf 110 100 Td /Fm0 Do (SECRET) Tj ET",
            "BT 1 0 0 1 110 260 Tm ET",
        ),
        (
            "BT ET under cm",
            "1 0 0 1 110 260 cm BT /F2 20 Tf 0 -160 Td /Fm0 Do (SECRET) Tj ET",
            "BT ET",
        ),
        (
            "an advance",
            "BT /F2 20 Tf 110 100 Td /Fm0 Do (SECRET) Tj ET",
            "BT /F2 20 Tf 0 260 Td (WWWWW) Tj ET",
        ),
    ] {
        let pdf = page(caller, &[form]);
        pdfium_keeps_the_callers_pen(what, &pdf);
        refuses(what, &pdf);
        redacts_clean(what, &page(&format!("/Fm0 Do {IN_REGION}"), &[form]));
    }
}

/// ONE FORM DEEPER, both ways: a page `Do` inside a text object of a form whose own `Do` (with no
/// `BT`) draws the `Tm`; and a page drawing, outside any text object, a form whose text object
/// draws it. The second is the check holding in a form's stream, not only the page's.
#[test]
fn a_do_inside_a_text_object_refuses_at_any_depth() {
    let tm = "BT 1 0 0 1 110 260 Tm ET";
    let nested = page(
        "BT /F2 20 Tf 110 100 Td /Fm0 Do (SECRET) Tj ET",
        &["/Fm1 Do", tm],
    );
    pdfium_keeps_the_callers_pen("nested", &nested);
    refuses("nested", &nested);
    redacts_clean(
        "nested, after ET",
        &page(&format!("/Fm0 Do {IN_REGION}"), &["/Fm1 Do", tm]),
    );

    let in_a_form = page(
        "/Fm0 Do",
        &["BT /F2 20 Tf 110 100 Td /Fm1 Do (SECRET) Tj ET", tm],
    );
    pdfium_keeps_the_callers_pen("in a form", &in_a_form);
    refuses("in a form", &in_a_form);
    redacts_clean(
        "in a form, after ET",
        &page("/Fm0 Do", &[&format!("/Fm1 Do {IN_REGION}"), tm]),
    );
}

/// THE TEXT OBJECT OPENED IN ONE `/Contents` ELEMENT and the `Do` in the next: the elements are
/// one stream (§7.8.2), so the text object is still open at the `Do`.
#[test]
fn a_text_object_open_across_a_contents_boundary_still_refuses() {
    let form = "BT 1 0 0 1 110 260 Tm ET";
    let split = document(
        &["BT /F2 20 Tf 110 100 Td\n", "/Fm0 Do (SECRET) Tj ET\n"],
        &[form],
    );
    pdfium_keeps_the_callers_pen("split", &split);
    refuses("split", &split);
    redacts_clean(
        "split, after ET",
        &document(&["/Fm0 Do\n", &format!("{IN_REGION}\n")], &[form]),
    );
}

/// THE WIDER RULE'S OVER-REFUSAL, recorded: an image `Do` inside a text object moves no reader's
/// pen (round 0), and refuses; the check comes before the name is resolved. So does a form drawn
/// after the text object's last show, which moves nothing that follows. And a form with no `BT` of
/// its own, whose `Td` and `Tm` MuPDF applies to the shared pen, names this rule rather than
/// `[text-outside-text-object]`, because the `Do` refuses before the form is walked.
#[test]
fn an_image_a_last_draw_or_a_form_without_bt_refuses_by_this_rule() {
    refuses(
        "image",
        &page("BT /F2 20 Tf 110 100 Td /Im0 Do (SECRET) Tj ET", &[]),
    );
    refuses(
        "after the last show",
        &page(
            "BT /F2 20 Tf 110 100 Td (SECRET) Tj /Fm0 Do ET",
            &["BT 1 0 0 1 110 260 Tm ET"],
        ),
    );
    for form in ["0 160 Td", "1 0 0 1 110 260 Tm"] {
        refuses(
            form,
            &page("BT /F2 20 Tf 110 100 Td /Fm0 Do (SECRET) Tj ET", &[form]),
        );
    }
    redacts_clean(
        "image, after ET",
        &page(&format!("/Im0 Do {IN_REGION}"), &[]),
    );
}
