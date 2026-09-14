//! Unit tests for the qpdf rotation engine.
//!
//! Here rather than in `tests/` because two of them read [`handle::live`], which is
//! `pub(crate)` and `#[cfg(test)]`: the live-handle count is evidence for this crate about
//! this crate, not an API. The end-to-end properties — nothing lost, content byte-identical —
//! are integration tests, where the shared closure harness lives.

use std::sync::Arc;

use burrow_types::{Clock, Error, Limits, ManualClock, Result, Rotation};

use super::Qpdf;
use super::handle;
use crate::minimal_pdf::{self, RotationPlacement};
use crate::{OpenOptions, PageRotator};

fn stopped() -> Arc<dyn Clock> {
    Arc::new(ManualClock::new(0))
}

/// A clock that moves forward every time it is read.
///
/// `ManualClock` only moves when told, and the loop whose deadline this has to exercise is
/// inside the engine — there is no point between its iterations where a test could advance a
/// clock by hand. Local to this file rather than added to `burrow-types`: a clock that
/// advances on observation is a testing device, and putting it in the shared crate would make
/// it reachable from the fuzz targets, where a clock that moves on its own would turn every
/// long input into a `LimitExceeded` and hide whatever the input was really doing.
#[derive(Debug)]
pub(super) struct SteppingClock {
    step_ms: u64,
    now_ms: std::sync::atomic::AtomicU64,
}

impl SteppingClock {
    pub(super) const fn new(step_ms: u64) -> Self {
        Self {
            step_ms,
            now_ms: std::sync::atomic::AtomicU64::new(0),
        }
    }
}

impl Clock for SteppingClock {
    fn now_ms(&self) -> u64 {
        self.now_ms
            .fetch_add(self.step_ms, std::sync::atomic::Ordering::SeqCst)
    }
}

fn options() -> OpenOptions<'static> {
    OpenOptions::new(Limits::default(), stopped())
}

fn open(bytes: Vec<u8>) -> Result<<Qpdf as PageRotator>::Source> {
    PageRotator::open(&Qpdf::new(), bytes.into_boxed_slice(), &options())
}

/// Rotate and reopen, so assertions are about the EMITTED bytes rather than about the
/// in-memory document the operation happened to leave behind.
fn rotate_and_reopen(
    bytes: Vec<u8>,
    pages: &[u64],
    rotation: Rotation,
) -> Result<<Qpdf as PageRotator>::Source> {
    let source = open(bytes)?;
    let output = Qpdf::new().rotate(&source, pages, rotation, &options())?;
    drop(source);
    open(output)
}

#[test]
fn a_document_with_no_rotate_anywhere_reads_as_zero() {
    let source = open(minimal_pdf::pdf_with_page_tree(
        4,
        RotationPlacement::Absent,
    ))
    .unwrap();
    for page in 0..4 {
        assert_eq!(
            Qpdf::new().effective_rotation(&source, page).unwrap(),
            Rotation::None
        );
    }
}

#[test]
fn a_rotate_on_the_root_is_inherited_by_every_page() {
    // THE CASE A PAGE-DICTIONARY READ GETS WRONG. No page carries `/Rotate`; the root does.
    let source = open(minimal_pdf::pdf_with_page_tree(
        4,
        RotationPlacement::OnTheRoot(90),
    ))
    .unwrap();
    for page in 0..4 {
        assert_eq!(
            Qpdf::new().effective_rotation(&source, page).unwrap(),
            Rotation::Clockwise90,
            "page {page} inherits the root's rotation"
        );
    }
}

#[test]
fn a_rotate_on_a_branch_reaches_only_that_branch() {
    let source = open(minimal_pdf::pdf_with_page_tree(
        4,
        RotationPlacement::OnTheSecondBranch(180),
    ))
    .unwrap();
    let rotations: Vec<_> = (0..4)
        .map(|page| Qpdf::new().effective_rotation(&source, page).unwrap())
        .collect();
    assert_eq!(
        rotations,
        vec![
            Rotation::None,
            Rotation::None,
            Rotation::Clockwise180,
            Rotation::Clockwise180
        ]
    );
}

