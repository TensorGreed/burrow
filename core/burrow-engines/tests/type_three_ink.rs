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
    page_with_procedure_dict(
        content,
        procedure,
        "",
        "<< /g 7 0 R >>",
        bbox,
        width,
        font_resources,
        page_resources,
        extra,
    )
}

/// As [`page`], with `procedure_dict` written into the glyph procedure's stream dictionary.
#[allow(clippy::too_many_arguments)]
fn page_with_procedure_dict(
    content: &str,
    procedure: &[u8],
    procedure_dict: &str,
    char_procs: &str,
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
    // A CALLER NAMING ITS OWN `/Font` replaces the default, rather than adding a second key --
    // which the engine would repair, and redaction would refuse for that reason instead.
    let fonts = if page_resources.contains("/Font") {
        ""
    } else {
        "/Font << /T3 5 0 R >>"
    };
    let mut objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] \
             /Resources << {fonts} {page_resources} >> /Contents 4 0 R >>"
        )
        .into_bytes(),
        stream("", content.as_bytes()),
        format!(
            "<< /Type /Font /Subtype /Type3 /FontBBox [{bbox}] /FontMatrix [1 0 0 1 0 0] \
             /CharProcs 6 0 R /Encoding << /Type /Encoding /Differences [97 /g] >> \
             /FirstChar 97 /LastChar 97 /Widths [{width}] /Resources {font_resources} >>"
        )
        .into_bytes(),
        char_procs.as_bytes().to_vec(),
        stream(procedure_dict, procedure),
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

/// TWO FONTS, ONE PROCEDURE (#125's code review). `/CharProcs` is shared, so the procedure is
/// scanned once -- and its image was judged against whichever font was scanned first. With the
/// wide-boxed font first, the small-boxed one inherited "inside" and the page returned `Ok` over
/// 6,000 dark pixels. The verdict is per font now; both name orders, and a second font with no box
/// at all, must refuse.
#[test]
fn a_procedure_shared_by_two_fonts_is_judged_against_each_fonts_box() {
    let procedure = b"10 0 d0\nq 200 0 0 30 -280 245 cm BI /W 1 /H 1 /BPC 8 /CS /G ID \x00 EI Q\n";
    let font = |bbox: Option<&str>| {
        format!(
            "<< /Type /Font /Subtype /Type3 {} /FontMatrix [1 0 0 1 0 0] /CharProcs 6 0 R \
             /Encoding << /Type /Encoding /Differences [97 /g] >> /FirstChar 97 /LastChar 97 \
             /Widths [10] /Resources << >> >>",
            bbox.map_or(String::new(), |b| format!("/FontBBox [{b}]"))
        )
    };
    let wide = font(Some("-300 0 10 300"));
    for (what, first, second, small) in [
        ("wide font named first", "A", "B", font(Some("0 0 10 10"))),
        ("wide font named second", "Z", "B", font(Some("0 0 10 10"))),
        ("small font with no box", "A", "B", font(None)),
    ] {
        // The wide font (object 8) under `first`, the small one (object 9) under `second`; the
        // page draws the small one at FAR, where its procedure's image lands in the region.
        let content = format!(
            "BT /{first} 1 Tf 500 500 Td (a) Tj ET BT /{second} 1 Tf {} {} Td (a) Tj ET",
            FAR.0, FAR.1
        );
        let pdf = page(
            &content,
            procedure,
            "0 0 10 10",
            10,
            "<< >>",
            &format!("/Font << /{first} 8 0 R /{second} 9 0 R >>"),
            &[&wide, &small],
        );
        reaches_and_refuses(what, &pdf, "type-three-image-outside-its-box");
    }
}

/// A `/MATRIX` ON THE PROCEDURE (#125's security review): PDFium applies it before the procedure's
/// own `cm`, so an image the scan judged inside its box lands in the region. Refused rather than
/// modelled; its twin is the same procedure without the `/Matrix`, which the box contains.
#[test]
fn a_glyph_procedure_carrying_its_own_matrix_refuses() {
    let procedure = b"200 0 d0\nq 200 0 0 30 0 0 cm BI /W 1 /H 1 /BPC 8 /CS /G ID \x00 EI Q\n";
    let moved = page_with_procedure_dict(
        &show(FAR),
        procedure,
        "/Matrix [1 0 0 1 -280 245]",
        "<< /g 7 0 R >>",
        "0 0 200 30",
        200,
        "<< >>",
        "",
        &[],
    );
    reaches_and_refuses(
        "/Matrix on the procedure",
        &moved,
        "type-three-procedure-matrix",
    );
    let plain = page(&show(FAR), procedure, "0 0 200 30", 200, "<< >>", "", &[]);
    assert_eq!(
        dark_in_region(&plain),
        0,
        "without /Matrix the image stays at the glyph"
    );
    redact(&plain).expect("an image inside its box, its glyph outside the region, is kept");
}

