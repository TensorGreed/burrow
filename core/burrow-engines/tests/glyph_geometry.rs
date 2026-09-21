//! Glyph geometry, checked against PDFium's character boxes (#129, ADR 0029 §6).
//!
//! The oracle itself is `support::char_box_oracle`; its header carries the argument for why
//! PDFium is admissible in a test binary and why the two box functions measure different things.
//!
//! # This file starts with the oracle, not with the geometry
//!
//! An oracle is a measuring instrument, and an instrument nobody calibrated reports whatever it
//! reports. So before any geometry is written against it, these assert what it can and cannot
//! see — that it reads real boxes off a real page, that two pages differing by one term give
//! different boxes, and that the tolerance is smaller than the smallest term this suite uses.
//!
//! Written first deliberately. Geometry written against an uncalibrated oracle is geometry
//! nothing has ever disagreed with, which is the state #129 exists to avoid.

#![cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "a test indexing a fixture it just built and asserted the length of; the workspace \
              lints are for attacker-controlled input in library code, where they stay denied. \
              The sibling test binaries carry the same allowance"
)]

mod support;

use support::char_box_oracle::{
    MIN_FIXTURE_DISPLACEMENT_PT, OracleChar, Rect, TOLERANCE_PT, assert_fixture_is_discriminating,
    chars_on_page,
};