#[test]
fn rotating_is_relative_to_the_inherited_value() {
    // Inherit 90, turn another 90, end at 180. An implementation that read the page
    // dictionary would see nothing, write 90, and UNDO the inherited quarter turn -- which
    // is a wrong document that passes every "did it write /Rotate?" assertion.
    let rotated = rotate_and_reopen(
        minimal_pdf::pdf_with_page_tree(4, RotationPlacement::OnTheRoot(90)),
        &[0],
        Rotation::Clockwise90,
    )
    .unwrap();
    assert_eq!(
        Qpdf::new().effective_rotation(&rotated, 0).unwrap(),
        Rotation::Clockwise180
    );
}

#[test]
fn rotating_one_page_leaves_every_other_page_alone_including_ones_that_inherit() {
    // THE TEST THE WRITE RULE EXISTS FOR. Every page inherits 90 from the root. Rotating
    // page 1 must put 180 on page 1's own dictionary; writing it to the root -- the node the
    // value was READ from, which is the obvious place to put it back -- would rotate all
    // four and report success.
    let rotated = rotate_and_reopen(
        minimal_pdf::pdf_with_page_tree(4, RotationPlacement::OnTheRoot(90)),
        &[0],
        Rotation::Clockwise90,
    )
    .unwrap();

    assert_eq!(
        Qpdf::new().effective_rotation(&rotated, 0).unwrap(),
        Rotation::Clockwise180,
        "the page that was asked for"
    );
    for page in 1..4 {
        assert_eq!(
            Qpdf::new().effective_rotation(&rotated, page).unwrap(),
            Rotation::Clockwise90,
            "page {page} inherits, and was not asked for"
        );
    }
}

#[test]
fn rotating_a_page_that_carries_its_own_value_leaves_the_others_alone_too() {
    let rotated = rotate_and_reopen(
        minimal_pdf::pdf_with_page_tree(4, RotationPlacement::OnEveryPage(270)),
        &[2],
        Rotation::Clockwise90,
    )
    .unwrap();
    let rotations: Vec<_> = (0..4)
        .map(|page| Qpdf::new().effective_rotation(&rotated, page).unwrap())
        .collect();
    assert_eq!(
        rotations,
        vec![
            Rotation::Clockwise270,
            Rotation::Clockwise270,
            // 270 + 90 = 360, reduced to none.
            Rotation::None,
            Rotation::Clockwise270
        ]
    );
}

#[test]
fn a_rotate_that_is_not_an_integer_is_malformed_rather_than_zero() {
    // TRAPPING ANSWERS CRASHES, NOT WRONG ANSWERS. `qpdf_oh_get_int_value` on a name object
    // returns 0 without raising, so nothing in qpdf objects to `/Rotate /Ninety` -- it would
    // read as "no rotation" and emit a page turned the wrong way. The type check is what
    // makes this a refusal.
    let mut bytes = minimal_pdf::pdf_with_page_tree(4, RotationPlacement::OnTheRoot(90));
    let at = find(&bytes, b"/Rotate 90").expect("the fixture writes /Rotate on the root");
    bytes.splice(at..at + b"/Rotate 90".len(), b"/Rotate /X".iter().copied());

    let source = open(bytes).unwrap();
    assert!(matches!(
        Qpdf::new().effective_rotation(&source, 0),
        Err(Error::Malformed(_))
    ));
}

#[test]
fn a_rotate_that_is_not_a_multiple_of_ninety_is_malformed_rather_than_rounded() {
    let mut bytes = minimal_pdf::pdf_with_page_tree(4, RotationPlacement::OnTheRoot(90));
    let at = find(&bytes, b"/Rotate 90").unwrap();
    bytes.splice(at..at + b"/Rotate 90".len(), b"/Rotate 45".iter().copied());

    let source = open(bytes).unwrap();
    assert!(matches!(
        Qpdf::new().effective_rotation(&source, 0),
        Err(Error::Malformed(_))
    ));
}

#[test]
fn a_page_past_the_end_is_an_invalid_argument() {
    let source = open(minimal_pdf::pdf_with_page_tree(
        4,
        RotationPlacement::Absent,
    ))
    .unwrap();
    assert!(matches!(
        Qpdf::new().effective_rotation(&source, 4),
        Err(Error::InvalidArgument(_))
    ));
    assert!(matches!(
        Qpdf::new().rotate(&source, &[4], Rotation::Clockwise90, &options()),
        Err(Error::InvalidArgument(_))
    ));
}

