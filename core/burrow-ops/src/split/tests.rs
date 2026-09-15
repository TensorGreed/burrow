//! Unit tests for `split`, against a fake extractor.
//!
//! The fake records what it was asked for, so a test can assert the RUNS rather than the
//! bytes: whether page 4 ended up in the second output is a question about `(first, count)`,
//! and answering it by inspecting a PDF would mean trusting a second parser. The golden and
//! leak tests in `tests/` do the byte-level work against the real engine.

use std::sync::{Arc, Mutex};

use burrow_engines::{OpenOptions, OutputReader, PageExtractor};
use burrow_types::{Clock, Deadline, Error, Limits, ManualClock, Result};

use super::{Cuts, every, runs_from, split};

/// Every extraction the fake was asked for, in order.
type Calls = Arc<Mutex<Vec<(u64, u64)>>>;

struct Fake {
    pages: u64,
    calls: Calls,
    /// Fail the nth `extract`, to check an error is not swallowed.
    fail_at: Option<usize>,
    /// Time that passes inside each `extract`, so a deadline can expire mid-operation.
    ///
    /// The clock has to move *while the split is running*: `Deadline::start` is called after
    /// the input is open, so advancing beforehand simply moves the start. The first version
    /// of the deadline test did that and passed three outputs through a budget of 10 ms.
    elapse_ms: u64,
    clock: Option<Arc<ManualClock>>,
    /// Each source page's `/Rotate`, so a part's promise is a slice of something real.
    ///
    /// Distinct per page by default. A vector of equal values is what makes the promise
    /// degrade to a page count, and one test asks for exactly that.
    declares: Vec<i64>,
    /// What the READER says the emitted bytes contain, when a test wants it to lie.
    ///
    /// The lie lives on the reader rather than on `extract`, for the reason `reorder`'s fake
    /// records: an operation that emits the wrong thing and a reader that misreports the right
    /// thing are different failures, and only the second one tests the verifier.
    reads_back_as: Option<Vec<i64>>,
    /// Whether this instance came from `fresh`. The writing instance is not a witness.
    fresh: bool,
    /// How many times `fresh` was called, so a test can assert one witness per part.
    freshes: Arc<Mutex<usize>>,
}

impl Fake {
    fn new(pages: u64) -> (Self, Calls) {
        let calls: Calls = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                pages,
                calls: Arc::clone(&calls),
                fail_at: None,
                elapse_ms: 0,
                clock: None,
                declares: (0..pages)
                    .map(|n| i64::try_from(n % 4).unwrap_or(0) * 90)
                    .collect(),
                reads_back_as: None,
                fresh: false,
                freshes: Arc::new(Mutex::new(0)),
            },
            calls,
        )
    }
}

impl PageExtractor for Fake {
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
        _source: &Self::Source,
        _options: &OpenOptions<'_>,
        deadline: &Deadline,
    ) -> Result<Vec<i64>> {
        // CHECKPOINTED PER PAGE, like the real one: the sweep is what a deep page tree makes
        // expensive, and a fake that never checkpoints would let the deadline test pass while
        // the real engine sat outside the budget.
        if let Some(clock) = &self.clock {
            for _ in &self.declares {
                deadline.checkpoint(clock.as_ref())?;
            }
        }
        Ok(self.declares.clone())
    }

    fn extract(
        &self,
        _source: &Self::Source,
        first: u64,
        count: u64,
        _options: &OpenOptions<'_>,
    ) -> Result<Vec<u8>> {
        if let Some(clock) = &self.clock {
            clock.advance(self.elapse_ms);
        }
        let mut calls = self.calls.lock().expect("calls");
        if self.fail_at == Some(calls.len()) {
            return Err(Error::Malformed("fake: refused".to_owned()));
        }
        calls.push((first, count));
        // The "document" is the run, so order and membership are both observable -- and the
        // reader below turns it back into the rotations this part really holds, which is what
        // makes the fake HONEST by construction. A test that wants a lie sets `reads_back_as`.
        Ok(vec![
            u8::try_from(first % 251).unwrap_or(0),
            u8::try_from(count % 251).unwrap_or(0),
        ])
    }
}

