//! The operation's own half, against an engine that lies on the way back.
//!
//! V1's frames, V2 and V3 are the engine's and are tested there, on the real qpdf
//! (`burrow_engines::redact::flatten`). What is tested here is V1's shared half -- the page count
//! and the rotation vector with page N's entry 0 -- which a correct engine cannot fail, so the
//! engine is a fake.

#![allow(clippy::unwrap_used, clippy::expect_used, reason = "tests")]

use std::sync::Arc;

use burrow_engines::redact::flatten::Picture;
use burrow_engines::{OpenOptions, OutputReader, PageFlattener};
use burrow_types::{Deadline, Error, Limits, ManualClock, Result};

struct FakeRead {
    rotations: Vec<i64>,
}

#[derive(Clone)]
struct Fake {
    /// What the INPUT has.
    rotations: Vec<i64>,
    /// What the READ-BACK reports. `None` is an honest engine: the input's, page N's turned 0.
    read_back: Option<Vec<i64>>,
    page: usize,
}

impl Fake {
    fn honest(rotations: Vec<i64>, page: usize) -> Self {
        Self {
            rotations,
            read_back: None,
            page,
        }
    }
}

impl PageFlattener for Fake {
    fn flatten_page(
        &self,
        _bytes: &[u8],
        _page: usize,
        _picture: &Picture<'_>,
        _options: &OpenOptions<'_>,
    ) -> Result<Vec<u8>> {
        Ok(b"%PDF-1.7\n%flattened\n".to_vec())
    }

    fn rotations_before_flattening(
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
        let honest = {
            let mut rotations = self.rotations.clone();
            rotations[self.page] = 0;
            rotations
        };
        Ok(FakeRead {
            rotations: self.read_back.clone().unwrap_or(honest),
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

fn run(engine: &Fake) -> Result<Vec<u8>> {
    let options = OpenOptions::new(Limits::DEFAULT, Arc::new(ManualClock::new(0)));
    super::page(
        engine,
        b"%PDF",
        engine.page,
        &Picture::new(1, 1, &[0, 0, 0]),
        &options,
    )
}

#[test]
fn an_honest_engine_produces_a_document() {
    // NON-VACUITY: every test below would pass for an operation that rejects everything.
    let done = run(&Fake::honest(vec![0, 90, 270], 1)).expect("an honest engine is accepted");
    assert!(done.starts_with(b"%PDF"));
}

#[test]
fn the_flattened_page_s_rotation_is_promised_as_zero() {
    // KILLS: promising the input's vector unchanged. Page 1 is written `/Rotate 0`, so a read-back
    // that kept its 90 is a page turned twice (ADR 0031 §3).
    let mut fake = Fake::honest(vec![0, 90, 270], 1);
    fake.read_back = Some(vec![0, 90, 270]);
    let error = run(&fake).expect_err("page 1 kept its rotation");
    assert!(
        matches!(error, Error::OutputRejected(ref what) if what.contains("flatten")),
        "the rejection must name the operation: {error:?}"
    );
}

#[test]
fn another_page_turned_or_a_page_lost_is_rejected() {
    // KILLS: skipping `verify::output`.
    for read_back in [vec![0, 0, 0], vec![0, 0]] {
        let mut fake = Fake::honest(vec![0, 90, 270], 1);
        fake.read_back = Some(read_back.clone());
        let error = run(&fake).expect_err("the read-back differs from the promise");
        assert!(
            matches!(error, Error::OutputRejected(_)),
            "{read_back:?}: {error:?}"
        );
    }
}

#[test]
fn a_page_past_the_end_is_refused_before_the_engine_runs() {
    let fake = Fake::honest(vec![0, 0], 2);
    let options = OpenOptions::new(Limits::DEFAULT, Arc::new(ManualClock::new(0)));
    let error = super::page(&fake, b"%PDF", 2, &Picture::new(1, 1, &[0, 0, 0]), &options)
        .expect_err("there is no page 3");
    assert!(matches!(error, Error::InvalidArgument(_)), "{error:?}");
}
