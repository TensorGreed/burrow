//! PDFium's character boxes, as a differential oracle for burrow's own glyph geometry.
//!
//! # Why an oracle at all
//!
//! ADR 0029 §6 records that *"no glyph remains inside the region"* re-derives positions **with
//! the same geometry code the operation used to decide what to remove**. A bug therefore hides
//! from the check meant to catch it: the operation removes the wrong glyphs and verification
//! agrees they are gone, because both asked the same wrong question. That is #111's shape in
//! geometry rather than in mapping, and none of the `/ToUnicode` reasoning touches it.
//!
//! So the geometry is checked against a second implementation that shares no code with it.
//!
//! # Why PDFium is admissible here, when `check-pdfium-is-render-only.sh` exists
//!
//! That checker is a claim about **what a visitor downloads**. This is a test binary. Nothing
//! here reaches the web bundle, the bridge, a wasm export or a `Cargo.toml` the site builds
//! from — and `burrow-engines`' native test target already links `libpdfium.so`.
//!
//! It is an **oracle for the geometry code, not a component of the operation**. Shipping it
//! would reintroduce every objection ADR 0029's *Alternatives considered* raises against
//! `FPDFText_*`.
//!
//! # Three properties, because the three quantities are different
//!
//! `fpdf_text.h` offers three, and using the wrong one for the wrong question makes the oracle
//! either permanently red or vacuous. All three were measured on real pages before this design
//! was settled, not read off the header:
//!
//! | | what it is | measured |
//! |---|---|---|
//! | `FPDFText_GetCharOrigin` | the pen position | Helvetica `(AV)` at `100 700 Td`: 100.00 then 108.00, exactly the advances |
//! | `FPDFText_GetLooseCharBox` | *"the entire glyph bounds, without taking the actual glyph shape into account"* | for Helvetica its width **is** the advance (8.00 = 667/1000 × 12); for a Type 3 glyph drawn away from its origin it is **not** — 100…130 against an advance of 100…106 |
//! | `FPDFText_GetCharBox` | the box **as inked** | for that Type 3 glyph, 124…130 — entirely **outside** the advance box |
//!
//! That last row is the whole reason this module has three properties instead of one:
//!
//! * **P1, placement** — burrow's computed origin equals `FPDFText_GetCharOrigin`, within
//!   [`TOLERANCE_PT`]. Every term #129 tests moves the origin, and nothing about ascent, descent
//!   or bounding boxes enters into where it lands, so this is the term check with no
//!   interpretation in it. The loose box is **not** used for this: it is the advance box for a
//!   simple font and something wider for a Type 3, so comparing a computed advance box against
//!   it would be right in one case and wrong in the other.
//! * **P2, ink containment** — `FPDFText_GetCharBox` lies **inside burrow's conservative box**.
//!   This is the correctness property: a glyph's outline can be drawn far from its origin, so a
//!   region test against the advance box alone misses ink that is inside the region while the
//!   advance box sits outside it. Measured above at 18 pt of separation on a 12 pt font.
//! * **P3, tightness** — the conservative box may not be arbitrarily large, or P2 passes by
//!   covering the page. Bounded against the loose box's area.
//!
//! # The conservative box, and why the runtime needs one as well as the test
//!
//! P2 is a **test-time** check: it catches a geometry pass whose box does not cover the ink. It
//! cannot run at redaction time, because it needs PDFium. So the operation carries the same
//! safety structurally: the box it reasons about is the **advance box unioned with the scaled
//! `/FontBBox`**, so uncertainty removes more rather than less.
//!
//! A Type 3 `/CharProcs` entry can draw anywhere at all, and CFF and TrueType outlines overhang
//! their advances routinely — accents, swashes, italic descenders. The advance box is where the
//! glyph's *pen* is, not where its *ink* is, and redaction is about the ink.
//!
//! **What that does not cover, stated rather than implied:** a `/FontBBox` that lies. A font may
//! declare a box its glyphs exceed, and nothing in the file contradicts it. P2 catches it in the
//! suite; at runtime it is a residue, and the fixture that would provoke it belongs with the
//! refusals rather than here.

