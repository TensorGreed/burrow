//! The flatten render on the real PDFium: ADR 0031 *Fixtures* rows for §1 step 6 (the engine
//! half), rule 6 (render side), rule 8, rule 9's flags, and §4's transform matrix.
//!
//! Each test names the mutation that turns it red.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "tests"
)]

use std::sync::Arc;

use burrow_types::{Clock, Deadline, Error, Limits, ManualClock, Result};

use super::{Painted, Through, created, render_painted, render_painted_through};
use crate::pdfium::{Pdfium, PdfiumDocument};
use crate::pdfsyntax::region::{PageFrame, Region};
use crate::qpdf::Qpdf;
use crate::redact::flatten::cross_check::{check_paint, check_rectangle, is_paint_disagreement};
use crate::redact::flatten::{Picture, pixels_for};
use crate::redact::graph::{OpensForRedaction, PdfDocument};
use crate::{Annotations, DocumentEngine, OpenOptions, PageRenderer};

fn options(limits: Limits) -> OpenOptions<'static> {
    OpenOptions::new(limits, Arc::new(ManualClock::new(0)) as Arc<dyn Clock>)
}

fn web_limits() -> Limits {
    let mut limits = Limits::DEFAULT;
    limits.max_pixels = 4_194_304;
    limits
}

/// A one-page document: page entries `page`, content `content`, `objects` numbered from 5, and
/// `catalog` added to the catalogue.
fn document(page: &str, content: &str, objects: &[String], catalog: &str) -> Vec<u8> {
    let mut bodies: Vec<String> = vec![
        format!("<< /Type /Catalog /Pages 2 0 R {catalog} >>"),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        format!(
            "<< /Type /Page /Parent 2 0 R {page} {} /Contents 4 0 R >>",
            if page.contains("/Resources") {
                ""
            } else {
                "/Resources << >>"
            }
        ),
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len() + 1
        ),
    ];
    bodies.extend(objects.iter().cloned());
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in bodies.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", bodies.len() + 1).as_bytes(),
    );
    for offset in offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            bodies.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// Page 1's frame as qpdf reads it: what the paint converts against.
fn qpdf_frame(bytes: &[u8]) -> PageFrame {
    let (document, _) = Qpdf
        .open_for_redaction(bytes, &options(Limits::DEFAULT))
        .unwrap();
    crate::redact::frame::of(&document.page(0).unwrap()).unwrap()
}

fn open(bytes: &[u8], limits: Limits) -> PdfiumDocument {
    Pdfium::new()
        .open(bytes.to_vec().into_boxed_slice(), &options(limits))
        .unwrap()
}

fn paint(bytes: &[u8], region: &Region, limits: Limits, through: Through) -> Result<Painted> {
    let frame = qpdf_frame(bytes);
    let doc = open(bytes, limits);
    let options = options(limits);
    let deadline = Deadline::start(options.clock.as_ref(), &options.limits);
    render_painted_through(&doc, 0, &frame, region, &options, &deadline, through)
}

fn honest(bytes: &[u8], region: &Region) -> Painted {
    paint(bytes, region, Limits::DEFAULT, super::honest).unwrap()
}

/// The plain render the selection picture uses (`Annotations::Drawn`), as RGB, at `size`.
fn plain(bytes: &[u8], size: (u32, u32)) -> Vec<u8> {
    let doc = open(bytes, Limits::DEFAULT);
    let options = options(Limits::DEFAULT);
    let deadline = Deadline::start(options.clock.as_ref(), &options.limits);
    let raster = PageRenderer::render(
        &Pdfium::new(),
        &doc,
        0,
        size.0,
        size.1,
        Annotations::Drawn,
        &options,
        &deadline,
    )
    .unwrap();
    let (pixels, _) = raster.rgba.as_chunks::<4>();
    pixels.iter().flat_map(|&[r, g, b, _]| [r, g, b]).collect()
}

