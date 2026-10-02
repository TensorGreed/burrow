//! Each #229 shape draws annotation ink into the region it is redacted over, and is refused; each
//! twin draws none there and redacts, with its annotation still drawn.
//!
//! Measured with PDFium **drawing annotations** (`dark_pixels_with_annotations`), which neither
//! burrow's renderer nor its verification does -- the reason these shapes returned `Ok` before
//! #229. The code review of #229 found the engine tests' fixtures pinned the refusals' shape
//! without drawing into the region; these are the fixtures that do, so deleting a refusal would
//! leave ink where the person asked for none.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::collections::BTreeSet;

use burrow_engines::pdfsyntax::region::Region;
use support::char_box_oracle::dark_pixels_with_annotations;

/// A file of `objects`, numbered from 1, with a classic cross-reference.
fn pdf(objects: &[String]) -> Vec<u8> {
    let mut out = String::from("%PDF-1.7\n");
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.push_str(&format!("{} 0 obj\n{body}\nendobj\n", index + 1));
    }
    let xref_at = out.len();
    out.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objects.len() + 1
    ));
    for offset in &offsets {
        out.push_str(&format!("{offset:010} 00000 n \n"));
    }
    out.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
        objects.len() + 1
    ));
    out.into_bytes()
}

fn stream(dict: &str, data: &str) -> String {
    format!(
        "<< {dict} /Length {} >>\nstream\n{data}\nendstream",
        data.len()
    )
}

/// A 300 x 400 page drawing only `KEEP`, clear of every region here, with one annotation.
fn page_with(page_extra: &str, annotation: &str, appearance: &str) -> Vec<u8> {
    page_with_objects(
        page_extra,
        annotation,
        "<< /N 7 0 R >>",
        &[appearance.to_owned()],
    )
}

/// As [`page_with`], with the annotation's `/AP` given -- none, when empty -- and the objects it
/// names numbered from 7.
fn page_with_objects(page_extra: &str, annotation: &str, ap: &str, objects: &[String]) -> Vec<u8> {
    let mut all = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 400] /Contents 4 0 R \
             /Resources << /Font << /F1 5 0 R >> >> /Annots [6 0 R] {page_extra} >>"
        ),
        stream("", "BT /F1 12 Tf 200 20 Td (KEEP) Tj ET"),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_owned(),
        // NO `/AP` AT ALL when `ap` is empty.
        if ap.is_empty() {
            format!("<< /Type /Annot {annotation} >>")
        } else {
            format!("<< /Type /Annot {annotation} /AP {ap} >>")
        },
    ];
    all.extend(objects.iter().cloned());
    pdf(&all)
}

fn appearance(bbox: &str, draws: &str) -> String {
    stream(
        &format!("/Type /XObject /Subtype /Form {bbox} /Resources << /Font << /F1 5 0 R >> >>"),
        draws,
    )
}

/// Redact over `region` (display space), returning the refusal rule or the output.
fn redact(bytes: &[u8], region: (i32, i32, i32, i32)) -> Result<Vec<u8>, String> {
    let (left, top, width, height) = region;
    let region = Region {
        left: f64::from(left),
        top: f64::from(top),
        width: f64::from(width),
        height: f64::from(height),
    };
    support::redact_page(bytes, 0, BTreeSet::from([0]), region)
        .map(|(output, _)| output)
        .map_err(|error| format!("{error:?}"))
}

/// The upright page's top band, which every upright shape here draws into.
const BAND: (i32, i32, i32, i32) = (0, 30, 300, 40);