#![allow(
    dead_code,
    reason = "each helper lands with the term whose fixture uses it"
)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "test support: a failure here is a broken harness, and it should be loud. The \
              workspace lints are for library code, where they stay denied -- the sibling test \
              binaries carry the same allowance for the same reason"
)]

use std::os::raw::{c_char, c_double, c_float, c_int, c_void};

/// How far burrow's edge may sit from PDFium's and still count as agreement, in PDF points.
///
/// # Decided before the first measurement, and this is the record of that
///
/// A tolerance chosen after seeing a result is fitted to the result: it will always pass, and it
/// will pass a defect just as readily. This number is derived from two bounds, neither of them a
/// measurement of burrow's output.
///
/// **From below — it must exceed float noise.** PDFium fills `FS_RECTF` with `float`. PDF's
/// maximum user-space coordinate is 14,400; at that magnitude an `f32` ulp is about 0.001 pt,
/// and a box composed through a CTM, a text matrix, a Form `/Matrix` and a `/FontMatrix`
/// accumulates a handful of those. The worst plausible disagreement from arithmetic alone is
/// under 0.01 pt.
///
/// **From above — it must not be able to hide any term.** Every fixture in this suite is built
/// so that the term it exercises displaces its glyph by **at least 1 pt**. This is a twentieth
/// of that. A term that is ignored entirely cannot land within the tolerance of one that is
/// applied.
///
/// So it is 5x the worst arithmetic noise and 20x below the smallest defect any fixture can
/// produce, and the gap between those two numbers is what makes it defensible rather than
/// arbitrary. A fixture whose term moves a glyph by less than 1 pt is a broken fixture, not a
/// reason to raise this.
///
/// **This is not a knob.** A run that disagrees by more than this means the geometry is wrong,
/// or that burrow and PDFium disagree about something real and it gets investigated and written
/// down. Raising it to make a run pass is the one edit this constant exists to forbid.
pub const TOLERANCE_PT: f64 = 0.05;

/// How much larger than PDFium's loose box burrow's conservative box may be, by area.
///
/// P2 is satisfied trivially by a box the size of the page, so it needs a companion. Four is
/// loose enough for a `/FontBBox` that genuinely covers a tall glyph set — a Type 3 font's box
/// can be several times the advance in both axes — and tight enough that a degenerate box fails.
pub const MAX_CONSERVATIVE_AREA_RATIO: f64 = 4.0;

/// The smallest displacement a fixture's term must produce, in PDF points.
///
/// Asserted by [`assert_fixture_is_discriminating`], so the tolerance argument above stays true
/// of the suite rather than being a claim about fixtures nobody checked.
pub const MIN_FIXTURE_DISPLACEMENT_PT: f64 = 1.0;

/// A rectangle in PDF user space, as both PDFium and burrow produce.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub left: f64,
    pub bottom: f64,
    pub right: f64,
    pub top: f64,
}

impl Rect {
    /// The largest distance between corresponding edges, which is what the tolerance bounds.
    #[must_use]
    pub fn max_edge_distance(&self, other: &Self) -> f64 {
        [
            (self.left - other.left).abs(),
            (self.bottom - other.bottom).abs(),
            (self.right - other.right).abs(),
            (self.top - other.top).abs(),
        ]
        .into_iter()
        .fold(0.0_f64, f64::max)
    }

    /// How far `inner` protrudes outside `self`, zero when it is contained.
    ///
    /// Signed per edge and then maximised, so a box that is inside on three edges and outside on
    /// one reports the one — which is the case a "does it overlap" test would miss.
    #[must_use]
    pub fn escape_of(&self, inner: &Self) -> f64 {
        [
            self.left - inner.left,
            inner.right - self.right,
            self.bottom - inner.bottom,
            inner.top - self.top,
        ]
        .into_iter()
        .fold(0.0_f64, f64::max)
    }
}

/// PDFium's `FS_RECTF`, which is `float` and in a different field order from [`Rect`].
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct FsRectF {
    left: c_float,
    top: c_float,
    right: c_float,
    bottom: c_float,
}

