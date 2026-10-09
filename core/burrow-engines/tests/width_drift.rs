//! #291: fractional glyph widths, which PDFium stores as integers -- TRUNCATED for a simple or CID
//! font, ROUNDED for a Type 3 (measured: 0.9 -> 0, 599.9 -> 599; Type 3 0.4 -> 0, 0.5 -> 1) -- so
//! over enough padding the secret drifts: burrow places it outside the region, PDFium draws it in.
//!
//! The fix is a BOUND, not a replica (the owner's decision): each glyph whose width is not whole in
//! thousandths of an em adds under one thousandth of the font size to a drift that resets at every
//! absolute positioning operator, and a glyph's box is widened by it. Each leak shape here inks the
//! region on the input and must leave none of SECRET's letters on the page; the twins show the
//! bound is scoped -- whole widths add nothing, and `Td`, `'` and `Tm` reset it.
//!
//! And two refusals the measurement found: a Type 3 width that rounds to 0 (PDFium then advances
//! by the procedure's `d0` width), and a simple width that a 32-bit float rounds to 65,535.

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

/// A one-page document drawing `content`, with `/F1` object 5 and `/F2` (Helvetica) object 6, and
/// `objects` from object 7.
fn document(content: &[u8], f1: &[u8], objects: &[&[u8]]) -> Vec<u8> {
    let stream = [
        format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(),
        content,
        b"\nendstream",
    ]
    .concat();
    let mut all: Vec<&[u8]> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] /Resources << /Font << /F1 5 0 R \
          /F2 6 0 R >> >> /Contents 4 0 R >>",
        &stream,
        f1,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ];
    all.extend_from_slice(objects);
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

/// `pads` copies of `pad` in `/F1` at size 10 and `Tz` 10000 (each fractional unit of width moves
/// the pen a point), then SECRET in `/F2` at size 20 -- after `between`, which a twin uses to reset
/// the position. Starts at x = 110, where PDFium draws SECRET when the pads advance by nothing.
fn padded(pad: &[u8], pads: usize, between: &str) -> Vec<u8> {
    let mut content = b"BT /F1 10 Tf 10000 Tz 110 260 Td <".to_vec();
    for _ in 0..pads {
        content.extend_from_slice(pad);
    }
    content.extend_from_slice(format!("> Tj 100 Tz {between} /F2 20 Tf (SECRET) Tj ET").as_bytes());
    content
}

fn redact(pdf: &[u8]) -> burrow_types::Result<(Vec<u8>, burrow_engines::redact::Report)> {
    let covered: BTreeSet<usize> = [0].into_iter().collect();
    support::redact_page(pdf, 0, covered, REGION)
}

fn dark_in_region(pdf: &[u8]) -> u64 {
    dark_pixels_with_annotations(pdf, 0, PIXELS).0
}

fn secret_letters(pdf: &[u8]) -> usize {
    chars_on_page(pdf, 0)
        .into_iter()
        .filter(|c| !c.generated && "SECRT".contains(char::from_u32(c.unicode).unwrap_or(' ')))
        .count()
}

/// The shape inks the region, redacts, and no SECRET letter survives anywhere on the page.
#[track_caller]
fn redacts_clean(what: &str, pdf: &[u8]) {
    assert!(
        dark_in_region(pdf) > 0,
        "{what}: the input must ink the region"
    );
    let (out, _) = redact(pdf).unwrap_or_else(|error| panic!("{what}: {error}"));
    assert_eq!(dark_in_region(&out), 0, "{what}: ink left in the region");
    assert_eq!(
        secret_letters(&out),
        0,
        "{what}: SECRET's letters survive, moved"
    );
}

#[track_caller]
fn refuses(what: &str, pdf: &[u8], rule: &str) {
    match redact(pdf) {
        Err(error) => assert!(
            error.to_string().contains(&format!("[{rule}]")),
            "{what}: {error}"
        ),
        Ok(_) => panic!("{what}: redacted, where [{rule}] must refuse"),
    }
}

/// A Helvetica `/F1` with `/Widths` from code 65: `A` 600 and `B` `b`.
fn simple(b: &str) -> Vec<u8> {
    format!(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 65 /LastChar 66 \
         /Widths [600 {b}] >>"
    )
    .into_bytes()
}

