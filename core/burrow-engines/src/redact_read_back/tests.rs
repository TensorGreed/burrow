//! The second reading's rules against a reading that says what each test tells it to.
//!
//! A FAKE THAT LIES ON THE WAY BACK (CLAUDE.md, definition of done): every rule is driven by a
//! reading that disagrees with burrow in exactly one way, and must refuse naming that rule; and by
//! a near-miss that differs from it by as little as the rule allows, and must pass. The native
//! wiring -- that `redact_page` asks PDFium at all, and refuses on what it says -- is in
//! `qpdf::tests::second_reading`.

use super::*;
use crate::pdfsyntax::region::PageFrame;
use crate::redact_verify::Walked;
use std::cell::Cell;

/// A reading of a two-page document whose redacted page is page 0.
struct Fake {
    input_pages: Option<u64>,
    reading: Option<PageReading>,
    fail_input: Option<fn() -> Error>,
    fail_output: Option<fn() -> Error>,
    asked: Cell<usize>,
}

impl Fake {
    fn honest() -> Self {
        Self {
            input_pages: Some(2),
            reading: Some(PageReading {
                pages: 2,
                page: 0,
                size: (612.0, 792.0),
                chars: vec![visible(72.0, 700.0), visible(85.0, 700.0)],
            }),
            fail_input: None,
            fail_output: None,
            asked: Cell::new(0),
        }
    }

    fn with_chars(chars: Vec<PlacedChar>) -> Self {
        let mut fake = Self::honest();
        if let Some(reading) = fake.reading.as_mut() {
            reading.chars = chars;
        }
        fake
    }

    fn reading_mut(&mut self) -> &mut PageReading {
        self.reading
            .as_mut()
            .expect("the honest reading has a page")
    }
}

impl SecondReading for Fake {
    fn page_count(&self, _bytes: &[u8]) -> Result<Option<u64>> {
        self.asked.set(self.asked.get() + 1);
        match &self.fail_input {
            Some(error) => Err(error()),
            None => Ok(self.input_pages),
        }
    }

    fn read_page(&self, _bytes: &[u8], _page: usize) -> Result<Option<PageReading>> {
        self.asked.set(self.asked.get() + 1);
        match &self.fail_output {
            Some(error) => Err(error()),
            None => Ok(self.reading.clone()),
        }
    }
}

/// A character at `(x, y)` whose box is the point itself, so only its origin can place it.
fn visible(x: f64, y: f64) -> PlacedChar {
    boxed((x, y), (x, y, x, y))
}

fn generated(x: f64, y: f64) -> PlacedChar {
    PlacedChar {
        generated: true,
        ..visible(x, y)
    }
}

/// A character at `origin` whose box is `(left, bottom, right, top)`.
fn boxed(origin: (f64, f64), (left, bottom, right, top): (f64, f64, f64, f64)) -> PlacedChar {
    PlacedChar {
        origin,
        bounds: Rect {
            left,
            bottom,
            right,
            top,
        },
        generated: false,
    }
}

fn letter() -> PageFrame {
    PageFrame {
        display_box: Rect {
            left: 0.0,
            bottom: 0.0,
            right: 612.0,
            top: 792.0,
        },
        rotate: 0,
        user_unit: 1.0,
    }
}

/// The region `[100, 200] x [592, 692]` in content space on an unrotated letter page.
fn agreement(glyphs: usize) -> Agreement {
    Agreement {
        page: 0,
        region: Region {
            left: 100.0,
            top: 100.0,
            width: 100.0,
            height: 100.0,
        },
        walked: Walked {
            frame: letter(),
            glyphs,
            marked_work: 3,
        },
        promised_pages: 2,
    }
}

fn verdict(fake: &Fake, expected: &Agreement) -> Result<()> {
    agrees(fake, b"input", b"output", expected)
}

