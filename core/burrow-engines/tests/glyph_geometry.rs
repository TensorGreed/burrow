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

use burrow_engines::pdfsyntax::geometry::{
    Encoding, Form, FormUses, Glyph, GlyphMetrics, Matrix, Rect as GeometryRect, Refusal,
    Resources, Watch, check_form_sharing, glyphs_in, remove_glyphs,
};
use burrow_engines::pdfsyntax::region::Region;
use burrow_types::{Deadline, Limits, ManualClock, Result};
use support::char_box_oracle::{
    MIN_FIXTURE_DISPLACEMENT_PT, OracleChar, Rect, TOLERANCE_PT, assert_fixture_is_discriminating,
    chars_on_page, ink_overlaps, origins_close, page_size,
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

/// A one-page PDF whose only glyph is a Type 3 procedure that draws **away from its origin**.
///
/// `/Widths [500]` and a `/FontMatrix` of 0.001 put the advance at half the font size; the
/// procedure draws a square at 2000..2500 in glyph space, which is 24..30 text-space units out.
/// So the ink is four advances to the right of the pen, and `/FontBBox [0 0 3000 3000]` is the
/// only thing in the file that says so.
fn page_with_off_origin_glyph() -> Vec<u8> {
    let glyph = b"500 0 d0\n2000 2000 500 500 re f\n";
    let content = b"BT\n/F1 12 Tf\n100 700 Td\n(a) Tj\nET\n";
    let objects: Vec<Vec<u8>> = vec![
        b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
        b"<< /Type /Pages /Count 1 /Kids [3 0 R] >>".to_vec(),
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
           /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
            .to_vec(),
        format!("<< /Length {} >>\nstream\n", content.len())
            .into_bytes()
            .into_iter()
            .chain(content.iter().copied())
            .chain(b"endstream".iter().copied())
            .collect(),
        b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 3000 3000] \
           /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /square 6 0 R >> \
           /Encoding << /Type /Encoding /Differences [97 /square] >> \
           /FirstChar 97 /LastChar 97 /Widths [500] /Resources << >> >>"
            .to_vec(),
        format!("<< /Length {} >>\nstream\n", glyph.len())
            .into_bytes()
            .into_iter()
            .chain(glyph.iter().copied())
            .chain(b"endstream".iter().copied())
            .collect(),
    ];

    let mut out: Vec<u8> = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", index + 1).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref_at = out.len();
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    out
}

#[test]
fn a_glyphs_ink_can_sit_entirely_outside_its_advance_box() {
    // THE LEAK THE CONSERVATIVE BOX EXISTS FOR, measured rather than argued.
    //
    // A region test against the advance box alone would put this glyph's box at 100..106 while
    // its ink is at 124..130 -- so a region covering the ink contains no part of the box, the
    // glyph is judged outside, and the operation leaves visible marks inside the area a person
    // asked to have cleared. Type 3 makes it trivial to construct; CFF and TrueType outlines
    // overhang their advances routinely, for accents, swashes and italic descenders.
    let page = page_with_off_origin_glyph();
    let chars = chars_on_page(&page, 0);
    assert_eq!(chars.len(), 1, "the fixture should draw exactly one glyph");
    let glyph = chars[0];

    // The pen is where the text object put it.
    assert!(
        (glyph.origin.0 - 100.0).abs() <= TOLERANCE_PT
            && (glyph.origin.1 - 700.0).abs() <= TOLERANCE_PT,
        "the origin should be the Td, and is {:?}",
        glyph.origin
    );

    // THE ADVANCE BOX, as a geometry pass computing only widths would produce it: the origin,
    // half the font size wide, the font size tall.
    let advance_box = Rect {
        left: glyph.origin.0,
        bottom: glyph.origin.1,
        right: glyph.origin.0 + 6.0,
        top: glyph.origin.1 + 12.0,
    };
    let escaped = advance_box.escape_of(&glyph.ink);
    assert!(
        escaped >= MIN_FIXTURE_DISPLACEMENT_PT,
        "this fixture exists to put ink outside the advance box, and the ink escapes it by only \
         {escaped:.4} pt -- the fixture no longer demonstrates anything.\n  advance: \
         {advance_box:?}\n  ink:     {:?}",
        glyph.ink
    );

    // AND THE CONSERVATIVE BOX COVERS IT. `/FontBBox [0 0 3000 3000]` scaled by the
    // `/FontMatrix` and the font size is 0..36 in both axes from the origin; unioned with the
    // advance box that is what the operation must reason about.
    let scaled_font_bbox = Rect {
        left: glyph.origin.0,
        bottom: glyph.origin.1,
        right: glyph.origin.0 + 36.0,
        top: glyph.origin.1 + 36.0,
    };
    assert_eq!(
        scaled_font_bbox.escape_of(&glyph.ink),
        0.0,
        "the scaled /FontBBox must contain the ink, or the conservative box is not conservative"
    );
}

#[test]
fn the_region_test_uses_the_conservative_box_and_not_the_advance_box() {
    // THE CONSEQUENCE, stated as the question a redaction actually asks: does this glyph
    // intersect the region? The two boxes give OPPOSITE answers on this fixture, which is why
    // the choice is a correctness decision rather than a matter of taste.
    let page = page_with_off_origin_glyph();
    let glyph = chars_on_page(&page, 0)[0];

    // A region drawn tightly around the ink.
    let region = Rect {
        left: 123.0,
        bottom: 723.0,
        right: 131.0,
        top: 731.0,
    };
    let intersects = |a: &Rect, b: &Rect| {
        a.left < b.right && b.left < a.right && a.bottom < b.top && b.bottom < a.top
    };

    let advance_box = Rect {
        left: glyph.origin.0,
        bottom: glyph.origin.1,
        right: glyph.origin.0 + 6.0,
        top: glyph.origin.1 + 12.0,
    };
    let conservative = Rect {
        left: glyph.origin.0,
        bottom: glyph.origin.1,
        right: glyph.origin.0 + 36.0,
        top: glyph.origin.1 + 36.0,
    };

    assert!(
        intersects(&region, &glyph.ink),
        "the fixture's region must contain the ink, or this test asks nothing"
    );
    assert!(
        !intersects(&region, &advance_box),
        "the advance box must NOT meet the region, or the two answers are not opposite"
    );
    assert!(
        intersects(&region, &conservative),
        "the conservative box must meet the region -- this is the answer the operation needs"
    );
}