#[test]
fn an_unbounded_appearance_draws_into_the_region_and_is_refused() {
    let draws = "BT /F1 18 Tf 0 290 Td (ANNOTATIONINK) Tj ET";
    let unbounded = page_with(
        "",
        "/Subtype /Stamp /Rect [20 50 120 70]",
        &appearance("", draws),
    );
    let (inside, _) = dark_pixels_with_annotations(&unbounded, 0, BAND);
    assert!(
        inside > 300,
        "the fixture must draw into the region: {inside} dark pixels"
    );
    let refused = redact(&unbounded, BAND).expect_err("refused");
    assert!(
        refused.contains("[annotation-appearance-unbounded]"),
        "{refused}"
    );

    // THE TWIN: bounded, the same appearance is fitted into its `/Rect` and draws nothing there.
    let bounded = page_with(
        "",
        "/Subtype /Stamp /Rect [20 50 120 70]",
        &appearance("/BBox [0 0 100 20]", draws),
    );
    assert_eq!(dark_pixels_with_annotations(&bounded, 0, BAND).0, 0);
    let output = redact(&bounded, BAND).expect("a bounded appearance redacts");
    let (inside, total) = dark_pixels_with_annotations(&output, 0, BAND);
    assert_eq!(inside, 0);
    assert!(total > 0, "the kept annotation and KEEP are still drawn");
}

#[test]
fn a_no_rotate_annotation_draws_beside_its_rect_and_is_refused() {
    // On the page turned 90 degrees the `/Rect` shows at x 120..140, y 200..300; PDFium draws the
    // upright appearance from its corner, x 140..240, y 200..220. The region is beside the `/Rect`.
    const BESIDE: (i32, i32, i32, i32) = (150, 200, 90, 20);
    let block = appearance("/BBox [0 0 100 20]", "0 g 0 0 100 20 re f");
    let no_rotate = page_with(
        "/Rotate 90",
        "/Subtype /Stamp /Rect [200 120 300 140] /F 16",
        &block,
    );
    let (inside, _) = dark_pixels_with_annotations(&no_rotate, 0, BESIDE);
    assert!(
        inside > 1_000,
        "the fixture must draw into the region: {inside} dark pixels"
    );
    let refused = redact(&no_rotate, BESIDE).expect_err("refused");
    assert!(refused.contains("[annotation-no-rotate]"), "{refused}");

    // THE TWIN: the same annotation without NoRotate turns with the page and stays in its `/Rect`.
    let turning = page_with(
        "/Rotate 90",
        "/Subtype /Stamp /Rect [200 120 300 140]",
        &block,
    );
    assert_eq!(dark_pixels_with_annotations(&turning, 0, BESIDE).0, 0);
    let output = redact(&turning, BESIDE).expect("an annotation that turns with the page redacts");
    let (inside, total) = dark_pixels_with_annotations(&output, 0, BESIDE);
    assert_eq!(inside, 0);
    assert!(total > 1_000, "the kept annotation is still drawn");
}

#[test]
fn a_highlight_drawn_at_its_quadrilaterals_is_refused() {
    let draws = "BT /F1 14 Tf 2 4 Td (HILITEINK) Tj ET";
    let block = appearance("/BBox [0 0 100 20]", draws);
    let outside = page_with(
        "",
        "/Subtype /Highlight /Rect [20 50 120 70] /QuadPoints [0 370 300 370 0 330 300 330] \
         /PDFIUM_HasGeneratedAP true",
        &block,
    );
    let (inside, _) = dark_pixels_with_annotations(&outside, 0, BAND);
    assert!(
        inside > 300,
        "the fixture must draw into the region: {inside} dark pixels"
    );
    let refused = redact(&outside, BAND).expect_err("refused");
    assert!(
        refused.contains("[annotation-quads-outside-rect]"),
        "{refused}"
    );

    // THE TWIN: the quadrilateral inside the `/Rect`, so PDFium draws there and nowhere else.
    let inside_rect = page_with(
        "",
        "/Subtype /Highlight /Rect [20 50 120 70] /QuadPoints [20 70 120 70 20 50 120 50] \
         /PDFIUM_HasGeneratedAP true",
        &block,
    );
    assert_eq!(dark_pixels_with_annotations(&inside_rect, 0, BAND).0, 0);
    let output = redact(&inside_rect, BAND).expect("quadrilaterals inside the /Rect redact");
    let (inside, total) = dark_pixels_with_annotations(&output, 0, BAND);
    assert_eq!(inside, 0);
    assert!(total > 0, "the kept annotation is still drawn");
}

