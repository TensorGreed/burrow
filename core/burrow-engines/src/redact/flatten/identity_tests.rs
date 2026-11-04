//! ADR 0031 *Fixtures*, §5 `[page-identity-mismatch]` and "frames as floats", against fake engines.
//!
//! Each test names the mutation that turns it red. The real-engine fixtures are in
//! `pdfium/flatten_identity_tests.rs`.

#![allow(
    clippy::unwrap_used,
    clippy::panic,
    clippy::cast_possible_truncation,
    reason = "tests"
)]

use std::cell::Cell;

use burrow_types::{Clock, Deadline, Error, Limits, ManualClock, Result};

use super::{
    Disagreement, PdfiumPages, check_pages, check_rotation, is_page_identity_mismatch,
    quarter_turns_to_degrees, survey,
};
use crate::redact::frames::{FrameRule, PageFrameReading, PageFrames};

/// qpdf's reading of pages with these display sizes.
fn frames(sizes: &[(f64, f64)]) -> PageFrames {
    PageFrames {
        pages: sizes.len() as u64,
        readings: sizes
            .iter()
            .map(|&(width, height)| PageFrameReading::Frame { width, height })
            .collect(),
    }
}

/// A fake PDFium: a count and a size per index, and a clock its every read may advance.
struct Fake<'c> {
    count: u64,
    sizes: Vec<(f32, f32)>,
    clock: &'c ManualClock,
    tick_ms: u64,
    reads: Cell<u64>,
}

impl<'c> Fake<'c> {
    /// PDFium agreeing with qpdf: every size rounded to 32 bits.
    fn echo(sizes: &[(f64, f64)], clock: &'c ManualClock) -> Self {
        Self {
            count: sizes.len() as u64,
            sizes: sizes.iter().map(|&(w, h)| (w as f32, h as f32)).collect(),
            clock,
            tick_ms: 0,
            reads: Cell::new(0),
        }
    }
}

impl PdfiumPages for Fake<'_> {
    fn page_count(&self) -> Result<u64> {
        Ok(self.count)
    }

    fn page_size(&self, index: u64) -> Result<(f32, f32)> {
        self.reads.set(self.reads.get() + 1);
        self.clock.advance(self.tick_ms);
        let at = usize::try_from(index).unwrap();
        self.sizes
            .get(at)
            .copied()
            .ok_or_else(|| Error::InvalidArgument(format!("page {index} past the end")))
    }
}

fn run(frames: &PageFrames, fake: &Fake<'_>, limits: &Limits) -> Result<super::Survey> {
    let deadline = Deadline::start(fake.clock as &dyn Clock, limits);
    check_pages(frames, fake, &deadline, fake.clock)
}

fn surveyed(frames: &PageFrames, fake: &Fake<'_>) -> super::Survey {
    let deadline = Deadline::start(fake.clock as &dyn Clock, &Limits::DEFAULT);
    survey(frames, fake, &deadline, fake.clock).unwrap()
}

const LETTER: (f64, f64) = (612.0, 792.0);
const A4: (f64, f64) = (595.275_590_551_181, 841.889_763_779_528);

/// The twin every fixture below differs from: agreeing engines pass, and every page was read.
#[test]
fn agreeing_engines_pass_and_every_page_is_examined() {
    let clock = ManualClock::new(0);
    let sizes = [LETTER, A4, (100.0, 200.0)];
    let fake = Fake::echo(&sizes, &clock);
    let survey = run(&frames(&sizes), &fake, &Limits::DEFAULT).unwrap();
    assert_eq!(survey.examined, 3);
    assert!(survey.disagreements.is_empty());
    assert_eq!(fake.reads.get(), 3, "every page's no-load size was read");
}

/// KILLS: MI1, the count comparison deleted.
#[test]
fn a_count_disagreement_refuses_both_ways() {
    let clock = ManualClock::new(0);
    for (qpdf, pdfium) in [(2_usize, 3_u64), (3, 2)] {
        let sizes = vec![LETTER; qpdf];
        let mut fake = Fake::echo(&[LETTER; 3], &clock);
        fake.count = pdfium;
        let got = run(&frames(&sizes), &fake, &Limits::DEFAULT);
        assert!(
            got.as_ref().is_err_and(is_page_identity_mismatch),
            "qpdf {qpdf} vs pdfium {pdfium}: {got:?}"
        );
        let survey = surveyed(&frames(&sizes), &fake);
        assert_eq!(
            survey.disagreements,
            vec![Disagreement::Count {
                qpdf: qpdf as u64,
                pdfium
            }]
        );
        assert_eq!(survey.examined, 2, "the pages both engines have");
    }
}