/// The rule id in brackets that a rejection names, or a description of what came back instead.
fn rule_of(result: Result<()>) -> String {
    match result {
        Ok(()) => "Ok".to_owned(),
        Err(Error::OutputRejected(message)) => message
            .split('[')
            .nth(1)
            .and_then(|rest| rest.split(']').next())
            .map_or_else(|| format!("no rule in {message:?}"), str::to_owned),
        Err(other) => format!("not a rejection: {other:?}"),
    }
}

#[test]
fn an_honest_reading_passes_and_both_documents_were_read() {
    let fake = Fake::honest();
    assert_eq!(rule_of(verdict(&fake, &agreement(2))), "Ok");
    assert_eq!(
        fake.asked.get(),
        2,
        "the input's count and the output's page"
    );
}

// ---- R1 -----------------------------------------------------------------------------------

#[test]
fn r1_refuses_an_origin_the_renderer_places_inside_the_region() {
    let fake = Fake::with_chars(vec![visible(72.0, 700.0), visible(150.0, 640.0)]);
    assert_eq!(
        rule_of(verdict(&fake, &agreement(2))),
        "second-reading-glyph-in-region"
    );
}

#[test]
fn r1_twin_a_generated_character_inside_the_region_passes() {
    // PDFium invents a space in the gap a redaction cuts (ADR 0029 §6).
    let fake = Fake::with_chars(vec![visible(72.0, 700.0), generated(150.0, 640.0)]);
    assert_eq!(rule_of(verdict(&fake, &agreement(2))), "Ok");
}

// The region below is `[100, 200] x [592, 692]`; shrunk by the tolerance, `[100.05, 199.95] x
// [592.05, 691.95]`. Every box here has its ORIGIN OUTSIDE the region, so the origin alone would
// pass every one of them: what refuses is the box.

#[test]
fn r1_refuses_a_box_that_straddles_the_regions_edge_from_an_origin_outside_it() {
    // THE FIXTURE (owner, 2026-10-19): a kept character whose origin is left of the region and
    // whose box crosses the left edge. Counted, though it is not wholly inside.
    let straddling = boxed((90.0, 640.0), (90.0, 635.0, 101.0, 650.0));
    assert!(
        straddling.origin.0 < 100.0 && straddling.bounds.left < 100.0,
        "the fixture must begin outside the region, or it tests the origin rule"
    );
    let fake = Fake::with_chars(vec![visible(72.0, 700.0), straddling]);
    assert_eq!(
        rule_of(verdict(&fake, &agreement(2))),
        "second-reading-glyph-in-region"
    );
}

#[test]
fn r1_twin_a_box_wholly_outside_that_touches_the_edge_passes() {
    // THE TWIN: the same character, its box ending exactly on the region's left edge. Touching is
    // not crossing; nor is reaching within the tolerance of it. Just past the tolerance is.
    let at = |right: f64| {
        Fake::with_chars(vec![
            visible(72.0, 700.0),
            boxed((90.0, 640.0), (90.0, 635.0, right, 650.0)),
        ])
    };
    assert_eq!(rule_of(verdict(&at(100.0), &agreement(2))), "Ok");
    assert_eq!(
        rule_of(verdict(&at(100.0 + TOLERANCE_PT * 0.8), &agreement(2))),
        "Ok",
        "a box within the tolerance of the edge is agreement"
    );
    assert_eq!(
        rule_of(verdict(&at(100.0 + TOLERANCE_PT * 1.2), &agreement(2))),
        "second-reading-glyph-in-region"
    );
}