#[test]
fn an_appearance_reached_again_as_its_own_state_dictionary_is_refused() {
    // BOTH REVIEWS OF #229's MEMO: an `/AP` naming itself as its `/N`. Read as an `/AP` it is
    // `/N`, `/D` and `/R`; PDFium then reads it as the state dictionary and draws `/Off`, which a
    // memo shared between the two roles never examined.
    let unbounded = appearance("", "BT /F1 18 Tf 0 290 Td (ANNOTATIONINK) Tj ET");
    let looping = page_with_objects(
        "",
        "/Subtype /Stamp /Rect [20 50 120 70] /AS /Off",
        "7 0 R",
        &["<< /N 7 0 R /Off 8 0 R >>".to_owned(), unbounded],
    );
    let (inside, _) = dark_pixels_with_annotations(&looping, 0, BAND);
    assert!(
        inside > 300,
        "the fixture must draw into the region: {inside} dark pixels"
    );
    let refused = redact(&looping, BAND).expect_err("refused");
    assert!(
        refused.contains("[annotation-appearance-unbounded]"),
        "{refused}"
    );
}

#[test]
fn a_highlight_named_by_a_string_is_still_a_highlight() {
    // #229's REVIEWS: PDFium reads `/Subtype` as a byte string, so `(Highlight)` draws at its
    // quadrilaterals as a Highlight does.
    let block = appearance(
        "/BBox [0 0 100 20]",
        "BT /F1 14 Tf 2 4 Td (HILITEINK) Tj ET",
    );
    let string = page_with(
        "",
        "/Subtype (Highlight) /Rect [20 50 120 70] /QuadPoints [0 370 300 370 0 330 300 330] \
         /PDFIUM_HasGeneratedAP true",
        &block,
    );
    let (inside, _) = dark_pixels_with_annotations(&string, 0, BAND);
    assert!(
        inside > 300,
        "the fixture must draw into the region: {inside} dark pixels"
    );
    let refused = redact(&string, BAND).expect_err("refused");
    assert!(
        refused.contains("[annotation-quads-outside-rect]"),
        "{refused}"
    );
}

#[test]
fn a_highlight_with_no_appearance_is_drawn_at_its_quadrilaterals_and_refused() {
    // #229's THIRD CODE REVIEW: the worst shape measured, and no fixture had it -- every helper
    // wrote an `/AP`, so a quadrilateral check folded into the appearance walk's early return for
    // a missing `/AP` passed every suite, leaving 12,000 dark pixels in the region after an `Ok`.
    // PDFium builds this appearance itself, at the quadrilaterals.
    let outside = page_with_objects(
        "",
        "/Subtype /Highlight /Rect [20 50 120 70] /QuadPoints [0 370 300 370 0 330 300 330] \
         /C [0 0 0]",
        "",
        &[],
    );
    let (inside, _) = dark_pixels_with_annotations(&outside, 0, BAND);
    assert!(
        inside > 1_000,
        "the fixture must draw into the region: {inside} dark pixels"
    );
    let refused = redact(&outside, BAND).expect_err("refused");
    assert!(
        refused.contains("[annotation-quads-outside-rect]"),
        "{refused}"
    );

    // THE TWIN: quadrilaterals inside the `/Rect`, built there and nowhere else.
    let inside_rect = page_with_objects(
        "",
        "/Subtype /Highlight /Rect [20 50 120 70] /QuadPoints [20 70 120 70 20 50 120 50] \
         /C [0 0 0]",
        "",
        &[],
    );
    assert_eq!(dark_pixels_with_annotations(&inside_rect, 0, BAND).0, 0);
    let output = redact(&inside_rect, BAND).expect("quadrilaterals inside the /Rect redact");
    let (inside, total) = dark_pixels_with_annotations(&output, 0, BAND);
    assert_eq!(inside, 0);
    assert!(total > 1_000, "the kept highlight is still drawn");
}