/// A real producer's vertical document declares no vertical writing at all.
///
/// # What was measured, and why this fixture is hand-built
///
/// LibreOffice 24.2.7.2 was asked for a vertically-written Japanese paragraph
/// (`style:writing-mode="tb-rl"`). What came out contains **no `Identity-V`, no CID font and no
/// `WMode`**: a subset simple font and one fresh `Tm` per glyph, stepping `y` down by the font
/// size each time. Exactly the shape below.
///
/// That is worth pinning, because it says the `WMode` refusal is **not** what makes a vertical
/// document safe in the common case. This one is walked correctly, glyph by glyph, by the
/// ordinary horizontal machinery — and a rule that refused "documents that look vertical" would
/// refuse it for nothing.
///
/// # The real output is committed, and this is still hand-built
///
/// `tests/redaction/fixtures/producer-vertical-writing.pdf` is LibreOffice's actual output,
/// committed after a licence audit of the Noto CJK subset it embeds
/// (`tests/redaction/PROVENANCE.md`). The test below reads it. *This* fixture stays hand-built
/// and stays here, because the two answer different questions: the committed PDF says what a
/// producer really emits, and these bytes say what the walk does with that shape when nothing
/// else about the document varies. Replacing the second with the first would mean every future
/// failure came with a subset font attached to it.
///
/// # How to retake the measurement
///
/// `tools/make-producer-fixtures.py`, whose `vertical_writing` builder holds the flat-ODF
/// source. Taken 2026-09-21 against **LibreOffice 24.2.7.2** on Linux — an earlier draft of
/// this comment said "25.x", which was guessed rather than read, and the version is exactly
/// the kind of thing a later reader would rely on.
///
/// # Both halves are asserted
///
/// An earlier draft of this test called the oracle and nothing else, so it established that the
/// *fixture* descends and said nothing about burrow — while its name and its doc both claimed
/// the walk handles it. Found by review. It now walks the same bytes through `glyphs_in` and
/// requires the two to agree.
#[test]
fn a_vertical_run_laid_out_by_positioning_is_not_a_vertical_writing_mode() {
    let mut body = String::new();
    for step in 0..6 {
        let y = 700.0 - f64::from(step) * 24.0;
        // ONE `Tm` PER GLYPH, absolute, exactly as the producer emitted it. Not `TD`: a `Tm`
        // replaces both matrices, so a walk that treated it as relative would compound the
        // displacement and put the last glyph 360 points below where it is.
        body.push_str(&format!("1 0 0 1 509.35 {y} Tm /F1 24 Tf (A) Tj\n"));
    }
    let pdf = page_with(&body);
    let chars = support::char_box_oracle::chars_on_page(&pdf, 0);
    assert_eq!(chars.len(), 6, "the fixture must draw six glyphs");

    for pair in chars.windows(2) {
        let (upper, lower) = (&pair[0], &pair[1]);
        let drop = upper.origin.1 - lower.origin.1;
        assert!(
            (drop - 24.0).abs() < 0.05,
            "the run must descend by the font size, not advance across: {drop}"
        );
        assert!(
            (upper.origin.0 - lower.origin.0).abs() < 0.05,
            "a column, not a row: {} against {}",
            upper.origin.0,
            lower.origin.0
        );
    }

    // THE HALF THE NAME PROMISES. The walk is not refused, and it puts every glyph where
    // PDFium puts it -- which is what "walked correctly by the ordinary horizontal machinery"
    // has to mean if the sentence is to carry the weight ADR 0029 puts on it.
    let walked = glyphs_in(
        format!("BT\n{body}\nET\n").as_bytes(),
        &Helvetica,
        &unwatched(),
    )
    .expect("a positioned vertical run is not refused");
    assert_eq!(
        walked.len(),
        chars.len(),
        "the walk and the oracle disagree on count"
    );
    for (glyph, oracle) in walked.iter().zip(&chars) {
        assert!(
            (glyph.origin.0 - oracle.origin.0).abs() < TOLERANCE_PT
                && (glyph.origin.1 - oracle.origin.1).abs() < TOLERANCE_PT,
            "the walk put a glyph at {:?} and PDFium at {:?}",
            glyph.origin,
            oracle.origin
        );
    }
}

/// The resource seam the walk needs, standing in for a font resolver that does not exist yet.
///
/// Widths are Helvetica's for `A`. They do not affect this fixture's origins — every glyph is
/// placed by an absolute `Tm` — but a stub that lied about them would be a stub that could not
/// be reused for an advance test, so it tells the truth.
struct Helvetica;

impl Resources for Helvetica {
    fn within(&self, _name: &[u8]) -> Result<Option<Box<dyn Resources + '_>>> {
        // ONE FLAT RESOURCE SET. This fake models a page whose forms declare no `/Resources`
        // of their own, so every name resolves outwards -- which is what `None` means. It is
        // stated rather than defaulted: a trait default of `Ok(None)` would let a REAL
        // resolver inherit silently, and inheriting silently is the defect `within` exists to
        // fix.
        Ok(None)
    }
    fn form(&self, _name: &[u8]) -> Result<Option<Form>> {
        Ok(None)
    }

    fn glyph(&self, _name: &[u8], _code: u32) -> Result<GlyphMetrics> {
        Ok(GlyphMetrics {
            width: 667.0,
            bytes_per_code: 1,
            font_bbox: Some(GeometryRect {
                left: -166.0,
                bottom: -225.0,
                right: 1000.0,
                top: 931.0,
            }),
            font_matrix: Matrix::scale(0.001, 0.001),
            encoding: Encoding::Simple,
        })
    }

    fn bytes_per_code(&self, _name: &[u8]) -> Result<u8> {
        Ok(1)
    }
}

/// A padded operand run must not place a glyph where the renderer does not.
///
/// # The leak this closes, with both numbers measured
///
/// `ops::operations` gathers every pending operand into the operation; a renderer keeps a small
/// parameter buffer and takes the **last** N. The walk read the **first** N. So
/// `0 0 0 0 0 0 1 0 0 1 100 700 Tm` drew at `(100, 700)` for PDFium and computed `(0, 0)` here,
/// and a region over the visible word met no glyph box at all.
///
/// The suite could not have caught this on its own: every other fixture in it writes exact
/// operand counts, so a harness measuring what it can generate measured nothing. Found by
/// review, and this test is the witness.
#[test]
fn a_padded_operand_run_never_places_a_glyph_away_from_the_renderer() {
    let honest = "1 0 0 1 100 700 Tm /F1 12 Tf (A) Tj";
    let padded = "0 0 0 0 0 0 1 0 0 1 100 700 Tm /F1 12 Tf (A) Tj";

    // PDFium draws BOTH at the same place -- it takes the last six operands.
    let honest_chars = chars_on_page(&page_with(honest), 0);
    let padded_chars = chars_on_page(&page_with(padded), 0);
    assert_eq!(honest_chars.len(), 1);
    assert_eq!(padded_chars.len(), 1);
    assert!(
        (honest_chars[0].origin.0 - padded_chars[0].origin.0).abs() < TOLERANCE_PT
            && (honest_chars[0].origin.1 - padded_chars[0].origin.1).abs() < TOLERANCE_PT,
        "the two pages must be the same page to the renderer, or this test asks nothing: \
         {:?} against {:?}",
        honest_chars[0].origin,
        padded_chars[0].origin
    );

    // burrow agrees with the renderer on the honest page...
    let walked = glyphs_in(
        format!("BT\n{honest}\nET\n").as_bytes(),
        &Helvetica,
        &unwatched(),
    )
    .expect("walks");
    assert_eq!(walked.len(), 1);
    assert!((walked[0].origin.0 - honest_chars[0].origin.0).abs() < TOLERANCE_PT);

    // ...and REFUSES the padded one rather than placing it at the origin. The refusal is named:
    // accepting "some error" here would pass with the arity check deleted, because a walk that
    // read the first six operands and then hit `(A) Tj` with no font would refuse too.
    let outcome = glyphs_in(
        format!("BT\n{padded}\nET\n").as_bytes(),
        &Helvetica,
        &unwatched(),
    );
    match outcome {
        Err(error) => assert!(
            Refusal::OperandCountMismatch.caught(&error),
            "refused, but by a different rule: {error:?}"
        ),
        Ok(glyphs) => panic!(
            "a padded operand run placed {} glyph(s) at {:?}; PDFium drew at {:?}",
            glyphs.len(),
            glyphs.first().map(|g| g.origin),
            padded_chars[0].origin
        ),
    }
}