/// KILLS: MI2 (only page N's size compared, here any single index), MI9 (width only).
#[test]
fn one_page_s_size_disagreeing_refuses_wherever_it_is_and_on_either_side() {
    let clock = ManualClock::new(0);
    let sizes = [LETTER, LETTER, LETTER, LETTER];
    for page in 0..sizes.len() {
        for side in 0..2 {
            let mut fake = Fake::echo(&sizes, &clock);
            let entry = &mut fake.sizes[page];
            if side == 0 {
                entry.0 = 611.0;
            } else {
                entry.1 = 791.0;
            }
            let got = run(&frames(&sizes), &fake, &Limits::DEFAULT);
            assert!(
                got.as_ref().is_err_and(is_page_identity_mismatch),
                "page {page} side {side}: {got:?}"
            );
            let survey = surveyed(&frames(&sizes), &fake);
            assert_eq!(survey.disagreements.len(), 1);
            assert!(matches!(
                survey.disagreements[0],
                Disagreement::Size { page: p, .. } if p == page as u64
            ));
        }
    }
}

/// ADR row "a quarter-turn page whose sizes agree only unswapped": qpdf's display frame for a
/// `/Rotate 90` page of 200 x 100 is (100, 200); a PDFium reporting (200, 100) is not the same
/// page shown the same way.
#[test]
fn a_quarter_turn_page_whose_sizes_agree_only_unswapped_refuses() {
    let clock = ManualClock::new(0);
    let display = [(100.0, 200.0)];
    let mut fake = Fake::echo(&display, &clock);
    fake.sizes[0] = (200.0, 100.0);
    assert!(
        run(&frames(&display), &fake, &Limits::DEFAULT)
            .is_err_and(|e| is_page_identity_mismatch(&e))
    );
}

/// ADR row "frames as floats". 612.1 is not representable: qpdf reads it as the nearest `f64`,
/// PDFium as the nearest `f32`, and the two agree only after qpdf's is rounded to 32 bits.
///
/// KILLS: MI3, the comparison made without rounding (`f64::from(pdfium) == qpdf`).
#[test]
fn a_value_whose_64_and_32_bit_readings_differ_agrees_after_rounding() {
    let clock = ManualClock::new(0);
    let sizes = [(612.1, 792.0)];
    assert_ne!(
        f64::from(612.1_f32),
        612.1,
        "the fixture needs the two widths to differ"
    );
    let fake = Fake::echo(&sizes, &clock);
    assert!(run(&frames(&sizes), &fake, &Limits::DEFAULT).is_ok());
    // THE NEAR-MISS: one ulp away in 32 bits is a disagreement.
    let mut nudged = Fake::echo(&sizes, &clock);
    nudged.sizes[0].0 = f32::from_bits(612.1_f32.to_bits() + 1);
    assert!(run(&frames(&sizes), &nudged, &Limits::DEFAULT).is_err());
}

/// A page qpdf could not frame has nothing to compare with, so it fails closed.
///
/// KILLS: MI7, `Refused` readings skipped.
#[test]
fn a_page_qpdf_could_not_frame_refuses() {
    let clock = ManualClock::new(0);
    let sizes = [LETTER, LETTER];
    let mut read = frames(&sizes);
    read.readings[1] = PageFrameReading::Refused {
        rule: FrameRule::PageAttributeInherited,
        error: Error::Unsupported("pdf redaction [page-attribute-inherited]: …".to_owned()),
    };
    let fake = Fake::echo(&sizes, &clock);
    assert!(run(&read, &fake, &Limits::DEFAULT).is_err_and(|e| is_page_identity_mismatch(&e)));
    assert_eq!(
        surveyed(&read, &fake).disagreements,
        vec![Disagreement::Unframed { page: 1 }]
    );
}

/// The deadline is checked between pages: a read that spends it stops the pass at the next page.
///
/// KILLS: MI6, the per-page checkpoint deleted.
#[test]
fn a_pass_that_spends_the_deadline_stops_before_the_next_page() {
    let clock = ManualClock::new(0);
    let sizes = vec![LETTER; 10];
    let mut limits = Limits::DEFAULT;
    limits.max_duration_ms = 100;
    let mut fake = Fake::echo(&sizes, &clock);
    fake.tick_ms = 60;
    let got = run(&frames(&sizes), &fake, &limits);
    assert!(matches!(got, Err(Error::LimitExceeded { .. })), "{got:?}");
    assert_eq!(fake.reads.get(), 2, "the third read never began");
    // THE TWIN: the same pass with time to spare reads every page.
    let unhurried = ManualClock::new(0);
    let fake = Fake::echo(&sizes, &unhurried);
    assert!(run(&frames(&sizes), &fake, &limits).is_ok());
    assert_eq!(fake.reads.get(), 10);
}