impl OutputReader for Fake {
    /// The rotations the part being read back holds.
    type Read = Vec<i64>;

    fn fresh(&self) -> Self {
        *self.freshes.lock().expect("freshes") += 1;
        Self {
            pages: self.pages,
            calls: Arc::clone(&self.calls),
            fail_at: None,
            elapse_ms: 0,
            clock: self.clock.clone(),
            declares: self.declares.clone(),
            reads_back_as: self.reads_back_as.clone(),
            fresh: true,
            freshes: Arc::clone(&self.freshes),
        }
    }

    fn open_output(&self, bytes: &[u8], _options: &OpenOptions<'_>) -> Result<Self::Read> {
        if !self.fresh {
            // THE WRITING INSTANCE IS NOT A WITNESS, and this is how the fake says so: reading
            // through it yields nothing, so a `verify` that forgot `fresh()` fails loudly
            // rather than agreeing with itself.
            return Ok(Vec::new());
        }
        if let Some(lie) = &self.reads_back_as {
            return Ok(lie.clone());
        }
        // Undo `extract`'s encoding: the part is the run it was asked for.
        let first = usize::from(*bytes.first().unwrap_or(&0));
        let count = usize::from(*bytes.get(1).unwrap_or(&0));
        Ok(self
            .declares
            .get(first..first + count)
            .map(<[i64]>::to_vec)
            .unwrap_or_default())
    }

    fn page_count(&self, read: &Self::Read) -> Result<u64> {
        Ok(u64::try_from(read.len()).unwrap_or(0))
    }

    fn rotations(
        &self,
        read: &Self::Read,
        _options: &OpenOptions<'_>,
        deadline: &Deadline,
    ) -> Result<Vec<i64>> {
        if let Some(clock) = &self.clock {
            for _ in read {
                deadline.checkpoint(clock.as_ref())?;
            }
        }
        Ok(read.clone())
    }
}

fn options(limits: Limits) -> (OpenOptions<'static>, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::new(0));
    let opts = OpenOptions::new(limits, Arc::clone(&clock) as Arc<dyn Clock>);
    (opts, clock)
}

fn input() -> Box<[u8]> {
    vec![0u8; 32].into_boxed_slice()
}

// ---------------------------------------------------------------- the partition

#[test]
fn cuts_become_runs_that_cover_every_page_once() {
    let runs = runs_from(Cuts::after_pages(&[3, 7]), 10).expect("valid cuts");
    assert_eq!(runs, vec![(0, 3), (3, 4), (7, 3)]);

    // THE INVARIANT, restated as arithmetic: every page in exactly one run.
    let covered: u64 = runs.iter().map(|(_, count)| count).sum();
    assert_eq!(covered, 10, "the runs do not cover the document");
    let mut seen: Vec<u64> = runs
        .iter()
        .flat_map(|(first, count)| *first..(*first + *count))
        .collect();
    seen.sort_unstable();
    assert_eq!(
        seen,
        (0..10).collect::<Vec<_>>(),
        "a page was lost or duplicated"
    );
}

#[test]
fn no_cuts_is_a_one_way_split() {
    assert_eq!(
        runs_from(Cuts::after_pages(&[]), 4).expect("valid"),
        vec![(0, 4)]
    );
}

#[test]
fn a_cut_outside_the_document_is_refused() {
    for bad in [&[10u64][..], &[11][..], &[0][..]] {
        let err = runs_from(Cuts::after_pages(bad), 10).expect_err("must refuse");
        assert!(matches!(err, Error::InvalidArgument(_)), "{bad:?}: {err:?}");
    }
}

#[test]
fn repeated_or_descending_cuts_are_refused() {
    for bad in [&[3u64, 3][..], &[7, 3][..]] {
        let err = runs_from(Cuts::after_pages(bad), 10).expect_err("must refuse");
        assert!(matches!(err, Error::InvalidArgument(_)), "{bad:?}: {err:?}");
    }
}

