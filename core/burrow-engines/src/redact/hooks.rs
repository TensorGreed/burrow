//! Three seams a test needs and no document can reach, shared by every engine the policy runs on.
//!
//! Moved here from `qpdf::tests` with the policy (#191): they are read by the generic witness and
//! the generic operation, so they belong beside them rather than beside one engine's tests.

// The `Cleared` the verification receives is built inside a closure in `redact_page` and
// consumed immediately. Nothing returns it, and a security review measured what that costs:
// forcing `cut_fonts` empty disabled the mapping check for every document and the whole suite
// stayed green, because `redact_verify`'s fakes construct a `Cleared` by hand and `burrow-ops`'
// fake engine never verifies. THE WIRING IS THE SEAM NEITHER FAKE REACHES.
//
// A `thread_local` rather than a parameter because the closure's signature is
// `Fn(&[u8]) -> Result<()>`, and widening it for a test would put the test in the type.
thread_local! {
    pub(crate) static LAST_EXPECTATION: std::cell::RefCell<Option<crate::redact_verify::Cleared>> =
        const { std::cell::RefCell::new(None) };
}

/// Record what the check was handed. Called from `redact_page`.
pub(crate) fn record_expectation(expected: &crate::redact_verify::Cleared) {
    LAST_EXPECTATION.with(|slot| *slot.borrow_mut() = Some(expected.clone()));
}

/// What the last redaction on this thread told its check.
///
/// Read only by `qpdf::tests`, which drive the real engine; the web engine's tests, in
/// `qpdf::web_differential_tests`, assert on the outcome instead.
#[cfg(all(feature = "native-engines", burrow_native_engines, target_os = "linux"))]
pub(crate) fn last_expectation() -> Option<crate::redact_verify::Cleared> {
    LAST_EXPECTATION.with(|slot| slot.borrow().clone())
}