#[test]
fn r1_refuses_a_box_across_each_edge() {
    // One per edge, so that no single comparison can be dropped without a test failing; and the
    // twin of each, the same box moved to touch that edge from outside, passes.
    let cases = [
        (
            "left",
            (90.0, 640.0),
            (90.0, 635.0, 101.0, 650.0),
            (90.0, 635.0, 100.0, 650.0),
        ),
        (
            "right",
            (210.0, 640.0),
            (199.0, 635.0, 210.0, 650.0),
            (200.0, 635.0, 210.0, 650.0),
        ),
        (
            "bottom",
            (150.0, 580.0),
            (145.0, 580.0, 155.0, 593.0),
            (145.0, 580.0, 155.0, 592.0),
        ),
        (
            "top",
            (150.0, 700.0),
            (145.0, 691.0, 155.0, 705.0),
            (145.0, 692.0, 155.0, 705.0),
        ),
    ];
    for (edge, origin, across, touching) in cases {
        let fake = Fake::with_chars(vec![visible(72.0, 700.0), boxed(origin, across)]);
        assert_eq!(
            rule_of(verdict(&fake, &agreement(2))),
            "second-reading-glyph-in-region",
            "a box across the {edge} edge"
        );
        let twin = Fake::with_chars(vec![visible(72.0, 700.0), boxed(origin, touching)]);
        assert_eq!(
            rule_of(verdict(&twin, &agreement(2))),
            "Ok",
            "a box touching the {edge} edge from outside"
        );
    }
}

#[test]
fn r1_refuses_a_box_larger_than_the_region_around_it() {
    // Neither inside nor straddling one edge: a box that covers the whole region from an origin
    // outside it. Crossing, on every edge at once.
    let fake = Fake::with_chars(vec![
        visible(72.0, 700.0),
        boxed((50.0, 560.0), (50.0, 560.0, 250.0, 720.0)),
    ]);
    assert_eq!(
        rule_of(verdict(&fake, &agreement(2))),
        "second-reading-glyph-in-region"
    );
}

#[test]
fn r1_reads_a_box_reported_with_its_corners_swapped() {
    let fake = Fake::with_chars(vec![
        visible(72.0, 700.0),
        boxed((90.0, 640.0), (101.0, 650.0, 90.0, 635.0)),
    ]);
    assert_eq!(
        rule_of(verdict(&fake, &agreement(2))),
        "second-reading-glyph-in-region"
    );
}

#[test]
fn a_box_that_is_not_a_number_is_no_answer() {
    for at in 0..4 {
        let mut b = [90.0, 635.0, 95.0, 650.0];
        b[at] = f64::NAN;
        let fake = Fake::with_chars(vec![
            visible(72.0, 700.0),
            boxed((90.0, 640.0), (b[0], b[1], b[2], b[3])),
        ]);
        assert_eq!(
            rule_of(verdict(&fake, &agreement(2))),
            "second-reading-unavailable",
            "a NaN in the box's coordinate {at}"
        );
    }
    let mut twin = boxed((90.0, 640.0), (f64::NAN, 635.0, 95.0, 650.0));
    twin.generated = true;
    let fake = Fake::with_chars(vec![visible(72.0, 700.0), twin]);
    assert_eq!(rule_of(verdict(&fake, &agreement(2))), "Ok");
}

#[test]
fn r1_holds_the_tolerance_on_both_sides_of_the_edge() {
    // The region's left edge is x = 100; it is shrunk by TOLERANCE_PT before the test.
    let just_inside_tolerance = Fake::with_chars(vec![
        visible(72.0, 700.0),
        visible(100.0 + TOLERANCE_PT * 0.8, 640.0),
    ]);
    assert_eq!(
        rule_of(verdict(&just_inside_tolerance, &agreement(2))),
        "Ok",
        "an origin within the tolerance of the edge is agreement, not a glyph in the region"
    );
    let past_tolerance = Fake::with_chars(vec![
        visible(72.0, 700.0),
        visible(100.0 + TOLERANCE_PT * 1.2, 640.0),
    ]);
    assert_eq!(
        rule_of(verdict(&past_tolerance, &agreement(2))),
        "second-reading-glyph-in-region"
    );
}

