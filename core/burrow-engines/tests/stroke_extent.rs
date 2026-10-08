//! #278: a stroke inks as wide as the graphics state says, wherever the state was set.
//!
//! Every shape here is from #278's specification review, which measured each against PDFium on
//! `main` before any code was written. The reaching shapes all returned `Ok` there with ink in
//! the region:
//!
//! - a width set only through `gs` (3,168 dark pixels at 2 px/pt);
//! - a miter limit set only through `gs`;
//! - a page `/ExtGState` a form falls back to (7,128);
//! - an indirect `/LW` (7,128);
//! - an escaped key (7,128);
//! - a `gs` inside a text object (7,128);
//! - a projecting square cap under a miter limit below the square root of two (20);
//! - text drawn in a stroking rendering mode (2,772).
//!
//! What this file pins:
//!
//! - Each reaching shape now refuses, or, for stroked **text**, has its glyph removed.
//! - Each near-miss twin returns `Ok`.
//! - Each fixture that is meant to draw into the region does draw into it, measured by PDFium on
//!   the input, so a refusal here is a refusal of ink that is there (DECISIONS.md rule 7).
//!   Each `Ok` leaves no dark pixel in the region, measured on the output.
//!
//! Two over-refusals are pinned rather than narrowed (rule 4): a page `/LW` that PDFium does not
//! reach from a form that has an `/ExtGState` category of its own, and a negative `/LW`. In both,
//! PDFium draws a thin line and burrow refuses.

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

/// The same rectangle for the pixel count, at one pixel a point: left, top, width, height.
const PIXELS: (i32, i32, i32, i32) = (100, 100, 200, 50);

/// A Helvetica with flat widths, for the stroked-text cases.
const FONT: &str = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 32 \
                    /LastChar 126 /Widths [600 600 600 600 600 600 600 600 600 600 600 600 600 \
                    600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 \
                    600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 \
                    600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 \
                    600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 600 \
                    600 600 600 600 600] /Encoding /WinAnsiEncoding >>";

/// The horizontal stroke most cases draw: its centreline at y=200, 50 points below the region.
const LINE: &str = "0 G 50 200 m 350 200 l S";

/// A one-page document: catalog 1, pages 2, page 3, content 4, font 5, then `extra` from 6.
fn page(content: &str, resources: &str, extra: &[&str]) -> Vec<u8> {
    let content = content.as_bytes();
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] /Resources {resources} \
             /Contents 4 0 R >>"
        )
        .into_bytes(),
        [
            format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(),
            content,
            b"\nendstream",
        ]
        .concat(),
        FONT.as_bytes().to_vec(),
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

/// A form whose own resources are `resources`, drawing `content`.
fn form(resources: &str, content: &str) -> String {
    format!(
        "<< /Type /XObject /Subtype /Form /BBox [0 0 400 400] /Resources {resources} \
         /Length {} >>\nstream\n{content}\nendstream",
        content.len()
    )
}

/// The page's `/ExtGState` naming one state, `GS0`.
fn gs0(state: &str) -> String {
    format!("<< /ExtGState << /GS0 {state} >> >>")
}

fn redact(pdf: &[u8]) -> burrow_types::Result<(Vec<u8>, burrow_engines::redact::Report)> {
    let covered: BTreeSet<usize> = [0].into_iter().collect();
    support::redact_page(pdf, 0, covered, REGION)
}

/// How many glyphs the walk places on the first page.
fn glyphs(pdf: &[u8]) -> usize {
    burrow_engines::glyphs_on_first_page(pdf, &support::walk_options())
        .expect("the walk reads the page")
        .len()
}

fn dark_in_region(pdf: &[u8]) -> u64 {
    dark_pixels_with_annotations(pdf, 0, PIXELS).0
}

/// `pdf` draws into the region, and the redaction refuses it under `rule`.
#[track_caller]
fn reaches_and_refuses(what: &str, pdf: &[u8], rule: &str) {
    let dark = dark_in_region(pdf);
    assert!(
        dark > 0,
        "{what}: the fixture must draw into the region, and PDFium drew nothing there"
    );
    match redact(pdf) {
        Ok((out, _)) => panic!(
            "{what}: Ok over {dark} dark pixels in the region ({} bytes out), expected [{rule}]",
            out.len()
        ),
        Err(error) => {
            let message = error.to_string();
            assert!(
                message.contains(&format!("[{rule}]")),
                "{what}: refused, but not as [{rule}]: {message}"
            );
        }
    }
}