fn pixel(rgb: &[u8], width: u32, x: u32, y: u32) -> [u8; 3] {
    let at = ((y * width + x) * 3) as usize;
    [rgb[at], rgb[at + 1], rgb[at + 2]]
}

/// A region inside a display frame `dw` x `dh`, off every centre line, edges between pixels.
fn region_in(dw: f64, dh: f64) -> Region {
    Region {
        left: dw * 0.31 + 0.137,
        top: dh * 0.23 + 0.211,
        width: dw * 0.2,
        height: dh * 0.1,
    }
}

/// §4's transform matrix, render side (ADR 0031 *Fixtures*): every rotation by box origin by crop
/// offset, PDFium's paint through `FPDF_PageToDevice` against burrow's display-space check, which
/// must agree; and every pixel the paint did not touch equals the selection picture's render at
/// the same size (one route, rule 7; the paint touches only its rectangle).
///
/// KILLS: a sign flip in region.rs's display-to-user conversion; the swap removed from
/// `displayed_size`; the fill skipped; the rounding pixel removed; RGB channel order swapped.
#[test]
fn every_rotation_origin_and_crop_paints_where_burrow_s_arithmetic_says() {
    let mut examined = 0;
    for rotate in [0, 90, 180, 270] {
        for media in ["[0 0 300 500]", "[100 50 400 550]", "[-200 -100 100 400]"] {
            for crop in ["", "/CropBox [10 20 210 420]"] {
                let crop = if media.starts_with("[-") && !crop.is_empty() {
                    "/CropBox [-190 -80 10 320]"
                } else if media.starts_with("[100") && !crop.is_empty() {
                    "/CropBox [110 70 310 470]"
                } else {
                    crop
                };
                let page = format!("/MediaBox {media} {crop} /Rotate {rotate}");
                // The whole media box red, so the paint's black and the page's colour differ.
                let fill = media
                    .trim_matches(['[', ']'])
                    .split(' ')
                    .collect::<Vec<_>>();
                let content = format!(
                    "1 0 0 rg {} {} {} {} re f",
                    fill[0],
                    fill[1],
                    fill[2].parse::<f64>().unwrap() - fill[0].parse::<f64>().unwrap(),
                    fill[3].parse::<f64>().unwrap() - fill[1].parse::<f64>().unwrap()
                );
                let bytes = document(&page, &content, &[], "");
                let frame = qpdf_frame(&bytes);
                let display = frame.displayed_size();
                let region = region_in(display.0, display.1);
                let painted = honest(&bytes, &region);
                check_paint(display, &painted.picture(), &region, &Limits::DEFAULT)
                    .unwrap_or_else(|e| panic!("{page}: {e}"));

                let size = (painted.width, painted.height);
                let unpainted = plain(&bytes, size);
                let mut outside = 0_u64;
                for y in 0..size.1 {
                    for x in 0..size.0 {
                        let got = pixel(&painted.rgb, size.0, x, y);
                        if painted.painted.contains(x, y) {
                            assert_eq!(got, [0, 0, 0], "{page}: ({x}, {y}) inside the paint");
                        } else {
                            assert_eq!(
                                got,
                                pixel(&unpainted, size.0, x, y),
                                "{page}: ({x}, {y}) outside the paint"
                            );
                            outside += 1;
                        }
                    }
                }
                // THE PAGE IS RED where nothing was painted: the channels are in RGB order.
                assert_eq!(pixel(&painted.rgb, size.0, 2, 2), [255, 0, 0], "{page}");
                assert!(outside > 0, "{page}");
                examined += 1;
            }
        }
    }
    assert_eq!(examined, 24, "4 rotations x 3 origins x 2 crops");
}