/// SIMPLE FONT: 250 pads of width 0.9 -- 0 to PDFium, 0.9 to burrow -- put SECRET 225 points right
/// of where PDFium draws it, outside the region: `Ok` with SECRET kept, on `main`. The drift bound
/// widens it back. THE TWINS: whole-width pads drift nothing (SECRET is where both draw it), and a
/// `Td` after the pads resets the drift, so a SECRET moved well clear of the region is kept.
#[test]
fn fractional_simple_widths_drift_the_secret_and_the_bound_catches_it() {
    redacts_clean(
        "250 pads of 0.9",
        &document(&padded(b"42", 250, ""), &simple("0.9"), &[]),
    );
    // RESET BY `Td`: the pads end, the position moves 300 points right by `Td`, and SECRET is drawn
    // there, far outside the region to both readers. Nothing reaches the region, and SECRET stays.
    let reset = document(&padded(b"42", 250, "300 0 Td"), &simple("0.9"), &[]);
    let (out, _) = redact(&reset).expect("the reset twin redacts");
    assert_eq!(
        secret_letters(&out),
        6,
        "after a reset the far SECRET is not removed"
    );
}

/// CID FONT: `/W [66 [0.9]]`, which PDFium truncates too.
#[test]
fn fractional_cid_widths_drift_the_secret_and_the_bound_catches_it() {
    let t0: &[u8] = b"<< /Type /Font /Subtype /Type0 /BaseFont /Helvetica /Encoding /Identity-H \
                      /DescendantFonts [7 0 R] >>";
    let cid: &[u8] = b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Helvetica /CIDSystemInfo \
                       << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor \
                       8 0 R /DW 1000 /W [66 [0.9]] /CIDToGIDMap /Identity >>";
    let descriptor: &[u8] = b"<< /Type /FontDescriptor /FontName /Helvetica /Flags 32 \
                              /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 800 /Descent -200 \
                              /CapHeight 700 /StemV 80 >>";
    redacts_clean(
        "250 CID pads of 0.9",
        &document(&padded(b"0042", 250, ""), t0, &[cid, descriptor]),
    );
}

/// A Type 3 `/F1` with `B` of width `b` (font matrix 0.001) whose procedure is `d0 d`.
fn type_three(b: &str, d: &str) -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) {
    let font = format!(
        "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] /FontMatrix [0.001 0 0 0.001 0 0] \
         /CharProcs 7 0 R /Encoding << /Type /Encoding /Differences [65 /A /B] >> /FirstChar 65 \
         /LastChar 66 /Widths [600 {b}] /Resources << >> >>"
    )
    .into_bytes();
    let procs = b"<< /A 8 0 R /B 9 0 R >>".to_vec();
    let a = b"<< /Length 8 >>\nstream\n600 0 d0\nendstream".to_vec();
    let body = format!("{d} 0 d0");
    let b_proc = format!("<< /Length {} >>\nstream\n{body}\nendstream", body.len()).into_bytes();
    (font, procs, a, b_proc)
}

/// TYPE 3: PDFium ROUNDS -- 1.6 to 2 -- so it draws the pen AHEAD of burrow's, and the bound's
/// RIGHT-hand widening is the one that matters. 250 pads from x = -420 put burrow's SECRET at about
/// -20..60, left of the region, and PDFium's at about 80..160, inside it. A width under a thousandth
/// refuses `[type-three-width-zero]` -- PDFium may round it to 0 and then advance by the procedure's
/// `d0` width, measured 1,000 for `/Widths [0]` over `1000 0 d0`.
#[test]
fn type_three_widths_round_and_a_width_under_a_thousandth_refuses() {
    let (font, procs, a, b) = type_three("1.6", "0");
    let mut content = b"BT /F1 10 Tf 10000 Tz -420 260 Td <".to_vec();
    content.extend_from_slice(&b"42".repeat(250));
    content.extend_from_slice(b"> Tj 100 Tz /F2 20 Tf (SECRET) Tj ET");
    redacts_clean(
        "Type 3 pads of 1.6, SECRET drawn ahead of burrow's pen",
        &document(&content, &font, &[&procs, &a, &b]),
    );
    for (w, d) in [
        ("0", "1000"),
        ("0.4", "0"),
        ("0.4", "1000"),
        ("0.9", "1000"),
    ] {
        let (font, procs, a, b) = type_three(w, d);
        refuses(
            &format!("/Widths [.. {w}] over {d} 0 d0"),
            &document(&padded(b"42", 10, ""), &font, &[&procs, &a, &b]),
            "type-three-width-zero",
        );
    }
}