/// `pdf` redacts, and PDFium draws nothing dark in the region of the output.
#[track_caller]
fn redacts_clean(what: &str, pdf: &[u8]) {
    let out = match redact(pdf) {
        Ok((out, _)) => out,
        Err(error) => panic!("{what}: expected Ok, refused: {error}"),
    };
    assert_eq!(
        dark_in_region(&out),
        0,
        "{what}: the output still inks the region"
    );
}

/// The refusal `pdf` produces, whatever PDFium draws: for the over-refusals pinned here.
#[track_caller]
fn refuses(what: &str, pdf: &[u8], rule: &str) {
    match redact(pdf) {
        Ok(_) => panic!("{what}: expected [{rule}], got Ok"),
        Err(error) => {
            let message = error.to_string();
            assert!(message.contains(&format!("[{rule}]")), "{what}: {message}");
        }
    }
}

#[test]
fn a_width_set_only_through_gs_is_the_width_the_stroke_is_boxed_at() {
    reaches_and_refuses(
        "/LW 110 /ML 1",
        &page(&format!("/GS0 gs {LINE}"), &gs0("<< /LW 110 /ML 1 >>"), &[]),
        "vector-in-region",
    );
    // REACH 60/2 x sqrt(2) = 42.4, so the box tops out at 242.4.
    redacts_clean(
        "/LW 60 /ML 1",
        &page(&format!("/GS0 gs {LINE}"), &gs0("<< /LW 60 /ML 1 >>"), &[]),
    );
    reaches_and_refuses(
        "/LW 110, default miter limit",
        &page(&format!("/GS0 gs {LINE}"), &gs0("<< /LW 110 >>"), &[]),
        "vector-in-region",
    );
    // REACH 9/2 x 10 = 45: the default miter limit's worst case, still short of 250.
    redacts_clean(
        "/LW 9, default miter limit",
        &page(&format!("/GS0 gs {LINE}"), &gs0("<< /LW 9 >>"), &[]),
    );
}

#[test]
fn a_miter_limit_set_only_through_gs_reaches_as_far_as_its_spike() {
    // A 20:1 V whose tip is at y=190: a miter limit of 50 lets the join's spike reach the region.
    let v = "0 G 10 w /GS1 gs 191.5 20 m 200 190 l 208.5 20 l S";
    reaches_and_refuses(
        "/ML 50",
        &page(v, "<< /ExtGState << /GS1 << /ML 50 >> >> >>", &[]),
        "vector-in-region",
    );
    // REACH 5 x 11 = 55, to 245; and a limit of 11 bevels this join, so PDFium inks less still.
    redacts_clean(
        "/ML 11",
        &page(v, "<< /ExtGState << /GS1 << /ML 11 >> >> >>", &[]),
    );
}

#[test]
fn a_form_with_no_ext_gstate_falls_back_to_the_pages() {
    let draw = format!("/GS0 gs {LINE}");
    let fm0 = form("<< /Font << /F1 5 0 R >> >>", &draw);
    reaches_and_refuses(
        "page /GS0 /LW 120",
        &page(
            "/Fm0 Do",
            "<< /XObject << /Fm0 6 0 R >> /ExtGState << /GS0 << /LW 120 >> >> >>",
            &[&fm0],
        ),
        "vector-in-region",
    );
    redacts_clean(
        "page /GS0 /LW 9",
        &page(
            "/Fm0 Do",
            "<< /XObject << /Fm0 6 0 R >> /ExtGState << /GS0 << /LW 9 >> >> >>",
            &[&fm0],
        ),
    );
}

/// The case that kills "take the largest value any scope sets": PDFium keeps the `120 w` set
/// before the `Do`, because the form's own `/GS0` -- or its category lacking the name -- does not
/// reach the page's `/LW 2`.
#[test]
fn the_width_in_force_is_a_candidate_when_a_scope_does_not_set_one() {
    let draw = format!("/GS0 gs {LINE}");
    for (what, own) in [
        (
            "own /GS0 without /LW",
            "<< /ExtGState << /GS0 << /CA 1 >> >> >>",
        ),
        (
            "own category lacks /GS0",
            "<< /ExtGState << /GSx << /CA 1 >> >> >>",
        ),
    ] {
        let fm0 = form(own, &draw);
        let resources = "<< /XObject << /Fm0 6 0 R >> /ExtGState << /GS0 << /LW 2 >> >> >>";
        reaches_and_refuses(
            what,
            &page("120 w /Fm0 Do", resources, &[&fm0]),
            "vector-in-region",
        );
        redacts_clean(what, &page("9 w /Fm0 Do", resources, &[&fm0]));
    }
}