/// §1 step 6 on the real engine: a transform two pixels off, and a picture nobody painted, are
/// refused; rounding half a pixel late is absorbed. KILLS: deleting the cross-check's pixel test.
#[test]
fn the_check_refuses_a_lying_transform_and_an_unpainted_picture_from_the_engine() {
    let bytes = document(
        "/MediaBox [0 0 612 792]",
        "0 0 1 rg 0 0 612 792 re f",
        &[],
        "",
    );
    let display = (612.0, 792.0);
    let region = region_in(display.0, display.1);

    let shifted: Through = |t, x, y| t.page_to_device(x, y).map(|(a, b)| (a + 2, b));
    let lied = paint(&bytes, &region, Limits::DEFAULT, shifted).unwrap();
    let error = check_paint(display, &lied.picture(), &region, &Limits::DEFAULT).unwrap_err();
    assert!(is_paint_disagreement(&error), "{error}");

    let unpainted = plain(&bytes, (1275, 1650));
    let error = check_paint(
        display,
        &Picture::new(1275, 1650, &unpainted),
        &region,
        &Limits::DEFAULT,
    )
    .unwrap_err();
    assert!(is_paint_disagreement(&error), "{error}");

    // Twin: the honest paint passes.
    let painted = honest(&bytes, &region);
    check_paint(display, &painted.picture(), &region, &Limits::DEFAULT).unwrap();
}

/// Rule 8's edge (ADR 0031 *Fixtures*): a red hairline in the column one past the column the
/// region's right edge touches is painted over; red ink two columns past it stays.
///
/// On Letter one pixel is 0.48 pt. The region's right edge is at pixel 300.4, so it touches
/// column 300; rule 8's pixel is column 301; the paint, from `round(300.4) + 2`, ends at 302.
/// KILLS: removing rule 8's pixel from `PAINT_MARGIN`.
#[test]
fn a_hairline_one_pixel_outside_the_region_is_painted_and_ink_two_outside_is_not() {
    let px = |p: f64| p * 0.48;
    let region = Region {
        left: px(150.0),
        top: px(150.0),
        width: px(300.4) - px(150.0),
        height: px(150.0),
    };
    // User-space y of the region's rows: 792 - top - height .. 792 - top.
    let (low, high) = (792.0 - px(300.0) + 2.0, 792.0 - px(150.0) - 2.0);
    let line = |column: f64| {
        let x = px(column + 0.5);
        format!("1 0 0 RG 0 w {x} {low} m {x} {high} l S")
    };
    let rows = 160..290;
    for (column, painted_over) in [(301_u32, true), (302_u32, false)] {
        let bytes = document("/MediaBox [0 0 612 792]", &line(f64::from(column)), &[], "");
        let unpainted = plain(&bytes, (1275, 1650));
        assert!(
            rows.clone()
                .all(|y| pixel(&unpainted, 1275, column, y) != [255, 255, 255]),
            "column {column}: the hairline is drawn there before the paint"
        );
        let painted = honest(&bytes, &region);
        check_paint(
            (612.0, 792.0),
            &painted.picture(),
            &region,
            &Limits::DEFAULT,
        )
        .unwrap();
        let after: Vec<[u8; 3]> = rows
            .clone()
            .map(|y| pixel(&painted.rgb, 1275, column, y))
            .collect();
        if painted_over {
            assert!(after.iter().all(|p| *p == [0, 0, 0]), "column {column}");
        } else {
            assert!(
                after
                    .iter()
                    .all(|p| *p != [0, 0, 0] && *p != [255, 255, 255]),
                "column {column}: ink two columns out is not painted"
            );
        }
    }
}