// A read-back failure a test can force, for the one question no document can ask.
//
// `region_is_cleared` only fails on a document that a correct operation never produces, so
// "does the operation propagate a rejection" has no fixture. Read by the witness's `open_output`
// -- in the witness rather than in the verify closure, because a hook in the closure would be
// bypassed by the very mutation this exists to catch.
thread_local! {
    pub(crate) static FORCED_FAILURE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the read-back should fail, and the error it fails with.
pub(crate) fn forced_read_back_failure() -> Option<burrow_types::Error> {
    FORCED_FAILURE
        .with(std::cell::Cell::get)
        .then(|| burrow_types::Error::Malformed("planted read-back failure".to_owned()))
}

// THE NARROWING, SWITCHED OFF, for the one question the verification exists to answer: if font
// surgery stops removing the mappings of the codes it removed, does the read-back refuse? No
// document can ask it, because a correct operation always narrows. #218's defect was a check that
// never fired on exactly this, and a review found it by editing the engine; this is that edit as a
// seam. Counted, so a test can assert the narrowing was actually reached and skipped.
thread_local! {
    pub(crate) static SKIP_NARROWING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    pub(crate) static NARROWINGS_SKIPPED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Whether font surgery should leave `/ToUnicode` and `/Differences` alone, counting each skip.
pub(crate) fn narrowing_skipped() -> bool {
    let skip = SKIP_NARROWING.with(std::cell::Cell::get);
    if skip {
        NARROWINGS_SKIPPED.with(|n| n.set(n.get() + 1));
    }
    skip
}

// ---- #206: the second reading, and the placement defects it exists to catch -------------------

// A PLACEMENT DEFECT, planted in the font reader rather than in the document.
//
// The second reading exists for a defect in PLACING a glyph, which burrow makes in both of its
// passes -- the operation's and the read-back's -- so the two agree. No document produces one: a
// correct reader places every glyph where PDFium does. So the defect is planted where it would be,
// in the font facts both passes read, and both inherit it as a real one would: every simple font's
// `/Widths` read as a tenth of what the file says. A fixture then draws plain, visible text whose
// second glyph only the true width carries into the region; with the plant on, burrow removes
// nothing and its own verification passes the output, and only the second reading can refuse.
//
// In the reader and not in the walk, because `pdfsyntax::geometry` holds itself to having no
// test-only code above its tests (`nothing_above_the_tests_is_compiled_only_for_them_except_the_
// recorder`). Counted, so a test can assert the plant applied.
thread_local! {
    pub(crate) static WIDTHS_MISREAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    pub(crate) static WIDTHS_MISREAD_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Whether the font reader should misread `/Widths`, counting each time it does.
pub(crate) fn widths_misread() -> bool {
    let misread = WIDTHS_MISREAD.with(std::cell::Cell::get);
    if misread {
        WIDTHS_MISREAD_COUNT.with(|n| n.set(n.get() + 1));
    }
    misread
}

thread_local! {
    /// Make the second reading fail outright, so a test can ask whether its error is a refusal.
    pub(crate) static SECOND_READING_FAILS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Make the second reading read this page instead of the one it was asked about.
    pub(crate) static SECOND_READING_PAGE: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    /// How many times the second reading read a page, so a test can show it ran at all.
    pub(crate) static SECOND_READINGS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Make qpdf lose the document's last page as it opens it for redaction: the reader losing a
    /// page before the writer ever heard of it (#111), which no fixture can make qpdf do on demand.
    pub(crate) static READER_DROPS_A_PAGE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Make qpdf drop the document's last page as it writes the redacted output.
    pub(crate) static WRITER_DROPS_A_PAGE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

thread_local! {
    /// Run burrow's own verification alone, without the second reading. Only for a twin of an
    /// OLDER rule whose fixture PDFium cannot open: without this, that twin would be refused by
    /// the second reading and stop saying anything about its own rule. Every such twin also runs
    /// without it and requires the second reading's refusal, so the skip hides nothing.
    pub(crate) static SECOND_READING_SKIPPED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

thread_local! {
    /// Run when the second reading begins, before its first open: a test that needs the clock to
    /// pass the budget DURING the second reading, and not before it, advances its clock here.
    pub(crate) static ON_SECOND_READING: std::cell::RefCell<Option<Box<dyn Fn()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Run [`ON_SECOND_READING`]'s closure, if a test set one.
pub(crate) fn second_reading_begins() {
    ON_SECOND_READING.with(|slot| {
        if let Some(run) = slot.borrow().as_ref() {
            run();
        }
    });
}

/// Whether this thread's redactions verify without the second reading.
pub(crate) fn second_reading_skipped() -> bool {
    SECOND_READING_SKIPPED.with(std::cell::Cell::get)
}

/// Whether the second reading should fail, and the error it fails with.
pub(crate) fn forced_second_reading_failure() -> Option<burrow_types::Error> {
    SECOND_READING_FAILS
        .with(std::cell::Cell::get)
        .then(|| burrow_types::Error::Malformed("planted second-reading failure".to_owned()))
}

/// The page the second reading reads when asked for `page`. Counts the reading.
pub(crate) fn second_reading_page(page: usize) -> usize {
    SECOND_READINGS.with(|n| n.set(n.get() + 1));
    SECOND_READING_PAGE
        .with(std::cell::Cell::get)
        .unwrap_or(page)
}

// THE POST-LAUNCH ANNOTATION REMOVAL, FOR ITS OWN TESTS (#136 D3). For launch the operation
// refuses an annotation in the region; the tests of the removal it will do afterwards (#239,
// #229, #240) switch it back on, per test, with [`removing_annotations`]. Production never can:
// this module is test-only.
thread_local! {
    pub(crate) static ANNOTATIONS_REMOVED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether the operation removes an annotation in the region rather than refusing it.
pub(crate) fn annotations_removed() -> bool {
    ANNOTATIONS_REMOVED.with(std::cell::Cell::get)
}

/// Switch the post-launch removal on until the guard drops. Re-entrant: the guard restores what
/// it found, so a nested guard dropping first does not switch an outer one's removal off.
#[must_use = "the removal is switched back when the guard drops"]
pub(crate) fn removing_annotations() -> RemovingAnnotations {
    RemovingAnnotations {
        previous: ANNOTATIONS_REMOVED.with(|flag| flag.replace(true)),
    }
}

/// See [`removing_annotations`].
pub(crate) struct RemovingAnnotations {
    /// The switch as the guard found it, restored on drop.
    previous: bool,
}

impl Drop for RemovingAnnotations {
    fn drop(&mut self) {
        ANNOTATIONS_REMOVED.with(|flag| flag.set(self.previous));
    }
}

thread_local! {
    /// Set by a test to take the sole-referrer condition out of the empty `/Dests` key's
    /// admission, so the part re-scan behind it -- the second line of defence -- can be shown to
    /// refuse on its own (owner, 2026-10-15).
    pub(crate) static SOLE_REFERRER_SKIPPED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Whether a test has taken the sole-referrer condition out. See [`SOLE_REFERRER_SKIPPED`].
pub(crate) fn sole_referrer_skipped() -> bool {
    SOLE_REFERRER_SKIPPED.with(std::cell::Cell::get)
}

// ---- R10: which buffer each verifier opens (ADR 0006, amended 2026-09-27) ----------------------

/// A buffer a verification step opened, by the method that opened it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Opened {
    /// The witness's open for burrow's own walk: `Witness::open_output`.
    Walk,
    /// The second reading's input page count: `PdfiumReading::page_count`.
    PageCount,
    /// The second reading's page read: `PdfiumReading::read_page`.
    ReadPage,
}

thread_local! {
    /// Whether this thread records what the verifiers open. Off unless a test turns it on, so
    /// the rest of the suite pays no digest.
    pub(crate) static RECORDING_OPENS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Each open, in order: which method, and the sha256 of the buffer it was handed.
    pub(crate) static OPENS: std::cell::RefCell<Vec<(Opened, String)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Record that `which` was handed `bytes`, if this thread is recording. Called AT THE OPENS --
/// `Witness::open_output` and the second reading's two methods -- rather than in the closure, so
/// a mutation that hands any of them the wrong buffer is seen as what that method read (R10's
/// round 0). On the caller's thread: both readings open there before any engine thread runs.
pub(crate) fn record_open(which: Opened, bytes: &[u8]) {
    if RECORDING_OPENS.with(std::cell::Cell::get) {
        use sha2::Digest as _;
        let digest = sha2::Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        OPENS.with(|opens| opens.borrow_mut().push((which, digest)));
    }
}

/// Start recording on this thread, with nothing recorded yet.
pub(crate) fn start_recording_opens() {
    OPENS.with(|opens| opens.borrow_mut().clear());
    RECORDING_OPENS.with(|flag| flag.set(true));
}

/// Stop recording and hand back everything recorded since [`start_recording_opens`].
pub(crate) fn take_opens() -> Vec<(Opened, String)> {
    RECORDING_OPENS.with(|flag| flag.set(false));
    OPENS.with(|opens| std::mem::take(&mut *opens.borrow_mut()))
}