#[test]
fn a_document_with_no_pages_cannot_be_split() {
    let err = runs_from(Cuts::after_pages(&[]), 0).expect_err("must refuse");
    assert!(matches!(err, Error::InvalidArgument(_)), "{err:?}");
}

// ---------------------------------------------------------------- every N

#[test]
fn every_n_cuts_where_a_person_would_expect() {
    assert_eq!(every(5, 10, &Limits::DEFAULT).expect("valid"), vec![5]);
    assert_eq!(every(5, 11, &Limits::DEFAULT).expect("valid"), vec![5, 10]);
    assert_eq!(every(1, 3, &Limits::DEFAULT).expect("valid"), vec![1, 2]);
    // A run larger than the document is one part, not an error: "every 100 pages" of a
    // 10-page document is the document.
    assert_eq!(
        every(100, 10, &Limits::DEFAULT).expect("valid"),
        Vec::<u64>::new()
    );
}

#[test]
fn every_n_agrees_with_the_partition_it_describes() {
    // The two halves of the "every N" affordance, checked against each other rather than
    // each against a hand-written list: whatever `every` produces must be cuts `runs_from`
    // accepts, and the runs must cover the document.
    for pages in 1..40u64 {
        for n in 1..12u64 {
            let cuts = every(n, pages, &Limits::DEFAULT).expect("valid");
            let runs = runs_from(Cuts::after_pages(&cuts), pages)
                .unwrap_or_else(|e| panic!("every({n}, {pages}) produced unusable cuts: {e:?}"));
            let covered: u64 = runs.iter().map(|(_, c)| c).sum();
            assert_eq!(covered, pages, "every({n}, {pages}) lost pages");
            assert!(
                runs.iter().all(|(_, c)| *c <= n),
                "every({n}, {pages}) produced a run longer than {n}"
            );
        }
    }
}

#[test]
fn an_enormous_page_count_is_refused_rather_than_allocated() {
    // `every(1, u64::MAX)` used to allocate until it panicked with a capacity overflow, in
    // library code, which `core/CLAUDE.md` forbids. The result length is `pages / every` and
    // both come from the caller, so the ceiling has to be applied before the vector exists.
    let err = every(1, u64::MAX, &Limits::DEFAULT).expect_err("must refuse");
    match err {
        Error::LimitExceeded { limit, .. } => assert_eq!(limit, "max_pages"),
        other => panic!("expected LimitExceeded, got {other:?}"),
    }
}

#[test]
fn a_run_of_zero_pages_is_refused() {
    assert!(matches!(
        every(0, 10, &Limits::DEFAULT),
        Err(Error::InvalidArgument(_))
    ));
}

// ---------------------------------------------------------------- the operation

#[test]
fn each_run_is_extracted_once_in_order() {
    let (fake, calls) = Fake::new(10);
    let (opts, _clock) = options(Limits::DEFAULT);

    let outputs = split(&fake, input(), Cuts::after_pages(&[3, 7]), &opts).expect("split");

    assert_eq!(outputs.len(), 3);
    assert_eq!(*calls.lock().expect("calls"), vec![(0, 3), (3, 4), (7, 3)]);
    assert_eq!(outputs, vec![vec![0, 3], vec![3, 4], vec![7, 3]]);
}

#[test]
fn a_failing_extraction_fails_the_split() {
    // ALL OR NOTHING, as merge is. A split that returned the outputs it managed would hand
    // somebody a partial partition with no indication which part is missing.
    let (mut fake, _calls) = Fake::new(10);
    fake.fail_at = Some(1);
    let (opts, _clock) = options(Limits::DEFAULT);

    let err = split(&fake, input(), Cuts::after_pages(&[3, 7]), &opts).expect_err("must fail");
    assert!(matches!(err, Error::Malformed(_)), "{err:?}");
}

