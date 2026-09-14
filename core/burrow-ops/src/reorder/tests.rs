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
use std::rc::Rc;
use std::sync::Arc;

use burrow_engines::{OpenOptions, PageReorderer};
use burrow_types::{Clock, Deadline, Error, Limits, ManualClock, Permutation, Result, Stage};

use super::reorder;

struct FakeReorderer {
    pages: u64,
    /// The zero-based orders the engine was handed, in the order it was handed them.
    asked: RefCell<Vec<Vec<u64>>>,
    /// How far the fake moves the clock inside `reorder`, so `max_duration_ms` is reachable.
    elapse_ms: u64,
    clock: Arc<ManualClock>,
    /// What the fake's writer put in the document, as the fake's reader will report it.
    ///
    /// Shared with every `fresh()` instance. A reader that re-derived this from the request
    /// would agree with the operation by construction and verify nothing.
    emitted: Rc<RefCell<Vec<i64>>>,
    /// THE VERIFIER'S ONLY LEVER: what the reader reports instead of `emitted`.
    ///
    /// A shorter vector is a page lost (#61's shape); a same-length different one is a
    /// permutation that is not the one asked for. Neither is reachable through
    /// `PageReorderer` alone, because its writer returns `Ok` either way.
    reads_back_as: Option<Vec<i64>>,
    /// How many times `OutputReader::fresh` was called, shared across every instance.
    ///
    /// ADR 0022's first requirement is that the witness is not the instance that produced the
    /// bytes, and nothing else here can see the difference.
    freshes: Rc<RefCell<usize>>,
    /// Every page displays the same way, which is the document the witness cannot see into.
    uniform: bool,
    /// Milliseconds each page of a rotation sweep costs, so a budget can be spent on one.
    sweep_ms: u64,
    /// Whether this instance came from `OutputReader::fresh`.
    ///
    /// THE FRESHNESS IS ASSERTED, NOT ASSUMED. A fake that reads the emitted document back
    /// identically whoever asks leaves ADR 0022's first requirement untested: swap
    /// `engine.fresh()` for `engine` in `verify::output` and every test still passes. Found by
    /// code review, with that mutation run. So the instance that WROTE the bytes reports
    /// nonsense when asked to read them.
    fresh: bool,
}