/// Rule 6 on the render side (ADR 0031 *Fixtures*): the web's ceiling page renders, one row more
/// is refused, and redaction's own ceiling refuses one row more natively -- each **before a
/// bitmap is created**. KILLS: the ceilings checked after `FPDFBitmap_Create`.
#[test]
fn the_pixel_ceilings_refuse_before_any_bitmap_is_created() {
    let region = Region {
        left: 10.0,
        top: 10.0,
        width: 10.0,
        height: 10.0,
    };
    let at = |w: &str, h: &str| document(&format!("/MediaBox [0 0 {w} {h}]"), "", &[], "");

    let ceiling = paint(&at("983", "983"), &region, web_limits(), super::honest).unwrap();
    assert_eq!((ceiling.width, ceiling.height), (2048, 2048));

    let over = paint(&at("983", "983.5"), &region, web_limits(), super::honest);
    assert!(
        matches!(
            over,
            Err(Error::LimitExceeded {
                limit: "max_pixels",
                ..
            })
        ),
        "{over:?}"
    );
    assert!(
        !created::any(2048, 2049),
        "a bitmap was made for a refused page"
    );

    // Native: 2992.5 x 6888.75 pt is 6235 x 14352, one row past 89,478,485.
    assert_eq!(
        (pixels_for(2992.5).unwrap(), pixels_for(6888.75).unwrap()),
        (6235, 14352)
    );
    let over = paint(
        &at("2992.5", "6888.75"),
        &region,
        Limits::DEFAULT,
        super::honest,
    );
    assert!(
        matches!(
            over,
            Err(Error::LimitExceeded {
                limit: "redaction_flatten_pixels",
                ..
            })
        ),
        "{over:?}"
    );
    assert!(
        !created::any(6235, 14352),
        "a bitmap was made for a refused page"
    );
}

/// The operation's deadline is the render's: a spent one refuses before the engine is asked.
#[test]
fn a_spent_deadline_refuses_before_the_render() {
    let bytes = document("/MediaBox [0 0 612 792]", "", &[], "");
    let frame = qpdf_frame(&bytes);
    let clock = Arc::new(ManualClock::new(0));
    let mut limits = Limits::DEFAULT;
    limits.max_duration_ms = 10;
    let options = OpenOptions::new(limits, Arc::clone(&clock) as Arc<dyn Clock>);
    let doc = Pdfium::new()
        .open(bytes.into_boxed_slice(), &options)
        .unwrap();
    let deadline = Deadline::start(options.clock.as_ref(), &options.limits);
    clock.advance(11);
    let outcome = render_painted(
        &doc,
        0,
        &frame,
        &region_in(612.0, 792.0),
        &options,
        &deadline,
    );
    assert!(
        matches!(
            outcome,
            Err(Error::LimitExceeded {
                limit: "max_duration_ms",
                ..
            })
        ),
        "{outcome:?}"
    );
}

/// A region the cross-check would refuse is refused by the paint too, before PDFium is asked to
/// fill anything: wholly off the page.
#[test]
fn a_region_off_the_page_is_refused_by_both_sides() {
    let bytes = document("/MediaBox [0 0 612 792]", "", &[], "");
    let off = Region {
        left: 700.0,
        top: 10.0,
        width: 10.0,
        height: 10.0,
    };
    assert!(matches!(
        paint(&bytes, &off, Limits::DEFAULT, super::honest),
        Err(Error::InvalidArgument(_))
    ));
    assert!(matches!(
        check_rectangle((612.0, 792.0), (1275, 1650), &off),
        Err(Error::InvalidArgument(_))
    ));
}

