//! Unit tests for the merge operation's own logic.
//!
//! **Against a fake assembler, deliberately.** Everything here is about what `merge` does
//! *around* the engine — the aggregate ceilings, the order, which input an error is
//! attributed to, and which failures are attributed to no input at all. None of that needs
//! a PDF, and running it without `native-engines` means it runs on every CI job rather than
//! only the one that links the vendored libraries.
//!
//! The engine's own behaviour is covered by `core/burrow-engines/tests/merge.rs`, which
//! does need real files.
//!
//! The fake **accounts for what it is asked to do** — every call is recorded, and the
//! assembly counts its own sources — because a fake that cannot notice being misused is a
//! fake that certifies whatever it is handed. That lesson is from M1 PR 4a-i, whose Rust
//! fake returned a fixed logger handle and was structurally incapable of noticing the
//! per-operation logger leak it was meant to cover.

use std::sync::{Arc, Mutex};

use burrow_engines::{OpenOptions, PageAssembler};
use burrow_types::{Clock, Error, Limits, ManualClock, Password, Stage};

use super::{Input, check_total_input_bytes, merge};

/// What the fake was asked to do, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    Begin { bytes: usize, has_password: bool },
    Append { bytes: usize, has_password: bool },
    Finish { sources: usize },
}

#[derive(Debug, Clone)]
struct Script {
    /// Fail `begin` with this error.
    begin_fails: Option<&'static str>,
    /// Fail `append` on the nth call (0-based) with a malformed error.
    append_fails_at: Option<usize>,
    /// Pages each document reports.
    pages_each: u64,
    /// Append this input (1-based, counting the first document as input 1) contributing
    /// nothing, while every other input contributes normally.
    ///
    /// A fake whose inputs ALL contribute nothing makes `position()` return 0 every time, so
    /// "the message names which input" is satisfied by a verifier that hardcodes `input 1`.
    /// Found by code review. This expresses the case a person actually hits: one file among
    /// several silently dropped.
    drops_input: Option<usize>,
    /// What the output reads back as, in pages. `None` means "whatever was assembled".
    ///
    /// THE VERIFIER'S ONLY LEVER (ADR 0022). A real engine cannot be asked to write a document
    /// with the wrong number of pages in it, so the check that refuses one can only be tested
    /// against a fake that lies on the way back. That is what `OutputReader` being a separate
    /// trait buys.
    reads_back_as: Option<u64>,
    /// Milliseconds the fake puts on the clock per call, so time passes where work does.
    ///
    /// A `ManualClock` that nobody advances never expires, so a deadline test written
    /// without this passes for the wrong reason — it asserts a timeout that could not have
    /// happened. The engine is where the time goes, so the engine is what moves the clock.
    ms_per_call: u64,
    /// The clock to advance. `None` when a test does not care about time.
    clock: Option<Arc<ManualClock>>,
}

/// A one-page document per input, and nothing else scripted.
///
/// **Not `#[derive(Default)]`**, which would make `pages_each` zero — and a fake whose every
/// input has no pages is refused by the verifier under `Expected::Merged`'s zero-page rule,
/// for a reason that has nothing to do with the test doing the asking. One page is the
/// smallest document a real merge can be handed.
impl Default for Script {
    fn default() -> Self {
        Self {
            begin_fails: None,
            append_fails_at: None,
            pages_each: 1,
            drops_input: None,
            reads_back_as: None,
            ms_per_call: 0,
            clock: None,
        }
    }
}

struct Fake {
    script: Script,
    /// Whether this instance came from `OutputReader::fresh`.
    ///
    /// THE FRESHNESS IS ASSERTED, NOT ASSUMED. A fake that reads the emitted document back
    /// identically whoever asks leaves ADR 0022's first requirement untested: swap
    /// `engine.fresh()` for `engine` in `verify::output` and every test still passes. Found by
    /// code review, with that mutation run. So the instance that WROTE the bytes reports
    /// nonsense when asked to read them -- which is what "a corrupted instance can agree with
    /// itself" looks like from outside.
    fresh: bool,
    calls: Arc<Mutex<Vec<Call>>>,
    /// How many times `OutputReader::fresh` was called, shared across every instance.
    ///
    /// ADR 0022's first requirement is that the witness is not the instance that produced the
    /// bytes, and nothing else here can see the difference: a verifier reading through
    /// `&self` would agree with every honest fake in this file.
    freshes: Arc<Mutex<usize>>,
}