/// A page whose font's `/Encoding` is an **embedded CMap stream**, with the `WMode` the caller
/// asks for and an innocuous name.
///
/// # Why this fixture is a document and not a byte string
///
/// The `WMode` rule was, until this fixture, true only at unit level: `writing_mode_of` was
/// exercised on hand-written programs, and nothing established that the thing a resolver would
/// hand it is the thing the file actually holds. A CMap is a **stream** — it has a dictionary,
/// a `/Length`, a name the producer chose, and a `/UseCMap` it may point at — and none of that
/// is visible when the test writes the program inline.
///
/// So this builds the real shape: a Type 0 font whose `/Encoding` is an indirect reference to a
/// stream, wrapping a CIDFontType2 descendant. The name is `/Ordinary-H` — a spelling chosen to
/// look horizontal, since the whole point is that the name decides nothing.
fn page_with_embedded_cmap(wmode: u8) -> Vec<u8> {
    let program = format!(
        "%!PS-Adobe-3.0 Resource-CMap\n\
         /CIDInit /ProcSet findresource begin\n\
         12 dict begin\n\
         begincmap\n\
         /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> def\n\
         /CMapName /Ordinary-H def\n\
         /CMapType 1 def\n\
         /WMode {wmode} def\n\
         1 begincodespacerange\n<0000> <ffff>\nendcodespacerange\n\
         1 begincidrange\n<0000> <ffff> 0\nendcidrange\n\
         endcmap\n\
         CMapName currentdict /CMap defineresource pop\n\
         end\nend\n"
    );
    let content = "BT\n/F1 24 Tf\n100 700 Td\n<0024> Tj\nET\n";

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
    // THE `/Encoding` IS A REFERENCE TO A STREAM, which is the whole subject. A resolver has to
    // follow it, decode it and read it; the three things it can do instead — trust the name,
    // give up, or not look — are exactly what `Encoding`'s arms now make it say out loud.
    objects.push(
        "<< /Type /Font /Subtype /Type0 /BaseFont /Ordinary \
         /Encoding 6 0 R /DescendantFonts [7 0 R] >>"
            .to_owned(),
    );
    objects.push(format!(
        "<< /Type /CMap /CMapName /Ordinary-H /WMode {wmode} \
         /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> \
         /Length {} >>\nstream\n{program}endstream",
        program.len()
    ));
    objects.push(
        "<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Ordinary \
         /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> \
         /FontDescriptor 8 0 R /DW 1000 >>"
            .to_owned(),
    );
    objects.push(
        "<< /Type /FontDescriptor /FontName /Ordinary /Flags 4 \
         /FontBBox [0 -200 1000 900] /ItalicAngle 0 /Ascent 900 /Descent -200 \
         /CapHeight 700 /StemV 80 >>"
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

/// The CMap program out of a built fixture, by the same route a resolver would take.
///
/// Deliberately crude — it finds the one stream whose dictionary says `/Type /CMap` — because
/// it is **standing in for a resolver that does not exist**, not pretending to be one. What it
/// establishes is that the bytes the seam is asked about are the bytes the document holds,
/// which is the step the unit tests could not take.
fn cmap_stream_of(pdf: &[u8]) -> (Option<i64>, Vec<u8>) {
    let text = String::from_utf8_lossy(pdf).into_owned();
    let at = text
        .find("/Type /CMap")
        .expect("the fixture has a CMap stream");
    let dictionary_end = text[at..].find(">>\nstream\n").expect("the stream opens") + at;
    let wmode = text[at..dictionary_end].find("/WMode ").map(|offset| {
        text[at + offset + "/WMode ".len()..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse::<i64>()
            .expect("a /WMode this fixture wrote")
    });
    let body_at = dictionary_end + ">>\nstream\n".len();
    let body_end = text[body_at..]
        .find("endstream")
        .expect("the stream closes")
        + body_at;
    (wmode, text.as_bytes()[body_at..body_end].to_vec())
}

/// Resources as a resolver that reads the fixture's CMap would report them.
struct CidFont {
    encoding: Encoding,
}

impl Resources for CidFont {
    fn within(&self, _name: &[u8]) -> Result<Option<Box<dyn Resources + '_>>> {
        // ONE FLAT RESOURCE SET. This fake models a page whose forms declare no `/Resources`
        // of their own, so every name resolves outwards -- which is what `None` means. It is
        // stated rather than defaulted: a trait default of `Ok(None)` would let a REAL
        // resolver inherit silently, and inheriting silently is the defect `within` exists to
        // fix.
        Ok(None)
    }
    fn form(&self, _name: &[u8]) -> Result<Option<Form>> {
        Ok(None)
    }

    fn glyph(&self, _name: &[u8], _code: u32) -> Result<GlyphMetrics> {
        Ok(GlyphMetrics {
            width: 1000.0,
            bytes_per_code: 2,
            font_bbox: Some(GeometryRect {
                left: 0.0,
                bottom: -200.0,
                right: 1000.0,
                top: 900.0,
            }),
            font_matrix: Matrix::scale(0.001, 0.001),
            encoding: self.encoding.clone(),
        })
    }

    fn bytes_per_code(&self, _name: &[u8]) -> Result<u8> {
        Ok(2)
    }
}

#[test]
fn an_embedded_cmap_declaring_wmode_one_is_refused_whatever_it_is_called() {
    let pdf = page_with_embedded_cmap(1);

    // THE NON-VACUITY CONTROL, first. A fixture PDFium reads nothing from would make every
    // assertion below pass over nothing — and this one is a Type 0 font with no embedded
    // program, so it is exactly the fixture that could come back empty.
    let chars = chars_on_page(&pdf, 0);
    assert_eq!(chars.len(), 1, "the fixture must draw one glyph");

    // The bytes the seam is asked about are the bytes the DOCUMENT holds.
    let (dictionary_wmode, program) = cmap_stream_of(&pdf);
    assert_eq!(dictionary_wmode, Some(1));
    assert!(
        program.contains_str("/CMapName /Ordinary-H def"),
        "the fixture's evasion is the name, so the name has to be the innocuous one"
    );
    assert!(
        !program.contains_str("Identity-V"),
        "a fixture naming Identity-V would pass a check keyed on the name, which is the check \
         this exists to disprove"
    );

    let resources = CidFont {
        encoding: Encoding::Embedded {
            dictionary_wmode,
            program,
        },
    };
    match glyphs_in(
        b"BT /F1 24 Tf 100 700 Td <0024> Tj ET",
        &resources,
        &unwatched(),
    ) {
        Err(error) => assert!(
            Refusal::VerticalWriting.caught(&error),
            "refused, but by a different rule: {error:?}"
        ),
        Ok(glyphs) => panic!(
            "a CMap declaring 'WMode 1' under the name /Ordinary-H placed {} glyph(s)",
            glyphs.len()
        ),
    }
}

#[test]
fn the_horizontal_twin_of_that_document_is_not_refused() {
    // THE NEAR-MISS, and it differs from the fixture above in ONE DIGIT. A rule that refused
    // this would refuse an ordinary CID document, and a redaction nobody can run leaks nothing
    // only because it never runs.
    let pdf = page_with_embedded_cmap(0);
    assert_eq!(chars_on_page(&pdf, 0).len(), 1);

    let (dictionary_wmode, program) = cmap_stream_of(&pdf);
    assert_eq!(dictionary_wmode, Some(0));
    let resources = CidFont {
        encoding: Encoding::Embedded {
            dictionary_wmode,
            program,
        },
    };
    let glyphs = glyphs_in(
        b"BT /F1 24 Tf 100 700 Td <0024> Tj ET",
        &resources,
        &unwatched(),
    )
    .expect("the horizontal twin must walk");
    assert_eq!(glyphs.len(), 1);
}

#[test]
fn a_resolver_that_cannot_read_the_cmap_stream_is_refused_rather_than_assumed_horizontal() {
    // THE CASE THAT USED TO BE SILENT. Before `Encoding`, a resolver that failed to decode this
    // stream — an unknown filter, a bad `/Length`, a missing object — had nowhere to say so,
    // and the `WritingMode` it returned by default was `Horizontal`. The document is the same
    // vertical one; only the resolver's success differs.
    let pdf = page_with_embedded_cmap(1);
    assert_eq!(chars_on_page(&pdf, 0).len(), 1, "same fixture, still drawn");

    let resources = CidFont {
        encoding: Encoding::UnreadableCMap,
    };
    match glyphs_in(
        b"BT /F1 24 Tf 100 700 Td <0024> Tj ET",
        &resources,
        &unwatched(),
    ) {
        Err(error) => assert!(
            Refusal::UnreadableCMap.caught(&error),
            "refused, but by a different rule: {error:?}"
        ),
        Ok(glyphs) => panic!("an unread CMap placed {} glyph(s)", glyphs.len()),
    }
}

/// `contains` for byte slices, spelled once.
trait ContainsStr {
    fn contains_str(&self, needle: &str) -> bool;
}

impl ContainsStr for Vec<u8> {
    fn contains_str(&self, needle: &str) -> bool {
        self.windows(needle.len()).any(|w| w == needle.as_bytes())
    }
}

/// What a real producer's vertical document actually contains, read off the committed file.
///
/// # Why this reads a committed PDF rather than building one
///
/// ADR 0029's fourth condition rests on a claim about the world: that a vertical document need
/// not declare vertical writing, so the `WMode` refusal covers the CMap case and **only** that.
/// A hand-built fixture cannot establish that — it can only restate what its author believed.
/// This reads `tests/redaction/fixtures/producer-vertical-writing.pdf`, which is LibreOffice
/// 24.2.7.2's own output, licence-audited before it landed (`tests/redaction/PROVENANCE.md`).
///
/// If LibreOffice ever starts emitting a vertical CMap, this test fails and the ADR's fourth
/// condition needs rewriting — which is the point of pinning the claim to a file rather than to
/// a sentence.
#[test]
fn a_real_producers_vertical_document_declares_no_writing_mode_at_all() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/redaction/fixtures/producer-vertical-writing.pdf");
    let pdf = std::fs::read(&path).expect("the committed producer fixture");

    // THE NON-VACUITY CONTROL. A fixture PDFium reads nothing from would make every assertion
    // below pass over nothing, and this one carries a subset font, so it is exactly the kind
    // that could come back empty.
    let chars = chars_on_page(&pdf, 0);
    assert_eq!(chars.len(), 6, "the phrase is six glyphs");

    // THE FINDING. No CID font, no `Identity-V`, and no `WMode` anywhere in the file: the
    // producer laid the run out by POSITIONING a simple font, not by declaring a writing mode.
    // So there is nothing here for the `WMode` rule to catch, and nothing it should catch.
    for absent in ["/WMode", "Identity-V", "/Type0", "/CIDFont"] {
        assert!(
            !pdf.as_slice().contains_str(absent),
            "the producer emitted {absent}, so ADR 0029's fourth condition needs rewriting: \
             it says a vertical document need not declare vertical writing, and this is the \
             document that says so"
        );
    }
    assert!(
        pdf.as_slice().contains_str("/Subtype/Type1"),
        "the finding is that a SIMPLE font carries this, which is the surprising half"
    );

    // And the run really is vertical: a column, descending, whatever the font says.
    for pair in chars.windows(2) {
        let (upper, lower) = (&pair[0], &pair[1]);
        assert!(
            upper.origin.1 - lower.origin.1 > MIN_FIXTURE_DISPLACEMENT_PT,
            "the run must descend: {} then {}",
            upper.origin.1,
            lower.origin.1
        );
        assert!(
            (upper.origin.0 - lower.origin.0).abs() < TOLERANCE_PT,
            "a column, not a row: {} against {}",
            upper.origin.0,
            lower.origin.0
        );
    }
}

impl ContainsStr for &[u8] {
    fn contains_str(&self, needle: &str) -> bool {
        self.windows(needle.len()).any(|w| w == needle.as_bytes())
    }
}

/// The width every glyph in the redaction fixtures advances by, in glyph space.
///
/// # Why the fixture declares its own widths
///
/// The first version of these tests used `/Helvetica` with no `/Widths` and a stub that
/// returned 667 for every code. PDFium used the *real* Helvetica metrics — `C` and `D` are 722,
/// not 667 — so the adjustment was short by 0.66 pt per removed `C`, and the kept glyphs moved.
/// The algorithm was right and the harness was lying: a stub that invents metrics is measuring
/// its own transcription of a metric table, not the arithmetic under test.
///
/// So the fixture's font dictionary carries `/Widths`, the stub returns the same number, and the
/// two agree **by construction** rather than by my getting Adobe's table right.
const FIXTURE_WIDTH: f64 = 600.0;

/// A one-page PDF like [`page_with`], but whose font declares [`FIXTURE_WIDTH`] for every code.
fn page_with_declared_widths(body: &str) -> Vec<u8> {
    let content = format!("BT\n{body}\nET\n");
    let widths: String = (32..=126)
        .map(|_| format!("{FIXTURE_WIDTH} "))
        .collect::<Vec<_>>()
        .join("");
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
    objects.push(format!(
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding \
         /FirstChar 32 /LastChar 126 /Widths [{}] >>",
        widths.trim_end()
    ));

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

/// Resources reporting exactly the widths the fixture declares.
struct DeclaredWidths;

impl Resources for DeclaredWidths {
    fn within(&self, _name: &[u8]) -> Result<Option<Box<dyn Resources + '_>>> {
        // ONE FLAT RESOURCE SET. This fake models a page whose forms declare no `/Resources`
        // of their own, so every name resolves outwards -- which is what `None` means. It is
        // stated rather than defaulted: a trait default of `Ok(None)` would let a REAL
        // resolver inherit silently, and inheriting silently is the defect `within` exists to
        // fix.
        Ok(None)
    }
    fn form(&self, _name: &[u8]) -> Result<Option<Form>> {
        Ok(None)
    }

    fn glyph(&self, _name: &[u8], _code: u32) -> Result<GlyphMetrics> {
        Ok(GlyphMetrics {
            width: FIXTURE_WIDTH,
            bytes_per_code: 1,
            font_bbox: Some(GeometryRect {
                left: -166.0,
                bottom: -225.0,
                right: 1000.0,
                top: 931.0,
            }),
            font_matrix: Matrix::scale(0.001, 0.001),
            encoding: Encoding::Simple,
        })
    }

    fn bytes_per_code(&self, _name: &[u8]) -> Result<u8> {
        Ok(1)
    }
}

/// The glyphs PDFium reports that the **file** actually draws.
///
/// # PDFium invents a space where a redaction leaves a gap
///
/// Measured: cutting `C` out of `ABCDE` leaves `[<4142> -600 <4445>] TJ`, and
/// `FPDFText_CountChars` then reports **five** characters — `A`, `B`, a `U+0020` that is in no
/// string in the file, `D`, `E`. PDFium's text layer synthesises a space when the gap between
/// two glyphs is wide enough, which is exactly what a positioning adjustment creates.
///
/// That is worth stating beyond this test. ADR 0029 §6's read-back reads the text layer, so a
/// verifier counting characters sees a space appear where a secret was removed — not a leak,
/// since no glyph from the removed run survives, but a difference that a naive "the text is
/// shorter by one" assertion would trip over, and a shape worth knowing before §6 asserts on
/// character counts.
fn drawn_chars(pdf: &[u8]) -> Vec<OracleChar> {
    chars_on_page(pdf, 0)
}

/// Redact a glyph out of `body` and return PDFium's char origins before and after.
///
/// The redaction runs over the page's content stream and the page is rebuilt around it, so what
/// the oracle reads afterwards is a real PDF that a renderer laid out from scratch — not this
/// module's arithmetic played back.
fn origins_across_a_redaction(body: &str, cut: usize) -> (Vec<OracleChar>, Vec<OracleChar>) {
    let content = format!("BT\n{body}\nET\n");
    let before = drawn_chars(&page_with_declared_widths(body));

    let glyphs =
        glyphs_in(content.as_bytes(), &DeclaredWidths, &unwatched()).expect("the fixture walks");
    let removed = glyphs
        .get(cut..=cut)
        .expect("the fixture has a glyph at that index");
    let edited = remove_glyphs(content.as_bytes(), None, removed, &unwatched())
        .expect("the redaction applies");

    // The edited stream goes back into a page the same way the original did.
    let text = String::from_utf8(edited).expect("the rewrite is text");
    let inner = text
        .trim_start_matches("BT\n")
        .trim_end_matches('\n')
        .trim_end_matches("ET")
        .trim_end()
        .to_owned();
    (before, drawn_chars(&page_with_declared_widths(&inner)))
}

/// The characters PDFium reports that the **file** actually draws.
///
/// # PDFium invents characters, and it will say which
///
/// A positioning adjustment wide enough to look like a gap produces a `U+0020`; a `T*` line
/// move produces `U+000D U+000A`. Neither is in any string in the document, so a comparison
/// against burrow's walk must not expect them — and a wide kern the producer wrote does the
/// same thing before any redaction, so character indices and glyph indices are not the same
/// sequence even on an untouched page.
///
/// # This guessed before it asked, and the guess was wrong
///
/// The rule used to be structural: *a synthetic character carries no advance, so its origin
/// equals the next character's*. It held on every hand-built fixture in this file and failed
/// on the first real document — a LaTeX page where PDFium gave a synthetic space an origin of
/// its own, between the two glyphs it sat between. The walk found 138 glyphs and the filter
/// left 140.
///
/// `FPDFText_IsGenerated` is the renderer answering the question directly. A heuristic that
/// agrees with it on the cases you thought of is not the same thing.
fn drawn_characters(chars: &[OracleChar]) -> Vec<&OracleChar> {
    chars.iter().filter(|char| !char.generated).collect()
}

/// Every kept glyph is where it was, over the characters the file actually draws.
///
/// `cut` indexes the **drawn** sequence, which is the one `glyphs_in` produces — see
/// [`drawn_characters`] for why that is not PDFium's character sequence.
#[track_caller]
fn assert_kept_glyphs_held(before: &[OracleChar], after: &[OracleChar], cut: usize, label: &str) {
    let drawn_before = drawn_characters(before);
    let drawn_after = drawn_characters(after);
    let expected: Vec<&&OracleChar> = drawn_before
        .iter()
        .enumerate()
        .filter(|(at, _)| *at != cut)
        .map(|(_, char)| char)
        .collect();
    assert_eq!(
        drawn_after.len(),
        expected.len(),
        "{label}: {} glyph(s) drawn after the redaction against {} expected -- the counts must \
         match once PDFium's synthetic characters are removed",
        drawn_after.len(),
        expected.len()
    );

    for (want, found) in expected.into_iter().zip(&drawn_after) {
        assert_eq!(
            want.unicode, found.unicode,
            "{label}: a different glyph stayed"
        );
        let moved = (want.origin.0 - found.origin.0).hypot(want.origin.1 - found.origin.1);
        assert!(
            moved < TOLERANCE_PT,
            "{label}: removing glyph {cut} moved U+{:04X} by {moved} pt, from {:?} to {:?} -- \
             the line reflowed, which changes what the remaining text appears to say",
            want.unicode,
            want.origin,
            found.origin
        );
    }
}

/// Removing a glyph must not move the glyphs that remain.
///
/// # Why this is asserted against PDFium and not against the walk
///
/// The adjustment is computed from `Glyph::displacement`, which the walk also used to move the
/// pen. Checking the result with the walk would compare that expression against itself and pass
/// for any consistent mistake — including the one this test exists for, where the adjustment is
/// omitted entirely and both sides agree the run is simply shorter.
///
/// `FPDFText_GetCharOrigin` is a third party to that argument. Every kept glyph's origin must be
/// the same before and after, to the tolerance pre-registered in `char_box_oracle.rs`.
#[test]
fn a_redaction_does_not_move_the_glyphs_that_remain() {
    // Three cut positions, because they fail differently. A last-glyph cut needs no adjustment
    // at all, so an implementation that always appends one is wrong in a way a middle-only
    // test cannot see; a first-glyph cut puts the adjustment before any kept run.
    for (label, cut) in [("first", 0usize), ("middle", 2), ("last", 4)] {
        let (before, after) = origins_across_a_redaction("/F1 12 Tf 100 700 Td (ABCDE) Tj", cut);
        assert_eq!(before.len(), 5, "{label}: the fixture draws five glyphs");

        // THE NON-VACUITY CONTROL. If the redaction removed nothing, every origin below
        // matches trivially and the test asserts nothing at all. Stated over the glyph that
        // was cut rather than over the total, because PDFium adds a synthetic space at the gap.
        let removed = before
            .get(cut)
            .expect("the fixture has a glyph at that index");
        assert!(
            !after.iter().any(|char| char.unicode == removed.unicode
                && (char.origin.0 - removed.origin.0).abs() < TOLERANCE_PT),
            "{label}: U+{:04X} is still on the page at {:?}, so nothing was redacted and the \
             comparison below is vacuous",
            removed.unicode,
            removed.origin
        );

        assert_kept_glyphs_held(&before, &after, cut, label);
    }
}

#[test]
fn a_redaction_across_a_kern_neither_swallows_nor_doubles_it() {
    // The kern is a displacement the producer chose, between glyphs the cut does not touch.
    // Dropping it shifts everything after it left; emitting it twice shifts them right.
    //
    // `-50` rather than something larger, and the reason is the finding on `drawn_chars`: a
    // wide enough kern makes PDFium synthesise a space of its own, which put a character in
    // `before` that the walk never drew and slid every index after it by one. The first draft
    // used `-200` and compared the walk's glyph 2 against PDFium's char 2, which were different
    // glyphs. A fixture whose indices do not line up is not a smaller version of this test.
    let (before, after) = origins_across_a_redaction("/F1 12 Tf 100 700 Td [(AB) -50 (CD)] TJ", 2);
    assert_eq!(
        before.len(),
        4,
        "A, B, C, D and no synthetic space, or the indices below name the wrong glyphs"
    );
    assert_kept_glyphs_held(&before, &after, 2, "across a kern");
}

#[test]
fn a_redacted_space_puts_back_its_word_spacing_too() {
    // `Tw` rides with a single-byte space and with nothing else. An adjustment that omits it
    // shifts everything after the cut by the word spacing — small enough to read as rounding,
    // and PDFium is what tells the difference.
    let (before, after) = origins_across_a_redaction("/F1 12 Tf 6 Tw 100 700 Td (A B C) Tj", 1);
    assert_eq!(before.len(), 5, "A, space, B, space, C");
    assert_kept_glyphs_held(&before, &after, 1, "a redacted space");
}

/// A two-page document whose form object 6 is drawn by page 1 **and** by an annotation's
/// appearance stream on page 2.
///
/// # Why the annotation is the interesting second use
///
/// An appearance stream is reached through `/Annots` → `/AP` → `/N`, not through a page's
/// `/Resources`. A scan that walks page resources alone counts **one** use of the form and
/// concludes it is unshared — which is the direction that edits in place and silently removes
/// the text from the other page. The test below measures both counts and requires them to
/// differ, so the fixture demonstrates the miss rather than merely containing it.
fn document_sharing_a_form_with_an_annotation() -> Vec<u8> {
    let form = "/F1 10 Tf BT 0 0 Td (SHARED) Tj ET\n";
    let page_one = "q 1 0 0 1 100 700 cm /Fm0 Do Q\n";
    let mut objects: Vec<String> = Vec::new();
    objects.push("<< /Type /Catalog /Pages 2 0 R >>".to_owned()); // 1
    objects.push("<< /Type /Pages /Count 2 /Kids [3 0 R 4 0 R] >>".to_owned()); // 2
    objects.push(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /XObject << /Fm0 6 0 R >> >> /Contents 5 0 R >>"
            .to_owned(),
    ); // 3
    // PAGE TWO'S OWN RESOURCES DO NOT MENTION THE FORM. Only its annotation does.
    objects.push(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << >> /Contents 5 0 R /Annots [7 0 R] >>"
            .to_owned(),
    ); // 4
    objects.push(format!(
        "<< /Length {} >>\nstream\n{page_one}endstream",
        page_one.len()
    )); // 5
    objects.push(format!(
        "<< /Type /XObject /Subtype /Form /BBox [0 0 200 50] /Length {} >>\nstream\n{form}endstream",
        form.len()
    )); // 6
    objects.push(
        "<< /Type /Annot /Subtype /Widget /Rect [100 100 300 150] /F 4 \
         /AP << /N 6 0 R >> >>"
            .to_owned(),
    ); // 7

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

/// Count references to object `number` in `pdf`, optionally ignoring annotations.
///
/// Deliberately crude — a regex over `N 0 R` inside the dictionaries that can reach a form —
/// because it is **standing in for the qpdf-side resource walk**, not pretending to be one. What
/// it establishes is that the annotation really is a second reference and that a scan confined
/// to page `/Resources` really does miss it. The production walk resolves objects properly and
/// compares identity through `ObjectHandle::object()`.
fn reference_count(pdf: &[u8], number: u32, include_annotations: bool) -> usize {
    let text = String::from_utf8_lossy(pdf).into_owned();
    let needle = format!("{number} 0 R");
    text.split("obj")
        .filter(|chunk| include_annotations || !chunk.contains("/AP"))
        .map(|chunk| chunk.matches(&needle).count())
        .sum::<usize>()
        // The object's own `N 0 obj` header is not a reference to it.
        .saturating_sub(0)
}

#[test]
fn an_annotation_appearance_is_a_second_use_a_page_scan_would_miss() {
    let pdf = document_sharing_a_form_with_an_annotation();

    // NON-VACUITY FIRST: the form really is drawn, and PDFium really renders its text on
    // page one. A fixture whose form were unreachable would make both counts below trivial.
    let drawn = chars_on_page(&pdf, 0);
    assert_eq!(
        drawn.len(),
        6,
        "page one must draw the form's six glyphs, or the fixture draws nothing"
    );

    let with_annotations = reference_count(&pdf, 6, true);
    let pages_only = reference_count(&pdf, 6, false);

    assert_eq!(
        pages_only, 1,
        "a scan confined to page resources sees one use, which is the miss this fixture is for"
    );
    assert_eq!(
        with_annotations, 2,
        "counting the annotation's appearance stream sees the second use"
    );
    assert!(
        with_annotations > pages_only,
        "if these agree the fixture does not demonstrate the miss it exists to demonstrate"
    );
}

#[test]
fn the_shared_form_is_refused_and_the_letterhead_case_is_not() {
    // The two halves of the rule, on the same document shape, differing only in whether the
    // region reaches inside the shared form.
    struct Counted(usize);
    impl FormUses for Counted {
        fn uses(&self, _form: u64) -> Result<usize> {
            Ok(self.0)
        }
    }

    let form_body = "/F1 10 Tf BT 0 0 Td (SHARED) Tj ET";
    let resources = TestResources::with_form(6, form_body);
    let page = b"q 1 0 0 1 100 700 cm /Fm0 Do Q /F1 10 Tf BT 0 0 Td (BODY) Tj ET";
    let glyphs = glyphs_in(page, &resources, &unwatched()).expect("walks");

    let inside: Vec<Glyph> = glyphs
        .iter()
        .filter(|g| g.source.form.is_some())
        .cloned()
        .collect();
    let outside: Vec<Glyph> = glyphs
        .iter()
        .filter(|g| g.source.form.is_none())
        .cloned()
        .collect();
    assert!(
        !inside.is_empty() && !outside.is_empty(),
        "the fixture needs both"
    );

    // Reaching inside a form used twice: refused.
    let refusal = check_form_sharing(&inside, &Counted(2)).expect_err("must refuse");
    assert!(
        Refusal::SharedFormWouldChangeElsewhere.caught(&refusal),
        "refused by a different rule: {refusal:?}"
    );

    // The same document, the same shared form, region on the page's own text: allowed.
    check_form_sharing(&outside, &Counted(2))
        .expect("a letterhead the region avoids must not bar the redaction");

    // And the form itself, when it is drawn only once: allowed.
    check_form_sharing(&inside, &Counted(1)).expect("a form drawn once has nowhere else to reach");
}

/// Resources with one form, for the sharing tests.
struct TestResources {
    form: Form,
}

impl TestResources {
    fn with_form(id: u64, content: &str) -> Self {
        Self {
            form: Form {
                id,
                matrix: Matrix::IDENTITY,
                content: content.as_bytes().to_vec(),
            },
        }
    }
}

impl Resources for TestResources {
    fn within(&self, _name: &[u8]) -> Result<Option<Box<dyn Resources + '_>>> {
        // ONE FLAT RESOURCE SET. This fake models a page whose forms declare no `/Resources`
        // of their own, so every name resolves outwards -- which is what `None` means. It is
        // stated rather than defaulted: a trait default of `Ok(None)` would let a REAL
        // resolver inherit silently, and inheriting silently is the defect `within` exists to
        // fix.
        Ok(None)
    }
    fn form(&self, name: &[u8]) -> Result<Option<Form>> {
        Ok((name == b"Fm0").then(|| self.form.clone()))
    }

    fn glyph(&self, _name: &[u8], _code: u32) -> Result<GlyphMetrics> {
        Ok(GlyphMetrics {
            width: FIXTURE_WIDTH,
            bytes_per_code: 1,
            font_bbox: None,
            font_matrix: Matrix::scale(0.001, 0.001),
            encoding: Encoding::Simple,
        })
    }

    fn bytes_per_code(&self, _name: &[u8]) -> Result<u8> {
        Ok(1)
    }
}

#[test]
fn a_redaction_inside_a_quote_operator_keeps_the_line_move() {
    // `'` IS `T*` THEN `Tj`. Rewriting it as a bare `TJ` drops the line move, so every kept
    // glyph slides up by the leading -- one whole line, which on a paragraph re-flows the page
    // and on a form puts values against the wrong labels.
    //
    // Measured by a review before this fixture existed: +14 pt in y on every kept glyph. The
    // three fixtures above could not see it because all of them use `Tj` or `TJ` only, and
    // `'` is ordinary output from the dvips family rather than an adversarial shape.
    let (before, after) = origins_across_a_redaction("/F1 12 Tf 14 TL 100 700 Td (AB) ' (CD) '", 0);
    let drawn = drawn_characters(&before);
    assert_eq!(
        drawn.len(),
        4,
        "A, B, C, D, once PDFium's synthetic CR/LF are removed"
    );
    assert!(
        drawn
            .iter()
            .any(|char| (char.origin.1 - drawn[0].origin.1).abs() > 1.0),
        "the fixture must span two lines, or the line move is not exercised"
    );
    assert_kept_glyphs_held(&before, &after, 0, "a quote operator");
}

#[test]
fn a_redaction_inside_a_double_quote_operator_keeps_its_spacing_operands() {
    // `"` IS `aw Tw`, `ac Tc`, `T*`, THEN `Tj`. Dropping the two numeric operands changes the
    // word and character spacing for every later glyph in the stream, not only inside the
    // operator -- measured: B, C and D moved from 129.4 / 139.6 / 149.8 to 117.4 / 124.6 /
    // 131.8, as well as up a line.
    let (before, after) =
        origins_across_a_redaction("/F1 12 Tf 14 TL 100 700 Td 9 3 (A B) \" (CD) Tj", 0);
    assert!(
        drawn_characters(&before).len() >= 4,
        "the fixture must draw the run the operands apply to"
    );
    assert_kept_glyphs_held(&before, &after, 0, "a double-quote operator");
}

#[test]
fn pdfium_synthesises_a_space_once_a_kern_is_wide_enough() {
    // THE THRESHOLD, PINNED. ADR 0029 §6 asserts that a `-200` kern at 12 pt already makes
    // PDFium synthesise a space *before* any redaction — which is why character indices and
    // glyph indices are not the same sequence even on an untouched page. A review could not
    // verify it because nothing exercised `-200`; this is that measurement, committed.
    //
    // It is also the reason `a_redaction_across_a_kern_neither_swallows_nor_doubles_it` uses
    // `-50`: a fixture past the threshold has a synthetic character in `before`, and the cut
    // index would name a different glyph.
    let count = |kern: i32| {
        let body = format!("/F1 12 Tf 100 700 Td [(AB) {kern} (CD)] TJ");
        chars_on_page(&page_with_declared_widths(&body), 0)
    };

    for narrow in [-50, -100] {
        assert_eq!(
            count(narrow).len(),
            4,
            "a {narrow} kern must not synthesise anything, or the kern fixture's indices drift"
        );
    }
    for wide in [-150, -200, -500] {
        let chars = count(wide);
        assert_eq!(chars.len(), 5, "a {wide} kern synthesises one character");
        assert_eq!(
            chars[2].unicode, 0x0020,
            "and the character it synthesises is a space"
        );
        assert!(
            drawn_characters(&chars).len() == 4,
            "which `drawn_characters` removes, leaving the four glyphs the file draws"
        );
    }
}

/// The real resolver's glyph origins, against PDFium's, on the committed producer corpus.
///
/// # The first time the walk has met a real document
///
/// Everything on #131 was tested against a `Resources` fake that returned one width for every
/// code and never refused. This drives the **qpdf-backed** resolver over files four different
/// producers wrote, and compares every origin against `FPDFText_GetCharOrigin`.
///
/// It is the join the fakes could not exercise: the walk's arithmetic was checked against
/// PDFium already, and the resolver's reading of a font dictionary never was.
#[test]
fn the_real_resolver_agrees_with_pdfium_on_producer_documents() {
    for name in [
        "producer-writer.pdf",
        "producer-latex.pdf",
        "producer-vertical-writing.pdf",
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/redaction/fixtures")
            .join(name);
        let pdf = std::fs::read(&path).expect("the committed fixture");

        let walked = burrow_engines::glyphs_on_first_page(&pdf, &support::walk_options())
            .unwrap_or_else(|error| panic!("{name}: the real resolver refused: {error:?}"));
        let chars = chars_on_page(&pdf, 0);
        let drawn = drawn_characters(&chars);

        // NON-VACUITY FIRST: both sides must have found something, or every comparison below
        // passes over nothing.
        assert!(
            !walked.is_empty() && !drawn.is_empty(),
            "{name}: walk {} glyph(s), PDFium {} char(s)",
            walked.len(),
            drawn.len()
        );
        assert_eq!(
            walked.len(),
            drawn.len(),
            "{name}: the walk found {} glyph(s) and PDFium {} -- a count mismatch makes the \
             pairwise comparison below meaningless",
            walked.len(),
            drawn.len()
        );

        for (at, (glyph, char)) in walked.iter().zip(&drawn).enumerate() {
            let apart = (glyph.origin.0 - char.origin.0).hypot(glyph.origin.1 - char.origin.1);
            assert!(
                apart < TOLERANCE_PT,
                "{name}: glyph {at} is {apart} pt from where PDFium put it, {:?} against {:?} \
                 -- the resolver read this font's metrics differently from the renderer",
                glyph.origin,
                char.origin
            );
        }
    }
}

/// A redaction that emits bytes, held to PDFium on the corpus four real producers wrote.
///
/// # The join the fakes could not reach
///
/// Every step of the assembly was tested against a fake `Steps`. This drives the **qpdf** one
/// over whole documents and reads the result back with a renderer: the glyphs inside the
/// region must be gone, and every glyph outside it must be exactly where it was.
///
/// The second half is the one a fake cannot check at all. A fake cannot reflow a page.
#[test]
fn a_real_redaction_removes_the_region_and_moves_nothing_else() {
    // COUNTED, because every assertion below is inside the `Ok` arm and a refusal `continue`s.
    // A mutation that refused unconditionally left this test green over all three fixtures.
    let mut redacted_documents = 0usize;
    for name in [
        "producer-writer.pdf",
        "producer-latex.pdf",
        "producer-vertical-writing.pdf",
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/redaction/fixtures")
            .join(name);
        let pdf = std::fs::read(&path).expect("the committed fixture");

        let before = chars_on_page(&pdf, 0);
        let drawn_before = drawn_characters(&before);
        assert!(
            !drawn_before.is_empty(),
            "{name}: the fixture must draw text, or nothing below asks anything"
        );

        // THE REGION IS DERIVED FROM THE ORACLE, not guessed. Three hand-picked boxes were
        // tried first and two of them reached no glyph at all, so the test reported a pass
        // over a redaction that removed nothing -- a fixture that cannot fail, which is the
        // failure `CLAUDE.md`'s harness rule is about. This puts the region tightly around one
        // character PDFium actually found, and the assertion below is that THAT character is
        // the one that goes.
        let (_, height) = page_size(&pdf, 0);
        let target = drawn_before
            .iter()
            .max_by(|a, b| a.ink.top.total_cmp(&b.ink.top))
            .expect("at least one drawn character");
        let region = Region {
            left: target.ink.left - 1.0,
            top: height - target.ink.top - 1.0,
            width: (target.ink.right - target.ink.left) + 2.0,
            height: (target.ink.top - target.ink.bottom) + 2.0,
        };

        let redacted: std::collections::BTreeSet<usize> = [0].into_iter().collect();
        let (out, report) = match support::redact_page(&pdf, 0, redacted, region) {
            Ok(result) => result,
            Err(error) => {
                // A REFUSAL IS AN OUTCOME, not a failure -- but it must be a named one,
                // so a silent `Err` cannot pass for a redaction that did nothing.
                let text = format!("{error:?}");
                assert!(
                    text.contains('[') && text.contains(']'),
                    "{name}: refused without naming a rule: {text}"
                );
                eprintln!("  {name:<34} refused: {text}");
                continue;
            }
        };

        let after = chars_on_page(&out, 0);
        let drawn_after = drawn_characters(&after);
        eprintln!(
            "  {name:<34} {} -> {} glyph(s), {} font(s) cut, {} retained",
            drawn_before.len(),
            drawn_after.len(),
            report.fonts.iter().filter(|f| f.cut).count(),
            report.retained().count()
        );

        redacted_documents += 1;

        // THE OUTPUT IS A DOCUMENT. A redaction that emitted something unreadable would
        // satisfy "the secret is gone" trivially.
        assert!(out.starts_with(b"%PDF"), "{name}: the output must be a PDF");

        // The three properties, and they are not the same property.
        //
        // **Everything the region reaches is gone.** The leak direction, and the only one of
        // the three that is a correctness failure rather than a fidelity one.
        let mut reached = 0usize;
        for want in &drawn_before {
            if !ink_overlaps(
                &want.ink,
                region.left,
                region.top,
                region.width,
                region.height,
                height,
            ) {
                continue;
            }
            reached += 1;
            assert!(
                !drawn_after.iter().any(
                    |got| got.unicode == want.unicode && origins_close(got.origin, want.origin)
                ),
                "{name}: U+{:04X} at {:?} has ink inside the region and is still drawn",
                want.unicode,
                want.origin
            );
        }
        assert!(
            reached > 0,
            "{name}: the region reached no glyph, so every assertion here is vacuous"
        );

        // **Nothing appeared.** A new character at a new origin is a page that reflowed.
        for got in &drawn_after {
            assert!(
                drawn_before
                    .iter()
                    .any(|want| want.unicode == got.unicode
                        && origins_close(got.origin, want.origin)),
                "{name}: U+{:04X} appears at {:?} after the redaction and was not there \
                 before -- the page reflowed",
                got.unicode,
                got.origin
            );
        }

        // **Over-removal is reported, not asserted against.** The conservative box is the
        // advance box unioned with the scaled `/FontBBox`, which spans the whole em -- so a
        // region drawn tightly around one glyph's ink legitimately reaches its neighbours'
        // boxes. That is the direction that does not leak, and ADR 0029 records it as a
        // disclosure rather than a defect. Counting it here is what keeps it visible: a number
        // that grows is a question, and a silent pass is not.
        // `saturating_sub`, matching `redaction_corpus.rs`. Plain `usize` subtraction panics on
        // underflow, and PDFium synthesising a space can make `drawn_after` larger than the
        // arithmetic here assumes -- a reporting line that panics is a test failing for the
        // wrong reason.
        let extra = drawn_before
            .len()
            .saturating_sub(reached + drawn_after.len());
        eprintln!("  {name:<34} region reached {reached}, removed {extra} more");
    }

    assert_eq!(
        redacted_documents, 3,
        "all three real-producer documents must redact; a refusal that covered them would \
         leave every assertion above unexecuted and this test still green"
    );
}

/// A watch that never expires: a STOPPED clock, so the walk's checkpoints are inert.
///
/// For tests of what the walk computes. The deadline itself is tested with a clock that moves;
/// a stopped one here is deliberate and named so, because a stopped clock in a production path is
/// exactly how `max_duration_ms` stopped existing once before.
fn unwatched() -> Watch<'static> {
    static STOPPED: ManualClock = ManualClock::new(0);
    Watch::new(Deadline::start(&STOPPED, &Limits::DEFAULT), &STOPPED)
}
