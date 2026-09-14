//! Unit tests for the rotate operation, on a fake engine.
//!
//! What happens *inside* an engine — inheritance, the per-page write, handle release — is
//! tested against real qpdf in `burrow-engines`. What is tested here is the part that belongs
//! to the operation: normalisation, one-based page numbers, the refusals, and that the engine
//! is asked for exactly what the caller asked for.

use std::cell::RefCell;
use std::rc::Rc;
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
    /// What the fake's writer put in the document, as the fake's reader will report it.
    ///
    /// Shared with every `fresh()` instance, because that is the one thing a fresh reader of
    /// the same bytes must agree about. A fake whose reader re-derived the answer from the
    /// request would agree with the operation by construction and verify nothing.
    emitted: Rc<RefCell<Vec<i64>>>,
    /// THE VERIFIER'S ONLY LEVER: what the reader reports instead of `emitted`.
    ///
    /// A shorter vector is a page lost (#61's shape); a same-length different one is a turn
    /// that landed on the wrong page. Both are documents the fake's *writer* claims to have
    /// produced and its *reader* contradicts, which is the whole situation ADR 0022 exists
    /// for, and it is unreachable through `PageRotator` alone.
    reads_back_as: Option<Vec<i64>>,
    /// How many times `OutputReader::fresh` was called, shared across every instance.
    ///
    /// ADR 0022's first requirement is that the witness is not the instance that produced the
    /// bytes, and nothing else here can see the difference: a verifier that read through
    /// `&self` would agree with every honest fake in this file.
    freshes: Rc<RefCell<usize>>,
    /// Whether this instance came from `OutputReader::fresh`.
    ///
    /// THE FRESHNESS IS ASSERTED, NOT ASSUMED. A fake that reads the emitted document back
    /// identically whoever asks leaves ADR 0022's first requirement untested: swap
    /// `engine.fresh()` for `engine` in `verify::output` and every test still passes. Found by
    /// code review, with that mutation run. So the instance that WROTE the bytes reports
    /// nonsense when asked to read them -- which is what "a corrupted instance can agree with
    /// itself" looks like from outside, and it makes the mutation fail
    /// `an_honest_engine_is_not_refused`.
    fresh: bool,
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
            emitted: Rc::new(RefCell::new(Vec::new())),
            reads_back_as: None,
            freshes: Rc::new(RefCell::new(0)),
            fresh: false,
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

    /// A fake whose writer is honest and whose reader is not.
    fn lying(pages: u64, reads_back_as: Vec<i64>) -> Self {
        Self {
            reads_back_as: Some(reads_back_as),
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
        // The document this fake would have written: every page at the rotation
        // `effective_rotation` reports above, plus the turn for the pages that were named.
        let mut emitted = vec![0_i64; usize::try_from(self.pages).expect("pages fit in usize")];
        for &index in pages {
            let at = usize::try_from(index).expect("index fits in usize");
            if let Some(slot) = emitted.get_mut(at) {
                *slot = rotation.degrees();
            }
        }
        *self.emitted.borrow_mut() = emitted;
        self.clock.advance(self.elapse_ms);
        Ok(b"%PDF-1.7\n".to_vec())
    }
}

impl burrow_engines::OutputReader for FakeRotator {
    type Read = Vec<i64>;

    fn fresh(&self) -> Self {
        *self.freshes.borrow_mut() += 1;
        // A new instance that shares the emitted document and the clock -- and shares the
        // lie, so a test can aim it at the read-back without the writer knowing.
        Self {
            pages: self.pages,
            asked: RefCell::new(Vec::new()),
            emitted: Rc::clone(&self.emitted),
            reads_back_as: self.reads_back_as.clone(),
            freshes: Rc::clone(&self.freshes),
            fresh: true,
            elapse_ms: 0,
            clock: Arc::clone(&self.clock),
        }
    }

    fn open_output(&self, _bytes: &[u8], _options: &OpenOptions<'_>) -> Result<Self::Read> {
        if !self.fresh {
            // The writing instance is not a witness. See `fresh`.
            return Ok(Vec::new());
        }
        Ok(self
            .reads_back_as
            .clone()
            .unwrap_or_else(|| self.emitted.borrow().clone()))
    }

    fn page_count(&self, read: &Self::Read) -> Result<u64> {
        Ok(u64::try_from(read.len()).expect("page count fits in u64"))
    }

    fn rotations(&self, read: &Self::Read, _options: &OpenOptions<'_>) -> Result<Vec<i64>> {
        Ok(read.clone())
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

// --- ADR 0022: the output is read back, through an engine that did not write it -----------

#[test]
fn the_output_is_read_back_through_a_fresh_engine() {
    // Not an incidental count. One `fresh` per operation is the whole of ADR 0022's first
    // requirement -- a verifier that read through `&self` would leave this at zero and every
    // other test in this file would still pass.
    let engine = FakeRotator::with_pages(3);
    run(&engine, &[2], 90).expect("rotate");
    assert_eq!(*engine.freshes.borrow(), 1);
}

#[test]
fn a_rotation_that_lands_on_the_wrong_page_is_refused() {
    // The failure both rotate modules call their worst: the right number of pages, the right
    // turn, on a page nobody named. Page 2 was asked for; the reader says page 1 moved.
    let engine = FakeRotator::lying(3, vec![90, 0, 0]);
    let err = run(&engine, &[2], 90).expect_err("a wrong page must be refused");
    assert!(matches!(err, Error::OutputRejected(_)), "got {err:?}");
    // And the writer was asked for the right thing, so the refusal is the READER's -- without
    // this the test would pass against an operation that simply asked for the wrong page.
    assert_eq!(
        engine.asked.borrow().as_slice(),
        &[Asked {
            pages: vec![1],
            degrees: 90
        }]
    );
}

#[test]
fn an_output_that_lost_a_page_is_refused() {
    // Issue #61's shape: qpdf reads five pages and writes four. Nothing in `PageRotator` can
    // see it, because the call that lost the page returned `Ok`.
    let engine = FakeRotator::lying(3, vec![0, 90]);
    let err = run(&engine, &[2], 90).expect_err("a lost page must be refused");
    assert!(matches!(err, Error::OutputRejected(_)), "got {err:?}");
}

#[test]
fn an_honest_engine_is_not_refused() {
    // The other half of the lever: the same document, the same turn, no lie. Without this the
    // three tests above are satisfied by a verifier that refuses everything.
    let engine = FakeRotator::with_pages(3);
    run(&engine, &[2], 90).expect("an honest read-back must pass");
}
