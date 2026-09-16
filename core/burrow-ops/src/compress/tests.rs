//! Unit tests for the compress operation.
//!
//! The engine-level suite (`burrow-engines`' `compress_tests`) covers the lever and what it
//! costs the container. What is left to this one is the operation's own decisions: which
//! ceilings it applies, the never-worse rule, and the ADR 0022 refusal — which on a correct
//! engine and an undamaged file can never fire, so it is untested by construction unless a
//! fake engine lies on the way back.

use std::sync::Arc;

use burrow_engines::{DocumentCompressor, OpenOptions, OutputReader};
use burrow_types::{Clock, Deadline, Error, Limits, ManualClock, Result};

use super::{Outcome, compress};

fn stopped() -> Arc<dyn Clock> {
    Arc::new(ManualClock::new(0))
}

fn options() -> OpenOptions<'static> {
    OpenOptions::new(Limits::default(), stopped())
}

/// A source the fakes below hand around: a page count and a rotation vector, nothing more.
#[derive(Clone)]
struct FakeSource {
    rotations: Vec<i64>,
}

/// An engine whose behaviour every test dials in, because the failures under test cannot be
/// produced by a correct one.
#[derive(Clone)]
struct Fake {
    /// What `open`/`rotations` report about the input.
    rotations: Vec<i64>,
    /// The bytes `compress` returns.
    produced: Vec<u8>,
    /// What the READ-BACK reports. `None` means "the same as the input", i.e. an honest engine.
    read_back: Option<Vec<i64>>,
}

impl Fake {
    fn honest(pages: usize, produced: usize) -> Self {
        Self {
            rotations: vec![0; pages],
            produced: vec![b'x'; produced],
            read_back: None,
        }
    }
}

impl DocumentCompressor for Fake {
    type Source = FakeSource;

    fn name(&self) -> &'static str {
        "fake"
    }

    fn open(&self, _bytes: Box<[u8]>, _options: &OpenOptions<'_>) -> Result<Self::Source> {
        Ok(FakeSource {
            rotations: self.rotations.clone(),
        })
    }

    fn pages(&self, source: &Self::Source) -> Result<u64> {
        Ok(u64::try_from(source.rotations.len()).unwrap_or(u64::MAX))
    }

    fn rotations(
        &self,
        source: &Self::Source,
        _options: &OpenOptions<'_>,
        _deadline: &Deadline,
    ) -> Result<Vec<i64>> {
        Ok(source.rotations.clone())
    }

    fn compress(&self, _source: &Self::Source, _options: &OpenOptions<'_>) -> Result<Vec<u8>> {
        Ok(self.produced.clone())
    }
}

impl OutputReader for Fake {
    type Read = FakeSource;

    fn fresh(&self) -> Self {
        self.clone()
    }

    fn open_output(&self, _bytes: &[u8], _options: &OpenOptions<'_>) -> Result<Self::Read> {
        Ok(FakeSource {
            // THE LIE LIVES HERE. An honest engine reads back what it wrote.
            rotations: self
                .read_back
                .clone()
                .unwrap_or_else(|| self.rotations.clone()),
        })
    }

    fn page_count(&self, read: &Self::Read) -> Result<u64> {
        Ok(u64::try_from(read.rotations.len()).unwrap_or(u64::MAX))
    }

    fn rotations(
        &self,
        read: &Self::Read,
        _options: &OpenOptions<'_>,
        _deadline: &Deadline,
    ) -> Result<Vec<i64>> {
        Ok(read.rotations.clone())
    }
}

// ------------------------------------------------------------------ the never-worse rule

#[test]
fn a_smaller_re_encoding_comes_back_with_both_numbers() {
    let engine = Fake::honest(4, 40);
    let out = compress(&engine, vec![0u8; 100].into_boxed_slice(), &options()).unwrap();
    assert_eq!(
        out,
        Outcome::Smaller {
            document: vec![b'x'; 40],
            original_bytes: 100,
        }
    );
    assert_eq!(out.original_bytes(), 100);
    assert_eq!(out.produced_bytes(), 40);
    assert_eq!(out.document(), Some(&[b'x'; 40][..]));
}

#[test]
fn a_larger_re_encoding_is_discarded_and_says_so() {
    let engine = Fake::honest(4, 250);
    let out = compress(&engine, vec![0u8; 100].into_boxed_slice(), &options()).unwrap();
    assert_eq!(
        out,
        Outcome::NotSmaller {
            original_bytes: 100,
            produced_bytes: 250,
        }
    );
    // NOTHING IS RETURNED, and the caller's own bytes are not handed back as though burrow had
    // produced them. Both numbers are what lets a page say which way it went.
    assert_eq!(out.document(), None);
}

