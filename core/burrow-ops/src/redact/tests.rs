//! The operation's own half, against an engine that lies on the way back.
//!
//! The region check is `burrow_engines::redact_verify`'s and is tested there. What is tested
//! here is the half `burrow-ops` adds — the page count and the `/Rotate` vector — and the two
//! guards around it. Those cannot be produced by a correct engine, so the engine is a fake.

use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;
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
    /// How many times `redact_page` ran. Each call's document differs from every other's (R10):
    /// with a constant document, "verify one call's output and return another's" is an
    /// equivalent mutation and no test can see it. Shared, because `fresh` clones the fake.
    calls: Rc<Cell<usize>>,
    /// Every call returns the SAME document instead: the near-miss twin, where a second call's
    /// bytes equal the first's and identity by content cannot tell them apart -- by design.
    same_every_call: bool,
    /// The sha256 of every buffer the read-back was handed, in order.
    opened: Rc<RefCell<Vec<String>>>,
}

impl Fake {
    fn honest(pages: usize) -> Self {
        Self {
            rotations: vec![0; pages],
            read_back: None,
            calls: Rc::new(Cell::new(0)),
            same_every_call: false,
            opened: Rc::new(RefCell::new(Vec::new())),
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
        self.calls.set(self.calls.get() + 1);
        let call = if self.same_every_call {
            1
        } else {
            self.calls.get()
        };
        Ok((
            format!("%PDF-1.7\n%call {call}\n").into_bytes(),
            Report {
                fonts: Vec::new(),
                dropped_carried_text: 0,
            },
        ))
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

    fn open_output(&self, bytes: &[u8], _options: &OpenOptions<'_>) -> Result<Self::Read> {
        self.opened.borrow_mut().push(digest(bytes));
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

/// The sha256 of `bytes`, as hex.
fn digest(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
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
    super::page(engine, INPUT, page, &covered, band(), &options())
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

/// An engine whose frames every `page_frames` test dials in: the function builds the answer afresh
/// on each call, because a reading carries an `Error`, which does not clone. The page count is the
/// readings' own unless the second field lies about it.
struct Framer(
    fn() -> Vec<burrow_engines::redact::frames::PageFrameReading>,
    Option<u64>,
);

impl burrow_engines::PageFrameReader for Framer {
    fn page_frames(
        &self,
        _bytes: &[u8],
        _options: &OpenOptions<'_>,
    ) -> Result<burrow_engines::redact::frames::PageFrames> {
        let readings = (self.0)();
        let pages = self
            .1
            .unwrap_or_else(|| u64::try_from(readings.len()).unwrap());
        Ok(burrow_engines::redact::frames::PageFrames { pages, readings })
    }
}

mod frames {
    use burrow_engines::redact::frames::{FrameRule, MAX_FRAME_SIDE, PageFrameReading};
    use burrow_types::{Error, Result};

    use super::{Framer, options};

    fn frame(width: f64, height: f64) -> PageFrameReading {
        PageFrameReading::Frame { width, height }
    }

    fn refused(rule: FrameRule, token: &str) -> PageFrameReading {
        PageFrameReading::Refused {
            rule,
            error: Error::Unsupported(format!("pdf redaction [{token}]: why")),
        }
    }

    fn run(engine: &Framer) -> Result<Vec<PageFrameReading>> {
        super::super::page_frames(engine, b"%PDF", &options())
    }

    #[test]
    fn an_honest_engine_s_frames_and_refusals_come_through_in_order() {
        // NON-VACUITY: without it every lie below passes for an operation that refuses everything.
        let honest = Framer(
            || {
                vec![
                    frame(612.0, 792.0),
                    refused(FrameRule::UserUnitNotOne, "user-unit-not-one"),
                    frame(MAX_FRAME_SIDE, 1.0),
                ]
            },
            Some(3),
        );
        let readings = run(&honest).expect("an honest engine is not refused");
        assert_eq!(readings.len(), 3);
        assert!(matches!(
            readings[0],
            PageFrameReading::Frame { width, height } if width == 612.0 && height == 792.0
        ));
        assert!(matches!(
            readings[1],
            PageFrameReading::Refused {
                rule: FrameRule::UserUnitNotOne,
                ..
            }
        ));
    }

    /// KILLS: deleting the shape check. Each lie, beside an honest page, is refused as a bug.
    #[test]
    fn an_engine_that_lies_about_a_frame_or_a_rule_is_refused() {
        type Answer = fn() -> Vec<PageFrameReading>;
        let lies: [(&str, Answer); 7] = [
            ("NaN", || vec![frame(612.0, 792.0), frame(f64::NAN, 1.0)]),
            ("infinite", || {
                vec![frame(612.0, 792.0), frame(1.0, f64::INFINITY)]
            }),
            ("zero", || vec![frame(612.0, 792.0), frame(0.0, 1.0)]),
            ("negative", || vec![frame(612.0, 792.0), frame(1.0, -1.0)]),
            ("past the largest side", || {
                vec![frame(612.0, 792.0), frame(MAX_FRAME_SIDE * 2.0, 1.0)]
            }),
            ("a rule its error does not name", || {
                vec![
                    frame(612.0, 792.0),
                    refused(FrameRule::UserUnitNotOne, "page-frame-unreadable"),
                ]
            }),
            ("a refusal that names no rule", || {
                vec![
                    frame(612.0, 792.0),
                    PageFrameReading::Refused {
                        rule: FrameRule::DisplayBox,
                        error: Error::Internal("no rule".to_owned()),
                    },
                ]
            }),
        ];
        for (what, lie) in lies {
            let error = run(&Framer(lie, None)).expect_err(what);
            assert!(
                matches!(error, Error::Internal(ref why) if why.contains("page frames")),
                "{what}: {error:?}"
            );
        }
    }

    /// KILLS: deleting the count check. Two honest frames for a document of three pages, and of
    /// one, are each refused by name.
    #[test]
    fn an_engine_that_reports_one_reading_short_or_one_too_many_is_refused() {
        let two: fn() -> Vec<PageFrameReading> = || vec![frame(612.0, 792.0), frame(612.0, 792.0)];
        for (what, pages) in [("one short", 3), ("one too many", 1)] {
            let error = run(&Framer(two, Some(pages))).expect_err(what);
            assert!(
                matches!(error, Error::Internal(ref why) if why.contains("number of readings")),
                "{what}: {error:?}"
            );
        }
        // THE NEAR-MISS: the same readings, counted right.
        assert_eq!(run(&Framer(two, Some(2))).expect("counted right").len(), 2);
    }
}

/// The input every test hands `run`.
const INPUT: &[u8] = b"%PDF";

#[test]
fn redact_verify_same_bytes() {
    // ADR 0006's R10, as amended 2026-09-27 (#206): the bytes verification examines are PROVEN
    // identical to the bytes that are emitted. This is the test the ADR names, the operation's
    // half: the frame check `verify::output` runs must read the document `page` returns. Compared
    // by CONTENT -- the sha256 of every buffer the read-back opened against the sha256 of the
    // returned document -- which is what the amended rule states (a property, not a mechanism).
    //
    // The fake's documents differ on every `redact_page` call, so a check handed another call's
    // document is observably handed something else.
    //
    // KILLS: handing the check the INPUT (M3), a copy one byte different (M3b), or a second
    // call's document (M4). With the old constant fake, M3 and M4 survived the whole suite.
    let fake = Fake::honest(3);
    let done = run(&fake, 0, &[0]).expect("an honest engine is not rejected");
    let opened = fake.opened.borrow();
    println!(
        "R10 ops half: {} redact_page call(s), {} read-back open(s); expected 1 and 1",
        fake.calls.get(),
        opened.len()
    );
    assert_eq!(fake.calls.get(), 1, "the engine is asked to redact once");
    assert_eq!(opened.len(), 1, "exactly one read-back per redaction");
    assert_eq!(
        opened.first(),
        Some(&digest(&done.document)),
        "the read-back opened something other than the document that was returned"
    );
    assert!(
        !opened.contains(&digest(INPUT)),
        "the read-back opened the input"
    );
}

#[test]
fn redact_verify_same_bytes_is_not_vacuous() {
    // The fake really does return different bytes per call, and differs from the input: without
    // this, the equality above would be a claim about one constant.
    let fake = Fake::honest(3);
    let opts = options();
    let covered: BTreeSet<usize> = [0].into_iter().collect();
    let (first, _) = fake
        .redact_page(INPUT, 0, &covered, band(), &opts)
        .expect("redacts");
    let (second, _) = fake
        .redact_page(INPUT, 0, &covered, band(), &opts)
        .expect("redacts");
    assert_ne!(
        digest(&first),
        digest(&second),
        "the fake's calls must drift"
    );
    assert_ne!(
        digest(&first),
        digest(INPUT),
        "the output must not be the input"
    );
}

#[test]
fn a_document_equal_in_content_passes_by_design() {
    // THE NEAR-MISS TWIN. Two calls returning identical bytes: identity by content cannot tell a
    // second call's document from the first's, and that is the rule as amended -- a benign clone
    // of the same bytes is the same document to whoever receives it. So a second, equal call is
    // not a finding, and the test says so rather than leaving it to be rediscovered as a gap.
    let mut fake = Fake::honest(3);
    fake.same_every_call = true;
    let done = run(&fake, 0, &[0]).expect("an honest engine is not rejected");
    let covered: BTreeSet<usize> = [0].into_iter().collect();
    let (again, _) = fake
        .redact_page(INPUT, 0, &covered, band(), &options())
        .expect("redacts");
    assert_eq!(digest(&again), digest(&done.document));
    assert_eq!(fake.opened.borrow().as_slice(), [digest(&again)]);
}

#[test]
fn a_rejected_redaction_reads_back_once_and_returns_nothing() {
    // A refusal at the frame check still opened exactly the document the engine produced, and
    // nobody receives it.
    let mut fake = Fake::honest(3);
    fake.read_back = Some(vec![0, 0]);
    run(&fake, 0, &[0]).expect_err("two pages came back from three");
    assert_eq!(fake.calls.get(), 1);
    assert_eq!(
        fake.opened.borrow().as_slice(),
        [digest(b"%PDF-1.7\n%call 1\n")]
    );
}
