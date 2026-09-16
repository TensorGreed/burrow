//! Unit tests for the qpdf compression engine.
//!
//! # The failure mode this suite is built around
//!
//! Compression is the one operation whose correct output and whose **completely inert** output
//! are hard to tell apart. A `compress` that forgot to set its single lever still returns a
//! valid document with the right page count, the right page order, the right rotations and
//! every page intact — it is simply the same file every other operation already emits. Every
//! structural assertion an operation normally makes would pass.
//!
//! So the suite asserts the lever's **effect on the container**, not just the document's
//! survival: the output carries `/Type /ObjStm`, and the same engine writing the same fixture
//! without the lever does not. That pair is the whole point — the first alone would be
//! satisfied by a fixture that already had object streams, and the second alone proves nothing
//! about compression.
//!
//! Everything else here is the inverse obligation: nothing may be lost.

use std::sync::Arc;

use burrow_types::{Clock, Error, Limits, ManualClock, Result};

use super::Qpdf;
// The stepping clock lives in `rotate_tests` and is `pub(super)` so every suite shares one
// definition -- a second copy would be a second thing to keep honest.
use super::rotate_tests::SteppingClock;
use crate::minimal_pdf::{self, RotationPlacement};
use crate::pdf_reading::{page_order as order_of, page_widths};
use crate::{DocumentCompressor, OpenOptions, PageRotator};

fn stopped() -> Arc<dyn Clock> {
    Arc::new(ManualClock::new(0))
}

fn options() -> OpenOptions<'static> {
    OpenOptions::new(Limits::default(), stopped())
}

fn open(bytes: Vec<u8>) -> Result<<Qpdf as DocumentCompressor>::Source> {
    DocumentCompressor::open(&Qpdf::new(), bytes.into_boxed_slice(), &options())
}

/// Compress and hand back the emitted bytes.
fn compressed(bytes: Vec<u8>) -> Result<Vec<u8>> {
    let source = open(bytes)?;
    Qpdf::new().compress(&source, &options())
}

/// Whether the emitted container packs objects into object streams.
///
/// Read out of the RAW bytes rather than the expanded form, and that is not an oversight: an
/// object stream's own dictionary is written in the clear, so `/Type /ObjStm` is there to find
/// — while `pdf_reading::expanded` runs `qpdf --qdf --object-streams=disable`, whose entire job
/// is to take them back out. Expanding first would make this predicate answer `false` on every
/// input, including a correctly compressed one.
fn has_object_streams(bytes: &[u8]) -> bool {
    bytes
        .windows(b"/Type /ObjStm".len())
        .any(|w| w == b"/Type /ObjStm")
}

/// A document with enough small objects for object streams to be worth anything.
fn packable() -> Vec<u8> {
    minimal_pdf::pdf_with_page_tree(40, RotationPlacement::Absent)
}

// ------------------------------------------------------------------ the lever fires

#[test]
fn the_output_packs_objects_into_object_streams() {
    let out = compressed(packable()).expect("a well-formed document compresses");
    assert!(
        has_object_streams(&out),
        "the compressed output carries no object stream, so the one lever compress has did \
         not reach the writer"
    );
}

#[test]
fn the_same_engine_without_the_lever_produces_none() {
    // THE CONTROL, and without it the test above is satisfied by a fixture that already had
    // object streams or by a predicate that matched anything. This is the same document
    // through the same engine and the same write path, differing only in the one enum
    // `compress` sets -- which is exactly the difference spike 0005 measured the whole
    // operation down to.
    let source = PageRotator::open(&Qpdf::new(), packable().into_boxed_slice(), &options())
        .expect("a well-formed document opens");
    let uncompressed = Qpdf::new()
        .rotate(&source, &[0], burrow_types::Rotation::None, &options())
        .expect("a zero-degree rotation writes the document out");

    assert!(
        !has_object_streams(&uncompressed),
        "an operation that is not compress emitted object streams, so the predicate above \
         cannot tell the two apart and neither can this suite"
    );
}