/// THE FLOAT32 EDGE OF THE ZERO REFUSAL: PDFium computes the stored width as
/// `roundf(f32(f32(w) x f32(a)) x 1000)`, so `w = 5` at `a = 0.0001` -- 0.5 in f64 -- is 0.49999997
/// and rounds to 0, and PDFium advances by the procedure's `100000 0 d0` (measured, `Ok` over the
/// secret when the edge was 0.5). The near-miss, `w = 10`, is one thousandth and is a width.
#[test]
fn a_type_three_width_at_the_float32_rounding_edge_refuses() {
    let font = |w: &str| {
        format!(
            "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1 1] /FontMatrix [0.0001 0 0 0.0001 0 0] \
             /CharProcs 7 0 R /Encoding << /Type /Encoding /Differences [65 /A] >> /FirstChar 65 \
             /LastChar 65 /Widths [{w}] /Resources << >> >>"
        )
    };
    let procs: &[u8] = b"<< /A 8 0 R >>";
    let procedure: &[u8] = b"<< /Length 11 >>\nstream\n100000 0 d0\nendstream";
    let content = b"BT /F1 10 Tf 10 260 Td (A) Tj /F2 20 Tf (SECRET) Tj ET";
    let at = |a: &str, w: &str| font(w).replace("0.0001 0 0 0.0001", &format!("{a} 0 0 {a}"));
    refuses(
        "w = 0.7142857142857143 at a = 0.0007 (0.5 in f64, 0.49999997 in f32)",
        &document(
            content,
            at("0.0007", "0.7142857142857143").as_bytes(),
            &[procs, procedure],
        ),
        "type-three-width-zero",
    );
    refuses(
        "w = 5 at a = 0.0001",
        &document(content, font("5").as_bytes(), &[procs, procedure]),
        "type-three-width-zero",
    );
    redact(&document(
        content,
        font("10").as_bytes(),
        &[procs, procedure],
    ))
    .expect("w = 10 at a = 0.0001 is one thousandth, a width");
}

/// THE FLOAT32 EDGE: 65,534.999 is 65,535 as a 32-bit float -- PDFium's "not set" -- and PDFium
/// draws it at 0 (measured). Refused `[width-out-of-range]`; 65,534.4 stays a width.
#[test]
fn a_width_a_float_rounds_to_65535_refuses() {
    let content = b"BT /F1 10 Tf 110 260 Td (AB) Tj ET";
    refuses(
        "65534.999",
        &document(content, &simple("65534.999"), &[]),
        "width-out-of-range",
    );
    redact(&document(content, &simple("65534.4"), &[])).expect("65534.4 is a width");
}

/// A TYPE 3 WIDTH OF 2^20 THOUSANDTHS OR MORE refuses: past about 2^21, f32's steps exceed a
/// thousandth, so a width burrow calls whole is stored a step away and adds no drift. Measured:
/// `/Widths [1073741.856 -1073741.824]` under a matrix of 1 is stored as 2^30 and -2^30, so each pair
/// moves PDFium's pen by 0 and burrow's by 0.032 em, and 600 pairs returned `Ok` over the secret.
/// The width alone cannot reach the edge under the usual matrix -- the strict reader refuses a
/// number that large first, `[number-unreadable]` -- so these use a matrix of 1. The near-miss,
/// 1,048.575 em, is a width.
#[test]
fn a_type_three_width_past_float32_precision_refuses() {
    let font = |widths: &str| {
        format!(
            "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1 1] /FontMatrix [1 0 0 1 0 0] \
             /CharProcs 7 0 R /Encoding << /Type /Encoding /Differences [65 /A /B] >> /FirstChar 65 \
             /LastChar 66 /Widths [{widths}] /Resources << >> >>"
        )
    };
    let procs: &[u8] = b"<< /A 8 0 R /B 8 0 R >>";
    let procedure: &[u8] = b"<< /Length 7 >>\nstream\n0 0 d0\nendstream";
    let content = format!(
        "BT /F1 12 Tf 110 260 Td ({}) Tj /F2 20 Tf (SECRET) Tj ET",
        "AB".repeat(600)
    );
    refuses(
        "the 2^30 pair",
        &document(
            content.as_bytes(),
            font("1073741.856 -1073741.824").as_bytes(),
            &[procs, procedure],
        ),
        "width-out-of-range",
    );
    refuses(
        "3e6 at a matrix of 1",
        &document(
            b"BT /F1 1 Tf 110 260 Td (A) Tj ET",
            font("3000000 1").as_bytes(),
            &[procs, procedure],
        ),
        "width-out-of-range",
    );
    redact(&document(
        b"BT /F1 1 Tf 110 260 Td (A) Tj ET",
        font("1048.575 1").as_bytes(),
        &[procs, procedure],
    ))
    .expect("1,048.575 em is under 2^20 thousandths, a width");
}

