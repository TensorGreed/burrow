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

use super::{Input, merge};

/// What the fake was asked to do, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    Begin { bytes: usize, has_password: bool },
    Append { bytes: usize, has_password: bool },
    Finish { sources: usize },
}

#[derive(Debug, Default)]
struct Script {
    /// Fail `begin` with this error.
    begin_fails: Option<&'static str>,
    /// Fail `append` on the nth call (0-based) with a malformed error.
    append_fails_at: Option<usize>,
    /// Pages each document reports.
    pages_each: u64,
    /// Milliseconds the fake puts on the clock per call, so time passes where work does.
    ///
    /// A `ManualClock` that nobody advances never expires, so a deadline test written
    /// without this passes for the wrong reason — it asserts a timeout that could not have
    /// happened. The engine is where the time goes, so the engine is what moves the clock.
    ms_per_call: u64,
    /// The clock to advance. `None` when a test does not care about time.
    clock: Option<Arc<ManualClock>>,
}

struct Fake {
    script: Script,
    calls: Arc<Mutex<Vec<Call>>>,
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
                calls: Arc::clone(&calls),
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
        assembly.pages += self.script.pages_each;
        Ok(self.script.pages_each)
    }

    fn finish(&self, assembly: Self::Assembly) -> burrow_types::Result<Vec<u8>> {
        self.record(Call::Finish {
            sources: assembly.seen.len(),
        });
        // The "document" is the sequence of input lengths, so order is observable.
        Ok(assembly
            .seen
            .iter()
            .map(|n| u8::try_from(*n % 251).unwrap_or(0))
            .collect())
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

    assert_eq!(out, vec![10, 20, 30], "the inputs arrived out of order");
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

    assert_eq!(out, vec![42]);
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