/// PINNED OVER-REFUSAL (rule 4): the form's category lacks the name, so PDFium ignores the `gs`
/// and draws the default thin line; burrow cannot tell that from a fallback and boxes at 120.
#[test]
fn a_page_width_a_form_does_not_reach_is_over_refused() {
    let fm0 = form(
        "<< /ExtGState << /GSx << /CA 1 >> >> >>",
        &format!("/GS0 gs {LINE}"),
    );
    let pdf = page(
        "/Fm0 Do",
        "<< /XObject << /Fm0 6 0 R >> /ExtGState << /GS0 << /LW 120 >> >> >>",
        &[&fm0],
    );
    assert_eq!(dark_in_region(&pdf), 0, "PDFium draws the thin line");
    refuses("own category lacks the name", &pdf, "vector-in-region");
}

#[test]
fn an_indirect_width_is_applied_as_pdfium_applies_it() {
    reaches_and_refuses(
        "/LW 6 0 R -> 120",
        &page(
            &format!("/GS0 gs {LINE}"),
            &gs0("<< /LW 6 0 R >>"),
            &["120"],
        ),
        "vector-in-region",
    );
    redacts_clean(
        "/LW 6 0 R -> 9",
        &page(&format!("/GS0 gs {LINE}"), &gs0("<< /LW 6 0 R >>"), &["9"]),
    );
}

#[test]
fn a_width_or_limit_that_is_not_one_agreed_number_refuses() {
    for state in [
        "<< /LW (120) >>",
        "<< /LW [120] >>",
        "<< /LW /Big >>",
        "<< /LW 1000000000 >>",
        "<< /ML (50) >>",
        "<< /ML [50] >>",
        "<< /ML 1000000000 >>",
    ] {
        refuses(
            state,
            &page(&format!("/GS0 gs {LINE}"), &gs0(state), &[]),
            "ext-gstate-line-unreadable",
        );
    }
}

#[test]
fn an_ext_gstate_written_as_a_stream_is_ignored_as_pdfium_ignores_it() {
    let pdf = page(
        &format!("/GS0 gs {LINE}"),
        "<< /ExtGState << /GS0 6 0 R >> >>",
        &["<< /LW 120 /Length 0 >>\nstream\n\nendstream"],
    );
    assert_eq!(dark_in_region(&pdf), 0, "PDFium ignores the stream entry");
    redacts_clean("stream entry", &pdf);
}

#[test]
fn an_escaped_key_is_the_key_both_readers_read() {
    reaches_and_refuses(
        "/L#57 120",
        &page(&format!("/GS0 gs {LINE}"), &gs0("<< /L#57 120 >>"), &[]),
        "vector-in-region",
    );
    redacts_clean(
        "/L#57 9",
        &page(&format!("/GS0 gs {LINE}"), &gs0("<< /L#57 9 >>"), &[]),
    );
}

#[test]
fn where_the_gs_runs_decides_how_long_it_lasts() {
    reaches_and_refuses(
        "gs inside a text object",
        &page(&format!("BT /GS0 gs ET {LINE}"), &gs0("<< /LW 120 >>"), &[]),
        "vector-in-region",
    );
    let saved = page(&format!("q /GS0 gs Q {LINE}"), &gs0("<< /LW 120 >>"), &[]);
    assert_eq!(dark_in_region(&saved), 0, "Q restores the width");
    redacts_clean("gs inside q/Q", &saved);
    let fm0 = form("<< /ExtGState << /GS0 << /LW 120 >> >> >>", "/GS0 gs");
    let inside_form = page(
        &format!("/Fm0 Do {LINE}"),
        "<< /XObject << /Fm0 6 0 R >> >>",
        &[&fm0],
    );
    assert_eq!(
        dark_in_region(&inside_form),
        0,
        "the form's state ends with the form"
    );
    redacts_clean("gs inside a form, stroke after it", &inside_form);
}

/// PINNED OVER-REFUSAL (rule 4): PDFium draws a negative width thin; burrow boxes its magnitude.
#[test]
fn a_negative_width_is_boxed_at_its_magnitude() {
    let pdf = page(&format!("/GS0 gs {LINE}"), &gs0("<< /LW -120 >>"), &[]);
    assert_eq!(
        dark_in_region(&pdf),
        0,
        "PDFium draws a negative width thin"
    );
    refuses("/LW -120", &pdf, "vector-in-region");
}