struct FakeAssembly {
    pages: u64,
    /// Bytes of every document, in the order they arrived. The output is derived from
    /// this, so an assembly that dropped or reordered one produces different bytes.
    seen: Vec<usize>,
}

impl Fake {
    fn new(script: Script) -> (Self, Arc<Mutex<Vec<Call>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                script,
                fresh: false,
                calls: Arc::clone(&calls),
                freshes: Arc::new(Mutex::new(0)),
            },
            calls,
        )
    }

    fn record(&self, call: Call) {
        self.calls.lock().expect("calls mutex").push(call);
        if let Some(clock) = &self.script.clock {
            clock.advance(self.script.ms_per_call);
        }
    }
}

impl PageAssembler for Fake {
    type Assembly = FakeAssembly;

    fn name(&self) -> &'static str {
        "fake"
    }

    fn begin(
        &self,
        first: Box<[u8]>,
        options: &OpenOptions<'_>,
    ) -> burrow_types::Result<Self::Assembly> {
        self.record(Call::Begin {
            bytes: first.len(),
            has_password: options.password.is_some(),
        });
        if let Some(why) = self.script.begin_fails {
            return Err(Error::Malformed(why.to_owned()));
        }
        Ok(FakeAssembly {
            pages: self.script.pages_each,
            seen: vec![first.len()],
        })
    }

    fn pages(&self, assembly: &Self::Assembly) -> burrow_types::Result<u64> {
        Ok(assembly.pages)
    }

    fn append(
        &self,
        assembly: &mut Self::Assembly,
        next: Box<[u8]>,
        options: &OpenOptions<'_>,
    ) -> burrow_types::Result<u64> {
        let nth = assembly.seen.len() - 1;
        self.record(Call::Append {
            bytes: next.len(),
            has_password: options.password.is_some(),
        });
        if self.script.append_fails_at == Some(nth) {
            return Err(Error::Malformed("fake: refused".to_owned()));
        }
        assembly.seen.push(next.len());
        // `nth` is 0 for the first APPEND, which is input 2 -- the first document went through
        // `begin`. So the 1-based input number of this append is `nth + 2`.
        let contributed = if self.script.drops_input == Some(nth + 2) {
            0
        } else {
            self.script.pages_each
        };
        assembly.pages += contributed;
        Ok(contributed)
    }

    fn finish(&self, assembly: Self::Assembly) -> burrow_types::Result<Vec<u8>> {
        self.record(Call::Finish {
            sources: assembly.seen.len(),
        });
        // The "document" is the sequence of input lengths, so order is observable, followed
        // by the page total -- which is what makes the fake's READER honest about the document
        // its WRITER built. Deriving the count as `inputs * pages_each` instead made every
        // dropped-input case fail the page-count check first, so the "which input" message was
        // unreachable. Found by code review.
        let mut bytes: Vec<u8> = assembly
            .seen
            .iter()
            .map(|n| u8::try_from(*n % 251).unwrap_or(0))
            .collect();
        bytes.push(u8::try_from(assembly.pages % 251).unwrap_or(0));
        Ok(bytes)
    }
}

/// Reading the output back (ADR 0022).
///
/// The fake's "document" is one byte per input plus the page total, so its page count is
/// derivable from the bytes -- which means the honest reading and the lying one are both
/// expressible, and the lie is what tests the refusal.
impl burrow_engines::OutputReader for Fake {
    type Read = u64;

    fn fresh(&self) -> Self {
        *self.freshes.lock().expect("freshes mutex") += 1;
        // Shares the call log, so a test can still see what the verifier did, and nothing
        // else -- which is all `fresh` means for a fake with no document state.
        Self {
            script: self.script.clone(),
            fresh: true,
            calls: Arc::clone(&self.calls),
            freshes: Arc::clone(&self.freshes),
        }
    }

    fn open_output(
        &self,
        bytes: &[u8],
        _options: &OpenOptions<'_>,
    ) -> burrow_types::Result<Self::Read> {
        if !self.fresh {
            // The writing instance is not a witness. See `fresh`.
            return Ok(0);
        }
        // The last byte is the page total the writer put there; see `finish`.
        let assembled = u64::from(bytes.last().copied().unwrap_or(0));
        Ok(self.script.reads_back_as.unwrap_or(assembled))
    }