#[test]
fn running_out_of_time_stops_the_split_between_outputs() {
    // The checkpoint is BETWEEN outputs, which is the only granularity available -- the work
    // inside `extract` is one engine call per page and no engine offers a cancellation hook.
    // So the assertion is that it stops after the output that overran, not during it.
    let (mut fake, calls) = Fake::new(10);
    let (opts, clock) = options(Limits::with(|l| l.max_duration_ms = 10));
    fake.elapse_ms = 30;
    fake.clock = Some(Arc::clone(&clock));

    let err = split(&fake, input(), Cuts::after_pages(&[3, 7]), &opts).expect_err("must fail");
    match err {
        Error::LimitExceeded { limit, .. } => assert_eq!(limit, "max_duration_ms"),
        other => panic!("expected LimitExceeded, got {other:?}"),
    }
    assert_eq!(
        calls.lock().expect("calls").len(),
        1,
        "the deadline should have stopped the split after the first output overran"
    );
}

#[test]
fn an_invalid_cut_is_refused_before_any_extraction() {
    let (fake, calls) = Fake::new(10);
    let (opts, _clock) = options(Limits::DEFAULT);

    let err = split(&fake, input(), Cuts::after_pages(&[99]), &opts).expect_err("must fail");
    assert!(matches!(err, Error::InvalidArgument(_)), "{err:?}");
    assert!(
        calls.lock().expect("calls").is_empty(),
        "an unusable request still reached the engine"
    );
}

// ---------------------------------------------------------------- the promise (ADR 0022)
//
// On a correct engine and an undamaged file the refusal never fires, so it is untested by
// construction unless a fake lies. These are the tests that make it fire.

// THE MUTATION THAT DELETES THE CHECK FAILS, measured rather than asserted. Replacing the
// verification loop's `outputs.iter()` with `outputs.iter().take(0)` -- so it runs zero times --
// fails FOUR of the tests below:
//
//     a_part_that_reads_back_with_the_wrong_rotations_is_refused
//     a_part_with_the_wrong_page_count_is_refused
//     a_part_built_from_the_wrong_run_is_caught_where_the_pages_differ
//     every_part_is_verified_through_a_fresh_engine
//
// The mutation was asserted to have applied before the suite ran, because a `replace` that
// matched nothing leaves the suite green and the green reads as "this defence works" when
// nothing was mutated at all -- which `CLAUDE.md` records happening twice in one session.

#[test]
fn every_part_is_verified_through_a_fresh_engine() {
    // ONE WITNESS PER PART, not one for the split. "The first part is right" says nothing about
    // the partition, which is the whole reason the promise is per part rather than a total.
    let (fake, _calls) = Fake::new(10);
    let freshes = Arc::clone(&fake.freshes);
    let (opts, _clock) = options(Limits::DEFAULT);
    let outputs = split(&fake, input(), Cuts::after_pages(&[3, 7]), &opts).expect("split");
    assert_eq!(outputs.len(), 3);
    assert_eq!(
        *freshes.lock().expect("freshes"),
        3,
        "each part must be read back through its own fresh engine; the writing instance is not \
         a witness to its own output"
    );
}

#[test]
fn a_part_that_reads_back_with_the_wrong_rotations_is_refused() {
    // THE FAKE LIES ON THE WAY BACK. The engine emits the right parts and the reader reports
    // something else -- which is the shape of the failure ADR 0022 exists for: an operation
    // whose intent and whose output disagree.
    let (mut fake, _calls) = Fake::new(10);
    fake.reads_back_as = Some(vec![0, 0, 0]);
    let (opts, _clock) = options(Limits::DEFAULT);
    let error = split(&fake, input(), Cuts::after_pages(&[3, 7]), &opts).expect_err("must fail");
    assert!(
        matches!(error, Error::OutputRejected(_)),
        "a part whose pages read back displaying differently was returned to the caller: \
         {error:?}"
    );
}