/// A reading list shorter than the count is a bug, never an agreement.
#[test]
fn fewer_readings_than_the_count_is_internal() {
    let clock = ManualClock::new(0);
    let mut read = frames(&[LETTER, LETTER]);
    read.readings.pop();
    let fake = Fake::echo(&[LETTER, LETTER], &clock);
    assert!(matches!(
        run(&read, &fake, &Limits::DEFAULT),
        Err(Error::Internal(_))
    ));
}

/// KILLS: MI8 (a quarter turn mapped to the wrong degrees); the check itself deleted.
#[test]
fn rotation_maps_quarter_turns_and_compares_exactly() {
    assert_eq!(quarter_turns_to_degrees(0).unwrap(), 0);
    assert_eq!(quarter_turns_to_degrees(1).unwrap(), 90);
    assert_eq!(quarter_turns_to_degrees(2).unwrap(), 180);
    assert_eq!(quarter_turns_to_degrees(3).unwrap(), 270);
    for bad in [-1, 4, i32::MIN, i32::MAX] {
        assert!(matches!(
            quarter_turns_to_degrees(bad),
            Err(Error::Internal(_))
        ));
    }
    for qpdf in [0_u16, 90, 180, 270] {
        for pdfium in [0_u16, 90, 180, 270] {
            let got = check_rotation(qpdf, pdfium);
            if qpdf == pdfium {
                assert!(got.is_ok());
            } else {
                assert!(
                    got.is_err_and(|e| is_page_identity_mismatch(&e)),
                    "{qpdf} {pdfium}"
                );
            }
        }
    }
}

/// The refusal is recognised by its message and by nothing near it.
#[test]
fn the_refusal_is_recognised_exactly() {
    assert!(is_page_identity_mismatch(&super::mismatch()));
    assert!(!is_page_identity_mismatch(&Error::Malformed(
        super::PAGE_IDENTITY_MISMATCH.to_owned()
    )));
    assert!(!is_page_identity_mismatch(&Error::Unsupported(format!(
        "{} ",
        super::PAGE_IDENTITY_MISMATCH
    ))));
}

proptest::proptest! {
    /// An engine that reads every frame as qpdf does, rounded to 32 bits, never disagrees; one
    /// page nudged by one ulp gives exactly one disagreement, at that page.
    #[test]
    fn agreement_is_exact_and_one_ulp_is_one_disagreement(
        sizes in proptest::collection::vec((0.01_f64..14_400.0, 0.01_f64..14_400.0), 1..24),
        pick in 0_usize..24,
        height in proptest::bool::ANY,
    ) {
        let clock = ManualClock::new(0);
        let fake = Fake::echo(&sizes, &clock);
        let read = frames(&sizes);
        let survey = surveyed(&read, &fake);
        proptest::prop_assert!(survey.disagreements.is_empty());
        proptest::prop_assert_eq!(survey.examined, sizes.len() as u64);

        let page = pick % sizes.len();
        let mut nudged = Fake::echo(&sizes, &clock);
        let side = if height { &mut nudged.sizes[page].1 } else { &mut nudged.sizes[page].0 };
        *side = f32::from_bits(side.to_bits() + 1);
        let survey = surveyed(&read, &nudged);
        proptest::prop_assert_eq!(survey.disagreements.len(), 1);
        let at_page = matches!(survey.disagreements[0], Disagreement::Size { page: p, .. } if p == page as u64);
        proptest::prop_assert!(at_page);
    }

    /// A count off by `k` either way is one disagreement, and the pages both have are compared.
    #[test]
    fn a_count_off_by_k_is_one_disagreement_over_the_common_pages(
        pages in 1_usize..16,
        k in 1_u64..8,
        more in proptest::bool::ANY,
    ) {
        let clock = ManualClock::new(0);
        let sizes = vec![LETTER; pages];
        let mut fake = Fake::echo(&vec![LETTER; pages + 8], &clock);
        let pdfium = if more { pages as u64 + k } else { (pages as u64).saturating_sub(k) };
        fake.count = pdfium;
        let survey = surveyed(&frames(&sizes), &fake);
        proptest::prop_assert_eq!(survey.examined, (pages as u64).min(pdfium));
        proptest::prop_assert_eq!(
            survey.disagreements,
            vec![Disagreement::Count { qpdf: pages as u64, pdfium }]
        );
    }
}