// Declared HERE rather than in the crate's `pdfium/ffi.rs`, because nothing that ships may
// reach these. `#[link]` is explicit for the reason `examples/dump-page-text.rs` records: a
// target declaring its own symbols does not inherit build.rs's `cargo:rustc-link-lib`, and
// without it the link fails with "undefined reference to `FPDF_InitLibrary'". The search path
// and the rpath do come from build.rs, so only the name is needed.
#[link(name = "pdfium")]
unsafe extern "C" {
    fn FPDF_InitLibrary();
    fn FPDF_LoadMemDocument64(
        data: *const c_void,
        size: usize,
        password: *const c_char,
    ) -> *mut c_void;
    fn FPDF_CloseDocument(document: *mut c_void);
    fn FPDF_LoadPage(document: *mut c_void, index: c_int) -> *mut c_void;
    fn FPDF_ClosePage(page: *mut c_void);
    fn FPDF_GetPageWidthF(page: *mut c_void) -> f32;
    fn FPDF_GetPageHeightF(page: *mut c_void) -> f32;
    fn FPDFText_LoadPage(page: *mut c_void) -> *mut c_void;
    fn FPDFText_ClosePage(text_page: *mut c_void);
    fn FPDFText_CountChars(text_page: *mut c_void) -> c_int;
    fn FPDFText_GetUnicode(text_page: *mut c_void, index: c_int) -> u32;
    fn FPDFText_GetCharBox(
        text_page: *mut c_void,
        index: c_int,
        left: *mut c_double,
        right: *mut c_double,
        bottom: *mut c_double,
        top: *mut c_double,
    ) -> c_int;
    fn FPDFText_GetLooseCharBox(text_page: *mut c_void, index: c_int, rect: *mut FsRectF) -> c_int;
    /// Whether PDFium **invented** this character rather than reading it from the file.
    ///
    /// A positioning adjustment wide enough to look like a gap produces a space; a `T*` line
    /// move produces a CR/LF pair. Neither is in any string in the document.
    ///
    /// This suite guessed at that before it used this call -- "a synthetic character has no
    /// advance, so its origin equals the next one's" -- and the guess was wrong on a real
    /// LaTeX document, where PDFium gave a synthetic space an origin between the two glyphs it
    /// sat between. Asking is not a heuristic.
    fn FPDFText_IsGenerated(text_page: *mut core::ffi::c_void, index: c_int) -> c_int;

    fn FPDFText_GetCharOrigin(
        text_page: *mut c_void,
        index: c_int,
        x: *mut c_double,
        y: *mut c_double,
    ) -> c_int;
}

/// PDFium's marker for a hyphen it has decided ends a line.
const PDFIUM_LINE_FINAL_HYPHEN: u32 = 0x0002;

/// What PDFium reports for a hyphen it has not.
const HYPHEN_MINUS: u32 = 0x002D;

/// Undo the one re-labelling PDFium applies that depends on the *rest of the page*.
///
/// # Why this exists, and what was measured
///
/// `FPDFText_GetUnicode` is not a function of the character. For a hyphen that is the last
/// character of its line **and has another line after it**, PDFium reports `U+0002` — its
/// internal soft-hyphen-at-a-line-break marker — instead of `U+002D`. Four variants of one
/// page, differing only in their content stream, separate the condition exactly:
///
/// | line 1 | line 2 present | reported |
/// |---|---|---|
/// | `BURROW-SECRET-` | yes | `… 0054` **`0002`** |
/// | `BURROW-SECRET-` | no | `… 0054 002D` |
/// | `BURROW-SECRET` | yes | `… 0054` |
/// | `BURROW-SECRET-04` | yes | `… 0054 002D 0030 0034` |
///
/// It reproduces with an unembedded Helvetica and a four-character string, so it is PDFium's
/// text layer and not anything about the fixture's subset font.
///
/// # Why the redaction tests cannot live with it
///
/// **A redaction changes this condition without moving anything.** Removing `04` from the end
/// of `BURROW-SECRET-04` leaves the hyphen at the identical origin, drawn by the identical
/// code, through a font whose `/Differences` still names it — and PDFium's answer for it
/// changes from `002D` to `0002`. `redaction_corpus.rs` read that as a glyph that "appears and
/// was not drawn before — the page reflowed", which is exactly backwards: nothing reflowed,
/// and the instrument re-labelled.
///
/// The reverse direction is the one that would have mattered more. A redaction that removes
/// the *following line* turns a `0002` into a `002D`, and the corpus test's other half asks
/// whether a glyph the region reached is **gone** by comparing unicodes — so an un-canonicalised
/// hyphen would read as removed while still being drawn. That is a leak check failing open,
/// which is why this normalises **both** sides rather than special-casing the direction that
/// was observed.
///
/// Folding `0002` into `002D` is the conservative direction for both: it can only make two
/// characters compare equal that PDFium already agrees are the same glyph.
#[must_use]
pub fn canonical_unicode(unicode: u32) -> u32 {
    if unicode == PDFIUM_LINE_FINAL_HYPHEN {
        HYPHEN_MINUS
    } else {
        unicode
    }
}