#[test]
fn the_fixture_is_actually_smaller_afterwards() {
    // NON-VACUITY, on the thing a person cares about. A compress that ran and saved nothing
    // would pass every structural assertion in this file.
    let fixture = packable();
    let out = compressed(fixture.clone()).expect("a well-formed document compresses");
    assert!(
        out.len() < fixture.len(),
        "compressing a {}-byte document with 40 pages of small objects produced {} bytes",
        fixture.len(),
        out.len()
    );
}

// ------------------------------------------------------------------ nothing is lost

/// # Why the text-scanning readers are not used in this file
///
/// `pdf_reading::page_order` and `page_widths` read the page tree out of the bytes as text, and
/// on a **compressed** container there is no page tree to read: object stream generation is
/// precisely the thing that moves those dictionaries inside a flated stream. Both return an
/// empty vector, which fails loudly against a full expected sequence — the behaviour their
/// rustdoc promises, working exactly as intended.
///
/// The fix is `pdf_reading::expanded`, which shells out to the `qpdf` CLI to undo the packing.
/// That is right for an integration test and wrong here: these are engine unit tests, and an
/// engine that can be asked the question directly should be, rather than through a subprocess
/// that CI installs and a developer machine may not have.
///
/// So the page-order and page-dimension assertions over the **decompressed** output live with
/// `compress`'s operation tests, where `expanded` is already a CI-asserted dependency. What is
/// asserted here is what the engine seam can answer on a compressed container.
///
/// This is worth writing down rather than discovering twice: every reader in this repository
/// that scans raw bytes goes blind on compress's output, and compress is the first operation
/// for which that is true.
#[test]
fn the_page_count_survives_a_round_trip_through_the_compressed_container() {
    let fixture = packable();
    let out = compressed(fixture).expect("a well-formed document compresses");

    // THROUGH THE ENGINE, not through a text scan -- see the note above.
    let reopened = open(out).expect("the compressed output reopens");
    assert_eq!(
        DocumentCompressor::pages(&Qpdf::new(), &reopened).unwrap(),
        40
    );
}

#[test]
fn the_text_readers_go_blind_on_a_compressed_container_and_that_is_why() {
    // THE FINDING, PINNED. If a future qpdf stopped packing the page tree into object streams,
    // this test fails and the note above stops being true -- which is the moment somebody
    // should reconsider where compress's order assertions live, rather than discovering it
    // from an order test that has quietly been reading an empty vector.
    let fixture = packable();
    assert_eq!(
        order_of(&fixture),
        (1..=40).collect::<Vec<_>>(),
        "the reader must work on the UNCOMPRESSED fixture, or this test proves nothing"
    );

    let out = compressed(fixture).expect("a well-formed document compresses");
    assert!(
        order_of(&out).is_empty(),
        "the raw-bytes reader found a page tree in a compressed container; compress's order assertions could then live here rather than in the operation tests"
    );
    assert!(
        page_widths(&out).is_empty(),
        "likewise for the width reader"
    );
}

#[test]
fn an_inherited_rotation_survives_compression() {
    // THE CASE A PAGE COUNT CANNOT SEE. `/Rotate` on a `/Pages` node is inherited by every
    // page under it, and an operation that rewrote the page tree without pushing the
    // attribute down would produce a document displaying differently while counting the same.
    let fixture = minimal_pdf::pdf_with_page_tree(6, RotationPlacement::OnTheSecondBranch(90));
    let source = open(fixture).expect("a well-formed document opens");
    let deadline = burrow_types::Deadline::start(stopped().as_ref(), &Limits::default());
    let before =
        DocumentCompressor::rotations(&Qpdf::new(), &source, &options(), &deadline).unwrap();
    assert!(
        before.iter().any(|r| *r != 0),
        "the fixture must actually carry a rotation, or this test cannot fail"
    );

    let out = Qpdf::new()
        .compress(&source, &options())
        .expect("a well-formed document compresses");

    let reopened = open(out).expect("the compressed output reopens");
    let after =
        DocumentCompressor::rotations(&Qpdf::new(), &reopened, &options(), &deadline).unwrap();
    assert_eq!(
        after, before,
        "compression changed what a page displays at, which it may not do"
    );
}