/// PADDED `cm` OPERANDS (#125's security review): PDFium takes the last six, and a scan taking the
/// first six read the identity. Refused by count, as the page walk refuses one.
#[test]
fn a_padded_cm_in_a_glyph_procedure_refuses() {
    let procedure =
        b"10 0 d0\nq 1 0 0 1 0 0 200 0 0 30 -280 245 cm BI /W 1 /H 1 /BPC 8 /CS /G ID \x00 EI Q\n";
    reaches_and_refuses(
        "padded cm",
        &page(&show(FAR), procedure, "0 0 10 10", 10, "<< >>", "", &[]),
        "operand-count-mismatch",
    );
}

/// TWO `cm`s COMPOSE IN ORDER (#125's security review): the second applies inside the first. In the
/// right order this image lands in the region, outside a box that the wrong order would put it
/// inside -- so a composition reversed returned `Ok` over 6,200 dark pixels.
#[test]
fn two_cms_in_a_glyph_procedure_compose_in_order() {
    let procedure = b"10 0 d0\nq 200 0 0 30 0 0 cm 1 0 0 1 -1.4 8.1667 cm \
                      BI /W 1 /H 1 /BPC 8 /CS /G ID \x00 EI Q\n";
    reaches_and_refuses(
        "two cms",
        &page(&show(FAR), procedure, "-2 0 200 40", 10, "<< >>", "", &[]),
        "type-three-image-outside-its-box",
    );
}

/// THE CUT RULE THROUGH A SHARED PROCEDURE (#125's security review). Two fonts name one image
/// procedure, each box containing the image; the region cuts a glyph of whichever font is scanned
/// second. A cache hit that did not record the font as drawing an image let that cut go through,
/// with the bitmap left in `/CharProcs`. Both name orders.
#[test]
fn a_cut_glyph_of_the_second_font_sharing_an_image_procedure_refuses() {
    let procedure = b"200 0 d0\nq 200 0 0 30 0 0 cm BI /W 1 /H 1 /BPC 8 /CS /G ID \x00 EI Q\n";
    let font = "<< /Type /Font /Subtype /Type3 /FontBBox [0 0 200 30] /FontMatrix [1 0 0 1 0 0] \
                /CharProcs 6 0 R /Encoding << /Type /Encoding /Differences [97 /g] >> \
                /FirstChar 97 /LastChar 97 /Widths [200] /Resources << >> >>";
    for (first, second) in [("A", "B"), ("Z", "B")] {
        // `first` is drawn far from the region; `second`, the one the region cuts, at (100, 255).
        let content =
            format!("BT /{first} 1 Tf 380 10 Td (a) Tj ET BT /{second} 1 Tf 100 255 Td (a) Tj ET");
        let pdf = page(
            &content,
            procedure,
            "0 0 200 30",
            200,
            "<< >>",
            &format!("/Font << /{first} 8 0 R /{second} 9 0 R >>"),
            &[font, font],
        );
        reaches_and_refuses(
            &format!("{first} then {second}"),
            &pdf,
            "type-three-image-cut",
        );
    }
}

/// `/Subtype (Type3)`: A STRING WHERE A NAME BELONGS (#125's second security review). PDFium reads
/// `/Subtype` by its bytes, so the font is Type 3 to it; read as "not a name", every Type 3 rule
/// skipped it -- a procedure painting, drawing an image or SHOWING TEXT into the region returned
/// `Ok` with all of it still there (the text case is older than this slice, on main). Refused. The
/// fill and the image are here; the text is the corpus fixture `evade-type3-subtype-as-a-string`.
#[test]
fn a_type_three_font_whose_subtype_is_a_string_refuses() {
    for (what, procedure) in [
        (
            "a fill",
            format!("10 0 d0\n{INTO_REGION} re f\n").into_bytes(),
        ),
        (
            "an image",
            b"10 0 d0\nq 200 0 0 30 -280 245 cm BI /W 1 /H 1 /BPC 8 /CS /G ID \x00 EI Q\n".to_vec(),
        ),
    ] {
        let pdf = page(&show(FAR), &procedure, "0 0 10 10", 10, "<< >>", "", &[]);
        let pdf = with_string_subtype(&pdf);
        reaches_and_refuses(what, &pdf, "subtype-not-a-name");
    }
}

