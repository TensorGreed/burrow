//! Unit tests for the reorder operation, on a fake engine.
//!
//! What happens *inside* an engine — the page-handle permutation, the flattened page tree,
//! what an inherited `/Rotate` does — is tested against real qpdf in `burrow-engines`. What is
//! tested here is the part that belongs to the operation: one-based page numbers, the
//! ceilings, and that the engine is asked for **exactly** the order the caller named.
//!
//! That last one is the whole reason the fake records its request. A reorder returns `Ok` with
//! the same page count whether it moved the right pages, the wrong pages, or none — the order
//! is the only thing that separates them, and against a fake it is only visible in the ask.

use std::cell::RefCell;
use std::sync::Arc;

use burrow_engines::{OpenOptions, PageReorderer};
use burrow_types::{Clock, Error, Limits, ManualClock, Permutation, Result, Stage};

use super::reorder;

struct FakeReorderer {
    pages: u64,
    /// The zero-based orders the engine was handed, in the order it was handed them.
    asked: RefCell<Vec<Vec<u64>>>,
    /// How far the fake moves the clock inside `reorder`, so `max_duration_ms` is reachable.
    elapse_ms: u64,
    clock: Arc<ManualClock>,
}

impl FakeReorderer {
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

impl PageReorderer for FakeReorderer {
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