/// ROTATED: under `Tm [0 1 -1 0 ..]` the advance runs up the page, so the drift must widen the box
/// along y, not x. PDFium draws SECRET from y = 255, in the region; burrow, 225 points higher.
#[test]
fn the_drift_follows_a_rotated_text_matrix() {
    let mut content = b"BT /F1 10 Tf 10000 Tz 0 1 -1 0 220 255 Tm <".to_vec();
    content.extend_from_slice(&b"42".repeat(250));
    content.extend_from_slice(b"> Tj 100 Tz /F2 20 Tf (SECRET) Tj ET");
    redacts_clean(
        "rotated, 250 pads of 0.9",
        &document(&content, &simple("0.9"), &[]),
    );
}

/// `'` MOVES TO THE NEXT LINE, so it resets the drift like `Td`. The line starts at x = 320, right
/// of the region, with `TL` 0, so `'` returns to that start: both readers draw SECRET there, clear of
/// the region. Without the reset the 225 points of drift from the pads would widen SECRET's box back
/// over the region and remove it, so this is the reset's witness and not only a twin.
#[test]
fn a_quote_operator_resets_the_drift() {
    let mut content = b"BT /F1 10 Tf 10000 Tz 0 TL 320 260 Td <".to_vec();
    content.extend_from_slice(&b"42".repeat(250));
    content.extend_from_slice(b"> Tj 100 Tz /F2 20 Tf (SECRET) ' ET");
    let pdf = document(&content, &simple("0.9"), &[]);
    let (out, _) = redact(&pdf).expect("the quote twin redacts");
    assert_eq!(
        secret_letters(&out),
        6,
        "after `'` the SECRET right of the region is not removed"
    );
}

/// `Tm` RESETS THE DRIFT, the same witness as the `'` one: both readers draw SECRET at x = 320, right
/// of the region, and only an unreset drift would widen its box back over it.
#[test]
fn a_text_matrix_resets_the_drift() {
    let mut content = b"BT /F1 10 Tf 10000 Tz 320 260 Td <".to_vec();
    content.extend_from_slice(&b"42".repeat(250));
    content.extend_from_slice(b"> Tj 100 Tz /F2 20 Tf 1 0 0 1 320 260 Tm (SECRET) Tj ET");
    let pdf = document(&content, &simple("0.9"), &[]);
    let (out, _) = redact(&pdf).expect("the Tm twin redacts");
    assert_eq!(
        secret_letters(&out),
        6,
        "after `Tm` the SECRET right of the region is not removed"
    );
}

/// THE DRIFT IS A SUM OF MAGNITUDES. Truncation errs in glyph space, and `Tz` carries the sign: 250
/// pads of 0.9 at `Tz` 10000 put burrow 225 points ahead of PDFium, and 250 of 0.1 at `Tz` -10000
/// bring it back only 25. Burrow draws SECRET at 310, right of the region; PDFium at 110, inside.
/// A drift that added the signed increments would be 0 here.
#[test]
fn the_drift_does_not_cancel_across_a_negative_horizontal_scale() {
    let font: &[u8] =
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 65 /LastChar 66 \
                        /Widths [0.9 0.1] >>";
    let mut content = b"BT /F1 10 Tf 10000 Tz 110 260 Td <".to_vec();
    content.extend_from_slice(&b"41".repeat(250));
    content.extend_from_slice(b"> Tj -10000 Tz <");
    content.extend_from_slice(&b"42".repeat(250));
    content.extend_from_slice(b"> Tj 100 Tz /F2 20 Tf (SECRET) Tj ET");
    redacts_clean(
        "+Tz pads of 0.9, then -Tz pads of 0.1",
        &document(&content, font, &[]),
    );
}

/// WHOLE WIDTHS ADD NOTHING, including one whose f64 product is not exactly whole: 9 x 0.001 x 1000
/// is 9.000000000000002. 22 pads of 9 at `Tz` 10000 put SECRET at x = 308 for both readers, just
/// right of the region; a drift of 22 points, added for widths that are whole, would widen its box
/// back over the region and remove it.
#[test]
fn whole_widths_add_no_drift() {
    let pdf = document(&padded(b"42", 22, ""), &simple("9"), &[]);
    let (out, _) = redact(&pdf).expect("whole pads redact");
    assert_eq!(
        secret_letters(&out),
        6,
        "SECRET, right of the region, is not removed"
    );
}

/// THE SIMPLE-WIDTH EDGE, from below: 65,534.998 is still a width -- measured, PDFium draws it.
#[test]
fn a_width_just_under_the_float32_edge_is_a_width() {
    let content = b"BT /F1 10 Tf 110 260 Td (AB) Tj ET";
    redact(&document(content, &simple("65534.998"), &[])).expect("65534.998 is a width");
}