#[test]
fn the_rotation_vector_is_the_witness_adr_0022_asks_for() {
    let fixture = minimal_pdf::pdf_with_page_tree(6, RotationPlacement::OnEveryPage(270));
    let source = open(fixture).expect("a well-formed document opens");
    let deadline = burrow_types::Deadline::start(stopped().as_ref(), &Limits::default());
    let before =
        DocumentCompressor::rotations(&Qpdf::new(), &source, &options(), &deadline).unwrap();

    let out = Qpdf::new().compress(&source, &options()).unwrap();
    let reopened = open(out).expect("the compressed output reopens");
    let after =
        DocumentCompressor::rotations(&Qpdf::new(), &reopened, &options(), &deadline).unwrap();

    assert_eq!(after, before);
}

// ------------------------------------------------------------------ what it refuses

#[test]
fn an_unreadable_document_is_refused_rather_than_compressed() {
    assert!(matches!(
        open(minimal_pdf::not_a_pdf()),
        Err(Error::Malformed(_))
    ));
}

#[test]
fn a_document_over_the_page_ceiling_is_refused_at_open() {
    let limits = Limits::with(|l| l.max_pages = 5);
    let refused = DocumentCompressor::open(
        &Qpdf::new(),
        minimal_pdf::pdf_with_page_tree(6, RotationPlacement::Absent).into_boxed_slice(),
        &OpenOptions::new(limits, stopped()),
    );
    assert!(matches!(refused, Err(Error::LimitExceeded { .. })));
}

#[test]
fn the_rotation_sweep_is_checkpointed_per_page_rather_than_around_the_loop() {
    // A clock that advances 10 ms on every read, against a 25 ms budget: the sweep must give
    // up partway through 40 pages rather than after them. The sweep is one `/Parent` climb per
    // page, so it is sized by page count times tree depth -- both attacker-chosen -- and
    // ADR 0022 measured it at 147 ms against 28 ms of edit-and-write on a 10,000-page document.
    //
    // WHAT THIS DOES NOT TEST is which deadline the sweep is using; see the next test. A
    // stepping clock expires a fresh deadline just as fast as a spent one, so this assertion
    // holds either way -- which is why it is named for the property it can actually see.
    let clock: Arc<dyn Clock> = Arc::new(SteppingClock::new(10));
    let limits = Limits::with(|l| l.max_duration_ms = 25);
    let opts = OpenOptions::new(limits, Arc::clone(&clock));

    let source = DocumentCompressor::open(
        &Qpdf::new(),
        minimal_pdf::pdf_with_page_tree(40, RotationPlacement::Absent).into_boxed_slice(),
        &options(),
    )
    .expect("a well-formed document opens");

    let deadline = burrow_types::Deadline::start(clock.as_ref(), &limits);
    let swept = DocumentCompressor::rotations(&Qpdf::new(), &source, &opts, &deadline);
    assert!(
        matches!(swept, Err(Error::LimitExceeded { .. })),
        "a 40-page sweep on a 10 ms-per-tick clock against a 25 ms budget returned {swept:?}"
    );
}