/// One character as PDFium sees it: what it is, and where both of its boxes are.
#[derive(Debug, Clone, Copy)]
pub struct OracleChar {
    /// The Unicode PDFium decoded, **canonicalised** by [`canonical_unicode`].
    ///
    /// **Not used to decide anything.** A lying `/ToUnicode` moves no glyph, which is precisely
    /// why geometry is checkable by this oracle when mapping is not — see the module header.
    ///
    /// It is canonicalised because PDFium's answer for one character is not a property of that
    /// character alone: see [`canonical_unicode`] for the measurement. [`Self::raw_unicode`]
    /// keeps what PDFium actually said.
    pub unicode: u32,
    /// What `FPDFText_GetUnicode` returned, before [`canonical_unicode`].
    ///
    /// Kept so the canonicalisation is a rule with a probe behind it rather than a smoothing
    /// nobody can see the effect of: `a_line_final_hyphen_is_relabelled_by_pdfium` asserts this
    /// field is `0x0002` on a shape where [`Self::unicode`] is `0x002D`. A rule that matches
    /// nothing passes everything.
    pub raw_unicode: u32,
    /// `FPDFText_GetCharOrigin` — the pen position, in page space.
    ///
    /// **The instrument P1 uses**, because it is the one quantity with no font-metric
    /// interpretation in it: every term this issue tests moves the origin, and nothing about
    /// ascent, descent or bounding boxes enters into where it lands.
    pub origin: (f64, f64),
    /// `FPDFText_GetLooseCharBox` — the quantity burrow's geometry computes.
    pub loose: Rect,
    /// `FPDFText_GetCharBox` — the inked box, which burrow's must contain.
    pub ink: Rect,
    /// Whether PDFium **invented** this character rather than reading it from the file.
    ///
    /// `FPDFText_IsGenerated`. A wide positioning adjustment produces a space; a `T*` line move
    /// produces a CR/LF pair. Neither is in any string in the document, and a comparison
    /// against burrow's walk must not expect it.
    ///
    /// This suite guessed at this before it asked — "a synthetic character has no advance, so
    /// its origin equals the next one's" — and the guess was wrong on a real LaTeX document,
    /// where PDFium gave a synthetic space an origin of its own between two glyphs. The guess
    /// held on every hand-built fixture and failed on the first real one, which is what a
    /// corpus is for.
    pub generated: bool,
}

/// Every character on page `index` of `bytes`, as PDFium reads them.
///
/// # It runs on one thread, and a `Once` was not enough
///
/// The first version guarded only `FPDF_InitLibrary` with a `Once`, on the strength of
/// `core/CLAUDE.md`'s note that concurrent init aborts the process. The suite passed with
/// `--test-threads=1` and **died with `SIGABRT` under cargo's default parallelism**.
///
/// `pdfium/thread.rs` had already written down why: *"PDFium is not thread-safe anywhere, not
/// just at init. A `Once` fixes that one call and nothing else."* ADR 0011 chose a dedicated
/// thread over a mutex, because a mutex gives mutual exclusion and PDFium also keeps
/// **thread-local** state — which is undefined behaviour no test would reliably show.
///
/// That module's `submit` is `pub(super)`, so a test in `tests/` cannot use it. This mirrors it
/// rather than reaching for the mutex the ADR rejected: every PDFium call in this oracle runs on
/// one long-lived worker thread, and callers hand it a closure and wait for the reply.
///
/// # Panics
///
/// If PDFium cannot open the document or the page — a fixture this cannot open is a broken
/// fixture, and a test that skipped it would report a pass over nothing.
#[must_use]
pub fn chars_on_page(bytes: &[u8], index: i32) -> Vec<OracleChar> {
    let owned = bytes.to_vec();
    on_the_pdfium_thread(move || read_chars(&owned, index))
}