    fn page_count(&self, read: &Self::Read) -> burrow_types::Result<u64> {
        Ok(*read)
    }

    fn rotations(
        &self,
        read: &Self::Read,
        _options: &OpenOptions<'_>,
        _deadline: &burrow_types::Deadline,
    ) -> burrow_types::Result<Vec<i64>> {
        Ok(vec![0; usize::try_from(*read).unwrap_or(0)])
    }
}

fn options(limits: Limits) -> (OpenOptions<'static>, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::new(0));
    let opts = OpenOptions::new(limits, Arc::clone(&clock) as Arc<dyn burrow_types::Clock>);
    (opts, clock)
}

fn input(len: usize) -> Input<'static> {
    Input::new(vec![0u8; len].into_boxed_slice())
}

#[test]
fn merges_inputs_in_order() {
    let (fake, calls) = Fake::new(Script {
        pages_each: 2,
        ..Script::default()
    });
    let (opts, _clock) = options(Limits::DEFAULT);

    let out = merge(&fake, vec![input(10), input(20), input(30)], &opts).expect("merge");

    // The trailing byte is the page total the fake's writer records for its reader; see
    // `finish`. The prefix is what says the inputs arrived in order.
    assert_eq!(out, vec![10, 20, 30, 6], "the inputs arrived out of order");
    assert_eq!(
        *calls.lock().expect("calls"),
        vec![
            Call::Begin {
                bytes: 10,
                has_password: false
            },
            Call::Append {
                bytes: 20,
                has_password: false
            },
            Call::Append {
                bytes: 30,
                has_password: false
            },
            Call::Finish { sources: 3 },
        ]
    );
}

#[test]
fn merging_one_document_is_the_identity() {
    // The ROADMAP invariant, at the operation level: one input means `begin` and `finish`
    // and nothing in between.
    let (fake, calls) = Fake::new(Script {
        pages_each: 7,
        ..Script::default()
    });
    let (opts, _clock) = options(Limits::DEFAULT);

    let out = merge(&fake, vec![input(42)], &opts).expect("merge");

    // One input byte, then the page total the fake's writer records for its reader.
    assert_eq!(out, vec![42, 7]);
    assert_eq!(
        *calls.lock().expect("calls"),
        vec![
            Call::Begin {
                bytes: 42,
                has_password: false
            },
            Call::Finish { sources: 1 },
        ]
    );
}

#[test]
fn no_inputs_is_an_invalid_argument_not_an_empty_document() {
    let (fake, calls) = Fake::new(Script::default());
    let (opts, _clock) = options(Limits::DEFAULT);

    let err = merge(&fake, Vec::new(), &opts).expect_err("empty input list must fail");
    assert!(matches!(err, Error::InvalidArgument(_)), "got {err:?}");
    assert!(
        calls.lock().expect("calls").is_empty(),
        "the engine must not be touched when there is nothing to merge"
    );
}

#[test]
fn each_input_carries_its_own_password() {
    // Not one password for the set: five documents can have five.
    let secret = Password::new(b"open sesame");
    let (fake, calls) = Fake::new(Script::default());
    let (opts, _clock) = options(Limits::DEFAULT);

    let inputs = vec![
        Input::new(vec![0u8; 4].into_boxed_slice()),
        Input::with_password(vec![0u8; 5].into_boxed_slice(), &secret),
    ];
    merge(&fake, inputs, &opts).expect("merge");

    let calls = calls.lock().expect("calls");
    assert_eq!(
        calls[0],
        Call::Begin {
            bytes: 4,
            has_password: false
        }
    );
    assert_eq!(
        calls[1],
        Call::Append {
            bytes: 5,
            has_password: true
        },
        "the second input's password did not reach the engine, or the first input got one"
    );
}