#[test]
fn an_exactly_equal_re_encoding_counts_as_not_smaller() {
    // `>=`, NOT `>`. A re-encoding that lands on the input's size achieved nothing, and
    // returning it would be reporting a saving of zero bytes as a result. Measured at 5.3% of
    // qpdf's own corpus, so this is the common case rather than a boundary curiosity.
    let engine = Fake::honest(4, 100);
    let out = compress(&engine, vec![0u8; 100].into_boxed_slice(), &options()).unwrap();
    assert!(
        matches!(
            out,
            Outcome::NotSmaller {
                produced_bytes: 100,
                original_bytes: 100
            }
        ),
        "got {out:?}"
    );
}

// ------------------------------------------------------------------ ADR 0022

#[test]
fn an_engine_that_returns_the_wrong_page_count_is_refused() {
    // THE TEST THAT MAKES THE CHECK FIRE. On a correct engine and an undamaged file the refusal
    // never happens, so without a fake that lies the verification is untested by construction --
    // and the mutation that deletes it would pass.
    let engine = Fake {
        rotations: vec![0; 5],
        produced: vec![b'x'; 40],
        read_back: Some(vec![0; 4]),
    };
    let refused = compress(&engine, vec![0u8; 100].into_boxed_slice(), &options());
    assert!(
        matches!(refused, Err(Error::OutputRejected(_))),
        "got {refused:?}"
    );
}

#[test]
fn an_engine_that_moves_a_rotation_is_refused() {
    // The page COUNT is right and the vector is not: compression may not change what a page
    // displays at. A count-only promise would accept this.
    let engine = Fake {
        rotations: vec![0, 90, 0, 270],
        produced: vec![b'x'; 40],
        read_back: Some(vec![0, 0, 90, 270]),
    };
    let refused = compress(&engine, vec![0u8; 100].into_boxed_slice(), &options());
    assert!(
        matches!(refused, Err(Error::OutputRejected(_))),
        "got {refused:?}"
    );
}

#[test]
fn a_document_that_did_not_shrink_is_never_verified() {
    // The ordering is deliberate: verification sits between the engine and the CALLER, and a
    // document that is not going to the caller has none to protect. A lying read-back here
    // would be caught if verification ran -- it does not, and that is the assertion.
    let engine = Fake {
        rotations: vec![0; 5],
        produced: vec![b'x'; 250],
        read_back: Some(vec![0; 1]),
    };
    let out = compress(&engine, vec![0u8; 100].into_boxed_slice(), &options()).unwrap();
    assert!(matches!(out, Outcome::NotSmaller { .. }), "got {out:?}");
}

// ------------------------------------------------------------------ limits

#[test]
fn a_document_over_the_page_ceiling_is_refused() {
    let engine = Fake::honest(11, 40);
    let limits = Limits::with(|l| l.max_pages = 10);
    let refused = compress(
        &engine,
        vec![0u8; 100].into_boxed_slice(),
        &OpenOptions::new(limits, stopped()),
    );
    assert!(
        matches!(refused, Err(Error::LimitExceeded { limit, .. }) if limit == "max_pages"),
        "got {refused:?}"
    );
}

/// An engine whose `rotations` answers differently after `compress` has run.
///
/// ADR 0022: *compute the promise BEFORE the operation runs*. `compress` does — but moving
/// that line below the write left every test green, because the ordinary fake answers the
/// same either way and real qpdf does not mutate the source across a write. So the ordering
/// the comment claims was unpinned. Found by security review.
///
/// This fake makes the two readings differ, so the promise can only match the output if it
/// was taken first.
struct ShiftingFake {
    written: std::cell::Cell<bool>,
}

impl DocumentCompressor for ShiftingFake {
    type Source = FakeSource;

    fn name(&self) -> &'static str {
        "shifting"
    }

    fn open(&self, _bytes: Box<[u8]>, _options: &OpenOptions<'_>) -> Result<Self::Source> {
        Ok(FakeSource {
            rotations: vec![0, 90],
        })
    }

    fn pages(&self, source: &Self::Source) -> Result<u64> {
        Ok(u64::try_from(source.rotations.len()).unwrap_or(u64::MAX))
    }

    fn rotations(
        &self,
        _source: &Self::Source,
        _options: &OpenOptions<'_>,
        _deadline: &Deadline,
    ) -> Result<Vec<i64>> {
        // BEFORE the write, the truth. AFTER it, something else -- which is what a hostile or
        // broken engine looks like, and the case ADR 0022 exists for.
        if self.written.get() {
            Ok(vec![270, 180])
        } else {
            Ok(vec![0, 90])
        }
    }

    fn compress(&self, _source: &Self::Source, _options: &OpenOptions<'_>) -> Result<Vec<u8>> {
        self.written.set(true);
        Ok(vec![b'x'; 40])
    }
}

impl OutputReader for ShiftingFake {
    type Read = FakeSource;

    fn fresh(&self) -> Self {
        // A FRESH INSTANCE THAT REMEMBERS THE WRITE, because the read-back has to see the
        // output's real rotations -- which are the input's. `fresh` is about a fresh parse,
        // not about forgetting what the document is.
        Self {
            written: std::cell::Cell::new(self.written.get()),
        }
    }

