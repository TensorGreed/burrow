//! #125: ink a Type 3 glyph procedure draws, which the walk never boxes.
//!
//! The walk boxes a Type 3 glyph by its advance and `/FontBBox` and does not look at what the
//! procedure draws. #125's specification review measured what that let through on `main`, each an
//! `Ok`:
//!
//! - filled paths from a glyph far from the region, drawn into it: 2,276 dark pixels left;
//! - the same under `d1`: 2,242;
//! - `sh` with no path operator: 14,960;
//! - an inline image drawn outside its glyph's box, under `d0` and as a `d1` mask: 5,309 and 5,382;
//! - a glyph drawn as an image and cut by the region: 0 left on the page, but the bitmap still in
//!   `/CharProcs`.
//!
//! The rules (ADR 0029 §3, owner's decisions 2026-10-08): a page drawing with a Type 3 font whose
//! procedures paint a path or a shading refuses `[type-three-procedure-paints]`, wherever the
//! region is; a procedure drawing an inline image outside its font's `/FontBBox` refuses
//! `[type-three-image-outside-its-box]`; and a region reaching a glyph of a font that draws an
//! image refuses `[type-three-image-cut]`.
//!
//! Each leaking shape here draws into the region, measured by PDFium on the input, and refuses by
//! name; each near-miss redacts.

#![cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::collections::BTreeSet;

use burrow_engines::pdfsyntax::region::Region;
use support::char_box_oracle::dark_pixels_with_annotations;

/// The region, in display space: x 100..300, and y 250..300 in PDF space on a 400-point page.
const REGION: Region = Region {
    left: 100.0,
    top: 100.0,
    width: 200.0,
    height: 50.0,
};

/// The same rectangle for the pixel count, at one pixel a point.
const PIXELS: (i32, i32, i32, i32) = (100, 100, 200, 50);

/// Where most glyphs sit: far from the region, so a glyph box of `[0 0 10 10]` never reaches it.
const FAR: (u32, u32) = (380, 10);

/// One-page document with a Type 3 font `/T3` whose only glyph, code 97, is `procedure`.
///
/// Objects: catalog 1, pages 2, page 3, content 4, font 5, `/CharProcs` 6, the procedure 7, then
/// `extra` from 8. `font_resources` is the Type 3 font's own `/Resources`.
fn page(
    content: &str,
    procedure: &[u8],
    bbox: &str,
    width: u32,
    font_resources: &str,
    page_resources: &str,
    extra: &[&str],
) -> Vec<u8> {
    let stream = |dict: &str, data: &[u8]| -> Vec<u8> {
        [
            format!("<< {dict} /Length {} >>\nstream\n", data.len()).as_bytes(),
            data,
            b"\nendstream",
        ]
        .concat()
    };
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] \
             /Resources << /Font << /T3 5 0 R >> {page_resources} >> /Contents 4 0 R >>"
        )
        .into_bytes(),
        stream("", content.as_bytes()),
        format!(
            "<< /Type /Font /Subtype /Type3 /FontBBox [{bbox}] /FontMatrix [1 0 0 1 0 0] \
             /CharProcs 6 0 R /Encoding << /Type /Encoding /Differences [97 /g] >> \
             /FirstChar 97 /LastChar 97 /Widths [{width}] /Resources {font_resources} >>"
        )
        .into_bytes(),
        b"<< /g 7 0 R >>".to_vec(),
        stream("", procedure),
    ];
    objects.extend(extra.iter().map(|e| e.as_bytes().to_vec()));

    let mut out = b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// Show glyph `a` of `/T3` at `(x, y)`, size 1.
fn show(at: (u32, u32)) -> String {
    format!("BT /T3 1 Tf {} {} Td (a) Tj ET", at.0, at.1)
}

/// A procedure body that draws the region's rectangle from a glyph at `FAR`, in glyph space.
const INTO_REGION: &str = "-280 245 200 30";

fn redact(pdf: &[u8]) -> burrow_types::Result<(Vec<u8>, burrow_engines::redact::Report)> {
    let covered: BTreeSet<usize> = [0].into_iter().collect();
    support::redact_page(pdf, 0, covered, REGION)
}

fn dark_in_region(pdf: &[u8]) -> u64 {
    dark_pixels_with_annotations(pdf, 0, PIXELS).0
}

#[track_caller]
fn refuses(what: &str, pdf: &[u8], rule: &str) {
    match redact(pdf) {
        Ok((out, _)) => panic!("{what}: Ok ({} bytes), expected [{rule}]", out.len()),
        Err(error) => {
            let message = error.to_string();
            assert!(message.contains(&format!("[{rule}]")), "{what}: {message}");
        }
    }
}

/// `pdf` inks the region, and refuses under `rule`.
#[track_caller]
fn reaches_and_refuses(what: &str, pdf: &[u8], rule: &str) {
    assert!(
        dark_in_region(pdf) > 0,
        "{what}: the procedure must ink the region, and PDFium drew nothing there"
    );
    refuses(what, pdf, rule);
}

