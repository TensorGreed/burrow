//! Unit tests for the rotate operation, on a fake engine.
//!
//! What happens *inside* an engine — inheritance, the per-page write, handle release — is
//! tested against real qpdf in `burrow-engines`. What is tested here is the part that belongs
//! to the operation: normalisation, one-based page numbers, the refusals, and that the engine
//! is asked for exactly what the caller asked for.

use std::cell::RefCell;
use std::sync::Arc;

use burrow_engines::{OpenOptions, PageRotator};
use burrow_types::{Clock, Error, Limits, ManualClock, Result, Rotation, Stage};

use super::{Pages, rotate};

/// What the fake engine was asked to do, so a test can assert on the request rather than only
/// on the answer. A rotation that quietly rotates the wrong page returns `Ok` either way.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct Asked {
    pages: Vec<u64>,
    degrees: i64,
}

struct FakeRotator {
    pages: u64,
    asked: RefCell<Vec<Asked>>,
    /// How far the fake moves the clock inside `rotate`.
    ///
    /// `merge` and `split`'s fakes both have this, and `rotate`'s did not -- so no test here
    /// could exercise `max_duration_ms` at all, and the rustdoc table claiming it applies went
    /// unmeasured. Found by code review.
    elapse_ms: u64,
    clock: Arc<ManualClock>,
}

impl FakeRotator {
    fn with_pages(pages: u64) -> Self {
        Self {
            pages,
            asked: RefCell::new(Vec::new()),
            elapse_ms: 0,
            clock: Arc::new(ManualClock::new(0)),
        }
    }

    fn taking_ms(pages: u64, elapse_ms: u64) -> Self {
        Self {
            elapse_ms,
            ..Self::with_pages(pages)
        }
    }

    fn options(&self, limits: Limits) -> OpenOptions<'static> {
        OpenOptions::new(limits, Arc::clone(&self.clock) as Arc<dyn Clock>)
    }
}

impl PageRotator for FakeRotator {
    type Source = u64;

    fn name(&self) -> &'static str {
        "fake"
    }

    fn open(&self, _bytes: Box<[u8]>, _options: &OpenOptions<'_>) -> Result<Self::Source> {
        Ok(self.pages)
    }

    fn pages(&self, source: &Self::Source) -> Result<u64> {
        Ok(*source)
    }

    fn effective_rotation(&self, _source: &Self::Source, _index: u64) -> Result<Rotation> {
        Ok(Rotation::None)
    }

    fn rotate(
        &self,
        _source: &Self::Source,
        pages: &[u64],
        rotation: Rotation,
        _options: &OpenOptions<'_>,
    ) -> Result<Vec<u8>> {
        self.asked.borrow_mut().push(Asked {
            pages: pages.to_vec(),
            degrees: rotation.degrees(),
        });
        self.clock.advance(self.elapse_ms);
        Ok(b"%PDF-1.7\n".to_vec())
    }
}

fn run(engine: &FakeRotator, numbers: &[u64], degrees: i64) -> Result<Vec<u8>> {
    run_under(engine, numbers, degrees, Limits::DEFAULT)
}

/// The same, under explicit ceilings, on the fake's own clock.
///
/// The clock has to be the fake's: `max_duration_ms` is only observable if the thing that
/// advances time and the thing that reads it are the same clock.
fn run_under(
    engine: &FakeRotator,
    numbers: &[u64],
    degrees: i64,
    limits: Limits,
) -> Result<Vec<u8>> {
    rotate(
        engine,
        b"%PDF-1.7\n".to_vec().into_boxed_slice(),
        Pages::numbered(numbers),
        degrees,
        &engine.options(limits),
    )
}

#[test]
fn one_based_page_numbers_reach_the_engine_zero_based() {
    // The off-by-one that would rotate the wrong page and report success. Asserted on the
    // REQUEST, because the fake returns the same bytes whatever it is asked.
    let engine = FakeRotator::with_pages(5);
    run(&engine, &[1, 3, 5], 90).unwrap();
    assert_eq!(
        engine.asked.borrow().as_slice(),
        &[Asked {
            pages: vec![0, 2, 4],
            degrees: 90
        }]
    );
}

#[test]
fn every_multiple_of_ninety_is_accepted_and_reduced() {
    let cases: [(i64, i64); 8] = [
        (0, 0),
        (90, 90),
        (180, 180),
        (270, 270),
        (360, 0),
        (-90, 270),
        (-450, 270),
        (990, 270),
    ];
    for (given, expected) in cases {
        let engine = FakeRotator::with_pages(2);
        run(&engine, &[1], given).unwrap();
        assert_eq!(
            engine.asked.borrow()[0].degrees,
            expected,
            "{given} degrees should reach the engine as {expected}"
        );
    }
}

#[test]
fn anything_that_is_not_a_multiple_of_ninety_is_refused() {
    for degrees in [1, 45, -45, 89, 91, 359] {
        let engine = FakeRotator::with_pages(2);
        assert!(
            matches!(run(&engine, &[1], degrees), Err(Error::InvalidArgument(_))),
            "{degrees} degrees should be refused"
        );
        // AND THE DOCUMENT IS NEVER OPENED. A bad argument should not cost a parse of an
        // untrusted file; the engine records nothing because it was never asked.
        assert!(engine.asked.borrow().is_empty());
    }
}