#[test]
fn r1_converts_the_region_with_the_outputs_frame_rotation_included() {
    // Rotated 90: the region's displayed top-left corner is the content box's bottom-left, so
    // the same display region lands somewhere else in content space. An origin inside the
    // unrotated conversion and outside the rotated one must pass on the rotated page.
    let mut expected = agreement(2);
    expected.walked.frame.rotate = 90;
    let mut fake = Fake::with_chars(vec![visible(72.0, 700.0), visible(150.0, 640.0)]);
    fake.reading_mut().size = (792.0, 612.0);
    let converted = expected
        .region
        .to_content_space(&expected.walked.frame)
        .expect("the region converts");
    let inside_rotated = (converted.left + converted.right) / 2.0;
    let inside_rotated_y = (converted.bottom + converted.top) / 2.0;
    assert!(
        !(150.0 >= converted.left
            && 150.0 <= converted.right
            && 640.0 >= converted.bottom
            && 640.0 <= converted.top),
        "the fixture point must be outside the rotated conversion for this to mean anything"
    );
    assert_eq!(rule_of(verdict(&fake, &expected)), "Ok");
    fake.reading_mut()
        .chars
        .push(visible(inside_rotated, inside_rotated_y));
    assert_eq!(
        rule_of(verdict(&fake, &expected)),
        "second-reading-glyph-in-region"
    );
}

#[test]
fn a_region_too_small_to_compare_in_refuses() {
    // 0.1 pt is twice the tolerance: nothing is left once it comes off each side.
    let fake = Fake::honest();
    let mut expected = agreement(2);
    expected.region.width = 2.0 * TOLERANCE_PT;
    assert_eq!(
        rule_of(verdict(&fake, &expected)),
        "second-reading-region-too-small"
    );
    let mut expected = agreement(2);
    expected.region.height = 2.0 * TOLERANCE_PT;
    assert_eq!(
        rule_of(verdict(&fake, &expected)),
        "second-reading-region-too-small"
    );
    // THE TWIN: just wider than that leaves a sliver, and an honest reading passes.
    let mut expected = agreement(2);
    expected.region.width = 2.0 * TOLERANCE_PT + 0.01;
    expected.region.height = 2.0 * TOLERANCE_PT + 0.01;
    assert_eq!(rule_of(verdict(&fake, &expected)), "Ok");
}

#[test]
fn an_origin_that_is_not_a_number_is_no_answer() {
    // Every comparison with NaN is false, so R1 would read it as outside the region.
    let fake = Fake::with_chars(vec![visible(72.0, 700.0), visible(f64::NAN, 640.0)]);
    assert_eq!(
        rule_of(verdict(&fake, &agreement(2))),
        "second-reading-unavailable"
    );
    // A NaN on a generated character is not read, and so is not a refusal.
    let twin = Fake::with_chars(vec![visible(72.0, 700.0), generated(f64::NAN, 640.0)]);
    assert_eq!(rule_of(verdict(&twin, &agreement(2))), "Ok");
}

#[test]
fn a_page_past_the_character_ceiling_is_no_answer() {
    let fake = Fake::with_chars(vec![visible(72.0, 700.0); MAX_CHARS + 1]);
    assert_eq!(
        rule_of(verdict(&fake, &agreement(2))),
        "second-reading-unavailable"
    );
    let twin = Fake::with_chars(vec![visible(72.0, 700.0); MAX_CHARS]);
    assert_eq!(rule_of(verdict(&twin, &agreement(2))), "Ok");
}

// ---- R2 -----------------------------------------------------------------------------------

#[test]
fn r2_refuses_a_reading_that_sees_nothing_where_burrow_places_glyphs() {
    let fake = Fake::with_chars(vec![]);
    assert_eq!(
        rule_of(verdict(&fake, &agreement(2))),
        "second-reading-no-text"
    );
    // Generated characters alone are not a reading of the file.
    let only_generated = Fake::with_chars(vec![generated(72.0, 700.0)]);
    assert_eq!(
        rule_of(verdict(&only_generated, &agreement(2))),
        "second-reading-no-text"
    );
}

