//! What can be read back from a redacted document, and what cannot (#134, ADR 0022, ADR 0029 §6).
//!
//! # The name of the thing is the first decision
//!
//! This checks that **the region is cleared**. It does not check that the secret is gone, and
//! the two are not the same sentence. A reader who sees `Redacted` will take it to mean the
//! second, so the variant is [`crate::redact::Cleared`] and every message here says *region*.
//!
//! # What it asserts
//!
//! Re-derived from the **emitted bytes** through a fresh parse, never from the operation's own
//! record of what it did — a rewriter that believed it had removed a glyph and had not would
//! agree with itself:
//!
//! 1. **No text-showing operator remains whose glyphs fall inside the region.** The walk runs
//!    again over the output and every glyph's conservative box is tested against the region.
//! 2. **No `/ToUnicode` or `/Differences` entry remains for a code the output no longer draws
//!    — for the fonts the operation reports having cut.** Computed from the output alone: the
//!    codes each font still draws against the codes it still maps. The qualification belongs in
//!    the bullet rather than four screens away in [`Cleared::cut_fonts`], because it is the
//!    difference between a true sentence and a false one: a *retained* font still maps
//!    everything it ever mapped, by design.
//! 3. **No page key outside ADR 0029 §2's allowlist.** The same list `prune` uses.
//!
//! # What it does not assert, stated because the wording could be read as though it did
//!
//! - **Not that the secret is absent.** The secret may exist in the file in a form this cannot
//!   see: inside an embedded font program, in a channel on the unreachable catalogue, or as
//!   pixels. §7's disclosure is what covers those, and it is a disclosure precisely because
//!   nothing here can turn it into an assertion.
//! - **Not that the embedded font no longer describes the removed run.** `hb-subset` is what
//!   would close that and it is not here; the font's own `cmap` and glyph outlines are
//!   untouched by design, which ADR 0029 records as a condition for revisiting.
//! - **Not that the region is visually blank.** Vector ink and images inside the region are
//!   refused by the operation rather than removed, so reaching this point means there were
//!   none — but that is a property of the path taken, not something read back from the bytes.
//! - **Not that annotations intersecting the region are gone.** ADR 0029 §6 lists that among
//!   what verification may assert, and this does not assert it: `/Annots` is on §2's allowlist,
//!   so check 3 passes over a page still carrying one. `remove_annotations_in` does the removal
//!   and is tested operation-side, so this is a gap in the *read-back* rather than a live leak
//!   — but a reader comparing this list against §6 would otherwise conclude the check is here.
//! - **Not that text outside the region is untouched**, beyond what the page count and the
//!   `/Rotate` vector say. Those are `burrow_ops::verify::output`'s, and they are weak: a page
//!   whose text reflowed has the same count and the same rotation.
//!
//! # The geometry is #111's shape, and this does not close it
//!
//! The region-to-content conversion and the "did it go" check both come from **the same walk**
//! that decided what to remove. If `glyphs_in` places a glyph wrongly, it places it wrongly
//! twice and the two agree. That is #111 for `split`'s page count, one level worse: there the
//! promise was read from the operation's own source, here it is recomputed by the same code.
//!
//! What narrows it is that the input to the second pass is different — the **emitted bytes**,
//! reparsed — so every defect in writing, splicing and re-encoding is caught even though
//! defects in *placing* are not. The residue is placement, and the instrument that would close
//! it is `FPDFText_GetCharOrigin`: `tests/glyph_geometry.rs` pins the walk against PDFium on
//! committed fixtures, which is the calibration this check inherits rather than performs.

use std::collections::{BTreeMap, BTreeSet};

use burrow_types::{Error, Result};

use crate::pdfsyntax::geometry::Glyph;
use crate::pdfsyntax::region::{PageFrame, Region};

