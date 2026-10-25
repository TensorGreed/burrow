//! F-2 (ADR 0029): a run of removed glyphs leaves ONE `TJ` number, not one per glyph.
//!
//! The rewriter replaces each removed glyph with the `TJ` adjustment that reproduces its advance,
//! so the kept text after it does not move. Writing one adjustment per glyph put every removed
//! glyph's width in the output in the clear -- a sequence of widths is the glyph-position
//! de-redaction class. The rule: every number between two kept codes of one show operation --
//! each removed glyph's adjustment and any producer kern among them -- is summed and written
//! once. What remains disclosed is the run's total width, which ADR 0029 records as a residual.
//!
//! Each test reads the output through PDFium, not through burrow: the removed glyphs are gone,
//! the kept ones are where PDFium drew them before the removal, and the content stream holds the
//! number of `TJ` numbers the rule says it must. The twin is a single removed glyph, which is
//! one number under either rule.

#![cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "../testsupport/pdf_reading.rs"]
mod pdf_reading;
mod support;

use std::collections::BTreeSet;

use burrow_engines::pdfsyntax::region::Region;
use support::char_box_oracle::{chars_on_page, dark_pixels_with_annotations, origins_close};

const HELVETICA: &[u8] = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>";

/// A one-page, 400-point document drawing `content` with `/F2` Helvetica.
fn page(content: &str) -> Vec<u8> {
    let content = content.as_bytes();
    let all: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 400] /Resources << /Font << /F2 4 0 R \
          >> >> /Contents 5 0 R >>"
            .to_vec(),
        HELVETICA.to_vec(),
        [
            format!("<< /Length {} >>\nstream\n", content.len()).as_bytes(),
            content,
            b"\nendstream",
        ]
        .concat(),
    ];
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

/// What PDFium draws on the page, as `(character, origin)` in drawing order.
fn drawn(pdf: &[u8]) -> Vec<(char, (f64, f64))> {
    chars_on_page(pdf, 0)
        .into_iter()
        .filter(|c| !c.generated)
        .map(|c| (char::from_u32(c.unicode).unwrap_or('?'), c.origin))
        .collect()
}

/// The numbers inside the output's one `TJ` array, read from the page's content stream.
///
/// Through `qpdf --qdf`, because qpdf flates the content stream it writes; and the array must be
/// found, so a stream this cannot read panics rather than counting zero numbers.
fn tj_numbers(pdf: &[u8]) -> Vec<f64> {
    let expanded = pdf_reading::expanded(pdf);
    let text = String::from_utf8_lossy(&expanded);
    let end = text
        .find("] TJ")
        .expect("the output's content stream has a rewritten `TJ`");
    let start = text[..end].rfind('[').expect("the `TJ` array opens");
    text[start + 1..end]
        .split_whitespace()
        .filter_map(|token| token.parse::<f64>().ok())
        .collect()
}

/// Redact `region` (display space) and hold the output to the rule.
///
/// `removed` is what must be gone; every other character PDFium drew before must be drawn after,
/// at the same origin; and the rewritten array must hold exactly `numbers` numbers.
#[track_caller]
fn redacts_to(what: &str, content: &str, region: Region, removed: &str, numbers: usize) {
    let pdf = page(content);
    let before = drawn(&pdf);
    // THE FIXTURE DISCRIMINATES: every character to be removed is drawn inside the region, and
    // every kept one outside it -- by PDFium's own origins, not burrow's.
    let (left, right) = (region.left, region.left + region.width);
    for (c, (x, _)) in &before {
        assert_eq!(
            removed.contains(*c),
            (left..right).contains(x),
            "{what}: the fixture does not separate {c:?} at x = {x} from the region {left}..{right}"
        );
    }
    let covered: BTreeSet<usize> = [0].into_iter().collect();
    let (out, _) = support::redact_page(&pdf, 0, covered, region)
        .unwrap_or_else(|error| panic!("{what}: {error}"));
    let after = drawn(&out);
    let kept: Vec<_> = before
        .iter()
        .filter(|(c, _)| !removed.contains(*c))
        .collect();
    assert_eq!(
        after.len(),
        kept.len(),
        "{what}: PDFium draws {after:?} after, where it should draw {kept:?}"
    );
    for ((c, at), (was_c, was)) in after.iter().zip(kept) {
        assert_eq!(c, was_c, "{what}: drawn in a different order");
        assert!(
            origins_close(*at, *was),
            "{what}: {c:?} moved from {was:?} to {at:?}"
        );
    }
    // The fixtures' regions are whole points, so the pixel rectangle is the region itself.
    let pixels = (
        whole(region.left),
        whole(region.top),
        whole(region.width),
        whole(region.height),
    );
    assert_eq!(
        dark_pixels_with_annotations(&out, 0, pixels).0,
        0,
        "{what}: ink left in the region"
    );
    let found = tj_numbers(&out);
    assert_eq!(
        found.len(),
        numbers,
        "{what}: the rewritten `TJ` holds {found:?}; one removed run is one number"
    );
}

