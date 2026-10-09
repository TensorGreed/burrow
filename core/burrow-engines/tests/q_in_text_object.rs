//! #300: PDFium saves the text position -- the text and line matrices -- with the graphics state,
//! so a `Q` inside a text object moves the pen back to where the matching `q` left it. burrow's walk
//! kept the position outside the graphics state, so it placed the glyphs after such a `Q` somewhere
//! PDFium does not draw them: `Ok` with SECRET drawn in the region, measured.
//!
//! The rule (DECISIONS rules 1 and 4): a `q` or `Q` while a text object is open refuses
//! `[graphics-state-in-text]`. 0 of 99 real documents carry one (#300's round 0, every stream).
//! Each leak shape here is first shown to be a leak -- PDFium draws SECRET inside the region -- and
//! then shown to refuse; each test's twin keeps the `q`/`Q` but moves them outside the text object,
//! and redacts clean. Where a comment states where PDFium draws SECRET, the test asserts it. The
//! list is round 0's: the `Q` half alone, the `q` half alone, the canonical shape and its variants,
//! a text object boundary, a `/Contents` array, a form, marked content, and the guard that a form's
//! own `q`/`Q` is not judged against its caller's open text object.

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

const HELVETICA: &[u8] = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>";

fn stream(content: &[u8], dictionary: &str) -> Vec<u8> {
    [
        format!("<< {dictionary} /Length {} >>\nstream\n", content.len()).as_bytes(),
        content,
        b"\nendstream",
    ]
    .concat()
}