/// What one page's read-back must show.
///
/// Carried rather than recomputed at the check, because the region is the caller's and the page
/// is the caller's; everything else is derived from the bytes.
#[derive(Debug, Clone)]
pub struct Cleared {
    /// Which page was redacted.
    pub page: usize,
    /// The region, in the frame [`crate::pdfsyntax::region::Region`] documents.
    pub region: Region,
    /// The fonts the operation reports having **cut**, packed as
    /// [`crate::redact::FontOutcome::font`].
    ///
    /// # The mapping check applies to these and only these, and that is not a weakening
    ///
    /// The assertion "no `/ToUnicode` or `/Differences` entry remains for a code the output no
    /// longer draws" is **false for a retained font, by design**. A font shared with a page
    /// outside the operation is left intact precisely so that page's text keeps working, so it
    /// still maps every code it ever mapped — which is what ADR 0029 §7's disclosure says out
    /// loud, and the reason it is a disclosure rather than a fix.
    ///
    /// Found by wiring the check up: `a_font_whose_encoding_is_shared_with_another_page_is_
    /// retained_rather_than_cut` began failing verification, and it was right to and the check
    /// was wrong.
    ///
    /// # What taking this from the report leaves undetectable
    ///
    /// The set comes from the operation's own account of what it did, which is the circularity
    /// #111 is about — so it is worth saying exactly which direction it fails in.
    ///
    /// - The operation claims it **cut** a font it did not: the orphaned mappings are still
    ///   there and the check fires. Caught.
    /// - The operation claims it **retained** a font it actually cut: the check skips it. Not
    ///   caught — but the failure that produces is another page's text reflowing, which no
    ///   read-back of *this* page could see anyway, and which the sharing rules exist to stop
    ///   upstream.
    ///
    /// So the circularity costs nothing the read-back could otherwise have offered.
    pub cut_fonts: BTreeSet<u64>,
}

/// What a fresh reading of the emitted bytes offers.
///
/// # A separate seam from [`crate::OutputReader`], deliberately
///
/// `OutputReader` offers a page count and a `/Rotate` vector, which is what four operations
/// need and is all they need. Redaction needs the page's glyphs, its fonts' mappings and its
/// dictionary keys — three reads no other operation has ever wanted. Widening the shared trait
/// would make every implementor grow methods it never calls, so this is its own trait and
/// `burrow_ops::verify::output` still does the half that is shared.
pub trait ClearedWitness {
    /// A document opened for reading back. Never the one that produced the bytes.
    type Read;

    /// An engine sharing no document state with the one that wrote these bytes.
    ///
    /// The same requirement, and the same limits on the web, as [`crate::OutputReader::fresh`].
    #[must_use]
    fn fresh(&self) -> Self
    where
        Self: Sized;

    /// Open bytes this crate just produced.
    ///
    /// # Errors
    ///
    /// Whatever the engine reports. Not a malformed input: burrow wrote this.
    fn open_output(&self, bytes: &[u8]) -> Result<Self::Read>;

    /// The page's frame, for converting the region into content space.
    ///
    /// **Read from the OUTPUT, not carried from the input.** A redaction that changed the
    /// display box or the rotation would otherwise be checked against the frame it was asked
    /// about rather than the one it produced, and the region would be converted with numbers
    /// the emitted document does not have.
    ///
    /// # Errors
    ///
    /// Whatever reading the page's boxes refused.
    fn frame_of(&self, read: &Self::Read, page: usize) -> Result<PageFrame>;

    /// Every glyph the walk places on `page`, from the read-back document.
    ///
    /// # Errors
    ///
    /// Whatever walking refused. **A refusal here is a rejected output**, not a refused input:
    /// a document burrow wrote and cannot walk is one it should not hand back.
    fn glyphs_on(&self, read: &Self::Read, page: usize) -> Result<Vec<Glyph>>;