#[test]
fn r2_twin_an_empty_reading_of_a_page_burrow_also_reads_as_empty_passes() {
    let fake = Fake::with_chars(vec![]);
    assert_eq!(rule_of(verdict(&fake, &agreement(0))), "Ok");
}

// ---- R3 -----------------------------------------------------------------------------------

#[test]
fn r3_refuses_a_page_the_renderer_shows_at_another_size() {
    let mut fake = Fake::honest();
    fake.reading_mut().size = (612.0, 792.5);
    assert_eq!(
        rule_of(verdict(&fake, &agreement(2))),
        "second-reading-page-size"
    );
}

#[test]
fn r3_twin_float_noise_in_the_size_passes() {
    let mut fake = Fake::honest();
    fake.reading_mut().size = (612.005, 791.995);
    assert_eq!(rule_of(verdict(&fake, &agreement(2))), "Ok");
}

#[test]
fn r3_compares_the_rotated_size_on_a_rotated_page() {
    let mut expected = agreement(2);
    expected.walked.frame.rotate = 270;
    let mut fake = Fake::honest();
    fake.reading_mut().size = (792.0, 612.0);
    assert_eq!(rule_of(verdict(&fake, &expected)), "Ok");
    fake.reading_mut().size = (612.0, 792.0);
    assert_eq!(
        rule_of(verdict(&fake, &expected)),
        "second-reading-page-size"
    );
}

// ---- R4 -----------------------------------------------------------------------------------

#[test]
fn r4_refuses_an_output_with_another_page_count_than_was_promised() {
    let mut fake = Fake::honest();
    fake.reading_mut().pages = 1;
    assert_eq!(
        rule_of(verdict(&fake, &agreement(2))),
        "second-reading-page-count"
    );
    fake.reading_mut().pages = 3;
    assert_eq!(
        rule_of(verdict(&fake, &agreement(2))),
        "second-reading-page-count"
    );
}

// ---- R5 -----------------------------------------------------------------------------------

#[test]
fn r5_refuses_an_input_the_renderer_counts_differently_from_qpdf() {
    let mut fake = Fake::honest();
    fake.input_pages = Some(3);
    assert_eq!(
        rule_of(verdict(&fake, &agreement(2))),
        "second-reading-input-page-count"
    );
    assert_eq!(
        fake.asked.get(),
        1,
        "R5 is decided before the output is read"
    );
}

#[test]
fn r4_and_r5_twin_counts_that_agree_pass() {
    // The near-miss for both: the same counts the promise holds, from both documents.
    let fake = Fake::honest();
    assert_eq!(rule_of(verdict(&fake, &agreement(2))), "Ok");
    let mut one_page = agreement(2);
    one_page.promised_pages = 1;
    let mut fake = Fake::honest();
    fake.input_pages = Some(1);
    fake.reading_mut().pages = 1;
    assert_eq!(rule_of(verdict(&fake, &one_page)), "Ok");
}

// ---- the reading itself ---------------------------------------------------------------------

#[test]
fn a_reading_of_another_page_is_refused() {
    let mut fake = Fake::honest();
    fake.reading_mut().page = 1;
    assert_eq!(
        rule_of(verdict(&fake, &agreement(2))),
        "second-reading-wrong-page"
    );
}

#[test]
fn no_answer_is_a_refusal_not_a_clean_reading() {
    let mut fake = Fake::honest();
    fake.input_pages = None;
    assert_eq!(
        rule_of(verdict(&fake, &agreement(2))),
        "second-reading-unavailable"
    );
    let mut fake = Fake::honest();
    fake.reading = None;
    assert_eq!(
        rule_of(verdict(&fake, &agreement(2))),
        "second-reading-unavailable"
    );
}