#[test]
fn page_zero_is_refused_rather_than_treated_as_the_first_page() {
    // A caller passing a zero-based index by mistake would otherwise rotate page 1 silently.
    let engine = FakeRotator::with_pages(3);
    assert!(matches!(
        run(&engine, &[0], 90),
        Err(Error::InvalidArgument(_))
    ));
}

#[test]
fn a_page_past_the_end_is_refused() {
    let engine = FakeRotator::with_pages(3);
    assert!(matches!(
        run(&engine, &[4], 90),
        Err(Error::InvalidArgument(_))
    ));
    // The boundary itself is fine.
    assert!(run(&FakeRotator::with_pages(3), &[3], 90).is_ok());
}

#[test]
fn naming_no_pages_is_refused_before_the_document_is_opened() {
    let engine = FakeRotator::with_pages(3);
    assert!(matches!(
        run(&engine, &[], 90),
        Err(Error::InvalidArgument(_))
    ));
    assert!(engine.asked.borrow().is_empty());
}

#[test]
fn a_zero_degree_rotation_still_runs() {
    // It is a legitimate request -- "set these pages to no rotation relative to now", which
    // is the identity -- and it is the case the round-trip property test needs. Refusing it
    // would make `rotate(x, 0)` special in a way nothing else here is.
    let engine = FakeRotator::with_pages(2);
    assert!(run(&engine, &[1], 0).is_ok());
    assert_eq!(engine.asked.borrow()[0].degrees, 0);
}

#[test]
fn pages_reports_what_it_was_given() {
    assert_eq!(Pages::numbered(&[1, 2, 3]).len(), 3);
    assert!(Pages::numbered(&[]).is_empty());
    // NOT deduplicated. `len` is what the caller named, and refusing a repeat is `rotate`'s
    // job -- a `len` that quietly counted distinct pages would make the ceiling check below
    // disagree with the request it is checking.
    assert_eq!(Pages::numbered(&[1, 1, 1]).len(), 3);
}

#[test]
fn the_same_page_named_twice_is_refused_here_and_not_only_in_the_engine() {
    // The fake accepts anything, so if this rule lived only in qpdf this test could not
    // exist -- which is the point. A rule in one place is a rule one engine can forget.
    let engine = FakeRotator::with_pages(4);
    assert!(matches!(
        run(&engine, &[2, 2], 90),
        Err(Error::InvalidArgument(_))
    ));
    assert!(engine.asked.borrow().is_empty());
}

#[test]
fn the_page_ceiling_is_enforced_here_on_the_document_and_on_the_selection() {
    // The document's count, over the ceiling.
    let engine = FakeRotator::with_pages(11);
    match run_under(&engine, &[1], 90, Limits::with(|l| l.max_pages = 10)) {
        Err(Error::LimitExceeded { limit, stage, .. }) => {
            assert_eq!(limit, "max_pages");
            assert_eq!(stage, Stage::PageCount);
        }
        other => panic!("expected a page-count refusal, got {other:?}"),
    }

    // And the selection, under a ceiling the document passes. Reachable only with repeats,
    // which are refused immediately after -- so the order of the two checks is what this
    // pins down, not merely that both exist.
    let engine = FakeRotator::with_pages(10);
    match run_under(
        &engine,
        &[1, 2, 3, 1],
        90,
        Limits::with(|l| l.max_pages = 3),
    ) {
        Err(Error::LimitExceeded { limit, .. }) => assert_eq!(limit, "max_pages"),
        other => panic!("expected a page-count refusal, got {other:?}"),
    }

    // The boundary is allowed.
    let engine = FakeRotator::with_pages(10);
    assert!(run_under(&engine, &[1, 2, 3], 90, Limits::with(|l| l.max_pages = 10)).is_ok());
}

#[test]
fn a_rotation_that_runs_past_the_deadline_is_refused() {
    // The clock advances inside the engine call, and the checkpoint after it catches the
    // overrun. Cooperative, as `Limits` says: the overshoot is up to one engine call, and the
    // engine is where the per-page checks live.
    let engine = FakeRotator::taking_ms(4, 2_000);
    match run_under(
        &engine,
        &[1],
        90,
        Limits::with(|l| l.max_duration_ms = 1_000),
    ) {
        Err(Error::LimitExceeded { limit, .. }) => assert_eq!(limit, "max_duration_ms"),
        other => panic!("expected a deadline refusal, got {other:?}"),
    }
}

#[test]
fn a_rotation_inside_the_deadline_is_allowed() {
    // The other half, so the test above is measuring the budget rather than the checkpoint
    // being unconditional.
    let engine = FakeRotator::taking_ms(4, 200);
    assert!(
        run_under(
            &engine,
            &[1],
            90,
            Limits::with(|l| l.max_duration_ms = 1_000)
        )
        .is_ok()
    );
}