/// A whole-point coordinate of a fixture's region, as the pixel count PDFium renders it at.
fn whole(value: f64) -> i32 {
    assert!(
        value.fract() == 0.0 && (0.0..=400.0).contains(&value),
        "a fixture region off whole points: {value}"
    );
    #[allow(clippy::cast_possible_truncation)]
    let pixels = value as i32;
    pixels
}

/// x 100..300, y 250..300 in PDF space.
const WIDE: Region = Region {
    left: 100.0,
    top: 100.0,
    width: 200.0,
    height: 50.0,
};

/// THE FIXTURE. `Wim` -- three glyphs of three different widths (944, 222 and 833 thousandths in
/// Helvetica) -- removed from between kept text, with the producer's kerns on either side of it.
/// One number: the two kerns and the three advances, summed. One adjustment per glyph wrote five.
#[test]
fn a_removed_run_of_three_glyphs_across_kerns_leaves_one_number() {
    redacts_to(
        "kerned run",
        "BT /F2 20 Tf 50 260 Td [(AB) -2000 (Wim) -8000 (CD)] TJ ET",
        WIDE,
        "Wim",
        1,
    );
}

/// THE SAME RULE INSIDE ONE STRING, with no producer numbers at all: `40 Tc` spreads the glyphs
/// so the region takes `Wim` out of the middle of `ABWimCD`. One number, which is the three
/// advances and their character spacing.
#[test]
fn a_removed_run_of_three_glyphs_inside_one_string_leaves_one_number() {
    redacts_to(
        "one string",
        "BT /F2 20 Tf 40 Tc 10 260 Td (ABWimCD) Tj ET",
        Region {
            left: 100.0,
            top: 100.0,
            width: 150.0,
            height: 50.0,
        },
        "Wim",
        1,
    );
}

/// THE TWIN: one removed glyph, in one string with no producer numbers, is one number under
/// either rule -- and the kept text after it stays put.
#[test]
fn a_single_removed_glyph_leaves_one_number() {
    redacts_to(
        "single glyph",
        "BT /F2 20 Tf 40 Tc 10 260 Td (ABWCD) Tj ET",
        Region {
            left: 100.0,
            top: 100.0,
            width: 50.0,
            height: 50.0,
        },
        "W",
        1,
    );
}

/// L-4 (F-2 round 1): A TRAILING RUN FOLLOWED BY A SHOW OPERATION ON THE SAME PEN. `W` ends the
/// first `Tj`, so its one number is the last item of the rewritten array; nothing in that array
/// comes after it, but the second `Tj` draws `CD` from where the first left the pen. That number
/// is what keeps `C` and `D` in place -- dropping the array's final flush moves them left by
/// `W`'s advance, which this asserts against PDFium's origins.
#[test]
fn a_trailing_run_keeps_a_later_show_operation_on_the_same_pen_in_place() {
    redacts_to(
        "trailing run",
        "BT /F2 20 Tf 40 Tc 10 260 Td (ABW) Tj (CD) Tj ET",
        Region {
            left: 100.0,
            top: 100.0,
            width: 50.0,
            height: 50.0,
        },
        "W",
        1,
    );
}