    /// For each font on `page`, its packed identity and the codes it still **maps** — the union
    /// of its `/ToUnicode` domain and its `/Differences` names.
    ///
    /// # Errors
    ///
    /// Whatever reading the font refused.
    fn mapped_codes(&self, read: &Self::Read, page: usize) -> Result<BTreeMap<u64, BTreeSet<u32>>>;

    /// For each font on `page`, its packed identity and the codes the page still **draws** with
    /// it.
    ///
    /// # Errors
    ///
    /// Whatever walking refused.
    fn drawn_codes(&self, read: &Self::Read, page: usize) -> Result<BTreeMap<u64, BTreeSet<u32>>>;

    /// The top-level keys of `page`'s dictionary.
    ///
    /// # Errors
    ///
    /// Whatever reading the page refused.
    fn page_keys(&self, read: &Self::Read, page: usize) -> Result<Vec<Vec<u8>>>;
}

/// Read `bytes` back and refuse them if the region is not cleared.
///
/// # Errors
///
/// [`Error::OutputRejected`] naming which of the three checks failed and the page it failed on.
/// **Never the file's content**: the messages carry counts, a page number and a rule name.
///
/// Whatever the engine reported, wrapped the same way — a document burrow just wrote and cannot
/// read back is a rejected output rather than a malformed input, and calling it `Malformed`
/// would blame the user's file. The same reasoning `burrow_ops::verify::output` gives.
pub fn region_is_cleared<W: ClearedWitness>(
    witness: &W,
    bytes: &[u8],
    expected: &Cleared,
) -> Result<()> {
    // A FRESH ENGINE. The handle that produced the bytes holds a page tree it built and then
    // edited, and an engine in a bad state agrees with itself.
    let fresh = witness.fresh();
    let read = fresh
        .open_output(bytes)
        .map_err(|error| wrapped("it cannot be read back", error))?;

    // 1. NOTHING THE REGION REACHES IS STILL DRAWN.
    let glyphs = fresh.glyphs_on(&read, expected.page).map_err(|error| {
        wrapped(
            &format!("its page {} cannot be walked", expected.page),
            error,
        )
    })?;
    // THE FRAME COMES FROM THE OUTPUT. See `ClearedWitness::frame_of`.
    let frame = fresh
        .frame_of(&read, expected.page)
        .map_err(|error| wrapped("its page frame cannot be read", error))?;
    let frame_region = expected
        .region
        .to_content_space(&frame)
        .map_err(|error| rejected(format!("its region cannot be converted: {error}")))?;
    let remaining = glyphs
        .iter()
        .filter(|glyph| glyph.conservative_box().intersects(&frame_region))
        .count();
    if remaining > 0 {
        return Err(rejected(format!(
            "{remaining} glyph(s) inside the region are still drawn on page {}",
            expected.page
        )));
    }

    // 2. NO MAPPING SURVIVES FOR A CODE THE OUTPUT NO LONGER DRAWS.
    let mapped = fresh
        .mapped_codes(&read, expected.page)
        .map_err(|error| wrapped("its fonts cannot be read", error))?;
    let drawn = fresh
        .drawn_codes(&read, expected.page)
        .map_err(|error| wrapped("its drawn codes cannot be read", error))?;
    for (font, codes) in &mapped {
        // RETAINED FONTS ARE EXEMPT, and `Cleared::cut_fonts` says why at length: a font left
        // intact for a page outside this operation still maps everything it ever mapped, and
        // §7 discloses that rather than claiming otherwise.
        if !expected.cut_fonts.contains(font) {
            continue;
        }
        let still_drawn = drawn.get(font).cloned().unwrap_or_default();
        let orphaned = codes.difference(&still_drawn).count();
        if orphaned > 0 {
            return Err(rejected(format!(
                "a font on page {} still maps {orphaned} code(s) the page no longer draws",
                expected.page
            )));
        }
    }

    // 3. NO PAGE KEY OUTSIDE §2'S ALLOWLIST.
    let keys = fresh
        .page_keys(&read, expected.page)
        .map_err(|error| wrapped("its page keys cannot be read", error))?;
    let carriers = crate::prune::page_keys_outside_the_allowlist_of(&keys);
    if !carriers.is_empty() {
        return Err(rejected(format!(
            "page {} still carries {} key(s) outside the allowlist",
            expected.page,
            carriers.len()
        )));
    }

    Ok(())
}

