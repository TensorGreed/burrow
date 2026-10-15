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
/// invented it, and the calls that open and close the page's text. Nothing else.
const PERMITTED: [&str; 5] = [
    "FPDFText_ClosePage",
    "FPDFText_CountChars",
    "FPDFText_GetCharOrigin",
    "FPDFText_IsGenerated",
    "FPDFText_LoadPage",
];

/// Names the scan must REJECT, one probe each: what a character says, a page's text, and the ink
/// box R1 does not read until #206's ink half. Not the rule -- the rule is the equality with
/// [`PERMITTED`], which rejects every other name too -- but the shapes the scan is shown to see.
const REJECTED: &[&str] = &[
    "FPDFText_GetUnicode",
    "FPDFText_GetText",
    "FPDFText_GetBoundedText",
    "FPDFText_GetTextObject",
    "FPDFText_GetFontInfo",
    "FPDFText_FindStart",
    "FPDFText_GetSchResultIndex",
    "FPDFText_GetCharBox",
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
fn the_pdfium_text_functions_named_are_the_five_permitted() {
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

// ---- the recording (#206's web half) -------------------------------------------------------

mod recording {
    use super::*;
    use crate::redact_read_back::recorded::{
        Answer, Failure, MAX_ANSWER_LEN, Recorded, agrees_recorded, decode, decode_answer, digest,
        encode, encode_answer, record,
    };

    const INPUT: &[u8] = b"the input";
    const OUTPUT: &[u8] = b"the output";

    fn honest_recording() -> Recorded {
        record(&Fake::honest(), INPUT, OUTPUT, 0).expect("the honest fake answers")
    }

    #[test]
    fn a_recording_round_trips_exactly() {
        let recorded = honest_recording();
        assert_eq!(recorded.input_digest, digest(INPUT));
        assert_eq!(recorded.output_digest, digest(OUTPUT));
        let bytes = encode(&recorded).expect("encodes");
        assert_eq!(decode(&bytes).expect("decodes"), recorded);
    }

    #[test]
    fn an_honest_recording_agrees_as_the_live_reading_does() {
        assert_eq!(
            rule_of(agrees_recorded(
                honest_recording(),
                INPUT,
                OUTPUT,
                &agreement(2)
            )),
            "Ok"
        );
        // AND THE REPLAY JUDGES BY THE SAME RULES: a recording of a page PDFium reads no
        // character on is refused as R2 refuses it live.
        let empty = record(&Fake::with_chars(vec![]), INPUT, OUTPUT, 0).expect("recorded");
        assert_eq!(
            rule_of(agrees_recorded(empty, INPUT, OUTPUT, &agreement(2))),
            "second-reading-no-text"
        );
    }

    #[test]
    fn a_recording_of_other_bytes_is_refused_by_name() {
        // The near-miss is the honest case above. Each of these swaps one document.
        for (what, input, output) in [
            ("another output", INPUT, &b"another output"[..]),
            ("another input", &b"another input"[..], OUTPUT),
            ("input and output swapped", OUTPUT, INPUT),
        ] {
            assert_eq!(
                rule_of(agrees_recorded(
                    honest_recording(),
                    input,
                    output,
                    &agreement(2)
                )),
                "second-reading-other-document",
                "{what}"
            );
        }
    }

    #[test]
    fn a_failure_crosses_as_a_type_and_is_rebuilt_as_wrapped_rebuilds_it() {
        // DESIGN REVIEW L1. A ceiling passes through as itself, an internal fault as itself, and
        // anything else is no answer -- a refusal by name, never "malformed".
        let limit = || Error::LimitExceeded {
            limit: "max_duration_ms",
            stage: burrow_types::Stage::Deadline,
            requested: 1_200,
            allowed: 1_000,
        };
        let cases = [
            (limit(), "not a rejection: LimitExceeded"),
            (Error::Internal("x".to_owned()), "not a rejection: Internal"),
            (
                Error::Malformed("the page".to_owned()),
                "second-reading-unavailable",
            ),
            (
                Error::Unsupported("too many".to_owned()),
                "second-reading-unavailable",
            ),
        ];
        for (error, expected) in cases {
            let failure = Failure::of(&error);
            let bytes = encode_answer(&Answer::Failed(failure)).expect("encodes");
            let Answer::Failed(back) = decode_answer(&bytes).expect("decodes") else {
                panic!("a failure decoded as a recording");
            };
            assert_eq!(back, failure);
            let rebuilt = rule_of(Err(back.into_error()));
            assert!(rebuilt.starts_with(expected), "{error:?}: {rebuilt}");
        }
        // AND NO TEXT: the rebuilt ceiling carries the numbers, and the error text is dropped.
        assert!(matches!(
            Failure::of(&limit()).into_error(),
            Error::LimitExceeded {
                limit: "max_duration_ms",
                stage: burrow_types::Stage::Deadline,
                requested: 1_200,
                allowed: 1_000,
            }
        ));
        assert!(
            !format!("{:?}", Failure::of(&Error::Malformed("SECRET".to_owned())))
                .contains("SECRET")
        );
    }

    #[test]
    fn an_answer_past_its_ceiling_or_of_the_wrong_shape_does_not_decode() {
        assert!(
            decode_answer(&vec![1_u8; MAX_ANSWER_LEN + 1]).is_err(),
            "past the ceiling"
        );
        let good = encode_answer(&Answer::Failed(Failure::NoAnswer)).expect("encodes");
        let mut kind = good.clone();
        kind[1] = 9;
        let mut stage = encode_answer(&Answer::Failed(Failure::of(&Error::LimitExceeded {
            limit: "max_pages",
            stage: burrow_types::Stage::PageCount,
            requested: 2,
            allowed: 1,
        })))
        .expect("encodes");
        stage[3] = 99;
        let mut padded = good.clone();
        padded[4] = 1;
        for (what, bytes) in [
            ("a no-answer carrying a number", padded),
            ("a kind past the table", kind),
            ("a stage past the table", stage),
            ("one byte short", good[..good.len() - 1].to_vec()),
            ("one byte long", [good.as_slice(), &[0]].concat()),
        ] {
            assert!(
                matches!(decode_answer(&bytes), Err(Error::Malformed(_))),
                "{what}"
            );
        }
        // AND A RECORDING STILL DECODES THROUGH THE SAME DOOR.
        let recorded = honest_recording();
        assert_eq!(
            decode_answer(&encode(&recorded).expect("encodes")).expect("decodes"),
            Answer::Read(recorded)
        );
    }

    #[test]
    fn the_nesting_ceiling_is_asked_before_any_reading_is_requested() {
        let mut deep = agreement(2);
        deep.walked.marked_work = MAX_MARKED_WORK + 1;
        assert_eq!(rule_of(may_load(&deep)), "second-reading-nesting");
        let mut at = agreement(2);
        at.walked.marked_work = MAX_MARKED_WORK;
        assert_eq!(rule_of(may_load(&at)), "Ok");
    }

    #[test]
    fn a_malformed_recording_does_not_decode() {
        let good = encode(&honest_recording()).expect("encodes");
        let mut cases: Vec<(&str, Vec<u8>)> = vec![
            ("empty", Vec::new()),
            ("one byte short", good[..good.len() - 1].to_vec()),
            ("one byte long", [good.as_slice(), &[0]].concat()),
        ];
        let mut version = good.clone();
        version[0] = 2;
        cases.push(("another version", version));
        let mut flag = good.clone();
        let last = flag.len() - 1;
        flag[last] = 2;
        cases.push(("a generated flag of 2", flag));
        let mut count = good.clone();
        // The count field sits just before the characters.
        let at = 1 + 32 + 32 + 8 * 5;
        count[at..at + 4]
            .copy_from_slice(&u32::try_from(MAX_CHARS + 1).expect("fits").to_le_bytes());
        cases.push(("a count past the ceiling", count));
        for (what, bytes) in cases {
            assert!(
                matches!(decode(&bytes), Err(Error::Malformed(_))),
                "{what} decoded"
            );
        }
    }
}
