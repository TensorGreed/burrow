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
use burrow_types::{Clock, Deadline, Error, Limits, ManualClock, Result, Rotation, Stage};

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
    /// What each page of the input displays at, **as written** -- so an out-of-spec value is
    /// expressible, which is the whole point of `PageRotator::rotations` recording rather than
    /// judging. `None` means every page is at 0.
    declared: Option<Vec<i64>>,
    /// Milliseconds each page of a rotation sweep costs, so a budget can be spent on one.
    sweep_ms: u64,
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
            declared: None,
            sweep_ms: 0,
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

    /// A fake whose every rotation sweep costs `sweep_ms` per page.
    fn sweeping_ms(pages: u64, sweep_ms: u64) -> Self {
        Self {
            sweep_ms,
            ..Self::with_pages(pages)
        }
    }

    /// A document whose pages display at the given values, out-of-spec ones included.
    fn displaying(declared: Vec<i64>) -> Self {
        let pages = u64::try_from(declared.len()).expect("pages fit in u64");
        Self {
            declared: Some(declared),
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

    /// What page `index` displays at, as written.
    fn declared_at(&self, source: &u64, index: u64) -> i64 {
        let _ = source;
        let at = usize::try_from(index).expect("index fits in usize");
        self.declared
            .as_ref()
            .and_then(|d| d.get(at).copied())
            .unwrap_or(0)
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

    fn effective_rotation(&self, source: &Self::Source, index: u64) -> Result<Rotation> {
        // THE JUDGING READ, so the fake refuses what a real engine refuses: an out-of-spec
        // value here is `Malformed`. `rotations` below is the recording read and does not.
        let degrees = self.declared_at(source, index);
        Rotation::from_degrees(degrees)
            .map_err(|_| Error::Malformed("/Rotate is not a multiple of 90".to_owned()))
    }

    fn rotations(
        &self,
        source: &Self::Source,
        _options: &OpenOptions<'_>,
        deadline: &Deadline,
    ) -> Result<Vec<i64>> {
        // THE SWEEP COSTS TIME, and checkpoints the deadline it was HANDED. Both halves are
        // what a real engine does, and both are what the budget test below needs: a fake whose
        // sweep is free cannot tell the operation's budget from a fresh one.
        let mut rotations = Vec::new();
        for index in 0..*source {
            deadline.checkpoint(self.clock.as_ref())?;
            self.clock.advance(self.sweep_ms);
            rotations.push(self.declared_at(source, index));
        }
        Ok(rotations)
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
        let mut emitted: Vec<i64> = (0..self.pages)
            .map(|i| self.declared_at(&self.pages, i))
            .collect();
        for &index in pages {
            let at = usize::try_from(index).expect("index fits in usize");
            if let Some(slot) = emitted.get_mut(at) {
                // THE ENGINE'S OWN EXPRESSION -- `rotation.after(effective_rotation(page))`.
                // Written as `*slot + rotation.degrees()` first, which overflows on a
                // `/Rotate` near `i64::MAX` exactly as the operation did before it normalised;
                // a fake that panics where the engine does not is a fake that cannot test the
                // fix.
                let base = Rotation::from_degrees(*slot)
                    .expect("a named page's rotation is a multiple of 90 by now");
                *slot = rotation.after(base).degrees();
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
            declared: self.declared.clone(),
            sweep_ms: self.sweep_ms,
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

// --- an out-of-spec /Rotate is recorded, not fatal, on a page nobody named ----------------

#[test]
fn a_page_displaying_out_of_spec_does_not_fail_a_rotation_of_other_pages() {
    // THE REGRESSION ADR 0022 INTRODUCED AND THIS CLOSES. The promise sweep normalised every
    // page, so a `/Rotate 45` anywhere in the document turned a rotation of page 1 into
    // `Malformed` -- an operation refused over a page it was never going to touch. The witness
    // only has to be STABLE between the read before and the read after, and 45 is stable.
    let engine = FakeRotator::displaying(vec![0, 45, 180, 0]);
    let out = run(&engine, &[1], 90).expect("a page nobody named must not fail the operation");
    assert_eq!(out, b"%PDF-1.7\n");

    // AND THE 45 IS CARRIED THROUGH, rather than dropped or rounded: the verifier compared a
    // promise containing it against an output containing it, which is what made the operation
    // succeed. If the promise had zeroed it, this would still pass -- so the emitted document
    // is asserted too.
    assert_eq!(*engine.emitted.borrow(), vec![90, 45, 180, 0]);
}

#[test]
fn turning_a_page_that_displays_out_of_spec_is_still_refused() {
    // THE OTHER HALF, and without it the change above is indistinguishable from dropping the
    // check. A turn applied to 45 is undefined -- 45 + 90 is not one of the four -- so naming
    // that page is still `Malformed`. That refusal is what `rotate`'s `# Errors` has always
    // promised; the regression was applying it to pages nobody named.
    let engine = FakeRotator::displaying(vec![0, 45, 180, 0]);
    let err = run(&engine, &[2], 90).expect_err("turning an out-of-spec page must be refused");
    assert!(matches!(err, Error::Malformed(_)), "got {err:?}");
}

#[test]
fn an_enormous_rotation_on_a_named_page_is_normalised_rather_than_overflowing() {
    // A PANIC IN LIBRARY CODE, reproduced against real qpdf by security review and pinned
    // here where it runs on every job. `/Rotate 9223372036854775710` is the largest multiple
    // of 90 an `i64` holds, and `Rotation::from_degrees` accepts ANY multiple of 90 -- so
    // `current + turn` overflowed. Under `overflow-checks` (every test build, and
    // cargo-fuzz's, which drives `rotate` directly) that panicked; in release it wrapped.
    //
    // It became reachable the moment the sweep started RECORDING the raw value instead of
    // normalising it, which is the fix for the `/Rotate 45` regression above. The two changes
    // are a pair, and this is the half that keeps the first one safe.
    let enormous = i64::MAX - (i64::MAX % 90);
    let engine = FakeRotator::displaying(vec![enormous, 0]);

    run(&engine, &[1], 90).expect("an enormous multiple of 90 is a quarter turn, not an error");

    // AND IT LANDED WHERE THE ENGINE PUTS IT: normalised, then turned. Asserted against the
    // same expression the engines use rather than a literal, so the promise and the write
    // cannot drift apart without this failing.
    let expected = Rotation::from_degrees(90)
        .expect("90 is a quarter turn")
        .after(Rotation::from_degrees(enormous).expect("a multiple of 90"))
        .degrees();
    assert_eq!(*engine.emitted.borrow(), vec![expected, 0]);
}

#[test]
fn a_rotation_that_is_not_a_multiple_of_ninety_on_a_named_page_is_malformed_not_invalid() {
    // THE ATTRIBUTION, and the privacy defect it carried. `from_degrees` reports a bad
    // *argument* -- which this is not: the number it refused is the document's own `/Rotate`,
    // and the old `InvalidArgument` message quoted it verbatim ("... got 135"), putting a
    // value derived from the file into an error string. `core/CLAUDE.md` forbids that outright.
    let engine = FakeRotator::displaying(vec![45, 0]);
    let err = run(&engine, &[1], 90).expect_err("turning an out-of-spec page must be refused");
    match err {
        Error::Malformed(why) => assert!(
            !why.contains("45") && !why.contains("135"),
            "the message carries a value derived from the file: {why}"
        ),
        other => panic!("expected Malformed, got {other:?}"),
    }
}

#[test]
fn an_enormous_rotation_on_a_page_nobody_named_is_carried_through() {
    // AND THE OTHER HALF: the value is only arithmetic on a page the request named. On any
    // other page it is a witness, and a witness is only required to be stable.
    let enormous = i64::MAX - (i64::MAX % 90);
    let engine = FakeRotator::displaying(vec![0, enormous]);

    run(&engine, &[1], 90).expect("a page nobody named must not fail the operation");
    assert_eq!(*engine.emitted.borrow(), vec![90, enormous]);
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
    let engine = FakeRotator::sweeping_ms(3, 3);
    let err = run_under(&engine, &[1], 90, Limits::with(|l| l.max_duration_ms = 10))
        .expect_err("the budget must cover both sweeps");
    assert!(
        matches!(err, Error::LimitExceeded { limit, stage, .. } if limit == "max_duration_ms"
            && stage == Stage::Deadline),
        "got {err:?}"
    );

    // THE CONTROL: the same work under a budget that fits it. Without this the assertion above
    // is satisfied by an operation that refuses every document with a clock attached.
    let engine = FakeRotator::sweeping_ms(3, 3);
    run_under(
        &engine,
        &[1],
        90,
        Limits::with(|l| l.max_duration_ms = 1_000),
    )
    .expect("a budget that covers both sweeps must not refuse");
}