/// One annotation per case on a 400 x 400 page, each in its own 40 x 40 cell unless the case
/// names a `/Rect`; a `/Square` unless it names a `/Subtype`; with `/AP /N` the black form (object
/// 5) unless it names an `/AP`. Object 6 is an optional-content group the catalogue turns off;
/// object 7 a red form. Returns the document and the user-space point each case is probed at.
fn annotated(cases: &[(&str, &str)]) -> (Vec<u8>, Vec<(f64, f64)>) {
    let form = |ink: &str| {
        let body = format!("{ink} rg 0 0 40 40 re f");
        format!(
            "<< /Type /XObject /Subtype /Form /BBox [0 0 40 40] /Length {} >>\nstream\n{body}\nendstream",
            body.len() + 1
        )
    };
    let mut objects = vec![
        form("0 0 0"),
        "<< /Type /OCG /Name (off) >>".to_owned(),
        form("1 0 0"),
    ];
    let mut annots = Vec::new();
    let mut probes = Vec::new();
    for (i, (rect, extra)) in cases.iter().enumerate() {
        annots.push(format!("{} 0 R", 8 + i));
        let rect = if rect.is_empty() {
            let (x, y) = (20 + 60 * (i % 6) as u32, 20 + 60 * (i / 6) as u32);
            probes.push((f64::from(x + 20), f64::from(y + 20)));
            format!("[{x} {y} {} {}]", x + 40, y + 40)
        } else {
            // A named rectangle is probed 10 pt inside its lower-left corner.
            let numbers: Vec<f64> = rect
                .trim_matches(['[', ']'])
                .split(' ')
                .map(|n| n.parse().unwrap())
                .collect();
            probes.push((numbers[0] + 10.0, numbers[1] + 10.0));
            (*rect).to_owned()
        };
        let subtype = if extra.contains("/Subtype") {
            ""
        } else {
            "/Subtype /Square"
        };
        let ap = if extra.contains("/AP") {
            ""
        } else {
            "/AP << /N 5 0 R >>"
        };
        objects.push(format!(
            "<< /Type /Annot {subtype} /Rect {rect} {ap} {extra} >>"
        ));
    }
    let page = format!("/MediaBox [0 0 400 400] /Annots [{}]", annots.join(" "));
    let catalog = "/OCProperties << /OCGs [6 0 R] /D << /OFF [6 0 R] >> >>";
    (document(&page, "", &objects, catalog), probes)
}

/// Rule 9 (ADR 0031 §2): the picture holds exactly what PDFium draws with `FPDF_ANNOT` alone and
/// no form-fill handle, **measured on the pinned build** (DECISIONS rule 2's amendment) rather
/// than read from the specification. Twin: the ordinary annotation (`/F 4`), drawn.
///
/// What the pinned PDFium does, as measured 2026-10-30 (aarch64):
/// - NoView (`/F 36`, with Print) and Hidden (`/F 2`): **not drawn**. No flags (`/F 0`): drawn.
/// - ToggleNoView (256) is **ignored**: `/F 260` drawn, `/F 292` (with NoView) not.
/// - `/AP /N` as a dictionary of states with no `/AS`: the **`/Off`** state is drawn, whatever
///   the key order. With `/AS /Off`, the same.
/// - An annotation under optional content its catalogue turns **off**: **drawn**. PDFium does
///   not apply `/OC` to annotations on this route, so the picture can show what a viewer that
///   honours it hides; one render serves the selection and the output (rule 7), so it is shown
///   in both.
/// - A `/Rect` partly outside the page's box: drawn where it falls on the page.
/// - `/Popup`, `/Open` true or false, with an `/AP` or an empty one: **not drawn**.
/// - `/AP /R` and `/AP /D`: never drawn; `/N` is.
///
/// KILLS: `FPDF_PRINTING` added to the flags (NoView-but-Print appears, unflagged vanishes).
#[test]
fn rule_9_annotations_draw_as_measured_on_the_pinned_pdfium() {
    const INK: [u8; 3] = [0, 0, 0];
    const RED: [u8; 3] = [255, 0, 0];
    const NONE: [u8; 3] = [255, 255, 255];
    let cases: &[(&str, &str, [u8; 3])] = &[
        ("", "/F 4", INK),
        ("", "/F 0", INK),
        ("", "/F 36", NONE),
        ("", "/F 2", NONE),
        ("", "/F 260", INK),
        ("", "/F 292", NONE),
        ("", "/F 4 /AP << /N << /On 5 0 R /Off 7 0 R >> >>", RED),
        ("", "/F 4 /AP << /N << /Off 7 0 R /On 5 0 R >> >>", RED),
        (
            "",
            "/F 4 /AP << /N << /On 5 0 R /Off 7 0 R >> >> /AS /On",
            INK,
        ),
        ("", "/F 4 /OC 6 0 R", INK),
        ("[370 370 410 410]", "/F 4", INK),
        ("", "/Subtype /Popup /F 4 /Open true", NONE),
        ("", "/Subtype /Popup /F 4 /Open false", NONE),
        ("", "/Subtype /Popup /F 4 /Open true /AP << >>", NONE),
        ("", "/F 4 /AP << /N 5 0 R /R 7 0 R /D 7 0 R >>", INK),
    ];
    let shapes: Vec<(&str, &str)> = cases.iter().map(|(r, e, _)| (*r, *e)).collect();
    let (bytes, probes) = annotated(&shapes);
    // The region is away from every cell, so the paint does not cover a probe.
    let region = Region {
        left: 200.0,
        top: 385.0,
        width: 10.0,
        height: 10.0,
    };
    let painted = honest(&bytes, &region);
    let scale = 150.0 / 72.0;
    let mut examined = 0;
    for ((_, extra, expected), (x, y)) in cases.iter().zip(probes) {
        let (px, py) = ((x * scale) as u32, ((400.0 - y) * scale) as u32);
        assert!(
            !painted.painted.contains(px, py),
            "{extra}: probe under the paint"
        );
        assert_eq!(
            pixel(&painted.rgb, painted.width, px, py),
            *expected,
            "{extra}"
        );
        examined += 1;
    }
    assert_eq!(examined, 15);
}