#[test]
fn naming_no_pages_or_the_same_page_twice_is_refused() {
    let source = open(minimal_pdf::pdf_with_page_tree(
        4,
        RotationPlacement::Absent,
    ))
    .unwrap();
    assert!(matches!(
        Qpdf::new().rotate(&source, &[], Rotation::Clockwise90, &options()),
        Err(Error::InvalidArgument(_))
    ));
    // Rotating a page twice would turn it 180 while the caller asked for 90, and nothing in
    // the output says which happened.
    assert!(matches!(
        Qpdf::new().rotate(&source, &[1, 1], Rotation::Clockwise90, &options()),
        Err(Error::InvalidArgument(_))
    ));
}

#[test]
fn a_document_over_the_page_ceiling_is_refused_at_open() {
    let bytes = minimal_pdf::pdf_with_page_tree(4, RotationPlacement::Absent);
    let limits = Limits::with(|l| l.max_pages = 3);
    let refused = PageRotator::open(
        &Qpdf::new(),
        bytes.clone().into_boxed_slice(),
        &OpenOptions::new(limits, stopped()),
    );
    assert!(matches!(refused, Err(Error::LimitExceeded { .. })));

    // The selection half of this used to live here and asserted `is_ok()` under a name
    // claiming a refusal -- it measured the opposite of what it said. It is now
    // `a_selection_over_the_page_ceiling_is_refused`, which actually trips the check. What is
    // left here is the boundary: a document exactly at the ceiling is allowed.
    let limits = Limits::with(|l| l.max_pages = 2);
    let source = PageRotator::open(
        &Qpdf::new(),
        minimal_pdf::pdf_with_page_tree(2, RotationPlacement::Absent).into_boxed_slice(),
        &OpenOptions::new(limits, stopped()),
    )
    .unwrap();
    let allowed = Qpdf::new().rotate(
        &source,
        &[0, 1],
        Rotation::Clockwise90,
        &OpenOptions::new(limits, stopped()),
    );
    assert!(allowed.is_ok(), "two pages is exactly the ceiling");
}

#[test]
fn every_object_handle_the_rotation_takes_is_released() {
    // THE LEAK NO EXISTING CEILING WOULD SEE. `qpdf_oh_*` handles accumulate in a map on the
    // document for its whole life; nothing in qpdf's C API reports how many are live, and
    // `max_memory_bytes` samples RSS at operation boundaries, so a handle per page-tree node
    // per page is steady growth under every ceiling there is until it is not.
    //
    // The count is this crate's own: every route from qpdf to a handle goes through
    // `ObjectHandle`, which releases on drop. That is a type argument; this is the
    // measurement, taken on the real code path.
    let bytes = minimal_pdf::pdf_with_page_tree(40, RotationPlacement::OnTheRoot(90));
    let source = open(bytes).unwrap();

    let baseline = handle::live();
    // Every page, so the ancestor walk runs 40 times over a two-level tree.
    let pages: Vec<u64> = (0..40).collect();
    let output = Qpdf::new()
        .rotate(&source, &pages, Rotation::Clockwise90, &options())
        .unwrap();
    assert!(!output.is_empty());

    assert_eq!(
        handle::live(),
        baseline,
        "the rotation left object handles alive; they are freed only when the document is"
    );

    // And reading, which is the walk on its own.
    let baseline = handle::live();
    for page in 0..40 {
        let _ = Qpdf::new().effective_rotation(&source, page).unwrap();
    }
    assert_eq!(
        handle::live(),
        baseline,
        "the inheritance walk leaks handles"
    );
}

#[test]
fn the_handle_count_probe_can_fail() {
    // THE PROBE'S OWN PROBE. A counter that never moves reports "no leak" for every input,
    // including a real leak -- so the test above would be green whether or not release
    // worked. This holds a handle deliberately and asserts the count notices, which is what
    // makes the assertion above a measurement rather than a formality.
    let source = open(minimal_pdf::pdf_with_page_tree(
        2,
        RotationPlacement::Absent,
    ))
    .unwrap();
    let baseline = handle::live();
    let held = super::rotate::page_handle_for_test(&source, 0).unwrap();
    assert_eq!(
        handle::live(),
        baseline + 1,
        "a live handle must be visible to the count the leak test relies on"
    );
    drop(held);
    assert_eq!(handle::live(), baseline);
}