#[test]
fn the_total_input_size_is_checked_before_any_document_is_opened() {
    // THE HOLE THIS CLOSES: each input is under the ceiling and the set is not. Without an
    // aggregate check a caller who set 100 bytes would get 150 through.
    let (fake, calls) = Fake::new(Script::default());
    let (opts, _clock) = options(Limits::with(|l| l.max_input_bytes = 100));

    let err = merge(&fake, vec![input(60), input(60), input(30)], &opts)
        .expect_err("150 bytes against a 100-byte ceiling must fail");

    match err {
        Error::LimitExceeded {
            limit,
            stage,
            requested,
            allowed,
        } => {
            assert_eq!(limit, "max_input_bytes");
            assert_eq!(stage, Stage::InputSize);
            assert_eq!(requested, 150, "the sum, not the largest input");
            assert_eq!(allowed, 100);
        }
        other => panic!("expected LimitExceeded, got {other:?}"),
    }

    assert!(
        calls.lock().expect("calls").is_empty(),
        "nothing may be opened before the aggregate size check -- non-negotiable #3 asks \
         for the check to come before anything is allocated"
    );
}

#[test]
fn the_boundary_of_the_total_is_allowed_and_one_past_it_is_not() {
    let (fake, _) = Fake::new(Script::default());
    let (opts, _clock) = options(Limits::with(|l| l.max_input_bytes = 100));
    merge(&fake, vec![input(50), input(50)], &opts).expect("exactly at the ceiling is allowed");

    let (fake, _) = Fake::new(Script::default());
    let err = merge(&fake, vec![input(50), input(51)], &opts).expect_err("one past must fail");
    assert!(matches!(err, Error::LimitExceeded { .. }), "got {err:?}");
}

#[test]
fn a_failing_input_names_its_own_index() {
    let (fake, _) = Fake::new(Script {
        append_fails_at: Some(1),
        ..Script::default()
    });
    let (opts, _clock) = options(Limits::DEFAULT);

    let err = merge(&fake, vec![input(1), input(2), input(3)], &opts).expect_err("must fail");
    match err {
        Error::InputFailed { index, source } => {
            assert_eq!(index, 2, "the third input failed, so the index is 2");
            assert!(matches!(*source, Error::Malformed(_)), "got {source:?}");
        }
        other => panic!("expected InputFailed, got {other:?}"),
    }
}

#[test]
fn a_failing_first_input_is_index_zero() {
    let (fake, _) = Fake::new(Script {
        begin_fails: Some("fake: refused"),
        ..Script::default()
    });
    let (opts, _clock) = options(Limits::DEFAULT);

    let err = merge(&fake, vec![input(1), input(2)], &opts).expect_err("must fail");
    assert!(
        matches!(err, Error::InputFailed { index: 0, .. }),
        "got {err:?}"
    );
}

#[test]
fn one_failing_input_produces_no_output_at_all() {
    // ADR 0017 §2. The test is that `finish` is never reached: a partial merge would show
    // up here as a `Finish` call with fewer sources than inputs.
    let (fake, calls) = Fake::new(Script {
        append_fails_at: Some(1),
        ..Script::default()
    });
    let (opts, _clock) = options(Limits::DEFAULT);

    let _ = merge(&fake, vec![input(1), input(2), input(3)], &opts).expect_err("must fail");

    let calls = calls.lock().expect("calls");
    assert!(
        !calls.iter().any(|c| matches!(c, Call::Finish { .. })),
        "the assembly was finished despite an input failing, which is how a silently short \
         document ships: {calls:?}"
    );
}

#[test]
fn an_input_failure_is_not_wrapped_twice() {
    // An engine that already attributes a failure keeps its own index. "input 2 of input 0"
    // helps nobody, and the inner number is the one that means something.
    let inner = Error::InputFailed {
        index: 9,
        source: Box::new(Error::Malformed("inner".to_owned())),
    };
    match super::at(3, inner) {
        Error::InputFailed { index, .. } => assert_eq!(index, 9),
        other => panic!("expected InputFailed, got {other:?}"),
    }
}