/// Rule 8 at the bitmap's own edge: a region overrunning the page's bottom-right corner. The
/// cross-check cannot see this one by itself, because the paint and the check share
/// `PixelRect::outward`'s outward rounding and its clamp into the bitmap; a clamp one pixel short
/// moves both rectangles together and the check still passes. So the last row and the last
/// column are held black here, pixel by pixel, after an `Ok` check.
///
/// KILLS: `outward` clamping right/bottom to `width - 1` / `height - 1` (both sides shrink, the
/// check passes, the page's red shows in the last row and column); the paint told the bitmap is
/// one row shorter than it is (the check refuses).
#[test]
fn a_region_over_the_bottom_right_corner_paints_the_last_row_and_column() {
    let bytes = document(
        "/MediaBox [0 0 612 792]",
        "1 0 0 rg 0 0 612 792 re f",
        &[],
        "",
    );
    let region = Region {
        left: 500.0,
        top: 700.0,
        width: 200.0,
        height: 200.0,
    };
    let painted = honest(&bytes, &region);
    let (width, height) = (painted.width, painted.height);
    assert_eq!((width, height), (1275, 1650), "612 x 792 pt at 150 dpi");
    check_paint(
        (612.0, 792.0),
        &painted.picture(),
        &region,
        &Limits::DEFAULT,
    )
    .unwrap();

    // The region's first column and row, as burrow places them: 500 and 700 pt from the top-left.
    let first_column = (500.0 * f64::from(width) / 612.0).floor() as u32;
    let first_row = (700.0 * f64::from(height) / 792.0).floor() as u32;
    let mut examined = 0_u32;
    for y in first_row..height {
        let got = pixel(&painted.rgb, width, width - 1, y);
        assert_eq!(got, [0, 0, 0], "last column, row {y}");
        examined += 1;
    }
    for x in first_column..width {
        let got = pixel(&painted.rgb, width, x, height - 1);
        assert_eq!(got, [0, 0, 0], "last row, column {x}");
        examined += 1;
    }
    assert_eq!(examined, (height - first_row) + (width - first_column));
    // Twin: outside the region the page is red, so black above is the paint and not the page.
    assert_eq!(
        pixel(&painted.rgb, width, width - 1, first_row - 4),
        [255, 0, 0]
    );
    assert_eq!(
        pixel(&painted.rgb, width, first_column - 4, height - 1),
        [255, 0, 0]
    );
}