/// The first offset of `needle` in `haystack`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[test]
fn a_parent_cycle_is_refused_rather_than_walked_forever() {
    // THE CASE `MAX_PAGE_TREE_DEPTH` EXISTS FOR, and it was argued in three places and
    // reached by no test until code review counted them. The first branch's `/Parent` points
    // at a page beneath it, so a walk that trusted the tree to be a tree never terminates.
    //
    // The test passing at all is the assertion: an unbounded walk hangs here rather than
    // failing, so a regression is a timeout, not a wrong answer.
    let bytes = minimal_pdf::pdf_with_page_tree_shaped(
        4,
        RotationPlacement::Absent,
        minimal_pdf::TreeShape::ParentCycle,
    );
    let source = open(bytes).unwrap();

    assert!(
        matches!(
            Qpdf::new().effective_rotation(&source, 0),
            Err(Error::Malformed(_))
        ),
        "a page under the cyclic branch must be refused"
    );
    // And the well-formed half of the same document still answers, so the guard is refusing
    // the cycle rather than refusing the fixture.
    assert_eq!(
        Qpdf::new().effective_rotation(&source, 3).unwrap(),
        Rotation::None
    );
}

#[test]
fn a_page_tree_deeper_than_the_walk_is_refused() {
    // The legitimate direction on the same ceiling: no cycle, just more `/Pages` nodes than
    // the walk will follow. `MAX_PAGE_TREE_DEPTH` is 64; 70 links is past it.
    let bytes = minimal_pdf::pdf_with_page_tree_shaped(
        2,
        RotationPlacement::Absent,
        minimal_pdf::TreeShape::DeepChain(70),
    );
    let source = open(bytes).unwrap();
    assert!(matches!(
        Qpdf::new().effective_rotation(&source, 0),
        Err(Error::Malformed(_))
    ));

    // AND THE CEILING IS NOT SO LOW THAT AN ORDINARY DOCUMENT TRIPS IT. A guard that refused
    // everything would pass the test above and break every real file.
    let shallow = minimal_pdf::pdf_with_page_tree_shaped(
        2,
        RotationPlacement::Absent,
        minimal_pdf::TreeShape::DeepChain(8),
    );
    let source = open(shallow).unwrap();
    assert_eq!(
        Qpdf::new().effective_rotation(&source, 0).unwrap(),
        Rotation::None
    );
}

#[test]
fn a_selection_over_the_page_ceiling_is_refused() {
    // The half of `the_page_ceiling_applies_to_the_document_and_to_the_selection` that used
    // to assert `is_ok()` under a name claiming a refusal. Reaching it needs a selection
    // longer than the document's page count, which only a repeated page can be -- so this
    // measures the backstop, and the `InvalidArgument` for the repeat comes after it.
    let source = PageRotator::open(
        &Qpdf::new(),
        minimal_pdf::pdf_with_page_tree(2, RotationPlacement::Absent).into_boxed_slice(),
        &OpenOptions::new(Limits::with(|l| l.max_pages = 2), stopped()),
    )
    .unwrap();
    match Qpdf::new().rotate(
        &source,
        &[0, 0, 0],
        Rotation::Clockwise90,
        &OpenOptions::new(Limits::with(|l| l.max_pages = 2), stopped()),
    ) {
        Err(Error::LimitExceeded { limit, .. }) => assert_eq!(limit, "max_pages"),
        other => panic!("expected a page-count refusal, got {other:?}"),
    }
}

#[test]
fn the_deadline_is_checked_between_pages() {
    // `max_duration_ms` on the engine path. The clock advances a second per reading, so a
    // 40-page rotation under a 5-second budget cannot finish -- and the refusal has to come
    // from inside `rotate`, because `burrow_ops::rotate`'s own checkpoints sit either side of
    // this whole call. An earlier version discarded `options` entirely and with it the clock,
    // so the ceiling was unchecked across roughly 1.9 M FFI calls. Found by security review.
    let limits = Limits::with(|l| l.max_duration_ms = 5_000);
    let options = OpenOptions::new(
        limits,
        Arc::new(SteppingClock::new(1_000)) as Arc<dyn Clock>,
    );

    let source = PageRotator::open(
        &Qpdf::new(),
        minimal_pdf::pdf_with_page_tree(40, RotationPlacement::Absent).into_boxed_slice(),
        &OpenOptions::new(limits, Arc::new(ManualClock::new(0)) as Arc<dyn Clock>),
    )
    .unwrap();

    let pages: Vec<u64> = (0..40).collect();
    match Qpdf::new().rotate(&source, &pages, Rotation::Clockwise90, &options) {
        Err(Error::LimitExceeded { limit, .. }) => assert_eq!(limit, "max_duration_ms"),
        other => panic!("expected a deadline refusal, got {other:?}"),
    }
}