impl FakeReorderer {
    fn with_pages(pages: u64) -> Self {
        Self {
            pages,
            asked: RefCell::new(Vec::new()),
            elapse_ms: 0,
            clock: Arc::new(ManualClock::new(0)),
            emitted: Rc::new(RefCell::new(Vec::new())),
            reads_back_as: None,
            freshes: Rc::new(RefCell::new(0)),
            uniform: false,
            sweep_ms: 0,
            fresh: false,
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

    /// A fake whose every rotation sweep costs `sweep_ms` per page.
    fn sweeping_ms(pages: u64, sweep_ms: u64) -> Self {
        Self {
            sweep_ms,
            ..Self::with_pages(pages)
        }
    }

    /// A document whose every page displays at 0, and a reader that lies about the order.
    fn uniform_but_lying(pages: u64, reads_back_as: Vec<i64>) -> Self {
        Self {
            uniform: true,
            ..Self::lying(pages, reads_back_as)
        }
    }

    /// What each page displays at, with no clock and no deadline.
    ///
    /// The sweep meters this; the writer consults it directly. Separate so the fake does not
    /// charge itself twice, which would make the budget test measure the fake.
    ///
    /// ONE DISTINCT VALUE PER PAGE, not a flat vector. A fake whose pages all read back the
    /// same would make the verifier's order check vacuous -- `add-operation` §2c's mistake,
    /// inside the thing that exists to catch it. The values are multiples of 90 because that
    /// is what a rotation is, and distinct so an order is observable.
    fn declared(&self, source: &u64) -> Vec<i64> {
        if self.uniform {
            return vec![0; usize::try_from(*source).expect("pages fit in usize")];
        }
        (0..*source).map(|n| ((n % 4) * 90).cast_signed()).collect()
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

    fn rotations(
        &self,
        source: &Self::Source,
        options: &OpenOptions<'_>,
        deadline: &Deadline,
    ) -> Result<Vec<i64>> {
        // THE SWEEP COSTS TIME, and checkpoints the deadline it was HANDED. Both halves are
        // what a real engine does, and both are what the test below needs: a fake whose sweep
        // is free cannot tell the operation's budget from a fresh one.
        for _ in 0..*source {
            deadline.checkpoint(self.clock.as_ref())?;
            self.clock.advance(self.sweep_ms);
        }
        let _ = options;
        Ok(self.declared(source))
    }

    fn reorder(
        &self,
        source: &Self::Source,
        order: &Permutation,
        options: &OpenOptions<'_>,
    ) -> Result<Vec<u8>> {
        self.asked.borrow_mut().push(order.order().to_vec());
        // The document this fake would have written: the input's own rotations, moved by the
        // permutation it was actually handed. Honest by construction -- the lie, when a test
        // wants one, lives on the reader.
        let _ = options;
        let before = self.declared(source);
        let mut emitted = Vec::with_capacity(before.len());
        for &from in order.order() {
            let at = usize::try_from(from).expect("index fits in usize");
            emitted.push(*before.get(at).expect("a permutation names pages it has"));
        }
        *self.emitted.borrow_mut() = emitted;
        self.clock.advance(self.elapse_ms);
        Ok(b"%PDF-1.7\n".to_vec())
    }
}

impl burrow_engines::OutputReader for FakeReorderer {
    type Read = Vec<i64>;

    fn fresh(&self) -> Self {
        *self.freshes.borrow_mut() += 1;
        Self {
            pages: self.pages,
            asked: RefCell::new(Vec::new()),
            elapse_ms: 0,
            clock: Arc::clone(&self.clock),
            emitted: Rc::clone(&self.emitted),
            reads_back_as: self.reads_back_as.clone(),
            freshes: Rc::clone(&self.freshes),
            uniform: self.uniform,
            sweep_ms: self.sweep_ms,
            fresh: true,
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

    fn rotations(
        &self,
        read: &Self::Read,
        _options: &OpenOptions<'_>,
        deadline: &Deadline,
    ) -> Result<Vec<i64>> {
        for _ in 0..read.len() {
            deadline.checkpoint(self.clock.as_ref())?;
            self.clock.advance(self.sweep_ms);
        }
        Ok(read.clone())
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

// --- ADR 0022: the output is read back, through an engine that did not write it -----------

#[test]
fn the_output_is_read_back_through_a_fresh_engine() {
    let engine = FakeReorderer::with_pages(4);
    run(&engine, &[4, 1, 3, 2]).expect("reorder");
    assert_eq!(*engine.freshes.borrow(), 1);
}

#[test]
fn a_permutation_that_is_not_the_one_asked_for_is_refused() {
    // The fake's pages display at 0, 90, 180, 270, so an order IS observable -- which is the
    // whole reason `PageReorderer::rotations` returns distinct values per page. The reader
    // reports the input unchanged while the caller asked for a reversal.
    let engine = FakeReorderer::lying(4, vec![0, 90, 180, 270]);
    let err = run(&engine, &[4, 3, 2, 1]).expect_err("a wrong permutation must be refused");
    assert!(matches!(err, Error::OutputRejected(_)), "got {err:?}");
    // The engine was asked for the right permutation, so the refusal came from the read-back.
    assert_eq!(engine.asked.borrow().as_slice(), &[vec![3, 2, 1, 0]]);
}

#[test]
fn an_output_that_lost_a_page_is_refused() {
    // Issue #61 again, on the operation whose invariant is "no page is lost".
    let engine = FakeReorderer::lying(4, vec![270, 180, 90]);
    let err = run(&engine, &[4, 3, 2, 1]).expect_err("a lost page must be refused");
    assert!(matches!(err, Error::OutputRejected(_)), "got {err:?}");
}

#[test]
fn an_honest_engine_is_not_refused() {
    let engine = FakeReorderer::with_pages(4);
    run(&engine, &[4, 3, 2, 1]).expect("an honest read-back must pass");
}

#[test]
fn a_reorder_of_pages_that_all_display_the_same_way_is_only_a_page_count() {
    // WHAT THIS CHECK CANNOT DO, asserted rather than only written down. Four pages that all
    // display at 0 make every permutation indistinguishable, so a reader reporting the
    // identity is accepted even though a reversal was asked for. `verify`'s header says this;
    // a test is what keeps the claim honest when the witness changes.
    let engine = FakeReorderer::uniform_but_lying(4, vec![0, 0, 0, 0]);
    run(&engine, &[4, 3, 2, 1]).expect("indistinguishable, and therefore accepted");
}

#[test]
fn one_budget_covers_the_promise_sweep_and_the_read_back() {
    // THE INVARIANT `verify` STATES IN CAPITALS, asserted. Producing the output and checking it
    // spend ONE `max_duration_ms`; a sweep that started its own `Deadline` would hand the
    // operation a second full budget, because `Deadline::start` resets the origin AND the
    // budget. Both sweeps did exactly that until security review measured 56 ms returned
    // against a 50 ms ceiling.
    //
    // The fake's sweep costs 3 ms per page over 3 pages, so the promise sweep spends 9 of a
    // 10 ms budget and the read-back's sweep runs out. Under per-sweep deadlines every phase
    // starts fresh and nothing is ever refused.
    let engine = FakeReorderer::sweeping_ms(3, 3);
    let err = run_under(
        &engine,
        &[3, 2, 1],
        Limits::with(|l| l.max_duration_ms = 10),
    )
    .expect_err("the budget must cover both sweeps");
    assert!(
        matches!(err, Error::LimitExceeded { limit, stage, .. } if limit == "max_duration_ms"
            && stage == Stage::Deadline),
        "got {err:?}"
    );

    // THE CONTROL: the same work under a budget that fits it. Without this the assertion above
    // is satisfied by an operation that refuses every document with a clock attached.
    let engine = FakeReorderer::sweeping_ms(3, 3);
    run_under(
        &engine,
        &[3, 2, 1],
        Limits::with(|l| l.max_duration_ms = 1_000),
    )
    .expect("a budget that covers both sweeps must not refuse");
}
