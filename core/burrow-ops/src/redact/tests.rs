//! The operation's own half, against an engine that lies on the way back.
//!
//! The region check is `burrow_engines::redact_verify`'s and is tested there. What is tested
//! here is the half `burrow-ops` adds — the page count and the `/Rotate` vector — and the two
//! guards around it. Those cannot be produced by a correct engine, so the engine is a fake.

use std::collections::BTreeSet;
use std::sync::Arc;

use burrow_engines::pdfsyntax::region::Region;
use burrow_engines::redact::Report;
use burrow_engines::{OpenOptions, OutputReader, PageRedactor};
use burrow_types::{Deadline, Error, Limits, ManualClock, Result};

/// A document read back, as the fake reports it.
struct FakeRead {
    rotations: Vec<i64>,
}

/// An engine whose read-back every test dials in.
#[derive(Clone)]
struct Fake {
    /// What the INPUT has.
    rotations: Vec<i64>,
    /// What the READ-BACK reports. `None` is an honest engine.
    read_back: Option<Vec<i64>>,
}

impl Fake {
    fn honest(pages: usize) -> Self {
        Self {
            rotations: vec![0; pages],
            read_back: None,
        }
    }
}

impl PageRedactor for Fake {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn redact_page(
        &self,
        _bytes: &[u8],
        _page: usize,
        _redacted: &BTreeSet<usize>,
        _region: Region,
        _options: &OpenOptions<'_>,
    ) -> Result<(Vec<u8>, Report)> {
        Ok((b"%PDF-1.7\n".to_vec(), Report { fonts: Vec::new() }))
    }

    fn input_rotations(
        &self,
        _bytes: &[u8],
        _options: &OpenOptions<'_>,
        _deadline: &Deadline,
    ) -> Result<Vec<i64>> {
        Ok(self.rotations.clone())
    }
}

impl OutputReader for Fake {
    type Read = FakeRead;

    fn fresh(&self) -> Self {
        self.clone()
    }

    fn open_output(&self, _bytes: &[u8], _options: &OpenOptions<'_>) -> Result<Self::Read> {
        Ok(FakeRead {
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

fn options() -> OpenOptions<'static> {
    OpenOptions::new(Limits::default(), Arc::new(ManualClock::new(0)))
}

fn band() -> Region {
    Region {
        left: 0.0,
        top: 0.0,
        width: 100.0,
        height: 100.0,
    }
}

fn run(engine: &Fake, page: usize, redacted: &[usize]) -> Result<super::Redaction> {
    let covered: BTreeSet<usize> = redacted.iter().copied().collect();
    super::page(engine, b"%PDF", page, &covered, band(), &options())
}

#[test]
fn an_honest_engine_produces_a_document() {
    // NON-VACUITY. Without it every test below passes for an operation that rejects everything.
    let done = run(&Fake::honest(3), 0, &[0]).expect("an honest engine is not rejected");
    assert!(done.document.starts_with(b"%PDF"));
}

#[test]
fn a_read_back_that_loses_a_page_is_rejected() {
    // KILLS: skipping `verify::output`. The region check inside the engine says nothing about
    // how many pages came out, so a redaction that dropped one would be accepted without this.
    let mut fake = Fake::honest(3);
    fake.read_back = Some(vec![0, 0]);
    let error = run(&fake, 0, &[0]).expect_err("two pages came back from three");
    assert!(
        matches!(error, Error::OutputRejected(ref what) if what.contains("redact")),
        "the rejection must name the operation: {error:?}"
    );
}

#[test]
fn a_read_back_whose_rotation_changed_is_rejected() {
    // The vector, not just the count. A redaction must not turn a page.
    let mut fake = Fake::honest(3);
    fake.read_back = Some(vec![0, 90, 0]);
    let error = run(&fake, 0, &[0]).expect_err("page 1 came back rotated");
    assert!(matches!(error, Error::OutputRejected(_)), "got: {error:?}");
}

#[test]
fn the_promise_is_read_from_the_input_and_not_from_the_output() {
    // KILLS: taking the rotation promise from the output. That would be the output agreeing
    // with itself, which is the thing `OutputReader::fresh` exists to stop -- and it would make
    // the test above pass, because the promise would move with the lie.
    //
    // The input has three pages rotated 0, 90, 0; the read-back claims three pages all at 0. A
    // promise taken from the output matches; one taken from the input does not.
    let mut fake = Fake::honest(3);
    fake.rotations = vec![0, 90, 0];
    fake.read_back = Some(vec![0, 0, 0]);
    let error = run(&fake, 0, &[0]).expect_err("the output's rotations are not the input's");
    assert!(matches!(error, Error::OutputRejected(_)), "got: {error:?}");
}

#[test]
fn a_page_the_operation_does_not_cover_is_refused() {
    // KILLS: dropping the page-in-set check. `cut_fonts` decides cuttability across `redacted`,
    // so a page outside it has its fonts judged against a set that excludes it.
    let error = run(&Fake::honest(3), 0, &[1, 2]).expect_err("page 0 is not covered");
    assert!(
        matches!(error, Error::InvalidArgument(ref what) if what.contains("covers")),
        "got: {error:?}"
    );
}