/// Whether an oracle ink box overlaps a region, converting between the two frames.
///
/// PDFium's boxes are measured from the bottom of the page and a region from the top, so this
/// is the one place the two meet. Written here rather than reached for from the operation: a
/// test that used the operation's own conversion would be asking the instrument whether it
/// agrees with itself.
///
/// Lives in the oracle because two test binaries need it and a copy in each is two things that
/// can disagree about which way up a page is.
#[must_use]
pub fn ink_overlaps(
    ink: &Rect,
    region_left: f64,
    region_top: f64,
    width: f64,
    height: f64,
    page_height: f64,
) -> bool {
    let top = page_height - region_top;
    let bottom = top - height;
    ink.right > region_left
        && ink.left < region_left + width
        && ink.top > bottom
        && ink.bottom < top
}

/// Two origins within the oracle's pre-registered tolerance.
#[must_use]
pub fn origins_close(a: (f64, f64), b: (f64, f64)) -> bool {
    (a.0 - b.0).hypot(a.1 - b.1) < TOLERANCE_PT
}

/// The page's size in points, as PDFium computes it.
///
/// A region is measured from the top of the page and PDFium's origins are measured from the
/// bottom, so a test that builds a region around a character PDFium found needs the height to
/// convert between them. Reading it from PDFium rather than from the fixture's `/MediaBox`
/// keeps the region derived entirely from the oracle: a region built from burrow's own reading
/// of the page would be the #111 circularity, the instrument that decided what to remove also
/// deciding where to look.
///
/// # Panics
///
/// If PDFium cannot open the document or the page.
#[must_use]
pub fn page_size(bytes: &[u8], index: i32) -> (f64, f64) {
    let owned = bytes.to_vec();
    on_the_pdfium_thread(move || {
        // SAFETY: PDFium does not copy the buffer, and `owned` outlives every call below.
        let doc =
            unsafe { FPDF_LoadMemDocument64(owned.as_ptr().cast(), owned.len(), std::ptr::null()) };
        assert!(!doc.is_null(), "PDFium could not open the fixture");
        // SAFETY: `doc` is live and `index` is a page in it.
        let page = unsafe { FPDF_LoadPage(doc, index) };
        assert!(!page.is_null(), "PDFium could not load page {index}");
        // SAFETY: `page` is live.
        let size = unsafe { (FPDF_GetPageWidthF(page), FPDF_GetPageHeightF(page)) };
        // SAFETY: each handle is live and released once, innermost first.
        unsafe {
            FPDF_ClosePage(page);
            FPDF_CloseDocument(doc);
        }
        (f64::from(size.0), f64::from(size.1))
    })
}