#[test]
fn running_out_of_time_is_not_blamed_on_a_file() {
    // The inversion this project has now fixed three times: PR 2 natively, ADR 0015 §2 on
    // the web, and here. A deadline failure must NOT arrive as `InputFailed`, because that
    // tells someone to remove a document that is perfectly fine.
    let clock = Arc::new(ManualClock::new(0));
    // A fake clock, not a sleep: a timeout test that really waits is a test nobody runs.
    // The fake advances it on every call, so the budget is spent by doing work.
    let (fake, calls) = Fake::new(Script {
        ms_per_call: 5_000,
        clock: Some(Arc::clone(&clock)),
        ..Script::default()
    });
    let limits = Limits::with(|l| l.max_duration_ms = 10);
    let opts = OpenOptions::new(limits, Arc::clone(&clock) as Arc<dyn burrow_types::Clock>);

    let err = merge(&fake, vec![input(1), input(2)], &opts).expect_err("must time out");

    // The mutation guard: assert the clock actually moved. A `ms_per_call` that silently
    // stayed zero would make this test pass on a deadline that never expired.
    assert!(
        clock.now_ms() >= 5_000,
        "the fake did not advance the clock, so nothing here tested a deadline"
    );
    assert!(
        !calls.lock().expect("calls").is_empty(),
        "the engine was never called, so no time could have passed"
    );
    match err {
        Error::LimitExceeded { limit, stage, .. } => {
            assert_eq!(limit, "max_duration_ms");
            assert_eq!(stage, Stage::Deadline);
        }
        Error::InputFailed { index, source } => panic!(
            "the deadline was blamed on input {index} ({source:?}) -- running out of time is \
             the operation's outcome, not a file's"
        ),
        other => panic!("expected LimitExceeded, got {other:?}"),
    }
}

#[test]
fn the_debug_of_an_input_carries_no_bytes() {
    // Never log file content, including through a derive.
    let secret = Password::new(b"hunter2");
    let rendered = format!(
        "{:?}",
        Input::with_password(
            b"%PDF-1.7 secret content".to_vec().into_boxed_slice(),
            &secret
        )
    );
    assert!(!rendered.contains("secret"), "{rendered}");
    assert!(!rendered.contains("hunter2"), "{rendered}");
    assert!(rendered.contains("has_password: true"), "{rendered}");
}

// ---------------------------------------------------------------- the pre-flight

// `check_total_input_bytes` is the aggregate half of `max_input_bytes`, pulled out so a
// caller can apply it before the bytes exist (issue #51). What has to hold is not that it
// is correct in isolation -- it is four lines -- but that it and `merge` cannot disagree.

#[test]
fn the_preflight_refuses_exactly_what_merge_refuses() {
    // THE PROPERTY, exercised across the boundary rather than asserted once. For each set of
    // sizes, the pre-flight's verdict and the real operation's verdict must match, including
    // the numbers: a pre-flight that refused something `merge` would accept is a tool that
    // turns away work it can do, and one that accepted something `merge` refuses puts the
    // failure back where it was.
    let ceiling = 1_000u64;
    let limits = Limits::with(|l| l.max_input_bytes = ceiling);

    // Both sides of the boundary, and the boundary itself.
    for sizes in [
        vec![1usize],
        vec![500, 499],
        vec![500, 500],
        vec![500, 501],
        vec![400, 400, 400],
        vec![1000],
        vec![1001],
    ] {
        let (fake, _calls) = Fake::new(Script {
            pages_each: 1,
            ..Script::default()
        });
        let (opts, _clock) = options(limits);

        let preflight = check_total_input_bytes(
            sizes.iter().map(|n| u64::try_from(*n).expect("size fits")),
            &limits,
        );
        let operation = merge(&fake, sizes.iter().map(|n| input(*n)).collect(), &opts);

        match (&preflight, &operation) {
            (Ok(total), Ok(_)) => {
                let expected: u64 = sizes
                    .iter()
                    .map(|n| u64::try_from(*n).expect("size fits"))
                    .sum();
                assert_eq!(
                    *total, expected,
                    "{sizes:?}: the pre-flight's total is wrong"
                );
            }
            (
                Err(Error::LimitExceeded {
                    limit: a,
                    stage: sa,
                    requested: ra,
                    allowed: aa,
                }),
                Err(Error::LimitExceeded {
                    limit: b,
                    stage: sb,
                    requested: rb,
                    allowed: ab,
                }),
            ) => {
                assert_eq!(
                    (a, sa, ra, aa),
                    (b, sb, rb, ab),
                    "{sizes:?}: different refusals"
                );
            }
            (p, o) => panic!(
                "{sizes:?}: the pre-flight and the operation disagree -- pre-flight {p:?}, \
                 operation {o:?}"
            ),
        }
    }
}

#[test]
fn the_preflight_refuses_before_anything_is_opened() {
    // The point of it. An over-ceiling set must be refused without the engine being touched
    // -- on the web that is the difference between a typed refusal and a tab that ran out of
    // memory inside the transport.
    let limits = Limits::with(|l| l.max_input_bytes = 10);
    let (fake, calls) = Fake::new(Script::default());
    let (opts, _clock) = options(limits);

    let err = merge(&fake, vec![input(6), input(6)], &opts).expect_err("must refuse");
    assert!(matches!(err, Error::LimitExceeded { .. }), "{err:?}");
    assert!(
        calls.lock().expect("calls").is_empty(),
        "the engine was called for a set that was already over the ceiling"
    );
}