#[test]
fn an_engine_error_is_a_refusal_and_a_limit_passes_through_as_itself() {
    let mut fake = Fake::honest();
    fake.fail_output = Some(|| Error::Malformed("planted".to_owned()));
    assert_eq!(
        rule_of(verdict(&fake, &agreement(2))),
        "second-reading-unavailable"
    );
    let mut fake = Fake::honest();
    fake.fail_input = Some(|| Error::Unsupported("planted".to_owned()));
    assert_eq!(
        rule_of(verdict(&fake, &agreement(2))),
        "second-reading-unavailable"
    );
    let mut fake = Fake::honest();
    fake.fail_output = Some(|| Error::LimitExceeded {
        limit: "max_duration_ms",
        stage: burrow_types::Stage::Deadline,
        requested: 61_000,
        allowed: 60_000,
    });
    assert!(
        matches!(
            verdict(&fake, &agreement(2)),
            Err(Error::LimitExceeded {
                limit: "max_duration_ms",
                ..
            })
        ),
        "running out of time is the operation's outcome, not a verdict on the page"
    );
}

#[test]
fn a_rejection_names_no_coordinate() {
    // What reaches an error message is security posture: counts and page numbers, never a place.
    let fake = Fake::with_chars(vec![visible(72.0, 700.0), visible(150.25, 640.75)]);
    let Err(Error::OutputRejected(message)) = verdict(&fake, &agreement(2)) else {
        panic!("R1 should have refused");
    };
    for coordinate in ["150", "640", "100", "692"] {
        assert!(
            !message.contains(coordinate),
            "the rejection carries {coordinate}: {message}"
        );
    }
}

// ---- positions, not text ------------------------------------------------------------------

/// The PDFium text functions this crate's source may name: where a character is, whether PDFium
/// invented it, the box PDFium gives it, and the calls that open and close the page's text.
/// Nothing else.
const PERMITTED: [&str; 6] = [
    "FPDFText_ClosePage",
    "FPDFText_CountChars",
    "FPDFText_GetCharBox",
    "FPDFText_GetCharOrigin",
    "FPDFText_IsGenerated",
    "FPDFText_LoadPage",
];

/// Names the scan must REJECT, one probe each: what a character says, a page's text, and the ink
/// loose box R1 does not read. Not the rule -- the rule is the equality with
/// [`PERMITTED`], which rejects every other name too -- but the shapes the scan is shown to see.
const REJECTED: &[&str] = &[
    "FPDFText_GetUnicode",
    "FPDFText_GetText",
    "FPDFText_GetBoundedText",
    "FPDFText_GetTextObject",
    "FPDFText_GetFontInfo",
    "FPDFText_FindStart",
    "FPDFText_GetSchResultIndex",
    "FPDFText_GetLooseCharBox",
    "FPDFLink_LoadWebLinks",
    "FPDFLink_GetURL",
    "FPDFTextObj_GetText",
];

/// Every `FPDFText_*`, `FPDFTextObj_*` and `FPDFLink_*` name in `source`'s CODE -- comments
/// stripped, string literals kept, so a `#[link_name = "..."]` attribute binding one of them to an
/// innocent Rust name is counted as the name it binds.
///
/// A scan, not a parse, with two misses stated: a line is cut at its first `//`, so a name after a
/// string literal containing `//` on the same line is not seen; and a name assembled at compile
/// time (`concat!`) is not seen. Neither is reachable by accident.
fn named(source: &str) -> std::collections::BTreeSet<String> {
    let mut found = std::collections::BTreeSet::new();
    for line in source.lines() {
        let code = line.split("//").next().unwrap_or_default();
        for prefix in ["FPDFText_", "FPDFTextObj_", "FPDFLink_"] {
            let mut rest = code;
            while let Some(at) = rest.find(prefix) {
                let tail = &rest[at..];
                let end = tail
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(tail.len());
                // `FPDFTextObj_` also begins with `FPDFText`, but not with `FPDFText_`, so the
                // two prefixes never report the same name twice.
                found.insert(tail[..end].to_owned());
                rest = &tail[end..];
            }
        }
    }
    found
}