/// The worker every PDFium call in this module runs on. See [`chars_on_page`].
fn on_the_pdfium_thread<T, F>(work: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    type Job = Box<dyn FnOnce() + Send>;
    static QUEUE: std::sync::OnceLock<std::sync::Mutex<std::sync::mpsc::Sender<Job>>> =
        std::sync::OnceLock::new();

    let sender = QUEUE.get_or_init(|| {
        let (sender, receiver) = std::sync::mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("pdfium-oracle".to_owned())
            .spawn(move || {
                // ONE CALLER, ONE THREAD, structurally -- the same property ADR 0011 gets for
                // the shipped engine.
                // SAFETY: this is the only `FPDF_InitLibrary` call in this binary, and it
                // happens before any other PDFium call on the only thread that makes them.
                unsafe { FPDF_InitLibrary() };
                while let Ok(job) = receiver.recv() {
                    job();
                }
            })
            .expect("the oracle's PDFium thread should start");
        std::sync::Mutex::new(sender)
    });

    let (reply, answer) = std::sync::mpsc::channel();
    sender
        .lock()
        .expect("the oracle's queue is not poisoned")
        .send(Box::new(move || {
            // A panic inside the job would kill the worker and hang every later caller, so it is
            // caught and re-raised on the CALLER's thread, where the test harness can report it
            // against the test that caused it.
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
            let _ = reply.send(outcome);
        }))
        .expect("the oracle's PDFium thread should be alive");
    match answer.recv().expect("the oracle's PDFium thread replied") {
        Ok(value) => value,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

/// The body of [`chars_on_page`], which runs only on the worker thread.
fn read_chars(bytes: &[u8], index: i32) -> Vec<OracleChar> {
    // SAFETY: PDFium does not copy the buffer, and `bytes` outlives every call below.
    let doc =
        unsafe { FPDF_LoadMemDocument64(bytes.as_ptr().cast(), bytes.len(), std::ptr::null()) };
    assert!(!doc.is_null(), "PDFium could not open the fixture");
    // SAFETY: `doc` is a live document and `index` is a page in it, which the caller establishes.
    let page = unsafe { FPDF_LoadPage(doc, index) };
    assert!(!page.is_null(), "PDFium could not load page {index}");
    // SAFETY: `page` is live.
    let text = unsafe { FPDFText_LoadPage(page) };
    assert!(!text.is_null(), "PDFium could not read the page's text");

    // SAFETY: `text` is a live text page.
    let count = unsafe { FPDFText_CountChars(text) };
    let mut out = Vec::new();
    for at in 0..count {
        let (mut l, mut r, mut b, mut t) = (0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64);
        // SAFETY: `text` is live, `at` is below the count, and the four pointers are to live
        // locals. PDFium leaves them untouched on failure, which the assertion below catches.
        let ok = unsafe { FPDFText_GetCharBox(text, at, &mut l, &mut r, &mut b, &mut t) };
        assert!(ok != 0, "FPDFText_GetCharBox refused character {at}");
        let mut loose = FsRectF::default();
        // SAFETY: as above; `loose` is a live local of the layout PDFium expects.
        let ok = unsafe { FPDFText_GetLooseCharBox(text, at, &mut loose) };
        assert!(ok != 0, "FPDFText_GetLooseCharBox refused character {at}");
        // SAFETY: as above.
        let unicode = unsafe { FPDFText_GetUnicode(text, at) };
        let (mut ox, mut oy) = (0.0_f64, 0.0_f64);
        // SAFETY: as above; both pointers are to live locals.
        let ok = unsafe { FPDFText_GetCharOrigin(text, at, &mut ox, &mut oy) };
        assert!(ok != 0, "FPDFText_GetCharOrigin refused character {at}");
        // SAFETY: as above.
        let generated = unsafe { FPDFText_IsGenerated(text, at) };
        out.push(OracleChar {
            unicode: canonical_unicode(unicode),
            raw_unicode: unicode,
            origin: (ox, oy),
            // `FPDFText_IsGenerated` returns -1 when it cannot tell. Treating "cannot tell" as
            // "from the file" keeps an unknown character in the comparison rather than
            // silently dropping it, which is the direction that fails loudly.
            generated: generated == 1,
            loose: Rect {
                left: f64::from(loose.left),
                bottom: f64::from(loose.bottom),
                right: f64::from(loose.right),
                top: f64::from(loose.top),
            },
            ink: Rect {
                left: l,
                bottom: b,
                right: r,
                top: t,
            },
        });
    }

    // SAFETY: each handle is live and released once, innermost first.
    unsafe {
        FPDFText_ClosePage(text);
        FPDF_ClosePage(page);
        FPDF_CloseDocument(doc);
    }
    out
}

/// Assert burrow's origins against PDFium's — P1, the placement property.
///
/// Separate from [`assert_agrees`] because it is what every term test needs and it needs no box
/// at all. A term that is ignored moves the origin; a box comparison would fold that together
/// with questions about metrics.
///
/// # Panics
///
/// Naming the character and the distance.
pub fn assert_origins_agree(computed: &[(f64, f64)], oracle: &[OracleChar], what: &str) {
    assert_eq!(
        computed.len(),
        oracle.len(),
        "{what}: burrow placed {} glyph(s) and PDFium found {}",
        computed.len(),
        oracle.len()
    );
    assert!(
        !oracle.is_empty(),
        "{what}: the page has no characters, so this assertion examined nothing"
    );
    for (at, (mine, theirs)) in computed.iter().zip(oracle).enumerate() {
        let character = char::from_u32(theirs.unicode).unwrap_or('?');
        let apart = (mine.0 - theirs.origin.0)
            .abs()
            .max((mine.1 - theirs.origin.1).abs());
        assert!(
            apart <= TOLERANCE_PT,
            "{what}: glyph {at} ({character:?}) is {apart:.4} pt from PDFium's origin, past the \
             {TOLERANCE_PT} pt tolerance.\n  burrow: {mine:?}\n  pdfium: {:?}",
            theirs.origin
        );
    }
}

/// Assert burrow's boxes against PDFium's, both properties, for a whole page.
///
/// # Panics
///
/// Naming the character, the property, and the distance — a failure that said only "boxes
/// differ" would send the reader back to the fixture to work out which glyph and which edge.
pub fn assert_agrees(computed: &[Rect], oracle: &[OracleChar], what: &str) {
    assert_eq!(
        computed.len(),
        oracle.len(),
        "{what}: burrow found {} glyph(s) and PDFium found {} — a count mismatch is a geometry \
         failure too, and comparing the boxes pairwise would hide it",
        computed.len(),
        oracle.len()
    );
    assert!(
        !oracle.is_empty(),
        "{what}: the page has no characters, so this assertion examined nothing"
    );

    for (at, (mine, theirs)) in computed.iter().zip(oracle).enumerate() {
        let character = char::from_u32(theirs.unicode).unwrap_or('?');
        // P2 -- the ink must be inside burrow's CONSERVATIVE box, or a redaction reasoning
        // about that box leaves ink outside what it removed. This is the property that matters
        // and the reason the box is not the advance box.
        let escaped = mine.escape_of(&theirs.ink);
        assert!(
            escaped <= TOLERANCE_PT,
            "{what}: glyph {at} ({character:?}) has ink {escaped:.4} pt OUTSIDE burrow's box -- \
             a region test against this box would miss it.\n  burrow: {mine:?}\n  ink:    {:?}",
            theirs.ink
        );
        // P3 -- and the box may not be arbitrarily large, or P2 passes by covering the page.
        let area = |r: &Rect| (r.right - r.left).max(0.0) * (r.top - r.bottom).max(0.0);
        let (ours, loose) = (area(mine), area(&theirs.loose));
        assert!(
            ours <= loose * MAX_CONSERVATIVE_AREA_RATIO + 1.0,
            "{what}: glyph {at} ({character:?})'s box is {ours:.2} pt^2 against PDFium's \
             {loose:.2} -- more than {MAX_CONSERVATIVE_AREA_RATIO}x, so \"conservative\" has \
             become \"covers the page\" and P2 proves nothing.\n  burrow: {mine:?}",
        );
    }
}

/// Assert that a fixture's term actually moves a glyph far enough for the tolerance to mean
/// something.
///
/// [`TOLERANCE_PT`]'s argument rests on every fixture displacing by at least
/// [`MIN_FIXTURE_DISPLACEMENT_PT`]. That is a property of the fixtures, so it is checked on the
/// fixtures rather than asserted in prose: a term whose effect is smaller than the tolerance is
/// a term the oracle cannot see, and a suite full of those would be green and empty.
///
/// # Panics
///
/// If `with` and `without` place their first glyph closer together than the minimum.
pub fn assert_fixture_is_discriminating(with: &[OracleChar], without: &[OracleChar], what: &str) {
    let (Some(a), Some(b)) = (with.first(), without.first()) else {
        panic!("{what}: one of the two fixtures has no glyphs, so it discriminates nothing");
    };
    let moved = a.loose.max_edge_distance(&b.loose);
    assert!(
        moved >= MIN_FIXTURE_DISPLACEMENT_PT,
        "{what}: the term moves its glyph only {moved:.4} pt, under the \
         {MIN_FIXTURE_DISPLACEMENT_PT} pt this suite requires. The tolerance argument in \
         `TOLERANCE_PT` depends on every fixture clearing it -- fix the fixture, not the \
         tolerance."
    );
}