#[test]
fn a_part_with_the_wrong_page_count_is_refused() {
    // The weaker half of the same promise, and the one that catches #61's lost page: the part
    // reads back short.
    let (mut fake, _calls) = Fake::new(10);
    fake.reads_back_as = Some(vec![0]);
    let (opts, _clock) = options(Limits::DEFAULT);
    let error = split(&fake, input(), Cuts::after_pages(&[3, 7]), &opts).expect_err("must fail");
    match error {
        Error::OutputRejected(message) => assert!(
            message.contains("pages were expected"),
            "the refusal does not name the two counts: {message}"
        ),
        other => panic!("a short part was returned to the caller: {other:?}"),
    }
}

#[test]
fn a_part_built_from_the_wrong_run_is_caught_where_the_pages_differ() {
    // WHAT THE SLICE ADDS OVER A COUNT. The lie has the RIGHT NUMBER of pages and the wrong
    // ones: it is the second part's rotations reported for the first part. A page count cannot
    // see this, which is why ADR 0022's table was amended from one to the other.
    let (mut fake, _calls) = Fake::new(10);
    // Part one is pages 0-2, which the fake declares as 0, 90, 180. Report 270, 0, 90 -- the
    // right length, the wrong run.
    fake.reads_back_as = Some(vec![270, 0, 90]);
    let (opts, _clock) = options(Limits::DEFAULT);
    let error = split(&fake, input(), Cuts::after_pages(&[3, 7]), &opts).expect_err("must fail");
    assert!(
        matches!(error, Error::OutputRejected(_)),
        "a part built from the wrong run of pages was accepted: {error:?}"
    );
}

#[test]
fn a_partition_of_pages_that_all_display_the_same_way_is_only_a_page_count() {
    // THE RESIDUE, ASSERTED RATHER THAN LEFT AS PROSE. `Expected::Split`'s rustdoc says the
    // promise degrades to a per-part page count when every page displays the same way. A
    // statement about what a check CANNOT do is worth as much as one about what it can, and it
    // is worth exactly nothing if nobody has run it.
    // NINE PAGES CUT INTO THREE EQUAL PARTS, so the one lie below is the right LENGTH for every
    // part. With uneven parts it tripped the page-count check instead and the test passed for
    // the wrong reason -- measuring the count residue rather than the rotation one, which is a
    // different sentence in the rustdoc.
    let (mut fake, _calls) = Fake::new(9);
    fake.declares = vec![0; 9];
    // Three pages that are not part one's own -- and indistinguishable from them, because every
    // page in this document declares the same rotation.
    fake.reads_back_as = Some(vec![0, 0, 0]);
    let (opts, _clock) = options(Limits::DEFAULT);
    let outputs = split(&fake, input(), Cuts::after_pages(&[3, 6]), &opts);
    assert!(
        outputs.is_ok(),
        "this is the documented blind spot: on a uniform document the promise is a page count, \
         and a test asserting otherwise would be asserting the residue away rather than \
         measuring it"
    );
}

#[test]
fn one_budget_covers_the_source_sweep_and_every_read_back() {
    // NOT A FRESH DEADLINE PER PART. `Deadline::start` resets the origin AND the budget, so a
    // verification that started its own would hand each part a whole `max_duration_ms` -- the
    // defect ADR 0022 records being fixed three times over in `rotations`. With three parts and
    // a budget that the sweep plus the read-backs must share, an implementation that restarted
    // would succeed where this must refuse.
    let (mut fake, _calls) = Fake::new(10);
    // `Limits` is `#[non_exhaustive]`, so it is adjusted rather than built.
    let mut limits = Limits::DEFAULT;
    limits.max_duration_ms = 20;
    let (opts, clock) = options(limits);
    fake.clock = Some(Arc::clone(&clock));
    fake.elapse_ms = 8;
    let error = split(&fake, input(), Cuts::after_pages(&[3, 7]), &opts).expect_err("must fail");
    assert!(
        matches!(error, Error::LimitExceeded { .. }),
        "three parts at 8 ms each fitted inside a 20 ms budget, so something restarted it: \
         {error:?}"
    );
}