/// A one-page document whose `/Contents` are `contents` (one stream each, objects 5 on), with
/// `/F2` Helvetica as object 4, and `form`, when given, as `/Fm0` after them.
fn document(contents: &[&[u8]], form: Option<&[u8]>) -> Vec<u8> {
    let first = 5;
    let references: Vec<String> = (0..contents.len())
        .map(|i| format!("{} 0 R", first + i))
        .collect();
    let form_number = first + contents.len();
    let xobjects = if form.is_some() {
        format!("/XObject << /Fm0 {form_number} 0 R >>")
    } else {
        String::new()
    };
    let page = format!(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] /Resources << /Font << /F2 4 0 R >> \
         {xobjects} >> /Contents [{}] >>",
        references.join(" ")
    );
    let mut all: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        page.into_bytes(),
        HELVETICA.to_vec(),
    ];
    for content in contents {
        all.push(stream(content, ""));
    }
    if let Some(form) = form {
        all.push(stream(
            form,
            "/Type /XObject /Subtype /Form /BBox [0 0 400 400] /Resources << /Font << /F2 4 0 R >> >>",
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

fn page(content: &str) -> Vec<u8> {
    document(&[content.as_bytes()], None)
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
            error.to_string().contains("[graphics-state-in-text]"),
            "{what}: refused, but not by this rule: {error}"
        ),
        Ok(_) => panic!("{what}: redacted, where [graphics-state-in-text] must refuse"),
    }
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

/// The twin redacts, and leaves no ink in the region.
#[track_caller]
fn redacts_clean(what: &str, pdf: &[u8]) {
    let (out, _) = redact(pdf).unwrap_or_else(|error| panic!("{what}: {error}"));
    assert_eq!(dark_in_region(&out), 0, "{what}: ink left in the region");
}

const FIFTEEN_W: &str = "(WWWWWWWWWWWWWWW) Tj";

/// THE CANONICAL SHAPE: fifteen W's inside `q`/`Q` put SECRET past the region to burrow; PDFium
/// restores the pen and draws it at x = 110. And the same with each position operator inside the
/// `q` instead -- `Tm`, `Td`, `T*`, `'`, `"`, a `TJ` adjustment, a nested `q`/`Q` -- all measured to
/// be undone by the `Q`. The rule refuses at the `q`, before any variant's operator is read, so the
/// variants are evidence about PDFium -- each draws SECRET in the region -- not eight tests of the
/// walk. The twin wraps the whole text object in the `q`/`Q`, and both readers draw SECRET at
/// x = 393, past the region.
#[test]
fn a_q_inside_a_text_object_refuses_in_every_shape_that_moves_the_pen() {
    for inside in [
        FIFTEEN_W,
        "1 0 0 1 300 60 Tm",
        "300 -200 Td",
        "T*",
        "(WWW) '",
        "0 0 (WWW) \"",
        "[(WW) -9000 (W)] TJ",
        "q (WWWWWWWW) Tj Q (WWWWWWW) Tj",
    ] {
        refuses_a_leak(
            inside,
            &page(&format!(
                "BT /F2 20 Tf 200 TL 110 260 Td q {inside} Q (SECRET) Tj ET"
            )),
        );
    }
    let twin = page(&format!(
        "q BT /F2 20 Tf 110 260 Td {FIFTEEN_W} (SECRET) Tj ET Q"
    ));
    secret_drawn_at("the twin", &twin, 393.2, 260.0);
    redacts_clean("the twin, q BT..ET Q", &twin);
}

/// THE `Q` HALF ALONE, which is the half that matters: a `q` outside any text object and its `Q`
/// inside the next one. PDFium restores the pen where the PREVIOUS text object left it (#300's round
/// 0 measured (170, 400) for its shape), so SECRET is drawn back on the first line. The twin closes
/// the `Q` after the `ET`, which a `BT` then resets.
#[test]
fn a_q_restored_inside_a_text_object_refuses() {
    refuses_a_leak(
        "q outside, Q inside",
        &page(&format!(
            "BT /F2 20 Tf 110 260 Td (WW) Tj ET q BT /F2 20 Tf 110 60 Td {FIFTEEN_W} Q (SECRET) Tj ET"
        )),
    );
    redacts_clean(
        "Q after the ET",
        &page(&format!(
            "BT /F2 20 Tf 110 260 Td (WW) Tj ET q BT /F2 20 Tf 110 60 Td {FIFTEEN_W} (SECRET) Tj ET Q"
        )),
    );
}

/// THE `q` HALF ALONE: a `q` inside a text object whose `Q` comes after the `ET`. PDFium shows no
/// effect -- the next `BT` resets -- so this is the rule's over-refusal, recorded (rule 4), at a
/// census cost of 0. The twin moves the `q` before the `BT`.
#[test]
fn a_q_saved_inside_a_text_object_refuses_though_its_q_is_outside() {
    let shape =
        page("BT /F2 20 Tf 110 260 Td q (WW) Tj ET Q BT /F2 20 Tf 110 60 Td (SECRET) Tj ET");
    // MEASURED, not stated: PDFium draws SECRET where burrow does, outside the region.
    secret_drawn_at("q inside, Q outside", &shape, 110.0, 60.0);
    refuses("q inside, Q outside", &shape);
    redacts_clean(
        "q before the BT",
        &page("q BT /F2 20 Tf 110 260 Td (WW) Tj ET Q BT /F2 20 Tf 110 60 Td (SECRET) Tj ET"),
    );
}

/// ACROSS A TEXT OBJECT BOUNDARY: the `q` in one text object, its `Q` in the next. PDFium restores
/// the first object's pen inside the second, measured. The twin wraps both text objects in the
/// `q`/`Q`.
#[test]
fn a_saved_position_restored_in_a_later_text_object_refuses() {
    refuses_a_leak(
        "q in the first BT, Q in the next",
        &page(
            "BT /F2 20 Tf 110 260 Td q (WW) Tj ET BT /F2 20 Tf 110 60 Td (WWWWWWWW) Tj Q (SECRET) \
             Tj ET",
        ),
    );
    redacts_clean(
        "q BT..ET BT..ET Q",
        &page(
            "q BT /F2 20 Tf 110 260 Td (WW) Tj ET BT /F2 20 Tf 110 60 Td (WWWWWWWW) Tj (SECRET) \
             Tj ET Q",
        ),
    );
}

/// A `/Contents` ARRAY: the text object opens in one stream and the `q`/`Q` are in the next. The
/// walk reads the page's streams joined, as PDFium does, so the text object is still open. The twin
/// splits a `q` ... `Q` around a whole text object across the same two streams.
#[test]
fn a_q_in_the_next_content_stream_of_an_open_text_object_refuses() {
    let second = format!("q {FIFTEEN_W} Q (SECRET) Tj ET");
    refuses_a_leak(
        "BT in stream 1, q/Q in stream 2",
        &document(&[b"BT /F2 20 Tf 110 260 Td", second.as_bytes()], None),
    );
    redacts_clean(
        "q in stream 1, BT..ET Q in stream 2",
        &document(&[b"q", b"BT /F2 20 Tf 110 260 Td (SECRET) Tj ET Q"], None),
    );
}

/// A FORM XOBJECT whose own content carries the canonical shape. The twin's form wraps its whole
/// text object in `q`/`Q`.
#[test]
fn a_q_inside_a_text_object_in_a_form_refuses() {
    let leak = format!("BT /F2 20 Tf 110 260 Td q {FIFTEEN_W} Q (SECRET) Tj ET");
    refuses_a_leak(
        "the canonical shape in a form",
        &document(&[b"/Fm0 Do"], Some(leak.as_bytes())),
    );
    redacts_clean(
        "q BT..ET Q in a form",
        &document(
            &[b"/Fm0 Do"],
            Some(b"q BT /F2 20 Tf 110 260 Td (SECRET) Tj ET Q"),
        ),
    );
}

/// THE GUARD AGAINST OVER-REACH: a form drawn INSIDE the page's text object, whose own content is a
/// `q`/`Q` around a text object of its own that moves its pen 200 points down and fifteen W's along.
/// PDFium keeps a form's text position apart from its caller's -- asserted here: the caller's SECRET
/// stays at (110, 260) -- so the form's walk starts with no text object open and this redacts. A
/// walk that carried the caller's open text object into the form would refuse it.
#[test]
fn a_forms_own_q_is_not_judged_against_its_callers_text_object() {
    let form = format!("q BT /F2 20 Tf 0 -200 Td {FIFTEEN_W} ET Q");
    let pdf = document(
        &[b"BT /F2 20 Tf 110 260 Td /Fm0 Do (SECRET) Tj ET"],
        Some(form.as_bytes()),
    );
    secret_drawn_at("a form that moves its own pen", &pdf, 110.0, 260.0);
    redacts_clean("BT .. /Fm0 Do (SECRET) Tj ET, Fm0 = q BT..ET Q", &pdf);
}

/// MARKED CONTENT around the `q` changes nothing, measured. The twin moves the `q`/`Q` outside the
/// text object, keeping the marked content inside it.
#[test]
fn a_q_inside_marked_content_inside_a_text_object_refuses() {
    refuses_a_leak(
        "BT /P BMC q .. EMC Q",
        &page(&format!(
            "BT /F2 20 Tf 110 260 Td /P BMC q {FIFTEEN_W} EMC Q (SECRET) Tj ET"
        )),
    );
    redacts_clean(
        "q BT /P BMC .. EMC ET Q",
        &page(&format!(
            "q BT /F2 20 Tf 110 260 Td /P BMC {FIFTEEN_W} EMC (SECRET) Tj ET Q"
        )),
    );
}
