//! Unit tests for `split`, against a fake extractor.
//!
//! The fake records what it was asked for, so a test can assert the RUNS rather than the
//! bytes: whether page 4 ended up in the second output is a question about `(first, count)`,
//! and answering it by inspecting a PDF would mean trusting a second parser. The golden and
//! leak tests in `tests/` do the byte-level work against the real engine.

use std::sync::{Arc, Mutex};

use burrow_engines::{OpenOptions, PageExtractor};
use burrow_types::{Clock, Error, Limits, ManualClock, Result};

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
        // The "document" is the run, so order and membership are both observable.
        Ok(vec![
            u8::try_from(first % 251).unwrap_or(0),
            u8::try_from(count % 251).unwrap_or(0),
        ])
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