/// The one shape of rejection this module produces.
fn rejected(what: String) -> Error {
    Error::OutputRejected(format!("redact: the region is not cleared -- {what}"))
}

/// Wrap an engine error as a rejection — **unless it is a limit or an internal fault**.
///
/// # Running out of time is the operation's outcome, not a verdict on the document
///
/// `burrow_ops::verify::rejected` has this guard and says why: wrapping a `LimitExceeded`
/// produced *"burrow produced a document whose pages cannot be read (LimitExceeded …)"*, which
/// tells a person their file is broken when the truth is that the work did not fit in
/// `max_duration_ms`. This module was written without the guard, and its wrapping is worse:
/// **"the region is not cleared"** over a redaction that ran out of time.
///
/// `Internal` passes through for the same reason in the other direction — a burrow invariant
/// failing is not a statement about the region either.
fn wrapped(what: &str, error: Error) -> Error {
    if matches!(error, Error::LimitExceeded { .. } | Error::Internal(_)) {
        return error;
    }
    rejected(format!("{what}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::{Cleared, ClearedWitness, region_is_cleared};
    use crate::pdfsyntax::geometry::{Glyph, GlyphSource, Matrix, Rect};
    use crate::pdfsyntax::region::{PageFrame, Region};
    use std::cell::RefCell;
    use std::collections::{BTreeMap, BTreeSet};
    use std::rc::Rc;

    /// A witness that answers whatever it is told to, and records what it was asked.
    ///
    /// **The instrument that lies on the way back.** The definition of done requires one: a
    /// verification that cannot fail is not a verification, and the only way to make a real
    /// engine emit a document that should be rejected is to break the operation that wrote it.
    /// This breaks the reading instead, which tests the same thing about the check.
    #[derive(Default)]
    struct Liar {
        glyphs: Vec<Glyph>,
        mapped: BTreeMap<u64, BTreeSet<u32>>,
        drawn: BTreeMap<u64, BTreeSet<u32>>,
        keys: Vec<Vec<u8>>,
        /// How many times `fresh` was called, and which page each read asked about.
        ///
        /// **Shared with every copy `fresh` makes**, because the reads happen on the copy and
        /// the assertions are made on the original. The first version recorded onto the copy
        /// and the original saw nothing — a fake that cannot observe what it was asked is a
        /// fake that asserts nothing.
        freshened: Rc<RefCell<usize>>,
        frame_pages: Rc<RefCell<Vec<usize>>>,
        /// Every page index every read was asked about, in call order.
        asked: Rc<RefCell<Vec<(&'static str, usize)>>>,
    }

    impl ClearedWitness for Liar {
        type Read = ();

        fn fresh(&self) -> Self {
            *self.freshened.borrow_mut() += 1;
            Self {
                glyphs: self.glyphs.clone(),
                mapped: self.mapped.clone(),
                drawn: self.drawn.clone(),
                keys: self.keys.clone(),
                freshened: Rc::clone(&self.freshened),
                frame_pages: Rc::clone(&self.frame_pages),
                asked: Rc::clone(&self.asked),
            }
        }

        fn open_output(&self, _bytes: &[u8]) -> crate::Result<Self::Read> {
            Ok(())
        }

        fn frame_of(&self, _read: &Self::Read, page: usize) -> crate::Result<PageFrame> {
            self.frame_pages.borrow_mut().push(page);
            Ok(PageFrame {
                display_box: Rect {
                    left: 0.0,
                    bottom: 0.0,
                    right: 612.0,
                    top: 792.0,
                },
                rotate: 0,
                user_unit: 1.0,
            })
        }

        fn glyphs_on(&self, _read: &Self::Read, page: usize) -> crate::Result<Vec<Glyph>> {
            self.asked.borrow_mut().push(("glyphs_on", page));
            Ok(self.glyphs.clone())
        }

        fn mapped_codes(
            &self,
            _read: &Self::Read,
            page: usize,
        ) -> crate::Result<BTreeMap<u64, BTreeSet<u32>>> {
            self.asked.borrow_mut().push(("mapped_codes", page));
            Ok(self.mapped.clone())
        }

        fn drawn_codes(
            &self,
            _read: &Self::Read,
            page: usize,
        ) -> crate::Result<BTreeMap<u64, BTreeSet<u32>>> {
            self.asked.borrow_mut().push(("drawn_codes", page));
            Ok(self.drawn.clone())
        }

        fn page_keys(&self, _read: &Self::Read, page: usize) -> crate::Result<Vec<Vec<u8>>> {
            self.asked.borrow_mut().push(("page_keys", page));
            Ok(self.keys.clone())
        }
    }

    /// A glyph sitting at `(x, y)` in content space, one point square.
    fn glyph_at(x: f64, y: f64) -> Glyph {
        Glyph {
            origin: (x, y),
            to_page: Matrix::translate(x, y),
            text_to_page: Matrix::translate(x, y),
            advance: 1.0,
            font_bbox: None,
            font_size: 1.0,
            scaled_font_size: 1.0,
            displacement: 1.0,
            source: GlyphSource {
                font: b"F1".to_vec(),
                code: 65,
                form: None,
                operation: (0, 1),
                operand: 0,
                code_index: 0,
                bytes_per_code: 1,
            },
        }
    }

    /// The band `[40, 540] x [632, 752]` in content space, which `glyph_at(100, 700)` is inside
    /// and `glyph_at(100, 100)` is not.
    fn band() -> Region {
        Region {
            left: 40.0,
            top: 40.0,
            width: 500.0,
            height: 120.0,
        }
    }

    fn expect(page: usize, cut_fonts: BTreeSet<u64>) -> Cleared {
        Cleared {
            page,
            region: band(),
            cut_fonts,
        }
    }

    #[test]
    fn a_glyph_left_inside_the_region_is_rejected() {
        let liar = Liar {
            glyphs: vec![glyph_at(100.0, 700.0)],
            ..Liar::default()
        };
        let error = region_is_cleared(&liar, b"%PDF", &expect(0, BTreeSet::new()))
            .expect_err("a glyph inside the region is still drawn");
        let text = format!("{error}");
        assert!(text.contains("still drawn"), "got: {text}");
        assert!(text.contains("region is not cleared"), "got: {text}");
    }

    #[test]
    fn a_glyph_outside_the_region_is_accepted() {
        // THE NEAR-MISS. Without it the test above passes for a check that rejects everything,
        // which would refuse every redaction there is.
        let liar = Liar {
            glyphs: vec![glyph_at(100.0, 100.0)],
            ..Liar::default()
        };
        region_is_cleared(&liar, b"%PDF", &expect(0, BTreeSet::new()))
            .expect("a glyph well below the band is not in it");
    }

    #[test]
    fn a_mapping_left_for_a_code_a_cut_font_no_longer_draws_is_rejected() {
        let liar = Liar {
            mapped: [(7, [65u32, 66].into_iter().collect())]
                .into_iter()
                .collect(),
            drawn: [(7, [65u32].into_iter().collect())].into_iter().collect(),
            ..Liar::default()
        };
        let error = region_is_cleared(&liar, b"%PDF", &expect(0, [7].into_iter().collect()))
            .expect_err("code 66 is mapped and not drawn");
        assert!(format!("{error}").contains("no longer draws"), "{error}");
    }

    #[test]
    fn the_same_mapping_on_a_retained_font_is_accepted() {
        // THE EXEMPTION, and it is not a weakening: a font left intact for a page outside the
        // operation still maps everything it ever mapped, which is what ADR 0029 §7 discloses.
        // Without this test, `cut_fonts` could be ignored entirely and nothing would fail.
        let liar = Liar {
            mapped: [(7, [65u32, 66].into_iter().collect())]
                .into_iter()
                .collect(),
            drawn: [(7, [65u32].into_iter().collect())].into_iter().collect(),
            ..Liar::default()
        };
        region_is_cleared(&liar, b"%PDF", &expect(0, BTreeSet::new()))
            .expect("font 7 was retained, so its mappings are disclosed rather than checked");
    }

    #[test]
    fn a_page_key_outside_the_allowlist_is_rejected() {
        let liar = Liar {
            keys: vec![b"Contents".to_vec(), b"PieceInfo".to_vec()],
            ..Liar::default()
        };
        let error = region_is_cleared(&liar, b"%PDF", &expect(0, BTreeSet::new()))
            .expect_err("/PieceInfo is not on the allowlist");
        assert!(
            format!("{error}").contains("outside the allowlist"),
            "{error}"
        );
    }

    #[test]
    fn the_allowlisted_keys_are_accepted() {
        // The near-miss for the check above: `/Contents` and `/MediaBox` are on the list, and a
        // check that rejected them would reject every page.
        let liar = Liar {
            keys: vec![b"Contents".to_vec(), b"MediaBox".to_vec(), b"Type".to_vec()],
            ..Liar::default()
        };
        region_is_cleared(&liar, b"%PDF", &expect(0, BTreeSet::new())).expect("all allowlisted");
    }

    #[test]
    fn the_read_back_goes_through_a_fresh_engine() {
        // The handle that wrote the bytes holds a page tree it built and then edited, and an
        // engine in a bad state agrees with itself. `OutputReader::fresh` exists for this and
        // so does `ClearedWitness::fresh`; nothing failed when the call was removed.
        let liar = Liar::default();
        region_is_cleared(&liar, b"%PDF", &expect(0, BTreeSet::new())).expect("clean");
        assert_eq!(
            *liar.freshened.borrow(),
            1,
            "the check must open the output through a fresh witness"
        );
    }

    #[test]
    fn every_read_is_made_for_the_page_being_verified() {
        // ONE TEST PINNED `frame_of`'S PAGE AND FOUR METHODS WENT UNPINNED. A security review
        // planted `Cleared { page: 0 }` and nothing failed, because every end-to-end test
        // redacts page 0 and the `Liar` ignored the argument everywhere else. A verification
        // that checked the wrong page would have shipped.
        let liar = Liar::default();
        region_is_cleared(&liar, b"%PDF", &expect(3, BTreeSet::new())).expect("clean");
        let asked = liar.asked.borrow().clone();
        assert!(!asked.is_empty(), "the check must read something");
        for (method, page) in &asked {
            assert_eq!(*page, 3, "{method} was asked about page {page}, not page 3");
        }
        // And every read the check makes is represented, so a method dropped from the check
        // shows up here rather than silently not being asked.
        for wanted in ["glyphs_on", "mapped_codes", "drawn_codes", "page_keys"] {
            assert!(
                asked.iter().any(|(method, _)| *method == wanted),
                "{wanted} was never called: {asked:?}"
            );
        }
    }

    #[test]
    fn a_glyph_whose_box_reaches_the_region_is_rejected_even_when_its_origin_does_not() {
        // KILLS: testing the ORIGIN as a point instead of the conservative box. The box is the
        // advance box unioned with the scaled `/FontBBox`, and a glyph can sit outside the
        // region while its ink reaches in -- which is the whole reason the box exists. The
        // other fixtures put the origin at the box corner, so both spellings pass on them.
        let mut glyph = glyph_at(100.0, 620.0);
        // A box that extends 100 pt upward from an origin below the band.
        glyph.font_bbox = Some(Rect {
            left: 0.0,
            bottom: 0.0,
            right: 1.0,
            top: 100.0,
        });
        glyph.to_page = Matrix::translate(100.0, 620.0);
        let liar = Liar {
            glyphs: vec![glyph],
            ..Liar::default()
        };
        let error = region_is_cleared(&liar, b"%PDF", &expect(0, BTreeSet::new()))
            .expect_err("the glyph's box reaches into the band even though its origin does not");
        assert!(format!("{error}").contains("still drawn"), "{error}");
    }

    #[test]
    fn the_frame_is_read_for_the_page_being_verified() {
        // The region is converted with the OUTPUT's frame, so the frame has to be the one on
        // the page that was redacted. Asking for a different page would convert the region with
        // numbers from a page nobody edited.
        let liar = Liar::default();
        region_is_cleared(&liar, b"%PDF", &expect(3, BTreeSet::new())).expect("clean");
        assert_eq!(
            *liar.frame_pages.borrow(),
            vec![3],
            "the frame must be read for the page under verification"
        );
    }
}

#[cfg(test)]
mod limit_tests {
    use super::{Cleared, ClearedWitness, region_is_cleared};
    use crate::pdfsyntax::geometry::Glyph;
    use crate::pdfsyntax::region::{PageFrame, Region};
    use burrow_types::{Error, Result};
    use std::collections::{BTreeMap, BTreeSet};

    /// A witness whose walk runs out of time.
    struct OutOfTime;

    impl ClearedWitness for OutOfTime {
        type Read = ();

        fn fresh(&self) -> Self {
            Self
        }

        fn open_output(&self, _bytes: &[u8]) -> Result<Self::Read> {
            Ok(())
        }

        fn frame_of(&self, _read: &Self::Read, _page: usize) -> Result<PageFrame> {
            Ok(PageFrame {
                display_box: crate::pdfsyntax::geometry::Rect {
                    left: 0.0,
                    bottom: 0.0,
                    right: 612.0,
                    top: 792.0,
                },
                rotate: 0,
                user_unit: 1.0,
            })
        }

        fn glyphs_on(&self, _read: &Self::Read, _page: usize) -> Result<Vec<Glyph>> {
            Err(Error::LimitExceeded {
                limit: "max_duration_ms",
                stage: burrow_types::Stage::Deadline,
                requested: 61_000,
                allowed: 60_000,
            })
        }

        fn mapped_codes(
            &self,
            _read: &Self::Read,
            _page: usize,
        ) -> Result<BTreeMap<u64, BTreeSet<u32>>> {
            Ok(BTreeMap::new())
        }

        fn drawn_codes(
            &self,
            _read: &Self::Read,
            _page: usize,
        ) -> Result<BTreeMap<u64, BTreeSet<u32>>> {
            Ok(BTreeMap::new())
        }

        fn page_keys(&self, _read: &Self::Read, _page: usize) -> Result<Vec<Vec<u8>>> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn a_limit_from_the_read_back_passes_through_rather_than_becoming_a_rejection() {
        // Running out of time is the operation's outcome, not a verdict on the document. Without
        // the guard this comes back as "the region is not cleared", which tells a person their
        // redaction failed when what happened is that the work did not fit in the budget.
        let expected = Cleared {
            page: 0,
            region: Region {
                left: 0.0,
                top: 0.0,
                width: 10.0,
                height: 10.0,
            },
            cut_fonts: BTreeSet::new(),
        };
        let error = region_is_cleared(&OutOfTime, b"%PDF", &expected).expect_err("the limit");
        assert!(
            matches!(error, Error::LimitExceeded { .. }),
            "a limit must reach the caller as a limit, got: {error:?}"
        );
    }
}