    fn reorder(
        &self,
        _source: &Self::Source,
        order: &Permutation,
        _options: &OpenOptions<'_>,
    ) -> Result<Vec<u8>> {
        self.asked.borrow_mut().push(order.order().to_vec());
        self.clock.advance(self.elapse_ms);
        Ok(b"%PDF-1.7\n".to_vec())
    }
}

fn run(engine: &FakeReorderer, order: &[u64]) -> Result<Vec<u8>> {
    run_under(engine, order, Limits::DEFAULT)
}

/// The same, under explicit ceilings, on the fake's own clock.
///
/// The clock has to be the fake's: `max_duration_ms` is only observable if the thing that
/// advances time and the thing that reads it are the same clock.
fn run_under(engine: &FakeReorderer, order: &[u64], limits: Limits) -> Result<Vec<u8>> {
    reorder(
        engine,
        b"%PDF-1.7\n".to_vec().into_boxed_slice(),
        order,
        &engine.options(limits),
    )
}

#[test]
fn one_based_page_numbers_reach_the_engine_zero_based() {
    // The off-by-one that would reorder into a different document and report success.
    // Asserted on the REQUEST, because the fake returns the same bytes whatever it is asked.
    let engine = FakeReorderer::with_pages(4);
    run(&engine, &[4, 1, 3, 2]).unwrap();
    assert_eq!(engine.asked.borrow().as_slice(), &[vec![3, 0, 2, 1]]);
}

#[test]
fn the_identity_still_reaches_the_engine() {
    // Not short-circuited HERE. The ROADMAP's "the identity permutation is a no-op" is about
    // what comes out, and the engine is what decides that -- it is the layer that knows
    // whether skipping the work is cheaper than doing it (ADR 0021). An operation that
    // returned the input bytes unchanged would also skip every limit the engine applies.
    let engine = FakeReorderer::with_pages(3);
    run(&engine, &[1, 2, 3]).unwrap();
    assert_eq!(engine.asked.borrow().as_slice(), &[vec![0, 1, 2]]);
}

#[test]
fn page_zero_is_refused_rather_than_treated_as_the_first_page() {
    // A caller passing a ZERO-based list by mistake would otherwise get a silently different
    // document -- and `[0, 1, 2]` on a three-page document is a valid-looking request that
    // means "pages 1, 2 and 3" to one layer and "pages 0, 1 and 2" to the next.
    //
    // THE ORDER HERE IS `[0, 2, 3]`, AND THAT MATTERS. The obvious case, `[0, 1, 2]`, does not
    // measure this rule at all: coerce page 0 to page 1 and it becomes `[0, 0, 1]`, a
    // duplicate, which `Permutation::of` refuses for a completely different reason -- so the
    // test passed with `checked_sub` mutated to `saturating_sub`. `[0, 2, 3]` coerces to the
    // IDENTITY, which every other rule accepts, so only the zero check can refuse it.
    // Measured by code review, which planted exactly that mutation.
    let engine = FakeReorderer::with_pages(3);
    assert!(matches!(
        run(&engine, &[0, 2, 3]),
        Err(Error::InvalidArgument(_))
    ));
    // AND THE ENGINE WAS NEVER ASKED. A bad argument must not cost a reorder.
    assert!(engine.asked.borrow().is_empty());

    // The order that would be right if pages were numbered from one, kept alongside it: the
    // case above is about the ZERO, not about `[0, 2, 3]` being an odd list.
    assert!(run(&FakeReorderer::with_pages(3), &[1, 2, 3]).is_ok());
}

#[test]
fn a_page_past_the_end_is_refused() {
    let engine = FakeReorderer::with_pages(3);
    assert!(matches!(
        run(&engine, &[1, 2, 4]),
        Err(Error::InvalidArgument(_))
    ));
    assert!(engine.asked.borrow().is_empty());

    // The last page itself is fine, which is what makes the case above a boundary rather
    // than an assertion that large numbers are refused.
    assert!(run(&FakeReorderer::with_pages(3), &[1, 2, 3]).is_ok());
}

#[test]
fn a_page_named_twice_is_refused_rather_than_duplicated() {
    // THE INVARIANT, from the ROADMAP: no page lost, added or duplicated. `[1, 1, 3]` asks
    // for a three-page document containing page 1 twice and page 2 not at all -- which has
    // the right page count and is not a permutation of anything.
    let engine = FakeReorderer::with_pages(3);
    assert!(matches!(
        run(&engine, &[1, 1, 3]),
        Err(Error::InvalidArgument(_))
    ));
    assert!(engine.asked.borrow().is_empty());
}

#[test]
fn an_order_that_names_the_wrong_number_of_pages_is_refused() {
    // Both directions. A short order loses pages and a long one cannot be satisfied; neither
    // is a permutation, and a partial reorder is not a thing this operation offers.
    let engine = FakeReorderer::with_pages(4);
    assert!(matches!(
        run(&engine, &[1, 2]),
        Err(Error::InvalidArgument(_))
    ));
    assert!(matches!(
        run(&engine, &[1, 2, 3, 4, 4]),
        Err(Error::InvalidArgument(_))
    ));
    assert!(engine.asked.borrow().is_empty());
}

#[test]
fn naming_no_pages_at_all_is_refused_for_a_document_that_has_some() {
    let engine = FakeReorderer::with_pages(3);
    assert!(matches!(run(&engine, &[]), Err(Error::InvalidArgument(_))));
    assert!(engine.asked.borrow().is_empty());
}

#[test]
fn the_page_ceiling_is_enforced_here_on_the_document_and_on_the_order() {
    // BOTH, and not only in the engine. `split` checks `max_pages` itself even though its
    // engine also does, for the reason its comment records: a ceiling that lives in exactly
    // one place is a ceiling one engine can forget.
    let engine = FakeReorderer::with_pages(11);
    match run_under(&engine, &[1], Limits::with(|l| l.max_pages = 10)) {
        Err(Error::LimitExceeded { limit, stage, .. }) => {
            assert_eq!(limit, "max_pages");
            assert_eq!(stage, Stage::PageCount);
        }
        other => panic!("expected a page-count refusal, got {other:?}"),
    }
    assert!(engine.asked.borrow().is_empty());

    // The order's own length, under a ceiling the document passes. Only reachable with an
    // order longer than the document -- which `Permutation` refuses immediately after -- so
    // this pins the ORDER of the two checks, not merely that both exist.
    let engine = FakeReorderer::with_pages(3);
    match run_under(
        &engine,
        &[1, 2, 3, 4, 5, 6],
        Limits::with(|l| l.max_pages = 4),
    ) {
        Err(Error::LimitExceeded { limit, stage, .. }) => {
            assert_eq!(limit, "max_pages");
            assert_eq!(stage, Stage::PageCount);
        }
        other => panic!("expected a page-count refusal, got {other:?}"),
    }

    // The boundary is allowed, so the two cases above measure a budget rather than a check
    // that always fires.
    let engine = FakeReorderer::with_pages(10);
    let order: Vec<u64> = (1..=10).collect();
    assert!(run_under(&engine, &order, Limits::with(|l| l.max_pages = 10)).is_ok());
}

#[test]
fn a_reorder_that_runs_past_the_deadline_is_refused() {
    // The clock advances inside the engine call and the checkpoint after it catches the
    // overrun. Cooperative, as `Limits` says: the overshoot is up to one engine call, and the
    // per-page checkpoints live in the engine.
    let engine = FakeReorderer::taking_ms(4, 2_000);
    match run_under(
        &engine,
        &[4, 3, 2, 1],
        Limits::with(|l| l.max_duration_ms = 1_000),
    ) {
        Err(Error::LimitExceeded { limit, .. }) => assert_eq!(limit, "max_duration_ms"),
        other => panic!("expected a deadline refusal, got {other:?}"),
    }
}

#[test]
fn a_reorder_inside_the_deadline_is_allowed() {
    // The other half, so the test above is measuring the budget rather than a checkpoint that
    // is unconditional.
    let engine = FakeReorderer::taking_ms(4, 200);
    assert!(
        run_under(
            &engine,
            &[4, 3, 2, 1],
            Limits::with(|l| l.max_duration_ms = 1_000)
        )
        .is_ok()
    );
}

#[test]
fn a_zero_page_document_takes_only_an_empty_order() {
    // qpdf refuses to open one, so this is unreachable through the real engine -- which is
    // exactly why it is here. The operation must not be the layer that panics or divides by
    // the page count if an engine ever reports zero.
    let engine = FakeReorderer::with_pages(0);
    assert!(run(&engine, &[]).is_ok());
    assert!(matches!(run(&engine, &[1]), Err(Error::InvalidArgument(_))));
}