    fn open_output(&self, _bytes: &[u8], _options: &OpenOptions<'_>) -> Result<Self::Read> {
        Ok(FakeSource {
            rotations: vec![0, 90],
        })
    }

    fn page_count(&self, read: &Self::Read) -> Result<u64> {
        Ok(u64::try_from(read.rotations.len()).unwrap_or(u64::MAX))
    }

    fn rotations(
        &self,
        read: &Self::Read,
        _options: &OpenOptions<'_>,
        _deadline: &Deadline,
    ) -> Result<Vec<i64>> {
        Ok(read.rotations.clone())
    }
}

#[test]
fn the_promise_is_the_reading_taken_before_the_write() {
    // The output genuinely displays [0, 90]. If the promise is read BEFORE the write it is
    // [0, 90] too and the operation succeeds; if it is read after, it is [270, 180] and
    // verification refuses a document that was never wrong.
    //
    // So moving the promise below `engine.compress` turns this green test red -- which is the
    // only thing that pins ADR 0022's ordering for this operation.
    let engine = ShiftingFake {
        written: std::cell::Cell::new(false),
    };
    let out = compress(&engine, vec![0u8; 100].into_boxed_slice(), &options())
        .expect("the promise must be the pre-write reading");
    assert!(matches!(out, Outcome::Smaller { .. }), "got {out:?}");
}

/// A clock that advances on every read, so any checkpoint at all eventually refuses.
struct Stepping(std::sync::atomic::AtomicU64);

impl Clock for Stepping {
    fn now_ms(&self) -> u64 {
        self.0.fetch_add(10, std::sync::atomic::Ordering::SeqCst)
    }
}

fn stepping(limits: Limits) -> OpenOptions<'static> {
    OpenOptions::new(
        limits,
        Arc::new(Stepping(std::sync::atomic::AtomicU64::new(0))) as Arc<dyn Clock>,
    )
}

#[test]
fn the_operation_refuses_when_its_own_budget_is_spent() {
    // THIS ONE DOES NOT PIN THE OPERATION'S OWN CHECKPOINTS, and its first version claimed it
    // did. On the `Smaller` path the refusal that fires is `verify::output`'s -- verification
    // has three checkpoints of its own and runs on every returned document -- so deleting both
    // of `compress`'s left this green. Code review measured it.
    //
    // It is kept because it does assert something real: that the whole operation is bounded on
    // the path a caller usually takes. What pins the operation's own checkpoints is the test
    // below, on the path verification never reaches.
    let engine = Fake::honest(4, 40);
    let refused = compress(
        &engine,
        vec![0u8; 100].into_boxed_slice(),
        &stepping(Limits::with(|l| l.max_duration_ms = 5)),
    );
    assert!(
        matches!(refused, Err(Error::LimitExceeded { limit, .. }) if limit == "max_duration_ms"),
        "got {refused:?}"
    );
}

#[test]
fn the_not_smaller_path_is_bounded_too_and_nothing_else_bounds_it() {
    // THE PATH THAT SKIPS VERIFICATION, which is the never-worse branch and roughly 8.7% of
    // qpdf's own corpus (3.4% strictly larger plus 5.3% identical). Nothing downstream
    // checkpoints here: the operation returns before `verify::output` is reached, so if
    // `compress`'s own two checkpoints go, a spent budget becomes a silent success.
    //
    // Measured by code review, with both checkpoints deleted:
    //   mutated:   Ok(NotSmaller { original_bytes: 100, produced_bytes: 250 })
    //   unmutated: Err(LimitExceeded { limit: "max_duration_ms", ... })
    //
    // A limit escape that the entire workspace reported as green. This is the test that closes
    // it.
    let engine = Fake::honest(4, 250);
    let refused = compress(
        &engine,
        vec![0u8; 100].into_boxed_slice(),
        &stepping(Limits::with(|l| l.max_duration_ms = 5)),
    );
    assert!(
        matches!(refused, Err(Error::LimitExceeded { limit, .. }) if limit == "max_duration_ms"),
        "a spent budget on the NotSmaller path returned {refused:?}"
    );

    // THE CONTROL: the same engine and the same path, with budget to spare. Without it the
    // assertion above is satisfied by an operation that refuses everything.
    let allowed = compress(&engine, vec![0u8; 100].into_boxed_slice(), &options()).unwrap();
    assert!(
        matches!(allowed, Outcome::NotSmaller { .. }),
        "got {allowed:?}"
    );
}

#[test]
fn a_budget_with_room_does_not_refuse() {
    // THE CONTROL for the test above: without it, that assertion is satisfied by an operation
    // that refuses everything.
    let engine = Fake::honest(4, 40);
    let out = compress(&engine, vec![0u8; 100].into_boxed_slice(), &options());
    assert!(matches!(out, Ok(Outcome::Smaller { .. })), "got {out:?}");
}