#[test]
fn a_procedure_that_paints_refuses_the_page_wherever_its_ink_falls() {
    for (what, procedure) in [
        ("d0, a fill", format!("10 0 d0\n{INTO_REGION} re f\n")),
        (
            "d1, a fill",
            format!("10 0 0 0 10 10 d1\n{INTO_REGION} re f\n"),
        ),
        (
            "d0, a stroke",
            format!("10 0 d0\n20 w {INTO_REGION} re S\n"),
        ),
    ] {
        reaches_and_refuses(
            what,
            &page(
                &show(FAR),
                procedure.as_bytes(),
                "0 0 10 10",
                10,
                "<< >>",
                "",
                &[],
            ),
            "type-three-procedure-paints",
        );
    }
}

#[test]
fn a_procedure_that_paints_with_a_shading_and_no_path_operator_refuses() {
    let procedure = format!("10 0 d0\nq {INTO_REGION} re W n /Sh1 sh Q\n");
    reaches_and_refuses(
        "re W n /Sh1 sh",
        &page(
            &show(FAR),
            procedure.as_bytes(),
            "0 0 10 10",
            10,
            "<< /Shading << /Sh1 8 0 R >> >>",
            "",
            &[
                "<< /ShadingType 2 /ColorSpace /DeviceGray /Coords [0 0 1 0] \
               /Function << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [0] /N 1 >> \
               /Extend [true true] >>",
            ],
        ),
        "type-three-procedure-paints",
    );
}

/// PINNED OVER-REFUSAL (owner, 2026-10-08): the rule is page-wide. A glyph that fills only its own
/// box, far from the region, still refuses the page -- matplotlib's default PDF output draws every
/// glyph this way.
#[test]
fn a_procedure_that_paints_only_inside_its_own_box_far_from_the_region_still_refuses() {
    let pdf = page(
        &show(FAR),
        b"10 0 d0\n0 0 10 10 re f\n",
        "0 0 10 10",
        10,
        "<< >>",
        "",
        &[],
    );
    assert_eq!(dark_in_region(&pdf), 0, "nothing of it is in the region");
    refuses("paints inside its box", &pdf, "type-three-procedure-paints");
}

#[test]
fn a_procedure_that_only_clips_is_not_refused() {
    let pdf = page(
        &show(FAR),
        b"10 0 0 0 10 10 d1\nq 0 0 10 10 re W n Q\n",
        "0 0 10 10",
        10,
        "<< >>",
        "",
        &[],
    );
    redact(&pdf).expect("a clip paints nothing");
}

#[test]
fn an_inline_image_drawn_outside_its_glyphs_box_refuses() {
    for (what, procedure) in [
        (
            "d0, 8-bit gray",
            b"10 0 d0\nq 200 0 0 30 -280 245 cm BI /W 1 /H 1 /BPC 8 /CS /G ID \x00 EI Q\n".as_slice(),
        ),
        (
            "d1, an image mask",
            b"10 0 0 0 10 10 d1\nq 200 0 0 30 -280 245 cm BI /W 1 /H 1 /BPC 1 /IM true ID \x00 EI Q\n"
                .as_slice(),
        ),
    ] {
        reaches_and_refuses(
            what,
            &page(&show(FAR), procedure, "0 0 10 10", 10, "<< >>", "", &[]),
            "type-three-image-outside-its-box",
        );
    }
}

#[test]
fn a_glyph_drawn_as_an_image_inside_its_box_is_refused_when_the_region_cuts_it() {
    // THE CARRIER: the glyph's box covers its image, so the region removes the glyph -- and the
    // bitmap would stay in `/CharProcs`.
    let procedure = b"200 0 d0\nq 200 0 0 30 0 0 cm BI /W 1 /H 1 /BPC 8 /CS /G ID \x00 EI Q\n";
    reaches_and_refuses(
        "glyph drawn as an image, in the region",
        &page(
            &show((100, 255)),
            procedure,
            "0 0 200 30",
            200,
            "<< >>",
            "",
            &[],
        ),
        "type-three-image-cut",
    );
    // THE TWIN: the same font, its glyph far from the region. Not cut, so not refused -- a TeX
    // bitmap document redacts wherever the region misses its bitmap-font text.
    let far = page(&show(FAR), procedure, "0 0 200 30", 200, "<< >>", "", &[]);
    redact(&far).expect("a bitmap glyph the region does not reach is kept");
}

#[test]
fn a_painting_font_named_only_inside_a_form_refuses_too() {
    let draw = show(FAR);
    let form = format!(
        "<< /Type /XObject /Subtype /Form /BBox [0 0 400 400] \
         /Resources << /Font << /T3 5 0 R >> >> /Length {} >>\nstream\n{draw}\nendstream",
        draw.len()
    );
    let procedure = format!("10 0 d0\n{INTO_REGION} re f\n");
    let pdf = page(
        "/Fm0 Do",
        procedure.as_bytes(),
        "0 0 10 10",
        10,
        "<< >>",
        "/XObject << /Fm0 8 0 R >>",
        &[&form],
    );
    reaches_and_refuses("through a form", &pdf, "type-three-procedure-paints");
}