/// The sweep spends the deadline it is HANDED, not one of its own.
///
/// `Deadline::start` resets the origin and the budget, so a sweep that started its own gave
/// the operation a second full `max_duration_ms` — measured by security review at 56 ms
/// returned against a 50 ms ceiling, and in direct contradiction of what `verify::output` says
/// in capitals about spending one budget.
///
/// An already-expired deadline is the sharpest way to ask: a sweep that honours it refuses
/// before it reads a single page, and a sweep that starts its own reads all of them.
#[test]
fn the_rotation_sweep_refuses_against_an_already_spent_deadline() {
    use burrow_types::Deadline;

    let source = open(minimal_pdf::pdf_with_page_tree(
        6,
        RotationPlacement::OnTheRoot(90),
    ))
    .expect("open");

    let clock = Arc::new(ManualClock::new(0));
    let limits = Limits::with(|l| l.max_duration_ms = 10);
    let deadline = Deadline::start(clock.as_ref(), &limits);
    clock.advance(11);

    let err = PageRotator::rotations(
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
    let rotations = PageRotator::rotations(
        &Qpdf::new(),
        &source,
        &OpenOptions::new(limits, Arc::clone(&clock) as Arc<dyn Clock>),
        &deadline,
    )
    .expect("a budget with room must not refuse");
    assert_eq!(rotations, vec![90; 6]);
}

/// The sweep records an out-of-spec `/Rotate` rather than refusing it.
///
/// The engine half of the `/Rotate 45` regression: `effective_rotation` judges and this
/// records, and a page nobody named is only ever recorded.
#[test]
fn the_rotation_sweep_records_a_value_that_is_not_a_multiple_of_ninety() {
    use burrow_types::Deadline;

    let source = open(minimal_pdf::pdf_with_page_tree(
        4,
        RotationPlacement::OnTheRootAndTheFirstPage {
            root: 0,
            first_page: 45,
        },
    ))
    .expect("open");

    let clock = Arc::new(ManualClock::new(0));
    let deadline = Deadline::start(clock.as_ref(), &Limits::default());
    let rotations = PageRotator::rotations(&Qpdf::new(), &source, &options(), &deadline)
        .expect("an out-of-spec value is recorded, not refused");
    assert_eq!(rotations, vec![45, 0, 0, 0]);

    // AND THE JUDGING READ STILL REFUSES IT, which is what makes the pair a pair rather than a
    // check that was deleted.
    let err = Qpdf::new()
        .effective_rotation(&source, 0)
        .expect_err("the judging read must still refuse");
    assert!(matches!(err, Error::Malformed(_)), "got {err:?}");
}

/// A sweep that fails part-way leaks no handle.
#[test]
fn a_refused_rotation_sweep_releases_every_handle_it_took() {
    use burrow_types::Deadline;

    let before = handle::live();
    {
        let source = open(minimal_pdf::pdf_with_page_tree(
            6,
            RotationPlacement::Absent,
        ))
        .expect("open");
        let clock = Arc::new(SteppingClock::new(4)) as Arc<dyn Clock>;
        let limits = Limits::with(|l| l.max_duration_ms = 10);
        let deadline = Deadline::start(clock.as_ref(), &limits);

        let err = PageRotator::rotations(
            &Qpdf::new(),
            &source,
            &OpenOptions::new(limits, Arc::clone(&clock)),
            &deadline,
        )
        .expect_err("the stepping clock must run the sweep out of time part-way");
        assert!(
            matches!(err, Error::LimitExceeded { limit, .. } if limit == "max_duration_ms"),
            "got {err:?}"
        );
    }
    assert_eq!(
        handle::live(),
        before,
        "the refused sweep left {} handle(s) behind",
        handle::live() - before
    );
}
