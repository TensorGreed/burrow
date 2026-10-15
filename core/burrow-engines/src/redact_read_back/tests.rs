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

fn visible(x: f64, y: f64) -> PlacedChar {
    PlacedChar {
        origin: (x, y),
        generated: false,
    }
}

fn generated(x: f64, y: f64) -> PlacedChar {
    PlacedChar {
        origin: (x, y),
        generated: true,
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
            marked_depth: 3,
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

// ---- geometry, never mapping ----------------------------------------------------------------

/// The PDFium text functions that return what a character SAYS, or a page's text. None may be
/// declared in this crate's shipped source (module header, "Geometry, never mapping").
const TEXT_READING_FUNCTIONS: &[&str] = &[
    "FPDFText_GetUnicode",
    "FPDFText_GetText",
    "FPDFText_GetBoundedText",
    "FPDFText_GetTextObject",
    "FPDFText_GetFontInfo",
    "FPDFText_FindStart",
    "FPDFText_GetSchResultIndex",
    "FPDFLink_LoadWebLinks",
    "FPDFLink_GetURL",
    "FPDFTextObj_GetText",
];

/// Whether `source` declares `name` as a function, outside a comment.
fn declares(source: &str, name: &str) -> bool {
    let call = format!("fn {name}(");
    source.lines().any(|line| {
        let code = line.split("//").next().unwrap_or_default();
        code.contains(&call)
    })
}

#[test]
fn the_declaration_scan_matches_its_probes() {
    // PER-RULE PROBES: each name matches its own declaration and rejects the near-miss of the
    // same name in a comment, every run, so the scan below is a measurement.
    for name in TEXT_READING_FUNCTIONS {
        let positive = format!("unsafe extern \"C\" {{\n    fn {name}(page: X) -> c_int;\n}}\n");
        let near_miss =
            format!("    // fn {name}( is never declared\n    /// `{name}` is not read\n");
        assert!(declares(&positive, name), "the scan misses {name}");
        assert!(
            !declares(&near_miss, name),
            "the scan reads {name} in a comment"
        );
    }
}

#[test]
fn no_text_reading_function_is_declared() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut stack = vec![root];
    let mut examined = Vec::new();
    let mut found = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("the source tree reads") {
            let path = entry.expect("an entry reads").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                // This file names every forbidden function in its probes, so it is the one
                // file the scan cannot read for itself.
                if path.ends_with("redact_read_back/tests.rs") {
                    continue;
                }
                let source = std::fs::read_to_string(&path).expect("a source file reads");
                for name in TEXT_READING_FUNCTIONS {
                    if declares(&source, name) {
                        found.push(format!("{} declares {name}", path.display()));
                    }
                }
                examined.push(path);
            }
        }
    }
    let ffi = examined
        .iter()
        .filter(|path| path.ends_with("pdfium/ffi.rs") || path.ends_with("pdfium/read_back.rs"))
        .count();
    eprintln!(
        "  text-reading scan: {} source file(s) examined, {} of them PDFium's declarations and \
         reading; {} forbidden name(s) checked",
        examined.len(),
        ffi,
        TEXT_READING_FUNCTIONS.len()
    );
    assert_eq!(
        ffi, 2,
        "the scan did not examine both of PDFium's declaration and reading files"
    );
    assert!(
        declares(
            &std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/pdfium/ffi.rs")
            )
            .expect("ffi.rs reads"),
            "FPDFText_GetCharOrigin"
        ),
        "the scan does not see the origin function it permits, so it would not see a forbidden one"
    );
    assert!(found.is_empty(), "{}", found.join("\n"));
}

// ---- the nesting ceiling ------------------------------------------------------------------

#[test]
fn a_page_nested_past_the_ceiling_is_refused_before_it_is_loaded() {
    let fake = Fake::honest();
    let mut expected = agreement(2);
    expected.walked.marked_depth = MAX_MARKED_DEPTH + 1;
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
    expected.walked.marked_depth = MAX_MARKED_DEPTH;
    assert_eq!(rule_of(verdict(&fake, &expected)), "Ok");
    assert_eq!(fake.asked.get(), 2);
}
