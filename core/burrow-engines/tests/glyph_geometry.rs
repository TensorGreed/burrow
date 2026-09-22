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
    Encoding, Form, GlyphMetrics, Matrix, Rect as GeometryRect, Refusal, Resources, glyphs_in,
};
use burrow_types::Result;
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
    let walked = glyphs_in(format!("BT\n{body}\nET\n").as_bytes(), &Helvetica)
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
    let walked = glyphs_in(format!("BT\n{honest}\nET\n").as_bytes(), &Helvetica).expect("walks");
    assert_eq!(walked.len(), 1);
    assert!((walked[0].origin.0 - honest_chars[0].origin.0).abs() < TOLERANCE_PT);

    // ...and REFUSES the padded one rather than placing it at the origin. The refusal is named:
    // accepting "some error" here would pass with the arity check deleted, because a walk that
    // read the first six operands and then hit `(A) Tj` with no font would refuse too.
    let outcome = glyphs_in(format!("BT\n{padded}\nET\n").as_bytes(), &Helvetica);
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
    match glyphs_in(b"BT /F1 24 Tf 100 700 Td <0024> Tj ET", &resources) {
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
    let glyphs = glyphs_in(b"BT /F1 24 Tf 100 700 Td <0024> Tj ET", &resources)
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
    match glyphs_in(b"BT /F1 24 Tf 100 700 Td <0024> Tj ET", &resources) {
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