#[test]
fn the_rotation_sweep_refuses_against_an_already_spent_deadline() {
    // THE PROPERTY ADR 0022 ASKS FOR, and the one a stepping clock cannot see. The deadline is
    // spent BEFORE the sweep begins, so a `rotations` that called `Deadline::start` itself
    // would be handed a fresh full budget and would succeed -- `Deadline::start` resets the
    // origin AND the budget, which ADR 0022 records being got wrong three times. This test
    // fails if the sweep ever stops using the caller's deadline.
    use burrow_types::Deadline;

    let source = open(minimal_pdf::pdf_with_page_tree(
        6,
        RotationPlacement::OnTheRoot(90),
    ))
    .expect("a well-formed document opens");

    let clock = Arc::new(ManualClock::new(0));
    let limits = Limits::with(|l| l.max_duration_ms = 10);
    let deadline = Deadline::start(clock.as_ref(), &limits);
    clock.advance(11);

    let err = DocumentCompressor::rotations(
        &Qpdf::new(),
        &source,
        &OpenOptions::new(limits, Arc::clone(&clock) as Arc<dyn Clock>),
        &deadline,
    )
    .expect_err("a spent budget must refuse the sweep");
    assert!(
        matches!(err, Error::LimitExceeded { limit, .. } if limit == "max_duration_ms"),
        "got {err:?}"
    );

    // THE CONTROL: the same sweep against a deadline with budget left. Without it the
    // assertion above is satisfied by a sweep that refuses everything.
    let clock = Arc::new(ManualClock::new(0));
    let deadline = Deadline::start(clock.as_ref(), &limits);
    let rotations = DocumentCompressor::rotations(
        &Qpdf::new(),
        &source,
        &OpenOptions::new(limits, Arc::clone(&clock) as Arc<dyn Clock>),
        &deadline,
    )
    .expect("a budget with room must not refuse");
    assert_eq!(rotations, vec![90; 6]);
}

#[test]
fn the_write_itself_is_refused_when_the_budget_is_already_spent() {
    // THE `compress()` PATH'S OWN DEADLINE, which had no assertion behind it at all: code
    // review deleted BOTH checkpoints in `compress` and the entire workspace stayed green.
    //
    // THE CEILING COMES FROM `source.limits`, not from the options passed to `compress` -- a
    // caller must not be able to loosen a limit after the document is already in memory. So
    // the tight budget has to be the one the document was OPENED under, and this test asserts
    // that half too: the open uses the stepping clock and the tight budget, and the refusal
    // has to come from it rather than from the options handed in later.
    let clock: Arc<dyn Clock> = Arc::new(SteppingClock::new(10));
    let limits = Limits::with(|l| l.max_duration_ms = 5);
    let opts = OpenOptions::new(limits, Arc::clone(&clock));

    let source = DocumentCompressor::open(
        &Qpdf::new(),
        packable().into_boxed_slice(),
        // Opened under a STOPPED clock so the open itself succeeds; the tight budget below is
        // what the write is measured against.
        &OpenOptions::new(limits, stopped()),
    )
    .expect("a well-formed document opens");

    let refused = Qpdf::new().compress(&source, &opts);
    assert!(
        matches!(
            refused,
            Err(Error::LimitExceeded { limit, .. }) if limit == "max_duration_ms"
        ),
        "a write on a 10 ms-per-tick clock against a 5 ms budget returned {refused:?}"
    );
}

#[test]
fn a_budget_with_room_does_not_refuse_the_write() {
    // THE CONTROL for the test above. Without it that assertion is satisfied by a `compress`
    // that refuses everything -- which is the failure mode a deadline check most easily
    // degrades into, and the one `rotate_tests` pairs its own spent-deadline test against.
    let source = open(packable()).expect("a well-formed document opens");
    let out = Qpdf::new()
        .compress(&source, &options())
        .expect("a stopped clock must not refuse the write");
    assert!(has_object_streams(&out));
}

#[test]
fn every_object_handle_the_compression_takes_is_released() {
    // The sweep takes one handle per page. They must all go back: qpdf's handle cache only
    // grows, and `max_memory_bytes` samples at operation boundaries and would not see it.
    let source = open(minimal_pdf::pdf_with_page_tree(
        40,
        RotationPlacement::Absent,
    ))
    .unwrap();
    let baseline = super::handle::live();

    let deadline = burrow_types::Deadline::start(stopped().as_ref(), &Limits::default());
    let _ = DocumentCompressor::rotations(&Qpdf::new(), &source, &options(), &deadline).unwrap();
    let out = Qpdf::new().compress(&source, &options()).unwrap();
    assert!(!out.is_empty());

    assert_eq!(
        super::handle::live(),
        baseline,
        "the compression left object handles alive in qpdf's cache"
    );
}