#[test]
fn a_projecting_cap_under_a_low_miter_limit_is_inside_the_box() {
    reaches_and_refuses(
        "2 J 1 M",
        &page("0 G 40 w 2 J 1 M 175 175 m 225 225 l S", "<< >>", &[]),
        "vector-in-region",
    );
    // TEN POINTS LOWER: the cap's corner at 215 + 20 x sqrt(2) = 243.3.
    redacts_clean(
        "2 J 1 M, lowered",
        &page("0 G 40 w 2 J 1 M 175 165 m 225 215 l S", "<< >>", &[]),
    );
    reaches_and_refuses(
        "/LW 40 /LC 2 /ML 1",
        &page(
            "0 G /GS0 gs 175 175 m 225 225 l S",
            &gs0("<< /LW 40 /LC 2 /ML 1 >>"),
            &[],
        ),
        "vector-in-region",
    );
    redacts_clean(
        "/LW 40 /LC 2 /ML 1, lowered",
        &page(
            "0 G /GS0 gs 175 165 m 225 215 l S",
            &gs0("<< /LW 40 /LC 2 /ML 1 >>"),
            &[],
        ),
    );
}

/// THE RESIDUAL, pinned so it is seen: a zero width is PDFium's thinnest line, one device pixel,
/// which the walk boxes at zero. At y=249.8 PDFium leaves faint pixels in the region's edge row,
/// none of them dark. ADR 0029 §5 records it; the walk has no device scale to do better.
#[test]
fn a_zero_width_line_at_the_edge_is_the_recorded_residual() {
    let pdf = page(
        "0 G /GS0 gs 50 249.8 m 350 249.8 l S",
        &gs0("<< /LW 0 >>"),
        &[],
    );
    assert_eq!(dark_in_region(&pdf), 0);
    redacts_clean("/LW 0 at the edge", &pdf);
}

/// A STROKED GLYPH IS TEXT: its box grows by the stroke's reach, so the region removes it where
/// it inks the region. Refusing it as ink instead would turn away every faux-bold run.
#[test]
fn a_glyph_drawn_in_a_stroking_mode_is_removed_where_its_outline_reaches() {
    let fonts = "<< /Font << /F1 5 0 R >> >>";
    for (what, content, resources) in [
        (
            "2 Tr at 240 w",
            "0 G 0 g 240 w BT /F1 20 Tf 2 Tr 190 120 Td (I) Tj ET".to_owned(),
            fonts.to_owned(),
        ),
        (
            "1 Tr, width through gs",
            "0 G 0 g /GS0 gs BT /F1 20 Tf 1 Tr 190 120 Td (I) Tj ET".to_owned(),
            "<< /Font << /F1 5 0 R >> /ExtGState << /GS0 << /LW 240 >> >> >>".to_owned(),
        ),
        (
            "an out-of-range mode is treated as stroking",
            "0 G 0 g 240 w BT /F1 20 Tf 9 Tr 190 120 Td (I) Tj ET".to_owned(),
            fonts.to_owned(),
        ),
    ] {
        let pdf = page(&content, &resources, &[]);
        if what != "an out-of-range mode is treated as stroking" {
            assert!(
                dark_in_region(&pdf) > 0,
                "{what}: the stroke must reach the region"
            );
        }
        let (out, _) = match redact(&pdf) {
            Ok(done) => done,
            Err(error) => panic!("{what}: expected the glyph removed, refused: {error}"),
        };
        assert_eq!(glyphs(&pdf), 1, "{what}: the input shows one glyph");
        assert_eq!(glyphs(&out), 0, "{what}: the stroked glyph was not removed");
        assert_eq!(
            dark_in_region(&out),
            0,
            "{what}: the output still inks the region"
        );
    }
    // THE TWIN: filled, not stroked. Its box is the glyph's, outside the region, and it stays.
    let filled = page(
        "0 G 0 g 240 w BT /F1 20 Tf 0 Tr 190 120 Td (I) Tj ET",
        fonts,
        &[],
    );
    assert_eq!(dark_in_region(&filled), 0);
    redacts_clean("0 Tr", &filled);
    let (out, _) = redact(&filled).expect("redacts");
    assert_eq!(
        glyphs(&out),
        1,
        "the filled glyph outside the region is kept"
    );
}