/// A one-page PDF drawing `text` with the given text-object body.
///
/// Hand-built and byte-exact, in the style of `testsupport/minimal_pdf.rs`: a fixture measures
/// the shape it was written for, and a library's optimiser rewriting it measures the optimiser.
/// `/Helvetica` is a standard 14 font, so PDFium supplies the metrics and the fixture carries no
/// font program — which is what keeps these fixtures about **geometry** rather than about font
/// parsing.
fn page_with(body: &str) -> Vec<u8> {
    let content = format!("BT\n{body}\nET\n");
    let mut objects: Vec<String> = Vec::new();
    objects.push("<< /Type /Catalog /Pages 2 0 R >>".to_owned());
    objects.push("<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_owned());
    objects.push(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
            .to_owned(),
    );
    objects.push(format!(
        "<< /Length {} >>\nstream\n{content}endstream",
        content.len()
    ));
    objects.push(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_owned(),
    );

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

#[test]
fn the_oracle_reads_boxes_off_a_real_page() {
    // THE NON-VACUITY CONTROL, and it comes first: every assertion in this file rests on the
    // oracle returning something. An oracle that returned an empty list would make every
    // comparison below pass over nothing.
    let page = page_with("/F1 12 Tf 100 700 Td (AB) Tj");
    let chars = chars_on_page(&page, 0);

    assert_eq!(chars.len(), 2, "PDFium read {} characters", chars.len());
    assert_eq!(chars[0].unicode, u32::from('A'));
    assert_eq!(chars[1].unicode, u32::from('B'));

    // The boxes are where the content stream put them, to the point.
    assert!(
        (chars[0].loose.left - 100.0).abs() < 1.0,
        "the first glyph should start at the Td, and starts at {:?}",
        chars[0].loose
    );
    assert!(
        chars[1].loose.left > chars[0].loose.left,
        "the second glyph should advance past the first"
    );
    // And they have extent, which a degenerate box would not.
    for (at, c) in chars.iter().enumerate() {
        assert!(
            c.loose.right > c.loose.left && c.loose.top > c.loose.bottom,
            "glyph {at}'s loose box is degenerate: {:?}",
            c.loose
        );
    }
}

#[test]
fn the_ink_box_sits_inside_the_loose_box() {
    // THE RELATIONSHIP THE TWO PROPERTIES REST ON. `assert_agrees` asserts P1 against the loose
    // box and P2 against the ink box; both only make sense if PDFium's own two boxes stand in
    // the containment relation this claims. Measured here rather than assumed from the header
    // comment in `fpdf_text.h`.
    let page = page_with("/F1 24 Tf 100 700 Td (Hg) Tj");
    let chars = chars_on_page(&page, 0);
    assert!(!chars.is_empty());

    for (at, c) in chars.iter().enumerate() {
        let escaped = c.loose.escape_of(&c.ink);
        assert!(
            escaped <= TOLERANCE_PT,
            "glyph {at}'s ink escapes its own loose box by {escaped:.4} pt -- the oracle's two \
             properties cannot both be asserted against boxes that do not nest.\n  loose: {:?}\n  \
             ink:   {:?}",
            c.loose,
            c.ink
        );
    }
}

#[test]
fn the_oracle_can_tell_two_pages_apart_by_one_term() {
    // THE DISCRIMINATION CONTROL. An oracle that returned the same boxes whatever the content
    // stream said would make every term's test pass. `Ts` is used because it moves a glyph on
    // one axis only, so a difference cannot be a coincidence of two changes cancelling.
    let flat = chars_on_page(&page_with("/F1 12 Tf 100 700 Td (A) Tj"), 0);
    let risen = chars_on_page(&page_with("/F1 12 Tf 100 700 Td 4 Ts (A) Tj"), 0);

    assert_fixture_is_discriminating(&risen, &flat, "Ts of 4 on a 12 pt font");

    // AND IN THE RIGHT DIRECTION, which the distance alone does not say: a rise lifts the glyph.
    assert!(
        risen[0].loose.bottom > flat[0].loose.bottom,
        "a positive Ts should raise the glyph: {:?} vs {:?}",
        risen[0].loose,
        flat[0].loose
    );
}

#[test]
fn the_tolerance_is_smaller_than_the_smallest_term_this_suite_uses() {
    // THE TOLERANCE'S OWN ARGUMENT, at COMPILE time. Both are constants, so clippy pointed out
    // that a runtime assertion on them is always-true -- which is the lint being right about
    // something better: this can be a `const` block, and then changing one constant without the
    // other does not build at all rather than failing a test somebody has to run.
    const {
        assert!(
            TOLERANCE_PT * 20.0 <= MIN_FIXTURE_DISPLACEMENT_PT,
            "the tolerance must stay at least twenty times below the minimum fixture \
             displacement; one of the two was changed without the other"
        );
    }

    // And the smallest term the suite actually uses clears the minimum -- checked against a real
    // page rather than against the constant, so a fixture written later cannot quietly fall
    // under it while this test still passes.
    let flat = chars_on_page(&page_with("/F1 12 Tf 100 700 Td (AA) Tj"), 0);
    let spaced = chars_on_page(&page_with("/F1 12 Tf 100 700 Td 2 Tc (AA) Tj"), 0);
    let moved = spaced[1].loose.left - flat[1].loose.left;
    assert!(
        moved >= MIN_FIXTURE_DISPLACEMENT_PT,
        "Tc of 2 moves the second glyph only {moved:.4} pt"
    );
}

#[test]
fn escape_of_measures_the_worst_edge_rather_than_any_overlap() {
    // THE HELPER'S OWN PROBE. `escape_of` is what P2 rests on, and a version that reported
    // "these overlap" rather than "this protrudes" would pass a box hanging off one side --
    // which is exactly the shape a missing `Tz` or `Ts` produces.
    let outer = Rect {
        left: 0.0,
        bottom: 0.0,
        right: 10.0,
        top: 10.0,
    };
    let inside = Rect {
        left: 1.0,
        bottom: 1.0,
        right: 9.0,
        top: 9.0,
    };
    let over_one_edge = Rect {
        left: 1.0,
        bottom: 1.0,
        right: 12.5,
        top: 9.0,
    };

    assert_eq!(outer.escape_of(&inside), 0.0);
    assert_eq!(outer.escape_of(&over_one_edge), 2.5);
    // Three edges inside and one outside must report the one.
    assert!(
        outer.escape_of(&over_one_edge) > TOLERANCE_PT,
        "a box hanging 2.5 pt off one edge must not read as contained"
    );
}

#[test]
fn a_char_count_mismatch_is_a_failure_rather_than_a_pairwise_comparison() {
    // `assert_agrees` compares pairwise, so a page where burrow finds fewer glyphs than PDFium
    // would otherwise compare the ones it found and pass. This asserts the guard exists, by
    // calling it with a deliberately short list.
    let oracle: Vec<OracleChar> = chars_on_page(&page_with("/F1 12 Tf 100 700 Td (AB) Tj"), 0);
    assert_eq!(oracle.len(), 2);

    let one_box = vec![oracle[0].loose];
    let outcome = std::panic::catch_unwind(|| {
        support::char_box_oracle::assert_agrees(&one_box, &oracle, "deliberate mismatch");
    });
    assert!(
        outcome.is_err(),
        "assert_agrees accepted one box against two characters"
    );
}