#[test]
fn the_name_scan_matches_its_probes() {
    // PER-NAME PROBES, every run: each rejected name is seen as a declaration, as a call, and
    // behind a `link_name` that hides it under another Rust name; and is NOT seen in a comment.
    for name in REJECTED {
        let declared = format!("unsafe extern \"C\" {{\n    fn {name}(page: X) -> c_int;\n}}\n");
        let called = format!("    let n = unsafe {{ {name}(text, 0) }};\n");
        let aliased = format!(
            "unsafe extern \"C\" {{\n    #[link_name = \"{name}\"]\n    fn harmless(page: X);\n}}\n"
        );
        let commented =
            format!("    // fn {name}( is never declared\n    /// `{name}` is not read\n");
        for (shape, source) in [
            ("declared", &declared),
            ("called", &called),
            ("aliased", &aliased),
        ] {
            assert!(
                named(source).contains(*name),
                "the scan misses {name} {shape}"
            );
        }
        assert!(
            named(&commented).is_empty(),
            "the scan reads {name} in a comment"
        );
    }
    // THE `link_name` NEAR-MISS: an attribute binding a PDFium function OUTSIDE the three
    // families is not a text function, and the scan must not count it as one.
    let near_miss =
        "unsafe extern \"C\" {\n    #[link_name = \"FPDF_LoadPage\"]\n    fn load(d: X);\n}\n";
    assert!(
        named(near_miss).is_empty(),
        "the scan counts FPDF_LoadPage as a text function"
    );
    // AND THE PREFIX BOUNDARY: `FPDFTextObj_` is its own family, reported once.
    assert_eq!(
        named("fn FPDFTextObj_GetText(o: X);"),
        std::collections::BTreeSet::from(["FPDFTextObj_GetText".to_owned()])
    );
}

#[test]
fn the_pdfium_text_functions_named_are_the_six_permitted() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut stack = vec![root];
    let mut examined = 0_usize;
    let mut found: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("the source tree reads") {
            let path = entry.expect("an entry reads").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                // This file names every rejected function in its probes, so it is the one file
                // the scan cannot read for itself.
                if path.ends_with("redact_read_back/tests.rs") {
                    continue;
                }
                let source = std::fs::read_to_string(&path).expect("a source file reads");
                for name in named(&source) {
                    found
                        .entry(name)
                        .or_default()
                        .push(path.display().to_string());
                }
                examined += 1;
            }
        }
    }
    let names: Vec<&str> = found.keys().map(String::as_str).collect();
    eprintln!(
        "  pdfium text-function scan: {examined} source file(s) examined; named {} of the {} \
         permitted: {names:?}",
        names.len(),
        PERMITTED.len()
    );
    assert!(examined > 0, "the scan examined no source file");
    // EQUALITY, not absence: every permitted name must be SEEN (so a scan that saw nothing fails),
    // and nothing else may be.
    assert_eq!(
        names,
        PERMITTED.to_vec(),
        "the PDFium text functions named in this crate's source, with where: {found:?}"
    );
}

// ---- the nesting ceiling ------------------------------------------------------------------

#[test]
fn a_page_nested_past_the_ceiling_is_refused_before_it_is_loaded() {
    let fake = Fake::honest();
    let mut expected = agreement(2);
    expected.walked.marked_work = MAX_MARKED_WORK + 1;
    assert_eq!(rule_of(verdict(&fake, &expected)), "second-reading-nesting");
    assert_eq!(
        fake.asked.get(),
        1,
        "only the input's page count was read: the page was never asked for"
    );
}

#[test]
fn a_page_nested_to_the_ceiling_is_read() {
    let fake = Fake::honest();
    let mut expected = agreement(2);
    expected.walked.marked_work = MAX_MARKED_WORK;
    assert_eq!(rule_of(verdict(&fake, &expected)), "Ok");
    assert_eq!(fake.asked.get(), 2);
}