#[test]
fn an_empty_set_totals_zero_rather_than_failing() {
    // `merge` refuses an empty list separately, with `InvalidArgument`, and it should stay
    // that way -- so the pre-flight must not invent a different answer for the same input.
    // A caller asking "is nothing too big" gets `no`.
    let total = check_total_input_bytes([], &Limits::DEFAULT).expect("empty is not over a limit");
    assert_eq!(total, 0);
}

#[test]
fn a_total_that_overflows_is_internal_rather_than_a_limit() {
    // Sizes arrive from a caller. Two near-maximum ones must not wrap around into a small
    // total that passes the ceiling -- the silent-truncation class `core/CLAUDE.md` denies
    // casts for, arriving as arithmetic instead.
    let err = check_total_input_bytes([u64::MAX, 1], &Limits::DEFAULT).expect_err("must fail");
    assert!(
        matches!(err, Error::Internal(_)),
        "an overflowing total must not be reported as a limit: {err:?}"
    );
}

// --- ADR 0022: the output is read back, through an engine that did not write it -----------

#[test]
fn the_output_is_read_back_through_a_fresh_engine() {
    // One `fresh` per operation. A verifier that read through `&self` would leave this at
    // zero and every other test in this file would still pass.
    let (fake, _) = Fake::new(Script::default());
    let (opts, _clock) = options(Limits::DEFAULT);
    merge(&fake, vec![input(4), input(5)], &opts).expect("merge");
    assert_eq!(*fake.freshes.lock().expect("freshes"), 1);
}

#[test]
fn an_output_with_the_wrong_page_count_is_refused() {
    // The assembly says four pages went in; the reader says three came out. No `PageAssembler`
    // call failed, which is exactly why this needs a separate reader to catch.
    let (fake, _) = Fake::new(Script {
        pages_each: 2,
        reads_back_as: Some(3),
        ..Script::default()
    });
    let (opts, _clock) = options(Limits::DEFAULT);
    let err = merge(&fake, vec![input(4), input(5)], &opts).expect_err("must be refused");
    assert!(matches!(err, Error::OutputRejected(_)), "got {err:?}");
}

#[test]
fn the_message_names_which_input_contributed_nothing() {
    // NOT INPUT 1. Every other test of this refusal has every input contributing nothing, so
    // `position()` returns 0 and a verifier that hardcoded "input 1" would pass them all.
    // Here the second of three inputs is the one that was dropped.
    let (fake, _) = Fake::new(Script {
        pages_each: 2,
        drops_input: Some(2),
        ..Script::default()
    });
    let (opts, _clock) = options(Limits::DEFAULT);
    let err = merge(&fake, vec![input(4), input(5), input(6)], &opts).expect_err("must be refused");
    match err {
        Error::OutputRejected(why) => assert!(why.contains("input 2"), "got {why:?}"),
        other => panic!("got {other:?}"),
    }
}

#[test]
fn an_input_that_contributed_nothing_is_refused_and_named() {
    // #62's shape at the merge boundary: a co-input's pages absent from an output that is
    // otherwise a valid PDF. The message names WHICH input, one-based, because the caller's
    // next move is to remove a file from a list.
    let (fake, _) = Fake::new(Script {
        pages_each: 0,
        ..Script::default()
    });
    let (opts, _clock) = options(Limits::DEFAULT);
    let err = merge(&fake, vec![input(4), input(5)], &opts).expect_err("must be refused");
    match err {
        Error::OutputRejected(why) => assert!(why.contains("input 1"), "got {why:?}"),
        other => panic!("got {other:?}"),
    }
}

#[test]
fn an_honest_engine_is_not_refused() {
    // The other half of the lever: without this the two refusals above are satisfied by a
    // verifier that refuses everything.
    let (fake, _) = Fake::new(Script {
        pages_each: 2,
        ..Script::default()
    });
    let (opts, _clock) = options(Limits::DEFAULT);
    merge(&fake, vec![input(4), input(5)], &opts).expect("an honest read-back must pass");
}