/// The same for a Form XObject (`/Subtype (Form)`), which the walk read as "not a form" and never
/// entered: its text in the region stayed, 879 dark pixels before and after (older than this slice).
#[test]
fn a_form_whose_subtype_is_a_string_refuses() {
    let draw = "BT /F1 20 Tf 110 260 Td (SECRETS) Tj ET";
    let font = "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>";
    let form = |subtype: &str| {
        format!(
            "<< /Type /XObject /Subtype {subtype} /BBox [0 0 400 400] \
             /Resources << /Font << /F1 9 0 R >> >> /Length {} >>\nstream\n{draw}\nendstream",
            draw.len()
        )
    };
    let build = |subtype: &str| {
        page(
            "/Fm0 Do",
            b"10 0 d0\n",
            "0 0 10 10",
            10,
            "<< >>",
            "/XObject << /Fm0 8 0 R >>",
            &[&form(subtype), font],
        )
    };
    reaches_and_refuses("(Form)", &build("(Form)"), "subtype-not-a-name");
    // THE TWIN: the same form under a name redacts, and its text is gone.
    let named = build("/Form");
    assert!(dark_in_region(&named) > 0);
    let (out, _) = redact(&named).expect("a named form redacts");
    assert_eq!(dark_in_region(&out), 0);
}

/// `/Subtype /Type3` rewritten in place as `(Type3)`, the same length so the cross-reference holds.
fn with_string_subtype(pdf: &[u8]) -> Vec<u8> {
    let from = b"/Subtype /Type3";
    let to = b"/Subtype(Type3)";
    let at = pdf
        .windows(from.len())
        .position(|w| w == from)
        .expect("the Type 3 font's subtype");
    let (head, rest) = pdf.split_at(at);
    let tail = rest.get(from.len()..).expect("the subtype's own bytes");
    let out = [head, to.as_slice(), tail].concat();
    assert!(
        !out.windows(from.len()).any(|w| w == from),
        "exactly one Type 3 subtype, and it was rewritten"
    );
    out
}

/// 4,000 NAMES FOR ONE TYPE 3 FONT OF 4,000 PROCEDURE KEYS (#125's second security review): the
/// font was judged once per name and the deadline read on no cache hit -- 7.4 s against 500 ms in
/// the review's measurement, 7.3 s against this test's 50 ms in ours. Judged once per font object,
/// the deadline read per key, it stops at the budget instead: the refusal must be the DURATION
/// limit, so a test that stopped for any other reason, or on any other ceiling, fails.
#[test]
fn many_names_for_one_type_three_font_stop_at_the_deadline() {
    let keys: String = (0..4000).map(|i| format!("/g{i} 7 0 R ")).collect();
    let names: String = (0..4000).map(|i| format!("/T{i} 5 0 R ")).collect();
    let shows: String = (0..4000)
        .map(|i| format!("BT /T{i} 1 Tf 380 10 Td (a) Tj ET\n"))
        .collect();
    let pdf = page_with_procedure_dict(
        &shows,
        b"10 0 d0\n",
        "",
        &format!("<< {keys}>>"),
        "0 0 10 10",
        10,
        "<< >>",
        &format!("/Font << {names}>>"),
        &[],
    );
    let started = std::time::Instant::now();
    let covered: BTreeSet<usize> = [0].into_iter().collect();
    let outcome = support::redact_page_with(
        &pdf,
        0,
        covered,
        REGION,
        burrow_types::Limits::with(|l| l.max_duration_ms = 50),
    );
    let elapsed = started.elapsed();
    match outcome {
        Err(burrow_types::Error::LimitExceeded {
            limit: "max_duration_ms",
            ..
        }) => {}
        other => panic!(
            "expected the deadline to stop it, got {:?}",
            other.map(|_| ())
        ),
    }
    assert!(
        elapsed < std::time::Duration::from_secs(3),
        "took {elapsed:?}"
    );
}

/// A TYPE 3 FONT WHOSE OWN `/Resources` NAMES THE FONT AGAIN redacts (#125's third code review).
///
/// The review predicted that the old sharing walk -- procedures walked per name, outside the
/// once-per-font guard -- refused this as `[resource-graph-cycle]`. Run against that shape, it did
/// not: the procedures are not yet on the open path when the inner reach walks them. It redacts
/// in both shapes. Pinned so a change to that guard changes this outcome on purpose: the procedure
/// draws nothing and is far from the region, so `Ok` is right.
#[test]
fn a_type_three_font_naming_itself_in_its_own_resources_redacts() {
    let pdf = page(
        &show(FAR),
        b"10 0 d0\n",
        "0 0 10 10",
        10,
        "<< /Font << /T3 5 0 R >> >>",
        "",
        &[],
    );
    assert!(
        pdf.windows(b"/Font << /T3 5 0 R >>".len())
            .filter(|w| *w == b"/Font << /T3 5 0 R >>")
            .count()
            >= 2,
        "the font's own resources name it, as the page's do"
    );
    redact(&pdf).expect("a self-naming Type 3 font far from the region redacts");
}
